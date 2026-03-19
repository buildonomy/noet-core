//! BID generation and caching tests
//!
//! ## Test matrix
//!
//! Two axes:
//!
//! **Axis 1 — parse driver**
//! - `sequential`: `parse_sequential`, which drives parse phases in a naked loop with no
//!   `BatchStart`/`BatchEnd`/`drain_epoch` machinery.  `BeliefBase` / `DbConnection` are
//!   passed directly (no accumulator).  Child documents are discovered organically via
//!   `UnresolvedReference` diagnostics, exactly as before the epoch/accumulator layer was
//!   introduced.  If these fail, the bug is in the core parse pipeline, not epochs.
//! - `parallel`: `parse_all` with default jobs, `BeliefBase` / `DbConnection` wrapped in
//!   `BeliefAccumulator` + `QueryHandle`.  Mirrors `Commands::Parse` in `main.rs`.  If
//!   sequential passes but parallel fails, the regression is in the epoch/accumulator
//!   machinery.
//!
//! **Axis 2 — global cache type**
//! - `in_memory`: in-memory `BeliefBase`.  No DB.  Parse 1 passes `rx` into
//!   `parse_sequential` so events are applied to `global_bb` incrementally.  Parse 2
//!   uses a `CountingBeliefBase` wrapper so graph-modifying events are counted and any
//!   non-zero count fails the test.
//! - `db`: `DbConnection` backed by SQLite.  Parse 1 writes rewrites AND commits events
//!   to the DB.  Parse 2 is cold-started from the DB (fresh `global_bb` = `DbConnection`).
//!
//! ## Success criteria (identical for all four tests)
//!
//! - **Parse 1**: any `rewritten_content` is written to disk (and to DB when applicable).
//! - **Parse 2**: zero `rewritten_content`, zero `dependent_paths`, node count ≤ parse-1
//!   node count.
//!
//! ## How to read failures
//!
//! ```text
//! sequential_in_memory fails  → bug in parse_sequential / parse_next with plain BeliefBase
//! sequential_db        fails  → bug in parse_sequential with DbConnection, or DB commit path
//! parallel_in_memory   fails  → regression in accumulator/epoch machinery (in-memory)
//! parallel_db          fails  → regression in accumulator/epoch machinery (DB path)
//! ```

use noet_core::{
    beliefbase::{BeliefAccumulator, BeliefBase, BeliefGraph, BeliefSink},
    codec::{
        network::{detect_network_file, NETWORK_NAME},
        DocumentCompiler, CODECS,
    },
    db::{db_init, DbConnection, Transaction},
    error::BuildonomyError,
    event::BeliefEvent,
    properties::{Bid, WeightSet},
    query::{BeliefSource, Expression, Query},
};
use sqlx::Row;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use test_log::test;
use tokio::sync::mpsc::unbounded_channel;

use super::common::{extract_bids_from_content, generate_test_root};

// ============================================================================
// CountingBeliefBase — BeliefSink wrapper that counts graph-modifying events
// ============================================================================
//
// Used as `global_bb` for parse-2 runs in the in-memory tests.  Every call to
// `apply_batch` increments `graph_event_count` for each event that is not a
// pure metadata event (`FileParsed`, `BatchStart`, `BatchEnd`), then delegates
// to the inner `BeliefBase`.  After `parse_sequential` / `parse_all` returns,
// the test asserts that `graph_event_count() == 0`.

#[derive(Clone)]
struct CountingBeliefBase {
    inner: BeliefBase,
    graph_event_count: Arc<Mutex<usize>>,
}

impl CountingBeliefBase {
    fn new(inner: BeliefBase) -> Self {
        Self {
            inner,
            graph_event_count: Arc::new(Mutex::new(0)),
        }
    }

    fn graph_event_count(&self) -> usize {
        *self.graph_event_count.lock().unwrap()
    }

    fn states_len(&self) -> usize {
        self.inner.states().len()
    }
}

impl BeliefSink for CountingBeliefBase {
    async fn apply_batch(&mut self, events: &[BeliefEvent]) -> Result<(), BuildonomyError> {
        for event in events {
            match event {
                BeliefEvent::FileParsed(_)
                | BeliefEvent::BatchStart
                | BeliefEvent::BatchEnd
                | BeliefEvent::BuiltInTest => {}
                other => {
                    tracing::warn!("[parse 2] Unexpected graph-modifying event: {:?}", other);
                    *self.graph_event_count.lock().unwrap() += 1;
                }
            }
        }
        self.inner.apply_batch(events).await
    }
}

impl BeliefSource for CountingBeliefBase {
    fn eval_unbalanced(
        &self,
        expr: &Expression,
    ) -> impl std::future::Future<Output = Result<BeliefGraph, BuildonomyError>> + Send {
        self.inner.eval_unbalanced(expr)
    }

    fn eval_trace(
        &self,
        expr: &Expression,
        weight_filter: WeightSet,
    ) -> impl std::future::Future<Output = Result<BeliefGraph, BuildonomyError>> + Send {
        self.inner.eval_trace(expr, weight_filter)
    }

    fn get_all_paths(
        &self,
        network_bid: Bid,
        include_index: bool,
    ) -> impl std::future::Future<Output = Result<Vec<(String, Bid)>, BuildonomyError>> + Send {
        self.inner.get_all_paths(network_bid, include_index)
    }

    fn get_file_mtimes(
        &self,
    ) -> impl std::future::Future<Output = Result<BTreeMap<PathBuf, i64>, BuildonomyError>> + Send
    {
        self.inner.get_file_mtimes()
    }

    fn export_beliefgraph(
        &self,
    ) -> impl std::future::Future<Output = Result<BeliefGraph, BuildonomyError>> + Send {
        self.inner.export_beliefgraph()
    }

    fn eval_query(
        &self,
        query: &Query,
        all_or_none: bool,
    ) -> impl std::future::Future<Output = Result<BeliefGraph, BuildonomyError>> + Send {
        self.inner.eval_query(query, all_or_none)
    }
}

// ============================================================================
// Shared helpers
// ============================================================================

/// Write a rewritten `ParseResult` to disk and return the extracted BIDs.
///
/// Handles both file paths and directory paths (network index detection).
async fn apply_rewrite(
    path: &Path,
    content: &str,
) -> Result<Vec<noet_core::properties::Bid>, Box<dyn std::error::Error>> {
    let extracted = extract_bids_from_content(content).unwrap_or_default();
    let mut write_path = path.to_path_buf();
    if write_path.is_dir() {
        if let Some(detected) = detect_network_file(&write_path) {
            write_path = detected;
        } else {
            write_path.push(NETWORK_NAME);
        }
    }
    fs::write(&write_path, content)?;
    Ok(extracted)
}

/// Collect (non-asset) BIDs from a `BeliefBase` for comparison with written BIDs.
fn cached_non_asset_bids(global_bb: &BeliefBase) -> BTreeSet<noet_core::properties::Bid> {
    let mut asset_bids: BTreeSet<_> = global_bb
        .paths()
        .asset_map()
        .map()
        .iter()
        .map(|(_, bid, _)| *bid)
        .collect();
    asset_bids.extend(
        global_bb
            .paths()
            .href_map()
            .map()
            .iter()
            .map(|(_, bid, _)| *bid),
    );
    global_bb
        .states()
        .values()
        .map(|n| n.bid)
        .filter(|b| !asset_bids.contains(b))
        .collect()
}

// ============================================================================
// TEST 1 — Sequential / in-memory
// ============================================================================
//
// parse_sequential + BeliefBase directly, no accumulator, no epoch machinery.
// The naked iterator baseline.  Any failure here is in parse_next itself,
// not in the epoch/accumulator layer.

#[test(tokio::test)]
async fn test_sequential_in_memory() -> Result<(), Box<dyn std::error::Error>> {
    tracing::info!("=== SEQUENTIAL / IN-MEMORY ===");
    let (_test_tempdir, test_root) = generate_test_root("network_1")?;
    tracing::info!(
        "Test dir: {:?}  contents: {}",
        test_root,
        fs::read_dir(&test_root)
            .unwrap()
            .map(|f| format!("{f:?}"))
            .collect::<Vec<String>>()
            .join(", ")
    );
    tracing::info!("Codec extensions: {:?}", CODECS.extensions());

    // ── Parse 1 ──────────────────────────────────────────────────────────────
    // parse_sequential drives parse_next in a naked loop — no BatchStart/BatchEnd,
    // no drain_epoch, no ProtoIndex work-queue.  The caller drains events from the
    // channel into global_bb after the call returns.
    let (tx, mut rx) = unbounded_channel::<BeliefEvent>();
    let mut global_bb = BeliefBase::empty();
    let mut compiler = DocumentCompiler::new(&test_root, Some(tx), None, false)?;

    let mut written_bids = BTreeSet::default();
    written_bids.insert(compiler.builder().api().bid);

    tracing::info!("[Sequential/Memory] Parse 1");
    let parse_results = compiler
        .parse_sequential(&mut global_bb, false, Some(&mut rx))
        .await?;

    let mut writes = BTreeMap::<String, usize>::default();
    for result in &parse_results {
        if let Some(ref content) = result.rewritten_content {
            let bids = apply_rewrite(&result.path, content).await?;
            tracing::debug!("REWRITTEN: {:?}  bids={:?}", result.path, bids);
            *writes.entry(format!("{:?}", result.path)).or_default() += 1;
            for bid in bids {
                written_bids.insert(bid);
            }
        }
    }

    tracing::debug!(
        "[Sequential/Memory] After parse 1: {} nodes in global_bb, {} in session_bb",
        global_bb.states().len(),
        compiler.builder().session_bb().states().len(),
    );
    tracing::debug!(
        "File writes:\n - {}",
        writes
            .iter()
            .map(|(k, v)| format!("{k}: {v}"))
            .collect::<Vec<String>>()
            .join("\n - ")
    );

    // BID consistency check.
    let cached_bids = cached_non_asset_bids(&global_bb);
    for extra in cached_bids.difference(&written_bids) {
        if let Some(node) = global_bb.states().get(extra) {
            tracing::warn!(
                "EXTRA cached (not written): bid={extra} title={:?} id={:?} kind={:?}",
                node.title,
                node.id,
                node.kind
            );
        }
    }
    for missing in written_bids.difference(&cached_bids) {
        tracing::warn!(
            "WRITTEN but not cached: bid={missing} in_states={} in_paths_nets={}",
            global_bb.states().contains_key(missing),
            global_bb.paths().nets().contains(missing)
        );
    }
    debug_assert!(
        written_bids.eq(&cached_bids),
        "Written BIDs != cached BIDs\nWritten: {written_bids:?}\nCached:  {cached_bids:?}"
    );

    // ── Parse 2 ──────────────────────────────────────────────────────────────
    let pre_second_parse_count = global_bb.states().len();
    tracing::info!(
        "[Sequential/Memory] Parse 2: expecting no rewrites. Pre-count: {pre_second_parse_count}"
    );

    tracing::debug!(
        "[Sequential/Memory] global_bb before parse 2:\npaths and relations:\n{}\npathmaps:\n{}",
        global_bb.clone().consume(),
        global_bb.paths()
    );

    let (tx2, mut rx2) = unbounded_channel::<BeliefEvent>();
    let mut compiler2 = DocumentCompiler::new(&test_root, Some(tx2), None, false)?;

    // Wrap global_bb in a counting sink so any graph-modifying events on parse 2
    // are caught and counted rather than silently absorbed.
    let mut counting_bb = CountingBeliefBase::new(global_bb);

    let parse_results2 = compiler2
        .parse_sequential(&mut counting_bb, false, Some(&mut rx2))
        .await?;

    let second_event_count = counting_bb.graph_event_count();
    assert_eq!(
        second_event_count, 0,
        "[Sequential/Memory] Parse 2 must not generate graph-modifying events, got {second_event_count}"
    );

    for result in &parse_results2 {
        if result.rewritten_content.is_some() {
            tracing::warn!(
                "[Sequential/Memory] UNEXPECTED REWRITE on parse 2: {:?}",
                result.path
            );
        }
        debug_assert!(
            result.rewritten_content.is_none(),
            "[Sequential/Memory] Parse 2 must not rewrite {:?}",
            result.path
        );
        if !result.dependent_paths.is_empty() {
            tracing::warn!(
                "[Sequential/Memory] Parse 2: {:?} has dependent_paths: {:?}",
                result.path,
                result.dependent_paths
            );
        }
        assert!(
            result.dependent_paths.is_empty(),
            "[Sequential/Memory] Parse 2 must not produce dependent_paths for {:?}",
            result.path
        );
    }

    let post_second_parse_count = counting_bb.states_len();
    debug_assert!(
        post_second_parse_count <= pre_second_parse_count,
        "[Sequential/Memory] Parse 2 introduced new nodes: \
         pre={pre_second_parse_count} post={post_second_parse_count}"
    );

    Ok(())
}

// ============================================================================
// TEST 2 — Sequential / DB
// ============================================================================
//
// parse_sequential + DbConnection directly, no accumulator, no epoch machinery.
// Parse 1 commits events to the DB.  Parse 2 cold-starts from the DB.
// If test 1 passes but this fails, the issue is in DbConnection's BeliefSource
// impl or the DB commit path, not in the epoch/accumulator layer.

#[test(tokio::test)]
async fn test_sequential_db() -> Result<(), Box<dyn std::error::Error>> {
    tracing::info!("=== SEQUENTIAL / DB ===");
    let (_test_tempdir, test_root) = generate_test_root("network_1")?;

    let db_path = test_root.join("belief_cache.db");
    let db_pool = db_init(db_path).await?;
    let db = DbConnection(db_pool);

    // ── Parse 1 ──────────────────────────────────────────────────────────────
    // parse_sequential: naked parse_next loop, no epoch machinery.
    // Events are drained from the channel and committed to the DB after the call.
    tracing::info!("[Sequential/DB] Parse 1: populate DB");
    let (tx, mut rx) = unbounded_channel::<BeliefEvent>();
    let mut compiler = DocumentCompiler::new(&test_root, Some(tx), None, true)?;

    let parse_results = compiler
        .parse_sequential(&mut db.clone(), false, None)
        .await?;
    tracing::info!(
        "[Sequential/DB] Parse 1 completed: {} documents",
        parse_results.len()
    );

    // Write rewrites to disk.
    for result in &parse_results {
        if let Some(ref content) = result.rewritten_content {
            let bids = apply_rewrite(&result.path, content).await?;
            tracing::debug!("REWRITTEN: {:?}  bids={:?}", result.path, bids);
        }
    }

    // Commit all events to DB.
    let mut transaction = Transaction::default();
    let mut event_count = 0usize;
    while let Ok(event) = rx.try_recv() {
        transaction.add_event(&event).ok();
        event_count += 1;
    }
    tracing::info!("[Sequential/DB] Committing {event_count} events to DB");
    transaction.execute(&db.0).await?;

    // Verify DB was populated.
    let node_count: i64 = sqlx::query("SELECT COUNT(*) as count FROM beliefs")
        .fetch_one(&db.0)
        .await?
        .get("count");
    let edge_count: i64 = sqlx::query("SELECT COUNT(*) as count FROM relations")
        .fetch_one(&db.0)
        .await?
        .get("count");
    let path_count: i64 = sqlx::query("SELECT COUNT(*) as count FROM paths")
        .fetch_one(&db.0)
        .await?
        .get("count");
    tracing::info!(
        "[Sequential/DB] DB after commit: {node_count} nodes, {edge_count} edges, {path_count} paths"
    );

    // ── Parse 2 ──────────────────────────────────────────────────────────────
    tracing::info!("[Sequential/DB] Parse 2: cold-start from DB, expecting no rewrites");
    let (tx2, mut rx2) = unbounded_channel::<BeliefEvent>();
    let mut compiler2 = DocumentCompiler::new(&test_root, Some(tx2), None, false)?;

    let parse_results2 = compiler2
        .parse_sequential(&mut db.clone(), false, None)
        .await?;

    for result in &parse_results2 {
        if result.rewritten_content.is_some() {
            tracing::warn!(
                "[Sequential/DB] UNEXPECTED REWRITE on parse 2: {:?}\n{}",
                result.path,
                result.rewritten_content.as_deref().unwrap_or("")
            );
        }
        assert!(
            result.rewritten_content.is_none(),
            "[Sequential/DB] Parse 2 must not rewrite {:?}",
            result.path
        );
        assert!(
            result.dependent_paths.is_empty(),
            "[Sequential/DB] Parse 2 must not produce dependent_paths for {:?}",
            result.path
        );
    }

    // No graph-modifying events on parse 2.
    let mut second_event_count = 0usize;
    while let Ok(event) = rx2.try_recv() {
        if !matches!(
            event,
            BeliefEvent::FileParsed(_) | BeliefEvent::BatchStart | BeliefEvent::BatchEnd
        ) {
            tracing::warn!("[Sequential/DB] Unexpected event on parse 2: {:?}", event);
            second_event_count += 1;
        }
    }
    assert_eq!(
        second_event_count, 0,
        "[Sequential/DB] Parse 2 must not generate graph-modifying events, got {second_event_count}"
    );

    Ok(())
}

// ============================================================================
// TEST 3 — Parallel / in-memory
// ============================================================================
//
// parse_all(default jobs) + BeliefAccumulator<BeliefBase> + QueryHandle.
// Mirrors Commands::Parse in main.rs.
// If test 1 passes but this fails, the regression is in the accumulator/epoch
// machinery on the in-memory path.

#[test(tokio::test)]
async fn test_parallel_in_memory() -> Result<(), Box<dyn std::error::Error>> {
    tracing::info!("=== PARALLEL / IN-MEMORY ===");
    let (_test_tempdir, test_root) = generate_test_root("network_1")?;
    tracing::info!(
        "Test dir: {:?}  contents: {}",
        test_root,
        fs::read_dir(&test_root)
            .unwrap()
            .map(|f| format!("{f:?}"))
            .collect::<Vec<String>>()
            .join(", ")
    );
    tracing::info!("Codec extensions: {:?}", CODECS.extensions());

    // ── Parse 1 ──────────────────────────────────────────────────────────────
    let (accum_tx, accum_rx) = unbounded_channel::<BeliefEvent>();
    let accum = BeliefAccumulator::new(BeliefBase::empty(), accum_rx);
    let global_handle = accum.query_handle();

    tracing::info!("[Parallel/Memory] Initialize DocumentCompiler");
    let mut compiler = DocumentCompiler::new(&test_root, Some(accum_tx), None, false)?;

    let mut written_bids = BTreeSet::default();
    written_bids.insert(compiler.builder().api().bid);

    tracing::info!("[Parallel/Memory] Parse 1: parse_all");
    let parse_results = compiler.parse_all(global_handle, false).await?;

    let mut writes = BTreeMap::<String, usize>::default();
    for result in &parse_results {
        if let Some(ref content) = result.rewritten_content {
            let bids = apply_rewrite(&result.path, content).await?;
            tracing::debug!("REWRITTEN: {:?}  bids={:?}", result.path, bids);
            *writes.entry(format!("{:?}", result.path)).or_default() += 1;
            for bid in bids {
                written_bids.insert(bid);
            }
        }
    }

    // Recover populated BeliefBase; drains remaining channel events.
    let global_bb = accum.into_inner().await?;

    tracing::debug!(
        "[Parallel/Memory] After parse 1: {} nodes in global_bb, {} in session_bb, {} in doc_bb",
        global_bb.states().len(),
        compiler.builder().session_bb().states().len(),
        compiler.builder().doc_bb().states().len(),
    );
    tracing::debug!(
        "File writes:\n - {}",
        writes
            .iter()
            .map(|(k, v)| format!("{k}: {v}"))
            .collect::<Vec<String>>()
            .join("\n - ")
    );

    // BID consistency check.
    let cached_bids = cached_non_asset_bids(&global_bb);
    for extra in cached_bids.difference(&written_bids) {
        if let Some(node) = global_bb.states().get(extra) {
            tracing::warn!(
                "EXTRA cached (not written): bid={extra} title={:?} id={:?} kind={:?}",
                node.title,
                node.id,
                node.kind
            );
        }
    }
    for missing in written_bids.difference(&cached_bids) {
        tracing::warn!(
            "WRITTEN but not cached: bid={missing} in_states={} in_paths_nets={}",
            global_bb.states().contains_key(missing),
            global_bb.paths().nets().contains(missing)
        );
    }
    debug_assert!(
        written_bids.eq(&cached_bids),
        "Written BIDs != cached BIDs\nWritten: {written_bids:?}\nCached:  {cached_bids:?}"
    );

    // ── Parse 2 ──────────────────────────────────────────────────────────────
    let pre_second_parse_count = global_bb.states().len();
    tracing::info!(
        "[Parallel/Memory] Parse 2: expecting no rewrites. Pre-count: {pre_second_parse_count}"
    );

    let (accum_tx2, accum_rx2) = unbounded_channel::<BeliefEvent>();
    let accum2 = BeliefAccumulator::new(global_bb, accum_rx2);
    let global_handle2 = accum2.query_handle();

    let mut compiler2 = DocumentCompiler::new(&test_root, Some(accum_tx2), None, false)?;

    tracing::info!("[Parallel/Memory] Re-running parse_all for parse 2");
    let final_parse_results = compiler2.parse_all(global_handle2, false).await?;

    for result in &final_parse_results {
        tracing::debug!("[Parallel/Memory] Parse 2 doc {:?}", result.path);
        if result.rewritten_content.is_some() {
            tracing::warn!(
                "[Parallel/Memory] UNEXPECTED REWRITE on parse 2: {:?}",
                result.path
            );
        }
        debug_assert!(
            result.rewritten_content.is_none(),
            "[Parallel/Memory] Parse 2 must not rewrite {:?}",
            result.path
        );
        if !result.dependent_paths.is_empty() {
            tracing::warn!(
                "[Parallel/Memory] Parse 2: {:?} has dependent_paths: {:?}",
                result.path,
                result.dependent_paths
            );
        }
        assert!(
            result.dependent_paths.is_empty(),
            "[Parallel/Memory] Parse 2 must not produce dependent_paths for {:?}",
            result.path
        );
    }

    let global_bb2 = accum2.into_inner().await?;
    let post_second_parse_count = global_bb2.states().len();
    debug_assert!(
        post_second_parse_count <= pre_second_parse_count,
        "[Parallel/Memory] Parse 2 introduced new nodes: \
         pre={pre_second_parse_count} post={post_second_parse_count}"
    );

    Ok(())
}

// ============================================================================
// TEST 4 — Parallel / DB
// ============================================================================
//
// parse_all(default jobs) + DbConnection, no accumulator wrapper on the DB itself.
// Parse 1 commits events to the DB.  Parse 2 cold-starts from the DB.
// Requires DbConnection::resolve_net_path to correctly handle repo-relative
// NetPath queries like "subnet/file.md".
//
// If test 2 passes but this fails, the regression is in the parallel epoch
// machinery on the DB path.

#[test(tokio::test)]
async fn test_parallel_db() -> Result<(), Box<dyn std::error::Error>> {
    tracing::info!("=== PARALLEL / DB ===");
    let (_test_tempdir, test_root) = generate_test_root("network_1")?;

    let db_path = test_root.join("belief_cache.db");
    let db_pool = db_init(db_path).await?;
    let db = DbConnection(db_pool);

    // ── Parse 1 ──────────────────────────────────────────────────────────────
    tracing::info!("[Parallel/DB] Parse 1: populate DB");
    let (accum_tx, mut accum_rx) = unbounded_channel::<BeliefEvent>();
    let mut compiler = DocumentCompiler::new(&test_root, Some(accum_tx), None, true)?;

    let parse_results = compiler.parse_all(db.clone(), false).await?;
    tracing::info!(
        "[Parallel/DB] Parse 1 completed: {} documents",
        parse_results.len()
    );

    // Write rewrites to disk.
    for result in &parse_results {
        if let Some(ref content) = result.rewritten_content {
            let bids = apply_rewrite(&result.path, content).await?;
            tracing::debug!("REWRITTEN: {:?}  bids={:?}", result.path, bids);
        }
    }

    // Commit events to DB.
    let mut transaction = Transaction::default();
    let mut event_count = 0usize;
    while let Ok(event) = accum_rx.try_recv() {
        transaction.add_event(&event).ok();
        event_count += 1;
    }
    tracing::info!("[Parallel/DB] Committing {event_count} events to DB");
    transaction.execute(&db.0).await?;

    // Verify DB was populated.
    let node_count: i64 = sqlx::query("SELECT COUNT(*) as count FROM beliefs")
        .fetch_one(&db.0)
        .await?
        .get("count");
    let edge_count: i64 = sqlx::query("SELECT COUNT(*) as count FROM relations")
        .fetch_one(&db.0)
        .await?
        .get("count");
    let path_count: i64 = sqlx::query("SELECT COUNT(*) as count FROM paths")
        .fetch_one(&db.0)
        .await?
        .get("count");
    tracing::info!(
        "[Parallel/DB] DB after commit: {node_count} nodes, {edge_count} edges, {path_count} paths"
    );

    // ── Parse 2 ──────────────────────────────────────────────────────────────
    tracing::info!("[Parallel/DB] Parse 2: cold-start from DB, expecting no rewrites");
    let (accum_tx2, mut accum_rx2) = unbounded_channel::<BeliefEvent>();
    let mut compiler2 = DocumentCompiler::new(&test_root, Some(accum_tx2), None, false)?;

    let parse_results2 = compiler2.parse_all(db.clone(), false).await?;

    for result in &parse_results2 {
        tracing::debug!("[Parallel/DB] Parse 2 doc {:?}", result.path);
        if result.rewritten_content.is_some() {
            tracing::warn!(
                "[Parallel/DB] UNEXPECTED REWRITE on parse 2: {:?}\n{}",
                result.path,
                result.rewritten_content.as_deref().unwrap_or("")
            );
        }
        assert!(
            result.rewritten_content.is_none(),
            "[Parallel/DB] Parse 2 must not rewrite {:?}",
            result.path
        );
        assert!(
            result.dependent_paths.is_empty(),
            "[Parallel/DB] Parse 2 must not produce dependent_paths for {:?}",
            result.path
        );
    }

    // No graph-modifying events on parse 2.
    let mut second_event_count = 0usize;
    while let Ok(event) = accum_rx2.try_recv() {
        if !matches!(
            event,
            BeliefEvent::FileParsed(_) | BeliefEvent::BatchStart | BeliefEvent::BatchEnd
        ) {
            tracing::warn!("[Parallel/DB] Unexpected event on parse 2: {:?}", event);
            second_event_count += 1;
        }
    }
    assert_eq!(
        second_event_count, 0,
        "[Parallel/DB] Parse 2 must not generate graph-modifying events, got {second_event_count}"
    );

    Ok(())
}
