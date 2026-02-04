//! BeliefSource Equivalence Tests
//!
//! Tests that different BeliefSource implementations (BeliefBase in-memory vs DbConnection)
//! return identical results for the same queries.
//!
//! This validates Issue 34 fix: cache stability and orphaned edge handling.
//!
//! ## Trace Node Handling
//!
//! Trace nodes are an important part of query results - they indicate nodes with incomplete
//! relation sets. The equivalence test verifies that:
//! 1. Both sources return the same set of nodes (including Trace nodes)
//! 2. Both sources mark the same nodes as Trace (consistent completeness metadata)
//! 3. Relations match exactly
//!
//! For RelationIn queries, both sources should mark all returned nodes as Trace since
//! we're not guaranteeing complete relation sets for matching nodes.

#![cfg(feature = "service")]

use sqlx::Row;
use std::collections::{BTreeMap, BTreeSet};
use tempfile::tempdir;
use test_log::test;

use noet_core::{
    beliefbase::{BeliefBase, BeliefGraph, BidGraph},
    db::{db_init, DbConnection, Transaction},
    properties::{buildonomy_namespace, BeliefNode, BeliefRelation, Bid, WeightKind, WeightSet},
    query::{BeliefSource, Expression, RelationPred, StatePred},
};

/// Test that DbConnection and BeliefBase return identical results for the same queries
///
/// Test design:
/// 1. Manually build a BeliefBase with known test data
/// 2. Use compute_diff to generate events (test_bb vs empty)
/// 3. Populate DB with those events via Transaction
/// 4. Run identical queries on both BeliefBase and DbConnection
/// 5. Compare BeliefGraph results
#[test(tokio::test)]
async fn test_belief_source_equivalence() -> Result<(), Box<dyn std::error::Error>> {
    tracing::info!("Testing BeliefSource equivalence: DbConnection vs BeliefBase (Issue 34)");

    // Initialize DB
    let test_tempdir = tempdir()?;
    let db_path = test_tempdir.path().join("test_belief_cache.db");
    let db_pool = db_init(db_path).await?;
    let db = DbConnection(db_pool);

    // Manually build a test BeliefBase with known data
    tracing::info!("Building test BeliefBase with known nodes and relations");

    let net_bid = Bid::new(buildonomy_namespace());
    let doc1_bid = Bid::new(net_bid);
    let doc2_bid = Bid::new(net_bid);
    let section1_bid = Bid::new(doc1_bid);
    let section2_bid = Bid::new(doc1_bid);

    let mut states = BTreeMap::new();

    // Network node
    states.insert(
        net_bid,
        BeliefNode {
            bid: net_bid,
            kind: Default::default(),
            title: "Test Network".to_string(),
            schema: Some("buildonomy.Network".to_string()),
            payload: Default::default(),
            id: None,
        },
    );

    // Document 1
    states.insert(
        doc1_bid,
        BeliefNode {
            bid: doc1_bid,
            kind: Default::default(),
            title: "Document 1".to_string(),
            schema: Some("buildonomy.Document".to_string()),
            payload: Default::default(),
            id: None,
        },
    );

    // Document 2
    states.insert(
        doc2_bid,
        BeliefNode {
            bid: doc2_bid,
            kind: Default::default(),
            title: "Document 2".to_string(),
            schema: Some("buildonomy.Document".to_string()),
            payload: Default::default(),
            id: None,
        },
    );

    // Section 1 (child of doc1)
    states.insert(
        section1_bid,
        BeliefNode {
            bid: section1_bid,
            kind: Default::default(),
            title: "Section 1".to_string(),
            schema: Some("buildonomy.Section".to_string()),
            payload: Default::default(),
            id: None,
        },
    );

    // Section 2 (child of doc2)
    states.insert(
        section2_bid,
        BeliefNode {
            bid: section2_bid,
            kind: Default::default(),
            title: "Section 2".to_string(),
            schema: Some("buildonomy.Section".to_string()),
            payload: Default::default(),
            id: None,
        },
    );

    // Build relations: doc1 -> section1, doc2 -> section2, net -> doc1, net -> doc2
    let edges = vec![
        BeliefRelation {
            source: doc1_bid,
            sink: section1_bid,
            weights: WeightSet::from(WeightKind::Section),
        },
        BeliefRelation {
            source: doc2_bid,
            sink: section2_bid,
            weights: WeightSet::from(WeightKind::Section),
        },
        BeliefRelation {
            source: net_bid,
            sink: doc1_bid,
            weights: WeightSet::from(WeightKind::Section),
        },
        BeliefRelation {
            source: net_bid,
            sink: doc2_bid,
            weights: WeightSet::from(WeightKind::Section),
        },
    ];

    let relations = BidGraph::from_edges(edges);
    let test_bb = BeliefBase::new(states, relations)?;

    tracing::info!(
        "Test BeliefBase created: {} states, {} relations",
        test_bb.states().len(),
        test_bb.relations().as_graph().edge_count()
    );

    // Generate events via compute_diff (empty vs test_bb = all adds)
    tracing::info!("Generating events via compute_diff");
    let empty_bb = BeliefBase::empty();
    // Include all BIDs in parsed_nodes so they get included in diff
    let parsed_nodes: BTreeSet<Bid> = test_bb.states().keys().copied().collect();
    let diff_events = BeliefBase::compute_diff(&empty_bb, &test_bb, &parsed_nodes)?;
    tracing::info!("Generated {} diff events", diff_events.len());

    // Populate DB with events
    let mut transaction = Transaction::default();
    for event in diff_events {
        transaction.add_event(&event).ok();
    }
    transaction.execute(&db.0).await?;
    tracing::info!("Events committed to DB");

    // Verify DB has correct content
    let verify_count = sqlx::query("SELECT COUNT(*) as count FROM beliefs")
        .fetch_one(&db.0)
        .await?;
    let db_node_count: i64 = verify_count.get("count");
    tracing::info!("DB contains {} nodes", db_node_count);

    assert_eq!(
        db_node_count as usize,
        test_bb.states().len(),
        "DB should contain same number of nodes as test_bb"
    );

    // Now run equivalence tests on various query types
    tracing::info!("Starting equivalence tests for various Expression types");

    // Test 1: Query all states
    tracing::info!("Test 1: Expression::StateIn(StatePred::Any)");
    let expr_all = Expression::StateIn(StatePred::Any);
    let session_result = test_bb.eval_unbalanced(&expr_all).await?;
    let db_result = db.eval_unbalanced(&expr_all).await?;

    assert_belief_graphs_equivalent(
        &session_result,
        &db_result,
        "StateIn(Any) should return identical results",
    );

    // Test 2: Query specific BIDs
    tracing::info!("Test 2: Expression::StateIn(StatePred::Bid(...))");
    let sample_bids = vec![doc1_bid, section1_bid, doc2_bid];
    let expr_bids = Expression::StateIn(StatePred::Bid(sample_bids.clone()));
    let session_result = test_bb.eval_unbalanced(&expr_bids).await?;
    let db_result = db.eval_unbalanced(&expr_bids).await?;

    assert_belief_graphs_equivalent(
        &session_result,
        &db_result,
        &format!(
            "StateIn(Bid({:?})) should return identical results",
            sample_bids
        ),
    );

    // Test 3: Query by schema
    tracing::info!("Test 3: Expression::StateIn(StatePred::Schema(...))");
    let doc_schema = "buildonomy.Document".to_string();
    let expr_schema = Expression::StateIn(StatePred::Schema(doc_schema.clone()));
    let session_result = test_bb.eval_unbalanced(&expr_schema).await?;
    let db_result = db.eval_unbalanced(&expr_schema).await?;

    assert_belief_graphs_equivalent(
        &session_result,
        &db_result,
        &format!(
            "StateIn(Schema({})) should return identical results",
            doc_schema
        ),
    );

    // Test 4: Query relations
    tracing::info!("Test 4: Expression::RelationIn(RelationPred::Any)");
    let expr_relations = Expression::RelationIn(RelationPred::Any);
    let session_result = test_bb.eval_unbalanced(&expr_relations).await?;
    let db_result = db.eval_unbalanced(&expr_relations).await?;

    assert_belief_graphs_equivalent(
        &session_result,
        &db_result,
        "RelationIn(Any) should return identical results",
    );

    // Test 5: eval_trace
    tracing::info!("Test 5: eval_trace with Subsection filter");
    let session_trace = test_bb
        .eval_trace(&expr_all, WeightSet::from(WeightKind::Section))
        .await?;
    let db_trace = db
        .eval_trace(&expr_all, WeightSet::from(WeightKind::Section))
        .await?;

    assert_belief_graphs_equivalent(
        &session_trace,
        &db_trace,
        "eval_trace should return identical results",
    );

    tracing::info!("All BeliefSource equivalence tests PASSED ✅");
    Ok(())
}

/// Helper function to assert two BeliefGraphs are equivalent
fn assert_belief_graphs_equivalent(
    session_graph: &BeliefGraph,
    db_graph: &BeliefGraph,
    message: &str,
) {
    use std::collections::BTreeSet;

    // Compare ALL states (including Trace nodes)
    let session_all_bids: BTreeSet<Bid> = session_graph.states.keys().copied().collect();

    let db_all_bids: BTreeSet<Bid> = db_graph.states.keys().copied().collect();

    let session_only = &session_all_bids - &db_all_bids;
    let db_only = &db_all_bids - &session_all_bids;

    if !session_only.is_empty() || !db_only.is_empty() {
        tracing::error!("State BID mismatch for: {}", message);
        tracing::error!("Session has {} states", session_all_bids.len());
        tracing::error!("DB has {} states", db_all_bids.len());

        if !session_only.is_empty() {
            tracing::error!("BIDs only in session_bb: {:?}", session_only);
        }
        if !db_only.is_empty() {
            tracing::error!("BIDs only in db: {:?}", db_only);
        }

        panic!("{} - State BID sets differ", message);
    }

    // Compare Trace marking consistency
    let session_trace_bids: BTreeSet<Bid> = session_graph
        .states
        .values()
        .filter(|n| !n.kind.is_complete())
        .map(|n| n.bid)
        .collect();

    let db_trace_bids: BTreeSet<Bid> = db_graph
        .states
        .values()
        .filter(|n| !n.kind.is_complete())
        .map(|n| n.bid)
        .collect();

    let session_trace_only = &session_trace_bids - &db_trace_bids;
    let db_trace_only = &db_trace_bids - &session_trace_bids;

    if !session_trace_only.is_empty() || !db_trace_only.is_empty() {
        tracing::error!("Trace marking mismatch for: {}", message);
        tracing::error!("Session has {} Trace nodes", session_trace_bids.len());
        tracing::error!("DB has {} Trace nodes", db_trace_bids.len());

        if !session_trace_only.is_empty() {
            tracing::error!("Trace only in session_bb: {:?}", session_trace_only);
        }
        if !db_trace_only.is_empty() {
            tracing::error!("Trace only in db: {:?}", db_trace_only);
        }

        panic!("{} - Trace marking differs", message);
    }

    // Compare relations (edge count and structure)
    let session_edge_count = session_graph.relations.as_graph().edge_count();
    let db_edge_count = db_graph.relations.as_graph().edge_count();

    assert_eq!(
        session_edge_count, db_edge_count,
        "{} - Relation count differs: session={}, db={}",
        message, session_edge_count, db_edge_count
    );

    // Compare specific edges (source -> sink pairs)
    let session_edges: BTreeSet<(Bid, Bid)> = session_graph
        .relations
        .as_graph()
        .raw_edges()
        .iter()
        .map(|e| {
            let source = session_graph.relations.as_graph()[e.source()];
            let target = session_graph.relations.as_graph()[e.target()];
            (source, target)
        })
        .collect();

    let db_edges: BTreeSet<(Bid, Bid)> = db_graph
        .relations
        .as_graph()
        .raw_edges()
        .iter()
        .map(|e| {
            let source = db_graph.relations.as_graph()[e.source()];
            let target = db_graph.relations.as_graph()[e.target()];
            (source, target)
        })
        .collect();

    let session_only_edges = &session_edges - &db_edges;
    let db_only_edges = &db_edges - &session_edges;

    if !session_only_edges.is_empty() || !db_only_edges.is_empty() {
        tracing::error!("Edge mismatch for: {}", message);

        if !session_only_edges.is_empty() {
            tracing::error!("Edges only in session_bb: {:?}", session_only_edges);
        }
        if !db_only_edges.is_empty() {
            tracing::error!("Edges only in db: {:?}", db_only_edges);
        }

        panic!("{} - Edge sets differ", message);
    }

    tracing::info!("✅ {} - Graphs are equivalent", message);
}
