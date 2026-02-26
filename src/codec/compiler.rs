use crate::{
    beliefbase::BeliefBase,
    codec::{
        assets::get_stylesheet_urls,
        belief_ir::ProtoBeliefNode,
        builder::GraphBuilder,
        network::{detect_network_file, NetworkCodec, NETWORK_NAME},
        DocCodec, UnresolvedReference, CODECS,
    },
    error::BuildonomyError,
    event::BeliefEvent,
    nodekey::NodeKey,
    paths::{os_path_to_string, string_to_os_path, AnchorPath},
    properties::{
        asset_namespace, buildonomy_namespace, BeliefKind, BeliefNode, Bid, Bref, Weight,
        WeightKind, WEIGHT_DOC_PATHS,
    },
    query::{BeliefSource, Expression, NeighborsExpression, Query},
};

use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    fs,
    io::Write,
    path::{Path, PathBuf},
    time::SystemTime,
};
use toml_edit::value;

/// A wrapper around GraphBuilder that manages recursive document parsing with queue
/// management and loop prevention.
///
/// ## Overview
///
/// This compiler acts as a "filesystem orchestrator" that discovers files, reads content, and
/// feeds it to the builder for parsing. It automatically handles the complex dependency
/// resolution workflow where documents reference each other and need multiple parse passes.
///
/// ## Two-Queue Architecture
///
/// The compiler maintains two separate queues to handle the recursive nature of document parsing:
///
/// ### Primary Queue (Never-Parsed Files)
/// Contains files that have never been parsed in this session. These are processed first using
/// a simple FIFO order. Files are added to this queue when:
/// - Compiler is initialized with an entry point
/// - A parsed document discovers a new dependency
/// - File watcher detects a new file
///
/// ### Reparse Queue (Pending Re-resolution)
/// Contains files that were parsed but had unresolved dependencies. These files need to be
/// re-parsed after their dependencies have been processed so they can inject the resolved BIDs.
/// This queue uses **priority ordering** - files with the fewest unresolved dependencies are
/// processed first, maximizing the likelihood of successful resolution.
///
/// ## Parse Flow Example
///
/// Consider this document structure:
/// ```text
/// network/
///   ├── index.md
///   ├── README.md           → references sub_1.md, sub_2.md
///   ├── sub_1.md            → references sub_2.md
///   └── sub_2.md
/// ```
///
/// Parse sequence:
/// 1. **Parse .noet** (primary queue)
///    - Discovers README.md, sub_1.md, sub_2.md via `ProtoBeliefNode::from_file`
///    - Adds them to primary queue in lexical order
///
/// 2. **Parse README.md** (primary queue)
///    - Contains unresolved links to sub_1.md and sub_2.md
///    - Returns dependent_paths = ["sub_1.md", "sub_2.md"]
///    - These are already in queue, so just track the dependency
///    - README.md added to reparse queue
///
/// 3. **Parse sub_1.md** (primary queue)
///    - Contains unresolved link to sub_2.md
///    - Returns dependent_paths = ["sub_2.md"]
///    - sub_1.md added to reparse queue
///
/// 4. **Parse sub_2.md** (primary queue)
///    - No external dependencies
///    - Returns dependent_paths = []
///    - NOT added to reparse queue
///
/// 5. **Re-parse sub_1.md** (reparse queue, 0 unresolved deps)
///    - Now sub_2.md is in cache with its BID
///    - Link to sub_2.md resolved and injected
///    - Returns dependent_paths = [] (all resolved)
///    - File content rewritten with BID
///
/// 6. **Re-parse README.md** (reparse queue, 0 unresolved deps)
///    - Now both sub_1.md and sub_2.md are in cache
///    - Both links resolved and injected
///    - Returns dependent_paths = []
///    - File content rewritten with BIDs
///
/// ## Loop Prevention
///
/// Each file tracks parse count. If a file is parsed more than `max_reparse_count` times
/// (default: 3), an error is returned. This prevents infinite loops from circular dependencies
/// or bugs.
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
    /// Optional output directory for HTML generation
    html_output_dir: Option<PathBuf>,
    /// Optional JavaScript to inject into generated HTML (e.g., live reload script)
    html_script: Option<String>,
    /// Use CDN for Open Props (requires internet, smaller output)
    use_cdn: bool,
    /// Base URL for sitemap and canonical URLs (e.g., <https://username.github.io/repo>)
    base_url: Option<String>,
    builder: GraphBuilder,
    primary_queue: VecDeque<PathBuf>,
    reparse_queue: VecDeque<PathBuf>,
    pending_dependencies: HashMap<PathBuf, Vec<PathBuf>>,
    processed: HashMap<PathBuf, usize>, // Track parse count per path
    max_reparse_count: usize,           // Prevent infinite loops
    /// Track BIDs of nodes updated since last reparse round
    last_round_updates: HashSet<Bref>,
    /// Whether reparse queue is stable (no new dependencies discovered)
    reparse_stable: bool,
    /// Network files that need HTML generation deferred until all documents are parsed
    deferred_html: HashSet<PathBuf>,
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
    /// * `max_reparse_count` - Maximum times a file can be reparsed (default: 3)
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
    ) -> Result<Self, BuildonomyError> {
        // Copy static assets (CSS, JS, templates) to HTML output directory if configured
        if let Some(ref html_dir) = html_output_dir {
            Self::copy_static_assets(html_dir, use_cdn)?;
        }
        let entry_path = entry_point.as_ref().canonicalize()?;

        let builder = GraphBuilder::new(&entry_path, tx)?;
        let mut primary_queue = VecDeque::new();
        primary_queue.push_back(entry_path);

        Ok(Self {
            write,
            html_output_dir,
            html_script,
            use_cdn,
            base_url,
            builder,
            primary_queue,
            reparse_queue: VecDeque::new(),
            pending_dependencies: HashMap::new(),
            processed: HashMap::new(),
            max_reparse_count: max_reparse_count.unwrap_or(3),
            last_round_updates: HashSet::new(),
            reparse_stable: false,
            deferred_html: HashSet::new(),
        })
    }

    /// Get the HTML output directory if configured
    pub fn html_output_dir(&self) -> Option<&Path> {
        self.html_output_dir.as_deref()
    }

    /// Create a new compiler with an entry point (file or directory) and default arguments: no
    /// receiver of BeliefEvents, default reparse count, and write=false.
    ///
    /// # Arguments
    /// * `entry_point` - The file or directory to start parsing from
    pub fn simple(entry_point: impl AsRef<Path>) -> Result<Self, BuildonomyError> {
        let entry_path = entry_point.as_ref().canonicalize()?;

        let builder = GraphBuilder::new(&entry_path, None)?;
        let mut primary_queue = VecDeque::new();
        primary_queue.push_back(entry_path);

        Ok(Self {
            write: false,
            html_output_dir: None,
            html_script: None,
            use_cdn: false,
            base_url: None,
            builder,
            primary_queue,
            reparse_queue: VecDeque::new(),
            pending_dependencies: HashMap::new(),
            processed: HashMap::new(),
            max_reparse_count: 3,
            last_round_updates: HashSet::new(),
            reparse_stable: false,
            deferred_html: HashSet::new(),
        })
    }

    /// Initialize a directory as a BeliefNetwork by placing an index.md file with the
    /// input arguments at that location.
    pub async fn create_network_file<P>(
        repo_path: P,
        id: &str,
        maybe_title: Option<String>,
        maybe_summary: Option<String>,
    ) -> Result<PathBuf, BuildonomyError>
    where
        P: AsRef<std::path::Path> + std::fmt::Debug,
    {
        let net_codec = NetworkCodec::default();
        if net_codec
            .proto(repo_path.as_ref(), repo_path.as_ref())?
            .is_some()
        {
            return Err(BuildonomyError::Codec(format!(
                "Network file at path {repo_path:?} is already initialized."
            )));
        }

        let mut proto = ProtoBeliefNode::default();

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
        file.write_all(&format!("---{}\n---\n", proto.document).into_bytes())?;
        Ok(file_path)
    }

    /// Parse the next item in the queue, returning None if queue is empty
    ///
    /// This method prioritizes the primary queue (never-parsed files) over the reparse queue.
    /// For the reparse queue, it selects files with the most resolved dependencies first.
    ///
    /// # Arguments
    /// * `global_bb` - The belief cache to query during parsing (typically a DbConnection)
    ///
    /// # Returns
    /// * `Ok(Some(ParseResult))` - Successfully parsed a document
    /// * `Ok(None)` - Queue is empty, nothing to parse
    /// * `Err(_)` - Parse error or infinite loop detected
    pub async fn parse_next<B: BeliefSource + Clone>(
        &mut self,
        global_bb: B,
    ) -> Result<Option<ParseResult>, BuildonomyError> {
        // 1. PEEK at next item (don't pop until we have a successful parse)
        let path = if let Some(p) = self.primary_queue.front() {
            p.clone()
        } else if let Some(p) = self.peek_next_reparse_candidate() {
            p.clone()
        } else {
            self.finalize().await?;
            let Some(path) = self.primary_queue.front().cloned() else {
                tracing::info!("[Compiler] No cached assets to verify, parsing complete");
                return Ok(None);
            };
            path
        };

        // 2a. Check parse count before attempting
        let parse_count = self.processed.get(&path).copied().unwrap_or(0);

        if parse_count >= self.max_reparse_count {
            // Max retries reached - remove from queues and return with error diagnostic
            self.remove_from_queues(&path);
            tracing::warn!(
                "[Compiler] Max reparse limit reached for {:?} ({} attempts)",
                path,
                parse_count
            );

            return Ok(Some(ParseResult {
                path: path.clone(),
                rewritten_content: None,
                dependent_paths: Vec::new(),
                diagnostics: vec![crate::codec::ParseDiagnostic::parse_error(
                    format!("Max reparse limit ({}) reached", self.max_reparse_count),
                    parse_count,
                )],
            }));
        }

        // 2b. Increment parse count
        *self.processed.entry(path.clone()).or_insert(0) += 1;
        tracing::info!(
            "\n \
            Parsing file {}\n \
            ============={}\n \
            (attempt {}/{}",
            path.to_string_lossy(),
            "=".repeat(path.to_string_lossy().len()),
            parse_count + 1,
            self.max_reparse_count
        );

        // 3. Determine the actual file path (may differ from path if path is a directory)
        let file_path = if path.is_dir() {
            // BeliefNetwork directories are enqueued as the directory, not the contained
            // index file.
            if let Some(detected_path) = detect_network_file(&path) {
                detected_path
            } else {
                return Err(BuildonomyError::Codec(
                    "Cannot parse a directory without an index file".to_string(),
                ));
            }
        } else {
            path.clone()
        };

        // 3a. Check if this is an asset file (not a known document codec extension)
        if !file_path.is_dir() && CODECS.path_get(&file_path).is_none() {
            return self.process_asset(path).await;
        }

        // 4. Try to read the file
        let content = {
            match tokio::fs::read_to_string(&file_path).await {
                Ok(c) => c,
                Err(e) => {
                    // IO error - return as diagnostic
                    tracing::warn!("[Compiler] Failed to read {:?}: {}", path, e);

                    // Remove from queues (file might be deleted or inaccessible)
                    self.remove_from_queues(&path);

                    return Ok(Some(ParseResult {
                        path: path.clone(),
                        rewritten_content: None,
                        dependent_paths: Vec::new(),
                        diagnostics: vec![crate::codec::ParseDiagnostic::parse_error(
                            format!("Failed to read file: {e}"),
                            parse_count + 1,
                        )],
                    }));
                }
            }
        };

        // 5. Try to parse the content
        let (mut parse_result, codec) = match self
            .builder
            .parse_content(&path, content, global_bb.clone())
            .await
        {
            Ok(with_codec) => (with_codec.result, with_codec.codec),
            Err(e) => {
                // Parse error - return as diagnostic
                tracing::warn!("[Compiler] Failed to parse {:?}: {}", path, e);

                return Ok(Some(ParseResult {
                    path: path.clone(),
                    rewritten_content: None,
                    dependent_paths: Vec::new(),
                    diagnostics: vec![crate::codec::ParseDiagnostic::parse_error(
                        format!("Parse failed: {e}"),
                        parse_count + 1,
                    )],
                }));
            }
        };

        // 6. SUCCESS! Now we can safely remove from queues
        self.remove_from_queues(&path);

        // 6a. Track file mtime for cache invalidation
        self.builder
            .tx()
            .send(crate::event::BeliefEvent::FileParsed(file_path.clone()))?;

        // 7. Write rewritten content if available
        if let Some(contents) = parse_result.rewritten_content.as_ref() {
            if self.write {
                tracing::info!("[Compiler] Writing rewritten content to {:?}", file_path);
                if let Err(e) = tokio::fs::write(&file_path, contents).await {
                    // Write error - add as warning but continue
                    parse_result
                        .diagnostics
                        .push(crate::codec::ParseDiagnostic::warning(format!(
                            "Failed to write rewritten content: {e}"
                        )));
                }
            } else {
                tracing::debug!(
                    "[Compiler] Write disabled, skipping file write for {:?}",
                    file_path
                );
            }
        }

        // 7a. Phase 1: Try immediate HTML generation
        if let Some(html_dir) = &self.html_output_dir {
            // Get title from first node (document node)
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
                        .and_then(|b_val| b_val.as_str().and_then(|b| Bid::try_from(b).ok()))
                        .unwrap_or(Bid::nil());
                    (bid, title)
                })
                .unwrap_or((Bid::nil(), "No doc node found".to_string()));

            match codec.generate_html() {
                Ok(fragments) => {
                    // Convert absolute path to repo-relative path
                    let repo_relative_path = file_path
                        .strip_prefix(self.builder.repo_root())
                        .unwrap_or(file_path.as_path());

                    // Get base directory for output (always use parent directory, never the file itself)
                    let base_dir = repo_relative_path.parent().unwrap_or(Path::new(""));

                    for (filename, html_body) in fragments {
                        // Join base directory with filename to get relative path
                        let rel_path = base_dir.join(&filename);

                        if let Err(e) = self
                            .write_fragment(html_dir, &rel_path, html_body, &title, &bid)
                            .await
                        {
                            parse_result
                                .diagnostics
                                .push(crate::codec::ParseDiagnostic::warning(format!(
                                    "Failed to write HTML fragment {}: {e}",
                                    rel_path.display()
                                )));
                        }
                    }
                }
                Err(e) => {
                    parse_result
                        .diagnostics
                        .push(crate::codec::ParseDiagnostic::warning(format!(
                            "Failed to generate HTML: {e}"
                        )));
                }
            }

            // 7b. Queue for deferred generation if codec requests it
            if codec.should_defer() {
                tracing::debug!(
                    "[Compiler] Queueing for deferred HTML generation: {:?}",
                    file_path
                );
                self.deferred_html.insert(path.clone());
            }
        }

        // 8. Extract dependent paths from SinkDependency diagnostics
        let unresolved_references: Vec<&UnresolvedReference> = parse_result
            .diagnostics
            .iter()
            .filter_map(|d| d.as_unresolved_reference())
            .collect();

        let mut dependent_paths = Vec::<(String, Bref)>::new();

        if !unresolved_references.is_empty() && !self.reparse_queue.contains(&path) {
            // tracing::debug!(
            //     "[Compiler] File {:?} has unresolved references, adding to reparse queue",
            //     path
            // );
            self.reparse_queue.push_back(path.clone());
            // New file with unresolved refs means reparse queue is not stable
            self.reparse_stable = false;
        }

        // 9. Handle dependent paths (files that need this file)
        for unresolved in unresolved_references.iter() {
            // 9a. Check if this is an asset reference (NodeKey::Path with net == asset_namespace)
            let is_asset_reference = unresolved.other_keys.iter().any(|key| {
                if let NodeKey::Path { net, .. } = key {
                    *net == asset_namespace().bref()
                } else {
                    false
                }
            });

            if is_asset_reference {
                self.process_asset_reference(&path, unresolved);
            } else {
                let Some((net_dep_path_str, net)) = unresolved.as_sink_dependency() else {
                    continue;
                };
                self.process_unresolved_reference(&path, &net_dep_path_str, net);
                dependent_paths.push((net_dep_path_str, net));
            }
        }

        // 10. Clean up resolved dependencies
        if unresolved_references.is_empty() && self.pending_dependencies.contains_key(&path) {
            self.pending_dependencies.remove(&path);
        }

        Ok(Some(ParseResult {
            path,
            rewritten_content: parse_result.rewritten_content,
            dependent_paths,
            diagnostics: parse_result.diagnostics,
        }))
    }

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
            tracing::info!(
                "Force re-parse enabled, will re-parse {} files",
                doc_paths.len()
            );
            doc_paths
        } else {
            // Query cached mtimes
            let cached_mtimes = cache.get_file_mtimes().await?;
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
                                tracing::info!(
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
                                tracing::info!(
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

    /// Parse all items in the queue until empty or error
    ///
    /// This method will continue parsing until both the primary and reparse queues are empty,
    /// or until an unrecoverable error occurs.
    ///
    /// # Arguments
    /// * `global_bb` - The belief cache to query during parsing
    /// * `force` - If true, force re-parse all files ignoring cache
    ///
    /// # Returns
    /// * `Ok(Vec<ParseResult>)` - All successfully parsed documents
    /// * `Err(_)` - First unrecoverable error encountered (parsing stops on error)
    pub async fn parse_all<B: BeliefSource + Clone>(
        &mut self,
        global_bb: B,
        force: bool,
    ) -> Result<Vec<ParseResult>, BuildonomyError> {
        // Check for stale files (or all files if force=true)
        let stale_files = self.check_stale_files(&global_bb, force).await?;

        if !stale_files.is_empty() {
            let action = if force {
                "force re-parse"
            } else {
                "modified/deleted files, will re-parse"
            };
            tracing::info!("Found {} files to {}", stale_files.len(), action);

            for path in stale_files {
                self.enqueue(path);
            }
        }

        let mut results = Vec::new();

        while let Some(result) = self.parse_next(global_bb.clone()).await? {
            results.push(result);
        }

        Ok(results)
    }

    pub fn cache(&self) -> &BeliefBase {
        self.builder().session_bb()
    }

    /// Peek at the next file from the reparse queue without removing it
    ///
    /// Files with the fewest unresolved dependencies are prioritized first.
    fn peek_next_reparse_candidate(&mut self) -> Option<PathBuf> {
        if self.reparse_queue.is_empty() {
            return None;
        }

        // If primary queue is empty and reparse queue was stable (no new dependencies
        // discovered in last round), only reparse if we've seen new node updates
        if self.primary_queue.is_empty() && self.reparse_stable {
            if self.last_round_updates.is_empty() {
                tracing::debug!(
                    "[Compiler] Reparse queue stable and no new updates - skipping reparse round"
                );
                return None;
            } else {
                tracing::debug!(
                    "[Compiler] Reparse queue stable but {} new updates detected - proceeding",
                    self.last_round_updates.len()
                );
            }
        }

        // Find the file with the fewest unresolved dependencies
        let (best_idx, _) = self
            .reparse_queue
            .iter()
            .enumerate()
            .map(|(idx, path)| {
                let unresolved_count = self
                    .pending_dependencies
                    .get(path)
                    .map(|deps| {
                        deps.iter()
                            .filter(|d| !self.processed.contains_key(*d))
                            .count()
                    })
                    .unwrap_or(0);
                (idx, unresolved_count)
            })
            .min_by_key(|(_, count)| *count)?;

        self.reparse_queue.get(best_idx).cloned()
    }

    // /// Select the next file from the reparse queue, prioritizing by resolution impact
    // ///
    // /// Files with the fewest unresolved dependencies are processed first, as they are
    // /// most likely to complete successfully and unblock other files.
    // fn next_reparse_candidate(&mut self) -> Option<PathBuf> {
    //     if self.reparse_queue.is_empty() {
    //         return None;
    //     }

    //     // Find the file with the fewest unresolved dependencies
    //     let (best_idx, _) = self
    //         .reparse_queue
    //         .iter()
    //         .enumerate()
    //         .map(|(idx, path)| {
    //             let unresolved_count = self
    //                 .pending_dependencies
    //                 .get(path)
    //                 .map(|deps| {
    //                     deps.iter()
    //                         .filter(|d| !self.processed.contains_key(*d))
    //                         .count()
    //                 })
    //                 .unwrap_or(0);
    //             (idx, unresolved_count)
    //         })
    //         .min_by_key(|(_, count)| *count)?;

    //     self.reparse_queue.remove(best_idx)
    // }

    /// Add a path to the queue (e.g., from file watcher)
    ///
    /// This method checks if the path is already in either queue to avoid duplicates.
    /// New paths are added to the primary queue.
    /// Enqueue a path for parsing if not already queued
    pub fn enqueue(&mut self, path: impl AsRef<Path>) {
        let path = path.as_ref().to_path_buf();
        if !self.primary_queue.contains(&path) && !self.reparse_queue.contains(&path) {
            // tracing::debug!("[Compiler] Enqueuing path: {:?}", path);
            self.primary_queue.push_back(path);
        }
    }

    /// Enqueue a path at the front of the primary queue (for prioritized parsing like file modifications)
    pub fn enqueue_front(&mut self, path: impl AsRef<Path>) {
        let path = path.as_ref().to_path_buf();
        // Remove from reparse queue if present (fresh content takes precedence)
        self.reparse_queue.retain(|p| p != &path);

        if !self.primary_queue.contains(&path) {
            // tracing::debug!("[Compiler] Enqueuing path at front (priority): {:?}", path);
            self.primary_queue.push_front(path);
        }
    }

    /// Handle file modification event (reset parse count and prioritize)
    pub fn on_file_modified(&mut self, path: impl AsRef<Path>) {
        let path = path.as_ref().to_path_buf();

        // Reset parse count - it's fresh content
        self.processed.remove(&path);

        // Enqueue at front for priority parsing
        self.enqueue_front(path);
    }

    /// Handle file deletion event (clean up all tracking)
    pub fn on_file_deleted(&mut self, path: impl AsRef<Path>) {
        let path = path.as_ref().to_path_buf();

        self.remove_from_queues(&path);
        self.processed.remove(&path);
        self.pending_dependencies.remove(&path);
    }

    /// Reset the parse count for a path (useful for file watcher re-parses)
    ///
    /// This allows a file to be re-parsed even if it has already been processed,
    /// which is necessary when the file changes on disk.
    pub fn reset_processed(&mut self, path: impl AsRef<Path>) {
        let path = path.as_ref();
        self.processed.remove(path);
        // Also clear any pending dependency tracking for this file
        self.pending_dependencies.remove(path);
    }

    /// Clear all processed tracking (for fresh re-parse of entire tree)
    ///
    /// This resets the parse count for all files but keeps the queue state.
    /// Useful when you want to re-parse everything from scratch while maintaining
    /// the builder's session_bb.
    pub fn clear_processed(&mut self) {
        self.processed.clear();
        self.pending_dependencies.clear();
    }

    /// Remove a path from all queues
    fn remove_from_queues(&mut self, path: &PathBuf) {
        self.primary_queue.retain(|p| p != path);
        self.reparse_queue.retain(|p| p != path);
    }

    async fn finalize(&mut self) -> Result<(), BuildonomyError> {
        // Both queues empty - check if there are cached assets to verify
        tracing::info!("[Compiler] Both queues empty, checking for cached assets");

        // Query session_bb for assets discovered during this parse session
        // (mtime-based invalidation via check_stale_files handles cached assets)
        let assets: Vec<(String, Bid)> = self
            .builder
            .session_bb()
            .get_network_paths(asset_namespace())
            .await
            .unwrap_or_default();

        tracing::info!("[Compiler] Found {} cached assets to check", assets.len());

        // Enqueue any assets not yet processed in this session
        let mut newly_enqueued = 0;
        for (repo_relative_path, _bid) in assets.iter() {
            // Skip empty path (represents the network node itself, not an asset file)
            if repo_relative_path.is_empty() {
                continue;
            }

            let asset_absolute_path = self
                .builder
                .repo_root()
                .join(string_to_os_path(repo_relative_path));

            if !self.processed.contains_key(&asset_absolute_path) {
                tracing::info!(
                    "[Compiler] Enqueuing cached asset for content check: {:?}",
                    asset_absolute_path
                );
                self.primary_queue.push_back(asset_absolute_path);
                newly_enqueued += 1;
            }
        }

        if newly_enqueued > 0 {
            tracing::info!(
                "[Compiler] Enqueued {} cached assets for content verification",
                newly_enqueued
            );
            // Continue to process the newly enqueued assets
        } else {
            // Generate HTML outputs now that all documents are parsed
            if self.html_output_dir().is_some() {
                self.generate_spa_shell().await?;
            }
        }

        tracing::debug!(
            "State of session_bb at finalize:\n{}\n{}",
            self.builder().session_bb().clone().consume(),
            self.builder().session_bb().paths()
        );

        Ok(())
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
    ) -> Result<(), BuildonomyError> {
        if self.html_output_dir().is_none() {
            return Ok(()); // No HTML output configured
        }

        // Generate deferred HTML with synchronized context
        self.generate_deferred_html(global_bb.clone()).await?;

        // Generate sitemap from document paths
        self.generate_sitemap(global_bb.clone()).await?;

        // Query synchronized global_bb for asset manifest
        let asset_manifest: BTreeMap<String, Bid> = global_bb
            .get_all_document_paths(asset_namespace())
            .await
            .unwrap_or_default()
            .into_iter()
            .collect();

        self.create_asset_hardlinks(&asset_manifest).await?;

        // Export BeliefGraph to JSON for client-side use
        let graph = global_bb.export_beliefgraph().await?;
        self.export_beliefbase_json(graph).await?;

        Ok(())
    }

    async fn process_asset(
        &mut self,
        path: PathBuf,
    ) -> Result<Option<ParseResult>, BuildonomyError> {
        // 3. Determine the actual file path (may differ from path if path is a directory)
        let file_path = if path.is_dir() {
            // BeliefNetwork directories are enqueued as the directory, not the contained
            // index.md file.
            if let Some(detected_path) = detect_network_file(&path) {
                detected_path
            } else {
                return Err(BuildonomyError::Codec(
                    "Cannot parse a directory without an index file".to_string(),
                ));
            }
        } else {
            path.clone()
        };

        // 2a. Check parse count before attempting
        let parse_count = self.processed.get(&path).copied().unwrap_or(0);

        // This is an asset file - process it as a static asset
        tracing::info!("[Compiler] Detected asset file: {:?}", file_path);

        // Read file bytes and compute SHA256 hash
        let file_bytes = match tokio::fs::read(&file_path).await {
            Ok(bytes) => bytes,
            Err(e) => {
                tracing::warn!("[Compiler] Failed to read asset {:?}: {}", file_path, e);
                self.remove_from_queues(&path);
                return Ok(Some(ParseResult {
                    path: path.clone(),
                    rewritten_content: None,
                    dependent_paths: Vec::new(),
                    diagnostics: vec![crate::codec::ParseDiagnostic::parse_error(
                        format!("Failed to read asset file: {e}"),
                        parse_count + 1,
                    )],
                }));
            }
        };

        // Compute SHA256 hash of file content
        let mut hasher = Sha256::new();
        hasher.update(&file_bytes);
        let hash_bytes = hasher.finalize();
        let hash_str = format!("{:x}", hash_bytes);

        // Get repo-relative path for this asset
        let repo_relative_path = file_path
            .strip_prefix(self.builder.repo_root())
            .unwrap_or(&file_path)
            .to_string_lossy()
            .replace('\\', "/");

        // Check if asset already tracked at this path
        // session_bb is now populated with assets from global_bb via initialize_stack
        let maybe_existing = self
            .builder
            .session_bb()
            .paths()
            .net_get_from_path(&asset_namespace().bref(), &repo_relative_path)
            .map(|(_, bid)| {
                let node = self.builder.session_bb().states().get(&bid);
                let existing_hash = node
                    .and_then(|n| n.payload.get("content_hash"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                (bid, existing_hash.to_string())
            });

        // Determine if we need to update based on hash comparison
        let (asset_bid, needs_update) = match maybe_existing {
            Some((bid, existing_hash)) if existing_hash == hash_str => {
                // Path exists with SAME hash → Skip (no change)
                tracing::debug!(
                    "[Compiler] Asset unchanged: {:?} (BID: {})",
                    repo_relative_path,
                    bid
                );
                (bid, false)
            }
            Some((bid, existing_hash)) => {
                // Path exists with DIFFERENT hash → Content changed
                tracing::info!(
                    "[Compiler] Asset content changed: {:?} (BID: {}, old hash: {}, new hash: {})",
                    repo_relative_path,
                    bid,
                    existing_hash,
                    hash_str
                );
                (bid, true)
            }
            None => {
                // Path doesn't exist → New asset, generate stable UUID
                let new_bid = Bid::new(asset_namespace());
                tracing::info!(
                    "[Compiler] New asset discovered: {:?} (BID: {})",
                    repo_relative_path,
                    new_bid
                );
                (new_bid, true)
            }
        };

        if needs_update {
            // Create asset BeliefNode with content_hash in payload
            let mut payload = toml::Table::new();
            payload.insert("content_hash".to_string(), toml::Value::String(hash_str));

            let asset_node = BeliefNode {
                bid: asset_bid,
                kind: BeliefKind::External.into(),
                payload,
                ..Default::default()
            };

            // Build NodeKey array - single BID for update (not rename)
            let node_keys = vec![NodeKey::Bid { bid: asset_bid }];

            let mut update_queue = Vec::new();

            // Ensure asset_namespace network node exists before creating relations
            if !self
                .builder
                .session_bb()
                .states()
                .contains_key(&asset_namespace())
            {
                let asset_net_node = BeliefNode::asset_network();
                update_queue.push(BeliefEvent::NodeUpdate(
                    asset_net_node.keys(
                        Some(buildonomy_namespace()),
                        None,
                        self.builder.session_bb(),
                    ),
                    asset_net_node.toml(),
                    crate::event::EventOrigin::Remote,
                ));
                update_queue.push(BeliefEvent::RelationChange(
                    asset_namespace(),
                    buildonomy_namespace(),
                    WeightKind::Section,
                    None,
                    crate::event::EventOrigin::Remote,
                ));
            }

            update_queue.push(BeliefEvent::NodeUpdate(
                node_keys,
                asset_node.toml(),
                crate::event::EventOrigin::Remote,
            ));

            // Create Section relation to asset_namespace with repo-relative path
            let mut edge_payload = toml::Table::new();
            edge_payload.insert(
                WEIGHT_DOC_PATHS.to_string(),
                toml::Value::Array(vec![toml::Value::String(repo_relative_path.clone())]),
            );
            let weight = Weight {
                payload: edge_payload,
            };

            update_queue.push(BeliefEvent::RelationChange(
                asset_bid,
                asset_namespace(),
                WeightKind::Section,
                Some(weight),
                crate::event::EventOrigin::Remote,
            ));

            // Process into session_bb
            let mut derivatives = Vec::new();
            for event in update_queue.iter() {
                derivatives.append(&mut self.builder.session_bb_mut().process_event(event)?);
            }
            update_queue.append(&mut derivatives);

            // Process into doc_bb so assets are available for cache lookups
            for event in update_queue.iter() {
                self.builder.doc_bb_mut().process_event(event)?;
            }

            // Send to global cache via tx
            for event in update_queue.into_iter() {
                self.builder.tx().send(event)?;
            }

            // Emit FileParsed event for mtime tracking
            self.builder
                .tx()
                .send(BeliefEvent::FileParsed(path.clone()))?;

            tracing::info!(
                "[Compiler] Asset processed successfully: {:?}",
                repo_relative_path
            );
        }

        // Remove from queues and return success
        self.remove_from_queues(&path);
        Ok(Some(ParseResult {
            path: path.clone(),
            rewritten_content: None,
            dependent_paths: Vec::new(),
            diagnostics: Vec::new(),
        }))
    }

    fn process_asset_reference(&mut self, path: &PathBuf, unresolved: &UnresolvedReference) {
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
            // Resolve relative markdown link to absolute filesystem path
            let path_string = os_path_to_string(path);
            let path_ap = AnchorPath::new(&path_string);
            let repo_root = os_path_to_string(self.builder.repo_root());
            let asset_absolute_path = path_ap.join(asset_relative_path);
            if let Some(repo_relative_asset) = asset_absolute_path
                .as_anchor_path()
                .strip_prefix(&repo_root)
            {
                let absolute_path = string_to_os_path(&*asset_absolute_path);
                // Always enqueue asset files to check for content changes
                // even if already tracked in session_bb
                if !self.processed.contains_key(&absolute_path)
                    && !self.primary_queue.contains(&absolute_path)
                    && !self.reparse_queue.contains(&absolute_path)
                {
                    tracing::debug!(
                        "[Compiler] Queueing asset file for content check: {:?}",
                        asset_absolute_path
                    );
                    self.primary_queue.push_back(absolute_path);
                }

                // Check if asset already tracked via BeliefBase
                let asset_already_tracked = self
                    .builder
                    .session_bb()
                    .paths()
                    .net_get_from_path(&asset_namespace().bref(), repo_relative_asset)
                    .is_some();

                if !asset_already_tracked {
                    // Asset not yet in session_bb - document needs reparse after asset loads
                    tracing::info!(
                        "[Compiler] Document {:?} references untracked asset: {:?}",
                        path,
                        asset_absolute_path
                    );

                    // Add document to reparse queue (will reparse after asset is processed)
                    if !self.reparse_queue.contains(path) {
                        tracing::debug!(
                            "[Compiler] Adding document {:?} to reparse queue (awaiting asset)",
                            path
                        );
                        self.reparse_queue.push_back(path.clone());
                        self.reparse_stable = false;
                    }
                } else {
                    tracing::debug!(
                        "[Compiler] Asset already tracked: {:?}",
                        repo_relative_asset
                    );
                    // Asset file doesn't exist - leave as unresolved reference
                }
            } else {
                tracing::warn!(
                    "[Compiler] Cannot resolve asset path {:?} from document {:?}. Absolute path: {}, repo root: {:?}",
                    asset_relative_path,
                    path,
                    asset_absolute_path,
                    repo_root
                );
            }
        }
    }

    fn process_unresolved_reference(&mut self, path: &Path, net_dep_path_str: &str, net_ref: Bref) {
        let repo_pathmap = self
            .builder()
            .doc_bb()
            .paths()
            .get_map(&self.builder().repo().bref())
            .expect("builder.repo to be instantiated after parse_content was successfully called.");
        let Some(net) = self
            .builder()
            .doc_bb()
            .paths()
            .nets()
            .iter()
            .find(|net| net.bref() == net_ref)
            .copied()
        else {
            tracing::warn!("self.bulder().doc_bb() does not have a network node with bref {} initialized in its pathmapmap", net_ref);
            return;
        };
        let full_dep_path = if let Some((_home_net, net_path, _order)) =
            repo_pathmap.path(&net, &self.builder().doc_bb().paths())
        {
            debug_assert!(_home_net == net);
            // Convert relative path to absolute
            let dep_path = string_to_os_path(
                &AnchorPath::new(&net_path)
                    .join(net_dep_path_str)
                    .into_string(),
            );
            // Resolve relative to builder's repo_root
            self.builder.repo_root().join(dep_path)
        } else {
            tracing::warn!(
                "No connectivity between builder.repo and dependent path network {}",
                net
            );
            return;
        };

        // Canonicalize if it exists
        let canonical_dep_path = match full_dep_path.canonicalize() {
            Ok(p) => p,
            Err(_) => {
                tracing::debug!(
                    "[Compiler] Cannot canonicalize {:?}, treating as external",
                    full_dep_path
                );
                return; // Skip external/non-existent dependencies
            }
        };
        // Enqueue dependency if not already processed or queued
        if !self.processed.contains_key(&canonical_dep_path)
            && !self.primary_queue.contains(&canonical_dep_path)
            && !self.reparse_queue.contains(&canonical_dep_path)
        {
            // tracing::debug!(
            //     "[Compiler] Enqueuing new dependency: {:?}",
            //     canonical_dep_path
            // );
            self.primary_queue.push_back(canonical_dep_path.clone());
        }

        // Track this dependency relationship
        self.pending_dependencies
            .entry(path.to_path_buf())
            .or_default()
            .push(canonical_dep_path);
    }
    /// Check if there are pending items to parse
    pub fn has_pending(&self) -> bool {
        !self.primary_queue.is_empty() || !self.reparse_queue.is_empty()
    }

    /// Get the number of items in the primary parse queue
    pub fn primary_queue_len(&self) -> usize {
        self.primary_queue.len()
    }

    /// Get the number of items in the reparse queue
    pub fn reparse_queue_len(&self) -> usize {
        self.reparse_queue.len()
    }

    /// Get the total number of items across both queues
    pub fn total_queue_len(&self) -> usize {
        self.primary_queue.len() + self.reparse_queue.len()
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

    /// Get statistics about the compiler state (useful for debugging)
    pub fn stats(&self) -> CompilerStats {
        CompilerStats {
            primary_queue_len: self.primary_queue.len(),
            reparse_queue_len: self.reparse_queue.len(),
            processed_count: self.processed.len(),
            pending_dependencies_count: self.pending_dependencies.len(),
            total_parses: self.processed.values().sum(),
        }
    }

    /// Notify compiler of belief events (e.g., from event stream)
    ///
    /// This allows the compiler to track when new nodes are created/updated,
    /// enabling smarter reparse decisions. Only reparse if we've seen updates
    /// that could resolve pending dependencies.
    pub fn on_belief_event(&mut self, event: &BeliefEvent) {
        match event {
            BeliefEvent::NodeUpdate(keys, _, _) => {
                // Extract BIDs from keys and track them
                for key in keys {
                    match key {
                        NodeKey::Bid { bid } => {
                            self.last_round_updates.insert(bid.bref());
                        }
                        NodeKey::Bref { .. } => {
                            // Brefs don't have BIDs, skip
                        }
                        NodeKey::Path { net, .. } | NodeKey::Id { net, .. } => {
                            // Track network BID as a proxy for potential matches
                            if *net != Bref::default() {
                                self.last_round_updates.insert(*net);
                            }
                        }
                    }
                }
                // New updates mean reparse might be productive
                self.reparse_stable = false;
            }
            BeliefEvent::PathAdded(_, _, bid, _, _) | BeliefEvent::PathUpdate(_, _, bid, _, _) => {
                self.last_round_updates.insert(bid.bref());
                self.reparse_stable = false;
            }
            BeliefEvent::NodesRemoved(bids, _) => {
                for bid in bids {
                    self.last_round_updates.remove(&bid.bref());
                }
            }
            _ => {}
        }
    }

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

        tracing::info!(
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
        tracing::info!(
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

        tracing::info!(
            "[generate_deferred_html] Generating HTML for {} deferred network files",
            self.deferred_html.len()
        );

        for file_path in self.deferred_html.iter() {
            tracing::info!(
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
        use crate::codec::assets::{get_template, Layout};
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

        tracing::info!(
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
            .get_all_document_paths(repo_bid)
            .await
            .unwrap_or_default();

        tracing::info!(
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

        tracing::info!(
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
        use crate::codec::assets::{get_template, Layout};
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
        // Query for the node using path from synchronized global_bb
        let nodekey = NodeKey::Path {
            net: self.builder.repo().bref(),
            path: path_str.clone(),
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

        // Get title and generate fragments based on context availability
        let title = ctx.node.display_title().to_string();
        let fragments = codec.generate_deferred_html(&ctx)?;

        // Convert absolute path to repo-relative path
        let repo_relative_path = source_path
            .strip_prefix(self.builder.repo_root())
            .unwrap_or(source_path);

        // Get base directory for output (ctx.path for directories, parent for files)
        // ctx.path is home-network relative, so for network nodes it's just the network name
        // For document files, use the parent directory
        let base_dir = if ctx.node.kind.is_network() {
            // Network nodes: ctx.path is the network directory (may be subnet path)
            repo_relative_path
        } else {
            // Document nodes: use parent directory of the source file
            repo_relative_path.parent().unwrap_or(Path::new(""))
        };

        for (filename, html_body) in fragments.into_iter() {
            // Join base directory with filename to get relative path
            let rel_path = base_dir.join(&filename);

            // Write fragment using helper with title and BID
            self.write_fragment(html_output_dir, &rel_path, html_body, &title, &node.bid)
                .await?;
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
        use std::collections::HashSet;

        if manifest_data.is_empty() {
            return Ok(());
        }
        let Some(html_output_dir) = self.html_output_dir() else {
            return Ok(());
        };

        tracing::info!(
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

        tracing::info!(
            "[Compiler] Asset hardlinks created: {} unique files, {} total paths",
            copied_canonical.len(),
            manifest_data.len()
        );

        Ok(())
    }

    /// Mark that we've completed a reparse round
    ///
    /// Call this when primary queue is empty and we're about to start a reparse round.
    /// This allows tracking whether the reparse queue is stable (no new files discovered).
    pub fn start_reparse_round(&mut self) {
        if self.primary_queue.is_empty() && !self.reparse_queue.is_empty() {
            let had_updates = !self.last_round_updates.is_empty();
            self.last_round_updates.clear();

            if !had_updates {
                self.reparse_stable = true;
                tracing::debug!("[Compiler] Reparse round starting with stable queue");
            } else {
                tracing::debug!("[Compiler] Reparse round starting with new updates");
            }
        }
    }
}

/// Statistics about the compiler's current state
#[derive(Debug, Clone, Default)]
pub struct CompilerStats {
    pub primary_queue_len: usize,
    pub reparse_queue_len: usize,
    pub processed_count: usize,
    pub pending_dependencies_count: usize,
    pub total_parses: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(compiler.has_pending());
        assert_eq!(compiler.primary_queue_len(), 1);
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
        assert_eq!(stats.primary_queue_len, 1);
        assert_eq!(stats.reparse_queue_len, 0);
        assert_eq!(stats.processed_count, 0);
        assert_eq!(stats.total_parses, 0);
    }
}
