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
//! let service = WatchService::new(root_dir, tx, true)?;
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
//!         Event::Focus(focus_event) => {
//!             println!("Received focus update: {:?}", focus_event);
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
//! let service = WatchService::new(PathBuf::from("/workspace"), tx, true)?;
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
//!     properties::{BeliefNode, Bid},
//!     event::Event,
//! };
//! use std::{sync::mpsc::channel, path::PathBuf};
//!
//! let (tx, _rx) = channel::<Event>();
//! let service = WatchService::new(PathBuf::from("/workspace"), tx, true)?;
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
//!         bid: Bid::nil(),
//!         kind: Default::default(),
//!         title: "New Network".to_string(),
//!         schema: None,
//!         payload: Default::default(),
//!         id: Some("new-network".to_string()),
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
//! let service = WatchService::new(root_dir.clone(), tx, true)?;
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
    beliefbase::BeliefGraph,
    codec::{belief_ir::ProtoBeliefNode, compiler::DocumentCompiler, CodecMap},
    config::{LatticeConfigProvider, NetworkRecord, TomlConfigProvider},
    db::{db_init, DbConnection, Transaction},
    error::BuildonomyError,
    event::{BeliefEvent, Event},
    query::{BeliefSource, PaginatedQuery, Query, ResultsPage},
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
    sync::{mpsc::Sender, Arc},
    time::{Duration, SystemTime},
};
use tokio::{
    runtime::Runtime,
    sync::mpsc::{unbounded_channel, UnboundedReceiver},
    task::JoinHandle,
    time::sleep,
};

/// A file system watcher with debouncing for a belief network
type NetworkWatcher = Debouncer<RecommendedWatcher, FileIdMap>;

/// A watcher paired with its file update syncer
type WatcherWithSyncer = (NetworkWatcher, FileUpdateSyncer);

/// Map of network paths to their watchers and syncers
type NetworkWatcherMap = HashMap<PathBuf, WatcherWithSyncer>;

#[derive(Default)]
struct BnWatchers(pub Arc<Mutex<NetworkWatcherMap>>);

#[derive(Default)]
struct PaginationCache(pub Arc<RwLock<HashMap<Query, (SystemTime, BeliefGraph)>>>);

pub struct WatchService {
    watchers: Arc<Mutex<BnWatchers>>,
    pagination_cache: Arc<Mutex<PaginationCache>>,
    db: DbConnection,
    codecs: CodecMap,
    event_tx: Sender<Event>,
    runtime: Runtime,
    config_provider: Arc<dyn crate::config::LatticeConfigProvider>,
    write: bool,
    html_output_dir: Option<PathBuf>,
}

impl WatchService {
    pub fn new(
        root_dir: PathBuf,
        event_tx: Sender<Event>,
        write: bool,
    ) -> Result<Self, BuildonomyError> {
        Self::with_html_output(root_dir, event_tx, write, None)
    }

    pub fn with_html_output(
        root_dir: PathBuf,
        event_tx: Sender<Event>,
        write: bool,
        html_output_dir: Option<PathBuf>,
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

        let codecs = CodecMap::create();

        Ok(WatchService {
            watchers: Arc::new(Mutex::new(BnWatchers::default())),
            pagination_cache: Arc::new(Mutex::new(PaginationCache::default())),
            db,
            codecs,
            event_tx,
            runtime,
            config_provider,
            write,
            html_output_dir,
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
            ProtoBeliefNode::try_from(&record.node)?.write(&path)?;
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

    pub async fn get_states(
        &self,
        pq: PaginatedQuery,
    ) -> Result<ResultsPage<BeliefGraph>, BuildonomyError> {
        let (mut maybe_page, pagination_complete) = {
            while self.pagination_cache.lock().0.is_locked() {
                tracing::info!("[client operation] Waiting for read access to query cache");
                sleep(Duration::from_millis(100)).await;
            }
            let cache = self.pagination_cache.lock().0.read_arc();
            if let Some((_, res)) = cache.get(&pq.query) {
                let page = res.paginate(pq.limit, pq.offset);
                tracing::debug!("Returning page from cache. Page len: {}", page.count);
                let completely_paged =
                    page.count <= pq.offset.unwrap_or(0) + page.results.states.len();
                (Some(page), completely_paged)
            } else {
                (None, false)
            }
        };

        if pagination_complete {
            while self.pagination_cache.lock().0.is_locked() {
                tracing::info!("[client operation] Waiting for write access to query cache");
                sleep(Duration::from_millis(100)).await;
            }
            let mut cache = self.pagination_cache.lock().0.write_arc();
            cache.remove(&pq.query);
        }

        let page = match maybe_page.take() {
            Some(page) => page,
            None => {
                tracing::debug!("No cached query. Freshly evaluating ...");
                let connection = self.db_connection();
                let fresh_res = connection.eval_query(&pq.query, false).await?;
                let page = fresh_res.paginate(pq.limit, pq.offset);
                tracing::debug!("Returning fresh page. Page len: {}", page.count);
                {
                    tracing::debug!("Caching query");
                    while self.pagination_cache.lock().0.is_locked() {
                        tracing::info!("[client operation] Waiting for write access to query cache to insert new query");
                        sleep(Duration::from_millis(100)).await;
                    }
                    let mut cache = self.pagination_cache.lock().0.write_arc();
                    cache.insert(pq.query.clone(), (SystemTime::now(), fresh_res));
                }
                page
            }
        };
        assert!(!self.pagination_cache.lock().0.is_locked());
        Ok(page)
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
            self.codecs.clone(),
            &self.db,
            &self.event_tx,
            repo_path,
            true,
            &self.runtime,
            self.write,
            self.html_output_dir.clone(),
        )?;

        let compiler_ref = network_syncer.compiler.clone();
        let work_notifier = network_syncer.work_notifier.clone();
        let debouncer_paused = network_syncer.debouncer_paused.clone();
        let debouncer_codec_extensions = self.codecs.extensions();
        let mut debouncer = new_debouncer(
            Duration::from_secs(2),
            None,
            move |result: DebounceEventResult| {
                // Check if debouncer is paused (we're writing files)
                if debouncer_paused.load(std::sync::atomic::Ordering::Relaxed) {
                    tracing::debug!("[Debouncer] Paused, ignoring events");
                    return;
                }

                tracing::info!("[FileUpdateSyncer Debouncer] processing debounce event");
                match result {
                    Ok(events) => {
                        for event in events.iter() {
                            match event.event.kind {
                                EventKind::Create(_)
                                | EventKind::Modify(_)
                                | EventKind::Remove(_) => {
                                    // Filter paths to only valid document files
                                    let sync_paths: Vec<&PathBuf> = event
                                        .paths
                                        .iter()
                                        .filter(|&p| {
                                            !p.file_name()
                                                .map(|file_name| {
                                                    file_name
                                                        .to_str()
                                                        .map(|s| s.starts_with('.'))
                                                        .unwrap_or(false)
                                                })
                                                .unwrap_or(false)
                                                && p.extension()
                                                    .map(|ext| {
                                                        debouncer_codec_extensions
                                                            .iter()
                                                            .any(|ce| ce.as_str() == ext)
                                                    })
                                                    .unwrap_or(false)
                                        })
                                        .collect();

                                    if !sync_paths.is_empty() {
                                        // Enqueue changed files for re-parsing
                                        tracing::info!(
                                            "[Debouncer] {} files to enqueue",
                                            sync_paths.len()
                                        );
                                        while compiler_ref.is_locked() {
                                            tracing::debug!(
                                                "[Debouncer] Waiting for write access to compiler"
                                            );
                                            std::thread::sleep(Duration::from_millis(100));
                                        }
                                        tracing::info!("[Debouncer] Acquired write lock");
                                        let mut compiler = compiler_ref.write();
                                        for path in sync_paths {
                                            tracing::info!(
                                                "[Debouncer] File changed, enqueuing for re-parse: {:?}",
                                                path
                                            );
                                            // Reset processed count to allow re-parsing
                                            compiler.reset_processed(path);
                                            compiler.enqueue(path);
                                        }
                                        tracing::info!("[Debouncer] Finished enqueuing, compiler.has_pending()={}", compiler.has_pending());

                                        // Notify compiler thread that work is available
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
    pub debouncer_paused: Arc<std::sync::atomic::AtomicBool>,
}

impl FileUpdateSyncer {
    #[tracing::instrument(skip_all)]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        _codecs: CodecMap,
        global_bb: &DbConnection,
        tx: &Sender<Event>,
        root: &Path,
        notify: bool,
        runtime: &Runtime,
        write: bool,
        html_output_dir: Option<PathBuf>,
    ) -> Result<FileUpdateSyncer, BuildonomyError> {
        let (accum_tx, accum_rx) = unbounded_channel::<BeliefEvent>();

        // Create notification channel for waking up compiler thread
        let work_notifier = Arc::new(tokio::sync::Notify::new());

        // Flag to pause debouncer while we're writing files
        let debouncer_paused = Arc::new(std::sync::atomic::AtomicBool::new(false));

        // Create the compiler with the event channel and optional HTML output
        let compiler = Arc::new(RwLock::new(if let Some(html_dir) = html_output_dir {
            DocumentCompiler::with_html_output(
                root,
                Some(accum_tx),
                Some(3), // max_reparse_count
                write,   // write rewritten content back to files
                Some(html_dir),
            )?
        } else {
            DocumentCompiler::new(
                root,
                Some(accum_tx),
                Some(3), // max_reparse_count
                write,   // write rewritten content back to files
            )?
        }));

        let compiler_ref = compiler.clone();
        let compiler_notifier = work_notifier.clone();
        let compiler_global_bb = global_bb.clone();
        let transaction_events = Arc::new(RwLock::new(accum_rx));
        let transaction_global_bb = global_bb.clone();
        let transaction_tx = tx.clone();

        // doc_compiler thread
        let compiler_handle = runtime.spawn(async move {
            tracing::info!("[DocumentCompiler] Starting compiler thread");

            loop {
                // Wait for notification that work is available
                compiler_notifier.notified().await;

                tracing::info!(
                    "[DocumentCompiler] Notification received, processing all pending work"
                );

                // Process all pending work in a loop (like parse_all does)
                loop {
                    // Log queue state before parsing
                    {
                        let mut compiler_write = compiler_ref.write_arc();
                        let primary_empty = compiler_write.primary_queue_len() == 0;
                        let reparse_pending = compiler_write.reparse_queue_len() > 0;

                        tracing::info!(
                            "[DocumentCompiler] Loop iteration - primary_queue: {}, reparse_queue: {}, total_parsed: {}",
                            compiler_write.primary_queue_len(),
                            compiler_write.reparse_queue_len(),
                            compiler_write.stats().total_parses
                        );

                        // Mark start of reparse round if transitioning from primary to reparse queue
                        if primary_empty && reparse_pending {
                            compiler_write.start_reparse_round();
                        }
                    }

                    // Parse next document
                    let parse_result = {
                        while compiler_ref.is_locked() {
                            tracing::debug!(
                                "[DocumentCompiler] Waiting for write access to compiler"
                            );
                            sleep(Duration::from_millis(100)).await;
                        }
                        tracing::info!("[DocumentCompiler] Calling parse_next");
                        let mut compiler_write = compiler_ref.write_arc();
                        compiler_write.parse_next(compiler_global_bb.clone()).await
                    };

                    match parse_result {
                        Ok(Some(result)) => {
                            tracing::debug!(
                                "[belief-compiler] Successfully parsed: {:?}",
                                result.path
                            );

                            // Note: DocumentCompiler handles writing when created with write=true
                            // We don't write here to avoid duplicate writes

                            // Note: dependent_paths are already enqueued by parse_next()
                            if !result.dependent_paths.is_empty() {
                                tracing::info!(
                                    "[DocumentCompiler] Discovered {} dependent paths from {:?}: {:?}",
                                    result.dependent_paths.len(),
                                    result.path,
                                    result.dependent_paths.iter().map(|(p, _)| p).collect::<Vec<_>>()
                                );
                            } else {
                                tracing::info!(
                                    "[DocumentCompiler] No dependent paths discovered from {:?}",
                                    result.path
                                );
                            }
                            // Continue to next file in queue
                        }
                        Ok(None) => {
                            let stats = {
                                let compiler_read = compiler_ref.read_arc();
                                compiler_read.stats()
                            };
                            tracing::info!(
                                "[DocumentCompiler] parse_next returned None - Queue is empty. Final stats: primary={}, reparse={}, total_parses={}",
                                stats.primary_queue_len,
                                stats.reparse_queue_len,
                                stats.total_parses
                            );
                            // Break inner loop to wait for next notification
                            break;
                        }
                        Err(e) => {
                            tracing::error!("[belief-compiler] Parse error: {}", e);
                            // Continue processing other files despite error
                            sleep(Duration::from_millis(500)).await;
                        }
                    }
                }
            }
        });

        // transaction builder/executor thread
        let transaction_handle = runtime.spawn(async move {
            loop {
                let is_empty = {
                    while transaction_events.is_locked_exclusive() {
                        tracing::info!(
                            "[transaction handler] Waiting for read \n                             access to transaction event queue"
                        );
                        std::thread::sleep(Duration::from_millis(100));
                    }
                    let rx_read = transaction_events.read_arc();
                    rx_read.is_empty()
                };
                if !is_empty {
                    match perform_transaction(
                        transaction_events.clone(),
                        transaction_global_bb.clone(),
                        transaction_tx.clone(),
                        notify,
                    )
                    .await
                    {
                        Ok(_) => {
                            tracing::debug!("[belief-compiler] Successfully integrated belief updates to the global cache.");
                        }
                        Err(e) => {
                            tracing::warn!(
                                "[belief-compiler] Error performing belief update transaction. Error: {:?}", e
                            );
                        }
                    }
                } else {
                    sleep(Duration::from_secs(1)).await;
                }
            }
        });

        let syncer = FileUpdateSyncer {
            compiler,
            compiler_handle,
            transaction_handle,
            work_notifier: work_notifier.clone(),
            debouncer_paused,
        };

        // Trigger initial notification since files may already be enqueued
        work_notifier.notify_one();

        Ok(syncer)
    }
}

async fn perform_transaction(
    rx_lock: Arc<RwLock<UnboundedReceiver<BeliefEvent>>>,
    global_bb: DbConnection,
    tx: Sender<Event>,
    notify: bool,
) -> Result<(), BuildonomyError> {
    let mut transaction = Transaction::new();
    let mut poll_result = {
        while rx_lock.is_locked() {
            tracing::info!(
                "[perform_transaction] Waiting for write access to belief event receiver"
            );
            sleep(Duration::from_millis(100)).await;
        }
        let mut rx = rx_lock.write_arc();
        rx.try_recv()
    };

    while let Ok(event) = poll_result {
        transaction.add_event(&event)?;
        if notify {
            tx.send(Event::Belief(event))?;
        }

        poll_result = {
            while rx_lock.is_locked() {
                tracing::info!(
                    "[perform_transaction] Waiting for write access to belief event receiver"
                );
                sleep(Duration::from_millis(100)).await;
            }
            let mut rx = rx_lock.write_arc();
            rx.try_recv()
        };

        if poll_result.is_err() {
            // Add a little delay to ensure we process the entire transaction blast before
            // executing the collated transaction.
            sleep(Duration::from_millis(100)).await;
        }
    }
    if transaction.staged > 0 {
        transaction.execute(&global_bb.0).await?;
        match global_bb.is_db_balanced().await {
            Ok(_) => tracing::debug!("Global DB Cache is balanced"),
            Err(e) => tracing::warn!("Global DB Cache is Not Balanced. Errors: {}", e),
        };
    }
    Ok(())
}

#[derive(Default, Clone, Deserialize)]
pub struct PluginConfig;
