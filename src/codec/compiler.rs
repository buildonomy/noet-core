use crate::{
    beliefbase::{BeliefBase, BeliefSink, EpochDrain},
    codec::{
        assets::{get_stylesheet_urls, get_template, Layout},
        belief_ir::IRNode,
        builder::{GraphBuilder, ParseContentWithCodec},
        network::{detect_network_file, NetworkCodec, NETWORK_NAME},
        proto_index::ProtoIndex,
        DocCodec, ParseDiagnostic, UnresolvedReference, CODECS,
    },
    error::BuildonomyError,
    event::BeliefEvent,
    nodekey::NodeKey,
    paths::{os_path_to_string, string_to_os_path, AnchorPath, AnchorPathBuf},
    properties::{asset_namespace, Bid, Bref},
    query::{BeliefSource, Expression, NeighborsExpression, Query},
};

use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
    time::SystemTime,
};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use toml_edit::value;
use tracing::Instrument;

/// A wrapper around GraphBuilder that manages recursive document parsing with queue
/// management and loop prevention.
///
/// ## Overview
///
/// This compiler acts as a "filesystem orchestrator" that discovers files, reads content, and
/// feeds it to the builder for parsing. It automatically handles the complex dependency
/// resolution workflow where documents reference each other and need multiple parse passes.
///
/// ## Single-Queue Architecture
///
/// The compiler maintains one `remainder_queue` for all pending paths. Items with a low
/// `processed` count (never-parsed) are treated as initial-pass files; items with a higher
/// count are re-parses. Within the remainder loop, items are sorted by ascending `processed`
/// count so never-parsed assets and late-discovered files are dispatched before re-parses.
///
/// ## Loop Prevention
///
/// Each file tracks parse count in `processed`. If a file is parsed more than
/// `max_reparse_count` times (default: 2), a `ReparseLimitExceeded` sentinel is emitted.
///
/// ## Architecture: Cache Separation
///
/// The global cache is intentionally NOT stored in this struct to maintain the architectural
/// separation between the compiler (which reads from the cache) and the transaction handler
/// (which writes to the cache via BeliefEvents). The cache must be passed to each parse method.
///
/// This design ensures:
/// - Compiler thread: reads from cache, generates events
/// - Transaction thread: receives events, writes to cache
/// - No contention between reader and writer
pub struct DocumentCompiler {
    write: bool,
    /// Number of parallel jobs for epoch dispatch. 1 = sequential (default).
    /// Set via `--jobs N` CLI flag or `NOET_JOBS` environment variable.
    jobs: usize,
    /// Optional output directory for HTML generation
    html_output_dir: Option<PathBuf>,
    /// Optional JavaScript to inject into generated HTML (e.g., live reload script)
    html_script: Option<String>,
    /// Use CDN for Open Props (requires internet, smaller output)
    use_cdn: bool,
    /// Base URL for sitemap and canonical URLs (e.g., <https://username.github.io/repo>)
    base_url: Option<String>,
    builder: GraphBuilder,
    /// Pre-built filesystem index of network directories and their ordered children.
    ///
    /// Built once at compiler construction via a single WalkDir pass from `repo_root`.
    /// Passed as a cheap `Arc`-clone to each `parse_content` call so that both the fast
    /// and slow paths of `initialize_stack` share a single canonical source of truth for
    /// `sort_key_for(abs_path)` — eliminating the BN-DB dual-source sort-key instability.
    ///
    /// For Issue 57 parallel tasks, each task receives `proto_index.clone()` (zero-copy
    /// `Arc` handle) so all tasks share one pre-built read-only map with no WalkDir calls
    /// during parsing.
    proto_index: ProtoIndex,
    /// Single unified queue for all pending parse paths.
    ///
    /// Items are sorted by ascending `processed` count before each remainder-loop
    /// iteration: count=0 items (never parsed, including assets) run first; count≥1
    /// items are re-parses.  The `parse_sequential` initial pass drains this queue
    /// after seeding it with stale files, then the remainder loop handles late
    /// discoveries and re-parses. The `parse_all` batching path drains this queue into
    /// run count derived batches.
    remainder_queue: VecDeque<PathBuf>,
    processed: HashMap<PathBuf, usize>, // Track parse count per path
    max_reparse_count: usize,           // Prevent infinite loops
    /// Last parse result per path. Written by `process_one_parse_result` on every parse
    /// attempt (last write wins on reparse). Callers collect from this at the end of a
    /// parse run instead of maintaining their own local map. `ReparseLimitExceeded`
    /// sentinels are stored here and stripped by `promote_unresolved_to_warnings`.
    latest_results: HashMap<PathBuf, ParseResult>,
    /// Network files that need HTML generation deferred until all documents are parsed
    deferred_html: std::collections::HashSet<PathBuf>,
}

/// Result of parsing a single document
#[derive(Debug, Clone)]
pub struct ParseResult {
    pub path: PathBuf,
    pub rewritten_content: Option<String>,
    pub dependent_paths: Vec<(String, Bref)>,
    pub diagnostics: Vec<crate::codec::ParseDiagnostic>,
}

impl DocumentCompiler {
    /// Create a new compiler with an entry point (file or directory)
    ///
    /// # Arguments
    /// * `entry_point` - The file or directory to start parsing from
    /// * `tx` - Optional channel sender for BeliefEvents (if None, events are not transmitted)
    /// * `max_reparse_count` - Maximum times a file can be reparsed (default: 2)
    /// * `write` - write back ids to files or read only mode
    pub fn new(
        entry_point: impl AsRef<Path>,
        tx: Option<tokio::sync::mpsc::UnboundedSender<BeliefEvent>>,
        max_reparse_count: Option<usize>,
        write: bool,
    ) -> Result<Self, BuildonomyError> {
        Self::with_html_output(
            entry_point,
            tx,
            max_reparse_count,
            write,
            None,
            None,
            false,
            None,
            None,
        )
    }

    /// Create a new compiler with HTML output enabled
    #[allow(clippy::too_many_arguments)]
    pub fn with_html_output(
        entry_point: impl AsRef<Path>,
        tx: Option<tokio::sync::mpsc::UnboundedSender<BeliefEvent>>,
        max_reparse_count: Option<usize>,
        write: bool,
        html_output_dir: Option<PathBuf>,
        html_script: Option<String>,
        use_cdn: bool,
        base_url: Option<String>,
        jobs: Option<usize>,
    ) -> Result<Self, BuildonomyError> {
        // Copy static assets (CSS, JS, templates) to HTML output directory if configured
        if let Some(ref html_dir) = html_output_dir {
            Self::copy_static_assets(html_dir, use_cdn)?;
        }
        let entry_path = Self::normalize_queue_path(entry_point.as_ref().canonicalize()?);

        let builder = GraphBuilder::new(&entry_path, tx)?;

        // Build the ProtoIndex with a single WalkDir pass from repo_root.
        // Falls back to an empty index on error (e.g. entry_path is not yet a full repo)
        // so construction never fails due to a missing network file at startup.
        let proto_index = ProtoIndex::build(builder.repo_root()).unwrap_or_else(|e| {
            tracing::warn!(
                "[DocumentCompiler] ProtoIndex::build failed for {:?}: {e} — using empty index",
                builder.repo_root()
            );
            ProtoIndex::new()
        });

        // Resolve jobs: explicit arg > NOET_JOBS env var > 1 (sequential default).
        // Parallel dispatch is opt-in: users must pass --jobs N or set NOET_JOBS=N.
        // Default is 1 (sequential) until the parallel path is production-validated.
        let resolved_jobs = jobs
            .or_else(|| {
                std::env::var("NOET_JOBS")
                    .ok()
                    .and_then(|v| v.parse::<usize>().ok())
                    .filter(|&n| n > 0)
            })
            .unwrap_or(1);

        Ok(Self {
            write,
            jobs: resolved_jobs,
            html_output_dir,
            html_script,
            use_cdn,
            base_url,
            builder,
            proto_index,
            remainder_queue: VecDeque::new(),
            processed: HashMap::new(),
            max_reparse_count: max_reparse_count.unwrap_or(2),
            latest_results: HashMap::new(),
            deferred_html: std::collections::HashSet::new(),
        })
    }

    /// Get the HTML output directory if configured
    pub fn html_output_dir(&self) -> Option<&Path> {
        self.html_output_dir.as_deref()
    }

    /// Get the number of parallel jobs configured for epoch dispatch.
    /// 1 = sequential (existing behaviour). N > 1 = parallel.
    pub fn jobs(&self) -> usize {
        self.jobs
    }

    /// Set the number of parallel jobs. Used by CLI after construction.
    pub fn set_jobs(&mut self, jobs: usize) {
        self.jobs = jobs.max(1);
    }

    /// Create a new compiler with an entry point (file or directory) and default arguments: no
    /// receiver of BeliefEvents, default reparse count, and write=false.
    ///
    /// # Arguments
    /// * `entry_point` - The file or directory to start parsing from
    pub fn simple(entry_point: impl AsRef<Path>) -> Result<Self, BuildonomyError> {
        let entry_path = Self::normalize_queue_path(entry_point.as_ref().canonicalize()?);

        let builder = GraphBuilder::new(&entry_path, None)?;
        let proto_index = ProtoIndex::build(builder.repo_root()).unwrap_or_else(|e| {
            tracing::warn!(
                "[DocumentCompiler::simple] ProtoIndex::build failed for {:?}: {e} — using empty index",
                builder.repo_root()
            );
            ProtoIndex::new()
        });

        let jobs = std::env::var("NOET_JOBS")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|&n| n > 0)
            .unwrap_or(1);

        Ok(Self {
            write: false,
            jobs,
            html_output_dir: None,
            html_script: None,
            use_cdn: false,
            base_url: None,
            builder,
            proto_index,
            remainder_queue: VecDeque::new(),
            processed: HashMap::new(),
            max_reparse_count: 2,
            latest_results: HashMap::new(),
            deferred_html: std::collections::HashSet::new(),
        })
    }

    /// Initialize a directory as a BeliefNetwork by placing an index.md file with the
    /// input arguments at that location.
    pub async fn create_network_file<P>(
        repo_path: P,
        id: &str,
        maybe_title: Option<String>,
        maybe_summary: Option<String>,
        insert_children_marker: bool,
    ) -> Result<PathBuf, BuildonomyError>
    where
        P: AsRef<std::path::Path> + std::fmt::Debug,
    {
        let net_codec = NetworkCodec::default();
        if net_codec.proto(repo_path.as_ref())?.is_some() {
            return Err(BuildonomyError::Codec(format!(
                "Network file at path {repo_path:?} is already initialized."
            )));
        }

        let mut proto = IRNode::default();

        proto.document.insert("id", value(id));
        if let Some(title) = maybe_title {
            proto.document.insert("title", value(title));
        }
        if let Some(summary) = maybe_summary {
            proto.document.insert("text", value(summary));
        }

        let mut file_path = repo_path.as_ref().to_path_buf();
        if !file_path.is_dir() {
            file_path.pop();
        }
        file_path.push(NETWORK_NAME);
        let mut file = fs::File::create(&file_path)?;
        let mut body = format!("---{}\n---\n", proto.document);
        if insert_children_marker {
            body.push_str(&format!(
                "\n{}\n",
                crate::codec::network::NETWORK_CHILDREN_MARKER
            ));
        }
        file.write_all(body.as_bytes())?;
        Ok(file_path)
    }

    /// Parse the next item in the remainder queue, returning None if queue is empty.
    ///
    /// Used by `parse_sequential` and the watch service. Items are taken from the front
    /// of `remainder_queue` in FIFO order; callers that want priority ordering should
    /// sort the queue before calling (see `parse_sequential`'s remainder loop).
    ///
    /// # Arguments
    /// * `global_bb` - The belief cache to query during parsing (typically a DbConnection)
    ///
    /// Check for stale files by comparing cached mtimes with filesystem mtimes
    ///
    /// # Arguments
    /// * `cache` - The belief cache to query for cached mtimes
    /// * `force` - If true, treat all files as stale (force re-parse)
    ///
    /// # Returns
    /// * `Ok(Vec<PathBuf>)` - List of files that need to be re-parsed
    pub async fn check_stale_files<B: BeliefSource>(
        &self,
        cache: &B,
        force: bool,
    ) -> Result<Vec<PathBuf>, BuildonomyError> {
        // Query cached mtimes to determine which files to check
        let cached_mtimes = cache.get_file_mtimes().await?;

        tracing::debug!(
            "[Compiler] Checking stale files: found {} cached mtime entries",
            cached_mtimes.len()
        );

        let mut doc_paths = Vec::new();

        // Extract document paths from cached mtimes (these are files we've parsed before)
        for (path, cached_mtime) in cached_mtimes.iter() {
            // Filter to document paths only (no anchors)
            if !path.to_string_lossy().contains('#') {
                tracing::trace!(
                    "[Compiler] Found cached path: {} (mtime: {})",
                    path.display(),
                    cached_mtime
                );
                doc_paths.push(path.clone());
            }
        }

        tracing::debug!(
            "[Compiler] Extracted {} document paths from cache (filtered out anchors)",
            doc_paths.len()
        );

        let mut stale_files = if force {
            tracing::debug!(
                "Force re-parse enabled, will re-parse {} files",
                doc_paths.len()
            );
            doc_paths
        } else {
            let mut stale = Vec::new();
            for path in doc_paths {
                // Check current filesystem mtime
                match fs::metadata(&path) {
                    Ok(metadata) => {
                        let current_mtime = metadata
                            .modified()
                            .map_err(|e| {
                                BuildonomyError::Io(format!("Failed to get mtime: {}", e))
                            })?
                            .duration_since(SystemTime::UNIX_EPOCH)
                            .map_err(|e| BuildonomyError::Io(format!("SystemTimeError: {}", e)))?
                            .as_secs() as i64;

                        if let Some(cached_mtime) = cached_mtimes.get(&path) {
                            if current_mtime > *cached_mtime {
                                tracing::debug!(
                                    "File modified: {} (cached: {}, current: {})",
                                    path.display(),
                                    cached_mtime,
                                    current_mtime
                                );
                                stale.push(path);
                            } else if current_mtime < 0 {
                                // Clock skew: future mtime
                                tracing::warn!("File has future mtime: {}", path.display());
                                stale.push(path); // Safe: re-parse on suspicious mtime
                            }
                        } else {
                            // No cached mtime - file never parsed
                            tracing::debug!("No cached mtime for: {}", path.display());
                            stale.push(path);
                        }
                    }
                    Err(_) => {
                        // File deleted since cache - need to update network
                        tracing::warn!("Cached file no longer exists: {}", path.display());

                        // Parse parent directory to find containing network
                        // Network will re-scan and discover file is gone
                        let mut parent = path.as_path();
                        while let Some(p) = parent.parent() {
                            if detect_network_file(p).is_some() {
                                tracing::debug!(
                                    "Enqueueing parent network for deleted file: {}",
                                    p.display()
                                );
                                stale.push(p.to_path_buf());
                                break;
                            }
                            parent = p;
                        }
                    }
                }
            }
            stale
        };

        stale_files.sort();
        stale_files.dedup();
        Ok(stale_files)
    }

    /// Single authoritative place for post-parse result handling.
    ///
    /// Writes the result into `self.latest_results` (last write per path wins).
    /// On success: increments `self.processed`, generates HTML if configured,
    /// enqueues unresolved dependencies, and re-queues `path` itself if any
    /// unresolved references remain. On error or reparse-limit: always inserts
    /// a `ParseResult` with appropriate diagnostics — never silently drops.
    async fn process_one_parse_result(
        &mut self,
        path: PathBuf,
        task_result: Result<ParseContentWithCodec, BuildonomyError>,
    ) {
        // Always remove from the remainder queue: this path has been attempted and
        // its result is now in latest_results. Re-queuing (for unresolved refs) is
        // done explicitly below.
        self.remove_from_queues(&path);

        let parse_count = self.processed.get(&path).copied().unwrap_or(0);

        if parse_count > self.max_reparse_count {
            tracing::debug!("[Compiler] Max reparse limit reached for {:?}", path);
            self.latest_results
                .entry(path.clone())
                .or_insert(ParseResult {
                    path: path.clone(),
                    rewritten_content: None,
                    dependent_paths: Vec::new(),
                    diagnostics: Vec::new(),
                })
                .diagnostics
                .push(ParseDiagnostic::ReparseLimitExceeded);
            return;
        }

        match task_result {
            Err(e) => {
                tracing::warn!("[Compiler] Parse failed for {:?}: {}", path, e);
                self.latest_results.insert(
                    path.clone(),
                    ParseResult {
                        path,
                        rewritten_content: None,
                        dependent_paths: Vec::new(),
                        diagnostics: vec![ParseDiagnostic::parse_error(
                            format!("Parse failed: {e}"),
                            parse_count,
                        )],
                    },
                );
            }
            Ok(with_codec) => {
                let (mut parse_result, codec) = (with_codec.result, with_codec.codec);

                // HTML generation — only active when an html_output_dir is configured.
                if let Some(html_dir) = &self.html_output_dir.clone() {
                    let file_path = if path.is_dir() {
                        detect_network_file(&path).unwrap_or(path.clone())
                    } else {
                        path.clone()
                    };
                    let (bid, title) = codec
                        .nodes()
                        .first()
                        .map(|proto| {
                            let title = proto
                                .document
                                .get("title")
                                .and_then(|v| v.as_str())
                                .unwrap_or("Untitled")
                                .to_string();
                            let bid = proto
                                .document
                                .get("bid")
                                .and_then(|b_val| {
                                    b_val.as_str().and_then(|b| Bid::try_from(b).ok())
                                })
                                .unwrap_or(Bid::nil());
                            (bid, title)
                        })
                        .unwrap_or((Bid::nil(), "No doc node found".to_string()));

                    match codec.generate_html() {
                        Ok(fragments) => {
                            let repo_relative_path = file_path
                                .strip_prefix(self.builder.repo_root())
                                .unwrap_or(file_path.as_path());
                            let base_dir = repo_relative_path.parent().unwrap_or(Path::new(""));
                            for (filename, html_body) in fragments {
                                let rel_path = base_dir.join(&filename);
                                if let Err(e) = self
                                    .write_fragment(html_dir, &rel_path, html_body, &title, &bid)
                                    .await
                                {
                                    parse_result.diagnostics.push(ParseDiagnostic::warning(
                                        format!(
                                            "Failed to write HTML fragment {}: {e}",
                                            rel_path.display()
                                        ),
                                    ));
                                }
                            }
                        }
                        Err(e) => {
                            parse_result
                                .diagnostics
                                .push(ParseDiagnostic::warning(format!(
                                    "Failed to generate HTML: {e}"
                                )));
                        }
                    }
                    if codec.should_defer() {
                        self.deferred_html.insert(path.clone());
                    }
                }

                // Dependency tracking and self re-queue.
                let unresolved_refs: Vec<&UnresolvedReference> = parse_result
                    .diagnostics
                    .iter()
                    .filter_map(|d| d.as_unresolved_reference())
                    .collect();

                let mut dependent_paths = Vec::<(String, Bref)>::new();

                for unresolved in &unresolved_refs {
                    let is_asset = unresolved.other_keys.iter().any(|key| {
                        if let NodeKey::Path { net, .. } = key {
                            *net == asset_namespace().bref()
                        } else {
                            false
                        }
                    });
                    if is_asset {
                        self.process_asset_reference(&path, unresolved);
                    } else {
                        let Some((dep_str, net)) = unresolved.as_unresolved_source() else {
                            continue;
                        };
                        self.process_unresolved_reference(&path, &dep_str, net);
                        dependent_paths.push((dep_str, net));
                    }
                }

                // Re-queue self if any references are still unresolved. The remainder
                // loop's max_reparse_count gate (checked at the top of this function)
                // prevents infinite cycling.
                if !unresolved_refs.is_empty() && !self.remainder_queue.contains(&path) {
                    self.remainder_queue.push_back(path.clone());
                }

                self.latest_results.insert(
                    path.clone(),
                    ParseResult {
                        path,
                        rewritten_content: parse_result.rewritten_content,
                        dependent_paths,
                        diagnostics: parse_result.diagnostics,
                    },
                );
            }
        }
    }

    /// Process the results from one `parse_epoch` batch.
    ///
    /// Seeds the compiler's repo BID from the first task that discovers it, then
    /// delegates each result to `process_one_parse_result`.
    async fn process_epoch_batch_results(
        &mut self,
        batch_results: Vec<(PathBuf, Result<ParseContentWithCodec, BuildonomyError>)>,
        repo_seeded: &mut bool,
    ) -> Result<(), BuildonomyError> {
        // Seed repo BID once from the first task that discovers it.
        if !*repo_seeded {
            for (_, task_result) in &batch_results {
                if let Ok(ref with_codec) = task_result {
                    if with_codec.repo_bid != Bid::nil() {
                        self.builder.set_repo(with_codec.repo_bid);
                        if let Some(ref repo_node) = with_codec.repo_node {
                            let event = BeliefEvent::NodeUpdate(
                                vec![NodeKey::Bid {
                                    bid: with_codec.repo_bid,
                                }],
                                repo_node.toml(),
                                crate::event::EventOrigin::Remote,
                            );
                            let _ = self.builder.session_bb_mut().process_event(&event);
                        }
                        *repo_seeded = true;
                        break;
                    }
                }
            }
        }

        for (path, task_result) in batch_results {
            self.process_one_parse_result(path, task_result).await;
        }
        Ok(())
    }

    /// Drive the compiler to completion using a two-phase sequential strategy.
    ///
    /// ## Phase 1 — Network dirs, depth-first
    ///
    /// `proto_index.network_dirs()` returns every directory that owns an `index.md`,
    /// sorted by component count (shallowest first), ties broken lexically.  We group
    /// them into *depth batches* — all dirs at the same component depth run together —
    /// and process each batch before moving to the next depth.
    ///
    /// After each depth batch the optional `rx` is drained so the caller's `global_bb`
    /// stays warm.  This has two effects:
    ///   1. Every ancestor network node is committed to `session_bb` before any of its
    ///      leaf documents are parsed, eliminating the redundant slow-path
    ///      `initialize_stack` re-runs that produced duplicate `BeliefEvent`s.
    ///   2. In the MDN case (every directory is a network dir, ~14 000 dirs) the dirs
    ///      are still processed one-by-one — not as a single monolithic batch — so
    ///      `session_bb` grows incrementally and memory pressure stays bounded.
    ///
    /// ## Phase 2 — Leaf documents
    ///
    /// After all network dirs are processed, every non-directory path from
    /// `ordered_paths()` is dispatched in DFS order.  `rx` is drained after each file.
    ///
    /// ## Phase 3 — Remainder loop
    ///
    /// Assets and any files re-queued for unresolved references are processed in
    /// ascending `processed`-count order until the queue is empty.  `rx` is drained
    /// after each file.
    ///
    /// ## `rx` parameter
    ///
    /// Pass `Some(&mut rx)` when the caller owns a `BeliefBase` and wants it updated
    /// incrementally during the run (e.g. tests).  Pass `None` from the watch service,
    /// where the receiver is owned by a separate transaction task.
    ///
    /// **Note**: `global_bb` is cloned into each `parse_one_path` call, so draining
    /// `rx` into the *caller's* `global_bb` does not affect `cache_fetch` lookups
    /// inside the current run.  The drain is purely for the caller's benefit (e.g. so
    /// that parse 2 starts with a fully-populated `global_bb`).  The structural
    /// correctness fix — no duplicate slow-path `initialize_stack` calls — comes
    /// entirely from Phase 1's network-first ordering.
    pub async fn parse_sequential<B: BeliefSource + BeliefSink + Clone + Send + 'static>(
        &mut self,
        global_bb: &mut B,
        force: bool,
        mut rx: Option<&mut tokio::sync::mpsc::UnboundedReceiver<BeliefEvent>>,
    ) -> Result<Vec<ParseResult>, BuildonomyError> {
        // Seed remainder_queue with stale/modified files so they run first.
        let stale_files = self.check_stale_files(&*global_bb, force).await?;
        for path in stale_files {
            if !self.remainder_queue.contains(&path) {
                self.remainder_queue.push_back(path);
            }
        }

        // Helper: run one path. Does NOT increment `processed` — callers must do
        // that at batch-init time (before any file in the batch runs) so that all
        // files in the same batch see a consistent snapshot of the processed counts.
        // Defined as a macro because we need to borrow `self` mutably inside an outer
        // loop that also borrows `rx` mutably.
        macro_rules! run_one {
            ($path:expr) => {{
                let path: PathBuf = $path;
                let (actual_path, result) = Self::parse_one_path(
                    path.clone(),
                    &mut self.builder,
                    global_bb.clone(), // clone the snapshot for this dispatch
                    self.proto_index.clone(),
                    self.write,
                )
                .await;
                self.process_one_parse_result(actual_path, result).await;
            }};
        }

        // Helper: drain the belief-event channel and apply events to global_bb.
        //
        // `parse_one_path` writes events to `self.builder.tx()` (a sender the caller
        // paired with `rx`).  Draining here after each file/depth-group keeps channel
        // memory bounded and keeps the caller's `global_bb` incrementally warm so that
        // parse 2 starts with a fully-populated cache.
        //
        // NOTE: because `parse_one_path` receives `global_bb.clone()` (a snapshot at
        // dispatch time), applying events here does NOT affect in-flight lookups within
        // the current parse run.  The correctness guarantee comes entirely from Phase 1's
        // network-first ordering.  The drain is purely for the caller's benefit.
        macro_rules! drain_rx {
            () => {
                if let Some(ref mut receiver) = rx {
                    let mut batch: Vec<BeliefEvent> = Vec::new();
                    while let Ok(evt) = receiver.try_recv() {
                        batch.push(evt);
                    }
                    if !batch.is_empty() {
                        global_bb.apply_batch(&batch).await?;
                    }
                }
                // Note: global_bb is &mut so apply_batch mutations propagate to
                // the caller — the next parse_one_path clone sees the updated state.
            };
        }

        // ── Phase 1: network dirs, one depth-level at a time ─────────────────
        //
        // Group network_dirs() by component count so that all dirs at depth D are
        // fully committed to session_bb before depth D+1 begins.  Within each group
        // dirs are already in the lexical order returned by network_dirs().
        //
        // INVARIANT: network_dirs() returns dirs sorted shallowest-first (primary key:
        // component count ascending, secondary key: lexicographic).  The grouping loop
        // below relies on all dirs at the same depth being *contiguous* in the slice —
        // it uses a simple last-group-matches check rather than a full scan.  If
        // network_dirs() ever changes its sort order this assert will catch it.
        let net_dirs = self.proto_index.network_dirs();
        debug_assert!(
            net_dirs
                .windows(2)
                .all(|w| w[0].components().count() <= w[1].components().count()),
            "network_dirs sort invariant violated: component counts not non-decreasing; \
             depth-grouping in parse_sequential phase 1 will be incorrect"
        );
        let mut depth_groups: Vec<Vec<PathBuf>> = Vec::new();
        for dir in net_dirs {
            let depth = dir.components().count();
            if depth_groups
                .last()
                .map(|g: &Vec<PathBuf>| g.first().map(|p| p.components().count()).unwrap_or(0))
                .unwrap_or(0)
                == depth
            {
                depth_groups.last_mut().unwrap().push(dir);
            } else {
                depth_groups.push(vec![dir]);
            }
        }

        for group in depth_groups {
            // Increment processed counts for the whole depth group before any file runs,
            // so every file in the batch sees the same pre-batch snapshot of counts.
            let batch: Vec<PathBuf> = group
                .into_iter()
                .filter(|d| !self.processed.contains_key(d))
                .collect();
            for dir in &batch {
                *self.processed.entry(dir.clone()).or_insert(0) += 1;
            }
            for dir in batch {
                run_one!(dir);
                drain_rx!();
            }
        }

        // ── Phase 2: leaf documents in DFS order ─────────────────────────────
        // Collect the whole leaf batch first, increment all counts, then run.
        let leaf_batch: Vec<PathBuf> = self
            .proto_index
            .network_dirs()
            .into_iter()
            .flat_map(|net_dir| {
                self.proto_index
                    .children_of(&net_dir)
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|c| !c.is_dir())
                    .collect::<Vec<_>>()
            })
            .filter(|p| !self.processed.contains_key(p) && !self.remainder_queue.contains(p))
            .collect();
        for path in &leaf_batch {
            *self.processed.entry(path.clone()).or_insert(0) += 1;
        }
        for path in leaf_batch {
            run_one!(path);
            drain_rx!();
        }

        // ── Phase 3: remainder loop (assets + re-parses) ─────────────────────
        let path_order = self.proto_index.ordered_path_index();
        while !self.remainder_queue.is_empty() {
            let mut candidates: Vec<PathBuf> = self.remainder_queue.drain(..).collect();
            // Sort using pre-increment counts so the ordering reflects the state
            // at the start of this batch, not mid-batch mutations.
            candidates.sort_by_key(|p| {
                (
                    self.processed.get(p).copied().unwrap_or(0),
                    path_order.get(p).copied().unwrap_or(usize::MAX),
                )
            });
            // Increment the whole batch before any file runs.
            for path in &candidates {
                *self.processed.entry(path.clone()).or_insert(0) += 1;
            }
            for path in candidates {
                run_one!(path);
                drain_rx!();
            }
        }

        let mut results: Vec<ParseResult> = self.latest_results.drain().map(|(_, v)| v).collect();
        Self::promote_unresolved_to_warnings(&mut results);
        Ok(results)
    }

    /// Parse all items until empty using epoch-structured batching.
    ///
    /// ## Epoch Invariant
    ///
    /// An **epoch** is the set of files sharing the same parse count at the point they are
    /// dequeued. Files in epoch 0 have never been parsed; their nodes do not yet exist in
    /// `global_bb`. Files in epoch N ≥ 1 have been parsed N times; their nodes exist in
    /// `global_bb` from prior epochs.
    ///
    /// **Within a single epoch, no file's parse output is an input to any other file's parse
    /// in that same epoch.** Cross-file dependencies only flow across epoch boundaries.
    /// This invariant makes intra-epoch parallelism safe.
    ///
    /// ## Epoch 0: depth-grouped network-first batching
    ///
    /// Network directories from `network_dirs()` (shallowest-first, ties broken
    /// lexically) are grouped into *depth batches* — all dirs with the same component
    /// count form one epoch batch.  Each depth batch is committed (BatchEnd +
    /// drain_epoch) before the next depth begins, so every ancestor network node is
    /// in `global_bb` before any of its children run.
    ///
    /// After all network-dir depth batches, all leaf documents (non-dir children of
    /// every network) are gathered into a single parallel epoch batch and committed.
    ///
    /// This mirrors the two-phase structure of `parse_sequential` but with
    /// BatchStart/BatchEnd/drain_epoch fences around each batch.  In the MDN case
    /// (~14 000 network dirs at many depths) dirs are still processed one depth level
    /// at a time — not as one monolithic batch — so memory pressure stays bounded.
    ///
    /// **Phase 1 parallelism note**: depth-group batch sizes are bounded by the
    /// number of network directories at each depth, not by `self.jobs`.  A repo
    /// with a single root network produces a phase-1 batch of size 1 regardless of
    /// the jobs setting.  Merging adjacent depth groups to reach `self.jobs` is
    /// unsafe: a depth-D+1 subnet's `initialize_stack` may need the depth-D parent's
    /// events already in `global_bb`, which the epoch boundary guarantees.  If phase-1
    /// parallelism becomes a bottleneck, the fix is to restructure the dependency
    /// model (e.g. pre-seed parent nodes without a full parse), not to merge depth
    /// groups.  Phase 2 (leaf batch) and the remainder loop are already unbounded in
    /// batch size and fully utilize `self.jobs` parallelism.
    ///
    /// ## Remainder loop (epoch ≥ 1)
    ///
    /// After epoch-0, `remainder_queue` contains assets and any files that had unresolved
    /// references.  Items are sorted by ascending `processed` count before each batch:
    /// assets (count=0) run first, then re-parses ordered by count.
    pub async fn parse_all<B: BeliefSource + EpochDrain + Clone + Send + 'static>(
        &mut self,
        global_bb: B,
        force: bool,
    ) -> Result<Vec<ParseResult>, BuildonomyError> {
        // Seed remainder_queue with stale/modified files.
        let stale_files = self.check_stale_files(&global_bb, force).await?;
        if !stale_files.is_empty() {
            tracing::debug!("Found {} stale file(s) to re-parse", stale_files.len());
            for path in stale_files {
                if !self.remainder_queue.contains(&path) {
                    self.remainder_queue.push_back(path);
                }
            }
        }

        let cached_global_bb = global_bb;
        let mut repo_seeded = self.builder.repo() != Bid::nil();

        // ── Epoch 0, Phase 1: network dirs grouped by component depth ────────
        //
        // Build depth groups from network_dirs() (already sorted shallowest-first,
        // ties broken lexically).  Each group becomes one epoch batch so that all
        // ancestors at depth D are committed to global_bb before depth D+1 begins.
        //
        // INVARIANT: same as parse_sequential phase 1 — network_dirs() must return
        // dirs with non-decreasing component counts so that same-depth dirs are
        // contiguous and the last-group-matches grouping is correct.
        let net_dirs = self.proto_index.network_dirs();
        debug_assert!(
            net_dirs
                .windows(2)
                .all(|w| w[0].components().count() <= w[1].components().count()),
            "network_dirs sort invariant violated: component counts not non-decreasing; \
             depth-grouping in parse_all phase 1 will be incorrect"
        );
        let mut depth_groups: Vec<Vec<PathBuf>> = Vec::new();
        for dir in net_dirs {
            let depth = dir.components().count();
            if depth_groups
                .last()
                .and_then(|g: &Vec<PathBuf>| g.first())
                .map(|p| p.components().count())
                .unwrap_or(0)
                == depth
            {
                depth_groups.last_mut().unwrap().push(dir);
            } else {
                depth_groups.push(vec![dir]);
            }
        }

        for group in depth_groups {
            // Filter to only unprocessed dirs, increment counts for the whole
            // batch before any file in it runs.
            let batch: Vec<PathBuf> = group
                .into_iter()
                .filter(|d| !self.processed.contains_key(d))
                .collect();
            if batch.is_empty() {
                continue;
            }
            for dir in &batch {
                *self.processed.entry(dir.clone()).or_insert(0) += 1;
            }
            let _ = self.builder.tx().send(BeliefEvent::BatchStart);
            let results = self.parse_epoch(batch, cached_global_bb.clone()).await?;
            let _ = self.builder.tx().send(BeliefEvent::BatchEnd);
            cached_global_bb.drain_epoch().await?;
            self.process_epoch_batch_results(results, &mut repo_seeded)
                .await?;
        }

        // ── Epoch 0, Phase 2: all leaf documents across all networks ─────────
        //
        // Gather every non-dir child from every network directory.  Skip any path
        // already in processed (stale-seeded) or remainder_queue.  Increment counts
        // for the whole batch before dispatching.
        let leaf_batch: Vec<PathBuf> = self
            .proto_index
            .network_dirs()
            .into_iter()
            .flat_map(|net_dir| {
                self.proto_index
                    .children_of(&net_dir)
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|c| !c.is_dir())
                    .collect::<Vec<_>>()
            })
            .filter(|p| !self.processed.contains_key(p) && !self.remainder_queue.contains(p))
            .collect();

        if !leaf_batch.is_empty() {
            for path in &leaf_batch {
                *self.processed.entry(path.clone()).or_insert(0) += 1;
            }
            // All phase-1 network-dir epochs are committed before this point, so
            // leaf documents share no unresolved cross-dependencies with each other.
            // The entire leaf batch is one epoch: parse_epoch's semaphore bounds
            // concurrency to self.jobs regardless of batch size, so a batch smaller
            // than self.jobs simply runs all files concurrently with no idle workers.
            let _ = self.builder.tx().send(BeliefEvent::BatchStart);
            let leaf_results = self
                .parse_epoch(leaf_batch, cached_global_bb.clone())
                .await?;
            let _ = self.builder.tx().send(BeliefEvent::BatchEnd);
            cached_global_bb.drain_epoch().await?;
            self.process_epoch_batch_results(leaf_results, &mut repo_seeded)
                .await?;
        }

        // Seed remainder_queue with cached assets from session_bb not yet processed.
        // Assets discovered during epoch-0 via process_asset_reference are already
        // in remainder_queue; this catches cached assets whose referencing documents
        // were not re-parsed (mtime hit).
        {
            let assets: Vec<(String, Bid)> = self
                .builder
                .session_bb()
                .get_all_paths(asset_namespace(), false)
                .await
                .unwrap_or_default();
            for (repo_relative_path, _bid) in assets {
                if repo_relative_path.is_empty() {
                    continue;
                }
                let asset_path = Self::normalize_queue_path(
                    self.builder
                        .repo_root()
                        .join(string_to_os_path(&repo_relative_path)),
                );
                if !self.processed.contains_key(&asset_path)
                    && !self.remainder_queue.contains(&asset_path)
                {
                    self.remainder_queue.push_back(asset_path);
                }
            }
        }

        // SPA shell reads session_bb, emits no events — safe outside BatchStart/BatchEnd.
        if self.html_output_dir().is_some() {
            self.generate_spa_shell().await?;
        }

        // ── Remainder loop (epoch ≥ 1) ───────────────────────────────────────
        // Drain and sort by processed count ascending each iteration so assets
        // (count=0) run before re-parses, and shallower re-parses run first.
        // Increment counts for the whole batch before dispatch (consistent
        // pre-batch snapshot invariant, same as parse_sequential phase 3).
        // process_one_parse_result handles the ReparseLimitExceeded sentinel
        // internally, so no separate sentinel_paths splitting is needed here.
        let path_order = self.proto_index.ordered_path_index();
        while !self.remainder_queue.is_empty() {
            let mut candidates: Vec<PathBuf> = self.remainder_queue.drain(..).collect();
            // Sort on pre-increment counts: stable ordering within each processed
            // bucket, tiebroken by DFS position.
            candidates.sort_by_key(|p| {
                (
                    self.processed.get(p).copied().unwrap_or(0),
                    path_order.get(p).copied().unwrap_or(usize::MAX),
                )
            });
            for path in &candidates {
                *self.processed.entry(path.clone()).or_insert(0) += 1;
            }
            let _ = self.builder.tx().send(BeliefEvent::BatchStart);
            let batch_results = self
                .parse_epoch(candidates, cached_global_bb.clone())
                .await?;
            let _ = self.builder.tx().send(BeliefEvent::BatchEnd);
            cached_global_bb.drain_epoch().await?;
            self.process_epoch_batch_results(batch_results, &mut repo_seeded)
                .await?;
        }

        let mut results: Vec<ParseResult> = self.latest_results.drain().map(|(_, v)| v).collect();
        Self::promote_unresolved_to_warnings(&mut results);
        Ok(results)
    }

    /// Dispatch a batch of paths as parse tasks for one epoch.
    ///
    /// **Callers are responsible for incrementing `self.processed` for every path in
    /// the batch before calling this function**, so that all files in the batch see a
    /// consistent pre-batch snapshot of counts (same invariant as `parse_sequential`).
    ///
    /// When `self.jobs == 1` each path is parsed inline in the current async context
    /// using the compiler's own `builder` — no task spawn, no semaphore, no channel
    /// overhead beyond the normal `tx` send. This gives the sequential path the same
    /// BatchStart/BatchEnd envelope as the parallel path without any threading.
    ///
    /// When `self.jobs > 1` each path is spawned as a `tokio::task::spawn` task.
    /// Each task owns: a fresh `GraphBuilder`, a cloned `tx`, a cloned `global_bb`
    /// (`QueryHandle` is `Clone` + `Arc`-backed, cheap to clone), and a cloned
    /// `proto_index` (zero-copy `Arc` handle). No post-task merge step — events flow
    /// directly to `BeliefAccumulator` via `tx`. Concurrency is bounded by `self.jobs`
    /// via an `Arc<Semaphore>`.
    ///
    /// Results are returned in input path order for deterministic output.
    ///
    /// Returns `Vec<(PathBuf, Result<ParseContentWithCodec>)>` in path order.
    /// Resolve `path` to its concrete file path, read its bytes, route to either
    /// `builder.process_asset` (non-codec files) or `parse_content` (codec files),
    /// send a `FileParsed` mtime event on `builder.tx()`, and optionally write
    /// rewritten content back to disk.
    ///
    /// The write happens inside this function (before returning) so that
    /// subsequent epochs always read the BID-injected content from disk.
    /// Ordering is preserved: the write completes before `parse_epoch` returns,
    /// and `drain_epoch` / `BatchEnd` fences the epoch boundary before the next
    /// epoch's reads begin.  In the parallel path each task owns a distinct
    /// `file_path`, so concurrent writes within one epoch are safe.
    ///
    /// Asset routing is unified here: both the sequential and parallel branches of
    /// `parse_epoch`, as well as `parse_sequential`, call this single entry point so
    /// there are no scattered `CODECS.path_get` early-out checks elsewhere.
    async fn parse_one_path<B: BeliefSource + Clone + Send + 'static>(
        path: PathBuf,
        builder: &mut GraphBuilder,
        global_bb: B,
        proto_index: ProtoIndex,
        write: bool,
    ) -> (PathBuf, Result<ParseContentWithCodec, BuildonomyError>) {
        // Resolve directory → index file, or reject directories with no index.
        let file_path = if path.is_dir() {
            match detect_network_file(&path) {
                Some(p) => p,
                None => {
                    return (
                        path.clone(),
                        Err(BuildonomyError::Codec(format!(
                            "Directory has no index file: {}",
                            path.display()
                        ))),
                    );
                }
            }
        } else {
            path.clone()
        };

        // Read raw bytes — works for both text documents and binary assets.
        tracing::debug!("\n\n");
        tracing::debug!("[parse_one_path]: reading {:?}", file_path);
        let bytes = match tokio::fs::read(&file_path).await {
            Ok(b) => b,
            Err(e) => {
                return (
                    path,
                    Err(BuildonomyError::Codec(format!(
                        "Failed to read {}: {e}",
                        file_path.display()
                    ))),
                );
            }
        };

        // Route: asset (no registered codec) vs. codec document.
        //
        // TODO: code smell — ideally assets would be registered as a codec in CODECS and
        // routed through `parse_content` like everything else, eliminating this branch
        // entirely. `process_asset` exists as a stopgap because `DocCodec::parse` takes
        // `&str` (UTF-8 text) and the trait pipeline has no concept of binary content.
        // The right fix is to make the codec pipeline path-agnostic (byte-oriented read,
        // codec-selected decode), at which point `AssetCodec` becomes a real registered
        // codec and this dispatch disappears.
        if CODECS.path_get(&file_path).is_none() {
            tracing::debug!("[parse_one_path]: asset path {:?}", file_path);
            let result = builder.process_asset(&file_path, &bytes, global_bb).await;
            return (path, result);
        }

        // Codec document: decode bytes to UTF-8 then parse.
        tracing::debug!("[parse_one_path]: codec path {:?}", file_path);
        let content = match String::from_utf8(bytes) {
            Ok(s) => s,
            Err(e) => {
                return (
                    path,
                    Err(BuildonomyError::Codec(format!(
                        "File is not valid UTF-8 {}: {e}",
                        file_path.display()
                    ))),
                );
            }
        };

        let _ = builder
            .tx()
            .send(BeliefEvent::FileParsed(file_path.clone()));

        let mut result = builder
            .parse_content(&file_path, content, global_bb, proto_index)
            .await;

        // Write rewritten content (BID injection, link updates) back to disk so
        // subsequent epoch reads see the stabilised content.  Any write error is
        // reported as a warning diagnostic rather than aborting the build.
        if let Ok(ref mut with_codec) = result {
            if let Some(ref contents) = with_codec.result.rewritten_content {
                if write {
                    if let Err(e) = tokio::fs::write(&file_path, contents).await {
                        with_codec
                            .result
                            .diagnostics
                            .push(crate::codec::ParseDiagnostic::warning(format!(
                                "Failed to write rewritten content: {e}"
                            )));
                    }
                }
            }
        }

        (path, result)
    }

    async fn parse_epoch<B: BeliefSource + Clone + Send + 'static>(
        &mut self,
        paths: Vec<PathBuf>,
        global_bb: B,
    ) -> Result<Vec<(PathBuf, Result<ParseContentWithCodec, BuildonomyError>)>, BuildonomyError>
    {
        if self.jobs <= 1 {
            // ── Sequential (inline) path ─────────────────────────────────────────────
            // Runs in the current async context using self.builder directly.
            // parse_one_path handles both codec documents and asset files uniformly.
            // NOTE: processed counts were already incremented by the caller before
            // this function was invoked — do not increment here.
            let mut results = Vec::with_capacity(paths.len());
            for path in paths {
                let proto_index = self.proto_index.clone();
                let write = self.write;
                results.push(
                    Self::parse_one_path(
                        path,
                        &mut self.builder,
                        global_bb.clone(),
                        proto_index,
                        write,
                    )
                    .await,
                );
            }
            Ok(results)
        } else {
            // ── Parallel (spawned-task) path ─────────────────────────────────────────
            //
            // Each task writes to its own isolated channel rather than the shared `tx`.
            // After all tasks finish, we drain per-task buffers into the shared `tx` in
            // original path order (idx 0, 1, 2, …).  This guarantees that within a single
            // epoch batch the accumulator sees events document-by-document in a
            // deterministic lexical order, which is required for first-one-wins ID
            // collision resolution to be stable across runs.
            //
            // Concretely: if file A (idx=0) and file B (idx=1) both have `## Configuration`
            // with no explicit anchor in the same network, A's events always land in
            // session_bb before B's, so A always wins regardless of which task finishes
            // first at the OS scheduler level.
            let repo_root = self.builder.repo_root().to_path_buf();
            let repo_bid = self.builder.repo();
            // Snapshot all network-kinded nodes + their Section edges from the compiler's
            // session_bb so that each spawned task builder can pre-populate its own
            // session_bb with the full ancestor chain.  This is needed because
            // try_initialize_stack_from_session_cache walks upward through session_bb
            // via Section edges; without the full chain present the halo terminates at
            // the immediate parent, breaking the ancestor relation and causing
            // "Skipping update_relation" warnings for the repo root node.
            //
            // The snapshot is empty for epoch-0 (root not yet parsed); for subsequent
            // epochs it contains every network node parsed so far + their Section edges,
            // plus the const-namespace (href + asset) subgraphs so tasks don't hit
            // global_bb for those on the first file.
            let network_ancestors = Arc::new(self.builder.epoch_session_snapshot());
            let proto_index = self.proto_index.clone();
            let shared_tx = self.builder.tx().clone();
            let write = self.write;
            let semaphore = Arc::new(Semaphore::new(self.jobs));

            let n = paths.len();

            // Return type now includes the per-task event buffer alongside the parse result.
            type EpochTaskResult = (
                usize,
                PathBuf,
                Result<ParseContentWithCodec, BuildonomyError>,
                Vec<BeliefEvent>,
            );
            let mut join_set: JoinSet<EpochTaskResult> = JoinSet::new();

            // NOTE: processed counts were already incremented by the caller before
            // this function was invoked — do not increment here.
            for (idx, path) in paths.into_iter().enumerate() {
                let repo_root = repo_root.clone();
                let proto_index = proto_index.clone();
                let global_bb = global_bb.clone();
                let network_ancestors = Arc::clone(&network_ancestors);
                let sem = Arc::clone(&semaphore);
                let span = tracing::info_span!(
                    "parse_task",
                    task_idx = idx,
                    path = %path.display(),
                );

                join_set.spawn(
                    async move {
                        // Acquire a semaphore permit before doing any work so that at most
                        // `jobs` tasks run concurrently. The permit is released when this
                        // async block returns (i.e. when _permit is dropped).
                        let _permit = sem.acquire_owned().await.expect("semaphore closed");

                        // Per-task isolated channel.  All events from this document's
                        // parse_content call land here; they are forwarded to the shared
                        // channel in idx order after the join set drains, ensuring
                        // deterministic event ordering within the epoch batch.
                        let (task_tx, mut task_rx) =
                            tokio::sync::mpsc::unbounded_channel::<BeliefEvent>();

                        // Construct a fresh builder whose events go to the task-local channel.
                        // Seed repo_bid from the compiler's main builder so that
                        // initialize_stack's fast-path guard (`self.repo != Bid::nil()`) passes
                        // immediately, allowing try_initialize_stack_from_session_cache to query
                        // global_bb for the parent network instead of running the full O(depth)
                        // slow-path ancestor walk on every file.  repo_bid is nil for epoch-0
                        // (root not yet parsed) and stable for all subsequent epochs.
                        let mut builder = match GraphBuilder::new(&repo_root, Some(task_tx.clone()))
                        {
                            Ok(b) => b,
                            Err(e) => return (idx, path, Err(e), Vec::new()),
                        };
                        // Seed repo_bid and merge the full network ancestor chain into
                        // this task's session_bb so that try_initialize_stack_from_session_cache
                        // can walk upward through Section edges all the way to the repo root.
                        // seed_session is a no-op when repo_bid is nil (epoch-0).
                        builder.seed_session(repo_bid, &network_ancestors);

                        // Delegate directory resolution, file read, FileParsed event,
                        // parse_content, and optional write-back to the shared helper.
                        // parse_one_path handles both codec documents and asset files,
                        // so the parallel path gains asset handling for free.
                        let (orig_path, result) = DocumentCompiler::parse_one_path(
                            path,
                            &mut builder,
                            global_bb,
                            proto_index,
                            write,
                        )
                        .await;

                        // Drop the builder (and its task_tx clone) so the channel is fully
                        // closed; then drain all buffered events into a Vec for ordered replay.
                        drop(builder);
                        drop(task_tx);
                        let mut task_events = Vec::new();
                        while let Some(ev) = task_rx.recv().await {
                            task_events.push(ev);
                        }

                        (idx, orig_path, result, task_events)
                    }
                    .instrument(span),
                );
            }

            // Collect results from JoinSet (completion order) into an index-keyed map,
            // then reconstruct in original path order for deterministic output.
            type EpochIndexed = (
                PathBuf,
                Result<ParseContentWithCodec, BuildonomyError>,
                Vec<BeliefEvent>,
            );
            let mut indexed: HashMap<usize, EpochIndexed> = HashMap::with_capacity(n);
            while let Some(join_result) = join_set.join_next().await {
                match join_result {
                    Ok((idx, path, result, task_events)) => {
                        indexed.insert(idx, (path, result, task_events));
                    }
                    Err(e) => {
                        // A task panicked. Propagate as a BuildonomyError.
                        return Err(BuildonomyError::Custom(format!("parse task panicked: {e}")));
                    }
                }
            }

            // Replay per-task events to the shared channel in idx (lexical path) order.
            // This is the point where ordering is enforced: all of task 0's events are
            // forwarded before any of task 1's, etc., regardless of which task finished
            // first.  The surrounding BatchStart/BatchEnd in parse_all brackets the entire
            // epoch, so the accumulator still sees one coherent batch.
            let mut results = Vec::with_capacity(n);
            for i in 0..n {
                if let Some((path, result, task_events)) = indexed.remove(&i) {
                    for ev in task_events {
                        let _ = shared_tx.send(ev);
                    }
                    results.push((path, result));
                }
            }

            Ok(results)
        }
    }

    /// Promote lingering `UnresolvedReference` diagnostics to `Warning`.
    ///
    /// Called by `parse_all` after the parse loop. At that point `results` contains exactly
    /// one entry per path (the latest real parse attempt), so no staleness tracking is needed.
    /// Every surviving `UnresolvedReference` is a permanent author error.
    /// `ReparseLimitExceeded` sentinels are stripped — they are compiler-internal signals
    /// that callers should not see.
    ///
    /// Location and direction information is preserved in the `Warning`'s `location` field
    /// for callers (CLI, LSP) to format as they see fit.
    fn promote_unresolved_to_warnings(results: &mut [ParseResult]) {
        for result in results.iter_mut() {
            let mut promoted = Vec::with_capacity(result.diagnostics.len());
            for diagnostic in result.diagnostics.drain(..) {
                match diagnostic {
                    ParseDiagnostic::UnresolvedReference(ref u) => {
                        let keys_str = u
                            .other_keys
                            .iter()
                            .map(|k| format!("{k:?}"))
                            .collect::<Vec<_>>()
                            .join(", ");
                        promoted.push(ParseDiagnostic::Warning {
                            message: format!("unresolved link — tried [{}]", keys_str),
                            location: u.reference_location,
                        });
                    }
                    ParseDiagnostic::ReparseLimitExceeded => {}
                    other => promoted.push(other),
                }
            }
            result.diagnostics = promoted;
        }
    }

    pub fn cache(&self) -> &BeliefBase {
        self.builder().session_bb()
    }

    /// Normalise an absolute path for queue storage.
    ///
    /// On Windows, `Path::canonicalize()` returns paths with the `\\?\` extended-path
    /// prefix (e.g. `\\?\C:\tmp\foo`).  `GraphBuilder::new` already strips this prefix
    /// from `repo_root` via `os_path_to_string` + `string_to_os_path`.  Every path
    /// stored in the compiler's queues must go through the same normalisation so that
    /// `path.strip_prefix(repo_root)` works consistently at every consumption site.
    ///
    /// On Linux/macOS `os_path_to_string` is a no-op for absolute paths, so this
    /// function is zero-cost on those platforms.
    ///
    /// On Windows, `TempDir` (and some other path sources) may return 8.3 short-name
    /// paths (e.g. `RUNNER~1`) while `GraphBuilder::new` canonicalizes `repo_root` to
    /// long names.  Without canonicalization here, `strip_prefix` in `initialize_stack`
    /// silently fails for every file whose path came from such a source, preventing
    /// ancestor network nodes from being pushed into `doc_bb` and causing a panic in
    /// Phase 4 context injection.  Canonicalize first to resolve short-name aliases,
    /// falling back to the original path if the file does not (yet) exist.
    fn normalize_queue_path(path: PathBuf) -> PathBuf {
        let resolved = path.canonicalize().unwrap_or(path);
        string_to_os_path(&os_path_to_string(&resolved))
    }

    /// Enqueue a path for parsing if not already queued or processed.
    pub fn enqueue(&mut self, path: impl AsRef<Path>) {
        let path = Self::normalize_queue_path(path.as_ref().to_path_buf());
        if !self.remainder_queue.contains(&path) && !self.processed.contains_key(&path) {
            self.remainder_queue.push_back(path);
        }
    }

    /// Enqueue a path at the front of the remainder queue (for prioritized parsing).
    pub fn enqueue_front(&mut self, path: impl AsRef<Path>) {
        let path = Self::normalize_queue_path(path.as_ref().to_path_buf());
        // Remove any existing entry so it appears only at the front.
        self.remainder_queue.retain(|p| p != &path);
        self.remainder_queue.push_front(path);
    }

    /// Handle file modification event (reset parse count and prioritize).
    pub fn on_file_modified(&mut self, path: impl AsRef<Path>) {
        let path = path.as_ref().to_path_buf();
        self.processed.remove(&path);
        self.enqueue_front(path);
    }

    /// Handle file deletion event (clean up all tracking).
    pub fn on_file_deleted(&mut self, path: impl AsRef<Path>) {
        let path = path.as_ref().to_path_buf();
        self.remove_from_queues(&path);
        self.processed.remove(&path);
    }

    /// Clear all processed tracking (for fresh re-parse of entire tree).
    ///
    /// Resets the parse count for all files but keeps the queue state.
    pub fn clear_processed(&mut self) {
        self.processed.clear();
    }

    /// Remove a path from the remainder queue.
    fn remove_from_queues(&mut self, path: &PathBuf) {
        self.remainder_queue.retain(|p| p != path);
    }

    /// Finalize HTML generation tasks that require synchronized BeliefBase
    ///
    /// This method handles HTML finalization tasks that need complete event processing:
    /// - Deferred HTML generation (network indices need complete child relationships)
    /// - Sitemap generation (needs all document paths from global_bb)
    /// - Asset hardlinking (needs asset manifest)
    /// - BeliefGraph export to JSON (needs complete graph)
    ///
    /// Called by finalize() for watch service (has DbConnection).
    /// Can also be called separately by parse command after event synchronization.
    ///
    /// # Parameters
    /// - `global_bb`: Synchronized BeliefBase with all events processed
    pub async fn finalize_html<B: BeliefSource + Clone>(
        &self,
        global_bb: B,
    ) -> Result<Vec<crate::codec::ParseDiagnostic>, BuildonomyError> {
        let html_dir = match &self.html_output_dir {
            Some(dir) => dir.clone(),
            None => return Ok(Vec::new()), // No HTML output configured
        };

        // Generate deferred HTML with synchronized context
        self.generate_deferred_html(global_bb.clone()).await?;

        // Generate sitemap from document paths
        self.generate_sitemap(global_bb.clone()).await?;

        // Query synchronized global_bb for asset manifest
        let asset_manifest: BTreeMap<String, Bid> = global_bb
            .get_all_paths(asset_namespace(), false)
            .await
            .unwrap_or_default()
            .into_iter()
            .collect();

        self.create_asset_hardlinks(&asset_manifest).await?;

        // Export BeliefGraph to JSON for client-side use.
        // Step 1: Obtain graph and pathmap from the synchronized global_bb.
        let graph = global_bb.export_beliefgraph().await?;

        // Collects warnings generated during export (e.g. oversized networks).
        // Returned to the caller so they can surface them alongside parse diagnostics.
        let mut finalize_diagnostics: Vec<crate::codec::ParseDiagnostic> = Vec::new();

        // Reconstruct a temporary BeliefBase so we can access its PathMapMap.
        // BeliefBase::from(BeliefGraph) re-derives paths from the node/relation data,
        // giving us a PathMapMap that reflects the complete synchronized state.
        // We keep `temp_bb` alive for the duration of the export pipeline so the
        // read-guard returned by `paths()` remains valid.
        let temp_bb = crate::beliefbase::BeliefBase::from(graph.clone());

        // Step 2: Build compile-time search indices (always, before sharding decision).
        let search_manifest = {
            let pathmap = temp_bb.paths();
            crate::shard::search::build_search_indices(&graph.states, &pathmap, &html_dir).await
        };

        let search_manifest = match search_manifest {
            Ok((manifest, warnings)) => {
                finalize_diagnostics.extend(warnings);
                manifest
            }
            Err(e) => {
                tracing::warn!("[finalize_html] Search index generation failed: {e}. Continuing without search indices.");
                crate::shard::manifest::SearchManifest::new()
            }
        };

        // Step 3: Export BeliefBase (monolithic or sharded based on size).
        // Obtain a fresh pathmap guard for the export step.
        //
        // NOET_SHARD_THRESHOLD overrides the default 10MB threshold for development
        // testing (e.g. `NOET_SHARD_THRESHOLD=1 noet build` forces sharded output).
        let shard_config = match std::env::var("NOET_SHARD_THRESHOLD")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
        {
            Some(threshold) => {
                tracing::info!(
                    "[finalize_html] NOET_SHARD_THRESHOLD={threshold} — overriding default shard threshold"
                );
                crate::shard::manifest::ShardConfig {
                    shard_threshold: threshold,
                    ..crate::shard::manifest::ShardConfig::default()
                }
            }
            None => crate::shard::manifest::ShardConfig::default(),
        };
        let export_result = {
            let pathmap = temp_bb.paths();
            crate::shard::export::export_beliefbase(
                graph,
                &pathmap,
                &html_dir,
                &shard_config,
                &search_manifest,
            )
            .await
        };
        match export_result {
            Ok(crate::shard::ExportMode::Monolithic { size_mb }) => {
                tracing::debug!(
                    "[finalize_html] Exported monolithic beliefbase.json ({:.2} MB)",
                    size_mb
                );
            }
            Ok(crate::shard::ExportMode::Sharded { manifest }) => {
                tracing::info!(
                    "[finalize_html] Exported {} network shards to beliefbase/",
                    manifest.networks.len()
                );
            }
            Err(e) => {
                // Log and fall back to the legacy exporter so a build failure here
                // doesn't break the rest of the output.
                tracing::warn!(
                    "[finalize_html] Shard export failed ({e}). Falling back to legacy export."
                );
                let graph_fallback = global_bb.export_beliefgraph().await?;
                self.export_beliefbase_json(graph_fallback).await?;
            }
        }

        Ok(finalize_diagnostics)
    }

    fn process_asset_reference(&mut self, _path: &PathBuf, unresolved: &UnresolvedReference) {
        // Extract asset path from NodeKey
        let asset_path_key = unresolved.other_keys.iter().find_map(|key| {
            if let NodeKey::Path { net, path } = key {
                if *net == asset_namespace().bref() {
                    Some(path.as_str())
                } else {
                    None
                }
            } else {
                None
            }
        });

        if let Some(asset_relative_path) = asset_path_key {
            // asset_relative_path is already repo-relative: regularize_unchecked in nodekey.rs
            // already resolved the document-relative reference (e.g. ../assets/img.png) against
            // the owner's network-relative path, producing a repo-relative result
            // (e.g. subnet1/assets/img.png). Joining against the document's absolute path again
            // would double the subnet prefix. Instead, join only against repo_root.
            let repo_root = os_path_to_string(self.builder.repo_root());
            let asset_absolute_path = AnchorPathBuf::from(repo_root.clone())
                .as_anchor_path()
                .join(asset_relative_path);
            let _repo_relative_asset: &str = asset_relative_path;

            let absolute_path = Self::normalize_queue_path(string_to_os_path(&asset_absolute_path));
            // Add to remainder_queue for processing (dedup check avoids double-dispatch).
            if !self.processed.contains_key(&absolute_path)
                && !self.remainder_queue.contains(&absolute_path)
            {
                tracing::debug!(
                    "[Compiler] Queueing asset file for content check: {:?}",
                    asset_absolute_path
                );
                self.remainder_queue.push_back(absolute_path);
            }
        }
    }

    fn process_unresolved_reference(&mut self, path: &Path, net_dep_path_str: &str, net_ref: Bref) {
        // Use session_bb rather than doc_bb here. doc_bb is cleared and rebuilt for each
        // document in initialize_stack, so for plain .md files it only contains that file's
        // local nodes — the root network node (and its pathmap entry) is absent.
        //
        // session_bb is the right source because:
        //   1. process_unresolved_reference is only reachable after parse_content returns Ok,
        //      which means terminate_stack has already synced doc_bb into session_bb.
        //   2. Child documents are enqueued by processing the root network's own unresolved
        //      child-file references, so the root network is always parsed (and present in
        //      session_bb) before any child document's process_unresolved_reference runs.
        //   3. compute_diff only removes nodes reachable from parsed_content via section edges;
        //      the root network is never in a child document's parsed_content, so it is never
        //      evicted from session_bb by a subsequent parse.
        let repo_pathmap = self
            .builder()
            .session_bb()
            .paths()
            .get_map(&self.builder().repo().bref())
            .expect(
                "session_bb must contain the root network pathmap entry: the root network is \
                 always parsed before any child document (it enqueues them), and terminate_stack \
                 syncs doc_bb into session_bb before parse_content returns.",
            );
        let Some(net) = self
            .builder()
            .session_bb()
            .paths()
            .nets()
            .iter()
            .find(|net| net.bref() == net_ref)
            .copied()
        else {
            tracing::warn!(
                "[process_unresolved_reference] session_bb has no net with bref {} \
                 (dep={:?}, from={:?})",
                net_ref,
                net_dep_path_str,
                path,
            );
            return;
        };
        let full_dep_path = if let Some((_home_net, net_path, _order)) =
            repo_pathmap.path(&net, &self.builder().session_bb().paths())
        {
            debug_assert!(_home_net == net);
            // Convert relative path to absolute
            let dep_path = string_to_os_path(
                &AnchorPath::new(&net_path)
                    .join(net_dep_path_str)
                    .into_string(),
            );

            // Guard: if dep_path is absolute (e.g. an MDN slug like
            // "/en-US/docs/Web/JavaScript/Reference/..."), joining it onto
            // repo_root produces a nonsense path like
            // "/repo/.bench_corpora/.../en-US/docs/...".  These are external
            // URL slugs that will never exist on disk — skip them early rather
            // than letting canonicalize produce a confusing debug message.
            if dep_path.is_absolute() {
                tracing::trace!(
                    "[Compiler] Skipping absolute-slug dependency {:?} (external URL slug, not a repo path)",
                    dep_path
                );
                return;
            }

            // Resolve relative to builder's repo_root
            self.builder.repo_root().join(dep_path)
        } else {
            tracing::warn!(
                "No connectivity between builder.repo and dependent path network {}",
                net
            );
            return;
        };

        // Canonicalize if it exists, then normalise to strip any \\?\ prefix (Windows).
        let canonical_dep_path = match full_dep_path.canonicalize() {
            Ok(p) => Self::normalize_queue_path(p),
            Err(_) => {
                tracing::trace!(
                    "[Compiler] Cannot canonicalize {:?}, treating as external",
                    full_dep_path
                );
                return; // Skip external/non-existent dependencies
            }
        };

        // Guard: reject out-of-repo paths. Cross-repo references are not supported —
        // strip_prefix(repo_root) would silently fail at HTML/asset write sites.
        // A future --include-root CLI flag would lift this restriction.
        if !canonical_dep_path.starts_with(self.builder.repo_root()) {
            tracing::warn!(
                "[Compiler] Dependency {:?} (from {:?}) resolves outside repo_root {:?} — \
                 cross-repo references are not yet supported, skipping",
                canonical_dep_path,
                path,
                self.builder.repo_root(),
            );
            return;
        }

        // Enqueue the dependency if it has not yet been processed (count == 0 means
        // it either hasn't been seen at all, or was reset via on_file_modified).
        // Already-processed paths (count >= 1) have already contributed their parse
        // output; re-enqueueing them here would be redundant. Self re-queuing for
        // still-unresolved source files is handled by process_one_parse_result.
        let already_processed = self
            .processed
            .get(&canonical_dep_path)
            .copied()
            .unwrap_or(0)
            > 0;
        let already_queued = self.remainder_queue.contains(&canonical_dep_path);

        tracing::trace!(
            "[process_unresolved_reference] candidate={:?} already_processed={} already_queued={}",
            canonical_dep_path,
            already_processed,
            already_queued,
        );

        if !already_processed && !already_queued {
            if CODECS.path_get(&canonical_dep_path).is_some()
                && self.proto_index.sort_key_for(&canonical_dep_path).is_none()
            {
                tracing::warn!(
                    "[Compiler] Document dependency {:?} (referenced from {:?}) was not in \
                     the ProtoIndex — possible stale index or new file added after build. \
                     Enqueueing as late discovery.",
                    canonical_dep_path,
                    path,
                );
            }
            self.remainder_queue.push_back(canonical_dep_path.clone());
        }
    }
    /// Check if there are pending items to parse.
    pub fn has_pending(&self) -> bool {
        !self.remainder_queue.is_empty()
    }

    /// Get the number of items in the remainder queue.
    pub fn remainder_queue_len(&self) -> usize {
        self.remainder_queue.len()
    }

    /// Backward-compatible alias: returns remainder_queue_len.
    pub fn primary_queue_len(&self) -> usize {
        self.remainder_queue.len()
    }

    /// Returns 0 (reparse queue no longer exists separately).
    pub fn reparse_queue_len(&self) -> usize {
        0
    }

    /// Get the total number of items in the remainder queue.
    pub fn total_queue_len(&self) -> usize {
        self.remainder_queue.len()
    }

    /// Get a reference to the underlying builder
    pub fn builder(&self) -> &GraphBuilder {
        &self.builder
    }

    /// Get a mutable reference to the underlying builder
    pub fn builder_mut(&mut self) -> &mut GraphBuilder {
        &mut self.builder
    }

    /// Get statistics about processed files
    pub fn processed_count(&self) -> usize {
        self.processed.len()
    }

    /// Get the parse count for a specific file
    pub fn get_parse_count(&self, path: impl AsRef<Path>) -> usize {
        self.processed.get(path.as_ref()).copied().unwrap_or(0)
    }

    /// Get statistics about the compiler state (useful for debugging).
    pub fn stats(&self) -> CompilerStats {
        CompilerStats {
            remainder_queue_len: self.remainder_queue.len(),
            processed_count: self.processed.len(),
            total_parses: self.processed.values().sum(),
        }
    }

    /// Notify compiler of belief events (e.g., from event stream).
    ///
    /// No-op in the simplified single-queue architecture — the remainder loop
    /// handles re-parses by parse count rather than by tracking specific node updates.
    /// Kept for API compatibility with the watch service.
    pub fn on_belief_event(&mut self, _event: &BeliefEvent) {}

    /// Export BeliefGraph to JSON file for client-side use
    ///
    /// # Arguments
    /// * `graph` - BeliefGraph to export (from session_bb or database)
    ///
    /// # File Size Warning
    /// Emits warning if exported JSON exceeds 10MB
    pub async fn export_beliefbase_json(
        &self,
        graph: crate::beliefbase::BeliefGraph,
    ) -> Result<(), BuildonomyError> {
        let html_dir = match &self.html_output_dir {
            Some(dir) => dir,
            None => return Ok(()), // No HTML output configured
        };

        let json_path = html_dir.join("beliefbase.json");

        // Serialize to JSON
        let json_string = serde_json::to_string_pretty(&graph)
            .map_err(|e| BuildonomyError::Serialization(e.to_string()))?;

        let file_size_bytes = json_string.len();
        let file_size_mb = file_size_bytes as f64 / (1024.0 * 1024.0);

        // Warn if file is large
        const SIZE_WARNING_THRESHOLD_MB: f64 = 10.0;
        if file_size_mb > SIZE_WARNING_THRESHOLD_MB {
            tracing::warn!(
                "BeliefGraph export is {:.2} MB (exceeds {} MB threshold). \
                 Consider implementing pagination for large datasets.",
                file_size_mb,
                SIZE_WARNING_THRESHOLD_MB
            );
        }

        // Write to file
        tokio::fs::write(&json_path, json_string).await?;

        tracing::debug!(
            "Exported BeliefGraph to {} ({:.2} MB, {} states, {} relations)",
            json_path.display(),
            file_size_mb,
            graph.states.len(),
            graph.relations.0.edge_count()
        );

        Ok(())
    }

    /// Copy static assets (CSS, JS, templates) to HTML output directory
    ///
    /// Extracts all vendored assets using the asset management module.
    /// When use_cdn is true, skips Open Props extraction (uses CDN instead).
    fn copy_static_assets(html_output_dir: &Path, use_cdn: bool) -> Result<(), BuildonomyError> {
        // Extract vendored assets (CSS, JS, templates)
        crate::codec::assets::extract_assets(html_output_dir, use_cdn)?;

        let mode = if use_cdn { "CDN" } else { "local" };
        tracing::debug!(
            "Extracted static assets to {}/assets (mode: {})",
            html_output_dir.display(),
            mode
        );
        Ok(())
    }

    /// Generate HTML for all deferred network files after parsing completes.
    ///
    /// Network index.html files need to list child documents, but during initial parsing
    /// the children haven't been processed yet. This method generates network indices
    /// after all documents have been parsed and added to the belief base.
    ///
    /// Called automatically by parse_all() when both queues are empty.
    ///
    /// # Parameters
    /// - `global_bb`: Synchronized BeliefBase with complete graph relationships
    pub async fn generate_deferred_html<B: BeliefSource + Clone>(
        &self,
        global_bb: B,
    ) -> Result<(), BuildonomyError> {
        let html_output_dir = match &self.html_output_dir {
            Some(dir) => dir.clone(),
            None => return Ok(()), // No HTML output configured
        };

        if self.deferred_html.is_empty() {
            return Ok(());
        }

        tracing::debug!(
            "[generate_deferred_html] Generating HTML for {} deferred network files",
            self.deferred_html.len()
        );

        for file_path in self.deferred_html.iter() {
            tracing::debug!(
                "[generate_deferred_html] Generating HTML for file at path={:?}",
                file_path
            );

            if let Err(e) = self
                .generate_html_for_path(file_path, &html_output_dir, global_bb.clone())
                .await
            {
                tracing::warn!(
                    "[generate_deferred_html] Failed to generate HTML for {:?}: {}",
                    file_path,
                    e
                );
            }
        }

        Ok(())
    }

    /// Generate SPA shell (index.html) at HTML output root using Responsive template
    async fn generate_spa_shell(&self) -> Result<(), BuildonomyError> {
        let html_output_dir = match &self.html_output_dir {
            Some(dir) => dir.clone(),
            None => return Ok(()), // No HTML output configured
        };

        // Get repository root network node for metadata from synchronized BeliefBase
        let repo_bid = self.builder.repo();
        let repo_node = self
            .builder
            .session_bb()
            .states()
            .get(&repo_bid)
            .ok_or_else(|| {
                BuildonomyError::Codec("Repository root node not found in belief base".to_string())
            })?;

        // Generate SPA shell with responsive template
        let template = get_template(Layout::Responsive);

        // Get BID string for entry point
        let bid = repo_bid.to_string();
        let title = repo_node.display_title();

        // Get stylesheet URLs based on use_cdn parameter
        let stylesheet_urls = get_stylesheet_urls(self.use_cdn);

        // Format script tag if provided
        let script_tag = self
            .html_script
            .as_ref()
            .map(|s| format!("<script>{}</script>", s))
            .unwrap_or_default();

        // Replace template placeholders
        let html = template
            .replace(
                "{{CONTENT}}",
                r#"<div id="content-root"><p>Loading...</p></div>"#,
            )
            .replace("{{TITLE}}", &title)
            .replace("{{BID}}", &bid)
            .replace("{{SCRIPT}}", &script_tag)
            .replace("{{STYLESHEET_OPEN_PROPS}}", &stylesheet_urls.open_props)
            .replace("{{STYLESHEET_NORMALIZE}}", &stylesheet_urls.normalize)
            .replace("{{STYLESHEET_THEME_LIGHT}}", &stylesheet_urls.theme_light)
            .replace("{{STYLESHEET_THEME_DARK}}", &stylesheet_urls.theme_dark)
            .replace("{{STYLESHEET_LAYOUT}}", &stylesheet_urls.layout);

        let index_path = html_output_dir.join("index.html");
        tokio::fs::write(&index_path, html).await?;

        tracing::debug!(
            "[generate_spa_shell] Wrote SPA shell: {}",
            index_path.display()
        );

        Ok(())
    }

    /// Generate sitemap.xml with all document fragment URLs
    async fn generate_sitemap<B: BeliefSource + Clone>(
        &self,
        global_bb: B,
    ) -> Result<(), BuildonomyError> {
        let html_output_dir = match &self.html_output_dir {
            Some(dir) => dir.clone(),
            None => return Ok(()), // No HTML output configured
        };

        // Get all document paths from the repository network (including subnets)
        let repo_bid = self.builder.repo();
        let document_paths: Vec<(String, Bid)> = global_bb
            .get_all_paths(repo_bid, true)
            .await
            .unwrap_or_default();

        tracing::debug!(
            "[generate_sitemap] Found {} document paths for sitemap",
            document_paths.len()
        );

        // Build sitemap XML
        let mut sitemap = String::from(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
"#,
        );

        // Get codec extensions for link normalization
        let codec_extensions = crate::codec::CODECS.extensions();

        for (repo_relative_path, _bid) in document_paths {
            // Skip empty path (represents the network node itself)
            if repo_relative_path.is_empty() {
                continue;
            }

            // Skip anchor paths (sections within documents) - sitemap should only include document-level URLs
            if repo_relative_path.contains('#') {
                continue;
            }

            // Convert to HTML path (replace codec extension with .html)
            let mut html_path = repo_relative_path.clone();

            // Check if this is a directory path (network node) without an extension
            if Path::new(&html_path).extension().is_none() {
                // Directory paths should point to index.html
                html_path = format!("{}/index.html", html_path.trim_end_matches('/'));
            } else {
                // Regular files: replace codec extension with .html
                for ext in codec_extensions.iter() {
                    if html_path.ends_with(&format!(".{}", ext)) {
                        html_path = html_path.replace(&format!(".{}", ext), ".html");
                        break;
                    }
                }
            }

            // Sitemap points to static content in /pages/ subdirectory
            let static_path = format!("/pages/{}", html_path);

            // Generate full URL if base_url is configured, otherwise use relative path
            let full_url = if let Some(base) = &self.base_url {
                format!("{}{}", base.trim_end_matches('/'), static_path)
            } else {
                static_path
            };

            // Add URL entry
            sitemap.push_str(&format!("  <url>\n    <loc>{}</loc>\n  </url>\n", full_url));
        }

        sitemap.push_str("</urlset>\n");

        // Write sitemap.xml to output root
        let sitemap_path = html_output_dir.join("sitemap.xml");
        tokio::fs::write(&sitemap_path, sitemap).await?;

        tracing::debug!(
            "[generate_sitemap] Wrote sitemap: {}",
            sitemap_path.display()
        );

        Ok(())
    }

    /// Write HTML fragment to pages/ subdirectory with Layout::Simple wrapper
    async fn write_fragment(
        &self,
        html_output_dir: &Path,
        rel_path: &Path,
        html_body: String,
        title: &str,
        bid: &Bid,
    ) -> Result<(), BuildonomyError> {
        let pages_dir = html_output_dir.join("pages");
        let output_path = pages_dir.join(rel_path);

        // Ensure parent directories exist
        if let Some(parent) = output_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        // Wrap body with Layout::Simple template
        let template = get_template(Layout::Simple);

        // Generate SPA route (for interactive link and canonical URL)
        let spa_route = format!("/#/{}", rel_path.display());

        // Generate canonical URL (use base URL if configured, otherwise relative)
        let canonical_url = if let Some(base) = &self.base_url {
            format!("{}{}", base.trim_end_matches('/'), &spa_route)
        } else {
            spa_route.clone()
        };

        let html = template
            .replace("{{BODY}}", &html_body)
            .replace("{{CANONICAL}}", &canonical_url)
            .replace("{{SPA_ROUTE}}", &spa_route)
            .replace("{{TITLE}}", title)
            .replace("{{BID}}", &bid.to_string());

        // Inject optional script if configured
        let html = if let Some(script) = &self.html_script {
            html.replace("{{SCRIPT}}", &format!("<script>{}</script>", script))
        } else {
            html.replace("{{SCRIPT}}", "")
        };

        tokio::fs::write(&output_path, html).await?;

        tracing::debug!("Wrote HTML fragment: {}", output_path.display());
        Ok(())
    }

    /// The paths we're provided come from the builder. they should already be relative to repo_root
    async fn generate_html_for_path<B: BeliefSource + Clone>(
        &self,
        source_path: &Path,
        html_output_dir: &Path,
        global_bb: B,
    ) -> Result<(), BuildonomyError> {
        // Get file extension
        let path_str = os_path_to_string(source_path);
        let source_path_ap = AnchorPath::new(&path_str);
        let codec_factory = CODECS.get(&source_path_ap).ok_or_else(|| {
            let msg = format!("No codec available for {} files", source_path_ap);
            tracing::warn!("{}", msg);
            BuildonomyError::Codec(msg)
        })?;
        // Query for the node using repo-relative path. source_path is an absolute filesystem
        // path (stored in self.deferred_html), which was normalised by normalize_queue_path
        // before insertion, so strip_prefix against repo_root is safe.
        let repo_relative_str = source_path
            .strip_prefix(self.builder.repo_root())
            .map(os_path_to_string)
            .unwrap_or_else(|_| path_str.clone());
        let nodekey = NodeKey::Path {
            net: self.builder.repo().bref(),
            path: repo_relative_str.clone(),
        };
        let mut bb = BeliefBase::from(
            global_bb
                .eval_query(
                    &Query {
                        seed: Expression::from(&nodekey),
                        traverse: Some(NeighborsExpression {
                            filter: None,
                            upstream: 1,
                            downstream: 0,
                        }),
                    },
                    true,
                )
                .await?,
        );
        let Some(node) = bb.get(&nodekey) else {
            tracing::warn!(
                "[generate_html_for_path] No match found for path: '{}'\nbb.paths:\n{}",
                nodekey,
                bb.paths()
            );
            return Ok(());
        };
        let Some(ctx) = bb.get_context(&self.builder.repo(), &node.bid) else {
            tracing::warn!(
                "[generate_html_for_path] No match found for path: '{}'",
                nodekey
            );
            return Ok(());
        };

        // Generate HTML using fresh codec instance (deferred generation)
        let codec = codec_factory();

        // Get title for write_fragment fallback path
        let title = ctx.node.display_title().to_string();

        // Convert absolute path to repo-relative path.
        // source_path is normalised (via normalize_queue_path at insertion), so
        // strip_prefix against repo_root is safe on all platforms.
        let repo_relative_path = source_path
            .strip_prefix(self.builder.repo_root())
            .unwrap_or(source_path);

        // Get base directory for output (ctx.path for directories, parent for files)
        // ctx.path is home-network relative, so for network nodes it's just the network name
        // For document files, use the parent directory
        let base_dir = if source_path.is_dir() {
            // Network nodes may pass in directories as source_path
            repo_relative_path
        } else {
            // Document nodes: use parent directory of the source file
            repo_relative_path.parent().unwrap_or(Path::new(""))
        };

        // Compute the expected on-disk HTML output path so the deferred codec can read and
        // modify it in place (sentinel replacement). This mirrors write_fragment's layout:
        // html_output_dir / "pages" / base_dir / filename.
        //
        // For network nodes the deferred output filename is always "index.html".
        let deferred_filename_buf;
        let deferred_filename = if ctx.node.kind.is_network() {
            "index.html"
        } else {
            deferred_filename_buf = format!(
                "{}.html",
                source_path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("document")
            );
            deferred_filename_buf.as_str()
        };
        let existing_html_path = html_output_dir
            .join("pages")
            .join(base_dir)
            .join(deferred_filename);

        match codec.generate_deferred_html(&ctx, &existing_html_path)? {
            None => {
                // Codec handled the write itself (in-place sentinel replacement). Nothing to do.
            }
            Some((filename, html_body)) => {
                // Codec returned a fragment — write it via write_fragment as normal.
                let rel_path = base_dir.join(&filename);
                self.write_fragment(html_output_dir, &rel_path, html_body, &title, &node.bid)
                    .await?;
            }
        }

        Ok(())
    }

    /// Create content-addressed hardlinks for all tracked assets in HTML output directory
    /// discovered during parsing.
    ///
    /// This method:
    /// 1. Copies each unique asset (by content hash) to `static/{hash}.{ext}`
    /// 2. Creates hardlinks from semantic paths to the canonical location
    /// 3. Deduplicates automatically - same content = same physical file
    ///
    /// # Arguments
    /// * `html_output_dir` - Base directory for HTML output
    /// * `manifest_data` - Map of asset paths to their BIDs
    ///
    /// # Returns
    /// * `Ok(())` on success
    /// * `Err(BuildonomyError)` if filesystem operations fail
    pub async fn create_asset_hardlinks(
        &self,
        manifest_data: &BTreeMap<String, Bid>,
    ) -> Result<(), BuildonomyError> {
        if manifest_data.is_empty() {
            return Ok(());
        }
        let Some(html_output_dir) = self.html_output_dir() else {
            return Ok(());
        };

        tracing::debug!(
            "[Compiler] Creating asset hardlinks for {} assets",
            manifest_data.len()
        );

        let mut copied_canonical: HashSet<PathBuf> = HashSet::new();

        for (asset_path, asset_bid) in manifest_data.iter() {
            // Get asset node to extract content hash from payload
            let asset_node = self
                .builder
                .session_bb()
                .states()
                .get(asset_bid)
                .ok_or_else(|| {
                    BuildonomyError::Codec(format!("Asset node not found for BID: {}", asset_bid))
                })?;

            // Skip assets without content_hash (unresolved assets)
            let Some(content_hash) = asset_node
                .payload
                .get("content_hash")
                .and_then(|v| v.as_str())
            else {
                tracing::warn!(
                    "[Compiler] Skipping asset without content_hash: {} (path: {})",
                    asset_bid,
                    asset_path
                );
                continue;
            };

            // Get file extension from asset path
            let ext = Path::new(asset_path)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("");

            // Content-addressed canonical location: static/{hash}.{ext} or static/{hash}
            let canonical_name = if ext.is_empty() {
                content_hash.to_string()
            } else {
                format!("{}.{}", content_hash, ext)
            };
            let canonical = html_output_dir.join("static").join(&canonical_name);

            // Copy to canonical location (once per content hash)
            if !copied_canonical.contains(&canonical) {
                let repo_full_path = self.builder.repo_root().join(string_to_os_path(asset_path));

                // Verify source file exists
                if !repo_full_path.exists() {
                    tracing::warn!(
                        "[Compiler] Asset source file not found, skipping: {}",
                        repo_full_path.display()
                    );
                    continue;
                }

                // Create static directory if needed
                if let Some(parent) = canonical.parent() {
                    tokio::fs::create_dir_all(parent).await?;
                }

                // Copy file to canonical location
                tokio::fs::copy(&repo_full_path, &canonical).await?;
                copied_canonical.insert(canonical.clone());
            } else {
                tracing::debug!(
                    "[Compiler] Duplicate content detected: {} (hash: {}) - reusing canonical {}",
                    asset_path,
                    content_hash,
                    canonical.display()
                );
            }

            // Create hardlink at semantic path in pages/ subdirectory (where HTML documents are)
            let html_full_path = html_output_dir
                .join("pages")
                .join(string_to_os_path(asset_path));

            // Create parent directories for semantic path
            if let Some(parent) = html_full_path.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }

            // Remove existing file/link if present
            if html_full_path.exists() {
                tokio::fs::remove_file(&html_full_path).await?;
            }

            // Try to create hardlink, fall back to copy if hardlink fails
            match tokio::fs::hard_link(&canonical, &html_full_path).await {
                Ok(_) => {}
                Err(e) => {
                    // Hardlink failed (maybe filesystem doesn't support it), fall back to copy
                    tracing::debug!(
                        "[Compiler] Hardlink failed ({}), copying instead: {}",
                        e,
                        html_full_path.display()
                    );
                    tokio::fs::copy(&canonical, &html_full_path).await?;
                }
            }
        }

        tracing::debug!(
            "[Compiler] Asset hardlinks created: {} unique files, {} total paths",
            copied_canonical.len(),
            manifest_data.len()
        );

        Ok(())
    }
}

/// Statistics about the compiler's current state.
#[derive(Debug, Clone, Default)]
pub struct CompilerStats {
    pub remainder_queue_len: usize,
    pub processed_count: usize,
    pub total_parses: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        beliefbase::{BeliefBase, BeliefGraph},
        codec::{diagnostic::UnresolvedReference, network::NETWORK_CHILDREN_SENTINEL},
        event::BeliefEvent,
        nodekey::NodeKey,
        properties::{Bid, WeightKind},
        shard::{
            export::export_beliefbase,
            manifest::{SearchManifest, ShardConfig},
        },
    };
    use petgraph::Direction;
    use tokio::sync::mpsc::unbounded_channel;

    /// Helper: Create a test network directory with index.md file
    fn create_test_network(dir: &std::path::Path) {
        std::fs::write(
            dir.join("index.md"),
            r#"---
id: "test-network"
title: "Test Network"
---

# Test Network

Test network for unit tests.
"#,
        )
        .unwrap();
    }

    /// Helper: run promotion on a single-entry result slice and return the diagnostics.
    fn promote_single(
        diagnostics: Vec<crate::codec::ParseDiagnostic>,
    ) -> Vec<crate::codec::ParseDiagnostic> {
        let mut results = vec![ParseResult {
            path: std::path::PathBuf::from("docs/page.md"),
            rewritten_content: None,
            dependent_paths: vec![],
            diagnostics,
        }];
        DocumentCompiler::promote_unresolved_to_warnings(&mut results);
        results.remove(0).diagnostics
    }

    #[test]
    fn test_compiler_creation() {
        // This is a basic structure test - actual functional tests would require
        // setting up a test filesystem and mock cache
        let temp_dir = tempfile::tempdir().unwrap();
        create_test_network(temp_dir.path());
        let result = DocumentCompiler::new(temp_dir.path(), None, Some(5), false);
        assert!(result.is_ok());

        let compiler = result.unwrap();
        assert_eq!(compiler.max_reparse_count, 5);
        // Construction no longer pre-seeds the queue; parse_sequential/parse_all
        // iterate from proto_index.ordered_paths() directly.
        assert!(!compiler.has_pending());
        assert_eq!(compiler.remainder_queue_len(), 0);
        assert_eq!(compiler.reparse_queue_len(), 0);
    }

    #[test]
    fn test_enqueue_deduplication() {
        let temp_dir = tempfile::tempdir().unwrap();
        create_test_network(temp_dir.path());
        let mut compiler = DocumentCompiler::new(temp_dir.path(), None, None, false).unwrap();

        let test_path = temp_dir.path().join("test.md");
        compiler.enqueue(&test_path);
        let initial_len = compiler.total_queue_len();

        // Enqueuing the same path again should not increase queue size
        compiler.enqueue(&test_path);
        assert_eq!(compiler.total_queue_len(), initial_len);
    }

    #[test]
    fn test_stats() {
        let temp_dir = tempfile::tempdir().unwrap();
        create_test_network(temp_dir.path());
        let compiler = DocumentCompiler::new(temp_dir.path(), None, None, false).unwrap();

        let stats = compiler.stats();
        assert_eq!(stats.remainder_queue_len, 0);
        assert_eq!(stats.processed_count, 0);
        assert_eq!(stats.total_parses, 0);
    }

    // --- Diagnostic promotion tests ---

    #[test]
    fn test_promote_unresolved_to_warnings_converts_outgoing() {
        let net_bref = Bid::default().bref();
        let unresolved = UnresolvedReference {
            direction: Direction::Outgoing,
            self_bid: Bid::nil(),
            self_net: Bid::nil(),
            self_path: "docs/page.md".to_string(),
            other_keys: vec![NodeKey::Path {
                net: net_bref,
                path: "docs/missing.md".to_string(),
            }],
            weight_kind: WeightKind::Epistemic,
            weight_data: None,
            reference_location: Some((10, 3)),
        };

        let diagnostics = promote_single(vec![crate::codec::ParseDiagnostic::UnresolvedReference(
            unresolved,
        )]);

        assert_eq!(diagnostics.len(), 1);
        match &diagnostics[0] {
            crate::codec::ParseDiagnostic::Warning {
                message: msg,
                location,
            } => {
                assert!(msg.contains("unresolved link"), "message: {msg}");
                // Path is not embedded in the message — callers (CLI, LSP) are responsible
                // for prefixing path and location when rendering diagnostics.
                // Location is a structured field; callers (CLI, LSP) format it as needed.
                assert_eq!(
                    *location,
                    Some((10, 3)),
                    "location field must carry line:col"
                );
            }
            other => panic!("Expected Warning, got {other:?}"),
        }
    }

    #[test]
    fn test_promote_unresolved_to_warnings_promotes_unresolved_source() {
        let net_bref = Bid::default().bref();
        // Direction::Incoming — the source node of a relation could not be found.
        // These are promoted to warnings just like outgoing unresolved refs.
        let unresolved_source = UnresolvedReference {
            direction: Direction::Incoming,
            self_bid: Bid::nil(),
            self_net: Bid::nil(),
            self_path: "docs/page.md".to_string(),
            other_keys: vec![NodeKey::Path {
                net: net_bref,
                path: "docs/other.md".to_string(),
            }],
            weight_kind: WeightKind::Epistemic,
            weight_data: None,
            reference_location: None,
        };

        let diagnostics = promote_single(vec![crate::codec::ParseDiagnostic::UnresolvedReference(
            unresolved_source,
        )]);

        assert_eq!(diagnostics.len(), 1);
        assert!(
            matches!(
                &diagnostics[0],
                crate::codec::ParseDiagnostic::Warning { .. }
            ),
            "Unresolved source should be promoted to Warning"
        );
    }

    #[test]
    fn test_promote_unresolved_without_location() {
        let net_bref = Bid::default().bref();
        let unresolved = UnresolvedReference {
            direction: Direction::Outgoing,
            self_bid: Bid::nil(),
            self_net: Bid::nil(),
            self_path: "docs/page.md".to_string(),
            other_keys: vec![NodeKey::Path {
                net: net_bref,
                path: "docs/missing.md".to_string(),
            }],
            weight_kind: WeightKind::Epistemic,
            weight_data: None,
            reference_location: None,
        };

        let diagnostics = promote_single(vec![crate::codec::ParseDiagnostic::UnresolvedReference(
            unresolved,
        )]);

        assert_eq!(diagnostics.len(), 1);
        match &diagnostics[0] {
            crate::codec::ParseDiagnostic::Warning {
                message: msg,
                location,
            } => {
                assert!(msg.contains("unresolved link"), "message: {msg}");
                assert_eq!(
                    *location, None,
                    "no location when reference_location is absent"
                );
            }
            other => panic!("Expected Warning, got {other:?}"),
        }
    }

    #[test]
    fn test_promote_preserves_non_unresolved_diagnostics() {
        let diagnostics = promote_single(vec![
            crate::codec::ParseDiagnostic::warning("existing warning"),
            crate::codec::ParseDiagnostic::info("info message"),
            crate::codec::ParseDiagnostic::parse_error("parse failed", 1),
        ]);

        // All three non-UnresolvedReference diagnostics must pass through unchanged.
        assert_eq!(diagnostics.len(), 3);
        assert!(matches!(
            &diagnostics[0],
            crate::codec::ParseDiagnostic::Warning { .. }
        ));
        assert!(matches!(
            &diagnostics[1],
            crate::codec::ParseDiagnostic::Info { .. }
        ));
        assert!(matches!(
            &diagnostics[2],
            crate::codec::ParseDiagnostic::ParseError { .. }
        ));
    }

    #[test]
    fn test_promote_reparse_limit_exceeded_stripped() {
        // ReparseLimitExceeded is a compiler-internal sentinel that must not survive promotion.
        // Other diagnostics in the same result must be preserved.
        let diagnostics = promote_single(vec![
            crate::codec::ParseDiagnostic::ReparseLimitExceeded,
            crate::codec::ParseDiagnostic::warning("real warning"),
        ]);

        assert_eq!(
            diagnostics.len(),
            1,
            "ReparseLimitExceeded must be stripped"
        );
        assert!(matches!(
            &diagnostics[0],
            crate::codec::ParseDiagnostic::Warning { .. }
        ));
    }

    /// When parse_all's HashMap replaces earlier results with later ones, a resolved reparse
    /// produces no warning. An unresolved reparse still does.
    #[test]
    fn test_promote_reparse_resolved_produces_no_warning() {
        let net_bref = Bid::default().bref();
        let unresolved = UnresolvedReference {
            direction: Direction::Outgoing,
            self_bid: Bid::nil(),
            self_net: Bid::nil(),
            self_path: "docs/page.md".to_string(),
            other_keys: vec![NodeKey::Path {
                net: net_bref,
                path: "docs/other.md".to_string(),
            }],
            weight_kind: WeightKind::Epistemic,
            weight_data: None,
            reference_location: Some((5, 1)),
        };

        // A clean reparse replaced the failing attempt in the HashMap — no warning expected.
        let resolved = promote_single(vec![crate::codec::ParseDiagnostic::info("all good")]);
        assert!(
            resolved
                .iter()
                .all(|d| !matches!(d, crate::codec::ParseDiagnostic::Warning { .. })),
            "A resolved reparse must not produce a warning; diagnostics: {resolved:?}"
        );

        // A still-failing reparse is the sole entry in the HashMap — warning expected.
        let unresolved_diags =
            promote_single(vec![crate::codec::ParseDiagnostic::UnresolvedReference(
                unresolved,
            )]);
        assert_eq!(unresolved_diags.len(), 1);
        assert!(
            matches!(
                &unresolved_diags[0],
                crate::codec::ParseDiagnostic::Warning { .. }
            ),
            "A still-unresolved result must produce a warning; diagnostics: {unresolved_diags:?}"
        );
    }

    #[tokio::test]
    async fn test_broken_link_produces_warning_in_parse_result() {
        let temp_dir = tempfile::tempdir().unwrap();
        create_test_network(temp_dir.path());

        // Write a document with a link that references a node that does not exist.
        std::fs::write(
            temp_dir.path().join("page.md"),
            r#"---
title = "Page"
---

# Page

This has a [broken link](nonexistent.md "bref://000000000000000000000000").
"#,
        )
        .unwrap();

        let global_bb = BeliefBase::default();
        let mut compiler = DocumentCompiler::new(temp_dir.path(), None, Some(2), false).unwrap();
        let results = compiler.parse_all(global_bb, false).await.unwrap();

        // No raw UnresolvedReference should survive after promotion.
        let leftover_unresolved = results
            .iter()
            .flat_map(|r| r.diagnostics.iter())
            .filter(|d| matches!(d, crate::codec::ParseDiagnostic::UnresolvedReference(_)))
            .count();
        assert_eq!(
            leftover_unresolved, 0,
            "No UnresolvedReference should remain after parse_all; diagnostics: {results:#?}"
        );

        // The broken bref link must surface as a Warning.
        let has_unresolved_warning = results
            .iter()
            .flat_map(|r| r.diagnostics.iter())
            .any(|d| matches!(d, crate::codec::ParseDiagnostic::Warning { message, .. } if message.contains("unresolved link")));

        assert!(
            has_unresolved_warning,
            "Expected an 'unresolved link' warning; diagnostics: {results:#?}"
        );
    }

    /// Helper: compile a network directory to html_dir using the full event-loop pattern
    /// required by finalize_html (mirrors the parse command in main.rs).
    async fn compile_to_html(
        network_dir: &std::path::Path,
        html_dir: &std::path::Path,
    ) -> Result<(), Box<dyn std::error::Error>> {
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
            network_dir,
            Some(tx),
            Some(5),
            false,
            Some(html_dir.to_path_buf()),
            None,
            false,
            None,
            None,
        )?;

        let cache = compiler.builder().doc_bb().clone();
        compiler.parse_all(cache, false).await?;

        // Close the tx channel so the processor task finishes.
        compiler.builder_mut().close_tx();
        let final_bb = processor.await?;

        compiler.finalize_html(&final_bb).await?;
        Ok(())
    }

    // ------------------------------------------------------------------
    // Integration: search index generation
    // ------------------------------------------------------------------

    /// Verify that `search/manifest.json` and at least one `.idx.json` are
    /// always written by `finalize_html`, regardless of whether sharding fires.
    #[tokio::test]
    async fn test_finalize_html_always_writes_search_indices() {
        let src_dir = tempfile::tempdir().unwrap();
        let html_dir = tempfile::tempdir().unwrap();

        // Write minimal network.
        create_test_network(src_dir.path());
        std::fs::write(
            src_dir.path().join("doc.md"),
            "---\ntitle = \"Doc\"\n---\n\n# Doc\n\nHello world.\n",
        )
        .unwrap();

        // Compile into the html_dir (full event-loop pattern).
        compile_to_html(src_dir.path(), html_dir.path())
            .await
            .unwrap();

        let search_dir = html_dir.path().join("search");
        assert!(
            search_dir.exists(),
            "search/ directory should always be created"
        );

        let manifest_path = search_dir.join("manifest.json");
        assert!(
            manifest_path.exists(),
            "search/manifest.json should always be written"
        );

        // Parse the manifest and check it has at least one network entry.
        let manifest_json = std::fs::read_to_string(&manifest_path).unwrap();
        let manifest: crate::shard::SearchManifest = serde_json::from_str(&manifest_json).unwrap();
        assert!(
            !manifest.networks.is_empty(),
            "search manifest should list at least one network"
        );

        // Verify each listed .idx.json actually exists on disk.
        for entry in &manifest.networks {
            let idx_path = search_dir.join(&entry.path);
            assert!(
                idx_path.exists(),
                "search index file '{}' listed in manifest should exist on disk",
                entry.path
            );
        }
    }

    // ------------------------------------------------------------------
    // Integration: monolithic export
    // ------------------------------------------------------------------

    /// Small repos (below threshold) must write `beliefbase.json` and must NOT
    /// write `beliefbase/manifest.json`.
    #[tokio::test]
    async fn test_finalize_html_monolithic_below_threshold() {
        let src_dir = tempfile::tempdir().unwrap();
        let html_dir = tempfile::tempdir().unwrap();

        create_test_network(src_dir.path());

        compile_to_html(src_dir.path(), html_dir.path())
            .await
            .unwrap();

        // Monolithic: beliefbase.json must exist.
        assert!(
            html_dir.path().join("beliefbase.json").exists(),
            "monolithic export should write beliefbase.json"
        );

        // Monolithic: no beliefbase/manifest.json.
        assert!(
            !html_dir
                .path()
                .join("beliefbase")
                .join("manifest.json")
                .exists(),
            "monolithic export should NOT write beliefbase/manifest.json"
        );
    }

    // ------------------------------------------------------------------
    // Integration: sharded export
    // ------------------------------------------------------------------

    /// Verify sharded output structure when the shard threshold is forced to 1 byte
    /// by temporarily overriding ShardConfig in a helper. We test the shard module
    /// directly here (calling export_beliefbase with a tiny threshold) rather than
    /// wiring the threshold override all the way through finalize_html, which would
    /// require a test-only config parameter.
    #[tokio::test]
    async fn test_sharded_export_writes_correct_structure() {
        let src_dir = tempfile::tempdir().unwrap();
        let html_dir = tempfile::tempdir().unwrap();

        create_test_network(src_dir.path());
        std::fs::write(
            src_dir.path().join("doc.md"),
            "---\ntitle = \"Shard Doc\"\n---\n\n# Shard Doc\n\nContent here.\n",
        )
        .unwrap();

        // Compile to build a synchronized BeliefBase for graph extraction.
        // We use the event-loop pattern so the final_bb is fully populated.
        let final_bb = {
            let (tx, mut rx) = unbounded_channel::<BeliefEvent>();
            let mut event_bb = BeliefBase::empty();
            let processor = tokio::spawn(async move {
                while let Some(event) = rx.recv().await {
                    let _ = event_bb.process_event(&event);
                }
                event_bb
            });

            let mut compiler = DocumentCompiler::with_html_output(
                src_dir.path(),
                Some(tx),
                Some(5),
                false,
                Some(html_dir.path().to_path_buf()),
                None,
                false,
                None,
                None,
            )
            .unwrap();
            let cache = compiler.builder().doc_bb().clone();
            compiler.parse_all(cache, false).await.unwrap();
            compiler.builder_mut().close_tx();
            processor.await.unwrap()
        };

        let graph = final_bb.export_beliefgraph().await.unwrap();
        let pathmap = final_bb.paths();

        // Force sharded mode: threshold = 1 byte so any non-empty graph shards.
        let config = ShardConfig {
            shard_threshold: 1,
            memory_budget_mb: 200.0,
        };
        let empty_search_manifest = SearchManifest::new();

        let result = export_beliefbase(
            graph,
            &pathmap,
            html_dir.path(),
            &config,
            &empty_search_manifest,
        )
        .await
        .unwrap();

        // Must report as sharded.
        assert!(
            matches!(result, crate::shard::ExportMode::Sharded { .. }),
            "export should be sharded when threshold is 1 byte"
        );

        let bb_dir = html_dir.path().join("beliefbase");
        assert!(bb_dir.exists(), "beliefbase/ directory should be created");
        assert!(
            bb_dir.join("manifest.json").exists(),
            "beliefbase/manifest.json should be written"
        );
        assert!(
            bb_dir.join("global.json").exists(),
            "beliefbase/global.json should be written"
        );
        assert!(
            bb_dir.join("networks").exists(),
            "beliefbase/networks/ directory should be created"
        );

        // Manifest must be valid JSON with correct structure.
        let manifest_json = std::fs::read_to_string(bb_dir.join("manifest.json")).unwrap();
        let manifest: crate::shard::ShardManifest = serde_json::from_str(&manifest_json).unwrap();
        assert!(manifest.sharded, "manifest.sharded should be true");
        assert_eq!(manifest.memory_budget_mb, 200.0);

        // Every network listed in the manifest must have its shard file on disk.
        for net in &manifest.networks {
            let shard_path = bb_dir.join(&net.path);
            assert!(
                shard_path.exists(),
                "shard file '{}' listed in manifest should exist",
                net.path
            );
        }
    }

    // ------------------------------------------------------------------
    // Integration: backward compat — old beliefbase.json still loads
    // ------------------------------------------------------------------

    /// Verify that the monolithic `beliefbase.json` is valid JSON that can be
    /// deserialized as a `BeliefGraph` (backward compat with old viewer code).
    #[tokio::test]
    async fn test_monolithic_beliefbase_json_is_valid_belief_graph() {
        let src_dir = tempfile::tempdir().unwrap();
        let html_dir = tempfile::tempdir().unwrap();

        create_test_network(src_dir.path());

        compile_to_html(src_dir.path(), html_dir.path())
            .await
            .unwrap();

        let json_path = html_dir.path().join("beliefbase.json");
        assert!(json_path.exists(), "beliefbase.json must exist");

        let json = std::fs::read_to_string(&json_path).unwrap();
        let graph: BeliefGraph =
            serde_json::from_str(&json).expect("beliefbase.json must deserialize as BeliefGraph");

        // Sanity: the graph should have at least one node (the API node).
        assert!(
            !graph.states.is_empty(),
            "deserialized BeliefGraph should have at least one node"
        );
    }

    /// Verify that `finalize_html` replaces the network-children sentinel in index.html
    /// with an actual child listing `<ul>` when child documents exist.
    ///
    /// Regression test for: sentinel left unreplaced in phase-2 (generate_deferred_html).
    #[tokio::test]
    async fn test_finalize_html_replaces_sentinel_in_index() {
        let src_dir = tempfile::tempdir().unwrap();
        let html_dir = tempfile::tempdir().unwrap();

        // Root network with explicit network-children marker
        std::fs::write(
            src_dir.path().join("index.md"),
            "---\nid: \"root-network\"\ntitle: \"Root Network\"\n---\n\n# Root\n\n<!-- network-children -->\n",
        )
        .unwrap();

        // A child document that should appear in the listing
        std::fs::write(
            src_dir.path().join("child.md"),
            "---\nid: \"child-doc\"\ntitle: \"Child Doc\"\n---\n\n# Child\n\nSome content.\n",
        )
        .unwrap();

        compile_to_html(src_dir.path(), html_dir.path())
            .await
            .unwrap();

        let index_path = html_dir.path().join("pages").join("index.html");
        assert!(
            index_path.exists(),
            "pages/index.html must be written by phase 1"
        );

        let content = std::fs::read_to_string(&index_path).unwrap();

        assert!(
            !content.contains(NETWORK_CHILDREN_SENTINEL),
            "sentinel must be replaced by finalize_html; raw sentinel found in:\n{}",
            &content[..content.len().min(1000)]
        );
        assert!(
            content.contains("<ul>"),
            "replaced sentinel must contain a <ul> child listing; got:\n{}",
            &content[..content.len().min(1000)]
        );
        assert!(
            content.contains("child.html"),
            "child listing must link to child.html; got:\n{}",
            &content[..content.len().min(1000)]
        );
    }

    /// Verify sentinel replacement also works for subnet index.html files.
    #[tokio::test]
    async fn test_finalize_html_replaces_sentinel_in_subnet_index() {
        let src_dir = tempfile::tempdir().unwrap();
        let html_dir = tempfile::tempdir().unwrap();

        // Root network (no marker — sentinel appended automatically)
        std::fs::write(
            src_dir.path().join("index.md"),
            "---\nid: \"root-net\"\ntitle: \"Root\"\n---\n\n# Root\n",
        )
        .unwrap();

        // Subnet directory
        let subnet_dir = src_dir.path().join("sub");
        std::fs::create_dir_all(&subnet_dir).unwrap();
        std::fs::write(
            subnet_dir.join("index.md"),
            "---\nid: \"sub-net\"\ntitle: \"Subnet\"\n---\n\n# Subnet\n\n<!-- network-children -->\n",
        )
        .unwrap();
        std::fs::write(
            subnet_dir.join("page.md"),
            "---\nid: \"subnet-page\"\ntitle: \"Subnet Page\"\n---\n\n# Page\n\nContent.\n",
        )
        .unwrap();

        compile_to_html(src_dir.path(), html_dir.path())
            .await
            .unwrap();

        let subnet_index = html_dir.path().join("pages").join("sub").join("index.html");
        assert!(subnet_index.exists(), "pages/sub/index.html must exist");

        let content = std::fs::read_to_string(&subnet_index).unwrap();

        assert!(
            !content.contains(NETWORK_CHILDREN_SENTINEL),
            "sentinel must be replaced in subnet index.html; raw sentinel found in:\n{}",
            &content[..content.len().min(1000)]
        );
        assert!(
            content.contains("<ul>"),
            "subnet listing must contain a <ul>; got:\n{}",
            &content[..content.len().min(1000)]
        );
        assert!(
            content.contains("page.html"),
            "subnet listing must link to page.html; got:\n{}",
            &content[..content.len().min(1000)]
        );
    }
}
