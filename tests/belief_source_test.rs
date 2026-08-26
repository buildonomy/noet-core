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

use rustc_hash::FxHashMap;
use sqlx::Row;
use std::collections::BTreeSet;
use tempfile::tempdir;
use test_log::test;

use noet_core::{
    beliefbase::{BeliefBase, BeliefGraph, BidGraph},
    codec::DocumentCompiler,
    db::{db_init, DbConnection, Transaction},
    event::BeliefEvent,
    properties::{buildonomy_namespace, BeliefNode, BeliefRelation, Bid, WeightKind, WeightSet},
    query::{
        spec::{QueryPackage, QuerySpec, TapeFn},
        BeliefSource,
    },
};
use tokio::sync::mpsc::unbounded_channel;

#[path = "codec_test/common.rs"]
mod common;
use common::generate_test_root;

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

    let mut states = FxHashMap::default();

    // Network node
    states.insert(
        net_bid,
        BeliefNode {
            bid: net_bid,
            title: "Test Network".to_string(),
            schema: Some("buildonomy.Network".to_string()),
            ..Default::default()
        },
    );

    // Document 1
    states.insert(
        doc1_bid,
        BeliefNode {
            bid: doc1_bid,
            title: "Document 1".to_string(),
            schema: Some("buildonomy.Document".to_string()),
            ..Default::default()
        },
    );

    // Document 2
    states.insert(
        doc2_bid,
        BeliefNode {
            bid: doc2_bid,
            title: "Document 2".to_string(),
            schema: Some("buildonomy.Document".to_string()),
            ..Default::default()
        },
    );

    // Section 1 (child of doc1)
    states.insert(
        section1_bid,
        BeliefNode {
            bid: section1_bid,
            title: "Section 1".to_string(),
            schema: Some("buildonomy.Section".to_string()),
            ..Default::default()
        },
    );

    // Section 2 (child of doc2)
    states.insert(
        section2_bid,
        BeliefNode {
            bid: section2_bid,
            title: "Section 2".to_string(),
            schema: Some("buildonomy.Section".to_string()),
            ..Default::default()
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

    // Now run equivalence tests on various query types via evaluate()
    tracing::info!("Starting equivalence tests for various query types");

    // Helper closure: run the same QuerySpec against both backends and return graphs
    async fn eval_both(
        bb: &BeliefBase,
        db: &DbConnection,
        spec: QuerySpec,
    ) -> Result<(BeliefGraph, BeliefGraph), noet_core::BuildonomyError> {
        let mut pkg_bb = QueryPackage::new(spec.clone());
        bb.evaluate(&mut pkg_bb).await?;
        let bb_graph = pkg_bb.into_graph();

        let mut pkg_db = QueryPackage::new(spec);
        db.evaluate(&mut pkg_db).await?;
        let db_graph = pkg_db.into_graph();

        Ok((bb_graph, db_graph))
    }

    // Test 1: Query specific BIDs
    tracing::info!("Test 1: TapeFn::Bids (specific BIDs)");
    let sample_bids = vec![doc1_bid, section1_bid, doc2_bid];
    let spec_bids = QuerySpec::seed(TapeFn::Bids(sample_bids.clone()));
    let (session_result, db_result) = eval_both(&test_bb, &db, spec_bids).await?;
    assert_belief_graphs_equivalent(
        &session_result,
        &db_result,
        &format!("Bids({:?}) should return identical results", sample_bids),
    );

    // Test 2: Query all BIDs (export_beliefgraph equivalence)
    tracing::info!("Test 2: export_beliefgraph");
    let session_export = test_bb.export_beliefgraph().await?;
    let db_export = db.export_beliefgraph().await?;
    assert_belief_graphs_equivalent(
        &session_export,
        &db_export,
        "export_beliefgraph should return identical results",
    );

    tracing::info!("All BeliefSource equivalence tests PASSED \u{2705}");
    Ok(())
}

/// Regression test: `DbConnection::submap_by_bid` must return a leaf document's
/// own subtree (itself + its section children), matching `BeliefBase`'s behavior,
/// when the entry BID is a leaf document nested inside a non-root network.
///
/// ## Background
///
/// `parse_epoch`'s per-document session pre-seeding (`compiler.rs`) calls
/// `global_bb.submap_by_bid(net_bid, Some(doc_bid), 0, true)` before spawning each
/// file's parse task, to warm `session_bb` with the document's own prior-epoch
/// subtree and avoid expensive per-node `cache_fetch` fallbacks to `global_bb`
/// during Phase 1. This only helps if the seed is non-empty.
///
/// `DbConnection::submap_by_bid` resolves the entry BID's own `(net, path)` via
/// a `paths` table lookup, then re-feeds that leaf path into the free function
/// `submap()`, which was written to walk a path that may descend through nested
/// subnets (splitting on `/` and treating each segment as a child-network hop).
/// For a leaf document whose own path contains no `/` (e.g. `"subnet1_file1.md"`),
/// this walk treats the *whole filename* as a subnet-lookup key under the parent
/// network, which — because a document's own bref never resolves as a `path`
/// row under itself — resolves back to the *document's own BID* and recurses
/// with `network_bid = doc_bid`. Ordinary (non-network) documents have no `paths`
/// rows with `net = <their own bref>`, so the recursive `SELECT ... WHERE net = ?`
/// silently returns zero rows, and `submap_by_bid` returns `Ok(vec![])` instead of
/// the document's section subtree.
///
/// This is invisible for index.md / network-root documents (whose own BID *is*
/// a legitimate network bref with real `paths` rows under it, so the buggy
/// re-descent accidentally lands on real data) — which is why this bug went
/// undetected: it only manifests for ordinary leaf documents nested one or more
/// levels below the repo root.
#[test(tokio::test)]
async fn test_submap_by_bid_leaf_document_equivalence() -> Result<(), Box<dyn std::error::Error>> {
    // `network_1` has `subnet1/subnet1_file1.md` — an ordinary leaf document
    // (title "subnet1 file1") nested one level inside the `subnet1` subnet.
    // This is exactly the shape that triggers the bug: a non-root, non-network
    // document whose own path has no `/` segment.
    let (_tmp, test_root) = generate_test_root("network_1")?;

    // Parse into an in-memory BeliefBase (ground truth).
    let mut bb_global = BeliefBase::empty();
    let (tx_bb, mut rx_bb) = unbounded_channel::<BeliefEvent>();
    let mut compiler_bb = DocumentCompiler::new(&test_root, Some(tx_bb), None, false)?;
    compiler_bb
        .parse_sequential(&mut bb_global, false, Some(&mut rx_bb))
        .await?;
    while let Ok(event) = rx_bb.try_recv() {
        bb_global.process_event(&event)?;
    }

    // Parse the same fixture into a fresh file-backed DB (the buggy backend).
    let test_tempdir = tempdir()?;
    let db_path = test_tempdir.path().join("test_belief_cache.db");
    let db_pool = db_init(db_path).await?;
    let db = DbConnection(db_pool);
    let (tx_db, mut rx_db) = unbounded_channel::<BeliefEvent>();
    let mut compiler_db = DocumentCompiler::new(&test_root, Some(tx_db), None, false)?;
    compiler_db
        .parse_sequential(&mut db.clone(), false, Some(&mut rx_db))
        .await?;
    let mut transaction = Transaction::default();
    while let Ok(event) = rx_db.try_recv() {
        transaction.add_event(&event).ok();
    }
    transaction.execute(&db.0).await?;

    // Locate the leaf document's BID and owning network's BID in each backend.
    let leaf_bid_bb = bb_global
        .states()
        .values()
        .find(|n| n.title == "subnet1 file1")
        .map(|n| n.bid)
        .expect("subnet1_file1.md should be parsed into bb_global");
    let net_bid_bb = bb_global
        .states()
        .values()
        .find(|n| matches!(&n.id, noet_core::properties::NodeId::Explicit(s) if s == "belief-network-test-1-subnet-1"))
        .map(|n| n.bid)
        .expect("subnet1 network node should be parsed into bb_global");

    let leaf_bid_db: String = sqlx::query("SELECT bid FROM beliefs WHERE title = ?")
        .bind("subnet1 file1")
        .fetch_one(&db.0)
        .await?
        .get("bid");
    let leaf_bid_db = Bid::try_from(leaf_bid_db.as_str())?;
    let net_bid_db: String = sqlx::query("SELECT bid FROM beliefs WHERE id = ?")
        .bind("belief-network-test-1-subnet-1")
        .fetch_one(&db.0)
        .await?
        .get("bid");
    let net_bid_db = Bid::try_from(net_bid_db.as_str())?;

    // Note: BIDs are freshly generated per parse (the fixture sets no explicit
    // `bid` on this document), so leaf_bid_bb/leaf_bid_db and net_bid_bb/net_bid_db
    // are *not* expected to be equal across the two independent parses — each
    // backend is queried with its own BIDs below.

    // The ground-truth in-memory backend must return a non-empty subtree for
    // the leaf document (itself, at minimum).
    let bb_submap = bb_global
        .submap_by_bid(net_bid_bb, Some(leaf_bid_bb), 0, true)
        .await?;
    assert!(
        !bb_submap.is_empty(),
        "BeliefBase::submap_by_bid should return a non-empty subtree for a leaf \
         document; got {bb_submap:?}"
    );

    // The DB backend must match — this is the regression check. Prior to the
    // fix, DbConnection::submap_by_bid returns Ok(vec![]) here because its
    // path-segment walk misidentifies the leaf document's own BID as a
    // subnet and finds no `paths` rows under it.
    let db_submap = db
        .submap_by_bid(net_bid_db, Some(leaf_bid_db), 0, true)
        .await?;
    assert!(
        !db_submap.is_empty(),
        "DbConnection::submap_by_bid returned an empty subtree for leaf document \
         'subnet1 file1' (bid={leaf_bid_db}, net={net_bid_db}); expected at least \
         the document's own entry, matching BeliefBase's {} entries: {:?}. \
         This reproduces the per-doc session-seeding bug: parse_epoch's pre-spawn \
         seed (compiler.rs) silently falls back to an empty BeliefGraph for every \
         non-network leaf document, defeating session_bb pre-seeding and forcing \
         Phase 1 to hit global_bb per-node on reparse.",
        bb_submap.len(),
        bb_submap
    );

    // BIDs are ephemeral across independent parse runs, so compare structural
    // position (path strings and orderings) rather than raw BID values.
    assert_eq!(
        bb_submap.len(),
        db_submap.len(),
        "BeliefBase and DbConnection should return the same number of entries \
         for submap_by_bid on a leaf document; bb={bb_submap:?}, db={db_submap:?}"
    );
    let bb_paths: BTreeSet<(String, Vec<u16>)> = bb_submap
        .iter()
        .map(|(p, _, order)| (p.clone(), order.clone()))
        .collect();
    let db_paths: BTreeSet<(String, Vec<u16>)> = db_submap
        .iter()
        .map(|(p, _, order)| (p.clone(), order.clone()))
        .collect();
    assert_eq!(
        bb_paths, db_paths,
        "BeliefBase and DbConnection should return the same (path, ordering) set \
         for submap_by_bid on a leaf document"
    );

    Ok(())
}

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
    use petgraph::visit::{EdgeRef, IntoEdgeReferences};
    let session_edges: BTreeSet<(Bid, Bid)> = session_graph
        .relations
        .as_graph()
        .edge_references()
        .map(|e| {
            let source = session_graph.relations.as_graph()[e.source()];
            let target = session_graph.relations.as_graph()[e.target()];
            (source, target)
        })
        .collect();

    let db_edges: BTreeSet<(Bid, Bid)> = db_graph
        .relations
        .as_graph()
        .edge_references()
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
