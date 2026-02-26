/// Defines [PathMapMap], and [PathMap], who's primary job is to generate and
/// maintain relative paths between [BeliefNodes] within a [BeliefBase], even
/// when the relations within that set are changing.
///
/// # Network Sort-Space Reservation
///
/// `u16::MAX` (`NETWORK_SECTION_SORT_KEY`) is a reserved sentinel sort key used to
/// separate a network's two structural roles in its own PathMap:
///
/// - Sort positions `[0..u16::MAX-1]`: document children of the network (normal address space)
/// - Sort position `[u16::MAX]`: the network's own `index.md` content plane (gateway slot)
///
/// This mirrors the IP LAN gateway analogy: just as a LAN reserves an address for the
/// network control interface, PathMap reserves `u16::MAX` for the index.md subsection tree.
/// The `"index.md"` hardcoded entry in every network PathMap carries this order, and the
/// DFS in `PathMap::new` overrides the sort key to `u16::MAX` for anchor (heading/section)
/// children of the network root so their paths are correctly computed as
/// `[u16::MAX, heading_idx]` rather than colliding with document paths at `[heading_idx]`.
///
/// `process_relation_update` uses `nets.is_anchor(source)` to select the correct parent
/// entry when a network node has both a `""` (document parent, order `[]`) and an
/// `"index.md"` (section parent, order `[u16::MAX]`) entry in its `bid_map`.
use parking_lot::{ArcRwLockReadGuard, RawRwLock, RwLock};
use petgraph::visit::{depth_first_search, Control, DfsEvent};
use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
    fmt,
    sync::Arc,
};

use crate::{
    beliefbase::BidGraph,
    codec::network::NETWORK_NAME,
    event::{BeliefEvent, EventOrigin},
    paths::path::{as_anchor, to_anchor, AnchorPath},
    properties::{
        asset_namespace, href_namespace, BeliefKind, BeliefNode, Bid, Bref, WeightKind, WeightSet,
        WEIGHT_SORT_KEY,
    },
    query::WrappedRegex,
};

/// Reserved sort key for a network node's own `index.md` content plane.
///
/// Documents are children of the network at sort positions `[0..NETWORK_SECTION_SORT_KEY-1]`.
/// Headings/anchors parsed from `index.md` are children of the network at sort positions
/// `[NETWORK_SECTION_SORT_KEY, heading_idx]`, keeping the two sort spaces non-colliding.
pub const NETWORK_SECTION_SORT_KEY: u16 = u16::MAX;

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

/// Generate a terminal path segment for a relation.
/// This is the core logic for determining what string to use for a path segment:
/// 0. If sink is an API and source is a network, terminal path should be the source ID, else:
/// 1. Explicit path from weight metadata (if provided)
/// 2. Title anchor of the source node (if available and non-empty)
/// 3. Index as fallback
///
/// FIXME: explicit path should be escaped or to_anchorized so that we ensure valid urls
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
            .or_else(|| nets.ids.get(source).cloned())
            .unwrap_or_else(|| index.to_string())
    }
}

/// Generate a unique path name for a relation with collision detection.
/// If the generated path collides with an existing path (for a different bid),
/// use the Bref (BID namespace) to make it unique.
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
    let sink_ap = AnchorPath::from(sink_path);
    let mut full_path: String = sink_ap
        .join(nets.anchorize(source, &terminal_path))
        .into_string();

    // Check for collision with a different bid
    let has_collision = existing_map
        .iter()
        .any(|(path, bid, _)| path == &full_path && *bid != *source);

    if has_collision {
        // Use Bref (BID namespace) as fallback for collision
        terminal_path = source.bref().to_string();
    }
    full_path = sink_ap
        .join(nets.anchorize(source, &terminal_path))
        .into_string();
    // Since the Bref is unique per BID, this should guarantee we don't have any collisions
    debug_assert!(!existing_map
        .iter()
        .any(|(path, bid, _)| path == &full_path && *bid != *source));
    full_path
}

/// We want to ensure a consistent ordering of pathmaps: first order by the order element, and
/// equality order by the path string lexical order.
pub(crate) fn pathmap_order(a: &[u16], b: &[u16]) -> Ordering {
    if let Some(order) = a.iter().zip(b.iter()).find_map(|(sub_a, sub_b)| {
        let cmp = sub_a.cmp(sub_b);
        match cmp {
            Ordering::Equal => None,
            _ => Some(cmp),
        }
    }) {
        order
    } else {
        a.len().cmp(&b.len())
    }
}

/// [PathMapMap] serves as a central manager for all [PathMap] instances for a specific
/// [crate::properties::WeightKind] within a [crate::beliefbase::BeliefBase].
///
/// It orchestrates the creation, storage, and updating of [PathMap]s, each corresponding to a
/// distinct sub-network instantiated within the BeliefBase. Each
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
/// *   Render hierarchical views of [crate::beliefbase::BeliefBase]s.
/// *   Generate stable, relative URLs or paths for [crate::properties::BeliefNode]s.
/// *   Track how entities are interconnected across different, potentially nested,
///     networks.
///
/// It acts as the primary interface for querying and maintaining the overall
/// navigable structure of a [crate::beliefbase::BeliefBase].
#[derive(Debug, Clone)]
pub struct PathMapMap {
    map: BTreeMap<Bref, Arc<RwLock<PathMap>>>,
    root: Bid,
    nets: BTreeSet<Bid>,
    docs: BTreeSet<Bid>,
    apis: BTreeSet<Bid>,
    titles: BTreeMap<Bid, String>,
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
            titles: BTreeMap::default(),
            ids: BTreeMap::default(),
            relations: relations.clone(),
        };
        let api_pm = PathMap::new(WeightKind::Section, root, &pmm, relations);
        pmm.map.insert(root.bref(), Arc::new(RwLock::new(api_pm)));
        pmm
    }
}

impl fmt::Display for PathMapMap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (net_bref, pm_arc) in self.map.iter() {
            let net_pm = pm_arc.read();
            let net_anchor = self.nets().iter().find_map(|net_bid| {
                if net_bid.bref() == *net_bref {
                    self.titles.get(net_bid).cloned()
                } else {
                    None
                }
            });
            write!(
                f,
                "\n{}: {} anchored paths:\n{}\n\n",
                net_bref,
                net_anchor.unwrap_or_default(),
                net_pm
                    .map()
                    .iter()
                    .map(|(path, bid, order)| format!(
                        "{}\t{} <- \"{}\"",
                        order
                            .iter()
                            .map(|idx| idx.to_string())
                            .collect::<Vec<_>>()
                            .join("."),
                        bid.bref(),
                        path,
                    ))
                    .collect::<Vec<_>>()
                    .join("\n")
            )?;
        }
        Ok(())
    }
}

impl PathMapMap {
    #[tracing::instrument(skip(states, relations))]
    pub fn new(states: &BTreeMap<Bid, BeliefNode>, relations: Arc<RwLock<BidGraph>>) -> PathMapMap {
        // tracing::debug!(
        //     "[PathMapMap::new] Creating PathMapMap with {} states, {} relations",
        //     states.len(),
        //     relations.read_arc().as_graph().edge_count()
        // );
        let mut pmm = PathMapMap {
            relations: relations.clone(),
            ..Default::default()
        };
        for node in states.values() {
            pmm.titles.insert(node.bid, node.title.clone());
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
        let asset_node = BeliefNode::asset_network();
        pmm.nets.insert(asset_node.bid);
        pmm.titles.insert(asset_node.bid, asset_node.title.clone());
        let href_node = BeliefNode::href_network();
        pmm.nets.insert(href_node.bid);
        pmm.titles.insert(href_node.bid, href_node.title.clone());

        // Check for states vs relations mismatch
        let states_bids: std::collections::BTreeSet<_> = states.keys().copied().collect();
        let mut relation_bids = std::collections::BTreeSet::new();
        {
            let rel_guard = relations.read_arc();
            for idx in rel_guard.as_graph().node_indices() {
                relation_bids.insert(rel_guard.as_graph()[idx]);
            }
        }

        let in_states_not_relations: Vec<_> = states_bids.difference(&relation_bids).collect();
        let in_relations_not_states: Vec<_> = relation_bids.difference(&states_bids).collect();

        if !in_states_not_relations.is_empty() {
            tracing::warn!(
                "[PathMapMap::new] {} nodes in states but NOT in relations graph: {:?}",
                in_states_not_relations.len(),
                in_states_not_relations.iter().take(5).collect::<Vec<_>>()
            );
        }
        if !in_relations_not_states.is_empty() {
            tracing::error!(
                "[PathMapMap::new] ISSUE 34 VIOLATION: {} nodes in relations but NOT in states! \
                 DbConnection.eval_unbalanced/eval_trace should have loaded these. Sample: {:?}",
                in_relations_not_states.len(),
                in_relations_not_states.iter().take(5).collect::<Vec<_>>()
            );
            // Continue with graceful degradation - PathMap will skip orphaned nodes
        }

        pmm.map.clear();
        for net in pmm.nets.iter() {
            if !pmm.map.contains_key(&net.bref()) {
                let pm = PathMap::new(WeightKind::Section, *net, &pmm, relations.clone());
                // tracing::debug!(
                //     "[PathMapMap::new] Created PathMap for network {}: {} entries",
                //     net,
                //     pm.map().len()
                // );
                pmm.map.insert(net.bref(), Arc::new(RwLock::new(pm)));
            }
        }

        // tracing::debug!(
        //     "[PathMapMap::new] Completed PathMapMap with {} network maps",
        //     pmm.map.len()
        // );
        pmm
    }

    pub fn map(&self) -> &BTreeMap<Bref, Arc<RwLock<PathMap>>> {
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

    pub fn titles(&self) -> &BTreeMap<Bid, String> {
        &self.titles
    }

    pub fn is_anchor(&self, bid: &Bid) -> bool {
        !self.docs.contains(bid)
    }

    pub fn anchorize(&self, bid: &Bid, subpath: &str) -> String {
        if !self.is_anchor(bid) {
            subpath.to_string()
        } else {
            as_anchor(subpath)
        }
    }

    pub fn net_get_doc(&self, net: &Bref, node: &Bid) -> Option<(String, Bid, Vec<u16>)> {
        self.get_map(net)
            .and_then(|pm| pm.get_doc_from_id(node, self))
    }

    pub fn get_doc(&self, node: &Bid) -> Option<(String, Bid, Vec<u16>)> {
        self.map
            .values()
            .find_map(|pm_lock| pm_lock.read_arc().get_doc_from_id(node, self))
    }

    pub fn net_get_from_path(&self, net: &Bref, path: &str) -> Option<(Bid, Bid)> {
        let normalized_net = if *net == Bref::default() {
            &self.root.bref()
        } else {
            net
        };
        self.get_map(normalized_net)
            .and_then(|pm| pm.get(path.as_ref(), self))
    }

    pub fn net_get_from_title(&self, net: &Bref, path: &str) -> Option<(Bid, Bid)> {
        let normalized_net = if *net == Bref::default() {
            &self.root.bref()
        } else {
            net
        };
        self.get_map(normalized_net)
            .and_then(|pm| pm.get_from_title(path.as_ref(), self))
    }

    pub fn net_get_from_id(&self, net: &Bref, path: &str) -> Option<(Bid, Bid)> {
        let normalized_net = if *net == Bref::default() {
            &self.root.bref()
        } else {
            net
        };
        self.get_map(normalized_net)
            .and_then(|pm| pm.get_from_id(path.as_ref(), self))
    }

    pub fn get(&self, path: &str) -> Option<(Bid, Bid)> {
        self.map
            .values()
            .find_map(|pm_lock| pm_lock.read_arc().get(path, self))
    }

    pub fn net_path(&self, net: &Bref, bid: &Bid) -> Option<(Bid, String)> {
        self.net_indexed_path(net, bid)
            .map(|(net, path, _)| (net, path))
    }

    pub fn net_indexed_path(&self, net: &Bref, bid: &Bid) -> Option<(Bid, String, Vec<u16>)> {
        let normalized_net = if *net == Bref::default() {
            &self.root.bref()
        } else {
            net
        };
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

    pub fn get_map(&self, net: &Bref) -> Option<ArcRwLockReadGuard<RawRwLock, PathMap>> {
        let normalized_net = if *net == Bref::default() {
            &self.root.bref()
        } else {
            net
        };
        self.map
            .get(normalized_net)
            .map(|pm_lock| pm_lock.read_arc())
    }

    pub fn all_paths(&self) -> BTreeMap<Bref, Vec<(String, Bid, Vec<u16>)>> {
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
            .get(&self.root.bref())
            .map(|pm_lock| pm_lock.read_arc())
            .unwrap_or_else(|| {
                tracing::warn!("API map called on empty pathmap!");
                let epm =
                    PathMap::new(WeightKind::Section, self.root, self, self.relations.clone());
                let ephemeral_map = Arc::new(RwLock::new(epm));
                ephemeral_map.read_arc()
            })
    }

    #[tracing::instrument(skip(self))]
    pub fn asset_map(&self) -> ArcRwLockReadGuard<RawRwLock, PathMap> {
        self.map
            .get(&asset_namespace().bref())
            .map(|pm_lock| pm_lock.read_arc())
            .unwrap_or_else(|| {
                tracing::warn!("asset map called on empty pathmap!");
                let epm = PathMap::new(
                    WeightKind::Section,
                    asset_namespace(),
                    self,
                    self.relations.clone(),
                );
                let ephemeral_map = Arc::new(RwLock::new(epm));
                ephemeral_map.read_arc()
            })
    }

    #[tracing::instrument(skip(self))]
    pub fn href_map(&self) -> ArcRwLockReadGuard<RawRwLock, PathMap> {
        self.map
            .get(&href_namespace().bref())
            .map(|pm_lock| pm_lock.read_arc())
            .unwrap_or_else(|| {
                tracing::warn!("asset map called on empty pathmap!");
                let epm = PathMap::new(
                    WeightKind::Section,
                    href_namespace(),
                    self,
                    self.relations.clone(),
                );
                let ephemeral_map = Arc::new(RwLock::new(epm));
                ephemeral_map.read_arc()
            })
    }

    /// Process a queue of events and generate path mutation events
    /// This is the main entry point for updating PathMaps based on BeliefBase events
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
                // A RelationChange results in a derivative RelationUpdate if it materially changes
                // the sets relations. Therefore, only handle the relation update to remove
                // redundant processing.
                // BeliefEvent::RelationChange(source, sink, kind, weight, _) => {}
                // PathsAdded/PathsRemoved are derivative events - we don't process them
                // NodeRenamed, RelationRemoved, BalanceCheck - handled elsewhere or ignored
                _ => {}
            }
        }
        path_events
    }

    /// Process a NodeUpdate event to synchronize nets, docs, and titles
    pub fn process_node_update(&mut self, node: &BeliefNode, relations: &Arc<RwLock<BidGraph>>) {
        self.titles.insert(node.bid, node.title.clone());
        self.ids.insert(node.bid, node.id());

        if node.kind.contains(BeliefKind::API) {
            self.apis.insert(node.bid);
        }

        if node.kind.is_network() {
            self.nets.insert(node.bid);
            let pm = PathMap::new(WeightKind::Section, node.bid, self, relations.clone());
            self.map.insert(node.bid.bref(), Arc::new(RwLock::new(pm)));
        }
        if node.kind.is_document() || node.kind.is_external() {
            self.docs.insert(node.bid);
        }
    }

    /// Process a NodesRemoved event to clean up nets, docs, and titles
    pub fn process_nodes_removed(&mut self, bids: &[Bid]) {
        for bid in bids {
            self.nets.remove(bid);
            self.ids.remove(bid);
            self.docs.remove(bid);
            self.titles.remove(bid);
            self.map.remove(&bid.bref());
        }
    }
    /// Process a NodesRemoved event to clean up nets, docs, and titles
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
        if let Some(title) = self.titles.remove(from) {
            self.titles.insert(*to, title);
        };
        if let Some(pm) = self.map.remove(&from.bref()) {
            self.map.insert(to.bref(), pm);
        }
    }
}

/// [PathMap] generates unique relative paths between [crate::properties::BeliefNode]s based on the
/// graph structure for a particular [crate::properties::WeightKind] within a
/// [crate::beliefbase::BeliefBase::relations] hypergraph.
///
/// Since [crate::beliefbase::BeliefBase::relations] storeas a [crate::beliefbase::BidGraph]
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
/// all known belief networks. It plays a crucial role in generating table of content type
/// structures and navigating relative paths within a BeliefBase structure.
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

impl PathMap {
    pub fn new(
        kind: WeightKind,
        net: Bid,
        nets: &PathMapMap,
        relations: Arc<RwLock<BidGraph>>,
    ) -> PathMap {
        // Note this is reversed, because child edges are sorted based on the sink's weights for the
        // relations. A source without any sources floats (no dependencies), whereas a sink without
        // any sinks is a 'root', or 'main', or deepest abstraction node (depends on the deepest
        // relationships). We want to start our stack from the deepest abstraction nodes so that we
        // can sort their child stacks before inserting those stacks into the tree.
        let tree_graph = {
            let relations = relations.read_arc();
            relations.as_subgraph(kind, true)
        };
        let mut stack =
            BTreeMap::<Bid, (BTreeSet<Bid>, BTreeMap<Bid, (Vec<u16>, Vec<String>)>)>::new();
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
                    let (weight, paths) = tree_graph.edge_weight(sink, source).expect(
                        "Edge weight should exist since we received a DfsEvent for this relation",
                    );

                    // Handle multiple paths per relation
                    // Store ALL paths for this source in the sink's sub_paths
                    let all_paths = if !paths.is_empty() {
                        paths.clone()
                    } else {
                        let terminal_path =
                            generate_terminal_path(&source, &sink, None, *weight, nets);
                        // Anchorize the path if source is an anchor (adds # prefix)
                        let anchorized_path = nets.anchorize(&source, &terminal_path);
                        vec![anchorized_path]
                    };

                    // When the direct parent is the network root and the source is an
                    // anchor (heading/section), nest it under NETWORK_SECTION_SORT_KEY.
                    // The anchor's own sort key becomes the second element, placing it at
                    // [NETWORK_SECTION_SORT_KEY, anchor_idx] — fully non-colliding with
                    // document children at [doc_idx] (i.e. [0..NETWORK_SECTION_SORT_KEY-1]).
                    let sub_path_info = if sink == net && nets.is_anchor(&source) {
                        (vec![NETWORK_SECTION_SORT_KEY, *weight], all_paths)
                    } else {
                        (vec![*weight], all_paths)
                    };

                    stack.get_mut(&sink).map(|path_info| {
                        path_info.1.insert(source, sub_path_info);
                    }).expect("Never to encounter a sink edge prior to adding it to the stack during a DFS search");

                    let source_entry = stack
                        .entry(source)
                        .or_insert((BTreeSet::new(), BTreeMap::new()));
                    source_entry.0.insert(sink);

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

                            let (source_base_order, source_base_paths) =
                                sink_paths.get(&source).cloned().expect(
                                    "To have already mapped source to sink's sub-paths \
                                     during the DFS.",
                                );
                            for (bid, (path_order, sub_paths)) in source_sub_paths.iter() {
                                let mut sub_path_order = source_base_order.clone();
                                sub_path_order.extend(path_order);

                                // For each base path, join with each sub path
                                let mut joined_paths = Vec::new();
                                for base_path in source_base_paths.iter() {
                                    let base_ap = AnchorPath::from(base_path);
                                    for sub_path in sub_paths.iter() {
                                        joined_paths.push(base_ap.join(sub_path).into_string());
                                    }
                                }

                                sink_paths.insert(*bid, (sub_path_order, joined_paths));
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

        // tracing::debug!(
        //     "[PathMap::new] DFS completed for network {}. Found {} paths in inverted_path_map, {} loops detected",
        //     net,
        //     inverted_path_map.len(),
        //     loops.len()
        // );

        let mut map = Vec::from_iter(
            vec![
                (String::from(""), net, Vec::<u16>::default()),
                (
                    NETWORK_NAME.to_string(),
                    net,
                    vec![NETWORK_SECTION_SORT_KEY],
                ),
            ]
            .into_iter()
            .chain(
                inverted_path_map
                    .into_iter()
                    .flat_map(|(bid, (order, paths))| {
                        if nets.nets().contains(&bid) && bid != net && !subnets.contains(&bid) {
                            subnets.insert(bid);
                        }
                        // Generate a separate map entry for each path to this bid
                        paths
                            .into_iter()
                            .map(move |path| (path, bid, order.clone()))
                    }),
            ),
        );
        map.sort_by(|a, b| {
            let order_cmp = pathmap_order(&a.2, &b.2);
            match &order_cmp {
                Ordering::Equal => a.1.cmp(&b.1),
                _ => order_cmp,
            }
        });
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
            if let Some(title) = nets.titles().get(bid) {
                if !nets.is_anchor(bid) && !to_anchor(title).is_empty() {
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
        self.map.sort_by(|a, b| {
            let order_cmp = pathmap_order(&a.2, &b.2);
            match &order_cmp {
                Ordering::Equal => a.1.cmp(&b.1),
                _ => order_cmp,
            }
        });
    }

    pub fn map(&self) -> &Vec<(String, Bid, Vec<u16>)> {
        &self.map
    }

    pub fn subnets(&self) -> &BTreeSet<Bid> {
        &self.subnets
    }

    /// Returns the doc path and doc bid that contains the input path
    pub fn get_doc_from_id(
        &self,
        node: &Bid,
        nets: &PathMapMap,
    ) -> Option<(String, Bid, Vec<u16>)> {
        self.path(node, nets)
            .and_then(|(_home_net, path_ref, _order)| {
                let path_ap = AnchorPath::from(&path_ref);
                self.indexed_get(path_ap.filepath(), nets)
                    .map(|(_net, bid, order)| (path_ap.filepath().to_string(), bid, order))
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
                    nets.get_map(&net_bid.bref()).and_then(|subnet_path_map| {
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
                    nets.get_map(&net_bid.bref()).and_then(|subnet_path_map| {
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
                    nets.get_map(&net_bid.bref())
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
                let path_ap = AnchorPath::from(path);
                self.subnets.iter().find_map(|net_bid| {
                    let maybe_idx = self.bid_map.get(net_bid).and_then(|idx_vec| {
                        for idx in idx_vec.iter() {
                            let (subnet_path, _subnet_bid, _net_order) = &self.map[*idx];
                            if path.starts_with(subnet_path) {
                                return Some(idx);
                            }
                        }
                        None
                    });

                    let idx = maybe_idx?;

                    let (subnet_path, _subnet_bid, net_order) = &self.map[*idx];
                    let maybe_sub_path = path_ap
                        .strip_prefix(subnet_path)
                        .map(|sub_path| sub_path.to_string());
                    if let Some(sub_path) = maybe_sub_path {
                        nets.get_map(&net_bid.bref()).and_then(|subnet_path_map| {
                            subnet_path_map.indexed_get(&sub_path, nets).map(
                                |(home_net, bid, home_order)| {
                                    let mut full_order = net_order.clone();
                                    full_order.append(&mut home_order.clone());
                                    (home_net, bid, full_order)
                                },
                            )
                        })
                    } else {
                        None
                    }
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
                    let subnet_ap = AnchorPath::from(subnet_path);
                    nets.get_map(&net_bid.bref())
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
                                subnet_ap.join(&home_path).into_string(),
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
                nets.get_map(&subnet_bid.bref())
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

    /// Return a list of all paths connected to this subnet
    pub fn all_paths(&self, nets: &PathMapMap, visited: &mut BTreeSet<Bid>) -> Vec<String> {
        let mut paths = Vec::default();
        if visited.contains(&self.net) {
            return paths;
        }
        visited.insert(self.net);
        for (a_path, a_bid, _order) in self.map.iter() {
            if nets.nets().contains(a_bid) && !visited.contains(a_bid) {
                if let Some(sub_paths) = nets
                    .get_map(&a_bid.bref())
                    .map(|pm| pm.all_paths(nets, visited))
                {
                    let a_ap = AnchorPath::from(a_path);
                    for subnet_path in sub_paths.iter() {
                        paths.push(a_ap.join(subnet_path).into_string());
                    }
                }
            } else {
                paths.push(a_path.clone());
            }
        }

        paths
    }

    /// Return a list of all paths with their BIDs connected to this subnet
    pub fn all_paths_with_bids(
        &self,
        nets: &PathMapMap,
        visited: &mut BTreeSet<Bid>,
    ) -> Vec<(String, Bid)> {
        let mut paths = Vec::default();
        if visited.contains(&self.net) {
            return paths;
        }
        visited.insert(self.net);
        for (a_path, a_bid, _order) in self.map.iter() {
            if nets.nets().contains(a_bid) && !visited.contains(a_bid) {
                if let Some(sub_paths) = nets
                    .get_map(&a_bid.bref())
                    .map(|pm| pm.all_paths_with_bids(nets, visited))
                {
                    let a_ap = AnchorPath::from(a_path);
                    for (subnet_path, subnet_bid) in sub_paths.iter() {
                        paths.push((a_ap.join(subnet_path).into_string(), *subnet_bid));
                    }
                }
            } else {
                paths.push((a_path.clone(), *a_bid));
            }
        }

        paths
    }

    /// Return a list of all networks connected to this subnet (always includes self as ("", self.net))
    pub fn recursive_map(
        &self,
        nets: &PathMapMap,
        visited: &mut BTreeSet<Bid>,
    ) -> Vec<(String, Bid, Vec<u16>)> {
        let mut paths = Vec::default();
        if visited.contains(&self.net) {
            return paths;
        }
        visited.insert(self.net);
        let subnet_idxs = self
            .subnets()
            .iter()
            .map(|net_bid| {
                let idx_vec = self
                    .bid_map
                    .get(net_bid)
                    .expect("All nets to be in bid_map by construction");
                idx_vec[0]
            })
            .collect::<Vec<usize>>();

        for (idx, (elem_path, elem_bid, elem_order)) in self.map.iter().enumerate() {
            if subnet_idxs.contains(&idx) {
                let mut subs = nets
                    .get_map(&elem_bid.bref())
                    .map(|pm| pm.recursive_map(nets, visited))
                    .expect("all identified subnets to be registered with the pathmapmap");
                let sub_ap = AnchorPath::new(elem_path);
                for tuple in subs.iter_mut() {
                    tuple.0 = sub_ap.join(&tuple.0).into_string();
                    let mut new_order = elem_order.clone();
                    new_order.append(&mut tuple.2.clone());
                    tuple.2 = new_order;
                }
                paths.append(&mut subs);
            } else {
                paths.push((elem_path.clone(), *elem_bid, elem_order.clone()));
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
                } else if sink_order == order || (order.len() != sink_order.len() + 1 && direct) {
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
                            self.net.bref(),
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
        let mut sink_sub_indices = self.source_sub_indices(sink, false);
        if sink_sub_indices.is_empty() {
            return derivatives;
        }
        if nets.nets.contains(sink) && self.net != *sink {
            return derivatives;
        }

        // When sink is the network root, it has two entries in bid_map:
        //   - "" at order [] — parent for document children
        //   - "index.md" at order [NETWORK_SECTION_SORT_KEY] — parent for anchor/heading children
        //
        // Select only the entry appropriate for this source so that new_order is computed
        // from the correct base. Without this filter, both entries would contribute a
        // new_order and headings would incorrectly land at [heading_idx] instead of
        // [NETWORK_SECTION_SORT_KEY, heading_idx].
        if *sink == self.net && sink_sub_indices.len() > 1 {
            let source_is_anchor = nets.is_anchor(source);
            sink_sub_indices.retain(|(sink_index, _sub_indices)| {
                let sink_order = &self.map[*sink_index].2;
                if source_is_anchor {
                    // Headings belong under the "index.md" entry (order starts with NETWORK_SECTION_SORT_KEY)
                    sink_order.first() == Some(&NETWORK_SECTION_SORT_KEY)
                } else {
                    // Documents belong under the "" entry (empty order)
                    sink_order.is_empty()
                }
            });
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
                    self.net.bref(),
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
        let mut processed_path_set = BTreeSet::<String>::default();
        for (sink_index, sub_indices) in sink_sub_indices.iter().rev() {
            // Clone this so we don't keep a nonmutable reference into self.map;
            let (mut new_paths, new_order) = {
                let (sink_path, sink_bid, sink_order) = &self.map[*sink_index];
                let sink_ap = AnchorPath::from(sink_path);
                debug_assert!(*sink_bid == *sink);
                let mut new_order = sink_order.clone();
                new_order.push(new_idx);
                // Strip anchor from sink_path to avoid double anchors when generating child paths
                let sink_path_without_anchor = sink_ap.filepath();
                // Get all paths from the weight (new format supports multiple paths)
                let paths = new_weight.get_doc_paths();

                // Generate a path for each doc_path in the weight
                let new_paths: Vec<String> = if paths.is_empty() {
                    // No explicit paths, generate from anchor/index
                    vec![self.generate_path_name(
                        source,
                        sink,
                        sink_path_without_anchor,
                        None,
                        new_idx,
                        nets,
                    )]
                } else {
                    // Generate a unique path for each doc_path
                    paths
                        .iter()
                        .map(|p| {
                            self.generate_path_name(
                                source,
                                sink,
                                sink_path_without_anchor,
                                Some(p.clone()),
                                new_idx,
                                nets,
                            )
                        })
                        .collect()
                };
                (new_paths, new_order)
            };
            new_paths.sort();
            // Track which paths were filtered out by dedup so the update branch doesn't
            // mistake them for removals. A path already in processed_path_set was handled
            // by a prior iteration (e.g., a different parent entry for the same sink node)
            // and should not be removed from the map.
            let deduped_paths: BTreeSet<String> = new_paths
                .iter()
                .filter(|p| processed_path_set.contains(p.as_str()))
                .cloned()
                .collect();
            new_paths.retain(|path| !processed_path_set.contains(path));
            processed_path_set.append(&mut BTreeSet::from_iter(new_paths.iter().cloned()));

            let source_sub_indices = sub_indices
                .iter()
                .rev()
                .filter(|&idx| self.map[*idx].1 == *source)
                .copied()
                .collect::<Vec<usize>>();

            match source_sub_indices.is_empty() {
                true => {
                    // No existing entries for this source - add all paths as new entries
                    let mut insert_idx = sub_indices.last().copied().unwrap_or(*sink_index) + 1;
                    for new_path in new_paths {
                        let new_entry = (new_path.clone(), *source, new_order.clone());
                        derivatives.push(BeliefEvent::PathAdded(
                            self.net.bref(),
                            new_entry.0.clone(),
                            *source,
                            new_entry.2.clone(),
                            EventOrigin::Local,
                        ));
                        self.map.insert(insert_idx, new_entry);
                        insert_idx += 1;
                    }
                }
                false => {
                    // Update existing entries. Handle case where number of paths changed.
                    let old_entries: Vec<(usize, String, Vec<u16>)> = source_sub_indices
                        .iter()
                        .map(|idx| (*idx, self.map[*idx].0.clone(), self.map[*idx].2.clone()))
                        .collect();

                    // Get the old order from the first entry (all should have same order)
                    let old_order = &old_entries[0].2;

                    // Order vector length can change when document structure changes
                    if old_order.len() != new_order.len() {
                        tracing::warn!(
                            "[{}] Path order depth changed for source {}: old={:?}, new={:?}. \
                            This may require re-parsing dependent documents.",
                            self.net,
                            source,
                            old_order,
                            new_order
                        );
                    }

                    // Handle child path order updates if order changed
                    if *old_order != new_order {
                        // Find the first existing entry index
                        let first_idx = source_sub_indices[0];
                        let mut next_idx = first_idx + 1;
                        while next_idx < self.map.len() {
                            let next_order = &mut self.map[next_idx].2;
                            if !next_order.starts_with(old_order) {
                                break;
                            }
                            // Only update if lengths are compatible
                            if next_order.len() >= new_order.len() {
                                next_order[..new_order.len()].copy_from_slice(&new_order);
                            } else {
                                tracing::warn!(
                                    "[{}] Cannot update child path order - incompatible lengths",
                                    self.net
                                );
                            }
                            next_idx += 1;
                        }
                    }

                    // Compare old paths vs new paths
                    let old_paths: std::collections::BTreeSet<String> =
                        old_entries.iter().map(|(_, p, _)| p.clone()).collect();
                    let new_paths_set: std::collections::BTreeSet<String> =
                        new_paths.iter().cloned().collect();

                    // Paths to remove: in old but not in new, EXCLUDING paths that were
                    // filtered by processed_path_set dedup. Those paths were already handled
                    // by a prior iteration for a different parent entry of the same sink —
                    // they still belong in the map and must not be removed.
                    let paths_to_remove: Vec<String> = old_paths
                        .difference(&new_paths_set)
                        .filter(|p| !deduped_paths.contains(p.as_str()))
                        .cloned()
                        .collect();

                    // Paths to add: in new but not in old
                    let paths_to_add: Vec<String> =
                        new_paths_set.difference(&old_paths).cloned().collect();

                    // Remove old paths (in reverse to preserve indices)
                    for path_to_remove in paths_to_remove.iter() {
                        if let Some(_idx) = old_entries
                            .iter()
                            .find(|(_, p, _)| p == path_to_remove)
                            .map(|(i, _, _)| *i)
                        {
                            derivatives.push(BeliefEvent::PathsRemoved(
                                self.net.bref(),
                                vec![path_to_remove.clone()],
                                EventOrigin::Local,
                            ));
                        }
                    }

                    // Update existing paths that are kept (order or path may have changed)
                    for (old_idx, old_path, _) in old_entries.iter() {
                        if new_paths_set.contains(old_path) {
                            // Path is kept, update the order
                            self.map[*old_idx].2 = new_order.clone();
                            derivatives.push(BeliefEvent::PathUpdate(
                                self.net.bref(),
                                old_path.clone(),
                                *source,
                                new_order.clone(),
                                EventOrigin::Local,
                            ));
                        }
                    }

                    // Add new paths
                    if !paths_to_add.is_empty() {
                        let last_old_idx = source_sub_indices.last().copied().unwrap();
                        let mut insert_idx = last_old_idx + 1;
                        for new_path in paths_to_add {
                            let new_entry = (new_path.clone(), *source, new_order.clone());
                            derivatives.push(BeliefEvent::PathAdded(
                                self.net.bref(),
                                new_path,
                                *source,
                                new_order.clone(),
                                EventOrigin::Local,
                            ));
                            self.map.insert(insert_idx, new_entry);
                            insert_idx += 1;
                        }
                    }

                    // Remove entries marked for removal (reverse order to maintain indices)
                    for path_to_remove in paths_to_remove.iter().rev() {
                        if let Some(pos) = self
                            .map
                            .iter()
                            .position(|(p, b, _)| p == path_to_remove && b == source)
                        {
                            self.map.remove(pos);
                        }
                    }
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
                if let Some(source_title) = nets.titles.get(source) {
                    if !nets.is_anchor(source) && !to_anchor(source_title).is_empty() {
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
