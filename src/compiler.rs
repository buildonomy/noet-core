use crate::{
    beliefset::Beliefs,
    codec::{lattice_toml::ProtoBeliefNode, parser::BeliefSetParser, CodecMap},
    config::{LatticeConfigProvider, NetworkRecord, TomlConfigProvider},
    db::{db_init, DbConnection, Transaction},
    error::BuildonomyError,
    event::{BeliefEvent, Event},
    query::{BeliefCache, Focus, PaginatedQuery, Query, ResultsPage},
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
struct PaginationCache(pub Arc<RwLock<HashMap<Query, (SystemTime, Beliefs)>>>);

pub struct LatticeService {
    watchers: Arc<Mutex<BnWatchers>>,
    pagination_cache: Arc<Mutex<PaginationCache>>,
    db: DbConnection,
    codecs: CodecMap,
    event_tx: Sender<Event>,
    runtime: Runtime,
    config_provider: Arc<dyn crate::config::LatticeConfigProvider>,
}

impl LatticeService {
    pub fn new(root_dir: PathBuf, event_tx: Sender<Event>) -> Result<Self, BuildonomyError> {
        let runtime = tokio::runtime::Builder::new_current_thread()
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

        Ok(LatticeService {
            watchers: Arc::new(Mutex::new(BnWatchers::default())),
            pagination_cache: Arc::new(Mutex::new(PaginationCache::default())),
            db,
            codecs,
            event_tx,
            runtime,
            config_provider,
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
            self.enable_belief_network_syncer(&path)?;
        }
        for str_path in removed_networks.iter() {
            let path = PathBuf::from(&str_path);
            self.disable_belief_network_syncer(&path)?;
        }

        if nets != old_nets {
            self.config_provider.set_networks(nets.clone())?;
        }
        Ok(nets)
    }

    pub fn get_focus(&self) -> Result<Focus, BuildonomyError> {
        self.config_provider.get_focus()
    }

    pub fn set_focus(&self, focus: &Focus) -> Result<(), BuildonomyError> {
        self.config_provider.set_focus(focus.clone())
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
    ) -> Result<ResultsPage<Beliefs>, BuildonomyError> {
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

    pub fn enable_belief_network_syncer(&self, repo_path: &PathBuf) -> Result<(), BuildonomyError> {
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
        )?;

        let parser_ref = network_syncer.parser.clone();
        let debouncer_codec_extensions = self.codecs.extensions();
        let mut debouncer = new_debouncer(
            Duration::from_secs(2),
            None,
            move |result: DebounceEventResult| {
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
                                        while parser_ref.is_locked() {
                                            tracing::debug!(
                                                "[Debouncer] Waiting for write access to parser"
                                            );
                                            std::thread::sleep(Duration::from_millis(100));
                                        }
                                        let mut parser = parser_ref.write();
                                        for path in sync_paths {
                                            tracing::info!(
                                                "[Debouncer] File changed, enqueuing for re-parse: {:?}",
                                                path
                                            );
                                            // Reset processed count to allow re-parsing
                                            parser.reset_processed(path);
                                            parser.enqueue(path);
                                        }
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

    pub fn disable_belief_network_syncer(
        &self,
        repo_path: &PathBuf,
    ) -> Result<(), BuildonomyError> {
        let binding = self.watchers.lock();
        let mut watchers = binding.0.lock();
        if let Some((mut debouncer, update_syncer)) = watchers.remove(repo_path) {
            let unwatch_res = debouncer.watcher().unwatch(repo_path);
            update_syncer.parser_handle.abort();
            update_syncer.transaction_handle.abort();
            tracing::debug!("Unwatch_res(path: {:?}) = {:?}", repo_path, unwatch_res);
            unwatch_res?;
        }
        Ok(())
    }
}

pub(crate) struct FileUpdateSyncer {
    pub parser: Arc<RwLock<BeliefSetParser>>,
    pub parser_handle: JoinHandle<Result<(), BuildonomyError>>,
    pub transaction_handle: JoinHandle<Result<(), BuildonomyError>>,
}

impl FileUpdateSyncer {
    #[tracing::instrument(skip_all)]
    pub(crate) fn new(
        _codecs: CodecMap,
        global_cache: &DbConnection,
        tx: &Sender<Event>,
        root: &Path,
        notify: bool,
        runtime: &Runtime,
    ) -> Result<FileUpdateSyncer, BuildonomyError> {
        let (accum_tx, accum_rx) = unbounded_channel::<BeliefEvent>();

        // Create the parser with the event channel
        let parser = Arc::new(RwLock::new(BeliefSetParser::new(
            root,
            Some(accum_tx),
            Some(3), // max_reparse_count
            true,    // write rewritten content back to files
        )?));

        let parser_ref = parser.clone();
        let parser_global_cache = global_cache.clone();
        let transaction_events = Arc::new(RwLock::new(accum_rx));
        let transaction_global_cache = global_cache.clone();
        let transaction_tx = tx.clone();

        // doc_parser thread
        let parser_handle = runtime.spawn(async move {
            tracing::info!("[BeliefSetParser] Starting parser thread");

            loop {
                // Check if there's work to do
                let has_pending = {
                    while parser_ref.is_locked_exclusive() {
                        tracing::debug!("[BeliefSetParser] Waiting for read access to parser");
                        sleep(Duration::from_millis(100)).await;
                    }
                    let parser_read = parser_ref.read_arc();
                    parser_read.has_pending()
                };

                if has_pending {
                    // Parse next document
                    let parse_result = {
                        while parser_ref.is_locked() {
                            tracing::debug!("[BeliefSetParser] Waiting for write access to parser");
                            sleep(Duration::from_millis(100)).await;
                        }
                        let mut parser_write = parser_ref.write_arc();
                        parser_write.parse_next(parser_global_cache.clone()).await
                    };

                    match parse_result {
                        Ok(Some(result)) => {
                            tracing::debug!(
                                "[belief-compiler] Successfully parsed: {:?}",
                                result.path
                            );

                            // Write rewritten content if needed
                            if let Some(new_content) = result.rewritten_content {
                                tracing::info!("Writing updated content to file {:?}", result.path);
                                tokio::fs::write(&result.path, new_content).await?;
                            }

                            // Note: dependent_paths are already enqueued by parse_next()
                            if !result.dependent_paths.is_empty() {
                                tracing::debug!(
                                    "Discovered {} dependent paths from {:?}",
                                    result.dependent_paths.len(),
                                    result.path
                                );
                            }
                        }
                        Ok(None) => {
                            tracing::debug!("[BeliefSetParser] Queue is empty");
                            sleep(Duration::from_secs(1)).await;
                        }
                        Err(e) => {
                            tracing::error!("[belief-compiler] Parse error: {}", e);
                            // Continue processing other files despite error
                            sleep(Duration::from_millis(500)).await;
                        }
                    }
                } else {
                    // No work, sleep
                    sleep(Duration::from_secs(1)).await;
                }
            }
        });

        // transaction accumulator/executor thread
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
                        transaction_global_cache.clone(),
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
            parser,
            parser_handle,
            transaction_handle,
        };
        Ok(syncer)
    }
}

async fn perform_transaction(
    rx_lock: Arc<RwLock<UnboundedReceiver<BeliefEvent>>>,
    global_cache: DbConnection,
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
        transaction.execute(&global_cache.0).await?;
        match global_cache.is_db_balanced().await {
            Ok(_) => tracing::debug!("Global DB Cache is balanced"),
            Err(e) => tracing::warn!("Global DB Cache is Not Balanced. Errors: {}", e),
        };
    }
    Ok(())
}

#[derive(Default, Clone, Deserialize)]
pub struct PluginConfig;
