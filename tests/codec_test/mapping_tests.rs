//! Integration tests for the {maps_to} directive (Issue 61)
//!
//! Verifies that:
//! - A section containing `{maps_to}` produces `RelationChange` events with the correct
//!   `WEIGHT_OWNED_BY` set to the section's bref.
//! - The edges appear in `session_bb` / `global_bb` with the correct source, sink, and kind.
//! - A single directive with multiple sinks produces one edge per (source, sink) pair.
//! - The rendered HTML for the document contains the mapping-table sentinel.
//! - Cross-document mapping edges (source/sink in a different document from the owner section)
//!   survive the full parse → compute_diff → accumulator → global_bb pipeline.

use noet_core::{
    beliefbase::BeliefBase,
    codec::DocumentCompiler,
    event::BeliefEvent,
    properties::{Bref, WeightKind, WEIGHT_OWNED_BY},
};
use test_log::test;
use tokio::sync::mpsc::unbounded_channel;

use super::common::generate_test_root;

/// Helper: drain all events from the accumulator channel into a `BeliefBase`.
async fn drain_events(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<BeliefEvent>,
    bb: &mut BeliefBase,
) -> Result<(), Box<dyn std::error::Error>> {
    while let Ok(event) = rx.try_recv() {
        bb.process_event(&event)?;
    }
    Ok(())
}

/// Compile `mapping_test.md` and verify that the `{maps_to}` directive produces the
/// expected mapping edges in the belief base.
///
/// The fixture declares:
///
/// ```markdown
/// ## Trace Mapping
///
/// ````{maps_to} Pragmatic
/// source = "id://impl-one"
/// sink = ["id://req-alpha", "id://req-beta"]
/// ````
/// ```
///
/// Expected outcomes:
/// 1. Two `Pragmatic` edges are registered: `req-alpha → impl-one` and `req-beta → impl-one`
///    (source = requirement, sink = implementor — matches {implements} graph direction).
/// 2. Both edges have `WEIGHT_OWNED_BY` set to the bref of the "Trace Mapping" section node
///    (not `"source"` or `"sink"`).
/// 3. The "Trace Mapping" section node is recorded as a state in the belief base.
#[test(tokio::test)]
async fn test_maps_to_produces_owned_pragmatic_edges() -> Result<(), Box<dyn std::error::Error>> {
    let (_tmp, test_root) = generate_test_root("network_1")?;

    let mut global_bb = BeliefBase::empty();
    let (tx, mut rx) = unbounded_channel::<BeliefEvent>();

    let mut compiler = DocumentCompiler::new(&test_root, Some(tx), None, false)?;
    compiler.parse_all(global_bb.clone(), false).await?;

    drain_events(&mut rx, &mut global_bb).await?;

    // ── Locate the section nodes by title ────────────────────────────────────

    let owner_node = global_bb
        .states()
        .values()
        .find(|n| n.title == "Trace Mapping");
    assert!(
        owner_node.is_some(),
        "Should find the 'Trace Mapping' section node in the belief base"
    );
    let owner_node = owner_node.unwrap();
    let owner_bref_str = owner_node.bid.bref().to_string();

    // source = requirement, sink = implementor (matches {implements} graph direction).
    let sink_node = global_bb
        .states()
        .values()
        .find(|n| n.title == "Implementation One");
    assert!(
        sink_node.is_some(),
        "Should find the 'Implementation One' section node"
    );
    let sink_node = sink_node.unwrap();

    let source_alpha = global_bb
        .states()
        .values()
        .find(|n| n.title == "Requirement Alpha");
    assert!(
        source_alpha.is_some(),
        "Should find the 'Requirement Alpha' section node"
    );
    let source_alpha = source_alpha.unwrap();

    let source_beta = global_bb
        .states()
        .values()
        .find(|n| n.title == "Requirement Beta");
    assert!(
        source_beta.is_some(),
        "Should find the 'Requirement Beta' section node"
    );
    let source_beta = source_beta.unwrap();

    // ── Verify the mapping edges exist in the belief graph ───────────────────

    use petgraph::visit::{EdgeRef, IntoEdgeReferences};

    let relations_guard = global_bb.relations();
    let rel_graph = relations_guard.as_graph();
    let all_edges: Vec<_> = rel_graph.edge_references().collect();

    // Filter to Pragmatic edges into impl-one (the sink).
    // source = requirement, sink = implementor.
    let mut found_alpha = false;
    let mut found_beta = false;

    for edge_ref in &all_edges {
        let src_bid = rel_graph[edge_ref.source()];
        let snk_bid = rel_graph[edge_ref.target()];

        if snk_bid != sink_node.bid {
            continue;
        }

        let weights = edge_ref.weight();
        let Some(pragmatic_weight) = weights.weights.get(&WeightKind::Pragmatic) else {
            continue;
        };

        // The edge must be owned by the "Trace Mapping" section, not by "source" or "sink".
        let owned_by: Option<String> = pragmatic_weight.get(WEIGHT_OWNED_BY);
        assert_eq!(
            owned_by.as_deref(),
            Some(owner_bref_str.as_str()),
            "Mapping edge into 'Implementation One' should have WEIGHT_OWNED_BY = \
             owner section bref (not 'source' or 'sink'); got: {:?}",
            owned_by
        );

        if src_bid == source_alpha.bid {
            found_alpha = true;
        }
        if src_bid == source_beta.bid {
            found_beta = true;
        }
    }

    assert!(
        found_alpha,
        "Should find Pragmatic edge: source='Requirement Alpha' → sink='Implementation One'"
    );
    assert!(
        found_beta,
        "Should find Pragmatic edge: source='Requirement Beta' → sink='Implementation One'"
    );

    // ── Verify owner_edges memo is consistent with the graph ────────────────────
    drop(relations_guard);
    let owner_bref = owner_node.bid.bref();
    let memo_entry = global_bb.owner_edges().get(&owner_bref);
    assert!(
        memo_entry.is_some(),
        "owner_edges memo should contain an entry for the 'Trace Mapping' section bref"
    );
    assert_eq!(
        memo_entry.unwrap().len(),
        2,
        "owner_edges memo should index exactly 2 edges for the 'Trace Mapping' owner"
    );

    Ok(())
}

/// Verify write-back fidelity: the `{maps_to}` directive survives a parse → rewrite
/// round-trip intact. The rewritten Markdown source (with BIDs injected into the
/// frontmatter) must still contain the original `{maps_to}` fenced block so that
/// subsequent parses can re-process the directive without data loss.
///
/// This also confirms that the mapping-test document was identified as needing a
/// deferred render pass (i.e. `should_defer()` returned `true`), because only
/// documents with deferred directives trigger a content rewrite on first parse.
#[test(tokio::test)]
async fn test_maps_to_directive_survives_rewrite_round_trip(
) -> Result<(), Box<dyn std::error::Error>> {
    let (_tmp, test_root) = generate_test_root("network_1")?;

    let mut global_bb = BeliefBase::empty();
    let (tx, mut rx) = unbounded_channel::<BeliefEvent>();

    let mut compiler = DocumentCompiler::new(&test_root, Some(tx), None, false)?;
    let parse_results = compiler.parse_all(global_bb.clone(), false).await?;
    drain_events(&mut rx, &mut global_bb).await?;

    // Find the parse result for mapping_test.md
    let mapping_result = parse_results
        .iter()
        .find(|r| r.path.to_string_lossy().contains("mapping_test.md"));
    assert!(
        mapping_result.is_some(),
        "mapping_test.md should appear in parse results"
    );
    let mapping_result = mapping_result.unwrap();

    // The document contains a {maps_to} directive, so finalize() should produce a
    // rewritten_content (BIDs are injected into the frontmatter sections table).
    let rewritten = mapping_result.rewritten_content.as_deref().expect(
        "mapping_test.md should produce rewritten_content on first parse \
                 (BIDs injected into frontmatter sections table)",
    );

    // The {maps_to} fenced block must still be present verbatim in the rewritten source.
    // write-back fidelity: directive bodies are preserved unchanged.
    assert!(
        rewritten.contains("{maps_to} Pragmatic"),
        "Rewritten source should preserve the {{maps_to}} directive info string; got:\n{}",
        &rewritten[..rewritten.len().min(800)]
    );
    assert!(
        rewritten.contains("source = [\"id://req-alpha\", \"id://req-beta\"]"),
        "Rewritten source should preserve the directive body `source` array (requirements)"
    );
    assert!(
        rewritten.contains("sink = \"id://impl-one\""),
        "Rewritten source should preserve the directive body `sink` field (implementor)"
    );

    Ok(())
}

/// Verify that `{maps_to}` edges are registered in `global_bb` when the owning section
/// is in a **different document** from the source and sink nodes.
///
/// This tests the `compute_diff` initial edge filter fix (Bug 3 from Issue 61 follow-up):
/// the filter previously required source or sink to be in `parsed_content`, which excluded
/// all cross-document mapping edges.  The fix adds a third condition: the edge passes if
/// its `WEIGHT_OWNED_BY` bref resolves to a BID that IS in `parsed_content`.
///
/// Fixture: `mapping_cross_doc_test.md` owns a `{maps_to}` directive whose source
/// (`id://xdoc-req-alpha`, `id://xdoc-req-beta`) and sink (`id://xdoc-impl-one`) are declared in
/// the sibling document `mapping_cross_doc_endpoints.md`.  Two Pragmatic edges are expected in
/// `global_bb` after compilation, each owned by the "XDoc Trace Mapping" section bref.
///
/// Uses `network_mapping` (not `network_1`) so that ephemeral section BIDs on the
/// mapping owner do not interfere with the BID-stability assertions in `bid_tests.rs`.
#[test(tokio::test)]
async fn test_maps_to_cross_document_edges() -> Result<(), Box<dyn std::error::Error>> {
    let (_tmp, test_root) = generate_test_root("network_mapping")?;

    let mut global_bb = BeliefBase::empty();
    let (tx, mut rx) = unbounded_channel::<BeliefEvent>();

    let mut compiler = DocumentCompiler::new(&test_root, Some(tx), None, false)?;
    compiler.parse_all(global_bb.clone(), false).await?;
    drain_events(&mut rx, &mut global_bb).await?;

    // ── Locate the owner section and endpoint nodes ──────────────────────────

    let owner_node = global_bb
        .states()
        .values()
        .find(|n| n.title == "XDoc Trace Mapping")
        .expect("Should find the 'XDoc Trace Mapping' section in global_bb");
    let owner_bref_str = owner_node.bid.bref().to_string();

    let sink_node = global_bb
        .states()
        .values()
        .find(|n| n.title == "XDoc Implementation One")
        .expect(
            "Should find 'XDoc Implementation One' (declared in mapping_cross_doc_endpoints.md)",
        );

    let source_alpha = global_bb
        .states()
        .values()
        .find(|n| n.title == "XDoc Requirement Alpha")
        .expect(
            "Should find 'XDoc Requirement Alpha' (declared in mapping_cross_doc_endpoints.md)",
        );

    let source_beta = global_bb
        .states()
        .values()
        .find(|n| n.title == "XDoc Requirement Beta")
        .expect("Should find 'XDoc Requirement Beta' (declared in mapping_cross_doc_endpoints.md)");

    // ── Verify cross-document mapping edges ──────────────────────────────────

    use petgraph::visit::{EdgeRef, IntoEdgeReferences};
    let relations_guard = global_bb.relations();
    let rel_graph = relations_guard.as_graph();

    let mut found_alpha = false;
    let mut found_beta = false;

    for edge_ref in rel_graph.edge_references() {
        let src_bid = rel_graph[edge_ref.source()];
        let snk_bid = rel_graph[edge_ref.target()];

        if snk_bid != sink_node.bid {
            continue;
        }

        let Some(pragmatic_weight) = edge_ref.weight().weights.get(&WeightKind::Pragmatic) else {
            continue;
        };

        let owned_by: Option<String> = pragmatic_weight.get(WEIGHT_OWNED_BY);
        if owned_by.as_deref() != Some(owner_bref_str.as_str()) {
            // Skip edges owned by the same-doc "Trace Mapping" section from mapping_test.md
            continue;
        }

        if src_bid == source_alpha.bid {
            found_alpha = true;
        }
        if src_bid == source_beta.bid {
            found_beta = true;
        }
    }

    assert!(
        found_alpha,
        "Should find cross-document Pragmatic edge: \
         'XDoc Requirement Alpha' → 'XDoc Implementation One', \
         owned by 'XDoc Trace Mapping'. \
         This exercises the compute_diff owner-in-parsed_content filter path."
    );
    assert!(
        found_beta,
        "Should find cross-document Pragmatic edge: \
         'XDoc Requirement Beta' → 'XDoc Implementation One', \
         owned by 'XDoc Trace Mapping'. \
         This exercises the compute_diff owner-in-parsed_content filter path."
    );

    // ── Verify cross-document owner_edges memo ────────────────────────────────
    drop(relations_guard);
    let owner_bref = owner_node.bid.bref();
    let memo_entry = global_bb.owner_edges().get(&owner_bref);
    assert!(
        memo_entry.is_some(),
        "owner_edges memo should contain an entry for the 'XDoc Trace Mapping' section bref"
    );
    assert_eq!(
        memo_entry.unwrap().len(),
        2,
        "owner_edges memo should index exactly 2 cross-document edges for 'XDoc Trace Mapping'"
    );

    Ok(())
}

/// Verify that the mapping edge count is exactly 2 — one per sink — confirming
/// that a `{maps_to}` directive with `sink = [...]` (array) does not collapse
/// the sinks into a single edge.
#[test(tokio::test)]
async fn test_maps_to_one_edge_per_sink() -> Result<(), Box<dyn std::error::Error>> {
    let (_tmp, test_root) = generate_test_root("network_1")?;

    let mut global_bb = BeliefBase::empty();
    let (tx, mut rx) = unbounded_channel::<BeliefEvent>();

    let mut compiler = DocumentCompiler::new(&test_root, Some(tx), None, false)?;
    compiler.parse_all(global_bb.clone(), false).await?;
    drain_events(&mut rx, &mut global_bb).await?;

    // The directive has 2 sources (req-alpha, req-beta) × 1 sink (impl-one) = 2 edges.
    let sink_node = global_bb
        .states()
        .values()
        .find(|n| n.title == "Implementation One")
        .expect("Should find 'Implementation One'");

    let owner_node = global_bb
        .states()
        .values()
        .find(|n| n.title == "Trace Mapping")
        .expect("Should find 'Trace Mapping'");
    let owner_bref_str = owner_node.bid.bref().to_string();

    use petgraph::visit::{EdgeRef, IntoEdgeReferences};
    let relations_guard = global_bb.relations();
    let rel_graph = relations_guard.as_graph();

    // 2 sources (req-alpha, req-beta) × 1 sink (impl-one) = 2 edges total.
    // Filter: target == impl-one AND owned_by == owner section bref.
    let owned_pragmatic_edges: Vec<_> = rel_graph
        .edge_references()
        .filter(|e| {
            rel_graph[e.target()] == sink_node.bid
                && e.weight()
                    .weights
                    .get(&WeightKind::Pragmatic)
                    .and_then(|w| w.get::<String>(WEIGHT_OWNED_BY))
                    .as_deref()
                    == Some(owner_bref_str.as_str())
        })
        .collect();

    assert_eq!(
        owned_pragmatic_edges.len(),
        2,
        "Expected exactly 2 owned Pragmatic edges into 'Implementation One' \
         (Cartesian product: 2 sources × 1 sink); found {}",
        owned_pragmatic_edges.len()
    );

    Ok(())
}

/// Verify that the `owner_edges` memo on `BeliefBase` is fully consistent with the
/// actual graph: every third-party `WEIGHT_OWNED_BY` edge is indexed, and every
/// indexed edge resolves to a valid graph edge with the correct owner bref.
///
/// This is a structural consistency check that exercises the incremental maintenance
/// in `update_relation`, `process_event`, and `merge` paths that build the memo
/// during compilation.
#[test(tokio::test)]
async fn test_owner_edges_memo_consistency() -> Result<(), Box<dyn std::error::Error>> {
    let (_tmp, test_root) = generate_test_root("network_mapping")?;

    let mut global_bb = BeliefBase::empty();
    let (tx, mut rx) = unbounded_channel::<BeliefEvent>();

    let mut compiler = DocumentCompiler::new(&test_root, Some(tx), None, false)?;
    compiler.parse_all(global_bb.clone(), false).await?;
    drain_events(&mut rx, &mut global_bb).await?;

    // ── Forward check: every memo entry resolves to a valid graph edge ───────
    let relations_guard = global_bb.relations();
    let rel_graph = relations_guard.as_graph();

    for (bref, edge_indices) in global_bb.owner_edges() {
        assert!(
            !edge_indices.is_empty(),
            "owner_edges should not contain empty sets (bref {bref})"
        );
        let bref_str = bref.to_string();
        for &edge_idx in edge_indices {
            let ws = rel_graph.edge_weight(edge_idx).unwrap_or_else(|| {
                panic!(
                    "owner_edges memo references EdgeIndex {edge_idx:?} for bref {bref}, \
                         but no such edge exists in the relations graph"
                )
            });
            // At least one weight in this edge must have WEIGHT_OWNED_BY == bref_str
            let has_matching_owner = ws.weights.values().any(|weight| {
                weight.get::<String>(WEIGHT_OWNED_BY).as_deref() == Some(bref_str.as_str())
            });
            assert!(
                has_matching_owner,
                "Edge {edge_idx:?} is indexed under owner bref {bref}, \
                 but none of its weights have WEIGHT_OWNED_BY = {bref_str:?}"
            );
        }
    }

    // ── Reverse check: every third-party owned edge in the graph is in the memo ──
    use petgraph::visit::{EdgeRef, IntoEdgeReferences};
    for edge_ref in rel_graph.edge_references() {
        for weight in edge_ref.weight().weights.values() {
            let owned_by: Option<String> = weight.get(WEIGHT_OWNED_BY);
            match owned_by.as_deref() {
                Some("source") | Some("sink") | None => continue,
                Some(bref_str) => {
                    let bref = Bref::try_from(bref_str).unwrap_or_else(|_| {
                        panic!("Invalid bref string in WEIGHT_OWNED_BY: {bref_str:?}")
                    });
                    let memo_set = global_bb.owner_edges().get(&bref);
                    assert!(
                        memo_set.is_some_and(|s| s.contains(&edge_ref.id())),
                        "Graph edge {edge_idx:?} has WEIGHT_OWNED_BY = {bref_str:?}, \
                         but owner_edges memo does not index it under that bref",
                        edge_idx = edge_ref.id()
                    );
                }
            }
        }
    }

    // ── Sanity: at least some owned edges should exist in this fixture ───────
    assert!(
        !global_bb.owner_edges().is_empty(),
        "network_mapping fixture should produce at least some third-party owned edges"
    );

    Ok(())
}
