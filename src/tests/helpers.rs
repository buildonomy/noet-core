//! Shared test utilities for BeliefSet testing

use crate::{
    beliefset::{BeliefSet, BidGraph},
    properties::{
        BeliefKind, BeliefKindSet, BeliefNode, Bid, Weight, WeightKind, WeightSet, WEIGHT_SORT_KEY,
    },
};
use std::collections::BTreeMap;

/// Initialize logging for tests
pub fn init_logging() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_test_writer()
        .try_init()
        .ok();
}

/// Helper function to create a simple BeliefNode for testing
pub fn create_test_node(title: &str, kind: BeliefKind) -> BeliefNode {
    BeliefNode {
        title: title.to_string(),
        kind: BeliefKindSet(kind.into()),
        bid: Bid::new(Bid::nil()),
        ..Default::default()
    }
}

/// Helper function to create a test BeliefSet with some nodes and relations
pub fn create_test_beliefset() -> BeliefSet {
    init_logging();

    let mut states = BTreeMap::new();

    // Create a few test nodes
    let node1 = create_test_node("Node 1", BeliefKind::Document);
    let mut node2 = create_test_node("Node 2", BeliefKind::Document);
    let node3 = create_test_node("Node 3", BeliefKind::Symbol);
    let node4 = create_test_node("Node 4", BeliefKind::Symbol);

    let bid1 = node1.bid;
    // Make bid2 in the bid1 namespace
    let bid2 = Bid::new(bid1);
    node2.bid = bid2;
    let bid3 = node3.bid;
    let bid4 = node4.bid;

    states.insert(bid1, node1);
    states.insert(bid2, node2);
    states.insert(bid3, node3);
    states.insert(bid4, node4);

    // Create relations: node1 -> node3, node2 -> node4
    let mut weight1 = Weight::default();
    weight1.set(WEIGHT_SORT_KEY, 1u16).ok();
    let mut weights1 = WeightSet::empty();
    weights1.set(WeightKind::Section, weight1);

    let mut weight2 = Weight::default();
    weight2.set(WEIGHT_SORT_KEY, 2u16).ok();
    let mut weights2 = WeightSet::empty();
    weights2.set(WeightKind::Section, weight2);

    let relations = BidGraph::from_edges(vec![(bid1, bid3, weights1), (bid2, bid4, weights2)]);

    BeliefSet::new_unbalanced(states, relations, false)
}

/// Create a balanced test BeliefSet with the proper hierarchy:
/// API <- Network <- Document <- Anchors
///
/// This structure satisfies BeliefSet::is_balanced() checks and is suitable
/// for testing path updates, reindexing, and other operations that require
/// a fully valid BeliefSet structure.
pub fn create_balanced_test_beliefset() -> BeliefSet {
    init_logging();

    let mut states = BTreeMap::new();

    // Create API node
    let api = BeliefNode::api_state();
    states.insert(api.bid, api.clone());

    // Create Network node
    let network = create_test_node("Test Network", BeliefKind::Network);
    states.insert(network.bid, network.clone());

    // Create Document nodes (parent and children)
    let parent_doc = create_test_node("Parent Document", BeliefKind::Document);
    let child1_doc = create_test_node("Child 1", BeliefKind::Document);
    let child2_doc = create_test_node("Child 2", BeliefKind::Document);
    let child3_doc = create_test_node("Child 3", BeliefKind::Document);

    states.insert(parent_doc.bid, parent_doc.clone());
    states.insert(child1_doc.bid, child1_doc.clone());
    states.insert(child2_doc.bid, child2_doc.clone());
    states.insert(child3_doc.bid, child3_doc.clone());

    // Create relations for the hierarchy
    let mut edges = Vec::new();

    // Network -> API (Section)
    let mut net_api_weight = Weight::default();
    net_api_weight.set(WEIGHT_SORT_KEY, 0u16).ok();
    let mut net_api_weights = WeightSet::empty();
    net_api_weights.set(WeightKind::Section, net_api_weight);
    edges.push((network.bid, api.bid, net_api_weights));

    // Parent Document -> Network (Section)
    let mut doc_net_weight = Weight::default();
    doc_net_weight.set(WEIGHT_SORT_KEY, 0u16).ok();
    let mut doc_net_weights = WeightSet::empty();
    doc_net_weights.set(WeightKind::Section, doc_net_weight);
    edges.push((parent_doc.bid, network.bid, doc_net_weights));

    // Children -> Parent Document (Section with indices 0, 1, 2)
    for (idx, child) in [&child1_doc, &child2_doc, &child3_doc].iter().enumerate() {
        let mut child_weight = Weight::default();
        child_weight.set(WEIGHT_SORT_KEY, idx as u16).ok();
        let mut child_weights = WeightSet::empty();
        child_weights.set(WeightKind::Section, child_weight);
        edges.push((child.bid, parent_doc.bid, child_weights));
    }

    let relations = BidGraph::from_edges(&edges);

    BeliefSet::new(states, relations).unwrap()
}
