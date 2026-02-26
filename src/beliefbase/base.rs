//! BeliefBase: The main belief management structure.
//!
//! This module contains the BeliefBase implementation which manages a structured
//! collection of belief states and their relations while preserving global graph
//! structure and maintaining indices for efficient queries.

use crate::{
    event::{BeliefEvent, EventOrigin},
    nodekey::NodeKey,
    paths::{pathmap::pathmap_order, PathMapMap},
    properties::{
        asset_namespace, BeliefKind, BeliefNode, BeliefRefRelation, BeliefRelation, Bid, Bref,
        WeightKind, WeightSet, WEIGHT_DOC_PATHS, WEIGHT_OWNED_BY, WEIGHT_SORT_KEY,
    },
    BuildonomyError,
};

#[cfg(not(target_arch = "wasm32"))]
use crate::query::BeliefSource;

use crate::query::{Expression, RelationPred, SetOp, StatePred};
#[cfg(not(target_arch = "wasm32"))]
use parking_lot::{ArcRwLockReadGuard, RawRwLock, RwLock};

#[cfg(target_arch = "wasm32")]
use parking_lot::RwLock;
use petgraph::{
    algo::kosaraju_scc,
    visit::{depth_first_search, Control, DfsEvent, EdgeRef},
    Direction,
};
use std::{
    collections::{btree_map::Entry as BTreeEntry, BTreeMap, BTreeSet},
    fmt,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use super::{context::BeliefContext, BeliefGraph, BidGraph};

// Conditional type alias for thread-safe shared locks
// WASM uses Rc<RefCell<T>> (single-threaded)
// Native uses Arc<RwLock<T>> (multi-threaded)
#[cfg(not(target_arch = "wasm32"))]
type SharedLock<T> = Arc<RwLock<T>>;

#[cfg(target_arch = "wasm32")]
use std::{cell::RefCell, rc::Rc};

#[cfg(target_arch = "wasm32")]
type SharedLock<T> = Rc<RefCell<T>>;

#[derive(Debug)]
pub struct BeliefBase {
    states: BTreeMap<Bid, BeliefNode>,
    relations: SharedLock<BidGraph>,
    #[cfg(not(target_arch = "wasm32"))]
    bid_to_index: RwLock<BTreeMap<Bid, petgraph::graph::NodeIndex>>,
    #[cfg(target_arch = "wasm32")]
    bid_to_index: RefCell<BTreeMap<Bid, petgraph::graph::NodeIndex>>,
    index_dirty: AtomicBool,
    brefs: BTreeMap<Bref, Bid>,
    paths: SharedLock<PathMapMap>,
    errors: SharedLock<Vec<String>>,
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
        self.index_sync(false);
        #[cfg(not(target_arch = "wasm32"))]
        {
            BeliefBase {
                states: self.states.clone(),
                relations: Arc::new(RwLock::new(self.read_relations().clone())),
                bid_to_index: RwLock::new(self.read_bid_index().clone()),
                index_dirty: AtomicBool::new(false),
                brefs: self.brefs.clone(),
                paths: Arc::new(RwLock::new(self.read_paths().clone())),
                errors: Arc::new(RwLock::new(self.read_errors().clone())),
                api: self.api.clone(),
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            BeliefBase {
                states: self.states.clone(),
                relations: Rc::new(RefCell::new(self.read_relations().clone())),
                bid_to_index: RefCell::new(self.read_bid_index().clone()),
                index_dirty: AtomicBool::new(false),
                brefs: self.brefs.clone(),
                paths: Rc::new(RefCell::new(self.read_paths().clone())),
                errors: Rc::new(RefCell::new(self.read_errors().clone())),
                api: self.api.clone(),
            }
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
        #[cfg(not(target_arch = "wasm32"))]
        {
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
        #[cfg(target_arch = "wasm32")]
        {
            BeliefBase {
                states: BTreeMap::default(),
                relations: Rc::new(RefCell::new(BidGraph(petgraph::Graph::new()))),
                bid_to_index: RefCell::new(BTreeMap::default()),
                index_dirty: AtomicBool::new(false),
                brefs: BTreeMap::default(),
                paths: Rc::new(RefCell::new(PathMapMap::default())),
                errors: Rc::new(RefCell::new(Vec::new())),
                api: BeliefNode::api_state(),
            }
        }
    }

    // Helper methods for conditional lock access
    #[cfg(not(target_arch = "wasm32"))]
    fn read_relations(&self) -> ArcRwLockReadGuard<RawRwLock, BidGraph> {
        self.relations.read_arc()
    }

    #[cfg(target_arch = "wasm32")]
    fn read_relations(&self) -> std::cell::Ref<'_, BidGraph> {
        self.relations.borrow()
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn write_relations(&self) -> parking_lot::ArcRwLockWriteGuard<RawRwLock, BidGraph> {
        self.relations.write_arc()
    }

    #[cfg(target_arch = "wasm32")]
    fn write_relations(&self) -> std::cell::RefMut<'_, BidGraph> {
        self.relations.borrow_mut()
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn read_paths(&self) -> ArcRwLockReadGuard<RawRwLock, PathMapMap> {
        self.paths.read_arc()
    }

    #[cfg(target_arch = "wasm32")]
    fn read_paths(&self) -> std::cell::Ref<'_, PathMapMap> {
        self.paths.borrow()
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn write_paths(&self) -> parking_lot::ArcRwLockWriteGuard<RawRwLock, PathMapMap> {
        self.paths.write_arc()
    }

    #[cfg(target_arch = "wasm32")]
    fn write_paths(&self) -> std::cell::RefMut<'_, PathMapMap> {
        self.paths.borrow_mut()
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn read_errors(&self) -> parking_lot::RwLockReadGuard<'_, Vec<String>> {
        self.errors.read()
    }

    #[cfg(target_arch = "wasm32")]
    fn read_errors(&self) -> std::cell::Ref<'_, Vec<String>> {
        self.errors.borrow()
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn write_errors(&self) -> parking_lot::RwLockWriteGuard<'_, Vec<String>> {
        self.errors.write()
    }

    #[cfg(target_arch = "wasm32")]
    fn write_errors(&self) -> std::cell::RefMut<'_, Vec<String>> {
        self.errors.borrow_mut()
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn read_bid_index(
        &self,
    ) -> parking_lot::RwLockReadGuard<'_, BTreeMap<Bid, petgraph::graph::NodeIndex>> {
        self.bid_to_index.read()
    }

    #[cfg(target_arch = "wasm32")]
    fn read_bid_index(&self) -> std::cell::Ref<'_, BTreeMap<Bid, petgraph::graph::NodeIndex>> {
        self.bid_to_index.borrow()
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn write_bid_index(
        &self,
    ) -> parking_lot::RwLockWriteGuard<'_, BTreeMap<Bid, petgraph::graph::NodeIndex>> {
        self.bid_to_index.write()
    }

    #[cfg(target_arch = "wasm32")]
    fn write_bid_index(&self) -> std::cell::RefMut<'_, BTreeMap<Bid, petgraph::graph::NodeIndex>> {
        self.bid_to_index.borrow_mut()
    }

    pub fn new_unbalanced(
        states: BTreeMap<Bid, BeliefNode>,
        relations: BidGraph,
        inject_api: bool,
    ) -> BeliefBase {
        let mut bs = BeliefBase::empty();
        // Set relations
        {
            *bs.write_relations() = relations;
        }
        bs.states = states;
        bs.brefs = BTreeMap::from_iter(bs.states.keys().map(|bid| (bid.bref(), *bid)));
        if inject_api {
            bs.insert_state(bs.api.clone(), &[]);
        }
        bs.index_dirty.store(true, Ordering::SeqCst);
        bs.index_sync(false);

        // Build PathMapMap - for WASM, need to convert to Arc<RwLock<>> temporarily
        #[cfg(not(target_arch = "wasm32"))]
        {
            *bs.paths.write() = PathMapMap::new(bs.states(), bs.relations.clone());
        }
        #[cfg(target_arch = "wasm32")]
        {
            let relations_arc = Arc::new(RwLock::new(bs.read_relations().clone()));
            *bs.write_paths() = PathMapMap::new(bs.states(), relations_arc);
        }
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

    #[cfg(not(target_arch = "wasm32"))]
    pub fn paths(&self) -> ArcRwLockReadGuard<RawRwLock, PathMapMap> {
        self.index_sync(false);
        while self.paths.is_locked_exclusive() {
            tracing::info!("[BeliefBase] Waiting for read access to paths");
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        self.read_paths()
    }

    #[cfg(target_arch = "wasm32")]
    pub fn paths(&self) -> std::cell::Ref<'_, PathMapMap> {
        self.index_sync(false);
        self.read_paths()
    }

    pub fn brefs(&self) -> &BTreeMap<Bref, Bid> {
        &self.brefs
    }

    pub fn errors(&self) -> Vec<String> {
        self.read_errors().clone()
    }

    /// Synchronize our indices (namely the self.paths object and our bid_to_index object), if the
    /// index_dirty flag is set. If bit is true, then run built in test as well.
    fn index_sync(&self, bit: bool) {
        if !self.index_dirty.load(Ordering::SeqCst) {
            return;
        }
        // This block ensures we drop relations and index
        {
            let mut relations = self.write_relations();
            let mut index = self.write_bid_index();
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
            // Rebuild paths
            #[cfg(not(target_arch = "wasm32"))]
            let constructor_paths_map = PathMapMap::new(self.states(), self.relations.clone());
            #[cfg(target_arch = "wasm32")]
            let constructor_paths_map = {
                let relations_arc = Arc::new(RwLock::new(self.read_relations().clone()));
                PathMapMap::new(self.states(), relations_arc)
            };
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
            let mut errors = self.write_errors();
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
            let errors = self.read_errors();
            if !errors.is_empty() {
                tracing::debug!("Set isn't balanced. Errors:\n{}", errors.join("\n- "));
            }
        }
    }

    pub fn bid_to_index(&self, bid: &Bid) -> Option<petgraph::graph::NodeIndex> {
        self.index_sync(false);
        self.read_bid_index().get(bid).copied()
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn relations(&self) -> ArcRwLockReadGuard<RawRwLock, BidGraph> {
        self.index_sync(false);
        while self.relations.is_locked_exclusive() {
            tracing::info!("[BeliefBase] Waiting for read access to relations");
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        self.read_relations()
    }

    #[cfg(target_arch = "wasm32")]
    pub fn relations(&self) -> std::cell::Ref<'_, BidGraph> {
        self.index_sync(false);
        self.read_relations()
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
        }
    }

    // FIXME: This could introduce index issues, as BeliefContext has mutable access to self.
    pub fn get_context(&mut self, root_net: &Bid, bid: &Bid) -> Option<BeliefContext<'_>> {
        self.index_sync(false);
        assert!(
            self.is_balanced().is_ok(),
            "get_context called on an unbalanced BeliefBase. errors: {:?}",
            self.read_errors().clone()
        );
        let Some(node) = self.states.get(bid) else {
            tracing::debug!("[get_context] node {bid} is not loaded");
            return None;
        };
        let Some(root_pm) = self.paths().get_map(&root_net.bref()) else {
            tracing::debug!("[get_context] network {root_net} is not loaded");
            return None;
        };
        root_pm
            .path(bid, &self.paths())
            .map(|(home_net, root_path, _order)| {
                BeliefContext::new(node, root_path, *root_net, home_net, self, self.relations())
            })
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn consume(&mut self) -> BeliefGraph {
        let mut old_self = std::mem::take(self);
        let states = std::mem::take(&mut old_self.states);
        while self.relations.is_locked() {
            tracing::info!("[BeliefBase::consume] Waiting for write access to relations");
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let relations = std::mem::replace(
            old_self.write_relations().as_graph_mut(),
            petgraph::Graph::new(),
        );
        BeliefGraph {
            states,
            relations: BidGraph(relations),
        }
    }

    #[cfg(target_arch = "wasm32")]
    pub fn consume(&mut self) -> BeliefGraph {
        let mut old_self = std::mem::take(self);
        let states = std::mem::take(&mut old_self.states);
        // No lock checking needed in WASM (single-threaded)
        let relations = std::mem::replace(
            old_self.write_relations().as_graph_mut(),
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
            let sink_is_complete = old_set
                .states()
                .get(sink)
                .filter(|n| n.kind.is_complete())
                .is_some();
            if sink_is_complete {
                events.push(BeliefEvent::RelationRemoved(
                    *source,
                    *sink,
                    EventOrigin::Remote,
                ));
            }
        }

        // Phase 4: New edges
        let mut new_edges = Vec::new();
        for ((source, sink), weight) in parsed_edges
            .iter()
            .filter(|(k, _v)| !old_parsed_edges.contains_key(k))
        {
            let sink_order = new_set
                .paths()
                .indexed_path(sink)
                .map(|(_a, _b, order)| order)
                .unwrap_or_else(|| {
                    tracing::warn!("No entry in pathmap for sink {sink}");
                    Vec::default()
                });
            new_edges.push((
                BeliefEvent::RelationUpdate(*source, *sink, weight.clone(), EventOrigin::Remote),
                sink_order,
            ));
        }
        new_edges.sort_by(|a, b| pathmap_order(&a.1, &b.1));
        for (event, _order) in new_edges.into_iter() {
            events.push(event);
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
        let errors = self.read_errors();
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
        if self
            .states
            .contains_key(&crate::properties::href_namespace())
        {
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
            .filter_map(|b| self.paths().get_map(&b.bref()))
            .collect::<Vec<_>>();

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
        //
        // Network nodes are a special case: their incoming Section edges come from two
        // independent sort spaces — document children at [0..NETWORK_SECTION_SORT_KEY-1]
        // and anchor/heading children at [NETWORK_SECTION_SORT_KEY, *] — so the global
        // incoming key set is not a single contiguous [0..N). Instead we verify each
        // group independently.
        let paths_guard = self.paths();
        let net_bids = paths_guard.nets();
        let doc_bids = paths_guard.docs();

        for node in self.states().values() {
            let bid = &node.bid;
            // Collect incoming sort keys per WeightKind, keyed by whether the source is a
            // document (in doc_bids) or an anchor (not in doc_bids). Only matters for nets.
            let mut kind_map: BTreeMap<WeightKind, Vec<u16>> = BTreeMap::new();
            // For network sinks: separate doc-sourced and anchor-sourced keys.
            let mut kind_map_docs: BTreeMap<WeightKind, Vec<u16>> = BTreeMap::new();
            let mut kind_map_anchors: BTreeMap<WeightKind, Vec<u16>> = BTreeMap::new();
            let is_net = net_bids.contains(bid);

            if let Some(node_idx) = self.bid_to_index(bid) {
                for edge in relations
                    .as_graph()
                    .edges_directed(node_idx, Direction::Incoming)
                {
                    let source_bid = relations.as_graph()[edge.source()];
                    for (kind, weight_data) in edge.weight().weights.iter() {
                        let sort_key: u16 = weight_data
                            .get(crate::properties::WEIGHT_SORT_KEY)
                            .unwrap_or(0);
                        if is_net {
                            if doc_bids.contains(&source_bid) {
                                kind_map_docs.entry(*kind).or_default().push(sort_key);
                            } else {
                                kind_map_anchors.entry(*kind).or_default().push(sort_key);
                            }
                        } else {
                            kind_map.entry(*kind).or_default().push(sort_key);
                        }
                    }
                }
            }

            if is_net {
                // For network nodes, verify docs and anchors are each independently contiguous.
                for (label, map) in [("doc", &kind_map_docs), ("anchor", &kind_map_anchors)] {
                    for (kind, mut indices) in map.clone() {
                        indices.sort();
                        let expected: Vec<u16> = (0..indices.len() as u16).collect();
                        if indices != expected {
                            errors.push(format!(
                                "[BeliefBase::built_in_test invariant 2] {bid} (network) \
                                {kind:?} {label} edges are not correctly sorted. \
                                Received {indices:?}, Expected: {expected:?}"
                            ));
                        }
                    }
                }
            } else {
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
            let mut pmm = self.write_paths();
            #[cfg(not(target_arch = "wasm32"))]
            {
                pmm.process_event_queue(&event_queue, &self.relations)
            }
            #[cfg(target_arch = "wasm32")]
            {
                // For WASM, convert Rc to Arc temporarily for process_event_queue
                use parking_lot::RwLock;
                use std::sync::Arc;
                let relations_arc = Arc::new(RwLock::new(self.read_relations().clone()));
                pmm.process_event_queue(&event_queue, &relations_arc)
            }
        };

        // Append path events to derivatives for DbConnection and other subscribers
        derivative_events.append(&mut path_events);
        // tracing::debug!(
        //     "[process_event]: {event:?}\nderivatives:\n- {}",
        //     derivative_events
        //         .iter()
        //         .map(|e| format!("{e:?}"))
        //         .collect::<Vec<_>>()
        //         .join("\n- ")
        // );
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
            self.brefs.insert(bid.bref(), bid);
        }

        for replaced in to_replace.iter() {
            // Call replace_bid BEFORE removing from states, because replace_bid
            // needs to transfer edges from the replaced node to the new node
            events.push(BeliefEvent::NodeRenamed(*replaced, bid, EventOrigin::Local));
            events.append(&mut self.replace_bid(*replaced, bid));

            // Now remove from states (replace_bid already removed from graph)
            self.states.remove(replaced);
            self.brefs.remove(&replaced.bref());
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
            let relations = self.read_relations();
            let bid_to_index = self.read_bid_index();
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
                self.brefs.remove(&bid.bref());
            }
        }

        // Remove nodes from graph
        let mut relations = self.write_relations();
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
        #[cfg(not(target_arch = "wasm32"))]
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
        let mut relations = self.write_relations();
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
        let mut relations = self.write_relations();
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
                .filter_map(
                    |(source_idx, sink_idx, ks): &(
                        petgraph::graph::NodeIndex,
                        petgraph::graph::NodeIndex,
                        BTreeMap<WeightKind, u16>,
                    )| {
                        ks.get(kind)
                            .map(|weight_idx| (*source_idx, *sink_idx, *weight_idx))
                    },
                )
                .collect::<Vec<(petgraph::graph::NodeIndex, petgraph::graph::NodeIndex, u16)>>();
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

            let mut relations = self.write_relations();
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
        #[cfg(not(target_arch = "wasm32"))]
        while self.relations.is_locked() {
            tracing::info!("[BeliefBase::trim] Waiting for write access to relations");
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let mut write_relations = self.write_relations();
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
                let path_bid_tuples = paths_guard
                    .get_map(&net.bref())
                    .map(|pm| {
                        pm.recursive_map(&paths_guard, &mut std::collections::BTreeSet::new())
                    })
                    .unwrap_or_default();
                // Extract just the bids from (path, bid) tuples
                let bids: Vec<Bid> = path_bid_tuples
                    .iter()
                    .map(|(_path, bid, _order)| *bid)
                    .collect();
                BTreeMap::from_iter(
                    bids.iter()
                        .filter_map(|bid| self.states().get(bid).map(|node| (*bid, node.clone()))),
                )
            }
            StatePred::NetId(net, id) => {
                let paths_guard = self.paths();
                let maybe_match = paths_guard.net_get_from_id(net, id).and_then(|(_, bid)| {
                    self.states.get(&bid).map(|node| (node.bid, node.clone()))
                });
                BTreeMap::from_iter(maybe_match)
            }
            StatePred::Bid(bid_vec) => BTreeMap::from_iter(
                bid_vec
                    .iter()
                    .filter_map(|bid| self.states.get(bid).map(|node| (node.bid, node.clone()))),
            ),
            _ => {
                let res = BTreeMap::from_iter(
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
                );
                tracing::debug!("Found {res:?} matches");
                res
            }
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

#[cfg(not(target_arch = "wasm32"))]
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
            .get_map(&network_bid.bref())
            .map(|pm| {
                pm.recursive_map(&self.paths(), &mut BTreeSet::default())
                    .into_iter()
                    .map(|(path, bid, _order)| (path, bid))
                    .collect()
            })
            .unwrap_or_default())
    }

    async fn get_all_document_paths(
        &self,
        network_bid: Bid,
    ) -> Result<Vec<(String, Bid)>, BuildonomyError> {
        Ok(self
            .paths()
            .get_map(&network_bid.bref())
            .map(|pm| {
                pm.all_paths_with_bids(&self.paths(), &mut BTreeSet::default())
                    .into_iter()
                    .filter(|(path, _bid)| !path.is_empty())
                    .collect()
            })
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

#[cfg(not(target_arch = "wasm32"))]
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
            .get_map(&network_bid.bref())
            .map(|pm| {
                pm.recursive_map(&self.paths(), &mut BTreeSet::default())
                    .into_iter()
                    .map(|(path, bid, _order)| (path, bid))
                    .collect()
            })
            .unwrap_or_default())
    }

    async fn get_all_document_paths(
        &self,
        network_bid: Bid,
    ) -> Result<Vec<(String, Bid)>, BuildonomyError> {
        Ok(self
            .paths()
            .get_map(&network_bid.bref())
            .map(|pm| {
                pm.all_paths_with_bids(&self.paths(), &mut BTreeSet::default())
                    .into_iter()
                    .filter(|(path, _bid)| !path.is_empty())
                    .collect()
            })
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
