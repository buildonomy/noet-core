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
    properties::Bid,
    query::{BeliefSource, BoxFuture, SubmapResult},
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

use super::common::{all_xlsx_bids, extract_bids_from_content, generate_test_root};

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
    fn submap<'a>(
        &'a self,
        network_bid: Bid,
        path: &'a str,
        depth: u8,
        include_index: bool,
    ) -> BoxFuture<'a, SubmapResult> {
        self.inner.submap(network_bid, path, depth, include_index)
    }

    fn submap_by_bid<'a>(
        &'a self,
        network_bid: Bid,
        entry: Option<Bid>,
        depth: u8,
        include_index: bool,
    ) -> BoxFuture<'a, SubmapResult> {
        self.inner
            .submap_by_bid(network_bid, entry, depth, include_index)
    }

    fn get_file_mtimes(&self) -> BoxFuture<'_, Result<BTreeMap<PathBuf, i64>, BuildonomyError>> {
        self.inner.get_file_mtimes()
    }

    fn export_beliefgraph(&self) -> BoxFuture<'_, Result<BeliefGraph, BuildonomyError>> {
        self.inner.export_beliefgraph()
    }
}

// ============================================================================
// Shared helpers
// ============================================================================

/// Return a human-readable description of the first difference between two strings.
///
/// Shows the byte offset, a short context window around the divergence in each string,
/// and a summary of any trailing content that is present in one string but not the other.
/// Designed for `tracing::warn!` / `panic!` messages where a full diff is too verbose.
fn first_string_diff(label_a: &str, a: &str, label_b: &str, b: &str) -> String {
    let diverge = a
        .char_indices()
        .zip(b.char_indices())
        .find(|((_, ca), (_, cb))| ca != cb)
        .map(|((ia, _), _)| ia)
        .unwrap_or_else(|| a.len().min(b.len()));

    let window = 120;
    let start = diverge.saturating_sub(window / 2);
    let end_a = (diverge + window / 2).min(a.len());
    let end_b = (diverge + window / 2).min(b.len());

    // Clamp to valid char boundaries.
    let start = a[..start]
        .char_indices()
        .next_back()
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(0);
    let end_a = a[..end_a]
        .char_indices()
        .next_back()
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(end_a);
    let end_b = b[..end_b]
        .char_indices()
        .next_back()
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(end_b);

    let ctx_a = &a[start..end_a];
    let ctx_b = &b[start..end_b];

    let len_note = if a.len() != b.len() {
        format!(
            "\n  length: {label_a}={} bytes, {label_b}={} bytes (delta {})",
            a.len(),
            b.len(),
            a.len() as isize - b.len() as isize,
        )
    } else {
        format!("\n  length: {} bytes (equal)", a.len())
    };

    format!(
        "First diff at byte {diverge}:{len_note}\
         \n  {label_a} ctx: {ctx_a:?}\
         \n  {label_b} ctx: {ctx_b:?}",
    )
}

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
    // Binary codecs (xlsx/ods) write BIDs directly to disk via generate_source_bytes()
    // rather than through rewritten_content. Their nodes are therefore absent from
    // written_bids but present in cached_bids. Exclude them from the assertion;
    // parse-2 zero-graph-event check enforces BID stability for binary codec files.
    let xlsx_bids = all_xlsx_bids(&global_bb);
    let cached_bids: BTreeSet<_> = cached_non_asset_bids(&global_bb)
        .into_iter()
        .filter(|b| !xlsx_bids.contains(b))
        .collect();
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
    // Force parallel epoch dispatch: with jobs=1 (the default) Phase 1 depth-group
    // batches are processed inline one-at-a-time, so each subnet's events drain into
    // global_bb before the next sibling's push() runs — the bug is never triggered.
    // With jobs>1 the subnets in a depth-group batch are spawned as concurrent tasks,
    // each querying the same pre-epoch global_bb snapshot.  A subnet node whose
    // speculative_path_key generates NodeKey::Id { net: Bref::default() } will miss
    // in cache_fetch (no sibling task's output is visible yet), get a fresh time-based
    // BID, and be registered in PathMap under a bref-based path — corrupting any
    // cross-network link that resolves via PathMap during Phase 4 inject_context.
    //
    // Query actual parallelism: on a single-core machine the parallel epoch path is
    // never exercised (jobs is clamped to 1), so we set jobs to min(4, available) and
    // skip the parallel-specific assertion below if the result is still 1.
    let available_jobs = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    let jobs = available_jobs.min(4);
    compiler.set_jobs(jobs);
    tracing::info!(
        "[Parallel/Memory] Running with jobs={jobs} (available_parallelism={available_jobs})"
    );

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
    // Binary codecs (xlsx/ods) write BIDs directly to disk via generate_source_bytes()
    // rather than through rewritten_content. Exclude their BIDs from the assertion;
    // parse-2 zero-graph-event check enforces BID stability for binary codec files.
    let xlsx_bids = all_xlsx_bids(&global_bb);
    let cached_bids: BTreeSet<_> = cached_non_asset_bids(&global_bb)
        .into_iter()
        .filter(|b| !xlsx_bids.contains(b))
        .collect();
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

    // ── Parse 1 network BID stability check ──────────────────────────────────
    //
    // After parse 1, every network node in global_bb must have an id that is NOT
    // equal to its own bref. A bref-as-id means the node lost the ID collision check
    // in push(): speculative_path_key generated NodeKey::Id { net: Bref::default() }
    // (which regularizes to repo_bref), the initial cache_fetch missed because
    // global_bb didn't yet contain the node under that key, the Unresolved branch
    // assigned a fresh time-based BID, and then the ID collision check found the real
    // node under net=parent_bref and fired first-one-wins — clearing the incoming
    // node's id to its own bref.
    //
    // This assertion is key-type agnostic and needs no fixture links: it reads
    // directly from global_bb.states() and checks structural invariants.
    // On single-core machines (jobs==1) subnets are parsed serially so the race
    // never occurs and this check trivially passes — correct behaviour.
    {
        use noet_core::properties::BeliefKind;
        let bref_id_networks: Vec<_> = global_bb
            .states()
            .values()
            .filter(|node| {
                node.kind.is_network()
                    && !node.kind.0.contains(BeliefKind::API)
                    && !node.kind.0.contains(BeliefKind::External)
            })
            .filter(|node| {
                let id = node.id.anchor();
                !id.is_empty() && id == node.bid.bref().to_string()
            })
            .collect();
        for node in &bref_id_networks {
            tracing::warn!(
                "[Parallel/Memory] Parse 1: network node has bref as id — \
                 bid={} id={:?} title={:?}. This means first-one-wins fired \
                 incorrectly: the node lost its declared id due to a \
                 cache_fetch miss in speculative_path_key (net-scope bug).",
                node.bid,
                node.id,
                node.title,
            );
        }
        debug_assert!(
            bref_id_networks.is_empty(),
            "[Parallel/Memory] Parse 1: {} network node(s) have bref as id after parse 1.\n\
             A bref id on a network node means the node's declared id was clobbered by \
             first-one-wins in push() — the initial cache_fetch in speculative_path_key \
             used NodeKey::Id {{ net: Bref::default() }} (→ repo_bref after regularize), \
             missed in global_bb, assigned a fresh time-based BID via the Unresolved \
             branch, and then the ID collision check found the real node under \
             net=parent_bref and cleared the incoming node's id to its own bref.\n\
             Affected nodes:\n{}",
            bref_id_networks.len(),
            bref_id_networks
                .iter()
                .map(|n| format!("  bid={} id={:?} title={:?}", n.bid, n.id, n.title))
                .collect::<Vec<_>>()
                .join("\n"),
        );
    }

    // ── Parse 1 PathMap bref-as-path check ───────────────────────────────────
    //
    // After parse 1, every subnet network node must be registered in its parent's
    // PathMap under a human-readable directory name, not a 12-hex-char bref string.
    //
    // The parallel epoch race in speculative_path_key produces a fresh time-based
    // BID for the subnet network node (because NodeKey::Id { net: Bref::default() }
    // always misses in global_bb). That fresh BID's bref then becomes the path key
    // stored in the parent's PathMap — a 12-hex-char string like "e2e433e57b61"
    // instead of "subnet1". On parse 2, try_initialize_stack_from_session_cache
    // reconstructs the stack from this corrupted PathMap entry, propagating the
    // wrong net bref to every child node's build_path_key call.
    //
    // Detection: a path component that is exactly 12 lowercase hex chars is a bref
    // string standing in for a real directory name.
    if jobs > 1 {
        let bref_re = {
            // A 12-character lowercase hex string — the Display form of a Bref.
            // We check each path component individually so we catch "subnet/e2e433e57b61"
            // as well as bare "e2e433e57b61".
            |s: &str| s.len() == 12 && s.chars().all(|c| c.is_ascii_hexdigit())
        };

        let paths_guard = global_bb.paths();
        let mut bref_path_entries: Vec<String> = Vec::new();

        for (net_bref, path_map_lock) in paths_guard.map().iter() {
            let path_map = path_map_lock.read();
            for (path, bid, _sort_key) in path_map.map().iter() {
                // Check every slash-delimited component of the path.
                if path.split('/').any(|component| {
                    // Strip a leading "index.md#" prefix for in-network-index anchors.
                    let bare = component.strip_prefix("index.md#").unwrap_or(component);
                    bref_re(bare)
                }) {
                    let maybe_node = global_bb.states().get(bid);
                    bref_path_entries.push(format!(
                        "  net_bref={net_bref} path={path:?} bid={bid} \
                         id={:?} title={:?}",
                        maybe_node.map(|n| &n.id),
                        maybe_node.map(|n| n.title.as_str()),
                    ));
                    tracing::warn!(
                        "[Parallel/Memory] Parse 1 PathMap: bref-as-path entry — \
                         net_bref={net_bref} path={path:?} bid={bid} \
                         id={:?} title={:?}",
                        maybe_node.map(|n| &n.id),
                        maybe_node.map(|n| n.title.as_str()),
                    );
                }
            }
        }

        // Log inventory of all network nodes and their PathMap path keys for
        // cross-referencing against bref-as-path entries above.
        for node in global_bb.states().values().filter(|n| {
            use noet_core::properties::BeliefKind;
            n.kind.is_network()
                && !n.kind.0.contains(BeliefKind::API)
                && !n.kind.0.contains(BeliefKind::External)
        }) {
            tracing::debug!(
                "[Parallel/Memory] Parse 1 network node: bid={} bref={} id={:?} title={:?}",
                node.bid,
                node.bid.bref(),
                node.id,
                node.title.as_str(),
            );
        }

        debug_assert!(
            bref_path_entries.is_empty(),
            "[Parallel/Memory] Parse 1: {} PathMap entry/entries use a bref as a path \
             component instead of a human-readable directory name.\n\
             This indicates speculative_path_key generated NodeKey::Id {{ net: Bref::default() }} \
             for a subnet network node, missed in global_bb, assigned a fresh time-based BID, \
             and registered that BID's bref as the subnet's path in the parent PathMap.\n\
             On parse 2, try_initialize_stack_from_session_cache reconstructs the stack from \
             this entry, propagating the wrong net bref to all child node keys — causing \
             cache_fetch misses for every section and document in the subnet.\n\
             Affected entries:\n{}",
            bref_path_entries.len(),
            bref_path_entries.join("\n"),
        );
    }

    // ── Parse 2 ──────────────────────────────────────────────────────────────
    let pre_second_parse_count = global_bb.states().len();
    tracing::info!(
        "[Parallel/Memory] Parse 2: expecting no rewrites. Pre-count: {pre_second_parse_count}"
    );

    let (accum_tx2, accum_rx2) = unbounded_channel::<BeliefEvent>();
    let accum2 = BeliefAccumulator::new(global_bb, accum_rx2);
    let global_handle2 = accum2.query_handle();

    let mut compiler2 = DocumentCompiler::new(&test_root, Some(accum_tx2), None, false)?;
    compiler2.set_jobs(jobs);

    tracing::info!("[Parallel/Memory] Re-running parse_all for parse 2");
    let final_parse_results = compiler2.parse_all(global_handle2, false).await?;

    for result in &final_parse_results {
        tracing::debug!("[Parallel/Memory] Parse 2 doc {:?}", result.path);
        if let Some(ref new_content) = result.rewritten_content {
            // Resolve the on-disk path so we can read what the file actually contains
            // and produce a char-level diff against what generate_source() emitted.
            let disk_path = if result.path.is_dir() {
                detect_network_file(&result.path).unwrap_or_else(|| result.path.join(NETWORK_NAME))
            } else {
                result.path.clone()
            };
            let disk_content = std::fs::read_to_string(&disk_path)
                .unwrap_or_else(|e| format!("<read error: {e}>"));
            let diff = first_string_diff("disk", &disk_content, "generated", new_content);
            tracing::warn!(
                "[Parallel/Memory] UNEXPECTED REWRITE on parse 2: {:?}\n{}",
                result.path,
                diff,
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

    // ── Parse 2 PathMap bref-as-path check ───────────────────────────────────
    //
    // A bref-as-path entry that survives parse 2 means the stack reconstruction
    // in try_initialize_stack_from_session_cache is propagating the wrong net bref
    // from the corrupted parse-1 PathMap entry. Every child node of the affected
    // subnet will have generated keys with that wrong bref, missed in cache_fetch,
    // and received a fresh time-based BID — triggering the rewrite assertions above.
    if jobs > 1 {
        let bref_re = |s: &str| s.len() == 12 && s.chars().all(|c| c.is_ascii_hexdigit());
        let paths_guard2 = global_bb2.paths();
        let mut bref_path_entries2: Vec<String> = Vec::new();

        for (net_bref, path_map_lock) in paths_guard2.map().iter() {
            let path_map = path_map_lock.read();
            for (path, bid, _sort_key) in path_map.map().iter() {
                if path.split('/').any(|component| {
                    let bare = component.strip_prefix("index.md#").unwrap_or(component);
                    bref_re(bare)
                }) {
                    let maybe_node = global_bb2.states().get(bid);
                    bref_path_entries2.push(format!(
                        "  net_bref={net_bref} path={path:?} bid={bid} \
                         id={:?} title={:?}",
                        maybe_node.map(|n| &n.id),
                        maybe_node.map(|n| n.title.as_str()),
                    ));
                    tracing::warn!(
                        "[Parallel/Memory] Parse 2 PathMap: bref-as-path entry persists — \
                         net_bref={net_bref} path={path:?} bid={bid} \
                         id={:?} title={:?}",
                        maybe_node.map(|n| &n.id),
                        maybe_node.map(|n| n.title.as_str()),
                    );
                }
            }
        }

        debug_assert!(
            bref_path_entries2.is_empty(),
            "[Parallel/Memory] Parse 2: {} PathMap entry/entries still use a bref as a \
             path component. The stack reconstruction on re-parse is propagating the wrong \
             net bref from a corrupted parse-1 PathMap entry.\n\
             Affected entries:\n{}",
            bref_path_entries2.len(),
            bref_path_entries2.join("\n"),
        );
    }

    Ok(())
}

// ============================================================================
// TEST 4 — Parallel / DB
// ============================================================================
//
// parse_all(jobs>1) + BeliefAccumulator<DbConnection> + QueryHandle.
// Mirrors Commands::Parse in main.rs on the DB path.
// Parse 1 writes rewrites to disk and commits events to the DB via the
// accumulator's drain_epoch().  Parse 2 cold-starts from the same DB.
//
// The accumulator wrapper is essential: without it, drain_epoch() is a no-op
// on DbConnection and each parallel epoch's tasks query an empty DB.  With it,
// each BatchEnd flushes the epoch's events into the DB so subsequent epochs see
// the committed state — exactly as in production.
//
// If test 3 (parallel/memory) passes but this fails, the regression is in
// how the DB-backed BeliefSource handles a particular NodeKey variant.  The
// known case: TapeFn::Keys with NodeKey::Id passes the raw bref string to
// SQL without the Bref::default() → API-bref normalization that in-memory
// PathMapMap performs.

#[test(tokio::test)]
async fn test_parallel_db() -> Result<(), Box<dyn std::error::Error>> {
    tracing::info!("=== PARALLEL / DB ===");
    let (_test_tempdir, test_root) = generate_test_root("network_1")?;

    let db_path = test_root.join("belief_cache.db");
    let db_pool = db_init(db_path).await?;
    let db = DbConnection(db_pool);

    // Use the same parallelism as test_parallel_in_memory so the parallel epoch
    // path is exercised.  On a single-core machine jobs stays 1 (sequential) and
    // the test trivially passes — that is correct behaviour.
    let available_jobs = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    let jobs = available_jobs.min(4);
    tracing::info!(
        "[Parallel/DB] Running with jobs={jobs} (available_parallelism={available_jobs})"
    );

    // ── Parse 1 ──────────────────────────────────────────────────────────────
    //
    // Wrap DbConnection in BeliefAccumulator so drain_epoch() commits each
    // epoch's events to the DB.  Without the accumulator, DbConnection::drain_epoch
    // is a no-op and each parallel epoch's tasks query an empty DB — causing
    // cache_fetch misses for every node on every re-parse.
    tracing::info!("[Parallel/DB] Parse 1: populate DB via accumulator");
    let (accum_tx, accum_rx) = unbounded_channel::<BeliefEvent>();
    let accum = BeliefAccumulator::new(db.clone(), accum_rx);
    let global_handle = accum.query_handle();

    let mut compiler = DocumentCompiler::new(&test_root, Some(accum_tx), None, true)?;
    compiler.set_jobs(jobs);

    let parse_results = compiler.parse_all(global_handle, false).await?;
    tracing::info!(
        "[Parallel/DB] Parse 1 completed: {} documents",
        parse_results.len()
    );

    // Write rewrites to disk.
    for result in &parse_results {
        if let Some(ref content) = result.rewritten_content {
            let bids = apply_rewrite(&result.path, content).await?;
            tracing::debug!(
                "[Parallel/DB] REWRITTEN: {:?}  bids={:?}",
                result.path,
                bids
            );
        }
    }

    // Drain accumulator — flushes any remaining events into the DB and returns
    // the DbConnection for direct verification queries.
    let db_after_parse1 = accum.into_inner().await?;

    // Verify DB was populated.
    let node_count: i64 = sqlx::query("SELECT COUNT(*) as count FROM beliefs")
        .fetch_one(&db_after_parse1.0)
        .await?
        .get("count");
    let edge_count: i64 = sqlx::query("SELECT COUNT(*) as count FROM relations")
        .fetch_one(&db_after_parse1.0)
        .await?
        .get("count");
    let path_count: i64 = sqlx::query("SELECT COUNT(*) as count FROM paths")
        .fetch_one(&db_after_parse1.0)
        .await?
        .get("count");
    tracing::info!(
        "[Parallel/DB] DB after parse 1: {node_count} nodes, {edge_count} edges, {path_count} paths"
    );
    assert!(
        node_count > 0,
        "[Parallel/DB] DB must be populated after parse 1"
    );

    // ── Parse 2 ──────────────────────────────────────────────────────────────
    //
    // Cold-start from the DB: fresh accumulator wrapping the same DbConnection.
    // Every cache_fetch miss that fires on parse 2 represents a node that was
    // either not committed to the DB by parse 1, or whose key changed between
    // parses (indicating a BID stability or speculative_path_key bug).
    tracing::info!("[Parallel/DB] Parse 2: cold-start from DB, expecting no rewrites");
    let (accum_tx2, accum_rx2) = unbounded_channel::<BeliefEvent>();
    let accum2 = BeliefAccumulator::new(db_after_parse1, accum_rx2);
    let global_handle2 = accum2.query_handle();

    let mut compiler2 = DocumentCompiler::new(&test_root, Some(accum_tx2), None, false)?;
    compiler2.set_jobs(jobs);

    let parse_results2 = compiler2.parse_all(global_handle2, false).await?;

    for result in &parse_results2 {
        tracing::debug!("[Parallel/DB] Parse 2 doc {:?}", result.path);
        if let Some(ref new_content) = result.rewritten_content {
            let disk_path = if result.path.is_dir() {
                detect_network_file(&result.path).unwrap_or_else(|| result.path.join(NETWORK_NAME))
            } else {
                result.path.clone()
            };
            let disk_content = std::fs::read_to_string(&disk_path)
                .unwrap_or_else(|e| format!("<read error: {e}>"));
            let diff = first_string_diff("disk", &disk_content, "generated", new_content);
            tracing::warn!(
                "[Parallel/DB] UNEXPECTED REWRITE on parse 2: {:?}\n{}",
                result.path,
                diff,
            );
        }
        assert!(
            result.rewritten_content.is_none(),
            "[Parallel/DB] Parse 2 must not rewrite {:?}",
            result.path
        );
        if !result.dependent_paths.is_empty() {
            tracing::warn!(
                "[Parallel/DB] Parse 2: {:?} has dependent_paths: {:?}",
                result.path,
                result.dependent_paths
            );
        }
        assert!(
            result.dependent_paths.is_empty(),
            "[Parallel/DB] Parse 2 must not produce dependent_paths for {:?}",
            result.path
        );
    }

    let db_after_parse2 = accum2.into_inner().await?;

    // Verify parse 2 did not introduce new nodes.
    let node_count2: i64 = sqlx::query("SELECT COUNT(*) as count FROM beliefs")
        .fetch_one(&db_after_parse2.0)
        .await?
        .get("count");
    debug_assert!(
        node_count2 <= node_count,
        "[Parallel/DB] Parse 2 introduced new nodes: pre={node_count} post={node_count2}"
    );

    Ok(())
}
