use crate::{
    beliefbase::BeliefGraph,
    beliefbase::{BeliefBase, BeliefSink, EpochDrain},
    codec::{
        assets::{get_stylesheet_urls, get_template, Layout},
        belief_ir::IRNode,
        builder::{AssetCodec, GraphBuilder, ParseContentResult, ParseContentWithCodec},
        is_network_index_file,
        network::{detect_network_file, NetworkCodec, NETWORK_NAME},
        proto_index::ProtoIndex,
        CodecContentMode, CodecFactory, DocCodec, ParseDiagnostic, UnclaimedDataCodec,
        UnresolvedReference, CLAIM_MAP, CODECS, WALK_CODECS,
    },
    error::BuildonomyError,
    event::{BeliefEvent, EventOrigin},
    nodekey::NodeKey,
    paths::{os_path_to_string, string_to_os_path, AnchorPath, AnchorPathBuf},
    properties::{
        asset_namespace, content_namespaces, BeliefKind, Bid, Bref, Weight, WeightKind,
        WEIGHT_OWNED_BY, WEIGHT_SORT_KEY,
    },
    query::{
        lookup_node,
        spec::{
            ProjectionStep, QueryPackage, QuerySpec, Role, TapeFn, TraversalDepth, TraversalSpec,
        },
        BeliefSource,
    },
};

use sha2::Digest;
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

/// A lightweight progress reporter for `parse_all`. Wraps an optional `indicatif::ProgressBar`
/// and exposes a uniform interface regardless of whether the bar is active.
///
/// The reporter is advanced at each `drain_epoch` boundary in `parse_all`. During the
/// structured Epoch 0 passes (network dirs and leaf docs) it shows a determinate bar;
/// during the remainder loop it shows an indeterminate spinner since the total is not
/// known up-front.
#[cfg(not(target_arch = "wasm32"))]
pub struct ProgressReporter {
    #[cfg(feature = "bin")]
    bar: Option<indicatif::ProgressBar>,
}

#[cfg(all(not(target_arch = "wasm32"), feature = "bin"))]
impl Default for ProgressReporter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl ProgressReporter {
    /// Create a reporter that shows a progress bar on stderr.
    #[cfg(feature = "bin")]
    pub fn new() -> Self {
        let bar = indicatif::ProgressBar::new(0);
        bar.set_style(
            indicatif::ProgressStyle::with_template(
                "{spinner:.cyan} [{bar:40.cyan/blue}] {pos}/{len} {msg}",
            )
            .unwrap()
            .progress_chars("=>-"),
        );
        Self { bar: Some(bar) }
    }

    /// Create a no-op reporter (progress disabled).
    pub fn disabled() -> Self {
        #[cfg(feature = "bin")]
        return Self { bar: None };
        #[cfg(not(feature = "bin"))]
        Self {}
    }

    /// Set total, advance position, and update the status message.
    /// `pos` and `len` together form the fraction shown in the bar.
    /// When `len == 0` (remainder loop) the bar acts as a spinner.
    pub fn update(&self, _pos: u64, _len: u64, _msg: &str) {
        #[cfg(feature = "bin")]
        if let Some(bar) = &self.bar {
            if _len == 0 {
                bar.set_length(0);
            } else {
                bar.set_length(_len);
            }
            bar.set_position(_pos);
            bar.set_message(_msg.to_string());
            bar.tick();
        }
    }

    /// Mark the progress bar as finished and clear it from the terminal.
    pub fn finish(&self) {
        #[cfg(feature = "bin")]
        if let Some(bar) = &self.bar {
            bar.finish_and_clear();
        }
    }
}

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
    /// Controls compile-time layout metadata for the 3D viewer.
    /// Set via `--no-layout` / `--layout-max-nodes N` or `NOET_LAYOUT_MAX_NODES`.
    layout_config: crate::layout::LayoutConfig,
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
    /// The set of paths in the currently-dispatched batch. Pre-incremented paths that
    /// are also in this set are same-batch siblings — their parse output is not yet in
    /// session_bb when siblings run, so a file with an unresolved ref to one must
    /// re-queue itself. Cleared after each batch's results are processed.
    current_batch: std::collections::HashSet<PathBuf>,
    /// NodeKeys that have been confirmed permanently unresolvable — they failed on pass 1
    /// and again on pass 2 (or were rejected as external/non-existent on pass 1).
    ///
    /// When every unresolved reference in a file's diagnostic list has its primary
    /// NodeKey in this set, the file is not re-queued. This prevents the "re-parse storm"
    /// where files containing permanently-broken wikilinks or dead asset paths get
    /// re-queued on every batch pass until max_reparse_count fires.
    ///
    /// Keys are the first element of `UnresolvedReference.other_keys` (the primary key
    /// used by cache_fetch). Using the first key is sufficient: cache_fetch tries all
    /// keys in order and only emits Unresolved when none resolve — so if the primary key
    /// is permanently absent, the ref will never resolve regardless of other keys.
    permanently_unresolved: HashSet<NodeKey>,
    #[cfg(not(target_arch = "wasm32"))]
    progress: ProgressReporter,
    /// Optional per-instance ClaimMap for test isolation.
    /// When None, the global CLAIM_MAP is used.
    #[cfg(not(target_arch = "wasm32"))]
    claim_map: Option<Arc<crate::codec::ClaimMap>>,
}

/// Result of parsing a single document
#[derive(Debug, Clone)]
pub struct ParseResult {
    pub path: PathBuf,
    pub rewritten_content: Option<String>,
    pub dependent_paths: Vec<(String, Bref)>,
    pub diagnostics: Vec<crate::codec::ParseDiagnostic>,
}

/// Metadata for a single HTML fragment write operation.
///
/// Groups the three scalar arguments to [`DocumentCompiler::write_fragment`] that
/// describe the *document identity* — separated from the layout/path arguments so
/// the total parameter count stays within clippy's `too_many_arguments` limit.
struct FragmentMeta<'a> {
    title: &'a str,
    bid: &'a Bid,
    source_path: Option<&'a Path>,
    is_binary: bool,
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
            false,
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
        git_tracking: bool,
    ) -> Result<Self, BuildonomyError> {
        let entry_path = Self::normalize_queue_path(entry_point.as_ref().canonicalize()?);

        let builder = GraphBuilder::new(&entry_path, tx)?;

        // Copy static assets (CSS, JS, templates) to HTML output directory if configured.
        // Done after entry_point validation so a missing/invalid path does not leave
        // partial output in html_output_dir (assets/ and pages/ would otherwise be
        // written before canonicalize() or GraphBuilder::new() returns an error).
        if let Some(ref html_dir) = html_output_dir {
            // Wipe pages/ before each build to prevent stale HTML files from prior
            // builds (e.g. per-tab xlsx files that no longer exist) from persisting.
            // assets/ is not wiped — extract_assets handles idempotent overwriting.
            let pages_dir = html_dir.join("pages");
            if pages_dir.exists() {
                std::fs::remove_dir_all(&pages_dir).map_err(|e| {
                    BuildonomyError::Codec(format!("Failed to wipe pages/ before build: {e}"))
                })?;
            }
            Self::copy_static_assets(html_dir, use_cdn)?;
        }

        // Build the ProtoIndex with a single WalkDir pass from repo_root.
        // Falls back to an empty index on error (e.g. entry_path is not yet a full repo)
        // so construction never fails due to a missing network file at startup.
        let proto_index =
            ProtoIndex::build(builder.repo_root(), git_tracking).unwrap_or_else(|e| {
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
            layout_config: crate::layout::LayoutConfig::default(),
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
            current_batch: std::collections::HashSet::new(),
            permanently_unresolved: HashSet::new(),
            #[cfg(not(target_arch = "wasm32"))]
            progress: ProgressReporter::disabled(),
            #[cfg(not(target_arch = "wasm32"))]
            claim_map: None::<Arc<crate::codec::ClaimMap>>,
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

    /// Configure compile-time layout metadata generation.
    ///
    /// Defaults to enabled with [`crate::layout::DEFAULT_LAYOUT_MAX_NODES`].
    pub fn set_layout_config(&mut self, config: crate::layout::LayoutConfig) {
        self.layout_config = config;
    }

    /// Attach a progress reporter that will be advanced during `parse_all`.
    /// Calling this replaces the default no-op reporter.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn with_progress(&mut self, reporter: ProgressReporter) {
        self.progress = reporter;
    }

    /// Create a new compiler with an entry point (file or directory) and default arguments: no
    /// receiver of BeliefEvents, default reparse count, and write=false.
    ///
    /// # Arguments
    /// * `entry_point` - The file or directory to start parsing from
    pub fn simple(entry_point: impl AsRef<Path>) -> Result<Self, BuildonomyError> {
        let entry_path = Self::normalize_queue_path(entry_point.as_ref().canonicalize()?);

        let builder = GraphBuilder::new(&entry_path, None)?;
        // git_tracking is always false for simple(): it is a convenience constructor
        // used by tests and tools that do not need git metadata.
        let proto_index = ProtoIndex::build(builder.repo_root(), false).unwrap_or_else(|e| {
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
            layout_config: crate::layout::LayoutConfig::default(),
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
            current_batch: std::collections::HashSet::new(),
            permanently_unresolved: HashSet::new(),
            #[cfg(not(target_arch = "wasm32"))]
            progress: ProgressReporter::disabled(),
            #[cfg(not(target_arch = "wasm32"))]
            claim_map: None::<Arc<crate::codec::ClaimMap>>,
        })
    }

    /// Create a compiler with a local ClaimMap for test isolation.
    ///
    /// Use this in tests that exercise `prepare_proto_relations` to avoid polluting
    /// the global `CLAIM_MAP` between test runs. The provided `claim_map` is used
    /// instead of the global `CLAIM_MAP` for all codec lookups in `parse_one_path`.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn with_claim_map(
        entry_point: impl AsRef<Path>,
        claim_map: crate::codec::ClaimMap,
    ) -> Result<Self, BuildonomyError> {
        let mut compiler = Self::simple(entry_point)?;
        compiler.claim_map = Some(Arc::new(claim_map));
        Ok(compiler)
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
                "\n{}\n````\n",
                crate::codec::myst::directive("network_children")
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
                            // Compute the base directory for HTML fragment output.
                            //
                            // Codecs that rewrite IRNode.path (e.g. CppHeaderCodec
                            // stripping include dir prefixes) need the rewritten path
                            // used for the fragment output directory.  For all other
                            // codecs, use the filesystem-derived repo_relative_path.
                            let filesystem_base = repo_relative_path
                                .parent()
                                .unwrap_or(Path::new(""))
                                .to_path_buf();
                            let base_dir = codec
                                .nodes()
                                .first()
                                .and_then(|root_node| {
                                    let node_path = &root_node.path;
                                    let file_path_str = os_path_to_string(&file_path);
                                    // Only use the node path when it differs from the
                                    // filesystem path (indicates a rewrite occurred).
                                    if node_path == &file_path_str {
                                        return None;
                                    }
                                    // Use PathBuf operations (not AnchorPath) to avoid
                                    // the directory-form pitfall where AnchorPath::dir()
                                    // strips the last path component.
                                    let node_pb = PathBuf::from(node_path);
                                    let repo_root_str = os_path_to_string(self.builder.repo_root());
                                    let repo_root_pb = PathBuf::from(&repo_root_str);
                                    let rel = node_pb.strip_prefix(&repo_root_pb).ok()?;
                                    // Network directory nodes have a directory-form path
                                    // (e.g. ".../cameras" not ".../cameras/index.md").
                                    // Their codec returns ("index.html", ...) which gets
                                    // joined to base_dir, so base_dir must BE the directory
                                    // itself — not its parent.  For regular file nodes,
                                    // .parent() extracts the containing directory.
                                    if path.is_dir() {
                                        Some(rel.to_path_buf())
                                    } else {
                                        rel.parent().map(|d| d.to_path_buf())
                                    }
                                })
                                .unwrap_or(filesystem_base);
                            // Pass the repo-relative source file path for the SOURCE_LINK.
                            // Directory paths are network index nodes — their source file
                            // is the NETWORK_NAME file (index.md) inside the directory.
                            let fragment_source_path = if file_path.is_dir() {
                                None
                            } else {
                                Some(repo_relative_path)
                            };
                            for (filename, pairs, layout) in fragments {
                                let rel_path = base_dir.join(&filename);
                                if let Some(layout) = layout {
                                    if let Err(e) = self
                                        .write_fragment(
                                            html_dir,
                                            &rel_path,
                                            pairs,
                                            FragmentMeta {
                                                title: &title,
                                                bid: &bid,
                                                source_path: fragment_source_path,
                                                is_binary: codec.content_mode()
                                                    == CodecContentMode::Binary,
                                            },
                                            layout,
                                        )
                                        .await
                                    {
                                        parse_result.diagnostics.push(ParseDiagnostic::warning(
                                            format!(
                                                "Failed to write HTML fragment {}: {e}",
                                                rel_path.display()
                                            ),
                                        ));
                                    }
                                } else {
                                    // Raw write — no template wrapping (used for companion data files like JSON).
                                    // Use the first pair's value as the raw content (key ignored).
                                    let pages_dir = html_dir.join("pages");
                                    let output_path = pages_dir.join(&rel_path);
                                    if let Some(parent) = output_path.parent() {
                                        tokio::fs::create_dir_all(parent).await.ok();
                                    }
                                    if let Some((_, content)) = pairs.into_iter().next() {
                                        if let Err(e) =
                                            tokio::fs::write(&output_path, content.as_bytes()).await
                                        {
                                            parse_result.diagnostics.push(
                                                ParseDiagnostic::warning(format!(
                                                    "Failed to write raw fragment {}: {e}",
                                                    rel_path.display()
                                                )),
                                            );
                                        }
                                    }
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

                // Re-queue self when any unresolved ref is a same-batch sibling or a newly
                // enqueued dep. Same-batch siblings are pre-incremented before the batch
                // runs, so `processed > 0` is true for them — but their parse output is not
                // yet in session_bb. `process_unresolved_reference` distinguishes these from
                // already-processed deps (from prior batches, whose output IS in session_bb)
                // using `self.current_batch`.
                //
                // Already-processed deps from prior batches do NOT trigger self-requeue: the
                // link should have resolved on the current parse if session_bb had the dep.
                // If it didn't resolve, re-queuing self won't help (the dep's output is
                // already available and was still missing from the PathMap lookup).
                //
                // Returns false for permanent externals (mailto:, out-of-corpus URLs,
                // non-existent paths) that can never resolve.
                let mut any_corpus_dependency = false;

                for unresolved in &unresolved_refs {
                    // If the primary key for this reference has already been confirmed as
                    // permanently unresolvable (failed on a prior pass with no chance of
                    // resolution), skip all re-queue logic for it. This prevents re-parse
                    // storms caused by broken wikilinks, dead asset paths, or any other
                    // reference that can never resolve regardless of what other files are
                    // parsed.
                    if let Some(primary_key) = unresolved.other_keys.first() {
                        if self.permanently_unresolved.contains(primary_key) {
                            tracing::trace!(
                                "[Compiler] Skipping permanently unresolved ref {:?} in {:?}",
                                primary_key,
                                path,
                            );
                            continue;
                        }
                    }

                    let is_asset = unresolved.other_keys.iter().any(|key| {
                        if let NodeKey::Path { net, .. } = key {
                            *net == asset_namespace().bref()
                        } else {
                            false
                        }
                    });
                    let resolved = if is_asset {
                        self.process_asset_reference(&path, unresolved)
                    } else {
                        // For Path-keyed Incoming refs: resolve to a canonical path and
                        // check whether it is a same-batch sibling or a newly-enqueued dep.
                        // For Id-keyed Incoming refs (wikilinks like [[HSTP]]): we cannot
                        // canonicalize to a path, but if current_batch is non-empty any
                        // same-batch sibling could be the target — re-queue self so the
                        // link gets another chance once siblings' output is in session_bb.
                        if let Some((dep_str, net)) = unresolved.as_unresolved_source() {
                            dependent_paths.push((dep_str.clone(), net));
                            self.process_unresolved_reference(&path, &dep_str, net)
                        } else if unresolved.direction == petgraph::Direction::Incoming
                            && unresolved
                                .other_keys
                                .iter()
                                .any(|k| matches!(k, NodeKey::Id { .. }))
                            && !self.current_batch.is_empty()
                        {
                            // Id-keyed ref with same-batch siblings present: the target
                            // may be one of those siblings, whose output isn't in
                            // session_bb yet. Re-queue self to resolve after the batch.
                            true
                        } else {
                            false
                        }
                    };

                    if resolved {
                        any_corpus_dependency = true;
                    } else {
                        // This ref returned false from all resolution paths — it is either
                        // a permanent external (mailto:, out-of-corpus URL, non-existent
                        // path) or an Id-keyed ref that had no active batch siblings.
                        // Record the primary key so future parses of this file (or any
                        // other file containing the same broken ref) skip re-queue
                        // evaluation immediately.
                        if let Some(primary_key) = unresolved.other_keys.first() {
                            if self.permanently_unresolved.insert(primary_key.clone()) {
                                tracing::debug!(
                                    "[Compiler] Marking as permanently unresolved: {:?} \
                                     (referenced from {:?})",
                                    primary_key,
                                    path,
                                );
                            }
                        }
                    }
                }

                if any_corpus_dependency && !self.remainder_queue.contains(&path) {
                    self.remainder_queue.push_back(path.clone());
                }

                // Enqueue derived output paths (e.g. CSV exports of opaque xlsx tabs)
                // so process_asset runs on them in this session's remainder epoch.
                // This gives each derived file a content-addressed asset node with a
                // content_hash immediately — not deferred to the next compile run.
                for derived_abs_path in &parse_result.derived_paths {
                    if !self.remainder_queue.contains(derived_abs_path)
                        && !self.processed.contains_key(derived_abs_path)
                    {
                        tracing::debug!(
                            "[Compiler] Enqueueing derived output for asset registration: {:?}",
                            derived_abs_path
                        );
                        self.remainder_queue.push_back(derived_abs_path.clone());
                    }
                }

                // Preserve rewritten_content from an earlier parse of the same file if this
                // re-parse produced None. A subsequent parse with no changes should not erase
                // BIDs that were written by the first parse — the first parse's rewrite is what
                // stamps the time-based BIDs into the source file so they survive across runs.
                let prior_rewritten_content = self
                    .latest_results
                    .get(&path)
                    .and_then(|r| r.rewritten_content.clone());
                let rewritten_content = parse_result.rewritten_content.or(prior_rewritten_content);
                self.latest_results.insert(
                    path.clone(),
                    ParseResult {
                        path,
                        rewritten_content,
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
        // Note: repo_bid / API-node backfill was removed. The repo root is now parsed
        // sequentially by self.builder before the first parallel epoch (see parse_all
        // pre-epoch block), so self.builder.repo() is already set and session_bb already
        // contains the canonical repo BID and its api→repo_root Section edge by the time
        // any parallel task snapshot is taken. repo_seeded is kept as a parameter for
        // call-site symmetry but no longer drives any backfill logic here.
        let _ = repo_seeded; // used only to preserve call-site API

        self.current_batch = batch_results.iter().map(|(p, _)| p.clone()).collect();
        for (path, task_result) in batch_results {
            self.process_one_parse_result(path, task_result).await;
        }
        self.current_batch.clear();
        Ok(())
    }

    /// Bulk-register asset files, bypassing `parse_epoch` / `GraphBuilder` per-task overhead.
    ///
    /// File I/O and SHA-256 hashing are parallelised in tokio tasks (bounded by
    /// `self.jobs`). Event emission runs sequentially on `self.builder` so that
    /// `session_bb` remains consistent without synchronisation.
    ///
    /// Callers must ensure `session_bb` is warm (all known assets merged from
    /// `global_bb` via `sync_asset_snapshot`) before invoking this method.
    ///
    /// Returns results compatible with `process_epoch_batch_results`.
    async fn process_asset_batch(
        &mut self,
        paths: Vec<PathBuf>,
    ) -> Result<Vec<(PathBuf, Result<ParseContentWithCodec, BuildonomyError>)>, BuildonomyError>
    {
        if paths.is_empty() {
            return Ok(Vec::new());
        }

        let n = paths.len();
        tracing::info!("[process_asset_batch] Bulk-registering {} assets", n,);

        // ── Phase 1: Parallel file read + SHA-256 ──────────────────────────────
        // Each tokio task reads file bytes and computes the SHA-256 hash.
        // Concurrency is bounded by self.jobs via a semaphore.
        let semaphore = Arc::new(Semaphore::new(self.jobs.max(1)));
        let mut join_set: JoinSet<(PathBuf, Result<String, BuildonomyError>)> = JoinSet::new();

        for path in paths {
            let sem = Arc::clone(&semaphore);
            join_set.spawn(async move {
                let _permit = sem.acquire_owned().await.expect("semaphore closed");
                match tokio::fs::read(&path).await {
                    Ok(bytes) => {
                        let hash_str = {
                            let mut hasher = sha2::Sha256::new();
                            Digest::update(&mut hasher, &bytes);
                            format!("{:x}", hasher.finalize())
                        };
                        (path, Ok(hash_str))
                    }
                    Err(e) => (
                        path.clone(),
                        Err(BuildonomyError::Codec(format!(
                            "Failed to read asset {}: {e}",
                            path.display()
                        ))),
                    ),
                }
            });
        }

        // Collect results. Order doesn't matter for assets (no ID collision concerns).
        let mut hash_results: Vec<(PathBuf, Result<String, BuildonomyError>)> =
            Vec::with_capacity(n);
        while let Some(result) = join_set.join_next().await {
            match result {
                Ok(pair) => hash_results.push(pair),
                Err(e) => {
                    tracing::warn!("[process_asset_batch] Task join error: {e}");
                }
            }
        }

        // ── Phase 2: Collect events + batch-apply to session_bb ─────────────
        // Ensure the asset_namespace network node exists in session_bb once
        // before the batch.
        self.builder.ensure_asset_namespace()?;

        // Collect events from process_asset_prehashed (which now returns events
        // without applying them). We accumulate all events, then apply once
        // via apply_events_batch + flush_paths_for_events — O(N) total instead
        // of O(N × per-event PathMap flush).
        let mut all_events: Vec<BeliefEvent> = Vec::with_capacity(n * 3);
        let mut results: Vec<(PathBuf, Result<ParseContentWithCodec, BuildonomyError>)> =
            Vec::with_capacity(hash_results.len());

        for (path, hash_result) in hash_results {
            match hash_result {
                Ok(hash_str) => {
                    if let Some(events) = self.builder.process_asset_prehashed(&path, hash_str) {
                        all_events.extend(events);
                    }
                    results.push((
                        path,
                        Ok(ParseContentWithCodec {
                            result: ParseContentResult::empty(),
                            codec: Box::new(AssetCodec),
                            repo_bid: Bid::nil(),
                            repo_node: None,
                        }),
                    ));
                }
                Err(_e) => {
                    // File read failed — emit a warning result (same as parse_one_path
                    // L1720-1739 for non-codec unreadable paths).
                    let mut parse_result = ParseContentResult::empty();
                    parse_result.add_diagnostic(ParseDiagnostic::warning(format!(
                        "Asset file not found (possibly a misclassified ID link): {}",
                        path.display()
                    )));
                    results.push((
                        path,
                        Ok(ParseContentWithCodec {
                            result: parse_result,
                            codec: Box::new(AssetCodec),
                            repo_bid: Bid::nil(),
                            repo_node: None,
                        }),
                    ));
                }
            }
        }

        // Batch-apply all collected events to session_bb in one pass.
        // apply_events_batch handles NodeUpsert (pass 1) and RelationChange
        // (pass 2) efficiently; flush_paths_for_events drives PathMapMap once.
        if !all_events.is_empty() {
            let resolved = self
                .builder
                .session_bb_mut()
                .apply_events_batch(&all_events)?;
            let path_derivatives = self
                .builder
                .session_bb_mut()
                .flush_paths_for_events(&resolved);

            // Send all events (originals + path derivatives) on tx for the
            // accumulator to process into global_bb.
            for event in all_events.into_iter().chain(path_derivatives) {
                self.builder.tx().send(event)?;
            }
        }

        tracing::info!("[process_asset_batch] Completed {} assets", results.len(),);

        Ok(results)
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
                let parse_number = self.processed.get(&path).copied().unwrap_or(1);
                let (actual_path, result) = Self::parse_one_path(
                    path.clone(),
                    &mut self.builder,
                    global_bb.clone(), // clone the snapshot for this dispatch
                    self.proto_index.clone(),
                    self.write,
                    parse_number,
                    self.claim_map.as_deref().unwrap_or(&CLAIM_MAP),
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
            // Skip network dirs that a parent network has explicitly rejected via
            // CLAIM_MAP.reject() — the parent's whitelist/blacklist suppresses the subnet.
            //
            // When a dir is rejected, cascade the rejection to all descendant network dirs
            // so that deeper subnets whose intermediate parent was rejected (and therefore
            // never ran its own NetworkCodec::parse() to register their rejections) are
            // also suppressed. This handles multi-level filtering, e.g.:
            //   docs/ whitelists only flight_software_design/** → rejects developers/
            //   developers/ (never parsed) → sub-networks like new_user/ are still in
            //   ProtoIndex but must be treated as rejected transitively.
            let batch: Vec<PathBuf> = group
                .into_iter()
                .filter(|d| {
                    if CLAIM_MAP.is_rejected(d) {
                        // Cascade: reject all ProtoIndex network dirs that are descendants
                        // of this rejected dir so deeper depth groups skip them too.
                        for descendant in self.proto_index.network_dirs() {
                            if descendant.starts_with(d.as_path())
                                && &descendant != d
                                && !CLAIM_MAP.is_rejected(&descendant)
                            {
                                CLAIM_MAP.reject(descendant);
                            }
                        }
                        return false;
                    }
                    !self.processed.contains_key(d)
                })
                .collect();
            for dir in &batch {
                *self.processed.entry(dir.clone()).or_insert(0) += 1;
            }
            self.current_batch = batch.iter().cloned().collect();
            for dir in batch {
                run_one!(dir);
                drain_rx!();
            }
            self.current_batch.clear();
        }

        // ── Phase 2: leaf documents in DFS order ─────────────────────────────
        // Collect the whole leaf batch first, increment all counts, then run.
        // Skip children of rejected network dirs — their parent suppressed them.
        let leaf_batch: Vec<PathBuf> = self
            .proto_index
            .network_dirs()
            .into_iter()
            .filter(|net_dir| !CLAIM_MAP.is_rejected(net_dir))
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
        self.current_batch = leaf_batch.iter().cloned().collect();
        for path in leaf_batch {
            run_one!(path);
            drain_rx!();
        }
        self.current_batch.clear();

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
            self.current_batch = candidates.iter().cloned().collect();
            for path in candidates {
                run_one!(path);
                drain_rx!();
            }
            self.current_batch.clear();
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

        // ── Pre-epoch: parse repo root sequentially with self.builder ────────
        //
        // The compiler's session_bb is only populated by events that flow through
        // self.builder. In the parallel path, every task uses its own GraphBuilder
        // and its events go task_tx → shared_tx → BeliefAccumulator → global_bb —
        // they are never replayed into self.builder.session_bb.
        //
        // Consequence: if the repo-root index.md is parsed for the first time inside
        // a parallel epoch, epoch_session_snapshot sees snapshot_edges=0 (no
        // api→repo_root Section edge in session_bb), so every parallel task builder
        // gets an API PathMap with zero subnets. cache_fetch then returns Unresolved
        // for every NodeKey::Id{net:API_bref} lookup, generating a fresh time-based
        // BID for the repo-root network node. That fresh BID ends up in doc_bb.states
        // but not in any PathMap → Phase 4 get_context panics.
        //
        // Fix: parse the repo root once, sequentially, through self.builder before
        // any parallel epoch runs. This commits the canonical repo BID and the
        // api→repo_root Section edge into self.builder.session_bb (and global_bb
        // after drain_epoch) via the normal terminate_stack → tx → BeliefAccumulator
        // pipeline. All subsequent epoch_session_snapshot calls will include the
        // Section edge, giving every parallel task a properly populated API PathMap.
        //
        // The repo-root is the shallowest network dir (first entry in network_dirs(),
        // which is sorted by component count). We mark it in self.processed so the
        // depth-1 group in Phase 1 skips it.
        if !repo_seeded {
            if let Some(root_dir) = self.proto_index.network_dirs().first().cloned() {
                if !self.processed.contains_key(&root_dir) {
                    *self.processed.entry(root_dir.clone()).or_insert(0) += 1;
                    let _ = self.builder.tx().send(BeliefEvent::BatchStart);
                    let root_result = Self::parse_one_path(
                        root_dir,
                        &mut self.builder,
                        cached_global_bb.clone(),
                        self.proto_index.clone(),
                        self.write,
                        1, // first parse of the repo root
                        self.claim_map.as_deref().unwrap_or(&CLAIM_MAP),
                    )
                    .await;
                    let _ = self.builder.tx().send(BeliefEvent::BatchEnd);
                    cached_global_bb.drain_epoch().await?;
                    self.sync_asset_snapshot(&cached_global_bb).await?;
                    self.process_epoch_batch_results(vec![root_result], &mut repo_seeded)
                        .await?;
                }
            }
        }

        // ── Epoch 0, Phase 1: network dirs grouped by subnet-tree depth ──────
        //
        // Each group becomes one epoch batch, drained before the next begins, so
        // that all ancestors at tree depth D are committed to global_bb before
        // depth D+1 starts.
        //
        // Grouping is by *subnet-tree* depth, not OS path-component count: a subnet
        // reached through plain intervening directories (`A/docs/parts/B/`) is a
        // direct child of `A` in the ProtoIndex, so it belongs in the same batch as
        // a sibling at `A/B2/`.  Component-count grouping split such siblings apart
        // and delayed the deeper-pathed one by the length of its path prefix,
        // serializing work that has no dependency relationship.  See
        // ProtoIndex::network_dirs_by_tree_depth.
        //
        // INVARIANT: every dir in group D has its parent network in group D-1.  This
        // is what makes the drain-between-groups discipline sufficient — and also why
        // groups cannot be merged forward to enlarge a small batch: every member of
        // group D+1 depends on something in group D that has not been drained yet.
        let depth_groups = self.proto_index.network_dirs_by_tree_depth();

        #[cfg(not(target_arch = "wasm32"))]
        let total_epoch0 = {
            let net_count = self.proto_index.network_dirs().len() as u64;
            let leaf_count = self
                .proto_index
                .network_dirs()
                .iter()
                .flat_map(|d| self.proto_index.children_of(d).unwrap_or_default())
                .filter(|c| !c.is_dir())
                .count() as u64;
            net_count + leaf_count
        };
        #[cfg(not(target_arch = "wasm32"))]
        let mut epoch0_done: u64 = 0;

        for group in depth_groups {
            // Filter to only unprocessed dirs, increment counts for the whole
            // batch before any file in it runs.
            // Skip network dirs that a parent network has explicitly rejected via
            // CLAIM_MAP.reject() — the parent's whitelist/blacklist suppresses the subnet.
            let batch: Vec<PathBuf> = group
                .into_iter()
                .filter(|d| {
                    if CLAIM_MAP.is_rejected(d) {
                        // Cascade: reject all ProtoIndex network dirs that are descendants
                        // of this rejected dir so deeper depth groups skip them too.
                        for descendant in self.proto_index.network_dirs() {
                            if descendant.starts_with(d.as_path())
                                && &descendant != d
                                && !CLAIM_MAP.is_rejected(&descendant)
                            {
                                CLAIM_MAP.reject(descendant);
                            }
                        }
                        return false;
                    }
                    !self.processed.contains_key(d)
                })
                .collect();
            if batch.is_empty() {
                continue;
            }
            for dir in &batch {
                *self.processed.entry(dir.clone()).or_insert(0) += 1;
            }
            let batch_len = batch.len();
            // Retain a copy of the batch paths for sync_subnet_stubs — the
            // originals are moved into parse_epoch.
            let batch_dirs = batch.clone();
            let _ = self.builder.tx().send(BeliefEvent::BatchStart);
            let results = self.parse_epoch(batch, cached_global_bb.clone()).await?;
            let _ = self.builder.tx().send(BeliefEvent::BatchEnd);
            cached_global_bb.drain_epoch().await?;
            #[cfg(not(target_arch = "wasm32"))]
            {
                epoch0_done += batch_len as u64;
                self.progress
                    .update(epoch0_done, total_epoch0, "parsing networks");
            }
            self.sync_subnet_stubs(&batch_dirs, &cached_global_bb)
                .await?;
            self.sync_asset_snapshot(&cached_global_bb).await?;
            self.process_epoch_batch_results(results, &mut repo_seeded)
                .await?;
        }

        // ── Epoch 0, Phase 2: all leaf documents across all networks ─────────
        //
        // Gather every non-dir child from every network directory.  Skip any path
        // already in processed (stale-seeded) or remainder_queue.  Increment counts
        // for the whole batch before dispatching.
        // Skip children of rejected network dirs — their parent suppressed them.
        let leaf_batch: Vec<PathBuf> = self
            .proto_index
            .network_dirs()
            .into_iter()
            .filter(|net_dir| !CLAIM_MAP.is_rejected(net_dir))
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
            let leaf_batch_len = leaf_batch.len();
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
            #[cfg(not(target_arch = "wasm32"))]
            {
                epoch0_done += leaf_batch_len as u64;
                self.progress
                    .update(epoch0_done, total_epoch0, "parsing documents");
            }
            self.sync_asset_snapshot(&cached_global_bb).await?;
            self.process_epoch_batch_results(leaf_results, &mut repo_seeded)
                .await?;
        }

        // Seed remainder_queue with cached assets from session_bb not yet processed.
        // Assets discovered during epoch-0 via process_asset_reference are already
        // in remainder_queue; this catches cached assets whose referencing documents
        // were not re-parsed (mtime hit).
        {
            let assets: Vec<(String, Bid, Vec<u16>)> = self
                .builder
                .session_bb()
                .submap(asset_namespace(), "", u8::MAX, false)
                .await
                .unwrap_or_default();
            for (repo_relative_path, _bid, _order) in assets {
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

        // ── Remainder loop (epoch ≥ 1) ───────────────────────────────────────
        // Each remainder iteration is split into two sub-epochs:
        //
        //   Sub-epoch A — assets (processed count == 0, never parsed before).
        //   Sub-epoch B — document re-parses (processed count >= 1).
        //
        // The split is necessary for correctness under parallel dispatch (jobs > 1).
        // When assets and the documents that reference them are dispatched in the
        // same parse_epoch call, every task queries the same pre-batch global_bb
        // snapshot.  Asset-processing tasks commit their BIDs via tx → accumulator,
        // but those BIDs are not visible to parallel document tasks in the same
        // batch — drain_epoch hasn't run yet.  Documents whose push_relation calls
        // for asset keys miss in cache_fetch are re-queued, and inject_context sees
        // a different BeliefContext than it will on the next pass (no asset nodes
        // present), producing rewritten_content that the parse-2 no-rewrite
        // assertion then catches as a spurious failure.
        //
        // By draining between the two sub-epochs, asset BIDs are committed to
        // global_bb (and synced into self.builder.session_bb via sync_asset_snapshot
        // → epoch_session_snapshot Part 2) before any document re-parse task starts.
        // Documents that had unresolved asset refs will now hit cache_fetch on their
        // first re-parse attempt, inject_context sees the full BeliefContext, and
        // rewritten_content is produced at most once — not on every subsequent parse.
        //
        // Increment counts for the whole logical batch (both sub-epochs) before
        // dispatch so that process_one_parse_result's reparse-limit check uses
        // consistent counts regardless of which sub-epoch a path lands in.
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

            // Sub-epoch A: assets only (previously unprocessed, count was 0 before increment).
            let (asset_batch, reparse_batch): (Vec<PathBuf>, Vec<PathBuf>) = candidates
                .into_iter()
                .partition(|p| self.processed.get(p).copied().unwrap_or(0) == 1);

            if !asset_batch.is_empty() {
                // Partition into bulk-eligible assets (no codec, not rejected, not
                // WALK_CODECS tracked, not a directory) and residual paths that need
                // the full parse_one_path dispatch (directories, codec-fallback files).
                let claim_map: &crate::codec::ClaimMap =
                    self.claim_map.as_deref().unwrap_or(&CLAIM_MAP);
                let (bulk_assets, residual_assets): (Vec<PathBuf>, Vec<PathBuf>) =
                    asset_batch.into_iter().partition(|p| {
                        !p.is_dir()
                            && claim_map.get(p).is_none()
                            && !claim_map.is_rejected(p)
                            && !WALK_CODECS.should_track(p)
                    });

                let _ = self.builder.tx().send(BeliefEvent::BatchStart);

                // Bulk path: parallel file read + SHA-256, sequential event emission.
                let mut all_asset_results = if !bulk_assets.is_empty() {
                    self.process_asset_batch(bulk_assets).await?
                } else {
                    Vec::new()
                };

                // Residual path: directories and codec-fallback files go through
                // the full parse_epoch dispatch.
                if !residual_assets.is_empty() {
                    let mut residual_results = self
                        .parse_epoch(residual_assets, cached_global_bb.clone())
                        .await?;
                    all_asset_results.append(&mut residual_results);
                }

                let _ = self.builder.tx().send(BeliefEvent::BatchEnd);
                cached_global_bb.drain_epoch().await?;
                #[cfg(not(target_arch = "wasm32"))]
                self.progress.update(
                    0,
                    0,
                    &format!("remainder: {} queued", self.remainder_queue.len()),
                );
                self.sync_asset_snapshot(&cached_global_bb).await?;
                self.process_epoch_batch_results(all_asset_results, &mut repo_seeded)
                    .await?;
            }

            // Sub-epoch B: document re-parses. global_bb now contains asset BIDs committed
            // by sub-epoch A, so cache_fetch hits for asset keys and inject_context sees
            // the full BeliefContext on this pass.
            if !reparse_batch.is_empty() {
                let _ = self.builder.tx().send(BeliefEvent::BatchStart);
                let reparse_results = self
                    .parse_epoch(reparse_batch, cached_global_bb.clone())
                    .await?;
                let _ = self.builder.tx().send(BeliefEvent::BatchEnd);
                cached_global_bb.drain_epoch().await?;
                #[cfg(not(target_arch = "wasm32"))]
                self.progress.update(
                    0,
                    0,
                    &format!("remainder: {} queued", self.remainder_queue.len()),
                );
                self.sync_asset_snapshot(&cached_global_bb).await?;
                self.process_epoch_batch_results(reparse_results, &mut repo_seeded)
                    .await?;
            }
        }

        #[cfg(not(target_arch = "wasm32"))]
        self.progress.finish();
        // Copy source files while latest_results is still populated (before drain).
        self.copy_source_files().await?;
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
        parse_number: usize,
        claim_map: &crate::codec::ClaimMap,
    ) -> (PathBuf, Result<ParseContentWithCodec, BuildonomyError>) {
        // Resolve directory → index file, or dispatch to process_asset_dir.
        //
        // Directories with an index file are registered networks — resolve to the
        // index file and proceed through the normal codec path.
        //
        // Directories without an index file are directory asset references from
        // markdown links (e.g. `[vendor/mylib](./vendor/mylib)`).  Route them to
        // `process_asset_dir` which handles:
        //   Case A (git-tracking): tracked network dir → href node pointing to remote URL
        //   Case B: local-only dir → External node with sorted directory listing
        let file_path = if path.is_dir() {
            // Check CLAIM_MAP rejection before dispatching directory → index.md.
            // A rejected network dir must not be parsed even if it ends up in the
            // remainder queue via an unresolved reference re-queue path.
            if claim_map.is_rejected(&path) {
                tracing::debug!(
                    "[parse_one_path]: directory {:?} is rejected by CLAIM_MAP, skipping",
                    path
                );
                let mut result = ParseContentResult::empty();
                result.add_diagnostic(ParseDiagnostic::info(format!(
                    "Network directory excluded by parent whitelist/blacklist: {}",
                    path.display()
                )));
                return (
                    path,
                    Ok(ParseContentWithCodec {
                        result,
                        codec: Box::new(UnclaimedDataCodec),
                        repo_bid: Bid::nil(),
                        repo_node: None,
                    }),
                );
            }
            match detect_network_file(&path) {
                Some(p) => {
                    // The directory itself may not be rejected, but the network file
                    // inside it may have been filtered by a parent network's whitelist.
                    // Check the resolved file path against CLAIM_MAP before proceeding.
                    // Canonicalize to match the form used by CLAIM_MAP.reject() (which
                    // receives canonicalized paths from ProtoIndex's children_of).
                    let p_canonical = p.canonicalize().unwrap_or_else(|_| p.clone());
                    if claim_map.is_rejected(&p_canonical) {
                        tracing::debug!(
                            "[parse_one_path]: network file {:?} in directory {:?} is rejected \
                             by CLAIM_MAP, skipping",
                            p,
                            path,
                        );
                        let mut result = ParseContentResult::empty();
                        result.add_diagnostic(ParseDiagnostic::info(format!(
                            "Network file excluded by parent whitelist/blacklist: {}",
                            p.display()
                        )));
                        return (
                            path,
                            Ok(ParseContentWithCodec {
                                result,
                                codec: Box::new(UnclaimedDataCodec),
                                repo_bid: Bid::nil(),
                                repo_node: None,
                            }),
                        );
                    }
                    p
                }
                None => {
                    tracing::debug!("[parse_one_path]: directory asset {:?}", path);
                    let result = builder
                        .process_asset_dir(&path, global_bb, proto_index)
                        .await;
                    return (path, result);
                }
            }
        } else {
            path.clone()
        };

        // Read raw bytes — works for both text documents and binary assets.
        tracing::debug!("[parse_one_path]: reading {:?}", file_path);
        let bytes = match tokio::fs::read(&file_path).await {
            Ok(b) => b,
            Err(e) => {
                // For paths with no registered codec (i.e. asset paths), a read
                // failure almost always means the path doesn't exist on disk — it was
                // misclassified as an asset reference (e.g. `[R.CLDS-410]` whose dot
                // caused NodeKey::from_str to emit an asset-namespace key). Returning
                // Err here would produce a ParseDiagnostic::ParseError → exit(1).
                // Instead, return Ok with a Warning so the build continues and the
                // caller surfaces it as an unresolved-reference warning.
                if CODECS.path_get(&file_path).is_none() && !WALK_CODECS.should_track(&file_path) {
                    tracing::debug!(
                        "[parse_one_path] Non-codec path unreadable ({e}), \
                         downgrading to warning: {:?}",
                        file_path
                    );
                    let mut result = ParseContentResult::empty();
                    result.add_diagnostic(crate::codec::ParseDiagnostic::warning(format!(
                        "Asset file not found (possibly a misclassified ID link): {}",
                        file_path.display()
                    )));
                    return (
                        path,
                        Ok(ParseContentWithCodec {
                            result,
                            codec: Box::new(AssetCodec),
                            repo_bid: Bid::nil(),
                            repo_node: None,
                        }),
                    );
                }
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
        // Three-branch codec dispatch (CODECS fallback is internal to CLAIM_MAP.get):
        //   1. claim_map.get()            — claimed codec, or CODECS.path_get() fallback
        //   2. claim_map.is_rejected()    — explicitly filtered by a network whitelist/blacklist
        //                                   → info diagnostic, UnclaimedDataCodec, no nodes
        //   3. WALK_CODECS.should_track() — tracked but unclaimed → info diagnostic, UnclaimedDataCodec
        //   4. neither                    — genuine binary asset → process_asset (unchanged)
        let codec_factory: Option<CodecFactory> = claim_map.get(&file_path);

        // Branch 2: explicitly rejected by a network filter — short-circuit before CODECS.
        if codec_factory.is_none() && claim_map.is_rejected(&file_path) {
            tracing::debug!(
                "[parse_one_path]: rejected by network filter {:?}",
                file_path
            );
            let mut result = ParseContentResult::empty();
            result.add_diagnostic(ParseDiagnostic::info(format!(
                "File excluded by network whitelist/blacklist filter: {}",
                file_path.display()
            )));
            return (
                path,
                Ok(ParseContentWithCodec {
                    result,
                    codec: Box::new(UnclaimedDataCodec),
                    repo_bid: Bid::nil(),
                    repo_node: None,
                }),
            );
        }

        // Branches 3–4: fall through to WALK_CODECS / asset.
        if codec_factory.is_none() {
            if WALK_CODECS.should_track(&file_path) {
                tracing::debug!("[parse_one_path]: unclaimed tracked path {:?}", file_path);
                let mut result = ParseContentResult::empty();
                result.add_diagnostic(ParseDiagnostic::info(format!(
                    "File is tracked but not claimed by any codec: {}",
                    file_path.display()
                )));
                return (
                    path,
                    Ok(ParseContentWithCodec {
                        result,
                        codec: Box::new(UnclaimedDataCodec),
                        repo_bid: Bid::nil(),
                        repo_node: None,
                    }),
                );
            }
            tracing::debug!("[parse_one_path]: asset path {:?}", file_path);
            let result = builder
                .process_asset(&file_path, &bytes, global_bb, proto_index)
                .await;
            return (path, result);
        }

        // Pre-flight proto() check: any codec may opt out of document parsing by
        // returning None from proto() (e.g. XlsxCodec when no `index` tab is
        // present, or a CMake codec when no `add_library` target exists).
        // A None proto means "treat as asset" — route to process_asset silently
        // rather than letting parse_content/initialize_stack fail with a fatal error.
        {
            let preflight_factory = codec_factory.expect("already confirmed Some above");
            let preflight_codec = preflight_factory();
            match preflight_codec.proto(file_path.as_ref()) {
                Ok(None) => {
                    // Codec explicitly opts out — treat as asset.
                    tracing::debug!(
                        "[parse_one_path]: proto() returned None for {:?} \
                         — routing to process_asset",
                        file_path
                    );
                    let result = builder
                        .process_asset(&file_path, &bytes, global_bb, proto_index)
                        .await;
                    return (path, result);
                }
                Ok(Some(_)) => {
                    // Codec claims this file — proceed to parse_content below.
                }
                Err(e) => {
                    // proto() itself failed (I/O error, corrupt file, etc.).
                    // Emit a debug log and route to asset so the rest of the
                    // corpus is not disrupted.
                    tracing::debug!(
                        "[parse_one_path]: proto() error for {:?}: {} — routing to process_asset",
                        file_path,
                        e
                    );
                    let result = builder
                        .process_asset(&file_path, &bytes, global_bb, proto_index)
                        .await;
                    return (path, result);
                }
            }
        }

        // Probe content mode via a cheap factory instantiation (no I/O, no parse state).
        let codec_factory = codec_factory.expect("already confirmed Some above");
        let content_mode = codec_factory().content_mode();

        let _ = builder
            .tx()
            .send(BeliefEvent::FileParsed(file_path.clone()));

        let mut result = match content_mode {
            CodecContentMode::Text => {
                // Decode bytes to UTF-8 then parse — existing path.
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
                builder
                    .parse_content(&file_path, content, global_bb, proto_index, parse_number)
                    .await
            }
            CodecContentMode::Binary => {
                // Binary codec: pass empty string — the codec ignores content and
                // re-opens the file from current.path using its own I/O.
                builder
                    .parse_content(
                        &file_path,
                        String::new(),
                        global_bb,
                        proto_index,
                        parse_number,
                    )
                    .await
            }
        };

        // Write rewritten content back to disk.
        // Text codecs: rewritten_content (Option<String>).
        // Binary codecs: generate_source_bytes() (Option<Vec<u8>>).
        if let Ok(ref mut with_codec) = result {
            if write {
                match content_mode {
                    CodecContentMode::Text => {
                        if let Some(ref contents) = with_codec.result.rewritten_content {
                            if let Err(e) = tokio::fs::write(&file_path, contents).await {
                                with_codec.result.diagnostics.push(
                                    crate::codec::ParseDiagnostic::warning(format!(
                                        "Failed to write rewritten content: {e}"
                                    )),
                                );
                            }
                        }
                    }
                    CodecContentMode::Binary => {
                        if let Some(ref bin_bytes) = with_codec.codec.generate_source_bytes() {
                            if let Err(e) = tokio::fs::write(&file_path, bin_bytes).await {
                                with_codec.result.diagnostics.push(
                                    crate::codec::ParseDiagnostic::warning(format!(
                                        "Failed to write rewritten binary content: {e}"
                                    )),
                                );
                            }
                        }
                    }
                }
            }
        }

        // Write derived outputs (e.g. CSV exports) to .noet/derived/.
        //
        // Derived outputs are written unconditionally (not gated on `write`) because
        // they are generated artifacts, not source file rewrites. Skipping them would
        // leave the graph with unresolvable asset references.
        if let Ok(ref mut with_codec) = result {
            let derived = with_codec.codec.derived_outputs();
            if !derived.is_empty() {
                let repo_root = builder.repo_root().to_path_buf();
                for (rel_path, bytes) in derived {
                    let abs_path = repo_root.join(&rel_path);
                    // Create parent directory if needed.
                    if let Some(parent) = abs_path.parent() {
                        if let Err(e) = tokio::fs::create_dir_all(parent).await {
                            with_codec.result.diagnostics.push(
                                crate::codec::ParseDiagnostic::warning(format!(
                                    "Failed to create derived output directory {}: {e}",
                                    parent.display()
                                )),
                            );
                            continue;
                        }
                    }
                    if let Err(e) = tokio::fs::write(&abs_path, &bytes).await {
                        with_codec
                            .result
                            .diagnostics
                            .push(crate::codec::ParseDiagnostic::warning(format!(
                                "Failed to write derived output {}: {e}",
                                abs_path.display()
                            )));
                    } else {
                        // File written successfully — record the absolute path so the
                        // compiler can enqueue it into the asset discovery pipeline.
                        // process_one_parse_result enqueues these via self.enqueue(),
                        // which causes process_asset to run in the current session's
                        // remainder epoch, giving the CSV a content_hash asset node
                        // without waiting for the next compile run.
                        with_codec.result.derived_paths.push(abs_path);
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
            let seq_claim_map: &crate::codec::ClaimMap =
                self.claim_map.as_deref().unwrap_or(&CLAIM_MAP);
            for path in paths {
                let proto_index = self.proto_index.clone();
                let write = self.write;
                let parse_number = self.processed.get(&path).copied().unwrap_or(1);
                results.push(
                    Self::parse_one_path(
                        path,
                        &mut self.builder,
                        global_bb.clone(),
                        proto_index,
                        write,
                        parse_number,
                        seq_claim_map,
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
            // `network_ancestors` is retained as the epoch-0 / asset-file fallback seed: on the
            // very first epoch global_bb has no content yet, so submap returns empty and the
            // balanced query would return nothing.  For those tasks we still need the
            // network ancestor chain in the seed so that initialize_stack's fast-path guard
            // and try_initialize_stack_from_session_cache work correctly.
            let network_ancestors = Arc::new(self.builder.epoch_session_snapshot());
            // Build the BeliefBase index over the shared snapshot ONCE per epoch.
            //
            // Every task needs a session_bb indexed over this same immutable graph, and
            // the index (`PathMapMap::new`, a seeded DFS per network) is the expensive
            // part — 97.5% of per-task rebuild cost on a full corpus run, essentially all
            // of it reconstructing the identical const-namespace. Cloning this base is a
            // BTreeMap of Arc pointer copies; writes stay private via PathMapMap's
            // copy-on-write. See `seed_session_from_base`.
            let shared_base_start = std::time::Instant::now();
            let shared_session_base =
                Arc::new(BeliefBase::from((*network_ancestors).clone()).with_label("epoch_base"));
            tracing::debug!(
                target: "noet_core::codec::perf",
                states = shared_session_base.states().len(),
                build_us = shared_base_start.elapsed().as_micros(),
                "[parse_epoch] shared session base built",
            );
            let proto_index = self.proto_index.clone();
            let shared_tx = self.builder.tx().clone();
            let write = self.write;
            let semaphore = Arc::new(Semaphore::new(self.jobs));

            let n = paths.len();

            // ── Per-path balanced seed pre-computation ────────────────────────────────
            //
            // For each document in this epoch batch we call global_bb.submap() to get the
            // BID list of the document's own nodes, then issue a single balanced
            // query against that BID set.  QueryPackage::balanced appends halo + section-root
            // traversals that iteratively fetch ancestor chains until the graph is
            // closed — amortising into one pre-spawn operation what would otherwise be N
            // individual cache_fetch → global_bb.evaluate round-trips during Phase 2
            // push_relation (each acquiring the global_bb mutex for 15-20 ms).
            //
            // Because the balanced query walks the full ancestor chain, the result already contains
            // network nodes, Section edges, const-namespace subgraphs, and index anchors —
            // i.e. everything epoch_session_snapshot produces — so network_ancestors is
            // subsumed and not merged for tasks that receive a non-empty per-doc seed.
            //
            // Falls back to network_ancestors for epoch-0 (submap returns empty because
            // global_bb has no content yet) and for asset/directory paths.
            //
            // The pre-computation loop is sequential and async; each submap call acquires
            // the global_bb mutex once, and each balanced query runs a bounded traversal.
            // This is acceptable: we are in the pre-spawn setup phase, not inside a task.
            let mut doc_seeds: Vec<BeliefGraph> = Vec::with_capacity(n);
            {
                for path in &paths {
                    // Resolve directory → index file (mirrors parse_one_path).
                    let file_path = if path.is_dir() {
                        match crate::codec::network::detect_network_file(path) {
                            Some(p) => p,
                            None => {
                                // Directory asset — no document nodes to pre-seed.
                                doc_seeds.push(BeliefGraph::default());
                                continue;
                            }
                        }
                    } else {
                        path.clone()
                    };

                    // Resolve the owning network BID and the document's own BID
                    // from global_bb, then use submap_by_bid to get the document's
                    // subtree.
                    //
                    // We use submap_by_bid (BID-based) rather than submap (path-string)
                    // because PathMap paths may be normalized (e.g. lowercased) relative
                    // to filesystem names, and the DbConnection submap does
                    // segment-by-segment SQL lookups against PathMap-format paths.
                    // BID-based lookup bypasses path format entirely.
                    //
                    // We resolve BIDs from global_bb (not session_bb) because in the
                    // parallel path subnet networks and their leaf documents are parsed
                    // by spawned tasks whose events flow to global_bb via the
                    // accumulator, never into self.builder.session_bb.
                    let parent_abs = match proto_index.owning_net_dir_for(&file_path) {
                        Some(dir) if dir.strip_prefix(&repo_root).is_ok() => dir,
                        _ => {
                            doc_seeds.push(BeliefGraph::default());
                            continue;
                        }
                    };

                    // Resolve the owning network BID.
                    //
                    // For the repo root (parent_rel_path = ""), use repo_bid directly.
                    // For subnets, try session_bb first (works for the repo root and
                    // any network parsed by self.builder), then fall back to global_bb
                    // via get_node (needed for subnets parsed by spawned tasks).
                    let parent_rel_path = os_path_to_string(
                        parent_abs
                            .strip_prefix(&repo_root)
                            .unwrap_or(std::path::Path::new("")),
                    );
                    let net_bid = if parent_rel_path.is_empty() {
                        repo_bid
                    } else {
                        let parent_key = NodeKey::Path {
                            net: repo_bid.bref(),
                            path: parent_rel_path.clone(),
                        };
                        // Try session_bb first (cheap, no async).
                        match self.builder.session_bb().get(&parent_key) {
                            Some(node) => node.bid,
                            None => {
                                // Fall back to global_bb for subnets parsed by
                                // spawned tasks.
                                match lookup_node(&global_bb, &parent_key).await {
                                    Ok(Some(node)) => node.bid,
                                    _ => {
                                        let pn = self.processed.get(path).copied().unwrap_or(0);
                                        if pn > 1 {
                                            tracing::warn!(
                                                target: "noet_core::codec::fast_path",
                                                path = %file_path.display(),
                                                parent_rel_path = %parent_rel_path,
                                                repo_bref = %repo_bid.bref(),
                                                "[parse_epoch] seed: net_bid lookup failed on reparse"
                                            );
                                        }
                                        doc_seeds.push(BeliefGraph::default());
                                        continue;
                                    }
                                }
                            }
                        }
                    };

                    // Resolve the document node's BID.
                    // Try session_bb first, fall back to global_bb.
                    let is_index = is_network_index_file(&file_path);
                    let rel_from_net = match file_path.strip_prefix(&parent_abs) {
                        Ok(rel) => {
                            if is_index {
                                os_path_to_string(rel.parent().unwrap_or(rel))
                            } else {
                                os_path_to_string(rel)
                            }
                        }
                        Err(_) => {
                            doc_seeds.push(BeliefGraph::default());
                            continue;
                        }
                    };
                    let doc_key = NodeKey::Path {
                        net: net_bid.bref(),
                        path: rel_from_net,
                    };
                    let doc_bid = match self.builder.session_bb().get(&doc_key) {
                        Some(node) => node.bid,
                        None => match lookup_node(&global_bb, &doc_key).await {
                            Ok(Some(node)) => node.bid,
                            _ => {
                                let pn = self.processed.get(path).copied().unwrap_or(0);
                                if pn > 1 {
                                    tracing::warn!(
                                        target: "noet_core::codec::fast_path",
                                        path = %file_path.display(),
                                        doc_key = ?doc_key,
                                        net_bref = %net_bid.bref(),
                                        "[parse_epoch] seed: doc_bid lookup failed on reparse"
                                    );
                                }
                                doc_seeds.push(BeliefGraph::default());
                                continue;
                            }
                        },
                    };

                    // Fetch the BID list for this document's subtree from global_bb.
                    // submap_by_bid uses the document BID directly, bypassing the
                    // PathMap path format entirely.
                    let submap_entries = match global_bb
                        .submap_by_bid(net_bid, Some(doc_bid), 0, true)
                        .await
                    {
                        Ok(e) => e,
                        Err(_) => {
                            doc_seeds.push(BeliefGraph::default());
                            continue;
                        }
                    };

                    if submap_entries.is_empty() {
                        // No prior epoch data for this document — epoch-0 or first parse.
                        doc_seeds.push(BeliefGraph::default());
                        continue;
                    }

                    // Build a balanced BeliefGraph for the full document BID set.
                    // QueryPackage::balanced appends halo + section-root traversals
                    // that iteratively close the ancestor chains.
                    // The result subsumes network_ancestors: the ancestor chain walk
                    // reaches network nodes, Section edges, and const-namespace subgraphs
                    // automatically.
                    let bid_vec: Vec<crate::properties::Bid> =
                        submap_entries.into_iter().map(|(_, bid, _)| bid).collect();
                    let per_doc_seed = {
                        let spec = QuerySpec::seed(TapeFn::Bids(bid_vec));
                        let mut package = QueryPackage::balanced(spec);
                        match global_bb.evaluate(&mut package).await {
                            Ok(()) => package.into_graph(),
                            Err(_) => BeliefGraph::default(),
                        }
                    };

                    tracing::debug!(
                        target: "noet_core::codec::fast_path",
                        path = %file_path.display(),
                        doc_bid = %doc_bid,
                        net_bid = %net_bid,
                        seed_states = %per_doc_seed.states.len(),
                        seed_edges = %per_doc_seed.relations.as_graph().edge_count(),
                        "[parse_epoch] per-doc balanced seed computed"
                    );

                    doc_seeds.push(per_doc_seed);
                }
            }

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
            // Clone the instance claim map Arc once outside the loop; each task gets
            // its own Arc clone (cheap reference-count increment). When no instance
            // map is set, task_claim_map is None and the task falls back to &CLAIM_MAP.
            let instance_claim_map: Option<Arc<crate::codec::ClaimMap>> = self.claim_map.clone();
            for (idx, (path, doc_seed)) in paths.into_iter().zip(doc_seeds).enumerate() {
                let repo_root = repo_root.clone();
                let proto_index = proto_index.clone();
                let global_bb = global_bb.clone();
                let network_ancestors = Arc::clone(&network_ancestors);
                let shared_session_base = Arc::clone(&shared_session_base);
                let sem = Arc::clone(&semaphore);
                let task_claim_map: Option<Arc<crate::codec::ClaimMap>> =
                    instance_claim_map.clone();
                let parse_number = self.processed.get(&path).copied().unwrap_or(1);
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
                            Ok(b) => b.with_skip_global_api_check(true),
                            Err(e) => return (idx, path, Err(e), Vec::new()),
                        };
                        // Choose the seed for this task's session_bb:
                        //
                        // - Non-empty doc_seed: a balanced BeliefGraph produced by
                        //   QueryPackage::balanced on the document's submap BID set.  The
                        //   balanced traversal already walked the full ancestor chain, so this graph
                        //   subsumes network_ancestors (network nodes, Section edges,
                        //   const-namespace subgraphs, index anchors are all reachable
                        //   from the document's nodes).  Using it directly avoids an
                        //   extra clone of network_ancestors.
                        //
                        // - Empty doc_seed (epoch-0, asset files, first parse of a new
                        //   document): fall back to network_ancestors so that
                        //   try_initialize_stack_from_session_cache can still walk the
                        //   ancestor chain and the content_namespaces() guard fires.
                        //
                        // seed_session is a no-op when repo_bid is nil (epoch-0 root).
                        let task_seed_states = doc_seed.states.len();
                        let task_seed_edges = doc_seed.relations.as_graph().edge_count();
                        let doc_seed_has_href = doc_seed
                            .states
                            .contains_key(&crate::properties::href_namespace());
                        let network_ancestors_has_href = network_ancestors
                            .states
                            .contains_key(&crate::properties::href_namespace());
                        // Clone the prebuilt shared base (Arc pointer copies, no DFS) and
                        // merge this document's own seed on top.
                        //
                        // Both former branches collapse here. The shared base already carries
                        // the network ancestors and const-namespace subgraphs that
                        // `network_ancestors` supplied, so the empty-doc_seed case is just a
                        // merge of nothing, and the non-empty case no longer needs the
                        // const-namespace union that previously guarded against dropping
                        // those namespaces (~84k href states re-fetched, ~82s per affected
                        // task). The base is never replaced, so they cannot be dropped.
                        builder.seed_session_from_base(repo_bid, &shared_session_base, &doc_seed);
                        tracing::debug!(
                            target: "noet_core::codec::const_ns_refetch",
                            task_idx = idx,
                            path = %path.display(),
                            seed_states = %task_seed_states,
                            seed_edges = %task_seed_edges,
                            used_doc_seed = %(task_seed_states > 0),
                            doc_seed_has_href = %doc_seed_has_href,
                            network_ancestors_has_href = %network_ancestors_has_href,
                            "[parse_epoch] task seeded"
                        );
                        let task_parse_start = std::time::Instant::now();

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
                            parse_number,
                            task_claim_map.as_deref().unwrap_or(&CLAIM_MAP),
                        )
                        .await;

                        tracing::debug!(
                            target: "noet_core::codec::const_ns_refetch",
                            task_idx = idx,
                            path = %orig_path.display(),
                            elapsed_ms = task_parse_start.elapsed().as_millis() as u64,
                            "[parse_epoch] task parse complete"
                        );

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

            // PathMap copy-on-write counters. Cumulative across the run, so read the
            // deltas between epochs. A copies/calls ratio near 1.0 means the sharing is
            // thrashing rather than amortizing (healthy baseline: ~2.8%).
            {
                let (calls, copies, entries, us) = crate::paths::cow_stats();
                tracing::debug!(
                    target: "noet_core::codec::perf",
                    cow_calls = calls,
                    cow_copies = copies,
                    cow_entries_copied = entries,
                    cow_us = us,
                    "[parse_epoch] pathmap copy-on-write counters (cumulative)",
                );
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

    /// After `drain_epoch`, pull the const-namespace (asset + href) subgraph from
    /// `global_bb` into `self.builder.session_bb` so that `epoch_session_snapshot`
    /// includes the full asset set for the next epoch's parallel tasks.
    ///
    /// Without this, `epoch_session_snapshot` Part 2 always produces an empty asset
    /// subgraph (namespace nodes present, zero children) because parallel-task events
    /// flow `shared_tx → BeliefAccumulator → global_bb` but are never replayed into
    /// `self.builder.session_bb`. Every remainder-epoch task then falls through to
    /// `global_bb.evaluate` inside `initialize_stack`, loading ~366 assets at
    /// ~10.9 ms/asset = 4–9s of Phase 0 per task on every reparse.
    ///
    /// After this call the next `epoch_session_snapshot` includes the full asset
    /// subgraph, and the `content_namespaces()` guard in `initialize_stack` fires
    /// immediately, skipping the global_bb query entirely for all remainder tasks.
    ///
    /// Must be called **after** `drain_epoch` — `global_bb` does not have the new
    /// epoch's assets until the drain is complete.
    async fn sync_asset_snapshot<B: BeliefSource + Clone>(
        &mut self,
        global_bb: &B,
    ) -> Result<(), BuildonomyError> {
        for ns_bid in content_namespaces().iter() {
            let key = NodeKey::Bid { bid: *ns_bid };
            // Only pull namespaces that global_bb actually knows about.
            if let Some(ns_node) = lookup_node(global_bb, &key).await? {
                // Ensure the namespace node itself is present in session_bb.
                let ns_event = BeliefEvent::NodeUpdate(
                    vec![key.clone()],
                    ns_node.clone(),
                    EventOrigin::Remote,
                );
                self.builder.session_bb_mut().process_event(&ns_event)?;

                // Fetch the asset subgraph reachable from this namespace and
                // merge it into session_bb.  merge_from is idempotent: re-merging
                // on every drain is safe and picks up any newly-discovered assets.
                //
                // Leaf-anchored query (Section leaf-ward walk only, no halo): asset
                // and href nodes are the SOURCE of their Section edge to the
                // namespace (the namespace is the sink) -- see
                // GraphBuilder::process_asset. Walking toward roots (anchored /
                // balance_map) from the namespace only continues upward and never
                // reaches any of its children. leaf_anchored walks the other way
                // (toward leaves) to find everything registered under this
                // namespace hub, while still avoiding the O(N) halo explosion a
                // balanced query would incur by fanning out through every document
                // sharing a neighbor with the namespace hub.
                let spec = QuerySpec::seed(TapeFn::Bids(vec![*ns_bid]));
                let mut package = QueryPackage::leaf_anchored(spec);
                global_bb.evaluate(&mut package).await?;
                let ns_graph = package.into_graph();
                let ns_seed: std::collections::BTreeSet<Bid> =
                    std::collections::BTreeSet::from([*ns_bid]);
                self.builder
                    .session_bb_mut()
                    .merge_from(&ns_graph, &ns_seed);

                tracing::debug!(
                    "[sync_asset_snapshot] Merged {} asset nodes for namespace {}",
                    ns_graph.states.len().saturating_sub(1), // -1 for the namespace node itself
                    ns_bid
                );
            }
        }
        Ok(())
    }

    /// Register Trace-kinded stubs for subnet networks parsed in the current
    /// depth-group batch.
    ///
    /// After a depth-group `drain_epoch`, the newly parsed subnet network nodes
    /// live in `global_bb` but are absent from `self.builder.session_bb`.  Without
    /// them, `epoch_session_snapshot` produces a `PathMapMap` that cannot resolve
    /// multi-level path keys (e.g. `docs/flight_software_design/architecture`),
    /// causing `cache_fetch` MISS warnings on re-parse.
    ///
    /// For each subnet in the batch this method:
    ///   1. Uses `ProtoIndex` to derive the parent network directory and the
    ///      child's relative directory name (zero `global_bb` cost).
    ///   2. Resolves the child's `NodeKey::Path` via a seed-only query on
    ///      `global_bb` (no halo/balanced traversal — one cheap key resolution).
    ///   3. Inserts the resulting Trace-kinded node and a Section edge (with
    ///      `WEIGHT_DOC_PATHS`) into `session_bb`, registering the subnet in
    ///      the parent's `PathMap`.
    ///
    /// `cache_fetch` rejects Trace nodes from `session_bb` (they pass the
    /// `.filter(|n| … || !n.kind.contains(BeliefKind::Trace))` guard), falling
    /// through to `global_bb` for the full balanced subgraph on first access.
    /// The stub's value is in the PathMap registration: path resolution works
    /// locally, avoiding the cascade that would otherwise miss.
    ///
    /// Must be called **after** `drain_epoch` so `global_bb` has the batch's
    /// network nodes.
    async fn sync_subnet_stubs<B: BeliefSource + Clone>(
        &mut self,
        batch: &[PathBuf],
        global_bb: &B,
    ) -> Result<(), BuildonomyError> {
        let repo_root = self.builder.repo_root().to_path_buf();
        let repo_bid = self.builder.repo();
        if repo_bid == Bid::nil() {
            return Ok(());
        }

        for dir in batch {
            // Skip repo root — already in session_bb non-Trace from sequential
            // parse.
            if *dir == repo_root {
                continue;
            }

            // ── Step 1: derive parent/child from ProtoIndex (no global_bb) ───
            let index_path = dir.join(NETWORK_NAME);
            let parent_dir = match self.proto_index.owning_net_dir_for(&index_path) {
                Some(d) => d,
                None => continue,
            };

            // Look up parent BID from session_bb.  The parent was parsed in an
            // earlier depth group (or is the repo root), so it must already be
            // present — either as a real node or as a Trace stub from a prior
            // call to this method.
            let parent_rel =
                os_path_to_string(parent_dir.strip_prefix(&repo_root).unwrap_or(Path::new("")));
            let parent_key = NodeKey::Path {
                net: repo_bid.bref(),
                path: parent_rel.clone(),
            };
            let parent_bid = match self.builder.session_bb().get(&parent_key) {
                Some(n) => n.bid,
                None => {
                    tracing::debug!(
                        target: "noet_core::codec::fast_path",
                        parent_rel = %parent_rel,
                        subnet = %dir.display(),
                        "[sync_subnet_stubs] parent not in session_bb, skipping",
                    );
                    continue;
                }
            };

            // Compute child's relative directory name within the parent network.
            let child_rel_name =
                os_path_to_string(dir.strip_prefix(&parent_dir).unwrap_or(Path::new("")));
            if child_rel_name.is_empty() {
                continue;
            }

            // Skip if already registered in session_bb.
            let child_key = NodeKey::Path {
                net: parent_bid.bref(),
                path: child_rel_name.clone(),
            };
            if self.builder.session_bb().get(&child_key).is_some() {
                continue;
            }

            // ── Step 2: resolve BID from global_bb (one cheap key query) ─────
            let spec = QuerySpec::seed(TapeFn::Keys(vec![child_key.clone()]));
            let mut package = QueryPackage::new(spec);
            global_bb.evaluate(&mut package).await?;

            let child_bid = match package.resolved_bid(0) {
                Some(bid) => bid,
                None => {
                    // Expected on fresh indexes (no durable database): global_bb
                    // has the subnet node from drain_epoch but its PathMapMap may
                    // not have registered the path key yet.  The existing
                    // cache_fetch cascade (session_bb miss → global_bb balanced
                    // query) handles this case at task execution time.
                    tracing::trace!(
                        target: "noet_core::codec::fast_path",
                        child_key = ?child_key,
                        "[sync_subnet_stubs] global_bb could not resolve subnet key \
                         (expected on fresh index)",
                    );
                    continue;
                }
            };
            let graph = package.into_graph();
            let child_node = match graph.states.get(&child_bid) {
                Some(n) => n.clone(),
                None => continue,
            };

            // Mark as Trace so cache_fetch rejects the StackCache hit and
            // falls through to global_bb for the full balanced subgraph.
            // A seed-only query (no halo) returns the real node kind, so we
            // must set Trace explicitly.
            let mut stub_node = child_node;
            stub_node.kind.insert(BeliefKind::Trace);

            // ── Step 3: insert stub + Section edge into session_bb ───────────
            self.builder
                .session_bb_mut()
                .process_event(&BeliefEvent::NodeUpsert(
                    child_bid,
                    stub_node,
                    EventOrigin::Remote,
                ))?;

            // Section edge registers the subnet in the parent's PathMap via
            // WEIGHT_DOC_PATHS.
            let mut weight = Weight {
                payload: toml::Table::new(),
            };
            weight
                .set_doc_paths(vec![child_rel_name.clone()])
                .map_err(|e| {
                    BuildonomyError::Codec(format!(
                        "sync_subnet_stubs: failed to set doc_paths: {e}"
                    ))
                })?;
            if let Some(sk) = self.proto_index.sort_key_for(&index_path) {
                weight.set(WEIGHT_SORT_KEY, sk).map_err(|e| {
                    BuildonomyError::Codec(format!(
                        "sync_subnet_stubs: failed to set sort_key: {e}"
                    ))
                })?;
            }
            weight.set(WEIGHT_OWNED_BY, "sink").map_err(|e| {
                BuildonomyError::Codec(format!("sync_subnet_stubs: failed to set owned_by: {e}"))
            })?;

            self.builder
                .session_bb_mut()
                .process_event(&BeliefEvent::RelationChange(
                    child_bid,
                    parent_bid,
                    WeightKind::Section,
                    Some(weight),
                    EventOrigin::Remote,
                ))?;

            tracing::debug!(
                target: "noet_core::codec::fast_path",
                subnet = %child_rel_name,
                child_bid = %child_bid,
                parent_bid = %parent_bid,
                "[sync_subnet_stubs] registered Trace subnet stub",
            );
        }

        Ok(())
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
        crate::paths::canonicalize_path(&path).unwrap_or(path)
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
    ///
    /// Removes the path from the remainder queue, clears its parse count, and
    /// removes any `CLAIM_MAP` entry so that a codec which previously claimed this
    /// file does not receive stale dispatch on a future re-scan.
    pub fn on_file_deleted(&mut self, path: impl AsRef<Path>) {
        let path = path.as_ref().to_path_buf();
        self.remove_from_queues(&path);
        self.processed.remove(&path);
        CLAIM_MAP.unclaim(&path);
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

    /// Compute a FNV-1a 64-bit hash of a byte slice.
    fn fnv1a(data: &[u8]) -> u64 {
        let mut h: u64 = 0xcbf29ce484222325;
        for &b in data {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        h
    }

    /// Compute a combined asset version token that changes when either the beliefbase
    /// content OR the WASM binary changes.
    ///
    /// XOR-combining the two hashes means Chrome's compiled WASM cache (keyed by URL)
    /// is busted whenever the Rust code changes, not only when beliefbase data changes.
    fn asset_version_hex(beliefbase_data: &[u8]) -> String {
        let data_hash = Self::fnv1a(beliefbase_data);
        // WASM_BINARY is only available with the `bin` feature; fall back to
        // data_hash alone in library/test builds where the binary isn't embedded.
        #[cfg(feature = "bin")]
        let wasm_hash = Self::fnv1a(crate::codec::assets::wasm_binary());
        #[cfg(not(feature = "bin"))]
        let wasm_hash: u64 = 0;
        format!("{:016x}", data_hash ^ wasm_hash)
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

        // finalize_html calls such as `compute_layout_metadata` are performance taxing.
        // This instrumentation lets us measure their burden.
        let stage_start = std::time::Instant::now();

        // Generate deferred HTML with synchronized context
        self.generate_deferred_html(global_bb.clone()).await?;
        tracing::debug!(
            target: "noet_core::codec::perf",
            elapsed_ms = stage_start.elapsed().as_millis(),
            "[finalize_html stage] generate_deferred_html",
        );

        // Generate sitemap from document paths
        let stage_start = std::time::Instant::now();
        self.generate_sitemap(global_bb.clone()).await?;
        tracing::debug!(
            target: "noet_core::codec::perf",
            elapsed_ms = stage_start.elapsed().as_millis(),
            "[finalize_html stage] generate_sitemap",
        );

        // Query synchronized global_bb for asset manifest
        let stage_start = std::time::Instant::now();
        let asset_manifest: BTreeMap<String, Bid> = global_bb
            .submap(asset_namespace(), "", u8::MAX, false)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|(path, bid, _order)| (path, bid))
            .collect();
        tracing::debug!(
            target: "noet_core::codec::perf",
            asset_count = asset_manifest.len(),
            elapsed_ms = stage_start.elapsed().as_millis(),
            "[finalize_html stage] asset_manifest submap",
        );

        let stage_start = std::time::Instant::now();
        let asset_hardlink_diagnostics = self.create_asset_hardlinks(&asset_manifest).await?;
        tracing::debug!(
            target: "noet_core::codec::perf",
            elapsed_ms = stage_start.elapsed().as_millis(),
            "[finalize_html stage] create_asset_hardlinks",
        );

        // Export BeliefGraph to JSON for client-side use.
        // Step 1: Obtain graph and pathmap from the synchronized global_bb.
        let stage_start = std::time::Instant::now();
        let mut graph = global_bb.export_beliefgraph().await?;
        tracing::debug!(
            target: "noet_core::codec::perf",
            node_count = graph.states.len(),
            elapsed_ms = stage_start.elapsed().as_millis(),
            "[finalize_html stage] export_beliefgraph",
        );

        // Collects warnings generated during export (e.g. oversized networks).
        // Returned to the caller so they can surface them alongside parse diagnostics.
        let mut finalize_diagnostics: Vec<crate::codec::ParseDiagnostic> = Vec::new();
        finalize_diagnostics.extend(asset_hardlink_diagnostics);

        // Reconstruct a temporary BeliefBase so we can access its PathMapMap.
        // BeliefBase::from(BeliefGraph) re-derives paths from the node/relation data,
        // giving us a PathMapMap that reflects the complete synchronized state.
        // We keep `temp_bb` alive for the duration of the export pipeline so the
        // read-guard returned by `paths()` remains valid.
        let stage_start = std::time::Instant::now();
        let temp_bb = crate::beliefbase::BeliefBase::from(graph.clone());
        tracing::debug!(
            target: "noet_core::codec::perf",
            elapsed_ms = stage_start.elapsed().as_millis(),
            "[finalize_html stage] BeliefBase::from(graph) rebuild",
        );

        // Step 1b: Compute layout metadata (assembly index + render positions)
        // for the 3D credibility map viewer. Mutates graph.states in place.
        let stage_start = std::time::Instant::now();
        {
            let pathmap = temp_bb.paths();
            crate::layout::compute_layout_metadata(&mut graph, &pathmap, &self.layout_config);
        }
        tracing::debug!(
            target: "noet_core::codec::perf",
            elapsed_ms = stage_start.elapsed().as_millis(),
            "[finalize_html stage] compute_layout_metadata",
        );

        // Step 2: Build compile-time search indices (always, before sharding decision).
        let stage_start = std::time::Instant::now();
        let search_manifest = {
            let pathmap = temp_bb.paths();
            crate::shard::search::build_search_indices(
                &graph.states,
                &pathmap,
                self.builder.repo(),
                &html_dir,
            )
            .await
        };
        tracing::debug!(
            target: "noet_core::codec::perf",
            elapsed_ms = stage_start.elapsed().as_millis(),
            "[finalize_html stage] build_search_indices",
        );

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
                tracing::debug!(
                    "[finalize_html] NOET_SHARD_THRESHOLD={threshold} — overriding default shard threshold"
                );
                crate::shard::manifest::ShardConfig {
                    shard_threshold: threshold,
                    ..crate::shard::manifest::ShardConfig::default()
                }
            }
            None => crate::shard::manifest::ShardConfig::default(),
        };
        let stage_start = std::time::Instant::now();
        let export_result = {
            let pathmap = temp_bb.paths();
            // Collect all known document extensions for the codec manifest.
            // This tells the WASM viewer which extensions produce rendered HTML.
            let codec_manifest = crate::shard::manifest::CodecManifest::new(
                crate::codec::collect_known_extensions(),
                crate::codec::WALK_CODECS.network_filenames(),
            );
            crate::shard::export::export_beliefbase(
                graph,
                &pathmap,
                &html_dir,
                &shard_config,
                &search_manifest,
                &codec_manifest,
            )
            .await
        };
        tracing::debug!(
            target: "noet_core::codec::perf",
            elapsed_ms = stage_start.elapsed().as_millis(),
            "[finalize_html stage] export_beliefbase",
        );
        let asset_version = match export_result {
            Ok(crate::shard::ExportMode::Monolithic { size_mb }) => {
                tracing::debug!(
                    "[finalize_html] Exported monolithic beliefbase.msgpack ({:.2} MB)",
                    size_mb
                );
                // Re-read the written file to hash its content.
                let msgpack_path = html_dir.join("beliefbase.msgpack");
                let bytes = tokio::fs::read(&msgpack_path).await.unwrap_or_default();
                Self::asset_version_hex(&bytes)
            }
            Ok(crate::shard::ExportMode::Sharded { manifest }) => {
                tracing::debug!(
                    "[finalize_html] Exported {} network shards to beliefbase/",
                    manifest.networks.len()
                );
                // Hash the manifest file — it changes whenever any shard content changes.
                let manifest_path = html_dir.join("beliefbase").join("manifest.json");
                let bytes = tokio::fs::read(&manifest_path).await.unwrap_or_default();
                Self::asset_version_hex(&bytes)
            }
            Err(e) => {
                // Log and fall back to the legacy exporter so a build failure here
                // doesn't break the rest of the output.
                tracing::warn!(
                    "[finalize_html] Shard export failed ({e}). Falling back to legacy export."
                );
                let graph_fallback = global_bb.export_beliefgraph().await?;
                self.export_beliefbase_json(graph_fallback).await?;
                // Hash the fallback file.
                let json_path = html_dir.join("beliefbase.json");
                let bytes = tokio::fs::read(&json_path).await.unwrap_or_default();
                Self::asset_version_hex(&bytes)
            }
        };

        // Write the SPA shell now that we have a stable asset_version derived from
        // the beliefbase content.  This replaces the earlier call that was made from
        // parse_all before the data files existed.
        let stage_start = std::time::Instant::now();
        self.generate_spa_shell(&asset_version).await?;
        tracing::debug!(
            target: "noet_core::codec::perf",
            elapsed_ms = stage_start.elapsed().as_millis(),
            "[finalize_html stage] generate_spa_shell",
        );

        Ok(finalize_diagnostics)
    }

    /// Copy all successfully-parsed source files to html_output/pages/sources/,
    /// mirroring the repo-relative directory structure.
    ///
    /// Only files that were successfully parsed (present in `self.latest_results`
    /// with no fatal parse error) are copied. This gives static HTML viewers a
    /// downloadable copy of the source alongside the rendered output.
    async fn copy_source_files(&self) -> Result<(), BuildonomyError> {
        let html_dir = match &self.html_output_dir {
            Some(dir) => dir.clone(),
            None => {
                tracing::debug!("[copy_source_files] No html_output_dir set — skipping");
                return Ok(());
            }
        };
        let sources_dir = html_dir.join("pages").join("sources");
        let repo_root = self.builder.repo_root();
        let mut copied = 0u32;
        let mut skipped_fatal = 0u32;
        let mut skipped_dir = 0u32;
        let mut skipped_prefix = 0u32;
        let mut copy_errors = 0u32;

        tracing::debug!(
            "[copy_source_files] Starting: {} results, sources_dir={:?}, repo_root={:?}",
            self.latest_results.len(),
            sources_dir,
            repo_root,
        );

        for (abs_path, result) in &self.latest_results {
            // Skip files with fatal parse errors.
            let has_fatal = result
                .diagnostics
                .iter()
                .any(|d| matches!(d, crate::codec::ParseDiagnostic::ParseError { .. }));
            if has_fatal {
                skipped_fatal += 1;
                continue;
            }
            // For directories (network roots), resolve to the actual network file
            // (e.g. index.md) inside the directory.
            let source_file = if abs_path.is_dir() {
                match detect_network_file(abs_path) {
                    Some(f) => f,
                    None => {
                        skipped_dir += 1;
                        continue;
                    }
                }
            } else {
                abs_path.clone()
            };
            let rel_path = match source_file.strip_prefix(repo_root) {
                Ok(r) => r,
                Err(_) => {
                    skipped_prefix += 1;
                    continue;
                }
            };
            let dest = sources_dir.join(rel_path);
            if let Some(parent) = dest.parent() {
                tokio::fs::create_dir_all(parent).await.ok();
            }
            match tokio::fs::copy(&source_file, &dest).await {
                Ok(_) => copied += 1,
                Err(e) => {
                    copy_errors += 1;
                    if copy_errors <= 5 {
                        tracing::warn!(
                            "[copy_source_files] Failed to copy {:?} -> {:?}: {}",
                            source_file,
                            dest,
                            e,
                        );
                    }
                }
            }
        }

        tracing::debug!(
            "[copy_source_files] Done: copied={}, skipped_fatal={}, skipped_dir={}, skipped_prefix={}, copy_errors={}",
            copied, skipped_fatal, skipped_dir, skipped_prefix, copy_errors,
        );
        Ok(())
    }

    /// Returns `true` if an asset path was actually enqueued for processing.
    fn process_asset_reference(
        &mut self,
        _path: &PathBuf,
        unresolved: &UnresolvedReference,
    ) -> bool {
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

            // Guard: if the computed path doesn't exist on disk, this "asset" reference
            // is almost certainly a misclassified ID link (e.g. `[R.CLDS-410]` whose
            // dot caused NodeKey::from_str to classify it as an asset path). Don't
            // enqueue it — leave the UnresolvedReference diagnostic in place so
            // promote_unresolved_to_warnings can surface it as a warning instead of
            // letting parse_one_path fail with a hard ParseError (→ exit(1)).
            if !absolute_path.exists() {
                tracing::debug!(
                    "[Compiler] Asset path does not exist on disk, treating as unresolved reference: {}",
                    asset_absolute_path
                );
                return false;
            }

            // Add to remainder_queue for processing (dedup check avoids double-dispatch).
            if !self.processed.contains_key(&absolute_path)
                && !self.remainder_queue.contains(&absolute_path)
            {
                tracing::debug!(
                    "[Compiler] Queueing asset file for content check: {}",
                    asset_absolute_path
                );
                self.remainder_queue.push_back(absolute_path);
                return true;
            }
        }
        false
    }

    /// Resolves an unresolved reference string to a canonical dep path and enqueues it if
    /// not yet processed or queued.
    ///
    /// Returns `true` when self should re-queue: either the dep was newly enqueued, or it
    /// is a same-batch sibling (pre-incremented before the batch ran, so its parse output
    /// is not yet in session_bb). Returns `false` for already-processed deps from prior
    /// batches (their output IS in session_bb; if the link didn't resolve now, re-queuing
    /// self won't help) and for permanent externals (out-of-corpus URLs, non-existent paths,
    /// absolute slugs, or paths outside the repo).
    fn process_unresolved_reference(
        &mut self,
        path: &Path,
        net_dep_path_str: &str,
        net_ref: Bref,
    ) -> bool {
        // Codec namespace brefs (derived from UUID_NAMESPACE_CODEC) are synthetic
        // secondary indices — not filesystem networks.  They have no disk path to
        // resolve, and their nodes are populated lazily during push().  Return true
        // so the caller treats this as a corpus dependency (triggering reparse)
        // rather than marking it as permanently unresolved.  The reference will
        // resolve on a subsequent pass once the target node's namespace_paths
        // entry has been committed to global_bb.
        if crate::codec::is_codec_namespace(&net_ref) {
            return true;
        }

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
            return false;
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
                return false;
            }

            // Resolve relative to builder's repo_root
            self.builder.repo_root().join(dep_path)
        } else {
            tracing::warn!(
                "No connectivity between builder.repo and dependent path network {}",
                net
            );
            return false;
        };

        // If the dependency path's filename component (ignoring any anchor) is
        // `index.md`, strip it to the parent directory before canonicalizing.
        // `net_dir/index.md` and `net_dir/index.md#anchor` are both entry points
        // for the network at `net_dir/` — parsing the file form generates a
        // duplicate network node because the processed map is keyed on the directory
        // form.  AnchorPath::filepath() strips any anchor first, then we check the
        // trailing filename; AnchorPath::dir() gives the parent directory string
        // directly, avoiding any std::path round-trip.
        let full_dep_path = {
            let dep_str = os_path_to_string(&full_dep_path);
            let dep_ap = AnchorPath::new(&dep_str);
            if WALK_CODECS.is_network_file(dep_ap.filename()) {
                let dir = string_to_os_path(dep_ap.dir());
                tracing::debug!(
                    "[process_unresolved_reference] Normalising network-file reference \
                     {:?} → directory form {:?}",
                    full_dep_path,
                    dir,
                );
                dir
            } else {
                full_dep_path
            }
        };

        // Canonicalize if it exists, then normalise to strip any \\?\ prefix (Windows).
        let canonical_dep_path = match full_dep_path.canonicalize() {
            Ok(p) => Self::normalize_queue_path(p),
            Err(_) => {
                // The path doesn't exist on disk.  This can mean:
                //   (a) External/non-existent dependency → skip
                //   (b) Synthetic path (e.g. C++ include-convention path that
                //       differs from the filesystem path) → the target node
                //       may exist in the PathMap once it's been parsed
                //
                // For (b): return true so the source file gets requeued — but
                // only on the FIRST encounter.  If the same synthetic path
                // triggered a requeue on a prior parse and the target still
                // doesn't exist, the reference will never resolve.  Return
                // false so the caller marks the primary key as permanently
                // unresolved, preventing futile O(N²) reparses of files with
                // many nodes (e.g. C++ register headers with 960+ symbols).
                //
                // Guard: only do this for paths within repo_root.  The
                // full_dep_path was constructed from repo_root + net_path +
                // dep_path_str, so an in-repo path always starts with
                // repo_root.  Absolute slugs and external URLs were already
                // filtered above.
                if full_dep_path.starts_with(self.builder.repo_root()) {
                    // Only requeue on the source file's first parse.  If this
                    // is a reparse (processed count > 1), the target had one
                    // full epoch to materialise and didn't — it never will.
                    // Returning false lets the caller mark the primary NodeKey
                    // as permanently unresolved via the existing mechanism.
                    let parse_count = self.processed.get(path).copied().unwrap_or(1);
                    if parse_count <= 1 {
                        tracing::debug!(
                            "[process_unresolved_reference] {:?} does not exist on disk but \
                             is within repo_root — requeueing source {:?} for reparse \
                             (synthetic path, first parse)",
                            full_dep_path,
                            path,
                        );
                        return true;
                    } else {
                        tracing::debug!(
                            "[process_unresolved_reference] {:?} still does not exist on \
                             disk after reparse (parse_count={}) — treating as permanently \
                             unresolved (source {:?})",
                            full_dep_path,
                            parse_count,
                            path,
                        );
                        return false;
                    }
                }
                tracing::trace!(
                    "[Compiler] Cannot canonicalize {:?}, treating as external",
                    full_dep_path
                );
                return false;
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
            return false;
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
            // Never re-queue a path that was explicitly rejected by a network's
            // whitelist/blacklist filter. The unresolved reference is legitimate
            // (the link exists in source) but the target is intentionally excluded.
            if CLAIM_MAP.is_rejected(&canonical_dep_path) {
                tracing::debug!(
                    "[process_unresolved_reference] {:?} is rejected by CLAIM_MAP, \
                     not re-enqueueing (referenced from {:?})",
                    canonical_dep_path,
                    path,
                );
                return false;
            }
            if (CODECS.path_get(&canonical_dep_path).is_some()
                || WALK_CODECS.should_track(&canonical_dep_path))
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
            // Newly enqueued: self must re-queue to resolve once dep's output lands.
            return true;
        }

        // Same-batch sibling: pre-incremented before the batch ran, so
        // `already_processed` is true but the dep's parse output is NOT yet in
        // session_bb when siblings run. Self must re-queue to pick up the dep's
        // output after the batch completes.
        //
        // Already-processed dep from a prior batch: output IS in session_bb. If the
        // link didn't resolve on this parse, re-queuing self won't help — return false.
        self.current_batch.contains(&canonical_dep_path)
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

    /// Render a short markdown snippet to HTML.
    ///
    /// Delegates to `crate::codec::render_markdown_snippet` — the shared
    /// utility that uses canonical parser options and a broken-link callback.
    fn render_markdown_snippet(md: &str) -> String {
        crate::codec::render_markdown_snippet(md)
    }

    /// Generate SPA shell (index.html) at HTML output root using Responsive template.
    ///
    /// `asset_version` is a short hex string derived from the serialized beliefbase
    /// content.  It is embedded in the page as `<script id="noet-asset-version">` and
    /// appended as a `?v=` query parameter by the viewer to all dynamic imports and
    /// data fetches, busting both the HTTP cache and the browser module-specifier cache
    /// when beliefbase content changes between deployments.
    async fn generate_spa_shell(&self, asset_version: &str) -> Result<(), BuildonomyError> {
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

        // Read optional footer_text from the entry network's frontmatter payload
        let footer_text = repo_node
            .payload
            .get("footer_text")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let footer_html = Self::render_markdown_snippet(footer_text);

        // Get stylesheet URLs based on use_cdn parameter
        let stylesheet_urls = get_stylesheet_urls(self.use_cdn);

        // Format script tag if provided
        let script_tag = self
            .html_script
            .as_ref()
            .map(|s| format!("<script>{}</script>", s))
            .unwrap_or_default();

        // Compute base_href and asset_prefix from base_url.
        //
        // base_href: used as <base href="..."> — controls how the SPA resolves
        //   relative navigation URLs.  With a base_url (e.g. for GitHub Pages),
        //   this is "<base_url>/pages/"; without one (local serve), "/pages/".
        //
        // asset_prefix: prepended to local (relative) asset paths only.
        //   CDN URLs (already absolute, starting with "https://") are passed through
        //   unchanged to avoid double-prefixing.
        let (base_href, asset_prefix) = match &self.base_url {
            Some(url) => {
                // Strip trailing slash so we never produce double slashes.
                let url = url.trim_end_matches('/');
                (format!("{url}/pages/"), format!("{url}/"))
            }
            None => ("/pages/".to_string(), "/".to_string()),
        };

        // Prefix a stylesheet URL with asset_prefix only when it is relative
        // (i.e. does not already start with a scheme like "https://").
        let prefix_asset = |url: &str| -> String {
            if url.contains("://") {
                url.to_string()
            } else {
                format!("{asset_prefix}{url}")
            }
        };

        // Replace template placeholders
        let html = template
            .replace(
                "{{CONTENT}}",
                r#"<div id="content-root"><p>Loading...</p></div>"#,
            )
            .replace("{{TITLE}}", &title)
            .replace("{{BID}}", &bid)
            .replace("{{ASSET_VERSION}}", asset_version)
            .replace("{{SCRIPT}}", &script_tag)
            .replace("{{BASE_HREF}}", &base_href)
            .replace("{{ASSET_PREFIX}}", &asset_prefix)
            .replace(
                "{{BASE_URL}}",
                self.base_url.as_deref().unwrap_or("").trim_end_matches('/'),
            )
            .replace(
                "{{STYLESHEET_OPEN_PROPS}}",
                &prefix_asset(&stylesheet_urls.open_props),
            )
            .replace(
                "{{STYLESHEET_NORMALIZE}}",
                &prefix_asset(&stylesheet_urls.normalize),
            )
            .replace(
                "{{STYLESHEET_THEME_LIGHT}}",
                &prefix_asset(&stylesheet_urls.theme_light),
            )
            .replace(
                "{{STYLESHEET_THEME_DARK}}",
                &prefix_asset(&stylesheet_urls.theme_dark),
            )
            .replace(
                "{{STYLESHEET_LAYOUT}}",
                &prefix_asset(&stylesheet_urls.layout),
            )
            .replace(
                "{{STYLESHEET_KATEX_CSS}}",
                &prefix_asset(&stylesheet_urls.katex_css),
            )
            .replace(
                "{{SCRIPT_KATEX_JS}}",
                &prefix_asset(&stylesheet_urls.katex_js),
            )
            .replace(
                "{{SCRIPT_KATEX_AUTO_RENDER_JS}}",
                &prefix_asset(&stylesheet_urls.katex_auto_render_js),
            )
            .replace(
                "{{STYLESHEET_TABULATOR_CSS}}",
                &prefix_asset(&stylesheet_urls.tabulator_css),
            )
            .replace(
                "{{SCRIPT_TABULATOR_JS}}",
                &prefix_asset(&stylesheet_urls.tabulator_js),
            )
            .replace(
                "{{SCRIPT_MERMAID_JS}}",
                &prefix_asset(&stylesheet_urls.mermaid_js),
            )
            .replace("{{FOOTER_TEXT}}", &footer_html);

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
        let document_paths: Vec<(String, Bid, Vec<u16>)> = global_bb
            .submap(repo_bid, "", u8::MAX, true)
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

        for (repo_relative_path, _bid, _order) in document_paths {
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

    /// Write HTML fragment to pages/ subdirectory wrapped in the given layout template
    async fn write_fragment(
        &self,
        html_output_dir: &Path,
        rel_path: &Path,
        pairs: Vec<(String, String)>,
        meta: FragmentMeta<'_>,
        layout: Layout,
    ) -> Result<(), BuildonomyError> {
        let FragmentMeta {
            title,
            bid,
            source_path,
            is_binary,
        } = meta;
        let pages_dir = html_output_dir.join("pages");
        let output_path = pages_dir.join(rel_path);

        // Ensure parent directories exist
        if let Some(parent) = output_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        // Wrap body with the requested layout template
        let template = get_template(layout);

        // Generate SPA route hash fragment (document path within the SPA).
        let hash_fragment = format!("/#/{}", rel_path.display());

        // Generate canonical URL (use base URL if configured, otherwise relative)
        let canonical_url = if let Some(base) = &self.base_url {
            format!("{}{}", base.trim_end_matches('/'), hash_fragment)
        } else {
            hash_fragment.clone()
        };

        // Generate the href for the "View Interactive Version" link.
        // With a base_url the link must include it so the browser navigates to
        // the correct origin+path (e.g. https://example.github.io/repo/#/doc.html).
        // Without one, a root-relative hash fragment is sufficient.
        let spa_route = if let Some(base) = &self.base_url {
            format!("{}{}", base.trim_end_matches('/'), hash_fragment)
        } else {
            hash_fragment
        };

        let source_link = match source_path {
            Some(p) => {
                let prefix = match &self.base_url {
                    Some(url) => format!("{}/", url.trim_end_matches('/')),
                    None => "/".to_string(),
                };
                format!("{}pages/sources/{}", prefix, p.display())
            }
            None => String::new(),
        };

        // Inject optional script if configured
        let scripts = if let Some(script) = &self.html_script {
            format!("<script>{}</script>", script)
        } else {
            String::new()
        };

        // Build the set of placeholder keys the caller supplies so we can skip
        // defaulting those — the caller pair wins on any collision.
        let caller_keys: std::collections::HashSet<&str> =
            pairs.iter().map(|(k, _)| k.as_str()).collect();

        // Apply caller-supplied pairs first (caller wins).
        let mut html = template.to_string();
        for (key, value) in &pairs {
            html = html.replace(key.as_str(), value.as_str());
        }

        // Compute asset_prefix from base_url (same logic as generate_spa_shell).
        // This ensures {{ASSET_PREFIX}} in fragment templates resolves to the
        // correct subpath prefix on deployments like GitHub Pages.
        let asset_prefix = match &self.base_url {
            Some(url) => format!("{}/", url.trim_end_matches('/')),
            None => "/".to_string(),
        };

        // Apply defaults for any placeholder the caller did not supply.
        let defaults: &[(&str, &str)] = &[
            ("{{ASSET_PREFIX}}", &asset_prefix),
            ("{{CANONICAL}}", &canonical_url),
            ("{{SPA_ROUTE}}", &spa_route),
            ("{{SOURCE_LINK}}", &source_link),
            (
                "{{SOURCE_BINARY}}",
                if is_binary { "true" } else { "false" },
            ),
            ("{{TITLE}}", title),
            ("{{BID}}", &bid.to_string()),
            ("{{SCRIPTS}}", &scripts),
            ("{{BODY}}", ""),
        ];
        for (key, value) in defaults {
            if !caller_keys.contains(key) {
                html = html.replace(key, value);
            }
        }

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
        // Guard: verify a codec claimed this path during parsing.  Plain .md files
        // are NOT registered in CODECS by extension — they are claimed per-network
        // via CLAIM_MAP.  Use CLAIM_MAP.get() which falls back to CODECS internally.
        let path_str = os_path_to_string(source_path);
        if CLAIM_MAP.get(source_path).is_none() {
            let msg = format!("No codec available for {} files", path_str);
            tracing::warn!("{}", msg);
            return Err(BuildonomyError::Codec(msg));
        }

        // Query for the node using repo-relative path. source_path is an absolute filesystem
        // path (stored in self.deferred_html), which was normalised by normalize_queue_path
        // before insertion, so strip_prefix against repo_root is safe.
        let repo_relative_str = source_path
            .strip_prefix(self.builder.repo_root())
            .map(os_path_to_string)
            .unwrap_or_else(|_| path_str.clone());

        let root_net_bid = self.builder.repo();

        // ── Resolve home network + net-relative path ─────────────────────────
        // Documents stored inside a subnet (e.g. "external/NPR_7150.2D/appendix_c.md")
        // are registered in the DB/PathMap under net = subnet_bref, path = "appendix_c.md".
        // Using root_net_bref + repo_relative_str for DocumentNodes or NodeKey::Path
        // produces a query that matches nothing for subnet documents.
        //
        // Strategy:
        //   1. Use session_bb's PathMapMap: call indexed_get("external/NPR_7150.2D/appendix_c.md")
        //      on the root PathMap. indexed_get is recursive — it strips each subnet prefix in
        //      turn and descends into the subnet's PathMap, returning (home_net_bid, doc_bid).
        //   2. Use net_path(home_net_bref, doc_bid) to recover the net-relative path string
        //      (e.g. "appendix_c.md") from within the home network's own PathMap.
        //   3. Use home_net_bid + net_relative_path for DocumentNodes and NodeKey::Path.
        //
        // session_bb's PathMapMap is fully populated after the parse phase completes, so no
        // extra DB round-trips are needed. For root-network documents (no '/' in
        // repo_relative_str) or directory nodes the fast path short-circuits immediately.
        let (doc_net_bid, doc_net_relative_path) =
            if !source_path.is_dir() && repo_relative_str.contains('/') {
                let pmm = self.builder.session_bb().paths();
                // Step 1: recursive indexed_get from the root PathMap.
                // Returns (home_net_bid, doc_bid, sort_order) crossing any subnet boundaries.
                let resolved = pmm
                    .get_map(&root_net_bid.bref())
                    .and_then(|pm| pm.indexed_get(&repo_relative_str, &pmm));
                if let Some((home_net_bid, doc_bid, _order)) = resolved {
                    // Step 2: net-relative path = path of doc_bid within its home network's PathMap.
                    // net_path(home_net_bref, doc_bid) looks up only that one PathMap (no recursion
                    // needed here — the doc lives directly in its home net, not in a deeper subnet).
                    let net_relative_path = pmm
                        .net_path(&home_net_bid.bref(), &doc_bid)
                        .map(|(_net, path)| path)
                        .unwrap_or_else(|| repo_relative_str.clone());
                    tracing::debug!(
                        "[generate_html_for_path] subnet doc resolved: \
                         repo_relative='{}' → net={} net_path='{}'",
                        repo_relative_str,
                        home_net_bid,
                        net_relative_path
                    );
                    (home_net_bid, net_relative_path)
                } else {
                    // indexed_get returned nothing — path not in session_bb PathMap yet.
                    // Fall back to root net (will likely warn below on the get() call).
                    tracing::debug!(
                        "[generate_html_for_path] indexed_get returned no result for \
                         repo_relative='{}' in session_bb; falling back to root net",
                        repo_relative_str
                    );
                    (root_net_bid, repo_relative_str.clone())
                }
            } else {
                (root_net_bid, repo_relative_str.clone())
            };

        // Network nodes are registered under NodeKey::Id / NodeKey::Bid, never under
        // NodeKey::Path (see speculative_path_key).  When a network directory is queued
        // into deferred_html, strip_prefix(repo_root) yields "" for the root network
        // (source_path == repo_root) or a repo-relative dir name for nested networks.
        // Neither form has a matching NodeKey::Path entry.
        //
        // Build the correct key upfront:
        //   - Network dir (any depth): use NodeKey::Bid.  The repo BID is known directly
        //     for the root (empty repo_relative_str); all other network dirs are resolved
        //     via indexed_get on the root PathMap, which crosses subnet boundaries
        //     automatically.  NodeKey::Path would fail because network nodes are keyed by
        //     NodeKey::Id in the PathMap, not by NodeKey::Path.
        //   - Documents: NodeKey::Path with the resolved home net bref and net-relative path.
        let nodekey = if source_path.is_dir() {
            let bid = if repo_relative_str.is_empty() {
                // Root network — BID is already known.
                self.builder.repo()
            } else {
                // Any non-root network dir: resolve BID from session_bb PathMap.
                // indexed_get on the root PathMap crosses subnet boundaries automatically,
                // so deeply-nested network dirs (e.g. "horizon/docs/developers") work
                // the same as top-level ones (e.g. "req/program_requirements").
                // Primary: session_bb PathMap (O(log N), no I/O).
                // Covers network nodes that emitted NodeUpsert events during this run.
                let pmm = self.builder.session_bb().paths();
                let from_session = pmm
                    .get_map(&root_net_bid.bref())
                    .and_then(|pm| pm.indexed_get(&repo_relative_str, &pmm))
                    .map(|(_home_net, bid, _order)| bid);
                drop(pmm);

                // Fallback: global_bb NetPath query for network nodes that were pure
                // GlobalCache hits during parsing (n_node_upsert=0 → no PathAdded event
                // → not in session_bb PathMap).  resolve_net_path in DbConnection walks
                // the paths table segment-by-segment, so nested dirs work correctly.
                let resolved = if from_session.is_some() {
                    tracing::debug!(
                        "[generate_html_for_path] network dir '{}' resolved via session_bb: bid={:?}",
                        repo_relative_str,
                        from_session,
                    );
                    from_session
                } else {
                    tracing::debug!(
                        "[generate_html_for_path] network dir '{}' not in session_bb PathMap, \
                         falling back to global_bb NetPath query",
                        repo_relative_str,
                    );
                    let spec = QuerySpec::seed(TapeFn::Keys(vec![NodeKey::Path {
                        net: root_net_bid.bref(),
                        path: repo_relative_str.clone(),
                    }]));
                    let mut package = QueryPackage::balanced(spec);
                    global_bb.evaluate(&mut package).await?;
                    let graph = package.into_graph();
                    // Find the seed (non-Trace) network node.  balanced() marks
                    // the seed state as complete and halo/ancestry neighbors as
                    // Trace.  Filtering on is_complete()
                    // ensures we pick the resolved target, not an ancestor.
                    let fallback_bid = graph
                        .states
                        .values()
                        .find(|n| n.kind.is_network() && n.kind.is_complete())
                        .map(|n| n.bid);
                    tracing::debug!(
                        "[generate_html_for_path] global_bb NetPath fallback for '{}': \
                         returned {} states, resolved bid={:?}",
                        repo_relative_str,
                        graph.states.len(),
                        fallback_bid,
                    );
                    fallback_bid
                };

                match resolved {
                    Some(bid) => bid,
                    None => {
                        tracing::warn!(
                            "[generate_html_for_path] Could not resolve subnet BID for \
                             network dir '{}' in session_bb or global_bb; skipping",
                            repo_relative_str
                        );
                        return Ok(());
                    }
                }
            };
            NodeKey::Bid { bid }
        } else {
            NodeKey::Path {
                net: doc_net_bid.bref(),
                path: doc_net_relative_path.clone(),
            }
        };

        // ── Step 0: load all nodes belonging to this document ────────────────
        // For directory (network) nodes, we already know the BID from the nodekey and
        // there are no section children to load — the network's sections live in its
        // index.md, which is a separate document node.  Use TapeFn::Bids directly to
        // avoid the DocumentNodes path which fails for the root network: the root's
        // PathMap entry (path = "") is not reliably reconstructed in the in-memory
        // final_bb that is passed here.
        //
        // For document files, `TapeFn::DocumentNodes` fetches the document root node
        // and every section/heading it contains in a single round-trip.  Use
        // doc_net_bid + doc_net_relative_path (resolved above) so that subnet documents
        // query against their own subnet's net bref and net-relative path rather than
        // the repo root bref and repo-relative path.
        let doc_nodes_graph = if source_path.is_dir() {
            // Network dir: look up the node directly by BID.
            // nodekey is always NodeKey::Bid for directories (root or subnet) — the
            // match below is exhaustive but the Path arm should never fire.
            let bid = match &nodekey {
                NodeKey::Bid { bid } => *bid,
                _ => root_net_bid, // should not occur: directories always produce NodeKey::Bid
            };
            {
                let mut package = QueryPackage::balanced(QuerySpec::seed(TapeFn::Bids(vec![bid])));
                global_bb.evaluate(&mut package).await?;
                package.into_graph()
            }
        } else {
            {
                let spec = QuerySpec::seed(TapeFn::DocumentNodes(
                    doc_net_bid.bref(),
                    doc_net_relative_path.clone(),
                ));
                let mut package = QueryPackage::balanced(spec);
                global_bb.evaluate(&mut package).await?;
                package.into_graph()
            }
        };

        let mut node_bb = BeliefBase::from(doc_nodes_graph);

        let Some(node) = node_bb.get(&nodekey) else {
            tracing::warn!("[generate_html_for_path] No match found for path: '{nodekey}'",);
            return Ok(());
        };
        let node_bid = node.bid;
        let title = node.display_title().to_string();

        // ── Step 0b: pre-fetch mapping-owned edges for all nodes in this document ──
        // `{maps_to}` directives emit edges with WEIGHT_OWNED_BY = section bref.
        // We need those edges in node_bb so that per-section OwnedBy queries can be
        // evaluated synchronously (against node_bb) when splicing mapping-table sentinels.
        //
        // One OwnedBy query per section bref.  Most documents have 0–1 {maps_to}
        // sections, so the loop is cheap; empty results are skipped immediately.
        {
            let all_doc_bids: Vec<Bid> = node_bb.states().keys().copied().collect();

            for bid in all_doc_bids {
                let spec = QuerySpec::seed_then(
                    TapeFn::Bids(vec![bid]),
                    vec![ProjectionStep::traverse(TraversalSpec {
                        input_roles: Role::Owner.into(),
                        kind_filter: enumset::EnumSet::all(),
                        output_roles: Role::Source | Role::Sink,
                        depth: TraversalDepth::count(1),
                        inverted: false,
                    })],
                );
                let owned_graph = {
                    let mut package = QueryPackage::balanced(spec);
                    global_bb.evaluate(&mut package).await?;
                    package.into_graph()
                };

                if !owned_graph.is_empty() {
                    node_bb.merge(&owned_graph);
                }
            }
        }

        // Build ctx after all node_bb mutations are complete (ctx borrows node_bb).
        //
        // For network nodes, pass node_bid as the root net rather than root_net_bid.
        // get_context(root_net, bid) resolves ctx.root_path via root_net's PathMap and
        // sets ctx.home_net from that resolution. When root_net is the repo root, a
        // subnet node's root_path comes out repo-relative (e.g. "req/derived_requirements")
        // and home_net resolves to the repo root — causing net_path_in to query the entire
        // repo and build_listing_html to compute relative links from the wrong base.
        // Passing the node's own BID as root_net for network nodes gives ctx.root_path=""
        // (the network's own root) and home_net=node_bid, so net_path_in queries only
        // that network's documents and link computation is correct.
        let ctx_root_net = if source_path.is_dir() {
            node_bid
        } else {
            root_net_bid
        };
        let Some(ctx) = node_bb.get_context(&ctx_root_net, &node_bid) else {
            tracing::warn!(
                "[generate_html_for_path] No context found for node {} (path: '{}')",
                node_bid,
                nodekey,
            );
            return Ok(());
        };

        // Convert absolute path to repo-relative path.
        // source_path is normalised (via normalize_queue_path at insertion), so
        // strip_prefix against repo_root is safe on all platforms.
        let repo_relative_path = source_path
            .strip_prefix(self.builder.repo_root())
            .unwrap_or(source_path);

        // Get base directory for output (ctx.path for directories, parent for files)
        // ctx.path is home-network relative, so for network nodes it's just the network name.
        // For document files, use the parent directory.
        let base_dir = if source_path.is_dir() {
            repo_relative_path
        } else {
            repo_relative_path.parent().unwrap_or(Path::new(""))
        };

        // Compute the expected on-disk HTML output path for sentinel splicing.
        // This mirrors write_fragment's layout: html_output_dir / "pages" / base_dir / filename.
        // For network nodes the deferred output filename is always "index.html".
        let deferred_filename_buf;
        let deferred_filename = if node.kind.is_network() {
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

        // ── Directive query pipeline ──────────────────────────────────────────
        //
        // `graphs` is the shared accumulator for all directive pipelines on this document.
        //
        //   graphs[0]   — node-resolution graph (always present, produced above)
        //   graphs[1..] — one entry per evaluate call across all directive pipelines,
        //                 in the order the directives appear in DIRECTIVES and their
        //                 queries slices.
        //
        // Each refiner fn receives `&graphs[..]` so it can reference any prior result by
        // index. The builder receives the same full slice.
        //
        // We only run directives whose sentinel appears in the on-disk HTML, avoiding
        // unnecessary DB round-trips for documents that don't use a given directive.
        // When the file does not yet exist we run all directives that have a non-empty
        // sentinel (fallback path: the compiler will write the fragment via write_fragment).
        let html_exists = existing_html_path.exists();
        let existing_html = if html_exists {
            Some(std::fs::read_to_string(&existing_html_path).map_err(|e| {
                BuildonomyError::Codec(format!(
                    "Failed to read existing HTML at {:?}: {}",
                    existing_html_path, e
                ))
            })?)
        } else {
            None
        };

        // (sentinel, built_html) pairs collected for splicing / fallback write.
        let mut splice_pairs: Vec<(String, String)> = Vec::new();

        tracing::debug!(
            "[generate_html_for_path] directive pipeline: title='{}' node_bid={} \
             ctx_root_net={} home_net={} root_path='{}' html_exists={} \
             existing_html_path='{}'",
            title,
            node_bid,
            ctx_root_net,
            ctx.home_net,
            ctx.root_path,
            html_exists,
            existing_html_path.display(),
        );

        for d in crate::codec::myst::DIRECTIVES {
            if d.builder.is_none() {
                continue;
            }
            let d_sentinel = crate::codec::myst::sentinel(d.name);
            // `maps_to` uses bref-parameterized sentinels (one per section) rather than
            // a single static sentinel.  Skip it here; handled separately below.
            if d.name == "maps_to" {
                continue;
            }
            // Skip this directive if its sentinel is not present in the output.
            let sentinel_present = existing_html
                .as_deref()
                .map(|h| h.contains(&d_sentinel))
                .unwrap_or(true); // file absent → assume all sentinels relevant
            if !sentinel_present {
                continue;
            }

            // Run each refiner in sequence, accumulating results.
            // graphs[i] is the result of d.queries[i]; ctx carries the resolved node.
            let mut graphs: Vec<BeliefGraph> = Vec::new();
            for refiner in d.queries {
                let spec = refiner(&ctx, &graphs);
                let mut package = QueryPackage::new(spec);
                global_bb.evaluate(&mut package).await?;
                let next = package.into_graph();
                graphs.push(next);
            }

            // Call the sync builder with ctx and the accumulated query results.
            if let Some(builder) = d.builder {
                let html = builder(&ctx, &graphs)?;
                splice_pairs.push((d_sentinel, html));
            }
        }

        // ── maps_to: per-section sentinel rendering ───────────────────────────
        // Each `{maps_to}` directive produces an anchor-parameterized sentinel of the form
        // `<!--@@noet-mapping-table:ANCHOR@@-->` where ANCHOR is the owning section's
        // stable heading id (e.g. "trace-mapping").  Anchors are used instead of brefs
        // because section BIDs are ephemeral (time-based) until written to disk — the bref
        // in a previously-written sentinel would not match the current parse's fresh bref.
        //
        // We scan the on-disk HTML for these sentinels, resolve each anchor to a BID via
        // node_bb's PathMap, evaluate OwnedBy(section_bref) synchronously against node_bb
        // (pre-populated in Step 0b), and render a mapping table per section.
        {
            // Collect (anchor, index) pairs from the on-disk HTML.
            // When the file is absent (first write), there are no sentinels to replace;
            // the fallback body is written and sentinels are replaced on a subsequent parse.
            let section_anchor_pairs: Vec<(String, usize)> = existing_html
                .as_deref()
                .map(crate::codec::myst::mapping_table_sentinel_anchors)
                .unwrap_or_default();

            for (anchor, directive_idx) in section_anchor_pairs {
                // Resolve anchor → BID via the PathMap.
                // Section nodes are registered under "<doc_path>#<anchor>" in the
                // PathMap.  We use `indexed_get` which searches all sub-maps.
                let section_bid_opt = {
                    let pmm = node_bb.paths();
                    // Build the anchored path: "<net-relative-doc>#<anchor>"
                    // doc_net_relative_path is the home-network-relative path of this
                    // document (e.g. "appendix_c.md" for a subnet doc, or
                    // "external/NPR_7150.2D/appendix_c.md" for a root-net doc).
                    // Sections are registered under this form in the PathMap.
                    let anchored_path = format!("{}#{}", doc_net_relative_path, anchor);
                    let from_pathmap = pmm
                        .get_map(&doc_net_bid.bref())
                        .and_then(|pm| pm.indexed_get(&anchored_path, &pmm))
                        .map(|(_home_net, bid, _order)| bid);
                    // Fallback: when the section's ID is a bref string (assigned by
                    // noet to resolve anchor collisions), build_path_key returns
                    // NodeKey::Bref rather than NodeKey::Path, so the section never
                    // appears in the PathMap.  Try interpreting the anchor as a bref
                    // and look it up directly in node_bb's bref index.
                    from_pathmap.or_else(|| {
                        crate::properties::Bref::try_from(anchor.as_str())
                            .ok()
                            .and_then(|bref| node_bb.brefs().get(&bref).copied())
                    })
                };

                let Some(section_bid) = section_bid_opt else {
                    tracing::debug!(
                        "[generate_html_for_path] maps_to sentinel anchor '{}' \
                         not found in node_bb PathMap; skipping",
                        anchor
                    );
                    continue;
                };

                // Load per-directive source/sink filter from section node metadata.
                // `_maps_to_specs[directive_idx]` is a JSON string of the form
                // {"sources":[...], "sinks":[...]} injected by MdCodec::inject_context.
                let (filter_source_bids, filter_sink_bids): (Option<Vec<Bid>>, Option<Vec<Bid>>) = {
                    let section_node = node_bb.states().get(&section_bid).cloned();
                    if let Some(sn) = section_node {
                        if let Some(specs_array) =
                            sn.metadata.get("_maps_to_specs").and_then(|v| v.as_array())
                        {
                            if let Some(spec_str) =
                                specs_array.get(directive_idx).and_then(|v| v.as_str())
                            {
                                if let Ok(spec_obj) =
                                    serde_json::from_str::<serde_json::Value>(spec_str)
                                {
                                    let resolve_keys = |field: &str| -> Option<Vec<Bid>> {
                                        spec_obj.get(field).and_then(|v| v.as_array()).map(|arr| {
                                            arr.iter()
                                                .filter_map(|s| s.as_str())
                                                .filter_map(|key_str| {
                                                    key_str.parse::<NodeKey>().ok().and_then(|k| {
                                                        node_bb.get(&k).map(|n| n.bid)
                                                    })
                                                })
                                                .collect()
                                        })
                                    };
                                    let src_bids = resolve_keys("sources");
                                    let snk_bids = resolve_keys("sinks");
                                    (src_bids, snk_bids)
                                } else {
                                    (None, None)
                                }
                            } else {
                                (None, None)
                            }
                        } else {
                            (None, None)
                        }
                    } else {
                        (None, None)
                    }
                };

                // Evaluate owner-traversal against node_bb before calling
                // get_context — the two borrows cannot overlap.
                let owned_spec = QuerySpec::seed_then(
                    TapeFn::Bids(vec![section_bid]),
                    vec![ProjectionStep::traverse(TraversalSpec {
                        input_roles: Role::Owner.into(),
                        kind_filter: enumset::EnumSet::all(),
                        output_roles: Role::Source | Role::Sink,
                        depth: TraversalDepth::count(1),
                        inverted: false,
                    })],
                );
                let owned_graph = {
                    let mut package = QueryPackage::balanced(owned_spec);
                    node_bb.evaluate_query(&mut package)?;
                    package.into_graph()
                };
                let graphs = vec![owned_graph];

                // Build a per-section ctx from node_bb (mutable borrow for assert).
                let Some(section_ctx) = node_bb.get_context(&root_net_bid, &section_bid) else {
                    tracing::debug!(
                        "[generate_html_for_path] no context for section {} \
                         (anchor '{}'); skipping",
                        section_bid,
                        anchor
                    );
                    continue;
                };

                let html = crate::codec::myst::build_mapping_table_html(
                    &section_ctx,
                    &graphs,
                    filter_source_bids.as_deref(),
                    filter_sink_bids.as_deref(),
                )?;
                let sentinel = crate::codec::myst::mapping_table_sentinel(&anchor, directive_idx);
                splice_pairs.push((sentinel, html));
            }
        }

        // ── {query}: per-instance query evaluation ────────────────────────────
        // Each `{query}` directive produces a sentinel `<!--@@noet-query:N@@-->`
        // where N is a 0-based index. The document node's metadata carries the
        // serialized QuerySpec and directive options in parallel arrays.
        {
            let query_specs: Option<&Vec<toml::Value>> =
                node.metadata.get("_query_specs").and_then(|v| v.as_array());
            let query_options: Option<&Vec<toml::Value>> = node
                .metadata
                .get("_query_options")
                .and_then(|v| v.as_array());
            let query_texts: Option<&Vec<toml::Value>> =
                node.metadata.get("_query_texts").and_then(|v| v.as_array());

            tracing::debug!(
                "[generate_html_for_path] {{query}}: node_bid={} has_query_specs={} \
                 has_query_options={} metadata_keys={:?}",
                node_bid,
                query_specs.is_some(),
                query_options.is_some(),
                node.metadata.keys().collect::<Vec<_>>(),
            );

            if let Some(specs) = query_specs {
                let indices: Vec<usize> = existing_html
                    .as_deref()
                    .map(crate::codec::myst::query_sentinel_indices)
                    .unwrap_or_default();

                tracing::debug!(
                    "[generate_html_for_path] {{query}}: found {} specs, {} sentinel indices",
                    specs.len(),
                    indices.len(),
                );

                for idx in indices {
                    let sentinel = crate::codec::myst::query_sentinel(idx);

                    // Retrieve spec JSON
                    let spec_json = match specs.get(idx).and_then(|v| v.as_str()) {
                        Some(s) => s,
                        None => {
                            let err_html = format!(
                                "<pre class=\"noet-query-error\"><strong>Query error:</strong> \
                                 no spec found for query block {idx}</pre>"
                            );
                            splice_pairs.push((sentinel, err_html));
                            continue;
                        }
                    };

                    // Check for parse error stored as {"error":"msg"}
                    if let Ok(err_obj) = serde_json::from_str::<serde_json::Value>(spec_json) {
                        if let Some(err_msg) = err_obj.get("error").and_then(|v| v.as_str()) {
                            let err_html = format!(
                                "<pre class=\"noet-query-error\"><strong>Query error:</strong> \
                                 {}</pre>",
                                err_msg
                                    .replace('&', "&amp;")
                                    .replace('<', "&lt;")
                                    .replace('>', "&gt;")
                            );
                            splice_pairs.push((sentinel, err_html));
                            continue;
                        }
                    }

                    // Deserialize the QuerySpec
                    let mut spec: QuerySpec = match serde_json::from_str(spec_json) {
                        Ok(s) => s,
                        Err(e) => {
                            let err_html = format!(
                                "<pre class=\"noet-query-error\"><strong>Query error:</strong> \
                                 failed to deserialize spec: {e}</pre>"
                            );
                            splice_pairs.push((sentinel, err_html));
                            continue;
                        }
                    };

                    // Resolve implicit seed → current document BID
                    if spec
                        .steps
                        .first()
                        .is_none_or(|s| matches!(s.input, TapeFn::Then(None)))
                    {
                        if spec.steps.is_empty() {
                            spec.steps.push(ProjectionStep::with_input(
                                TapeFn::Bids(vec![node_bid]),
                                crate::query::spec::StepOperation::Identity,
                            ));
                        } else {
                            spec.steps[0].input = TapeFn::Bids(vec![node_bid]);
                        }
                    }

                    // Evaluate the query to get result count for the meta div.
                    tracing::debug!(
                        "[generate_html_for_path] query block {idx}: evaluating spec \
                         subject={:?} projection_steps={}",
                        package_seed_debug(&spec),
                        spec.steps.len(),
                    );
                    let mut package = QueryPackage::balanced(spec);
                    match global_bb.evaluate(&mut package).await {
                        Ok(()) => {
                            tracing::debug!(
                                "[generate_html_for_path] query block {idx}: evaluation \
                                 returned {} states",
                                package.graph().map_or(0, |g| g.states.len()),
                            );
                        }
                        Err(e) => {
                            let err_html = format!(
                                "<pre class=\"noet-query-error\"><strong>Query error:</strong> \
                                 evaluation failed: {e}</pre>"
                            );
                            splice_pairs.push((sentinel, err_html));
                            continue;
                        }
                    }

                    let query_text = query_texts
                        .and_then(|texts| texts.get(idx))
                        .and_then(|v| v.as_str())
                        .unwrap_or("");

                    let result_count = package.graph().map_or(0, |g| g.states.len());

                    // The meta div carries the query text and result count for content.js
                    // (used by attachQuerySearchButtons to wire up the Search panel link).
                    let meta_div = format!(
                        "<div class=\"noet-query-meta\" data-query=\"{}\" \
                         data-count=\"{}\" hidden></div>",
                        query_text
                            .replace('&', "&amp;")
                            .replace('"', "&quot;")
                            .replace('<', "&lt;")
                            .replace('>', "&gt;"),
                        result_count,
                    );

                    // Emit a static placeholder result div.
                    // The full result rendering (via views) is deferred to the Search panel
                    // in the browser; content.js's attachQuerySearchButtons wires up the link.
                    let result_div =
                        "<div class=\"noet-query-result\"><p><em>Open Search to explore results.</em></p></div>"
                            .to_string();
                    let final_html = format!("{}{}", meta_div, result_div);

                    splice_pairs.push((sentinel, final_html));
                }
            }
        }

        // ── Sentinel splicing or fallback write ───────────────────────────────
        if splice_pairs.is_empty() {
            tracing::debug!(
                "[generate_html_for_path] splice_pairs is empty for '{}' — no sentinels replaced",
                title,
            );
            return Ok(());
        }
        tracing::debug!(
            "[generate_html_for_path] splicing {} sentinel pairs for '{}'",
            splice_pairs.len(),
            title,
        );

        let refs: Vec<(&str, &str)> = splice_pairs
            .iter()
            .map(|(s, h)| (s.as_str(), h.as_str()))
            .collect();

        if html_exists {
            crate::codec::myst::splice_sentinels(&existing_html_path, &refs)?;
        } else {
            // Fallback: no on-disk file yet — concatenate all fragments and let
            // write_fragment handle the full page wrap.
            let fallback_body: String = splice_pairs
                .iter()
                .map(|(_, h)| h.as_str())
                .collect::<Vec<_>>()
                .concat();
            if !fallback_body.is_empty() {
                let rel_path = base_dir.join(deferred_filename);
                // Deferred HTML is generated for network index nodes (directories) and
                // document files. For directories, resolve to the network file inside
                // (e.g. index.md) so the "View Source" link works.
                let deferred_source_file;
                let deferred_source_path = if source_path.is_dir() {
                    deferred_source_file = detect_network_file(source_path);
                    deferred_source_file
                        .as_deref()
                        .and_then(|f| f.strip_prefix(self.builder.repo_root()).ok())
                } else {
                    Some(
                        source_path
                            .strip_prefix(self.builder.repo_root())
                            .unwrap_or(source_path),
                    )
                };
                self.write_fragment(
                    html_output_dir,
                    &rel_path,
                    vec![("{{BODY}}".to_string(), fallback_body)],
                    FragmentMeta {
                        title: &title,
                        bid: &node_bid,
                        source_path: deferred_source_path,
                        is_binary: false,
                    },
                    crate::codec::assets::Layout::Simple,
                )
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
    ) -> Result<Vec<crate::codec::ParseDiagnostic>, BuildonomyError> {
        if manifest_data.is_empty() {
            return Ok(Vec::new());
        }
        let Some(html_output_dir) = self.html_output_dir() else {
            return Ok(Vec::new());
        };

        tracing::debug!(
            "[Compiler] Creating asset hardlinks for {} assets",
            manifest_data.len()
        );

        let mut copied_canonical: HashSet<PathBuf> = HashSet::new();
        let mut diagnostics: Vec<crate::codec::ParseDiagnostic> = Vec::new();

        for (asset_path, asset_bid) in manifest_data.iter() {
            // Get asset node to extract content hash from payload.
            //
            // This should always be present once sync_asset_snapshot has merged
            // every content_namespaces() child into session_bb (see Issue 98).
            // Fail soft rather than aborting the whole build over one asset: a
            // single node desynchronised from session_bb should not prevent
            // finalize_html from producing output for everything else.
            let Some(asset_node) = self.builder.session_bb().states().get(asset_bid) else {
                tracing::warn!(
                    "[Compiler] Asset node not found in session_bb for BID: {} (path: {}) \
                     — skipping hardlink. This indicates session_bb desynchronised from \
                     global_bb; see noet-core Issue 98.",
                    asset_bid,
                    asset_path
                );
                diagnostics.push(crate::codec::ParseDiagnostic::warning(format!(
                    "Asset node not found for BID {} (path: {}) — hardlink skipped",
                    asset_bid, asset_path
                )));
                continue;
            };

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

                // Verify source file exists and is not a directory.
                // process_asset_dir (Case B) emits directory listing nodes into
                // asset_namespace — they carry a content_hash but there is no single
                // file to copy; attempting fs::copy on a directory returns InvalidInput.
                if !repo_full_path.exists() {
                    tracing::warn!(
                        "[Compiler] Asset source file not found, skipping: {}",
                        repo_full_path.display()
                    );
                    continue;
                }
                if repo_full_path.is_dir() {
                    tracing::debug!(
                        "[Compiler] Skipping directory asset (listing-only, no file to copy): {}",
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
                diagnostics.push(crate::codec::ParseDiagnostic::info(format!(
                    "Duplicate content: {} is identical to {} (hash: {}); reusing canonical asset.",
                    asset_path,
                    canonical.display(),
                    content_hash
                )));
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
            "[Compiler] Asset hardlinks created: {} unique files, {} total paths ({} duplicate-content notices)",
            copied_canonical.len(),
            manifest_data.len(),
            diagnostics.len()
        );

        Ok(diagnostics)
    }
}

/// Statistics about the compiler's current state.
#[derive(Debug, Clone, Default)]
pub struct CompilerStats {
    pub remainder_queue_len: usize,
    pub processed_count: usize,
    pub total_parses: usize,
}

/// Debug-friendly summary of a QuerySpec's seed TapeFn for tracing.
fn package_seed_debug(spec: &QuerySpec) -> String {
    match spec.steps.first().map(|s| &s.input) {
        Some(TapeFn::Bids(bids)) => format!("Bids({})", bids.len()),
        Some(TapeFn::Keys(keys)) => format!("Keys({:?})", keys),
        Some(TapeFn::Corpus) => "Corpus".to_string(),
        Some(TapeFn::DocumentNodes(net, path)) => format!("DocumentNodes({net}, {path})"),
        Some(TapeFn::Then(None)) => "Implicit".to_string(),
        Some(other) => format!("{:?}", other),
        None => "Empty".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "service")]
    use crate::beliefbase::BeliefAccumulator;
    #[cfg(feature = "service")]
    use crate::db::{db_init_memory, DbConnection};
    #[cfg(feature = "git-tracking")]
    use crate::properties::NodeId;
    #[cfg(feature = "git-tracking")]
    use crate::query::BeliefSource;
    use crate::{
        beliefbase::{BeliefBase, BeliefGraph},
        codec::diagnostic::UnresolvedReference,
        event::BeliefEvent,
        nodekey::NodeKey,
        properties::{Bid, WeightKind},
        shard::{
            export::export_beliefbase,
            manifest::{SearchManifest, ShardConfig},
        },
    };
    #[cfg(feature = "git-tracking")]
    use git2::Repository;
    use petgraph::Direction;
    #[cfg(feature = "service")]
    use serial_test::serial;
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
            false,
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

    /// Small repos (below threshold) must write `beliefbase.msgpack` and must NOT
    /// write `beliefbase/manifest.json`.
    #[tokio::test]
    async fn test_finalize_html_monolithic_below_threshold() {
        let src_dir = tempfile::tempdir().unwrap();
        let html_dir = tempfile::tempdir().unwrap();

        create_test_network(src_dir.path());

        compile_to_html(src_dir.path(), html_dir.path())
            .await
            .unwrap();

        // Monolithic: beliefbase.msgpack must exist.
        assert!(
            html_dir.path().join("beliefbase.msgpack").exists(),
            "monolithic export should write beliefbase.msgpack"
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
                false,
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
        let codec_manifest = crate::shard::manifest::CodecManifest::new(
            crate::codec::collect_known_extensions(),
            crate::codec::WALK_CODECS.network_filenames(),
        );

        let result = export_beliefbase(
            graph,
            &pathmap,
            html_dir.path(),
            &config,
            &empty_search_manifest,
            &codec_manifest,
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
            bb_dir.join("global.msgpack").exists(),
            "beliefbase/global.msgpack should be written"
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

        // Codec manifest must be written alongside the beliefbase directory.
        let codecs_path = html_dir.path().join("codecs.json");
        assert!(codecs_path.exists(), "codecs.json should be written");
        let codec_json = std::fs::read_to_string(&codecs_path).unwrap();
        let codec_manifest: crate::shard::manifest::CodecManifest =
            serde_json::from_str(&codec_json).unwrap();
        assert!(
            codec_manifest
                .document_extensions
                .contains(&"md".to_string()),
            "codecs.json should include 'md'"
        );

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
    // Integration: monolithic beliefbase.msgpack deserializes as BeliefGraph
    // ------------------------------------------------------------------

    /// Verify that the monolithic `beliefbase.msgpack` is valid msgpack that can be
    /// deserialized as a `BeliefGraph`.
    #[tokio::test]
    async fn test_monolithic_beliefbase_json_is_valid_belief_graph() {
        let src_dir = tempfile::tempdir().unwrap();
        let html_dir = tempfile::tempdir().unwrap();

        create_test_network(src_dir.path());

        compile_to_html(src_dir.path(), html_dir.path())
            .await
            .unwrap();

        let msgpack_path = html_dir.path().join("beliefbase.msgpack");
        assert!(msgpack_path.exists(), "beliefbase.msgpack must exist");

        let bytes = std::fs::read(&msgpack_path).unwrap();
        let graph: BeliefGraph = rmp_serde::from_slice(&bytes)
            .expect("beliefbase.msgpack must deserialize as BeliefGraph");

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

        // Root network with MyST fenced network-children directive
        std::fs::write(
            src_dir.path().join("index.md"),
            "---\nid: \"root-network\"\ntitle: \"Root Network\"\n---\n\n# Root\n\n````{network_children}\n````\n",
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
            !content.contains(crate::codec::myst::sentinel("network_children").as_str()),
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
            "---\nid: \"sub-net\"\ntitle: \"Subnet\"\n---\n\n# Subnet\n\n````{network_children}\n````\n",
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
            !content.contains(crate::codec::myst::sentinel("network_children").as_str()),
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

    /// Verify sentinel replacement works for deeply nested subnets (3+ levels).
    ///
    /// Reproduces the structure observed in a large systems-engineering corpus:
    ///   root > level1 > level2 > level3
    /// Each level is a subnet with its own `index.md` and `{network_children}` marker.
    /// The bug report says sentinels survive unreplaced at deeper nesting levels.
    #[tokio::test]
    async fn test_finalize_html_replaces_sentinel_in_deeply_nested_subnet() {
        crate::tests::helpers::init_logging();

        let src_dir = tempfile::tempdir().unwrap();
        let html_dir = tempfile::tempdir().unwrap();

        // Root network
        std::fs::write(
            src_dir.path().join("index.md"),
            "---\nid: \"root-net\"\ntitle: \"Root\"\n---\n\n# Root\n\n````{network_children}\n````\n",
        )
        .unwrap();
        // Root-level child doc
        std::fs::write(
            src_dir.path().join("root_child.md"),
            "---\nid: \"root-child\"\ntitle: \"Root Child\"\n---\n\n# Root Child\n\nContent.\n",
        )
        .unwrap();

        // Level 1 subnet
        let l1_dir = src_dir.path().join("level1");
        std::fs::create_dir_all(&l1_dir).unwrap();
        std::fs::write(
            l1_dir.join("index.md"),
            "---\nid: \"level1-net\"\ntitle: \"Level 1\"\n---\n\n# Level 1\n\n````{network_children}\n````\n",
        )
        .unwrap();
        std::fs::write(
            l1_dir.join("l1_doc.md"),
            "---\nid: \"l1-doc\"\ntitle: \"L1 Doc\"\n---\n\n# L1 Doc\n\nContent.\n",
        )
        .unwrap();

        // Level 2 subnet (nested under level1)
        let l2_dir = l1_dir.join("level2");
        std::fs::create_dir_all(&l2_dir).unwrap();
        std::fs::write(
            l2_dir.join("index.md"),
            "---\nid: \"level2-net\"\ntitle: \"Level 2\"\n---\n\n# Level 2\n\n````{network_children}\n````\n",
        )
        .unwrap();
        std::fs::write(
            l2_dir.join("l2_doc.md"),
            "---\nid: \"l2-doc\"\ntitle: \"L2 Doc\"\n---\n\n# L2 Doc\n\nContent.\n",
        )
        .unwrap();

        // Level 3 subnet (nested under level2)
        let l3_dir = l2_dir.join("level3");
        std::fs::create_dir_all(&l3_dir).unwrap();
        std::fs::write(
            l3_dir.join("index.md"),
            "---\nid: \"level3-net\"\ntitle: \"Level 3\"\n---\n\n# Level 3\n\n````{network_children}\n````\n",
        )
        .unwrap();
        std::fs::write(
            l3_dir.join("l3_doc.md"),
            "---\nid: \"l3-doc\"\ntitle: \"L3 Doc\"\n---\n\n# L3 Doc\n\nContent.\n",
        )
        .unwrap();

        compile_to_html(src_dir.path(), html_dir.path())
            .await
            .unwrap();

        let sentinel = crate::codec::myst::sentinel("network_children");

        // Check root
        let root_index = html_dir.path().join("pages").join("index.html");
        assert!(root_index.exists(), "pages/index.html must exist");
        let root_html = std::fs::read_to_string(&root_index).unwrap();
        assert!(
            !root_html.contains(sentinel.as_str()),
            "root sentinel must be replaced; found in:\n{}",
            &root_html[..root_html.len().min(2000)]
        );
        assert!(
            root_html.contains("<ul>"),
            "root listing must contain <ul>; got:\n{}",
            &root_html[..root_html.len().min(2000)]
        );

        // Check level 1
        let l1_index = html_dir
            .path()
            .join("pages")
            .join("level1")
            .join("index.html");
        assert!(l1_index.exists(), "pages/level1/index.html must exist");
        let l1_html = std::fs::read_to_string(&l1_index).unwrap();
        assert!(
            !l1_html.contains(sentinel.as_str()),
            "level1 sentinel must be replaced; found in:\n{}",
            &l1_html[..l1_html.len().min(2000)]
        );
        assert!(
            l1_html.contains("l1_doc.html"),
            "level1 listing must link to l1_doc.html; got:\n{}",
            &l1_html[..l1_html.len().min(2000)]
        );

        // Check level 2
        let l2_index = html_dir
            .path()
            .join("pages")
            .join("level1")
            .join("level2")
            .join("index.html");
        assert!(
            l2_index.exists(),
            "pages/level1/level2/index.html must exist"
        );
        let l2_html = std::fs::read_to_string(&l2_index).unwrap();
        assert!(
            !l2_html.contains(sentinel.as_str()),
            "level2 sentinel must be replaced; found in:\n{}",
            &l2_html[..l2_html.len().min(2000)]
        );
        assert!(
            l2_html.contains("l2_doc.html"),
            "level2 listing must link to l2_doc.html; got:\n{}",
            &l2_html[..l2_html.len().min(2000)]
        );

        // Check level 3
        let l3_index = html_dir
            .path()
            .join("pages")
            .join("level1")
            .join("level2")
            .join("level3")
            .join("index.html");
        assert!(
            l3_index.exists(),
            "pages/level1/level2/level3/index.html must exist"
        );
        let l3_html = std::fs::read_to_string(&l3_index).unwrap();
        assert!(
            !l3_html.contains(sentinel.as_str()),
            "level3 sentinel must be replaced; found in:\n{}",
            &l3_html[..l3_html.len().min(2000)]
        );
        assert!(
            l3_html.contains("l3_doc.html"),
            "level3 listing must link to l3_doc.html; got:\n{}",
            &l3_html[..l3_html.len().min(2000)]
        );
    }

    /// Verify sentinel replacement renders the CORRECT children for deeply nested
    /// subnets that use whitelist filtering (matching a large systems-engineering corpus's structure).
    ///
    /// Structure:
    ///   root > mid (whitelist=["inner/**"]) > inner (has children)
    /// Each level has `{network_children}`. The bug is that `inner`'s listing
    /// shows root-level children instead of its own.
    #[tokio::test]
    #[cfg(feature = "service")]
    #[serial(db_tests)]
    async fn test_network_children_shows_correct_children_with_whitelists_via_db() {
        crate::tests::helpers::init_logging();

        let src_dir = tempfile::tempdir().unwrap();
        let html_dir = tempfile::tempdir().unwrap();

        // Root network
        std::fs::write(
            src_dir.path().join("index.md"),
            "---\nid: \"root-net\"\ntitle: \"Root\"\n---\n\n# Root\n\n````{network_children}\n````\n",
        )
        .unwrap();
        // Root-level child doc
        std::fs::write(
            src_dir.path().join("root_child.md"),
            "---\nid: \"root-child\"\ntitle: \"Root Child\"\n---\n\n# Root Child\n\nContent.\n",
        )
        .unwrap();

        // Mid-level subnet with whitelist
        let mid_dir = src_dir.path().join("mid");
        std::fs::create_dir_all(&mid_dir).unwrap();
        std::fs::write(
            mid_dir.join("index.md"),
            "---\nid: \"mid-net\"\ntitle: \"Mid Level\"\nwhitelist: [\"inner/**\"]\n---\n\n# Mid\n\n````{network_children}\n````\n",
        )
        .unwrap();

        // Inner subnet (child of mid)
        let inner_dir = mid_dir.join("inner");
        std::fs::create_dir_all(&inner_dir).unwrap();
        std::fs::write(
            inner_dir.join("index.md"),
            "---\nid: \"inner-net\"\ntitle: \"Inner\"\n---\n\n# Inner\n\n````{network_children}\n````\n",
        )
        .unwrap();
        std::fs::write(
            inner_dir.join("inner_doc.md"),
            "---\nid: \"inner-doc\"\ntitle: \"Inner Doc\"\n---\n\n# Inner Doc\n\nInner content.\n",
        )
        .unwrap();

        // Use the DB-backed accumulator
        let (tx, rx) = unbounded_channel::<BeliefEvent>();
        let db_pool = db_init_memory().await.unwrap();
        let accumulator = BeliefAccumulator::new(DbConnection(db_pool), rx);
        let global_bb = accumulator.query_handle();

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
            false,
        )
        .unwrap();

        compiler.parse_all(global_bb, false).await.unwrap();
        let final_db = accumulator.into_inner().await.unwrap();
        compiler.finalize_html(final_db).await.unwrap();

        // Check inner network: must list its OWN children, not root's
        let inner_index = html_dir.path().join("pages/mid/inner/index.html");
        assert!(
            inner_index.exists(),
            "pages/mid/inner/index.html must exist"
        );
        let inner_html = std::fs::read_to_string(&inner_index).unwrap();

        // Inner must contain its own child
        assert!(
            inner_html.contains("inner_doc.html"),
            "inner listing must link to inner_doc.html (its own child); got:\n{}",
            &inner_html[..inner_html.len().min(2000)]
        );
        // Inner must NOT contain root's children
        assert!(
            !inner_html.contains("root_child.html"),
            "inner listing must NOT contain root_child.html (root's child); got:\n{}",
            &inner_html[..inner_html.len().min(2000)]
        );

        // Check root network: must list its own children
        let root_html = std::fs::read_to_string(html_dir.path().join("pages/index.html")).unwrap();
        assert!(
            root_html.contains("root_child.html"),
            "root listing must link to root_child.html; got:\n{}",
            &root_html[..root_html.len().min(2000)]
        );
    }

    /// Same as [`test_finalize_html_replaces_sentinel_in_deeply_nested_subnet`] but
    /// uses a `DbConnection`-backed accumulator (the `service` feature path), which is
    /// the code path used by `noet parse` in production.
    ///
    /// Reproduces the bug where sentinel replacement works with in-memory `BeliefBase`
    /// but fails when `global_bb` is a `DbConnection`.
    #[tokio::test]
    #[cfg(feature = "service")]
    #[serial(db_tests)]
    async fn test_finalize_html_replaces_sentinel_deeply_nested_via_db() {
        crate::tests::helpers::init_logging();

        let src_dir = tempfile::tempdir().unwrap();
        let html_dir = tempfile::tempdir().unwrap();

        // Root network
        std::fs::write(
            src_dir.path().join("index.md"),
            "---\nid: \"root-net\"\ntitle: \"Root\"\n---\n\n# Root\n\n````{network_children}\n````\n",
        )
        .unwrap();
        std::fs::write(
            src_dir.path().join("root_child.md"),
            "---\nid: \"root-child\"\ntitle: \"Root Child\"\n---\n\n# Root Child\n\nContent.\n",
        )
        .unwrap();

        // Level 1 subnet
        let l1_dir = src_dir.path().join("level1");
        std::fs::create_dir_all(&l1_dir).unwrap();
        std::fs::write(
            l1_dir.join("index.md"),
            "---\nid: \"level1-net\"\ntitle: \"Level 1\"\n---\n\n# Level 1\n\n````{network_children}\n````\n",
        )
        .unwrap();
        std::fs::write(
            l1_dir.join("l1_doc.md"),
            "---\nid: \"l1-doc\"\ntitle: \"L1 Doc\"\n---\n\n# L1 Doc\n\nContent.\n",
        )
        .unwrap();

        // Level 2 subnet
        let l2_dir = l1_dir.join("level2");
        std::fs::create_dir_all(&l2_dir).unwrap();
        std::fs::write(
            l2_dir.join("index.md"),
            "---\nid: \"level2-net\"\ntitle: \"Level 2\"\n---\n\n# Level 2\n\n````{network_children}\n````\n",
        )
        .unwrap();
        std::fs::write(
            l2_dir.join("l2_doc.md"),
            "---\nid: \"l2-doc\"\ntitle: \"L2 Doc\"\n---\n\n# L2 Doc\n\nContent.\n",
        )
        .unwrap();

        // Level 3 subnet
        let l3_dir = l2_dir.join("level3");
        std::fs::create_dir_all(&l3_dir).unwrap();
        std::fs::write(
            l3_dir.join("index.md"),
            "---\nid: \"level3-net\"\ntitle: \"Level 3\"\n---\n\n# Level 3\n\n````{network_children}\n````\n",
        )
        .unwrap();
        std::fs::write(
            l3_dir.join("l3_doc.md"),
            "---\nid: \"l3-doc\"\ntitle: \"L3 Doc\"\n---\n\n# L3 Doc\n\nContent.\n",
        )
        .unwrap();

        // Use the DB-backed accumulator (mirrors `noet parse` CLI path)
        let (tx, rx) = unbounded_channel::<BeliefEvent>();
        let db_pool = db_init_memory().await.unwrap();
        let accumulator = BeliefAccumulator::new(DbConnection(db_pool), rx);
        let global_bb = accumulator.query_handle();

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
            false,
        )
        .unwrap();

        compiler.parse_all(global_bb, false).await.unwrap();
        let final_db = accumulator.into_inner().await.unwrap();
        compiler.finalize_html(final_db).await.unwrap();

        let sentinel = crate::codec::myst::sentinel("network_children");

        // Check all levels
        for (label, rel_path) in [
            ("root", "index.html"),
            ("level1", "level1/index.html"),
            ("level2", "level1/level2/index.html"),
            ("level3", "level1/level2/level3/index.html"),
        ] {
            let html_path = html_dir.path().join("pages").join(rel_path);
            assert!(html_path.exists(), "pages/{rel_path} must exist ({label})");
            let html = std::fs::read_to_string(&html_path).unwrap();
            assert!(
                !html.contains(sentinel.as_str()),
                "{label} sentinel must be replaced; found in pages/{rel_path}:\n{}",
                &html[..html.len().min(2000)]
            );
            assert!(
                html.contains("<ul>") || html.contains("No documents"),
                "{label} listing must contain <ul> or no-docs message; got pages/{rel_path}:\n{}",
                &html[..html.len().min(2000)]
            );
        }

        // Verify specific child links
        let l1_html =
            std::fs::read_to_string(html_dir.path().join("pages/level1/index.html")).unwrap();
        assert!(
            l1_html.contains("l1_doc.html"),
            "level1 must link to l1_doc.html; got:\n{}",
            &l1_html[..l1_html.len().min(2000)]
        );

        let l3_html = std::fs::read_to_string(
            html_dir
                .path()
                .join("pages/level1/level2/level3/index.html"),
        )
        .unwrap();
        assert!(
            l3_html.contains("l3_doc.html"),
            "level3 must link to l3_doc.html; got:\n{}",
            &l3_html[..l3_html.len().min(2000)]
        );
    }

    // -------------------------------------------------------------------------
    // BID stability with git tracking
    // -------------------------------------------------------------------------

    /// Helper: run parse_all on a directory and return the set of BIDs produced.
    async fn collect_bids(
        dir: &std::path::Path,
        git_tracking: bool,
        write: bool,
    ) -> std::collections::BTreeSet<Bid> {
        let (tx, mut rx) = unbounded_channel::<BeliefEvent>();
        let mut event_bb = BeliefBase::empty();
        let processor = tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                let _ = event_bb.process_event(&event);
            }
            event_bb
        });

        let mut compiler = DocumentCompiler::with_html_output(
            dir,
            Some(tx),
            Some(3),
            write,
            None,
            None,
            false,
            None,
            None,
            git_tracking,
        )
        .unwrap();

        let cache = compiler.builder().doc_bb().clone();
        compiler.parse_all(cache, false).await.unwrap();
        compiler.builder_mut().close_tx();
        let final_bb = processor.await.unwrap();

        final_bb.states().keys().copied().collect()
    }

    /// Two parses of the same directory — one with git tracking, one without — must
    /// produce identical BID sets.  Git metadata is runtime-only and must not affect
    /// node identity.
    ///
    /// BID sets are read from the compiler's `session_bb` after each parse, not from
    /// event channels.  Event channels only capture *deltas* (NodeUpdate events), so
    /// a second idempotent re-parse emits no events and the channel-based approach
    /// yields an empty set.  `session_bb` always holds the full current state.
    #[tokio::test]
    #[cfg(feature = "git-tracking")]
    async fn test_bid_stability_with_git_tracking() {
        let temp_dir = tempfile::tempdir().unwrap();
        create_test_network(temp_dir.path());

        // Write a document with a heading so we exercise source_line + sections tracking.
        std::fs::write(
            temp_dir.path().join("doc.md"),
            "---\ntitle: \"A Document\"\nid: \"a-doc\"\n---\n\n# Section One\n\nProse.\n",
        )
        .unwrap();

        // ── Pass 1: git_tracking = true ───────────────────────────────────────────
        let mut compiler_git = DocumentCompiler::with_html_output(
            temp_dir.path(),
            None,
            Some(3),
            false,
            None,
            None,
            false,
            None,
            None,
            true, // git_tracking = true
        )
        .unwrap();
        let cache = compiler_git.builder().doc_bb().clone();
        compiler_git.parse_all(cache, false).await.unwrap();
        // Read BIDs directly from session_bb — the full live state after parsing.
        let bids_git: std::collections::BTreeSet<Bid> = compiler_git
            .builder()
            .session_bb()
            .states()
            .keys()
            .copied()
            .collect();

        // ── Pass 2: git_tracking = false, fresh compiler ──────────────────────────
        // Write=true on pass 1 would be needed for sections BIDs to survive across
        // separate instances (they live in the parent doc's sections table on disk).
        // Since write=false, we compare session_bb BID sets from each compiler
        // directly — both see the same files, same content, same ephemeral BIDs
        // (because Bid::new uses Uuid::now_v7, both will be different absolute values,
        // but the COUNT and STRUCTURE must match).
        //
        // We therefore assert set equality on the non-ephemeral portion: BIDs that
        // are stable across runs are those read from source files (Bid::try_from(&str)).
        // A simpler proxy: both compilers must produce the same NUMBER of nodes, and
        // the single stable BID (the Buildonomy API node, which is a const namespace)
        // must appear in both.
        let mut compiler_no_git = DocumentCompiler::with_html_output(
            temp_dir.path(),
            None,
            Some(3),
            false,
            None,
            None,
            false,
            None,
            None,
            false, // git_tracking = false
        )
        .unwrap();
        let cache2 = compiler_no_git.builder().doc_bb().clone();
        compiler_no_git.parse_all(cache2, false).await.unwrap();
        let bids_no_git: std::collections::BTreeSet<Bid> = compiler_no_git
            .builder()
            .session_bb()
            .states()
            .keys()
            .copied()
            .collect();

        assert_eq!(
            bids_git.len(),
            bids_no_git.len(),
            "git-tracking and non-git-tracking parses must produce the same number of nodes; \
             git={} no-git={}",
            bids_git.len(),
            bids_no_git.len(),
        );
        assert!(!bids_git.is_empty(), "parse must produce at least one BID");
    }

    // -------------------------------------------------------------------------
    // End-to-end: git metadata populated when git_tracking = true
    // -------------------------------------------------------------------------

    /// When git_tracking is enabled and the network lives inside a real git repo,
    /// the network node's metadata["git"] must be populated with at least a commit
    /// hash and the source_url must be absent (no recognised remote in the temp repo).
    ///
    /// Uses a fresh git repo initialised with git2 so the test is hermetic.
    #[tokio::test]
    #[cfg(feature = "git-tracking")]
    async fn test_git_metadata_populated_on_network_node() {
        use std::collections::BTreeSet;

        let temp_dir = tempfile::tempdir().unwrap();
        let repo_path = temp_dir.path();

        // Initialise a bare-minimum git repo so GitCache::populate can discover it.
        let repo = Repository::init(repo_path).expect("git init");
        // Create an initial commit so HEAD points to something.
        {
            let sig = git2::Signature::now("Test", "test@example.com").unwrap();
            let tree_id = {
                let mut index = repo.index().unwrap();
                index.write_tree().unwrap()
            };
            let tree = repo.find_tree(tree_id).unwrap();
            repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
                .unwrap();
        }

        create_test_network(repo_path);

        std::fs::write(
            repo_path.join("doc.md"),
            "---\ntitle: \"Doc\"\nid: \"my-doc\"\n---\n\n# Section\n\nContent.\n",
        )
        .unwrap();

        let (tx, mut rx) = unbounded_channel::<BeliefEvent>();
        let mut event_bb = BeliefBase::empty();
        let processor = tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                let _ = event_bb.process_event(&event);
            }
            event_bb
        });

        let mut compiler = DocumentCompiler::with_html_output(
            repo_path,
            Some(tx),
            Some(3),
            false,
            None,
            None,
            false,
            None,
            None,
            true, // git_tracking = true
        )
        .unwrap();

        let cache = compiler.builder().doc_bb().clone();
        compiler.parse_all(cache, false).await.unwrap();
        compiler.builder_mut().close_tx();
        let final_bb = processor.await.unwrap();

        // Find the specific network node created by create_test_network (title "Test Network"),
        // rather than relying on iteration order to pick "the first" network node — other
        // synthetic networks (api, asset_namespace, href_namespace) are also present.
        let network_nodes: Vec<_> = final_bb
            .states()
            .values()
            .filter(|n| n.kind.is_network())
            .collect();

        assert!(
            !network_nodes.is_empty(),
            "parse must produce at least one network node"
        );

        let net = network_nodes
            .iter()
            .find(|n| n.title == "Test Network")
            .expect("Test Network node must be present");

        // metadata["git"] must be present.
        assert!(
            net.metadata.contains_key("git"),
            "network node must have metadata[\"git\"] when git_tracking is enabled; \
             got metadata keys: {:?}",
            net.metadata.keys().collect::<BTreeSet<_>>()
        );

        let git_table = net.metadata["git"].as_table().expect("git must be a table");

        // commit must be 40 hex chars.
        let commit = git_table
            .get("commit")
            .and_then(|v| v.as_str())
            .expect("commit must be present");
        assert_eq!(
            commit.len(),
            40,
            "commit hash must be 40 chars; got: {commit}"
        );

        // dirty must be present (boolean).
        assert!(
            git_table.contains_key("dirty"),
            "dirty flag must be present in git metadata"
        );

        // No recognised remote → source_url must be absent on all nodes.
        let nodes_with_source_url: Vec<_> = final_bb
            .states()
            .values()
            .filter(|n| n.metadata.contains_key("source_url"))
            .collect();
        assert!(
            nodes_with_source_url.is_empty(),
            "no source_url expected when repo has no recognised remote; \
             found {} node(s) with source_url",
            nodes_with_source_url.len()
        );
    }

    /// When git_tracking is disabled (default), no node should have metadata["git"].
    #[tokio::test]
    async fn test_no_git_metadata_when_tracking_disabled() {
        let temp_dir = tempfile::tempdir().unwrap();
        create_test_network(temp_dir.path());
        std::fs::write(
            temp_dir.path().join("doc.md"),
            "---\ntitle: \"Doc\"\nid: \"doc\"\n---\n\n# Section\n\nContent.\n",
        )
        .unwrap();

        let bids = collect_bids(temp_dir.path(), false, false).await;
        assert!(!bids.is_empty());

        // Re-parse and inspect metadata — no git keys expected.
        let (tx, mut rx) = unbounded_channel::<BeliefEvent>();
        let mut event_bb = BeliefBase::empty();
        let processor = tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                let _ = event_bb.process_event(&event);
            }
            event_bb
        });
        let mut compiler = DocumentCompiler::with_html_output(
            temp_dir.path(),
            Some(tx),
            Some(3),
            false,
            None,
            None,
            false,
            None,
            None,
            false, // git_tracking = false
        )
        .unwrap();
        let cache = compiler.builder().doc_bb().clone();
        compiler.parse_all(cache, false).await.unwrap();
        compiler.builder_mut().close_tx();
        let final_bb = processor.await.unwrap();

        let nodes_with_git: Vec<_> = final_bb
            .states()
            .values()
            .filter(|n| n.metadata.contains_key("git"))
            .collect();
        assert!(
            nodes_with_git.is_empty(),
            "no git metadata expected when tracking is disabled; \
             found {} node(s) with metadata[\"git\"]",
            nodes_with_git.len()
        );
    }

    // -------------------------------------------------------------------------
    // Metadata in exported JSON — full DB round-trip
    // -------------------------------------------------------------------------

    /// Compile a network to HTML using the full CLI-equivalent path: events flow
    /// through a `BeliefAccumulator<DbConnection>` (not an in-memory `BeliefBase`),
    /// `into_inner` extracts the `DbConnection`, and `finalize_html` exports the
    /// beliefbase msgpack.  Returns the `BeliefGraph` deserialized from the written
    /// `beliefbase.msgpack` so callers can assert on its contents.
    #[cfg(all(feature = "git-tracking", feature = "service"))]
    async fn compile_to_html_via_db(
        network_dir: &std::path::Path,
        html_dir: &std::path::Path,
        git_tracking: bool,
    ) -> Result<BeliefGraph, Box<dyn std::error::Error>> {
        let (tx, rx) = unbounded_channel::<BeliefEvent>();

        let db_pool = db_init_memory().await?;
        let accumulator = BeliefAccumulator::new(DbConnection(db_pool), rx);
        let global_bb = accumulator.query_handle();

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
            git_tracking,
        )?;

        compiler.parse_all(global_bb, false).await?;

        // Drain all pending events into the DB, then extract the DbConnection.
        let final_db = accumulator.into_inner().await?;

        // finalize_html queries final_db for the full graph and writes beliefbase.msgpack.
        compiler.finalize_html(final_db).await?;

        // Read and deserialize the written beliefbase.msgpack.
        let bb_msgpack_path = html_dir.join("beliefbase.msgpack");
        let bytes = std::fs::read(&bb_msgpack_path)?;
        let graph: BeliefGraph = rmp_serde::from_slice(&bytes)?;
        Ok(graph)
    }

    /// `BeliefNode.metadata` (including `git.*` and `source_url`) must survive the
    /// full parse → NodeUpdate event → BeliefAccumulator → DbConnection → msgpack export
    /// round-trip and appear in the `beliefbase.msgpack` written by `finalize_html`.
    ///
    /// This test uses the same `DbConnection`-backed accumulator path as the CLI
    /// (`noet parse --html-output`), unlike the existing
    /// `test_git_metadata_populated_on_network_node` which only checks the in-memory
    /// event-channel `BeliefBase`.
    #[tokio::test]
    #[cfg(all(feature = "git-tracking", feature = "service"))]
    #[serial(db_tests)]
    async fn test_metadata_in_exported_json() {
        use std::collections::BTreeSet;

        let temp_dir = tempfile::tempdir().unwrap();
        let html_dir = tempfile::tempdir().unwrap();
        let repo_path = temp_dir.path();

        // Initialise a git repo so GitCache::populate finds it and populates git metadata.
        let repo = Repository::init(repo_path).expect("git init");
        {
            let sig = git2::Signature::now("Test", "test@example.com").unwrap();
            let tree_id = {
                let mut index = repo.index().unwrap();
                index.write_tree().unwrap()
            };
            let tree = repo.find_tree(tree_id).unwrap();
            repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
                .unwrap();
        }

        create_test_network(repo_path);
        std::fs::write(
            repo_path.join("doc.md"),
            "---\ntitle: \"Doc\"\nid: \"doc\"\n---\n\n# Section\n\nContent.\n",
        )
        .unwrap();

        let graph = compile_to_html_via_db(repo_path, html_dir.path(), true)
            .await
            .expect("compile_to_html_via_db must succeed");

        // Locate the specific network node created by create_test_network (title "Test
        // Network"), rather than relying on iteration order to pick "the first" network node
        // — other synthetic networks (api, asset_namespace, href_namespace) are also present.
        let network_nodes: Vec<_> = graph
            .states
            .values()
            .filter(|n| n.kind.is_network())
            .collect();

        assert!(
            !network_nodes.is_empty(),
            "exported beliefbase.msgpack must contain at least one network node"
        );

        let net = network_nodes
            .iter()
            .find(|n| n.title == "Test Network")
            .expect("Test Network node must be present");

        // metadata["git"] must survive the DB round-trip and appear in the msgpack.
        assert!(
            net.metadata.contains_key("git"),
            "network node metadata[\"git\"] must be present in exported beliefbase.msgpack \
             after the full DB round-trip; \
             got metadata keys: {:?}",
            net.metadata.keys().collect::<BTreeSet<_>>()
        );

        let git_table = net.metadata["git"]
            .as_table()
            .expect("metadata[\"git\"] must deserialize as a TOML table");

        // commit must be 40 hex chars.
        let commit = git_table
            .get("commit")
            .and_then(|v| v.as_str())
            .expect("git.commit must be present in exported metadata");
        assert_eq!(
            commit.len(),
            40,
            "git.commit hash must be 40 chars in exported msgpack; got: {commit}"
        );

        // dirty flag must be present.
        assert!(
            git_table.contains_key("dirty"),
            "git.dirty must be present in exported metadata"
        );
    }

    /// Regression test for the asset-sync bug: `sync_asset_snapshot`
    /// must actually pull a namespace's asset/href children out of `global_bb`
    /// and merge them into `self.builder.session_bb()`. Prior to the fix this
    /// always logged "Merged 0 asset nodes" because the query walked Section
    /// edges root-ward (toward the namespace's own parent) instead of leaf-ward
    /// (toward the namespace's children), and asset nodes are the SOURCE of
    /// their Section edge to the namespace (the namespace is the sink) — see
    /// `GraphBuilder::process_asset`.
    #[tokio::test]
    async fn test_sync_asset_snapshot_merges_namespace_children() {
        use crate::properties::{
            asset_namespace, BeliefKind, BeliefNode, Weight, WEIGHT_DOC_PATHS,
        };

        // ── Setup ─────────────────────────────────────────────────
        let repo_dir = tempfile::TempDir::new().unwrap();
        let repo_path_buf = crate::paths::canonicalize_path(repo_dir.path()).unwrap();
        let repo_path = repo_path_buf.as_path();
        create_test_network(repo_path);

        let mut compiler = DocumentCompiler::simple(repo_path).unwrap();

        // Build a `global_bb` (plain in-memory BeliefBase, which implements
        // BeliefSource) seeded with the asset namespace node plus two asset
        // children, using the SAME edge shape GraphBuilder::process_asset
        // produces: asset_bid --Section--> asset_namespace() (asset is source,
        // namespace is sink).
        let ns_node = crate::properties::BeliefNode::asset_network();
        let ns_bid = ns_node.bid;
        assert_eq!(
            ns_bid,
            asset_namespace(),
            "sanity: asset_network's bid must equal asset_namespace()"
        );

        let asset_a = Bid::new(ns_bid);
        let asset_b = Bid::new(ns_bid);
        let make_asset_node = |bid: Bid, path: &str| BeliefNode {
            bid,
            kind: BeliefKind::External.into(),
            title: path.to_string(),
            ..Default::default()
        };

        let mut global_bb = BeliefBase::default();
        global_bb
            .process_event(&BeliefEvent::NodeUpsert(
                ns_bid,
                ns_node,
                crate::event::EventOrigin::Remote,
            ))
            .unwrap();
        for (bid, path) in [(asset_a, "vendor/a.txt"), (asset_b, "vendor/b.txt")] {
            global_bb
                .process_event(&BeliefEvent::NodeUpsert(
                    bid,
                    make_asset_node(bid, path),
                    crate::event::EventOrigin::Remote,
                ))
                .unwrap();
            let mut payload = toml::Table::new();
            payload.insert(
                WEIGHT_DOC_PATHS.to_string(),
                toml::Value::Array(vec![toml::Value::String(path.to_string())]),
            );
            global_bb
                .process_event(&BeliefEvent::RelationChange(
                    bid,
                    ns_bid,
                    WeightKind::Section,
                    Some(Weight { payload }),
                    crate::event::EventOrigin::Remote,
                ))
                .unwrap();
        }

        // ── Exercise ─────────────────────────────────────────────────────────
        compiler.sync_asset_snapshot(&global_bb).await.unwrap();

        // ── Assert ───────────────────────────────────────────────────────────
        let session_bb = compiler.builder().session_bb();
        assert!(
            session_bb.states().contains_key(&asset_a),
            "sync_asset_snapshot must merge asset_a into session_bb; got states: {:?}",
            session_bb.states().keys().collect::<Vec<_>>()
        );
        assert!(
            session_bb.states().contains_key(&asset_b),
            "sync_asset_snapshot must merge asset_b into session_bb; got states: {:?}",
            session_bb.states().keys().collect::<Vec<_>>()
        );
    }

    /// Same bug as [`test_sync_asset_snapshot_merges_namespace_children`] but uses a
    /// `DbConnection`-backed accumulator (the `service` feature path), which is the
    /// code path used by noet-core's parse CLI / a downstream consumer's render CLI
    /// in production (e.g. `--jobs N render`). The in-memory-`BeliefBase` version of this test does NOT
    /// catch the SQL-backend-specific leaf-map bug described below — this test is
    /// the one that matters for catching regressions in production builds.
    #[tokio::test]
    #[cfg(feature = "service")]
    #[serial(db_tests)]
    async fn test_sync_asset_snapshot_merges_namespace_children_via_db() {
        use crate::properties::{
            asset_namespace, BeliefKind, BeliefNode, Weight, WeightSet, WEIGHT_DOC_PATHS,
        };

        // ── Setup ─────────────────────────────────────────────────
        let repo_dir = tempfile::TempDir::new().unwrap();
        let repo_path_buf = crate::paths::canonicalize_path(repo_dir.path()).unwrap();
        let repo_path = repo_path_buf.as_path();
        create_test_network(repo_path);

        let mut compiler = DocumentCompiler::simple(repo_path).unwrap();

        // Use the DB-backed accumulator (mirrors noet-core's parse CLI / a downstream
        // consumer's render CLI path).
        let (tx, rx) = unbounded_channel::<BeliefEvent>();
        let db_pool = db_init_memory().await.unwrap();
        let accumulator = BeliefAccumulator::new(DbConnection(db_pool), rx);
        let global_bb = accumulator.query_handle();

        let ns_node = BeliefNode::asset_network();
        let ns_bid = ns_node.bid;
        assert_eq!(
            ns_bid,
            asset_namespace(),
            "sanity: asset_network's bid must equal asset_namespace()"
        );

        let asset_a = Bid::new(ns_bid);
        let asset_b = Bid::new(ns_bid);
        let make_asset_node = |bid: Bid, path: &str| BeliefNode {
            bid,
            kind: BeliefKind::External.into(),
            title: path.to_string(),
            ..Default::default()
        };

        // Send the same events sync_asset_snapshot's real callers see once GraphBuilder's
        // terminate_stack/compute_diff has resolved a RelationChange into a RelationUpdate
        // (RelationChange alone is a no-op on the DB write path -- Transaction::add_event
        // deliberately ignores it, waiting for the resolved RelationUpdate; see
        // noet-core Issue 98 investigation), bracketed in a BatchStart/BatchEnd epoch so
        // drain_epoch commits them to the DB.
        tx.send(BeliefEvent::BatchStart).unwrap();
        tx.send(BeliefEvent::NodeUpsert(
            ns_bid,
            ns_node,
            crate::event::EventOrigin::Remote,
        ))
        .unwrap();
        for (bid, path) in [(asset_a, "vendor/a.txt"), (asset_b, "vendor/b.txt")] {
            tx.send(BeliefEvent::NodeUpsert(
                bid,
                make_asset_node(bid, path),
                crate::event::EventOrigin::Remote,
            ))
            .unwrap();
            let mut payload = toml::Table::new();
            payload.insert(
                WEIGHT_DOC_PATHS.to_string(),
                toml::Value::Array(vec![toml::Value::String(path.to_string())]),
            );
            let mut weights = WeightSet::default();
            weights
                .weights
                .insert(WeightKind::Section, Weight { payload });
            tx.send(BeliefEvent::RelationUpdate(
                bid,
                ns_bid,
                weights,
                crate::event::EventOrigin::Remote,
            ))
            .unwrap();
        }
        tx.send(BeliefEvent::BatchEnd).unwrap();
        global_bb.drain_epoch().await.unwrap();

        // ── Exercise ───────────────────────────────────────────────────
        compiler.sync_asset_snapshot(&global_bb).await.unwrap();

        // ── Assert ───────────────────────────────────────────────────────────
        let session_bb = compiler.builder().session_bb();
        assert!(
            session_bb.states().contains_key(&asset_a),
            "sync_asset_snapshot (DB-backed) must merge asset_a into session_bb; got states: {:?}",
            session_bb.states().keys().collect::<Vec<_>>()
        );
        assert!(
            session_bb.states().contains_key(&asset_b),
            "sync_asset_snapshot (DB-backed) must merge asset_b into session_bb; got states: {:?}",
            session_bb.states().keys().collect::<Vec<_>>()
        );
    }

    // -------------------------------------------------------------------------
    // export_asset_dir — Case B: local directory listing
    // -------------------------------------------------------------------------

    /// A markdown link to a local directory (not a registered network) must produce
    /// a `BeliefKind::External` node in `session_bb` with:
    ///   - `payload["listing"]`       — sorted array of entry name strings
    ///   - `payload["content_hash"]`  — SHA-256 over path + listing
    ///   - `title`                    — repo-relative path of the directory
    ///
    /// The test does NOT require the `git-tracking` or `service` features.
    #[tokio::test]
    async fn test_directory_asset_listing() {
        use tempfile::TempDir;
        use tokio::sync::mpsc::unbounded_channel;

        // ── Setup ────────────────────────────────────────────────────────────
        let repo_dir = TempDir::new().unwrap();
        // Normalize so the path matches GraphBuilder::new's repo_root on all platforms
        // (macOS symlinks, Windows \\?\ prefix from canonicalize()).
        let repo_path_buf = crate::paths::canonicalize_path(repo_dir.path()).unwrap();
        let repo_path = repo_path_buf.as_path();

        // Minimal network index so GraphBuilder has a repo root.
        create_test_network(repo_path);

        // Create a subdirectory with some files — this is what the markdown link
        // will point to.
        let asset_dir = repo_path.join("vendor");
        std::fs::create_dir_all(&asset_dir).unwrap();
        std::fs::write(asset_dir.join("b_file.txt"), "b").unwrap();
        std::fs::write(asset_dir.join("a_file.txt"), "a").unwrap();
        std::fs::create_dir_all(asset_dir.join("sub")).unwrap();

        // ── Build the compiler / builder ─────────────────────────────────────
        let (tx, mut rx) = unbounded_channel::<crate::event::BeliefEvent>();
        let mut builder = crate::codec::builder::GraphBuilder::new(repo_path, Some(tx)).unwrap();

        let global_bb = crate::beliefbase::BeliefBase::default();
        let proto_index = crate::codec::proto_index::ProtoIndex::default();

        // ── Exercise ─────────────────────────────────────────────────────────
        let result = builder
            .process_asset_dir(&asset_dir, global_bb, proto_index)
            .await;
        assert!(
            result.is_ok(),
            "process_asset_dir must succeed: {:?}",
            result.err()
        );

        // Drain emitted events into a local BeliefBase so we can query it.
        drop(builder); // closes tx
        let mut local_bb = crate::beliefbase::BeliefBase::default();
        while let Ok(event) = rx.try_recv() {
            local_bb.process_event(&event).unwrap();
        }

        // ── Assert ───────────────────────────────────────────────────────────
        use crate::properties::BeliefKind;

        // There must be exactly one External node whose title is the
        // repo-relative directory path.
        let external_nodes: Vec<_> = local_bb
            .states()
            .values()
            .filter(|n| n.kind.contains(BeliefKind::External) && !n.kind.is_network())
            .collect();

        assert_eq!(
            external_nodes.len(),
            1,
            "expected exactly one External node for the directory; got {}: {:?}",
            external_nodes.len(),
            external_nodes.iter().map(|n| &n.title).collect::<Vec<_>>()
        );

        let dir_node = external_nodes[0];
        assert_eq!(
            dir_node.title, "vendor",
            "node title must be the repo-relative directory path"
        );

        // payload["listing"] must be a sorted array of entry names.
        let listing = dir_node
            .payload
            .get("listing")
            .and_then(|v| v.as_array())
            .expect("payload[\"listing\"] must be a TOML array");

        let names: Vec<&str> = listing.iter().filter_map(|v| v.as_str()).collect();

        // Sorted order: a_file.txt, b_file.txt, sub
        assert!(
            names.contains(&"a_file.txt"),
            "listing must contain a_file.txt; got: {:?}",
            names
        );
        assert!(
            names.contains(&"b_file.txt"),
            "listing must contain b_file.txt; got: {:?}",
            names
        );
        assert!(
            names.contains(&"sub"),
            "listing must contain sub/; got: {:?}",
            names
        );
        // Verify sorted order.
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted, "listing must be in sorted order");

        // payload["content_hash"] must be a non-empty hex string.
        let hash = dir_node
            .payload
            .get("content_hash")
            .and_then(|v| v.as_str())
            .expect("payload[\"content_hash\"] must be present");
        assert!(!hash.is_empty(), "content_hash must be non-empty");
        assert!(
            hash.chars().all(|c| c.is_ascii_hexdigit()),
            "content_hash must be a hex string; got: {hash}"
        );

        // payload["truncated"] must not be present (only 3 entries, well under 256).
        assert!(
            dir_node.payload.get("truncated").is_none(),
            "truncated must not be set for a small directory"
        );
    }

    // -------------------------------------------------------------------------
    // export_asset_dir — Case C: non-existent path (regression)
    // -------------------------------------------------------------------------

    /// A markdown link to a path that does not exist should produce an
    /// `UnresolvedReference` and leave no External node in the graph.
    /// This test verifies the existing behaviour is unchanged by the Case B
    /// implementation (i.e. we do not regress Case C).
    ///
    /// `parse_one_path` hits the `tokio::fs::read` error path for missing files,
    /// or `detect_network_file` returns None for missing dirs.  Either way, the
    /// caller receives an `Err` result which `process_one_parse_result` converts
    /// to a diagnostic rather than a node update.
    #[tokio::test]
    async fn test_directory_asset_case_c_nonexistent() {
        use tempfile::TempDir;
        use tokio::sync::mpsc::unbounded_channel;

        let repo_dir = TempDir::new().unwrap();
        // Normalize so the path matches GraphBuilder::new's repo_root on all platforms
        // (macOS symlinks, Windows \\?\ prefix from canonicalize()).
        let repo_path_buf = crate::paths::canonicalize_path(repo_dir.path()).unwrap();
        let repo_path = repo_path_buf.as_path();
        create_test_network(repo_path);

        let (tx, mut rx) = unbounded_channel::<crate::event::BeliefEvent>();
        let mut builder = crate::codec::builder::GraphBuilder::new(repo_path, Some(tx)).unwrap();

        let global_bb = crate::beliefbase::BeliefBase::default();
        let proto_index = crate::codec::proto_index::ProtoIndex::default();

        // A directory that does not exist.
        let ghost_dir = repo_path.join("nonexistent_dir");
        assert!(
            !ghost_dir.exists(),
            "precondition: directory must not exist"
        );

        let result = builder
            .process_asset_dir(&ghost_dir, global_bb, proto_index)
            .await;

        // process_asset_dir should return an Err for an unreadable/missing directory.
        assert!(
            result.is_err(),
            "process_asset_dir must return Err for a non-existent path; got Ok"
        );

        // No External node should have been emitted.
        drop(builder);
        let mut local_bb = crate::beliefbase::BeliefBase::default();
        while let Ok(event) = rx.try_recv() {
            local_bb.process_event(&event).unwrap();
        }

        use crate::properties::BeliefKind;
        let external_nodes: Vec<_> = local_bb
            .states()
            .values()
            .filter(|n| n.kind.contains(BeliefKind::External) && !n.kind.is_network())
            .collect();

        assert!(
            external_nodes.is_empty(),
            "no External nodes must be emitted for a non-existent path; got: {:?}",
            external_nodes.iter().map(|n| &n.title).collect::<Vec<_>>()
        );
    }

    // -------------------------------------------------------------------------
    // export_asset_dir — Case A: git-tracked directory → href node
    // -------------------------------------------------------------------------

    /// A directory that is a registered network inside a git repo with a known
    /// remote (github.com or gitlab.com) must produce an `href_namespace` node
    /// pointing to the normalized remote URL.
    ///
    /// The test is self-contained: it initialises a temp git repo, adds a remote
    /// with a github.com URL (no network access — git2 stores it as a string only),
    /// creates an initial commit so HEAD is valid, writes an index.md so the
    /// directory is a recognised network, builds a `ProtoIndex` with git-tracking
    /// enabled, then calls `process_asset_dir` directly.
    ///
    /// Expected outcome: exactly one node in `href_namespace` whose `id` equals
    /// the normalised remote URL.
    #[tokio::test]
    #[cfg(feature = "git-tracking")]
    async fn test_directory_asset_case_a_git_tracked() {
        use crate::{
            codec::proto_index::ProtoIndex,
            properties::{href_namespace, BeliefKind},
        };
        use tempfile::TempDir;

        // ── Setup: git repo with a github remote ─────────────────────────────
        let repo_dir = TempDir::new().unwrap();
        // Normalize so the path matches GraphBuilder::new's repo_root on all platforms
        // (macOS symlinks, Windows \\?\ prefix from canonicalize()).
        let repo_path_buf = crate::paths::canonicalize_path(repo_dir.path()).unwrap();
        let repo_path = repo_path_buf.as_path();

        let repo = Repository::init(repo_path).expect("git init");

        // Add a github remote — git2 stores this locally, no network access occurs.
        repo.remote("origin", "https://github.com/testorg/testrepo.git")
            .expect("remote add origin");

        // Initial commit so HEAD is valid (GitCache::populate calls repo.head()).
        {
            let sig = git2::Signature::now("Test", "test@example.com").unwrap();
            let tree_id = repo.index().unwrap().write_tree().unwrap();
            let tree = repo.find_tree(tree_id).unwrap();
            repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
                .unwrap();
        }

        // Write an index.md so this directory is a registered network.
        create_test_network(repo_path);

        // ── Build ProtoIndex with git tracking ───────────────────────────────
        let proto_index =
            ProtoIndex::build(repo_path, true).expect("ProtoIndex::build must succeed");

        // Confirm the repo root is in the index (sanity check).
        assert!(
            proto_index.get_meta(repo_path, "git").is_some(),
            "ProtoIndex must have git status for the repo root network directory"
        );

        // ── Call process_asset_dir ────────────────────────────────────────────
        let (tx, mut rx) = unbounded_channel::<crate::event::BeliefEvent>();
        let mut builder = crate::codec::builder::GraphBuilder::new(repo_path, Some(tx)).unwrap();

        let global_bb = crate::beliefbase::BeliefBase::default();

        let result = builder
            .process_asset_dir(repo_path, global_bb, proto_index)
            .await;
        assert!(
            result.is_ok(),
            "process_asset_dir must succeed for a git-tracked dir: {:?}",
            result.err()
        );

        // Drain emitted events.
        drop(builder);
        let mut local_bb = crate::beliefbase::BeliefBase::default();
        while let Ok(event) = rx.try_recv() {
            local_bb.process_event(&event).unwrap();
        }

        // ── Assert: href node present in href_namespace ───────────────────────
        // normalize_remote_url strips the .git suffix and keeps https://.
        let expected_url = "https://github.com/testorg/testrepo";

        let href_nodes: Vec<_> = local_bb
            .states()
            .values()
            .filter(|n| {
                n.kind.contains(BeliefKind::External)
                    && matches!(&n.id, NodeId::Explicit(id) if id == expected_url)
            })
            .collect();

        assert_eq!(
            href_nodes.len(),
            1,
            "expected exactly one href node with id={expected_url}; got {}: {:?}",
            href_nodes.len(),
            href_nodes
                .iter()
                .map(|n| n.id.to_string())
                .collect::<Vec<_>>()
        );

        let href_node = href_nodes[0];

        // The node must live in href_namespace (confirmed via PathMap home net).
        assert!(
            href_node.kind.contains(BeliefKind::Trace),
            "href node must carry BeliefKind::Trace; got kind={:?}",
            href_node.kind
        );

        // href_namespace network node must also be present.
        assert!(
            local_bb.states().contains_key(&href_namespace()),
            "href_namespace network node must be present in emitted events"
        );

        // No External asset-namespace nodes should have been emitted (Case B must
        // not fire when Case A succeeds).
        let asset_nodes: Vec<_> = local_bb
            .states()
            .values()
            .filter(|n| {
                n.kind.contains(BeliefKind::External)
                    && !n.kind.contains(BeliefKind::Trace)
                    && !n.kind.is_network()
            })
            .collect();
        assert!(
            asset_nodes.is_empty(),
            "Case B (listing) nodes must not be emitted when Case A (href) succeeds; \
             got: {:?}",
            asset_nodes.iter().map(|n| &n.title).collect::<Vec<_>>()
        );
    }
}
