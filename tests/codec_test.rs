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
    beliefset::BeliefSet,
    codec::{lattice_toml::NETWORK_CONFIG_NAME, BeliefSetParser, CODECS},
    error::BuildonomyError,
    event::BeliefEvent,
    properties::Bid,
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
async fn test_belief_set_accumulator_bid_generation_and_caching(
) -> Result<(), Box<dyn std::error::Error>> {
    tracing::info!("Initialize global_cache (BeliefSet) and other necessary dependencies.");
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
    let mut global_cache = BeliefSet::empty();
    let (accum_tx, mut accum_rx) = unbounded_channel::<BeliefEvent>();
    tracing::info!(
        "Initialized BeliefSet codec extension types: {:?}",
        CODECS.extensions()
    );

    tracing::info!("Initialize BeliefSetParser");
    let mut parser = BeliefSetParser::new(&test_root, Some(accum_tx), None, false)?;

    let mut docs_to_reparse = BTreeSet::default();
    let mut written_bids = BTreeSet::default();
    written_bids.insert(parser.accumulator().api().bid);

    tracing::info!("Run parser.parse_all()");
    let parse_results = parser.parse_all(global_cache.clone()).await?;

    let mut writes = BTreeMap::<String, usize>::default();
    for parse_result in parse_results {
        let doc_entry = writes
            .entry(format!("{:?}", parse_result.path))
            .or_default();

        if let Some(rewrite_content) = parse_result.rewritten_content {
            let mut write_path = parse_result.path.clone();
            if write_path.is_dir() {
                write_path.push(NETWORK_CONFIG_NAME);
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
            global_cache.process_event(&event)?;
            // tracing::debug!("global cache event: {:?}", event);
        }
    }
    tracing::debug!(
        "Global cache nodes: {}, accum.stack_cache nodes: {}, accum.set nodes: {}",
        global_cache.states().len(),
        parser.accumulator().stack_cache().states().len(),
        parser.accumulator().set().states().len()
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
    let cached_bids = BTreeSet::from_iter(global_cache.states().values().map(|n| n.bid));
    debug_assert!(
        written_bids.eq(&cached_bids),
        "Written: {written_bids:?}\n\nCached: {cached_bids:?}"
    );

    // 8. Initialize a NEW BeliefSetParser using the SAME global_cache and repository state
    tracing::info!(
        "Initialize a NEW BeliefSetParser for the second parsing run, reusing global_cache."
    );
    let (accum_tx, mut accum_rx) = unbounded_channel::<BeliefEvent>();
    parser = BeliefSetParser::new(&test_root, Some(accum_tx), None, false)?;
    written_bids = BTreeSet::default();
    written_bids.insert(parser.accumulator().api().bid);

    tracing::info!("Re-running parser.parse_all()");
    let final_parse_results = parser.parse_all(global_cache.clone()).await?;

    for parse_result in final_parse_results {
        tracing::debug!("Parsing doc {:?}", parse_result.path);
        debug_assert!(parse_result.rewritten_content.is_none());
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
