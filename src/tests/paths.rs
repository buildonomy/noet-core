//! Tests for path update and reindexing logic

use super::helpers::*;
use crate::{
    beliefbase::{BeliefBase, BidGraph},
    event::BeliefEvent,
    nodekey::NodeKey,
    paths::{serialize_order, to_anchor, PathMap, PathMapMap, NETWORK_SECTION_SORT_KEY},
    properties::{
        BeliefKind, BeliefKindSet, BeliefNode, Bid, Bref, NodeId, Weight, WeightKind, WeightSet,
        WEIGHT_SORT_KEY,
    },
};
use parking_lot::RwLock;
use petgraph::visit::{EdgeRef, IntoEdgeReferences};
use rustc_hash::FxHashMap;
use std::{collections::BTreeSet, sync::Arc};
use test_log::test;

/// Assert that `pm`'s `bid_map`/`order_map`/`path_map` exactly match what a
/// from-scratch scan of `pm.map()` would produce.
///
/// This is the safety net for the `map_insert`/`map_remove`/`patch_order_in_place`
/// helpers in `PathMap::process_relation_update`, which patch indices in place
/// instead of clearing and rebuilding all index maps after every mutation.
/// A bug in any of those helpers (an off-by-one shift, a stale key left behind,
/// etc.) would show up here as a mismatch against the ground-truth scan, even if
/// the resulting `path` strings all happen to be correct (which is all the
/// existing `test_event_driven_pathmap_matches_constructor` checks).
///
/// `path_map` is covered here as well: a desynchronised path index does not
/// crash, it silently fails to resolve a link, so it needs a ground-truth check
/// that runs in the default test suite.
fn assert_pathmap_indices_consistent(pm: &PathMap, label: &str) {
    let mut expected_bid_map: FxHashMap<Bid, Vec<usize>> = FxHashMap::default();
    let mut expected_order_map: FxHashMap<String, usize> = FxHashMap::default();
    // `path_map` is scalar: one path resolves to one entry. Last writer wins,
    // matching `rebuild_indices`.
    let mut expected_path_map: FxHashMap<String, usize> = FxHashMap::default();
    for (idx, (path, bid, order)) in pm.map().iter().enumerate() {
        expected_bid_map.entry(*bid).or_default().push(idx);
        expected_order_map.insert(serialize_order(order), idx);
        expected_path_map.insert(path.clone(), idx);
    }
    assert_eq!(
        pm.bid_map(),
        &expected_bid_map,
        "{label}: bid_map diverged from a from-scratch scan of pm.map()"
    );
    assert_eq!(
        pm.order_map(),
        &expected_order_map,
        "{label}: order_map diverged from a from-scratch scan of pm.map()"
    );
    assert_eq!(
        pm.path_map(),
        &expected_path_map,
        "{label}: path_map diverged from a from-scratch scan of pm.map()"
    );
}

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
    let errors = set.built_in_test();
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
    let final_errors = set.check_path_invariants();
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
        grandchild.clone(),
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

    // The incremental map_insert/map_remove/patch_order_in_place helpers in
    // process_relation_update must keep bid_map/order_map exactly in
    // sync with pm.map() -- not just "the path strings happen to match", which
    // is all the paths_eq assertion below actually checks.
    {
        let paths_event = set.paths();
        let net_bref = paths_event
            .nets()
            .iter()
            .find(|bid| **bid != set.api().bid)
            .cloned()
            .unwrap()
            .bref();
        let pm = paths_event.get_map(&net_bref).unwrap();
        assert_pathmap_indices_consistent(&pm, "after child1 reorder");
    }

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

/// Repeatedly append new children under the same sink, simulating how a
/// const-namespace PathMap (e.g. href_namespace/asset_namespace) grows across
/// the whole session as more documents are parsed/merged — always adding new
/// entries at (or near) the tail rather than replacing existing ones.
///
/// This exercises `map_insert`'s common-case path (append) directly and checks
/// that `bid_map`/`order_map` stay exactly consistent with `pm.map()`
/// after every single insertion, not just at the end.
#[test]
fn test_incremental_append_keeps_indices_consistent() {
    let mut set = create_balanced_test_beliefbase();

    let network = set
        .states()
        .values()
        .find(|n| n.title == "Test Network")
        .unwrap()
        .clone();

    let net_bref = network.bid.bref();

    // Append 10 new document children under the network root, one event at a
    // time, each with a strictly increasing sort key so every insertion lands
    // at the tail — the common case for ever-growing namespace PathMaps.
    for i in 0..10u16 {
        let new_doc = create_test_node(&format!("Appended Doc {i}"), BeliefKind::Document);
        let insert_event =
            BeliefEvent::NodeUpdate(vec![], new_doc.clone(), crate::event::EventOrigin::Remote);
        set.process_event(&insert_event).unwrap();

        // Existing children (Parent Document at 0) occupy index 0, so start
        // appended docs at sort key 1 and up.
        let mut w = Weight::default();
        w.set(WEIGHT_SORT_KEY, 1u16 + i).ok();
        let relation_event = BeliefEvent::RelationChange(
            new_doc.bid,
            network.bid,
            WeightKind::Section,
            Some(w),
            crate::event::EventOrigin::Remote,
        );
        set.process_event(&relation_event).unwrap();

        let paths = set.paths();
        let pm = paths.get_map(&net_bref).unwrap();
        assert_pathmap_indices_consistent(&pm, &format!("after appending doc {i}"));
    }

    // Final sanity check: all 10 appended docs plus the original parent doc
    // are present with distinct paths.
    let paths = set.paths();
    let pm = paths.get_map(&net_bref).unwrap();
    let doc_count = pm
        .map()
        .iter()
        .filter(|(_, bid, _)| {
            set.states()
                .get(bid)
                .is_some_and(|n| n.title.starts_with("Appended Doc"))
        })
        .count();
    assert_eq!(doc_count, 10, "all 10 appended docs should be present");
}

/// The collision check in `generate_path_name_with_collision_check`
/// resolves candidates through the `path_map` index instead of scanning the whole
/// map. The index must return exactly the entries a scan would have matched, for
/// every path present *and* absent, or a collision is silently missed (producing a
/// duplicate path) or falsely reported (producing a spurious bref fallback).
///
/// This asserts that equivalence directly against a ground-truth scan, rather than
/// inferring it from the absence of downstream symptoms.
#[test]
fn test_path_index_lookup_matches_full_scan() {
    let mut set = create_balanced_test_beliefbase();
    let network = set
        .states()
        .values()
        .find(|n| n.title == "Test Network")
        .unwrap()
        .clone();
    let net_bref = network.bid.bref();

    // Build up a map with several siblings, including two that will produce the
    // same title-derived path segment (a real collision) so the duplicate-path
    // branch is exercised, not just the unique-path one.
    for (i, title) in ["Alpha", "Beta", "Alpha", "Gamma", "Beta"]
        .iter()
        .enumerate()
    {
        let doc = create_test_node(title, BeliefKind::Document);
        set.process_event(&BeliefEvent::NodeUpdate(
            vec![],
            doc.clone(),
            crate::event::EventOrigin::Remote,
        ))
        .unwrap();
        let mut w = Weight::default();
        w.set(WEIGHT_SORT_KEY, 1u16 + i as u16).ok();
        set.process_event(&BeliefEvent::RelationChange(
            doc.bid,
            network.bid,
            WeightKind::Section,
            Some(w),
            crate::event::EventOrigin::Remote,
        ))
        .unwrap();
    }

    let paths = set.paths();
    let pm = paths.get_map(&net_bref).unwrap();
    assert_pathmap_indices_consistent(&pm, "after building collision fixture");

    // Every path present in the map must resolve, via the index, to exactly the
    // set of entries a linear scan would find.
    let map = pm.map();
    assert!(map.len() >= 5, "fixture should have produced entries");
    for (path, _, _) in map.iter() {
        let by_scan: Vec<(usize, Bid)> = map
            .iter()
            .enumerate()
            .filter(|(_, (p, _, _))| p == path)
            .map(|(i, (_, bid, _))| (i, *bid))
            .collect();
        let by_index: Vec<(usize, Bid)> = {
            let mut v: Vec<(usize, Bid)> = pm
                .path_map()
                .get(path)
                .map(|&i| vec![(i, map[i].1)])
                .unwrap_or_default();
            v.sort();
            v
        };
        assert_eq!(
            by_index, by_scan,
            "path_map lookup for {path:?} disagrees with a full scan of pm.map()"
        );
    }

    // A path that is absent must yield no candidates — the no-collision fast path.
    assert!(
        pm.path_map().get("definitely/not/present.md").is_none(),
        "absent path should have no index entry"
    );

    // Paths must be unique per BID: the collision check is what guarantees this,
    // so a regression in it shows up here as two different BIDs sharing a path.
    let mut seen: std::collections::HashMap<&String, Bid> = std::collections::HashMap::new();
    for (path, bid, _) in map.iter() {
        if let Some(prev) = seen.insert(path, *bid) {
            assert_eq!(
                prev, *bid,
                "two distinct BIDs share path {path:?} — collision check failed to disambiguate"
            );
        }
    }
}

/// Insert a new child in the *middle* of an existing sibling range (not at the
/// tail), forcing `map_insert` to shift every entry after the insertion point.
/// Verifies the shift correctly patches `bid_map`/`order_map` for
/// every shifted entry, not just the newly inserted one.
#[test]
fn test_incremental_mid_insert_shifts_indices_consistently() {
    let mut set = create_balanced_test_beliefbase();

    let parent_doc = set
        .states()
        .values()
        .find(|n| n.title == "Parent Document")
        .unwrap()
        .clone();

    // Force a genuine mid-map insert into `parent_doc`'s children (initially
    // Child 1/2/3 at sort keys 0/1/2) WITHOUT ever passing through an invalid
    // intermediate state where two edges into the same sink share a sort key.
    // Real production code (assign_sort_key + reindex_sink_edges) never
    // produces duplicate sort keys on one sink, and order_map — a
    // BTreeMap<String, usize> keyed by the order vector — cannot represent
    // two *different* sources sharing one order key, so testing that
    // (invalid) state would exercise behavior nothing in the codebase
    // actually relies on.
    //
    // Valid sequence to insert a new child at position 1 (between Child 1 and
    // Child 2), vacating key 1 without ever colliding:
    //   1. Move Child 3 from 2 -> 3 (3 is free)
    //   2. Move Child 2 from 1 -> 2 (now free, vacated by step 1)
    //   3. Insert the new child at 1 (now free, vacated by step 2)
    let child2 = set
        .states()
        .values()
        .find(|n| n.title == "Child 2")
        .unwrap()
        .clone();
    let child3 = set
        .states()
        .values()
        .find(|n| n.title == "Child 3")
        .unwrap()
        .clone();

    let bump = |set: &mut BeliefBase, bid: Bid, sink: Bid, new_key: u16| {
        let mut w = Weight::default();
        w.set(WEIGHT_SORT_KEY, new_key).ok();
        let mut ws = WeightSet::empty();
        ws.set(WeightKind::Section, w);
        set.process_event(&BeliefEvent::RelationUpdate(
            bid,
            sink,
            ws,
            crate::event::EventOrigin::Remote,
        ))
        .unwrap();
    };
    bump(&mut set, child3.bid, parent_doc.bid, 3);
    bump(&mut set, child2.bid, parent_doc.bid, 2);

    let new_child = create_test_node("Inserted Between", BeliefKind::Document);
    set.process_event(&BeliefEvent::NodeUpdate(
        vec![],
        new_child.clone(),
        crate::event::EventOrigin::Remote,
    ))
    .unwrap();
    let mut w = Weight::default();
    w.set(WEIGHT_SORT_KEY, 1u16).ok();
    set.process_event(&BeliefEvent::RelationChange(
        new_child.bid,
        parent_doc.bid,
        WeightKind::Section,
        Some(w),
        crate::event::EventOrigin::Remote,
    ))
    .unwrap();

    let paths = set.paths();
    let net_bref = paths
        .nets()
        .iter()
        .find(|bid| **bid != set.api().bid)
        .cloned()
        .unwrap()
        .bref();
    let pm = paths.get_map(&net_bref).unwrap();
    assert_pathmap_indices_consistent(&pm, "after mid-map insert + reorder");

    // Child 1, the new child, Child 2, and Child 3 must all still be reachable
    // at distinct paths, in that relative order — i.e. the shift correctly
    // repositioned every sibling, not just the ones adjacent to the insertion.
    let ordered_titles: Vec<String> = pm
        .map()
        .iter()
        .filter(|(_, bid, _)| {
            set.states().get(bid).is_some_and(|n| {
                n.kind.contains(BeliefKind::Document) && n.title != "Parent Document"
            })
        })
        .map(|(_, bid, _)| set.states().get(bid).unwrap().title.clone())
        .collect();
    assert_eq!(
        ordered_titles,
        vec!["Child 1", "Inserted Between", "Child 2", "Child 3"],
        "children should be positioned in sort-key order after the mid-map insert"
    );
}

/// Remove a child, then add more children under the same sink. Exercises
/// `map_remove` followed by further `map_insert` calls in the same PathMap,
/// checking that removal's index patch-up doesn't leave stale entries behind
/// for insertions that come after it.
#[test]
fn test_incremental_remove_then_insert_keeps_indices_consistent() {
    let mut set = create_balanced_test_beliefbase();

    let child2 = set
        .states()
        .values()
        .find(|n| n.title == "Child 2")
        .unwrap()
        .clone();
    let parent_doc = set
        .states()
        .values()
        .find(|n| n.title == "Parent Document")
        .unwrap()
        .clone();

    // Remove Child 2 (middle element) — triggers reindexing of Child 3.
    set.process_event(&BeliefEvent::NodesRemoved(
        vec![child2.bid],
        crate::event::EventOrigin::Remote,
    ))
    .unwrap();

    {
        let paths = set.paths();
        let net_bref = paths
            .nets()
            .iter()
            .find(|bid| **bid != set.api().bid)
            .cloned()
            .unwrap()
            .bref();
        let pm = paths.get_map(&net_bref).unwrap();
        assert_pathmap_indices_consistent(&pm, "after removing Child 2");
    }

    // Now add a brand new child under Parent Document, appended after the
    // reindexed Child 3.
    let new_child = create_test_node("Child 4", BeliefKind::Document);
    set.process_event(&BeliefEvent::NodeUpdate(
        vec![],
        new_child.clone(),
        crate::event::EventOrigin::Remote,
    ))
    .unwrap();
    let mut w = Weight::default();
    w.set(WEIGHT_SORT_KEY, 2u16).ok();
    set.process_event(&BeliefEvent::RelationChange(
        new_child.bid,
        parent_doc.bid,
        WeightKind::Section,
        Some(w),
        crate::event::EventOrigin::Remote,
    ))
    .unwrap();

    let paths = set.paths();
    let net_bref = paths
        .nets()
        .iter()
        .find(|bid| **bid != set.api().bid)
        .cloned()
        .unwrap()
        .bref();
    let pm = paths.get_map(&net_bref).unwrap();
    assert_pathmap_indices_consistent(&pm, "after remove then insert");

    // Compare against a from-scratch constructor PathMapMap to double check
    // path strings are also correct, not just internally self-consistent.
    drop(pm);
    drop(paths);
    let relations_guard = set.relations();
    let relations_arc = Arc::new(RwLock::new(relations_guard.clone()));
    let paths_constructor = PathMapMap::new(set.states(), relations_arc);
    let constructor_paths: BTreeSet<String> = paths_constructor
        .all_paths()
        .values()
        .flatten()
        .map(|(path, _, _)| path.clone())
        .collect();
    let event_paths: BTreeSet<String> = set
        .paths()
        .all_paths()
        .values()
        .flatten()
        .map(|(path, _, _)| path.clone())
        .collect();
    assert_eq!(
        event_paths, constructor_paths,
        "Event-driven and constructor PathMaps must agree after remove+insert"
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

    let mut states = FxHashMap::default();

    let api = BeliefNode::api_state();
    states.insert(api.bid, api.clone());

    // Network node
    let net = BeliefNode {
        bid: Bid::new(api.bid),
        title: "Test Network".to_string(),
        kind: BeliefKindSet(BeliefKind::Network.into()),
        id: NodeId::Explicit(to_anchor("test-network")),
        ..Default::default()
    };
    states.insert(net.bid, net.clone());

    // Document children — these go into PathMapMap::docs()
    let doc_a = BeliefNode {
        bid: Bid::new(net.bid),
        title: "Doc A".to_string(),
        kind: BeliefKindSet(BeliefKind::Document.into()),
        id: NodeId::Explicit(to_anchor("doc-a")),
        ..Default::default()
    };
    let doc_b = BeliefNode {
        bid: Bid::new(net.bid),
        title: "Doc B".to_string(),
        kind: BeliefKindSet(BeliefKind::Document.into()),
        id: NodeId::Explicit(to_anchor("doc-b")),
        ..Default::default()
    };
    states.insert(doc_a.bid, doc_a.clone());
    states.insert(doc_b.bid, doc_b.clone());

    // Anchor children (Symbol kind — not Document, so is_anchor() returns true)
    let anchor_x = BeliefNode {
        bid: Bid::new(net.bid),
        title: "Heading X".to_string(),
        kind: BeliefKindSet(BeliefKind::Symbol.into()),
        id: NodeId::Explicit(to_anchor("heading-x")),
        ..Default::default()
    };
    let anchor_y = BeliefNode {
        bid: Bid::new(net.bid),
        title: "Heading Y".to_string(),
        kind: BeliefKindSet(BeliefKind::Symbol.into()),
        id: NodeId::Explicit(to_anchor("heading-y")),
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
    // built_in_test() checks edge sort keys. For network nodes it verifies
    // docs and anchors are each independently contiguous (not globally contiguous),
    // so the [0, 1] doc keys and [0, 1] anchor keys both satisfy the invariant.
    let errors = set.built_in_test();
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

    {
        let paths = set.paths();
        let pm = paths.get_map(&net_bid.bref()).unwrap();
        assert_pathmap_indices_consistent(&pm, "after doc_a/anchor_x re-issue");
    }

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

/// `indexed_path` narrows its candidate networks through the `node_to_nets`
/// reverse index instead of probing every `PathMap`. The narrowed lookup must
/// return *exactly* what the exhaustive scan returns, for every BID in the
/// graph and for BIDs that are absent — otherwise a node silently resolves to
/// the wrong home network, or to none at all.
///
/// The subtle case this guards: `node_to_nets` records only *direct*
/// containment, while `PathMap::path` also resolves a BID held by a subnet by
/// recursing into it. If the narrowing dropped subnet-holding parents, a node
/// reachable only through a subnet would regress to `None`. The fixture below
/// includes a subnet for that reason.
///
/// Asserted against ground truth rather than inferred from downstream symptoms,
/// mirroring `test_path_index_lookup_matches_full_scan`.
#[test]
fn test_indexed_path_narrowing_matches_full_scan() {
    let set = create_balanced_test_beliefbase();
    let paths = set.paths();

    // The balanced fixture must actually contain a subnet, or the case this
    // test exists to cover would be vacuous.
    let has_subnet = paths
        .map()
        .values()
        .any(|pm| !pm.read().subnets().is_empty());
    assert!(
        has_subnet,
        "fixture must contain a subnet for the recursion case to be covered"
    );

    // Every BID the graph knows about must resolve identically both ways.
    for bid in set.states().keys() {
        assert_eq!(
            paths.indexed_path(bid),
            paths.scan_indexed_path(bid),
            "narrowed indexed_path disagreed with full scan for {bid}"
        );
    }

    // An unknown BID has no reverse-index entry and must take the fallback
    // path, still agreeing with the scan (both should be None).
    let unknown = Bid::new(Bid::nil());
    assert_eq!(
        paths.indexed_path(&unknown),
        paths.scan_indexed_path(&unknown),
        "narrowed indexed_path disagreed with full scan for an unknown BID"
    );
}

/// The subnet-holder set consulted by `indexed_path` is a *cache*, so the
/// failure mode is staleness: a network that gains a subnet after the cache is
/// warmed would be skipped as a candidate, and nodes reachable only through
/// that subnet would silently resolve to `None`.
///
/// This warms the cache, then mutates the graph so a network gains a subnet,
/// and re-checks equivalence against the exhaustive scan. Without invalidation
/// this fails; the narrowing test above would not catch it, because it never
/// mutates after reading.
#[test]
fn test_indexed_path_narrowing_survives_subnet_mutation() {
    let mut set = create_balanced_test_beliefbase();

    // Warm the cache in the *live* PathMapMap (not a temporary), so the
    // memoized subnet-holder set is the one the mutation below must invalidate.
    let warmed_holders = {
        let paths = set.paths();
        for bid in set.states().keys() {
            let _ = paths.indexed_path(bid);
        }
        paths.subnet_ancestors_for_test()
    };

    // Introduce a new network and nest it under an existing one, so the parent
    // gains a subnet it did not have when the cache was warmed.
    let parent = set
        .states()
        .values()
        .find(|n| n.title == "Test Network")
        .unwrap()
        .clone();
    let child_net = create_test_node("Nested Network", BeliefKind::Network);
    set.process_event(&BeliefEvent::NodeUpdate(
        vec![],
        child_net.clone(),
        crate::event::EventOrigin::Remote,
    ))
    .unwrap();
    set.process_event(&BeliefEvent::RelationChange(
        child_net.bid,
        parent.bid,
        WeightKind::Section,
        Some(Weight::default()),
        crate::event::EventOrigin::Remote,
    ))
    .unwrap();

    // Add a document inside the nested network: reachable from the parent only
    // by recursing into the subnet, which is precisely the case the cache
    // controls.
    let nested_doc = create_test_node("Nested Doc", BeliefKind::Document);
    set.process_event(&BeliefEvent::NodeUpdate(
        vec![],
        nested_doc.clone(),
        crate::event::EventOrigin::Remote,
    ))
    .unwrap();
    set.process_event(&BeliefEvent::RelationChange(
        nested_doc.bid,
        child_net.bid,
        WeightKind::Section,
        Some(Weight::default()),
        crate::event::EventOrigin::Remote,
    ))
    .unwrap();

    let paths = set.paths();

    // The parent must actually have gained a subnet, or the mutation did not
    // exercise the case and the assertions below would be vacuous.
    let holders_now = paths.subnet_ancestors_for_test();
    let child_bref = child_net.bid.bref();
    assert!(
        holders_now
            .get(&child_bref)
            .is_some_and(|a| a.contains(&parent.bid.bref())),
        "fixture did not nest the child under the parent — test would be vacuous"
    );
    assert!(
        !warmed_holders.contains_key(&child_bref),
        "child was already a subnet before mutation — test would be vacuous"
    );

    for bid in set.states().keys() {
        assert_eq!(
            paths.indexed_path(bid),
            paths.scan_indexed_path(bid),
            "narrowed indexed_path disagreed with full scan after subnet mutation \
             for {bid} — subnet-holder cache is likely stale"
        );
    }
}

/// `indexed_path`'s fallback exists for BIDs absent from `node_to_nets`, and
/// it costs a probe of *every* network. That is only acceptable if the index
/// is genuinely incomplete; if `node_to_nets` covers every BID any `PathMap`
/// can resolve, a miss is proof of absence and the scan is pure waste.
///
/// This asserts that completeness invariant directly: for every BID in the
/// graph, a `node_to_nets` miss implies the exhaustive scan also finds nothing.
#[test]
fn test_node_to_nets_miss_implies_no_path() {
    let set = create_balanced_test_beliefbase();
    let paths = set.paths();

    let mut checked_misses = 0usize;
    for bid in set.states().keys() {
        if paths.node_to_nets_contains_for_test(bid) {
            continue;
        }
        checked_misses += 1;
        assert_eq!(
            paths.scan_indexed_path(bid),
            None,
            "BID {bid} is absent from node_to_nets yet the exhaustive scan \
             resolves it — the index is incomplete and the fallback is load-bearing"
        );
    }

    // Also check a BID the map has never seen, which must miss both ways.
    let unknown = Bid::new(Bid::nil());
    assert!(!paths.node_to_nets_contains_for_test(&unknown));
    assert_eq!(paths.scan_indexed_path(&unknown), None);
    checked_misses += 1;

    assert!(
        checked_misses > 0,
        "fixture produced no index misses — invariant untested"
    );
}

/// The `indexed_path` counters exist to say *which* of its two routes costs
/// the time. A counter that never moves, or that attributes to the wrong
/// route, is worse than none — it produces confident wrong conclusions.
///
/// Asserts both routes are actually reached and increment their own counters:
/// a known BID takes the narrowed route, an unknown BID takes the fallback.
///
/// The counters are process-global statics and other tests in this binary also
/// call `indexed_path`, so this is `#[serial]` and asserts *lower bounds on
/// deltas* rather than exact equality — an exact-match version would pass or
/// fail depending on thread scheduling.
#[test]
#[serial_test::serial]
fn test_indexed_path_counters_attribute_to_the_right_route() {
    let set = create_balanced_test_beliefbase();
    let paths = set.paths();

    let known = *set.states().keys().next().unwrap();
    let unknown = Bid::new(Bid::nil());

    let before = crate::paths::pathmap::indexed_path_stats();
    let _ = paths.indexed_path(&known);
    let after_known = crate::paths::pathmap::indexed_path_stats();

    assert!(
        after_known.0 > before.0,
        "a known BID must increment the indexed-route call counter"
    );
    assert_eq!(
        after_known.1, before.1,
        "a known BID must not touch the fallback counter"
    );
    assert!(
        after_known.2 > before.2,
        "the indexed route must record at least one probe"
    );

    let _ = paths.indexed_path(&unknown);
    let after_unknown = crate::paths::pathmap::indexed_path_stats();

    assert!(
        after_unknown.1 > after_known.1,
        "an unknown BID must increment the no-index call counter"
    );
    // An index miss short-circuits to `None` instead of scanning every network,
    // so it must record *no* probes. This is the assertion that would catch a
    // reintroduced exhaustive fallback.
    assert_eq!(
        after_unknown.2, after_known.2,
        "an index miss must probe nothing — a nonzero probe count means the \
         exhaustive scan is back on the hot path"
    );
}

/// Regression: the malformed `index.md<dir>/<slug>` path.
///
/// A corpus run produced 51,591 distinct paths of the form
/// `index.md<repo-dir>/<heading-slug>` — the network filename concatenated
/// to `<repo-dir-name>/<slug>` with no separator. Every one had exactly one
/// slash and the repo directory name as its first segment, and the slug was an
/// ordinary heading anchor.
///
/// The shape that produces it: heading anchors parented **directly to the
/// network root**, which is the `sink == net && is_anchor(source)` branch in
/// `PathMap::new`. That branch prepends `network_filename` on the assumption
/// that `anchorize` returned a leading `#`.
///
/// This test asserts the *correct* behaviour, so it fails while the defect is
/// live and documents the fix when it lands.
#[test]
fn test_network_index_anchor_path_is_well_formed() {
    let mut set = create_balanced_test_beliefbase();
    let network = set
        .states()
        .values()
        .find(|n| n.title == "Test Network")
        .unwrap()
        .clone();
    let net_bref = network.bid.bref();

    // A heading anchor hanging directly off the network root — what an
    // `index.md` containing `## Slide 1` produces.
    let anchor = create_test_node("Slide 1", BeliefKind::Core);
    assert!(
        anchor.kind.is_anchor(),
        "fixture node must classify as an anchor"
    );
    set.process_event(&BeliefEvent::NodeUpdate(
        vec![],
        anchor.clone(),
        crate::event::EventOrigin::Remote,
    ))
    .unwrap();

    let mut w = Weight::default();
    w.set(WEIGHT_SORT_KEY, 0u16).ok();
    set.process_event(&BeliefEvent::RelationChange(
        anchor.bid,
        network.bid,
        WeightKind::Section,
        Some(w),
        crate::event::EventOrigin::Remote,
    ))
    .unwrap();

    let entry_for = |pmm: &PathMapMap| -> String {
        pmm.get_map(&net_bref)
            .unwrap()
            .map()
            .iter()
            .find(|(_, bid, _)| *bid == anchor.bid)
            .map(|(path, _, _)| path.clone())
            .expect("anchor should have a PathMap entry")
    };

    // Both construction paths must agree and both must be well-formed. The
    // event-driven path (process_event) and the from-scratch constructor
    // (PathMapMap::new, which runs the DFS in PathMap::new) reach the
    // NETWORK_SECTION_SORT_KEY branch by different routes.
    let event_entry = entry_for(&set.paths());

    let relations_arc = Arc::new(RwLock::new(set.relations().clone()));
    let constructed = PathMapMap::new(set.states(), relations_arc);
    let ctor_entry = entry_for(&constructed);

    for (label, entry) in [("event-driven", &event_entry), ("constructor", &ctor_entry)] {
        assert!(
            entry.contains('#'),
            "{label}: network-index anchor path must contain '#', got {entry:?}"
        );
        assert!(
            !entry.contains("index.md") || entry.contains("index.md#"),
            "{label}: network filename must be followed by '#', got {entry:?}"
        );
    }
    assert_eq!(
        event_entry, ctor_entry,
        "event-driven and constructor paths diverged"
    );
}

/// Regression: an explicit `doc_path` on an anchor parented directly to a
/// network must not be prefixed with the network filename.
///
/// This is the shape an `alias-template` registration produces: the node is an
/// anchor (a heading), its Section sink *is* a network (the href namespace), and
/// its `doc_paths` weight carries a complete path that never passed through
/// `anchorize`. The NETWORK_SECTION_SORT_KEY branch used to prepend the network
/// filename unconditionally, yielding `index.md<path>` — unreachable, and shared
/// by every alias in that namespace. Measured on one corpus: 51,591 such paths.
///
/// Bare `#anchor` subpaths must still be qualified, so both cases are asserted.
#[test]
fn test_explicit_doc_path_on_network_anchor_is_not_prefixed() {
    let mut set = create_balanced_test_beliefbase();
    let network = set
        .states()
        .values()
        .find(|n| n.title == "Test Network")
        .unwrap()
        .clone();
    let net_bref = network.bid.bref();

    // An anchor carrying an explicit alias-style doc_path, hung off the network.
    let aliased = create_test_node("Aliased Section", BeliefKind::Core);
    set.process_event(&BeliefEvent::NodeUpdate(
        vec![],
        aliased.clone(),
        crate::event::EventOrigin::Remote,
    ))
    .unwrap();

    let mut w = Weight::default();
    w.set(WEIGHT_SORT_KEY, 0u16).ok();
    w.set_doc_paths(vec!["catalog/some-product".to_string()])
        .unwrap();
    set.process_event(&BeliefEvent::RelationChange(
        aliased.bid,
        network.bid,
        WeightKind::Section,
        Some(w),
        crate::event::EventOrigin::Remote,
    ))
    .unwrap();

    let relations_arc = Arc::new(RwLock::new(set.relations().clone()));
    let constructed = PathMapMap::new(set.states(), relations_arc);
    let entry = constructed
        .get_map(&net_bref)
        .unwrap()
        .map()
        .iter()
        .find(|(_, bid, _)| *bid == aliased.bid)
        .map(|(path, _, _)| path.clone())
        .expect("aliased anchor should have a PathMap entry");

    assert_eq!(
        entry, "catalog/some-product",
        "an explicit doc_path must be stored verbatim, not prefixed with the \
         network filename"
    );
    assert!(
        !entry.starts_with("index.md"),
        "network filename must not be glued onto a complete path, got {entry:?}"
    );
}

/// Regression: `anchorize` returns a subpath unmodified for URLs and
/// absolute paths, but `PathMap::new`'s NETWORK_SECTION_SORT_KEY branch prepends
/// the network filename assuming a leading '#'. Confirm the two probes that
/// detect that mismatch are reachable, so a zero-hit corpus run means "did not
/// occur" rather than "cannot fire".
#[test]
fn test_anchorize_carveout_probe_is_reachable() {
    let set = create_balanced_test_beliefbase();
    let paths = set.paths();
    // `PathMapMap::is_anchor` is `!docs.contains(bid)`, so any BID the map has
    // never seen is treated as an anchor — which is exactly the classification
    // the carve-out below operates under.
    let anchor_bid = Bid::new(Bid::nil());
    assert!(
        paths.is_anchor(&anchor_bid),
        "an unknown BID should classify as an anchor"
    );

    // Normal slug: gets a '#'.
    let normal = paths.anchorize(&anchor_bid, "some-heading");
    assert!(
        normal.starts_with('#'),
        "plain slug should be anchorized, got {normal:?}"
    );

    // URL: hits the carve-out and comes back unprefixed -- the shape that
    // produces `index.md<subpath>` downstream.
    let url = paths.anchorize(&anchor_bid, "https://example.com/browse/X-1");
    assert!(
        !url.starts_with('#'),
        "URL should bypass anchorization, got {url:?}"
    );

    // Absolute path: same carve-out.
    let abs = paths.anchorize(&anchor_bid, "/some-dir/cdr-slide-2");
    assert!(
        !abs.starts_with('#'),
        "absolute path should bypass anchorization, got {abs:?}"
    );
}

/// Issue 102 Part 3 / Step 1: can a stub and a content node actually co-exist
/// on one path?
///
/// `path_map`'s value is a `Vec` because two entries were believed able to share
/// a path: an `External|Trace` stub created for an unresolved URL, later claimed
/// by a content node declaring the same URL as an alias. `indexed_get` carries a
/// preference loop to pick the content node when that happens.
///
/// A corpus run measured up to 34 claimants on one path, which looked like
/// confirmation. It was not — those were `alias-template` registrations flattened
/// onto one malformed string by two unrelated defects (both fixed in 85c631a);
/// after the fix, 1.29M lookups produced **zero** multi-candidate results. Since
/// there is only ever one stub per URL, the tolerated case can produce at most
/// two claimants and could never have explained 34.
///
/// This asserts what the stub-claim path actually does, so the `Vec` is either
/// justified by a reachable state or shown to be scaffolding around a fixed bug.
#[test]
fn test_stub_and_content_claim_do_not_share_a_path() {
    let url = "https://example.com/browse/THING-1";

    let mut set = create_balanced_test_beliefbase();
    let network = set
        .states()
        .values()
        .find(|n| n.title == "Test Network")
        .unwrap()
        .clone();

    // 0. The href namespace network node itself, as `ensure_href_namespace` does.
    //    Children cannot register under a namespace that has no PathMap.
    let href_net = BeliefNode::href_network();
    set.process_event(&BeliefEvent::NodeUpdate(
        vec![],
        href_net.clone(),
        crate::event::EventOrigin::Remote,
    ))
    .unwrap();

    // 1. The stub, as `ensure_href_entry` builds it: External|Trace, BID derived
    //    from the URL, Section edge into href_namespace with the URL as doc_path.
    let stub = BeliefNode {
        bid: crate::properties::buildonomy_href_bid(url),
        kind: BeliefKindSet::from(BeliefKind::External | BeliefKind::Trace),
        title: url.to_string(),
        id: NodeId::Explicit(url.to_string()),
        ..Default::default()
    };
    set.process_event(&BeliefEvent::NodeUpdate(
        vec![],
        stub.clone(),
        crate::event::EventOrigin::Remote,
    ))
    .unwrap();
    let mut stub_w = Weight::default();
    stub_w.set(WEIGHT_SORT_KEY, 0u16).ok();
    stub_w.set_doc_paths(vec![url.to_string()]).unwrap();
    set.process_event(&BeliefEvent::RelationChange(
        stub.bid,
        crate::properties::href_namespace(),
        WeightKind::Section,
        Some(stub_w),
        crate::event::EventOrigin::Remote,
    ))
    .unwrap();

    // Precondition: the stub really is registered. Without this the test could
    // report "one entry" simply because the stub never existed.
    let href_bref = crate::properties::href_namespace().bref();
    {
        let paths = set.paths();
        let hm = paths.get_map(&href_bref).expect("href PathMap must exist");
        assert!(
            hm.map().iter().any(|(_, bid, _)| *bid == stub.bid),
            "stub was not registered in the href namespace; test would be vacuous"
        );
    }

    // 2. A content node claims the same URL, as an `alias-template` registration
    //    does: an ordinary node with a Section edge into href_namespace carrying
    //    the same doc_path.
    // The claim must carry the merge keys the real pipeline supplies.
    // `insert_state`'s absorb path (`to_replace` -> NodeRenamed -> replace_bid)
    // is driven entirely by the `keys` argument; passing an empty vec bypasses
    // it, so the stub would survive for reasons that have nothing to do with
    // whether the mechanism works.
    let content = create_test_node("Real Document", BeliefKind::Document);
    let claim_keys = vec![NodeKey::Path {
        net: crate::properties::href_namespace().bref(),
        path: url.to_string(),
    }];
    set.process_event(&BeliefEvent::NodeUpdate(
        claim_keys,
        content.clone(),
        crate::event::EventOrigin::Remote,
    ))
    .unwrap();
    let mut doc_w = Weight::default();
    doc_w.set(WEIGHT_SORT_KEY, 0u16).ok();
    set.process_event(&BeliefEvent::RelationChange(
        content.bid,
        network.bid,
        WeightKind::Section,
        Some(doc_w),
        crate::event::EventOrigin::Remote,
    ))
    .unwrap();
    let mut alias_w = Weight::default();
    alias_w.set(WEIGHT_SORT_KEY, 1u16).ok();
    alias_w.set_doc_paths(vec![url.to_string()]).unwrap();
    set.process_event(&BeliefEvent::RelationChange(
        content.bid,
        crate::properties::href_namespace(),
        WeightKind::Section,
        Some(alias_w),
        crate::event::EventOrigin::Remote,
    ))
    .unwrap();

    let paths = set.paths();
    let hm = paths.get_map(&href_bref).unwrap();
    assert_pathmap_indices_consistent(&hm, "after stub + content claim");

    let claimants: Vec<Bid> = hm
        .path_map()
        .get(url)
        .map(|&i| vec![hm.map()[i].1])
        .unwrap_or_default();

    assert_eq!(
        claimants.len(),
        1,
        "path {url:?} must resolve to exactly one BID, got {claimants:?}"
    );

    // The survivor must be the content node, and the stub must be gone from
    // `states` — i.e. this is absorption, not the stub quietly failing to
    // register. Without these two assertions a single claimant is ambiguous.
    assert_eq!(
        claimants,
        vec![content.bid],
        "the content node should own the path after claiming it"
    );
    assert!(
        !set.states().contains_key(&stub.bid),
        "stub {} survived absorption; insert_state should have retired it via \
         NodeRenamed -> replace_bid",
        stub.bid
    );
}

/// Without the merge key, the stub survives and the path has two claimants.
///
/// This is the negative case for `test_stub_and_content_claim_do_not_share_a_path`:
/// same setup, but the claiming `NodeUpdate` carries no key resolving to the
/// stub, so `insert_state` cannot absorb it. It pins *where* the invariant comes
/// from — node identity, not the PathMap.
///
/// A PathMap-level eviction was tried here and removed: clearing the index left
/// the stub node and its Section edge in the graph, so the next `PathMap::new`
/// rebuilt the duplicate from the relations and the eviction re-fired forever.
/// If this test ever starts reporting one claimant, the fix belongs upstream at
/// the claim site, and this test should be inverted rather than deleted.
#[test]
fn test_claim_without_merge_key_leaves_stub_in_place() {
    let url = "https://example.com/browse/THING-3";

    let mut set = create_balanced_test_beliefbase();
    let href_net = BeliefNode::href_network();
    set.process_event(&BeliefEvent::NodeUpdate(
        vec![],
        href_net,
        crate::event::EventOrigin::Remote,
    ))
    .unwrap();

    let stub = BeliefNode {
        bid: crate::properties::buildonomy_href_bid(url),
        kind: BeliefKindSet::from(BeliefKind::External | BeliefKind::Trace),
        title: url.to_string(),
        id: NodeId::Explicit(url.to_string()),
        ..Default::default()
    };
    set.process_event(&BeliefEvent::NodeUpdate(
        vec![],
        stub.clone(),
        crate::event::EventOrigin::Remote,
    ))
    .unwrap();
    let mut stub_w = Weight::default();
    stub_w.set(WEIGHT_SORT_KEY, 0u16).ok();
    stub_w.set_doc_paths(vec![url.to_string()]).unwrap();
    set.process_event(&BeliefEvent::RelationChange(
        stub.bid,
        crate::properties::href_namespace(),
        WeightKind::Section,
        Some(stub_w),
        crate::event::EventOrigin::Remote,
    ))
    .unwrap();

    let href_bref = crate::properties::href_namespace().bref();
    {
        let paths = set.paths();
        let hm = paths.get_map(&href_bref).unwrap();
        assert!(
            hm.path_map().contains_key(url),
            "precondition: stub must hold the path before the claim"
        );
    }

    // Claim the same path WITHOUT the merge key, so `insert_state` cannot absorb
    // the stub. Nothing downstream compensates, so both entries persist.
    let content = create_test_node("Claiming Document", BeliefKind::Document);
    set.process_event(&BeliefEvent::NodeUpdate(
        vec![],
        content.clone(),
        crate::event::EventOrigin::Remote,
    ))
    .unwrap();
    let mut alias_w = Weight::default();
    alias_w.set(WEIGHT_SORT_KEY, 1u16).ok();
    alias_w.set_doc_paths(vec![url.to_string()]).unwrap();
    set.process_event(&BeliefEvent::RelationChange(
        content.bid,
        crate::properties::href_namespace(),
        WeightKind::Section,
        Some(alias_w),
        crate::event::EventOrigin::Remote,
    ))
    .unwrap();

    let paths = set.paths();
    let hm = paths.get_map(&href_bref).unwrap();
    assert_pathmap_indices_consistent(&hm, "after stub eviction");

    // `path_map` is scalar, so it holds whichever entry was written last; the
    // stub is still present in `map` and in the graph.
    let entries_on_path: Vec<Bid> = hm
        .map()
        .iter()
        .filter(|(p, _, _)| p == url)
        .map(|(_, bid, _)| *bid)
        .collect();
    assert!(
        entries_on_path.contains(&stub.bid),
        "without a merge key the stub must survive — if it no longer does, the \
         claim path gained absorption and this test should be inverted; got \
         {entries_on_path:?}"
    );
    assert!(
        set.states().contains_key(&stub.bid),
        "stub node should still be in the graph"
    );
}

/// A third document's reference to the stub must survive the stub's absorption.
///
/// When a content node claims a URL an `External|Trace` stub already holds,
/// `insert_state` retires the stub. Any edge another document had already drawn
/// to that stub has to be re-pointed at the claimant, or the reference is
/// silently lost — no panic, no diagnostic, just an edge that no longer arrives.
/// `replace_bid` is what re-points them; this pins that it does.
#[test]
fn test_reference_to_stub_survives_content_claim() {
    let url = "https://example.com/browse/THING-2";

    let mut set = create_balanced_test_beliefbase();
    let network = set
        .states()
        .values()
        .find(|n| n.title == "Test Network")
        .unwrap()
        .clone();

    let href_net = BeliefNode::href_network();
    set.process_event(&BeliefEvent::NodeUpdate(
        vec![],
        href_net,
        crate::event::EventOrigin::Remote,
    ))
    .unwrap();

    // Stub for an unresolved link.
    let stub = BeliefNode {
        bid: crate::properties::buildonomy_href_bid(url),
        kind: BeliefKindSet::from(BeliefKind::External | BeliefKind::Trace),
        title: url.to_string(),
        id: NodeId::Explicit(url.to_string()),
        ..Default::default()
    };
    set.process_event(&BeliefEvent::NodeUpdate(
        vec![],
        stub.clone(),
        crate::event::EventOrigin::Remote,
    ))
    .unwrap();
    let mut stub_w = Weight::default();
    stub_w.set(WEIGHT_SORT_KEY, 0u16).ok();
    stub_w.set_doc_paths(vec![url.to_string()]).unwrap();
    set.process_event(&BeliefEvent::RelationChange(
        stub.bid,
        crate::properties::href_namespace(),
        WeightKind::Section,
        Some(stub_w),
        crate::event::EventOrigin::Remote,
    ))
    .unwrap();

    // A third document links to the stub (Epistemic, as an ordinary reference).
    let referrer = create_test_node("Referring Document", BeliefKind::Document);
    set.process_event(&BeliefEvent::NodeUpdate(
        vec![],
        referrer.clone(),
        crate::event::EventOrigin::Remote,
    ))
    .unwrap();
    let mut ref_w = Weight::default();
    ref_w.set(WEIGHT_SORT_KEY, 0u16).ok();
    set.process_event(&BeliefEvent::RelationChange(
        referrer.bid,
        stub.bid,
        WeightKind::Epistemic,
        Some(ref_w),
        crate::event::EventOrigin::Remote,
    ))
    .unwrap();

    let edges_to = |sink: Bid, set: &BeliefBase| -> usize {
        let rel = set.relations();
        let g = rel.as_graph();
        g.edge_references()
            .filter(|e| g[e.target()] == sink && g[e.source()] == referrer.bid)
            .count()
    };
    assert_eq!(
        edges_to(stub.bid, &set),
        1,
        "precondition: referrer must actually link to the stub"
    );

    // The content node now claims the URL, carrying the merge key that drives
    // absorption.
    let content = create_test_node("Real Document 2", BeliefKind::Document);
    let claim_keys = vec![NodeKey::Path {
        net: crate::properties::href_namespace().bref(),
        path: url.to_string(),
    }];
    set.process_event(&BeliefEvent::NodeUpdate(
        claim_keys,
        content.clone(),
        crate::event::EventOrigin::Remote,
    ))
    .unwrap();
    let mut doc_w = Weight::default();
    doc_w.set(WEIGHT_SORT_KEY, 0u16).ok();
    set.process_event(&BeliefEvent::RelationChange(
        content.bid,
        network.bid,
        WeightKind::Section,
        Some(doc_w),
        crate::event::EventOrigin::Remote,
    ))
    .unwrap();

    assert!(
        !set.states().contains_key(&stub.bid),
        "stub should have been absorbed by the claim"
    );
    assert_eq!(
        edges_to(content.bid, &set),
        1,
        "the referrer's edge must have been re-pointed at the claiming content \
         node; if this is 0 the reference was silently dropped"
    );
}
