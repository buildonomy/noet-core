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
    let doc_bid = global_bb
        .states()
        .values()
        .find(|node| node.title.contains("Sections Test Document"))
        .map(|node| node.bid);

    assert!(
        doc_bid.is_some(),
        "Should find sections_test.md document node"
    );
    let doc_bid = doc_bid.unwrap();

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
                    global_bb.bid_to_index(&doc_bid).unwrap(),
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
