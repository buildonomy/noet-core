//! BID generation and caching tests

use noet_core::{
    beliefbase::BeliefBase,
    codec::{
        network::{detect_network_file, NETWORK_NAME},
        DocumentCompiler, CODECS,
    },
    db::{db_init, DbConnection, Transaction},
    event::BeliefEvent,
};
use sqlx::Row;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
};
use test_log::test;
use tokio::sync::mpsc::unbounded_channel;

use super::common::{extract_bids_from_content, generate_test_root};

#[test(tokio::test)]
async fn test_belief_set_builder_bid_generation_and_caching(
) -> Result<(), Box<dyn std::error::Error>> {
    tracing::info!("Initialize global_bb (BeliefBase) and other necessary dependencies.");
    let (_test_tempdir, test_root) = generate_test_root("network_1")?;
    tracing::info!(
        "Test dir is {:?}. Test dir contents: {}",
        test_root,
        fs::read_dir(&test_root)
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
                if let Some(detected_path) = detect_network_file(&write_path) {
                    write_path = detected_path;
                } else {
                    // Default to first in NETWORK_CONFIG_NAMES (JSON)
                    write_path.push(NETWORK_NAME);
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

    // get asset and href bid set to remove from the cached_bids comparison to written bids
    let mut asset_bids = BTreeSet::from_iter(
        global_bb
            .paths()
            .asset_map()
            .map()
            .iter()
            .map(|(_path, bid, _order)| *bid),
    );
    asset_bids.append(&mut BTreeSet::from_iter(
        global_bb
            .paths()
            .href_map()
            .map()
            .iter()
            .map(|(_path, bid, _order)| *bid),
    ));

    tracing::info!("Ensure written bids match cached bids");
    let cached_bids = BTreeSet::from_iter(
        global_bb
            .states()
            .values()
            .map(|n| n.bid)
            .filter(|bid| !asset_bids.contains(bid)),
    );
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
    let (_test_tempdir, test_root) = generate_test_root("network_1")?;

    // Initialize DB in test directory
    let db_path = test_root.join("belief_cache.db");
    let db_pool = db_init(db_path).await?;
    let db = DbConnection(db_pool);

    tracing::info!("First parse with DbConnection as global cache");
    let (accum_tx, mut accum_rx) = unbounded_channel::<BeliefEvent>();
    let mut compiler = DocumentCompiler::new(&test_root, Some(accum_tx), None, true)?;

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
                "Document {:?} has rewritten content on second parse - indicates BID instability:\n{}",
                parse_result.path,
                parse_result.rewritten_content.clone().unwrap()
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
