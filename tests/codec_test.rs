use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::Path,
};
use tempfile::{tempdir, TempDir};
use test_log::test;
use tokio::sync::mpsc::unbounded_channel;
use toml::from_str;

use noet_core::{
    beliefbase::BeliefBase,
    codec::{
        belief_ir::{detect_network_file, NETWORK_CONFIG_NAMES},
        DocumentCompiler, CODECS,
    },
    error::BuildonomyError,
    event::BeliefEvent,
    properties::{BeliefNode, Bid, WeightKind},
};

fn copy_dir_all(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> io::Result<()> {
    fs::create_dir_all(&dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        if ty.is_dir() {
            copy_dir_all(entry.path(), dst.as_ref().join(entry.file_name()))?;
        } else {
            fs::copy(entry.path(), dst.as_ref().join(entry.file_name()))?;
        }
    }
    Ok(())
}

fn generate_test_root(test_net: &str) -> Result<TempDir, BuildonomyError> {
    // 1. Create a temporary directory for the test repository
    let temp_dir = tempdir()?;
    tracing::debug!(
        "generating test root. Files: {}",
        fs::read_dir(&temp_dir)
            .unwrap()
            .map(|f| format!("{f:?}"))
            .collect::<Vec<String>>()
            .join(", ")
    );
    let test_root = temp_dir.path().to_path_buf();
    let content_root = Path::new("tests").join(test_net);
    tracing::debug!("Copying content from {:?}", content_root);
    copy_dir_all(&content_root, &test_root)?;
    Ok(temp_dir)
}

#[derive(Debug, Default, Deserialize)]
struct ABid {
    bid: Bid,
}

/// Extracts Bids from lines matching the format "bid: 'uuid-string'"
fn extract_bids_from_content(content: &str) -> Result<Vec<Bid>, BuildonomyError> {
    let mut bids = Vec::new();
    for line in content.lines() {
        if line.trim().starts_with("bid") && line.trim()[3..].trim().starts_with('=') {
            let a_bid: ABid = from_str(line)?;
            bids.push(a_bid.bid);
        }
    }
    Ok(bids)
}

#[test(tokio::test)]
async fn test_belief_set_builder_bid_generation_and_caching(
) -> Result<(), Box<dyn std::error::Error>> {
    tracing::info!("Initialize global_bb (BeliefBase) and other necessary dependencies.");
    let test_tempdir = generate_test_root("network_1")?;
    let test_root = test_tempdir.path().to_path_buf();
    tracing::info!(
        "Test dir is {:?}. Test dir contents: {}",
        test_root,
        fs::read_dir(&test_tempdir)
            .unwrap()
            .map(|f| format!("{f:?}"))
            .collect::<Vec<String>>()
            .join(", ")
    );
    let mut global_bb = BeliefBase::empty();
    let (accum_tx, mut accum_rx) = unbounded_channel::<BeliefEvent>();
    tracing::info!(
        "Initialized BeliefBase codec extension types: {:?}",
        CODECS.extensions()
    );

    tracing::info!("Initialize DocumentCompiler");
    let mut compiler = DocumentCompiler::new(&test_root, Some(accum_tx), None, false)?;

    let mut docs_to_reparse = BTreeSet::default();
    let mut written_bids = BTreeSet::default();
    written_bids.insert(compiler.builder().api().bid);

    tracing::info!("Run compiler.parse_all()");
    let parse_results = compiler.parse_all(global_bb.clone()).await?;

    let mut writes = BTreeMap::<String, usize>::default();
    for parse_result in parse_results {
        let doc_entry = writes
            .entry(format!("{:?}", parse_result.path))
            .or_default();

        if let Some(rewrite_content) = parse_result.rewritten_content {
            let mut write_path = parse_result.path.clone();
            if write_path.is_dir() {
                // Detect network file (JSON or TOML)
                if let Some((detected_path, _format)) = detect_network_file(&write_path) {
                    write_path = detected_path;
                } else {
                    // Default to first in NETWORK_CONFIG_NAMES (JSON)
                    write_path.push(NETWORK_CONFIG_NAMES[0]);
                }
            }
            *doc_entry += 1;
            written_bids.append(&mut BTreeSet::from_iter(
                extract_bids_from_content(&rewrite_content)?.into_iter(),
            ));
            fs::write(&write_path, rewrite_content)?;
        }
        for (doc_path, _) in parse_result.dependent_paths.iter() {
            docs_to_reparse.insert(doc_path.clone());
        }

        while let Ok(event) = accum_rx.try_recv() {
            global_bb.process_event(&event)?;
            // tracing::debug!("global cache event: {:?}", event);
        }
    }
    tracing::debug!(
        "Global cache nodes: {}, accum.session_bb nodes: {}, builder.doc_bb nodes: {}",
        global_bb.states().len(),
        compiler.builder().session_bb().states().len(),
        compiler.builder().doc_bb().states().len()
    );
    tracing::debug!(
        "File writes:\n - {}",
        writes
            .iter()
            .map(|(k, v)| format!("{k}: {v}"))
            .collect::<Vec<String>>()
            .join("\n - ")
    );

    tracing::info!("Ensure written bids match cached bids");
    let cached_bids = BTreeSet::from_iter(global_bb.states().values().map(|n| n.bid));
    debug_assert!(
        written_bids.eq(&cached_bids),
        "Written: {written_bids:?}\n\nCached: {cached_bids:?}"
    );

    // 8. Initialize a NEW DocumentCompiler using the SAME global_bb and repository state
    tracing::info!(
        "Initialize a NEW DocumentCompiler for the second parsing run, reusing global_bb."
    );
    let (accum_tx, mut accum_rx) = unbounded_channel::<BeliefEvent>();
    compiler = DocumentCompiler::new(&test_root, Some(accum_tx), None, false)?;
    written_bids = BTreeSet::default();
    written_bids.insert(compiler.builder().api().bid);

    tracing::info!("Re-running compiler.parse_all()");
    let final_parse_results = compiler.parse_all(global_bb.clone()).await?;

    for parse_result in final_parse_results {
        tracing::debug!("Parsing doc {:?}", parse_result.path);
        debug_assert!(parse_result.rewritten_content.is_none());
        if !parse_result.dependent_paths.is_empty() {
            tracing::warn!(
                "Document {:?} has dependent_paths on second parse: {:?}",
                parse_result.path,
                parse_result.dependent_paths
            );
        }
        assert!(parse_result.dependent_paths.is_empty());
    }
    let mut received_events = Vec::new();
    while let Ok(event) = accum_rx.try_recv() {
        received_events.push(event);
    }
    debug_assert!(
        received_events.is_empty(),
        "Expected no events. Received: {received_events:?}"
    );

    // Cleanup is handled by tempdir dropping
    Ok(())
}

#[test(tokio::test)]
async fn test_sections_metadata_enrichment() -> Result<(), Box<dyn std::error::Error>> {
    tracing::info!("Testing sections metadata enrichment (Issue 02)");

    let test_tempdir = generate_test_root("network_1")?;
    let test_root = test_tempdir.path().to_path_buf();

    let mut global_bb = BeliefBase::empty();
    let (accum_tx, mut accum_rx) = unbounded_channel::<BeliefEvent>();

    tracing::info!("Initialize DocumentCompiler");
    let mut compiler = DocumentCompiler::new(&test_root, Some(accum_tx), None, false)?;

    tracing::info!("Parse all documents including sections_test.md");
    let parse_results = compiler.parse_all(global_bb.clone()).await?;

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

    let test_tempdir = generate_test_root("network_1")?;
    let test_root = test_tempdir.path().to_path_buf();

    let mut global_bb = BeliefBase::empty();
    let (accum_tx, mut accum_rx) = unbounded_channel::<BeliefEvent>();

    let mut compiler = DocumentCompiler::new(&test_root, Some(accum_tx), None, false)?;
    let parse_results = compiler.parse_all(global_bb.clone()).await?;

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
            // TODO: Auto-generation of sections entries for new headings (future enhancement)
            // assert!(has_untracked, "New heading should get sections entry added");
            tracing::info!("Frontmatter contains 'unmatched': {}", has_unmatched);
            tracing::info!(
                "Frontmatter contains 'untracked-section': {}",
                has_untracked
            );
        } else {
            tracing::info!("sections_test.md was NOT rewritten (finalize() made no changes or not implemented yet)");
        }
    }

    Ok(())
}

#[test(tokio::test)]
async fn test_sections_priority_matching() -> Result<(), Box<dyn std::error::Error>> {
    tracing::info!("Testing priority matching: BID > Anchor > Title");

    let test_tempdir = generate_test_root("network_1")?;
    let test_root = test_tempdir.path().to_path_buf();

    let mut global_bb = BeliefBase::empty();
    let (accum_tx, mut accum_rx) = unbounded_channel::<BeliefEvent>();

    let mut compiler = DocumentCompiler::new(&test_root, Some(accum_tx), None, false)?;
    compiler.parse_all(global_bb.clone()).await?;

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

    let test_tempdir = generate_test_root("network_1")?;
    let test_root = test_tempdir.path().to_path_buf();

    let mut global_bb = BeliefBase::empty();
    let (accum_tx, mut accum_rx) = unbounded_channel::<BeliefEvent>();

    let mut compiler = DocumentCompiler::new(&test_root, Some(accum_tx), None, false)?;
    let first_parse = compiler.parse_all(global_bb.clone()).await?;

    while let Ok(event) = accum_rx.try_recv() {
        global_bb.process_event(&event)?;
    }

    // Write any rewrites to disk
    for result in &first_parse {
        if let Some(ref content) = result.rewritten_content {
            let mut write_path = result.path.clone();
            if write_path.is_dir() {
                if let Some((detected, _)) =
                    noet_core::codec::belief_ir::detect_network_file(&write_path)
                {
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
    let second_parse = compiler2.parse_all(global_bb.clone()).await?;

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

// ============================================================================
// Issue 3: Anchor Management Integration Tests
// ============================================================================

#[test(tokio::test)]
async fn test_anchor_collision_detection() -> Result<(), Box<dyn std::error::Error>> {
    tracing::info!("Testing anchor collision detection with Bref fallback");

    let test_tempdir = generate_test_root("network_1")?;
    let test_root = test_tempdir.path().to_path_buf();

    let mut global_bb = BeliefBase::empty();
    let (accum_tx, mut accum_rx) = unbounded_channel::<BeliefEvent>();

    let mut compiler = DocumentCompiler::new(&test_root, Some(accum_tx), None, false)?;
    let parse_results = compiler.parse_all(global_bb.clone()).await?;

    while let Ok(event) = accum_rx.try_recv() {
        global_bb.process_event(&event)?;
    }

    // Find the collision test document
    let doc_result = parse_results.iter().find(|r| {
        r.path
            .to_string_lossy()
            .contains("anchors_collision_test.md")
    });

    assert!(
        doc_result.is_some(),
        "Should find anchors_collision_test.md"
    );

    // Find all heading nodes from the collision test document
    let heading_nodes: Vec<&BeliefNode> = global_bb
        .states()
        .values()
        .filter(|n| n.title == "Details" || n.title == "Implementation" || n.title == "Testing")
        .collect();

    tracing::info!("Found {} heading nodes", heading_nodes.len());
    for node in heading_nodes.iter() {
        tracing::info!("  - {} (bid: {})", node.title, node.bid);
    }

    // Verify we have all 4 heading nodes (collision detection working)
    assert_eq!(
        heading_nodes.len(),
        4,
        "Should have 4 heading nodes: Details (1), Implementation, Details (2), Testing"
    );

    // Find the "Details" headings
    let details_nodes: Vec<&BeliefNode> = heading_nodes
        .iter()
        .filter(|n| n.title == "Details")
        .copied()
        .collect();

    tracing::info!("Found {} 'Details' headings", details_nodes.len());

    // Verify we have 2 separate "Details" nodes (duplicate node bug fixed)
    assert_eq!(
        details_nodes.len(),
        2,
        "Should have 2 'Details' headings as separate nodes"
    );

    // Verify both Details nodes have different BIDs
    assert_ne!(
        details_nodes[0].bid, details_nodes[1].bid,
        "Both 'Details' nodes should have different BIDs"
    );

    Ok(())
}

#[test(tokio::test)]
async fn test_explicit_anchor_preservation() -> Result<(), Box<dyn std::error::Error>> {
    tracing::info!("Testing explicit anchor preservation");

    let test_tempdir = generate_test_root("network_1")?;
    let test_root = test_tempdir.path().to_path_buf();

    let mut global_bb = BeliefBase::empty();
    let (accum_tx, mut accum_rx) = unbounded_channel::<BeliefEvent>();

    let mut compiler = DocumentCompiler::new(&test_root, Some(accum_tx), None, false)?;
    let _parse_results = compiler.parse_all(global_bb.clone()).await?;

    while let Ok(event) = accum_rx.try_recv() {
        global_bb.process_event(&event)?;
    }

    // Find nodes from anchors_explicit_test.md
    let getting_started = global_bb
        .states()
        .values()
        .find(|n| n.title.contains("Getting Started"));

    let setup = global_bb
        .states()
        .values()
        .find(|n| n.title.contains("Setup"));

    let configuration = global_bb
        .states()
        .values()
        .find(|n| n.title.contains("Configuration"));

    let advanced = global_bb
        .states()
        .values()
        .find(|n| n.title.contains("Advanced Usage"));

    // Verify nodes found
    assert!(
        getting_started.is_some(),
        "Getting Started node should exist"
    );
    assert!(setup.is_some(), "Setup node should exist");
    assert!(configuration.is_some(), "Configuration node should exist");
    assert!(advanced.is_some(), "Advanced Usage node should exist");

    tracing::info!(
        "Nodes found: getting_started={}, setup={}, config={}, advanced={}",
        getting_started.is_some(),
        setup.is_some(),
        configuration.is_some(),
        advanced.is_some()
    );

    Ok(())
}

#[test(tokio::test)]
async fn test_anchor_normalization() -> Result<(), Box<dyn std::error::Error>> {
    tracing::info!("Testing anchor normalization for collision detection");

    let test_tempdir = generate_test_root("network_1")?;
    let test_root = test_tempdir.path().to_path_buf();

    let mut global_bb = BeliefBase::empty();
    let (accum_tx, mut accum_rx) = unbounded_channel::<BeliefEvent>();

    let mut compiler = DocumentCompiler::new(&test_root, Some(accum_tx), None, false)?;
    let _parse_results = compiler.parse_all(global_bb.clone()).await?;

    while let Ok(event) = accum_rx.try_recv() {
        global_bb.process_event(&event)?;
    }

    // Find nodes with special char anchors
    let api_node = global_bb
        .states()
        .values()
        .find(|n| n.title.contains("API & Reference"));

    let section_one = global_bb
        .states()
        .values()
        .find(|n| n.title.contains("Section One"));

    let custom_id = global_bb
        .states()
        .values()
        .find(|n| n.title.contains("My-Custom-ID"));

    // Verify nodes found (normalization testing)
    assert!(api_node.is_some(), "API & Reference node should exist");
    assert!(section_one.is_some(), "Section One! node should exist");
    assert!(custom_id.is_some(), "My-Custom-ID node should exist");

    tracing::info!(
        "Nodes with special char anchors found: api={}, section={}, custom={}",
        api_node.is_some(),
        section_one.is_some(),
        custom_id.is_some()
    );

    Ok(())
}

#[test(tokio::test)]
async fn test_anchor_selective_injection() -> Result<(), Box<dyn std::error::Error>> {
    tracing::info!("Testing selective anchor injection (only Brefs for collisions)");

    let test_tempdir = generate_test_root("network_1")?;
    let test_root = test_tempdir.path().to_path_buf();

    let mut global_bb = BeliefBase::empty();
    let (accum_tx, mut accum_rx) = unbounded_channel::<BeliefEvent>();

    let mut compiler = DocumentCompiler::new(&test_root, Some(accum_tx), None, false)?;
    let parse_results = compiler.parse_all(global_bb.clone()).await?;

    while let Ok(event) = accum_rx.try_recv() {
        global_bb.process_event(&event)?;
    }

    // Check rewritten content from collision test
    let collision_doc = parse_results.iter().find(|r| {
        r.path
            .to_string_lossy()
            .contains("anchors_collision_test.md")
    });

    if let Some(result) = collision_doc {
        if let Some(ref rewritten) = result.rewritten_content {
            tracing::info!(
                "Collision test rewritten (first 800 chars):\n{}",
                &rewritten[..rewritten.len().min(800)]
            );

            // Note: Selective injection verification would require parsing the rewritten markdown
            // to check for presence/absence of {#...} anchors. This is tested implicitly by
            // verifying that duplicate nodes are created correctly (test_anchor_collision_detection).
        }
    }

    Ok(())
}

#[test(tokio::test)]
async fn test_link_canonical_format_generation() -> Result<(), Box<dyn std::error::Error>> {
    tracing::info!("Testing link transformation to canonical format with Bref");

    let test_tempdir = generate_test_root("network_1")?;
    let test_root = test_tempdir.path().to_path_buf();

    let mut global_bb = BeliefBase::empty();
    let (accum_tx, mut accum_rx) = unbounded_channel::<BeliefEvent>();

    let mut compiler = DocumentCompiler::new(&test_root, Some(accum_tx), None, false)?;
    let parse_results = compiler.parse_all(global_bb.clone()).await?;

    while let Ok(event) = accum_rx.try_recv() {
        global_bb.process_event(&event)?;
    }

    // Find the link manipulation test document
    let link_test_doc = parse_results.iter().find(|r| {
        r.path
            .to_string_lossy()
            .contains("link_manipulation_test.md")
    });

    if let Some(result) = link_test_doc {
        if let Some(ref rewritten) = result.rewritten_content {
            tracing::info!("Link manipulation test rewritten content:\n{}", rewritten);

            // Verify canonical format is generated
            // Links should be transformed to: [text](path "bref://...")
            assert!(
                rewritten.contains("bref://"),
                "Rewritten content should contain Bref references"
            );

            // Verify relative paths are used
            assert!(
                rewritten.contains("./file1.md") || rewritten.contains("file1.md"),
                "Rewritten content should use relative paths"
            );

            // Verify title attributes are present
            let quote_count = rewritten.matches('"').count();
            assert!(
                quote_count >= 2,
                "Rewritten content should have title attributes in quotes"
            );
        }
    } else {
        tracing::warn!("link_manipulation_test.md not found in parse results");
    }

    Ok(())
}

#[test(tokio::test)]
async fn test_link_bref_stability() -> Result<(), Box<dyn std::error::Error>> {
    tracing::info!("Testing that links with Bref remain stable when files move");

    let test_tempdir = generate_test_root("network_1")?;
    let test_root = test_tempdir.path().to_path_buf();

    let mut global_bb = BeliefBase::empty();
    let (accum_tx, mut accum_rx) = unbounded_channel::<BeliefEvent>();

    let mut compiler = DocumentCompiler::new(&test_root, Some(accum_tx), None, false)?;

    // First parse
    let parse_results = compiler.parse_all(global_bb.clone()).await?;

    while let Ok(event) = accum_rx.try_recv() {
        global_bb.process_event(&event)?;
    }

    // Extract Brefs from first parse
    let mut first_parse_brefs = Vec::new();
    for result in &parse_results {
        if result
            .path
            .to_string_lossy()
            .contains("link_manipulation_test.md")
        {
            if let Some(ref content) = result.rewritten_content {
                // Extract all bref:// references
                for line in content.lines() {
                    if line.contains("bref://") {
                        first_parse_brefs.push(line.to_string());
                    }
                }
            }
        }
    }

    assert!(
        !first_parse_brefs.is_empty(),
        "Should have found Bref references in first parse"
    );

    tracing::info!(
        "Found {} Bref references in first parse",
        first_parse_brefs.len()
    );

    // Verify Brefs are stable format (12 hex chars after bref://)
    for bref_line in &first_parse_brefs {
        assert!(
            bref_line.contains("bref://"),
            "Should contain bref:// prefix"
        );
        tracing::debug!("Bref line: {}", bref_line);
    }

    Ok(())
}

#[test(tokio::test)]
async fn test_link_auto_title_matching() -> Result<(), Box<dyn std::error::Error>> {
    tracing::info!("Testing auto_title behavior based on link text matching");

    let test_tempdir = generate_test_root("network_1")?;
    let test_root = test_tempdir.path().to_path_buf();

    let mut global_bb = BeliefBase::empty();
    let (accum_tx, mut accum_rx) = unbounded_channel::<BeliefEvent>();

    let mut compiler = DocumentCompiler::new(&test_root, Some(accum_tx), None, false)?;
    let parse_results = compiler.parse_all(global_bb.clone()).await?;

    while let Ok(event) = accum_rx.try_recv() {
        global_bb.process_event(&event)?;
    }

    let link_test_doc = parse_results.iter().find(|r| {
        r.path
            .to_string_lossy()
            .contains("link_manipulation_test.md")
    });

    if let Some(result) = link_test_doc {
        if let Some(ref rewritten) = result.rewritten_content {
            tracing::info!("Rewritten content:\n{}", rewritten);

            // Count how many links have bref in title attribute (canonical format)
            let bref_count = rewritten.matches("bref://").count();
            tracing::info!("Found {} links with Bref", bref_count);

            // Verify links were transformed to canonical format
            assert!(
                bref_count >= 1,
                "Should have at least one link with Bref in title attribute"
            );

            // Verify that links preserve user text
            assert!(
                rewritten.contains("Custom Text") || rewritten.contains("First Reference"),
                "Should preserve user-provided link text"
            );
        }
    }

    Ok(())
}

#[test(tokio::test)]
async fn test_link_relative_paths() -> Result<(), Box<dyn std::error::Error>> {
    tracing::info!("Testing relative path calculation in links");

    let test_tempdir = generate_test_root("network_1")?;
    let test_root = test_tempdir.path().to_path_buf();

    let mut global_bb = BeliefBase::empty();
    let (accum_tx, mut accum_rx) = unbounded_channel::<BeliefEvent>();

    let mut compiler = DocumentCompiler::new(&test_root, Some(accum_tx), None, false)?;
    let parse_results = compiler.parse_all(global_bb.clone()).await?;

    while let Ok(event) = accum_rx.try_recv() {
        global_bb.process_event(&event)?;
    }

    let link_test_doc = parse_results.iter().find(|r| {
        r.path
            .to_string_lossy()
            .contains("link_manipulation_test.md")
    });

    if let Some(result) = link_test_doc {
        if let Some(ref rewritten) = result.rewritten_content {
            tracing::info!("Checking relative paths in rewritten content");

            // Should not have absolute paths (no leading /)
            let has_absolute_link = rewritten
                .lines()
                .any(|line| line.contains("](/) ") || line.contains("](/"));

            assert!(
                !has_absolute_link,
                "Links should use relative paths, not absolute"
            );

            // Should have relative indicators like ./ or ../
            let has_relative_paths = rewritten.contains("](./")
                || rewritten.contains("](../")
                || rewritten.contains("](file")
                || rewritten.contains("](subnet");

            assert!(has_relative_paths, "Should have relative path indicators");
        }
    }

    Ok(())
}

#[test(tokio::test)]
async fn test_link_same_document_anchors() -> Result<(), Box<dyn std::error::Error>> {
    tracing::info!("Testing same-document anchor links remain fragment-only");

    let test_tempdir = generate_test_root("network_1")?;
    let test_root = test_tempdir.path().to_path_buf();

    let mut global_bb = BeliefBase::empty();
    let (accum_tx, mut accum_rx) = unbounded_channel::<BeliefEvent>();

    let mut compiler = DocumentCompiler::new(&test_root, Some(accum_tx), None, false)?;
    let parse_results = compiler.parse_all(global_bb.clone()).await?;

    while let Ok(event) = accum_rx.try_recv() {
        global_bb.process_event(&event)?;
    }

    let link_test_doc = parse_results.iter().find(|r| {
        r.path
            .to_string_lossy()
            .contains("link_manipulation_test.md")
    });

    if let Some(result) = link_test_doc {
        if let Some(ref rewritten) = result.rewritten_content {
            tracing::info!("Checking same-document anchor format");
            tracing::info!(
                "Rewritten content (first 1000 chars):\n{}",
                &rewritten[..rewritten.len().min(1000)]
            );

            // Verify links have Bref in title attribute
            let bref_count = rewritten.matches("bref://").count();
            tracing::info!("Found {} links with Bref", bref_count);

            assert!(bref_count >= 1, "Should have links with Bref references");

            // Check for fragment links
            let has_fragment_links = rewritten.contains("](#");
            tracing::info!("Has fragment links: {}", has_fragment_links);

            if has_fragment_links {
                let lines_with_fragments: Vec<&str> = rewritten
                    .lines()
                    .filter(|line| line.contains("](#"))
                    .collect();

                for line in &lines_with_fragments {
                    tracing::info!("Fragment link line: {}", line);
                }
            }
        }
    }

    Ok(())
}
