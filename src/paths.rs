/// Defines [PathMapMap], and [PathMap], who's primary job is to generate and
/// maintain relative paths between [BeliefNodes] within a [BeliefSet], even
/// when the relations within that set are changing.
use parking_lot::{ArcRwLockReadGuard, RawRwLock, RwLock};
use petgraph::visit::{depth_first_search, Control, DfsEvent};
use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
    fmt,
    path::Path,
    sync::Arc,
};
use url::Url;

use crate::{
    beliefset::BidGraph,
    error::BuildonomyError,
    event::{BeliefEvent, EventOrigin},
    nodekey::{get_doc_path, to_anchor, trim_doc_path, trim_joiners, trim_path_sep, TRIM},
    properties::{
        BeliefKind, BeliefNode, Bid, WeightKind, WeightSet, WEIGHT_DOC_PATH, WEIGHT_SORT_KEY,
    },
    query::WrappedRegex,
};

/// `core::paths::PathMap::subtree` returns a `Vec<BeliefRow>`. `BeliefRow` contains the information
/// necessary to render a belief node within the graph structure of a particular
/// `BeliefSet::relations`` a hierarchical structure.
#[derive(Clone, Debug, PartialEq)]
pub struct BeliefRow {
    pub sink: Bid,
    pub net: Bid,
    pub path: String,
    pub bid: Bid,
    pub order: Vec<u16>,
    pub has_branches: bool,
}

impl BeliefRow {
    /// Create an absolute url to the BeliefRow::bid entity.
    pub fn href(&self, origin: String) -> Result<String, BuildonomyError> {
        let origin = Url::parse(&origin)?;
        Ok(origin.join(&self.path)?.as_str().to_string())
    }
}

/// [IdMap] tracks the mapping between semantic IDs (from TOML schema) and BIDs.
/// IDs provide globally unique references like "asp_sarah_embodiment_rest".
#[derive(Clone, Debug, Default)]
pub struct IdMap {
    id_to_bid: BTreeMap<String, Bid>,
    bid_to_id: BTreeMap<Bid, String>,
}

impl IdMap {
    /// Insert or update an ID mapping
    pub fn insert(&mut self, id: String, bid: Bid) {
        // Remove old mapping if bid already had a different id
        if let Some(old_id) = self.bid_to_id.get(&bid) {
            if old_id != &id {
                self.id_to_bid.remove(old_id);
            }
        }
        // Remove old mapping if id was associated with a different bid
        if let Some(old_bid) = self.id_to_bid.get(&id) {
            if old_bid != &bid {
                self.bid_to_id.remove(old_bid);
            }
        }
        self.id_to_bid.insert(id.clone(), bid);
        self.bid_to_id.insert(bid, id);
    }

    /// Get the Bid associated with an ID
    pub fn get_bid(&self, id: &str) -> Option<&Bid> {
        self.id_to_bid.get(id)
    }

    /// Get the Bid associated with an ID
    pub fn get_bid_from_regex(&self, re: &WrappedRegex) -> Option<&Bid> {
        self.id_to_bid
            .iter()
            .find(|(id, _bid)| re.is_match(id))
            .map(|(_id, bid)| bid)
    }

    /// Get the ID associated with a Bid
    pub fn get_id(&self, bid: &Bid) -> Option<&String> {
        self.bid_to_id.get(bid)
    }

    /// Remove a mapping by Bid
    pub fn remove(&mut self, bid: &Bid) -> Option<String> {
        if let Some(id) = self.bid_to_id.remove(bid) {
            self.id_to_bid.remove(&id);
            Some(id)
        } else {
            None
        }
    }
}

/// [PathMap] generates unique relative paths between [crate::properties::BeliefNode]s based on the
/// graph structure for a particular [crate::properties::WeightKind] within a
/// [crate::beliefset::BeliefSet::relations] hypergraph.
///
/// Since [crate::beliefset::BeliefSet::relations] storeas a [crate::beliefset::BidGraph]
/// hypergraph, there are multiple possible relational path structures within the object. A PathMap
/// generates a [crate::properties::WeightKind]-specific tree structure from the BidGraph, and
/// assigns each node within that tree a unique path. This helps source documents reference node
/// relationships using relative links.
///
/// PathMap maintains the order of paths based on relationship weights and handles connections to
/// sub-networks, which are themselves represented by other `PathMap` instances.
///
/// Each `PathMap` is built around a specific "net" [crate::properties::Bid], which acts as the root
/// or entry point for the paths contained within this map. The `kind` field determines which type
/// of relationship weights (e.g., Subsection, Epistemic) are used to construct the hierarchy.
///
/// The `map` field stores the primary path information: a vector of tuples, where each tuple
/// contains a [String] (the path), a [crate::properties::Bid] (the belief node at that path), and a
/// `Vec<u16>` representing the order of the node within the hierarchy.
///
/// `subnets` is a [BTreeMap] that links paths within this `PathMap` to the
/// [crate::properties::Bid]s of other networks, allowing for navigation across different network
/// segments.
///
/// `loops` keeps track of detected cycles in the underlying belief graph to prevent infinite
/// recursion during path generation.
///
/// `PathMap` is primarily used by [PathMapMap] to manage and query the overall path structure of
/// all known belief networks. It plays a crucial role in generating `BeliefRow`s for UI rendering
/// and in propagating structural changes through `BeliefEvent`s.
#[derive(Debug, Clone)]
pub struct PathMap {
    // usize is the order for path, such that when map.keys() is order by usize the map is
    // ordered by relation weight.
    map: Vec<(String, Bid, Vec<u16>)>,
    bid_map: BTreeMap<Bid, Vec<usize>>,
    path_map: BTreeMap<String, usize>,
    id_map: IdMap,
    title_map: IdMap,
    kind: WeightKind,
    net: Bid,
    subnets: BTreeSet<Bid>,
    pub loops: BTreeSet<(Bid, Bid)>,
}

/// [PathMapMap] serves as a central manager for all [PathMap] instances for a specific
/// [crate::properties::WeightKind] within a [crate::beliefset::BeliefSet].
///
/// It orchestrates the creation, storage, and updating of [PathMap]s, each corresponding to a
/// distinct sub-network instantiated within the BeliefSet. Each
/// [crate::properties::BeliefKind::Network] is similar to a separate hard drive, so PathMapMap is
/// responsible for generating a 'Logical Drive' based off how each one of these networks is mounted
/// to each other.
///
/// **Core Responsibilities:**
///
/// 1.  **Network Aggregation:** It holds a map (`map`) where keys are network
///     identifier `Bid`s and values are `Arc<RwLock<PathMap>>` instances.
///     This allows for concurrent access and modification of individual network
///     path structures.
///
/// 2. **Path Resolution:** Provides methods to query paths for specific [Bid]s across all managed
///    networks ([Self::path], [Self::get]) or within a particular network ([Self::net_path],
///    [Self::net_get_from_path], [Self::net_get_from_id], [Self::net_get_from_title]). It handles path
///    resolution that might span across sub-networks.
///
/// 3.  **Hierarchy Management:** It uses a `BidGraph` (`relations`) to
///     understand the relationships between [BeliefNode]s. This graph is the
///     basis for constructing the hierarchical paths within each [PathMap].
///
/// 4.  **Root and Network Identification:** It maintains a `root` `Bid` (typically
///     an API state node) and sets of `nets` (all network root `Bid`s) and
///     `docs` (`Bid`s of document nodes). This helps in initializing `PathMap`s
///     and in special path handling for documents (e.g., using `#` for document
///     fragments).
///
/// **Usage:**
///
/// `PathMapMap` is crucial for applications that need to:
/// *   Render hierarchical views of [crate::beliefset::BeliefSet]s.
/// *   Generate stable, relative URLs or paths for [crate::properties::BeliefNode]s.
/// *   Track how entities are interconnected across different, potentially nested,
///     networks.
///
/// It acts as the primary interface for querying and maintaining the overall
/// navigable structure of a [crate::beliefset::BeliefSet].
#[derive(Debug, Clone)]
pub struct PathMapMap {
    map: BTreeMap<Bid, Arc<RwLock<PathMap>>>,
    root: Bid,
    nets: BTreeSet<Bid>,
    docs: BTreeSet<Bid>,
    apis: BTreeSet<Bid>,
    anchors: BTreeMap<Bid, String>,
    ids: BTreeMap<Bid, String>,
    relations: Arc<RwLock<BidGraph>>,
}

impl Default for PathMapMap {
    #[tracing::instrument]
    fn default() -> PathMapMap {
        let mut nets = BTreeSet::new();
        let root = BeliefNode::api_state().bid;
        nets.insert(root);
        let map = BTreeMap::new();
        let relations = Arc::new(RwLock::new(BidGraph::default()));
        let mut pmm = PathMapMap {
            map,
            nets,
            root,
            docs: BTreeSet::default(),
            apis: BTreeSet::default(),
            anchors: BTreeMap::default(),
            ids: BTreeMap::default(),
            relations: relations.clone(),
        };
        let api_pm = PathMap::new(WeightKind::Section, root, &pmm, relations);
        pmm.map.insert(root, Arc::new(RwLock::new(api_pm)));
        pmm
    }
}

impl fmt::Display for PathMapMap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "nets:\n\
            {}\n\
            api_net anchored paths:\n - {}",
            self.map
                .values()
                .map(|pm_arc| {
                    let pm = pm_arc.read();
                    format!(
                        "{}: subs: {}",
                        pm.net,
                        pm.subnets()
                            .iter()
                            .map(|b| b.to_string())
                            .collect::<Vec<String>>()
                            .join(", ")
                    )
                })
                .collect::<Vec<String>>()
                .join("\n"),
            self.api_map()
                .all_paths(self, &mut BTreeSet::default())
                .join("\n - "),
        )
    }
}

// 'doc' in this case means path or doc, namely anything with a real file path.
#[tracing::instrument()]
pub fn path_join(base: &str, end: &str, end_is_anchor: bool) -> String {
    // Handle empty end string - just return base
    if end.is_empty() || trim_joiners(end).is_empty() {
        return trim_joiners(base).to_string();
    }

    if end_is_anchor {
        format!("{}#{}", get_doc_path(base), end)
    } else {
        let path_base = trim_doc_path(base);
        if path_base.is_empty() {
            trim_joiners(end).to_string()
        } else {
            format!("{}/{}", trim_joiners(path_base), trim_joiners(end))
        }
    }
}

/// Calculates the path relative to `base`.
///
/// If `base_is_doc` is true, it treats `base` as a document path and expects `full` to start
/// with `base` followed by `#`, returning the part after the `#`.
///
/// If `base_is_doc` is false, it expects `full` to start with `base` and returns the
/// remaining suffix.
///
/// Returns an empty string if `full` and `base` are identical.
///
/// # Arguments
///
/// *   `full`: The full path.
/// *   `base`: The base path to calculate the relative path from.
/// *   `base_is_doc`: A boolean indicating whether `base` represents a document node,
///     which uses `#` for fragments.
///
/// # Returns
///
/// A `Result` containing the relative `path` on success, or a `BuildonomyError`
/// if `full` is not relative to `base` according to the rules specified.
pub fn relative_path(full_ref: &str, base_ref: &str) -> Result<String, BuildonomyError> {
    let err = BuildonomyError::Serialization(format!(
        "Path {full_ref:?} is not relative to {base_ref:?}"
    ));
    let mut full = full_ref;
    while full.starts_with('/') {
        full = &full[1..];
    }
    let mut base = base_ref;
    while base.starts_with('/') {
        base = &base[1..];
    }
    if full.starts_with(base) || base.is_empty() {
        if base.ends_with(TRIM) || base.is_empty() {
            Ok(full[base.len()..].to_string())
        } else if full[base.len()..].starts_with(TRIM) {
            Ok(full[base.len() + 1..].to_string())
        } else if full[base.len()..].is_empty() {
            Ok("".to_string())
        } else {
            Err(err)
        }
    } else {
        Err(err)
    }
}

impl PathMapMap {
    #[tracing::instrument(skip(states, relations))]
    pub fn new(states: &BTreeMap<Bid, BeliefNode>, relations: Arc<RwLock<BidGraph>>) -> PathMapMap {
        let mut pmm = PathMapMap {
            relations: relations.clone(),
            ..Default::default()
        };
        for node in states.values() {
            pmm.anchors.insert(node.bid, to_anchor(&node.title));
            if let Some(id) = node.id.as_ref() {
                pmm.ids.insert(node.bid, id.clone());
            }
            if node.kind.contains(BeliefKind::API) {
                pmm.apis.insert(node.bid);
            }

            if node.kind.is_network() {
                pmm.nets.insert(node.bid);
            }

            if node.kind.is_document() {
                pmm.docs.insert(node.bid);
            }
        }
        // Ensure the api net is always present
        pmm.nets.insert(pmm.api());

        pmm.map.clear();
        for net in pmm.nets.iter() {
            if !pmm.map.contains_key(net) {
                let pm = PathMap::new(WeightKind::Section, *net, &pmm, relations.clone());
                pmm.map.insert(*net, Arc::new(RwLock::new(pm)));
            }
        }
        pmm
    }

    pub fn map(&self) -> &BTreeMap<Bid, Arc<RwLock<PathMap>>> {
        &self.map
    }

    pub fn relations(&self) -> ArcRwLockReadGuard<RawRwLock, BidGraph> {
        self.relations.read_arc()
    }

    pub fn nets(&self) -> &BTreeSet<Bid> {
        &self.nets
    }

    pub fn docs(&self) -> &BTreeSet<Bid> {
        &self.docs
    }

    pub fn anchors(&self) -> &BTreeMap<Bid, String> {
        &self.anchors
    }

    pub fn is_anchor(&self, bid: &Bid) -> bool {
        !self.docs.contains(bid)
    }

    pub fn net_get_doc(&self, net: &Bid, node: &Bid) -> Option<(String, Bid, Vec<u16>)> {
        self.get_map(net)
            .and_then(|pm| pm.get_doc_from_id(node, self))
    }

    pub fn get_doc(&self, node: &Bid) -> Option<(String, Bid, Vec<u16>)> {
        self.map
            .values()
            .find_map(|pm_lock| pm_lock.read_arc().get_doc_from_id(node, self))
    }

    pub fn net_get_from_path(&self, net: &Bid, path: &str) -> Option<(Bid, Bid)> {
        let normalized_net = if *net == Bid::nil() { &self.root } else { net };
        self.get_map(normalized_net)
            .and_then(|pm| pm.get(path.as_ref(), self))
    }

    pub fn net_get_from_title(&self, net: &Bid, path: &str) -> Option<(Bid, Bid)> {
        let normalized_net = if *net == Bid::nil() { &self.root } else { net };
        self.get_map(normalized_net)
            .and_then(|pm| pm.get_from_title(path.as_ref(), self))
    }

    pub fn net_get_from_id(&self, net: &Bid, path: &str) -> Option<(Bid, Bid)> {
        let normalized_net = if *net == Bid::nil() { &self.root } else { net };
        self.get_map(normalized_net)
            .and_then(|pm| pm.get_from_id(path.as_ref(), self))
    }

    pub fn get(&self, path: &str) -> Option<(Bid, Bid)> {
        self.map
            .values()
            .find_map(|pm_lock| pm_lock.read_arc().get(path, self))
    }

    pub fn net_path(&self, net: &Bid, bid: &Bid) -> Option<(Bid, String)> {
        self.net_indexed_path(net, bid)
            .map(|(net, path, _)| (net, path))
    }

    pub fn net_indexed_path(&self, net: &Bid, bid: &Bid) -> Option<(Bid, String, Vec<u16>)> {
        let normalized_net = if *net == Bid::nil() { &self.root } else { net };
        self.get_map(normalized_net)
            .and_then(|pm| pm.path(bid, self))
    }

    pub fn path(&self, bid: &Bid) -> Option<(Bid, String)> {
        self.indexed_path(bid)
            .map(|(home_net, home_path, _)| (home_net, home_path))
    }

    pub fn indexed_path(&self, bid: &Bid) -> Option<(Bid, String, Vec<u16>)> {
        self.map
            .values()
            .find_map(|pm| pm.read_arc().path(bid, self))
    }

    pub fn all_local_paths(&self, bid: &Bid) -> Vec<(Bid, Vec<String>)> {
        self.map
            .values()
            .filter_map(|pm| pm.read_arc().all_local_paths(bid))
            .collect::<Vec<(Bid, Vec<String>)>>()
    }

    pub fn get_map(&self, net: &Bid) -> Option<ArcRwLockReadGuard<RawRwLock, PathMap>> {
        let normalized_net = if *net == Bid::nil() { &self.root } else { net };
        self.map
            .get(normalized_net)
            .map(|pm_lock| pm_lock.read_arc())
    }

    pub fn all_paths(&self) -> BTreeMap<Bid, Vec<(String, Bid, Vec<u16>)>> {
        self.map
            .iter()
            .map(|(net, pm)| (*net, pm.read_arc().map().clone()))
            .collect()
    }

    pub fn api(&self) -> Bid {
        self.root
    }

    #[tracing::instrument(skip(self))]
    pub fn api_map(&self) -> ArcRwLockReadGuard<RawRwLock, PathMap> {
        self.map
            .get(&self.root)
            .map(|pm_lock| pm_lock.read_arc())
            .unwrap_or_else(|| {
                tracing::warn!("API map called on empty pathmap!");
                let epm =
                    PathMap::new(WeightKind::Section, self.root, self, self.relations.clone());
                let ephemeral_map = Arc::new(RwLock::new(epm));
                ephemeral_map.read_arc()
            })
    }

    /// Process a queue of events and generate path mutation events
    /// This is the main entry point for updating PathMaps based on BeliefSet events
    pub fn process_event_queue(
        &mut self,
        events: &[&BeliefEvent],
        relations: &Arc<RwLock<BidGraph>>,
    ) -> Vec<BeliefEvent> {
        let mut path_events = Vec::new();

        for event in events {
            match event {
                BeliefEvent::NodeUpdate(_, toml_str, _) => {
                    if let Ok(node) = BeliefNode::try_from(&toml_str[..]) {
                        self.process_node_update(&node, relations);
                    }
                }
                BeliefEvent::NodesRemoved(bids, _) => {
                    self.process_nodes_removed(bids);
                }
                BeliefEvent::NodeRenamed(from, to, _) => {
                    self.process_node_renamed(from, to);
                    for pm_lock in self.map.values() {
                        let mut pm = pm_lock.write();
                        path_events.append(&mut pm.process_event(event, self));
                    }
                }
                BeliefEvent::RelationUpdate(..) | BeliefEvent::RelationRemoved(..) => {
                    // Process this relation update for each PathMap that has matching WeightKind
                    for pm_lock in self.map.values() {
                        let mut pm = pm_lock.write();
                        path_events.append(&mut pm.process_event(event, self));
                    }
                }
                // A relationInsert results in a derivative RelationUpdate if it materially changes
                // the sets relations. Therefore, only handle the relation update to remove
                // redundant processing.
                // BeliefEvent::RelationInsert(source, sink, kind, weight, _) => {}
                // PathsAdded/PathsRemoved are derivative events - we don't process them
                // NodeRenamed, RelationRemoved, BalanceCheck - handled elsewhere or ignored
                _ => {}
            }
        }
        path_events
    }

    /// Process a NodeUpdate event to synchronize nets, docs, and anchors
    pub fn process_node_update(&mut self, node: &BeliefNode, relations: &Arc<RwLock<BidGraph>>) {
        self.anchors.insert(node.bid, to_anchor(&node.title));
        if let Some(id) = node.id.as_ref() {
            self.ids.insert(node.bid, id.clone());
        }

        if node.kind.contains(BeliefKind::API) {
            self.apis.insert(node.bid);
        }

        if node.kind.is_network() {
            self.nets.insert(node.bid);
            let pm = PathMap::new(WeightKind::Section, node.bid, self, relations.clone());
            self.map.insert(node.bid, Arc::new(RwLock::new(pm)));
        }
        if node.kind.is_document() {
            self.docs.insert(node.bid);
        }
    }

    /// Process a NodesRemoved event to clean up nets, docs, and anchors
    pub fn process_nodes_removed(&mut self, bids: &[Bid]) {
        for bid in bids {
            self.nets.remove(bid);
            self.ids.remove(bid);
            self.docs.remove(bid);
            self.anchors.remove(bid);
            self.map.remove(bid);
        }
    }
    /// Process a NodesRemoved event to clean up nets, docs, and anchors
    pub fn process_node_renamed(&mut self, from: &Bid, to: &Bid) {
        if self.nets.remove(from) {
            self.nets.insert(*to);
        }
        if let Some(key) = self.ids.remove(from) {
            self.ids.insert(*to, key);
        }
        if self.docs.remove(from) {
            self.docs.insert(*to);
        }
        if let Some(anchor) = self.anchors.remove(from) {
            self.anchors.insert(*to, anchor);
        };
        if let Some(pm) = self.map.remove(from) {
            self.map.insert(*to, pm);
        }
    }
}

/// Generate a terminal path segment for a relation.
/// This is the core logic for determining what string to use for a path segment:
/// 0. If sink is an API and source is a network, terminal path should be the source ID, else:
/// 1. Explicit path from weight metadata (if provided)
/// 2. Title anchor of the source node (if available and non-empty)
/// 3. Index as fallback
fn generate_terminal_path(
    source: &Bid,
    sink: &Bid,
    explicit_path: Option<&str>,
    index: u16,
    nets: &PathMapMap,
) -> String {
    if nets.apis.contains(sink) && nets.nets.contains(source) {
        source.to_string()
    } else {
        explicit_path
            .filter(|p| !p.is_empty())
            .map(|p| p.to_string())
            .or_else(|| {
                nets.anchors
                    .get(source)
                    .filter(|anchor| !anchor.is_empty())
                    .cloned()
            })
            .unwrap_or_else(|| index.to_string())
    }
}

/// Generate a unique path name for a relation with collision detection.
/// If the generated path collides with an existing path (for a different bid),
/// prepend the index to make it unique.
fn generate_path_name_with_collision_check(
    source: &Bid,
    sink: &Bid,
    sink_path: &str,
    explicit_path: Option<&str>,
    index: u16,
    nets: &PathMapMap,
    existing_map: &[(String, Bid, Vec<u16>)],
) -> String {
    let mut terminal_path = generate_terminal_path(source, sink, explicit_path, index, nets);
    let mut full_path = path_join(sink_path, &terminal_path, nets.is_anchor(source));

    // Check for collision with a different bid
    let has_collision = existing_map
        .iter()
        .any(|(path, bid, _)| path == &full_path && *bid != *source);

    if has_collision {
        terminal_path = format!("{index}-{terminal_path}");
    }
    full_path = path_join(sink_path, &terminal_path, nets.is_anchor(source));
    // Since the index is unique, this should guarantee we don't have any collisions
    debug_assert!(!existing_map
        .iter()
        .any(|(path, bid, _)| path == &full_path && *bid != *source));
    full_path
}

fn pathmap_order(a: &(String, Bid, Vec<u16>), b: &(String, Bid, Vec<u16>)) -> Ordering {
    if let Some(order) = a.2.iter().zip(b.2.iter()).find_map(|(sub_a, sub_b)| {
        let cmp = sub_a.cmp(sub_b);
        match cmp {
            Ordering::Equal => None,
            _ => Some(cmp),
        }
    }) {
        order
    } else {
        a.2.len().cmp(&b.2.len())
    }
}

impl PathMap {
    pub fn new(
        kind: WeightKind,
        net: Bid,
        nets: &PathMapMap,
        relations: Arc<RwLock<BidGraph>>,
    ) -> PathMap {
        // Note this is reversed, because child edges are sorted based on the sink's weights for the
        // relations. A source without any sources is a bottom node (no dependencies), whereas a
        // sink without any sinks is a 'root', or 'main', or highest abstraction node. We want to
        // start our stack from the highest abstraction nodes so that we can sort their child stacks
        // before inserting those stacks into the tree.
        let tree_graph = {
            let relations = relations.read_arc();
            relations.as_subgraph(kind, true)
        };
        let mut stack = BTreeMap::<Bid, (BTreeSet<Bid>, BTreeMap<Bid, (Vec<u16>, String)>)>::new();
        let mut loops = BTreeSet::<(Bid, Bid)>::new();
        let mut subnets = BTreeSet::<Bid>::new();
        depth_first_search(&tree_graph, vec![net], |event| {
            match event {
                DfsEvent::Discover(sink, _) => {
                    // Initialize onto our stack if we haven't already initialized off a TreeEdge event.
                    stack
                        .entry(sink)
                        .or_insert_with(|| (BTreeSet::new(), BTreeMap::new()));
                    Control::<()>::Continue
                }
                DfsEvent::TreeEdge(sink, source)
                | DfsEvent::BackEdge(sink, source)
                | DfsEvent::CrossForwardEdge(sink, source) => {
                    // TreeeEdge: source isn't discovered and will be visited after this event
                    // CrossForwardEdge: source was already visited, so sink is an additional parent.
                    // BackEdge: There's a search already in progress for sink, meaning this is a loop.
                    if let DfsEvent::BackEdge(_, _) = event {
                        loops.insert((sink, source));
                    }
                    let (weight, maybe_sub_path) = tree_graph.edge_weight(sink, source).expect(
                        "Edge weight should exist since we received a DfsEvent for this relation",
                    );

                    let sub_path = generate_terminal_path(
                        &source,
                        &sink,
                        maybe_sub_path.as_deref(),
                        *weight,
                        nets,
                    );

                    let sub_path_info = (vec![*weight], sub_path);

                    stack.get_mut(&sink).map(|path_info| {
                        path_info.1.insert(source, sub_path_info);
                    }).expect("Never to encounter a sink edge prior to adding it to the stack during a DFS search");

                    if stack
                        .get_mut(&source)
                        .map(|path_info| {
                            path_info.0.insert(sink);
                        })
                        .is_none()
                    {
                        stack.insert(source, (BTreeSet::from_iter(vec![sink]), BTreeMap::new()));
                    }

                    if nets.nets().contains(&source) && source != net {
                        // Prune network subnets - they have their own separate PathMaps.
                        // Note: The subnet is already in the parent's path map (added above to sink_paths),
                        // so it will appear in the final PathMap. We just don't want to traverse into it.
                        let (sinks, source_sub_paths) = stack
                            .remove(&source)
                            .expect("Source should be in stack since we just added/updated it");
                        debug_assert!(!sinks.is_empty());
                        debug_assert!(source_sub_paths.is_empty());
                        for sink in sinks.iter() {
                            let (_, sink_paths) = stack
                                .get(sink)
                                .expect("To have all sinks still present in the stack");

                            debug_assert!(sink_paths.get(&source).is_some());
                        }
                        Control::Prune
                    } else {
                        Control::Continue
                    }
                }
                DfsEvent::Finish(source, _) => {
                    // sort the sinks's sources based on edge weights. create vec with self row on top
                    // and append sorted child vecs. Pop self from stack and push self vec onto the next
                    // parent.
                    if source != net {
                        let (sinks, source_sub_paths) = stack
                            .remove(&source)
                            .expect("Never to have a finish event prior to a discover event");
                        for sink in sinks.iter() {
                            if loops.contains(&(*sink, source)) {
                                tracing::info!(
                                    "Avoiding infinite paths, not inserting sub-paths \
                                    of {} into path set for {}",
                                    source,
                                    sink
                                );
                                continue;
                            }
                            let (_, sink_paths) = stack
                                .get_mut(sink)
                                .expect("To have all sinks still present in the stack");

                            let (source_base_order, source_base_path) =
                                sink_paths.get(&source).cloned().expect(
                                    "To have already mapped source to sink's sub-paths \
                                     during the DFS.",
                                );
                            for (bid, (path_order, sub_path)) in source_sub_paths.iter() {
                                let mut sub_path_order = source_base_order.clone();
                                sub_path_order.extend(path_order);
                                sink_paths.insert(
                                    *bid,
                                    (
                                        sub_path_order,
                                        path_join(
                                            &source_base_path,
                                            &sub_path.clone(),
                                            nets.is_anchor(bid),
                                        ),
                                    ),
                                );
                            }
                        }
                    }
                    Control::Continue
                }
            }
        });

        // It's possible that top was never in the graph in the first place
        let (_sinks, inverted_path_map) = stack
            .remove(&net)
            .expect("To always discover the PathMap net in the DFS search");

        let mut map = Vec::from_iter(
            vec![(String::from(""), net, Vec::<u16>::default())]
                .into_iter()
                .chain(inverted_path_map.into_iter().map(|(bid, (order, path))| {
                    if nets.nets().contains(&bid) && bid != net && !subnets.contains(&bid) {
                        subnets.insert(bid);
                    }
                    (path, bid, order)
                })),
        );
        map.sort_by(pathmap_order);
        let mut bid_map = BTreeMap::new();
        let mut path_map = BTreeMap::new();
        for (idx, (path, bid, _order)) in map.iter().enumerate() {
            let bid_idx_vec = bid_map.entry(*bid).or_insert(Vec::<usize>::default());
            bid_idx_vec.push(idx);
            path_map.insert(path.clone(), idx);
        }

        let mut id_map = IdMap::default();
        let mut title_map = IdMap::default();
        for (_, bid, _) in map.iter() {
            if let Some(title) = nets.anchors().get(bid) {
                if !nets.is_anchor(bid) && !title.is_empty() {
                    title_map.insert(title.clone(), *bid);
                }
            }
            if let Some(id) = nets.ids.get(bid) {
                id_map.insert(id.clone(), *bid);
            }
        }
        // tracing::debug!(
        //     "Initialized pathmap for {}, contains {} paths and subnets: {:?}",
        //     net,
        //     path_map.len(),
        //     subnets
        // );
        let mut pathmap = PathMap {
            map,
            bid_map,
            path_map,
            id_map,
            title_map,
            kind,
            net,
            subnets,
            loops,
        };
        pathmap.sort();
        pathmap
    }

    fn sort(&mut self) {
        self.map.sort_by(pathmap_order);
    }

    pub fn map(&self) -> &Vec<(String, Bid, Vec<u16>)> {
        &self.map
    }

    pub fn subnets(&self) -> &BTreeSet<Bid> {
        &self.subnets
    }

    /// Returns the doc path and doc bid that contains the input path
    pub fn get_doc<P: AsRef<Path> + std::fmt::Debug>(
        &self,
        path_ref: P,
        nets: &PathMapMap,
    ) -> Option<(String, Bid)> {
        let path = get_doc_path(&path_ref.as_ref().to_string_lossy()).to_string();
        self.get(&path, nets).map(|(_net, bid)| (path, bid))
    }

    /// Returns the doc path and doc bid that contains the input path
    pub fn get_doc_from_id(
        &self,
        node: &Bid,
        nets: &PathMapMap,
    ) -> Option<(String, Bid, Vec<u16>)> {
        self.path(node, nets)
            .and_then(|(_home_net, path_ref, _order)| {
                let doc_path = get_doc_path(&path_ref);
                self.indexed_get(doc_path, nets)
                    .map(|(_net, bid, order)| (doc_path.to_string(), bid, order))
            })
    }

    /// Returns the net and doc bid that matches the input doc title
    pub fn get_from_title(&self, title: &str, nets: &PathMapMap) -> Option<(Bid, Bid)> {
        let anchored_title = to_anchor(title);
        self.title_map
            .get_bid(&anchored_title)
            .map(|bid| (self.net, *bid))
            .or_else(|| {
                self.subnets.iter().find_map(|net_bid| {
                    nets.get_map(net_bid).and_then(|subnet_path_map| {
                        subnet_path_map.get_from_title(&anchored_title, nets)
                    })
                })
            })
    }

    /// Returns the net and doc bid that matches the input doc title
    pub fn get_from_title_regex(
        &self,
        title: &WrappedRegex,
        nets: &PathMapMap,
    ) -> Option<(Bid, Bid)> {
        self.title_map
            .get_bid_from_regex(title)
            .map(|bid| (self.net, *bid))
            .or_else(|| {
                self.subnets.iter().find_map(|net_bid| {
                    nets.get_map(net_bid).and_then(|subnet_path_map| {
                        subnet_path_map.get_from_title_regex(title, nets)
                    })
                })
            })
    }

    /// Returns the net and bid that matches the input node id
    pub fn get_from_id(&self, id: &str, nets: &PathMapMap) -> Option<(Bid, Bid)> {
        self.id_map
            .get_bid(id)
            .map(|bid| (self.net, *bid))
            .or_else(|| {
                self.subnets.iter().find_map(|net_bid| {
                    nets.get_map(net_bid)
                        .and_then(|subnet_path_map| subnet_path_map.get_from_id(id, nets))
                })
            })
    }

    // Returns (home net bid, path bid)
    pub fn get(&self, path: &str, nets: &PathMapMap) -> Option<(Bid, Bid)> {
        self.indexed_get(path, nets)
            .map(|(net_bid, path_bid, _)| (net_bid, path_bid))
    }

    pub fn indexed_get(&self, path: &str, nets: &PathMapMap) -> Option<(Bid, Bid, Vec<u16>)> {
        self.map
            .iter()
            .find_map(|(a_path, a_bid, a_order)| {
                if a_path == path {
                    Some((self.net, *a_bid, a_order.clone()))
                } else {
                    None
                }
            })
            .or_else(|| {
                self.subnets.iter().find_map(|net_bid| {
                    let first_idx = self
                        .bid_map
                        .get(net_bid)
                        .and_then(|idx_vec| idx_vec.first().copied())
                        .expect("pathmap subnets to be synchronized with pathmap.bid_map");
                    let (subnet_path, _subnet_bid, net_order) = &self.map[first_idx];
                    relative_path(path, subnet_path).ok().and_then(|sub_path| {
                        nets.get_map(net_bid).and_then(|subnet_path_map| {
                            subnet_path_map.indexed_get(&sub_path, nets).map(
                                |(home_net, bid, home_order)| {
                                    let mut full_order = net_order.clone();
                                    full_order.append(&mut home_order.clone());
                                    (home_net, bid, full_order)
                                },
                            )
                        })
                    })
                })
            })
    }

    /// Returns: (home_network Bid, full_path from this pathmap to the bid,
    /// crossing any subnet paths)
    pub fn path(&self, bid: &Bid, nets: &PathMapMap) -> Option<(Bid, String, Vec<u16>)> {
        self.map
            .iter()
            .find_map(|(a_path, a_bid, order)| {
                if *bid == *a_bid {
                    Some((self.net, a_path.clone(), order.clone()))
                } else {
                    None
                }
            })
            .or_else(|| {
                self.subnets.iter().find_map(|net_bid| {
                    let first_idx = self
                        .bid_map
                        .get(net_bid)
                        .and_then(|idx_vec| idx_vec.first().copied())
                        .expect("pathmap subnets to be synchronized with pathmap.bid_map");
                    let (subnet_path, _subnet_bid, net_order) = &self.map[first_idx];
                    nets.get_map(net_bid)
                        .and_then(|subnet_path_map| subnet_path_map.path(bid, nets))
                        .map(|(home_net_bid, home_path, home_order)| {
                            let mut full_order = net_order.clone();
                            full_order.append(&mut home_order.clone());

                            // tracing::debug!(
                            //     "combined subnet path for bid {}:\
                            //     \n\tres: {}\
                            //     \n\tsubnet_path: {:?}\
                            //     \n\tself.net: {}\
                            //     \n\tbid home_net: {}",
                            //     bid,
                            //     res.1,
                            //     subnet_path,
                            //     self.net,
                            //     res.0
                            // );
                            (
                                home_net_bid,
                                path_join(subnet_path, &home_path, false),
                                full_order,
                            )
                        })
                })
            })
    }

    /// Returns: (home_network Bid, home_network_path to the bid (not relative to this pathmap). If
    /// the bid is a known network, shortcuts and returns the bid and an empty path.
    pub fn home_path(&self, bid: &Bid, nets: &PathMapMap) -> Option<(Bid, String)> {
        // If this bid is a network node, return the network itself as home
        // with an empty path, since network nodes are roots of their own network.
        if nets.nets().contains(bid) {
            Some((*bid, String::from("")))
        } else {
            self.map.iter().find_map(|(a_path, a_bid, _order)| {
                if *bid == *a_bid {
                    Some((self.net, a_path.clone()))
                } else {
                    None
                }
            })
        }
        .or_else(|| {
            self.subnets.iter().find_map(|subnet_bid| {
                nets.get_map(subnet_bid)
                    .and_then(|subnet_path_map| subnet_path_map.home_path(bid, nets))
            })
        })
    }

    pub fn all_local_paths(&self, bid: &Bid) -> Option<(Bid, Vec<String>)> {
        let paths = self
            .map
            .iter()
            .filter_map(|(a_path, a_bid, _order)| {
                if *bid == *a_bid {
                    Some(a_path.clone())
                } else {
                    None
                }
            })
            .collect::<Vec<String>>();
        if paths.is_empty() {
            None
        } else {
            Some((self.net, paths))
        }
    }

    // Return a list of all paths connected to this subnet
    pub fn all_paths(&self, nets: &PathMapMap, visited: &mut BTreeSet<Bid>) -> Vec<String> {
        let mut paths = Vec::default();
        if visited.contains(&self.net) {
            return paths;
        }
        visited.insert(self.net);
        for (a_path, a_bid, _order) in self.map.iter() {
            if nets.nets().contains(a_bid) && !visited.contains(a_bid) {
                if let Some(sub_paths) = nets.get_map(a_bid).map(|pm| pm.all_paths(nets, visited)) {
                    for subnet_path in sub_paths.iter() {
                        paths.push(format!(
                            "{}/{}",
                            trim_path_sep(a_path),
                            trim_path_sep(subnet_path)
                        ));
                    }
                }
            } else {
                paths.push(a_path.clone());
            }
        }

        paths
    }

    /// Returns the indices for all paths that are descendents of source. If direct is true, then
    /// only returns the direct descendants, if false, returns all descendants within this PathMap.
    /// NOTE: This assumes the pathmap is already sorted.
    fn source_sub_indices(&self, source: &Bid, direct: bool) -> Vec<(usize, Vec<usize>)> {
        let mut indices = Vec::default();
        let Some(sink_starts) = self.bid_map.get(source) else {
            return indices;
        };
        for sink_start in sink_starts.iter() {
            let mut sink_subs = Vec::default();
            let (_path, _bid, sink_order) = &self.map[*sink_start];
            for (idx, (_path, _bid, order)) in self.map[*sink_start..].iter().enumerate() {
                if !order.starts_with(sink_order) {
                    break;
                } else if sink_order == order || (!order.len() != sink_order.len() + 1 && direct) {
                    continue;
                } else {
                    sink_subs.push(idx + *sink_start);
                }
            } // Determine if source is already a direct child of sink in our path. If so, acquire
            indices.push((*sink_start, sink_subs));
        }
        indices
    }

    /// Generate a unique path name for a relation (wrapper for backward compatibility)
    fn generate_path_name(
        &self,
        source: &Bid,
        sink: &Bid,
        sink_path: &str,
        explicit_path: Option<String>,
        index: u16,
        nets: &PathMapMap,
    ) -> String {
        generate_path_name_with_collision_check(
            source,
            sink,
            sink_path,
            explicit_path.as_deref(),
            index,
            nets,
            &self.map,
        )
    }

    /// Process a relation event and generate path mutations
    pub fn process_event(&mut self, event: &BeliefEvent, nets: &PathMapMap) -> Vec<BeliefEvent> {
        let res = match event {
            BeliefEvent::NodeRenamed(from, to, _) => {
                let mut derivatives = Vec::default();
                for idx in 0..self.map.len() {
                    if self.map[idx].1 == *from {
                        let (path, bid, order) = &mut self.map[idx];
                        *bid = *to;
                        derivatives.push(BeliefEvent::PathUpdate(
                            self.net,
                            path.clone(),
                            *bid,
                            order.clone(),
                            EventOrigin::Local,
                        ));
                    }
                }
                if let Some(map_indices) = self.bid_map.remove(from) {
                    self.bid_map.insert(*to, map_indices);
                }
                if let Some(id) = self.id_map.remove(from) {
                    self.id_map.insert(id, *to);
                }
                if let Some(title) = self.title_map.remove(from) {
                    self.title_map.insert(title, *to);
                }
                if self.subnets.remove(from) {
                    self.subnets.insert(*to);
                }
                let new_loops = BTreeSet::from_iter(self.loops.iter().map(|(source, sink)| {
                    let new_source = if *source == *from { *to } else { *source };
                    let new_sink = if *sink == *from { *to } else { *sink };
                    (new_source, new_sink)
                }));
                self.loops = new_loops;
                derivatives
            }
            BeliefEvent::RelationUpdate(source, sink, weightset, _) => {
                self.process_relation_update(source, sink, weightset, nets)
            }
            BeliefEvent::RelationRemoved(source, sink, _) => {
                self.process_relation_update(source, sink, &WeightSet::default(), nets)
            }
            _ => Vec::default(),
        };
        // if !res.is_empty() {
        //     tracing::debug!("{} derivatives: {:?}", event, res);
        // }
        res
    }

    fn process_relation_update(
        &mut self,
        source: &Bid,
        sink: &Bid,
        weightset: &WeightSet,
        nets: &PathMapMap,
    ) -> Vec<BeliefEvent> {
        // FIXME: This isn't checking for loops at all
        let mut derivatives = Vec::default();
        let sink_sub_indices = self.source_sub_indices(sink, false);
        if sink_sub_indices.is_empty() {
            return derivatives;
        }
        if nets.nets.contains(sink) && self.net != *sink {
            return derivatives;
        }
        let Some(new_weight) = weightset.get(&self.kind) else {
            // This looks exactly like a removal event to this pathmap.
            // collect all the sources to the source -- we have to remove them as well as their
            // paths are dependent on this removed relation.
            let mut paths = Vec::new();
            for (_sink_index, sub_indices) in sink_sub_indices.iter().rev() {
                if let Some(source_order) = sub_indices.iter().find_map(|idx| {
                    if *source == self.map[*idx].1 {
                        Some(self.map[*idx].2.clone())
                    } else {
                        None
                    }
                }) {
                    for sub_idx in sub_indices.iter().rev() {
                        let starts_with = self.map[*sub_idx].2.starts_with(&source_order);
                        if starts_with {
                            let (rm_path, _rm_bid, _rm_order) = self.map.remove(*sub_idx);
                            paths.push(rm_path);
                        }
                    }
                }
            }
            if !paths.is_empty() {
                derivatives.push(BeliefEvent::PathsRemoved(
                    self.net,
                    paths,
                    EventOrigin::Local,
                ));
                self.bid_map.clear();
                self.path_map.clear();
                for (idx, (path, bid, _order)) in self.map.iter().enumerate() {
                    let bid_idx_vec = self.bid_map.entry(*bid).or_default();
                    bid_idx_vec.push(idx);
                    self.path_map.insert(path.clone(), idx);
                }
            }
            return derivatives;
        };
        let Some(new_idx) = new_weight.get::<u16>(WEIGHT_SORT_KEY) else {
            tracing::error!(
                "All valid RelationUpdates are expected to hold weight sorting indexes \
                within their edge payload within the {} variable. Ignoring edge",
                WEIGHT_SORT_KEY
            );
            return derivatives;
        };
        // Reverse the iterator so that we can manipulate self.map from back to front and not
        // destroy our index mappings while we mutate the map.
        for (sink_index, sub_indices) in sink_sub_indices.iter().rev() {
            // Clone this so we don't keep a nonmutable reference into self.map;
            let (new_path, new_order) = {
                let (sink_path, sink_bid, sink_order) = &self.map[*sink_index];
                debug_assert!(*sink_bid == *sink);
                let mut new_order = sink_order.clone();
                new_order.push(new_idx);
                let new_path = self.generate_path_name(
                    source,
                    sink,
                    sink_path,
                    new_weight.get::<String>(WEIGHT_DOC_PATH),
                    new_idx,
                    nets,
                );
                (new_path, new_order)
            };

            let source_sub_indices = sub_indices
                .iter()
                .rev()
                .filter(|&idx| self.map[*idx].1 == *source)
                .copied()
                .collect::<Vec<usize>>();
            let new_entry = (new_path, *source, new_order);
            match source_sub_indices.is_empty() {
                true => {
                    let last_entry_idx = sub_indices.last().copied().unwrap_or(*sink_index);
                    // Ensure we're inserting in the same order as our explicit WEIGHT_SORT_KEY
                    // suggests.
                    if sub_indices.is_empty() {
                        if new_idx != 0 {
                            tracing::warn!("edge index is {}, expected 0", new_idx);
                            // *new_entry.2.last_mut().unwrap() = 0;
                        }
                    } else {
                        let (_path, _bid, order) = &self.map[last_entry_idx];
                        let last_order = order[new_entry.2.len() - 1];
                        if new_idx - 1 != last_order {
                            tracing::warn!("edge index is {}, expected one greater than last index, which is {}",
                              new_idx, last_order
                            );
                            // *new_entry.2.last_mut().unwrap() = last_order + 1;
                        }
                    }
                    derivatives.push(BeliefEvent::PathAdded(
                        self.net,
                        new_entry.0.clone(),
                        *source,
                        new_entry.2.clone(),
                        EventOrigin::Local,
                    ));
                    self.map.insert(last_entry_idx + 1, new_entry);
                }
                false => {
                    // There should never be a case where we have duplicate sink<-source edges
                    // within the same WeightKind subgraph.
                    debug_assert!(source_sub_indices.len() == 1);
                    let idx_to_update = source_sub_indices
                        .first()
                        .expect("We know this vec is of len 1");
                    debug_assert!(
                        &self.map[*idx_to_update].1 == source,
                        "We shouldn't ever be overwriting a path to another bid, \
                        just changing its relative path or ordering."
                    );
                    let old_order = &self.map[*idx_to_update].2.clone();
                    debug_assert!(
                        old_order.len() == new_entry.2.len(),
                        "[{}] '{}' old order: {:?}, new order: {:?}",
                        self.net,
                        new_entry.0,
                        old_order,
                        new_entry.2
                    );
                    if *old_order != new_entry.2 {
                        let mut next_idx = idx_to_update + 1;
                        while next_idx < self.map.len() {
                            let next_order = &mut self.map[next_idx].2;
                            if !next_order.starts_with(old_order) {
                                break;
                            }
                            next_order[..new_entry.2.len()].copy_from_slice(&new_entry.2);
                            next_idx += 1;
                        }
                    }
                    derivatives.push(BeliefEvent::PathUpdate(
                        self.net,
                        new_entry.0.clone(),
                        *source,
                        new_entry.2.clone(),
                        EventOrigin::Local,
                    ));
                    self.map[*idx_to_update] = new_entry;
                }
            }
        }

        if !derivatives.is_empty() {
            // Regenerate our support indices
            self.bid_map.clear();
            self.path_map.clear();
            for (idx, (path, bid, _order)) in self.map.iter().enumerate() {
                let bid_idx_vec = self.bid_map.entry(*bid).or_default();
                bid_idx_vec.push(idx);
                self.path_map.insert(path.clone(), idx);
            }

            if nets.nets.contains(source) && self.net != *source {
                self.subnets.insert(*source);
            }

            // Update our title and id maps
            if derivatives
                .iter()
                .any(|e| matches!(e, BeliefEvent::PathAdded(..)))
            {
                if let Some(source_title) = nets.anchors.get(source) {
                    if !nets.is_anchor(source) && !source_title.is_empty() {
                        // We only get title anchors if the title anchor is non-empty
                        self.title_map.insert(source_title.clone(), *source);
                    }
                }
                if let Some(id_str) = nets.ids.get(source) {
                    self.id_map.insert(id_str.clone(), *source);
                }
            }
        }
        derivatives
    }
}
