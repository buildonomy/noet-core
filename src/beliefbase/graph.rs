//! Graph data structures for representing belief relationships.
//!
//! This module provides the core graph types used throughout the belief system:
//! - [`BidGraph`]: Owned graph with WeightSet edges
//! - [`BidRefGraph`]: Borrowed graph with &WeightSet edges
//! - [`BeliefGraph`]: Combined states and relations for serialization/queries

use crate::{
    properties::{BeliefKind, BeliefNode, BeliefRefRelation, Bid, WeightSet},
    query::{Expression, RelationPred, ResultsPage, StatePred, DEFAULT_LIMIT},
};
use petgraph::{
    graphmap::GraphMap,
    visit::{depth_first_search, Control, DfsEvent},
    Directed, Direction, IntoWeightedEdge,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{btree_map::Entry as BTreeEntry, BTreeMap, BTreeSet},
    fmt,
    ops::{Deref, DerefMut},
};

use super::BeliefBase;

pub type BidSubGraph = GraphMap<Bid, (u16, Vec<String>), Directed>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BidGraph(pub petgraph::Graph<Bid, WeightSet>);

impl Default for BidGraph {
    fn default() -> Self {
        BidGraph(petgraph::Graph::new())
    }
}

impl BidGraph {
    pub fn as_graph(&self) -> &petgraph::Graph<Bid, WeightSet> {
        &self.0
    }

    pub fn as_graph_mut(&mut self) -> &mut petgraph::Graph<Bid, WeightSet> {
        &mut self.0
    }

    pub fn from_edges<I>(iterable: I) -> Self
    where
        I: IntoIterator,
        I::Item: IntoWeightedEdge<WeightSet, NodeId = Bid>,
    {
        let mut graph = petgraph::Graph::new();
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

    pub fn filter(&self, pred: &RelationPred, invert: bool) -> BidRefGraph<'_> {
        let edges = self.as_graph().raw_edges().iter().filter(|edge| {
            let source = self.as_graph()[edge.source()];
            let sink = self.as_graph()[edge.target()];
            let weights = &edge.weight;
            let is_match = pred.match_ref(&BeliefRefRelation {
                source: &source,
                sink: &sink,
                weights,
            });
            (is_match && !invert) || (!is_match && invert)
        });

        BidRefGraph::from_edges(edges.map(|edge| {
            (
                self.as_graph()[edge.source()],
                self.as_graph()[edge.target()],
                &edge.weight,
            )
        }))
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

    pub fn as_subgraph(&self, kind: crate::properties::WeightKind, reverse: bool) -> BidSubGraph {
        let edges = self.as_graph().raw_edges().iter().filter_map(|edge| {
            let source = self.as_graph()[edge.source()];
            let sink = self.as_graph()[edge.target()];
            let weight = edge.weight.get(&kind);
            weight.map(|w| {
                let paths: Vec<String> = w.get_doc_paths();
                let sort_key: u16 = w.get(crate::properties::WEIGHT_SORT_KEY).unwrap_or(0);
                if reverse {
                    (sink, source, (sort_key, paths))
                } else {
                    (source, sink, (sort_key, paths))
                }
            })
        });
        BidSubGraph::from_edges(edges)
    }

    pub fn sink_subgraph(
        &self,
        start_node: Bid,
        kind: crate::properties::WeightKind,
    ) -> BTreeSet<Bid> {
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

    pub fn source_subgraph(
        &self,
        start_node: Bid,
        kind: crate::properties::WeightKind,
    ) -> BTreeSet<Bid> {
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

    pub fn filter(&self, pred: &RelationPred, invert: bool) -> BidRefGraph<'_> {
        let edges = self.as_graph().raw_edges().iter().filter(|edge| {
            let source = self.as_graph()[edge.source()];
            let sink = self.as_graph()[edge.target()];
            let weights = &edge.weight;
            let is_match = pred.match_ref(&BeliefRefRelation {
                source: &source,
                sink: &sink,
                weights,
            });
            (is_match && !invert) || (!is_match && invert)
        });
        BidRefGraph::from_edges(edges.map(|edge| {
            (
                self.as_graph()[edge.source()],
                self.as_graph()[edge.target()],
                &edge.weight,
            )
        }))
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
#[derive(Debug, Default, Clone, Deserialize, Serialize)]
pub struct BeliefGraph {
    pub states: BTreeMap<Bid, BeliefNode>,
    pub relations: BidGraph,
}

impl BeliefGraph {
    pub fn is_empty(&self) -> bool {
        self.states.is_empty() && self.relations.as_graph().node_count() == 0
    }

    pub fn display_contents(&self) -> String {
        let edge_tuple = self
            .relations
            .as_graph()
            .raw_edges()
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
                    .weight
                    .weights
                    .iter()
                    .map(|(k, v)| {
                        format!(
                            "{}[{}]",
                            k,
                            v.get(crate::properties::WEIGHT_OWNED_BY)
                                .map(|owner: String| if &owner == "source" { "+" } else { "-" })
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
                if self.states.contains_key(&bid) {
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
                                if let BTreeEntry::Vacant(e) = self.states.entry(sink_bid) {
                                    e.insert(sink_node.clone());
                                } else {
                                    // This is expected for unbalanced sets, such as produced by eval_unbalanced.
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
        for edge in rhs.relations.as_graph().raw_edges() {
            let source = rhs.relations.as_graph()[edge.source()];
            let sink = rhs.relations.as_graph()[edge.target()];

            if source == sink {
                tracing::warn!(
                    "Ignoring self-connection (infinite loop) between [{} - {}] with weights {:?}",
                    source,
                    self.states
                        .get(&source)
                        .map(|n| n.title.as_str())
                        .unwrap_or_default(),
                    edge.weight
                );
                continue;
            }

            // Only add edges for nodes that have a state in the now-merged state map.
            if self.states.contains_key(&source) || self.states.contains_key(&sink) {
                if let BTreeEntry::Vacant(e) = self.states.entry(sink) {
                    if let Some(rhs_state) = rhs.states.get(&sink) {
                        // tracing::debug!(
                        //     "Adding source {} {} to lhs",
                        //     rhs_state.bid,
                        //     rhs_state.display_title()
                        // );
                        e.insert(rhs_state.clone());
                    } else {
                        tracing::warn!("neither lhs or rhs contains node with sink id: {}", sink,);
                    }
                }
                if let BTreeEntry::Vacant(e) = self.states.entry(source) {
                    if let Some(rhs_state) = rhs.states.get(&source) {
                        // tracing::debug!(
                        //     "Adding source {} {} to lhs",
                        //     rhs_state.bid,
                        //     rhs_state.display_title()
                        // );
                        e.insert(rhs_state.clone());
                    } else {
                        tracing::warn!(
                            "neither lhs or rhs contains node with source id: {}",
                            source,
                        );
                    }
                }
                let source_idx = *bid_to_index
                    .entry(source)
                    .or_insert_with(|| self.relations.as_graph_mut().add_node(source));
                let sink_idx = *bid_to_index
                    .entry(sink)
                    .or_insert_with(|| self.relations.as_graph_mut().add_node(sink));
                self.relations.as_graph_mut().update_edge(
                    source_idx,
                    sink_idx,
                    edge.weight.clone(),
                );
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

    /// Union with trace nodes included. Used during traversal where we want to accumulate
    /// nodes even if they're marked as Trace (incomplete relations). rhs wins on conflict,
    /// except that a Trace rhs node never downgrades a complete lhs node.
    pub fn union_mut_with_trace(&mut self, rhs: &BeliefGraph) {
        // Accept all nodes from rhs, including Trace nodes
        for node in rhs.states.values() {
            match self.states.entry(node.bid) {
                BTreeEntry::Vacant(e) => {
                    e.insert(node.clone());
                }
                BTreeEntry::Occupied(mut e) => {
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
            states: BTreeMap::from_iter(
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
            states: BTreeMap::from_iter(
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

    /// In order to (attempt to) fullfill the balanced beliefbase invariants, this will keep building
    /// queries so long as there are subsection relation sinks who's nodes are not loaded.
    pub fn build_balance_expr(&self) -> Option<Expression> {
        self.build_downstream_expr(Some(crate::properties::WeightKind::Section.into()))
    }

    /// Find BIDs referenced in relations but not present in states.
    /// Returns a sorted, deduplicated list of orphaned BIDs.
    pub fn find_orphaned_edges(&self) -> Vec<Bid> {
        let mut missing = Vec::new();
        for edge in self.relations.as_graph().raw_edges() {
            let source = self.relations.as_graph()[edge.source()];
            let sink = self.relations.as_graph()[edge.target()];
            if !self.states.contains_key(&source) {
                missing.push(source);
            }
            if !self.states.contains_key(&sink) {
                missing.push(sink);
            }
        }
        missing.sort();
        missing.dedup();
        missing
    }

    /// Construct a query expression to access any missing relationships, optionally filtered
    /// by WeightSet.
    ///
    /// dir: Comes from petgraph::Graph::externals, which defines dir as: Return an iterator over
    /// either the nodes without edges to them (Incoming) or from them (Outgoing).
    fn find_externals(&self, weights: Option<WeightSet>, dir: Direction) -> BTreeSet<Bid> {
        let filter_weights = weights.unwrap_or(WeightSet::full());
        let mut external_bids = BTreeSet::default();
        let filtered_edge_graph = self
            .relations
            .filter(&RelationPred::Kind(filter_weights.clone()), false);
        let other_dir = match dir {
            Direction::Incoming => Direction::Outgoing,
            Direction::Outgoing => Direction::Incoming,
        };
        // filter out orphaned nodes
        let other_externals = filtered_edge_graph
            .as_graph()
            .externals(other_dir)
            .collect::<Vec<_>>();
        let edge_externals = filtered_edge_graph
            .as_graph()
            .externals(dir)
            .collect::<Vec<_>>();
        for edge_idx in edge_externals.iter() {
            if other_externals.contains(edge_idx) {
                tracing::debug!("Filtering out orphaned node");
                continue;
            }
            let bid = filtered_edge_graph.as_graph()[*edge_idx];
            external_bids.insert(bid);
        }
        external_bids
    }

    /// Find the nodes in the relation graph filtered by `weights` EdgeWeights without edges TO them
    /// which are either 1) not in our self.states or who's state.kind contains BeliefKind::Trace
    /// (meaning not all their relationships are loaded)
    pub fn build_upstream_expr(&self, weights: Option<WeightSet>) -> Option<Expression> {
        let external_bids = self.find_externals(weights, Direction::Incoming);
        if external_bids.is_empty() {
            None
        } else {
            Some(Expression::StateIn(StatePred::Bid(Vec::from_iter(
                external_bids,
            ))))
        }
    }

    /// Find the nodes in the relation graph filtered by `weights` EdgeWeights without edges FROM them
    /// which are either 1) not in our self.states or who's state.kind contains BeliefKind::Trace
    /// (meaning not all their relationships are loaded)
    pub fn build_downstream_expr(&self, weights: Option<WeightSet>) -> Option<Expression> {
        let external_bids = self.find_externals(weights, Direction::Outgoing);
        if external_bids.is_empty() {
            None
        } else {
            Some(Expression::StateIn(StatePred::Bid(Vec::from_iter(
                external_bids,
            ))))
        }
    }

    pub fn paginate(
        &self,
        limit: Option<usize>,
        offset: Option<usize>,
    ) -> ResultsPage<BeliefGraph> {
        let count = self.states.len();
        let start = offset.unwrap_or(0);
        let mut page_limit = limit.unwrap_or(DEFAULT_LIMIT);
        if page_limit > (count - start) {
            page_limit = count;
        }
        let results = match count > page_limit || start > 0 {
            true => {
                let states = BTreeMap::from_iter(self.states.iter().enumerate().filter_map(
                    |(idx, (bid, node))| {
                        if idx < start {
                            None
                        } else if idx < (start + page_limit) {
                            Some((*bid, node.clone()))
                        } else {
                            None
                        }
                    },
                ));
                let relations = BidGraph::from_edges(
                    self.relations
                        .as_graph()
                        .raw_edges()
                        .iter()
                        .filter(|edge| {
                            let source = self.relations.as_graph()[edge.source()];
                            let sink = self.relations.as_graph()[edge.target()];
                            states.contains_key(&source) && states.contains_key(&sink)
                        })
                        .map(|edge| {
                            (
                                self.relations.as_graph()[edge.source()],
                                self.relations.as_graph()[edge.target()],
                                edge.weight.clone(),
                            )
                        }),
                );

                // log::debug!(
                //     "[paginate] self relation count: {}, self state count: {}, paginate state count {}, paginate relation count {}",
                //     self.states().len(), self.relations().node_count(), states.len(), relations.node_count()
                // );
                BeliefGraph { states, relations }
            }
            false => BeliefGraph {
                states: self.states.clone(),
                relations: self.relations.clone(),
            },
        };
        ResultsPage {
            count,
            start,
            results,
        }
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
        let states: BTreeMap<Bid, BeliefNode> = nodes.iter().map(|n| (n.bid, n.clone())).collect();
        let relations = BidGraph::from_edges(
            edges
                .into_iter()
                .map(|(src, snk, sk)| (src, snk, make_weights(sk))),
        );
        BeliefGraph { states, relations }
    }

    /// Extract the sort_key for the single edge (source→sink) in `g`, panicking if absent.
    fn edge_sort_key(g: &BeliefGraph, source: Bid, sink: Bid) -> Option<u16> {
        g.relations.as_graph().raw_edges().iter().find_map(|e| {
            let s = g.relations.as_graph()[e.source()];
            let t = g.relations.as_graph()[e.target()];
            if s == source && t == sink {
                e.weight.get(&WeightKind::Section)?.get(WEIGHT_SORT_KEY)
            } else {
                None
            }
        })
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
        assert_eq!(
            r1.states.keys().collect::<Vec<_>>(),
            r2.states.keys().collect::<Vec<_>>()
        );
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
        assert_eq!(
            r1.states.keys().collect::<Vec<_>>(),
            r2.states.keys().collect::<Vec<_>>(),
            "state sets equal under disjoint merge"
        );
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

        assert_eq!(
            r1.states.keys().collect::<Vec<_>>(),
            r2.states.keys().collect::<Vec<_>>(),
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
}
