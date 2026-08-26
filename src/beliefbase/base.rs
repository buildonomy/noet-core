//! BeliefBase: The main belief management structure.
//!
//! This module contains the BeliefBase implementation which manages a structured
//! collection of belief states and their relations while preserving global graph
//! structure and maintaining indices for efficient queries.

#[cfg(not(target_arch = "wasm32"))]
use crate::beliefbase::EpochDrain;
use crate::codec::ParseDiagnostic;
use crate::{
    event::{BeliefEvent, EventOrigin},
    nodekey::NodeKey,
    paths::{pathmap::pathmap_order, PathMapMap},
    properties::{
        asset_namespace, const_namespaces, BeliefKind, BeliefNode, BeliefRelation, Bid, Bref,
        NodeId, WeightKind, WeightSet, WEIGHT_DOC_PATHS, WEIGHT_OWNED_BY, WEIGHT_SORT_KEY,
    },
    BuildonomyError,
};

#[cfg(not(target_arch = "wasm32"))]
use crate::query::spec::QuerySpec;
#[cfg(not(target_arch = "wasm32"))]
use crate::query::{BeliefSource, BoxFuture, SubmapResult};

use crate::query::spec::{
    Composition, CompositionOp, EdgePredicate, NodeFilter, PackageStage, ProjectionStep,
    QueryPackage, Role, SortPayload, StepOperation, TapeContent, TapeEntry, TapeFn, TapePayload,
    TextSearchProvider, TraversalSpec,
};

use enumset::EnumSet;
#[cfg(not(target_arch = "wasm32"))]
use parking_lot::{ArcRwLockReadGuard, RawRwLock, RwLock};

#[cfg(target_arch = "wasm32")]
use parking_lot::RwLock;
use petgraph::{
    algo::kosaraju_scc,
    visit::{depth_first_search, Control, DfsEvent, EdgeRef, IntoEdgeReferences},
    Direction,
};

/// Local alias so all `bid_to_index` maps use a consistent type that matches
/// `StableGraph`'s node-index type.  Changing `BidGraph` from `Graph` to
/// `StableGraph` requires this to be `stable_graph::NodeIndex` everywhere.
type NodeIndex = petgraph::stable_graph::NodeIndex;
type EdgeIndex = petgraph::stable_graph::EdgeIndex;

use rustc_hash::FxHashMap;
use std::{
    collections::{
        btree_map::Entry as BTreeEntry, hash_map::Entry as HashEntry, BTreeMap, BTreeSet,
    },
    fmt,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use super::{
    context::BeliefContext,
    graph::{MergeOp, MergePrecedence},
    BeliefGraph, BidGraph,
};

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
    /// Short diagnostic label identifying which role this instance plays
    /// (e.g. `"doc_bb"`, `"session_bb"`, `"global_bb"`).  Printed in every
    /// tracing macro so log lines can be attributed without ambiguity.
    pub label: &'static str,
    // FxHashMap (not BTreeMap): pure keyed lookup with no order dependency — see
    // Issue 101 (BTree→HashMap investigation). Bid is already high-entropy (UUID-backed),
    // so a non-cryptographic hasher is a safe, intentional choice for this embedded-library
    // hot path (no untrusted-input DoS surface). Deterministic *output* order, when needed
    // (e.g. diagnostic messages, JSON export), is produced by sorting a `Vec` collected from
    // this map at the point of use — never by relying on the map's own iteration order.
    states: FxHashMap<Bid, BeliefNode>,
    relations: SharedLock<BidGraph>,
    #[cfg(not(target_arch = "wasm32"))]
    bid_to_index: RwLock<FxHashMap<Bid, NodeIndex>>,
    #[cfg(target_arch = "wasm32")]
    bid_to_index: RefCell<FxHashMap<Bid, NodeIndex>>,

    /// True when the last `built_in_test` run found no invariant violations.
    balanced: AtomicBool,
    brefs: BTreeMap<Bref, Bid>,
    paths: SharedLock<PathMapMap>,
    diagnostics: SharedLock<Vec<ParseDiagnostic>>,
    api: BeliefNode,

    /// Owner-edge memo: indexes edges by their `WEIGHT_OWNED_BY` third-party bref.
    ///
    /// Only third-party owners are indexed (not `"source"`, `"sink"`, or absent).
    /// Each entry maps an owner bref to the set of `EdgeIndex` values in `self.relations`
    /// whose `WeightSet` contains at least one weight with that bref as `WEIGHT_OWNED_BY`.
    ///
    /// Maintained incrementally by `update_relation`, `replace_bid`, `remove_nodes`,
    /// and `trim`. Built from scratch in `new_unbalanced`.
    /// Cloned on `Clone` (mirrors `brefs` — correctness index, not performance counter).
    ///
    /// Enables O(1) owned-edge lookup for the Owner input role in `apply_traversal`,
    /// replacing the previous O(E) full-graph scan.
    owner_edges: BTreeMap<Bref, BTreeSet<EdgeIndex>>,

    /// Memoized "next sort key" per `(sink, WeightKind)` pair.
    ///
    /// Seeded lazily from the current max incoming sort index on a sink the first time a
    /// new edge is added to it.  Avoids the O(K) `max()` scan inside
    /// `generate_edge_update` for every new edge, turning an O(K²) batch into O(K).
    ///
    /// **Invariant**: must be invalidated (entry removed) whenever an edge is removed from
    /// a sink so the counter is re-seeded from the compacted post-removal state.
    /// Cleared entirely on `Clone` — clones are short-lived and don't inherit the counter.
    next_sort_key: BTreeMap<(Bid, WeightKind), u16>,
}

// ---------------------------------------------------------------------------
// EpochDrain — no-op impl for BeliefBase (sequential / test path)
// ---------------------------------------------------------------------------

/// `BeliefBase` is used directly in the sequential parse path and in tests.
/// It has no event channel to drain, so `drain_epoch` is a no-op.
#[cfg(not(target_arch = "wasm32"))]
impl EpochDrain for BeliefBase {
    fn drain_epoch(
        &self,
    ) -> impl std::future::Future<Output = Result<(), crate::BuildonomyError>> + Send {
        std::future::ready(Ok(()))
    }
}

impl From<BeliefGraph> for BeliefBase {
    fn from(beliefs: BeliefGraph) -> Self {
        BeliefBase::new_unbalanced(beliefs.states, beliefs.relations, false).with_label("bg_bb")
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
        BeliefBase::new(FxHashMap::default(), BidGraph::default())
            .expect("Single state set with no relations to pass the BeliefBase built in test")
    }
}

impl Clone for BeliefBase {
    fn clone(&self) -> BeliefBase {
        #[cfg(not(target_arch = "wasm32"))]
        {
            BeliefBase {
                label: self.label,
                states: self.states.clone(),
                relations: Arc::new(RwLock::new(self.read_relations().clone())),
                bid_to_index: RwLock::new(self.read_bid_index().clone()),

                balanced: AtomicBool::new(self.balanced.load(Ordering::SeqCst)),
                brefs: self.brefs.clone(),
                paths: Arc::new(RwLock::new(self.read_paths().clone())),
                diagnostics: Arc::new(RwLock::new(self.read_diagnostics().clone())),
                api: self.api.clone(),

                owner_edges: self.owner_edges.clone(),
                // Clones start with an empty sort-key memo — the counter must not be
                // inherited across clone boundaries (stale edge counts on the clone's
                // independent graph copy would produce duplicate sort indices).
                next_sort_key: BTreeMap::new(),
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            BeliefBase {
                label: self.label,
                states: self.states.clone(),
                relations: Rc::new(RefCell::new(self.read_relations().clone())),
                bid_to_index: RefCell::new(self.read_bid_index().clone()),

                balanced: AtomicBool::new(self.balanced.load(Ordering::SeqCst)),
                brefs: self.brefs.clone(),
                paths: Rc::new(RefCell::new(self.read_paths().clone())),
                diagnostics: Rc::new(RefCell::new(self.read_diagnostics().clone())),
                api: self.api.clone(),
                owner_edges: self.owner_edges.clone(),
                // Clones start with an empty sort-key memo (see native comment above).
                next_sort_key: BTreeMap::new(),
            }
        }
    }
}

// BeliefBase: A structured collection of `BeliefState`s and their relations that can be queried and
// manipulated while preserving a global graph structure.
//
// - Creates a cache that maps belief IDs and belief paths to quick lookup information such as:
//   local path, title, bid, content summary, version control state, belief type
// - Creates typed belief-to-belief directional relationships between belief objects
//
// Static Invariants for a balanced BeliefBase (checked by [BeliefBase::built_in_test] and
// BeliefBase::check_path_invariants):
//
// 0. Each BeliefRelationKind sub-graph forms a directed acyclic graph. sub-graph cycles are not
//    supported.
//
// 1. All nodes within the relation hyper-graph have:
//
//    0. A corresponding state ([crate::properties::BeliefNode]) and,
//
//    1. A corresponding API path.
//
// 2. Each Belief relation is ordered by BeliefRelationKind weights. Each weight specifies a
//    different graph type. The relation graph is therefore something like a multigraph. Because of
//    the weights, each sub-graph has a deterministic ordering. In this manner, the relation graph
//    can produce deterministically serialized results, necessary for things like creating table of
//    contents, or serialized procedural outcomes.
//
// Operational rules:
//
// 1. The holder of a link is a 'sink' whereas the resource its accessing is the source. Parent ==
//    sink, child == source. In non-parent-child relationships this is intuitive, but it also makes
//    sense for subsections. In as the child contains its self state (source), and the parent is
//    indexing its child relationships, so 'sinking'/consuming data from the child nodes. Think
//    about the direction the information is flowing.
//
// 2. PathMaps identify how to acquire the source starting from known network locations.

impl BeliefBase {
    pub fn empty() -> BeliefBase {
        #[cfg(not(target_arch = "wasm32"))]
        {
            BeliefBase {
                label: "bb",
                states: FxHashMap::default(),
                relations: Arc::new(RwLock::new(BidGraph(
                    petgraph::stable_graph::StableGraph::new(),
                ))),
                bid_to_index: RwLock::new(FxHashMap::default()),

                balanced: AtomicBool::new(true),
                brefs: BTreeMap::default(),
                paths: Arc::new(RwLock::new(PathMapMap::default())),
                diagnostics: Arc::new(RwLock::new(Vec::new())),
                api: BeliefNode::api_state(),

                owner_edges: BTreeMap::new(),
                next_sort_key: BTreeMap::new(),
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            BeliefBase {
                label: "bb",
                states: FxHashMap::default(),
                relations: Rc::new(RefCell::new(BidGraph(
                    petgraph::stable_graph::StableGraph::new(),
                ))),
                bid_to_index: RefCell::new(FxHashMap::default()),

                balanced: AtomicBool::new(true),
                brefs: BTreeMap::default(),
                paths: Rc::new(RefCell::new(PathMapMap::default())),
                diagnostics: Rc::new(RefCell::new(Vec::new())),
                api: BeliefNode::api_state(),
                owner_edges: BTreeMap::new(),
                next_sort_key: BTreeMap::new(),
            }
        }
    }

    /// Set the diagnostic label on this instance.  Returns `self` so it can be
    /// chained directly after construction:
    ///
    /// ```ignore
    /// let bb = BeliefBase::empty().with_label("session_bb");
    /// ```
    pub fn with_label(mut self, label: &'static str) -> Self {
        self.label = label;
        self
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
    fn read_diagnostics(&self) -> parking_lot::RwLockReadGuard<'_, Vec<ParseDiagnostic>> {
        self.diagnostics.read()
    }

    #[cfg(target_arch = "wasm32")]
    fn read_diagnostics(&self) -> std::cell::Ref<'_, Vec<ParseDiagnostic>> {
        self.diagnostics.borrow()
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn write_diagnostics(&self) -> parking_lot::RwLockWriteGuard<'_, Vec<ParseDiagnostic>> {
        self.diagnostics.write()
    }

    #[cfg(target_arch = "wasm32")]
    fn write_diagnostics(&self) -> std::cell::RefMut<'_, Vec<ParseDiagnostic>> {
        self.diagnostics.borrow_mut()
    }

    /// Drain all accumulated [`ParseDiagnostic`] values and clear the internal buffer.
    /// Invariant-violation messages (written by `built_in_test`) are surfaced as
    /// `ParseDiagnostic::ParseError`; all other messages (e.g. ID-collision warnings written by
    /// `insert_state`) are surfaced as `ParseDiagnostic::Warning`.
    ///
    /// Use [`Self::is_balanced`] to check whether any error-level diagnostics were recorded.
    pub fn drain_diagnostics(&self) -> Vec<ParseDiagnostic> {
        std::mem::take(&mut *self.write_diagnostics())
    }

    /// Return a snapshot of all accumulated [`ParseDiagnostic`] values without consuming them.
    pub fn diagnostics(&self) -> Vec<ParseDiagnostic> {
        self.read_diagnostics().clone()
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn read_bid_index(&self) -> parking_lot::RwLockReadGuard<'_, FxHashMap<Bid, NodeIndex>> {
        self.bid_to_index.read()
    }

    #[cfg(target_arch = "wasm32")]
    fn read_bid_index(&self) -> std::cell::Ref<'_, FxHashMap<Bid, NodeIndex>> {
        self.bid_to_index.borrow()
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn write_bid_index(&self) -> parking_lot::RwLockWriteGuard<'_, FxHashMap<Bid, NodeIndex>> {
        self.bid_to_index.write()
    }

    #[cfg(target_arch = "wasm32")]
    fn write_bid_index(&self) -> std::cell::RefMut<'_, FxHashMap<Bid, NodeIndex>> {
        self.bid_to_index.borrow_mut()
    }

    pub fn new_unbalanced(
        states: FxHashMap<Bid, BeliefNode>,
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
        // Build bid_to_index once inline from the loaded graph — O(N), no dirty flag needed.
        {
            let relations = bs.write_relations();
            let mut index = bs.write_bid_index();
            *index = FxHashMap::from_iter(
                relations
                    .as_graph()
                    .node_indices()
                    .map(|idx| (relations.as_graph()[idx], idx)),
            );
            // Ensure all states nodes are in the relations graph.
            drop(index);
            drop(relations);
        }
        // Any states node not yet in the graph must be registered now.
        let missing_bids: Vec<Bid> = bs
            .states
            .keys()
            .filter(|bid| !bs.read_bid_index().contains_key(bid))
            .copied()
            .collect();
        for bid in missing_bids {
            bs.graph_insert_node(bid);
        }

        // Build owner_edges memo from the loaded graph — O(E), single pass.
        bs.build_owner_edges();

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
        states: FxHashMap<Bid, BeliefNode>,
        relations: BidGraph,
    ) -> Result<BeliefBase, BuildonomyError> {
        let set = BeliefBase::new_unbalanced(states, relations, true);
        Ok(set)
    }

    pub fn api(&self) -> &BeliefNode {
        &self.api
    }

    pub fn states(&self) -> &FxHashMap<Bid, BeliefNode> {
        &self.states
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn paths(&self) -> ArcRwLockReadGuard<RawRwLock, PathMapMap> {
        while self.paths.is_locked_exclusive() {
            tracing::debug!(
                label = self.label,
                "[BeliefBase] Waiting for read access to paths"
            );
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        self.read_paths()
    }

    #[cfg(target_arch = "wasm32")]
    pub fn paths(&self) -> std::cell::Ref<'_, PathMapMap> {
        self.read_paths()
    }

    pub fn brefs(&self) -> &BTreeMap<Bref, Bid> {
        &self.brefs
    }

    pub fn owner_edges(&self) -> &BTreeMap<Bref, BTreeSet<EdgeIndex>> {
        &self.owner_edges
    }

    /// Build a [`BeliefGraph`] containing all edges owned by `owner_bref`
    /// (where `WEIGHT_OWNED_BY` matches the bref) and their endpoint nodes.
    ///
    /// Endpoint nodes are marked with [`BeliefKind::Trace`] because the
    /// returned graph is a partial view — it does not contain the full
    /// relation set for each node.
    ///
    /// Uses the `owner_edges` memo for O(K) lookup where K is the number
    /// of edges owned by this bref. Returns an empty graph if no edges
    /// are indexed under `owner_bref`.
    pub fn graph_for_owner(&self, owner_bref: &Bref) -> BeliefGraph {
        let Some(edge_indices) = self.owner_edges.get(owner_bref) else {
            return BeliefGraph::default();
        };

        let mut states = FxHashMap::default();
        let mut edges = Vec::new();
        let relations = self.read_relations();
        let graph = relations.as_graph();

        for &edge_idx in edge_indices {
            let Some((src_idx, snk_idx)) = graph.edge_endpoints(edge_idx) else {
                continue;
            };
            let Some(ws) = graph.edge_weight(edge_idx) else {
                continue;
            };
            let source = graph[src_idx];
            let sink = graph[snk_idx];

            if let HashEntry::Vacant(e) = states.entry(source) {
                if let Some(state) = self.states().get(&source) {
                    let mut source_state = state.clone();
                    source_state.kind.insert(BeliefKind::Trace);
                    e.insert(source_state);
                }
            }
            if let HashEntry::Vacant(e) = states.entry(sink) {
                if let Some(state) = self.states().get(&sink) {
                    let mut sink_state = state.clone();
                    sink_state.kind.insert(BeliefKind::Trace);
                    e.insert(sink_state);
                }
            }

            edges.push(BeliefRelation {
                source,
                sink,
                weights: ws.clone(),
            });
        }

        BeliefGraph {
            states,
            relations: BidGraph::from_edges(edges),
        }
    }

    /// Insert a node into the relations graph and update bid_to_index atomically.
    /// Must NOT be called while holding any lock on relations or bid_to_index.
    fn graph_insert_node(&self, bid: Bid) -> NodeIndex {
        let idx = self.write_relations().as_graph_mut().add_node(bid);
        self.write_bid_index().insert(bid, idx);
        idx
    }

    /// Remove a node from the relations graph and update bid_to_index atomically.
    /// Must NOT be called while holding any lock on relations or bid_to_index.
    fn graph_remove_node(&self, idx: NodeIndex, bid: &Bid) {
        self.write_relations().as_graph_mut().remove_node(idx);
        self.write_bid_index().remove(bid);
    }

    pub fn bid_to_index(&self, bid: &Bid) -> Option<NodeIndex> {
        self.read_bid_index().get(bid).copied()
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn relations(&self) -> ArcRwLockReadGuard<RawRwLock, BidGraph> {
        while self.relations.is_locked_exclusive() {
            tracing::debug!(
                label = self.label,
                "[BeliefBase] Waiting for read access to relations"
            );
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        self.read_relations()
    }

    #[cfg(target_arch = "wasm32")]
    pub fn relations(&self) -> std::cell::Ref<'_, BidGraph> {
        self.read_relations()
    }

    pub fn get(&self, key: &NodeKey) -> Option<BeliefNode> {
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

    pub fn get_context(&self, root_net: &Bid, bid: &Bid) -> Option<BeliefContext<'_>> {
        assert!(
            self.is_balanced().is_ok(),
            "get_context called on an unbalanced BeliefBase. diagnostics: {:?}",
            self.diagnostics()
        );
        let Some(node) = self.states.get(bid) else {
            tracing::debug!(label = self.label, "[get_context] node {bid} is not loaded");
            return None;
        };
        let Some(root_pm) = self.paths().get_map(&root_net.bref()) else {
            tracing::debug!(
                label = self.label,
                "[get_context] network {root_net} is not loaded"
            );
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
            tracing::debug!(
                label = self.label,
                "[BeliefBase::consume] Waiting for write access to relations"
            );
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let relations = std::mem::replace(
            old_self.write_relations().as_graph_mut(),
            petgraph::stable_graph::StableGraph::new(),
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
            petgraph::stable_graph::StableGraph::new(),
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
            use petgraph::visit::EdgeRef;
            BidGraph::from_edges(new_relations_graph.edge_references().filter_map(|edge| {
                let source = new_relations_graph[edge.source()];
                let sink = new_relations_graph[edge.target()];
                // Initial scope guard: include edges where source, sink, OR a bref-identified
                // third-party owner is in parsed_content.  Mapping edges (owned by a section
                // node via WEIGHT_OWNED_BY = bref_str) have source/sink from foreign documents
                // and would otherwise be filtered out even though the owning section IS parsed.
                let owner_in_parsed = || {
                    edge.weight().weights.values().any(|weight| {
                        matches!(
                            weight.get::<String>(WEIGHT_OWNED_BY).as_deref(),
                            Some(s) if s != "source" && s != "sink"
                        ) && {
                            let bref_str =
                                weight.get::<String>(WEIGHT_OWNED_BY).unwrap_or_default();
                            Bref::try_from(bref_str.as_str())
                                .ok()
                                .and_then(|bref| new_set.brefs().get(&bref).copied())
                                .map(|owner_bid| parsed_content.contains(&owner_bid))
                                .unwrap_or(false)
                        }
                    })
                };
                if !(parsed_content.contains(&source)
                    || parsed_content.contains(&sink)
                    || owner_in_parsed())
                {
                    return None;
                }

                let mut weightset = WeightSet::empty();
                // For Section weights, the source (child) must be in parsed_content — not
                // just the sink (parent). The outer guard admits edges where only the sink
                // is in parsed_content, which can allow Section edges from other documents'
                // section nodes that leaked into doc_bb via Phase 2 missing_structure merges.
                // For Epistemic/Pragmatic, parsed_content.contains(owner) is the correct
                // discriminator and is already sufficient (push_relation sets ownership
                // correctly for those kinds, so leaked edges from other docs have an owner
                // not in parsed_content).
                let source_is_parsed = parsed_content.contains(&source);
                for (kind, weight) in edge.weight().weights.iter() {
                    let owner_bid_buf: Option<Bid>;
                    let (owner, _sign): (&Bid, &str) =
                        match weight.get::<String>(WEIGHT_OWNED_BY).as_deref() {
                            Some("source") => (&source, "+"),
                            Some("sink") | None => (&sink, "-"),
                            Some(bref_str) => {
                                // Third-party bref owner: resolve via new_set's bref map.
                                // If unresolvable (owner deleted), fall back to sink-owned behavior.
                                // The edge will be GC'd by owner_index in terminate_stack anyway.
                                owner_bid_buf = Bref::try_from(bref_str)
                                    .ok()
                                    .and_then(|bref| new_set.brefs().get(&bref).copied());
                                owner_bid_buf
                                    .as_ref()
                                    .map(|b| (b, "o"))
                                    .unwrap_or((&sink, "-"))
                            }
                        };
                    // tracing::debug!("{}--[{}{}]-->{}", source, kind, _sign, sink);
                    if (*kind == WeightKind::Section && source_is_parsed)
                        || (*kind != WeightKind::Section && parsed_content.contains(owner))
                    {
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
                        // Node absent from new_set: it was removed (e.g. a node that was
                        // deleted or superseded). Record it and continue DFS to find any
                        // stale descendants.
                        old_content.insert(sink);
                        Control::<()>::Continue
                    } else {
                        // Node is present in new_set. If it was parsed in this pass it is
                        // a seed we started from — prune here (its own diff is handled by
                        // Phase 2). If it belongs to a different document, also prune so
                        // we don't walk into foreign subtrees.
                        //
                        // Note: collision BIDs (stale time-based section BIDs replaced by a
                        // fresh BID on reparse) are removed from session_bb in push() before
                        // terminate_stack runs, so they will not appear in new_set and will
                        // be caught by the first branch above. No DFS expansion is needed
                        // for those cases.
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
                    // Strip Trace from the cached (old) node before comparing: Trace is an
                    // ephemeral bookkeeping flag meaning "incomplete relation set loaded from
                    // cache". It is never written to source files and must not trigger a
                    // NodeUpdate when it is the sole difference between session_bb and doc_bb.
                    let mut old_node_normalized = old_node.clone();
                    old_node_normalized.kind.remove(BeliefKind::Trace);
                    new_node != old_node_normalized
                } else {
                    true
                };

                if should_update {
                    events.push(BeliefEvent::NodeUpdate(
                        vec![NodeKey::Bid { bid: *node_bid }],
                        new_node.clone(),
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
            use petgraph::visit::EdgeRef;
            BTreeMap::<(Bid, Bid), WeightSet>::from_iter(new_relations_graph.edge_references().map(
                |edge| {
                    let source = new_relations_graph[edge.source()];
                    let sink = new_relations_graph[edge.target()];
                    ((source, sink), edge.weight().clone())
                },
            ))
        };
        let old_relations = old_set.relations();
        let old_relations_graph = old_relations.as_graph();
        use petgraph::visit::EdgeRef;
        let old_parsed_edges = BTreeMap::<(Bid, Bid), WeightSet>::from_iter(
            old_relations_graph.edge_references().filter_map(|edge| {
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
                // Symmetric guard to the new_relations filter above: Section weights require
                // source∈parsed_content; Epistemic/Pragmatic use the ownership check.
                let source_is_parsed =
                    parsed_content.contains(&source) || removed_nodes.contains(&source);
                for (kind, weight) in edge.weight().weights.iter() {
                    let owner_bid_buf: Option<Bid>;
                    let (owner, _sign): (&Bid, &str) =
                        match weight.get::<String>(WEIGHT_OWNED_BY).as_deref() {
                            Some("source") => (&source, "+"),
                            Some("sink") | None => (&sink, "-"),
                            Some(bref_str) => {
                                // Third-party bref owner: resolve via old_set's bref map.
                                // If unresolvable (owner deleted), fall back to sink-owned behavior.
                                // The edge will be GC'd by owner_index in terminate_stack anyway.
                                owner_bid_buf = Bref::try_from(bref_str)
                                    .ok()
                                    .and_then(|bref| old_set.brefs().get(&bref).copied());
                                owner_bid_buf
                                    .as_ref()
                                    .map(|b| (b, "o"))
                                    .unwrap_or((&sink, "-"))
                            }
                        };
                    // tracing::debug!("{}--[{}{}]-->{}", source, kind, _sign, sink);
                    if (*kind == WeightKind::Section && source_is_parsed)
                        || (*kind != WeightKind::Section
                            && (parsed_content.contains(owner) || removed_nodes.contains(owner)))
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
        // Memo of sink BIDs that are absent from both new_set and old_set pathmaps, to
        // suppress duplicate warnings when the same unresolvable sink appears in many
        // mapping rows of the same document.
        let mut missing_sink_warned: std::collections::BTreeSet<Bid> = Default::default();
        for ((source, sink), weight) in parsed_edges
            .iter()
            .filter(|(k, _v)| !old_parsed_edges.contains_key(k))
        {
            // Primary lookup: new_set (doc_bb) pathmap.
            // Fallback: old_set (session_bb) pathmap — authoritative for external nodes
            // (e.g. mapping sinks in a different document) whose pathmap entries were
            // never merged into doc_bb because cache_fetch found them via StackCache
            // rather than GlobalCache and therefore did not populate missing_structure.
            let sink_order = new_set
                .paths()
                .indexed_path(sink)
                .or_else(|| old_set.paths().indexed_path(sink))
                .map(|(_a, _b, order)| order)
                .unwrap_or_else(|| {
                    if missing_sink_warned.insert(*sink) {
                        tracing::warn!(
                            label = new_set.label,
                            "No entry in pathmap for sink {sink} (checked doc_bb and session_bb)"
                        );
                    }
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
        if !self.balanced.load(Ordering::SeqCst) {
            Err(BuildonomyError::Custom(
                "BeliefBase is unbalanced; check diagnostics for details".into(),
            ))
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
            BeliefEvent::NodeUpdate(_keys, node, _) => {
                // Validate that the node exists with matching state
                if let Some(existing) = self.states().get(&node.bid) {
                    if existing != node {
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
            BeliefEvent::NodeUpsert(bid, node, _) => {
                // Validate that the node exists with matching state
                if let Some(existing) = self.states().get(bid) {
                    if existing != node {
                        return Err(format!(
                            "NodeUpsert mismatch for {}: expected {:?}, found {:?}",
                            bid, node, existing
                        ));
                    }
                } else {
                    return Err(format!(
                        "NodeUpsert claims {} exists but it's not in states",
                        bid
                    ));
                }
            }
            // For other event types, we could add validation but they're less critical
            _ => {}
        }
        Ok(())
    }

    pub fn check_path_invariants(&self) -> Vec<String> {
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
    /// Runs the full structural validation: graph invariants, edge sort-key checks,
    /// and a PathMapMap consistency diff (event-driven vs. freshly constructed).
    /// Updates `self.balanced` and appends to `self.diagnostics`. Also returns all
    /// errors found so callers (e.g. `GraphBuilder::built_in_test`) can aggregate them.
    ///
    /// Caution! This is not cheap in terms of computation or memory.
    pub fn built_in_test(&self) -> Vec<String> {
        // tracing::debug!(
        //     "Invariant #1 is checked in check_path_invariants"
        // );
        let mut errors = self.check_path_invariants();

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
        // Diff event-driven PathMapMap against a freshly constructed one.
        #[cfg(not(target_arch = "wasm32"))]
        let constructor_paths_map = PathMapMap::new(self.states(), self.relations.clone());
        #[cfg(target_arch = "wasm32")]
        let constructor_paths_map = {
            let relations_arc = Arc::new(RwLock::new(self.read_relations().clone()));
            PathMapMap::new(self.states(), relations_arc)
        };
        let constructor_paths: BTreeSet<String> = constructor_paths_map
            .all_paths()
            .values()
            .flatten()
            .map(|(path, _, _)| path.clone())
            .collect();
        let event_paths: BTreeSet<String> = self
            .paths()
            .all_paths()
            .values()
            .flatten()
            .map(|(path, _, _)| path.clone())
            .collect();
        if event_paths != constructor_paths {
            errors.push(format!(
                "[BeliefBase::built_in_test] Event-driven and constructor PathMapMaps should \
                    have identical paths.\n \
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

        let has_errors = !errors.is_empty();
        if has_errors {
            tracing::debug!(
                label = self.label,
                "Set isn't balanced. Diagnostics:\n{}",
                errors.join("\n- ")
            );
        }
        self.balanced.store(!has_errors, Ordering::SeqCst);
        self.write_diagnostics().extend(
            errors
                .iter()
                .map(|msg| ParseDiagnostic::parse_error(msg.clone(), 0)),
        );

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
                    tracing::warn!(label = self.label, "Local event validation failed: {}", e);
                    debug_assert!(false, "Local event doesn't match internal state: {event:?}");
                }
            }
            return Ok(vec![]); // Event already applied, nothing more to do
        }

        // Handle Remote events: apply changes and generate derivatives
        let mut derivative_events = vec![];
        match event {
            BeliefEvent::NodeUpdate(keys, node, _) => {
                derivative_events.append(&mut self.insert_state(node.clone(), keys));
            }
            BeliefEvent::NodeUpsert(bid, node, _) => {
                // No merge/replace semantics — BID is already canonical. Call insert_state
                // with only the BID key so to_replace can never fire (a BID key always
                // self-resolves). Debug-assert that insert_state produces no removal
                // derivatives, which would indicate an unexpected BID collision.
                let derivatives = self.insert_state(node.clone(), &[NodeKey::Bid { bid: *bid }]);
                #[cfg(debug_assertions)]
                for d in &derivatives {
                    debug_assert!(
                        !matches!(
                            d,
                            BeliefEvent::NodesRemoved(..) | BeliefEvent::NodeRenamed(..)
                        ),
                        "NodeUpsert triggered unexpected removal derivative: {d:?}. \
                         BID {bid} was expected to be canonical."
                    );
                }
                derivative_events.extend(derivatives);
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
                // Guard: if either node is absent, update_relation would warn-and-return
                // anyway, but it would still reach process_event_queue below, which fans
                // out to every PathMap in O(networks). Skip the whole event early to avoid
                // the O(N_networks) lock-acquisition cost on every dropped RelationUpdate.
                if self.bid_to_index(source).is_none() || self.bid_to_index(sink).is_none() {
                    tracing::warn!(
                        label = self.label,
                        "process_event: skipping RelationUpdate ({} -> {}), source or sink missing",
                        source,
                        sink,
                    );
                    return Ok(vec![]);
                }
                // update_relation handles both reindexing and path event generation
                let mut reindex_events = self.update_relation(*source, *sink, weight_set.clone());
                derivative_events.append(&mut reindex_events);
            }
            BeliefEvent::RelationChange(source, sink, kind, maybe_weight, origin) => {
                // Pre-assign a sort key from the memo for new edges so that
                // `generate_edge_update` skips its O(K) max() scan over incoming edges.
                // Only applies when the incoming weight lacks a sort key (new edge) and
                // the edge does not already exist in the graph (existing edges already
                // carry a sort key so the scan branch is never taken).
                let needs_sort_key = maybe_weight
                    .as_ref()
                    .map(|w| w.payload.get(WEIGHT_SORT_KEY).is_none())
                    .unwrap_or(false);

                let patched: Option<BeliefEvent> = if needs_sort_key {
                    let edge_has_sort_key = self
                        .bid_to_index(source)
                        .zip(self.bid_to_index(sink))
                        .and_then(|(src_idx, snk_idx)| {
                            self.relations().as_graph().find_edge(src_idx, snk_idx)
                        })
                        .and_then(|edge_idx| {
                            self.relations()
                                .as_graph()
                                .edge_weight(edge_idx)
                                .and_then(|ws| ws.get(kind))
                                .and_then(|w| w.get::<u16>(WEIGHT_SORT_KEY))
                        })
                        .is_some();

                    if edge_has_sort_key {
                        None // Existing edge — generate_edge_update uses the stored sort key.
                    } else {
                        let assigned = self.assign_sort_key(sink, kind);
                        let mut patched_weight = maybe_weight.clone().unwrap_or_default();
                        patched_weight
                            .set(WEIGHT_SORT_KEY, assigned)
                            .expect("failed to set u16 sort key in Weight payload");
                        Some(BeliefEvent::RelationChange(
                            *source,
                            *sink,
                            *kind,
                            Some(patched_weight),
                            *origin,
                        ))
                    }
                } else {
                    None
                };

                let event_to_resolve: &BeliefEvent = patched.as_ref().unwrap_or(event);
                if let Some(relation_mutated_event) = self.generate_edge_update(event_to_resolve) {
                    let &BeliefEvent::RelationUpdate(source, sink, ref weight_set, _) =
                        &relation_mutated_event
                    else {
                        panic!("Unexpected return value from BeliefBase::generate_edge_update");
                    };
                    // Same guard as RelationUpdate above.
                    if self.bid_to_index(&source).is_none() || self.bid_to_index(&sink).is_none() {
                        tracing::warn!(
                            label = self.label,
                            "process_event: skipping RelationChange-derived update ({} -> {}), source or sink missing",
                            source, sink,
                        );
                        return Ok(vec![]);
                    }
                    let mut reindex_events = self.update_relation(source, sink, weight_set.clone());
                    derivative_events.push(relation_mutated_event);
                    derivative_events.append(&mut reindex_events);
                }
            }
            BeliefEvent::RelationRemoved(source, sink, _) => {
                // Call update_relation with empty WeightSet to trigger proper reindexing
                // of remaining edges on the sink, ensuring contiguous sort indices [0..N)
                // Guard: if sink is absent there is nothing to reindex.
                if self.bid_to_index(source).is_none() || self.bid_to_index(sink).is_none() {
                    tracing::warn!(
                        label = self.label,
                        "process_event: skipping RelationRemoved ({} -> {}), source or sink missing",
                        source, sink,
                    );
                    return Ok(vec![]);
                }
                // Evict the memo so the counter is re-seeded from the post-reindex state
                // on the next edge addition to this sink.
                self.invalidate_sort_key_memo_for_sink(sink);
                let mut reindex_events = self.update_relation(*source, *sink, WeightSet::default());
                derivative_events.append(&mut reindex_events);
            }
            BeliefEvent::FileParsed(_) => {
                // Metadata only, handled by Transaction for mtime tracking
            }
            BeliefEvent::BatchStart | BeliefEvent::BatchEnd => {
                // No-op at the backing-store level. All batch semantics (event collection,
                // node-first reordering, cache invalidation, index_sync) are owned by
                // BeliefAccumulator. process_event only sees the reordered event slice
                // after BatchEnd has been processed by the accumulator.
            }
            BeliefEvent::BuiltInTest => {
                // Run a full built_in_test check (validates PathMapMap consistency).
                self.built_in_test();
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
        Ok(derivative_events)
    }

    /// Apply a batch of `NodeUpdate` and `NodeUpsert` / `RelationChange` / `RelationUpdate`
    /// events to the graph **without** triggering a `PathMapMap` update per event.
    ///
    /// This is the three-pass pattern used by `merge_graph_mut`:
    /// - Pass 1: all node upserts — registers BIDs in the graph so Pass 2 can look them up.
    /// - Pass 2: all relation upserts — `update_relation` called, reindex derivatives discarded.
    ///   `RelationChange` is resolved to `RelationUpdate` here so the returned vec contains
    ///   concrete events for `flush_paths_for_events`.
    ///
    /// Caller **must** call `flush_paths_for_events` on the returned events afterward to
    /// synchronize the `PathMapMap` in a single pass.
    ///
    /// # Safety contract
    /// Only `NodeUpdate`, `NodeUpsert`, `RelationChange`, and `RelationUpdate` events are
    /// accepted. `RelationRemoved` and `NodesRemoved` events must be processed via
    /// `process_event` individually *before* calling this method, while sort indices are
    /// still contiguous. Passing removal events here will trigger a `debug_assert` panic
    /// in debug builds and be silently skipped in release builds.
    pub fn apply_events_batch(
        &mut self,
        events: &[BeliefEvent],
    ) -> Result<Vec<BeliefEvent>, BuildonomyError> {
        let mut resolved: Vec<BeliefEvent> = Vec::with_capacity(events.len());

        // Pass 1: node upserts — register all BIDs in the graph before touching relations.
        #[cfg(not(target_arch = "wasm32"))]
        let (mut pass1_insert_us, mut pass1_n_node_update, mut pass1_n_node_upsert) =
            (0u128, 0u32, 0u32);
        #[cfg(not(target_arch = "wasm32"))]
        let t_pass1 = std::time::Instant::now();

        for event in events {
            match event {
                BeliefEvent::NodeUpdate(keys, node, _) => {
                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        pass1_n_node_update += 1;
                    }
                    #[cfg(not(target_arch = "wasm32"))]
                    let t_ins = std::time::Instant::now();

                    let _derivatives = self.insert_state(node.clone(), keys);

                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        pass1_insert_us += t_ins.elapsed().as_micros();
                    }
                    #[cfg(debug_assertions)]
                    for d in &_derivatives {
                        debug_assert!(
                            !matches!(
                                d,
                                BeliefEvent::NodesRemoved(..) | BeliefEvent::NodeRenamed(..)
                            ),
                            "apply_events_batch: NodeUpdate triggered unexpected removal \
                             derivative {d:?}. Removal events must be processed via \
                             process_event before calling apply_events_batch."
                        );
                    }
                    resolved.push(event.clone());
                }
                BeliefEvent::NodeUpsert(bid, node, _) => {
                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        pass1_n_node_upsert += 1;
                    }
                    #[cfg(not(target_arch = "wasm32"))]
                    let t_ins = std::time::Instant::now();

                    let _derivatives =
                        self.insert_state(node.clone(), &[NodeKey::Bid { bid: *bid }]);

                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        pass1_insert_us += t_ins.elapsed().as_micros();
                    }
                    #[cfg(debug_assertions)]
                    for d in &_derivatives {
                        debug_assert!(
                            !matches!(
                                d,
                                BeliefEvent::NodesRemoved(..) | BeliefEvent::NodeRenamed(..)
                            ),
                            "apply_events_batch: NodeUpsert triggered unexpected removal \
                             derivative {d:?}. BID {bid} was expected to be canonical."
                        );
                    }
                    resolved.push(event.clone());
                }
                BeliefEvent::RelationRemoved(..) | BeliefEvent::NodesRemoved(..) => {
                    debug_assert!(
                        false,
                        "apply_events_batch received removal event {event:?}. \
                         Process removals via process_event before calling apply_events_batch."
                    );
                }
                _ => {} // RelationChange and RelationUpdate handled in Pass 2.
            }
        }

        #[cfg(not(target_arch = "wasm32"))]
        let pass1_total_us = t_pass1.elapsed().as_micros();

        // Pass 2: relation upserts — reindex derivatives discarded; PathMapMap untouched.
        // RelationChange is resolved to RelationUpdate so flush_paths_for_events sees
        // concrete final-weight events.
        //
        // Sort-key pre-assignment: `generate_edge_update` assigns sort keys by scanning all
        // incoming edges on the sink (an O(K) max() scan). When K edges are added to the same
        // sink in one batch this becomes O(K²). We avoid that by pre-assigning sort keys via
        // `assign_sort_key`, which uses `self.next_sort_key` (a memo shared with
        // `process_event`) and seeds from the current graph max on first use per (sink, kind).

        // Fine-grained timing accumulators for Pass 2.
        // Each measures a distinct work unit per RelationChange event:
        //   sort_key_us  — bid_to_index lookups + edge_has_sort_key check + assign_sort_key
        //   gen_edge_us  — generate_edge_update (merge logic, find_edge, payload scan)
        //   update_rel_us — update_relation (write_relations lock, edge insert, reindex)
        // RelationUpdate events (already resolved) only hit update_rel_us.
        #[cfg(not(target_arch = "wasm32"))]
        let (mut sort_key_us, mut gen_edge_us, mut update_rel_us) = (0u128, 0u128, 0u128);
        #[cfg(not(target_arch = "wasm32"))]
        let (mut n_relation_change, mut n_relation_update, mut n_skipped, mut n_no_change) =
            (0u32, 0u32, 0u32, 0u32);

        for event in events {
            match event {
                BeliefEvent::RelationChange(source, sink, kind, maybe_weight, origin) => {
                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        n_relation_change += 1;
                    }

                    // Only new edges without an existing sort key need pre-assignment.
                    #[cfg(not(target_arch = "wasm32"))]
                    let t_sort = std::time::Instant::now();

                    let needs_sort_key = maybe_weight
                        .as_ref()
                        .map(|w| w.payload.get(WEIGHT_SORT_KEY).is_none())
                        .unwrap_or(false);

                    let event_to_resolve: std::borrow::Cow<BeliefEvent> = if needs_sort_key {
                        // Skip pre-assignment for edges that already exist in the graph:
                        // generate_edge_update will find WEIGHT_SORT_KEY in the stored weight.
                        let edge_has_sort_key = self
                            .bid_to_index(source)
                            .zip(self.bid_to_index(sink))
                            .and_then(|(src_idx, snk_idx)| {
                                self.relations().as_graph().find_edge(src_idx, snk_idx)
                            })
                            .and_then(|edge_idx| {
                                self.relations()
                                    .as_graph()
                                    .edge_weight(edge_idx)
                                    .and_then(|ws| ws.get(kind))
                                    .and_then(|w| w.get::<u16>(WEIGHT_SORT_KEY))
                            })
                            .is_some();

                        if edge_has_sort_key {
                            std::borrow::Cow::Borrowed(event)
                        } else {
                            // New edge — assign from the shared memo counter.
                            let assigned = self.assign_sort_key(sink, kind);
                            let mut patched_weight = maybe_weight.clone().unwrap_or_default();
                            patched_weight
                                .set(WEIGHT_SORT_KEY, assigned)
                                .expect("failed to set u16 sort key in Weight payload");
                            std::borrow::Cow::Owned(BeliefEvent::RelationChange(
                                *source,
                                *sink,
                                *kind,
                                Some(patched_weight),
                                *origin,
                            ))
                        }
                    } else {
                        std::borrow::Cow::Borrowed(event)
                    };

                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        sort_key_us += t_sort.elapsed().as_micros();
                    }

                    #[cfg(not(target_arch = "wasm32"))]
                    let t_gen = std::time::Instant::now();

                    let maybe_resolved = self.generate_edge_update(&event_to_resolve);

                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        gen_edge_us += t_gen.elapsed().as_micros();
                    }

                    if let Some(resolved_event) = maybe_resolved {
                        if let BeliefEvent::RelationUpdate(src, snk, ref ws, _) = resolved_event {
                            if self.bid_to_index(&src).is_some()
                                && self.bid_to_index(&snk).is_some()
                            {
                                #[cfg(not(target_arch = "wasm32"))]
                                let t_upd = std::time::Instant::now();

                                let _ = self.update_relation(src, snk, ws.clone());

                                #[cfg(not(target_arch = "wasm32"))]
                                {
                                    update_rel_us += t_upd.elapsed().as_micros();
                                }
                            } else {
                                #[cfg(not(target_arch = "wasm32"))]
                                {
                                    n_skipped += 1;
                                }
                            }
                        }
                        resolved.push(resolved_event);
                    } else {
                        #[cfg(not(target_arch = "wasm32"))]
                        {
                            n_no_change += 1;
                        }
                    }
                }
                BeliefEvent::RelationUpdate(src, snk, ws, _) => {
                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        n_relation_update += 1;
                    }
                    if self.bid_to_index(src).is_some() && self.bid_to_index(snk).is_some() {
                        #[cfg(not(target_arch = "wasm32"))]
                        let t_upd = std::time::Instant::now();

                        let _ = self.update_relation(*src, *snk, ws.clone());

                        #[cfg(not(target_arch = "wasm32"))]
                        {
                            update_rel_us += t_upd.elapsed().as_micros();
                        }
                    } else {
                        #[cfg(not(target_arch = "wasm32"))]
                        {
                            n_skipped += 1;
                        }
                    }
                    resolved.push(event.clone());
                }
                _ => {} // Nodes already handled in Pass 1; removals guarded above.
            }
        }

        #[cfg(not(target_arch = "wasm32"))]
        {
            tracing::debug!(
                target: "noet_core::codec::perf",
                label = self.label,
                pass1_n_node_update,
                pass1_n_node_upsert,
                pass1_insert_ms = pass1_insert_us / 1000,
                pass1_total_ms = pass1_total_us / 1000,
                n_relation_change,
                n_relation_update,
                n_skipped,
                n_no_change,
                sort_key_ms = sort_key_us / 1000,
                gen_edge_ms = gen_edge_us / 1000,
                update_rel_ms = update_rel_us / 1000,
                "[apply_events_batch] timing breakdown"
            );
        }

        Ok(resolved)
    }

    /// Flush the `PathMapMap` for a set of already-applied events (the output of
    /// `apply_events_batch`). Calls `process_event_queue` exactly once with the full
    /// event slice. Returns path derivative events for subscribers (e.g. `DbConnection`).
    pub fn flush_paths_for_events(&self, events: &[BeliefEvent]) -> Vec<BeliefEvent> {
        let event_refs: Vec<&BeliefEvent> = events.iter().collect();
        let mut pmm = self.write_paths();
        #[cfg(not(target_arch = "wasm32"))]
        {
            pmm.process_event_queue(&event_refs, &self.relations)
        }
        #[cfg(target_arch = "wasm32")]
        {
            use parking_lot::RwLock;
            use std::sync::Arc;
            let relations_arc = Arc::new(RwLock::new(self.read_relations().clone()));
            pmm.process_event_queue(&event_refs, &relations_arc)
        }
    }

    /// Insert or update a node, enforcing first-one-wins collision policy for `NodeKey::Id`.
    ///
    /// # Collision policy
    ///
    /// When the incoming node is **new** to this store and its `NodeKey::Id` key already
    /// resolves to a *different* existing node, the incoming node is the loser.  We clear
    /// its `id` field to its bref so the conflicting Id key is no longer registered for it,
    /// and we record the winner's BID in `collision_winner_bids` so the `to_replace` loop
    /// below skips it (preventing the winner from being incorrectly absorbed into the loser).
    ///
    /// This check must happen *before* the `to_replace` loop, which would otherwise
    /// misinterpret a cross-document Id collision as a same-node BID rename and incorrectly
    /// replace (remove + rename) the winning node.
    ///
    /// **Document-beats-anchor**: when an incoming *document* node shares an Id key with an
    /// existing *anchor* (section) node, the document wins. The anchor's id is clobbered to its
    /// bref in-place so the Id key is freed before the document is inserted. `push()` in
    /// `builder.rs` performs the same clobber on both `doc_bb` and `session_bb` for the builder
    /// path; this guard covers any path that reaches `insert_state` directly (e.g. parallel epoch
    /// tasks).
    ///
    /// Return a vector of events for each node that was renamed when matching on the merge keys.
    pub(crate) fn insert_state(&mut self, node: BeliefNode, merge: &[NodeKey]) -> Vec<BeliefEvent> {
        let mut events = Vec::<BeliefEvent>::new();

        let mut node = node;
        let node_bref = node.bid.bref().to_string();
        // BIDs of nodes that already own a conflicting NodeKey::Id in this store (the winners).
        // These must be excluded from `to_replace` below so they are NOT absorbed into the
        // incoming loser node.
        let mut collision_winner_bids: BTreeSet<Bid> = BTreeSet::new();

        // ID-collision guard: fires for new nodes, and also for existing nodes that are
        // *changing* their id to a value already owned by another node.
        //
        // New node: any NodeKey::Id collision means the incoming node loses (first-one-wins).
        //
        // Existing node with unchanged id: skip — this node already won its ID on a prior
        // parse; re-checking would treat the stored owner as a spurious loser.
        //
        // Existing node with a *changed* id: the update is attempting to claim a new Id key.
        // If that key is already owned by a different node we cannot evict the owner, so the
        // incoming node's id is cleared to its bref instead (same first-one-wins rule).
        // None if node is new; Some(old_id_string) if it already exists in this store.
        let old_id = self.states.get(&node.bid).map(|old| old.id());
        let is_new_node_for_collision = old_id.is_none();
        // An existing node is changing its id when the stored id differs from the incoming one.
        let id_is_changing = old_id.as_ref().is_some_and(|oid| *oid != node.id());

        if is_new_node_for_collision || id_is_changing {
            for key in merge.iter() {
                if let NodeKey::Id { .. } = key {
                    if let Some(existing) = self.get(key) {
                        if existing.bid != node.bid {
                            let incoming_is_document = node.kind.0.contains(BeliefKind::Document);
                            let incoming_is_network = node.kind.0.contains(BeliefKind::Network);
                            let existing_is_anchor = existing.kind.is_anchor();
                            let _existing_is_network =
                                existing.kind.0.contains(BeliefKind::Network);

                            if incoming_is_network && existing_is_anchor {
                                // Network beats anchor: a network node owns its directory name
                                // by definition.  A heading in the network's own index.md that
                                // matches the directory name (e.g. `# Power` in `power/index.md`)
                                // loses — clobber the anchor's id to its bref so the Id key is
                                // freed before we insert the network node.
                                let anchor_bref = existing.bid.bref().to_string();
                                let msg = format!(
                                    "ID collision on {:?}: incoming network node {} beats existing \
                                     anchor {}; anchor id reset to bref '{}'. \
                                     Add an explicit id to the heading to resolve.",
                                    key, node.bid, existing.bid, anchor_bref,
                                );
                                tracing::debug!(label = self.label, "insert_state: {}", msg);
                                self.write_diagnostics().push(ParseDiagnostic::warning(msg));
                                if let Some(stored) = self.states.get_mut(&existing.bid) {
                                    stored.id = NodeId::Explicit(anchor_bref);
                                    let clobber_event = BeliefEvent::NodeUpdate(
                                        vec![
                                            NodeKey::Bid { bid: stored.bid },
                                            NodeKey::Bref {
                                                bref: stored.bid.bref(),
                                            },
                                        ],
                                        stored.clone(),
                                        EventOrigin::Local,
                                    );
                                    events.push(clobber_event);
                                }
                            } else if incoming_is_document && existing_is_anchor {
                                // Document beats anchor: clobber the existing anchor's id to its
                                // bref in-place so the Id key is freed before we insert the
                                // document.  The winner (document) keeps its id; the anchor loses.
                                let anchor_bref = existing.bid.bref().to_string();
                                let msg = format!(
                                    "ID collision on {:?}: incoming document {} beats existing \
                                     anchor {}; anchor id reset to bref '{}'. \
                                     Add an explicit anchor id in the section's source file to resolve.",
                                    key, node.bid, existing.bid, anchor_bref,
                                );
                                tracing::debug!(label = self.label, "insert_state: {}", msg);
                                self.write_diagnostics().push(ParseDiagnostic::warning(msg));
                                // Mutate the stored anchor in-place — no eviction needed.
                                // Emit a NodeUpdate derivative so PathMapMap.process_event_queue
                                // sees the id change and calls update_path_segment on the
                                // affected PathMaps to regenerate the stale path string.
                                if let Some(stored) = self.states.get_mut(&existing.bid) {
                                    stored.id = NodeId::Explicit(anchor_bref);
                                    // Serialize after mutation so the event carries the new id.
                                    // Use minimal Bid+Bref keys — PathMapMap only needs the BID
                                    // to detect the id change; full path keys require a &BeliefBase
                                    // borrow that conflicts with &mut self here.
                                    let clobber_event = BeliefEvent::NodeUpdate(
                                        vec![
                                            NodeKey::Bid { bid: stored.bid },
                                            NodeKey::Bref {
                                                bref: stored.bid.bref(),
                                            },
                                        ],
                                        stored.clone(),
                                        EventOrigin::Local,
                                    );
                                    events.push(clobber_event);
                                }
                                // Do NOT add existing.bid to collision_winner_bids: the anchor
                                // lost, so the to_replace loop may legitimately absorb it if
                                // another key matches.
                            } else {
                                // First-one-wins: incoming loses.
                                // Clear our id to bref so the conflicting key is no longer
                                // generated for us when we are inserted below. Record the winner's
                                // BID so the to_replace loop skips it (we must NOT rename/absorb
                                // the winner into the incoming loser node).
                                let msg = format!(
                                    "ID collision on {:?}: existing node {} keeps the id; \
                                     incoming node {} has its id cleared to bref '{}'. \
                                     Add an explicit anchor to one of the headings to resolve.",
                                    key, existing.bid, node.bid, node_bref,
                                );
                                tracing::debug!(label = self.label, "insert_state: {}", msg);
                                self.write_diagnostics().push(ParseDiagnostic::warning(msg));
                                collision_winner_bids.insert(existing.bid);
                                let colliding_slug = if let NodeKey::Id { id, .. } = key {
                                    id.clone()
                                } else {
                                    node.id()
                                };
                                node.id = NodeId::Collision(colliding_slug);
                                // If this node was previously stored, emit a NodeUpdate so
                                // PathMapMap sees the id change and regenerates stale path
                                // entries — mirrors the clobber event emitted in the
                                // document-beats-anchor branch above.
                                if self.states.contains_key(&node.bid) {
                                    let clobber_event = BeliefEvent::NodeUpdate(
                                        vec![
                                            NodeKey::Bid { bid: node.bid },
                                            NodeKey::Bref {
                                                bref: node.bid.bref(),
                                            },
                                        ],
                                        node.clone(),
                                        EventOrigin::Local,
                                    );
                                    events.push(clobber_event);
                                }
                                // Stop checking: id is now resolved to bref, no further Id keys apply.
                                break;
                            }
                        }
                    }
                }
            }
        }

        let mut to_replace = BTreeSet::<Bid>::new();
        for key in merge.iter() {
            // Fast path for NodeKey::Bid: self.get() for a Bid key is a direct
            // O(log N) lookup. If the key's BID matches node.bid it would be
            // stripped by to_replace.remove(&node.bid) anyway, so skip. If the
            // BID differs, use the full path so the existing node is correctly
            // absorbed.
            if let NodeKey::Bid { bid } = key {
                if *bid != node.bid {
                    if let Some(existing) = self.states.get(bid) {
                        if !collision_winner_bids.contains(&existing.bid) {
                            to_replace.insert(existing.bid);
                        }
                    }
                }
                // If bid == node.bid: would be stripped by to_replace.remove below; skip.
                continue;
            }
            if let Some(existing) = self.get(key) {
                // Skip collision winners — they keep their own BID and must not be
                // absorbed into the incoming (losing) node.
                if !collision_winner_bids.contains(&existing.bid) {
                    to_replace.insert(existing.bid);
                }
            }
        }
        to_replace.remove(&node.bid);
        if !to_replace.is_empty() {
            tracing::debug!(
                label = self.label,
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
            // For new nodes: register in the relations graph and bid_to_index now
            // so that subsequent relation lookups find this node without a full rebuild.
            if is_new_node && !self.read_bid_index().contains_key(&bid) {
                self.graph_insert_node(bid);
            }
        }

        // The NodeRenamed/NodesRemoved events emitted below are LOCAL bookkeeping,
        // not the authoritative record of this absorption.
        //
        // They exist to drive *this* base's own PathMapMap: `process_event` folds
        // them into its `event_queue`, which is what re-points index entries off
        // the absorbed BID. They carry `EventOrigin::Local` precisely because the
        // state is already applied here.
        //
        // The authoritative derivatives — the ones every `BeliefSink` applies — are
        // produced by `resolve_merge_keys` in `beliefbase/accumulator.rs` at
        // `BatchEnd`. Only it can see the pending batch and carry absorptions
        // across batches, which a deterministic (UUID v5) stub BID requires. Do not
        // forward these to `tx`; that would race the accumulator's version.
        //
        // The absorption must nonetheless happen *here*. `GraphBuilder` drives
        // `doc_bb` and `session_bb` through `process_event` directly, never through
        // an accumulator, and a single-file parse can mint a stub and then claim it
        // later in the same file. Without local absorption those bases would carry
        // the duplicate for the rest of the parse, and everything computed from
        // them — `compute_diff` output above all — would inherit it.
        for replaced in to_replace.iter() {
            // Call replace_bid BEFORE removing from states, because replace_bid
            // needs to transfer edges from the replaced node to the new node
            events.push(BeliefEvent::NodeRenamed(*replaced, bid, EventOrigin::Local));
            events.append(&mut self.replace_bid(*replaced, bid));

            // Now remove from states (replace_bid already removed from graph)
            self.states.remove(replaced);
            self.brefs.remove(&replaced.bref());
        }
        // index_dirty is no longer used; bid_to_index is updated incrementally above
        // and in replace_bid / remove_nodes.
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

        let mut sink_kinds: BTreeMap<Bid, BTreeSet<WeightKind>> = BTreeMap::new();
        // Collect owner_edges entries to deindex before nodes (and their edges) are removed.
        let mut owner_edges_to_deindex: Vec<(EdgeIndex, WeightSet)> = Vec::new();
        {
            let relations = self.read_relations();
            let bid_to_index = self.read_bid_index();
            for bid in bids {
                if let Some(&node_idx) = bid_to_index.get(bid) {
                    // Outgoing edges (this node as source)
                    for edge in relations.as_graph().edges(node_idx) {
                        let sink = relations.as_graph()[edge.target()];
                        let kinds = edge
                            .weight()
                            .weights
                            .keys()
                            .copied()
                            .collect::<BTreeSet<_>>();
                        sink_kinds.entry(sink).or_default().extend(kinds);
                        owner_edges_to_deindex.push((edge.id(), edge.weight().clone()));
                    }
                    // Incoming edges (this node as sink) — also removed by remove_node.
                    for edge in relations
                        .as_graph()
                        .edges_directed(node_idx, Direction::Incoming)
                    {
                        owner_edges_to_deindex.push((edge.id(), edge.weight().clone()));
                    }
                }
            }
        }

        // Deindex owner_edges before the graph nodes are removed.
        for (edge_idx, ws) in &owner_edges_to_deindex {
            self.deindex_owner_edge(*edge_idx, ws);
        }

        // Remove nodes from states
        for bid in bids {
            if self.states.remove(bid).is_some() {
                self.brefs.remove(&bid.bref());
            }
        }

        // Remove nodes from graph and update bid_to_index incrementally.
        // StableGraph indices are stable across removals, so we can remove one-by-one safely.
        {
            let mut relations = self.write_relations();
            let mut index = self.write_bid_index();
            for bid in bids {
                if let Some(&idx) = index.get(bid) {
                    relations.as_graph_mut().remove_node(idx);
                    index.remove(bid);
                }
            }
        }
        // Reindex edges for affected sinks using the centralized reindex_sink_edges.
        // Also evict sort-key memo entries for each affected sink so that the counter
        // is re-seeded from the compacted post-removal state on the next edge addition.
        let mut derivative_events = vec![];
        for (sink, kinds) in sink_kinds {
            if bids.contains(&sink) {
                continue; // Already removed — no edges left to reindex.
            }
            self.invalidate_sort_key_memo_for_sink(&sink);
            let mut reindex_events = self.reindex_sink_edges(&sink, &kinds);
            derivative_events.append(&mut reindex_events);
        }

        derivative_events
    }

    /// Pre-assign a sort key for a new `(sink, kind)` edge from the memoized counter,
    /// seeding the counter from the current graph max on first use.
    ///
    /// Returns the sort key to use and advances the counter.  Callers must only invoke
    /// this for **new** edges (i.e. the `(source, sink, kind)` triple does not yet exist
    /// in the graph), because the counter is not aware of existing sort indices on the edge.
    ///
    /// The memo must be invalidated (entry removed) whenever an edge is removed from a
    /// sink so the counter is re-seeded from the compacted post-reindex state.
    fn assign_sort_key(&mut self, sink: &Bid, kind: &WeightKind) -> u16 {
        // Pre-compute the seed before touching next_sort_key to avoid a double-borrow:
        // `or_insert_with` takes `&mut self.next_sort_key` while the closure would also
        // need `&self` to call `bid_to_index` / `relations`.
        let seed: Option<u16> = if self.next_sort_key.contains_key(&(*sink, *kind)) {
            None // Already seeded — seed computation not needed.
        } else {
            Some(
                self.bid_to_index(sink)
                    .map(|sink_idx| {
                        self.relations()
                            .as_graph()
                            .edges_directed(sink_idx, Direction::Incoming)
                            .filter_map(|edge| {
                                edge.weight()
                                    .get(kind)
                                    .and_then(|w| w.get::<u16>(WEIGHT_SORT_KEY))
                            })
                            .max()
                            .map(|m| m + 1)
                            .unwrap_or(0)
                    })
                    .unwrap_or(0),
            )
        };
        let counter = self
            .next_sort_key
            .entry((*sink, *kind))
            .or_insert_with(|| seed.unwrap_or(0));
        let key = *counter;
        *counter += 1;
        key
    }

    /// Evict all memoized sort-key counters for the given `sink`.
    ///
    /// Must be called whenever edges are removed from `sink` so that the next addition
    /// re-seeds the counter from the compacted post-`reindex_sink_edges` state.
    fn invalidate_sort_key_memo_for_sink(&mut self, sink: &Bid) {
        self.next_sort_key.retain(|(s, _), _| s != sink);
    }

    /// Extract third-party owner brefs from a `WeightSet`.
    ///
    /// Returns brefs for `WEIGHT_OWNED_BY` values that are neither `"source"`,
    /// `"sink"`, nor absent — i.e. only third-party bref owners that need indexing.
    fn third_party_owner_brefs(ws: &WeightSet) -> impl Iterator<Item = Bref> + '_ {
        ws.weights.values().filter_map(|weight| {
            match weight.get::<String>(WEIGHT_OWNED_BY).as_deref() {
                Some("source") | Some("sink") | None => None,
                Some(bref_str) => Bref::try_from(bref_str).ok(),
            }
        })
    }

    /// Add an edge index to `owner_edges` for all third-party owner brefs in `ws`.
    fn index_owner_edge(&mut self, edge_idx: EdgeIndex, ws: &WeightSet) {
        for bref in Self::third_party_owner_brefs(ws) {
            self.owner_edges.entry(bref).or_default().insert(edge_idx);
        }
    }

    /// Remove an edge index from `owner_edges` for all third-party owner brefs in `ws`.
    /// Cleans up empty entries.
    fn deindex_owner_edge(&mut self, edge_idx: EdgeIndex, ws: &WeightSet) {
        for bref in Self::third_party_owner_brefs(ws) {
            if let BTreeEntry::Occupied(mut entry) = self.owner_edges.entry(bref) {
                entry.get_mut().remove(&edge_idx);
                if entry.get().is_empty() {
                    entry.remove();
                }
            }
        }
    }

    /// Build `owner_edges` from scratch by scanning all edges in the relations graph.
    /// Called by `new_unbalanced` during construction.
    fn build_owner_edges(&mut self) {
        self.owner_edges.clear();
        // Collect (edge_idx, bref) pairs while holding the relations read lock,
        // then populate owner_edges after the lock is dropped.
        let entries: Vec<(EdgeIndex, Bref)> = {
            let relations = self.read_relations();
            relations
                .as_graph()
                .edge_references()
                .flat_map(|edge_ref| {
                    let edge_idx = edge_ref.id();
                    Self::third_party_owner_brefs(edge_ref.weight())
                        .map(move |bref| (edge_idx, bref))
                })
                .collect()
        };
        for (edge_idx, bref) in entries {
            self.owner_edges.entry(bref).or_default().insert(edge_idx);
        }
    }

    fn generate_edge_update(&self, event: &BeliefEvent) -> Option<BeliefEvent> {
        let BeliefEvent::RelationChange(source, sink, kind, maybe_weight, origin) = event else {
            return None;
        };

        let source_idx = self.bid_to_index(source);
        let sink_idx = self.bid_to_index(sink);

        let present_weight = if let (Some(src_idx), Some(snk_idx)) = (source_idx, sink_idx) {
            self.relations()
                .as_graph()
                .find_edge(src_idx, snk_idx)
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
            tracing::debug!(
                label = self.label,
                "[BeliefBase::update_relation] Waiting for write access to relations"
            );
            std::thread::sleep(std::time::Duration::from_millis(100));
        }

        let maybe_source_idx = self.bid_to_index(&source);
        let maybe_sink_idx = self.bid_to_index(&sink);
        if maybe_source_idx.is_none() || maybe_sink_idx.is_none() {
            // Skip if either node has been removed
            tracing::warn!(
                label = self.label,
                "Skipping update_relation({} -[{}]-> {}), source is missing: {}, sink is missing: {}",
                self.states().get(&source).map(|n| n.display_title()).unwrap_or(source.to_string()),
                new_weight_set.weights.keys().map(|k| k.to_string()).collect::<Vec<String>>().join(", "),
                self.states().get(&sink).map(|n| n.display_title()).unwrap_or(sink.to_string()),
                maybe_source_idx.is_none(),
                maybe_sink_idx.is_none(),
            );
            return vec![];
        }

        let source_idx = maybe_source_idx.unwrap();
        let sink_idx = maybe_sink_idx.unwrap();

        // Tracks what changed for owner_edges memo maintenance (applied after lock drop).
        enum OwnerEdgeDelta {
            Removed {
                edge_idx: EdgeIndex,
                old_ws: WeightSet,
            },
            Updated {
                edge_idx: EdgeIndex,
                old_ws: WeightSet,
                new_ws: WeightSet,
            },
            Added {
                edge_idx: EdgeIndex,
                new_ws: WeightSet,
            },
        }

        let (affected_kinds, owner_delta) = {
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
            let affected_kinds: BTreeSet<WeightKind> = old_weight_set
                .difference(&new_weight_set)
                .weights
                .keys()
                .copied()
                .collect();

            // Update or add/remove the edge.
            let delta = if new_weight_set.is_empty() {
                // Remove edge
                if let Some(edge_idx) = relations.as_graph().find_edge(source_idx, sink_idx) {
                    relations.as_graph_mut().remove_edge(edge_idx);
                    Some(OwnerEdgeDelta::Removed {
                        edge_idx,
                        old_ws: old_weight_set,
                    })
                } else {
                    None
                }
            } else if let Some(edge_idx) = relations.as_graph().find_edge(source_idx, sink_idx) {
                // Update existing edge
                let edge_weight = relations
                    .as_graph_mut()
                    .edge_weight_mut(edge_idx)
                    .expect("We got this edge index from the graph, why can't we access it?");
                *edge_weight = new_weight_set.clone();
                Some(OwnerEdgeDelta::Updated {
                    edge_idx,
                    old_ws: old_weight_set,
                    new_ws: new_weight_set,
                })
            } else {
                // Add new edge
                let edge_idx =
                    relations
                        .as_graph_mut()
                        .add_edge(source_idx, sink_idx, new_weight_set.clone());
                Some(OwnerEdgeDelta::Added {
                    edge_idx,
                    new_ws: new_weight_set,
                })
            };

            (affected_kinds, delta)
        }; // relations lock dropped here

        // Maintain owner_edges memo outside the relations lock.
        match owner_delta {
            Some(OwnerEdgeDelta::Removed {
                edge_idx,
                ref old_ws,
            }) => {
                self.deindex_owner_edge(edge_idx, old_ws);
            }
            Some(OwnerEdgeDelta::Updated {
                edge_idx,
                ref old_ws,
                ref new_ws,
            }) => {
                self.deindex_owner_edge(edge_idx, old_ws);
                self.index_owner_edge(edge_idx, new_ws);
            }
            Some(OwnerEdgeDelta::Added {
                edge_idx,
                ref new_ws,
            }) => {
                self.index_owner_edge(edge_idx, new_ws);
            }
            None => {}
        }

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
                label = self.label,
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
                        NodeIndex,
                        NodeIndex,
                        BTreeMap<WeightKind, u16>,
                    )| {
                        ks.get(kind)
                            .map(|weight_idx| (*source_idx, *sink_idx, *weight_idx))
                    },
                )
                .collect::<Vec<(NodeIndex, NodeIndex, u16)>>();
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

        if let Some(replaced_idx) = self.bid_to_index(&replaced_bid) {
            // Ensure new_bid has a graph node before we acquire the relations write lock.
            // graph_insert_node is a no-op if the node already exists in bid_to_index.
            if !self.read_bid_index().contains_key(&new_bid) {
                self.graph_insert_node(new_bid);
            }
            let new_idx = self
                .bid_to_index(&new_bid)
                .expect("new_bid must be in graph after graph_insert_node");

            // Collect owner_edges deltas while holding the relations lock.
            // Each entry: (edge_idx_to_deindex, ws) for removals, or (edge_idx_to_index, ws) for additions.
            let mut to_deindex: Vec<(EdgeIndex, WeightSet)> = Vec::new();
            let mut to_index: Vec<(EdgeIndex, WeightSet)> = Vec::new();

            {
                let mut relations = self.write_relations();

                let mut outgoing = relations
                    .as_graph()
                    .neighbors_directed(replaced_idx, petgraph::Direction::Outgoing)
                    .detach();
                while let Some((edge_idx, sink_idx)) = outgoing.next(relations.as_graph()) {
                    let sink = relations.as_graph()[sink_idx];
                    let old_ws = relations
                        .as_graph()
                        .edge_weight(edge_idx)
                        .expect("Edge should exist")
                        .clone();
                    to_deindex.push((edge_idx, old_ws));
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

                    if let Some(existing_edge_idx) =
                        relations.as_graph().find_edge(new_idx, sink_idx)
                    {
                        let existing_ws = relations.as_graph()[existing_edge_idx].clone();
                        to_deindex.push((existing_edge_idx, existing_ws));
                        let existing_weight = &mut relations.as_graph_mut()[existing_edge_idx];
                        *existing_weight = existing_weight.union(&from_weight);
                        let merged_ws = relations.as_graph()[existing_edge_idx].clone();
                        to_index.push((existing_edge_idx, merged_ws));
                    } else if !from_weight.is_empty() {
                        let new_edge_idx = relations.as_graph_mut().add_edge(
                            new_idx,
                            sink_idx,
                            from_weight.clone(),
                        );
                        to_index.push((new_edge_idx, from_weight));
                    }
                }

                let mut incoming = relations
                    .as_graph()
                    .neighbors_directed(replaced_idx, petgraph::Direction::Incoming)
                    .detach();
                while let Some((edge_idx, source_idx)) = incoming.next(relations.as_graph()) {
                    let source = relations.as_graph()[source_idx];
                    let old_ws = relations
                        .as_graph()
                        .edge_weight(edge_idx)
                        .expect("Edge should exist")
                        .clone();
                    to_deindex.push((edge_idx, old_ws));
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

                    if let Some(existing_edge_idx) =
                        relations.as_graph().find_edge(source_idx, new_idx)
                    {
                        let existing_ws = relations.as_graph()[existing_edge_idx].clone();
                        to_deindex.push((existing_edge_idx, existing_ws));
                        let existing_weight = &mut relations.as_graph_mut()[existing_edge_idx];
                        *existing_weight = existing_weight.union(&from_weight);
                        let merged_ws = relations.as_graph()[existing_edge_idx].clone();
                        to_index.push((existing_edge_idx, merged_ws));
                    } else if !from_weight.is_empty() {
                        let new_edge_idx = relations.as_graph_mut().add_edge(
                            source_idx,
                            new_idx,
                            from_weight.clone(),
                        );
                        to_index.push((new_edge_idx, from_weight));
                    }
                }
            } // relations lock dropped here

            // Apply owner_edges deltas outside the relations lock.
            for (edge_idx, ws) in &to_deindex {
                self.deindex_owner_edge(*edge_idx, ws);
            }
            for (edge_idx, ws) in &to_index {
                self.index_owner_edge(*edge_idx, ws);
            }

            self.graph_remove_node(replaced_idx, &replaced_bid);
        }
        derivative_events
    }

    /// If the BeliefBase is singular (only one state in the set) returns a clone of the
    /// state. Otherwise None
    pub fn into_state(&mut self) -> Option<BeliefNode> {
        let BeliefGraph { mut states, .. } = self.consume();
        // Remove the API node first (it's not "content" for this method's purpose),
        // then take whatever single content node remains, if any.
        states.remove(&self.api.bid);
        let maybe_node = states
            .keys()
            .next()
            .copied()
            .and_then(|bid| states.remove(&bid));
        if !states.is_empty() {
            tracing::warn!(
                label = self.label,
                "Converted a multi-node BeliefBase into a BeliefNode. Remaining nodes: {:?}",
                states
            );
        }
        maybe_node
    }

    /// Merge `rhs` into `self` using an unbounded DFS seed (`self.states ∩ rhs.states`).
    ///
    /// # Warning
    ///
    /// This is intentionally `pub(crate)` — callers outside `beliefbase` must use
    /// [`merge_from`] with an explicit seed set to avoid O(session_bb_size × rhs_edges)
    /// fan-out as `session_bb` grows across a corpus run (Issue 47 BN-1).
    ///
    /// Legitimate internal uses: shard loading onto a fresh/empty base, and tests where
    /// the lhs is small enough that the unbounded seed is harmless.
    #[allow(dead_code)] // used by shard/export.rs and wasm.rs under feature flags
    pub(crate) fn merge(&mut self, rhs: &BeliefGraph) {
        self.merge_graph_mut(rhs, None, MergePrecedence::LhsWins);
    }

    /// Like `merge`, but restricts the DFS seed set in the relation merge to `seed_bids`.
    ///
    /// Use at call sites where the relevant rhs BIDs are already known (e.g. the current
    /// file's `parsed_bids` during Phase 2 of `parse_content`). This prevents the DFS from
    /// seeding from all of `self.states`, keeping the cost O(rhs_size) rather than
    /// O(session_bb_size × rhs_edges).
    pub fn merge_from(&mut self, rhs: &BeliefGraph, seed_bids: &BTreeSet<Bid>) {
        self.merge_graph_mut(rhs, Some(seed_bids), MergePrecedence::LhsWins);
    }

    /// [`merge_from`] with an explicit node-collision policy.
    ///
    /// Use [`MergePrecedence::RhsWins`] when `rhs` is more authoritative than `self` —
    /// e.g. seeding a parallel parse task, where `self` is a shared epoch base that
    /// accumulates across the epoch and `rhs` is a per-document seed queried fresh from
    /// `global_bb`.
    ///
    /// [`merge_from`]: BeliefBase::merge_from
    pub fn merge_from_with(
        &mut self,
        rhs: &BeliefGraph,
        seed_bids: &BTreeSet<Bid>,
        precedence: MergePrecedence,
    ) {
        self.merge_graph_mut(rhs, Some(seed_bids), precedence);
    }

    /// Core implementation for `merge` and `merge_from`.
    ///
    /// Merges `rhs` into `self` using [`MergeOp`]s produced by
    /// [`BeliefGraph::to_event_stream`].
    ///
    /// **Performance contract — O(rhs_size):**
    ///
    /// 1. **Node pass**: Apply every [`MergeOp::NodeUpsert`] directly via
    ///    `self.states.insert` — no TOML serialisation.
    ///    New nodes are registered in the relations graph and `bid_to_index`
    ///    incrementally via `graph_insert_node` so Pass 2 can look up `NodeIndex`
    ///    immediately without any rebuild.
    ///
    /// 2. **Relation pass**: Apply every [`MergeOp::RelationUpdate`] via
    ///    `update_relation`, which uses the always-current `bid_to_index`.
    ///
    /// 3. **PathMapMap pass**: Drive `process_event_queue` once with the full op list
    ///    converted to `BeliefEvent` refs, keeping PathMap metadata consistent.
    ///
    /// This eliminates the O(N²) behaviour of the previous per-event `process_event`
    /// loop (each call triggered `insert_state` over the entire
    /// growing `session_bb`, with a full `bid_to_index` rebuild after each mutation).
    ///
    /// `seed_bids`: when `Some`, scopes the ops to the halo around those seeds
    /// (seeds + neighbours + balanced Section ancestors). When `None`, all rhs nodes
    /// and relations are emitted.
    fn merge_graph_mut(
        &mut self,
        rhs: &BeliefGraph,
        seed_bids: Option<&BTreeSet<Bid>>,
        precedence: MergePrecedence,
    ) {
        let ops = rhs.to_event_stream_with(self, seed_bids, precedence);
        if ops.is_empty() {
            return;
        }

        // ------------------------------------------------------------------
        // Pass 1: apply node upserts directly — no TOML round-trip.
        // New nodes are registered in the relations graph and bid_to_index
        // incrementally so Pass 2's update_relation can look up NodeIndex immediately.
        // ------------------------------------------------------------------
        for op in &ops {
            if let MergeOp::NodeUpsert(node) = op {
                let is_new = !self.states.contains_key(&node.bid);
                let changed = is_new || self.states.get(&node.bid) != Some(node);
                if changed {
                    self.states.insert(node.bid, node.clone());
                    self.brefs.insert(node.bid.bref(), node.bid);
                    if is_new && !self.read_bid_index().contains_key(&node.bid) {
                        self.graph_insert_node(node.bid);
                    }
                }
            }
        }

        // ------------------------------------------------------------------
        // Pass 2: apply relation updates using the freshly rebuilt index.
        // Derivative reindex events are collected but not re-processed here —
        // process_event_queue handles PathMap state in pass 3.
        // ------------------------------------------------------------------
        for op in &ops {
            if let MergeOp::RelationUpdate(source, sink, weight_set) = op {
                // Discard derivative reindex events — process_event_queue (pass 3)
                // rebuilds and sorts all PathMaps from the full op list at once.
                let _ = self.update_relation(*source, *sink, weight_set.clone());
            }
        }

        // ------------------------------------------------------------------
        // Pass 3: drive PathMapMap with the full op list, converted to BeliefEvent refs.
        // process_event_queue runs its own two-pass (titles first, then PathMaps) and
        // sorts all PathMaps at the end — correctness is independent of arrival order.
        // ------------------------------------------------------------------
        let belief_events: Vec<BeliefEvent> = ops.iter().map(MergeOp::to_belief_event).collect();
        let event_refs: Vec<&BeliefEvent> = belief_events.iter().collect();
        let mut pmm = self.write_paths();
        #[cfg(not(target_arch = "wasm32"))]
        {
            pmm.process_event_queue(&event_refs, &self.relations);
        }
        #[cfg(target_arch = "wasm32")]
        {
            use parking_lot::RwLock;
            use std::sync::Arc;
            let relations_arc = Arc::new(RwLock::new(self.read_relations().clone()));
            pmm.process_event_queue(&event_refs, &relations_arc);
        }
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
            tracing::debug!(
                label = self.label,
                "[BeliefBase::trim] Waiting for write access to relations"
            );
            std::thread::sleep(std::time::Duration::from_millis(100));
        }

        // Collect edges to remove and their owner weight sets (for memo deindexing),
        // then remove edges and drop the lock before updating owner_edges.
        let mut owner_deindex: Vec<(EdgeIndex, WeightSet)> = Vec::new();
        let mut remove_events = Vec::new();
        {
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

            for (edge_idx, source, sink) in to_remove.into_iter().rev() {
                if let Some(ws) = write_relations.as_graph().edge_weight(edge_idx) {
                    owner_deindex.push((edge_idx, ws.clone()));
                }
                write_relations.as_graph_mut().remove_edge(edge_idx);
                remove_events.push(BeliefEvent::RelationRemoved(
                    source,
                    sink,
                    EventOrigin::Local,
                ));
            }
        } // write_relations lock dropped

        for (edge_idx, ws) in &owner_deindex {
            self.deindex_owner_edge(*edge_idx, ws);
        }
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // QuerySpec evaluation
    // ═══════════════════════════════════════════════════════════════════════════

    /// Evaluate a [`QueryPackage`] directly against in-memory state.
    ///
    /// Inspects [`QueryPackage::stage`] and resumes from wherever the
    /// package left off:
    ///
    /// - **Constructed** → resolve seed `TapeFn` to `TapeFn::Bids`, then
    ///   fall through to projection and output.
    /// - **Anchored** → apply projection steps, then fall through
    ///   to output.
    /// - **Projecting** → resume projection from where the tape left
    ///   off, then fall through to output.
    /// - **Projected** → materialize graph from tape (if not already done).
    pub fn evaluate_query(&self, package: &mut QueryPackage) -> Result<(), BuildonomyError> {
        self.evaluate_query_with_search(package, None)
    }

    /// Evaluate a query package with an optional text search provider.
    ///
    /// When `text_search` is `Some`, `TextMatch` filter steps are evaluated
    /// by delegating to the provider. This is the entry point for WASM
    /// evaluation where search indices are available.
    pub fn evaluate_query_with_search(
        &self,
        package: &mut QueryPackage,
        text_search: Option<&dyn TextSearchProvider>,
    ) -> Result<(), BuildonomyError> {
        // ── Anchor: resolve seed TapeFn to Bids ─────────────────
        // For Keys seeds, record which key resolved to which BID so
        // the seed payload can link output BIDs back to their anchor keys.
        let mut anchor_map: Option<Vec<(usize, Bid)>> = None;
        if package.stage() == PackageStage::Constructed {
            let original_tapefn = package
                .spec()
                .steps
                .first()
                .map(|s| s.input.clone())
                .unwrap_or(TapeFn::Bids(vec![]));

            // If the first step is a Compose with no top-level seed (branches
            // carry their own seeds), use an empty Bids set so downstream code
            // doesn't error on Then(None).
            let needs_top_level_seed = original_tapefn.is_seed();

            if needs_top_level_seed {
                let seed = self.eval_seed(&original_tapefn)?;
                if let TapeFn::Keys(keys) = &original_tapefn {
                    let mapping: Vec<(usize, Bid)> = keys
                        .iter()
                        .enumerate()
                        .filter_map(|(i, key)| {
                            self.resolve_key_to_bid(key)
                                .filter(|bid| seed.contains(bid))
                                .map(|bid| (i, bid))
                        })
                        .collect();
                    if !mapping.is_empty() {
                        anchor_map = Some(mapping);
                    }
                }
                if let Some(first_step) = package.spec_mut().steps.first_mut() {
                    first_step.input = TapeFn::Bids(seed.iter().copied().collect());
                }
            } else {
                // No top-level seed — set empty Bids so the tape-based
                // evaluator can proceed. Compose branches provide their
                // own seeds internally.
                if let Some(first_step) = package.spec_mut().steps.first_mut() {
                    first_step.input = TapeFn::Bids(vec![]);
                }
            }
        }

        // ── Store anchor map for the first step's payload ───────────────
        // The anchor map records which seed key resolved to which BID.
        // It will be attached to the first step's tape entry as payload.
        if let Some(map) = anchor_map {
            package.set_anchor_map(map);
        }

        // ── Project: apply projection steps into tape ───────────────
        if package.stage() < PackageStage::Projected {
            self.apply_projection_steps_to_package(package, text_search)?;
        }

        // ── Materialize graph from tape ─────────────────────────
        if package.stage() == PackageStage::Projected && package.graph().is_none() {
            self.materialize_graph(package)?;
        }

        Ok(())
    }

    /// Apply projection steps directly into the package's tape.
    ///
    /// Recovers the current BID set from the package state:
    /// - If the tape has entries (resuming from `Projecting`), the current
    ///   set is the last tape entry's result.
    /// - Otherwise, the seed comes from the first step's `TapeFn::Bids`
    ///   input (which must already be resolved at this point).
    ///
    /// For steps with `TapeFn::Fold { op: Union, range: None }`, the input
    /// is `seed ∪ all prior tape BIDs` instead of the chain output.
    ///
    /// Skips steps already recorded in the tape, then applies remaining
    /// steps, pushing a [`TapeEntry`] for each.
    fn apply_projection_steps_to_package(
        &self,
        package: &mut QueryPackage,
        text_search: Option<&dyn TextSearchProvider>,
    ) -> Result<(), BuildonomyError> {
        let steps = package.spec().steps.clone();
        let seed_tapefn = steps
            .first()
            .map(|s| s.input.clone())
            .unwrap_or(TapeFn::Bids(vec![]));
        // Count completed projection steps.
        let completed = package.tape().steps.len();

        let seed: BTreeSet<Bid> = match &seed_tapefn {
            TapeFn::Bids(bids) => bids.iter().copied().collect(),
            _ => {
                return Err(BuildonomyError::Command(
                    "apply_projection_steps_to_package called with unresolved seed".to_string(),
                ));
            }
        };

        // Recover the chain set from the last tape entry, or use seed.
        let mut current: BTreeSet<Bid> = if let Some(last) = package.tape().steps.last() {
            last.content.output_bids().into_iter().collect()
        } else {
            seed.clone()
        };

        for (step_idx, step) in steps.iter().enumerate().skip(completed) {
            let label = if step.label.is_empty() {
                step_idx.to_string()
            } else {
                step.label.clone()
            };

            // Compute the label of the previous step (for Terminal/Orphan range resolution).
            let prev_label: Option<String> = if step_idx > 0 {
                let prev = &steps[step_idx - 1];
                Some(if prev.label.is_empty() {
                    (step_idx - 1).to_string()
                } else {
                    prev.label.clone()
                })
            } else {
                None
            };
            let input =
                package
                    .tape()
                    .eval_input(&step.input, &seed, &current, prev_label.as_deref());

            // If this is the first step, consume the pending anchor map.
            let step_payload = if step_idx == 0 {
                package.take_anchor_map().map(TapePayload::AnchorMap)
            } else {
                None
            };

            match &step.operation {
                StepOperation::Identity => {
                    // Pass-through: output = input, no edges.
                    current = input;
                    package.tape_mut().steps.push(TapeEntry {
                        label,
                        content: TapeContent::Nodes(current.iter().copied().collect()),
                        payload: step_payload,
                    });
                }
                StepOperation::Filter(filter) => {
                    let (filtered, mut payload) = self.apply_filter(&input, filter, text_search)?;
                    current = filtered;
                    // Prefer filter payload (scores), but attach anchor map if no filter payload.
                    if payload.is_none() {
                        payload = step_payload;
                    }
                    package.tape_mut().steps.push(TapeEntry {
                        label,
                        content: TapeContent::Nodes(current.iter().copied().collect()),
                        payload,
                    });
                }
                StepOperation::Traverse(trav) => {
                    current = self.apply_traversal_to_tape(&input, trav, &label, package)?;
                }
                StepOperation::Compose(comp) => {
                    // Each branch carries its own seed in its first step.
                    // If the compose step itself has a seed, use it as the
                    // initial set; otherwise start empty.
                    let comp_seed_tapefn = &step.input;
                    let seed_set = if comp_seed_tapefn.is_seed() {
                        self.eval_seed(comp_seed_tapefn)?
                    } else {
                        BTreeSet::new()
                    };
                    let left_result = self.apply_projection_steps(
                        comp_seed_tapefn,
                        &comp.left,
                        seed_set.clone(),
                        text_search,
                    )?;
                    let right_result = self.apply_projection_steps(
                        comp_seed_tapefn,
                        &comp.right,
                        seed_set,
                        text_search,
                    )?;

                    let left_start = package.tape().len();
                    package.tape_mut().steps.push(TapeEntry {
                        label: format!("{label}.L"),
                        content: TapeContent::Nodes(left_result.iter().copied().collect()),
                        payload: None,
                    });
                    let right_start = package.tape().len();
                    package.tape_mut().steps.push(TapeEntry {
                        label: format!("{label}.R"),
                        content: TapeContent::Nodes(right_result.iter().copied().collect()),
                        payload: None,
                    });
                    let right_end = package.tape().len();

                    let intersection: Vec<Bid> =
                        left_result.intersection(&right_result).copied().collect();
                    let combined: BTreeSet<Bid> = match comp.op {
                        CompositionOp::And => intersection.iter().copied().collect(),
                        CompositionOp::Or => left_result.union(&right_result).copied().collect(),
                        CompositionOp::Not => {
                            left_result.difference(&right_result).copied().collect()
                        }
                    };
                    current = combined.clone();
                    package.tape_mut().steps.push(TapeEntry {
                        label,
                        content: TapeContent::Compose {
                            op: comp.op,
                            left: left_start..right_start,
                            right: right_start..right_end,
                            result: combined.into_iter().collect(),
                            intersection,
                        },
                        payload: None,
                    });
                }
            };
        }
        Ok(())
    }

    /// Materialize a [`BeliefGraph`] from a `Projected` package's tape.
    ///
    /// Pure tape-to-graph transformation. Uses
    /// `tape.graph_context_boundary()` to find where halo/ancestry
    /// steps begin in the tape, then distinguishes primary from Trace:
    ///
    /// - **Primary** = seed ∪ `tape.fold_bids(0..boundary)`
    /// - **All** = seed ∪ `tape.cumulative_bids()`
    /// - **Trace** = all - primary
    ///
    /// No traversals — halo/ancestry are already in the tape as
    /// `TapeFn::Fold` projection steps.
    /// Apply Trace coloring and (if needed) materialize edges into
    /// the package graph.
    ///
    /// Uses `tape.graph_context_boundary()` to distinguish primary from
    /// Trace BIDs:
    /// - **Primary** = tape entries before the boundary
    /// - **Trace** = tape entries at/after the boundary (halo, balance)
    ///
    /// When the package already has a graph (DB path, edges built during
    /// traversal), this mutates states in-place. When no graph exists
    /// (sync path), constructs a new graph from `self`.
    pub(crate) fn materialize_graph(
        &self,
        package: &mut QueryPackage,
    ) -> Result<(), BuildonomyError> {
        let boundary = package.tape().graph_context_boundary();

        // primary = union of all user-step tape entries (everything
        // before the graph context boundary). This includes all hops
        // of multi-hop traversals, not just the final frontier.
        let primary: BTreeSet<Bid> = package.tape().fold_bids(0..boundary);

        // all = seed ∪ primary ∪ graph context BIDs (halo + ancestry)
        let mut all_bids = primary.clone();
        // Include seed BIDs so edges connecting seed to first-hop results
        // are present in the package graph.
        if let Some(first_step) = package.spec().steps.first() {
            if let TapeFn::Bids(bids) = &first_step.input {
                all_bids.extend(bids.iter().copied());
            }
        }
        all_bids.extend(package.tape().fold_bids(boundary..package.tape().len()));

        if package.graph().is_some() {
            // ── Mutate existing graph: Trace-color states in-place ─────
            let states_guard = self.states();
            let graph = package.graph_mut().as_mut().unwrap();
            for &bid in &all_bids {
                if let Some(node) = states_guard.get(&bid) {
                    let mut node = node.clone();
                    if !primary.contains(&bid) {
                        node.kind.insert(BeliefKind::Trace);
                    }
                    graph.states.insert(bid, node);
                }
            }
        } else {
            // ── Build graph from tape (sync path) ─────────────────
            // Tape edge indices reference self.relations(). Copy the
            // referenced edges into a new package graph.
            let states_guard = self.states();
            let mut states: FxHashMap<Bid, BeliefNode> = FxHashMap::default();
            for &bid in &all_bids {
                if let Some(node) = states_guard.get(&bid) {
                    let mut node = node.clone();
                    if !primary.contains(&bid) {
                        node.kind.insert(BeliefKind::Trace);
                    }
                    states.insert(bid, node);
                }
            }

            let rel_guard = self.relations();
            let src_graph = rel_guard.as_graph();

            let mut graph = petgraph::stable_graph::StableGraph::<Bid, WeightSet>::new();
            let mut idx_map: BTreeMap<Bid, petgraph::graph::NodeIndex> = BTreeMap::new();

            // Ensure all result BIDs have nodes in the package graph.
            for &bid in &all_bids {
                idx_map.entry(bid).or_insert_with(|| graph.add_node(bid));
            }

            // Copy edges from the source graph where both endpoints are
            // in the result set. Build a remap from source EdgeIndex to
            // package EdgeIndex so tape entries can be rewritten.
            let bid_index = self.read_bid_index();
            let mut copied_edges: BTreeSet<EdgeIndex> = BTreeSet::new();
            let mut edge_remap: BTreeMap<EdgeIndex, EdgeIndex> = BTreeMap::new();
            for &bid in &all_bids {
                let Some(&node_idx) = bid_index.get(&bid) else {
                    continue;
                };
                for edge_ref in src_graph.edges_directed(node_idx, petgraph::Direction::Outgoing) {
                    let src_eidx = edge_ref.id();
                    if !copied_edges.insert(src_eidx) {
                        continue;
                    }
                    let target_bid = src_graph[edge_ref.target()];
                    if !all_bids.contains(&target_bid) {
                        continue;
                    }
                    let Some(weight) = src_graph.edge_weight(src_eidx) else {
                        continue;
                    };
                    let pkg_src = *idx_map.entry(bid).or_insert_with(|| graph.add_node(bid));
                    let pkg_snk = *idx_map
                        .entry(target_bid)
                        .or_insert_with(|| graph.add_node(target_bid));
                    let pkg_eidx = graph.add_edge(pkg_src, pkg_snk, weight.clone());
                    edge_remap.insert(src_eidx, pkg_eidx);
                }
            }

            drop(rel_guard);

            package.set_graph(BeliefGraph {
                states,
                relations: crate::beliefbase::BidGraph(graph),
            });

            // Rewrite tape edge indices to reference the package graph.
            for entry in &mut package.tape_mut().steps {
                if let TapeContent::Edges { edges, .. } = &mut entry.content {
                    for eidx in edges.iter_mut() {
                        if let Some(&new_idx) = edge_remap.get(eidx) {
                            *eidx = new_idx;
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Resolve a single [`NodeKey`] to its BID, if present in this BeliefBase.
    fn resolve_key_to_bid(&self, key: &NodeKey) -> Option<Bid> {
        match key {
            NodeKey::Bid { bid } => {
                if self.states().contains_key(bid) {
                    Some(*bid)
                } else {
                    None
                }
            }
            NodeKey::Bref { bref } => self
                .brefs()
                .get(bref)
                .filter(|bid| self.states().contains_key(bid))
                .copied(),
            NodeKey::Path { net, path } => self
                .paths()
                .net_get_from_path(net, path)
                .map(|(_, bid)| bid)
                .filter(|bid| self.states().contains_key(bid)),
            NodeKey::Id { net, id } => self
                .paths()
                .net_get_from_id(net, id)
                .map(|(_, bid)| bid)
                .filter(|bid| self.states().contains_key(bid)),
        }
    }

    /// Resolve a seed [`TapeFn`] to its initial BID set.
    fn eval_seed(&self, seed: &TapeFn) -> Result<BTreeSet<Bid>, BuildonomyError> {
        match seed {
            TapeFn::Bids(bids) => Ok(bids
                .iter()
                .filter(|bid| self.states().contains_key(bid))
                .copied()
                .collect()),
            TapeFn::Keys(keys) => {
                let mut set = BTreeSet::new();
                for key in keys {
                    match key {
                        NodeKey::Bid { bid } => {
                            if self.states().contains_key(bid) {
                                set.insert(*bid);
                            }
                        }
                        NodeKey::Bref { bref } => {
                            if let Some(&bid) = self.brefs().get(bref) {
                                if self.states().contains_key(&bid) {
                                    set.insert(bid);
                                }
                            }
                        }
                        NodeKey::Path { net, path } => {
                            if let Some((_, bid)) = self.paths().net_get_from_path(net, path) {
                                if self.states().contains_key(&bid) {
                                    set.insert(bid);
                                }
                            }
                        }
                        NodeKey::Id { net, id } => {
                            if let Some((_, bid)) = self.paths().net_get_from_id(net, id) {
                                if self.states().contains_key(&bid) {
                                    set.insert(bid);
                                }
                            }
                        }
                    }
                }
                Ok(set)
            }
            TapeFn::Corpus => Ok(self
                .states()
                .iter()
                .filter(|(_, node)| !node.kind.0.is_empty())
                .map(|(bid, _)| *bid)
                .collect()),
            TapeFn::DocumentNodes(net, doc_path) => {
                let paths_guard = self.paths();
                let prefix = format!("{}#", doc_path);
                let bids: BTreeSet<Bid> = paths_guard
                    .get_map(net)
                    .map(|pm| {
                        pm.map()
                            .iter()
                            .filter_map(|(path, bid, _order)| {
                                if path == doc_path || path.starts_with(&prefix) {
                                    Some(*bid)
                                } else {
                                    None
                                }
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                Ok(bids
                    .into_iter()
                    .filter(|bid| self.states().contains_key(bid))
                    .collect())
            }
            TapeFn::Then(None) => Err(BuildonomyError::Command(
                "Context-dependent seed must be resolved before evaluation".to_string(),
            )),
            _ => Err(BuildonomyError::Command(format!(
                "TapeFn variant {:?} cannot be used as a seed",
                seed
            ))),
        }
    }

    /// Apply a sequence of projection steps, transforming the BID set at each step.
    ///
    /// Used by `apply_composition` for evaluating Compose sub-branches.
    /// The canonical package-level evaluation path is
    /// `apply_projection_steps_to_package`, which records a full Tape.
    fn apply_projection_steps(
        &self,
        seed_tapefn: &TapeFn,
        steps: &[ProjectionStep],
        mut current: BTreeSet<Bid>,
        text_search: Option<&dyn TextSearchProvider>,
    ) -> Result<BTreeSet<Bid>, BuildonomyError> {
        for step in steps {
            // If the step has its own seed (Keys, Bids, Corpus), evaluate it
            // to produce the starting set instead of chaining from `current`.
            // This is essential for Compose branches where each side has an
            // independently-seeded pipeline.
            if step.input.is_seed() {
                current = self.eval_seed(&step.input)?;
            }
            current = match &step.operation {
                StepOperation::Identity => current,
                StepOperation::Filter(filter) => {
                    self.apply_filter(&current, filter, text_search)?.0
                }
                StepOperation::Traverse(trav) => self.apply_traversal(&current, trav)?,
                StepOperation::Compose(comp) => {
                    self.apply_composition(seed_tapefn, &current, comp, text_search)?
                }
            };
        }
        Ok(current)
    }

    /// Apply a [`NodeFilter`] to retain only matching BIDs.
    ///
    /// Returns the filtered BID set and an optional [`TapePayload`] carrying
    /// per-BID scores (for `TextMatch` filters). The caller is responsible
    /// for attaching the payload to the corresponding [`TapeEntry`].
    ///
    /// When a [`TextSearchProvider`] is supplied, `TextMatch` filters are
    /// evaluated by delegating to the provider's search index and intersecting
    /// results with the current BID set. Without a provider, `TextMatch`
    /// returns an error.
    pub(crate) fn apply_filter(
        &self,
        current: &BTreeSet<Bid>,
        filter: &NodeFilter,
        text_search: Option<&dyn TextSearchProvider>,
    ) -> Result<(BTreeSet<Bid>, Option<TapePayload>), BuildonomyError> {
        match filter {
            NodeFilter::Predicate(pred) => Ok((
                current
                    .iter()
                    .filter(|bid| {
                        self.states()
                            .get(bid)
                            .map(|node| pred.evaluate(node).matched)
                            .unwrap_or(false)
                    })
                    .copied()
                    .collect(),
                None,
            )),
            NodeFilter::TextMatch { path: _, query } => {
                let Some(provider) = text_search else {
                    return Err(BuildonomyError::Command(format!(
                        "TextMatch('{query}') requires a TextSearchProvider; \
                         not available in this evaluation context"
                    )));
                };
                let results = provider.text_search(query);
                let scored: std::collections::BTreeMap<Bid, f64> = results.into_iter().collect();
                // Intersect with current pipeline set, preserving scores.
                let matched: Vec<Bid> = current
                    .iter()
                    .filter(|bid| scored.contains_key(bid))
                    .copied()
                    .collect();
                let scores: Vec<SortPayload> = matched
                    .iter()
                    .map(|bid| SortPayload {
                        score: scored.get(bid).map(|s| *s as f32),
                    })
                    .collect();
                let bid_set: BTreeSet<Bid> = matched.into_iter().collect();
                Ok((bid_set, Some(TapePayload::Scores(scores))))
            }
        }
    }

    /// Apply a [`TraversalSpec`] to walk edges from the current BID set.
    ///
    /// Returns only the **discovered** BIDs — the resolved endpoints from
    /// matched edges. The input set is used for cycle prevention but is NOT
    /// included in the output. This matches the query model spec (§5.2):
    /// "a Traversal maps the current node set to a new node set."
    fn apply_traversal(
        &self,
        current: &BTreeSet<Bid>,
        trav: &TraversalSpec,
    ) -> Result<BTreeSet<Bid>, BuildonomyError> {
        // Inverted traversal: return input nodes that produce NO output.
        if trav.inverted {
            let non_inverted = TraversalSpec {
                inverted: false,
                ..trav.clone()
            };
            let mut orphans = BTreeSet::new();
            for &bid in current {
                let single = BTreeSet::from([bid]);
                let output = self.apply_traversal(&single, &non_inverted)?;
                if output.is_empty() {
                    orphans.insert(bid);
                }
            }
            return Ok(orphans);
        }

        let mut result: BTreeSet<Bid> = BTreeSet::new();
        // Filter const namespace BIDs (href, asset, buildonomy, codec) from
        // the traversal frontier.  These are hub nodes whose halo fans out
        // to every document in the corpus.  No traversal ever needs to walk
        // FROM a const namespace BID — their children are looked up
        // individually by key, never enumerated via graph traversal.
        let const_ns_brefs: BTreeSet<Bref> =
            const_namespaces().iter().map(|bid| bid.bref()).collect();
        let mut frontier: BTreeSet<Bid> = current
            .iter()
            .filter(|bid| !const_ns_brefs.contains(&bid.bref()))
            .copied()
            .collect();
        let mut visited = current.clone();

        let rel_guard = self.relations();
        let graph = rel_guard.as_graph();

        let has_owner_input = trav.input_roles.contains(Role::Owner);
        let edge_filter = trav.depth.edge_filter.as_ref();

        for _depth in 0..trav.depth.max_hops() {
            let mut next_frontier: BTreeSet<Bid> = BTreeSet::new();

            // ── Source/Sink input roles: per-node directed edge iteration ──
            for &bid in &frontier {
                let Some(&idx) = self.read_bid_index().get(&bid) else {
                    continue;
                };

                if trav.input_roles.contains(Role::Source) {
                    // This node is source → outgoing edges → target is sink
                    for edge_ref in graph.edges_directed(idx, petgraph::Direction::Outgoing) {
                        let ws = edge_ref.weight();
                        if !edge_matches_with_filter(ws, &trav.kind_filter, edge_filter) {
                            continue;
                        }
                        self.collect_output_bids(
                            graph,
                            &edge_ref,
                            &trav.output_roles,
                            &mut next_frontier,
                        );
                    }
                }

                if trav.input_roles.contains(Role::Sink) {
                    // This node is sink → incoming edges → source is the other end
                    for edge_ref in graph.edges_directed(idx, petgraph::Direction::Incoming) {
                        let ws = edge_ref.weight();
                        if !edge_matches_with_filter(ws, &trav.kind_filter, edge_filter) {
                            continue;
                        }
                        self.collect_output_bids(
                            graph,
                            &edge_ref,
                            &trav.output_roles,
                            &mut next_frontier,
                        );
                    }
                }
            }

            // ── Owner input role: use owner_edges memo for O(1) lookup ─────
            if has_owner_input {
                for &bid in &frontier {
                    let bref = bid.bref();
                    if let Some(edge_indices) = self.owner_edges.get(&bref) {
                        for &edge_idx in edge_indices {
                            let Some(ws) = graph.edge_weight(edge_idx) else {
                                continue;
                            };
                            if !edge_matches_with_filter(ws, &trav.kind_filter, edge_filter) {
                                continue;
                            }
                            let Some((src_idx, snk_idx)) = graph.edge_endpoints(edge_idx) else {
                                continue;
                            };
                            if trav.output_roles.contains(Role::Source) {
                                next_frontier.insert(graph[src_idx]);
                            }
                            if trav.output_roles.contains(Role::Sink) {
                                next_frontier.insert(graph[snk_idx]);
                            }
                        }
                    }
                }
            }

            // Remove already-visited BIDs from next frontier.
            next_frontier.retain(|bid| !visited.contains(bid));

            if next_frontier.is_empty() {
                break;
            }

            result.extend(next_frontier.iter());
            visited.extend(next_frontier.iter());
            frontier = next_frontier;
        }

        Ok(result)
    }

    /// Apply a traversal with per-hop tape recording. Each hop pushes a
    /// [`TapeEntry`] with [`TapeContent::Edges`] containing the edge indices
    /// discovered at that depth. Returns the combined output BIDs.
    ///
    /// This is the tape-recording counterpart of [`Self::apply_traversal`].
    /// The non-recording variant is still used by composition branch
    /// evaluation via [`Self::apply_projection_steps`].
    fn apply_traversal_to_tape(
        &self,
        current: &BTreeSet<Bid>,
        trav: &TraversalSpec,
        label: &str,
        package: &mut QueryPackage,
    ) -> Result<BTreeSet<Bid>, BuildonomyError> {
        // Inverted traversal: return input nodes that produce NO output.
        // Record a single Nodes tape entry (no per-hop edges).
        if trav.inverted {
            let non_inverted = TraversalSpec {
                inverted: false,
                ..trav.clone()
            };
            let mut orphans = BTreeSet::new();
            for &bid in current {
                let single = BTreeSet::from([bid]);
                let output = self.apply_traversal(&single, &non_inverted)?;
                if output.is_empty() {
                    orphans.insert(bid);
                }
            }
            package
                .tape_mut()
                .steps
                .push(crate::query::spec::TapeEntry {
                    label: label.to_string(),
                    content: crate::query::spec::TapeContent::Nodes(
                        orphans.iter().copied().collect(),
                    ),
                    payload: None,
                });
            return Ok(orphans);
        }

        let mut result: BTreeSet<Bid> = BTreeSet::new();
        let mut frontier = current.clone();
        let mut visited = current.clone();
        let tape_start = package.tape().len();

        let rel_guard = self.relations();
        let graph = rel_guard.as_graph();

        let has_owner_input = trav.input_roles.contains(Role::Owner);
        let edge_filter = trav.depth.edge_filter.as_ref();

        for _depth in 0..trav.depth.max_hops() {
            let mut next_frontier = BTreeSet::new();
            let mut hop_edges: Vec<EdgeIndex> = Vec::new();

            // ── Source/Sink input roles: per-node directed edge iteration ──
            for &bid in &frontier {
                let Some(&idx) = self.read_bid_index().get(&bid) else {
                    continue;
                };

                if trav.input_roles.contains(Role::Source) {
                    for edge_ref in graph.edges_directed(idx, petgraph::Direction::Outgoing) {
                        let ws = edge_ref.weight();
                        if !edge_matches_with_filter(ws, &trav.kind_filter, edge_filter) {
                            continue;
                        }
                        hop_edges.push(edge_ref.id());
                        self.collect_output_bids(
                            graph,
                            &edge_ref,
                            &trav.output_roles,
                            &mut next_frontier,
                        );
                    }
                }

                if trav.input_roles.contains(Role::Sink) {
                    for edge_ref in graph.edges_directed(idx, petgraph::Direction::Incoming) {
                        let ws = edge_ref.weight();
                        if !edge_matches_with_filter(ws, &trav.kind_filter, edge_filter) {
                            continue;
                        }
                        hop_edges.push(edge_ref.id());
                        self.collect_output_bids(
                            graph,
                            &edge_ref,
                            &trav.output_roles,
                            &mut next_frontier,
                        );
                    }
                }
            }

            // ── Owner input role: use owner_edges memo for O(1) lookup ─────
            if has_owner_input {
                for &bid in &frontier {
                    let bref = bid.bref();
                    if let Some(edge_indices) = self.owner_edges.get(&bref) {
                        for &edge_idx in edge_indices {
                            let Some(ws) = graph.edge_weight(edge_idx) else {
                                continue;
                            };
                            if !edge_matches_with_filter(ws, &trav.kind_filter, edge_filter) {
                                continue;
                            }
                            hop_edges.push(edge_idx);
                            let Some((src_idx, snk_idx)) = graph.edge_endpoints(edge_idx) else {
                                continue;
                            };
                            if trav.output_roles.contains(Role::Source) {
                                next_frontier.insert(graph[src_idx]);
                            }
                            if trav.output_roles.contains(Role::Sink) {
                                next_frontier.insert(graph[snk_idx]);
                            }
                        }
                    }
                }
            }

            // Remove already-visited BIDs from next frontier.
            next_frontier.retain(|bid| !visited.contains(bid));

            // Record this hop in the tape, sorted by WEIGHT_SORT_KEY.
            if !hop_edges.is_empty() {
                // Sort edges by (sort_key, source_bid, sink_bid) so tape entries
                // reflect structural document order rather than petgraph insertion order.
                hop_edges.sort_by(|a, b| {
                    let key_of = |eidx: &EdgeIndex| -> (u16, Bid, Bid) {
                        let sort_key = graph
                            .edge_weight(*eidx)
                            .map(|ws| ws.sort_key(&trav.kind_filter))
                            .unwrap_or(u16::MAX);
                        let (src, snk) = graph
                            .edge_endpoints(*eidx)
                            .map(|(s, k)| (graph[s], graph[k]))
                            .unwrap_or((Bid::nil(), Bid::nil()));
                        (sort_key, src, snk)
                    };
                    key_of(a).cmp(&key_of(b))
                });

                // Derive output BIDs from the sorted edge order so they
                // inherit structural ordering.
                let mut ordered_output = Vec::new();
                let mut seen_output = std::collections::HashSet::new();
                for &eidx in &hop_edges {
                    let Some((src_idx, snk_idx)) = graph.edge_endpoints(eidx) else {
                        continue;
                    };
                    if trav.output_roles.contains(Role::Source) {
                        let bid = graph[src_idx];
                        if next_frontier.contains(&bid) && seen_output.insert(bid) {
                            ordered_output.push(bid);
                        }
                    }
                    if trav.output_roles.contains(Role::Sink) {
                        let bid = graph[snk_idx];
                        if next_frontier.contains(&bid) && seen_output.insert(bid) {
                            ordered_output.push(bid);
                        }
                    }
                    if trav.output_roles.contains(Role::Owner) {
                        if let Some(ws) = graph.edge_weight(eidx) {
                            let brefs = self.brefs();
                            for weight in ws.weights.values() {
                                if let Some(owner_str) = weight.get::<String>(WEIGHT_OWNED_BY) {
                                    if let Ok(bref) = Bref::try_from(owner_str.as_str()) {
                                        if let Some(&bid) = brefs.get(&bref) {
                                            if next_frontier.contains(&bid)
                                                && seen_output.insert(bid)
                                            {
                                                ordered_output.push(bid);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                package.tape_mut().steps.push(TapeEntry {
                    label: label.to_string(),
                    content: TapeContent::Edges {
                        edges: hop_edges,
                        output_bids: ordered_output,
                    },
                    payload: None,
                });
            }

            if next_frontier.is_empty() {
                break;
            }

            result.extend(next_frontier.iter());
            visited.extend(next_frontier.iter());
            frontier = next_frontier;
        }

        // Ensure at least one tape entry per step so stage() can detect completion.
        if package.tape().len() == tape_start {
            tracing::trace!(
                label = label,
                input_count = current.len(),
                "traversal produced no edges — pushing empty tape entry"
            );
            package.tape_mut().steps.push(TapeEntry {
                label: label.to_string(),
                content: TapeContent::Edges {
                    edges: vec![],
                    output_bids: vec![],
                },
                payload: None,
            });
        }

        Ok(result)
    }

    /// Collect output BIDs from an edge based on the requested output roles.
    ///
    /// For `Source`/`Sink`: the structural endpoints of the edge.
    /// For `Owner`: resolves each weight's `WEIGHT_OWNED_BY` bref to a BID
    /// via the bref index.
    fn collect_output_bids(
        &self,
        graph: &petgraph::stable_graph::StableGraph<Bid, WeightSet>,
        edge_ref: &petgraph::stable_graph::EdgeReference<'_, WeightSet>,
        output_roles: &EnumSet<Role>,
        result: &mut BTreeSet<Bid>,
    ) {
        if output_roles.contains(Role::Source) {
            result.insert(graph[edge_ref.source()]);
        }
        if output_roles.contains(Role::Sink) {
            result.insert(graph[edge_ref.target()]);
        }
        if output_roles.contains(Role::Owner) {
            let brefs = self.brefs();
            for weight in edge_ref.weight().weights.values() {
                if let Some(owner_str) = weight.get::<String>(WEIGHT_OWNED_BY) {
                    if let Ok(bref) = Bref::try_from(owner_str.as_str()) {
                        if let Some(&bid) = brefs.get(&bref) {
                            result.insert(bid);
                        }
                    }
                }
            }
        }
    }

    /// Apply a [`Composition`] by evaluating left and right branches and
    /// combining them with the specified set operation.
    ///
    /// Each branch is an independently-seeded pipeline: the first step of
    /// each branch carries its own seed (Keys/Bids/Corpus) via
    /// `inject_seed_into_branch`. If the parent compose step has a seed,
    /// it is used as the initial set; otherwise branches start empty and
    /// rely on their own first-step seeds.
    fn apply_composition(
        &self,
        seed_tapefn: &TapeFn,
        _current: &BTreeSet<Bid>,
        comp: &Composition,
        text_search: Option<&dyn TextSearchProvider>,
    ) -> Result<BTreeSet<Bid>, BuildonomyError> {
        // If the compose step itself has a seed, evaluate it as the initial
        // set. Otherwise start empty — each branch's first step provides
        // its own seed via the is_seed() check in apply_projection_steps.
        let seed = if seed_tapefn.is_seed() {
            self.eval_seed(seed_tapefn)?
        } else {
            BTreeSet::new()
        };
        let left =
            self.apply_projection_steps(seed_tapefn, &comp.left, seed.clone(), text_search)?;
        let right = self.apply_projection_steps(seed_tapefn, &comp.right, seed, text_search)?;

        let result = match comp.op {
            CompositionOp::And => left.intersection(&right).copied().collect(),
            CompositionOp::Or => left.union(&right).copied().collect(),
            CompositionOp::Not => left.difference(&right).copied().collect(),
        };
        Ok(result)
    }
}

/// Check whether a [`WeightSet`] matches the kind filter AND the optional edge predicate.
///
/// An edge matches if at least one of its `(kind, weight)` entries satisfies:
/// 1. The kind is in the `kind_filter`
/// 2. The optional `edge_filter` predicate matches the weight's payload
fn edge_matches_with_filter(
    ws: &WeightSet,
    kind_filter: &EnumSet<WeightKind>,
    edge_filter: Option<&EdgePredicate>,
) -> bool {
    ws.weights.iter().any(|(kind, weight)| {
        if !kind_filter.contains(*kind) {
            return false;
        }
        match edge_filter {
            None => true,
            Some(pred) => pred.matches_weight(weight),
        }
    })
}

#[cfg(not(target_arch = "wasm32"))]
impl BeliefSource for BeliefBase {
    /// Direct in-memory evaluation via the QueryPackage lifecycle.
    fn evaluate<'a>(
        &'a self,
        package: &'a mut QueryPackage,
    ) -> BoxFuture<'a, Result<(), BuildonomyError>> {
        Box::pin(async move { self.evaluate_query(package) })
    }

    /// Get all paths for a network as (path, target_bid) pairs.
    /// Useful for querying asset manifests or all documents in a network.
    /// Default implementation returns empty (in-memory BeliefBase doesn't cache paths).
    fn submap<'a>(
        &'a self,
        network_bid: Bid,
        path: &'a str,
        depth: u8,
        include_index: bool,
    ) -> BoxFuture<'a, SubmapResult> {
        Box::pin(async move {
            Ok(self
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
            Ok(self
                .paths()
                .submap_by_bid(&network_bid.bref(), entry, depth, include_index))
        })
    }

    /// Export the full graph without halo expansion or Trace coloring.
    ///
    /// Uses `QueryPackage::new` (not `balanced`) with all BIDs as a seed
    /// and an Identity operation. This produces the raw node+edge set —
    /// equivalent to `SELECT * FROM beliefs; SELECT * FROM relations`
    /// on the DB path.
    ///
    /// Avoids the `consume()` spin-wait that would block a tokio
    /// `current_thread` executor (e.g. the MCP server).
    fn export_beliefgraph(&self) -> BoxFuture<'_, Result<BeliefGraph, BuildonomyError>> {
        Box::pin(async move {
            let all_bids: Vec<Bid> = self.states().keys().copied().collect();
            let mut package = QueryPackage::new(QuerySpec::seed(TapeFn::Bids(all_bids)));
            self.evaluate_query(&mut package)?;
            Ok(package.into_graph())
        })
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl BeliefSource for &BeliefBase {
    fn evaluate<'a>(
        &'a self,
        package: &'a mut QueryPackage,
    ) -> BoxFuture<'a, Result<(), BuildonomyError>> {
        Box::pin(async move { self.evaluate_query(package) })
    }

    fn submap<'a>(
        &'a self,
        network_bid: Bid,
        path: &'a str,
        depth: u8,
        include_index: bool,
    ) -> BoxFuture<'a, SubmapResult> {
        Box::pin(async move {
            Ok(self
                .paths()
                .submap(&network_bid.bref(), path, depth, include_index)
                .into_iter()
                .filter(|(p, _bid, _order)| !p.is_empty())
                .collect())
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
            Ok(self
                .paths()
                .submap_by_bid(&network_bid.bref(), entry, depth, include_index)
                .into_iter()
                .filter(|(p, _bid, _order)| !p.is_empty())
                .collect())
        })
    }

    /// Export the full graph without halo expansion or Trace coloring.
    /// See [`BeliefSource::export_beliefgraph`] on `BeliefBase` for rationale.
    fn export_beliefgraph(&self) -> BoxFuture<'_, Result<BeliefGraph, BuildonomyError>> {
        Box::pin(async move {
            let all_bids: Vec<Bid> = self.states().keys().copied().collect();
            let mut package = QueryPackage::new(QuerySpec::seed(TapeFn::Bids(all_bids)));
            self.evaluate_query(&mut package)?;
            Ok(package.into_graph())
        })
    }
}
