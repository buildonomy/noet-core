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
    collections::{BTreeMap, BTreeSet, HashMap, VecDeque},
    fmt,
    sync::Arc,
};

use crate::{
    beliefbase::BidGraph,
    codec::NETWORK_NAME,
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
            .or_else(|| {
                // Use the stored id only if it is not a pure bref-fallback.
                // When id == bref, the author never set a meaningful id; prefer the
                // title-anchor for a human-readable path instead.
                nets.ids
                    .get(source)
                    .filter(|id| id.as_str() != source.bref().to_string().as_str())
                    .cloned()
            })
            .or_else(|| {
                nets.titles
                    .get(source)
                    .map(|t| to_anchor(t))
                    .filter(|s| !s.is_empty())
            })
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

/// Topologically sort the `RelationUpdate` / `RelationRemoved` events within `events`
/// so that a parent edge (sink→its_parent) is always processed before any child edge
/// whose source equals that sink.
///
/// # Why this is necessary
///
/// `PathMap::process_relation_update` requires the sink (parent) node to already have
/// an entry in the PathMap before it can insert the source (child).  If a child edge
/// arrives before its parent edge in the same batch, `sink_sub_indices` will be empty
/// and the insertion is silently dropped.
///
/// # Algorithm
///
/// Treat each relation event as a node in a dependency DAG.  Draw a dependency edge
/// from event E1 to event E2 when `E1.source == E2.sink` — meaning E1 establishes the
/// node that E2 needs as its parent.  Run Kahn's BFS to produce a topological ordering
/// of the relation events; roots (events whose sink is not produced by any other event
/// in this batch) are emitted first.
///
/// Non-relation events are passed through unchanged at their original positions; only
/// the relative order of `RelationUpdate`/`RelationRemoved` events changes.
///
/// Cycles (which would leave some events with `in_degree > 0` after BFS) are appended
/// in their original order after all acyclic events and a warning is emitted.
///
/// # Return value
///
/// Returns `Some(sorted)` when the relation events needed reordering, or `None` when the
/// original slice is already correct (no intra-batch dependencies, or fewer than 2 relation
/// events).  The `None` path performs no heap allocation beyond the initial scan.
fn sort_relation_events<'a>(events: &[&'a BeliefEvent]) -> Option<Vec<&'a BeliefEvent>> {
    // Collect relation events with their original slot index in `events`.
    let relation_events: Vec<(usize, &'a BeliefEvent)> = events
        .iter()
        .enumerate()
        .filter(|(_, e)| {
            matches!(
                e,
                BeliefEvent::RelationUpdate(..) | BeliefEvent::RelationRemoved(..)
            )
        })
        .map(|(i, e)| (i, *e))
        .collect();

    if relation_events.len() < 2 {
        // Nothing to reorder.
        return None;
    }

    // Helper: extract (source, sink) from a relation event.
    let relation_bids = |e: &BeliefEvent| -> (Bid, Bid) {
        match e {
            BeliefEvent::RelationUpdate(src, snk, ..)
            | BeliefEvent::RelationRemoved(src, snk, ..) => (*src, *snk),
            _ => unreachable!("non-relation event in relation_events"),
        }
    };

    // source_set: every BID that appears as a source (child) in this batch.
    // An event whose sink is in source_set depends on whoever establishes that sink.
    let source_set: HashMap<Bid, usize> = relation_events
        .iter()
        .enumerate()
        .map(|(local_idx, (_, e))| (relation_bids(e).0, local_idx))
        .collect();

    // Fast path: the input is already a valid topological order when every dependency
    // edge (i → j) flows forward (i < j).  This covers both the no-dependencies case
    // (source_set never matches any sink) and the already-ordered case, without
    // allocating sink_to_dependents or in_degree.
    let already_ordered = relation_events
        .iter()
        .enumerate()
        .all(|(local_idx, (_, e))| {
            let (_, sink) = relation_bids(e);
            // If this event's sink is itself a source in this batch, the event that
            // establishes it (at source_set[sink]) must appear before us (i < local_idx).
            source_set
                .get(&sink)
                .is_none_or(|&establishes_at| establishes_at < local_idx)
        });
    if already_ordered {
        return None;
    }

    // Build Kahn's in-degree and adjacency.
    // sink_to_dependents[S] = local indices of events that need S to be established first
    //                         (i.e. events whose sink == S).
    let mut sink_to_dependents: HashMap<Bid, Vec<usize>> = HashMap::new();
    let mut in_degree: Vec<usize> = vec![0usize; relation_events.len()];

    for (local_idx, (_, e)) in relation_events.iter().enumerate() {
        let (_, sink) = relation_bids(e);
        if source_set.contains_key(&sink) {
            // This event must wait until the event that establishes `sink` has run.
            in_degree[local_idx] += 1;
            sink_to_dependents.entry(sink).or_default().push(local_idx);
        }
    }

    // Kahn's BFS: start with all events whose sink is already established (in_degree == 0).
    let mut queue: VecDeque<usize> = in_degree
        .iter()
        .enumerate()
        .filter(|(_, &d)| d == 0)
        .map(|(i, _)| i)
        .collect();

    let mut sorted_local: Vec<usize> = Vec::with_capacity(relation_events.len());
    while let Some(local_idx) = queue.pop_front() {
        sorted_local.push(local_idx);
        let (source, _) = relation_bids(relation_events[local_idx].1);
        // Establishing `source` unblocks any event whose sink == source.
        if let Some(deps) = sink_to_dependents.get(&source) {
            for &dep_idx in deps {
                if in_degree[dep_idx] > 0 {
                    in_degree[dep_idx] -= 1;
                    if in_degree[dep_idx] == 0 {
                        queue.push_back(dep_idx);
                    }
                }
            }
        }
    }

    // Any events not yet emitted are part of a cycle; append in original order.
    if sorted_local.len() < relation_events.len() {
        let visited: BTreeSet<usize> = sorted_local.iter().copied().collect();
        let cycle_participants: Vec<usize> = (0..relation_events.len())
            .filter(|i| !visited.contains(i))
            .collect();
        tracing::warn!(
            "[sort_relation_events] detected {} cyclic relation event(s); appending in \
             original order. Involved sources: {:?}",
            cycle_participants.len(),
            cycle_participants
                .iter()
                .map(|&i| relation_bids(relation_events[i].1).0)
                .collect::<Vec<_>>(),
        );
        sorted_local.extend(cycle_participants);
    }

    // Reassemble: walk `events` in order; whenever we encounter a relation event,
    // replace it with the next event from `sorted_local` (which re-emits the same
    // set of relation events in dependency order).  Non-relation events are kept
    // at their original positions unchanged.
    let mut sorted_local_iter = sorted_local.iter();
    Some(
        events
            .iter()
            .map(|e| match e {
                BeliefEvent::RelationUpdate(..) | BeliefEvent::RelationRemoved(..) => {
                    let local_idx = sorted_local_iter
                        .next()
                        .expect("sorted_local must have one entry per relation event");
                    relation_events[*local_idx].1
                }
                _ => *e,
            })
            .collect(),
    )
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
    /// Reverse index: node BID → set of network Brefs whose PathMap contains that node.
    ///
    /// Used by `process_event_queue` to route `RelationUpdate`/`RelationRemoved` events
    /// only to the PathMaps that actually contain the source or sink node, rather than
    /// broadcasting to all O(N_networks) PathMaps. For most relations this reduces the
    /// fan-out from O(N_networks) to O(1).
    ///
    /// Maintained by:
    /// - `rebuild_node_to_nets`: called after a PathMap is (re)constructed.
    /// - `process_nodes_removed`: removes entries for deleted nodes.
    /// - `process_node_renamed`: remaps entries under the new BID.
    node_to_nets: BTreeMap<Bid, BTreeSet<Bref>>,
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
            node_to_nets: BTreeMap::default(),
        };
        let api_pm = PathMap::new(WeightKind::Section, root, &pmm, relations);
        pmm.rebuild_node_to_nets_for(&root.bref(), &api_pm);
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
    /// Rebuild the `node_to_nets` reverse-index entries for a single network's PathMap.
    ///
    /// Called after any `PathMap` is constructed or replaced.  Removes all stale entries
    /// for `net_bref` then re-inserts one entry per node currently in the PathMap's
    /// `bid_map`.
    fn rebuild_node_to_nets_for(&mut self, net_bref: &Bref, pm: &PathMap) {
        // Remove stale entries for this network from the reverse index.
        self.node_to_nets.values_mut().for_each(|nets| {
            nets.remove(net_bref);
        });
        // Prune now-empty entries to keep the map lean.
        self.node_to_nets.retain(|_, nets| !nets.is_empty());
        // Insert fresh entries for every node currently in this PathMap.
        for bid in pm.bid_map.keys() {
            self.node_to_nets.entry(*bid).or_default().insert(*net_bref);
        }
        // The network node itself is always reachable by its own PathMap.
        self.node_to_nets
            .entry(pm.net)
            .or_default()
            .insert(*net_bref);
    }

    #[tracing::instrument(skip(states, relations))]
    pub fn new(states: &BTreeMap<Bid, BeliefNode>, relations: Arc<RwLock<BidGraph>>) -> PathMapMap {
        // tracing::debug!(
        //     "[PathMapMap::new] Creating PathMapMap with {} states, {} relations",
        //     states.len(),
        //     relations.read_arc().as_graph().edge_count()
        // );
        let mut pmm = PathMapMap {
            relations: relations.clone(),
            node_to_nets: BTreeMap::default(),
            ..Default::default()
        };
        for node in states.values() {
            pmm.titles.insert(node.bid, node.title.clone());
            pmm.ids.insert(node.bid, node.id());
            if node.kind.contains(BeliefKind::API) {
                pmm.apis.insert(node.bid);
            }

            if node.kind.is_network() {
                pmm.nets.insert(node.bid);
            }

            if node.kind.is_document() || node.kind.is_external() {
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
        pmm.node_to_nets.clear();

        // Collect nets to avoid holding an immutable borrow on pmm.nets while calling
        // rebuild_node_to_nets_for (which takes &mut self).
        let nets_to_build: Vec<Bid> = pmm.nets.iter().copied().collect();
        for net in nets_to_build {
            if !pmm.map.contains_key(&net.bref()) {
                let pm = PathMap::new(WeightKind::Section, net, &pmm, relations.clone());
                // tracing::debug!(
                //     "[PathMapMap::new] Created PathMap for network {}: {} entries",
                //     net,
                //     pm.map().len()
                // );
                pmm.rebuild_node_to_nets_for(&net.bref(), &pm);
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
            .filter_map(|pm| pm.read_arc().path(bid, self))
            .min_by(|a, b| pathmap_order(&a.2, &b.2).then_with(|| a.1.cmp(&b.1)))
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
        // Tracks which network Brefs had their PathMap mutated this call, so the
        // sort pass at the end only touches dirty PathMaps.
        let mut dirty_nets: BTreeSet<Bref> = BTreeSet::new();
        // Derivative path events collected per net, used after the sort pass to
        // update node_to_nets incrementally from PathAdded/PathsRemoved signals.
        let mut net_derivatives: BTreeMap<Bref, Vec<BeliefEvent>> = BTreeMap::new();

        // Non-network nodes whose id or title changed this batch. Collected in pass 1
        // (single deserialise per event) so pass 2 can drive targeted path-string
        // regeneration in affected PathMaps without a full PathMap rebuild.
        // Value is the BID of the node whose path segment needs refreshing.
        let mut segment_dirty_bids: Vec<Bid> = Vec::new();

        // Pass 1: populate titles, docs, nets, and ids from all NodeUpdate/NodeRemoved events
        // before rebuilding any PathMaps. This ensures that when a network NodeUpdate triggers
        // PathMap::new in pass 2, every node referenced in the relations graph (including
        // freshly-generated href nodes) already has a title entry — preventing the Issue 34
        // "source has no title entry in nets" error caused by processing NodeUpdate(href_namespace)
        // before NodeUpdate(href_node) when both arrive in the same event queue.
        for event in events {
            match event {
                BeliefEvent::NodeUpdate(_, toml_str, _) => {
                    if let Ok(node) = BeliefNode::try_from(&toml_str[..]) {
                        // Detect id/title changes for non-network nodes so pass 2 can
                        // update only their path segments in affected PathMaps.
                        if !node.kind.is_network() {
                            let new_id = node.id();
                            let new_title = node.title.clone();
                            let id_changed =
                                self.ids.get(&node.bid).is_some_and(|old| *old != new_id);
                            let title_changed = self
                                .titles
                                .get(&node.bid)
                                .is_some_and(|old| *old != new_title);
                            if id_changed || title_changed {
                                segment_dirty_bids.push(node.bid);
                            }
                        }
                        self.titles.insert(node.bid, node.title.clone());
                        self.ids.insert(node.bid, node.id());
                        if node.kind.contains(BeliefKind::API) {
                            self.apis.insert(node.bid);
                        }
                        if node.kind.is_network() {
                            self.nets.insert(node.bid);
                        }
                        if node.kind.is_document() || node.kind.is_external() {
                            self.docs.insert(node.bid);
                        }
                    }
                }
                BeliefEvent::NodesRemoved(bids, _) => {
                    self.process_nodes_removed(bids);
                }
                _ => {}
            }
        }

        // Pass 2: rebuild network PathMaps and apply relation updates, now that titles is complete.
        // Nodes from prior merge_graph_mut calls (e.g. href_node added when an earlier file was
        // parsed) are already in self.titles because merge_graph_mut now fires NodeUpdate events
        // through the PathMapMap for every newly-added state. No backfill needed here.
        for event in events {
            if let BeliefEvent::NodeUpdate(_, toml_str, _) = event {
                if let Ok(node) = BeliefNode::try_from(&toml_str[..]) {
                    // Rebuild the PathMap for newly registered network nodes. All source nodes
                    // referenced in relations are already in self.titles (from pass 1 for nodes
                    // in this batch, from merge_graph_mut's NodeUpdate events for prior batches).
                    //
                    // Guard against Trace network nodes only when a PathMap already exists:
                    // a Trace node is balance scaffolding emitted by to_event_stream to satisfy
                    // process_relation_update's sink-must-exist requirement. If a PathMap already
                    // exists for this network (built from a prior complete NodeUpdate), skip the
                    // rebuild — the existing PathMap is correct and the O(session_bb) DFS would
                    // only produce a stale result. If no PathMap exists yet, build one even for
                    // a Trace node so that child RelationUpdate events can find the sink.
                    let pm_already_exists = self.map.contains_key(&node.bid.bref());
                    if node.kind.is_network()
                        && !(node.kind.contains(BeliefKind::Trace) && pm_already_exists)
                    {
                        let pm =
                            PathMap::new(WeightKind::Section, node.bid, self, relations.clone());
                        self.rebuild_node_to_nets_for(&node.bid.bref(), &pm);
                        dirty_nets.insert(node.bid.bref());
                        self.map.insert(node.bid.bref(), Arc::new(RwLock::new(pm)));
                    }
                }
            }
        }

        // Drive targeted path-segment regeneration for non-network nodes that changed
        // their id or title this batch (e.g. document-beats-anchor id clobber in
        // insert_state). `self.ids` was updated in pass 1, so each call reads the new id.
        for bid in &segment_dirty_bids {
            let target_nets: Vec<Bref> = self
                .node_to_nets
                .get(bid)
                .map(|s| s.iter().copied().collect())
                .unwrap_or_default();
            for net_bref in &target_nets {
                if let Some(pm_lock) = self.map.get(net_bref) {
                    let mut pm = pm_lock.write();
                    let events = pm.update_path_segment(bid, self);
                    if !events.is_empty() {
                        dirty_nets.insert(*net_bref);
                        net_derivatives
                            .entry(*net_bref)
                            .or_default()
                            .extend(events.iter().cloned());
                        path_events.extend(events);
                    }
                }
            }
        }

        // Sort RelationUpdate/RelationRemoved events so that a parent edge is always
        // processed before any child edge that depends on it.  Non-relation events keep
        // their original relative positions.  See `sort_relation_events` for details.
        // Returns None (zero allocation) when no reordering is needed.
        let sorted_storage;
        let sorted_events: &[&BeliefEvent] = match sort_relation_events(events) {
            Some(sorted) => {
                sorted_storage = sorted;
                sorted_storage.as_slice()
            }
            None => events,
        };
        for event in sorted_events {
            match event {
                BeliefEvent::NodeUpdate(..) => {}
                BeliefEvent::NodeRenamed(from, to, _) => {
                    self.process_node_renamed(from, to);
                    // NodeRenamed affects every PathMap that contains the old BID.
                    // Use the reverse index if available; fall back to full scan if the
                    // renamed node was not yet indexed (shouldn't happen in practice).
                    let target_nets: Vec<Bref> = self
                        .node_to_nets
                        .get(from)
                        .map(|s| s.iter().copied().collect())
                        .unwrap_or_else(|| self.map.keys().cloned().collect());
                    for net_bref in &target_nets {
                        if let Some(pm_lock) = self.map.get(net_bref) {
                            let mut pm = pm_lock.write();
                            let events = pm.process_event(event, self);
                            if !events.is_empty() {
                                dirty_nets.insert(*net_bref);
                                net_derivatives
                                    .entry(*net_bref)
                                    .or_default()
                                    .extend(events.iter().cloned());
                                path_events.extend(events);
                            }
                        }
                    }
                }
                BeliefEvent::RelationUpdate(source, sink, ..)
                | BeliefEvent::RelationRemoved(source, sink, ..) => {
                    // Route to only the PathMaps that contain source or sink.
                    // For most relations this is exactly one network; it is at most the
                    // union of two small sets — O(1) vs the previous O(N_networks) fan-out.
                    //
                    // We also include any PathMap whose network node IS the sink, because
                    // a network node always has an entry in its own PathMap and
                    // process_relation_update uses self.net == *sink to gate path insertion.
                    let mut candidate_nets: BTreeSet<Bref> = BTreeSet::new();
                    if let Some(nets) = self.node_to_nets.get(source) {
                        candidate_nets.extend(nets);
                    }
                    if let Some(nets) = self.node_to_nets.get(sink) {
                        candidate_nets.extend(nets);
                    }
                    // Instrumentation: log routing state for RelationUpdates where
                    // source is itself a network node (subnet→parent registration path).
                    // Log unconditionally for this case so we can see what's in
                    // self.nets and node_to_nets regardless of whether sink is already
                    // registered as a network.
                    if self.nets.contains(source) {
                        tracing::debug!(
                            target: "noet_core::paths::subnet_registration",
                            source = %source,
                            sink = %sink,
                            source_is_net = true,
                            sink_in_self_nets = self.nets.contains(sink),
                            sink_in_self_map = self.map.contains_key(&sink.bref()),
                            source_in_node_to_nets = self.node_to_nets.contains_key(source),
                            sink_in_node_to_nets = self.node_to_nets.contains_key(sink),
                            candidate_nets = ?candidate_nets,
                            node_to_nets_for_source = ?self.node_to_nets.get(source),
                            node_to_nets_for_sink = ?self.node_to_nets.get(sink),
                            nets_contains_sink = self.nets.contains(sink),
                            "routing RelationUpdate: source is a network node (subnet→parent path)",
                        );
                    }
                    for net_bref in &candidate_nets {
                        if let Some(pm_lock) = self.map.get(net_bref) {
                            let mut pm = pm_lock.write();
                            let events = pm.process_event(event, self);
                            if !events.is_empty() {
                                if matches!(**event, BeliefEvent::RelationUpdate(..)) {
                                    // Keep node_to_nets in sync within the batch: a
                                    // RelationUpdate that produces derivatives has just
                                    // inserted `source` into this PathMap.  Register it
                                    // now so that subsequent events in the same sorted
                                    // batch can route to this PathMap when `source`
                                    // appears as a sink.  `sink` was already in
                                    // node_to_nets — that's how this event was routed
                                    // here in the first place.
                                    self.node_to_nets
                                        .entry(*source)
                                        .or_default()
                                        .insert(*net_bref);
                                }
                                dirty_nets.insert(*net_bref);
                                net_derivatives
                                    .entry(*net_bref)
                                    .or_default()
                                    .extend(events.iter().cloned());
                                path_events.extend(events);
                            }
                        }
                    }
                }
                // A RelationChange results in a derivative RelationUpdate if it materially changes
                // the sets relations. Therefore, only handle the relation update to remove
                // redundant processing.
                // BeliefEvent::RelationChange(source, sink, kind, weight, _) => {}
                // PathsAdded/PathsRemoved are derivative events - we don't process them
                // NodeRenamed, RelationRemoved, BatchEnd - handled elsewhere or ignored
                _ => {}
            }
        }
        // (pass 2 loop closed above)

        // Sort-only pass for dirty PathMaps.
        //
        // process_relation_update already rebuilds bid_map/path_map/order_map internally
        // whenever it produces derivatives (L1955–1967). The only thing still needed here
        // is pm.sort(): process_relation_update appends new entries at the end of self.map
        // without respecting WEIGHT_SORT_KEY order, so source_sub_indices (which walks
        // self.map linearly by order prefix) would give wrong results on the next call
        // within the same batch if we skipped the sort.
        //
        // After sorting, update node_to_nets from the derivative events collected above:
        // - PathAdded(net_bref, _, bid, ..)  → bid is now in net_bref's PathMap.
        // - PathsRemoved(net_bref, ..)       → no BID info; fall back to full rebuild.
        // - PathUpdate                        → no membership change, skip.
        for net_bref in &dirty_nets {
            let Some(pm_lock) = self.map.get(net_bref) else {
                continue;
            };
            {
                let mut pm = pm_lock.write();
                // Skip the sort and index rebuild when the map is already in sorted order.
                // process_relation_update rebuilds the index maps after appending/updating
                // entries, so if positions haven't changed (already sorted) those indexes
                // are still valid.  is_sorted_by is O(N) with no allocations — cheap enough
                // to always check before paying for an O(N log N) sort + O(N) index rebuild.
                let already_sorted = pm.map.is_sorted_by(|a, b| {
                    matches!(
                        pathmap_order(&a.2, &b.2).then(a.1.cmp(&b.1)),
                        Ordering::Less | Ordering::Equal
                    )
                });
                if !already_sorted {
                    pm.sort();
                    // Rebuild index maps after sort. pm.sort() changes the position of every
                    // entry in self.map, so all index positions computed by process_relation_update
                    // are now stale. This is the single authoritative rebuild point.
                    // Collect entries first to satisfy the borrow checker (can't borrow
                    // pm.map immutably while mutating pm.bid_map/path_map/order_map).
                    let entries: Vec<(String, Bid, Vec<u16>)> = pm
                        .map
                        .iter()
                        .map(|(path, bid, order)| (path.clone(), *bid, order.clone()))
                        .collect();
                    pm.bid_map.clear();
                    pm.path_map.clear();
                    pm.order_map.clear();
                    for (idx, (path, bid, order)) in entries.iter().enumerate() {
                        pm.bid_map.entry(*bid).or_default().push(idx);
                        pm.path_map.insert(path.clone(), idx);
                        pm.order_map.insert(order_key(order), idx);
                    }
                    // Keep subnets consistent with the freshly rebuilt bid_map: a subnet
                    // whose relation was just removed no longer has a bid_map entry and must
                    // be evicted so recursive_map doesn't look it up and fail.
                    let stale_subnets: Vec<Bid> = pm
                        .subnets
                        .iter()
                        .filter(|bid| !pm.bid_map.contains_key(bid))
                        .copied()
                        .collect();
                    for bid in stale_subnets {
                        pm.subnets.remove(&bid);
                    }
                    // pm write guard dropped here.
                } // end if !already_sorted
            }

            // Update node_to_nets from derivative events for this net.
            // This must run regardless of whether the sort was needed: PathAdded events
            // are collected during the relation-routing pass above and must be reflected
            // in node_to_nets so that subsequent process_event_queue calls (for deeper
            // section nodes in the same Phase 1 push loop) can route their RelationUpdate
            // events to the correct PathMap via the reverse index.
            // If any PathsRemoved arrived we can't cheaply infer which BIDs left, so
            // fall back to a full rebuild from bid_map for this net only.
            let derivs = net_derivatives
                .get(net_bref)
                .map(|v| v.as_slice())
                .unwrap_or(&[]);
            let has_removal = derivs
                .iter()
                .any(|e| matches!(e, BeliefEvent::PathsRemoved(..)));

            if has_removal {
                // Full rebuild for this net: wipe its stale entries then re-insert.
                self.node_to_nets.values_mut().for_each(|nets| {
                    nets.remove(net_bref);
                });
                self.node_to_nets.retain(|_, nets| !nets.is_empty());
                let bids: Vec<Bid> = pm_lock.read().bid_map.keys().copied().collect();
                for bid in bids {
                    self.node_to_nets.entry(bid).or_default().insert(*net_bref);
                }
            } else {
                // Incremental update: PathAdded tells us exactly which BID was inserted.
                for event in derivs {
                    if let BeliefEvent::PathAdded(_, _, bid, ..) = event {
                        self.node_to_nets.entry(*bid).or_default().insert(*net_bref);
                    }
                }
            }
        }

        path_events
    }

    /// Process a NodesRemoved event to clean up nets, docs, titles, and the reverse index.
    pub fn process_nodes_removed(&mut self, bids: &[Bid]) {
        for bid in bids {
            self.nets.remove(bid);
            self.ids.remove(bid);
            self.docs.remove(bid);
            self.titles.remove(bid);
            // Remove this node from the reverse index entirely.
            self.node_to_nets.remove(bid);
            // If the removed node was a network, remove its PathMap and evict it from
            // all other nodes' reverse-index sets.
            let net_bref = bid.bref();
            if self.map.remove(&net_bref).is_some() {
                self.node_to_nets.values_mut().for_each(|nets| {
                    nets.remove(&net_bref);
                });
                self.node_to_nets.retain(|_, nets| !nets.is_empty());
            }
        }
    }

    /// Process a NodeRenamed event to update nets, docs, titles, and the reverse index.
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
        // Remap the PathMap key if this node was a network.
        if let Some(pm) = self.map.remove(&from.bref()) {
            self.map.insert(to.bref(), pm);
        }
        // Remap the reverse-index entry for the renamed node itself.
        if let Some(net_set) = self.node_to_nets.remove(from) {
            self.node_to_nets.insert(*to, net_set);
        }
        // If this node was a network, all other nodes that referenced it by Bref need
        // their set entries updated from from.bref() → to.bref().
        let from_bref = from.bref();
        let to_bref = to.bref();
        if from_bref != to_bref {
            for nets in self.node_to_nets.values_mut() {
                if nets.remove(&from_bref) {
                    nets.insert(to_bref);
                }
            }
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
    /// Index from order-vec (serialised as "sk1.sk2.sk3") to map index. Allows O(log N)
    /// ancestor lookup by order prefix during stack reconstruction.
    order_map: BTreeMap<String, usize>,
    id_map: IdMap,
    title_map: IdMap,
    kind: WeightKind,
    net: Bid,
    subnets: BTreeSet<Bid>,
    pub loops: BTreeSet<(Bid, Bid)>,
}

/// Serialise an order vec to a dot-joined string key suitable for `PathMap::order_map`.
#[inline]
fn order_key(order: &[u16]) -> String {
    order
        .iter()
        .map(|sk| sk.to_string())
        .collect::<Vec<_>>()
        .join(".")
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
            relations.as_subgraph_seeded(kind, true, net)
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
                    if nets.titles().get(&source).is_none() {
                        tracing::error!(
                            "[PathMap::new] ISSUE 34: source {} has no title entry in nets (sink={}, paths={:?})",
                            source, sink, paths
                        );
                    }

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
                    //
                    // Paths for these anchors are stored as "index.md#slug" rather than
                    // the bare "#slug" produced by anchorize(). The bare form causes two
                    // downstream failures:
                    //   1. AnchorPath::join("#slug") treats the empty path as a no-op and
                    //      returns the base unchanged, dropping the anchor entirely when
                    //      recursive_map joins subnet paths.
                    //   2. get_nav_tree cannot distinguish a bare slug from a path segment,
                    //      making correct href reconstruction impossible.
                    // Storing "index.md#slug" means normalize_path_extension converts it to
                    // "index.html#slug" and recursive_map's join produces correct full paths
                    // (e.g. "subnet1/index.html#slug") automatically.
                    let sub_path_info = if sink == net && nets.is_anchor(&source) {
                        let prefixed_paths = all_paths
                            .iter()
                            .map(|p| {
                                // p is "#slug" from anchorize(); prepend NETWORK_NAME so the
                                // path is self-contained: "index.md#slug".
                                format!("{NETWORK_NAME}{p}")
                            })
                            .collect::<Vec<_>>();
                        (vec![NETWORK_SECTION_SORT_KEY, *weight], prefixed_paths)
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
                                    // Use new_dir() so that dotted directory names (e.g.
                                    // "symbol.iterator") are treated as directory components
                                    // in join(), not as files with extension "iterator".
                                    // Without this, AnchorPath::from("symbol.iterator") sets
                                    // ext_sep and dir() returns "", silently dropping the
                                    // directory component when joining sub-paths.
                                    let base_ap = AnchorPath::new_dir(base_path);
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
                Ordering::Equal => a.0.cmp(&b.0),
                _ => order_cmp,
            }
        });
        let mut bid_map = BTreeMap::new();
        let mut path_map = BTreeMap::new();
        let mut order_map = BTreeMap::new();
        for (idx, (path, bid, order)) in map.iter().enumerate() {
            let bid_idx_vec = bid_map.entry(*bid).or_insert(Vec::<usize>::default());
            bid_idx_vec.push(idx);
            path_map.insert(path.clone(), idx);
            order_map.insert(order_key(order), idx);
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
            order_map,
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
                Ordering::Equal => a.0.cmp(&b.0),
                _ => order_cmp,
            }
        });
    }

    pub fn map(&self) -> &Vec<(String, Bid, Vec<u16>)> {
        &self.map
    }

    /// Look up the PathMap entry whose order vec equals `order`.
    /// Returns `(bid, path)` in O(log N) via the `order_map` index.
    /// Returns `None` if no entry has that exact order (e.g. the order is empty,
    /// which corresponds to the repo-root network itself and is not stored in any
    /// child PathMap entry).
    pub fn order_for(&self, order: &[u16]) -> Option<(Bid, &str)> {
        if order.is_empty() {
            return None;
        }
        self.order_map.get(&order_key(order)).map(|&idx| {
            let (path, bid, _) = &self.map[idx];
            (*bid, path.as_str())
        })
    }

    /// Look up the PathMap entry for `bid` and return its `(order, path)`.
    /// Uses `bid_map` for O(log N) index lookup, then reads the order vec from
    /// `self.map`. Returns the first entry when a BID maps to multiple paths
    /// (e.g. a subnet network that appears at more than one mount point).
    pub fn order_for_bid(&self, bid: &Bid) -> Option<(&Vec<u16>, &str)> {
        let &idx = self.bid_map.get(bid)?.first()?;
        let (path, _, order) = &self.map[idx];
        Some((order, path.as_str()))
    }

    /// Returns true if `bid` has any entry in this PathMap's `bid_map` index.
    /// Ownership-free alternative to `order_for_bid` for use in diagnostic checks.
    pub fn bid_has_path(&self, bid: &Bid) -> bool {
        self.bid_map.contains_key(bid)
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
        self.bid_map
            .get(bid)
            .and_then(|idx_vec| idx_vec.first().copied())
            .map(|idx| {
                let (path, _bid, order) = &self.map()[idx];
                (self.net, path.clone(), order.clone())
            })
            .or_else(|| {
                self.subnets.iter().find_map(|net_bid| {
                    let first_idx = self
                        .bid_map
                        .get(net_bid)
                        .and_then(|idx_vec| idx_vec.first().copied())
                        .expect("pathmap subnets to be synchronized with pathmap.bid_map");
                    let (subnet_path, _subnet_bid, net_order) = &self.map[first_idx];
                    // Use new_dir() so that dotted directory names (e.g. "symbol.iterator")
                    // are treated as directory components in join(), not as files with
                    // extension "iterator". Without new_dir(), AnchorPath::from("symbol.iterator")
                    // sets ext_sep and dir() returns "", causing the directory component to be
                    // silently dropped when joining sub-paths like "index.md#syntax" — producing
                    // "index.md#syntax" instead of the correct "symbol.iterator/index.md#syntax".
                    let subnet_ap = AnchorPath::new_dir(subnet_path);
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
                    // Use new_dir() so that dotted directory names (e.g. "symbol.iterator")
                    // are treated as directory components in join(), not as files.
                    let a_ap = AnchorPath::new_dir(a_path);
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

    /// Return a list of all networks connected to this subnet (always includes self as ("",
    /// self.net)) If entry is some, starts the map at the first index of that bid entry.
    pub fn recursive_map(
        &self,
        entry: Option<Bid>,
        nets: &PathMapMap,
        visited: &mut BTreeSet<Bid>,
    ) -> Vec<(String, Bid, Vec<u16>)> {
        let mut paths = Vec::default();
        if visited.contains(&self.net) {
            return paths;
        }
        let start_idx = match entry {
            Some(entry) => {
                let Some(start_idx) = self.bid_map.get(&entry).and_then(|starts| starts.first())
                else {
                    // Bid may be in a subnet, find out, and if it is run a recursive map on the
                    // home net for that bid, prepending the subnet path and subnet order onto the
                    // results.
                    if let Some((home_net, _path, _order)) = self.path(&entry, nets) {
                        debug_assert!(
                            home_net != self.net,
                            "If we don't have a bid_map index for entry, its path isn't in this map"
                        );
                        let Some(home_net_pm) = nets.get_map(&home_net.bref()) else {
                            tracing::warn!(
                                "Found bid with a recursive path search but not its home net"
                            );
                            return paths;
                        };
                        let Some((_home_net, path_to_home_net, entry_home_order)) =
                            self.path(&home_net, nets)
                        else {
                            tracing::warn!(
                                "Found bid with a recursive path search but not its home net"
                            );
                            return paths;
                        };
                        let to_home_ap = AnchorPath::new_dir(&path_to_home_net);
                        return home_net_pm
                            .recursive_map(Some(entry), nets, visited)
                            .into_iter()
                            .map(|(path, bid, mut order)| {
                                let mut full_order = entry_home_order.clone();
                                full_order.append(&mut order);
                                (to_home_ap.join(path).into_string(), bid, full_order)
                            })
                            .collect::<Vec<_>>();
                    }
                    return paths;
                };
                *start_idx
            }
            None => 0,
        };
        let order_prefix = self.map[start_idx].2.clone();
        visited.insert(self.net);

        for idx in start_idx..self.map.len() {
            let (elem_path, elem_bid, elem_order) = &self.map[idx];
            if elem_order[..order_prefix.len()] != order_prefix {
                break;
            }
            if self.subnets().contains(elem_bid) && !visited.contains(elem_bid) {
                let mut subs = nets
                    .get_map(&elem_bid.bref())
                    .map(|pm| pm.recursive_map(None, nets, visited))
                    .expect("all identified subnets to be registered with the pathmapmap");
                // Use new_dir() so that dotted directory names (e.g. "symbol.iterator")
                // are treated as directory components in join(), not as files.
                let sub_ap = AnchorPath::new_dir(elem_path);
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

    /// Regenerate path strings for all entries belonging to `bid` in this PathMap.
    ///
    /// Called when the node's id or title has changed in `nets` (e.g. after the
    /// document-beats-anchor id-clobber in `insert_state`). `nets.ids` and
    /// `nets.titles` are already up-to-date before this is called.
    ///
    /// For each entry `(old_path, bid, order)` in `self.map`:
    /// 1. Derive the parent's path from `order[..order.len()-1]` via `order_map`.
    /// 2. Re-run `generate_path_name` with the updated `nets`.
    /// 3. If the path string changed, update `self.map`, `self.path_map`, and
    ///    `self.bid_map` (order vecs hold indices; path_map keys change but the
    ///    indices themselves do not), and emit `PathUpdate` derivatives.
    pub(crate) fn update_path_segment(&mut self, bid: &Bid, nets: &PathMapMap) -> Vec<BeliefEvent> {
        let Some(indices) = self.bid_map.get(bid).cloned() else {
            return vec![];
        };
        let mut derivatives = Vec::new();
        for idx in indices {
            let (old_path, _bid, order) = &self.map[idx];
            let old_path = old_path.clone();
            let order = order.clone();

            // Derive the sort index (last element of order) and find the parent entry.
            let Some(&sort_idx) = order.last() else {
                // This is the network root entry — it has no terminal segment to regenerate.
                continue;
            };
            let parent_order = &order[..order.len() - 1];
            // The network root ("" entry) has order [] and is not in order_map, but its
            // path is "". The "index.md" entry has order [NETWORK_SECTION_SORT_KEY] and IS
            // in order_map. For direct children of the network root, parent_order is [].
            let parent_path: String = if parent_order.is_empty() {
                String::new()
            } else {
                match self.order_for(parent_order) {
                    Some((_parent_bid, p)) => p.to_string(),
                    None => {
                        tracing::warn!(
                            "[update_path_segment] net={} bid={}: cannot find parent for order={:?}",
                            self.net, bid, order
                        );
                        continue;
                    }
                }
            };

            // Find the sink BID: look for the entry at parent_order (that's the sink).
            // For direct children of the network root the sink is self.net itself.
            let sink_bid: Bid = if parent_order.is_empty() {
                self.net
            } else {
                match self.order_for(parent_order) {
                    Some((parent_bid, _)) => parent_bid,
                    None => continue,
                }
            };

            // Strip anchor from parent path before generating child path (mirrors
            // process_relation_update which calls sink_ap.filepath() for this purpose).
            let sink_ap = crate::paths::AnchorPath::from(parent_path.as_str());
            let sink_path_without_anchor = sink_ap.filepath().to_string();

            let new_path = self.generate_path_name(
                bid,
                &sink_bid,
                &sink_path_without_anchor,
                None,
                sort_idx,
                nets,
            );

            if new_path == old_path {
                continue;
            }

            tracing::debug!(
                "[update_path_segment] net={} bid={}: \"{}\" -> \"{}\"",
                self.net,
                bid,
                old_path,
                new_path
            );

            // Update path_map: remove old key, insert new key (index unchanged).
            self.path_map.remove(&old_path);
            self.path_map.insert(new_path.clone(), idx);
            self.map[idx].0 = new_path.clone();

            // Update id_map and title_map entries for this bid.
            // IdMap is keyed by path-string → Bid (id_to_bid) with reverse bid → path.
            // If this bid had an entry, remove the old path key and insert the new one.
            if self.id_map.get_id(bid).is_some() {
                self.id_map.remove(bid);
                self.id_map.insert(new_path.clone(), *bid);
            }
            if self.title_map.get_id(bid).is_some() {
                self.title_map.remove(bid);
                self.title_map.insert(new_path.clone(), *bid);
            }

            derivatives.push(BeliefEvent::PathUpdate(
                self.net.bref(),
                new_path,
                *bid,
                order,
                crate::event::EventOrigin::Local,
            ));
        }
        derivatives
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
                    // order_map values are indices into self.map; the BidReplace event only
                    // changes the Bid stored at those indices, not the order vecs, so
                    // order_map keys/values remain valid — no rebuild needed here.
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
        // Block non-network source nodes from being inserted into a foreign network's
        // PathMap via node_to_nets routing (e.g. a doc in subnet_A must not appear in
        // the root PathMap via RelationUpdate(doc, subnet_A)).
        //
        // Exception: when source is itself a network node (subnet linking up to its
        // parent), allow it through — this mirrors what PathMap::new's DFS does via
        // Control::Prune: record the subnet as a child in ancestor PathMaps but don't
        // recurse into the subnet's own children.
        if nets.nets.contains(sink) && self.net != *sink && !nets.nets.contains(source) {
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
                self.order_map.clear();
                for (idx, (path, bid, order)) in self.map.iter().enumerate() {
                    let bid_idx_vec = self.bid_map.entry(*bid).or_default();
                    bid_idx_vec.push(idx);
                    self.path_map.insert(path.clone(), idx);
                    self.order_map.insert(order_key(order), idx);
                }
                // Keep subnets consistent with bid_map: a subnet whose path was just removed no
                // longer has a bid_map entry and must be evicted from subnets to prevent
                // recursive_map from looking it up and failing.
                self.subnets.retain(|bid| self.bid_map.contains_key(bid));
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

                    // Update existing paths if their order changed
                    for (old_idx, old_path, old_order) in old_entries.iter() {
                        if new_paths_set.contains(old_path) && *old_order != new_order {
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
            // Rebuild all three index maps after any insertions or removals so that
            // the *next* process_relation_update call within the same Pass 2 batch
            // reads valid indices. self.map.insert() shifts all subsequent positions
            // by +1, making every stale index point at the wrong entry — this triggers
            // the debug_assert!(*sink_bid == *sink) guard at the top of the next call
            // for bid_map, and would return wrong results for order_for() (order_map)
            // and any path_map consumer between two calls in the same batch.
            //
            // The removal branch (above) already does a full three-map rebuild after
            // self.map.remove(). This block makes the insert/update branch consistent
            // with it. The sort pass at the end of process_event_queue does another
            // full rebuild after pm.sort() reorders everything — that remains the
            // single authoritative post-sort rebuild point.
            self.bid_map.clear();
            self.path_map.clear();
            self.order_map.clear();
            for (idx, (path, bid, order)) in self.map.iter().enumerate() {
                self.bid_map.entry(*bid).or_default().push(idx);
                self.path_map.insert(path.clone(), idx);
                self.order_map.insert(order_key(order), idx);
            }

            // subnets.retain (which needs bid_map to be current) is also deferred to
            // the sort pass for the same reason — see process_event_queue.

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
