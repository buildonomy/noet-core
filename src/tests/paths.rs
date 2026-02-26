//! Tests for path update and reindexing logic

use super::helpers::*;
use crate::{
    beliefbase::{BeliefBase, BidGraph},
    event::BeliefEvent,
    nodekey::NodeKey,
    paths::{to_anchor, PathMapMap, NETWORK_SECTION_SORT_KEY},
    properties::{
        BeliefKind, BeliefKindSet, BeliefNode, Bid, Bref, Weight, WeightKind, WeightSet,
        WEIGHT_SORT_KEY,
    },
};
use parking_lot::RwLock;
use std::{collections::BTreeMap, collections::BTreeSet, sync::Arc};
use test_log::test;

#[test]
fn test_relation_removal_triggers_reindexing() {
    // Start with a balanced test set
    let mut set = create_balanced_test_beliefbase();

    // Get the parent doc and children from the set
    let parent_doc = set
        .get(&NodeKey::Id {
            net: Bref::default(),
            id: to_anchor("Parent Document"),
        })
        .unwrap();
    let child2 = set
        .get(&NodeKey::Id {
            net: Bref::default(),
            id: to_anchor("Child 2"),
        })
        .unwrap();

    // Verify initial state is balanced
    let errors = set.built_in_test(true);
    assert!(
        errors.is_empty(),
        "Initial state should be balanced:\n{}",
        errors.join("\n")
    );

    // Remove child2 (middle element with index 1)
    let remove_event =
        BeliefEvent::NodesRemoved(vec![child2.bid], crate::event::EventOrigin::Remote);
    let derivative_events = set.process_event(&remove_event).unwrap();

    // Verify child3 was reindexed from 2 to 1
    let relations = set.relations();
    let parent_idx = set.bid_to_index(&parent_doc.bid).unwrap();
    let edges: Vec<_> = relations
        .as_graph()
        .edges_directed(parent_idx, petgraph::Direction::Incoming)
        .collect();

    assert_eq!(edges.len(), 2, "Should have 2 remaining edges");

    // Check that indices are contiguous [0, 1]
    let mut indices = edges
        .iter()
        .filter_map(|e| {
            e.weight()
                .get(&WeightKind::Section)
                .and_then(|w| w.get::<u16>(WEIGHT_SORT_KEY))
        })
        .collect::<Vec<_>>();
    indices.sort();
    assert_eq!(indices, vec![0, 1], "Indices should be reindexed to [0, 1]");

    // Verify there were derivative events for the reindexing
    assert!(
        !derivative_events.is_empty(),
        "Should have derivative events for reindexing"
    );

    // Verify set is still balanced after removal
    let final_errors = set.built_in_test(false);
    assert!(
        final_errors.is_empty(),
        "Final state should be balanced: {final_errors:?}"
    );
}

#[test]
fn test_parent_reindex_updates_child_order_vectors() {
    // Start with a balanced test set
    let mut set = create_balanced_test_beliefbase();

    // Add a grandchild to test order vector propagation
    let child1 = set
        .states()
        .values()
        .find(|n| n.title == "Child 1")
        .unwrap()
        .clone();
    let child3 = set
        .states()
        .values()
        .find(|n| n.title == "Child 3")
        .unwrap()
        .clone();

    // Add a grandchild under child1
    let grandchild = create_test_node("Grandchild", BeliefKind::Document);
    let grandchild_bid = grandchild.bid;

    let insert_event = BeliefEvent::NodeUpdate(
        vec![],
        toml::to_string(&grandchild).unwrap(),
        crate::event::EventOrigin::Remote,
    );
    set.process_event(&insert_event).unwrap();

    // Create relation: grandchild -> child3
    let grandchild_weight = Weight::default();
    let mut grandchild_weights = WeightSet::empty();
    grandchild_weights.set(WeightKind::Section, grandchild_weight);

    let insert_relation_event = BeliefEvent::RelationChange(
        grandchild_bid,
        child3.bid,
        WeightKind::Section,
        grandchild_weights.get(&WeightKind::Section).cloned(),
        crate::event::EventOrigin::Remote,
    );
    set.process_event(&insert_relation_event).unwrap();

    // Get initial grandchild order vector from PathMap
    let paths = set.paths();
    let net_bref = paths
        .nets()
        .iter()
        .find(|bid| **bid != set.api().bid)
        .cloned()
        .unwrap()
        .bref();

    let pm = paths.get_map(&net_bref).unwrap();
    let initial_grandchild_order = pm
        .map()
        .iter()
        .find(|(_, bid, _)| *bid == grandchild_bid)
        .map(|(_, _, order)| order.clone());
    assert!(
        initial_grandchild_order.is_some(),
        "grandchild should be in initial PathMap"
    );
    let initial_order = initial_grandchild_order.unwrap();
    drop(pm);
    drop(paths);

    // Change child3's index from 2 to 1 by removing child1
    let update_event =
        BeliefEvent::NodesRemoved(vec![child1.bid], crate::event::EventOrigin::Remote);
    set.process_event(&update_event).unwrap();

    // Get final grandchild order vector
    let paths = set.paths();
    let pm = paths.get_map(&net_bref).unwrap();
    let final_grandchild_order = pm
        .map()
        .iter()
        .find(|(_, bid, _)| *bid == grandchild_bid)
        .map(|(_, _, order)| order.clone());
    assert!(
        final_grandchild_order.is_some(),
        "grandchild should still be in PathMap after reorder"
    );
    let final_order = final_grandchild_order.unwrap();

    // The second element (parent's index in grandchild's order vector) should have changed from 2 to 1
    // (because reindexing happens after child1 was removed, so child3 ends up at index 1)
    assert_eq!(
        initial_order.len(),
        final_order.len(),
        "Order vector length should not change"
    );
    assert_ne!(
        initial_order[1], final_order[1],
        "Parent's index in grandchild's order vector should have changed"
    );
}

#[test]
fn test_event_driven_pathmap_matches_constructor() {
    // Start with a balanced test set
    let mut set = create_balanced_test_beliefbase();

    // Get references to nodes for manipulation
    let child1 = set
        .states()
        .values()
        .find(|n| n.title == "Child 1")
        .unwrap()
        .clone();
    let parent_doc = set
        .states()
        .values()
        .find(|n| n.title == "Parent Document")
        .unwrap()
        .clone();

    // Process some events to mutate the PathMapMap
    // Change child1's index from 0 to 2
    let mut new_weight = Weight::default();
    new_weight.set(WEIGHT_SORT_KEY, 2u16).ok();
    let mut new_weights = WeightSet::empty();
    new_weights.set(WeightKind::Section, new_weight);

    let update_event = BeliefEvent::RelationUpdate(
        child1.bid,
        parent_doc.bid,
        new_weights,
        crate::event::EventOrigin::Remote,
    );
    set.process_event(&update_event).unwrap();

    // Get event-driven paths
    let paths_event = set.paths();
    let event_all_paths = paths_event.all_paths();
    let event_paths: BTreeSet<String> = event_all_paths
        .values()
        .flatten()
        .map(|(path, _, _)| path.clone())
        .collect();

    // Create fresh PathMapMap from constructor with same states/relations
    let relations_guard = set.relations();
    let relations_arc = Arc::new(RwLock::new(relations_guard.clone()));
    let paths_constructor = crate::paths::PathMapMap::new(set.states(), relations_arc);

    let constructor_all_paths = paths_constructor.all_paths();
    let constructor_paths: BTreeSet<String> = constructor_all_paths
        .values()
        .flatten()
        .map(|(path, _, _)| path.clone())
        .collect();

    let paths_eq = event_paths == constructor_paths;
    assert!(
        paths_eq,
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
    );

    // Compare metadata
    assert_eq!(
        paths_event.nets().len(),
        paths_constructor.nets().len(),
        "nets metadata should match"
    );
    assert_eq!(
        paths_event.docs().len(),
        paths_constructor.docs().len(),
        "docs metadata should match"
    );
    assert_eq!(
        paths_event.titles().len(),
        paths_constructor.titles().len(),
        "anchors metadata should match"
    );
}

#[test]
fn test_pathmap_multiple_paths_per_relation() {
    // Create a BeliefBase with a relation that has multiple paths
    let mut set = create_balanced_test_beliefbase();

    // Get the parent document and child from the balanced set
    let parent_doc = set
        .get(&NodeKey::Id {
            net: Bref::default(),
            id: to_anchor("Parent Document"),
        })
        .unwrap()
        .clone();

    let child = set
        .get(&NodeKey::Id {
            net: Bref::default(),
            id: to_anchor("Child 1"),
        })
        .unwrap()
        .clone();

    // Update the existing relation with multiple paths (simulating symlinks or multiple references)
    let mut weight = Weight::default();
    weight.set(WEIGHT_SORT_KEY, 0u16).unwrap();
    weight
        .set_doc_paths(vec![
            "path_a.txt".to_string(),
            "sym_link_to_a.txt".to_string(),
            "another_ref_to_a.txt".to_string(),
        ])
        .unwrap();

    // Use RelationChange (not RelationUpdate) so that generate_edge_update merges paths correctly
    let event = BeliefEvent::RelationChange(
        child.bid,
        parent_doc.bid,
        WeightKind::Section,
        Some(weight),
        crate::event::EventOrigin::Remote,
    );
    set.process_event(&event).unwrap();

    // Get the network that the parent_doc belongs to
    let network = set
        .get(&NodeKey::Id {
            net: Bref::default(),
            id: to_anchor("Test Network"),
        })
        .unwrap();

    // Get the PathMap for the Test Network (where parent_doc is a member)
    let paths = set.paths();
    let path_map = paths.get_map(&network.bid.bref()).unwrap();

    // Verify that all three paths exist in the PathMap
    let child_entries: Vec<_> = path_map
        .map()
        .iter()
        .filter(|(_, bid, _)| *bid == child.bid)
        .collect();

    assert_eq!(
        child_entries.len(),
        3,
        "PathMap should contain 3 entries for the same child with different paths. Found {} entries",
        child_entries.len()
    );

    // Verify each path is unique and contains our expected paths
    let paths_set: BTreeSet<String> = child_entries.iter().map(|(p, _, _)| (*p).clone()).collect();
    assert_eq!(
        paths_set.len(),
        3,
        "All three paths should be unique in the PathMap"
    );

    // Verify the paths contain our expected values (with parent prefix)
    assert!(
        paths_set.contains("parent-document/path_a.txt"),
        "PathMap should contain parent-document/path_a.txt"
    );
    assert!(
        paths_set.contains("parent-document/sym_link_to_a.txt"),
        "PathMap should contain parent-document/sym_link_to_a.txt"
    );
    assert!(
        paths_set.contains("parent-document/another_ref_to_a.txt"),
        "PathMap should contain parent-document/another_ref_to_a.txt"
    );

    // Verify all entries have the same order (since they're from the same relation)
    let orders: BTreeSet<Vec<u16>> = child_entries.iter().map(|(_, _, o)| (*o).clone()).collect();
    assert_eq!(
        orders.len(),
        1,
        "All paths for the same relation should have the same order vector"
    );
}

/// Build a BeliefBase containing a network node, two document children, and two anchor
/// (heading/section) children — the minimal structure needed to test sort-space separation.
///
/// Graph structure:
///   api
///   └── net (Network)          — connected via Section weight 0 to api
///       ├── doc_a (Document)   — Section weight 0  → should land at order [0]
///       ├── doc_b (Document)   — Section weight 1  → should land at order [1]
///       ├── anchor_x (Symbol)  — Section weight 0  → should land at order [MAX, 0]
///       └── anchor_y (Symbol)  — Section weight 1  → should land at order [MAX, 1]
///
/// Anchors (headings) are identified by NOT being in `PathMapMap::docs()`.
/// Documents are identified by being in `PathMapMap::docs()`.
fn create_network_with_docs_and_anchors() -> BeliefBase {
    init_logging();

    let mut states = BTreeMap::new();

    let api = BeliefNode::api_state();
    states.insert(api.bid, api.clone());

    // Network node
    let net = BeliefNode {
        bid: Bid::new(api.bid),
        title: "Test Network".to_string(),
        kind: BeliefKindSet(BeliefKind::Network.into()),
        id: Some(to_anchor("test-network")),
        ..Default::default()
    };
    states.insert(net.bid, net.clone());

    // Document children — these go into PathMapMap::docs()
    let doc_a = BeliefNode {
        bid: Bid::new(net.bid),
        title: "Doc A".to_string(),
        kind: BeliefKindSet(BeliefKind::Document.into()),
        id: Some(to_anchor("doc-a")),
        ..Default::default()
    };
    let doc_b = BeliefNode {
        bid: Bid::new(net.bid),
        title: "Doc B".to_string(),
        kind: BeliefKindSet(BeliefKind::Document.into()),
        id: Some(to_anchor("doc-b")),
        ..Default::default()
    };
    states.insert(doc_a.bid, doc_a.clone());
    states.insert(doc_b.bid, doc_b.clone());

    // Anchor children (Symbol kind — not Document, so is_anchor() returns true)
    let anchor_x = BeliefNode {
        bid: Bid::new(net.bid),
        title: "Heading X".to_string(),
        kind: BeliefKindSet(BeliefKind::Symbol.into()),
        id: Some(to_anchor("heading-x")),
        ..Default::default()
    };
    let anchor_y = BeliefNode {
        bid: Bid::new(net.bid),
        title: "Heading Y".to_string(),
        kind: BeliefKindSet(BeliefKind::Symbol.into()),
        id: Some(to_anchor("heading-y")),
        ..Default::default()
    };
    states.insert(anchor_x.bid, anchor_x.clone());
    states.insert(anchor_y.bid, anchor_y.clone());

    let mut edges = Vec::new();

    // net -> api (Section 0)
    let mut w = Weight::default();
    w.set(WEIGHT_SORT_KEY, 0u16).ok();
    let mut ws = WeightSet::empty();
    ws.set(WeightKind::Section, w);
    edges.push((net.bid, api.bid, ws));

    // doc_a -> net (Section 0), doc_b -> net (Section 1)
    for (idx, doc) in [&doc_a, &doc_b].iter().enumerate() {
        let mut w = Weight::default();
        w.set(WEIGHT_SORT_KEY, idx as u16).ok();
        w.set_doc_paths(vec![format!("doc_{}.md", (b'a' + idx as u8) as char)])
            .ok();
        let mut ws = WeightSet::empty();
        ws.set(WeightKind::Section, w);
        edges.push((doc.bid, net.bid, ws));
    }

    // anchor_x -> net (Section 0), anchor_y -> net (Section 1)
    for (idx, anchor) in [&anchor_x, &anchor_y].iter().enumerate() {
        let mut w = Weight::default();
        w.set(WEIGHT_SORT_KEY, idx as u16).ok();
        w.set_doc_paths(vec![format!("#heading-{}", (b'x' + idx as u8) as char)])
            .ok();
        let mut ws = WeightSet::empty();
        ws.set(WeightKind::Section, w);
        edges.push((anchor.bid, net.bid, ws));
    }

    let relations = BidGraph::from_edges(&edges);
    BeliefBase::new(states, relations).unwrap()
}

/// Assert the sort-space invariant for a PathMap of the given network:
///
///   - Documents (in `docs`) are children of the network at order `[doc_idx]`
///   - Anchors (not in `docs`) are children of the network at order `[NETWORK_SECTION_SORT_KEY, anchor_idx]`
///   - The two ranges are fully non-overlapping
///   - No document order starts with NETWORK_SECTION_SORT_KEY
///   - No anchor order has length 1 (they must be nested under the sentinel)
fn assert_network_sort_space_invariant(set: &BeliefBase, net_bid: Bid, label: &str) {
    let paths = set.paths();
    let pm = paths
        .get_map(&net_bid.bref())
        .unwrap_or_else(|| panic!("{label}: could not get PathMap for network {net_bid}"));

    let docs = paths.docs();

    let mut doc_orders: Vec<Vec<u16>> = Vec::new();
    let mut anchor_orders: Vec<Vec<u16>> = Vec::new();

    for (path, bid, order) in pm.map().iter() {
        // Skip the network root entries themselves ("" and "index.md")
        if *bid == net_bid {
            continue;
        }
        if docs.contains(bid) {
            doc_orders.push(order.clone());
            assert!(
                order.first() != Some(&NETWORK_SECTION_SORT_KEY),
                "{label}: document '{path}' (bid={bid}) has order {order:?} which starts \
                 with NETWORK_SECTION_SORT_KEY ({NETWORK_SECTION_SORT_KEY}) — \
                 documents must not be in the reserved section sort space"
            );
        } else {
            anchor_orders.push(order.clone());
            assert_eq!(
                order.first(),
                Some(&NETWORK_SECTION_SORT_KEY),
                "{label}: anchor '{path}' (bid={bid}) has order {order:?} — \
                 anchors/headings must be in the reserved sort space \
                 [NETWORK_SECTION_SORT_KEY={NETWORK_SECTION_SORT_KEY}, *]"
            );
            assert!(
                order.len() >= 2,
                "{label}: anchor '{path}' (bid={bid}) has order {order:?} with length < 2 — \
                 anchors must be nested under the NETWORK_SECTION_SORT_KEY sentinel"
            );
        }
    }

    // At least one doc and one anchor must have been found for the test to be meaningful
    assert!(
        !doc_orders.is_empty(),
        "{label}: no document entries found in PathMap — test setup is wrong"
    );
    assert!(
        !anchor_orders.is_empty(),
        "{label}: no anchor entries found in PathMap — test setup is wrong"
    );

    // The two order sets must be fully disjoint
    let doc_order_set: BTreeSet<Vec<u16>> = doc_orders.into_iter().collect();
    let anchor_order_set: BTreeSet<Vec<u16>> = anchor_orders.into_iter().collect();
    let overlap: BTreeSet<_> = doc_order_set.intersection(&anchor_order_set).collect();
    assert!(
        overlap.is_empty(),
        "{label}: doc and anchor order vectors overlap: {overlap:?}"
    );
}

/// Test that document children of a network land at sort orders `[doc_idx]` and anchor
/// (heading/section) children land at `[NETWORK_SECTION_SORT_KEY, anchor_idx]`, keeping
/// the two sort spaces non-colliding.
///
/// Exercises both the constructor path (`PathMap::new` via `PathMapMap::new`) and the
/// incremental event-driven path (`process_relation_update` via `BeliefBase::process_event`).
#[test]
fn test_network_section_sort_key_reservation() {
    let set = create_network_with_docs_and_anchors();

    // Identify the network BID
    let net_bid = set
        .states()
        .values()
        .find(|n| n.kind.contains(BeliefKind::Network) && n.bid != set.api().bid)
        .map(|n| n.bid)
        .expect("test network node must exist");

    // ── 1. Constructor path ────────────────────────────────────────────────────
    // PathMapMap::new rebuilds all PathMaps from scratch via DFS.
    // This exercises the `effective_weight` override in the TreeEdge handler.
    assert_network_sort_space_invariant(&set, net_bid, "constructor");

    // ── 2. BeliefBase invariant ───────────────────────────────────────────────
    // built_in_test(true) checks edge sort keys. For network nodes it verifies
    // docs and anchors are each independently contiguous (not globally contiguous),
    // so the [0, 1] doc keys and [0, 1] anchor keys both satisfy the invariant.
    let errors = set.built_in_test(true);
    assert!(
        errors.is_empty(),
        "BeliefBase invariants must hold after network section sort key setup:\n{}",
        errors.join("\n")
    );

    // ── 3. Event-driven (incremental) path ────────────────────────────────────
    // Re-emit a RelationChange for one doc and one anchor to drive
    // process_relation_update, then re-check the sort-space invariant.
    // This exercises the sink_sub_indices filter added to process_relation_update.
    let mut set = set; // make mutable

    let doc_a = set
        .states()
        .values()
        .find(|n| n.title == "Doc A")
        .unwrap()
        .clone();
    let anchor_x = set
        .states()
        .values()
        .find(|n| n.title == "Heading X")
        .unwrap()
        .clone();

    // Re-issue the doc_a relation (same sort key, same path) — forces process_relation_update
    let mut w = Weight::default();
    w.set(WEIGHT_SORT_KEY, 0u16).ok();
    w.set_doc_paths(vec!["doc_a.md".to_string()]).ok();
    set.process_event(&BeliefEvent::RelationChange(
        doc_a.bid,
        net_bid,
        WeightKind::Section,
        Some(w),
        crate::event::EventOrigin::Remote,
    ))
    .unwrap();

    // Re-issue the anchor_x relation (same sort key)
    let mut w = Weight::default();
    w.set(WEIGHT_SORT_KEY, 0u16).ok();
    w.set_doc_paths(vec!["#heading-x".to_string()]).ok();
    set.process_event(&BeliefEvent::RelationChange(
        anchor_x.bid,
        net_bid,
        WeightKind::Section,
        Some(w),
        crate::event::EventOrigin::Remote,
    ))
    .unwrap();

    assert_network_sort_space_invariant(&set, net_bid, "event-driven");

    // Verify event-driven PathMap still matches a fresh constructor PathMap
    let relations_guard = set.relations();
    let relations_arc = Arc::new(RwLock::new(relations_guard.clone()));
    let paths_constructor = PathMapMap::new(set.states(), relations_arc);

    let constructor_all_paths = paths_constructor.all_paths();
    let constructor_paths: BTreeSet<String> = constructor_all_paths
        .values()
        .flatten()
        .map(|(path, _, _)| path.clone())
        .collect();

    let event_all_paths = set.paths().all_paths();
    let event_paths: BTreeSet<String> = event_all_paths
        .values()
        .flatten()
        .map(|(path, _, _)| path.clone())
        .collect();

    assert_eq!(
        event_paths,
        constructor_paths,
        "Event-driven and constructor PathMaps must agree after incremental update.\n\
         event_only: {:?}\n\
         constructor_only: {:?}",
        event_paths
            .difference(&constructor_paths)
            .collect::<Vec<_>>(),
        constructor_paths
            .difference(&event_paths)
            .collect::<Vec<_>>(),
    );
}
