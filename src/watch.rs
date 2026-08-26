//! # Watch Service - Continuous Parsing and File Watching
//!
//! The `watch` module provides [`WatchService`], a long-running service that automatically
//! monitors document directories for changes and keeps the in-memory cache and database
//! synchronized with the file system.
//!
//! ## Overview
//!
//! `WatchService` is designed for applications that need continuous parsing and synchronization:
//! - **File watching**: Automatically detects file changes via filesystem notifications
//! - **Debounced parsing**: Batches rapid file changes to avoid redundant parses
//! - **Database sync**: Keeps SQLite database in sync with parsed documents
//! - **Event streaming**: Emits [`Event`]s for cache updates and downstream processing
//!
//! ## When to Use WatchService
//!
//! Use `WatchService` when you need:
//! - **Long-running applications**: Servers, daemons, IDE integrations (LSP servers)
//! - **Continuous synchronization**: Keep database in sync with changing files
//! - **File watching**: Automatic reparsing when documents are modified
//! - **Multi-network management**: Watch multiple document networks simultaneously
//!
//! **Don't use WatchService** for:
//! - One-shot parsing (use [`DocumentCompiler::simple`] instead)
//! - Build scripts or short-lived commands (use direct parsing)
//! - Applications without file watching needs (use compiler directly)
//!
//! ## Quick Start
//!
//! ```rust,no_run
//! use noet_core::{watch::WatchService, event::Event};
//! use std::{sync::mpsc::channel, path::PathBuf};
//!
//! // Create event channel for receiving compiler events
//! let (tx, rx) = channel::<Event>();
//!
//! // Initialize service (creates its own runtime and database)
//! let root_dir = PathBuf::from("/path/to/workspace");
//! let service = WatchService::new(root_dir, tx, true, false)?;
//!
//! // Enable file watching for a document network
//! let network_path = PathBuf::from("/path/to/workspace/my_network");
//! service.enable_network_syncer(&network_path)?;
//!
//! // Service now watches for file changes and emits events
//! // Process events in your application
//! for event in rx {
//!     match event {
//!         Event::Belief(belief_event) => {
//!             println!("Received belief update: {:?}", belief_event);
//!         }
//!         Event::Ping => {
//!             // Keepalive event
//!         }
//!     }
//! }
//! # Ok::<(), noet_core::BuildonomyError>(())
//! ```
//!
//! ## File Watching Pattern
//!
//! The service automatically watches directories and reparses files when they change:
//!
//! ```rust,no_run
//! use noet_core::{watch::WatchService, event::Event};
//! use std::{sync::mpsc::channel, path::PathBuf};
//!
//! let (tx, rx) = channel::<Event>();
//! let service = WatchService::new(PathBuf::from("/workspace"), tx, true, false)?;
//!
//! // Enable watching - initial parse happens automatically
//! let network_path = PathBuf::from("/workspace/docs");
//! service.enable_network_syncer(&network_path)?;
//!
//! // Now modify a file in /workspace/docs/...
//! // The service will:
//! // 1. Detect the change via filesystem notification
//! // 2. Debounce rapid changes (300ms default)
//! // 3. Reparse the modified file
//! // 4. Emit Event::Belief updates
//! // 5. Sync changes to database
//!
//! // Disable watching when done
//! service.disable_network_syncer(&network_path)?;
//! # Ok::<(), noet_core::BuildonomyError>(())
//! ```
//!
//! ## Network Management
//!
//! Manage multiple document networks with persistent configuration:
//!
//! ```rust,no_run
//! use noet_core::{
//!     watch::WatchService,
//!     config::NetworkRecord,
//!     properties::{BeliefNode, Bid, NodeId},
//!     event::Event,
//! };
//! use std::{sync::mpsc::channel, path::PathBuf};
//!
//! let (tx, _rx) = channel::<Event>();
//! let service = WatchService::new(PathBuf::from("/workspace"), tx, true, false)?;
//!
//! // Get current networks (reads from config.toml)
//! let networks = service.get_networks()?;
//! println!("Currently configured networks: {}", networks.len());
//!
//! // Add a new network
//! let mut networks = service.get_networks()?;
//! networks.push(NetworkRecord {
//!     path: "/workspace/new_network".to_string(),
//!     node: BeliefNode {
//!         title: "New Network".to_string(),
//!         id: NodeId::Explicit("new-network".to_string()),
//!         ..Default::default()
//!     },
//! });
//! service.set_networks(Some(networks))?;
//!
//! // Configuration persists to /workspace/config.toml
//! # Ok::<(), noet_core::BuildonomyError>(())
//! ```
//!
//! ## Threading Model
//!
//! `WatchService` uses multiple threads for concurrent processing:
//!
//! ### Main Thread
//! - Owns the `WatchService` instance
//! - Coordinates watcher lifecycle (enable/disable)
//! - Receives events via `mpsc::channel`
//!
//! ### Per-Network Threads (spawned by `enable_network_syncer`)
//!
//! 1. **File Watcher Thread** (from `notify-debouncer-full`)
//!    - Monitors filesystem for changes
//!    - Debounces rapid modifications (300ms window)
//!    - Filters by codec extensions (.md, .toml, etc.)
//!    - Ignores dot files (.git, .DS_Store)
//!
//! 2. **Compiler Thread** (`FileUpdateSyncer::compiler_handle`)
//!    - Runs continuous parsing loop
//!    - Processes files from parse queue
//!    - Emits `BeliefEvent`s to transaction thread
//!    - Uses `DocumentCompiler` with incremental updates
//!
//! 3. **Transaction Thread** (`FileUpdateSyncer::transaction_handle`)
//!    - Receives `BeliefEvent`s from compiler
//!    - Batches events into database transactions
//!    - Updates SQLite database atomically
//!    - Forwards events to main application via `event_tx`
//!
//! ### Synchronization Points
//!
//! - **Parse Queue**: Compiler thread blocks on queue when empty
//! - **Event Channel**: Transaction thread blocks on event receiver
//! - **Database Lock**: Transaction thread serializes database writes
//! - **Watcher Mutex**: `BnWatchers` mutex guards watcher map access
//!
//! ### Shutdown
//!
//! - `disable_network_syncer()`: Aborts compiler and transaction handles for specific network
//! - Drop `WatchService`: Aborts all active watchers and threads
//! - Threads abort gracefully via `JoinHandle::abort()`
//!
//! ## Database Synchronization
//!
//! The service maintains a SQLite database that mirrors the parsed document graph:
//!
//! ```rust,no_run
//! use noet_core::watch::WatchService;
//! use std::{sync::mpsc::channel, path::PathBuf};
//!
//! let (tx, _rx) = channel();
//! let root_dir = PathBuf::from("/workspace");
//!
//! // Database created at /workspace/belief_cache.db
//! let service = WatchService::new(root_dir.clone(), tx, true, false)?;
//!
//! // Database location is fixed: {root_dir}/belief_cache.db
//! let db_path = root_dir.join("belief_cache.db");
//! assert!(db_path.exists(), "Database should be created on initialization");
//!
//! // For custom database paths, use db_init() and DbConnection directly:
//! use noet_core::db::{db_init, DbConnection};
//! let custom_db = PathBuf::from("/custom/path/cache.db");
//! let runtime = tokio::runtime::Builder::new_current_thread()
//!     .enable_all()
//!     .build()?;
//! let pool = runtime.block_on(db_init(custom_db))?;
//! let _db_conn = DbConnection(pool);
//!
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! ## CLI Tool Integration
//!
//! The `noet` CLI uses `WatchService` for continuous parsing:
//!
//! ```bash
//! # One-shot parse (uses DocumentCompiler::simple)
//! noet parse /path/to/network
//!
//! # Continuous watching (uses WatchService)
//! noet watch /path/to/network
//! ```
//!
//! See `src/bin/noet.rs` for implementation details.
//!
//! ## Error Handling
//!
//! The service handles errors gracefully:
//! - **Parse errors**: Emitted as `Event::Diagnostic`, parsing continues
//! - **File system errors**: Logged, watcher continues monitoring
//! - **Database errors**: Logged, may cause event loss but service continues
//! - **Invalid paths**: Return `BuildonomyError` on `enable_network_syncer()`
//!
//! ## Feature Flags
//!
//! This module requires the `service` feature flag:
//!
//! ```toml
//! [dependencies]
//! noet-core = { version = "0.1", features = ["service"] }
//! ```
//!
//! ## Examples
//!
//! See `examples/watch_service.rs` for a complete orchestration example.
//!
//! ## See Also
//!
//! - [`DocumentCompiler`] - The underlying compiler
//! - [`Event`] - Events emitted by the service
//! - [`DbConnection`] - Database connection wrapper
//! - [`LatticeConfigProvider`] - Configuration interface

use crate::{
    codec::{
        compiler::{CompilerStats, DocumentCompiler},
        network::detect_network_file,
        WALK_CODECS,
    },
    config::{LatticeConfigProvider, NetworkRecord, TomlConfigProvider},
    db::{db_init, DbConnection, Transaction},
    error::BuildonomyError,
    event::{BeliefEvent, Event},
};

use notify_debouncer_full::{
    new_debouncer,
    notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher},
    DebounceEventResult, Debouncer, FileIdMap,
};
use parking_lot::{Mutex, RwLock};
use serde::Deserialize;
use std::{
    collections::HashMap,
    fs::{read_to_string, write},
    path::{Path, PathBuf},
    result::Result,
    sync::{atomic::Ordering, mpsc::Sender, Arc},
    time::Duration,
};
use tokio::{
    runtime::Runtime,
    sync::{
        broadcast,
        mpsc::{unbounded_channel, UnboundedReceiver},
        watch,
    },
    task::JoinHandle,
};

/// A file system watcher with debouncing for a belief network
type NetworkWatcher = Debouncer<RecommendedWatcher, FileIdMap>;

/// A watcher paired with its file update syncer
type WatcherWithSyncer = (NetworkWatcher, FileUpdateSyncer);

/// Map of network paths to their watchers and syncers
type NetworkWatcherMap = HashMap<PathBuf, WatcherWithSyncer>;

#[derive(Default)]
struct BnWatchers(pub Arc<Mutex<NetworkWatcherMap>>);

pub struct WatchService {
    watchers: Arc<Mutex<BnWatchers>>,
    db: DbConnection,
    event_tx: Sender<Event>,
    runtime: Runtime,
    config_provider: Arc<dyn LatticeConfigProvider>,
    write: bool,
    html_output_dir: Option<PathBuf>,
    html_script: Option<String>,
    use_cdn: bool,
    base_url: Option<String>,
    git_tracking: bool,
}

impl WatchService {
    pub fn new(
        root_dir: PathBuf,
        event_tx: Sender<Event>,
        write: bool,
        git_tracking: bool,
    ) -> Result<Self, BuildonomyError> {
        Self::with_html_output(
            root_dir,
            event_tx,
            write,
            None,
            None,
            false,
            None,
            git_tracking,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn with_html_output(
        root_dir: PathBuf,
        event_tx: Sender<Event>,
        write: bool,
        html_output_dir: Option<PathBuf>,
        html_script: Option<String>,
        use_cdn: bool,
        base_url: Option<String>,
        git_tracking: bool,
    ) -> Result<Self, BuildonomyError> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .build()?;

        let db_path = root_dir.join("belief_cache.db");
        let db_pool = runtime.block_on(db_init(db_path))?;
        let db = DbConnection(db_pool);

        let config_path = root_dir.join("config.toml");
        tracing::debug!(
            "Initializing TomlConfigProvider with path: {:?}",
            config_path
        );
        let config_provider = TomlConfigProvider::new(config_path);
        let config_provider: Arc<dyn LatticeConfigProvider> = Arc::new(config_provider);

        Ok(WatchService {
            watchers: Arc::new(Mutex::new(BnWatchers::default())),
            db,
            event_tx,
            runtime,
            config_provider,
            write,
            html_output_dir,
            html_script,
            use_cdn,
            base_url,
            git_tracking,
        })
    }

    pub fn get_networks(&self) -> Result<Vec<NetworkRecord>, BuildonomyError> {
        self.config_provider.get_networks()
    }

    pub fn set_networks(
        &self,
        new_maybe_nets: Option<Vec<NetworkRecord>>,
    ) -> Result<Vec<NetworkRecord>, BuildonomyError> {
        let old_nets = self.get_networks()?;
        let nets = new_maybe_nets.unwrap_or_else(|| old_nets.clone());

        let invalid_paths: Vec<&String> = nets
            .iter()
            .filter_map(|record| {
                if PathBuf::from(&record.path).exists() {
                    None
                } else {
                    Some(&record.path)
                }
            })
            .collect();

        if !invalid_paths.is_empty() {
            return Err(BuildonomyError::NotFound(format!(
                "Belief Network file path(s) are not available: {invalid_paths:?}"
            )));
        }

        let mut removed_networks = Vec::<String>::default();
        let mut added_networks = nets.clone();
        if nets != old_nets {
            removed_networks = old_nets.iter().map(|record| record.path.clone()).collect();
            removed_networks.retain(|net| !added_networks.iter().any(|record| record.path == *net));
            added_networks.retain(|added_record| {
                !old_nets
                    .iter()
                    .any(|old_record| old_record.path == added_record.path)
            });
        }

        for record in added_networks.iter() {
            let path = PathBuf::from(&record.path);
            self.enable_network_syncer(&path)?;
        }
        for str_path in removed_networks.iter() {
            let path = PathBuf::from(&str_path);
            self.disable_network_syncer(&path)?;
        }

        if nets != old_nets {
            self.config_provider.set_networks(nets.clone())?;
        }
        Ok(nets)
    }

    pub fn db_connection(&self) -> DbConnection {
        self.db.clone()
    }

    /// Block until at least one full compile+transaction cycle has completed for every
    /// active network syncer, or until `timeout` elapses.
    ///
    /// Uses a `watch::Receiver` which is level-triggered: if the generation has already
    /// advanced past zero when this is called, it returns immediately without waiting.
    /// This eliminates the subscribe-before-update race that existed with `AtomicU64 +
    /// Notify`.
    ///
    /// Returns `Err(BuildonomyError::Timeout)` if the deadline is exceeded.
    pub fn wait_for_idle(&self, timeout: Duration) -> Result<(), BuildonomyError> {
        self.wait_for_next_idle(timeout, 0)
    }

    /// Snapshot the current commit generation for all active network syncers.
    ///
    /// Call this before triggering work, then pass the result to `wait_for_next_idle`
    /// to wait for a cycle that began *after* this snapshot was taken.
    pub fn current_generation(&self) -> u64 {
        let binding = self.watchers.lock();
        let watchers = binding.0.lock();
        watchers
            .values()
            .map(|(_debouncer, syncer)| *syncer.commit_generation_rx.borrow())
            .min()
            .unwrap_or(0)
    }

    /// Wait until every active network syncer has a commit generation strictly greater
    /// than `after_gen`, or until `timeout` elapses.
    ///
    /// The `watch::Receiver::wait_for` predicate is level-triggered: it checks the
    /// current value first and returns immediately if already satisfied.
    ///
    /// Returns `Err(BuildonomyError::Timeout)` if the deadline is exceeded.
    pub fn wait_for_next_idle(
        &self,
        timeout: Duration,
        after_gen: u64,
    ) -> Result<(), BuildonomyError> {
        let deadline = std::time::Instant::now() + timeout;

        // Clone the receiver handles while holding the lock, then release it before
        // entering block_on.
        let receivers: Vec<watch::Receiver<u64>> = {
            let binding = self.watchers.lock();
            let watchers = binding.0.lock();
            watchers
                .values()
                .map(|(_debouncer, syncer)| syncer.commit_generation_rx.clone())
                .collect()
        };

        for mut rx in receivers {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            self.runtime
                .block_on(async move {
                    tokio::time::timeout(remaining, async move {
                        // wait_for is level-triggered: checks current value first.
                        let _ = rx.wait_for(|&g| g > after_gen).await;
                    })
                    .await
                })
                .map_err(|_| BuildonomyError::Timeout("wait_for_idle timed out".to_string()))?;
        }
        Ok(())
    }

    pub fn get_content<P: AsRef<Path>>(&self, path: P) -> Result<String, BuildonomyError> {
        tracing::debug!("Reading {:?}", path.as_ref());
        Ok(read_to_string(path)?)
    }

    pub async fn set_content<P: AsRef<Path>>(
        &self,
        path: P,
        text: String,
    ) -> Result<(), BuildonomyError> {
        Ok(write(path, text)?)
    }

    pub fn enable_network_syncer(&self, repo_path: &PathBuf) -> Result<(), BuildonomyError> {
        let binding = self.watchers.lock();
        let mut watchers = binding.0.lock();
        if watchers.contains_key(repo_path) {
            return Err(BuildonomyError::Custom(format!(
                "BnWatchers already contains a file watcher for belief network at path {repo_path:?}"
            )));
        }

        let network_syncer = FileUpdateSyncer::new(
            &self.db,
            &self.event_tx,
            repo_path,
            true,
            &self.runtime,
            self.write,
            self.html_output_dir.clone(),
            self.html_script.clone(),
            self.use_cdn,
            self.base_url.clone(),
            self.git_tracking,
        )?;

        let compiler_ref = network_syncer.compiler.clone();
        let work_notifier = network_syncer.work_notifier.clone();

        // Enqueue the network root for initial parse, then mark the compiler and transaction
        // stages busy before firing notify_one. This ordering guarantees that wait_for_idle
        // cannot observe a spurious all-idle state between the notify and the compiler
        // task waking up and clearing bit 1 itself.
        {
            let mut compiler = compiler_ref.write();
            compiler.enqueue(repo_path);
            tracing::debug!(
                "[WatchService] Enqueued network root for initial parse: {:?}",
                repo_path
            );
        }
        work_notifier.notify_one();

        let ignored_write_paths = network_syncer.ignored_write_paths.clone();
        let debouncer_compiler_idle = network_syncer.compiler_idle.clone();
        let mut debouncer = new_debouncer(
            Duration::from_secs(2),
            None,
            move |result: DebounceEventResult| {
                tracing::debug!("[FileUpdateSyncer Debouncer] processing debounce event");
                match result {
                    Ok(events) => {
                        for event in events.iter() {
                            match event.event.kind {
                                EventKind::Create(_)
                                | EventKind::Modify(_)
                                | EventKind::Remove(_) => {
                                    // Hold off if the compiler is active. Any file-watcher
                                    // events that arrived while the compiler was running may
                                    // include writes the compiler itself produced. Deferring
                                    // here is safe: notify-debouncer-full buffers events
                                    // internally and will re-deliver them after the next
                                    // quiet window once the compiler goes idle.
                                    if !debouncer_compiler_idle.load(Ordering::SeqCst) {
                                        tracing::debug!(
                                            "[Debouncer] Compiler active, deferring debounce event"
                                        );
                                        return;
                                    }

                                    // Collect paths that are relevant to the compiler,
                                    // skipping directories and compiler-written files.
                                    let is_remove =
                                        matches!(event.event.kind, EventKind::Remove(_));
                                    let relevant_paths: Vec<&PathBuf> = event
                                        .paths
                                        .iter()
                                        .filter(|&p| {
                                            // Deletions: any path the compiler tracks is relevant.
                                            // Modifications/creates: only files (not directories).
                                            if !is_remove && !p.is_file() {
                                                return false;
                                            }

                                            // Fine-grained per-file guard: skip paths the
                                            // compiler has written in this idle window.
                                            let normalized = match crate::paths::canonicalize_path(p) {
                                                Ok(canonical) => {
                                                    tracing::trace!("[Debouncer] Normalized {:?} -> {:?}", p, canonical);
                                                    canonical
                                                }
                                                Err(_) => {
                                                    tracing::trace!("[Debouncer] Failed to normalize {:?}, using as-is", p);
                                                    p.clone()
                                                }
                                            };
                                            let ignored = ignored_write_paths.lock().unwrap();
                                            if ignored.contains(&normalized) {
                                                tracing::debug!("[Debouncer] Ignoring write to {:?} (normalized: {:?}, compiler wrote this file)", p, normalized);
                                                return false;
                                            }

                                            // For modifications/creates, only walk-tracked files
                                            // trigger a re-parse. Deletions of any tracked file
                                            // should be forwarded.
                                            // WALK_CODECS.should_track() is the canonical
                                            // walk-time visibility predicate for the two-registry
                                            // model — it covers .md (MdWalkCodec), .yaml
                                            // (YamlWalkCodec), and any shim-registered codecs.
                                            is_remove || WALK_CODECS.should_track(p)
                                        })
                                        .collect();

                                    if !relevant_paths.is_empty() {
                                        tracing::debug!(
                                            "[Debouncer] {} files to process (is_remove={})",
                                            relevant_paths.len(),
                                            is_remove
                                        );
                                        while compiler_ref.is_locked() {
                                            tracing::debug!(
                                                "[Debouncer] Waiting for write access to compiler"
                                            );
                                            std::thread::sleep(Duration::from_millis(100));
                                        }
                                        tracing::debug!("[Debouncer] Acquired write lock");
                                        let mut compiler = compiler_ref.write();
                                        for path in relevant_paths {
                                            if is_remove {
                                                tracing::debug!(
                                                    "[Debouncer] File deleted: {:?}",
                                                    path
                                                );
                                                compiler.on_file_deleted(path);
                                            } else {
                                                tracing::debug!(
                                                    "[Debouncer] File modified, enqueuing for re-parse: {:?}",
                                                    path
                                                );
                                                compiler.on_file_modified(path);
                                            }
                                        }
                                        tracing::debug!("[Debouncer] Finished processing, compiler.has_pending()={}", compiler.has_pending());

                                        // Notify compiler thread that work is available.
                                        work_notifier.notify_one();
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    Err(errors) => {
                        tracing::error!("Notify debouncer returned errors: {:?}", errors);
                    }
                }
            },
        )?;
        debouncer
            .watcher()
            .watch(repo_path, RecursiveMode::Recursive)?;

        watchers.insert(repo_path.clone(), (debouncer, network_syncer));

        Ok(())
    }

    pub fn disable_network_syncer(&self, repo_path: &PathBuf) -> Result<(), BuildonomyError> {
        let binding = self.watchers.lock();
        let mut watchers = binding.0.lock();
        if let Some((mut debouncer, update_syncer)) = watchers.remove(repo_path) {
            let unwatch_res = debouncer.watcher().unwatch(repo_path);
            update_syncer.compiler_handle.abort();
            update_syncer.transaction_handle.abort();
            tracing::debug!("Unwatch_res(path: {:?}) = {:?}", repo_path, unwatch_res);
            unwatch_res?;
        }
        Ok(())
    }
}

pub(crate) struct FileUpdateSyncer {
    pub compiler: Arc<RwLock<DocumentCompiler>>,
    pub compiler_handle: JoinHandle<Result<(), BuildonomyError>>,
    pub transaction_handle: JoinHandle<Result<(), BuildonomyError>>,
    pub work_notifier: Arc<tokio::sync::Notify>,
    pub ignored_write_paths: Arc<std::sync::Mutex<std::collections::HashSet<PathBuf>>>,
    /// Sender half of the commit-generation watch channel. The transaction task calls
    /// `send` to increment the generation after each true pipeline-idle commit.
    /// `wait_for_idle` / `wait_for_next_idle` clone the receiver and use `wait_for`,
    /// which is level-triggered and immune to the subscribe-before-update race.
    /// Must be retained here to keep the channel open; dropping it would make all
    /// `wait_for` calls return `Err` immediately.
    #[allow(dead_code)]
    pub commit_generation_tx: watch::Sender<u64>,
    /// Receiver half — cloned by `wait_for_idle` / `wait_for_next_idle` and by
    /// `current_generation` to read the latest value without blocking.
    pub commit_generation_rx: watch::Receiver<u64>,
    /// Compiler-idle flag: true when both compiler queues are empty. The debouncer reads
    /// this to decide whether to hold off enqueueing (avoiding false positives from
    /// compiler-written files appearing in the watcher event stream).
    pub compiler_idle: Arc<std::sync::atomic::AtomicBool>,
    /// Fired by the compiler task immediately after setting compiler_idle = true.
    /// The transaction task selects on this to know when to drain the remainder of the
    /// channel and signal wait_for_idle, without needing to inspect the channel itself.
    ///
    /// Forward-looking API: will be consumed by the LSP server (Issue 11) and peer
    /// replication layer (federated_belief_network.md) once those are implemented.
    #[allow(dead_code)]
    pub compiler_idle_notify: Arc<tokio::sync::Notify>,
    /// Broadcast sender for best-effort fan-out to LSP and future peer subscribers.
    /// The transaction task sends each BeliefEvent here alongside the DB write.
    /// Receivers that fall behind receive a Lagged error and should re-query the DB.
    ///
    /// Forward-looking API: will be consumed by the LSP server (Issue 11) and peer
    /// replication layer (federated_belief_network.md) once those are implemented.
    /// Clone this sender to create a new subscriber receiver:
    ///   `let rx = syncer.belief_broadcast.subscribe();`
    #[allow(dead_code)]
    pub belief_broadcast: broadcast::Sender<BeliefEvent>,
}

impl FileUpdateSyncer {
    #[tracing::instrument(skip_all)]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        global_bb: &DbConnection,
        tx: &Sender<Event>,
        root: &Path,
        notify: bool,
        runtime: &Runtime,
        write: bool,
        html_output_dir: Option<PathBuf>,
        html_script: Option<String>,
        use_cdn: bool,
        base_url: Option<String>,
        git_tracking: bool,
    ) -> Result<FileUpdateSyncer, BuildonomyError> {
        let (accum_tx, accum_rx) = unbounded_channel::<BeliefEvent>();

        // Create notification channel for waking up compiler thread
        let work_notifier = Arc::new(tokio::sync::Notify::new());

        // Set of paths to ignore in debouncer (files we're currently writing)
        let ignored_write_paths = Arc::new(std::sync::Mutex::new(std::collections::HashSet::new()));

        // watch channel for generation tracking. Level-triggered: wait_for checks the
        // current value before subscribing, so no notification can be lost.
        let (commit_generation_tx, commit_generation_rx) = watch::channel::<u64>(0);

        // Compiler-idle flag used by the debouncer hold-off logic.
        // Starts false (busy) because the network root will be enqueued immediately.
        let compiler_idle = Arc::new(std::sync::atomic::AtomicBool::new(false));

        // Fired by the compiler task immediately after setting compiler_idle = true.
        // The transaction task selects on this instead of polling or inspecting the channel.
        let compiler_idle_notify = Arc::new(tokio::sync::Notify::new());

        // Broadcast channel for best-effort fan-out to LSP and future peer subscribers.
        // Capacity 256: enough headroom for a large parse burst without unbounded memory.
        // Receivers that fall behind receive Lagged and should re-query the DB.
        let (belief_broadcast, _) = broadcast::channel::<BeliefEvent>(256);

        // Create the compiler with the event channel and optional HTML output
        let compiler = Arc::new(RwLock::new(if let Some(html_dir) = html_output_dir {
            DocumentCompiler::with_html_output(
                root,
                Some(accum_tx),
                Some(3), // max_reparse_count
                write,   // write rewritten content back to files
                Some(html_dir),
                html_script,
                use_cdn,
                base_url,
                None,
                git_tracking,
            )?
        } else {
            DocumentCompiler::with_html_output(
                root,
                Some(accum_tx),
                Some(3), // max_reparse_count
                write,   // write rewritten content back to files
                None,
                None,
                false,
                None,
                None,
                git_tracking,
            )?
        }));

        let compiler_ref = compiler.clone();
        let compiler_notifier = work_notifier.clone();
        let compiler_global_bb = global_bb.clone();

        let compiler_ignored_paths = ignored_write_paths.clone();
        let compiler_idle_flag = compiler_idle.clone();
        let compiler_idle_notify_flag = compiler_idle_notify.clone();

        // transaction task owns accum_rx exclusively — no RwLock wrapper needed.
        let transaction_global_bb = global_bb.clone();
        let transaction_tx = tx.clone();
        let transaction_commit_generation = commit_generation_tx.clone();
        let transaction_compiler_idle = compiler_idle.clone();
        let transaction_compiler_idle_notify = compiler_idle_notify.clone();
        let transaction_belief_broadcast = belief_broadcast.clone();

        // doc_compiler thread
        let compiler_handle = runtime.spawn(async move {
            tracing::info!("[DocumentCompiler] Starting compiler thread");

            // Check for stale files before starting the main loop.
            // This ensures modified files are re-parsed on watch service startup.
            {
                let mut compiler_write = compiler_ref.write_arc();
                match compiler_write.check_stale_files(&compiler_global_bb, false).await {
                    Ok(stale_files) => {
                        if !stale_files.is_empty() {
                            tracing::info!(
                                "[DocumentCompiler] Found {} stale files to re-parse",
                                stale_files.len()
                            );
                            for stale_file in stale_files {
                                compiler_write.on_file_modified(&stale_file);
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!("[DocumentCompiler] Failed to check stale files: {}", e);
                    }
                }
            }

            loop {
                // Wait for notification that work is available.
                compiler_notifier.notified().await;
                // Mark compiler busy so the debouncer hold-off activates.
                compiler_idle_flag.store(false, Ordering::SeqCst);

                tracing::info!(
                    "[DocumentCompiler] Notification received, processing all pending work"
                );

                // Drive the queue to completion with parse_all.
                let parse_results = {
                    let mut compiler_write = compiler_ref.write_arc();
                    tracing::info!(
                        "[DocumentCompiler] Starting parse_all - remainder_queue: {}, total_parsed: {}",
                        compiler_write.remainder_queue_len(),
                        compiler_write.stats().total_parses
                    );
                    compiler_write.parse_all(compiler_global_bb.clone(), false).await
                };

                match parse_results {
                    Ok(results) => {
                        // Update the debounce-ignore set for every path that was written,
                        // and log dependency discoveries.
                        for result in &results {
                            tracing::debug!(
                                "[belief-compiler] Successfully parsed: {:?}",
                                result.path
                            );

                            // Add this path to the ignore set so the debouncer does not
                            // re-enqueue it when the file watcher fires for a compiler write.
                            let mut path_to_ignore = result.path.clone();
                            if path_to_ignore.is_dir() {
                                if let Some(network_file_path) = detect_network_file(&path_to_ignore) {
                                    tracing::trace!(
                                        "[DocumentCompiler] Resolved BeliefNetwork directory {:?} -> file {:?}",
                                        path_to_ignore, network_file_path
                                    );
                                    path_to_ignore = network_file_path;
                                }
                            }
                            let normalized_path = match crate::paths::canonicalize_path(&path_to_ignore) {
                                Ok(canonical) => {
                                    tracing::trace!(
                                        "[DocumentCompiler] Normalized {:?} -> {:?}",
                                        path_to_ignore, canonical
                                    );
                                    canonical
                                }
                                Err(_) => {
                                    tracing::trace!(
                                        "[DocumentCompiler] Failed to normalize {:?}, using as-is",
                                        path_to_ignore
                                    );
                                    path_to_ignore.clone()
                                }
                            };
                            {
                                let mut ignored = compiler_ignored_paths.lock().unwrap();
                                ignored.insert(normalized_path.clone());
                                tracing::debug!(
                                    "[DocumentCompiler] Ignoring debouncer writes to {:?} (normalized from {:?}) until next compiler-idle",
                                    normalized_path, result.path
                                );
                            }

                            if !result.dependent_paths.is_empty() {
                                tracing::info!(
                                    "[DocumentCompiler] Discovered {} dependent paths from {:?}: {:?}",
                                    result.dependent_paths.len(),
                                    result.path,
                                    result.dependent_paths.iter().map(|(p, _)| p).collect::<Vec<_>>()
                                );
                            }
                        }

                        let stats = {
                            let compiler_read = compiler_ref.read_arc();
                            match compiler_read.finalize_html(compiler_global_bb.clone()).await {
                                Ok(diagnostics) => {
                                    for d in &diagnostics {
                                        match d {
                                            crate::codec::ParseDiagnostic::Warning { message, .. } => {
                                                tracing::warn!("[finalize_html] {}", message);
                                            }
                                            crate::codec::ParseDiagnostic::Info { message, .. } => {
                                                tracing::info!("[finalize_html] {}", message);
                                            }
                                            _ => {}
                                        }
                                    }
                                    compiler_read.stats()
                                }
                                Err(err) => {
                                    tracing::warn!(
                                        "[DocumentCompiler] Finalize html failed with error: {:?}",
                                        err
                                    );
                                    CompilerStats::default()
                                }
                            }
                        };
                        tracing::info!(
                            "[DocumentCompiler] parse_all complete. Final stats: remainder={}, total_parses={}",
                            stats.remainder_queue_len,
                            stats.total_parses
                        );
                    }
                    Err(e) => {
                        tracing::error!("[belief-compiler] parse_all error: {}", e);
                    }
                }

                // Queues are drained: flush ignored_write_paths and signal idle.
                {
                    let mut ignored = compiler_ignored_paths.lock().unwrap();
                    let count = ignored.len();
                    ignored.clear();
                    if count > 0 {
                        tracing::debug!(
                            "[DocumentCompiler] Flushed {} ignored write paths on idle",
                            count
                        );
                    }
                }
                compiler_idle_flag.store(true, Ordering::SeqCst);
                compiler_idle_notify_flag.notify_one();
            }
        });

        // transaction builder/executor thread
        //
        // Owns accum_rx exclusively. Uses tokio::select! to react to either:
        //   (a) new BeliefEvents arriving from the compiler, or
        //   (b) the compiler declaring itself idle via compiler_idle_notify.
        //
        // commit_generation is incremented and commit_notify fired only when both the
        // channel is empty AND compiler_idle == true, ensuring wait_for_idle cannot
        // unblock on a partial batch mid-compile.
        //
        // Each processed event is also forwarded to belief_broadcast for best-effort
        // delivery to LSP and future peer subscribers. Lagged receivers must re-query
        // the DB; the DB path is always reliable.
        let transaction_handle = runtime.spawn(async move {
            let mut accum_rx: UnboundedReceiver<BeliefEvent> = accum_rx;
            loop {
                tokio::select! {
                    // Branch A: a new event arrived from the compiler.
                    maybe_event = accum_rx.recv() => {
                        match maybe_event {
                            None => {
                                // Sender dropped — service is shutting down.
                                tracing::info!("[transaction handler] Event channel closed, exiting.");
                                return Ok(());
                            }
                            Some(first_event) => {
                                // Drain the channel into a single transaction batch.
                                let mut transaction = Transaction::new();
                                let mut events = vec![first_event];
                                // Non-blocking drain of any additional events already queued.
                                while let Ok(ev) = accum_rx.try_recv() {
                                    events.push(ev);
                                }
                                for event in events {
                                    transaction.add_event(&event)?;
                                    // Best-effort broadcast; ignored if no receivers.
                                    let _ = transaction_belief_broadcast.send(event.clone());
                                    if notify {
                                        transaction_tx.send(Event::Belief(event))?;
                                    }
                                }
                                if transaction.has_pending() {
                                    match transaction.execute(&transaction_global_bb.0).await {
                                        Ok(_) => {
                                            tracing::debug!(
                                                "[transaction handler] Committed {} staged events.",
                                                transaction.staged
                                            );
                                            match transaction_global_bb.is_db_balanced().await {
                                                Ok(_) => tracing::debug!("Global DB Cache is balanced"),
                                                Err(e) => tracing::warn!("Global DB Cache is Not Balanced. Errors: {}", e),
                                            }
                                        }
                                        Err(e) => {
                                            tracing::warn!(
                                                "[transaction handler] Error executing transaction: {:?}", e
                                            );
                                        }
                                    }
                                }
                                // Signal idle only if the compiler has also gone idle and
                                // the channel is now empty (no more events in flight).
                                if transaction_compiler_idle.load(Ordering::SeqCst)
                                    && accum_rx.is_empty()
                                {
                                    let next_gen = *transaction_commit_generation.borrow() + 1;
                                    let _ = transaction_commit_generation.send(next_gen);
                                    tracing::debug!(
                                        "[transaction handler] Channel empty + compiler idle \
                                         after batch: signalled wait_for_idle (generation={})",
                                        next_gen
                                    );
                                }
                            }
                        }
                    }

                    // Branch B: the compiler just declared itself idle.
                    _ = transaction_compiler_idle_notify.notified() => {
                        // Drain any events the compiler produced in its final parse
                        // iteration that arrived after our last recv() returned.
                        let mut transaction = Transaction::new();
                        while let Ok(ev) = accum_rx.try_recv() {
                            transaction.add_event(&ev)?;
                            let _ = transaction_belief_broadcast.send(ev.clone());
                            if notify {
                                transaction_tx.send(Event::Belief(ev))?;
                            }
                        }
                        if transaction.has_pending() {
                            match transaction.execute(&transaction_global_bb.0).await {
                                Ok(_) => {
                                    tracing::debug!(
                                        "[transaction handler] Committed {} staged events on \
                                         compiler-idle notification.",
                                        transaction.staged
                                    );
                                    match transaction_global_bb.is_db_balanced().await {
                                        Ok(_) => tracing::debug!("Global DB Cache is balanced"),
                                        Err(e) => tracing::warn!("Global DB Cache is Not Balanced. Errors: {}", e),
                                    }
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        "[transaction handler] Error executing transaction on \
                                         compiler-idle: {:?}", e
                                    );
                                }
                            }
                        }
                        // Channel is now empty and compiler is idle: full cycle complete.
                        let next_gen = *transaction_commit_generation.borrow() + 1;
                        let _ = transaction_commit_generation.send(next_gen);
                        tracing::debug!(
                            "[transaction handler] Compiler-idle notification processed: \
                             signalled wait_for_idle (generation={})",
                            next_gen
                        );
                    }
                }
            }
        });

        let syncer = FileUpdateSyncer {
            compiler,
            compiler_handle,
            transaction_handle,
            work_notifier: work_notifier.clone(),
            ignored_write_paths,
            commit_generation_tx,
            commit_generation_rx,
            compiler_idle,
            compiler_idle_notify,
            belief_broadcast,
        };

        // Do NOT call notify_one here. enable_network_syncer is responsible for enqueuing
        // the network root and firing notify_one. If we fire here the compiler wakes against
        // an empty queue, immediately declares itself idle, and wait_for_idle returns before
        // any real work has been done.

        Ok(syncer)
    }
}

#[derive(Default, Clone, Deserialize)]
pub struct PluginConfig;
