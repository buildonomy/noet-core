//! Tests for path update and reindexing logic

use super::helpers::*;
use crate::{
    event::BeliefEvent,
    nodekey::NodeKey,
    properties::{BeliefKind, Bid, Weight, WeightKind, WeightSet, WEIGHT_SORT_KEY},
};
use parking_lot::RwLock;
use std::{collections::BTreeSet, sync::Arc};
use test_log::test;

#[test]
fn test_relation_removal_triggers_reindexing() {
    // Start with a balanced test set
    let mut set = create_balanced_test_beliefset();

    // Get the parent doc and children from the set
    let parent_doc = set
        .get(&NodeKey::Title {
            net: Bid::nil(),
            title: "Parent Document".to_string(),
        })
        .unwrap();
    let child2 = set
        .get(&NodeKey::Title {
            net: Bid::nil(),
            title: "Child 2".to_string(),
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
        "Final state should be balanced: {:?}",
        final_errors
    );
}

#[test]
fn test_parent_reindex_updates_child_order_vectors() {
    // Start with a balanced test set
    let mut set = create_balanced_test_beliefset();

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

    let insert_relation_event = BeliefEvent::RelationInsert(
        grandchild_bid,
        child3.bid,
        WeightKind::Section,
        grandchild_weights
            .get(&WeightKind::Section)
            .unwrap()
            .clone(),
        crate::event::EventOrigin::Remote,
    );
    set.process_event(&insert_relation_event).unwrap();

    // Get initial grandchild order vector from PathMap
    let paths = set.paths();
    let net_bid = paths
        .nets()
        .iter()
        .find(|bid| **bid != set.api().bid)
        .cloned();
    assert!(net_bid.is_some());

    let pm = paths.get_map(&net_bid.unwrap()).unwrap();
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
    let pm = paths.get_map(&net_bid.unwrap()).unwrap();
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
    let mut set = create_balanced_test_beliefset();

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
        paths_event.anchors().len(),
        paths_constructor.anchors().len(),
        "anchors metadata should match"
    );
}
