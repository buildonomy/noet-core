//! Shard wire format types: target-independent serialization structs.
//!
//! These types define the JSON schemas for the sharded BeliefBase export
//! format. They must compile on **all** targets — including `wasm32` — because
//! `BeliefBaseWasm::load_shard` deserializes them in the browser.
//!
//! The export *logic* (writing files, async I/O) lives in `super::export`
//! and is gated behind `#[cfg(not(target_arch = "wasm32"))]`. The types here
//! have no such gate.
//!
//! ## Types
//!
//! - [`NetworkShard`] — contents of `beliefbase/networks/{bref}.json`
//! - [`GlobalShard`] — contents of `beliefbase/global.json`
//! - [`SerializableBidGraph`] — portable edge list (BID strings, not petgraph indices)
//! - [`SerializableEdge`] — one edge in a [`SerializableBidGraph`]
//!
//! ## References
//!
//! - `docs/design/search_and_sharding.md` §5 — Per-network shard format
//! - Issue 50: BeliefBase Sharding

use crate::properties::{BeliefNode, WeightSet};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// ── Per-network shard ─────────────────────────────────────────────────────────

/// The JSON representation of a per-network BeliefBase shard.
///
/// Written to `beliefbase/networks/{bref}.json`. Contains the `BeliefGraph`
/// subset for one network — states and intra-network edges only. Trace nodes
/// (cross-network references) are excluded; they are resolved via the global
/// shard or other loaded shards.
///
/// See `docs/design/search_and_sharding.md` §5 for the schema.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkShard {
    /// Short reference (5 hex chars) of the network.
    pub network_bref: String,
    /// Full BID of the network node.
    pub network_bid: String,
    /// States belonging to this network (non-Trace nodes from the network PathMap).
    pub states: BTreeMap<String, BeliefNode>,
    /// Intra-network relations (edges where both endpoints are in `states`).
    pub relations: SerializableBidGraph,
}

// ── Global shard ──────────────────────────────────────────────────────────────

/// The JSON representation of the global shard (`beliefbase/global.json`).
///
/// Contains the API node, system namespace nodes, and cross-network edges.
/// Always loaded in sharded mode; provides the foundation for cross-network
/// link resolution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlobalShard {
    /// Nodes that belong to no specific network: API node, namespace roots,
    /// and any node not found in any network's PathMap.
    pub states: BTreeMap<String, BeliefNode>,
    /// All edges that cross network boundaries.
    pub relations: SerializableBidGraph,
}

// ── Portable BidGraph serialization ──────────────────────────────────────────

/// A portable serialization of a `BidGraph` as a list of `(source, sink, weights)` triples.
///
/// `petgraph`'s own serialization uses internal node indices, which are not
/// stable across independent deserialization. This type uses BID strings as
/// node identifiers, matching the rest of the wire format.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SerializableBidGraph {
    pub edges: Vec<SerializableEdge>,
}

/// One edge in a [`SerializableBidGraph`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SerializableEdge {
    pub source: String,
    pub sink: String,
    pub weights: WeightSet,
}

// ── Native-only: BidGraph → SerializableBidGraph conversion ──────────────────

#[cfg(not(target_arch = "wasm32"))]
impl SerializableBidGraph {
    /// Build from a petgraph `BidGraph`, using node BIDs as the stable identifiers.
    pub fn from_bid_graph(graph: &crate::beliefbase::BidGraph) -> Self {
        let g = graph.as_graph();
        let edges = g
            .raw_edges()
            .iter()
            .map(|e: &petgraph::graph::Edge<WeightSet>| {
                let source_bid = g[e.source()];
                let sink_bid = g[e.target()];
                SerializableEdge {
                    source: source_bid.to_string(),
                    sink: sink_bid.to_string(),
                    weights: e.weight.clone(),
                }
            })
            .collect();
        Self { edges }
    }
}
