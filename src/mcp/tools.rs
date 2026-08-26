//! MCP tool handler implementations.
//!
//! Each function here corresponds to one MCP tool exposed by the BeliefBase server.
//! Handlers receive a reference to the loaded [`McpState`] and typed input structs
//! defined in [`super::types`].
//!
//! ## Dispatch pattern
//!
//! All tool handlers are `async`. Those that need `BeliefSource` methods call
//! `state.source_ref()` to get a `&dyn BeliefSource` and invoke trait methods
//! directly. This avoids `#[cfg]` match arms in tool handlers.
//!
//! ## Tool index
//!
//! | Tool                | Handler fn               | Status       |
//! |---------------------|--------------------------|--------------|
//! | `get_networks`      | [`get_networks`]         | implemented  |
//! | `search`            | [`search`]               | implemented  |
//! | `get_context`       | [`get_context`]          | implemented  |
//! | `get_submap`        | [`get_submap`]           | implemented  |
//! | `query`             | [`query`]                | implemented  |
//! | `check_consistency` | [`check_consistency`]    | implemented  |
//! | `bref`              | [`bref`]                 | implemented  |
//! | `get_traceability`  | [`get_traceability`]     | implemented  |
//! | `get_maps_to`       | [`get_maps_to`]          | implemented  |
//! | `get_maps_to_traceability` | [`get_maps_to_traceability`] | implemented |

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use petgraph::visit::{EdgeRef, IntoEdgeReferences};
use rmcp::ErrorData as McpError;

use crate::beliefbase::BeliefBase;
use crate::codec::belief_ir::toml_value_to_json;
use crate::mcp::state::McpState;
use crate::mcp::types::{
    BrefInput, BrefOutput, CheckConsistencyInput, ConsistencyOutput, EdgeEntry, GetContextInput,
    GetMapsToInput, GetMapsToTraceabilityInput, GetNetworksInput, GetSubmapInput,
    GetTraceabilityInput, MapsToClaim, MapsToOutput, MapsToOwner, MapsToSinkGroup, MapsToSource,
    MapsToTraceabilityOutput, NetworkEntry, NetworksOutput, NodeContextOutput, NodeSummary,
    OrphanedEdge, OwnedEdgeEntry, QueryInput, QueryOutput, RelatedNodeEntry, SearchHit,
    SearchInput, SubmapEdge, SubmapNode, SubmapOutput, TraceabilityOutput, TraceabilityRow,
    UnresolvedRef,
};
use crate::properties::{Bid, WeightKind};
use crate::query::{BeliefSource, QueryPackage, QuerySpec, TapeFn};
use crate::shard::manifest::SearchManifest;
use crate::shard::search::{query_search_index, SearchIndex};

// ── Orientation inline note (3-5 lines, always in get_networks response) ──────

const ORIENTATION_NOTE: &str = "\
BIDs are full UUIDs identifying nodes; brefs are 5-char hex aliases (e.g. \"a3f12\"). \
Edges flow source→sink where source=child (more specific) and sink=parent (more general). \
Start with search or get_context to find a node, then follow its sources/sinks. \
For gap analysis use owned_edges on ext-gap nodes to find maps_to claims. \
check_consistency surfaces broken refs and orphaned edges across all loaded networks.";

// ── get_networks ──────────────────────────────────────────────────────────────

/// Return all compiled networks with basic stats and an inline orientation note.
///
/// This is the recommended first call for any agent session. The `orientation`
/// field provides inline guidance regardless of whether the MCP client injected
/// the `noet://help/orientation` resource.
pub fn get_networks(
    state: &McpState,
    _input: GetNetworksInput,
) -> Result<NetworksOutput, McpError> {
    let networks: Vec<NetworkEntry> = state
        .manifest
        .networks
        .iter()
        .map(|meta| NetworkEntry {
            bref: meta.bref.clone(),
            bid: meta.bid.clone(),
            title: meta.title.clone(),
            node_count: meta.node_count,
            relation_count: meta.relation_count,
            path: meta.path.clone(),
        })
        .collect();

    Ok(NetworksOutput {
        networks,
        orientation: ORIENTATION_NOTE.to_string(),
    })
}

// ── search ────────────────────────────────────────────────────────────────────

/// Full-text TF-IDF search across all loaded search indices.
///
/// Loads `.idx.msgpack` files from the output directory referenced in `state`,
/// then calls `query_search_index` with TF-IDF + Levenshtein fuzzy matching.
pub fn search(state: &McpState, input: SearchInput) -> Result<Vec<SearchHit>, McpError> {
    tracing::debug!(query = %input.query, limit = input.limit, "search called");

    let Some(ref output_dir) = state.output_dir else {
        return Err(McpError::invalid_params(
            "search requires --output-dir (live mode search not yet implemented)".to_string(),
            None,
        ));
    };

    let search_dir = output_dir.join("search");
    if !search_dir.exists() {
        return Ok(vec![]);
    }

    // Collect the bref list to load. Prefer state.manifest.networks (populated in sharded
    // mode). In monolithic mode the manifest has no networks, so fall back to
    // search/manifest.json which is always written regardless of sharding.
    let brefs_from_manifest: Vec<String> = if !state.manifest.networks.is_empty() {
        state
            .manifest
            .networks
            .iter()
            .map(|m| m.bref.clone())
            .collect()
    } else {
        load_search_manifest_brefs(&search_dir)
    };

    // Load search indices. Filter by network bref if requested.
    let mut indices: Vec<SearchIndex> = Vec::new();
    for bref in &brefs_from_manifest {
        if let Some(ref net_filter) = input.network {
            if bref != net_filter {
                continue;
            }
        }
        let idx_path = search_dir.join(format!("{}.idx.msgpack", bref));
        match load_search_index(&idx_path) {
            Ok(idx) => indices.push(idx),
            Err(e) => {
                tracing::warn!(path = %idx_path.display(), error = %e, "failed to load search index — skipping");
            }
        }
    }

    if indices.is_empty() {
        return Ok(vec![]);
    }

    let index_refs: Vec<&SearchIndex> = indices.iter().collect();
    let raw_results = query_search_index(&index_refs, &input.query, input.limit);

    let hits = raw_results
        .into_iter()
        .map(|r| SearchHit {
            bid: r.bid,
            title: r.title,
            snippet: String::new(), // snippet extraction deferred (requires loaded shard text)
            score: r.score as f32,
            network: r.network_bref,
            path: r.path,
        })
        .collect();

    Ok(hits)
}

/// Deserialize a `.idx.msgpack` file into a [`SearchIndex`].
fn load_search_index(path: &Path) -> Result<SearchIndex, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("read error: {e}"))?;
    rmp_serde::from_slice(&bytes).map_err(|e| format!("msgpack decode error: {e}"))
}

/// Read bref strings from `search/manifest.json`.
///
/// Used as a fallback in monolithic mode (when `state.manifest.networks` is empty)
/// so that search still works even though no `ShardManifest` was produced.
/// `search/manifest.json` (`SearchManifest`) is always written by `finalize_html`
/// regardless of whether the data export is sharded or monolithic.
fn load_search_manifest_brefs(search_dir: &Path) -> Vec<String> {
    let manifest_path = search_dir.join("manifest.json");
    let bytes = match std::fs::read(&manifest_path) {
        Ok(b) => b,
        Err(e) => {
            tracing::debug!(
                path = %manifest_path.display(),
                error = %e,
                "search/manifest.json not found — search will return empty results"
            );
            return vec![];
        }
    };
    match serde_json::from_slice::<SearchManifest>(&bytes) {
        Ok(sm) => sm.networks.into_iter().map(|n| n.bref).collect(),
        Err(e) => {
            tracing::warn!(
                path = %manifest_path.display(),
                error = %e,
                "failed to parse search/manifest.json"
            );
            vec![]
        }
    }
}

// ── get_context ─────────────────────────────────────────────────────────────────

/// Return a node and its full relationship context.
///
/// Evaluates a balanced query for the requested BID via `evaluate`. The `sko`
/// halo discovers all neighbors (including owned-edge endpoints), and section
/// ancestry provides path context. The materialized graph is converted to a local
/// `BeliefBase` from which `BeliefContext` provides sources, sinks, and owned edges.
pub async fn get_context(
    state: &McpState,
    input: GetContextInput,
) -> Result<NodeContextOutput, McpError> {
    let bid = parse_bid(&input.bid)?;
    let src = state.source_ref();

    // Evaluate balanced query for this BID — halo + ancestry.
    let mut package = QueryPackage::balanced(QuerySpec::seed(TapeFn::Bids(vec![bid])));
    src.evaluate(&mut package)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;
    let graph = package.into_graph();
    if graph.states.is_empty() {
        return Err(McpError::invalid_params(
            format!("BID {bid} not found in loaded BeliefBase"),
            None,
        ));
    }

    // Reconstruct a local BeliefBase with PathMapMap + owner_edges memo.
    let local_bb = BeliefBase::from(graph);

    // Discover the home network via the reconstructed PathMapMap.
    let root_net = local_bb
        .paths()
        .indexed_path(&bid)
        .map(|(net, _, _)| net)
        .ok_or_else(|| {
            McpError::internal_error(
                format!("No path found for {bid} in reconstructed PathMapMap"),
                None,
            )
        })?;

    let ctx = local_bb.get_context(&root_net, &bid).ok_or_else(|| {
        McpError::internal_error(
            format!("get_context returned None for {bid} on local BeliefBase"),
            None,
        )
    })?;

    // Serialize node summary.
    let kind = ctx
        .node
        .payload
        .get("kind")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let node_summary = NodeSummary {
        bid: ctx.node.bid.to_string(),
        bref: ctx.node.bid.bref().to_string(),
        title: ctx.node.title.clone(),
        kind,
    };

    // Metadata: TOML table → serde_json map.
    let metadata = toml_table_to_json_map(&ctx.node.metadata);

    // Owned edges (endpoint + declared perspectives, deduplicated).
    let all_owned = ctx.all_owned_edges();
    let owned_edges: Vec<OwnedEdgeEntry> = all_owned
        .iter()
        .map(|oe| OwnedEdgeEntry {
            owner_bid: oe.owner_bid.to_string(),
            source_bid: oe.source_bid.to_string(),
            sink_bid: oe.sink_bid.to_string(),
            weight_kind: weight_kind_str(oe.weight_kind),
        })
        .collect();

    // Build owned-edge lookup for annotating individual edges below.
    let owned_index: HashMap<(Bid, Bid, WeightKind), Bid> = all_owned
        .iter()
        .map(|oe| ((oe.source_bid, oe.sink_bid, oe.weight_kind), oe.owner_bid))
        .collect();

    // Collect related nodes and edges from sources/sinks.
    let mut related_map: HashMap<Bid, RelatedNodeEntry> = HashMap::new();
    let mut edges: Vec<EdgeEntry> = Vec::new();

    for rel in ctx.sources() {
        let entry = related_map
            .entry(rel.other.bid)
            .or_insert_with(|| RelatedNodeEntry {
                direction: "source".to_string(),
                bid: rel.other.bid.to_string(),
                bref: rel.other.bid.bref().to_string(),
                title: rel.other.title.clone(),
                home_net: rel.home_net.to_string(),
                root_path: rel.root_path.clone(),
                weight_kinds: Vec::new(),
                link_title: rel.link_title.clone(),
            });
        for kind in rel.weight.weights.keys() {
            let kind_str = weight_kind_str(*kind);
            if !entry.weight_kinds.contains(&kind_str) {
                entry.weight_kinds.push(kind_str.clone());
            }
            let owner = owned_index.get(&(rel.other.bid, bid, *kind)).copied();
            edges.push(EdgeEntry {
                source_bid: rel.other.bid.to_string(),
                sink_bid: bid.to_string(),
                weight_kind: kind_str,
                owned_by: owner.map(|b| b.bref().to_string()),
            });
        }
    }

    for rel in ctx.sinks() {
        let entry = related_map
            .entry(rel.other.bid)
            .or_insert_with(|| RelatedNodeEntry {
                direction: "sink".to_string(),
                bid: rel.other.bid.to_string(),
                bref: rel.other.bid.bref().to_string(),
                title: rel.other.title.clone(),
                home_net: rel.home_net.to_string(),
                root_path: rel.root_path.clone(),
                weight_kinds: Vec::new(),
                link_title: rel.link_title.clone(),
            });
        for kind in rel.weight.weights.keys() {
            let kind_str = weight_kind_str(*kind);
            if !entry.weight_kinds.contains(&kind_str) {
                entry.weight_kinds.push(kind_str.clone());
            }
            let owner = owned_index.get(&(bid, rel.other.bid, *kind)).copied();
            edges.push(EdgeEntry {
                source_bid: bid.to_string(),
                sink_bid: rel.other.bid.to_string(),
                weight_kind: kind_str,
                owned_by: owner.map(|b| b.bref().to_string()),
            });
        }
    }

    let related_nodes: Vec<RelatedNodeEntry> = related_map.into_values().collect();

    let content = if input.include_content {
        ctx.node
            .payload
            .get("text")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    } else {
        None
    };

    Ok(NodeContextOutput {
        node: node_summary,
        home_net: ctx.home_net.to_string(),
        root_path: ctx.root_path.clone(),
        metadata,
        related_nodes,
        edges,
        owned_edges,
        content,
    })
}

// ── get_submap ────────────────────────────────────────────────────────────────

/// Return a pragmatic-edge traceability subgraph rooted at the given BID.
///
/// Uses `BeliefSource::submap_by_bid` in both static and live mode. In static mode
/// the in-memory `PathMapMap` is used; in live mode `DbConnection::submap_by_bid`
/// queries the DB paths table.
///
/// The home network BID is derived by evaluating a balanced query for the
/// entry BID, reconstructing a local `BeliefBase` from the result, and calling
/// `indexed_path`. This works identically against `BeliefBase` and `DbConnection`.
pub async fn get_submap(state: &McpState, input: GetSubmapInput) -> Result<SubmapOutput, McpError> {
    tracing::debug!(bid = %input.bid, depth = input.depth, direction = %input.direction, "get_submap called");

    let bid = parse_bid(&input.bid)?;
    let src = state.source_ref();

    // Derive the home network BID by fetching the node's balanced halo and
    // reconstructing a local BeliefBase with a fresh PathMapMap.
    let mut package = QueryPackage::balanced(QuerySpec::seed(TapeFn::Bids(vec![bid])));
    src.evaluate(&mut package)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;
    let bg = package.into_graph();
    if bg.states.is_empty() {
        return Err(McpError::invalid_params(
            format!("BID {bid} not found in loaded BeliefBase"),
            None,
        ));
    }
    let local_bb = BeliefBase::from(bg);
    let network_bid = local_bb
        .paths()
        .indexed_path(&bid)
        .map(|(net, _, _)| net)
        .ok_or_else(|| {
            McpError::invalid_params(
                format!("BID {bid} found but path not resolvable in reconstructed PathMapMap"),
                None,
            )
        })?;

    // Fetch the submap entries via BeliefSource::submap_by_bid.
    let entries = src
        .submap_by_bid(network_bid, Some(bid), input.depth, true)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

    let submap_bids: Vec<Bid> = entries.iter().map(|(_, b, _)| *b).collect();
    let node_bids_set: HashSet<Bid> = submap_bids.iter().copied().collect();

    // Fetch titles via a targeted balanced query on the submap BID set.
    let mut package = QueryPackage::balanced(QuerySpec::seed(TapeFn::Bids(submap_bids.clone())));
    src.evaluate(&mut package)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;
    let title_bg = package.into_graph();
    let titles: HashMap<Bid, String> = title_bg
        .states
        .into_iter()
        .map(|(b, node)| (b, node.title))
        .collect();

    let nodes: Vec<SubmapNode> = entries
        .iter()
        .map(|(path, node_bid, order)| SubmapNode {
            bid: node_bid.to_string(),
            bref: node_bid.bref().to_string(),
            title: titles.get(node_bid).cloned().unwrap_or_default(),
            path: path.clone(),
            depth: order.len().saturating_sub(1),
        })
        .collect();

    // Build edges: fetch the relation subgraph for the submap BID set.
    let mut edges: Vec<SubmapEdge> = {
        let bg = crate::query::lookup_edges(src, &submap_bids)
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        let graph = bg.relations.as_graph();
        let mut ev = Vec::new();
        for edge_ref in graph.edge_references() {
            let src = graph[edge_ref.source()];
            let snk = graph[edge_ref.target()];
            if node_bids_set.contains(&src) && node_bids_set.contains(&snk) {
                for kind in edge_ref.weight().weights.keys() {
                    ev.push(SubmapEdge {
                        source_bid: src.to_string(),
                        sink_bid: snk.to_string(),
                        weight_kind: weight_kind_str(*kind),
                    });
                }
            }
        }
        ev
    };

    edges.sort_unstable_by(|a, b| {
        a.source_bid
            .cmp(&b.source_bid)
            .then(a.sink_bid.cmp(&b.sink_bid))
            .then(a.weight_kind.cmp(&b.weight_kind))
    });
    edges.dedup_by(|a, b| {
        a.source_bid == b.source_bid && a.sink_bid == b.sink_bid && a.weight_kind == b.weight_kind
    });

    Ok(SubmapOutput { nodes, edges })
}

// ── query ───────────────────────────────────────────────────────────────────────────

/// Execute a query against the loaded BeliefBase or live DB.
///
/// Parses `input.query_string` using the textual query grammar and evaluates it via
/// [`QueryPackage`]. Returns matching node states and the edges among them.
pub async fn query(state: &McpState, input: QueryInput) -> Result<QueryOutput, McpError> {
    tracing::debug!(query = %input.query_string, "query called");

    let spec = crate::query::parser::parse(&input.query_string).map_err(|e| {
        McpError::invalid_params(
            format!(
                "Query parse error: {e}. \
                 See docs/design/query_model.md \u{00a7}9.5 for the textual grammar. \
                 Examples: \"id://net composed_of(*)\", \"title:auth AND schema:procedure\"."
            ),
            None,
        )
    })?;

    let mut package = QueryPackage::new(spec);
    state
        .source_ref()
        .evaluate(&mut package)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

    let bg = package.into_graph();

    // Serialize states as BID→BeliefNode JSON.
    let states: BTreeMap<String, serde_json::Value> = bg
        .states
        .iter()
        .map(|(bid, node)| {
            let v = serde_json::to_value(node).unwrap_or(serde_json::Value::Null);
            (bid.to_string(), v)
        })
        .collect();

    // Collect edges among the result set.
    let result_bids: HashSet<Bid> = bg.states.keys().copied().collect();
    let mut edges: Vec<EdgeEntry> = Vec::new();
    {
        let graph = bg.relations.as_graph();
        for edge_ref in graph.edge_references() {
            let src = graph[edge_ref.source()];
            let snk = graph[edge_ref.target()];
            if result_bids.contains(&src) && result_bids.contains(&snk) {
                for kind in edge_ref.weight().weights.keys() {
                    edges.push(EdgeEntry {
                        source_bid: src.to_string(),
                        sink_bid: snk.to_string(),
                        weight_kind: weight_kind_str(*kind),
                        owned_by: None,
                    });
                }
            }
        }
    }

    Ok(QueryOutput { states, edges })
}

// ── check_consistency ─────────────────────────────────────────────────────────

/// Surface unresolved cross-references and orphaned edges.
///
/// In both modes, exports the full `BeliefGraph` via `BeliefSource::export_beliefgraph`
/// and walks its edges to classify:
/// - `unresolved_refs`: edges where the source is present but the sink is absent
///   (reference compiled but target never written / not in this graph).
/// - `orphaned_edges`: edges where the source itself is absent (structural inconsistency).
///
/// In live mode the DB is always current, so this reflects the latest compile pass.
/// In static mode it reflects the last `noet parse` run.
///
/// `compiled_at` is read from `NetworkShardMeta.compiled_at` if present (TODO Issue 66).
/// For live mode, Step 6 will additionally source `unresolved_refs` from
/// `DocumentCompiler::last_diagnostics()` for richer diagnostic detail.
pub async fn check_consistency(
    state: &McpState,
    input: CheckConsistencyInput,
) -> Result<ConsistencyOutput, McpError> {
    tracing::debug!(network = ?input.network, "check_consistency called");

    // Export the full graph once, then walk it.
    let bg = state
        .source_ref()
        .export_beliefgraph()
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;
    let states = &bg.states;

    // Determine which network brefs to check.
    let target_brefs: Option<HashSet<&str>> = input.network.as_deref().map(|n| {
        let mut s = HashSet::new();
        s.insert(n);
        s
    });

    let mut unresolved_refs: Vec<UnresolvedRef> = Vec::new();
    let mut orphaned_edges: Vec<OrphanedEdge> = Vec::new();

    {
        let graph = bg.relations.as_graph();

        for edge_ref in graph.edge_references() {
            let src_bid = graph[edge_ref.source()];
            let snk_bid = graph[edge_ref.target()];

            let src_present = states.contains_key(&src_bid);
            let snk_present = states.contains_key(&snk_bid);

            // Filter by network if requested — check if either endpoint belongs to the
            // target network by bref prefix match on the BID.
            if let Some(ref target) = target_brefs {
                let src_bref = src_bid.bref().to_string();
                let snk_bref = snk_bid.bref().to_string();
                if !target.contains(src_bref.as_str()) && !target.contains(snk_bref.as_str()) {
                    continue;
                }
            }

            let reason = match (src_present, snk_present) {
                (true, true) => None,
                (false, true) => Some("source missing"),
                (true, false) => Some("sink missing"),
                (false, false) => Some("both missing"),
            };

            if let Some(reason_str) = reason {
                for kind in edge_ref.weight().weights.keys() {
                    // Classify: if source is present but sink is not, this looks like an
                    // unresolved reference (the author wrote id://something that didn't compile).
                    // If source is absent, it is an orphaned edge (structural inconsistency).
                    if src_present && !snk_present {
                        unresolved_refs.push(UnresolvedRef {
                            path: bg
                                .states
                                .get(&src_bid)
                                .and_then(|n| n.payload.get("path").and_then(|v| v.as_str()))
                                .unwrap_or("")
                                .to_string(),
                            source_bid: Some(src_bid.to_string()),
                            raw_key: snk_bid.to_string(),
                            weight_kind: weight_kind_str(*kind),
                            line: None,
                        });
                    } else {
                        orphaned_edges.push(OrphanedEdge {
                            source_bid: src_bid.to_string(),
                            sink_bid: snk_bid.to_string(),
                            weight_kind: weight_kind_str(*kind),
                            reason: reason_str.to_string(),
                        });
                    }
                }
            }
        }
    }

    // Deduplicate (graph iteration may yield the same edge multiple times for
    // multi-weight edges already split above, but identical entries can accumulate).
    unresolved_refs.dedup_by(|a, b| {
        a.source_bid == b.source_bid && a.raw_key == b.raw_key && a.weight_kind == b.weight_kind
    });
    orphaned_edges.dedup_by(|a, b| {
        a.source_bid == b.source_bid && a.sink_bid == b.sink_bid && a.weight_kind == b.weight_kind
    });

    let compiled_at: Option<String> = None; // TODO(Issue 66): read NetworkShardMeta.compiled_at

    let summary = format!(
        "{} network(s) loaded, {} unresolved ref(s), {} orphaned edge(s).",
        state.manifest.networks.len(),
        unresolved_refs.len(),
        orphaned_edges.len(),
    );

    Ok(ConsistencyOutput {
        unresolved_refs,
        orphaned_edges,
        summary,
        compiled_at,
    })
}

// ── bref ──────────────────────────────────────────────────────────────────────

/// Translate a full BID (UUID) into its 5-character hex bref alias.
///
/// A bref is the short, stable alias derived from a BID. It appears in paths,
/// edge ownership fields, search results, and network identifiers throughout
/// the BeliefBase. This tool is purely a computation — it requires no BeliefBase
/// lookup and works identically in static and live mode.
///
/// Agents should use this when they have a BID from one tool output and need the
/// bref form for display, filtering, or cross-referencing with another tool's
/// `network` parameter.
pub fn bref(_state: &McpState, input: BrefInput) -> Result<BrefOutput, McpError> {
    let bid = parse_bid(&input.bid)?;
    Ok(BrefOutput {
        bid: bid.to_string(),
        bref: bid.bref().to_string(),
    })
}

// ── get_traceability ──────────────────────────────────────────────────────────

/// Return the direct edge-count matrix for the structural submap of a node.
///
/// Rows are the nodes reachable from the entry node's home network via Section
/// edges (same traversal as `get_submap`). Columns are edge counts per WeightKind
/// per direction (in/out) for each row node. This answers: "what is the
/// connectivity of each node in this structural scope?"
pub async fn get_traceability(
    state: &McpState,
    input: GetTraceabilityInput,
) -> Result<TraceabilityOutput, McpError> {
    tracing::debug!(bid = %input.bid, depth = input.depth, "get_traceability called");

    let bid = parse_bid(&input.bid)?;
    let src = state.source_ref();

    // Resolve home network and submap entries via the balanced local BeliefBase.
    let mut package = QueryPackage::balanced(QuerySpec::seed(TapeFn::Bids(vec![bid])));
    src.evaluate(&mut package)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;
    let bg = package.into_graph();
    if bg.states.is_empty() {
        return Err(McpError::invalid_params(
            format!("BID {bid} not found"),
            None,
        ));
    }
    let local_bb = BeliefBase::from(bg);
    let network_bid = local_bb
        .paths()
        .indexed_path(&bid)
        .map(|(net, _, _)| net)
        .ok_or_else(|| McpError::invalid_params(format!("BID {bid} path not resolvable"), None))?;

    // Get the structural submap (Section edges only at the requested depth).
    let entries = src
        .submap_by_bid(network_bid, Some(bid), input.depth, true)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

    let submap_bids: Vec<Bid> = entries.iter().map(|(_, b, _)| *b).collect();

    // Batch-fetch titles for all submap nodes.
    let titles = batch_titles(src, &submap_bids).await?;

    // Batch-fetch edges incident to any submap node.
    let rel_bg = crate::query::lookup_edges(src, &submap_bids)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

    // Determine which weight kinds to count.
    let active_kinds = parse_weight_kinds(&input.weight_kinds);

    // Build a per-BID edge count map.
    let mut counts: HashMap<Bid, [usize; 6]> = HashMap::new(); // [sec_in, sec_out, ep_in, ep_out, pr_in, pr_out]
    {
        let graph = rel_bg.relations.as_graph();
        let submap_set: HashSet<Bid> = submap_bids.iter().copied().collect();
        for edge_ref in graph.edge_references() {
            let src_bid = graph[edge_ref.source()];
            let snk_bid = graph[edge_ref.target()];
            for kind in edge_ref.weight().weights.keys() {
                let kind_str = weight_kind_str(*kind);
                if !active_kinds.contains(&kind_str) {
                    continue;
                }
                let col_base: usize = match kind_str.as_str() {
                    "section" => 0,
                    "epistemic" => 2,
                    "pragmatic" => 4,
                    _ => continue,
                };
                // src_bid is source (child/more-specific), snk_bid is sink (parent).
                // For the sink node: source is coming IN to it.
                if submap_set.contains(&snk_bid) {
                    counts.entry(snk_bid).or_insert([0; 6])[col_base] += 1; // _in
                }
                // For the source node: it goes OUT to sink.
                if submap_set.contains(&src_bid) {
                    counts.entry(src_bid).or_insert([0; 6])[col_base + 1] += 1; // _out
                }
            }
        }
    }

    let rows: Vec<TraceabilityRow> = entries
        .iter()
        .map(|(path, node_bid, _order)| {
            let c = counts.get(node_bid).copied().unwrap_or([0; 6]);
            TraceabilityRow {
                path: path.clone(),
                bid: node_bid.to_string(),
                bref: node_bid.bref().to_string(),
                label: titles.get(node_bid).cloned().unwrap_or_default(),
                section_in: c[0],
                section_out: c[1],
                epistemic_in: c[2],
                epistemic_out: c[3],
                pragmatic_in: c[4],
                pragmatic_out: c[5],
            }
        })
        .collect();

    Ok(TraceabilityOutput {
        entry_bid: bid.to_string(),
        entry_home_net: network_bid.to_string(),
        rows,
    })
}

// ── get_maps_to ───────────────────────────────────────────────────────────────

/// Return the flat list of `{maps_to}` claims for a set of owner BIDs.
///
/// Evaluates a balanced query for the requested owner BIDs to obtain a local
/// `BeliefBase` with the `sko` halo (which discovers owned-edge endpoints).
/// Per-owner `get_context` → `all_owned_edges()` extracts the claims.
///
/// Returns one entry per (owner, source, sink, kind) tuple in owner-input order.
/// Answers: "what does each of these nodes claim to cover?"
pub async fn get_maps_to(
    state: &McpState,
    input: GetMapsToInput,
) -> Result<MapsToOutput, McpError> {
    tracing::debug!(bid_count = input.bids.len(), "get_maps_to called");

    let bids: Vec<Bid> = input
        .bids
        .iter()
        .map(|s| parse_bid(s))
        .collect::<Result<Vec<_>, _>>()?;

    let active_kinds = parse_weight_kinds(&input.weight_kinds);
    let src = state.source_ref();

    // Single balanced evaluation for all owner BIDs. The sko halo
    // discovers owned-edge endpoints as Trace context.
    let mut package = QueryPackage::balanced(QuerySpec::seed(TapeFn::Bids(bids.clone())));
    src.evaluate(&mut package)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;
    let local_bb = BeliefBase::from(package.into_graph());

    // Collect owned edges per owner via get_context → all_owned_edges().
    let mut owner_claims: HashMap<Bid, Vec<(Bid, Bid, String)>> = HashMap::new();
    let mut all_bids_set: HashSet<Bid> = bids.iter().copied().collect();

    for &owner_bid in &bids {
        // Discover home network for this owner.
        let root_net = match local_bb.paths().indexed_path(&owner_bid).map(|(n, _, _)| n) {
            Some(n) => n,
            None => {
                tracing::debug!(
                    "[get_maps_to] no path found for owner {owner_bid} in local PathMapMap"
                );
                continue;
            }
        };
        let Some(ctx) = local_bb.get_context(&root_net, &owner_bid) else {
            tracing::debug!("[get_maps_to] get_context returned None for owner {owner_bid}");
            continue;
        };
        for oe in ctx.all_owned_edges() {
            let kind_str = weight_kind_str(oe.weight_kind);
            if !active_kinds.contains(&kind_str) {
                continue;
            }
            all_bids_set.insert(oe.source_bid);
            all_bids_set.insert(oe.sink_bid);
            owner_claims
                .entry(owner_bid)
                .or_default()
                .push((oe.source_bid, oe.sink_bid, kind_str));
        }
    }

    // Batch-fetch titles for all referenced BIDs.
    let all_bid_vec: Vec<Bid> = all_bids_set.into_iter().collect();
    let titles = batch_titles(src, &all_bid_vec).await?;
    let label = |b: Bid| titles.get(&b).cloned().unwrap_or_default();

    let mut claims: Vec<MapsToClaim> = Vec::new();
    let mut owner_count = 0usize;

    // Emit in input BID order.
    for owner_bid in &bids {
        let edges = match owner_claims.get(owner_bid) {
            Some(e) if !e.is_empty() => e,
            _ => continue,
        };
        owner_count += 1;
        for (source_bid, sink_bid, kind_str) in edges {
            claims.push(MapsToClaim {
                owner_bid: owner_bid.to_string(),
                owner_bref: owner_bid.bref().to_string(),
                owner_label: label(*owner_bid),
                source_bid: source_bid.to_string(),
                source_bref: source_bid.bref().to_string(),
                source_label: label(*source_bid),
                sink_bid: sink_bid.to_string(),
                sink_bref: sink_bid.bref().to_string(),
                sink_label: label(*sink_bid),
                weight_kind: kind_str.clone(),
            });
        }
    }

    Ok(MapsToOutput {
        claims,
        owner_count,
    })
}

// ── get_maps_to_traceability ──────────────────────────────────────────────────

/// Return the full three-level claim index for the structural submap of a node.
///
/// Resolves the structural submap (Section-edge traversal from `bid`), then
/// evaluates a balanced query for all submap BIDs to obtain a local `BeliefBase`
/// with the `sko` halo. Per-owner `get_context` → `all_owned_edges()` extracts
/// the claims without scanning the full graph.
///
/// Returns owner → sink → \{kind: \[sources\]\} in submap encounter order.
/// Owners with no claims in the requested weight kinds are omitted.
///
/// Answers: "for each item in my traceability network, what does it claim to
/// cover, and via which source nodes per relationship kind?"
pub async fn get_maps_to_traceability(
    state: &McpState,
    input: GetMapsToTraceabilityInput,
) -> Result<MapsToTraceabilityOutput, McpError> {
    tracing::debug!(bid = %input.bid, depth = input.depth, "get_maps_to_traceability called");

    let bid = parse_bid(&input.bid)?;
    let active_kinds = parse_weight_kinds(&input.weight_kinds);
    let src = state.source_ref();

    // Resolve home network and structural submap.
    let mut package = QueryPackage::balanced(QuerySpec::seed(TapeFn::Bids(vec![bid])));
    src.evaluate(&mut package)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;
    let bg = package.into_graph();
    if bg.states.is_empty() {
        return Err(McpError::invalid_params(
            format!("BID {bid} not found"),
            None,
        ));
    }
    let entry_bb = BeliefBase::from(bg);
    let network_bid = entry_bb
        .paths()
        .indexed_path(&bid)
        .map(|(net, _, _)| net)
        .ok_or_else(|| McpError::invalid_params(format!("BID {bid} path not resolvable"), None))?;

    let entries = src
        .submap_by_bid(network_bid, Some(bid), input.depth, true)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

    let submap_bids: Vec<Bid> = entries.iter().map(|(_, b, _)| *b).collect();
    let submap_bid_set: HashSet<Bid> = submap_bids.iter().copied().collect();

    // Single balanced evaluation for all submap BIDs. The sko halo
    // discovers owned-edge endpoints as Trace context.
    let mut submap_package =
        QueryPackage::balanced(QuerySpec::seed(TapeFn::Bids(submap_bids.clone())));
    src.evaluate(&mut submap_package)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;
    let local_bb = BeliefBase::from(submap_package.into_graph());

    // Per-owner get_context → all_owned_edges() to build the claim index.
    let mut owner_order: Vec<Bid> = Vec::new();
    let mut owner_index: HashMap<Bid, BTreeMap<Bid, BTreeMap<String, Vec<Bid>>>> = HashMap::new();
    let mut all_bids: HashSet<Bid> = submap_bid_set;

    for &owner_bid in &submap_bids {
        let root_net = match local_bb.paths().indexed_path(&owner_bid).map(|(n, _, _)| n) {
            Some(n) => n,
            None => {
                tracing::debug!(
                    "[get_maps_to_traceability] no path found for {owner_bid} in local PathMapMap"
                );
                continue;
            }
        };
        let Some(ctx) = local_bb.get_context(&root_net, &owner_bid) else {
            tracing::debug!("[get_maps_to_traceability] get_context returned None for {owner_bid}");
            continue;
        };
        let owned = ctx.all_owned_edges();
        if owned.is_empty() {
            continue;
        }
        if !owner_order.contains(&owner_bid) {
            owner_order.push(owner_bid);
        }
        for oe in owned {
            let kind_str = weight_kind_str(oe.weight_kind);
            if !active_kinds.contains(&kind_str) {
                continue;
            }
            all_bids.insert(oe.source_bid);
            all_bids.insert(oe.sink_bid);
            let sink_map = owner_index.entry(owner_bid).or_default();
            let kind_map = sink_map.entry(oe.sink_bid).or_default();
            kind_map.entry(kind_str).or_default().push(oe.source_bid);
        }
    }

    // Batch-fetch titles for all referenced BIDs.
    let all_bid_vec: Vec<Bid> = all_bids.into_iter().collect();
    let titles = batch_titles(src, &all_bid_vec).await?;
    let label = |b: Bid| titles.get(&b).cloned().unwrap_or_default();

    // Materialise the index into the output structure, preserving submap encounter order.
    let owners: Vec<MapsToOwner> = owner_order
        .into_iter()
        .filter_map(|owner_bid| {
            owner_index
                .remove(&owner_bid)
                .map(|sink_map| (owner_bid, sink_map))
        })
        .map(|(owner_bid, sink_map)| {
            let sink_groups: Vec<MapsToSinkGroup> = sink_map
                .into_iter()
                .map(|(sink_bid, kind_map)| {
                    let by_kind: BTreeMap<String, Vec<MapsToSource>> = kind_map
                        .into_iter()
                        .map(|(kind_str, source_bids)| {
                            let sources = source_bids
                                .into_iter()
                                .map(|sb| MapsToSource {
                                    bid: sb.to_string(),
                                    bref: sb.bref().to_string(),
                                    label: label(sb),
                                })
                                .collect();
                            (kind_str, sources)
                        })
                        .collect();
                    MapsToSinkGroup {
                        sink_bid: sink_bid.to_string(),
                        sink_bref: sink_bid.bref().to_string(),
                        sink_label: label(sink_bid),
                        by_kind,
                    }
                })
                .collect();
            MapsToOwner {
                bid: owner_bid.to_string(),
                bref: owner_bid.bref().to_string(),
                label: label(owner_bid),
                sink_groups,
            }
        })
        .collect();

    Ok(MapsToTraceabilityOutput {
        entry_bid: bid.to_string(),
        entry_home_net: network_bid.to_string(),
        owners,
    })
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Fetch a title map for a batch of BIDs via a single balanced query.
///
/// Returns a `HashMap<Bid, String>` of node titles. BIDs not found in the source
/// are absent from the map (callers should default to an empty string).
async fn batch_titles(
    src: &dyn BeliefSource,
    bids: &[Bid],
) -> Result<HashMap<Bid, String>, McpError> {
    if bids.is_empty() {
        return Ok(HashMap::new());
    }
    let mut package = QueryPackage::balanced(QuerySpec::seed(TapeFn::Bids(bids.to_vec())));
    src.evaluate(&mut package)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;
    let bg = package.into_graph();
    Ok(bg.states.into_iter().map(|(b, n)| (b, n.title)).collect())
}

/// Parse a list of weight kind strings into a `HashSet<String>`.
/// Unknown values are silently ignored; an empty input returns all three kinds.
fn parse_weight_kinds(kinds: &[String]) -> HashSet<String> {
    let valid: HashSet<&str> = ["section", "epistemic", "pragmatic"]
        .iter()
        .copied()
        .collect();
    if kinds.is_empty() {
        return valid.into_iter().map(str::to_string).collect();
    }
    kinds
        .iter()
        .filter(|k| valid.contains(k.as_str()))
        .cloned()
        .collect()
}

/// Parse a BID string, returning a well-typed `McpError` on failure.
fn parse_bid(s: &str) -> Result<Bid, McpError> {
    Bid::try_from(s).map_err(|_| {
        McpError::invalid_params(
            format!("Invalid BID: {s:?}. Expected a UUID string (e.g. \"550e8400-e29b-41d4-a716-446655440000\")."),
            None,
        )
    })
}

/// Return a stable string representation of a `WeightKind`.
fn weight_kind_str(kind: WeightKind) -> String {
    match kind {
        WeightKind::Epistemic => "epistemic".to_string(),
        WeightKind::Section => "section".to_string(),
        WeightKind::Pragmatic => "pragmatic".to_string(),
    }
}

/// Convert a `toml::value::Table` to a `BTreeMap<String, serde_json::Value>`.
///
/// Delegates to `crate::codec::belief_ir::toml_value_to_json` — the canonical
/// implementation shared with `wasm.rs::extract_node_context`.
fn toml_table_to_json_map(table: &toml::value::Table) -> BTreeMap<String, serde_json::Value> {
    table
        .iter()
        .map(|(k, v)| (k.clone(), toml_value_to_json(v)))
        .collect()
}
