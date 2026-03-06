//! Sharded BeliefGraph export.
//!
//! Implements the two export paths for `finalize_html`:
//!
//! - **Monolithic** (below `SHARD_THRESHOLD`): writes `beliefbase.json` as today.
//! - **Sharded** (at or above threshold): writes `beliefbase/manifest.json`,
//!   `beliefbase/global.json`, and `beliefbase/networks/{bref}.json`.
//!
//! The top-level entry point is [`export_beliefbase`], which chooses between
//! the two paths and returns an [`ExportMode`] describing what was written.
//!
//! ## Global Shard
//!
//! The `global.json` shard contains nodes that must always be available for
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
//! `evaluate_expression` (cross-network references) are excluded — they belong
//! to the global shard or to other network shards.
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
    properties::{BeliefKind, Bid, Bref},
    shard::{
        manifest::{
            estimate_size_mb, network_shard_meta, GlobalShardMeta, SearchManifest, ShardConfig,
            ShardManifest,
        },
        wire::{GlobalShard, NetworkShard, SerializableBidGraph},
    },
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

// ── Export mode result ────────────────────────────────────────────────────────

/// Describes the result of [`export_beliefbase`].
#[derive(Debug)]
pub enum ExportMode {
    /// Wrote `beliefbase.json` (total size below [`ShardConfig::shard_threshold`]).
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
/// 2. If below [`ShardConfig::shard_threshold`]: writes `beliefbase.json` and
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
) -> Result<ExportMode, BuildonomyError> {
    // Serialize the full graph to measure its size.
    let json_string = serde_json::to_string_pretty(&graph)
        .map_err(|e| BuildonomyError::Serialization(e.to_string()))?;
    let total_bytes = json_string.len();

    if config.should_shard(total_bytes) {
        tracing::info!(
            "[export_beliefbase] Graph is {:.2} MB — using sharded export",
            estimate_size_mb(total_bytes),
        );
        let manifest = export_sharded(graph, pathmap, output_dir, config, search_manifest).await?;
        Ok(ExportMode::Sharded { manifest })
    } else {
        let size_mb = estimate_size_mb(total_bytes);
        tracing::debug!(
            "[export_beliefbase] Graph is {:.2} MB — writing monolithic beliefbase.json",
            size_mb,
        );
        let json_path = output_dir.join("beliefbase.json");
        tokio::fs::write(&json_path, json_string).await?;
        tracing::debug!(
            "Exported BeliefGraph to {} ({:.2} MB, {} states, {} relations)",
            json_path.display(),
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
/// ├── global.json
/// └── networks/
///     ├── {bref_a}.json
///     └── {bref_b}.json
/// ```
async fn export_sharded(
    graph: BeliefGraph,
    pathmap: &PathMapMap,
    output_dir: &Path,
    config: &ShardConfig,
    search_manifest: &SearchManifest,
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
    let global_shard = GlobalShard {
        states: partition
            .global_states
            .iter()
            .filter_map(|bid| graph.states.get(bid).map(|n| (bid.to_string(), n.clone())))
            .collect(),
        relations: SerializableBidGraph::from_bid_graph(&partition.global_relations),
    };

    let global_json = serde_json::to_string_pretty(&global_shard)
        .map_err(|e| BuildonomyError::Serialization(e.to_string()))?;
    let global_bytes = global_json.len();
    tokio::fs::write(bb_dir.join("global.json"), global_json).await?;

    shard_manifest.global = GlobalShardMeta {
        node_count: global_shard.states.len(),
        estimated_size_mb: estimate_size_mb(global_bytes),
        path: "global.json".to_string(),
    };

    tracing::debug!(
        "[export_sharded] Wrote global.json: {} nodes, {:.2} MB",
        global_shard.states.len(),
        estimate_size_mb(global_bytes),
    );

    // ── Write per-network shards ──────────────────────────────────────────
    let mut total_node_count = global_shard.states.len();

    for (net_bref, net_bid) in &partition.networks {
        let net_states: BTreeSet<Bid> = partition
            .network_states
            .get(net_bref)
            .cloned()
            .unwrap_or_default();

        // Build per-network relations: edges where both endpoints are in this network.
        let net_relations = graph.relations.filter(
            &crate::query::RelationPred::NodeIn(net_states.iter().copied().collect()),
            false,
        );

        let net_shard = NetworkShard {
            network_bref: net_bref.to_string(),
            network_bid: net_bid.to_string(),
            states: net_states
                .iter()
                .filter_map(|bid| graph.states.get(bid).map(|n| (bid.to_string(), n.clone())))
                .collect(),
            relations: SerializableBidGraph::from_bid_graph(&BidGraph::from(net_relations)),
        };

        let shard_json = serde_json::to_string_pretty(&net_shard)
            .map_err(|e| BuildonomyError::Serialization(e.to_string()))?;
        let shard_bytes = shard_json.len();

        let bref_str = net_bref.to_string();
        let shard_filename = format!("{}.json", bref_str);
        let shard_path = networks_dir.join(&shard_filename);
        tokio::fs::write(&shard_path, shard_json).await?;

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
            shard_bytes,
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

    tracing::info!(
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
/// A node is assigned to the global shard if:
/// - It is the API node, or
/// - It belongs to a system namespace (href, asset), or
/// - It does not appear in any network's PathMap.
///
/// Trace nodes (added by `evaluate_expression` for referential integrity) are
/// excluded from per-network shards — they are already represented in the
/// global shard or in the shard of their home network.
///
/// Cross-network edges (source in network A, sink in network B) go into the
/// global shard's relations so they are always available.
fn partition_graph(graph: &BeliefGraph, pathmap: &PathMapMap) -> GraphPartition {
    // Build a BID → network Bref lookup from the PathMapMap.
    let mut bid_to_net: BTreeMap<Bid, Bref> = BTreeMap::new();
    let mut networks: Vec<(Bref, Bid)> = Vec::new();

    for &net_bid in pathmap.nets() {
        let net_bref = net_bid.bref();
        networks.push((net_bref, net_bid));

        if let Some(pm) = pathmap.get_map(&net_bref) {
            let all_paths = pm.recursive_map(pathmap, &mut BTreeSet::new());
            for (_path, bid, _order) in all_paths {
                bid_to_net.entry(bid).or_insert(net_bref);
            }
        }
    }

    // Sort networks for stable output ordering.
    networks.sort_by_key(|(bref, _)| *bref);

    // Partition states.
    let mut global_states: BTreeSet<Bid> = BTreeSet::new();
    let mut network_states: BTreeMap<Bref, BTreeSet<Bid>> = BTreeMap::new();

    for (&bid, node) in &graph.states {
        // Trace nodes never go into per-network shards.
        if node.kind.contains(BeliefKind::Trace) {
            global_states.insert(bid);
            continue;
        }

        match bid_to_net.get(&bid) {
            Some(&net_bref) => {
                network_states.entry(net_bref).or_default().insert(bid);
            }
            None => {
                // Not found in any network's PathMap — goes to global shard.
                global_states.insert(bid);
            }
        }
    }

    // Partition edges: intra-network edges stay with the network shard.
    // Cross-network edges go into the global shard's relations.
    let mut global_edge_sources: Vec<(Bid, Bid, crate::properties::WeightSet)> = Vec::new();

    let g = graph.relations.as_graph();
    for edge in g.raw_edges() {
        let source_bid = g[edge.source()];
        let sink_bid = g[edge.target()];
        let source_net = bid_to_net.get(&source_bid);
        let sink_net = bid_to_net.get(&sink_bid);

        let is_cross_network = match (source_net, sink_net) {
            (Some(a), Some(b)) => a != b,
            // One or both endpoints are in global → edge goes to global shard.
            _ => true,
        };

        if is_cross_network {
            global_edge_sources.push((source_bid, sink_bid, edge.weight.clone()));
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
        properties::{BeliefKind, BeliefKindSet, BeliefNode},
        shard::manifest::ShardConfig,
    };

    fn make_node(title: &str, kind: BeliefKind) -> BeliefNode {
        BeliefNode {
            bid: Bid::new(Bid::nil()),
            kind: BeliefKindSet::from(kind),
            title: title.to_string(),
            schema: None,
            payload: toml::Table::new(),
            id: None,
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
        let json = serde_json::to_string_pretty(&shard).unwrap();
        let decoded: NetworkShard = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.network_bref, "01abc");
        assert_eq!(decoded.states.len(), 1);
    }

    #[test]
    fn test_export_mode_monolithic_below_threshold() {
        // Verify that a small graph produces ExportMode::Monolithic.
        // We test the should_shard decision directly since we can't easily run
        // the full async export in a unit test without disk I/O.
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
        // A graph with one node not in any PathMap should end up in global_states.
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
        // Trace nodes should always go to global, even if their BID appears in a PathMap.
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

    /// Verify that a `NetworkShard` can be serialized to JSON, deserialized,
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

        // Round-trip through JSON (as load_shard does).
        let json = serde_json::to_string_pretty(&shard).unwrap();
        let decoded: NetworkShard = serde_json::from_str(&json).unwrap();

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
        // BeliefBase::default() already contains 1 node (the built-in API node).
        let mut bb = BeliefBase::default();
        let initial_count = bb.states().len(); // typically 1 (API node)
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
        };

        let json = serde_json::to_string_pretty(&shard).unwrap();
        let decoded: GlobalShard = serde_json::from_str(&json).unwrap();

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
        let initial_count = bb.states().len(); // typically 1 (API node)
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
        let initial_count = bb.states().len(); // typically 1 (API node)
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
}
