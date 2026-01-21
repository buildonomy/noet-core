//! Tests for BeliefSet expression evaluation

use super::helpers::*;
use crate::{
    beliefset::BeliefSet,
    event::BeliefEvent,
    nodekey::NodeKey,
    properties::{BeliefKind, BeliefNode, Bid, Weight, WeightKind, WeightSet},
    query::{BeliefCache, Expression, RelationPred, SetOp, StatePred},
};
use petgraph::{visit::EdgeRef, Direction};
use std::collections::BTreeSet;
use test_log::test;

#[test]
fn test_evaluate_expression_state_in_any() {
    let beliefset = create_test_beliefset();
    let expr = Expression::StateIn(StatePred::Any);
    let result = beliefset.evaluate_expression(&expr);

    // Should return all states
    assert_eq!(result.states.len(), beliefset.states().len());
    assert_eq!(result.relations.as_graph().edge_count(), 2);
}

#[test]
fn test_evaluate_expression_state_in_bid() {
    let beliefset = create_test_beliefset();
    let bids: Vec<Bid> = beliefset
        .states()
        .values()
        .filter_map(|n| {
            if n.title == "Node 1" || n.title == "Node 2" {
                Some(n.bid)
            } else {
                None
            }
        })
        .collect();

    let expr = Expression::StateIn(StatePred::Bid(bids.clone()));
    let result = beliefset.evaluate_expression(&expr);

    // Should return all the specified nodes, but only nodes 1 and 2 are uncolored by the trace
    // kind.
    assert_eq!(result.states.len(), 4);
    assert!(result
        .states
        .get(&bids[0])
        .filter(|n| !n.kind.contains(BeliefKind::Trace))
        .is_some());
    assert!(result
        .states
        .get(&bids[1])
        .filter(|n| !n.kind.contains(BeliefKind::Trace))
        .is_some());
    // But all the relations, since Node1 and Node2 are disconnected
    assert_eq!(result.relations.as_graph().edge_count(), 2);
}

#[test]
fn test_evaluate_expression_state_not_in() {
    let beliefset = create_test_beliefset();
    let bids: Vec<Bid> = beliefset.states().keys().take(2).copied().collect();

    let expr = Expression::StateNotIn(StatePred::Bid(bids.clone()));
    let result = beliefset.evaluate_expression(&expr);

    // Should return all nodes except the specified ones
    assert_eq!(result.states.len(), beliefset.states().len() - 2);
    assert!(!result.states.contains_key(&bids[0]));
    assert!(!result.states.contains_key(&bids[1]));
}

#[test]
fn test_evaluate_expression_state_in_namespace() {
    let beliefset = create_test_beliefset();
    let first_bid = *beliefset.states().keys().next().unwrap();
    let bref = first_bid.namespace();

    let expr = Expression::StateIn(StatePred::InNamespace(vec![bref]));
    let result = beliefset.evaluate_expression(&expr);

    // Should return the node in that namespace, and all other nodes annotated with trace.
    assert!(result
        .states
        .values()
        .any(|n| n.title == "Node 2" && !n.kind.contains(BeliefKind::Trace)));
}

#[test]
fn test_evaluate_expression_relation_in_any() {
    let beliefset = create_test_beliefset();
    let expr = Expression::RelationIn(RelationPred::Any);
    let result = beliefset.evaluate_expression(&expr);

    // Should return all relations
    assert_eq!(result.relations.as_graph().edge_count(), 2);
    // Should include both source and sink nodes for each relation
    assert!(result.states.len() == 4);
}

#[test]
fn test_evaluate_expression_relation_in_source() {
    let beliefset = create_test_beliefset();
    let source_bid = *beliefset.states().keys().next().unwrap();

    let expr = Expression::RelationIn(RelationPred::SourceIn(vec![source_bid]));
    let result = beliefset.evaluate_expression(&expr);

    // Should only include relations with the specified source
    for edge in result.relations.as_graph().raw_edges() {
        let source = result.relations.as_graph()[edge.source()];
        assert_eq!(source, source_bid);
        // A relation query doesn't guarantee it will return all relations for the nodes in the
        // return set, so ensure the node is marked as a NodeKind::trace.
        let node = result.states.get(&source).unwrap();
        assert!(node.kind.contains(BeliefKind::Trace));
    }
}

#[test]
fn test_evaluate_expression_relation_in_sink() {
    let beliefset = create_test_beliefset();
    let all_bids: Vec<Bid> = beliefset.states().keys().copied().collect();
    let sink_bid = all_bids[2]; // Node 3, which is a sink

    let expr = Expression::RelationIn(RelationPred::SinkIn(vec![sink_bid]));
    let result = beliefset.evaluate_expression(&expr);

    assert_eq!(result.states.len(), 2);
    assert_eq!(result.relations.as_graph().edge_count(), 1);
    // Should only include relations with the specified sink
    for edge in result.relations.as_graph().raw_edges() {
        let sink = result.relations.as_graph()[edge.target()];
        assert_eq!(sink, sink_bid);
        // A relation query doesn't guarantee it will return all relations for the nodes in the
        // return set, so ensure the node is marked as a NodeKind::trace.
        let node = result.states.get(&sink).unwrap();
        assert!(node.kind.contains(BeliefKind::Trace));
    }
}

#[test]
fn test_evaluate_expression_relation_not_in() {
    let beliefset = create_test_beliefset();
    let source_bid = *beliefset.states().keys().next().unwrap();

    let expr = Expression::RelationNotIn(RelationPred::SourceIn(vec![source_bid]));
    let result = beliefset.evaluate_expression(&expr);

    // Should exclude relations with the specified source
    for edge in result.relations.as_graph().raw_edges() {
        let source = result.relations.as_graph()[edge.source()];
        assert_ne!(source, source_bid);
    }
}

#[test]
fn test_evaluate_expression_relation_kind() {
    let beliefset = create_test_beliefset();
    let mut weight_filter = WeightSet::empty();
    weight_filter.set(WeightKind::Section, Weight::default());

    let expr = Expression::RelationIn(RelationPred::Kind(weight_filter));
    let result = beliefset.evaluate_expression(&expr);

    // Should return all subsection relations
    assert_eq!(result.relations.as_graph().edge_count(), 2);
}

#[test]
fn test_evaluate_expression_dyad_union() {
    let beliefset = create_test_beliefset();
    let mut bids: Vec<(Bid, String)> = beliefset
        .states()
        .values()
        .map(|n| (n.bid, n.title.clone()))
        .collect();
    bids.sort_by(|(_, title_a), (_, title_b)| title_a.cmp(&title_b));

    let expr1 = Expression::StateIn(StatePred::Bid(vec![bids[0].0]));
    let expr2 = Expression::StateIn(StatePred::Bid(vec![bids[1].0]));
    let union_expr = Expression::Dyad(Box::new(expr1), SetOp::Union, Box::new(expr2));

    let result = beliefset.evaluate_expression(&union_expr);

    let non_trace_states = BTreeSet::from_iter(result.states.values().filter_map(|n| {
        if n.kind.is_complete() {
            Some(n.bid)
        } else {
            None
        }
    }));

    // Should contain both nodes
    assert_eq!(
        result.states.len(),
        4,
        "{}\n{:?}",
        result.display_contents(),
        bids
    );
    assert_eq!(
        non_trace_states.len(),
        2,
        "{}\n{:?}",
        result.display_contents(),
        bids
    );
    assert!(
        non_trace_states.contains(&bids[0].0),
        "{}",
        result.display_contents()
    );
    assert!(
        non_trace_states.contains(&bids[1].0),
        "{}",
        result.display_contents()
    );
}

#[test]
fn test_evaluate_expression_dyad_intersection() {
    let beliefset = create_test_beliefset();
    let mut bids: Vec<(Bid, String)> = beliefset
        .states()
        .values()
        .map(|n| (n.bid, n.title.clone()))
        .collect();
    bids.sort_by(|(_, title_a), (_, title_b)| title_a.cmp(&title_b));

    let expr1 = Expression::StateIn(StatePred::Bid(vec![bids[0].0, bids[1].0]));
    let expr2 = Expression::StateIn(StatePred::Bid(vec![bids[1].0, bids[2].0]));
    let intersection_expr = Expression::Dyad(Box::new(expr1), SetOp::Intersection, Box::new(expr2));

    let result = beliefset.evaluate_expression(&intersection_expr);
    let non_trace_states = BTreeSet::from_iter(result.states.values().filter_map(|n| {
        if n.kind.is_complete() {
            Some(n.bid)
        } else {
            None
        }
    }));

    // Should only contain bid[1]
    assert_eq!(non_trace_states.len(), 1, "{}", result.display_contents());
    assert!(
        non_trace_states.contains(&bids[1].0),
        "{}",
        result.display_contents()
    );
}

#[test]
fn test_evaluate_expression_dyad_difference() {
    let beliefset = create_test_beliefset();
    let mut bids: Vec<(Bid, String)> = beliefset
        .states()
        .values()
        .map(|n| (n.bid, n.title.clone()))
        .collect();
    bids.sort_by(|(_, title_a), (_, title_b)| title_a.cmp(&title_b));

    let expr1 = Expression::StateIn(StatePred::Bid(vec![bids[0].0, bids[1].0, bids[2].0]));
    let expr2 = Expression::StateIn(StatePred::Bid(vec![bids[2].0]));
    let difference_expr = Expression::Dyad(Box::new(expr1), SetOp::Difference, Box::new(expr2));

    let result = beliefset.evaluate_expression(&difference_expr);
    let non_trace_states = BTreeSet::from_iter(result.states.values().filter_map(|n| {
        if n.kind.is_complete() {
            Some(n.bid)
        } else {
            None
        }
    }));

    // its a little weird, but this should contain bids 0-2, because when we grab the relations
    // after performing the set difference, we discover that we are fully connected to bids[1]
    // (via the back path from bids[3] (which is trace colored) back to bids[1])
    assert_eq!(non_trace_states.len(), 3, "{}", result.display_contents());
    assert!(
        !non_trace_states.contains(&bids[3].0),
        "{}",
        result.display_contents()
    );
}

#[test]
fn test_evaluate_expression_dyad_symmetric_difference() {
    let beliefset = create_test_beliefset();
    let mut bids: Vec<(Bid, String)> = beliefset
        .states()
        .values()
        .map(|n| (n.bid, n.title.clone()))
        .collect();
    bids.sort_by(|(_, title_a), (_, title_b)| title_a.cmp(&title_b));

    // Contains 0
    let expr1 = Expression::StateIn(StatePred::Bid(vec![bids[0].0, bids[1].0]));
    let expr2 = Expression::StateIn(StatePred::Bid(vec![bids[1].0, bids[2].0]));
    let sym_diff_expr =
        Expression::Dyad(Box::new(expr1), SetOp::SymmetricDifference, Box::new(expr2));

    let result = beliefset.evaluate_expression(&sym_diff_expr);

    let non_trace_states = BTreeSet::from_iter(result.states.values().filter_map(|n| {
        if n.kind.is_complete() {
            Some(n.bid)
        } else {
            None
        }
    }));
    assert_eq!(non_trace_states.len(), 2, "{}", result.display_contents());
    assert!(
        non_trace_states.contains(&bids[0].0),
        "{}",
        result.display_contents()
    );
    assert!(
        non_trace_states.contains(&bids[2].0),
        "{}",
        result.display_contents()
    );
}

#[test]
fn test_evaluate_expression_nested_dyads() {
    let beliefset = create_test_beliefset();
    let mut bids: Vec<(Bid, String)> = beliefset
        .states()
        .values()
        .map(|n| (n.bid, n.title.clone()))
        .collect();
    bids.sort_by(|(_, title_a), (_, title_b)| title_a.cmp(&title_b));

    // (A ∪ B) ∩ (C ∪ D)
    let expr_a = Expression::StateIn(StatePred::Bid(vec![bids[0].0]));
    let expr_b = Expression::StateIn(StatePred::Bid(vec![bids[1].0]));
    let expr_c = Expression::StateIn(StatePred::Bid(vec![bids[1].0]));
    let expr_d = Expression::StateIn(StatePred::Bid(vec![bids[2].0]));

    // 0 -> 2(t), 1 -> 3(t)
    let union_ab = Expression::Dyad(Box::new(expr_a), SetOp::Union, Box::new(expr_b));
    // 1 -> 3(t), 1(t) -> 2
    let union_cd = Expression::Dyad(Box::new(expr_c), SetOp::Union, Box::new(expr_d));
    // 1 -> 3(t)
    let intersection_expr =
        Expression::Dyad(Box::new(union_ab), SetOp::Intersection, Box::new(union_cd));

    let result = beliefset.evaluate_expression(&intersection_expr);

    // Should only contain bids[1] (the intersection of {0,1} and {1,2})
    let non_trace_states = BTreeSet::from_iter(result.states.values().filter_map(|n| {
        if n.kind.is_complete() {
            Some(n.bid)
        } else {
            None
        }
    }));
    assert_eq!(non_trace_states.len(), 1, "{}", result.display_contents());
    assert!(
        non_trace_states.contains(&bids[1].0),
        "{}",
        result.display_contents()
    );
    assert_eq!(result.states.len(), 2, "{}", result.display_contents());
}

#[test]
fn test_evaluate_expression_empty_result() {
    let beliefset = create_test_beliefset();

    // Create a non-existent BID
    let nonexistent_bid = Bid::new(Bid::nil());
    let expr = Expression::StateIn(StatePred::Bid(vec![nonexistent_bid]));
    let result = beliefset.evaluate_expression(&expr);

    // Should return empty result
    assert_eq!(result.states.len(), 0);
    assert_eq!(result.relations.as_graph().edge_count(), 0);
}

#[test]
fn test_evaluate_expression_relations_follow_states() {
    let beliefset = create_test_beliefset();
    let bids: Vec<Bid> = beliefset.states().keys().copied().collect();

    // Select only the source node, relation should not be included
    let expr = Expression::StateIn(StatePred::Bid(vec![bids[0]]));
    let result = beliefset.evaluate_expression(&expr);

    // Includes bids[0] as well as its relation nodes (annotated with BeliefKind::Trace)
    assert_eq!(result.states.len(), 2);
    // Relations should only be included if both source and sink are in the state set
    assert_eq!(result.relations.as_graph().edge_count(), 1);
}

#[test]
fn test_evaluate_expression_state_in_bref() {
    let beliefset = create_test_beliefset();
    let first_bid = *beliefset.states().keys().next().unwrap();
    let bref = first_bid.namespace();

    let expr = Expression::StateIn(StatePred::Bref(vec![bref]));
    let result = beliefset.evaluate_expression(&expr);

    // Should return the node with matching bref
    assert!(result.states.contains_key(&first_bid));
}

#[test]
fn test_evaluate_expression_complex_relation_filter() {
    let beliefset = create_test_beliefset();
    let all_bids: Vec<Bid> = beliefset.states().keys().copied().collect();

    // Filter relations where either source or sink matches
    let expr = Expression::RelationIn(RelationPred::NodeIn(vec![all_bids[0]]));
    let result = beliefset.evaluate_expression(&expr);

    // Should include relations involving the specified node
    assert!(result.relations.as_graph().edge_count() >= 1);
}

#[test]
fn test_evaluate_expression_maintains_relation_weights() {
    let beliefset = create_test_beliefset();
    let expr = Expression::RelationIn(RelationPred::Any);
    let result = beliefset.evaluate_expression(&expr);

    // Verify that relation weights are preserved
    for edge in result.relations.as_graph().raw_edges() {
        assert!(!edge.weight.is_empty());
        // Check that subsection weight exists
        assert!(edge.weight.get(&WeightKind::Section).is_some());
    }
}

#[test]
fn test_evaluate_expression_state_preserves_node_properties() {
    let beliefset = create_test_beliefset();
    let first_bid = *beliefset.states().keys().next().unwrap();
    let original_node = beliefset.states().get(&first_bid).unwrap();

    let expr = Expression::StateIn(StatePred::Bid(vec![first_bid]));
    let result = beliefset.evaluate_expression(&expr);

    let result_node = result.states.get(&first_bid).unwrap();
    assert_eq!(result_node.title, original_node.title);
    assert_eq!(result_node.kind, original_node.kind);
    assert_eq!(result_node.bid, original_node.bid);
}

#[test]
fn test_evaluate_expression_empty_beliefset() {
    let beliefset = BeliefSet::empty();
    let expr = Expression::StateIn(StatePred::Any);
    let result = beliefset.evaluate_expression(&expr);

    // Should return empty result
    assert_eq!(result.states.len(), 0);
    assert_eq!(result.relations.as_graph().edge_count(), 0);
    assert!(result.is_empty());
}

#[tokio::test]
async fn test_evaluate_expression_subsection_chain_balancing() {
    init_logging();
    // Create a structure that mirrors the failing test:
    // API -> Network -> Document
    // where we query for Document and expect the balance to include Network->API

    let set = create_balanced_test_beliefset();
    let doc_node = set
        .get(&NodeKey::Title {
            net: Bid::nil(),
            title: "Parent Document".to_string(),
        })
        .unwrap();
    let network_node = set
        .get(&NodeKey::Title {
            net: Bid::nil(),
            title: "Test Network".to_string(),
        })
        .unwrap();
    let api_node = BeliefNode::api_state();

    // Now query for just the Document node (like cache_fetch does)
    let query_expr = Expression::StateIn(StatePred::Bid(vec![doc_node.bid]));
    let query_result = set
        .eval_query(
            &crate::query::Query {
                seed: query_expr,
                traverse: None,
            },
            true,
        )
        .await
        .unwrap();

    // The query result should include Document and its upstream relation to Network
    assert!(
        query_result.states.contains_key(&doc_node.bid),
        "WAT:\n{}",
        query_result
    );
    assert!(
        query_result.states.contains_key(&network_node.bid),
        "WAT:\n{}",
        query_result
    );

    // Now try to balance the query result
    let mut balanced_result = BeliefSet::from(query_result);

    // This should succeed - the balance should pull in the Network->API relation
    let balance_result = balanced_result.process_event(&BeliefEvent::BalanceCheck);

    // THIS IS WHERE THE BUG MANIFESTS:
    // If this assertion fails, it means the balance didn't include Network->API
    assert!(
        balance_result.is_ok(),
        "Failed to balance query result.\n\n Errors:\n\t- {}.\n\n\
             This indicates that querying for a document didn't properly include \
             the upstream network's connection to API during balancing. Api nodes: {}",
        balanced_result
            .errors()
            .iter()
            .map(|err_msg| err_msg.replace("\n", "\n\t"))
            .collect::<Vec<String>>()
            .join(",\n\t- "),
        balanced_result
            .states()
            .values()
            .filter(|n| n.kind.contains(BeliefKind::API))
            .map(|n| n.display_title())
            .collect::<Vec<String>>()
            .join(", ")
    );

    // Verify the balanced result includes all three nodes and both relations
    assert!(balanced_result.states().contains_key(&doc_node.bid));
    assert!(balanced_result.states().contains_key(&network_node.bid));
    assert!(balanced_result.states().contains_key(&api_node.bid));

    // Verify the Network->API relation is present
    let network_idx = balanced_result.bid_to_index(&network_node.bid).unwrap();
    let has_api_connection = balanced_result
        .relations()
        .as_graph()
        .edges_directed(network_idx, Direction::Outgoing)
        .any(|edge| {
            let sink = balanced_result.relations().as_graph()[edge.target()];
            sink == api_node.bid && edge.weight().get(&WeightKind::Section).is_some()
        });

    assert!(
        has_api_connection,
        "Balanced result is missing the Network->API Subsection relation"
    );
}
