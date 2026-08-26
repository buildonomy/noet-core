//! Sharded BeliefGraph export.
//!
//! Implements the two export paths for `finalize_html`:
//!
//! - **Monolithic** (below `SHARD_THRESHOLD`): writes `beliefbase.msgpack`.
//! - **Sharded** (at or above threshold): writes `beliefbase/manifest.json`,
//!   `beliefbase/global.msgpack`, and `beliefbase/networks/{bref}.msgpack`.
//!
//! The top-level entry point is [`export_beliefbase`], which chooses between
//! the two paths and returns an [`ExportMode`] describing what was written.
//!
//! ## Global Shard
//!
//! The `global.msgpack` shard contains nodes that must always be available for
//! cross-network link resolution:
//!
//! - The API node (`buildonomy_api_bid`)
//! - System namespace nodes (href, asset namespaces)
//! - Any `BeliefNode` whose BID is not owned by a specific network (i.e. not
//!   found under any network's PathMap)
//!
//! Cross-network relations (edges between nodes in different networks) are also
//! included in the global shard's `relations` so that the viewer can resolve
//! them with only the global shard loaded.
//!
//! ## Per-Network Shards
//!
//! Each network shard contains the `BeliefGraph` subset for one network:
//! all `BeliefNode` states reachable from the network's PathMap, plus all
//! edges whose both endpoints are in that network. Trace nodes introduced by
//! balanced traversal (cross-network references) are excluded — they belong
//! to the global shard or to other network shards.
//!
//! ## Wire Format
//!
//! All shards are serialized as **MessagePack** using `rmp_serde::to_vec_named`.
//! The manifest (`beliefbase/manifest.json`) remains JSON — it is tiny and is
//! read by JavaScript before WASM initializes. The monolithic export uses
//! MessagePack (`beliefbase.msgpack`) to match the sharded wire format.
//!
//! ## References
//!
//! - `docs/design/search_and_sharding.md` §3 — Output structure
//! - `docs/design/search_and_sharding.md` §5 — Per-network shard format
//! - Issue 50: BeliefBase Sharding

use crate::{
    beliefbase::{BeliefGraph, BidGraph},
    error::BuildonomyError,
    paths::PathMapMap,
    properties::{Bid, Bref, WEIGHT_OWNED_BY},
    shard::{
        manifest::{
            estimate_size_mb, network_shard_meta, CodecManifest, GlobalShardMeta, SearchManifest,
            ShardConfig, ShardManifest,
        },
        wire::{GlobalShard, NetworkShard, SerializableBidGraph},
    },
};
use petgraph::visit::{EdgeRef, IntoEdgeReferences};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

/// Serialize a shard value to MessagePack bytes using named fields.
///
/// `to_vec_named` uses string field names (like JSON) rather than integer
/// indices, which makes the format self-describing and forward-compatible
/// with optional fields added in future versions.
fn to_msgpack<T: serde::Serialize>(value: &T) -> Result<Vec<u8>, BuildonomyError> {
    rmp_serde::to_vec_named(value).map_err(|e| BuildonomyError::Serialization(e.to_string()))
}

// ── Export mode result ────────────────────────────────────────────────────────

/// Describes the result of [`export_beliefbase`].
#[derive(Debug)]
pub enum ExportMode {
    /// Wrote `beliefbase.msgpack` (total size below [`ShardConfig::shard_threshold`]).
    Monolithic { size_mb: f64 },
    /// Wrote `beliefbase/` directory with manifest and per-network shards.
    Sharded { manifest: ShardManifest },
}

// ── Top-level entry point ─────────────────────────────────────────────────────

/// Export the BeliefBase to `output_dir`, choosing monolithic or sharded format.
///
/// This is the replacement for `DocumentCompiler::export_beliefbase_json`. It:
///
/// 1. Serializes the full graph to measure its size.
/// 2. If below [`ShardConfig::shard_threshold`]: writes `beliefbase.msgpack` and
///    returns [`ExportMode::Monolithic`].
/// 3. If at or above threshold: calls `export_sharded` and returns
///    [`ExportMode::Sharded`].
///
/// The `search_manifest` argument is used to annotate network entries in the
/// BB manifest with their search index paths and sizes (sharded mode only).
///
/// # Arguments
///
/// * `graph`          — Full `BeliefGraph` (from `global_bb.export_beliefgraph()`)
/// * `pathmap`        — PathMapMap for network enumeration and node ownership
/// * `output_dir`     — HTML output directory root
/// * `config`         — Sharding configuration (threshold, memory budget)
/// * `search_manifest`— Search manifest returned by `build_search_indices`
pub async fn export_beliefbase(
    graph: BeliefGraph,
    pathmap: &PathMapMap,
    output_dir: &Path,
    config: &ShardConfig,
    search_manifest: &SearchManifest,
    codec_manifest: &CodecManifest,
) -> Result<ExportMode, BuildonomyError> {
    // Serialize the full graph to measure its size.
    let json_string = serde_json::to_string_pretty(&graph)
        .map_err(|e| BuildonomyError::Serialization(e.to_string()))?;
    let total_bytes = json_string.len();

    if config.should_shard(total_bytes) {
        tracing::debug!(
            "[export_beliefbase] Graph is {:.2} MB — using sharded export",
            estimate_size_mb(total_bytes),
        );
        let manifest = export_sharded(
            graph,
            pathmap,
            output_dir,
            config,
            search_manifest,
            codec_manifest,
        )
        .await?;
        Ok(ExportMode::Sharded { manifest })
    } else {
        let size_mb = estimate_size_mb(total_bytes);
        tracing::debug!(
            "[export_beliefbase] Graph is {:.2} MB — writing monolithic beliefbase.msgpack",
            size_mb,
        );
        let msgpack_bytes = to_msgpack(&graph)?;
        let msgpack_path = output_dir.join("beliefbase.msgpack");
        tokio::fs::write(&msgpack_path, msgpack_bytes).await?;
        // Write codec manifest alongside monolithic export.
        let codec_json = serde_json::to_string_pretty(codec_manifest)
            .map_err(|e| BuildonomyError::Serialization(e.to_string()))?;
        tokio::fs::write(output_dir.join("codecs.json"), codec_json).await?;
        tracing::debug!(
            "Exported BeliefGraph to {} ({:.2} MB, {} states, {} relations)",
            msgpack_path.display(),
            size_mb,
            graph.states.len(),
            graph.relations.as_graph().edge_count(),
        );
        Ok(ExportMode::Monolithic { size_mb })
    }
}

// ── Sharded export ────────────────────────────────────────────────────────────

/// Write `beliefbase/` with manifest, global shard, and per-network shards.
///
/// Called when the total export exceeds [`ShardConfig::shard_threshold`].
///
/// # Directory layout produced
///
/// ```text
/// beliefbase/
/// ├── manifest.json
/// ├── global.msgpack
/// └── networks/
///     ├── {bref_a}.msgpack
///     └── {bref_b}.msgpack
/// ```
async fn export_sharded(
    graph: BeliefGraph,
    pathmap: &PathMapMap,
    output_dir: &Path,
    config: &ShardConfig,
    search_manifest: &SearchManifest,
    codec_manifest: &CodecManifest,
) -> Result<ShardManifest, BuildonomyError> {
    let bb_dir = output_dir.join("beliefbase");
    let networks_dir = bb_dir.join("networks");
    tokio::fs::create_dir_all(&networks_dir).await?;

    let mut shard_manifest = ShardManifest::new(config.memory_budget_mb);

    // Build a lookup from bref string → search index size_kb (from the search manifest).
    let search_size_lookup: BTreeMap<&str, f64> = search_manifest
        .networks
        .iter()
        .map(|n| (n.bref.as_str(), n.size_kb))
        .collect();

    // Partition the full BeliefGraph into:
    //   - per-network state sets (keyed by network Bref)
    //   - global states (API node, namespace nodes, unowned nodes)
    let partition = partition_graph(&graph, pathmap);

    // ── Write global shard ────────────────────────────────────────────────
    let bref_index: BTreeMap<String, String> = partition
        .network_states
        .iter()
        .flat_map(|(net_bref, bids)| {
            let net_bref_str = net_bref.to_string();
            bids.iter()
                .map(move |bid| (bid.bref().to_string(), net_bref_str.clone()))
        })
        .collect();

    let global_shard = GlobalShard {
        states: partition
            .global_states
            .iter()
            .filter_map(|bid| graph.states.get(bid).map(|n| (bid.to_string(), n.clone())))
            .collect(),
        relations: SerializableBidGraph::from_bid_graph(&partition.global_relations),
        bref_index,
    };

    let global_bytes = to_msgpack(&global_shard)?;
    let global_byte_len = global_bytes.len();
    tokio::fs::write(bb_dir.join("global.msgpack"), global_bytes).await?;

    shard_manifest.global = GlobalShardMeta {
        node_count: global_shard.states.len(),
        estimated_size_mb: estimate_size_mb(global_byte_len),
        path: "global.msgpack".to_string(),
    };

    tracing::debug!(
        "[export_sharded] Wrote global.msgpack: {} nodes, {:.2} MB",
        global_shard.states.len(),
        estimate_size_mb(global_byte_len),
    );

    // ── Build bref → BID lookup for owner-node halo resolution ────────────
    // Cross-network {maps_to} edges carry a WEIGHT_OWNED_BY bref identifying
    // the section that declared the mapping. That owner node typically lives
    // in a different network shard. Without embedding it in each shard that
    // contains the edge, the viewer's extract_node_context can't resolve the
    // bref to a BID and silently drops the OwnedEdge.
    let bref_to_bid: BTreeMap<Bref, Bid> =
        graph.states.keys().map(|bid| (bid.bref(), *bid)).collect();

    // ── Write per-network shards ──────────────────────────────────────────
    let mut total_node_count = global_shard.states.len();

    for (net_bref, net_bid) in &partition.networks {
        let net_states: BTreeSet<Bid> = partition
            .network_states
            .get(net_bref)
            .cloned()
            .unwrap_or_default();

        // Build per-network relations: edges where at least one endpoint is in
        // this network's state set (source OR sink matches).
        // This captures both intra-network edges and href/asset→content edges.
        let net_relations_graph = {
            let g = graph.relations.as_graph();
            BidGraph::from_edges(g.edge_references().filter_map(|e| {
                let source = g[e.source()];
                let sink = g[e.target()];
                if net_states.contains(&source) || net_states.contains(&sink) {
                    Some((source, sink, e.weight().clone()))
                } else {
                    None
                }
            }))
        };

        // Collect the const-namespace nodes (href/asset stubs) that are
        // referenced by this shard's edges but are not themselves in net_states.
        // Embedding them here means the viewer can resolve links (e.g. render
        // "links to https://..." tooltips) with only the global shard + this
        // network shard loaded, without fetching the href or asset network shard.
        let mut referenced_extern_states: BTreeMap<String, crate::properties::BeliefNode> =
            BTreeMap::new();
        {
            let g = net_relations_graph.as_graph();
            let net_edges: Vec<_> = g.edge_references().collect();
            for edge in net_edges {
                for &endpoint_bid in &[g[edge.source()], g[edge.target()]] {
                    if !net_states.contains(&endpoint_bid) {
                        if let Some(node) = graph.states.get(&endpoint_bid) {
                            referenced_extern_states
                                .entry(endpoint_bid.to_string())
                                .or_insert_with(|| node.clone());
                        }
                    }
                }
            }
        }

        // Collect third-party owner nodes referenced by WEIGHT_OWNED_BY in
        // this shard's edges. These are typically {maps_to} directive owners
        // that live in a different network. Without them in the halo, the
        // viewer can't resolve the bref → BID for OwnedEdge construction.
        {
            let g = net_relations_graph.as_graph();
            for edge in g.edge_references() {
                for weight in edge.weight().weights.values() {
                    if let Some(owner_str) = weight.get::<String>(WEIGHT_OWNED_BY) {
                        // Skip "source" / "sink" sentinels — only third-party brefs.
                        if owner_str == "source" || owner_str == "sink" {
                            continue;
                        }
                        if let Ok(owner_bref) = Bref::try_from(owner_str.as_str()) {
                            if let Some(&owner_bid) = bref_to_bid.get(&owner_bref) {
                                if !net_states.contains(&owner_bid) {
                                    if let Some(node) = graph.states.get(&owner_bid) {
                                        referenced_extern_states
                                            .entry(owner_bid.to_string())
                                            .or_insert_with(|| node.clone());
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // For each referenced extern node, also pull its Section edge to its
        // namespace parent (e.g. href_node → href_namespace) and the parent
        // node itself. Without this, get_context in the viewer cannot walk up
        // to a known namespace root and reports "Node not found in any context".
        let mut extra_edges: Vec<(Bid, Bid, crate::properties::WeightSet)> = Vec::new();
        {
            let full_g = graph.relations.as_graph();
            for edge in full_g.edge_references() {
                let source = full_g[edge.source()];
                let sink = full_g[edge.target()];
                // Only Section edges from extern nodes to their namespace parent.
                if referenced_extern_states.contains_key(&source.to_string())
                    && !net_states.contains(&sink)
                    && edge
                        .weight()
                        .weights
                        .contains_key(&crate::properties::WeightKind::Section)
                {
                    // Include the namespace parent node.
                    if let Some(sink_node) = graph.states.get(&sink) {
                        referenced_extern_states
                            .entry(sink.to_string())
                            .or_insert_with(|| sink_node.clone());
                    }
                    extra_edges.push((source, sink, edge.weight().clone()));
                }
            }
        }

        // Merge own states with referenced extern states for the shard payload.
        let mut shard_states: BTreeMap<String, crate::properties::BeliefNode> = net_states
            .iter()
            .filter_map(|bid| graph.states.get(bid).map(|n| (bid.to_string(), n.clone())))
            .collect();
        shard_states.extend(referenced_extern_states);

        // Rebuild the relations graph with the extra Section edges included.
        let net_relations_graph = if extra_edges.is_empty() {
            net_relations_graph
        } else {
            let existing: Vec<(Bid, Bid, crate::properties::WeightSet)> = {
                let g = net_relations_graph.as_graph();
                g.edge_references()
                    .map(|e| (g[e.source()], g[e.target()], e.weight().clone()))
                    .collect()
            };
            BidGraph::from_edges(existing.into_iter().chain(extra_edges))
        };

        let net_shard = NetworkShard {
            network_bref: net_bref.to_string(),
            network_bid: net_bid.to_string(),
            states: shard_states,
            relations: SerializableBidGraph::from_bid_graph(&net_relations_graph),
        };

        let shard_bytes = to_msgpack(&net_shard)?;
        let shard_byte_len = shard_bytes.len();

        let bref_str = net_bref.to_string();
        let shard_filename = format!("{}.msgpack", bref_str);
        let shard_path = networks_dir.join(&shard_filename);
        tokio::fs::write(&shard_path, shard_bytes).await?;

        let net_title = graph
            .states
            .get(net_bid)
            .map(|n| n.display_title())
            .unwrap_or_else(|| bref_str.clone());

        let search_size_kb = search_size_lookup
            .get(bref_str.as_str())
            .copied()
            .unwrap_or(0.0);

        let meta = network_shard_meta(
            *net_bref,
            *net_bid,
            net_title,
            net_shard.states.len(),
            net_shard.relations.edges.len(),
            shard_byte_len,
            (search_size_kb * 1024.0) as usize,
        );

        tracing::debug!(
            "[export_sharded] Wrote networks/{}: {} nodes, {:.2} MB",
            shard_filename,
            meta.node_count,
            meta.estimated_size_mb,
        );

        total_node_count += meta.node_count;
        shard_manifest.networks.push(meta);
    }

    // ── Write shard manifest ──────────────────────────────────────────────
    let manifest_json = serde_json::to_string_pretty(&shard_manifest)
        .map_err(|e| BuildonomyError::Serialization(e.to_string()))?;
    tokio::fs::write(bb_dir.join("manifest.json"), manifest_json).await?;

    // ── Write codec manifest (sibling to beliefbase/) ─────────────────────
    let codec_json = serde_json::to_string_pretty(codec_manifest)
        .map_err(|e| BuildonomyError::Serialization(e.to_string()))?;
    tokio::fs::write(output_dir.join("codecs.json"), codec_json).await?;

    tracing::debug!(
        "[export_sharded] Wrote {} network shards + global ({} total nodes)",
        shard_manifest.networks.len(),
        total_node_count,
    );

    Ok(shard_manifest)
}

// ── Graph partitioning ────────────────────────────────────────────────────────

/// Result of partitioning a `BeliefGraph` into global and per-network sets.
struct GraphPartition {
    /// BIDs that belong to no specific network (API node, namespace roots, etc.).
    global_states: BTreeSet<Bid>,
    /// Cross-network edges that belong in the global shard.
    global_relations: BidGraph,
    /// Ordered list of `(Bref, Bid)` pairs for all networks.
    networks: Vec<(Bref, Bid)>,
    /// Per-network non-Trace state BID sets, keyed by network Bref.
    network_states: BTreeMap<Bref, BTreeSet<Bid>>,
}

/// Partition `graph` into global and per-network state sets using `pathmap`.
///
/// A node is assigned to a network if its BID appears in that network's PathMap.
/// A node is assigned to the global shard if it does not appear in any network's
/// PathMap (e.g. the API node, bare namespace roots, unresolved trace refs).
///
/// Edge assignment:
/// - Both endpoints in the same network → that network's shard.
/// - At least one endpoint has a network home → the network shard of the
///   endpoint that has a home (content endpoint wins over href/asset).
///   This keeps href→content edges out of global.json: they are already
///   included in the per-network shard via a "NodeIn" edge filter, which
///   matches edges where *either* endpoint is in the network's state set.
/// - Neither endpoint has a network home → global shard's relations.
///
/// This prevents content-namespace trace nodes (href stubs, asset stubs) from
/// bloating global.json: previously all Trace nodes and all cross-network edges
/// landed in global, inflating it to 17–31 MB and freezing the browser.
fn partition_graph(graph: &BeliefGraph, pathmap: &PathMapMap) -> GraphPartition {
    // Build a BID → network Bref lookup from the PathMapMap.
    let mut bid_to_net: BTreeMap<Bid, Bref> = BTreeMap::new();
    let mut networks: Vec<(Bref, Bid)> = Vec::new();

    for &net_bid in pathmap.nets() {
        let net_bref = net_bid.bref();
        networks.push((net_bref, net_bid));

        if let Some(pm) = pathmap.get_map(&net_bref) {
            for (_path, bid, _order) in pm.map() {
                // Use or_insert so that if this BID also appears in a sub-network's
                // own PathMap (for its own content), the sub-network's claim wins
                // (processed later in the loop, but or_insert means first-writer wins).
                // Subnet root BIDs appearing in the parent's map are intentionally
                // included here — they belong to the parent shard so they are available
                // when the parent shard is loaded.
                bid_to_net.entry(*bid).or_insert(net_bref);
            }
        }
    }

    // Sort networks for stable output ordering.
    networks.sort_by_key(|(bref, _)| *bref);

    // Partition states.
    let mut global_states: BTreeSet<Bid> = BTreeSet::new();
    let mut network_states: BTreeMap<Bref, BTreeSet<Bid>> = BTreeMap::new();

    for &bid in graph.states.keys() {
        // Trace nodes: assign to their home network if they have a PathMap entry
        // (e.g. href/asset nodes that live in a content-namespace shard), and only
        // fall back to global if they have no home. The old rule "all Trace → global"
        // bloated global.json with thousands of href/asset trace nodes, causing the
        // browser to freeze while parsing a 17 MB payload on every page load.
        match bid_to_net.get(&bid) {
            Some(&net_bref) => {
                network_states.entry(net_bref).or_default().insert(bid);
            }
            None => {
                // Not found in any network's PathMap — goes to global shard.
                // This correctly captures: API node, bare namespace roots, and
                // any Trace node that has no registered path (unresolved refs, etc.).
                global_states.insert(bid);
            }
        }
    }

    // Partition edges.
    //
    // An edge goes to global only if NEITHER endpoint has a network home.
    // If at least one endpoint has a home, the edge is already included in
    // that network's shard via the NodeIn edge filter in export_sharded
    // (which keeps edges where source OR sink is in the set).
    // Putting such edges in global too would duplicate them and bloat global.json
    // with tens of thousands of href→content edges.
    let mut global_edge_sources: Vec<(Bid, Bid, crate::properties::WeightSet)> = Vec::new();

    let g = graph.relations.as_graph();
    let partition_edges: Vec<_> = g.edge_references().collect();
    for edge in partition_edges {
        let source_bid = g[edge.source()];
        let sink_bid = g[edge.target()];
        let source_net = bid_to_net.get(&source_bid);
        let sink_net = bid_to_net.get(&sink_bid);

        // Only truly unowned edges (no network home on either side) go to global.
        if source_net.is_none() && sink_net.is_none() {
            global_edge_sources.push((source_bid, sink_bid, edge.weight().clone()));
        }
    }

    let global_relations = BidGraph::from_edges(
        global_edge_sources
            .iter()
            .map(|(src, sink, weights)| (*src, *sink, weights.clone())),
    );

    GraphPartition {
        global_states,
        global_relations,
        networks,
        network_states,
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        beliefbase::BeliefBase,
        paths::PathMapMap,
        properties::{
            BeliefKind, BeliefKindSet, BeliefNode, NodeId, WeightKind, WeightSet, WEIGHT_OWNED_BY,
        },
        shard::manifest::{CodecManifest, SearchManifest, ShardConfig},
    };

    fn make_node(title: &str, kind: BeliefKind) -> BeliefNode {
        BeliefNode {
            bid: Bid::new(Bid::nil()),
            kind: BeliefKindSet::from(kind),
            title: title.to_string(),
            schema: None,
            payload: toml::Table::new(),
            id: NodeId::default(),
            metadata: toml::Table::new(),
        }
    }

    #[test]
    fn test_shard_config_threshold_default() {
        let config = ShardConfig::default();
        assert!(!config.should_shard(1024));
        assert!(config.should_shard(crate::shard::manifest::SHARD_THRESHOLD));
    }

    #[test]
    fn test_serializable_bid_graph_empty() {
        let graph = BidGraph::default();
        let sg = SerializableBidGraph::from_bid_graph(&graph);
        assert!(sg.edges.is_empty());
    }

    #[test]
    fn test_network_shard_roundtrip() {
        let node = make_node("Test Doc", BeliefKind::Document);
        let bid = node.bid;
        let shard = NetworkShard {
            network_bref: "01abc".to_string(),
            network_bid: bid.to_string(),
            states: [(bid.to_string(), node)].into_iter().collect(),
            relations: SerializableBidGraph::default(),
        };
        let bytes = rmp_serde::to_vec_named(&shard).unwrap();
        let decoded: NetworkShard = rmp_serde::from_slice(&bytes).unwrap();
        assert_eq!(decoded.network_bref, "01abc");
        assert_eq!(decoded.states.len(), 1);
    }

    #[test]
    fn test_export_mode_monolithic_below_threshold() {
        let config = ShardConfig {
            shard_threshold: 1_000_000, // 1MB
            memory_budget_mb: 200.0,
        };
        let small_json = "{}";
        assert!(!config.should_shard(small_json.len()));
    }

    #[test]
    fn test_export_mode_sharded_above_threshold() {
        let config = ShardConfig {
            shard_threshold: 1, // absurdly small
            memory_budget_mb: 200.0,
        };
        assert!(config.should_shard(100));
    }

    #[test]
    fn test_partition_assigns_unowned_to_global() {
        let node = make_node("Orphan", BeliefKind::Document);
        let bid = node.bid;
        let graph = BeliefGraph {
            states: [(bid, node)].into_iter().collect(),
            relations: BidGraph::default(),
        };
        let pathmap = PathMapMap::default();
        let partition = partition_graph(&graph, &pathmap);
        assert!(partition.global_states.contains(&bid));
        assert!(partition.network_states.is_empty());
    }

    #[test]
    fn test_partition_excludes_trace_nodes_from_networks() {
        let mut node = make_node("Trace Node", BeliefKind::Document);
        node.kind.insert(BeliefKind::Trace);
        let bid = node.bid;
        let graph = BeliefGraph {
            states: [(bid, node)].into_iter().collect(),
            relations: BidGraph::default(),
        };
        let pathmap = PathMapMap::default();
        let partition = partition_graph(&graph, &pathmap);
        assert!(partition.global_states.contains(&bid));
    }

    /// Verify that a `NetworkShard` can be serialized to MessagePack, deserialized,
    /// and used to reconstruct a `BeliefGraph` suitable for `BeliefBase::merge`.
    /// This mirrors the logic in `BeliefBaseWasm::load_shard`.
    #[test]
    fn test_network_shard_deserialize_to_belief_graph() {
        use crate::beliefbase::BeliefBase;

        let node_a = make_node("Node A", BeliefKind::Document);
        let bid_a = node_a.bid;
        let node_b = make_node("Node B", BeliefKind::Document);
        let bid_b = node_b.bid;

        let shard = NetworkShard {
            network_bref: "01abc".to_string(),
            network_bid: bid_a.to_string(),
            states: [
                (bid_a.to_string(), node_a.clone()),
                (bid_b.to_string(), node_b.clone()),
            ]
            .into_iter()
            .collect(),
            relations: SerializableBidGraph::default(),
        };

        // Round-trip through MessagePack (as load_shard does).
        let bytes = rmp_serde::to_vec_named(&shard).unwrap();
        let decoded: NetworkShard = rmp_serde::from_slice(&bytes).unwrap();

        // Reconstruct a BeliefGraph from the decoded shard.
        let edges: Vec<(Bid, Bid, crate::properties::WeightSet)> = decoded
            .relations
            .edges
            .into_iter()
            .filter_map(|e| {
                let src = Bid::try_from(e.source.as_str()).ok()?;
                let snk = Bid::try_from(e.sink.as_str()).ok()?;
                Some((src, snk, e.weights))
            })
            .collect();
        let relations = BidGraph::from_edges(edges);
        let graph = BeliefGraph {
            states: decoded
                .states
                .into_iter()
                .filter_map(|(k, v)| Some((Bid::try_from(k.as_str()).ok()?, v)))
                .collect(),
            relations,
        };

        assert_eq!(graph.states.len(), 2);
        assert!(graph.states.contains_key(&bid_a));
        assert!(graph.states.contains_key(&bid_b));

        // Merge into a fresh BeliefBase and verify node presence.
        let mut bb = BeliefBase::default();
        let initial_count = bb.states().len();
        bb.merge(&graph);
        assert_eq!(bb.states().len(), initial_count + 2);
        assert!(bb.states().contains_key(&bid_a));
        assert!(bb.states().contains_key(&bid_b));
    }

    /// Verify that a `GlobalShard` deserializes correctly and its nodes can be
    /// merged then removed via `process_event(NodesRemoved)`.
    /// This mirrors the unload path in `BeliefBaseWasm::unload_shard`.
    #[test]
    fn test_global_shard_load_unload_cycle() {
        use crate::{
            beliefbase::BeliefBase,
            event::{BeliefEvent, EventOrigin},
        };

        let node = make_node("Global Node", BeliefKind::Document);
        let bid = node.bid;

        let shard = GlobalShard {
            states: [(bid.to_string(), node.clone())].into_iter().collect(),
            relations: SerializableBidGraph::default(),
            bref_index: BTreeMap::new(),
        };

        // Round-trip through MessagePack (as load_shard does).
        let bytes = rmp_serde::to_vec_named(&shard).unwrap();
        let decoded: GlobalShard = rmp_serde::from_slice(&bytes).unwrap();

        // Reconstruct graph and merge.
        let graph = BeliefGraph {
            states: decoded
                .states
                .into_iter()
                .filter_map(|(k, v)| Some((Bid::try_from(k.as_str()).ok()?, v)))
                .collect(),
            relations: BidGraph::default(),
        };

        let mut bb = BeliefBase::default();
        let initial_count = bb.states().len();
        bb.merge(&graph);
        assert_eq!(
            bb.states().len(),
            initial_count + 1,
            "node should be present after merge"
        );

        // Unload: remove via NodesRemoved event (mirrors BeliefBaseWasm::unload_shard).
        let bids_to_remove: Vec<Bid> = vec![bid];
        let event = BeliefEvent::NodesRemoved(bids_to_remove, EventOrigin::Remote);
        bb.process_event(&event)
            .expect("NodesRemoved should succeed");

        assert_eq!(
            bb.states().len(),
            initial_count,
            "node should be removed after unload"
        );
    }

    /// Verify that nodes shared between two loaded shards are not removed when
    /// only one shard is unloaded. This mirrors the `still_needed` filtering
    /// in `BeliefBaseWasm::unload_shard`.
    #[test]
    fn test_unload_skips_shared_nodes() {
        use crate::beliefbase::BeliefBase;
        use std::collections::BTreeSet;

        let shared_node = make_node("Shared", BeliefKind::Document);
        let shared_bid = shared_node.bid;
        let net_only_node = make_node("Net Only", BeliefKind::Document);
        let net_only_bid = net_only_node.bid;

        // Simulate: "global" shard has shared_bid, "net_a" shard has both.
        let global_bids: BTreeSet<Bid> = [shared_bid].into_iter().collect();
        let net_a_bids: BTreeSet<Bid> = [shared_bid, net_only_bid].into_iter().collect();

        // Build the full graph as if both shards were loaded.
        let mut bb = BeliefBase::default();
        let initial_count = bb.states().len();
        let graph = BeliefGraph {
            states: [
                (shared_bid, shared_node.clone()),
                (net_only_bid, net_only_node.clone()),
            ]
            .into_iter()
            .collect(),
            relations: BidGraph::default(),
        };
        bb.merge(&graph);
        assert_eq!(bb.states().len(), initial_count + 2);

        // Simulate unloading "net_a": compute to_remove excluding nodes still in "global".
        let still_needed: BTreeSet<Bid> = global_bids.iter().copied().collect();
        let to_remove: BTreeSet<Bid> = net_a_bids
            .into_iter()
            .filter(|bid| !still_needed.contains(bid))
            .collect();

        // Only net_only_bid should be removed.
        assert_eq!(to_remove.len(), 1);
        assert!(to_remove.contains(&net_only_bid));
        assert!(!to_remove.contains(&shared_bid));
    }

    /// Verify that cross-network WEIGHT_OWNED_BY owner nodes are embedded
    /// in the shard halo so the viewer can resolve OwnedEdge bref → BID.
    #[tokio::test]
    async fn test_shard_halo_includes_owned_by_owner_nodes() {
        // Create three nodes: net_root (network), doc_a (in net), owner (in a different net).
        let mut net_root = make_node("Network", BeliefKind::Network);
        net_root.bid = Bid::new(Bid::nil());
        let net_root_bid = net_root.bid;

        let mut doc_a = make_node("Doc A", BeliefKind::Document);
        doc_a.bid = Bid::new(net_root_bid);
        let doc_a_bid = doc_a.bid;

        // The "owner" node lives in a different network — it declared a {maps_to}
        // edge whose source or sink is doc_a.
        let mut owner = make_node("Owner Section", BeliefKind::Document);
        owner.bid = Bid::new(doc_a_bid);
        let owner_bid = owner.bid;
        let owner_bref = owner_bid.bref();

        // A foreign node that is the other endpoint of the maps_to edge.
        let mut foreign = make_node("Foreign Node", BeliefKind::Document);
        foreign.bid = Bid::new(owner_bid);
        let foreign_bid = foreign.bid;

        // Build an edge from foreign → doc_a with WEIGHT_OWNED_BY = owner's bref.
        let mut ws = WeightSet::from(WeightKind::Pragmatic);
        ws.weights
            .get_mut(&WeightKind::Pragmatic)
            .unwrap()
            .set(WEIGHT_OWNED_BY, owner_bref.to_string())
            .unwrap();

        // Section edge: doc_a is a child of net_root.
        let section_ws = WeightSet::from(WeightKind::Section);

        let relations = BidGraph::from_edges(vec![
            (doc_a_bid, net_root_bid, section_ws),
            (foreign_bid, doc_a_bid, ws),
        ]);

        let graph = BeliefGraph {
            states: [
                (net_root_bid, net_root),
                (doc_a_bid, doc_a),
                (owner_bid, owner),
                (foreign_bid, foreign),
            ]
            .into_iter()
            .collect(),
            relations,
        };

        // Use the full export pipeline with a tiny threshold to force sharded mode.
        let tmp_dir = tempfile::tempdir().unwrap();
        let config = ShardConfig {
            shard_threshold: 1,
            memory_budget_mb: 200.0,
        };

        // Build a real PathMapMap using BeliefBase.
        let mut bb = BeliefBase::default();
        bb.merge(&graph);
        let pathmap = bb.paths();

        let result = export_beliefbase(
            graph,
            &pathmap,
            tmp_dir.path(),
            &config,
            &SearchManifest::new(),
            &CodecManifest::new(vec![], vec![]),
        )
        .await
        .unwrap();

        // Verify sharded mode was used.
        assert!(
            matches!(result, ExportMode::Sharded { .. }),
            "expected sharded export"
        );

        // Read back all network shards and check that the owner node appears
        // in the shard that contains doc_a (where the WEIGHT_OWNED_BY edge lives).
        let networks_dir = tmp_dir.path().join("beliefbase/networks");
        let mut found_owner_in_halo = false;
        for entry in std::fs::read_dir(&networks_dir).unwrap() {
            let entry = entry.unwrap();
            let bytes = std::fs::read(entry.path()).unwrap();
            let shard: NetworkShard = rmp_serde::from_slice(&bytes).unwrap();

            if shard.states.contains_key(&doc_a_bid.to_string()) {
                // This is the shard containing doc_a — the owner should be in the halo.
                if shard.states.contains_key(&owner_bid.to_string()) {
                    found_owner_in_halo = true;
                }
            }
        }

        assert!(
            found_owner_in_halo,
            "Owner node (WEIGHT_OWNED_BY target) should be embedded in the shard halo \
             of the network containing the edge endpoint. Owner BID: {}, owner bref: {}",
            owner_bid, owner_bref
        );
    }
}
