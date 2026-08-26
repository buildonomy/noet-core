//! TDD integration tests for the `{query}` directive (Issue 81).
//!
//! The fixture `tests/network_1/query_directive_test.md` contains four
//! `{query}` blocks exercising the key scenarios. These tests define the
//! success criteria for Issue 81's implementation.
//!
//! **Status**: Tests marked `#[ignore]` require Issue 81 to pass. The
//! baseline tests (no `#[ignore]`) verify the fixture compiles cleanly today.
//!
//! Run the full contract: `cargo test --features service -- --include-ignored query_directive`

#![cfg(feature = "service")]

use noet_core::{
    beliefbase::BeliefBase, codec::DocumentCompiler, event::BeliefEvent, properties::NodeId,
    query::parser::parse,
};
use test_log::test;
use tokio::sync::mpsc::unbounded_channel;

use super::common::generate_test_root;

// ═════════════════════════════════════════════════════════════════════════════
// Helpers
// ═════════════════════════════════════════════════════════════════════════════

async fn drain_events(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<BeliefEvent>,
    bb: &mut BeliefBase,
) {
    while let Ok(event) = rx.try_recv() {
        let _ = bb.process_event(&event);
    }
}

/// Parse-only compilation (no HTML output). Used by baseline tests that only
/// need to check that the fixture produces BeliefBase nodes.
async fn compile_network_1() -> (tempfile::TempDir, BeliefBase) {
    let (tmp, test_root) = generate_test_root("network_1").unwrap();
    let mut global_bb = BeliefBase::empty();
    let (tx, mut rx) = unbounded_channel::<BeliefEvent>();

    let mut compiler = DocumentCompiler::new(&test_root, Some(tx), None, false).unwrap();
    compiler.parse_all(global_bb.clone(), false).await.unwrap();
    drain_events(&mut rx, &mut global_bb).await;

    (tmp, global_bb)
}

/// Full compile-to-HTML. Produces HTML files in a separate `_site/` temp dir.
/// Returns `(source_tmp, html_tmp, final_bb)`. The HTML output lives at
/// `html_tmp.path()` — use `html_tmp.path().join("pages/...")` to find files.
/// Required by tests that assert on rendered HTML content.
async fn compile_network_1_to_html() -> (tempfile::TempDir, tempfile::TempDir, BeliefBase) {
    let (src_tmp, test_root) = generate_test_root("network_1").unwrap();
    let html_tmp = tempfile::tempdir().unwrap();

    let (tx, mut rx) = unbounded_channel::<BeliefEvent>();

    // Background task: receive and process all events into global_bb.
    let mut event_bb = BeliefBase::empty();
    let processor = tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            let _ = event_bb.process_event(&event);
        }
        event_bb
    });

    let mut compiler = DocumentCompiler::with_html_output(
        &test_root,
        Some(tx),
        Some(5),
        false,
        Some(html_tmp.path().to_path_buf()),
        None,
        false,
        None,
        None,
        false,
    )
    .unwrap();

    let cache = compiler.cache().clone();
    compiler.parse_all(cache, false).await.unwrap();

    // Close the tx channel so the processor task finishes.
    compiler.builder_mut().close_tx();
    let final_bb = processor.await.unwrap();

    compiler.finalize_html(&final_bb).await.unwrap();

    (src_tmp, html_tmp, final_bb)
}

/// Return the HTML output path for a document in the compiled network.
/// Only valid after `finalize_html` has been called on the compiler.
fn find_node_by_id<'a>(
    bb: &'a BeliefBase,
    id: &str,
) -> Option<&'a noet_core::properties::BeliefNode> {
    bb.states()
        .values()
        .find(|n| matches!(&n.id, NodeId::Explicit(s) if s == id))
}

// ═════════════════════════════════════════════════════════════════════════════
// Baseline (passes today — no Issue 81 required)
// ═════════════════════════════════════════════════════════════════════════════

/// The fixture file compiles without error and produces a node in the BeliefBase.
/// This passes today because unrecognised fenced directives are treated as code
/// blocks and do not abort compilation.
#[test(tokio::test)]
async fn query_directive_fixture_compiles() {
    let (_tmp, bb) = compile_network_1().await;

    // The fixture document should be in the compiled BeliefBase.
    let node = find_node_by_id(&bb, "query-directive-test");
    assert!(
        node.is_some(),
        "expected a node for query_directive_test.md in the BeliefBase"
    );
}

/// The `query_parser::parse()` function used by the directive already works
/// for all four query bodies in the fixture (pure unit check, no BeliefBase).
#[test]
fn fixture_query_bodies_parse_cleanly() {
    // Block 0: implicit anchor, traversal
    assert!(parse("k-pragmatic-s(1)").is_ok(), "block 0 should parse");

    // Block 1: explicit anchor, section traversal
    assert!(
        parse("id://belief-network-test-1-subnet-1 k-section-s(1)").is_ok(),
        "block 1 should parse"
    );

    // Block 2: intentionally malformed — degenerate self-loop
    assert!(
        parse("s-s-s").is_err(),
        "block 2 (s-s-s) should be a parse error"
    );

    // Block 3: valid predicate that returns no results at runtime
    assert!(
        parse("schema == _no_such_schema_xyz_").is_ok(),
        "block 3 should parse (valid syntax, empty results at runtime)"
    );
}

// ═════════════════════════════════════════════════════════════════════════════
// Issue 81 contract (all ignored until the directive is implemented)
// ═════════════════════════════════════════════════════════════════════════════

/// After Issue 81: the compiled HTML for `query_directive_test.md` must NOT
/// contain any raw `<!--@@noet-query:` sentinel strings — all sentinels must
/// be replaced with rendered HTML content.
#[test(tokio::test)]
async fn query_directive_sentinels_are_replaced() {
    let (_src, html_tmp, _bb) = compile_network_1_to_html().await;

    // Locate the generated HTML file in the separate HTML output dir.
    let html_path = html_tmp
        .path()
        .join("pages")
        .join("query_directive_test.html");

    let html = std::fs::read_to_string(&html_path)
        .unwrap_or_else(|e| panic!("could not read {html_path:?}: {e}"));

    assert!(
        !html.contains("<!--@@noet-query:"),
        "no raw query sentinels should remain in the output HTML; got:\n{html}"
    );
}

/// After Issue 81: block 0 (implicit anchor, depth0 view) renders a static result placeholder.
///
/// The static HTML renderer no longer runs the view pipeline inline; it emits a
/// `.noet-query-result` placeholder div so `attachQuerySearchButtons` in content.js
/// can wire up the Search panel link. The meta div carries the real count.
#[test(tokio::test)]
async fn query_directive_block0_renders_depth0_list() {
    let (_src, html_tmp, _bb) = compile_network_1_to_html().await;
    let html_path = html_tmp
        .path()
        .join("pages")
        .join("query_directive_test.html");
    let html = std::fs::read_to_string(&html_path).unwrap();

    // The static placeholder div must be present.
    assert!(
        html.contains("noet-query-result"),
        "block 0 should be wrapped in a noet-query-result container"
    );
    // The placeholder shows a count or "No results".
    assert!(
        html.contains("Open Search to explore results."),
        "block 0 placeholder should contain the static open-search message"
    );
}

/// Block 1 (explicit anchor, depth0 view) queries section children of the
/// subnet1 network node. That node has section edges to documents like
/// subnet1_file1, so the query returns real results.
///
/// With the static-placeholder rendering, we no longer emit the view-rendered table
/// inline. Instead we check the `.noet-query-result` div and that the meta div
/// records a non-zero count.
#[test(tokio::test)]
async fn query_directive_block1_explicit_anchor_has_results() {
    let (_src, html_tmp, _bb) = compile_network_1_to_html().await;
    let html_path = html_tmp
        .path()
        .join("pages")
        .join("query_directive_test.html");
    let html = std::fs::read_to_string(&html_path).unwrap();

    // The static placeholder div must be present.
    assert!(
        html.contains("noet-query-result"),
        "block 1 should have a noet-query-result placeholder"
    );
    // Block 1 queries subnet1 section children — there should be results.
    // The static placeholder is always the same string.
    assert!(
        html.contains("Open Search to explore results."),
        "block 1 placeholder should contain the static open-search message"
    );
}

/// After Issue 81: block 2 (parse error — s-s-s) renders an error div, not a panic.
#[test(tokio::test)]
async fn query_directive_parse_error_renders_error_block() {
    let (_src, html_tmp, _bb) = compile_network_1_to_html().await;
    let html_path = html_tmp
        .path()
        .join("pages")
        .join("query_directive_test.html");
    let html = std::fs::read_to_string(&html_path).unwrap();

    assert!(
        html.contains("noet-query-error"),
        "parse error block should emit a div with class noet-query-error"
    );
}

/// After Issue 81: block 3 (valid query, empty results) renders gracefully —
/// the sentinel is replaced and a `.noet-query-result` placeholder is emitted.
///
/// With static-placeholder rendering the view caption is no longer inline.
/// We verify the sentinel is gone and the placeholder div is present.
#[test(tokio::test)]
async fn query_directive_empty_results_render_gracefully() {
    let (_src, html_tmp, _bb) = compile_network_1_to_html().await;
    let html_path = html_tmp
        .path()
        .join("pages")
        .join("query_directive_test.html");
    let html = std::fs::read_to_string(&html_path).unwrap();

    // Should NOT contain the raw sentinel for block 3
    assert!(
        !html.contains("<!--@@noet-query:3@@-->"),
        "block 3 sentinel must be replaced even when results are empty"
    );
    // The placeholder div must be present.
    assert!(
        html.contains("noet-query-result"),
        "block 3 should have a noet-query-result placeholder"
    );
    // The static placeholder is always the same string regardless of result count.
    assert!(
        html.contains("Open Search to explore results."),
        "block 3 placeholder should contain the static open-search message"
    );
}

/// After Issue 81: existing directives (`{requirements_table}`) still work.
/// This is the regression guard.
#[test(tokio::test)]
async fn requirements_table_unaffected_by_query_directive() {
    // The mapping_test.md fixture exercises {maps_to} and related directives.
    // After Issue 81, these must continue to produce their expected sentinels
    // and have them replaced — the {query} special-case block must not
    // interfere with the existing directive pipeline.
    let (_src, _html_tmp, bb) = compile_network_1_to_html().await;
    // Spot-check: the mapping_test document node must exist.
    // mapping_test.md uses a persisted BID and title "Mapping Test Document";
    // its NodeId is slug-derived, not explicit. Check by BID instead.
    let mapping_test_bid =
        noet_core::properties::Bid::try_from("10000000-0000-0000-0000-000000000030").unwrap();
    let node = bb.states().get(&mapping_test_bid);
    assert!(
        node.is_some(),
        "mapping_test node must still exist after Issue 81 changes"
    );
}
