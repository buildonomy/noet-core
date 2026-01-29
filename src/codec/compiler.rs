use crate::{
    beliefbase::BeliefBase,
    codec::{
        belief_ir::{detect_network_file, ProtoBeliefNode, NETWORK_CONFIG_NAMES},
        builder::GraphBuilder,
        UnresolvedReference, CODECS,
    },
    error::BuildonomyError,
    event::BeliefEvent,
    nodekey::NodeKey,
    properties::{
        asset_namespace, buildonomy_namespace, BeliefKind, BeliefNode, Bid, Weight, WeightKind,
        WEIGHT_DOC_PATHS,
    },
    query::BeliefSource,
};
use parking_lot::RwLock;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
    sync::Arc,
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
///   ├── BeliefNetwork.toml
///   ├── README.md           → references sub_1.md, sub_2.md
///   ├── sub_1.md            → references sub_2.md
///   └── sub_2.md
/// ```
///
/// Parse sequence:
/// 1. **Parse BeliefNetwork.toml** (primary queue)
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
    builder: GraphBuilder,
    primary_queue: VecDeque<PathBuf>,
    reparse_queue: VecDeque<PathBuf>,
    pending_dependencies: HashMap<PathBuf, Vec<PathBuf>>,
    processed: HashMap<PathBuf, usize>, // Track parse count per path
    max_reparse_count: usize,           // Prevent infinite loops
    /// Track BIDs of nodes updated since last reparse round
    last_round_updates: HashSet<Bid>,
    /// Whether reparse queue is stable (no new dependencies discovered)
    reparse_stable: bool,
    /// Asset tracking for file watcher integration.
    /// Maps repo-relative asset paths to content-addressed BIDs.
    /// Values may be identical for symlinks/copies (same content = same BID).
    ///
    /// Wrapped in Arc<RwLock<_>> for cross-thread access:
    /// - DocumentCompiler writes during compilation
    /// - FileWatcher reads to determine if filesystem events are relevant
    asset_manifest: Arc<RwLock<BTreeMap<String, Bid>>>,
}

/// Result of parsing a single document
#[derive(Debug, Clone)]
pub struct ParseResult {
    pub path: PathBuf,
    pub rewritten_content: Option<String>,
    pub dependent_paths: Vec<(String, Bid)>,
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
        Self::with_html_output(entry_point, tx, max_reparse_count, write, None)
    }

    /// Create a new compiler with HTML output enabled
    pub fn with_html_output(
        entry_point: impl AsRef<Path>,
        tx: Option<tokio::sync::mpsc::UnboundedSender<BeliefEvent>>,
        max_reparse_count: Option<usize>,
        write: bool,
        html_output_dir: Option<PathBuf>,
    ) -> Result<Self, BuildonomyError> {
        // Copy static assets (CSS) to HTML output directory if configured
        if let Some(ref html_dir) = html_output_dir {
            Self::copy_static_assets(html_dir)?;
        }
        let entry_path = entry_point.as_ref().canonicalize()?;

        let builder = GraphBuilder::new(&entry_path, tx)?;
        let mut primary_queue = VecDeque::new();
        primary_queue.push_back(entry_path);

        Ok(Self {
            write,
            html_output_dir,
            builder,
            primary_queue,
            reparse_queue: VecDeque::new(),
            pending_dependencies: HashMap::new(),
            processed: HashMap::new(),
            max_reparse_count: max_reparse_count.unwrap_or(3),
            last_round_updates: HashSet::new(),
            reparse_stable: false,
            asset_manifest: Arc::new(RwLock::new(BTreeMap::new())),
        })
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
            builder,
            primary_queue,
            reparse_queue: VecDeque::new(),
            pending_dependencies: HashMap::new(),
            processed: HashMap::new(),
            max_reparse_count: 3,
            last_round_updates: HashSet::new(),
            reparse_stable: false,
            asset_manifest: Arc::new(RwLock::new(BTreeMap::new())),
        })
    }

    /// Initialize a directory as a BeliefNetwork by placing a BeliefNetwork.toml file with the
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
        let mut proto = ProtoBeliefNode::new(&repo_path, &repo_path).unwrap_or_default();
        proto.document.insert("id", value(id));
        if let Some(title) = maybe_title {
            proto.document.insert("title", value(title));
        }
        if let Some(summary) = maybe_summary {
            proto.document.insert("text", value(summary));
        }

        proto.write(&repo_path)?;
        proto = ProtoBeliefNode::new(&repo_path, &repo_path)?;
        debug_assert!(
            proto
                .kind.is_network(),
            "Expected to generate an anchored BeliefKind::Network. Instead, our generated proto is {proto:?}"
        );
        debug_assert!(
            !proto.kind.contains(BeliefKind::Trace),
            "Expected to generate a BeliefKind::Network. Instead, our generated proto is {proto:?}"
        );
        Ok(PathBuf::from(proto.path))
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
            // Both queues empty - regenerate asset_manifest from BeliefBase
            // and enqueue any unparsed assets for content change detection
            tracing::debug!("[Compiler] Both queues empty, checking assets for content changes");

            let asset_map = self.builder.session_bb().paths().asset_map();
            let assets: Vec<(String, Bid)> = asset_map
                .map()
                .iter()
                .filter_map(|(path, bid, _order)| {
                    // Verify this is actually an External node (asset)
                    self.builder
                        .session_bb()
                        .states()
                        .get(bid)
                        .filter(|n| n.kind.is_external())
                        .map(|_| (path.clone(), *bid))
                })
                .collect();

            // Check each asset - if not yet processed, enqueue it for content verification
            let mut newly_enqueued = 0;
            for (repo_relative_path, _bid) in assets.iter() {
                // Reconstruct absolute path from repo-relative path
                let asset_absolute_path = self.builder.repo_root().join(repo_relative_path);

                // If we haven't processed this path yet, enqueue it to check for content changes
                if !self.processed.contains_key(&asset_absolute_path) {
                    tracing::debug!(
                        "[Compiler] Enqueuing unparsed asset for content check: {:?}",
                        asset_absolute_path
                    );
                    self.primary_queue.push_back(asset_absolute_path);
                    newly_enqueued += 1;
                }
            }

            if newly_enqueued > 0 {
                tracing::info!(
                    "[Compiler] Enqueued {} unparsed assets for content verification",
                    newly_enqueued
                );
                // Continue processing - don't return yet since we just added to queue
            } else {
                // Update asset_manifest with current state
                {
                    let mut manifest = self.asset_manifest.write();
                    manifest.clear();
                    for (path, bid) in assets.iter() {
                        manifest.insert(path.clone(), *bid);
                    }
                }

                // Check for duplicate BIDs (same content at multiple paths) and log at debug level
                let mut bid_to_paths: BTreeMap<Bid, Vec<String>> = BTreeMap::new();
                for (path, bid) in assets.iter() {
                    bid_to_paths.entry(*bid).or_default().push(path.clone());
                }

                for (bid, paths) in bid_to_paths.iter() {
                    if paths.len() > 1 {
                        tracing::debug!(
                            "[Compiler] Duplicate asset BID {} found at {} paths: {:?}",
                            bid,
                            paths.len(),
                            paths
                        );
                    }
                }

                tracing::info!(
                    "[Compiler] Asset manifest regenerated with {} unique assets",
                    assets.len()
                );

                return Ok(None);
            }

            // Fall through to process newly enqueued assets
            self.primary_queue.front().unwrap().clone()
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
            // BeliefNetwork.json or BeliefNetwork.toml file.
            if let Some((detected_path, _format)) = detect_network_file(&path) {
                detected_path
            } else {
                // Default to first in NETWORK_CONFIG_NAMES (JSON)
                path.join(NETWORK_CONFIG_NAMES[0])
            }
        } else {
            path.clone()
        };

        // 3a. Check if this is an asset file (not a known document codec extension)
        if !file_path.is_dir() {
            if let Some(ext) = file_path.extension().and_then(|e| e.to_str()) {
                if CODECS.get(ext).is_none() {
                    // This is an asset file - process it as a static asset
                    tracing::info!("[Compiler] Detected asset file: {:?}", file_path);

                    // Read file bytes and compute SHA256 hash
                    let file_bytes = match tokio::fs::read(&file_path).await {
                        Ok(bytes) => bytes,
                        Err(e) => {
                            tracing::warn!(
                                "[Compiler] Failed to read asset {:?}: {}",
                                file_path,
                                e
                            );
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
                        .net_get_from_path(&asset_namespace(), &repo_relative_path)
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
                            toml::Value::Array(vec![toml::Value::String(
                                repo_relative_path.clone(),
                            )]),
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
                            derivatives
                                .append(&mut self.builder.session_bb_mut().process_event(event)?);
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

                        // Update asset manifest
                        {
                            let mut manifest = self.asset_manifest.write();
                            manifest.insert(repo_relative_path.clone(), asset_bid);
                        }

                        tracing::info!(
                            "[Compiler] Asset processed successfully: {:?}",
                            repo_relative_path
                        );
                    }

                    // Remove from queues and return success
                    self.remove_from_queues(&path);
                    return Ok(Some(ParseResult {
                        path: path.clone(),
                        rewritten_content: None,
                        dependent_paths: Vec::new(),
                        diagnostics: Vec::new(),
                    }));
                }
            }
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
        let mut parse_result = match self
            .builder
            .parse_content(&path, content, global_bb.clone())
            .await
        {
            Ok(result) => result,
            Err(e) => {
                // Parse error - return as diagnostic but keep in queue (might be fixed)
                tracing::warn!("[Compiler] Failed to parse {:?}: {}", path, e);

                // Move to back of queue for retry later
                self.move_to_back(&path);

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

        // 7. Write rewritten content if available
        if let Some(contents) = parse_result.rewritten_content.as_ref() {
            // tracing::debug!("New content:\n\n{}\n", contents);
            if self.write {
                if let Err(e) = tokio::fs::write(&file_path, contents).await {
                    // Write error - add as warning but continue
                    parse_result
                        .diagnostics
                        .push(crate::codec::ParseDiagnostic::warning(format!(
                            "Failed to write rewritten content: {e}"
                        )));
                }
            }

            // 7a. Generate HTML if output directory is configured
            // Only regenerate on content changes (rewritten_content exists)
            if let Some(ref html_dir) = self.html_output_dir {
                if let Err(e) = self.generate_html_for_path(&path, html_dir).await {
                    // HTML generation error - add as warning but continue
                    parse_result
                        .diagnostics
                        .push(crate::codec::ParseDiagnostic::warning(format!(
                            "Failed to generate HTML: {e}"
                        )));
                }
            }
        } else if parse_count == 0 {
            // 7b. First successful parse with no content changes - still generate HTML
            // (handles case where HTML output dir is empty but repo is valid)
            if let Some(ref html_dir) = self.html_output_dir {
                if let Err(e) = self.generate_html_for_path(&path, html_dir).await {
                    // HTML generation error - add as warning but continue
                    parse_result
                        .diagnostics
                        .push(crate::codec::ParseDiagnostic::warning(format!(
                            "Failed to generate HTML: {e}"
                        )));
                }
            }
        }

        // 8. Extract dependent paths from SinkDependency diagnostics
        let unresolved_references: Vec<&UnresolvedReference> = parse_result
            .diagnostics
            .iter()
            .filter_map(|d| d.as_unresolved_reference())
            .collect();

        let mut dependent_paths = Vec::<(String, Bid)>::new();

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
                    *net == asset_namespace()
                } else {
                    false
                }
            });

            if is_asset_reference {
                // Extract asset path from NodeKey
                let asset_path_key = unresolved.other_keys.iter().find_map(|key| {
                    if let NodeKey::Path { net, path } = key {
                        if *net == asset_namespace() {
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
                    let doc_dir = path.parent().unwrap_or(&path);
                    let asset_absolute_path = doc_dir.join(asset_relative_path).canonicalize();

                    match asset_absolute_path {
                        Ok(absolute_path) => {
                            // Check if asset already tracked via asset_map
                            let repo_relative_asset = absolute_path
                                .strip_prefix(self.builder.repo_root())
                                .unwrap_or(&absolute_path)
                                .to_string_lossy()
                                .replace('\\', "/");

                            // Always enqueue asset files to check for content changes
                            // even if already tracked in session_bb
                            if !self.processed.contains_key(&absolute_path)
                                && !self.primary_queue.contains(&absolute_path)
                                && !self.reparse_queue.contains(&absolute_path)
                            {
                                tracing::debug!(
                                    "[Compiler] Queueing asset file for content check: {:?}",
                                    absolute_path
                                );
                                self.primary_queue.push_back(absolute_path.clone());
                            }

                            // Check if asset already tracked via BeliefBase
                            let asset_already_tracked = self
                                .builder
                                .session_bb()
                                .paths()
                                .net_get_from_path(&asset_namespace(), &repo_relative_asset)
                                .is_some();

                            if !asset_already_tracked {
                                // Asset not yet in session_bb - document needs reparse after asset loads
                                tracing::info!(
                                    "[Compiler] Document {:?} references untracked asset: {:?}",
                                    path,
                                    absolute_path
                                );

                                // Add document to reparse queue (will reparse after asset is processed)
                                if !self.reparse_queue.contains(&path) {
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
                            }
                        }
                        Err(e) => {
                            tracing::warn!(
                                "[Compiler] Cannot resolve asset path {:?} from document {:?}: {}",
                                asset_relative_path,
                                path,
                                e
                            );
                            // Asset file doesn't exist - leave as unresolved reference
                        }
                    }
                }

                // Skip normal dependency handling for asset references
                continue;
            }

            let Some((net_dep_path_str, net)) = unresolved.as_sink_dependency() else {
                continue;
            };
            dependent_paths.push((net_dep_path_str.clone(), net));
            let repo_pathmap = self
                .builder()
                .doc_bb()
                .paths()
                .get_map(&self.builder().repo())
                .expect(
                    "builder.repo to be instantiated after parse_content was successfully called.",
                );
            let full_dep_path = if let Some((_home_net, net_path, _order)) =
                repo_pathmap.path(&net, &self.builder().doc_bb().paths())
            {
                debug_assert!(_home_net == net);
                // Convert relative path to absolute
                let dep_path_str = PathBuf::from(net_path).join(net_dep_path_str);
                // Resolve relative to builder's repo_root
                self.builder.repo_root().join(dep_path_str)
            } else {
                tracing::warn!(
                    "No connectivity between builder.repo and dependent path network {}",
                    net
                );
                continue;
            };

            // Canonicalize if it exists
            let canonical_dep_path = match full_dep_path.canonicalize() {
                Ok(p) => p,
                Err(_) => {
                    tracing::debug!(
                        "[Compiler] Cannot canonicalize {:?}, treating as external",
                        full_dep_path
                    );
                    continue; // Skip external/non-existent dependencies
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
                .entry(path.clone())
                .or_default()
                .push(canonical_dep_path);
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

    /// Parse all items in the queue until empty or error
    ///
    /// This method will continue parsing until both the primary and reparse queues are empty,
    /// or until an unrecoverable error occurs.
    ///
    /// # Arguments
    /// * `global_bb` - The belief cache to query during parsing
    ///
    /// # Returns
    /// * `Ok(Vec<ParseResult>)` - All successfully parsed documents
    /// * `Err(_)` - First unrecoverable error encountered (parsing stops on error)
    pub async fn parse_all<B: BeliefSource + Clone>(
        &mut self,
        global_bb: B,
    ) -> Result<Vec<ParseResult>, BuildonomyError> {
        let mut results = Vec::new();

        while let Some(result) = self.parse_next(global_bb.clone()).await? {
            results.push(result);
        }

        // After all documents parsed, create asset hardlinks if HTML output is configured
        if let Some(ref html_dir) = self.html_output_dir {
            if let Err(e) = self.create_asset_hardlinks(html_dir).await {
                tracing::warn!("[Compiler] Failed to create asset hardlinks: {}", e);
                // Don't fail the entire parse - assets are supplementary
            }
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

    /// Move a path to the back of its current queue (for retry after error)
    fn move_to_back(&mut self, path: &PathBuf) {
        // Check primary queue first
        if let Some(pos) = self.primary_queue.iter().position(|p| p == path) {
            if let Some(removed) = self.primary_queue.remove(pos) {
                self.primary_queue.push_back(removed);
            }
            return;
        }

        // Check reparse queue
        if let Some(pos) = self.reparse_queue.iter().position(|p| p == path) {
            if let Some(removed) = self.reparse_queue.remove(pos) {
                self.reparse_queue.push_back(removed);
            }
        }
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

    /// Get a clone of the asset manifest Arc for file watcher integration
    ///
    /// This allows the file watcher to check if filesystem events correspond to tracked assets.
    /// The Arc can be cloned cheaply and the RwLock allows concurrent read access.
    pub fn asset_manifest(&self) -> Arc<RwLock<BTreeMap<String, Bid>>> {
        Arc::clone(&self.asset_manifest)
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
                            self.last_round_updates.insert(*bid);
                        }
                        NodeKey::Bref { .. } => {
                            // Brefs don't have BIDs, skip
                        }
                        NodeKey::Path { net, .. }
                        | NodeKey::Title { net, .. }
                        | NodeKey::Id { net, .. } => {
                            // Track network BID as a proxy for potential matches
                            if *net != Bid::nil() {
                                self.last_round_updates.insert(*net);
                            }
                        }
                    }
                }
                // New updates mean reparse might be productive
                self.reparse_stable = false;
            }
            BeliefEvent::PathAdded(_, _, bid, _, _) | BeliefEvent::PathUpdate(_, _, bid, _, _) => {
                self.last_round_updates.insert(*bid);
                self.reparse_stable = false;
            }
            BeliefEvent::NodesRemoved(bids, _) => {
                for bid in bids {
                    self.last_round_updates.remove(bid);
                }
            }
            _ => {}
        }
    }

    /// Generate index.html for each BeliefNetwork after parsing completes
    pub async fn generate_network_indices(&self) -> Result<(), BuildonomyError> {
        let html_dir = match &self.html_output_dir {
            Some(dir) => dir,
            None => return Ok(()), // No HTML output configured
        };

        let bb = self.builder.session_bb();
        let paths = bb.paths();

        // Get repository root network's PathMap to start traversal
        let repo_bid = self.builder.repo();
        let root_pm = match paths.get_map(&repo_bid) {
            Some(pm) => pm,
            None => return Ok(()), // No repository root network
        };

        // Get all networks in the hierarchy
        let mut visited = std::collections::BTreeSet::new();
        let all_networks = root_pm.all_net_paths(&paths, &mut visited);

        // Generate index for each network
        for (net_rel_path, net_bid) in all_networks {
            let network_pm = match paths.get_map(&net_bid) {
                Some(pm) => pm,
                None => continue,
            };

            // Get network node for title
            let network_node = bb.get(&NodeKey::Bid { bid: net_bid });
            let network_title = network_node
                .as_ref()
                .map(|n| {
                    if n.title.is_empty() {
                        "Network Index"
                    } else {
                        n.title.as_str()
                    }
                })
                .unwrap_or("Network Index");

            // Get all local paths (documents) in this network
            let local_paths = network_pm.map();
            let all_docs = paths.docs();
            let mut docs_with_paths: Vec<(String, String)> = Vec::new();

            for (doc_path, doc_bid, _order) in local_paths.iter() {
                // Only include actual documents, not sections/anchors
                if !all_docs.contains(doc_bid) {
                    continue;
                }
                // Convert .md to .html
                let html_path = if doc_path.ends_with(".md") {
                    doc_path.replace(".md", ".html")
                } else {
                    doc_path.clone()
                };

                // Get document title
                let doc_title = bb
                    .get(&NodeKey::Bid { bid: *doc_bid })
                    .map(|n| {
                        if n.title.is_empty() {
                            // Fallback to path filename
                            std::path::Path::new(&html_path)
                                .file_stem()
                                .and_then(|s| s.to_str())
                                .unwrap_or("Untitled")
                                .to_string()
                        } else {
                            n.title.clone()
                        }
                    })
                    .unwrap_or_else(|| {
                        std::path::Path::new(&html_path)
                            .file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or("Untitled")
                            .to_string()
                    });

                docs_with_paths.push((html_path, doc_title));
            }

            if docs_with_paths.is_empty() {
                continue; // Skip empty networks
            }

            // Sort by path
            docs_with_paths.sort_by(|a, b| a.0.cmp(&b.0));

            // Generate index HTML
            let index_html = Self::generate_index_page(network_title, &docs_with_paths)?;

            // Determine output directory
            let index_dir = if net_rel_path.is_empty() {
                html_dir.to_path_buf()
            } else {
                html_dir.join(&net_rel_path)
            };

            tokio::fs::create_dir_all(&index_dir).await?;
            let index_path = index_dir.join("index.html");
            tokio::fs::write(&index_path, index_html).await?;

            tracing::info!("Generated network index: {}", index_path.display());
        }

        Ok(())
    }

    /// Generate HTML content for network index page
    fn generate_index_page(
        network_title: &str,
        docs: &[(String, String)], // (html_path, title)
    ) -> Result<String, BuildonomyError> {
        use std::collections::BTreeMap;

        // Group documents by directory for better organization
        let mut by_dir: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();

        for (html_path, title) in docs {
            // Get directory part (or "." for root files)
            let dir = std::path::Path::new(html_path)
                .parent()
                .and_then(|p| p.to_str())
                .unwrap_or(".")
                .to_string();

            by_dir
                .entry(dir)
                .or_default()
                .push((html_path.clone(), title.clone()));
        }

        // Build HTML
        let mut html = format!(
            r#"<!DOCTYPE html>
<html>
<head>
  <meta charset="utf-8">
  <title>{}</title>
  <link rel="stylesheet" href="assets/default-theme.css">
</head>
<body>
  <article>
    <h1>{}</h1>
    <p>Total documents: {}</p>
"#,
            network_title,
            network_title,
            docs.len()
        );

        for (dir, mut files) in by_dir {
            files.sort_by(|a, b| a.0.cmp(&b.0));
            if dir != "." {
                html.push_str(&format!("    <h2>{}</h2>\n", dir));
            }
            html.push_str("    <ul>\n");
            for (path, title) in files {
                html.push_str(&format!(
                    "      <li><a href=\"{}\">{}</a></li>\n",
                    path, title
                ));
            }
            html.push_str("    </ul>\n");
        }

        html.push_str(
            r#"  </article>
</body>
</html>"#,
        );

        Ok(html)
    }

    /// Generate HTML for a parsed document
    /// Copy static assets (CSS, etc.) to HTML output directory
    fn copy_static_assets(html_output_dir: &Path) -> Result<(), BuildonomyError> {
        const DEFAULT_CSS: &str = include_str!("../../assets/default-theme.css");

        let assets_dir = html_output_dir.join("assets");
        std::fs::create_dir_all(&assets_dir)?;

        let css_path = assets_dir.join("default-theme.css");
        std::fs::write(&css_path, DEFAULT_CSS)?;

        tracing::info!("Copied static assets to {}", assets_dir.display());
        Ok(())
    }

    async fn generate_html_for_path(
        &self,
        source_path: &Path,
        html_output_dir: &Path,
    ) -> Result<(), BuildonomyError> {
        // Get file extension
        let ext = source_path
            .extension()
            .and_then(|s| s.to_str())
            .ok_or_else(|| {
                BuildonomyError::Codec(format!(
                    "Source file has no extension: {}",
                    source_path.display()
                ))
            })?;

        // Get codec for this file type
        let codec_arc = CODECS.get(ext).ok_or_else(|| {
            BuildonomyError::Codec(format!("No codec available for .{} files", ext))
        })?;

        // Generate HTML (drop lock before await)
        let html_opt = {
            let codec = codec_arc.lock();
            codec.generate_html()?
        };

        if let Some(html) = html_opt {
            // Compute output path relative to repo root
            let relative_path = source_path
                .strip_prefix(self.builder.repo_root())
                .unwrap_or(source_path);

            let mut html_path = html_output_dir.join(relative_path);
            html_path.set_extension("html");

            // Create parent directories
            if let Some(parent) = html_path.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }

            // Write HTML file
            tokio::fs::write(&html_path, html).await?;

            tracing::info!("Generated HTML: {}", html_path.display());
        }

        Ok(())
    }

    /// Create content-addressed hardlinks for all tracked assets in HTML output directory
    ///
    /// This method:
    /// 1. Copies each unique asset (by content hash) to `static/{hash}.{ext}`
    /// 2. Creates hardlinks from semantic paths to the canonical location
    /// 3. Deduplicates automatically - same content = same physical file
    ///
    /// # Arguments
    /// * `html_output_dir` - Base directory for HTML output
    ///
    /// # Returns
    /// * `Ok(())` on success
    /// * `Err(BuildonomyError)` if filesystem operations fail
    pub async fn create_asset_hardlinks(
        &self,
        html_output_dir: &Path,
    ) -> Result<(), BuildonomyError> {
        use std::collections::HashSet;

        // Clone manifest data to avoid holding lock across await points
        let manifest_data: BTreeMap<String, Bid> = {
            let manifest = self.asset_manifest.read();
            if manifest.is_empty() {
                return Ok(());
            }

            tracing::info!(
                "[Compiler] Creating asset hardlinks for {} assets",
                manifest.len()
            );

            manifest.clone()
        }; // Lock is dropped here

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

            let content_hash = asset_node
                .payload
                .get("content_hash")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    BuildonomyError::Codec(format!(
                        "Asset missing content_hash in payload: {}",
                        asset_bid
                    ))
                })?;

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
                let repo_full_path = self.builder.repo_root().join(asset_path);

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

                tracing::debug!(
                    "[Compiler] Copied asset to canonical: {} -> {}",
                    repo_full_path.display(),
                    canonical.display()
                );
            } else {
                tracing::debug!(
                    "[Compiler] Duplicate content detected: {} (hash: {}) - reusing canonical {}",
                    asset_path,
                    content_hash,
                    canonical.display()
                );
            }

            // Create hardlink at semantic path
            let html_full_path = html_output_dir.join(asset_path);

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
                Ok(_) => {
                    tracing::debug!(
                        "[Compiler] Hardlinked asset: {} -> {}",
                        html_full_path.display(),
                        canonical.display()
                    );
                }
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
#[derive(Debug, Clone)]
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

    #[test]
    fn test_compiler_creation() {
        // This is a basic structure test - actual functional tests would require
        // setting up a test filesystem and mock cache
        let temp_dir = std::env::temp_dir();
        let result = DocumentCompiler::new(&temp_dir, None, Some(5), false);
        assert!(result.is_ok());

        let compiler = result.unwrap();
        assert_eq!(compiler.max_reparse_count, 5);
        assert!(compiler.has_pending());
        assert_eq!(compiler.primary_queue_len(), 1);
        assert_eq!(compiler.reparse_queue_len(), 0);
    }

    #[test]
    fn test_enqueue_deduplication() {
        let temp_dir = std::env::temp_dir();
        let mut compiler = DocumentCompiler::new(&temp_dir, None, None, false).unwrap();

        let test_path = temp_dir.join("test.md");
        compiler.enqueue(&test_path);
        let initial_len = compiler.total_queue_len();

        // Enqueuing the same path again should not increase queue size
        compiler.enqueue(&test_path);
        assert_eq!(compiler.total_queue_len(), initial_len);
    }

    #[test]
    fn test_stats() {
        let temp_dir = std::env::temp_dir();
        let compiler = DocumentCompiler::new(&temp_dir, None, None, false).unwrap();

        let stats = compiler.stats();
        assert_eq!(stats.primary_queue_len, 1);
        assert_eq!(stats.reparse_queue_len, 0);
        assert_eq!(stats.processed_count, 0);
        assert_eq!(stats.total_parses, 0);
    }
}
