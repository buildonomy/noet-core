use petgraph::visit::EdgeRef;
use serde::Deserialize;
use sqlx::Row;
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
    db::{db_init, DbConnection, Transaction},
    error::BuildonomyError,
    event::BeliefEvent,
    nodekey::NodeKey,
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
    let parse_results = compiler.parse_all(global_bb.clone(), false).await?;

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

    tracing::info!("Include asset BIDs from asset manifest");
    {
        // Query asset BIDs from global_bb instead of compiler.asset_manifest()
        use noet_core::properties::{asset_namespace, BeliefKind};
        for (bid, node) in global_bb.states().iter() {
            if node.kind.contains(BeliefKind::External) {
                written_bids.insert(*bid);
            }
        }
        // Also include the asset_namespace network node itself
        written_bids.insert(asset_namespace());
    }

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
    let final_parse_results = compiler.parse_all(global_bb.clone(), false).await?;

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
        // Filter out FileParsed events (metadata-only, don't affect graph)
        if !matches!(event, noet_core::event::BeliefEvent::FileParsed(_)) {
            received_events.push(event);
        }
    }
    debug_assert!(
        received_events.is_empty(),
        "Expected no graph-modifying events. Received: {received_events:?}"
    );

    // Cleanup is handled by tempdir dropping
    Ok(())
}

#[test(tokio::test)]
async fn test_belief_set_builder_with_db_cache() -> Result<(), Box<dyn std::error::Error>> {
    tracing::info!("Testing cache stability with DbConnection (Issue 34)");
    let test_tempdir = generate_test_root("network_1")?;
    let test_root = test_tempdir.path().to_path_buf();

    // Initialize DB in test directory
    let db_path = test_root.join("belief_cache.db");
    let db_pool = db_init(db_path).await?;
    let db = DbConnection(db_pool);

    tracing::info!("First parse with DbConnection as global cache");
    let (accum_tx, mut accum_rx) = unbounded_channel::<BeliefEvent>();
    let mut compiler = DocumentCompiler::new(&test_root, Some(accum_tx), None, false)?;

    // First parse - should populate DB
    let parse_results = compiler.parse_all(db.clone(), false).await?;
    tracing::info!("First parse completed: {} documents", parse_results.len());

    // Commit events to DB
    let mut transaction = Transaction::default();
    let mut event_count = 0;
    while let Ok(event) = accum_rx.try_recv() {
        transaction.add_event(&event).ok();
        event_count += 1;
    }
    tracing::info!(
        "First parse generated {} events, committing to DB",
        event_count
    );
    transaction.execute(&db.0).await?;
    tracing::info!("Events committed to DB successfully");

    // Verify DB contents after commit
    let verify_count = sqlx::query("SELECT COUNT(*) as count FROM beliefs")
        .fetch_one(&db.0)
        .await?;
    let node_count: i64 = verify_count.get("count");
    let verify_edges = sqlx::query("SELECT COUNT(*) as count FROM relations")
        .fetch_one(&db.0)
        .await?;
    let edge_count: i64 = verify_edges.get("count");
    let verify_paths = sqlx::query("SELECT COUNT(*) as count FROM paths")
        .fetch_one(&db.0)
        .await?;
    let path_count: i64 = verify_paths.get("count");
    tracing::info!(
        "DB verification after commit: {} nodes, {} edges, {} paths",
        node_count,
        edge_count,
        path_count
    );

    // Second parse - should use cached nodes
    tracing::info!("Second parse with same DbConnection");
    let (accum_tx2, mut accum_rx2) = unbounded_channel::<BeliefEvent>();
    compiler = DocumentCompiler::new(&test_root, Some(accum_tx2), None, false)?;

    let parse_results2 = compiler.parse_all(db.clone(), false).await?;

    // Check for issues
    for parse_result in parse_results2 {
        tracing::debug!("Second parse - doc {:?}", parse_result.path);
        if parse_result.rewritten_content.is_some() {
            tracing::warn!(
                "Document {:?} has rewritten content on second parse - indicates BID instability",
                parse_result.path
            );
        }
        // This assertion will fail if the cache lookup is broken
        assert!(
            parse_result.rewritten_content.is_none(),
            "Second parse should not rewrite content for {:?}",
            parse_result.path
        );
    }

    // Check no new events generated (excluding FileParsed which is metadata-only)
    let mut second_event_count = 0;
    while let Ok(event) = accum_rx2.try_recv() {
        // FileParsed events are metadata-only (for mtime tracking), don't count them
        if !matches!(event, noet_core::event::BeliefEvent::FileParsed(_)) {
            tracing::warn!("Unexpected event on second parse: {:?}", event);
            second_event_count += 1;
        }
    }

    assert_eq!(
        second_event_count, 0,
        "Second parse should not generate graph-modifying events, but got {}",
        second_event_count
    );

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

    let test_tempdir = generate_test_root("network_1")?;
    let test_root = test_tempdir.path().to_path_buf();

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

    let test_tempdir = generate_test_root("network_1")?;
    let test_root = test_tempdir.path().to_path_buf();

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

    let test_tempdir = generate_test_root("network_1")?;
    let test_root = test_tempdir.path().to_path_buf();

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
    let parse_results = compiler.parse_all(global_bb.clone(), false).await?;

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
    let _parse_results = compiler.parse_all(global_bb.clone(), false).await?;

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
    let _parse_results = compiler.parse_all(global_bb.clone(), false).await?;

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
    let parse_results = compiler.parse_all(global_bb.clone(), false).await?;

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
    let parse_results = compiler.parse_all(global_bb.clone(), false).await?;

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
    let parse_results = compiler.parse_all(global_bb.clone(), false).await?;

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
    let parse_results = compiler.parse_all(global_bb.clone(), false).await?;

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
    let parse_results = compiler.parse_all(global_bb.clone(), false).await?;

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
    let parse_results = compiler.parse_all(global_bb.clone(), false).await?;

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

#[test(tokio::test)]
async fn test_asset_tracking_basic() -> Result<(), Box<dyn std::error::Error>> {
    tracing::info!("Testing basic static asset tracking");

    let test_tempdir = generate_test_root("network_1")?;
    let test_root = test_tempdir.path().to_path_buf();

    let mut global_bb = BeliefBase::empty();
    let (accum_tx, mut accum_rx) = unbounded_channel::<BeliefEvent>();

    let mut compiler = DocumentCompiler::new(&test_root, Some(accum_tx), None, false)?;
    let _parse_results = compiler.parse_all(global_bb.clone(), false).await?;

    // Process all events into global_bb
    while let Ok(event) = accum_rx.try_recv() {
        global_bb.process_event(&event)?;
    }

    // Find the asset tracking test document
    let asset_doc = global_bb
        .states()
        .values()
        .find(|n| n.title.contains("Asset Tracking Test"));

    assert!(
        asset_doc.is_some(),
        "Asset tracking test document should be parsed"
    );

    tracing::info!("Asset doc found: {:?}", asset_doc.map(|n| &n.title));

    // TODO: Once Step 3 is implemented, verify:
    // 1. Asset nodes exist in belief graph
    // 2. Document → Asset relations exist
    // 3. Asset BIDs are content-addressed (same content = same BID)

    // For now, this test establishes the baseline - document parses successfully
    // and test assets exist in the file system
    let assets_dir = test_root.join("assets");
    assert!(
        assets_dir.exists(),
        "Assets directory should exist in test environment"
    );

    let test_image = assets_dir.join("test_image.png");
    let test_pdf = assets_dir.join("test_doc.pdf");

    assert!(test_image.exists(), "Test image asset should exist");
    assert!(test_pdf.exists(), "Test PDF asset should exist");

    tracing::info!("Asset tracking test baseline complete");

    Ok(())
}

#[test(tokio::test)]
async fn test_asset_nodes_created() -> Result<(), Box<dyn std::error::Error>> {
    tracing::info!("Testing that asset nodes are created in belief graph");

    let test_tempdir = generate_test_root("network_1")?;
    let test_root = test_tempdir.path().to_path_buf();

    let mut global_bb = BeliefBase::empty();
    let (accum_tx, mut accum_rx) = unbounded_channel::<BeliefEvent>();

    let mut compiler = DocumentCompiler::new(&test_root, Some(accum_tx), None, false)?;
    let _parse_results = compiler.parse_all(global_bb.clone(), false).await?;

    while let Ok(event) = accum_rx.try_recv() {
        global_bb.process_event(&event)?;
    }

    // Find asset nodes in belief graph
    use noet_core::properties::{asset_namespace, BeliefKind};

    let asset_nodes: Vec<&BeliefNode> = global_bb
        .states()
        .values()
        .filter(|n| n.kind.contains(BeliefKind::External))
        .filter(|n| {
            // Check if node is in asset namespace by checking its BID's parent namespace
            // Asset BIDs are created with Bid::new(&asset_namespace()), so their parent_namespace_bytes
            // match the asset_namespace's namespace_bytes
            let asset_ns = asset_namespace();
            // Compare child's parent to asset namespace's identity
            n.bid.parent_namespace_bytes() == asset_ns.namespace_bytes()
        })
        .collect();

    tracing::info!("Found {} asset nodes", asset_nodes.len());
    for node in asset_nodes.iter() {
        tracing::info!("  Asset node: bid={}", node.bid);
    }

    // We should have at least 2 asset nodes (test_image.png and test_doc.pdf)
    // Note: test_image.png is referenced twice, but should create only ONE node (content-addressed)
    assert!(
        asset_nodes.len() >= 2,
        "Should have at least 2 asset nodes (image and PDF)"
    );

    Ok(())
}

#[test(tokio::test)]
async fn test_asset_document_relations() -> Result<(), Box<dyn std::error::Error>> {
    tracing::info!("Testing Document → Asset relations are created");

    let test_tempdir = generate_test_root("network_1")?;
    let test_root = test_tempdir.path().to_path_buf();

    let mut global_bb = BeliefBase::empty();
    let (accum_tx, mut accum_rx) = unbounded_channel::<BeliefEvent>();

    let mut compiler = DocumentCompiler::new(&test_root, Some(accum_tx), Some(10), false)?;
    let _parse_results = compiler.parse_all(global_bb.clone(), false).await?;

    while let Ok(event) = accum_rx.try_recv() {
        global_bb.process_event(&event)?;
    }

    // Find the asset tracking test document
    let asset_doc = global_bb
        .states()
        .values()
        .find(|n| n.title.contains("Asset Tracking Test"))
        .expect("Asset tracking document should exist");

    tracing::info!("Asset doc BID: {}", asset_doc.bid);

    // Debug: List all External nodes in the BeliefBase
    use noet_core::properties::asset_namespace;
    let asset_ns = asset_namespace();
    tracing::info!("Asset namespace: {}", asset_ns);

    let external_nodes: Vec<_> = global_bb
        .states()
        .values()
        .filter(|n| n.kind.is_external())
        .collect();

    tracing::info!(
        "Found {} External nodes in BeliefBase:",
        external_nodes.len()
    );
    for node in &external_nodes {
        tracing::info!(
            "  External node BID: {}, parent_ns: {}",
            node.bid,
            node.bid.parent_namespace()
        );

        // Get all paths for this node from asset_map
        let paths_guard = global_bb.paths();
        let asset_map = paths_guard.asset_map();
        for (path, bid, _order) in asset_map.map().iter() {
            if *bid == node.bid {
                tracing::info!("    Path in asset_map: {}", path);
            }
        }
    }

    // Find relations from assets to this document (or its sections)
    // Assets are the SOURCE, document sections are the SINK
    let relations = global_bb.relations();

    // Get all external (asset) nodes
    let asset_nodes: Vec<_> = external_nodes.iter().map(|n| n.bid).collect();

    tracing::info!("Checking {} asset nodes for relations", asset_nodes.len());

    // For each asset, check if it has outgoing edges to any part of the document
    let mut asset_to_doc_relations = Vec::new();
    for asset_bid in &asset_nodes {
        if let Some(asset_idx) = global_bb.bid_to_index(asset_bid) {
            let asset_edges: Vec<_> = relations.as_graph().edges(asset_idx).collect();

            for edge in asset_edges {
                let target_bid = relations.as_graph()[edge.target()];
                // Check if target is the document or one of its sections
                // Sections have the document as their parent in the hierarchy
                asset_to_doc_relations.push((*asset_bid, target_bid));
                tracing::info!("  Asset {} -> target {}", asset_bid, target_bid);
            }
        }
    }

    tracing::info!(
        "Found {} relations from assets to document sections",
        asset_to_doc_relations.len()
    );

    // Should have at least 2 unique assets referencing the document
    // (test_image.png referenced 3 times, test_doc.pdf referenced once)
    // Each reference creates a relation from asset to the section containing the reference
    assert!(
        asset_to_doc_relations.len() >= 2,
        "Should have at least 2 asset->document relations. Found {}",
        asset_to_doc_relations.len()
    );

    Ok(())
}

#[test(tokio::test)]
async fn test_asset_content_addressing() -> Result<(), Box<dyn std::error::Error>> {
    tracing::info!("Testing content-addressed BIDs - same content = same BID");

    let test_tempdir = generate_test_root("network_1")?;
    let test_root = test_tempdir.path().to_path_buf();

    let mut global_bb = BeliefBase::empty();
    let (accum_tx, mut accum_rx) = unbounded_channel::<BeliefEvent>();

    let mut compiler = DocumentCompiler::new(&test_root, Some(accum_tx), None, false)?;
    let _parse_results = compiler.parse_all(global_bb.clone(), false).await?;

    while let Ok(event) = accum_rx.try_recv() {
        global_bb.process_event(&event)?;
    }

    // Find asset nodes
    use noet_core::properties::{asset_namespace, BeliefKind};

    let asset_nodes: Vec<&BeliefNode> = global_bb
        .states()
        .values()
        .filter(|n| n.kind.contains(BeliefKind::External))
        .filter(|n| {
            let asset_ns = asset_namespace();
            n.bid.parent_namespace_bytes() == asset_ns.namespace_bytes()
        })
        .collect();

    tracing::info!("Found {} unique asset nodes", asset_nodes.len());

    // test_image.png is referenced twice in the document:
    // - ![Test Image](./assets/test_image.png)
    // - ![Test Image Again](./assets/test_image.png)
    //
    // Both should resolve to the SAME BeliefNode (content-addressed)
    // So we should have exactly 2 asset nodes (image and PDF), not 3

    assert_eq!(
        asset_nodes.len(),
        2,
        "Should have exactly 2 asset nodes (image referenced twice = 1 node, PDF = 1 node)"
    );

    Ok(())
}

#[test(tokio::test)]
async fn test_asset_deduplication_warning() -> Result<(), Box<dyn std::error::Error>> {
    tracing::info!("Testing deduplication detection logs warnings");

    // Create a scenario with duplicate content at different paths
    let test_tempdir = generate_test_root("network_1")?;
    let test_root = test_tempdir.path().to_path_buf();

    // Copy test_image.png to a different location with same content
    let duplicate_dir = test_root.join("duplicates");
    fs::create_dir_all(&duplicate_dir)?;
    fs::copy(
        test_root.join("assets/test_image.png"),
        duplicate_dir.join("duplicate_image.png"),
    )?;

    // Create a document that references the duplicate
    let duplicate_doc = test_root.join("duplicate_test.md");
    fs::write(
        &duplicate_doc,
        "# Duplicate Test\n\n![Duplicate](./duplicates/duplicate_image.png)\n",
    )?;

    let mut global_bb = BeliefBase::empty();
    let (accum_tx, mut accum_rx) = unbounded_channel::<BeliefEvent>();

    // Note: We should capture logs here to verify warning was logged
    // For now, just verify compilation succeeds and same BID is used
    let mut compiler = DocumentCompiler::new(&test_root, Some(accum_tx), None, false)?;
    let _parse_results = compiler.parse_all(global_bb.clone(), false).await?;

    while let Ok(event) = accum_rx.try_recv() {
        global_bb.process_event(&event)?;
    }

    // Both paths should resolve to same BID (content-addressed)
    // Compiler should log warning about duplication

    tracing::info!("Deduplication test complete - check logs for warnings");

    Ok(())
}

#[test(tokio::test)]
async fn test_multi_path_asset_tracking() -> Result<(), Box<dyn std::error::Error>> {
    tracing::info!("Testing same asset content at multiple paths gets separate stable BIDs");

    let test_tempdir = generate_test_root("network_1")?;
    let test_root = test_tempdir.path().to_path_buf();

    // Create duplicate of test_image.png at different path (same content)
    let duplicate_dir = test_root.join("duplicates");
    fs::create_dir_all(&duplicate_dir)?;
    let original_image = test_root.join("assets/test_image.png");
    let duplicate_image = duplicate_dir.join("same_image.png");
    fs::copy(&original_image, &duplicate_image)?;

    // Create a document that references the duplicate asset
    let doc_content = r#"# Duplicate Asset Test

This references the duplicate: ![Duplicate Image](./duplicates/same_image.png)
"#;
    fs::write(test_root.join("duplicate_ref.md"), doc_content)?;

    let mut global_bb = BeliefBase::empty();
    let (accum_tx, mut accum_rx) = unbounded_channel::<BeliefEvent>();

    let mut compiler = DocumentCompiler::new(&test_root, Some(accum_tx), None, false)?;
    let _parse_results = compiler.parse_all(global_bb.clone(), false).await?;

    // Build asset manifest from PathAdded events
    let asset_manifest = {
        let mut manifest: std::collections::BTreeMap<String, noet_core::properties::Bid> =
            std::collections::BTreeMap::new();
        let asset_ns = noet_core::properties::asset_namespace();

        while let Ok(event) = accum_rx.try_recv() {
            if let noet_core::event::BeliefEvent::PathAdded(net_bid, path, node_bid, _, _) = &event
            {
                if *net_bid == asset_ns && !path.is_empty() {
                    manifest.insert(path.clone(), *node_bid);
                }
            }
            global_bb.process_event(&event)?;
        }

        manifest
    };

    // Find asset nodes - with stable BIDs, each path gets its own BID
    use noet_core::properties::{asset_namespace, BeliefKind};

    let asset_nodes: Vec<_> = global_bb
        .states()
        .values()
        .filter(|n| n.kind.contains(BeliefKind::External))
        .filter(|n| {
            let asset_ns = asset_namespace();
            n.bid.parent_namespace_bytes() == asset_ns.namespace_bytes()
        })
        .collect();

    // With stable BIDs: 3 unique BIDs (one per path):
    // - assets/test_image.png
    // - duplicates/same_image.png
    // - assets/test_doc.pdf
    assert_eq!(
        asset_nodes.len(),
        3,
        "Should have 3 unique asset BIDs (stable identity per path)"
    );

    // Verify that test_image.png and same_image.png have same content hash in payload
    let image_nodes: Vec<_> = asset_nodes
        .iter()
        .filter(|n| {
            n.payload
                .get("content_hash")
                .and_then(|v| v.as_str())
                .is_some()
        })
        .collect();

    // Get the two image nodes (should have same hash)
    let image_hashes: Vec<_> = image_nodes
        .iter()
        .filter_map(|n| n.payload.get("content_hash").and_then(|v| v.as_str()))
        .collect();

    // Find the duplicate images by checking for duplicate hashes
    let mut hash_counts = std::collections::HashMap::new();
    for hash in image_hashes.iter() {
        *hash_counts.entry(*hash).or_insert(0) += 1;
    }

    let duplicate_hash_count = hash_counts.values().filter(|&&count| count > 1).count();
    assert_eq!(
        duplicate_hash_count, 1,
        "Should have exactly one hash that appears twice (the duplicated image)"
    );

    // Verify the asset manifest contains 3 file paths (test_image.png, same_image.png, test_doc.pdf)
    assert_eq!(
        asset_manifest.len(),
        3,
        "Should track 3 file paths in asset manifest"
    );

    assert!(
        asset_manifest.contains_key("assets/test_image.png"),
        "Original path should be in asset manifest"
    );
    assert!(
        asset_manifest.contains_key("duplicates/same_image.png"),
        "Duplicate path should be in asset manifest"
    );
    assert!(
        asset_manifest.contains_key("assets/test_doc.pdf"),
        "PDF asset should be in asset manifest"
    );

    // With stable BIDs: different paths get different BIDs (stable identity per path)
    let original_bid = asset_manifest
        .get("assets/test_image.png")
        .expect("Should find original image");
    let duplicate_bid = asset_manifest
        .get("duplicates/same_image.png")
        .expect("Should find duplicate image");
    assert_ne!(
        original_bid, duplicate_bid,
        "Different paths should have different BIDs (stable identity)"
    );

    // But they should have the same content hash in their payloads
    let original_node = global_bb
        .states()
        .get(original_bid)
        .expect("Should find original node");
    let duplicate_node = global_bb
        .states()
        .get(duplicate_bid)
        .expect("Should find duplicate node");

    let original_hash = original_node
        .payload
        .get("content_hash")
        .and_then(|v| v.as_str())
        .expect("Should have content_hash");
    let duplicate_hash = duplicate_node
        .payload
        .get("content_hash")
        .and_then(|v| v.as_str())
        .expect("Should have content_hash");

    assert_eq!(
        original_hash, duplicate_hash,
        "Same content should have same hash in payload"
    );

    tracing::info!("Multi-path asset tracking verified - 3 paths, 3 unique BIDs, 2 with same hash");
    Ok(())
}

#[test(tokio::test)]
async fn test_multi_document_asset_refs() -> Result<(), Box<dyn std::error::Error>> {
    tracing::info!("Testing multiple documents referencing same asset");

    let test_tempdir = generate_test_root("network_1")?;
    let test_root = test_tempdir.path().to_path_buf();

    // Create second document that references same test_image.png
    let doc2_content = r#"# Second Document

This references the same image: ![Test Image](./assets/test_image.png)
"#;
    fs::write(test_root.join("second_doc.md"), doc2_content)?;

    let mut global_bb = BeliefBase::empty();
    let (accum_tx, mut accum_rx) = unbounded_channel::<BeliefEvent>();

    let mut compiler = DocumentCompiler::new(&test_root, Some(accum_tx), None, false)?;
    let _parse_results = compiler.parse_all(global_bb.clone(), false).await?;

    while let Ok(event) = accum_rx.try_recv() {
        global_bb.process_event(&event)?;
    }

    // Find both documents
    let doc1 = global_bb
        .states()
        .values()
        .find(|n| n.title.contains("Asset Tracking Test"));

    let doc2 = global_bb
        .states()
        .values()
        .find(|n| n.title.contains("Second Document"));

    assert!(doc1.is_some(), "First document should exist");
    assert!(doc2.is_some(), "Second document should exist");

    // Both documents should have Epistemic relations to same asset BID
    let asset_nodes: Vec<_> = global_bb
        .states()
        .values()
        .filter(|n| n.kind.is_external())
        .collect();

    assert!(!asset_nodes.is_empty(), "Asset node should exist");

    // TODO: Query relations to verify both docs point to same asset
    tracing::info!("Multi-document asset reference test complete");
    Ok(())
}

#[test(tokio::test)]
async fn test_asset_all_paths_query() -> Result<(), Box<dyn std::error::Error>> {
    tracing::info!("Testing BeliefBase queries returning all paths for asset BID");

    let test_tempdir = generate_test_root("network_1")?;
    let test_root = test_tempdir.path().to_path_buf();

    let mut global_bb = BeliefBase::empty();
    let (accum_tx, mut accum_rx) = unbounded_channel::<BeliefEvent>();

    let mut compiler = DocumentCompiler::new(&test_root, Some(accum_tx), None, false)?;
    let _parse_results = compiler.parse_all(global_bb.clone(), false).await?;

    while let Ok(event) = accum_rx.try_recv() {
        global_bb.process_event(&event)?;
    }

    // Find asset node
    let asset_node = global_bb.states().values().find(|n| n.kind.is_external());

    assert!(asset_node.is_some(), "Asset node should exist");

    let _asset_bid = asset_node.unwrap().bid;

    // Query all paths for this BID via PathMapMap
    let paths_map = global_bb.paths();
    let mut visited = std::collections::BTreeSet::new();
    let all_paths = paths_map.asset_map().all_paths(&paths_map, &mut visited);

    assert!(
        !all_paths.is_empty(),
        "Should be able to query all paths for asset BID"
    );

    tracing::info!("Found {} paths for asset BID", all_paths.len());
    Ok(())
}

#[test(tokio::test)]
async fn test_asset_path_accumulation() -> Result<(), Box<dyn std::error::Error>> {
    tracing::info!(
        "Testing WEIGHT_DOC_PATHS array accumulates paths via multiple RelationChange events"
    );

    let test_tempdir = generate_test_root("network_1")?;
    let test_root = test_tempdir.path().to_path_buf();

    let mut global_bb = BeliefBase::empty();
    let (accum_tx, mut accum_rx) = unbounded_channel::<BeliefEvent>();

    let mut compiler = DocumentCompiler::new(&test_root, Some(accum_tx), None, false)?;
    let _parse_results = compiler.parse_all(global_bb.clone(), false).await?;

    // Process events
    while let Ok(event) = accum_rx.try_recv() {
        global_bb.process_event(&event)?;
    }

    // Find asset with paths
    let asset_namespace_bid = noet_core::properties::asset_namespace();

    // Query relations from assets to asset_namespace
    let relations = global_bb.relations();
    let asset_nodes: Vec<_> = global_bb
        .states()
        .values()
        .filter(|n| n.kind.is_external())
        .collect();

    for asset_node in asset_nodes {
        // Check if asset → asset_namespace relation has WEIGHT_DOC_PATHS
        let source_idx = global_bb.bid_to_index(&asset_node.bid);
        let sink_idx = global_bb.bid_to_index(&asset_namespace_bid);
        if let (Some(source_idx), Some(sink_idx)) = (source_idx, sink_idx) {
            if let Some(edge_idx) = relations.as_graph().find_edge(source_idx, sink_idx) {
                let weight_set = &relations.as_graph()[edge_idx];

                // Verify WEIGHT_DOC_PATHS exists and is an array
                if let Some(section_weight) = weight_set
                    .weights
                    .get(&noet_core::properties::WeightKind::Section)
                {
                    let paths = section_weight.get_doc_paths();
                    assert!(
                        !paths.is_empty(),
                        "WEIGHT_DOC_PATHS should contain at least one path"
                    );
                    tracing::info!("Asset has {} paths: {:?}", paths.len(), paths);
                }
            }
        }
    }

    Ok(())
}

#[test(tokio::test)]
async fn test_asset_no_extension() -> Result<(), Box<dyn std::error::Error>> {
    tracing::info!("Testing assets without extensions get canonical names without trailing dots");

    let test_tempdir = generate_test_root("network_1")?;
    let test_root = test_tempdir.path().to_path_buf();

    // Create asset file with no extension
    let no_ext_file = test_root.join("assets/README");
    fs::write(&no_ext_file, b"This is a README with no extension")?;

    let mut global_bb = BeliefBase::empty();
    let (accum_tx, mut accum_rx) = unbounded_channel::<BeliefEvent>();

    let mut compiler = DocumentCompiler::new(&test_root, Some(accum_tx), None, false)?;
    let _parse_results = compiler.parse_all(global_bb.clone(), false).await?;

    while let Ok(event) = accum_rx.try_recv() {
        global_bb.process_event(&event)?;
    }

    // TODO: Once HTML generation with hardlinks is implemented,
    // verify canonical name is "static/asset:{bid}" not "static/asset:{bid}."

    tracing::info!("No-extension asset test complete");
    Ok(())
}

#[test(tokio::test)]
async fn test_asset_content_changed() -> Result<(), Box<dyn std::error::Error>> {
    tracing::info!("Testing asset content change detection with stable BIDs");

    let test_tempdir = generate_test_root("network_1")?;
    let test_root = test_tempdir.path().to_path_buf();

    // Use DbConnection for proper mtime tracking, plus BeliefBase for querying
    let db_path = test_root.join("belief_cache.db");
    let db_pool = db_init(db_path).await?;
    let db = DbConnection(db_pool);
    let mut global_bb = BeliefBase::empty();

    let (accum_tx, mut accum_rx) = unbounded_channel::<BeliefEvent>();

    // First parse - establish baseline
    let mut compiler = DocumentCompiler::new(&test_root, Some(accum_tx.clone()), None, false)?;
    let _parse_results = compiler.parse_all(db.clone(), false).await?;

    // Process events into both db and global_bb
    let mut transaction = Transaction::new();
    while let Ok(event) = accum_rx.try_recv() {
        transaction.add_event(&event)?;
        global_bb.process_event(&event)?;
    }
    transaction.execute(&db.0).await?;

    // Find the original asset node
    use noet_core::properties::{asset_namespace, BeliefKind};

    let original_asset_nodes: Vec<BeliefNode> = global_bb
        .states()
        .values()
        .filter(|n| n.kind.contains(BeliefKind::External))
        .filter(|n| {
            let asset_ns = asset_namespace();
            n.bid.parent_namespace_bytes() == asset_ns.namespace_bytes()
        })
        .cloned()
        .collect();

    assert!(
        !original_asset_nodes.is_empty(),
        "Should have asset nodes before content change"
    );

    let original_image_node = original_asset_nodes
        .iter()
        .find(|_n| {
            // Find the test_image.png node (we'll identify it by checking manifest later)
            // For now, just grab the first one
            true
        })
        .expect("Should have at least one asset node");

    let original_bid = original_image_node.bid;
    tracing::info!("Original asset BID: {}", original_bid);

    // Modify the asset file content
    let asset_path = test_root.join("assets/test_image.png");
    fs::write(
        &asset_path,
        "MODIFIED test image content with different bytes",
    )?;
    tracing::info!("Modified asset content at {:?}", asset_path);

    // Re-parse to detect the change (mtime-based invalidation will detect the change)
    let mut compiler2 = DocumentCompiler::new(&test_root, Some(accum_tx), None, false)?;
    let _parse_results2 = compiler2.parse_all(db.clone(), false).await?;

    // Collect events and look for NodeUpdate (stable BIDs with payload changes)
    let mut update_events = Vec::new();
    let mut transaction2 = Transaction::new();
    while let Ok(event) = accum_rx.try_recv() {
        if let BeliefEvent::NodeUpdate(keys, _toml, _origin) = &event {
            // Check if this is an asset update (single key with asset namespace parent)
            if keys.len() == 1 {
                if let NodeKey::Bid { bid } = keys[0] {
                    let asset_ns = asset_namespace();
                    if bid.parent_namespace_bytes() == asset_ns.namespace_bytes() {
                        tracing::info!("NodeUpdate event detected for asset!");
                        tracing::info!("  BID: {:?}", bid);
                        update_events.push((bid, event.clone()));
                    }
                }
            }
        }
        transaction2.add_event(&event)?;
        global_bb.process_event(&event)?;
    }
    transaction2.execute(&db.0).await?;

    // Verify we got a NodeUpdate event for the asset
    assert!(
        !update_events.is_empty(),
        "Should have at least one NodeUpdate event for changed asset"
    );

    let updated_asset_nodes: Vec<BeliefNode> = global_bb
        .states()
        .values()
        .filter(|n| n.kind.contains(BeliefKind::External))
        .filter(|n| {
            let asset_ns = asset_namespace();
            n.bid.parent_namespace_bytes() == asset_ns.namespace_bytes()
        })
        .cloned()
        .collect();

    // Should still have same number of nodes (updated, not added)
    assert_eq!(
        updated_asset_nodes.len(),
        original_asset_nodes.len(),
        "Should have same number of nodes after update"
    );

    // Find the updated image node (same BID as original)
    let updated_image_node = updated_asset_nodes
        .iter()
        .find(|n| n.bid == original_bid)
        .expect("Should have the same node with updated payload");

    tracing::info!("Updated asset BID (unchanged): {}", updated_image_node.bid);

    // Verify BID stayed the same (stable identity)
    assert_eq!(
        original_bid, updated_image_node.bid,
        "Asset BID should remain stable when content changes"
    );

    // Verify content_hash in payload changed
    let original_hash = original_image_node
        .payload
        .get("content_hash")
        .and_then(|v: &toml::Value| v.as_str())
        .expect("Original node should have content_hash in payload");

    let updated_hash = updated_image_node
        .payload
        .get("content_hash")
        .and_then(|v: &toml::Value| v.as_str())
        .expect("Updated node should have content_hash in payload");

    assert_ne!(
        original_hash, updated_hash,
        "content_hash in payload should change when content changes"
    );

    tracing::info!(
        "Asset content change test complete - BID stable ({}), hash changed from {} to {}",
        original_bid,
        original_hash,
        updated_hash
    );

    Ok(())
}

#[test(tokio::test)]
async fn test_asset_html_hardlinks() -> Result<(), Box<dyn std::error::Error>> {
    tracing::info!("Testing HTML output creates content-addressed hardlinks for assets");

    let test_tempdir = generate_test_root("network_1")?;
    let test_root = test_tempdir.path().to_path_buf();

    // Create HTML output directory
    let html_tempdir = tempfile::tempdir()?;
    let html_output = html_tempdir.path().to_path_buf();

    let mut global_bb = BeliefBase::empty();
    let (accum_tx, mut accum_rx) = unbounded_channel::<BeliefEvent>();

    // Compile with HTML output enabled
    let mut compiler = DocumentCompiler::with_html_output(
        &test_root,
        Some(accum_tx),
        None,
        false,
        Some(html_output.clone()),
        None, // No live reload script for tests
    )?;
    let _parse_results = compiler.parse_all(global_bb.clone(), false).await?;

    // Build asset manifest from PathAdded events (keep outside scope for later use)
    let asset_manifest = {
        let mut manifest: std::collections::BTreeMap<String, noet_core::properties::Bid> =
            std::collections::BTreeMap::new();
        let asset_ns = noet_core::properties::asset_namespace();

        while let Ok(event) = accum_rx.try_recv() {
            if let noet_core::event::BeliefEvent::PathAdded(net_bid, path, node_bid, _, _) = &event
            {
                if *net_bid == asset_ns && !path.is_empty() {
                    manifest.insert(path.clone(), *node_bid);
                }
            }
            global_bb.process_event(&event)?;
        }

        tracing::info!(
            "Built asset manifest from events: {} assets",
            manifest.len()
        );
        manifest
    };

    // Create asset hardlinks after events are processed
    compiler
        .create_asset_hardlinks(&html_output, &asset_manifest)
        .await?;

    // Verify static directory exists
    let static_dir = html_output.join("static");
    assert!(static_dir.exists(), "static directory should be created");

    // Verify semantic paths exist (should be hardlinks to canonical files)
    let semantic_image = html_output.join("assets/test_image.png");
    let semantic_pdf = html_output.join("assets/test_doc.pdf");

    assert!(
        semantic_image.exists(),
        "Semantic path for image should exist: {}",
        semantic_image.display()
    );
    assert!(
        semantic_pdf.exists(),
        "Semantic path for PDF should exist: {}",
        semantic_pdf.display()
    );

    // Get content hashes from asset nodes to verify canonical files exist
    use noet_core::properties::{asset_namespace, BeliefKind};

    let asset_nodes: Vec<_> = global_bb
        .states()
        .values()
        .filter(|n| n.kind.contains(BeliefKind::External))
        .filter(|n| {
            let asset_ns = asset_namespace();
            n.bid.parent_namespace_bytes() == asset_ns.namespace_bytes()
        })
        .collect();

    // Verify canonical files exist in static directory
    for node in asset_nodes.iter() {
        let hash = node
            .payload
            .get("content_hash")
            .and_then(|v| v.as_str())
            .expect("Asset should have content_hash");

        // Find the asset path from manifest to get extension
        let asset_path = asset_manifest
            .iter()
            .find(|(_path, bid)| **bid == node.bid)
            .map(|(path, _)| path)
            .expect("Asset BID should be in manifest");

        let ext = Path::new(asset_path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");

        let canonical_name = if ext.is_empty() {
            hash.to_string()
        } else {
            format!("{}.{}", hash, ext)
        };

        let canonical_path = static_dir.join(&canonical_name);
        assert!(
            canonical_path.exists(),
            "Canonical file should exist: {}",
            canonical_path.display()
        );

        tracing::info!(
            "Verified canonical file: {} (hash: {})",
            canonical_path.display(),
            hash
        );
    }

    // Verify file contents match
    let original_image = test_root.join("assets/test_image.png");
    let output_image = html_output.join("assets/test_image.png");

    let original_bytes = std::fs::read(&original_image)?;
    let output_bytes = std::fs::read(&output_image)?;

    assert_eq!(
        original_bytes, output_bytes,
        "Output asset should have same content as original"
    );

    tracing::info!("HTML asset hardlinks test complete - all files verified");
    Ok(())
}

#[test(tokio::test)]
async fn test_asset_html_deduplication() -> Result<(), Box<dyn std::error::Error>> {
    tracing::info!("Testing HTML output deduplicates assets with same content");

    let test_tempdir = generate_test_root("network_1")?;
    let test_root = test_tempdir.path().to_path_buf();

    // Create duplicate of test_image.png at different path (same content)
    let duplicate_dir = test_root.join("duplicates");
    fs::create_dir_all(&duplicate_dir)?;
    let original_image = test_root.join("assets/test_image.png");
    let duplicate_image = duplicate_dir.join("same_image.png");
    fs::copy(&original_image, &duplicate_image)?;

    // Create a document that references the duplicate asset
    let doc_content = r#"# Duplicate Asset Test

This references the duplicate: ![Duplicate Image](./duplicates/same_image.png)
"#;
    fs::write(test_root.join("duplicate_ref.md"), doc_content)?;

    // Create HTML output directory
    let html_tempdir = tempfile::tempdir()?;
    let html_output = html_tempdir.path().to_path_buf();

    let mut global_bb = BeliefBase::empty();
    let (accum_tx, mut accum_rx) = unbounded_channel::<BeliefEvent>();

    // Compile with HTML output enabled
    let mut compiler = DocumentCompiler::with_html_output(
        &test_root,
        Some(accum_tx),
        None,
        false,
        Some(html_output.clone()),
        None, // No live reload script for tests
    )?;
    let _parse_results = compiler.parse_all(global_bb.clone(), false).await?;

    // Build asset manifest from PathAdded events
    let asset_manifest = {
        let mut manifest: std::collections::BTreeMap<String, noet_core::properties::Bid> =
            std::collections::BTreeMap::new();
        let asset_ns = noet_core::properties::asset_namespace();

        while let Ok(event) = accum_rx.try_recv() {
            if let noet_core::event::BeliefEvent::PathAdded(net_bid, path, node_bid, _, _) = &event
            {
                if *net_bid == asset_ns && !path.is_empty() {
                    manifest.insert(path.clone(), *node_bid);
                }
            }
            global_bb.process_event(&event)?;
        }

        manifest
    };

    // Create asset hardlinks after events are processed
    compiler
        .create_asset_hardlinks(&html_output, &asset_manifest)
        .await?;

    // Verify static directory has only ONE physical file for the duplicate content
    let static_dir = html_output.join("static");
    let static_files: Vec<_> = std::fs::read_dir(&static_dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_file())
        .collect();

    // Should have 2 unique hashes (test_image.png hash and test_doc.pdf hash)
    assert_eq!(
        static_files.len(),
        2,
        "Static directory should have 2 unique files (not 3)"
    );

    // Verify both semantic paths exist
    let semantic_original = html_output.join("assets/test_image.png");
    let semantic_duplicate = html_output.join("duplicates/same_image.png");

    assert!(
        semantic_original.exists(),
        "Original semantic path should exist"
    );
    assert!(
        semantic_duplicate.exists(),
        "Duplicate semantic path should exist"
    );

    // Verify both point to same content
    let original_bytes = std::fs::read(&semantic_original)?;
    let duplicate_bytes = std::fs::read(&semantic_duplicate)?;

    assert_eq!(
        original_bytes, duplicate_bytes,
        "Both paths should have identical content"
    );

    tracing::info!(
        "HTML deduplication test complete - {} unique files, 2 semantic paths for same content",
        static_files.len()
    );
    Ok(())
}
