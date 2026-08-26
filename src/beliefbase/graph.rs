//! Graph data structures for representing belief relationships.
//!
//! This module provides the core graph types used throughout the belief system:
//! - [`BidGraph`]: Owned graph with WeightSet edges
//! - [`BidRefGraph`]: Borrowed graph with &WeightSet edges
//! - [`BeliefGraph`]: Combined states and relations for serialization/queries

use crate::query::spec::QueryPackage;
#[cfg(not(target_arch = "wasm32"))]
use crate::query::{BeliefSource, BoxFuture, SubmapResult};
#[cfg(not(target_arch = "wasm32"))]
use crate::BuildonomyError;
use crate::{
    event::{BeliefEvent, EventOrigin},
    properties::{BeliefKind, BeliefNode, Bid, WeightKind, WeightSet, WEIGHT_SORT_KEY},
};
use enumset::EnumSet;
// ---------------------------------------------------------------------------
// MergeOp — typed output of to_event_stream
// ---------------------------------------------------------------------------

/// Which side wins when both the merge target (`lhs`) and the incoming graph (`rhs`)
/// hold a non-[`BeliefKind::Trace`] copy of the same node.
///
/// Applies to node states only — see [`BeliefGraph::to_event_stream_with`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergePrecedence {
    /// Keep `lhs`'s copy unless it is `Trace`. The historical default: `lhs` is the
    /// accumulating base and `rhs` is a partial view whose non-seed nodes are
    /// Trace-coloured scaffolding.
    LhsWins,
    /// Take `rhs`'s copy. For callers whose `rhs` is the more authoritative source —
    /// e.g. a per-document seed queried fresh from `global_bb`, merged onto a shared
    /// session base that accumulates across an epoch and is the likelier of the two
    /// to hold a stale node.
    RhsWins,
}

/// A single operation produced by [`BeliefGraph::to_event_stream`] for consumption
/// by [`super::BeliefBase::merge_graph_mut`].
///
/// Using a typed enum instead of [`BeliefEvent`] lets `merge_graph_mut` apply node
/// upserts directly via `self.states.insert` — **no TOML round-trip, no per-node
/// `index_sync`**.  The index is marked dirty exactly once after all node ops are
/// applied, and relation ops are then driven through `update_relation` (which uses
/// the by-then-rebuilt index) and `process_event_queue` (which only needs
/// `BeliefEvent` refs for the PathMapMap).
#[derive(Debug, Clone)]
pub enum MergeOp {
    /// Insert or replace a node directly into `states` (lhs-wins / Trace-downgrade
    /// semantics already applied by `to_event_stream`).
    NodeUpsert(BeliefNode),
    /// Add or update an edge in the relations graph.
    RelationUpdate(Bid, Bid, WeightSet),
}

impl MergeOp {
    /// Convert to the equivalent [`BeliefEvent`] for callers that need one (e.g.
    /// `process_event_queue`).  Only `NodeUpsert` and `RelationUpdate` variants are
    /// produced, so only those two conversions are needed.
    pub fn to_belief_event(&self) -> BeliefEvent {
        match self {
            MergeOp::NodeUpsert(node) => {
                BeliefEvent::NodeUpsert(node.bid, node.clone(), EventOrigin::Remote)
            }
            MergeOp::RelationUpdate(source, sink, weight_set) => {
                BeliefEvent::RelationUpdate(*source, *sink, weight_set.clone(), EventOrigin::Remote)
            }
        }
    }
}
use petgraph::{
    graphmap::GraphMap,
    visit::{depth_first_search, Control, DfsEvent, EdgeRef, IntoEdgeReferences},
    Directed, Direction, IntoWeightedEdge,
};
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use std::{
    collections::{hash_map::Entry as HashEntry, BTreeMap, BTreeSet, HashSet},
    fmt,
    ops::{Deref, DerefMut},
};

use super::BeliefBase;

/// Edge weight carried by a [`BidSubGraph`]: `(sort_key, doc_paths)`.
pub type SubGraphWeight = (u16, Vec<String>);

/// An edge as consumed by [`BidSubGraph::from_edges`]: `(source, sink, weight)`.
pub type SubGraphEdge = (Bid, Bid, SubGraphWeight);

pub type BidSubGraph = GraphMap<Bid, SubGraphWeight, Directed>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BidGraph(pub petgraph::stable_graph::StableGraph<Bid, WeightSet>);

impl Default for BidGraph {
    fn default() -> Self {
        BidGraph(petgraph::stable_graph::StableGraph::new())
    }
}

impl BidGraph {
    pub fn as_graph(&self) -> &petgraph::stable_graph::StableGraph<Bid, WeightSet> {
        &self.0
    }

    pub fn as_graph_mut(&mut self) -> &mut petgraph::stable_graph::StableGraph<Bid, WeightSet> {
        &mut self.0
    }

    pub fn from_edges<I>(iterable: I) -> Self
    where
        I: IntoIterator,
        I::Item: IntoWeightedEdge<WeightSet, NodeId = Bid>,
    {
        let mut graph = petgraph::stable_graph::StableGraph::new();
        let mut bid_to_index = BTreeMap::new();
        let edges = iterable
            .into_iter()
            .map(|edge| edge.into_weighted_edge())
            .collect::<Vec<(Bid, Bid, WeightSet)>>();

        for (source, sink, _) in edges.iter() {
            for bid in [source, sink] {
                if !bid_to_index.contains_key(bid) {
                    let index = graph.add_node(*bid);
                    bid_to_index.insert(*bid, index);
                }
            }
        }

        for (source, sink, weight) in edges {
            let source_idx = bid_to_index[&source];
            let sink_idx = bid_to_index[&sink];
            graph.add_edge(source_idx, sink_idx, weight);
        }

        BidGraph(graph)
    }

    pub fn retain<F: FnMut(&Bid, &Bid, &WeightSet) -> bool>(&mut self, mut f: F) {
        let to_remove = self
            .as_graph()
            .edge_indices()
            .filter(|edge_idx| {
                if let Some((source_idx, sink_idx)) = self.as_graph().edge_endpoints(*edge_idx) {
                    let source = self.as_graph()[source_idx];
                    let sink = self.as_graph()[sink_idx];
                    let weight = &self.as_graph()[*edge_idx];
                    !f(&source, &sink, weight)
                } else {
                    false
                }
            })
            .collect::<Vec<_>>();

        for edge_idx in to_remove {
            self.as_graph_mut().remove_edge(edge_idx);
        }
    }

    pub fn as_subgraph(&self, kind: WeightKind, reverse: bool) -> BidSubGraph {
        let edges = self.as_graph().edge_references().filter_map(|edge_ref| {
            let source = self.as_graph()[edge_ref.source()];
            let sink = self.as_graph()[edge_ref.target()];
            let weight = edge_ref.weight().get(&kind);
            weight.map(|w| {
                let paths: Vec<String> = w.get_doc_paths();
                let sort_key: u16 = w.get(WEIGHT_SORT_KEY).unwrap_or(0);
                if reverse {
                    (sink, source, (sort_key, paths))
                } else {
                    (source, sink, (sort_key, paths))
                }
            })
        });
        BidSubGraph::from_edges(edges)
    }

    /// Like [`Self::as_subgraph`] but DFS-bounded: only edges reachable from `seed` are
    /// included.  Cost is O(reachable nodes + reachable edges) rather than
    /// O(all_edges_in_graph), which matters when the full `BidGraph` contains
    /// thousands of unrelated networks (e.g. `global_bb` after many drain epochs).
    ///
    /// `reverse` has the same semantics as in [`Self::as_subgraph`]: when `true` the edge
    /// direction is flipped so the DFS walks from children toward parents (used by
    /// [`crate::paths::pathmap::PathMap::new`] which starts from the network root and walks downward in the
    /// reversed graph).
    ///
    /// Returns an empty subgraph when `seed` is not present in the graph.
    ///
    /// Prefer [`Self::as_subgraph_seeded_indexed`] when calling this in a loop over
    /// many seeds against the same graph: this method resolves `seed` by scanning
    /// every node, which is O(all nodes) *per call*.
    pub fn as_subgraph_seeded(&self, kind: WeightKind, reverse: bool, seed: Bid) -> BidSubGraph {
        let g = self.as_graph();
        let seed_idx = match g.node_indices().find(|idx| g[*idx] == seed) {
            Some(idx) => idx,
            None => {
                return BidSubGraph::from_edges(std::iter::empty::<(Bid, Bid, (u16, Vec<String>))>())
            }
        };
        self.subgraph_from_seed_idx(kind, reverse, seed_idx)
    }

    /// [`Self::as_subgraph_seeded`] with the Bid→NodeIndex lookup supplied by the caller.
    ///
    /// `StableGraph` is NodeIndex-keyed, not Bid-keyed, so resolving a seed Bid requires
    /// an index. Building that index costs O(all nodes), which is wasteful when the same
    /// graph is seeded repeatedly — `PathMapMap::new` does exactly that, once per network.
    /// Callers in that position should build the map once and pass it here.
    ///
    /// This was O(networks × graph size) and measured as 95.5% of per-task epoch
    /// seeding cost on a large corpus.
    pub fn as_subgraph_seeded_indexed(
        &self,
        kind: WeightKind,
        reverse: bool,
        seed: Bid,
        bid_to_idx: &FxHashMap<Bid, petgraph::stable_graph::NodeIndex>,
    ) -> BidSubGraph {
        let seed_idx = match bid_to_idx.get(&seed) {
            Some(idx) => *idx,
            None => {
                return BidSubGraph::from_edges(std::iter::empty::<(Bid, Bid, (u16, Vec<String>))>())
            }
        };
        self.subgraph_from_seed_idx(kind, reverse, seed_idx)
    }

    fn subgraph_from_seed_idx(
        &self,
        kind: WeightKind,
        reverse: bool,
        seed_idx: petgraph::stable_graph::NodeIndex,
    ) -> BidSubGraph {
        let g = self.as_graph();

        // Section edges in BidGraph go child → parent (source=child, sink=parent).
        // `as_subgraph_seeded(kind, reverse=true, seed=net)` is called from PathMap::new
        // where `net` is the network root.  To collect all descendants of `net` we must
        // walk Incoming edges from `net` (i.e., all children that point to it), then
        // their children, etc.  When reverse=false we want descendants via Outgoing edges.
        //
        // We perform a manual BFS/stack walk using edges_directed so we control direction
        // without needing the Reversed adaptor (which complicates the depth_first_search
        // call signature).  Correctness: the edge-filter pass below still guarantees only
        // `kind` edges appear in the result regardless of how wide the reachable set is.
        let walk_direction = if reverse {
            Direction::Incoming
        } else {
            Direction::Outgoing
        };

        let mut reachable: BTreeSet<petgraph::stable_graph::NodeIndex> = BTreeSet::new();
        let mut stack_walk: Vec<petgraph::stable_graph::NodeIndex> = vec![seed_idx];
        reachable.insert(seed_idx);
        while let Some(current) = stack_walk.pop() {
            for neighbor in g.neighbors_directed(current, walk_direction) {
                if reachable.insert(neighbor) {
                    stack_walk.push(neighbor);
                }
            }
        }

        // Collect `kind` edges between reachable nodes by walking outward from the
        // reachable set, rather than scanning every edge in the graph and filtering.
        // Cost becomes O(reachable edges) instead of O(all edges) — the second half of
        // the full-graph-scan fix.
        //
        // Edge *order* is load-bearing: `BidSubGraph` is a `GraphMap`, whose node and
        // edge iteration order follows insertion, and `PathMap::new` runs a DFS over the
        // result. The previous implementation inserted in `edge_references()` order,
        // which for `StableGraph` is ascending edge index. `edges_directed` instead
        // yields per-node in reverse insertion order, so we tag each edge with its index
        // and sort to reproduce the original sequence exactly.
        //
        // On `Direction::Outgoing` (note: *not* `walk_direction`): this iteration is
        // about enumerating each candidate edge exactly once, which is a separate
        // concern from the direction the reachability walk took. Every edge has exactly
        // one source, so visiting outgoing edges of every reachable node yields each
        // edge with a reachable source once, and the `target` check below completes the
        // old filter's `reachable.contains(source) && reachable.contains(target)`
        // predicate.
        //
        // Using `walk_direction` here would in fact produce the same edge *set* —
        // `reachable` is closed under `walk_direction`, so an edge touching a reachable
        // node in that direction has both endpoints reachable either way — but it would
        // then be `edge_ref.target()` that is trivially `node_idx` under `Incoming`,
        // making the guard below a no-op and the pairing subtly wrong. Enumerating from
        // the source keeps the guard meaningful in both modes.
        //
        // `reverse` is honoured in the two places it belongs: `walk_direction` above
        // (which nodes are reachable) and the source/sink swap below (edge orientation).
        //
        // `test_subgraph_seeded_matches_reference_implementation` pins all of this
        // against the pre-Issue-103 algorithm; hardcoding the opposite direction here
        // makes it fail.
        let mut indexed_edges: Vec<(usize, SubGraphEdge)> = Vec::new();
        for node_idx in reachable.iter().copied() {
            for edge_ref in g.edges_directed(node_idx, Direction::Outgoing) {
                if !reachable.contains(&edge_ref.target()) {
                    continue;
                }
                let Some(weight) = edge_ref.weight().get(&kind) else {
                    continue;
                };
                let source = g[edge_ref.source()];
                let sink = g[edge_ref.target()];
                let paths: Vec<String> = weight.get_doc_paths();
                let sort_key: u16 = weight.get(WEIGHT_SORT_KEY).unwrap_or(0);
                let entry = if reverse {
                    (sink, source, (sort_key, paths))
                } else {
                    (source, sink, (sort_key, paths))
                };
                indexed_edges.push((edge_ref.id().index(), entry));
            }
        }
        indexed_edges.sort_by_key(|(edge_idx, _)| *edge_idx);
        let edges = indexed_edges.into_iter().map(|(_, entry)| entry);
        BidSubGraph::from_edges(edges)
    }

    pub fn sink_subgraph(&self, start_node: Bid, kind: WeightKind) -> BTreeSet<Bid> {
        let subgraph = self.as_subgraph(kind, false);
        let mut subtree_nodes = BTreeSet::new();
        if subgraph.contains_node(start_node) {
            depth_first_search(&subgraph, Some(start_node), |event| {
                if let DfsEvent::Discover(bid, _) = event {
                    subtree_nodes.insert(bid);
                }
            });
        }
        subtree_nodes
    }

    pub fn source_subgraph(&self, start_node: Bid, kind: WeightKind) -> BTreeSet<Bid> {
        let subgraph = self.as_subgraph(kind, true); // REVERSED
        let mut subtree_nodes = BTreeSet::new();
        if subgraph.contains_node(start_node) {
            depth_first_search(&subgraph, Some(start_node), |event| {
                if let DfsEvent::Discover(bid, _) = event {
                    subtree_nodes.insert(bid);
                }
            });
        }
        subtree_nodes
    }
}

impl From<BidRefGraph<'_>> for BidGraph {
    fn from(ref_graph: BidRefGraph<'_>) -> Self {
        BidGraph::from_edges(ref_graph.as_graph().raw_edges().iter().map(|edge| {
            let source = ref_graph.as_graph()[edge.source()];
            let sink = ref_graph.as_graph()[edge.target()];
            (source, sink, edge.weight.clone())
        }))
    }
}

#[derive(Debug, Clone, Default)]
pub struct BidRefGraph<'a>(pub petgraph::Graph<Bid, &'a WeightSet>);

impl<'a> BidRefGraph<'a> {
    pub fn from_edges<I>(iterable: I) -> Self
    where
        I: IntoIterator,
        I::Item: IntoWeightedEdge<&'a WeightSet, NodeId = Bid>,
    {
        let mut graph = petgraph::Graph::new();
        let mut bid_to_index = BTreeMap::new();
        let edges = iterable
            .into_iter()
            .map(|edge| edge.into_weighted_edge())
            .collect::<Vec<(Bid, Bid, &WeightSet)>>();

        for (source, sink, _) in edges.iter() {
            for bid in [source, sink] {
                if !bid_to_index.contains_key(bid) {
                    let index = graph.add_node(*bid);
                    bid_to_index.insert(*bid, index);
                }
            }
        }

        for (source, sink, weight) in edges {
            let source_idx = bid_to_index[&source];
            let sink_idx = bid_to_index[&sink];
            graph.add_edge(source_idx, sink_idx, weight);
        }

        BidRefGraph(graph)
    }

    pub fn as_graph(&self) -> &petgraph::Graph<Bid, &'a WeightSet> {
        &self.0
    }

    pub fn as_graph_mut(&mut self) -> &mut petgraph::Graph<Bid, &'a WeightSet> {
        &mut self.0
    }

    pub fn retain<F: FnMut(&Bid, &Bid, &WeightSet) -> bool>(&mut self, mut f: F) {
        let to_remove = self
            .as_graph()
            .edge_indices()
            .filter(|edge_idx| {
                if let Some((source_idx, sink_idx)) = self.as_graph().edge_endpoints(*edge_idx) {
                    let source = self.as_graph()[source_idx];
                    let sink = self.as_graph()[sink_idx];
                    let weight = &self.as_graph()[*edge_idx];
                    !f(&source, &sink, weight)
                } else {
                    false
                }
            })
            .collect::<Vec<_>>();

        for edge_idx in to_remove {
            self.as_graph_mut().remove_edge(edge_idx);
        }
    }
}

impl<'a> Deref for BidRefGraph<'a> {
    type Target = petgraph::Graph<Bid, &'a WeightSet>;
    fn deref(&self) -> &petgraph::Graph<Bid, &'a WeightSet> {
        &self.0
    }
}

impl<'a> DerefMut for BidRefGraph<'a> {
    fn deref_mut(&mut self) -> &mut petgraph::Graph<Bid, &'a WeightSet> {
        &mut self.0
    }
}

/// Used for Serialization/Deserialization of `BeliefBase`s as well as for returning `BeliefSource`
/// query results.
///
/// `states` uses `FxHashMap` (not `BTreeMap`) — pure keyed lookup, no order dependency (see
/// Issue 101). Note: `#[derive(Serialize)]` therefore no longer emits `states` keys in sorted
/// order when this is written to `beliefbase.json`/msgpack — this is a cosmetic wire-format
/// change only (JSON/msgpack map key order is not semantically meaningful, and these export
/// artifacts are ephemeral/regenerated per build, not diffed byte-for-byte).
#[derive(Debug, Default, Clone, Deserialize, Serialize)]
pub struct BeliefGraph {
    pub states: FxHashMap<Bid, BeliefNode>,
    pub relations: BidGraph,
}

impl BeliefGraph {
    pub fn is_empty(&self) -> bool {
        self.states.is_empty() && self.relations.as_graph().node_count() == 0
    }

    pub fn display_contents(&self) -> String {
        let edge_refs: Vec<_> = self.relations.as_graph().edge_references().collect();
        let edge_tuple = edge_refs
            .iter()
            .map(|e| {
                let source_b = self.relations.as_graph()[e.source()];
                let sink_b = self.relations.as_graph()[e.target()];
                let source = self
                    .states
                    .get(&source_b)
                    .map(|n| {
                        let mut id_vec = vec![];
                        if !n.title.is_empty() {
                            id_vec.push(n.title.clone());
                        }
                        id_vec.push(n.bid.bref().to_string());
                        id_vec.join(": ")
                    })
                    .unwrap_or(source_b.bref().to_string());
                let sink = self
                    .states
                    .get(&sink_b)
                    .map(|n| {
                        let mut id_vec = vec![n.bid.bref().to_string()];
                        if !n.title.is_empty() {
                            id_vec.push(n.title.clone());
                        }
                        id_vec.join(": ")
                    })
                    .unwrap_or(sink_b.bref().to_string());
                let weights = e
                    .weight()
                    .weights
                    .iter()
                    .map(|(k, v)| {
                        format!(
                            "{}[{}]",
                            k,
                            v.get(crate::properties::WEIGHT_OWNED_BY)
                                .map(|owner: String| match owner.as_str() {
                                    "source" => "+",
                                    "sink" => "-",
                                    _ => "o", // third-party bref owner
                                })
                                .unwrap_or("-")
                        )
                    })
                    .collect::<Vec<String>>()
                    .join(", ");
                (source, sink, weights)
            })
            .collect::<Vec<(String, String, String)>>();
        let source_max_len = edge_tuple
            .iter()
            .max_by(|a, b| a.0.len().cmp(&b.0.len()))
            .map(|elem| elem.0.len())
            .unwrap_or_default();
        let sink_max_len = edge_tuple
            .iter()
            .max_by(|a, b| a.1.len().cmp(&b.1.len()))
            .map(|elem| elem.1.len())
            .unwrap_or_default();
        let edge_display = edge_tuple
            .iter()
            .map(|(source, sink, weights)| {
                format!("{source:>source_max_len$} -> {sink:<sink_max_len$}: {weights}")
            })
            .collect::<Vec<String>>()
            .join("\n- ");

        format!(
            "states:\n- {},\nrelations:\n- {}",
            self.states
                .values()
                .map(|n| format!(
                    "{}; {}",
                    n.keys(None, None, &BeliefBase::default())
                        .iter()
                        .map(|k| k.to_string())
                        .collect::<Vec<String>>()
                        .join(", "),
                    n.kind
                ))
                .collect::<Vec<String>>()
                .join(",\n- "),
            edge_display
        )
    }

    fn add_relations(&mut self, rhs: &BeliefGraph) {
        self.add_relations_seeded(rhs, None);
    }

    /// Like `add_relations`, but restricts the DFS seed set to `seed_bids` rather than
    /// seeding from all of `self.states`. Use this when the caller already knows which rhs
    /// nodes are relevant, avoiding an O(session_bb_size × rhs_edges) scan.
    ///
    /// `seed_bids` are looked up in `rhs.relations` — only seeds that exist in the rhs graph
    /// are used. If `seed_bids` is `None`, behaviour is identical to `add_relations`.
    pub(super) fn add_relations_seeded(
        &mut self,
        rhs: &BeliefGraph,
        seed_bids: Option<&BTreeSet<Bid>>,
    ) {
        let mut bid_to_index: BTreeMap<_, _> = self
            .relations
            .as_graph()
            .node_indices()
            .map(|idx| (self.relations.as_graph()[idx], idx))
            .collect();

        // find all rhs nodes reachable from our lhs set, both upstream and downstream. (clone so we
        // can reverse the graph)
        let mut rhs_relations = rhs.relations.as_graph().clone();
        let rhs_bid_to_index: BTreeMap<_, _> = rhs_relations
            .node_indices()
            .filter_map(|idx| {
                let bid = rhs_relations[idx];
                let in_seed = match seed_bids {
                    // Restricted mode: seed only from the caller-supplied set.
                    Some(seeds) => seeds.contains(&bid),
                    // Unrestricted mode (original behaviour): seed from anything already in self.
                    None => self.states.contains_key(&bid),
                };
                if in_seed {
                    Some((bid, idx))
                } else {
                    None
                }
            })
            .collect();

        for _ in &["forward", "reverse"] {
            let mut explored = BTreeSet::new();
            depth_first_search(
                &rhs_relations,
                rhs_bid_to_index.values().copied().collect::<Vec<_>>(),
                |event| match event {
                    DfsEvent::Discover(sink_idx, _) => {
                        if explored.contains(&sink_idx) {
                            Control::<()>::Prune
                        } else {
                            explored.insert(sink_idx);
                            let sink_bid = rhs_relations[sink_idx];
                            if let Some(sink_node) = rhs.states.get(&sink_bid) {
                                if let HashEntry::Vacant(e) = self.states.entry(sink_bid) {
                                    e.insert(sink_node.clone());
                                } else {
                                    // This is expected for partial graphs, such as Trace-marked halo results.
                                }
                            }
                            Control::Continue
                        }
                    }
                    _ => Control::Continue,
                },
            );
            // Now look upstream
            rhs_relations.reverse();
        }

        // Now, union the relations, only adding nodes that exist in the final state map.
        let rhs_edges: Vec<_> = rhs.relations.as_graph().edge_references().collect();
        for edge_ref in rhs_edges {
            let source = rhs.relations.as_graph()[edge_ref.source()];
            let sink = rhs.relations.as_graph()[edge_ref.target()];
            let edge_weight = edge_ref.weight().clone();

            if source == sink {
                tracing::warn!(
                    "Ignoring self-connection (infinite loop) between [{} - {}] with weights {:?}",
                    source,
                    self.states
                        .get(&source)
                        .map(|n| n.title.as_str())
                        .unwrap_or_default(),
                    edge_weight
                );
                continue;
            }

            // Only add edges for nodes that have a state in the now-merged state map.
            // First, try to fill any missing endpoint from rhs.states.
            if self.states.contains_key(&source) || self.states.contains_key(&sink) {
                if let HashEntry::Vacant(e) = self.states.entry(sink) {
                    if let Some(rhs_state) = rhs.states.get(&sink) {
                        // tracing::debug!(
                        //     "Adding sink {} {} to lhs",
                        //     rhs_state.bid,
                        //     rhs_state.display_title()
                        // );
                        e.insert(rhs_state.clone());
                    }
                }
                if let HashEntry::Vacant(e) = self.states.entry(source) {
                    if let Some(rhs_state) = rhs.states.get(&source) {
                        // tracing::debug!(
                        //     "Adding source {} {} to lhs",
                        //     rhs_state.bid,
                        //     rhs_state.display_title()
                        // );
                        e.insert(rhs_state.clone());
                    }
                }
                // Only insert the edge (and the relation graph nodes for its endpoints) when
                // both endpoints are confirmed present in self.states. Inserting a graph node
                // via add_node without a matching states entry creates an orphaned BID in the
                // relations graph — the root cause of Issue 34 "nodes in relations but not in
                // states" violations. If an endpoint is still absent here it means neither lhs
                // nor rhs carries its state; skip the edge entirely rather than creating an
                // orphan. The edge will be re-added when the missing node is later merged in.
                if self.states.contains_key(&source) && self.states.contains_key(&sink) {
                    let source_idx = *bid_to_index
                        .entry(source)
                        .or_insert_with(|| self.relations.as_graph_mut().add_node(source));
                    let sink_idx = *bid_to_index
                        .entry(sink)
                        .or_insert_with(|| self.relations.as_graph_mut().add_node(sink));
                    self.relations
                        .as_graph_mut()
                        .update_edge(source_idx, sink_idx, edge_weight);
                } else {
                    tracing::debug!(
                        "Skipping edge {} → {}: endpoint(s) absent from both lhs and rhs states \
                         (source_present={}, sink_present={}). Edge will be added when the \
                         missing node is merged.",
                        source,
                        sink,
                        self.states.contains_key(&source),
                        self.states.contains_key(&sink),
                    );
                }
            }
        }
    }

    /// The state set union between lhs and rhs. rhs states are only added when lhs does not contain
    /// that key.
    ///
    /// rhs relations are all added, overwriting lhs if a source+sink combo for that edge was present
    pub fn union(&self, rhs: &BeliefGraph) -> BeliefGraph {
        let mut out = self.clone();
        out.union_mut(rhs);
        out
    }

    pub fn union_mut(&mut self, rhs: &BeliefGraph) {
        // Union the states with the non-trace elements of rhs. rhs wins on conflict so that
        // callers can rely on passing the fresher/more-authoritative graph as rhs to overwrite
        // stale lhs content. This is consistent with edge semantics (update_edge also overwrites).
        for node in rhs.states.values().filter(|node| node.kind.is_complete()) {
            self.states.insert(node.bid, node.clone());
        }
        self.add_relations(rhs);
    }

    /// Like `union_mut`, but restricts the DFS seed set in the relation merge to `seed_bids`.
    ///
    /// Use this when merging a large accumulated `rhs` graph (e.g. `missing_structure` after
    /// processing all relations in a file) into a large `self` (e.g. `session_bb`). Without a
    /// restricted seed the DFS visits O(session_bb_size) nodes per call, making the total cost
    /// across a corpus O(N² × K). By supplying only the BIDs relevant to the current file the
    /// DFS is bounded by O(rhs_size) regardless of how large `self` has grown.
    ///
    /// Correctness contract: `seed_bids` must be a subset of BIDs present in `rhs`. Any rhs
    /// node reachable (forward or backward) from a seed will still be pulled into `self`; only
    /// the starting points of the DFS are narrowed.
    pub fn union_mut_from(&mut self, rhs: &BeliefGraph, seed_bids: &BTreeSet<Bid>) {
        // State merge is identical to union_mut — rhs wins on conflict. Seeds only affect the
        // relation DFS below.
        for node in rhs.states.values().filter(|node| node.kind.is_complete()) {
            self.states.insert(node.bid, node.clone());
        }
        self.add_relations_seeded(rhs, Some(seed_bids));
    }

    /// Union with trace nodes included. Used during traversal where we want to accumulate
    /// nodes even if they're marked as Trace (incomplete relations). rhs wins on conflict,
    /// except that a Trace rhs node never downgrades a complete lhs node.
    pub fn union_mut_with_trace(&mut self, rhs: &BeliefGraph) {
        for node in rhs.states.values() {
            match self.states.entry(node.bid) {
                HashEntry::Vacant(e) => {
                    e.insert(node.clone());
                }
                HashEntry::Occupied(mut e) => {
                    let existing = e.get();
                    // rhs wins unless rhs is Trace and lhs is already complete.
                    if !(node.kind.contains(BeliefKind::Trace) && existing.kind.is_complete()) {
                        *e.get_mut() = node.clone();
                    }
                }
            }
        }
        self.add_relations(rhs);
    }

    /// The (non-trace) state set intersection between lhs and rhs
    pub fn intersection(&self, rhs: &BeliefGraph) -> BeliefGraph {
        let lhs_states = BTreeSet::from_iter(
            self.states
                .values()
                .filter(|n| n.kind.is_complete())
                .map(|n| n.bid),
        );
        let rhs_states = BTreeSet::from_iter(
            rhs.states
                .values()
                .filter(|n| n.kind.is_complete())
                .map(|n| n.bid),
        );
        let mut beliefs = BeliefGraph {
            states: FxHashMap::from_iter(
                lhs_states
                    .intersection(&rhs_states)
                    .filter_map(|bid| self.states.get(bid).map(|n| (n.bid, n.clone()))),
            ),
            relations: BidGraph::default(),
        };
        beliefs.add_relations(self);
        beliefs.add_relations(rhs);
        beliefs
    }

    pub fn intersection_mut(&mut self, rhs: &BeliefGraph) {
        *self = self.intersection(rhs)
    }

    /// The (non-trace) state set difference between lhs and rhs
    pub fn difference(&self, rhs: &BeliefGraph) -> BeliefGraph {
        let lhs_states = BTreeSet::from_iter(
            self.states
                .values()
                .filter(|n| n.kind.is_complete())
                .map(|n| n.bid),
        );
        let rhs_states = BTreeSet::from_iter(
            rhs.states
                .values()
                .filter(|n| n.kind.is_complete())
                .map(|n| n.bid),
        );
        let mut beliefs = BeliefGraph {
            states: FxHashMap::from_iter(
                lhs_states
                    .difference(&rhs_states)
                    .filter_map(|bid| self.states.get(bid).map(|n| (n.bid, n.clone()))),
            ),
            relations: BidGraph::default(),
        };
        beliefs.add_relations(self);
        beliefs.add_relations(rhs);
        beliefs
    }

    pub fn difference_mut(&mut self, rhs: &BeliefGraph) {
        *self = self.difference(rhs);
    }

    pub fn symmetric_difference(&self, rhs: &BeliefGraph) -> BeliefGraph {
        self.difference(rhs).union(&rhs.difference(self))
    }

    pub fn symmetric_difference_mut(&mut self, rhs: &BeliefGraph) {
        *self = self.symmetric_difference(rhs);
    }

    /// Find BIDs referenced in relations but not present in states.
    /// Returns a deduplicated set of orphaned BIDs. Sortedness is an incidental
    /// property of the current `BTreeSet` return type, not a documented
    /// guarantee — no caller relies on iteration order, only membership/dedup.
    pub fn find_orphaned_edges(&self) -> BTreeSet<Bid> {
        let mut missing = BTreeSet::new();
        for node_idx in self.relations.as_graph().node_indices() {
            let bid = self.relations.as_graph()[node_idx];
            if !self.states.contains_key(&bid) {
                missing.insert(bid);
            }
        }
        missing
    }

    /// Find boundary nodes: nodes in the relation graph that have no edges of
    /// the specified weight kinds in the given direction.
    ///
    /// `kinds`: which edge weight kinds to consider. If empty, no edges match
    /// and every node is "external".
    ///
    /// `dir`: `Outgoing` finds nodes with no outgoing edges of the given kinds
    /// (i.e. roots/sinks in that subgraph). `Incoming` finds nodes with no
    /// incoming edges (i.e. leaves/sources).
    ///
    /// `with_orphans`: if true, also includes nodes present in the full relation
    /// graph but absent from the kind-filtered subgraph (they trivially have no
    /// edges of the specified kinds).
    ///
    /// Nodes already in `states` with `BeliefKind::External` are excluded —
    /// External nodes are always partially loaded by design, so re-fetching
    /// them would create an infinite balance loop.
    #[allow(dead_code)] // Retained for potential future use; last caller was to_event_stream.
    fn find_externals(
        &self,
        kinds: EnumSet<WeightKind>,
        dir: Direction,
        with_orphans: bool,
    ) -> BTreeSet<Bid> {
        // Build a filtered subgraph containing only edges that have at least
        // one weight kind in `kinds`.
        let g = self.relations.as_graph();
        let filtered_edges: Vec<_> = g
            .edge_references()
            .filter(|e| e.weight().weights.keys().any(|k| kinds.contains(*k)))
            .map(|e| (g[e.source()], g[e.target()]))
            .collect();
        let filtered: GraphMap<Bid, (), Directed> =
            GraphMap::from_edges(filtered_edges.iter().copied());

        // Find external nodes: those with no edges in `dir`.
        let mut external_bids = BTreeSet::default();
        for node in filtered.nodes() {
            let has_edges = match dir {
                Direction::Outgoing => filtered
                    .edges_directed(node, Direction::Outgoing)
                    .next()
                    .is_some(),
                Direction::Incoming => filtered
                    .edges_directed(node, Direction::Incoming)
                    .next()
                    .is_some(),
            };
            if !has_edges {
                external_bids.insert(node);
            }
        }

        // Orphan handling: nodes present in the full graph but absent from the
        // filtered subgraph (they have no edges of the specified kinds at all).
        if with_orphans {
            let filtered_set: HashSet<Bid> = filtered.nodes().collect();
            for idx in g.node_indices() {
                let bid = g[idx];
                if !filtered_set.contains(&bid) {
                    external_bids.insert(bid);
                }
            }
        }

        // Skip nodes whose Trace status is final — External nodes (href leaf
        // nodes, asset leaf nodes, API root) are always partially loaded by
        // design; no deeper fetch will ever return a non-Trace version.
        external_bids.retain(|bid| {
            !self
                .states
                .get(bid)
                .is_some_and(|n| n.kind.contains(BeliefKind::External))
        });
        external_bids
    }

    /// Convert this `BeliefGraph` (rhs) into an ordered `Vec<BeliefEvent>` suitable for
    /// applying to a `BeliefBase` via `process_event`.
    ///
    /// This replaces the `compute_diff`-based approach in `merge_graph_mut`. Cost is
    /// O(rhs_size) — no clone of session_bb relations, no `PathMapMap::new`.
    ///
    /// **Pass 1 — NodeUpdate events** (lhs-wins semantics):
    /// Emits `NodeUpdate` for every node in `rhs.states` that is absent from `lhs`, or
    /// where `lhs` only has a Trace copy. Nodes already present in `lhs` as complete are
    /// skipped. `insert_state` handles the Trace-overwrite correctly on receipt.
    ///
    /// **Pass 2 — RelationUpdate events** (topological: sink/parent before source/child):
    /// Builds a `BidSubGraph` from Section edges for DFS traversal order, seeded from
    /// `seed_bids` (the halo around freshly-parsed nodes). Section edges are emitted in
    /// `TreeEdge` (sink→source, i.e. parent→child) order. Non-Section edges are emitted
    /// afterward in a raw_edges() scan.
    ///
    /// Sibling ordering within the event stream is not required to be correct —
    /// `process_event_queue` sorts all PathMaps at end of pass 2.
    /// Build the ordered list of `MergeOp`s needed to merge `self` (rhs) into `lhs`.
    ///
    /// The returned ops are suitable for direct application by
    /// `super::BeliefBase::merge_graph_mut`:
    ///
    /// * `MergeOp::NodeUpsert` — node already filtered by lhs-wins / Trace-downgrade;
    ///   apply with `self.states.insert` directly, no TOML round-trip.
    /// * `MergeOp::RelationUpdate` — apply via `update_relation` after all node ops
    ///   and a single `index_dirty` flush.
    ///
    /// Replaces the old `to_event_stream → Vec<BeliefEvent>` signature.  The
    /// per-node `index_sync` that the old path triggered (via `process_event →
    /// insert_state → evaluate_query`) is eliminated; the index is marked dirty
    /// once after the node pass and rebuilt once before the relation pass.
    ///
    /// Uses `QueryPackage::balanced` to compute the scoped graph (halo +
    /// section ancestry with Trace coloring). The tape provides topological +
    /// sibling ordering for edges; lhs-wins dedup filters both nodes and edges.
    pub fn to_event_stream(
        &self,
        lhs: &BeliefBase,
        seed_bids: Option<&BTreeSet<Bid>>,
    ) -> Vec<MergeOp> {
        self.to_event_stream_with(lhs, seed_bids, MergePrecedence::LhsWins)
    }

    /// [`to_event_stream`] with an explicit collision policy for the node pass.
    ///
    /// Only node states are affected. Edges are unconditionally rhs-wins-on-difference
    /// under both policies: the dedup below matches on the exact
    /// `(source, sink, WeightSet)` triple, so an edge whose weight differs is always
    /// emitted and applied by `update_relation`.
    ///
    /// [`to_event_stream`]: BeliefGraph::to_event_stream
    pub fn to_event_stream_with(
        &self,
        lhs: &BeliefBase,
        seed_bids: Option<&BTreeSet<Bid>>,
        precedence: MergePrecedence,
    ) -> Vec<MergeOp> {
        let mut ops = Vec::new();

        // ----------------------------------------------------------------
        // Compute scoped graph: seed + halo + section ancestors, with
        // Trace coloring for non-seed nodes. When seed_bids is None,
        // the entire rhs is in scope.
        // ----------------------------------------------------------------
        let (scoped_graph, tape) = match seed_bids {
            None => (self.clone(), None),
            Some(seeds) => {
                let bb = super::BeliefBase::from(self.clone());
                let spec = crate::query::spec::QuerySpec::seed(crate::query::spec::TapeFn::Bids(
                    seeds.iter().copied().collect(),
                ));
                let mut package = QueryPackage::balanced(spec);
                if let Err(e) = bb.evaluate_query(&mut package) {
                    tracing::warn!("[to_event_stream] evaluate_query failed: {e}");
                    return ops;
                }
                let tape = package.tape().clone();
                let graph = package.into_graph();
                (graph, Some(tape))
            }
        };

        // ----------------------------------------------------------------
        // Pass 1: NodeUpdate events — lhs-wins with Trace downgrade.
        // ----------------------------------------------------------------
        for node in scoped_graph.states.values() {
            let should_emit = match precedence {
                MergePrecedence::LhsWins => match lhs.states().get(&node.bid) {
                    // lhs has a complete copy — lhs wins, skip
                    Some(existing) if !existing.kind.contains(BeliefKind::Trace) => false,
                    // lhs has Trace copy or no copy — emit to overwrite/insert
                    _ => true,
                },
                // rhs wins outright, Trace included. Skip only an exact match, which
                // would be a no-op event.
                MergePrecedence::RhsWins => lhs.states().get(&node.bid) != Some(node),
            };
            if should_emit {
                ops.push(MergeOp::NodeUpsert(node.clone()));
            }
        }

        // ----------------------------------------------------------------
        // Pass 2: RelationUpdate events.
        //
        // When the tape is available, iterate edges in tape order — this
        // gives topological + sibling ordering for Section edges (balance_map
        // traversal records parent-before-child), followed by halo edges.
        //
        // When no tape (seed_bids was None), emit all edges directly.
        //
        // Performance: the lhs-wins dedup check below used to be a linear scan
        // over ALL of lhs's edges per rhs edge (`edge_references().any(...)`),
        // making this pass O(rhs_edges × lhs_edges). That is exactly the
        // O(session_bb_size × rhs_edges) blowup documented on `BeliefBase::merge`
        // (Issue 47 BN-1) — it was fixed there via `merge_from`'s seeded DFS, but
        // this dedup scan runs regardless of whether a seed is supplied, so callers
        // merging repeatedly into a growing lhs (e.g. WASM shard loading in
        // `wasm::load_shard`, which always passes `seed_bids: None`) still pay it.
        // Precomputing a hash set of lhs's (source, sink, weight) triples once
        // turns each dedup check into an O(1) lookup, making the whole pass
        // O(lhs_edges + rhs_edges) instead of O(rhs_edges × lhs_edges).
        // ----------------------------------------------------------------
        let scoped_g = scoped_graph.relations.as_graph();

        let lhs_edge_set: HashSet<(Bid, Bid, WeightSet)> = {
            let lhs_rel = lhs.relations();
            let lhs_g = lhs_rel.as_graph();
            lhs_g
                .edge_references()
                .map(|le| (lhs_g[le.source()], lhs_g[le.target()], le.weight().clone()))
                .collect()
        };

        // Dedup: track emitted (source, sink) pairs.
        let mut emitted: BTreeSet<(Bid, Bid)> = BTreeSet::new();

        if let Some(ref tape) = tape {
            // Emit edges in tape order — topological + sibling ordered.
            for entry in &tape.steps {
                if let Some(edge_indices) = entry.content.edges() {
                    for &eidx in edge_indices {
                        let Some((src_idx, snk_idx)) = scoped_g.edge_endpoints(eidx) else {
                            continue;
                        };
                        let source = scoped_g[src_idx];
                        let sink = scoped_g[snk_idx];
                        let Some(weight) = scoped_g.edge_weight(eidx) else {
                            continue;
                        };
                        if emitted.contains(&(source, sink)) {
                            continue;
                        }
                        // lhs-wins: skip if lhs already has identical edge.
                        let already_present =
                            lhs_edge_set.contains(&(source, sink, weight.clone()));
                        if already_present {
                            emitted.insert((source, sink));
                            continue;
                        }
                        emitted.insert((source, sink));
                        ops.push(MergeOp::RelationUpdate(source, sink, weight.clone()));
                    }
                }
            }
        }

        // Emit any remaining edges not covered by the tape (non-tape path,
        // or edges in scoped_graph not recorded in tape entries).
        for edge_ref in scoped_g.edge_references() {
            let source = scoped_g[edge_ref.source()];
            let sink = scoped_g[edge_ref.target()];
            if emitted.contains(&(source, sink)) {
                continue;
            }
            // Both endpoints must be in scope.
            if !scoped_graph.states.contains_key(&source)
                || !scoped_graph.states.contains_key(&sink)
            {
                continue;
            }
            let edge_weight = edge_ref.weight().clone();
            let already_present = lhs_edge_set.contains(&(source, sink, edge_weight.clone()));
            if already_present {
                continue;
            }
            emitted.insert((source, sink));
            ops.push(MergeOp::RelationUpdate(source, sink, edge_weight));
        }

        ops
    }
}

impl PartialEq for BeliefGraph {
    fn eq(&self, other: &Self) -> bool {
        let lhs_states = BTreeSet::from_iter(self.states.keys().copied());
        let rhs_states = BTreeSet::from_iter(other.states.keys().copied());

        let intersection_count = lhs_states.intersection(&rhs_states).count();
        self.states.len() == intersection_count
    }
}

impl From<&BeliefBase> for BeliefGraph {
    fn from(beliefbase: &BeliefBase) -> Self {
        beliefbase.clone().consume()
    }
}

// ---------------------------------------------------------------------------
// BeliefSource — query model API for BeliefGraph
// ---------------------------------------------------------------------------
//
// Provides the `BeliefSource` API on a `BeliefGraph`, enabling callers to
// run `QuerySpec` evaluations against a pre-materialized graph without manually
// converting to `BeliefBase`.  `export_beliefgraph` operates directly on the
// graph fields.  Expensive operations (`evaluate`, `submap`, `submap_by_bid`)
// convert to a temporary `BeliefBase` — this is O(N+E) per call, so callers
// with repeated query needs should convert once and query the `BeliefBase`
// directly.  For single-node lookup or edge fetching, use the free functions
// `lookup_node` and `lookup_edges` in `crate::query`.

#[cfg(not(target_arch = "wasm32"))]
impl BeliefSource for BeliefGraph {
    fn evaluate<'a>(
        &'a self,
        package: &'a mut QueryPackage,
    ) -> BoxFuture<'a, Result<(), BuildonomyError>> {
        Box::pin(async move {
            let bb = BeliefBase::from(self.clone());
            bb.evaluate_query(package)
        })
    }

    fn submap<'a>(
        &'a self,
        network_bid: Bid,
        path: &'a str,
        depth: u8,
        include_index: bool,
    ) -> BoxFuture<'a, SubmapResult> {
        Box::pin(async move {
            let bb = BeliefBase::from(self.clone());
            Ok(bb
                .paths()
                .submap(&network_bid.bref(), path, depth, include_index))
        })
    }

    fn submap_by_bid<'a>(
        &'a self,
        network_bid: Bid,
        entry: Option<Bid>,
        depth: u8,
        include_index: bool,
    ) -> BoxFuture<'a, SubmapResult> {
        Box::pin(async move {
            let bb = BeliefBase::from(self.clone());
            Ok(bb
                .paths()
                .submap_by_bid(&network_bid.bref(), entry, depth, include_index))
        })
    }

    fn export_beliefgraph(&self) -> BoxFuture<'_, Result<BeliefGraph, BuildonomyError>> {
        Box::pin(async move { Ok(self.clone()) })
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl BeliefSource for &BeliefGraph {
    fn evaluate<'a>(
        &'a self,
        package: &'a mut QueryPackage,
    ) -> BoxFuture<'a, Result<(), BuildonomyError>> {
        Box::pin(async move { (*self).evaluate(package).await })
    }

    fn submap<'a>(
        &'a self,
        network_bid: Bid,
        path: &'a str,
        depth: u8,
        include_index: bool,
    ) -> BoxFuture<'a, SubmapResult> {
        Box::pin(async move {
            (*self)
                .submap(network_bid, path, depth, include_index)
                .await
        })
    }

    fn submap_by_bid<'a>(
        &'a self,
        network_bid: Bid,
        entry: Option<Bid>,
        depth: u8,
        include_index: bool,
    ) -> BoxFuture<'a, SubmapResult> {
        Box::pin(async move {
            (*self)
                .submap_by_bid(network_bid, entry, depth, include_index)
                .await
        })
    }

    fn export_beliefgraph(&self) -> BoxFuture<'_, Result<BeliefGraph, BuildonomyError>> {
        Box::pin(async move { Ok((*self).clone()) })
    }
}

impl fmt::Display for BeliefGraph {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.display_contents())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::properties::{
        BeliefKind, BeliefKindSet, BeliefNode, Weight, WeightKind, WeightSet, WEIGHT_SORT_KEY,
    };
    use crate::query::spec::{leaves, roots, QueryPackage, QuerySpec, TapeFn};
    use crate::query::BeliefSource;

    // -------------------------------------------------------------------------
    // Helpers
    // -------------------------------------------------------------------------

    fn make_node(bid: Bid, title: &str, kind: BeliefKind) -> BeliefNode {
        BeliefNode {
            bid,
            title: title.to_string(),
            kind: BeliefKindSet(kind.into()),
            ..Default::default()
        }
    }

    fn make_weights(sort_key: u16) -> WeightSet {
        let mut w = Weight::default();
        w.set(WEIGHT_SORT_KEY, sort_key).ok();
        let mut ws = WeightSet::empty();
        ws.set(WeightKind::Section, w);
        ws
    }

    /// Build a BeliefGraph from a node list and an edge list (source, sink, sort_key).
    fn make_graph(nodes: Vec<BeliefNode>, edges: Vec<(Bid, Bid, u16)>) -> BeliefGraph {
        let states: FxHashMap<Bid, BeliefNode> = nodes.iter().map(|n| (n.bid, n.clone())).collect();
        let relations = BidGraph::from_edges(
            edges
                .into_iter()
                .map(|(src, snk, sk)| (src, snk, make_weights(sk))),
        );
        BeliefGraph { states, relations }
    }

    /// Extract the sort_key for the single edge (source→sink) in `g`, panicking if absent.
    fn edge_sort_key(g: &BeliefGraph, source: Bid, sink: Bid) -> Option<u16> {
        g.relations.as_graph().edge_references().find_map(|e| {
            let s = g.relations.as_graph()[e.source()];
            let t = g.relations.as_graph()[e.target()];
            if s == source && t == sink {
                e.weight().get(&WeightKind::Section)?.get(WEIGHT_SORT_KEY)
            } else {
                None
            }
        })
    }

    // -------------------------------------------------------------------------
    // as_subgraph_seeded edge collection
    // -------------------------------------------------------------------------

    /// Reference implementation of the pre-subgraph_from_seed_idx algorithm: full-graph
    /// index build, then a scan over *every* edge filtered to the reachable set. The
    /// optimised version must agree with this exactly.
    fn reference_subgraph_seeded(
        bg: &BidGraph,
        kind: WeightKind,
        reverse: bool,
        seed: Bid,
    ) -> Vec<(Bid, Bid, (u16, Vec<String>))> {
        let g = bg.as_graph();
        let bid_to_idx: BTreeMap<Bid, petgraph::stable_graph::NodeIndex> =
            g.node_indices().map(|idx| (g[idx], idx)).collect();
        let Some(&seed_idx) = bid_to_idx.get(&seed) else {
            return Vec::new();
        };
        let walk_direction = if reverse {
            Direction::Incoming
        } else {
            Direction::Outgoing
        };
        let mut reachable: BTreeSet<petgraph::stable_graph::NodeIndex> = BTreeSet::new();
        let mut stack = vec![seed_idx];
        reachable.insert(seed_idx);
        while let Some(current) = stack.pop() {
            for neighbor in g.neighbors_directed(current, walk_direction) {
                if reachable.insert(neighbor) {
                    stack.push(neighbor);
                }
            }
        }
        g.edge_references()
            .filter_map(|edge_ref| {
                if !reachable.contains(&edge_ref.source())
                    || !reachable.contains(&edge_ref.target())
                {
                    return None;
                }
                let source = g[edge_ref.source()];
                let sink = g[edge_ref.target()];
                let weight = edge_ref.weight().get(&kind)?;
                let paths: Vec<String> = weight.get_doc_paths();
                let sort_key: u16 = weight.get(WEIGHT_SORT_KEY).unwrap_or(0);
                if reverse {
                    Some((sink, source, (sort_key, paths)))
                } else {
                    Some((source, sink, (sort_key, paths)))
                }
            })
            .collect()
    }

    /// Flatten a BidSubGraph into a comparable edge list, preserving iteration order
    /// (GraphMap iterates in insertion order, and PathMap::new DFSes over the result,
    /// so order is part of the contract — not just set equality).
    fn subgraph_edges(sg: &BidSubGraph) -> Vec<(Bid, Bid, (u16, Vec<String>))> {
        sg.all_edges().map(|(s, t, w)| (s, t, w.clone())).collect()
    }

    /// A graph with several networks, mixed edge kinds, a disjoint component, and a
    /// cycle — the shapes that could break seeded subgraph extraction.
    fn multi_net_fixture() -> (BidGraph, Vec<Bid>) {
        let net_a = Bid::new(Bid::nil());
        let net_b = Bid::new(Bid::nil());
        let a1 = Bid::new(net_a);
        let a2 = Bid::new(net_a);
        let a3 = Bid::new(net_a);
        let b1 = Bid::new(net_b);
        let b2 = Bid::new(net_b);

        let section = make_weights(0);
        let mut epistemic = WeightSet::empty();
        {
            let mut w = Weight::default();
            w.set(WEIGHT_SORT_KEY, 7).ok();
            epistemic.set(WeightKind::Epistemic, w);
        }

        let bg = BidGraph::from_edges(vec![
            // Network A subtree (Section)
            (a1, net_a, make_weights(0)),
            (a2, net_a, make_weights(1)),
            (a3, a1, make_weights(0)),
            // A non-Section edge between reachable nodes: must be excluded by kind
            (a2, a1, epistemic.clone()),
            // A cycle within A
            (a1, a3, make_weights(2)),
            // Disjoint network B
            (b1, net_b, section.clone()),
            (b2, net_b, make_weights(1)),
        ]);
        (bg, vec![net_a, net_b, a1, a2, a3, b1, b2])
    }

    #[test]
    fn test_subgraph_seeded_matches_reference_implementation() {
        let (bg, bids) = multi_net_fixture();
        for &seed in &bids {
            for kind in [WeightKind::Section, WeightKind::Epistemic] {
                for reverse in [true, false] {
                    let expected = reference_subgraph_seeded(&bg, kind, reverse, seed);
                    let actual = subgraph_edges(&bg.as_subgraph_seeded(kind, reverse, seed));
                    assert_eq!(
                        actual, expected,
                        "mismatch for seed={seed} kind={kind:?} reverse={reverse}"
                    );
                }
            }
        }
    }

    #[test]
    fn test_subgraph_seeded_indexed_matches_unindexed() {
        let (bg, bids) = multi_net_fixture();
        let g = bg.as_graph();
        let bid_to_idx: FxHashMap<Bid, petgraph::stable_graph::NodeIndex> =
            g.node_indices().map(|idx| (g[idx], idx)).collect();

        for &seed in &bids {
            for reverse in [true, false] {
                let unindexed =
                    subgraph_edges(&bg.as_subgraph_seeded(WeightKind::Section, reverse, seed));
                let indexed = subgraph_edges(&bg.as_subgraph_seeded_indexed(
                    WeightKind::Section,
                    reverse,
                    seed,
                    &bid_to_idx,
                ));
                assert_eq!(
                    indexed, unindexed,
                    "indexed/unindexed mismatch for seed={seed} reverse={reverse}"
                );
            }
        }
    }

    #[test]
    fn test_subgraph_seeded_disjoint_network_excludes_other_component() {
        let (bg, bids) = multi_net_fixture();
        let (net_a, net_b) = (bids[0], bids[1]);
        let (b1, b2) = (bids[5], bids[6]);

        let a_edges = subgraph_edges(&bg.as_subgraph_seeded(WeightKind::Section, true, net_a));
        for (s, t, _) in &a_edges {
            assert!(
                *s != b1 && *t != b1 && *s != b2 && *t != b2 && *s != net_b && *t != net_b,
                "network A's subgraph leaked a node from disjoint network B"
            );
        }
        assert!(!a_edges.is_empty(), "network A should have Section edges");
    }

    #[test]
    fn test_subgraph_seeded_missing_seed_is_empty() {
        let (bg, _) = multi_net_fixture();
        let absent = Bid::new(Bid::nil());
        assert!(bg
            .as_subgraph_seeded(WeightKind::Section, true, absent)
            .all_edges()
            .next()
            .is_none());

        let g = bg.as_graph();
        let bid_to_idx: FxHashMap<Bid, petgraph::stable_graph::NodeIndex> =
            g.node_indices().map(|idx| (g[idx], idx)).collect();
        assert!(bg
            .as_subgraph_seeded_indexed(WeightKind::Section, true, absent, &bid_to_idx)
            .all_edges()
            .next()
            .is_none());
    }

    // -------------------------------------------------------------------------
    // T1: Idempotency — union_mut(A, A) == A
    // -------------------------------------------------------------------------
    #[test]
    fn test_union_mut_idempotent() {
        let net = Bid::new(Bid::nil());
        let x = Bid::new(net);
        let y = Bid::new(net);

        let a = make_graph(
            vec![
                make_node(net, "Net", BeliefKind::Network),
                make_node(x, "X", BeliefKind::Document),
                make_node(y, "Y", BeliefKind::Document),
            ],
            vec![(x, net, 0), (y, net, 1)],
        );

        let mut result = a.clone();
        result.union_mut(&a);

        assert_eq!(result.states.len(), a.states.len(), "state count unchanged");
        assert_eq!(
            result.relations.as_graph().edge_count(),
            a.relations.as_graph().edge_count(),
            "edge count unchanged"
        );
        assert_eq!(edge_sort_key(&result, x, net), Some(0));
        assert_eq!(edge_sort_key(&result, y, net), Some(1));
    }

    // -------------------------------------------------------------------------
    // T2: Disjoint state sets are commutative (first-writer-wins is moot when
    //     there is no conflict — documents the ownership invariant happy path).
    // -------------------------------------------------------------------------
    #[test]
    fn test_union_mut_disjoint_states_commutative() {
        let net = Bid::new(Bid::nil());
        let x = Bid::new(net);
        let y = Bid::new(net);

        let a = make_graph(vec![make_node(x, "X", BeliefKind::Document)], vec![]);
        let b = make_graph(vec![make_node(y, "Y", BeliefKind::Document)], vec![]);

        let mut r1 = BeliefGraph::default();
        r1.union_mut(&a);
        r1.union_mut(&b);

        let mut r2 = BeliefGraph::default();
        r2.union_mut(&b);
        r2.union_mut(&a);

        assert_eq!(r1.states.len(), r2.states.len());
        let r1_bids: BTreeSet<Bid> = r1.states.keys().copied().collect();
        let r2_bids: BTreeSet<Bid> = r2.states.keys().copied().collect();
        assert_eq!(r1_bids, r2_bids);
    }

    // -------------------------------------------------------------------------
    // T3: Conflicting state for the same BID is non-commutative (rhs-wins).
    //     If two tasks produce a node with the same BID but different content,
    //     the merge result depends on insertion order — the last graph passed
    //     as rhs overwrites. Consistent with edge semantics (update_edge).
    // -------------------------------------------------------------------------
    #[test]
    fn test_union_mut_state_conflict_rhs_wins() {
        let net = Bid::new(Bid::nil());
        let shared = Bid::new(net);

        let a = make_graph(
            vec![make_node(shared, "Version A", BeliefKind::Document)],
            vec![],
        );
        let b = make_graph(
            vec![make_node(shared, "Version B", BeliefKind::Document)],
            vec![],
        );

        let mut r1 = BeliefGraph::default();
        r1.union_mut(&a);
        r1.union_mut(&b); // B applied last as rhs → wins

        let mut r2 = BeliefGraph::default();
        r2.union_mut(&b);
        r2.union_mut(&a); // A applied last as rhs → wins

        // rhs wins in both cases: last-applied graph's content takes effect.
        assert_eq!(r1.states[&shared].title, "Version B");
        assert_eq!(r2.states[&shared].title, "Version A");
        // Non-commutative: order still matters, but now consistently rhs-wins rather than lhs-wins.
        assert_ne!(r1.states[&shared].title, r2.states[&shared].title);
    }

    // -------------------------------------------------------------------------
    // T4: Conflicting edge for the same (source, sink) pair is non-commutative
    //     (last-writer-wins via update_edge). Documents WEIGHT_SORT_KEY
    //     sensitivity: the final sort key depends on merge order.
    // -------------------------------------------------------------------------
    #[test]
    fn test_union_mut_edge_conflict_is_non_commutative() {
        let net = Bid::new(Bid::nil());
        let x = Bid::new(net);
        let y = Bid::new(net);

        // Both graphs own the same edge x→y but with different sort keys.
        let a = make_graph(
            vec![
                make_node(x, "X", BeliefKind::Document),
                make_node(y, "Y", BeliefKind::Document),
            ],
            vec![(x, y, 0)],
        );
        let b = make_graph(
            vec![
                make_node(x, "X", BeliefKind::Document),
                make_node(y, "Y", BeliefKind::Document),
            ],
            vec![(x, y, 99)],
        );

        let mut r1 = BeliefGraph::default();
        r1.union_mut(&a);
        r1.union_mut(&b); // b applied last → sort_key=99 wins

        let mut r2 = BeliefGraph::default();
        r2.union_mut(&b);
        r2.union_mut(&a); // a applied last → sort_key=0 wins

        assert_eq!(
            edge_sort_key(&r1, x, y),
            Some(99),
            "b wins when applied last"
        );
        assert_eq!(
            edge_sort_key(&r2, x, y),
            Some(0),
            "a wins when applied last"
        );
        assert_ne!(
            edge_sort_key(&r1, x, y),
            edge_sort_key(&r2, x, y),
            "edge merge is order-dependent under conflict"
        );
    }

    // -------------------------------------------------------------------------
    // T5: Fully disjoint tasks are commutative — the ownership invariant
    //     happy path. This is the critical gate test for Issue 57: if tasks
    //     own disjoint BID sets and disjoint edge sets, parallel merging is
    //     safe regardless of order.
    // -------------------------------------------------------------------------
    #[test]
    fn test_union_mut_disjoint_tasks_commutative() {
        let net = Bid::new(Bid::nil());
        let x = Bid::new(net);
        let y = Bid::new(net);
        let p = Bid::new(net);
        let q = Bid::new(net);

        // Task A: owns nodes X, Y and edge X→Y
        let a = make_graph(
            vec![
                make_node(x, "X", BeliefKind::Document),
                make_node(y, "Y", BeliefKind::Document),
            ],
            vec![(x, y, 1)],
        );
        // Task B: owns nodes P, Q and edge P→Q — completely disjoint from A
        let b = make_graph(
            vec![
                make_node(p, "P", BeliefKind::Document),
                make_node(q, "Q", BeliefKind::Document),
            ],
            vec![(p, q, 2)],
        );

        let mut r1 = BeliefGraph::default();
        r1.union_mut(&a);
        r1.union_mut(&b);

        let mut r2 = BeliefGraph::default();
        r2.union_mut(&b);
        r2.union_mut(&a);

        // State sets must be identical.
        let r1_bids: BTreeSet<Bid> = r1.states.keys().copied().collect();
        let r2_bids: BTreeSet<Bid> = r2.states.keys().copied().collect();
        assert_eq!(r1_bids, r2_bids, "state sets equal under disjoint merge");
        // Edge counts must match.
        assert_eq!(
            r1.relations.as_graph().edge_count(),
            r2.relations.as_graph().edge_count(),
            "edge counts equal under disjoint merge"
        );
        // Individual edge weights must match.
        assert_eq!(edge_sort_key(&r1, x, y), edge_sort_key(&r2, x, y));
        assert_eq!(edge_sort_key(&r1, p, q), edge_sort_key(&r2, p, q));
    }

    // -------------------------------------------------------------------------
    // T6: Shared namespace / API node appears exactly once regardless of merge
    //     order. Because the API node is identical in both graphs (same BID,
    //     same content), first-writer-wins is idempotent and both orderings
    //     produce the same result.
    // -------------------------------------------------------------------------
    #[test]
    fn test_union_mut_shared_api_node_commutative() {
        let net = Bid::new(Bid::nil());
        let api = Bid::new(net);
        let x = Bid::new(net);
        let y = Bid::new(net);

        let api_node = make_node(api, "API", BeliefKind::Network);

        // Both tasks share the identical API node.
        let a = make_graph(
            vec![api_node.clone(), make_node(x, "X", BeliefKind::Document)],
            vec![(x, api, 0)],
        );
        let b = make_graph(
            vec![api_node.clone(), make_node(y, "Y", BeliefKind::Document)],
            vec![(y, api, 1)],
        );

        let mut r1 = BeliefGraph::default();
        r1.union_mut(&a);
        r1.union_mut(&b);

        let mut r2 = BeliefGraph::default();
        r2.union_mut(&b);
        r2.union_mut(&a);

        // API node appears exactly once in both results.
        assert_eq!(r1.states.len(), 3, "api + x + y, no duplicates (r1)");
        assert_eq!(r2.states.len(), 3, "api + x + y, no duplicates (r2)");

        // API node content is identical in both orderings.
        assert_eq!(r1.states[&api].title, r2.states[&api].title);

        // Both edges are present in both results.
        assert!(edge_sort_key(&r1, x, api).is_some());
        assert!(edge_sort_key(&r1, y, api).is_some());
        assert!(edge_sort_key(&r2, x, api).is_some());
        assert!(edge_sort_key(&r2, y, api).is_some());
    }

    // -------------------------------------------------------------------------
    // T7: Three-way merge associativity under disjoint ownership.
    //     merge(merge(base, A), B) == merge(merge(base, B), A)
    //     This is the compiler's post-epoch pattern extended to three tasks.
    // -------------------------------------------------------------------------
    #[test]
    fn test_union_mut_three_way_merge_associative_under_disjoint_ownership() {
        let net = Bid::new(Bid::nil());
        let base_node = Bid::new(net);
        let x = Bid::new(net);
        let y = Bid::new(net);
        let p = Bid::new(net);
        let q = Bid::new(net);

        let base = make_graph(
            vec![make_node(base_node, "Base", BeliefKind::Network)],
            vec![],
        );
        let a = make_graph(
            vec![
                make_node(x, "X", BeliefKind::Document),
                make_node(y, "Y", BeliefKind::Document),
            ],
            vec![(x, y, 10)],
        );
        let b = make_graph(
            vec![
                make_node(p, "P", BeliefKind::Document),
                make_node(q, "Q", BeliefKind::Document),
            ],
            vec![(p, q, 20)],
        );

        // merge(merge(base, A), B)
        let mut r1 = base.clone();
        r1.union_mut(&a);
        r1.union_mut(&b);

        // merge(merge(base, B), A)
        let mut r2 = base.clone();
        r2.union_mut(&b);
        r2.union_mut(&a);

        let r1_bids: BTreeSet<Bid> = r1.states.keys().copied().collect();
        let r2_bids: BTreeSet<Bid> = r2.states.keys().copied().collect();
        assert_eq!(
            r1_bids, r2_bids,
            "three-way merge produces identical state sets under disjoint ownership"
        );
        assert_eq!(
            r1.relations.as_graph().edge_count(),
            r2.relations.as_graph().edge_count(),
            "three-way merge produces identical edge counts under disjoint ownership"
        );
        assert_eq!(edge_sort_key(&r1, x, y), edge_sort_key(&r2, x, y));
        assert_eq!(edge_sort_key(&r1, p, q), edge_sort_key(&r2, p, q));
    }

    // -------------------------------------------------------------------------
    // BeliefSource impl tests
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn test_belief_source_evaluate_on_graph() {
        let net = Bid::new(Bid::nil());
        let doc = Bid::new(net);
        let sec = Bid::new(doc);

        let graph = make_graph(
            vec![
                make_node(net, "Net", BeliefKind::Network),
                make_node(doc, "Doc", BeliefKind::Document),
                make_node(sec, "Sec A", BeliefKind::Symbol),
            ],
            vec![(doc, net, 0), (sec, doc, 1)],
        );

        // Evaluate a simple BID-set query via the BeliefSource impl.
        let spec = QuerySpec::seed(TapeFn::Bids(vec![doc]));
        let mut package = QueryPackage::balanced(spec);
        graph.evaluate(&mut package).await.unwrap();
        let result = package.into_graph();

        // The seed BID (doc) must be present and non-Trace.
        assert!(
            result.states.contains_key(&doc),
            "doc node should be in result"
        );
        assert!(
            result.states[&doc].kind.is_complete(),
            "doc node should be non-Trace (seed)"
        );
        // Balanced query adds halo + ancestry, so net and sec should appear as Trace.
        assert!(
            result.states.contains_key(&net),
            "net (halo neighbor) should be in result"
        );
        assert!(
            result.states.contains_key(&sec),
            "sec (halo neighbor) should be in result"
        );
    }

    /// Test `TapeFn::Terminal` via `roots()`.  Given a section tree:
    ///
    ///     root ← doc_a ← sec1
    ///                   ← sec2
    ///          ← doc_b ← sec3
    ///
    /// Starting from sec1, `roots()` should walk upstream Section edges and
    /// return only `root` (the only node with no outgoing Section edges).
    #[tokio::test]
    async fn test_terminal_roles_roots() {
        let root = Bid::new(Bid::nil());
        let doc_a = Bid::new(root);
        let doc_b = Bid::new(root);
        let sec1 = Bid::new(doc_a);
        let sec2 = Bid::new(doc_a);
        let sec3 = Bid::new(doc_b);

        let graph = make_graph(
            vec![
                make_node(root, "Root", BeliefKind::Network),
                make_node(doc_a, "Doc A", BeliefKind::Document),
                make_node(doc_b, "Doc B", BeliefKind::Document),
                make_node(sec1, "Sec 1", BeliefKind::Symbol),
                make_node(sec2, "Sec 2", BeliefKind::Symbol),
                make_node(sec3, "Sec 3", BeliefKind::Symbol),
            ],
            vec![
                (doc_a, root, 0),
                (doc_b, root, 1),
                (sec1, doc_a, 0),
                (sec2, doc_a, 1),
                (sec3, doc_b, 0),
            ],
        );

        // Evaluate roots() starting from sec1.
        let spec = QuerySpec::seed_then(TapeFn::Bids(vec![sec1]), roots());
        let mut package = QueryPackage::new(spec);
        graph.evaluate(&mut package).await.unwrap();
        let result_bids: BTreeSet<Bid> = package
            .tape()
            .steps
            .last()
            .map(|e| e.content.output_bids().into_iter().collect())
            .unwrap_or_default();

        // Only `root` should survive the terminal filter.
        assert_eq!(result_bids.len(), 1, "Expected exactly 1 root");
        assert!(
            result_bids.contains(&root),
            "Root should be the only terminal node"
        );
    }

    /// Test `TapeFn::Terminal` via `leaves()` — starting from root, should find
    /// only leaf nodes (sec1, sec2).
    #[tokio::test]
    async fn test_terminal_roles_leaves() {
        let root = Bid::new(Bid::nil());
        let doc_a = Bid::new(root);
        let sec1 = Bid::new(doc_a);
        let sec2 = Bid::new(doc_a);

        let graph = make_graph(
            vec![
                make_node(root, "Root", BeliefKind::Network),
                make_node(doc_a, "Doc A", BeliefKind::Document),
                make_node(sec1, "Sec 1", BeliefKind::Symbol),
                make_node(sec2, "Sec 2", BeliefKind::Symbol),
            ],
            vec![(doc_a, root, 0), (sec1, doc_a, 0), (sec2, doc_a, 1)],
        );

        // leaves() from root should find sec1 and sec2.
        let spec = QuerySpec::seed_then(TapeFn::Bids(vec![root]), leaves());
        let mut package = QueryPackage::new(spec);
        graph.evaluate(&mut package).await.unwrap();
        let result_bids: BTreeSet<Bid> = package
            .tape()
            .steps
            .last()
            .map(|e| e.content.output_bids().into_iter().collect())
            .unwrap_or_default();

        assert_eq!(result_bids.len(), 2, "Expected 2 leaves");
        assert!(result_bids.contains(&sec1), "sec1 should be a leaf");
        assert!(result_bids.contains(&sec2), "sec2 should be a leaf");
    }
}
