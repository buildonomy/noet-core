//! Tests for BeliefBase functionality

use super::*;
use crate::nodekey::NodeKey;
use crate::properties::{
    BeliefKind, BeliefKindSet, BeliefNode, Bid, Weight, WeightKind, WeightSet,
};
use std::collections::BTreeMap;

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
        net: net_bid.bref(),
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
        net: net_bid.bref(),
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
