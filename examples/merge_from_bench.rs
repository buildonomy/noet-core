//! Ad-hoc benchmark: reproduce the exact `initialize_stack` const-namespace
//! load path — `session_bb.merge_from(&const_graph, &ns_seed)` — at realistic
//! scale (~84k asset nodes, star graph around the asset namespace), to
//! isolate whether this is the source of a production asset-heavy-corpus
//! regression (80+ seconds observed per call in a production log).
//!
//! Run: cargo run --release --example merge_from_bench --features service

use noet_core::beliefbase::{BeliefBase, BeliefGraph, BidGraph};
use noet_core::properties::{BeliefKind, BeliefKindSet, BeliefNode, Weight, WeightKind, WeightSet};
use noet_core::query::spec::{QueryPackage, QuerySpec, TapeFn};
use rustc_hash::FxHashMap;
use std::collections::BTreeSet;
use std::time::Instant;

fn make_asset_graph(n: usize) -> (BeliefGraph, noet_core::properties::Bid) {
    let mut states = FxHashMap::default();
    let ns_node = BeliefNode::asset_network();
    let ns_bid = ns_node.bid;
    states.insert(ns_bid, ns_node);

    let mut weight_set = WeightSet::default();
    weight_set
        .weights
        .insert(WeightKind::Section, Weight::default());

    let mut edges = Vec::with_capacity(n);
    for _ in 0..n {
        let node = BeliefNode {
            bid: noet_core::properties::Bid::new(ns_bid),
            kind: BeliefKindSet(BeliefKind::External | BeliefKind::Trace),
            ..Default::default()
        };
        let bid = node.bid;
        states.insert(bid, node);
        // Asset nodes are the SOURCE of their Section edge to the namespace
        // (namespace is the sink) — matches GraphBuilder::process_asset.
        edges.push((bid, ns_bid, weight_set.clone()));
    }

    let relations = BidGraph::from_edges(edges);
    (BeliefGraph { states, relations }, ns_bid)
}

fn main() {
    for &n in &[1_000usize, 10_000, 50_000, 84_000] {
        let (const_graph, ns_bid) = make_asset_graph(n);
        let ns_seed: BTreeSet<noet_core::properties::Bid> = BTreeSet::from([ns_bid]);

        // Step 1: evaluate_query (balanced) in isolation — mirrors
        // to_event_stream's scoped_graph computation.
        let bb = BeliefBase::from(const_graph.clone());
        let spec = QuerySpec::seed(TapeFn::Bids(vec![ns_bid]));
        let mut package = QueryPackage::balanced(spec);
        let start_eval = Instant::now();
        bb.evaluate_query(&mut package).unwrap();
        let eval_elapsed = start_eval.elapsed();
        let scoped_graph = package.into_graph();
        println!(
            "n={n:>7}  evaluate_query(balanced) = {eval_elapsed:>10.2?}  scoped_graph.states.len()={}",
            scoped_graph.states.len()
        );

        // Step 2: full merge_from (includes the evaluate_query above internally).
        let mut session_bb = BeliefBase::empty().with_label("session_bb");
        let start = Instant::now();
        session_bb.merge_from(&const_graph, &ns_seed);
        let elapsed = start.elapsed();

        println!(
            "n={n:>7}  merge_from = {elapsed:>10.2?}  (session_bb now has {} states)",
            session_bb.states().len()
        );
    }
}
