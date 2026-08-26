//! Tests for QuerySpec evaluation pipeline

use super::helpers::*;
use crate::beliefbase::{BeliefBase, BidGraph};
use crate::nodekey::NodeKey;
use crate::paths::to_anchor;
use crate::properties::{Bref, Weight, WeightKind, WeightSet, WEIGHT_SORT_KEY};
use crate::query::{
    parser,
    spec::{QueryPackage, QuerySpec, TapeFn},
    BeliefSource,
};
use petgraph::{visit::EdgeRef, Direction};
use std::collections::BTreeSet;

#[tokio::test]
async fn test_evaluate_expression_subsection_chain_balancing() {
    init_logging();
    // Create a structure that mirrors the failing test:
    // API -> Network -> Document
    // where we query for Document and expect the balance to include Network->API

    let set = create_balanced_test_beliefbase();
    let doc_node = set
        .get(&NodeKey::Id {
            net: Bref::default(),
            id: to_anchor("Parent Document"),
        })
        .unwrap();
    let network_node = set
        .get(&NodeKey::Id {
            net: Bref::default(),
            id: to_anchor("Test Network"),
        })
        .unwrap();
    let api_node = crate::properties::BeliefNode::api_state();

    // Evaluate via QueryPackage::balanced, which triggers the full
    // evaluate_query pipeline including halo/ancestry/balance.
    let spec = QuerySpec::seed(TapeFn::Bids(vec![doc_node.bid]));
    let mut package = QueryPackage::balanced(spec);
    set.evaluate(&mut package).await.unwrap();
    let graph = package.into_graph();
    let balanced_result = BeliefBase::from(graph);

    // Verify the balanced result includes all three nodes and both relations
    assert!(balanced_result.states().contains_key(&doc_node.bid));
    assert!(balanced_result.states().contains_key(&network_node.bid));
    assert!(
        balanced_result.states().contains_key(&api_node.bid),
        "Balanced result must include the API node after balanced evaluation"
    );

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

// ── Composition evaluation integration tests ─────────────────────────────
//
// These tests build a BeliefBase with Section and Pragmatic edges, then
// evaluate parsed query strings end-to-end through the projection pipeline.

use crate::properties::Bid;

/// Test fixture: BeliefBase with mixed edge types and named BIDs.
///
/// Graph edges (source → sink):
/// ```text
///   A ──section──> Root     (A is child of Root)
///   B ──section──> Root
///   C ──section──> Root
///   B ──pragmatic──> A      (A uses B: B is provider, A is consumer)
///   C ──pragmatic──> B      (B uses C: C is provider, B is consumer)
/// ```
///
/// Traversal semantics:
/// - `A uses(1)` (k-pragmatic-s) = {B}  (A consumes B)
/// - `B uses(1)` = {C}                  (B consumes C)
/// - `C uses(1)` = {}                   (C consumes nothing)
/// - `A used_by(1)` (s-pragmatic-k) = {} (nothing consumes A)
/// - `B used_by(1)` = {A}               (A consumes B)
/// - `C used_by(1)` = {B}               (B consumes C)
struct CompositionFixture {
    bb: BeliefBase,
    root: Bid,
    a: Bid,
    b: Bid,
    c: Bid,
}

fn create_composition_fixture() -> CompositionFixture {
    init_logging();
    let mut states = rustc_hash::FxHashMap::default();

    let root = create_test_node("Root", crate::properties::BeliefKind::Network);
    let node_a = create_test_node("Node A", crate::properties::BeliefKind::Document);
    let node_b = create_test_node("Node B", crate::properties::BeliefKind::Document);
    let node_c = create_test_node("Node C", crate::properties::BeliefKind::Document);

    let bid_root = root.bid;
    let bid_a = node_a.bid;
    let bid_b = node_b.bid;
    let bid_c = node_c.bid;

    states.insert(root.bid, root);
    states.insert(node_a.bid, node_a);
    states.insert(node_b.bid, node_b);
    states.insert(node_c.bid, node_c);

    let mut edges = Vec::new();

    // Section edges: A, B, C -> Root
    for (idx, bid) in [bid_a, bid_b, bid_c].iter().enumerate() {
        let mut w = Weight::default();
        w.set(WEIGHT_SORT_KEY, idx as u16).ok();
        let mut ws = WeightSet::empty();
        ws.set(WeightKind::Section, w);
        edges.push((*bid, bid_root, ws));
    }

    // Pragmatic edges: B → A (A uses B), C → B (B uses C)
    // In the graph model, edges go source → sink. "A uses B" means
    // B is the source (provider), A is the sink (consumer).
    for (source, sink) in [(bid_b, bid_a), (bid_c, bid_b)] {
        let mut w = Weight::default();
        w.set(WEIGHT_SORT_KEY, 0u16).ok();
        let mut ws = WeightSet::empty();
        ws.set(WeightKind::Pragmatic, w);
        edges.push((source, sink, ws));
    }

    let relations = BidGraph::from_edges(&edges);
    let bb = BeliefBase::new_unbalanced(states, relations, false);

    CompositionFixture {
        bb,
        root: bid_root,
        a: bid_a,
        b: bid_b,
        c: bid_c,
    }
}

/// Helper: parse a query, evaluate against a BeliefBase, return the output BID set.
fn eval_query(bb: &BeliefBase, query: &str) -> BTreeSet<Bid> {
    let spec = parser::parse(query).unwrap_or_else(|e| panic!("parse failed for '{query}': {e}"));
    let mut package = QueryPackage::new(spec);
    bb.evaluate_query(&mut package)
        .unwrap_or_else(|e| panic!("eval failed for '{query}': {e}"));
    package
        .tape()
        .steps
        .last()
        .map(|e| e.content.output_bids().into_iter().collect())
        .unwrap_or_default()
}

#[test]
fn composition_and_evaluates() {
    let f = create_composition_fixture();

    // A uses(1) = {B}
    let result_a = eval_query(&f.bb, &format!("bid:{} uses(1)", f.a));
    assert_eq!(result_a, BTreeSet::from([f.b]), "A uses(1) should be {{B}}");

    // B used_by(1) = {A}
    let result_b = eval_query(&f.bb, &format!("bid:{} used_by(1)", f.b));
    assert_eq!(
        result_b,
        BTreeSet::from([f.a]),
        "B used_by(1) should be {{A}}"
    );

    // AND: {B} ∩ {A} = {} (disjoint)
    let result_and = eval_query(
        &f.bb,
        &format!("bid:{} uses(1) AND bid:{} used_by(1)", f.a, f.b),
    );
    assert!(
        result_and.is_empty(),
        "AND of disjoint sets should be empty"
    );
}

#[test]
fn composition_or_evaluates() {
    let f = create_composition_fixture();

    // OR: union of A's targets with B's sources = {B} ∪ {A} = {A, B}
    let result_or = eval_query(
        &f.bb,
        &format!("bid:{} uses(1) OR bid:{} used_by(1)", f.a, f.b),
    );
    assert!(result_or.contains(&f.a), "OR should include A");
    assert!(result_or.contains(&f.b), "OR should include B");
}

#[test]
fn composition_not_with_inverted_traversal() {
    let f = create_composition_fixture();

    // Root's section children = {A, B, C}
    let children = eval_query(&f.bb, &format!("bid:{} composed_of(1)", f.root));
    assert_eq!(children.len(), 3);

    // !uses(1) = nodes that consume nothing = {C}
    let non_consumers = eval_query(&f.bb, &format!("bid:{} composed_of(1) !uses(1)", f.root));
    assert_eq!(
        non_consumers,
        BTreeSet::from([f.c]),
        "!uses(1) should give nodes that consume nothing"
    );

    // children NOT !uses(1) = nodes that DO consume something = {A, B}
    let consumers = eval_query(
        &f.bb,
        &format!(
            "bid:{r} composed_of(1) NOT (bid:{r} composed_of(1) !uses(1))",
            r = f.root
        ),
    );
    assert_eq!(
        consumers,
        BTreeSet::from([f.a, f.b]),
        "children NOT !uses(1) = consumers"
    );
}

#[test]
fn inverted_traversal_gives_nodes_without_edges() {
    let f = create_composition_fixture();

    // !uses(1): children that use nothing = {C}
    let orphans = eval_query(&f.bb, &format!("bid:{} composed_of(1) !uses(1)", f.root));
    assert_eq!(orphans, BTreeSet::from([f.c]), "only C consumes nothing");

    // !used_by(1): children that nothing depends on = {A}
    // (B is used by A, C is used by B, but nothing uses A)
    let unused = eval_query(&f.bb, &format!("bid:{} composed_of(1) !used_by(1)", f.root));
    assert_eq!(unused, BTreeSet::from([f.a]), "nothing depends on A");
}

#[test]
fn terminal_fold_evaluates() {
    let f = create_composition_fixture();

    // uses(*) from A: depth 1 = {B}, depth 2 = {C}
    // FOLD(UNION) should collapse to {B, C}
    let result = eval_query(&f.bb, &format!("bid:{} uses(*) FOLD(UNION)", f.a));
    assert!(
        result.contains(&f.b) && result.contains(&f.c),
        "FOLD(UNION) should include all depths: got {:?}",
        result
    );
}

#[test]
fn grouped_composition_with_continuation_evaluates() {
    let f = create_composition_fixture();

    // First verify the composition alone gives {A, B}
    let compose_only = eval_query(
        &f.bb,
        &format!(
            "bid:{r} composed_of(1) NOT (bid:{r} composed_of(1) !uses(1))",
            r = f.root
        ),
    );
    assert_eq!(
        compose_only,
        BTreeSet::from([f.a, f.b]),
        "composition alone should give consumers A, B"
    );

    // THEN uses(1) from {A, B}: A→B and B→C.
    // Traversal excludes nodes already in the input set from output
    // (visited-set cycle prevention), so B is excluded → only {C}.
    let query = format!(
        "(bid:{r} composed_of(1) NOT (bid:{r} composed_of(1) !uses(1))) THEN uses(1)",
        r = f.root
    );
    let spec = parser::parse(&query).unwrap();
    let mut package = QueryPackage::new(spec);
    f.bb.evaluate_query(&mut package).unwrap();

    // Dump tape for debugging
    for (i, entry) in package.tape().steps.iter().enumerate() {
        let bids = entry.content.output_bids();
        let bid_labels: Vec<String> = bids
            .iter()
            .map(|b| {
                if *b == f.a {
                    "A".into()
                } else if *b == f.b {
                    "B".into()
                } else if *b == f.c {
                    "C".into()
                } else if *b == f.root {
                    "Root".into()
                } else {
                    format!("{}", b)
                }
            })
            .collect();
        eprintln!(
            "tape[{i}] label={:?} content_type={} bids={:?}",
            entry.label,
            match &entry.content {
                crate::query::spec::TapeContent::Nodes(_) => "Nodes",
                crate::query::spec::TapeContent::Edges { .. } => "Edges",
                crate::query::spec::TapeContent::Compose { .. } => "Compose",
                _ => "Other",
            },
            bid_labels,
        );
    }

    let result: BTreeSet<_> = package
        .tape()
        .steps
        .last()
        .map(|e| e.content.output_bids().into_iter().collect())
        .unwrap_or_default();
    assert_eq!(
        result,
        BTreeSet::from([f.c]),
        "uses(1) from {{A,B}} gives {{C}} (B excluded by visited-set)"
    );
}

/// `lookup_edges` must return edges incident to the given BIDs, even when the
/// neighbour on the other end is not itself in the list.
///
/// Regression: it built `QueryPackage::new`, but `materialize_graph` only copies
/// an edge when *both* endpoints are in the result set. A seed-only package
/// therefore returned the seed nodes with zero edges whenever the neighbour was
/// absent from `bids` -- silently, and regardless of what the store held. The
/// halo step that `QueryPackage::balanced` appends is what pulls the neighbour
/// in and makes the documented "incident to" contract true.
///
/// Caught while building rename expansion in the accumulator, which asks for the
/// edges incident to one absorbed BID and got nothing back.
#[tokio::test]
async fn test_lookup_edges_returns_edges_to_nodes_outside_the_bid_list() {
    use crate::beliefbase::BeliefSink;
    use crate::event::{BeliefEvent, EventOrigin};
    use crate::properties::{BeliefNode, Bid};

    let mk = |n: u128| Bid::from(uuid::Uuid::from_u128(n));
    let (subject, neighbour) = (mk(0xA), mk(0xB));
    let node = |bid| BeliefNode {
        bid,
        ..Default::default()
    };
    let mut ws = WeightSet::default();
    ws.set(WeightKind::Section, Weight::default());

    let mut base = BeliefBase::empty();
    base.apply_batch(&[
        BeliefEvent::NodeUpdate(
            vec![NodeKey::Bid { bid: subject }],
            node(subject),
            EventOrigin::Remote,
        ),
        BeliefEvent::NodeUpdate(
            vec![NodeKey::Bid { bid: neighbour }],
            node(neighbour),
            EventOrigin::Remote,
        ),
        BeliefEvent::RelationUpdate(subject, neighbour, ws, EventOrigin::Remote),
    ])
    .await
    .unwrap();

    // Ask for edges incident to `subject` ONLY. `neighbour` is deliberately
    // absent from the list -- that is the whole point of the test.
    let graph = crate::query::lookup_edges(&base, &[subject]).await.unwrap();

    assert_eq!(
        graph.relations.as_graph().edge_count(),
        1,
        "the edge subject -> neighbour is incident to `subject` and must be \
         returned even though `neighbour` was not passed in"
    );
}
