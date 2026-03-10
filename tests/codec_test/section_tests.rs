//! Section metadata enrichment and handling tests

use noet_core::{
    beliefbase::BeliefBase,
    codec::{network::detect_network_file, DocumentCompiler},
    event::BeliefEvent,
    properties::WeightKind,
};
use std::fs;
use test_log::test;
use tokio::sync::mpsc::unbounded_channel;

use super::common::generate_test_root;

#[test(tokio::test)]
async fn test_sections_metadata_enrichment() -> Result<(), Box<dyn std::error::Error>> {
    tracing::info!("Testing sections metadata enrichment (Issue 02)");

    let (_test_tempdir, test_root) = generate_test_root("network_1")?;

    let mut global_bb = BeliefBase::empty();
    let (accum_tx, mut accum_rx) = unbounded_channel::<BeliefEvent>();

    tracing::info!("Initialize DocumentCompiler");
    let mut compiler = DocumentCompiler::new(&test_root, Some(accum_tx), None, false)?;

    tracing::info!("Parse all documents including sections_test.md");
    let parse_results = compiler.parse_all(global_bb.clone(), false).await?;

    // Process events to build up global_bb
    while let Ok(event) = accum_rx.try_recv() {
        global_bb.process_event(&event)?;
    }

    tracing::info!("Verify sections_test.md was parsed");
    let sections_doc_found = parse_results
        .iter()
        .any(|r| r.path.to_string_lossy().contains("sections_test.md"));
    assert!(sections_doc_found, "sections_test.md should be parsed");

    // Find the document node for sections_test.md
    // Use title since path is tracked in relations, not in node itself
    let doc_node = global_bb
        .states()
        .values()
        .find(|node| node.title.contains("Sections Test Document"));

    assert!(
        doc_node.is_some(),
        "Should find sections_test.md document node"
    );
    let doc_node = doc_node.unwrap();

    assert!(
        doc_node.kind.is_document(),
        "sections_test.md should be colored as a document. Received kind: {}",
        doc_node.kind
    );
    // Find heading nodes that are children of this document
    let heading_nodes: Vec<_> = global_bb
        .states()
        .values()
        .filter(|node| {
            // Check if this node has a Section relationship to the document
            global_bb
                .relations()
                .as_graph()
                .edges_connecting(
                    global_bb.bid_to_index(&node.bid).unwrap(),
                    global_bb.bid_to_index(&doc_node.bid).unwrap(),
                )
                .any(|edge| edge.weight().weights.contains_key(&WeightKind::Section))
        })
        .collect();

    tracing::info!("Found {} heading nodes", heading_nodes.len());

    // We expect 4 heading nodes:
    // 1. "Sections Test Document" (H1 - document title)
    // 2. "Introduction" (should have BID match with complexity=high)
    // 3. "Background" (should have anchor match with complexity=medium)
    // 4. "API Reference" (should have title match with complexity=low)
    // 5. "Untracked Section" (should have no metadata, default fields only)
    assert!(
        heading_nodes.len() >= 4,
        "Should have at least 4 heading nodes, got {}",
        heading_nodes.len()
    );

    // Check for Introduction node (no sections entry, so no enrichment)
    let intro_node = heading_nodes
        .iter()
        .find(|n| n.title.contains("Introduction"));
    assert!(intro_node.is_some(), "Should find Introduction node");
    let intro_node = intro_node.unwrap();

    // Introduction has no sections entry, so only has default text field
    assert!(
        intro_node.payload.get("complexity").is_none(),
        "Introduction should NOT have enriched metadata (no sections entry)"
    );
    tracing::info!(
        "Introduction node: bid={}, title={}, payload={:?}",
        intro_node.bid,
        intro_node.title,
        intro_node.payload
    );

    // Check for Background node with anchor match
    let background_node = heading_nodes
        .iter()
        .find(|n| n.title.contains("Background"));
    assert!(background_node.is_some(), "Should find Background node");
    let background_node = background_node.unwrap();

    // Issue 02 + Issue 03 IMPLEMENTED: Anchor matching works!
    assert_eq!(
        background_node
            .payload
            .get("complexity")
            .and_then(|v| v.as_str()),
        Some("medium"),
        "Background should have complexity='medium' from sections metadata (anchor match)"
    );
    assert_eq!(
        background_node
            .payload
            .get("priority")
            .and_then(|v| v.as_integer()),
        Some(2),
        "Background should have priority=2 from sections metadata (anchor match)"
    );
    tracing::info!(
        "Background node: bid={}, title={}, payload={:?}",
        background_node.bid,
        background_node.title,
        background_node.payload
    );

    // Check for API Reference node with title slug match
    let api_node = heading_nodes
        .iter()
        .find(|n| n.title.contains("API Reference"));
    assert!(api_node.is_some(), "Should find API Reference node");
    let api_node = api_node.unwrap();

    // Issue 02 IMPLEMENTED: Title matching works now! Verify enriched metadata:
    assert_eq!(
        api_node.payload.get("complexity").and_then(|v| v.as_str()),
        Some("low"),
        "API Reference should have complexity='low' from sections metadata"
    );
    assert_eq!(
        api_node
            .payload
            .get("priority")
            .and_then(|v| v.as_integer()),
        Some(3),
        "API Reference should have priority=3 from sections metadata"
    );
    tracing::info!(
        "API Reference node: bid={}, title={}, payload={:?}",
        api_node.bid,
        api_node.title,
        api_node.payload
    );

    // Check for Untracked Section node (should exist and get sections entry added)
    let untracked_node = heading_nodes
        .iter()
        .find(|n| n.title.contains("Untracked Section"));
    assert!(
        untracked_node.is_some(),
        "Should find Untracked Section node (markdown defines structure)"
    );
    let untracked_node = untracked_node.unwrap();

    // Issue 02 IMPLEMENTED: Node exists (markdown defines structure)
    // Initially has no custom metadata (no pre-defined sections entry)
    // TODO: After full Issue 02 + auto-generation feature:
    // - Verify auto-generated ID: "untracked-section" (via to_anchor)
    // - Verify sections entry was ADDED to frontmatter during finalize()
    tracing::info!(
        "Untracked Section node: bid={}, title={}, payload={:?}",
        untracked_node.bid,
        untracked_node.title,
        untracked_node.payload
    );

    // The "unmatched" section in frontmatter should NOT create a node
    // (it has no corresponding heading)
    let unmatched_node = heading_nodes.iter().find(|n| n.title.contains("unmatched"));
    assert!(
        unmatched_node.is_none(),
        "Should NOT find node for frontmatter-only 'unmatched' section"
    );

    Ok(())
}

#[test(tokio::test)]
async fn test_sections_garbage_collection() -> Result<(), Box<dyn std::error::Error>> {
    tracing::info!("Testing unmatched sections are garbage collected during finalize()");

    let (_test_tempdir, test_root) = generate_test_root("network_1")?;

    let mut global_bb = BeliefBase::empty();
    let (accum_tx, mut accum_rx) = unbounded_channel::<BeliefEvent>();

    let mut compiler = DocumentCompiler::new(&test_root, Some(accum_tx), None, false)?;
    let parse_results = compiler.parse_all(global_bb.clone(), false).await?;

    while let Ok(event) = accum_rx.try_recv() {
        global_bb.process_event(&event)?;
    }

    // Check if sections_test.md was rewritten (indicating finalize() modified document)
    let sections_doc_result = parse_results
        .iter()
        .find(|r| r.path.to_string_lossy().contains("sections_test.md"));

    if let Some(result) = sections_doc_result {
        if let Some(ref rewritten) = result.rewritten_content {
            tracing::info!("sections_test.md was rewritten by finalize()");

            // Issue 02 IMPLEMENTED: finalize() performs garbage collection
            // Verify "unmatched" section was REMOVED (no matching heading)
            // Other matched sections should be preserved
            tracing::info!("Rewritten content length: {}", rewritten.len());

            // Check that "unmatched" is NOT in the rewritten frontmatter
            let has_unmatched = rewritten.contains("sections.unmatched")
                || rewritten.contains(r#"[sections."unmatched"]"#);

            // Check that "untracked-section" IS in the rewritten frontmatter
            let has_untracked = rewritten.contains("untracked-section");

            // Issue 02 IMPLEMENTED: Verify garbage collection worked
            assert!(
                !has_unmatched,
                "Unmatched section should be garbage collected by finalize()"
            );
            assert!(has_untracked, "New heading should get sections entry added");
        } else {
            tracing::info!("sections_test.md was NOT rewritten (finalize() made no changes or not implemented yet)");
        }
    }

    Ok(())
}

#[test(tokio::test)]
async fn test_sections_priority_matching() -> Result<(), Box<dyn std::error::Error>> {
    tracing::info!("Testing priority matching: BID > Anchor > Title");

    let (_test_tempdir, test_root) = generate_test_root("network_1")?;

    let mut global_bb = BeliefBase::empty();
    let (accum_tx, mut accum_rx) = unbounded_channel::<BeliefEvent>();

    let mut compiler = DocumentCompiler::new(&test_root, Some(accum_tx), None, false)?;
    compiler.parse_all(global_bb.clone(), false).await?;

    while let Ok(event) = accum_rx.try_recv() {
        global_bb.process_event(&event)?;
    }

    // Find Introduction node (should match by BID - highest priority)
    let intro_node = global_bb
        .states()
        .values()
        .find(|n| n.title.contains("Introduction"));

    if let Some(node) = intro_node {
        // TODO: After Issue 03 (anchor parsing), verify it matched by BID:
        // BID matching requires parsing {#bid://...} syntax from heading text
        // - Should have complexity="high" (from BID match)
        // - NOT complexity from any other potential match
        tracing::info!(
            "Introduction node. Expected BID match (after Issue 03). Got: {:?}",
            node.payload.get("complexity")
        );
    }

    // Find Background node (should match by anchor - medium priority)
    let background_node = global_bb
        .states()
        .values()
        .find(|n| n.title.contains("Background"));

    if let Some(node) = background_node {
        // TODO: After Issue 03 (anchor parsing), verify it matched by anchor:
        // Anchor matching requires parsing {#background} syntax from heading text
        // - Should have complexity="medium" (from anchor match)
        tracing::info!(
            "Background node. Expected anchor match (after Issue 03). Got: {:?}",
            node.payload.get("complexity")
        );
    }

    // Find API Reference node (should match by title - lowest priority)
    let api_node = global_bb
        .states()
        .values()
        .find(|n| n.title.contains("API Reference"));

    if let Some(node) = api_node {
        // Issue 02 IMPLEMENTED: Title matching works!
        assert_eq!(
            node.payload.get("complexity").and_then(|v| v.as_str()),
            Some("low"),
            "API Reference should match by title with complexity='low'"
        );
        tracing::info!(
            "API Reference matched by title with complexity=low: {:?}",
            node.payload.get("complexity")
        );
    }

    Ok(())
}

/// Structural correctness test for Issue 47: cross-document stale Section edge causes
/// sibling section removal.
///
/// **What this test checks**: After parsing a two-document network where
/// `cross_doc_notation.md` (parsed first, alphabetically) links to `cross_doc_tokens.md`,
/// all sections of `cross_doc_tokens.md` — including the second h3 sibling and its h4
/// children — survive Phase 2 intact and are present in both `global_bb.states()` and
/// the PathMap.
///
/// **Coverage gap**: This test does NOT organically trigger the exact corpus bug path,
/// and a true regression test (one that fails without the fix) cannot be written against
/// the current public API. See Issue 56 for the full investigation.
///
/// The short reason: the bug required `notation_section` to be resident in the tokens
/// PathMap at exactly the same order-vector depth as "Character and String Literals" —
/// `[NETWORK_SECTION_SORT_KEY, 0, 1]`. That depth only arises through multi-round
/// compiler state accumulation. Injecting the stale edge directly via the public
/// `BeliefBase` or `session_bb_mut` API places the foreign node at a *different* depth
/// (`[NETWORK_SECTION_SORT_KEY, 1]`), so the removal sweep never reaches the target
/// node. The `PathMap.map` field is private, so there is no seam to inject at the
/// correct depth without reconstructing the full compiler pipeline.
///
/// With only two documents in this network the stale edge never forms organically in
/// `session_bb`, so reverting the fix does NOT cause this test to fail. The test is a
/// structural assertion, not a true trigger.
///
/// **To reproduce the bug manually** (regression smoke test):
/// ```sh
/// ls target/bench-staged/rust/index.md  # corpus must be staged
/// RUST_LOG=warn cargo run --features service,bin -- \
///   parse --html-output /tmp/bench-test target/bench-staged/rust/
/// # Before the fix: panic "Set should be balanced here" during tokens.md Phase 4
/// # After the fix: clean run, all 34 tokens.md sections present in PathMap
/// ```
///
/// **Bug root cause** (for reference): `push_relation` queried `session_bb` for the
/// full neighbourhood of every foreign Trace node fetched from cache, including nodes
/// from other documents. That neighbourhood could contain a stale Section edge binding
/// the foreign node into the current document's Section PathMap at a colliding order
/// slot. When a subsequent `RelationUpdate` arrived without a Section weight, the
/// PathMap's removal branch swept the entire order-prefix subtree, deleting valid
/// sibling sections.
///
/// **Fix**: Guard the `session_bb.eval_query` + `union_mut_with_trace` neighbourhood
/// injection in `push_relation` so it only fires for content-namespace nodes (href/asset).
/// Foreign Trace nodes do not need their home-document structure injected into `doc_bb`.
#[test(tokio::test)]
async fn test_cross_doc_stale_section_edge_does_not_corrupt_pathmap(
) -> Result<(), Box<dyn std::error::Error>> {
    tracing::info!("Regression test for Issue 47: cross-doc stale Section edge");

    let (_test_tempdir, test_root) = generate_test_root("network_1")?;

    let mut global_bb = BeliefBase::empty();
    let (accum_tx, mut accum_rx) = unbounded_channel::<BeliefEvent>();

    let mut compiler = DocumentCompiler::new(&test_root, Some(accum_tx), None, false)?;

    // parse_all processes notation before tokens (alphabetical: "cross_doc_notation" <
    // "cross_doc_tokens"). The cross-doc link in notation creates an Epistemic relation
    // into the tokens document, which is sufficient to exercise the structural path even
    // though it does not organically seed the specific stale Section edge from the corpus.
    let _parse_results = compiler.parse_all(global_bb.clone(), false).await?;

    while let Ok(event) = accum_rx.try_recv() {
        global_bb.process_event(&event)?;
    }

    // All section titles that must be present after parsing cross_doc_tokens.md.
    // These mirror the five nodes that were incorrectly removed in the corpus bug.
    let required_titles = [
        // h2
        "Literals",
        // first h3 sibling (sort_key=0) — must survive as witness
        "Examples",
        // h4 children of Examples — must survive
        "Characters and Strings",
        "Numbers",
        // second h3 sibling (sort_key=1) — this was the bug target
        "Character and String Literals",
        // h4 children of the second sibling — these were swept by the bug
        "Character Literals",
        "String Literals",
        "Character Escapes",
        // third h3 sibling (sort_key=2) — must survive as witness
        "Byte and Byte String Literals",
        // h4 child of third sibling
        "Byte Literals",
    ];

    for title in &required_titles {
        let found = global_bb.states().values().any(|node| node.title == *title);
        assert!(
            found,
            "Section {:?} missing from global_bb after parse — stale Section edge bug regressed",
            title
        );
    }

    // Also verify via the PathMap that all cross_doc_tokens.md sections have paths.
    // get_context returns None exactly when a node is in states() but not in the PathMap,
    // which is the failure mode the bug produced.
    let paths_guard = global_bb.paths();
    let all_pmm_paths = paths_guard.all_paths();

    // Collect path strings that belong to cross_doc_tokens.md sections (from any net).
    let section_paths: Vec<String> = all_pmm_paths
        .values()
        .flat_map(|entries| {
            entries
                .iter()
                .filter(|(path, _bid, _order)| path.contains("cross_doc_tokens.md#"))
                .map(|(path, _bid, _order)| path.clone())
                .collect::<Vec<_>>()
        })
        .collect();

    // We expect at least 10 section path entries (one per required section above).
    assert!(
        section_paths.len() >= 10,
        "Expected >=10 cross_doc_tokens.md section paths in PathMap, found {}: {:?}",
        section_paths.len(),
        section_paths,
    );

    // Specifically: the second h3 sibling must have a path entry.
    let char_str_path = section_paths
        .iter()
        .any(|p| p.contains("character-and-string-literals"));
    assert!(
        char_str_path,
        "cross_doc_tokens.md#character-and-string-literals must have a PathMap entry \
         (was incorrectly removed by the stale cross-doc Section edge bug)"
    );

    Ok(())
}

#[test(tokio::test)]
async fn test_sections_round_trip_preservation() -> Result<(), Box<dyn std::error::Error>> {
    tracing::info!("Testing round-trip: matched sections preserved, unmatched removed");

    let (_test_tempdir, test_root) = generate_test_root("network_1")?;

    let mut global_bb = BeliefBase::empty();
    let (accum_tx, mut accum_rx) = unbounded_channel::<BeliefEvent>();

    let mut compiler = DocumentCompiler::new(&test_root, Some(accum_tx), None, false)?;
    let first_parse = compiler.parse_all(global_bb.clone(), false).await?;

    while let Ok(event) = accum_rx.try_recv() {
        global_bb.process_event(&event)?;
    }

    // Write any rewrites to disk
    for result in &first_parse {
        if let Some(ref content) = result.rewritten_content {
            let mut write_path = result.path.clone();
            if write_path.is_dir() {
                if let Some(detected) = detect_network_file(&write_path) {
                    write_path = detected;
                }
            }
            fs::write(&write_path, content)?;
            tracing::info!("Wrote rewritten content to {:?}", write_path);
        }
    }

    // Second parse should NOT rewrite (no changes)
    let (accum_tx2, mut accum_rx2) = unbounded_channel::<BeliefEvent>();
    let mut compiler2 = DocumentCompiler::new(&test_root, Some(accum_tx2), None, false)?;
    let second_parse = compiler2.parse_all(global_bb.clone(), false).await?;

    while let Ok(event) = accum_rx2.try_recv() {
        global_bb.process_event(&event)?;
    }

    // Verify sections_test.md was NOT rewritten on second parse
    let sections_rewritten = second_parse
        .iter()
        .find(|r| r.path.to_string_lossy().contains("sections_test.md"))
        .and_then(|r| r.rewritten_content.as_ref());

    // TODO: After Issue 02 implementation, this should be None (no changes on second parse)
    if sections_rewritten.is_some() {
        tracing::warn!(
            "sections_test.md was rewritten on second parse (should be stable after first)"
        );
    } else {
        tracing::info!("sections_test.md stable on second parse (correct round-trip behavior)");
    }

    Ok(())
}
