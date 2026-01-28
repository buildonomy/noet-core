use crate::{
    beliefbase::BeliefBase,
    codec::{
        belief_ir::{detect_network_file, ProtoBeliefNode, NETWORK_CONFIG_NAMES},
        builder::GraphBuilder,
        UnresolvedReference,
    },
    error::BuildonomyError,
    event::BeliefEvent,
    nodekey::NodeKey,
    properties::{BeliefKind, Bid},
    query::BeliefSource,
};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
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
        let entry_path = entry_point.as_ref().canonicalize()?;

        let builder = GraphBuilder::new(&entry_path, tx)?;
        let mut primary_queue = VecDeque::new();
        primary_queue.push_back(entry_path);

        Ok(Self {
            write,
            builder,
            primary_queue,
            reparse_queue: VecDeque::new(),
            pending_dependencies: HashMap::new(),
            processed: HashMap::new(),
            max_reparse_count: max_reparse_count.unwrap_or(3),
            last_round_updates: HashSet::new(),
            reparse_stable: false,
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
            builder,
            primary_queue,
            reparse_queue: VecDeque::new(),
            pending_dependencies: HashMap::new(),
            processed: HashMap::new(),
            max_reparse_count: 3,
            last_round_updates: HashSet::new(),
            reparse_stable: false,
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
            // Both queues empty
            return Ok(None);
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
