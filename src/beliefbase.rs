use crate::{
    event::{BeliefEvent, EventOrigin},
    nodekey::NodeKey,
    paths::{PathMap, PathMapMap},
    properties::{
        asset_namespace, href_namespace, BeliefKind, BeliefNode, BeliefRefRelation, BeliefRelation,
        Bid, Bref, WeightKind, WeightSet, WEIGHT_DOC_PATHS, WEIGHT_OWNED_BY, WEIGHT_SORT_KEY,
    },
    query::{BeliefSource, Expression, RelationPred, ResultsPage, SetOp, StatePred, DEFAULT_LIMIT},
    BuildonomyError,
};
use parking_lot::{ArcRwLockReadGuard, RawRwLock, RwLock};
use petgraph::{
    algo::kosaraju_scc,
    graphmap::GraphMap,
    visit::{depth_first_search, Control, DfsEvent, EdgeRef},
    Directed, Direction, IntoWeightedEdge,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{btree_map::Entry as BTreeEntry, BTreeMap, BTreeSet},
    fmt,
    ops::{Deref, DerefMut},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use url::Url;

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

    pub fn as_subgraph(&self, kind: WeightKind, reverse: bool) -> BidSubGraph {
        let edges = self.as_graph().raw_edges().iter().filter_map(|edge| {
            let source = self.as_graph()[edge.source()];
            let sink = self.as_graph()[edge.target()];
            let weight = edge.weight.get(&kind);
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

// ExtendedRelation tracks relation information with respect to a node. 'Other' refers to the
// external node. The self node is specified by the struture holding the ExtendedRelation (e.g. a
// [BeliefContext]).
#[derive(Debug)]
pub struct ExtendedRelation<'a> {
    pub other: &'a BeliefNode,
    pub home_net: Bid,
    pub relative_path: String,
    pub weight: &'a WeightSet,
}

impl<'a> ExtendedRelation<'a> {
    pub fn new(
        other_bid: Bid,
        relative_net: Bid,
        weight: &'a WeightSet,
        set: &'a BeliefBase,
    ) -> Option<ExtendedRelation<'a>> {
        let Some(other) = set.states().get(&other_bid) else {
            tracing::info!("Could not find 'other' node: {:?}", other_bid);
            return None;
        };

        let paths_guard = set.paths();
        let Some((home_net, relative_path)) = paths_guard
            .get_map(&relative_net)
            .and_then(|pm| pm.path(&other_bid, &paths_guard))
            .and_then(|(bid, path, _order)| Some((bid, path)))
        else {
            tracing::warn!("Could not find api_path to other node: {}", other);
            return None;
        };

        Some(ExtendedRelation {
            home_net,
            relative_path,
            other,
            weight,
        })
    }

    pub fn as_link_ref(&self) -> String {
        format!(
            "{}{}{}",
            self.other.bid.namespace(),
            if !self.other.title.is_empty() {
                ":"
            } else {
                ""
            },
            self.other.title
        )
    }
}

#[derive(Debug)]
pub struct BeliefContext<'a> {
    pub node: &'a BeliefNode,
    pub relative_path: String,
    pub relative_net: Bid,
    pub home_net: Bid,
    set: &'a BeliefBase,
    relations_guard: ArcRwLockReadGuard<RawRwLock, BidGraph>,
}

impl<'a> BeliefContext<'a> {
    pub fn href(&self, origin: String) -> Result<String, BuildonomyError> {
        let origin = Url::parse(&origin)?;
        Ok(origin.join(&self.relative_path)?.as_str().to_string())
    }

    /// Get a reference to the underlying BeliefBase
    pub fn belief_set(&self) -> &'a BeliefBase {
        self.set
    }

    /// Lazily compute source relations for this node
    pub fn sources(&'a self) -> Vec<ExtendedRelation<'a>> {
        let graph = self.relations_guard.as_graph();

        graph
            .raw_edges()
            .iter()
            .filter_map(|edge| {
                let source_bid = graph[edge.source()];
                let sink_bid = graph[edge.target()];
                if sink_bid == self.node.bid {
                    ExtendedRelation::new(source_bid, self.relative_net, &edge.weight, self.set)
                } else {
                    None
                }
            })
            .collect()
    }

    /// Lazily compute sink relations for this node
    pub fn sinks(&'a self) -> Vec<ExtendedRelation<'a>> {
        let graph = self.relations_guard.as_graph();

        graph
            .raw_edges()
            .iter()
            .filter_map(|edge| {
                let source_bid = graph[edge.source()];
                let sink_bid = graph[edge.target()];
                if source_bid == self.node.bid {
                    ExtendedRelation::new(sink_bid, self.relative_net, &edge.weight, self.set)
                } else {
                    None
                }
            })
            .collect()
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
                        id_vec.push(n.bid.namespace().to_string());
                        id_vec.join(": ")
                    })
                    .unwrap_or(source_b.namespace().to_string());
                let sink = self
                    .states
                    .get(&sink_b)
                    .map(|n| {
                        let mut id_vec = vec![n.bid.namespace().to_string()];
                        if !n.title.is_empty() {
                            id_vec.push(n.title.clone());
                        }
                        id_vec.join(": ")
                    })
                    .unwrap_or(sink_b.namespace().to_string());
                let weights = e
                    .weight
                    .weights
                    .iter()
                    .map(|(k, v)| {
                        format!(
                            "{}[{}]",
                            k,
                            v.get(WEIGHT_OWNED_BY)
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
        // First, union the states with the non-trace elements of rhs.
        for node in rhs.states.values().filter(|node| node.kind.is_complete()) {
            let self_node_entry = self.states.entry(node.bid).or_insert_with(|| node.clone());
            if self_node_entry.kind.contains(BeliefKind::Trace)
                && !node.kind.contains(BeliefKind::Trace)
            {
                // rhs asserts it contains all relations for this node, so remove the Trace kind.
                self_node_entry.kind.remove(BeliefKind::Trace);
            }
        }
        self.add_relations(rhs);
    }

    /// Union with trace nodes included. Used during traversal where we want to accumulate
    /// nodes even if they're marked as Trace (incomplete relations).
    pub fn union_mut_with_trace(&mut self, rhs: &BeliefGraph) {
        // Accept all nodes from rhs, including Trace nodes
        for node in rhs.states.values() {
            let self_node_entry = self.states.entry(node.bid).or_insert_with(|| node.clone());
            if self_node_entry.kind.contains(BeliefKind::Trace)
                && !node.kind.contains(BeliefKind::Trace)
            {
                // rhs asserts it contains all relations for this node, so remove the Trace kind.
                self_node_entry.kind.remove(BeliefKind::Trace);
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
        self.build_downstream_expr(Some(WeightKind::Section.into()))
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

#[derive(Debug)]
pub struct BeliefBase {
    states: BTreeMap<Bid, BeliefNode>,
    relations: Arc<RwLock<BidGraph>>,
    bid_to_index: RwLock<BTreeMap<Bid, petgraph::graph::NodeIndex>>,
    index_dirty: AtomicBool,
    brefs: BTreeMap<Bref, Bid>,
    paths: Arc<RwLock<PathMapMap>>,
    errors: Arc<RwLock<Vec<String>>>,
    api: BeliefNode,
}

impl From<BeliefGraph> for BeliefBase {
    fn from(beliefs: BeliefGraph) -> Self {
        // tracing::debug!(
        //     "[BeliefBase::from(BeliefGraph)] Creating BeliefBase with {} states, {} edges",
        //     beliefs.states.len(),
        //     beliefs.relations.0.edge_count()
        // );
        BeliefBase::new_unbalanced(beliefs.states, beliefs.relations, false)
    }
}

impl PartialEq for BeliefBase {
    fn eq(&self, other: &Self) -> bool {
        let lhs_states = BTreeSet::from_iter(self.states.keys().copied());
        let rhs_states = BTreeSet::from_iter(other.states.keys().copied());

        let intersection_count = lhs_states.intersection(&rhs_states).count();
        self.states.len() == intersection_count
    }
}

impl fmt::Display for BeliefBase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "BeliefBase({} nodes, {} edges)",
            self.states().len(),
            self.relations().as_graph().edge_count()
        )
    }
}

/// The same as [BeliefBase::empty] except it contains the api_node within the states and paths
/// properties.
impl Default for BeliefBase {
    fn default() -> BeliefBase {
        BeliefBase::new(BTreeMap::default(), BidGraph::default())
            .expect("Single state set with no relations to pass the BeliefBase built in test")
    }
}

impl Clone for BeliefBase {
    fn clone(&self) -> BeliefBase {
        BeliefBase {
            states: self.states.clone(),
            relations: Arc::new(RwLock::new(self.relations.read().clone())),
            bid_to_index: RwLock::new(self.bid_to_index.read().clone()),
            index_dirty: AtomicBool::new(self.index_dirty.load(Ordering::SeqCst)),
            brefs: self.brefs.clone(),
            paths: Arc::new(RwLock::new(self.paths.read().clone())),
            errors: Arc::new(RwLock::new(self.errors.read().clone())),
            api: self.api.clone(),
        }
    }
}

/// BeliefBase: A structured collection of `BeliefState`s and their relations that can be queried and
/// manipulated while preserving a global graph structure.
///
/// - Creates a cache that maps belief IDs and belief paths to quick lookup information such as:
///   local path, title, bid, content summary, version control state, belief type
/// - Creates typed belief-to-belief directional relationships between belief objects
///
/// Static Invariants for a balanced BeliefBase (checked by [BeliefBase::built_in_test] and
/// BeliefBase::check_path_invariants):
///
/// 0. Each BeliefRelationKind sub-graph forms a directed acyclic graph. sub-graph cycles are not
///    supported.
///
/// 1. All nodes within the relation hyper-graph have:
///
///    0. A corresponding state ([crate::properties::BeliefNode]) and,
///
///    1. A corresponding API path.
///
/// 2. Each Belief relation is ordered by BeliefRelationKind weights. Each weight specifies a
///    different graph type. The relation graph is therefore something like a hypergraph. Because of
///    the weights, each sub-graph has a deterministic ordering. In this manner, the relation graph
///    can produce deterministically serialized results, necessary for things like creating table of
///    contents, or serialized procedural outcomes.
///
/// Operational rules:
///
/// 1. The holder of a link is a 'sink' whereas the resource its accessing is the source. Parent ==
///    sink, child == source. In non-parent-child relationships this is intuitive, but it also makes
///    sense for subsections. In as the child contains its self state (source), and the parent is
///    indexing its child relationships, so 'sinking'/consuming data from the child nodes. Think
///    about the direction the information is flowing.
///
/// 2. PathMaps identify how to acquire the source starting from known network locations.
impl BeliefBase {
    pub fn empty() -> BeliefBase {
        BeliefBase {
            states: BTreeMap::default(),
            relations: Arc::new(RwLock::new(BidGraph(petgraph::Graph::new()))),
            bid_to_index: RwLock::new(BTreeMap::default()),
            index_dirty: AtomicBool::new(false),
            brefs: BTreeMap::default(),
            paths: Arc::new(RwLock::new(PathMapMap::default())),
            errors: Arc::new(RwLock::new(Vec::new())),
            api: BeliefNode::api_state(),
        }
    }

    pub fn new_unbalanced(
        states: BTreeMap<Bid, BeliefNode>,
        relations: BidGraph,
        inject_api: bool,
    ) -> BeliefBase {
        let mut bs = BeliefBase::empty();
        // Newly created RwLock, so we know there's no one else locking it.
        {
            *bs.relations.write_arc() = relations;
        }
        bs.states = states;
        bs.brefs = BTreeMap::from_iter(bs.states.keys().map(|bid| (bid.namespace(), *bid)));
        if inject_api {
            bs.insert_state(bs.api.clone(), &[]);
        }
        bs.index_dirty.store(true, Ordering::SeqCst);
        bs.index_sync(false);
        *bs.paths.write() = PathMapMap::new(bs.states(), bs.relations.clone());
        bs
    }

    pub fn new(
        states: BTreeMap<Bid, BeliefNode>,
        relations: BidGraph,
    ) -> Result<BeliefBase, BuildonomyError> {
        let set = BeliefBase::new_unbalanced(states, relations, true);
        Ok(set)
    }

    pub fn api(&self) -> &BeliefNode {
        &self.api
    }

    pub fn states(&self) -> &BTreeMap<Bid, BeliefNode> {
        &self.states
    }

    pub fn paths(&self) -> ArcRwLockReadGuard<RawRwLock, PathMapMap> {
        self.index_sync(false);
        while self.paths.is_locked_exclusive() {
            tracing::info!("[BeliefBase] Waiting for read access to paths");
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        self.paths.read_arc()
    }

    pub fn brefs(&self) -> &BTreeMap<Bref, Bid> {
        &self.brefs
    }

    pub fn errors(&self) -> Vec<String> {
        self.errors.read().clone()
    }

    /// Synchronize our indices (namely the self.paths object and our bid_to_index object), if the
    /// index_dirty flag is set. If bit is true, then run built in test as well.
    fn index_sync(&self, bit: bool) {
        if !self.index_dirty.load(Ordering::SeqCst) {
            return;
        }
        // This block ensures we drop relations and index
        {
            let mut relations = self.relations.write_arc();
            let mut index = self.bid_to_index.write();
            *index = BTreeMap::from_iter(
                relations
                    .as_graph()
                    .node_indices()
                    .map(|idx| (relations.as_graph()[idx], idx)),
            );
            // Ensure all nodes in states are also in the relations graph
            // This handles nodes that were added to states but have no edges
            for bid in self.states.keys() {
                index
                    .entry(*bid)
                    .or_insert_with(|| relations.as_graph_mut().add_node(*bid));
            }
        }
        self.index_dirty.store(false, Ordering::SeqCst);

        if bit {
            // Rebuild paths - write to the Arc<RwLock<PathMapMap>>
            let constructor_paths_map = PathMapMap::new(self.states(), self.relations.clone());
            let constructor_all_paths = constructor_paths_map.all_paths();
            let constructor_paths: BTreeSet<String> = constructor_all_paths
                .values()
                .flatten()
                .map(|(path, _, _)| path.clone())
                .collect();
            // Update the paths field with the new PathMapMap
            let event_all_paths = self.paths().all_paths();
            let event_paths: BTreeSet<String> = event_all_paths
                .values()
                .flatten()
                .map(|(path, _, _)| path.clone())
                .collect();
            let mut errors = self.errors.write();
            *errors = self.built_in_test(bit);
            if event_paths != constructor_paths {
                errors.push(format!(
                    "- Event-driven and constructor PathMapMaps should have identical paths.\n \
                        \tevent_paths:\n \
                        \t- {} \n \
                        \tconstructor_paths:\n \
                        \t- {} \n",
                    event_paths
                        .into_iter()
                        .collect::<Vec<String>>()
                        .join("\n\t- "),
                    constructor_paths
                        .into_iter()
                        .collect::<Vec<String>>()
                        .join("\n\t- ")
                ));
            }
            let errors = self.errors.read();
            if !errors.is_empty() {
                tracing::debug!("Set isn't balanced. Errors:\n{}", errors.join("\n- "));
            }
        }
    }

    pub fn bid_to_index(&self, bid: &Bid) -> Option<petgraph::graph::NodeIndex> {
        self.index_sync(false);
        self.bid_to_index.read().get(bid).copied()
    }

    pub fn relations(&self) -> ArcRwLockReadGuard<RawRwLock, BidGraph> {
        self.index_sync(false);
        while self.relations.is_locked_exclusive() {
            tracing::info!("[BeliefBase] Waiting for read access to relations");
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        self.relations.read_arc()
    }

    pub fn get(&self, key: &NodeKey) -> Option<BeliefNode> {
        self.index_sync(false);
        match key {
            NodeKey::Bid { bid } => self.states.get(bid).cloned(),
            NodeKey::Bref { bref } => self
                .brefs()
                .get(bref)
                .and_then(|bid| self.states.get(bid).cloned()),
            NodeKey::Id { net, id } => self
                .paths()
                .net_get_from_id(net, id)
                .and_then(|(_, bid)| self.states.get(&bid).cloned()),
            NodeKey::Path { net, path } => self
                .paths()
                .net_get_from_path(net, path)
                .and_then(|(_, bid)| self.states.get(&bid).cloned()),
            NodeKey::Title { net, title } => self
                .paths()
                .net_get_from_title(net, title)
                .and_then(|(_, bid)| self.states.get(&bid).cloned()),
        }
    }

    // FIXME: This could introduce index issues, as BeliefContext has mutable access to self.
    pub fn get_context(&mut self, relative_to_net: &Bid, bid: &Bid) -> Option<BeliefContext<'_>> {
        self.index_sync(false);
        assert!(
            self.is_balanced().is_ok(),
            "get_context called on an unbalanced BeliefBase. errors: {:?}",
            self.errors.read().clone()
        );
        let Some(node) = self.states.get(bid) else {
            tracing::debug!("[get_context] node {bid} is not loaded");
            return None;
        };
        let Some(relative_to_pm) = self.paths().get_map(relative_to_net) else {
            tracing::debug!("[get_context] network {relative_to_net} is not loaded");
            return None;
        };
        relative_to_pm
            .path(bid, &self.paths())
            .and_then(|(home_net, relative_path, _order)| {
                Some(BeliefContext {
                    node,
                    home_net,
                    relative_path,
                    relative_net: *relative_to_net,
                    set: self,
                    relations_guard: self.relations(),
                })
            })
    }

    pub fn consume(&mut self) -> BeliefGraph {
        let mut old_self = std::mem::take(self);
        let states = std::mem::take(&mut old_self.states);
        while self.relations.is_locked() {
            tracing::info!("[BeliefBase::consume] Waiting for write access to relations");
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let relations = std::mem::replace(
            old_self.relations.write_arc().as_graph_mut(),
            petgraph::Graph::new(),
        );
        BeliefGraph {
            states,
            relations: BidGraph(relations),
        }
    }

    /// Compares two BeliefBase manifolds (old vs new) and generates a consolidated set of events
    /// representing their differences. This is the core reconciliation function used during parsing.
    ///
    /// # Arguments
    /// * `old_set` - The previous state (typically from session_bb or global_bb)
    /// * `new_set` - The current state (typically from self.set after parsing)
    /// * `parsed_nodes` - The set of nodes that were fully parsed (for scoping the comparison)
    ///
    /// # Returns
    /// A vector of BeliefEvents in proper order:
    ///
    /// Sequence:
    /// 0. Find the structural connection between the new_set parsed graph and the old_set. Add
    ///    nodes and relations to ensure the produced diff-stream has this connectivity all the way to
    ///    the api node defined.
    /// 1. NodesRemoved - clean up removed nodes and their subtrees
    /// 2. NodeUpdate - transmit events for modified nodes
    /// 3. RelationRemoved - clean up removed edges
    /// 4. RelationUpdate - add events for edges that are completely new
    /// 5. RelationChange - update edges that are changed
    ///
    /// Note: To get path updates, run the diff events through old set and collect the derived
    /// path events.
    pub fn compute_diff(
        old_set: &BeliefBase,
        new_set: &BeliefBase,
        parsed_content: &BTreeSet<Bid>,
        // _parsed_structure: &BTreeSet<Bid>,
    ) -> Result<Vec<BeliefEvent>, BuildonomyError> {
        use std::collections::BTreeMap;
        let mut events = Vec::new();
        // Phase 0: Generate NodeUpdate events for new or changed nodes
        let new_relations_arc = new_set.relations();
        let new_relations: BidGraph = {
            let new_relations_graph = new_relations_arc.as_graph();
            BidGraph::from_edges(new_relations_graph.raw_edges().iter().filter_map(|edge| {
                let source = new_relations_graph[edge.source()];
                let sink = new_relations_graph[edge.target()];
                if !(parsed_content.contains(&source) || parsed_content.contains(&sink)) {
                    return None;
                }

                let mut weightset = WeightSet::empty();
                for (kind, weight) in edge.weight.weights.iter() {
                    let (owner, _sign) = weight
                        .get(WEIGHT_OWNED_BY)
                        .map(|val: String| {
                            if &val == "source" {
                                (&source, "+")
                            } else {
                                (&sink, "-")
                            }
                        })
                        .unwrap_or((&sink, "-"));
                    // tracing::debug!("{}--[{}{}]-->{}", source, kind, _sign, sink);
                    // parse_content sets owner to sink unless parent is an api node, meaning
                    // the owner isn't necessarily in parsed_content for section nodes, but we
                    // know by construction that parse_content contains sufficient information
                    // to insert the weightset in this special case for sections.
                    if *kind == WeightKind::Section || parsed_content.contains(owner) {
                        weightset.weights.insert(*kind, weight.clone());
                    }
                }
                if weightset.is_empty() {
                    None
                } else {
                    Some((source, sink, weightset))
                }
            }))
        };
        let mut node_events = Vec::new();
        let mut relation_events = Vec::new();

        // Phase 1: Identify removed nodes
        let old_structure = old_set.relations().as_subgraph(WeightKind::Section, true);
        let mut old_content = BTreeSet::new();
        depth_first_search(
            &old_structure,
            parsed_content.iter().copied().collect::<Vec<_>>(),
            |event| match event {
                DfsEvent::Discover(sink, _) => {
                    if !new_set.states().contains_key(&sink) {
                        old_content.insert(sink);
                        Control::<()>::Continue
                    } else {
                        // No sense in following traces
                        Control::Prune
                    }
                }
                _ => Control::Continue,
            },
        );
        let removed_nodes = old_content
            .difference(parsed_content)
            .cloned()
            .collect::<Vec<Bid>>();
        if !removed_nodes.is_empty() {
            events.push(BeliefEvent::NodesRemoved(
                removed_nodes.clone(),
                EventOrigin::Remote,
            ));
        }

        // Add nodes from scaffolding search (phase 0)
        events.append(&mut node_events);

        // Phase 2: Update changed nodes
        for node_bid in parsed_content.iter() {
            if let Some(set_node) = new_set.states().get(node_bid) {
                let new_node = set_node.clone();
                let should_update = if let Some(old_node) = old_set.states().get(node_bid) {
                    new_node.toml() != old_node.toml()
                } else {
                    true
                };

                if should_update {
                    events.push(BeliefEvent::NodeUpdate(
                        vec![NodeKey::Bid { bid: *node_bid }],
                        new_node.toml(),
                        EventOrigin::Remote,
                    ));
                }
            }
        }

        // Add relations from scaffolding search (phase 0)
        events.append(&mut relation_events);

        // Prepare data structures for phase 3 and 4
        let parsed_edges = {
            let new_relations_graph = new_relations.as_graph();
            BTreeMap::<(Bid, Bid), WeightSet>::from_iter(
                new_relations_graph.raw_edges().iter().map(|edge| {
                    let source = new_relations_graph[edge.source()];
                    let sink = new_relations_graph[edge.target()];
                    ((source, sink), edge.weight.clone())
                }),
            )
        };
        let old_relations = old_set.relations();
        let old_relations_graph = old_relations.as_graph();
        let old_parsed_edges = BTreeMap::<(Bid, Bid), WeightSet>::from_iter(
            old_relations_graph.raw_edges().iter().filter_map(|edge| {
                let source = old_relations_graph[edge.source()];
                let sink = old_relations_graph[edge.target()];
                if !(parsed_content.contains(&source)
                    || removed_nodes.contains(&source)
                    || parsed_content.contains(&sink)
                    || removed_nodes.contains(&sink))
                {
                    return None;
                }
                let mut weightset = WeightSet::empty();
                for (kind, weight) in edge.weight.weights.iter() {
                    let (owner, _sign) = weight
                        .get(WEIGHT_OWNED_BY)
                        .map(|val: String| {
                            if &val == "source" {
                                (&source, "+")
                            } else {
                                (&sink, "-")
                            }
                        })
                        .unwrap_or((&sink, "-"));
                    // tracing::debug!("{}--[{}{}]-->{}", source, kind, _sign, sink);
                    // parse_content sets owner to sink unless parent is an api node, meaning
                    // the owner isn't necessarily in parsed_content for section nodes, but we
                    // know by construction that parse_content contains sufficient information
                    // to insert the weightset in this special case for sections.
                    if *kind == WeightKind::Section
                        || parsed_content.contains(owner)
                        || removed_nodes.contains(owner)
                    {
                        weightset.weights.insert(*kind, weight.clone());
                    }
                }
                if weightset.is_empty() {
                    None
                } else {
                    Some(((source, sink), weightset))
                }
            }),
        );

        // Phase 3: Removed edges
        for ((source, sink), _weight) in old_parsed_edges
            .iter()
            .filter(|(k, _v)| !parsed_edges.contains_key(k))
        {
            events.push(BeliefEvent::RelationRemoved(
                *source,
                *sink,
                EventOrigin::Remote,
            ));
        }

        // Phase 4: New edges
        for ((source, sink), weight) in parsed_edges
            .iter()
            .filter(|(k, _v)| !old_parsed_edges.contains_key(k))
        {
            events.push(BeliefEvent::RelationUpdate(
                *source,
                *sink,
                weight.clone(),
                EventOrigin::Remote,
            ));
        }

        // Phase 5: Check for updated edges
        for (key, weights) in parsed_edges.iter() {
            if let Some(old_weights) = old_parsed_edges.get(key) {
                for (kind, new_weight) in weights.weights.iter() {
                    let insert = old_weights
                        .get(kind)
                        .filter(|old_weight| **old_weight == *new_weight)
                        .is_none();
                    if insert {
                        events.push(BeliefEvent::RelationChange(
                            key.0,
                            key.1,
                            *kind,
                            Some(new_weight.clone()),
                            EventOrigin::Remote,
                        ));
                    }
                }
            }
        }

        Ok(events)
    }

    pub fn is_balanced(&self) -> Result<(), BuildonomyError> {
        let errors = self.errors.read();
        if !errors.is_empty() {
            Err(BuildonomyError::Custom(errors.join("\n- ")))
        } else {
            Ok(())
        }
    }

    pub fn is_empty(&self) -> bool {
        let mut content_len = self.states.len();
        if self.states.contains_key(&self.api().bid) {
            content_len -= 1;
        }
        if self.states.contains_key(&asset_namespace()) {
            content_len -= 1;
        }
        if self.states.contains_key(&href_namespace()) {
            content_len -= 1;
        }
        content_len == 0
    }

    /// Validates that a Local event matches the current internal state.
    /// This is used in debug builds to catch inconsistencies in the event stream.
    #[cfg(debug_assertions)]
    fn validate_local_event(&self, event: &BeliefEvent) -> Result<(), String> {
        match event {
            BeliefEvent::RelationUpdate(source, sink, weight_set, _) => {
                if let (Some(source_idx), Some(sink_idx)) =
                    (self.bid_to_index(source), self.bid_to_index(sink))
                {
                    let relations = self.relations();
                    if let Some(edge_idx) = relations.as_graph().find_edge(source_idx, sink_idx) {
                        let actual_weight = &relations.as_graph()[edge_idx];
                        if actual_weight != weight_set {
                            return Err(format!(
                                "RelationUpdate mismatch: expected {weight_set:?}, found {actual_weight:?}"
                            ));
                        }
                    } else {
                        return Err(format!(
                            "RelationUpdate references non-existent edge: {source} -> {sink}"
                        ));
                    }
                } else {
                    return Err(format!(
                        "RelationUpdate references non-existent nodes: {source} -> {sink}"
                    ));
                }
            }
            BeliefEvent::NodesRemoved(bids, _) => {
                for bid in bids {
                    if self.states().contains_key(bid) {
                        return Err(format!(
                            "NodesRemoved claims {bid} was removed but it still exists"
                        ));
                    }
                }
            }
            BeliefEvent::NodeUpdate(_keys, toml_str, _) => {
                // Validate that the node exists with matching state
                if let Ok(node) = BeliefNode::try_from(&toml_str[..]) {
                    if let Some(existing) = self.states().get(&node.bid) {
                        if existing != &node {
                            return Err(format!(
                                "NodeUpdate mismatch for {}: expected {:?}, found {:?}",
                                node.bid, node, existing
                            ));
                        }
                    } else {
                        return Err(format!(
                            "NodeUpdate claims {} exists but it's not in states",
                            node.bid
                        ));
                    }
                }
            }
            // For other event types, we could add validation but they're less critical
            _ => {}
        }
        Ok(())
    }

    fn check_path_invariants(&self) -> Vec<String> {
        let mut errors = Vec::<String>::new();
        let relations = self.relations();

        // Collect all API nodes - these serve as anchor points for different schema versions
        let api_nodes: BTreeSet<Bid> = self
            .states()
            .iter()
            .filter(|(_, node)| node.kind.contains(BeliefKind::API))
            .map(|(bid, _)| *bid)
            .collect();
        let api_net_guards = api_nodes
            .iter()
            .filter_map(|b| self.paths().get_map(b))
            .collect::<Vec<ArcRwLockReadGuard<_, PathMap>>>();

        let mut pathless_nodes = BTreeSet::default();
        let mut stateless_nodes = BTreeSet::default();
        for bid in relations
            .as_graph()
            .node_indices()
            .map(|node_idx| relations.as_graph()[node_idx])
        {
            if !self.states().contains_key(&bid) {
                stateless_nodes.insert(bid);
            }

            // Check if this sink has a path to ANY API node (across all path maps)
            // or if the sink itself is an API node
            let paths_guard = self.paths();
            let has_api_path = api_net_guards
                .iter()
                .any(|pm_lock| pm_lock.path(&bid, &paths_guard).is_some());

            if !has_api_path {
                pathless_nodes.insert(bid);
            }
        }
        if !stateless_nodes.is_empty() {
            errors.push(format!(
                "[BeliefBase.built_in_test: invariant 1.0] relation nodes must map to \
                 a belief node. States for the following BIDs are missing:\n\t{}",
                stateless_nodes
                    .iter()
                    .map(|b| b.to_string())
                    .collect::<Vec<String>>()
                    .join("\n\t")
            ));
        }
        if !pathless_nodes.is_empty() {
            errors.push(format!(
                "[BeliefBase.built_in_test: invariant 1.1] relation nodes must have a path to \
                 an API node (or be an API node themselves). Paths for the following nodes are \
                 missing:\n\
                 \t{}\n\
                 set:\n{}",
                pathless_nodes
                    .iter()
                    .map(|b| b.to_string())
                    .collect::<Vec<String>>()
                    .join("\n\t"),
                self.clone().consume()
            ));
        }
        errors
    }

    /// Ensure the BeliefBase static invariants are true.
    ///
    /// The operational rules must be checked with test cases.
    ///
    /// Caution! This is not cheap in terms of computation or memory.
    pub fn built_in_test(&self, full: bool) -> Vec<String> {
        // tracing::debug!(
        //     "Invariant #1 is checked in check_path_invariants"
        // );
        let mut errors = self.check_path_invariants();

        if !full {
            return errors;
        }
        // tracing::debug!("Check invariant #0");
        let relations = self.relations();
        for scc in kosaraju_scc(&relations.as_subgraph(WeightKind::Epistemic, false)).iter() {
            if scc.len() > 1 {
                errors.push(format!(
                    "[BeliefBase::built_in_test invariant 0] epistemic edges contain cycle: {scc:?}"
                ));
            }
        }

        for scc in kosaraju_scc(&relations.as_subgraph(WeightKind::Pragmatic, false)).iter() {
            if scc.len() > 1 {
                errors.push(format!(
                    "[BeliefBase::built_in_test invariant 0] pragmatic edges contain cycle: {scc:?}"
                ));
            }
        }
        for scc in kosaraju_scc(&relations.as_subgraph(WeightKind::Section, false)).iter() {
            if scc.len() > 1 {
                errors.push(format!(
                    "[BeliefBase::built_in_test invariant 0] subsection edges contain cycle: {scc:?}"
                ));
            }
        }

        // tracing::debug!("Check invariant #2");
        for node in self.states().values() {
            let bid = &node.bid;
            let mut kind_map: BTreeMap<WeightKind, Vec<u16>> = BTreeMap::new();
            if let Some(node_idx) = self.bid_to_index(bid) {
                for edge in relations
                    .as_graph()
                    .edges_directed(node_idx, Direction::Incoming)
                {
                    for (kind, weight_data) in edge.weight().weights.iter() {
                        let sort_key: u16 = weight_data
                            .get(crate::properties::WEIGHT_SORT_KEY)
                            .unwrap_or(0);
                        kind_map.entry(*kind).or_default().push(sort_key);
                    }
                }
            }

            for (kind, mut indices) in kind_map {
                indices.sort();
                if node.kind.contains(BeliefKind::Trace) {
                    // If we have a trace node, the best we can check is to ensure there are no
                    // duplicates in our indices
                    let mut deduped = indices.clone();
                    deduped.dedup();
                    if indices.len() != deduped.len() {
                        errors.push(format!(
                         "[BeliefBase::build_in_test invariant 2] {bid} (tagged as trace) {kind:?} edges \
                         contains duplicate edge indices. Received {indices:?}"
                     ))
                    }
                } else {
                    let expected: Vec<u16> = (0..indices.len() as u16).collect();
                    if indices != expected {
                        errors.push(format!(
                            "[BeliefBase::built_in_test invariant 2] {bid} {kind:?} edges are not \
                            correctly sorted. Received {indices:?}, Expected: {expected:?}"
                        ));
                    }
                }
            }
        }
        errors
    }

    /// Processes a `BeliefEvent` to mutate the `BeliefBase`.
    ///
    /// This function is the primary entry point for all state changes. It is responsible for
    /// maintaining the integrity and invariants of the set.
    ///
    /// # Event Origin Handling
    /// - `EventOrigin::Local`: Event generated by this BeliefBase. State already updated,
    ///   so we validate consistency in debug builds and skip reapplication.
    /// - `EventOrigin::Remote`: Event from external source (DbConnection, file, network).
    ///   Must apply to synchronize state.
    pub fn process_event(
        &mut self,
        event: &BeliefEvent,
    ) -> Result<Vec<BeliefEvent>, BuildonomyError> {
        // Handle Local events: validate consistency but skip reapplication
        if let Some(crate::event::EventOrigin::Local) = event.origin() {
            #[cfg(debug_assertions)]
            {
                if let Err(e) = self.validate_local_event(event) {
                    tracing::warn!("Local event validation failed: {}", e);
                    debug_assert!(false, "Local event doesn't match internal state: {event:?}");
                }
            }
            return Ok(vec![]); // Event already applied, nothing more to do
        }

        // Handle Remote events: apply changes and generate derivatives
        let mut derivative_events = vec![];
        match event {
            BeliefEvent::NodeUpdate(keys, toml_str, _) => {
                let node = BeliefNode::try_from(&toml_str[..])?;
                derivative_events.append(&mut self.insert_state(node.clone(), keys));
            }

            BeliefEvent::NodesRemoved(bids, _) => {
                let bid_set: BTreeSet<Bid> = bids.iter().copied().collect();
                derivative_events.append(&mut self.remove_nodes(&bid_set));
            }
            // This case should handled by other, more atomic transactions. At least it is via
            // [GraphBuilder].
            BeliefEvent::NodeRenamed(_from, _to, _) => {}
            BeliefEvent::PathAdded(..)
            | BeliefEvent::PathUpdate(..)
            | BeliefEvent::PathsRemoved(..) => {
                // Path events are generated by PathMapMap and should not be processed here
                // They're returned as derivatives for DbConnection and other subscribers
            }
            BeliefEvent::RelationUpdate(source, sink, weight_set, _) => {
                // update_relation handles both reindexing and path event generation
                let mut reindex_events = self.update_relation(*source, *sink, weight_set.clone());
                derivative_events.append(&mut reindex_events);
            }
            BeliefEvent::RelationChange(..) => {
                if let Some(relation_mutated_event) = self.generate_edge_update(event) {
                    let &BeliefEvent::RelationUpdate(source, sink, ref weight_set, _) =
                        &relation_mutated_event
                    else {
                        panic!("Unexpected return value from BeliefBase::generate_edge_update");
                    };
                    let mut reindex_events = self.update_relation(source, sink, weight_set.clone());
                    derivative_events.push(relation_mutated_event);
                    derivative_events.append(&mut reindex_events);
                }
            }
            BeliefEvent::RelationRemoved(source, sink, _) => {
                // Call update_relation with empty WeightSet to trigger proper reindexing
                // of remaining edges on the sink, ensuring contiguous sort indices [0..N)
                let mut reindex_events = self.update_relation(*source, *sink, WeightSet::default());
                derivative_events.append(&mut reindex_events);
            }
            BeliefEvent::FileParsed(_) => {
                // Metadata only, handled by Transaction for mtime tracking
            }
            BeliefEvent::BalanceCheck => {
                // Just run a quick check for balanceCheck operations, not a full built_in_test check
                self.index_sync(false);
            }
            BeliefEvent::BuiltInTest => {
                // Run a full built_in_test check
                self.index_sync(true);
            }
        };

        // Build event queue: original event + all derivative events
        let mut event_queue: Vec<&BeliefEvent> = vec![event];
        event_queue.extend(derivative_events.iter());

        // Process ALL events through PathMapMap to generate and apply path mutations
        let mut path_events = {
            let mut pmm = self.paths.write_arc();
            pmm.process_event_queue(&event_queue, &self.relations)
        };

        // Append path events to derivatives for DbConnection and other subscribers
        derivative_events.append(&mut path_events);

        Ok(derivative_events)
    }

    /// Insert or replace a state while preserving path uniqueness
    ///
    /// Return a vector of events for each node that was renamed when matching on the merge keys.
    fn insert_state(&mut self, node: BeliefNode, merge: &[NodeKey]) -> Vec<BeliefEvent> {
        let mut events = Vec::<BeliefEvent>::new();
        let mut to_replace = BTreeSet::<Bid>::new();
        for key in merge.iter() {
            let results = self.evaluate_expression(&Expression::from(key));
            if let Some(node) = BeliefBase::from(results).get(key) {
                to_replace.insert(node.bid);
            }
        }
        to_replace.remove(&node.bid);
        if !to_replace.is_empty() {
            tracing::debug!(
                "insert_state: Node bid={}, id={:?}, kind={:?} will REPLACE nodes: {:?}. Merge keys: {:?}",
                node.bid, node.id, node.kind, to_replace, merge
            );
        }

        let mut updated = false;
        let is_new_node = !self.states.contains_key(&node.bid);
        if is_new_node {
            updated = true;
        } else if let Some(old) = self.states.get(&node.bid) {
            if *old != node {
                updated = true;
            }
        }

        let bid = node.bid;
        if updated {
            self.states.insert(bid, node);
            self.brefs.insert(bid.namespace(), bid);
        }

        for replaced in to_replace.iter() {
            // Call replace_bid BEFORE removing from states, because replace_bid
            // needs to transfer edges from the replaced node to the new node
            events.push(BeliefEvent::NodeRenamed(*replaced, bid, EventOrigin::Local));
            events.append(&mut self.replace_bid(*replaced, bid));

            // Now remove from states (replace_bid already removed from graph)
            self.states.remove(replaced);
            self.brefs.remove(&replaced.namespace());
        }
        // Our bid_to_indexes must be regenerated
        if updated || !to_replace.is_empty() {
            self.index_dirty.store(true, Ordering::SeqCst);
        }
        if !to_replace.is_empty() {
            events.push(BeliefEvent::NodesRemoved(
                to_replace.into_iter().collect(),
                EventOrigin::Local,
            ));
        }
        events
    }

    fn remove_nodes(&mut self, bids: &BTreeSet<Bid>) -> Vec<BeliefEvent> {
        if bids.is_empty() {
            return vec![];
        }

        // Ensure index is rebuilt before acquiring locks to avoid deadlock
        self.index_sync(false);

        let mut sink_kinds: BTreeMap<Bid, BTreeSet<WeightKind>> = BTreeMap::new();
        {
            let relations = self.relations.read_arc();
            let bid_to_index = self.bid_to_index.read();
            for bid in bids {
                if let Some(&node_idx) = bid_to_index.get(bid) {
                    // Find all sinks that this node has edges to, and what WeightKinds
                    for edge in relations.as_graph().edges(node_idx) {
                        let sink = relations.as_graph()[edge.target()];
                        let kinds = edge
                            .weight()
                            .weights
                            .keys()
                            .copied()
                            .collect::<BTreeSet<_>>();
                        sink_kinds.entry(sink).or_default().extend(kinds);
                    }
                }
            }
        }

        // Remove nodes from states
        for bid in bids {
            if self.states.remove(bid).is_some() {
                self.brefs.remove(&bid.namespace());
            }
        }

        // Remove nodes from graph
        let mut relations = self.relations.write_arc();
        relations
            .as_graph_mut()
            .retain_nodes(|g, idx| !bids.contains(&g[idx]));
        drop(relations);
        // Regenerate our bid_to_index cache
        if !bids.is_empty() {
            self.index_dirty.store(true, Ordering::SeqCst);
        }
        // Reindex edges for affected sinks using the centralized reindex_sink_edges
        let mut derivative_events = vec![];
        for (sink, kinds) in sink_kinds {
            let mut reindex_events = self.reindex_sink_edges(&sink, &kinds);
            derivative_events.append(&mut reindex_events);
        }

        derivative_events
    }

    fn generate_edge_update(&self, event: &BeliefEvent) -> Option<BeliefEvent> {
        self.index_sync(false);
        let BeliefEvent::RelationChange(source, sink, kind, maybe_weight, origin) = event else {
            return None;
        };

        let present_weight = if let (Some(source_idx), Some(sink_idx)) =
            (self.bid_to_index(source), self.bid_to_index(sink))
        {
            self.relations()
                .as_graph()
                .find_edge(source_idx, sink_idx)
                .map(|edge_idx| self.relations().as_graph()[edge_idx].clone())
        } else {
            None
        };
        let mut new_weights = present_weight.clone().unwrap_or(WeightSet::default());
        let mut changed = false;
        if let Some(weight) = maybe_weight {
            let new_weight = new_weights
                .weights
                .entry(*kind)
                .and_modify(|e| {
                    for (k, new_v) in weight.payload.iter() {
                        // Special handling for path merging
                        if k == WEIGHT_DOC_PATHS || k == "doc_path" {
                            // Get existing paths
                            let existing_paths = e.get_doc_paths();

                            // Get incoming paths (handle both old and new formats)
                            let incoming_paths = if k == WEIGHT_DOC_PATHS {
                                // New format: Vec<String>
                                new_v.clone().try_into::<Vec<String>>().unwrap_or_default()
                            } else {
                                // Old format: String
                                if let Ok(path) = new_v.clone().try_into::<String>() {
                                    vec![path]
                                } else {
                                    vec![]
                                }
                            };

                            // Merge intelligently: deduplicate and append
                            let mut merged: std::collections::BTreeSet<String> =
                                existing_paths.into_iter().collect();
                            let before_len = merged.len();
                            merged.extend(incoming_paths);

                            if merged.len() != before_len {
                                // Convert back to Vec and set using new format
                                let merged_vec: Vec<String> = merged.into_iter().collect();
                                if let Ok(()) = e.set_doc_paths(merged_vec) {
                                    changed = true;
                                }
                            }
                            // Skip the default insert logic below for path keys
                            continue;
                        }

                        // Standard merge logic for non-path keys
                        if let Some(present_v) = e.payload.get(k) {
                            if new_v != present_v {
                                e.payload.insert(k.to_string(), new_v.clone());
                                changed = true;
                            }
                        } else {
                            e.payload.insert(k.to_string(), new_v.clone());
                            changed = true
                        }
                    }
                })
                .or_insert_with(|| {
                    changed = true;
                    let mut normalized_weight = weight.clone();
                    // Normalize old format to new format for new edges
                    #[allow(deprecated)]
                    if normalized_weight.payload.contains_key("doc_path") {
                        if let Some(path) = normalized_weight.get::<String>("doc_path") {
                            normalized_weight.payload.remove("doc_path");
                            let _ = normalized_weight.set_doc_paths(vec![path]);
                        }
                    }
                    normalized_weight
                });
            // If this is a new edge entirely (no present_weight), always mark as changed
            if present_weight.is_none() {
                changed = true;
            }
            if new_weight.payload.get(WEIGHT_SORT_KEY).is_none() {
                let sink_kind_max_weight: Option<u16> = if let Some(sink_idx) =
                    self.bid_to_index(sink)
                {
                    self.relations()
                        .as_graph()
                        .edges_directed(sink_idx, Direction::Incoming)
                        .filter_map(|edge| {
                            // So long as we always insert an edge with a sort_key, we know that source->sink is
                            // not in this set.
                            debug_assert!(self.relations().as_graph()[edge.source()] != *source);
                            edge.weight()
                                .get(kind)
                                .and_then(|w| w.get::<u16>(WEIGHT_SORT_KEY))
                        })
                        .max()
                } else {
                    None
                };
                new_weight
                    .set(
                        WEIGHT_SORT_KEY,
                        sink_kind_max_weight.map(|w: u16| w + 1).unwrap_or(0),
                    )
                    .expect("To be able to put a u16 in as a toml_edit value");
                changed = true;
            }
        } else {
            changed = new_weights.remove(kind).is_some();
        }

        if changed {
            // tracing::debug!("Generating RelationUpdate");
            Some(BeliefEvent::RelationUpdate(
                *source,
                *sink,
                new_weights,
                *origin,
            ))
        } else {
            None
        }
    }

    /// Updates a relation edge and reindexes all edges for affected WeightKinds on the sink
    /// to ensure contiguous indices [0..N).
    ///
    /// Returns derivative RelationUpdate events for any edges whose indices changed.
    fn update_relation(
        &mut self,
        source: Bid,
        sink: Bid,
        new_weight_set: WeightSet,
    ) -> Vec<BeliefEvent> {
        while self.relations.is_locked() {
            tracing::info!("[BeliefBase::update_relation] Waiting for write access to relations");
            std::thread::sleep(std::time::Duration::from_millis(100));
        }

        let maybe_source_idx = self.bid_to_index(&source);
        let maybe_sink_idx = self.bid_to_index(&sink);
        if maybe_source_idx.is_none() || maybe_sink_idx.is_none() {
            // Skip if either node has been removed
            tracing::warn!(
                "Skipping update_relation({} -[{}]-> {}), source is missing: {}, sink is missing: {}, index_dirty: {}",
                source,
                new_weight_set.weights.keys().map(|k| k.to_string()).collect::<Vec<String>>().join(", "),
                sink,
                maybe_source_idx.is_none(),
                maybe_sink_idx.is_none(),
                self.index_dirty.load(Ordering::SeqCst)
            );
            return vec![];
        }

        let source_idx = maybe_source_idx.unwrap();
        let sink_idx = maybe_sink_idx.unwrap();
        let mut relations = self.relations.write_arc();
        let old_weight_set = {
            if let Some(edge_idx) = relations.as_graph().find_edge(source_idx, sink_idx) {
                relations
                    .as_graph()
                    .edge_weight(edge_idx)
                    .expect("We got this edge index from the graph so it should be valid.")
                    .clone()
            } else {
                WeightSet::default()
            }
        };
        // If we used to have more WeightKinds in this edge than the new_weights, we need to reindex
        // the sink's edges.
        let affected_kinds = old_weight_set
            .difference(&new_weight_set)
            .weights
            .keys()
            .copied()
            .collect();

        // Update or add/remove the edge
        if new_weight_set.is_empty() {
            if let Some(edge_idx) = relations.as_graph().find_edge(source_idx, sink_idx) {
                relations.as_graph_mut().remove_edge(edge_idx);
            }
        } else if let Some(edge_idx) = relations.as_graph().find_edge(source_idx, sink_idx) {
            let edge_weight = relations
                .as_graph_mut()
                .edge_weight_mut(edge_idx)
                .expect("We got this edge index from the graph, why can't we access it?");
            *edge_weight = new_weight_set;
        } else {
            relations
                .as_graph_mut()
                .add_edge(source_idx, sink_idx, new_weight_set);
        }
        drop(relations);

        // Reindex all edges for each affected WeightKind on this sink
        // Path events will be generated later by process_event_queue
        self.reindex_sink_edges(&sink, &affected_kinds)
    }

    /// Reindexes all edges for the specified WeightKinds on a sink to be contiguous [0..N).
    /// Returns RelationUpdate events for any edges whose indices changed.
    fn reindex_sink_edges(&mut self, sink: &Bid, kinds: &BTreeSet<WeightKind>) -> Vec<BeliefEvent> {
        let mut derivative_events = vec![];
        if kinds.is_empty() {
            return derivative_events;
        }

        let Some(sink_idx) = self.bid_to_index(sink) else {
            tracing::warn!(
                "could not acquire bid to index for {}, can't reindex sink edges!",
                sink
            );
            return derivative_events;
        };

        let mut changed = BTreeMap::<(_, _), BTreeMap<WeightKind, u16>>::new();
        let mut relations = self.relations.write_arc();
        let incoming_edges = {
            relations
                .as_graph()
                .edges_directed(sink_idx, Direction::Incoming)
                .map(|edge| {
                    (
                        edge.source(),
                        edge.target(),
                        BTreeMap::from_iter(edge.weight().weights.iter().filter_map(|(k, v)| {
                            v.get::<u16>(WEIGHT_SORT_KEY).map(|idx| (*k, idx))
                        })),
                    )
                })
                .collect::<Vec<(_, _, BTreeMap<WeightKind, u16>)>>()
        };

        for kind in kinds {
            // Collect all edges with this WeightKind, sorted by current index
            let mut kind_set = incoming_edges
                .iter()
                .filter_map(|(source_idx, sink_idx, ks)| {
                    ks.get(kind)
                        .map(|weight_idx| (*source_idx, *sink_idx, *weight_idx))
                })
                .collect::<Vec<(_, _, u16)>>();
            kind_set.sort_by_key(|(_, _, old_idx)| *old_idx);
            for (new_idx, (source_idx, sink_idx, old_idx)) in kind_set.into_iter().enumerate() {
                if new_idx as u16 != old_idx {
                    let changed_indices = changed.entry((source_idx, sink_idx)).or_default();
                    changed_indices.insert(*kind, new_idx as u16);
                }
            }
        }

        for ((source_idx, sink_idx), changed_indices) in changed.into_iter() {
            let (edge_idx, source, sink) = {
                let rel_graph = relations.as_graph();
                let edge_idx = rel_graph.find_edge(source_idx, sink_idx).expect(
                    "We got these node indices from the graph, own a mutable ARC \
                    to relations, and have not removed any edges since acquiring, \
                    so they should be valid.",
                );
                let source = rel_graph[source_idx];
                let sink = rel_graph[sink_idx];
                (edge_idx, source, sink)
            };
            let edge_weight = relations.as_graph_mut().edge_weight_mut(edge_idx).expect(
                "We got this edge index from the graph on the prior line so it should be valid.",
            );
            for (kind, new_idx) in changed_indices.into_iter() {
                let weight = edge_weight.weights.get_mut(&kind).expect(
                    "We only insert kind into changed_indices when we discovered kind \
                    in the weight. (see above how incoming_edges is constructed).",
                );
                weight.set(WEIGHT_SORT_KEY, new_idx).ok();
            }
            derivative_events.push(BeliefEvent::RelationUpdate(
                source,
                sink,
                edge_weight.clone(),
                EventOrigin::Local,
            ));
        }
        derivative_events
    }

    fn replace_bid(&mut self, replaced_bid: Bid, new_bid: Bid) -> Vec<BeliefEvent> {
        assert!(
            self.states.contains_key(&new_bid),
            "replace_bid called but new_bid is not in states"
        );
        let mut derivative_events = vec![];

        self.index_sync(false);

        if let Some(replaced_idx) = self.bid_to_index(&replaced_bid) {
            let new_idx_opt = self.bid_to_index(&new_bid);

            let mut relations = self.relations.write_arc();
            let new_idx = new_idx_opt.unwrap_or_else(|| relations.as_graph_mut().add_node(new_bid));

            let mut outgoing = relations
                .as_graph()
                .neighbors_directed(replaced_idx, petgraph::Direction::Outgoing)
                .detach();
            while let Some((edge_idx, sink_idx)) = outgoing.next(relations.as_graph()) {
                let sink = relations.as_graph()[sink_idx];
                let mut from_weight = relations
                    .as_graph_mut()
                    .remove_edge(edge_idx)
                    .expect("Edge should exist");
                from_weight.weights.remove(&WeightKind::Section);
                derivative_events.push(BeliefEvent::RelationRemoved(
                    replaced_bid,
                    sink,
                    EventOrigin::Local,
                ));

                if let Some(existing_edge_idx) = relations.as_graph().find_edge(new_idx, sink_idx) {
                    let existing_weight = &mut relations.as_graph_mut()[existing_edge_idx];
                    *existing_weight = existing_weight.union(&from_weight);
                } else if !from_weight.is_empty() {
                    relations
                        .as_graph_mut()
                        .add_edge(new_idx, sink_idx, from_weight);
                }
            }

            let mut incoming = relations
                .as_graph()
                .neighbors_directed(replaced_idx, petgraph::Direction::Incoming)
                .detach();
            while let Some((edge_idx, source_idx)) = incoming.next(relations.as_graph()) {
                let source = relations.as_graph()[source_idx];
                let mut from_weight = relations
                    .as_graph_mut()
                    .remove_edge(edge_idx)
                    .expect("Edge should exist");
                from_weight.weights.remove(&WeightKind::Section);
                derivative_events.push(BeliefEvent::RelationRemoved(
                    source,
                    replaced_bid,
                    EventOrigin::Local,
                ));

                if let Some(existing_edge_idx) = relations.as_graph().find_edge(source_idx, new_idx)
                {
                    let existing_weight = &mut relations.as_graph_mut()[existing_edge_idx];
                    *existing_weight = existing_weight.union(&from_weight);
                } else if !from_weight.is_empty() {
                    relations
                        .as_graph_mut()
                        .add_edge(source_idx, new_idx, from_weight);
                }
            }
            relations.as_graph_mut().remove_node(replaced_idx);
            self.index_dirty.store(true, Ordering::SeqCst);
        }
        derivative_events
    }

    /// If the BeliefBase is singular (only one state in the set) returns a clone of the
    /// state. Otherwise None
    pub fn into_state(&mut self) -> Option<BeliefNode> {
        let BeliefGraph { mut states, .. } = self.consume();
        let mut maybe_node = None;
        while let Some((_, a_state)) = states.pop_first() {
            if a_state.bid != self.api.bid {
                maybe_node = Some(a_state);
                break;
            }
        }
        if !states.is_empty() {
            tracing::warn!(
                "Converted a multi-node BeliefBase into a BeliefNode. Remaining nodes: {:?}",
                states
            );
        }
        maybe_node
    }

    pub fn merge(&mut self, rhs: &BeliefGraph) {
        let mut lhs = self.consume();
        lhs.union_mut(rhs);
        *self = BeliefBase::from(lhs);
    }

    pub fn set_merge(&mut self, rhs_set: &mut BeliefBase) {
        let mut lhs = self.consume();
        let rhs = rhs_set.consume();
        lhs.union_mut(&rhs);
        *self = BeliefBase::from(lhs);
    }

    /// Remove all relations where source or sink is not contained in the states set, or in the
    /// optional to_retain Bid set.
    pub fn trim(&mut self, to_retain: Option<BTreeSet<Bid>>) {
        while self.relations.is_locked() {
            tracing::info!("[BeliefBase::trim] Waiting for write access to relations");
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let mut write_relations = self.relations.write_arc();
        let retainable_set =
            to_retain.unwrap_or_else(|| BTreeSet::from_iter(self.states().keys().copied()));
        let to_remove = write_relations
            .as_graph()
            .edge_indices()
            .filter_map(|edge_idx| {
                if let Some((source_idx, sink_idx)) =
                    write_relations.as_graph().edge_endpoints(edge_idx)
                {
                    let source = write_relations.as_graph()[source_idx];
                    let sink = write_relations.as_graph()[sink_idx];
                    if !retainable_set.contains(&source) || !retainable_set.contains(&sink) {
                        Some((edge_idx, source, sink))
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();

        let mut remove_events = Vec::new();
        for (edge_idx, source, sink) in to_remove.into_iter().rev() {
            write_relations.as_graph_mut().remove_edge(edge_idx);
            remove_events.push(BeliefEvent::RelationRemoved(
                source,
                sink,
                EventOrigin::Local,
            ));
        }
    }

    // TODO this can be more efficient for some StatePreds (Bid and Bref) by using map.get instead
    // of filter operations.
    pub fn filter_states(
        &self,
        pred: &StatePred,
        rhs: Option<&BTreeMap<Bid, BeliefNode>>,
        invert: bool,
    ) -> BTreeMap<Bid, BeliefNode> {
        match pred {
            StatePred::Path(path_vec) => {
                let paths_guard = self.paths();
                let bids = BTreeSet::from_iter(path_vec.iter().filter_map(|pth| {
                    paths_guard
                        .api_map()
                        .get(pth, &paths_guard)
                        .map(|(_net, bid)| bid)
                }));
                BTreeMap::from_iter(
                    bids.iter()
                        .filter_map(|bid| self.states().get(bid).map(|node| (*bid, node.clone()))),
                )
            }
            StatePred::NetPath(net, path) => {
                let paths_guard = self.paths();
                let maybe_bid = paths_guard
                    .get_map(net)
                    .and_then(|pm| pm.get(path, &paths_guard).map(|(_net, bid)| bid));
                BTreeMap::from_iter(
                    maybe_bid
                        .iter()
                        .filter_map(|bid| self.states().get(bid).map(|node| (*bid, node.clone()))),
                )
            }
            StatePred::NetPathIn(net) => {
                let paths_guard = self.paths();
                let path_bid_pairs = paths_guard
                    .get_map(net)
                    .map(|pm| {
                        pm.all_net_paths(&paths_guard, &mut std::collections::BTreeSet::new())
                    })
                    .unwrap_or_default();
                // Extract just the bids from (path, bid) tuples
                let bids: Vec<Bid> = path_bid_pairs.iter().map(|(_path, bid)| *bid).collect();
                BTreeMap::from_iter(
                    bids.iter()
                        .filter_map(|bid| self.states().get(bid).map(|node| (*bid, node.clone()))),
                )
            }
            StatePred::Title(net, title) => {
                let paths_guard = self.paths();
                let maybe_bid = paths_guard.get_map(net).and_then(|pm| {
                    pm.get_from_title_regex(title, &paths_guard)
                        .map(|(_net, bid)| bid)
                });
                BTreeMap::from_iter(
                    maybe_bid
                        .iter()
                        .filter_map(|bid| self.states().get(bid).map(|node| (*bid, node.clone()))),
                )
            }
            _ => BTreeMap::from_iter(
                self.states
                    .iter()
                    .chain(rhs.unwrap_or(&BTreeMap::default()).iter())
                    .filter_map(|(bid, state)| {
                        let is_match = pred.match_state(state);
                        if (is_match && !invert) || (!is_match && invert) {
                            Some((*bid, state.clone()))
                        } else {
                            None
                        }
                    }),
            ),
        }
    }

    pub fn filter_states_mut(
        &mut self,
        pred: &StatePred,
        rhs: Option<&BTreeMap<Bid, BeliefNode>>,
        invert: bool,
    ) {
        self.states = self.filter_states(pred, rhs, invert);
    }

    /// Evaluate an expression and mark all resulting nodes as Trace, returning only
    /// Subsection relations. This is used during balance operations to prevent pulling
    /// in the entire graph when traversing upstream.
    pub fn evaluate_expression_as_trace(
        &self,
        expr: &Expression,
        weight_set: WeightSet,
    ) -> BeliefGraph {
        self.index_sync(false);
        match expr {
            Expression::StateIn(state_pred) => {
                let mut states = self.filter_states(state_pred, None, false);
                // Mark all states as Trace
                for node in states.values_mut() {
                    node.kind.insert(BeliefKind::Trace);
                }
                let state_set = states.keys().copied().collect::<Vec<Bid>>();
                // Only return relations matching the weight filter
                let relations = BidGraph::from(
                    self.relations()
                        .filter(&RelationPred::SourceIn(state_set.clone()), false)
                        .filter(&RelationPred::Kind(weight_set), false),
                );
                // Add sink nodes to states (marked as Trace) so union_mut doesn't filter out the relations
                for edge in relations.as_graph().raw_edges() {
                    let sink = relations.as_graph()[edge.target()];
                    if let BTreeEntry::Vacant(e) = states.entry(sink) {
                        if let Some(sink_state) = self.states().get(&sink) {
                            let mut trace_sink = sink_state.clone();
                            trace_sink.kind.insert(BeliefKind::Trace);
                            e.insert(trace_sink);
                        }
                    }
                }
                BeliefGraph { states, relations }
            }
            Expression::StateNotIn(state_pred) => {
                let mut states = self.filter_states(state_pred, None, true);
                for node in states.values_mut() {
                    node.kind.insert(BeliefKind::Trace);
                }
                let state_set = states.keys().copied().collect::<Vec<Bid>>();
                let relations = BidGraph::from(
                    self.relations()
                        .filter(&RelationPred::SourceIn(state_set.clone()), false)
                        .filter(&RelationPred::Kind(weight_set), false),
                );
                // Add sink nodes to states (marked as Trace) so union_mut doesn't filter out the relations
                for edge in relations.as_graph().raw_edges() {
                    let sink = relations.as_graph()[edge.target()];
                    if let BTreeEntry::Vacant(e) = states.entry(sink) {
                        if let Some(sink_state) = self.states().get(&sink) {
                            let mut trace_sink = sink_state.clone();
                            trace_sink.kind.insert(BeliefKind::Trace);
                            e.insert(trace_sink);
                        }
                    }
                }
                BeliefGraph { states, relations }
            }
            // Relation expression's use the standard evaluate_expression logic
            Expression::RelationIn(..) | Expression::RelationNotIn(..) => {
                self.evaluate_expression(expr)
            }
            Expression::Dyad(lhs_p, op, rhs_p) => {
                let mut lhs = self.evaluate_expression_as_trace(lhs_p, weight_set.clone());
                let rhs = self.evaluate_expression_as_trace(rhs_p, weight_set);
                match op {
                    SetOp::Union => lhs.union_mut(&rhs),
                    SetOp::Intersection => lhs.intersection_mut(&rhs),
                    SetOp::Difference => lhs.difference_mut(&rhs),
                    SetOp::SymmetricDifference => lhs.symmetric_difference_mut(&rhs),
                }
                lhs
            }
        }
    }

    pub fn evaluate_expression(&self, expr: &Expression) -> BeliefGraph {
        self.index_sync(false);
        match expr {
            Expression::StateIn(state_pred) => {
                let mut states = self.filter_states(state_pred, None, false);
                let state_set = states.keys().copied().collect::<Vec<Bid>>();
                let relations = BidGraph::from(
                    self.relations()
                        .filter(&RelationPred::NodeIn(state_set), false),
                );
                tracing::debug!("States: {states:?}\nRelations: {relations:?}");
                // Add sink nodes to maintain referential integrity
                // Mark them as Trace since we haven't loaded their full relation set
                for edge in relations.as_graph().raw_edges() {
                    let sink = relations.as_graph()[edge.target()];
                    if let BTreeEntry::Vacant(e) = states.entry(sink) {
                        if let Some(sink_state) = self.states().get(&sink) {
                            let mut trace_sink = sink_state.clone();
                            trace_sink.kind.insert(BeliefKind::Trace);
                            e.insert(trace_sink);
                        }
                    }
                    let source = relations.as_graph()[edge.source()];
                    if let BTreeEntry::Vacant(e) = states.entry(source) {
                        if let Some(source_state) = self.states().get(&source) {
                            let mut trace_source = source_state.clone();
                            trace_source.kind.insert(BeliefKind::Trace);
                            e.insert(trace_source);
                        }
                    }
                }
                BeliefGraph { states, relations }
            }
            Expression::StateNotIn(state_pred) => {
                let mut states = self.filter_states(state_pred, None, true);
                let state_set = states.keys().copied().collect::<Vec<Bid>>();
                let relations = BidGraph::from(
                    self.relations()
                        .filter(&RelationPred::NodeIn(state_set), false),
                );
                // Add sink nodes to maintain referential integrity
                // Mark them as Trace since we haven't loaded their full relation set
                for edge in relations.as_graph().raw_edges() {
                    let sink = relations.as_graph()[edge.target()];
                    if let BTreeEntry::Vacant(e) = states.entry(sink) {
                        if let Some(sink_state) = self.states().get(&sink) {
                            let mut trace_sink = sink_state.clone();
                            trace_sink.kind.insert(BeliefKind::Trace);
                            e.insert(trace_sink);
                        }
                    }
                    let source = relations.as_graph()[edge.source()];
                    if let BTreeEntry::Vacant(e) = states.entry(source) {
                        if let Some(source_state) = self.states().get(&source) {
                            let mut trace_source = source_state.clone();
                            trace_source.kind.insert(BeliefKind::Trace);
                            e.insert(trace_source);
                        }
                    }
                }
                BeliefGraph { states, relations }
            }
            Expression::RelationIn(relation_pred) => {
                let mut states = BTreeMap::new();
                let mut edges = Vec::new();
                for edge in self.relations().as_graph().raw_edges() {
                    let source = self.relations().as_graph()[edge.source()];
                    let sink = self.relations().as_graph()[edge.target()];
                    let rel = BeliefRefRelation {
                        source: &source,
                        sink: &sink,
                        weights: &edge.weight,
                    };
                    if relation_pred.match_ref(&rel) {
                        if let BTreeEntry::Vacant(e) = states.entry(sink) {
                            if let Some(state) = self.states().get(&sink) {
                                let mut sink_state = state.clone();
                                // We don't have the entirety of the node relation set, so insert
                                // the trace nodekind graph color
                                sink_state.kind.insert(BeliefKind::Trace);
                                e.insert(sink_state.clone());
                            }
                        }
                        if let BTreeEntry::Vacant(e) = states.entry(source) {
                            if let Some(state) = self.states().get(&source) {
                                let mut source_state = state.clone();
                                // We don't have the entirety of the node relation set, so insert
                                // the trace nodekind graph color
                                source_state.kind.insert(BeliefKind::Trace);
                                e.insert(source_state.clone());
                            }
                        }
                        edges.push(BeliefRelation::from(&rel));
                    }
                }
                BeliefGraph {
                    states,
                    relations: BidGraph::from_edges(edges),
                }
            }
            Expression::RelationNotIn(relation_pred) => {
                let mut states = BTreeMap::new();
                let mut edges = Vec::new();
                for edge in self.relations().as_graph().raw_edges() {
                    let source = self.relations().as_graph()[edge.source()];
                    let sink = self.relations().as_graph()[edge.target()];
                    let rel = BeliefRefRelation {
                        source: &source,
                        sink: &sink,
                        weights: &edge.weight,
                    };
                    if !relation_pred.match_ref(&rel) {
                        if let BTreeEntry::Vacant(e) = states.entry(sink) {
                            if let Some(state) = self.states().get(&sink) {
                                let mut sink_state = state.clone();
                                // We don't have the entirety of the node relation set, so insert
                                // the trace nodekind graph color
                                sink_state.kind.insert(BeliefKind::Trace);
                                e.insert(sink_state.clone());
                            }
                        }
                        if let BTreeEntry::Vacant(e) = states.entry(source) {
                            if let Some(state) = self.states().get(&source) {
                                let mut source_state = state.clone();
                                // We don't have the entirety of the node relation set, so insert
                                // the trace nodekind graph color
                                source_state.kind.insert(BeliefKind::Trace);
                                e.insert(source_state.clone());
                            }
                        }
                        edges.push(BeliefRelation::from(&rel));
                    }
                }
                BeliefGraph {
                    states,
                    relations: BidGraph::from_edges(edges),
                }
            }
            Expression::Dyad(lhs_p, op, rhs_p) => {
                let mut lhs = self.evaluate_expression(lhs_p);
                let rhs = self.evaluate_expression(rhs_p);
                match op {
                    SetOp::Union => lhs.union_mut(&rhs),
                    SetOp::Intersection => lhs.intersection_mut(&rhs),
                    SetOp::Difference => lhs.difference_mut(&rhs),
                    SetOp::SymmetricDifference => lhs.symmetric_difference_mut(&rhs),
                }
                lhs
            }
        }
    }
}

impl BeliefSource for BeliefBase {
    async fn eval_unbalanced(&self, expr: &Expression) -> Result<BeliefGraph, BuildonomyError> {
        Ok(self.evaluate_expression(expr))
    }

    /// Get all paths for a network as (path, target_bid) pairs.
    /// Useful for querying asset manifests or all documents in a network.
    /// Default implementation returns empty (in-memory BeliefBase doesn't cache paths).
    async fn get_network_paths(
        &self,
        network_bid: Bid,
    ) -> Result<Vec<(String, Bid)>, BuildonomyError> {
        Ok(self
            .paths()
            .get_map(&network_bid)
            .map(|pm| pm.all_net_paths(&self.paths(), &mut BTreeSet::default()))
            .unwrap_or_default())
    }

    async fn eval_trace(
        &self,
        expr: &Expression,
        weight_filter: WeightSet,
    ) -> Result<BeliefGraph, BuildonomyError> {
        Ok(self.evaluate_expression_as_trace(expr, weight_filter))
    }

    async fn export_beliefgraph(&self) -> Result<BeliefGraph, BuildonomyError> {
        // Clone and consume the entire BeliefBase to get complete BeliefGraph
        Ok(self.clone().consume())
    }
}

impl BeliefSource for &BeliefBase {
    async fn eval_unbalanced(&self, expr: &Expression) -> Result<BeliefGraph, BuildonomyError> {
        Ok(self.evaluate_expression(expr))
    }

    async fn get_network_paths(
        &self,
        network_bid: Bid,
    ) -> Result<Vec<(String, Bid)>, BuildonomyError> {
        Ok(self
            .paths()
            .get_map(&network_bid)
            .map(|pm| pm.all_net_paths(&self.paths(), &mut BTreeSet::default()))
            .unwrap_or_default())
    }

    async fn eval_trace(
        &self,
        expr: &Expression,
        weight_filter: WeightSet,
    ) -> Result<BeliefGraph, BuildonomyError> {
        Ok(self.evaluate_expression_as_trace(expr, weight_filter))
    }

    async fn export_beliefgraph(&self) -> Result<BeliefGraph, BuildonomyError> {
        // Clone and consume the entire BeliefBase to get complete BeliefGraph
        Ok((*self).clone().consume())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nodekey::NodeKey;
    use crate::properties::{BeliefKind, BeliefKindSet, Weight, WeightKind, WeightSet};

    /// Test for Issue 34: Relations referencing nodes not in states
    ///
    /// This simulates what happens when DbConnection.eval_unbalanced returns
    /// a BeliefGraph with incomplete data - the relations reference BIDs that
    /// aren't included in the states map.
    #[test]
    fn test_beliefgraph_with_orphaned_edges() {
        // Create three nodes
        let net_bid = Bid::new(Bid::nil());
        let node_a_bid = Bid::new(net_bid);
        let node_b_bid = Bid::new(net_bid);

        let node_a = BeliefNode {
            bid: net_bid,
            title: "Network".to_string(),
            kind: BeliefKindSet(BeliefKind::Network.into()),
            ..Default::default()
        };

        let node_b = BeliefNode {
            bid: node_a_bid,
            title: "Doc A".to_string(),
            kind: BeliefKindSet(BeliefKind::Document.into()),
            id: Some("doc-a".to_string()),
            ..Default::default()
        };

        let _node_c = BeliefNode {
            bid: node_b_bid,
            title: "Doc B".to_string(),
            kind: BeliefKindSet(BeliefKind::Document.into()),
            id: Some("doc-b".to_string()),
            ..Default::default()
        };

        // Create a BeliefGraph with only node_a and node_b in states,
        // but with relations that reference node_c (which is missing)
        let mut states = BTreeMap::new();
        states.insert(net_bid, node_a.clone());
        states.insert(node_a_bid, node_b.clone());
        // node_c is NOT in states!

        // Create relations that include edges to the missing node_c
        let mut relations = petgraph::Graph::new();
        let net_idx = relations.add_node(net_bid);
        let a_idx = relations.add_node(node_a_bid);
        let b_idx = relations.add_node(node_b_bid); // References missing node!

        let mut weights = WeightSet::empty();
        weights.set(WeightKind::Section, Weight::default());

        relations.add_edge(a_idx, net_idx, weights.clone());
        relations.add_edge(b_idx, net_idx, weights.clone()); // Orphaned edge!

        let bg = BeliefGraph {
            states,
            relations: BidGraph(relations),
        };

        // Convert to BeliefBase - this should trigger PathMap reconstruction
        // with the incomplete data
        let bs = BeliefBase::from(bg);

        // The PathMapMap should warn about nodes in relations but not in states
        // This is the symptom we're detecting
        let _paths = bs.paths();

        // Check for orphaned edges - this is the Issue 34 symptom
        // After fix, DbConnection won't return orphaned edges, but BeliefBase should handle them gracefully
        let orphaned = {
            let graph = BeliefGraph::from(&bs);
            graph.find_orphaned_edges()
        };

        // Document that orphaned edges exist in this test case
        assert_eq!(
            orphaned.len(),
            1,
            "Test setup should have 1 orphaned edge to verify graceful handling"
        );

        // Verify BeliefBase still functions despite orphaned edges
        // BID lookups should still work
        assert!(
            bs.get(&NodeKey::Bid { bid: net_bid }).is_some(),
            "Should find network node by BID despite orphaned edges"
        );
        assert!(
            bs.get(&NodeKey::Bid { bid: node_a_bid }).is_some(),
            "Should find node A by BID despite orphaned edges"
        );

        // Path/Title/Id lookups may fail (this is the Issue 34 symptom)
        // but BeliefBase shouldn't panic
        let by_id = bs.get(&NodeKey::Id {
            net: net_bid,
            id: "doc-a".to_string(),
        });
        // This documents current behavior - may fail due to incomplete PathMap
        if by_id.is_none() {
            println!("WARNING: PathMap lookup by ID failed due to orphaned edges");
            println!("This is the Issue 34 symptom - cache_fetch will fail");
        }
    }

    /// Test for Issue 34: BeliefBase::get() failing when PathMap is incomplete
    ///
    /// When relations have dangling references, PathMap construction may fail
    /// or produce incomplete results, breaking Path/Title/Id lookups.
    #[test]
    fn test_pathmap_with_incomplete_relations() {
        // Create a minimal network with proper structure
        let net_bid = Bid::new(Bid::nil());
        let doc_bid = Bid::new(net_bid);
        let section_bid = Bid::new(doc_bid);
        let orphan_bid = Bid::new(net_bid); // This will be in relations but not states

        let network = BeliefNode {
            bid: net_bid,
            title: "Test Network".to_string(),
            kind: BeliefKindSet(BeliefKind::Network.into()),
            ..Default::default()
        };

        let doc = BeliefNode {
            bid: doc_bid,
            title: "Test Doc".to_string(),
            kind: BeliefKindSet(BeliefKind::Document.into()),
            id: Some("test-doc".to_string()),
            ..Default::default()
        };

        let section = BeliefNode {
            bid: section_bid,
            title: "Test Section".to_string(),
            kind: BeliefKindSet(BeliefKind::Symbol.into()),
            ..Default::default()
        };

        // States includes network, doc, and section but NOT orphan
        let mut states = BTreeMap::new();
        states.insert(net_bid, network);
        states.insert(doc_bid, doc);
        states.insert(section_bid, section);

        // Relations includes an edge to the orphan node
        let mut relations = petgraph::Graph::new();
        let net_idx = relations.add_node(net_bid);
        let doc_idx = relations.add_node(doc_bid);
        let section_idx = relations.add_node(section_bid);
        let orphan_idx = relations.add_node(orphan_bid); // Orphaned!

        let mut weights = WeightSet::empty();
        weights.set(WeightKind::Section, Weight::default());

        relations.add_edge(doc_idx, net_idx, weights.clone());
        relations.add_edge(section_idx, doc_idx, weights.clone());
        relations.add_edge(orphan_idx, net_idx, weights.clone()); // Dangling reference!

        let bg = BeliefGraph {
            states,
            relations: BidGraph(relations),
        };

        // This should not panic despite the incomplete relations
        let bs = BeliefBase::from(bg);

        // Check for orphaned edges - this is the Issue 34 symptom
        let orphaned = {
            let graph = BeliefGraph::from(&bs);
            graph.find_orphaned_edges()
        };

        // Document that orphaned edges exist in this test case
        assert_eq!(
            orphaned.len(),
            1,
            "Test setup should have 1 orphaned edge to verify graceful handling"
        );

        // Verify BeliefBase still functions despite orphaned edges
        // BID lookups should still work
        assert!(
            bs.get(&NodeKey::Bid { bid: doc_bid }).is_some(),
            "Should find doc by BID despite orphaned edges"
        );
        assert!(
            bs.get(&NodeKey::Bid { bid: section_bid }).is_some(),
            "Should find section by BID despite orphaned edges"
        );

        // Path/Title/Id lookups may fail (this is the Issue 34 symptom)
        // but BeliefBase shouldn't panic
        let by_id = bs.get(&NodeKey::Id {
            net: net_bid,
            id: "test-doc".to_string(),
        });
        // This documents current behavior - may fail due to incomplete PathMap
        if by_id.is_none() {
            println!("WARNING: PathMap lookup by ID failed due to orphaned edges");
            println!("This is the Issue 34 symptom - cache_fetch will fail");
        }
    }

    /// Test detecting orphaned edges in relations
    ///
    /// Helper to identify when a BeliefGraph has relations referencing
    /// nodes that don't exist in states.
    #[test]
    fn test_detect_orphaned_edges() {
        let net_bid = Bid::new(Bid::nil());
        let node_a = Bid::new(net_bid);
        let node_b = Bid::new(net_bid);
        let orphan = Bid::new(net_bid);

        // States only has net, node_a and node_b
        let mut states = BTreeMap::new();
        states.insert(
            net_bid,
            BeliefNode {
                bid: net_bid,
                title: "Net".to_string(),
                kind: BeliefKindSet(BeliefKind::Network.into()),
                ..Default::default()
            },
        );
        states.insert(
            node_a,
            BeliefNode {
                bid: node_a,
                title: "A".to_string(),
                kind: BeliefKindSet(BeliefKind::Document.into()),
                ..Default::default()
            },
        );
        states.insert(
            node_b,
            BeliefNode {
                bid: node_b,
                title: "B".to_string(),
                kind: BeliefKindSet(BeliefKind::Document.into()),
                ..Default::default()
            },
        );

        // Relations includes edge to orphan
        let mut relations = petgraph::Graph::new();
        let net_idx = relations.add_node(net_bid);
        let a_idx = relations.add_node(node_a);
        let b_idx = relations.add_node(node_b);
        let orphan_idx = relations.add_node(orphan); // Not in states!

        let mut weights = WeightSet::empty();
        weights.set(WeightKind::Section, Weight::default());

        relations.add_edge(a_idx, net_idx, weights.clone());
        relations.add_edge(b_idx, net_idx, weights.clone());
        relations.add_edge(orphan_idx, net_idx, weights.clone()); // Orphaned!

        // Use BeliefGraph method to detect orphaned edges
        let graph = BeliefGraph {
            states,
            relations: BidGraph(relations),
        };

        let orphaned = graph.find_orphaned_edges();

        assert_eq!(orphaned.len(), 1, "Should detect 1 orphaned edge");
        assert_eq!(orphaned[0], orphan, "Should identify the orphan BID");
    }

    /// Test documenting that orphaned edges may not be caught by is_balanced()
    ///
    /// This test documents current behavior - is_balanced() may or may not
    /// detect orphaned edges depending on implementation. The primary symptom
    /// detection should happen when PathMap is constructed, not in is_balanced().
    #[test]
    fn test_orphaned_edges_behavior() {
        let net_bid = Bid::new(Bid::nil());
        let doc_bid = Bid::new(net_bid);
        let orphan_bid = Bid::new(net_bid);

        let mut states = BTreeMap::new();
        states.insert(
            net_bid,
            BeliefNode {
                bid: net_bid,
                title: "Net".to_string(),
                kind: BeliefKindSet(BeliefKind::Network.into()),
                ..Default::default()
            },
        );
        states.insert(
            doc_bid,
            BeliefNode {
                bid: doc_bid,
                title: "Doc".to_string(),
                kind: BeliefKindSet(BeliefKind::Document.into()),
                ..Default::default()
            },
        );

        let mut relations = petgraph::Graph::new();
        let net_idx = relations.add_node(net_bid);
        let doc_idx = relations.add_node(doc_bid);
        let orphan_idx = relations.add_node(orphan_bid);

        let mut weights = WeightSet::empty();
        weights.set(WeightKind::Section, Weight::default());

        relations.add_edge(doc_idx, net_idx, weights.clone());
        relations.add_edge(orphan_idx, net_idx, weights.clone());

        // Create BeliefBase with orphaned edge
        let bs = BeliefBase::new_unbalanced(states, BidGraph(relations), true);

        // Check for orphaned edges - Issue 34 symptom
        let orphaned = {
            let graph = BeliefGraph::from(&bs);
            graph.find_orphaned_edges()
        };

        // Document that orphaned edges exist in this test case
        assert_eq!(
            orphaned.len(),
            1,
            "Test setup should have 1 orphaned edge to verify graceful handling"
        );

        // Verify BeliefBase still functions despite orphaned edges
        // is_balanced() does NOT currently detect orphaned edges
        // (it only checks for external sinks in Section relations)

        // BID lookups should still work
        assert!(
            bs.get(&NodeKey::Bid { bid: net_bid }).is_some(),
            "Should find network by BID despite orphaned edges"
        );
        assert!(
            bs.get(&NodeKey::Bid { bid: doc_bid }).is_some(),
            "Should find doc by BID despite orphaned edges"
        );

        // Orphan BID cannot be found (as expected - it's not in states)
        assert!(
            bs.get(&NodeKey::Bid { bid: orphan_bid }).is_none(),
            "Should NOT find orphan BID - it's not in states"
        );
    }

    /// Test for traversal with Trace nodes
    ///
    /// This verifies that union_mut_with_trace correctly accumulates Trace nodes
    /// during traversal operations, fixing the bug where eval_trace marked nodes
    /// as Trace and union_mut filtered them out, causing traversal to fail.
    #[test]
    fn test_union_with_trace_nodes() {
        let net_bid = Bid::new(Bid::nil());
        let doc_a_bid = Bid::new(net_bid);
        let doc_b_bid = Bid::new(net_bid);

        // Create nodes
        let net_node = BeliefNode {
            bid: net_bid,
            title: "Network".to_string(),
            kind: BeliefKindSet(BeliefKind::Network.into()),
            ..Default::default()
        };

        let mut doc_a = BeliefNode {
            bid: doc_a_bid,
            title: "Doc A".to_string(),
            kind: BeliefKindSet(BeliefKind::Document.into()),
            ..Default::default()
        };

        let mut doc_b = BeliefNode {
            bid: doc_b_bid,
            title: "Doc B".to_string(),
            kind: BeliefKindSet(BeliefKind::Document.into()),
            ..Default::default()
        };

        // Mark doc_b as Trace (simulating eval_trace result)
        doc_b.kind.insert(BeliefKind::Trace);

        // Create initial BeliefGraph with net and doc_a
        let mut states = BTreeMap::new();
        states.insert(net_bid, net_node.clone());
        states.insert(doc_a_bid, doc_a.clone());

        let initial_bg = BeliefGraph {
            states,
            relations: BidGraph::default(),
        };

        // Create a second BeliefGraph with trace node (simulating eval_trace result)
        let mut trace_states = BTreeMap::new();
        trace_states.insert(doc_b_bid, doc_b.clone());

        let trace_bg = BeliefGraph {
            states: trace_states,
            relations: BidGraph::default(),
        };

        // Test union_mut (should filter out Trace nodes)
        let mut test_union_mut = initial_bg.clone();
        test_union_mut.union_mut(&trace_bg);
        assert_eq!(
            test_union_mut.states.len(),
            2,
            "union_mut should NOT add Trace nodes"
        );
        assert!(
            !test_union_mut.states.contains_key(&doc_b_bid),
            "union_mut should filter out Trace node"
        );

        // Test union_mut_with_trace (should include Trace nodes)
        let mut test_union_with_trace = initial_bg.clone();
        test_union_with_trace.union_mut_with_trace(&trace_bg);
        assert_eq!(
            test_union_with_trace.states.len(),
            3,
            "union_mut_with_trace should add Trace nodes"
        );
        assert!(
            test_union_with_trace.states.contains_key(&doc_b_bid),
            "union_mut_with_trace should include Trace node"
        );

        // Verify the Trace node maintains its Trace flag
        let added_node = test_union_with_trace.states.get(&doc_b_bid).unwrap();
        assert!(
            added_node.kind.contains(BeliefKind::Trace),
            "Trace flag should be preserved"
        );

        // Test that complete nodes overwrite Trace nodes
        doc_a.kind.insert(BeliefKind::Trace);
        let mut trace_doc_a = BTreeMap::new();
        trace_doc_a.insert(doc_a_bid, doc_a.clone());
        let trace_bg_a = BeliefGraph {
            states: trace_doc_a,
            relations: BidGraph::default(),
        };

        // Initial has complete doc_a, trace_bg_a has Trace doc_a
        let mut test_overwrite = initial_bg.clone();
        test_overwrite.union_mut_with_trace(&trace_bg_a);

        // Complete node should remain complete (Trace not added)
        let result_node = test_overwrite.states.get(&doc_a_bid).unwrap();
        assert!(
            !result_node.kind.contains(BeliefKind::Trace),
            "Complete node should remain complete when merged with Trace version"
        );

        // Test reverse: Trace node upgraded to complete
        let mut trace_initial = initial_bg.clone();
        trace_initial
            .states
            .get_mut(&doc_a_bid)
            .unwrap()
            .kind
            .insert(BeliefKind::Trace);

        let mut complete_doc_a = BTreeMap::new();
        doc_a.kind.remove(BeliefKind::Trace);
        complete_doc_a.insert(doc_a_bid, doc_a.clone());
        let complete_bg = BeliefGraph {
            states: complete_doc_a,
            relations: BidGraph::default(),
        };

        trace_initial.union_mut_with_trace(&complete_bg);
        let upgraded_node = trace_initial.states.get(&doc_a_bid).unwrap();
        assert!(
            !upgraded_node.kind.contains(BeliefKind::Trace),
            "Trace node should be upgraded to complete when merged with complete version"
        );
    }
}
