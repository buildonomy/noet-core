//! [`ShardStore`] — the shard directory as a [`BeliefSource`] + [`BeliefSink`].
//!
//! ## Why this exists
//!
//! `Bid::new` is time-based, so a node the compiler cannot resolve against
//! `global_bb` gets a fresh BID on every parse (`codec/builder.rs`,
//! `NodeSource::Generated`). Before this type, the only stores that survived an
//! invocation were `--write` (BIDs stamped into source frontmatter) and
//! `belief_cache.db` — neither of which is available on a generated corpus built
//! cold in CI. Every annotation anchored to `(bid, version)` was therefore orphaned
//! on the next build.
//!
//! `ShardStore` makes the previous run's shards the durable identity store. Because
//! `GraphBuilder::cache_fetch` is generic over [`BeliefSource`] and reaches its
//! backing store through a single `evaluate` call, **shard awareness lives entirely
//! behind the trait and the compiler is unchanged.**
//!
//! ## Shape
//!
//! ```text
//! open  → read manifest, load global shard (the routing table)
//! read  → evaluate against the in-memory BeliefBase; on a miss, resolve the key
//!         to its home shard, load it, retry once
//! write → apply_batch marks the touched network dirty; nothing hits disk
//! flush → the caller re-exports dirty shards at finalize_html
//! ```
//!
//! Loading a shard is `BeliefBase::merge`, whose third pass drives `PathMapMap`
//! and builds the path index — so `NodeKey::Path` resolves in memory with no SQL and
//! no `paths` table. This is the same mechanism the browser viewer already uses
//! (`wasm.rs::load_shard`).
//!
//! ## Routing
//!
//! The manifest plus the always-resident global shard is a complete routing table:
//!
//! | Key | Home shard found via |
//! |---|---|
//! | `Path { net, .. }`, `Id { net, .. }` | `net` **is** the network bref |
//! | `Bid`, `Bref` | `GlobalShard.bref_index` (node bref → home network bref) |
//!
//! A partially-loaded store is therefore *correct*, not merely fast — which is why
//! the memory budget is a footprint knob rather than a correctness concern. Each
//! shard additionally embeds a halo of the extern nodes its edges reach, so upward
//! traversal never dead-ends at an unloaded network (`shard/export.rs`).
//!
//! ## Monolithic mode
//!
//! Absence of `beliefbase/manifest.json` means the export was below the shard
//! threshold: `beliefbase.msgpack` is loaded as a single unit. Identity preservation
//! works there too; only *skip* requires per-network sharding.
//!
//! ## A read-only store is a snapshot, and it goes stale
//!
//! [`ShardStore::open_read_only`] loads the generation present at open. **Nothing
//! notifies it when the writer publishes a new one.** A reader that outlives a single
//! query must therefore poll — see [`ShardStore::generation_token`] and
//! [`ShardStore::is_stale`] — and reopen when the token changes.
//!
//! This is a deliberate consequence of readers taking no lock. The alternative,
//! blocking readers behind the writer, would stall the viewer on every export. Since
//! publication is atomic, a stale reader is always internally *consistent*; it is
//! merely behind. Serving a coherent older generation is a much better failure mode
//! than serving a torn current one, but a reader that never polls will serve the
//! opening generation forever.
//!
//! MCP static mode already polls `manifest.json`'s mtime for exactly this reason
//! (`mcp/mod.rs::maybe_reload`); `generation_token` is that check, generalised, so
//! each consumer does not re-invent it.
//!
//! ## Not a browser store
//!
//! The browser viewer (`wasm.rs::BeliefBaseWasm`) does the same *conversion* through
//! the same [`NetworkShard::into_graph`], but it is not and should not become a
//! `ShardStore`:
//!
//! - [`BeliefSource`] requires `Send + Sync`; `BeliefBaseWasm` is built on `RefCell`
//!   because `wasm32` is single-threaded.
//! - This store *reads files* synchronously. The browser has no filesystem and
//!   receives already-fetched bytes from JS, so fetch-on-miss — the behaviour most
//!   worth sharing — is precisely the part that cannot be.
//! - The viewer tracks per-shard BID membership so `unload_shard` knows what to drop;
//!   this store tracks only which networks are resident, because it never unloads.
//!
//! When server-side eviction lands (Issue 66 § Performance) the residency
//! bookkeeping converges and the question is worth reopening.
//!
//! ## References
//!
//! - `docs/project/0_open/ISSUE_66_INCREMENTAL_PARSE.md` § `ShardStore`
//! - `docs/design/annotation/living_corpus.md` §2 — Layer 2 is single-writer and a
//!   pure function of Layer 1

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::RwLock;

use crate::beliefbase::{BeliefBase, BeliefGraph, BeliefSink};
use crate::event::BeliefEvent;
use crate::nodekey::NodeKey;
use crate::properties::{Bid, Bref};
use crate::query::{BeliefSource, BoxFuture, QueryPackage, QuerySpec, SubmapResult, TapeFn};
use crate::shard::lock::WriteLock;
use crate::shard::manifest::ShardManifest;
use crate::shard::wire::{GlobalShard, NetworkShard};
use crate::BuildonomyError;

/// Filename of the monolithic export, used when no shard manifest is present.
const MONOLITHIC_FILE: &str = "beliefbase.msgpack";

/// Mutable interior, shared between clones.
///
/// `base` starts from `BeliefBase::default()`, which seeds the API / const-namespace
/// root. That is deliberate: the parse resolves const-namespace keys against
/// `global_bb`, so a store that began truly empty would differ from the DB-backed
/// store it replaces on the very first lookup.
#[derive(Debug, Default)]
struct StoreInner {
    /// The live graph. Authoritative while the process runs.
    base: BeliefBase,
    /// Network brefs whose shard has been loaded into `base`.
    loaded: BTreeSet<Bref>,
    /// Network brefs mutated since the last export.
    dirty: BTreeSet<Bref>,
    /// Node bref → home network bref, from the global shard.
    bref_index: BTreeMap<String, String>,
    /// Network bref → shard path, relative to `beliefbase/`.
    shard_paths: BTreeMap<Bref, String>,
    /// Generation published on disk when this store opened. Compared against the
    /// current token to answer [`ShardStore::is_stale`].
    opened_generation: Option<std::time::SystemTime>,
}

/// The shard directory presented as a belief store.
///
/// Cloning is cheap and shares one interior, matching how `BeliefAccumulator`
/// distributes its store across parallel parse tasks.
#[derive(Debug, Clone)]
pub struct ShardStore {
    output_dir: PathBuf,
    inner: Arc<RwLock<StoreInner>>,
    /// Present when opened for writing. Dropping this releases the lock, so it is
    /// held for as long as any clone of the store survives.
    _write_lock: Option<Arc<WriteLock>>,
}

impl ShardStore {
    /// Open the store read-only. Takes no lock and never blocks behind a writer.
    ///
    /// Readers are safe without a lock because publication is atomic: shards and the
    /// manifest are renamed into place rather than written through, so a reader sees
    /// a whole generation or the previous one.
    ///
    /// # This is a snapshot
    ///
    /// The store holds the generation present at open and is **not** notified when
    /// the writer publishes a new one. A long-lived reader must poll
    /// [`is_stale`](Self::is_stale) and reopen:
    ///
    /// ```no_run
    /// # use noet_core::shard::ShardStore;
    /// # fn f(output_dir: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    /// let mut store = ShardStore::open_read_only(output_dir)?;
    /// // ... later, on a timer or before serving a request ...
    /// if store.is_stale() {
    ///     store = ShardStore::open_read_only(output_dir)?;
    /// }
    /// # Ok(()) }
    /// ```
    pub fn open_read_only(output_dir: impl AsRef<Path>) -> Result<Self, BuildonomyError> {
        Self::open_inner(output_dir.as_ref(), None)
    }

    /// Open the store for writing, acquiring the exclusive writer lock.
    ///
    /// The lock is held for the lifetime of the store — for `noet serve`, the whole
    /// session — rather than only across an export, so "exactly one writer owns this
    /// output directory" holds continuously. Fails fast if another process holds it.
    pub fn open_writable(output_dir: impl AsRef<Path>) -> Result<Self, BuildonomyError> {
        let dir = output_dir.as_ref();
        let lock = WriteLock::acquire(dir)?;
        Self::open_inner(dir, Some(Arc::new(lock)))
    }

    /// A store with no backing directory: an ephemeral in-memory graph.
    ///
    /// Used when there is nowhere to persist to — `noet parse` without
    /// `--html-output`. Every read path behaves identically; there are simply no
    /// shards to fault in, so nothing is preserved across invocations. **BIDs are
    /// re-minted on every run in this mode**, which is why `--html-output` becomes
    /// required (Issue 66 § CLI consequences).
    ///
    /// Takes no lock: with no directory there is nothing to exclude anyone from.
    pub fn in_memory() -> Self {
        Self {
            output_dir: PathBuf::new(),
            inner: Arc::new(RwLock::new(StoreInner::default())),
            _write_lock: None,
        }
    }

    /// Whether this store has a backing directory.
    pub fn is_persistent(&self) -> bool {
        !self.output_dir.as_os_str().is_empty()
    }

    fn open_inner(
        output_dir: &Path,
        write_lock: Option<Arc<WriteLock>>,
    ) -> Result<Self, BuildonomyError> {
        // Capture the generation token *before* reading, so a publish that races this
        // open is detected by the next `is_stale` poll rather than being missed.
        let opened_generation = Self::generation_token(output_dir);

        let store = Self {
            output_dir: output_dir.to_path_buf(),
            inner: Arc::new(RwLock::new(StoreInner {
                opened_generation,
                ..Default::default()
            })),
            _write_lock: write_lock,
        };

        let bb_dir = output_dir.join("beliefbase");
        let manifest_path = bb_dir.join("manifest.json");

        if manifest_path.exists() {
            let manifest: ShardManifest =
                serde_json::from_str(&std::fs::read_to_string(&manifest_path).map_err(|e| {
                    BuildonomyError::Custom(format!("cannot read {}: {e}", manifest_path.display()))
                })?)
                .map_err(|e| {
                    BuildonomyError::Custom(format!("malformed {}: {e}", manifest_path.display()))
                })?;

            // The global shard is the routing table; it is always resident.
            let global_path = bb_dir.join("global.msgpack");
            if global_path.exists() {
                let global: GlobalShard = read_msgpack(&global_path)?;
                let bref_index = global.bref_index.clone();
                let graph = global.into_graph();
                let mut guard = store.inner.write();
                guard.base.merge(&graph);
                guard.bref_index = bref_index;
            }

            {
                let mut guard = store.inner.write();
                for net in &manifest.networks {
                    if let Ok(bref) = Bref::try_from(net.bref.as_str()) {
                        guard.shard_paths.insert(bref, net.path.clone());
                    }
                }
            }

            tracing::debug!(
                networks = manifest.networks.len(),
                "opened ShardStore over {}",
                output_dir.display()
            );
        } else {
            let mono = output_dir.join(MONOLITHIC_FILE);
            if mono.exists() {
                let graph: BeliefGraph = read_msgpack(&mono)?;
                store.inner.write().base.merge(&graph);
                tracing::debug!("opened ShardStore in monolithic mode ({})", mono.display());
            } else {
                tracing::debug!(
                    "no prior export at {} — starting empty",
                    output_dir.display()
                );
            }
        }

        Ok(store)
    }

    /// Load every network shard listed in the manifest.
    ///
    /// Fetch-on-miss makes this unnecessary for correctness. It exists because a
    /// parse is about to touch most of the corpus anyway, and loading up front is
    /// cheaper than a fault per network.
    pub fn load_all(&self) -> Result<usize, BuildonomyError> {
        let brefs: Vec<Bref> = self.inner.read().shard_paths.keys().copied().collect();
        let mut loaded = 0;
        for bref in brefs {
            if self.ensure_loaded(&bref)? {
                loaded += 1;
            }
        }
        Ok(loaded)
    }

    /// Load one network's shard if it is not already resident.
    ///
    /// Returns whether a load actually happened.
    pub fn ensure_loaded(&self, bref: &Bref) -> Result<bool, BuildonomyError> {
        let rel = {
            let guard = self.inner.read();
            if guard.loaded.contains(bref) {
                return Ok(false);
            }
            match guard.shard_paths.get(bref) {
                Some(p) => p.clone(),
                None => return Ok(false), // not a known network — a genuine miss
            }
        };

        let path = self.output_dir.join("beliefbase").join(&rel);
        if !path.exists() {
            tracing::warn!(
                "shard {} listed in the manifest is missing from disk; BIDs for this \
                 network will not be preserved",
                path.display()
            );
            return Ok(false);
        }

        let shard: NetworkShard = read_msgpack(&path)?;
        let graph = shard.into_graph();

        let mut guard = self.inner.write();
        guard.base.merge(&graph);
        guard.loaded.insert(*bref);
        tracing::debug!("loaded shard {bref} from {}", path.display());
        Ok(true)
    }

    /// Resolve the home network of a key, so a miss can be turned into a fetch.
    fn home_network(&self, key: &NodeKey) -> Option<Bref> {
        let guard = self.inner.read();
        match key {
            NodeKey::Path { net, .. } | NodeKey::Id { net, .. } => Some(*net),
            NodeKey::Bref { bref } => guard
                .bref_index
                .get(&bref.to_string())
                .and_then(|home| Bref::try_from(home.as_str()).ok()),
            NodeKey::Bid { bid } => guard
                .bref_index
                .get(&bid.bref().to_string())
                .and_then(|home| Bref::try_from(home.as_str()).ok()),
        }
    }

    /// Load the home shards of every seed key in `package` that is not yet resident.
    ///
    /// Returns whether anything was loaded, so the caller knows to retry.
    fn fault_in_seeds(&self, package: &QueryPackage) -> bool {
        let mut loaded_any = false;
        for step in &package.spec().steps {
            if let TapeFn::Keys(keys) = &step.input {
                for key in keys {
                    if let Some(home) = self.home_network(key) {
                        let already = self.inner.read().loaded.contains(&home);
                        if !already {
                            match self.ensure_loaded(&home) {
                                Ok(true) => loaded_any = true,
                                Ok(false) => {}
                                Err(e) => tracing::warn!("shard fetch-on-miss failed: {e}"),
                            }
                        }
                    }
                }
            }
        }
        loaded_any
    }

    /// Networks mutated since the last export.
    pub fn dirty_networks(&self) -> BTreeSet<Bref> {
        self.inner.read().dirty.clone()
    }

    /// Clear the dirty set — call after a successful export.
    pub fn clear_dirty(&self) {
        self.inner.write().dirty.clear();
    }

    /// Network brefs currently resident in memory.
    pub fn loaded_networks(&self) -> BTreeSet<Bref> {
        self.inner.read().loaded.clone()
    }

    /// Number of nodes currently resident.
    pub fn node_count(&self) -> usize {
        self.inner.read().base.states().len()
    }

    /// The output directory this store was opened over.
    pub fn output_dir(&self) -> &Path {
        &self.output_dir
    }

    /// An opaque token identifying the published generation on disk *right now*.
    ///
    /// Compare against [`opened_generation`](Self::opened_generation) to detect that
    /// a writer has published since this store opened. `None` means no generation is
    /// published (no manifest and no monolithic export).
    ///
    /// The token is the manifest's modification time. That is sound here despite
    /// mtime being unreliable for *skip* decisions (§ Content hashing rejects it for
    /// that) because the failure modes differ: a missed staleness signal costs a
    /// reader one stale generation until the next poll, whereas a missed skip signal
    /// silently corrupts the compiled graph. A reader also polls repeatedly, so a
    /// same-second publish is caught by the following poll.
    pub fn generation_token(output_dir: &Path) -> Option<std::time::SystemTime> {
        let manifest = output_dir.join("beliefbase").join("manifest.json");
        let probe = if manifest.exists() {
            manifest
        } else {
            output_dir.join(MONOLITHIC_FILE)
        };
        std::fs::metadata(probe).ok()?.modified().ok()
    }

    /// The generation token captured when this store was opened.
    pub fn opened_generation(&self) -> Option<std::time::SystemTime> {
        self.inner.read().opened_generation
    }

    /// Whether a newer generation has been published since this store opened.
    ///
    /// Reopen the store when this returns `true`; the in-memory graph is not
    /// refreshed in place. Always `false` for a writable store, which owns the
    /// generation and cannot be overtaken by another writer.
    pub fn is_stale(&self) -> bool {
        if self._write_lock.is_some() || !self.is_persistent() {
            return false;
        }
        Self::generation_token(&self.output_dir) != self.opened_generation()
    }

    /// Run `f` against the live graph.
    ///
    /// The export path needs a `&BeliefBase` to hand to `export_beliefbase`; this
    /// keeps the lock discipline inside the store rather than leaking a guard.
    pub fn with_base<R>(&self, f: impl FnOnce(&BeliefBase) -> R) -> R {
        f(&self.inner.read().base)
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn read_msgpack<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, BuildonomyError> {
    let bytes = std::fs::read(path)
        .map_err(|e| BuildonomyError::Custom(format!("cannot read {}: {e}", path.display())))?;
    rmp_serde::from_slice(&bytes)
        .map_err(|e| BuildonomyError::Custom(format!("malformed shard {}: {e}", path.display())))
}

/// Which networks an event's subjects belong to, for dirty tracking.
///
/// Path events name their network directly. Node and relation events do not, so
/// their subjects are resolved against the path index — the same index that decides
/// which shard a node is exported into, which is what makes the attribution agree
/// with the export partition.
fn event_networks(base: &BeliefBase, event: &BeliefEvent, out: &mut BTreeSet<Bref>) {
    match event {
        BeliefEvent::PathAdded(net, ..)
        | BeliefEvent::PathUpdate(net, ..)
        | BeliefEvent::PathsRemoved(net, ..) => {
            out.insert(*net);
        }
        BeliefEvent::NodeUpdate(_, node, _) | BeliefEvent::NodeUpsert(_, node, _) => {
            networks_of(base, &node.bid, out);
        }
        BeliefEvent::NodesRemoved(bids, _) => {
            for bid in bids {
                networks_of(base, bid, out);
            }
        }
        BeliefEvent::RelationUpdate(source, sink, ..)
        | BeliefEvent::RelationRemoved(source, sink, ..) => {
            networks_of(base, source, out);
            networks_of(base, sink, out);
        }
        _ => {}
    }
}

/// Networks whose path index contains `bid`.
fn networks_of(base: &BeliefBase, bid: &Bid, out: &mut BTreeSet<Bref>) {
    for (net, entries) in base.paths().all_paths() {
        if entries.iter().any(|(_, b, _)| b == bid) {
            out.insert(net);
        }
    }
}

// ---------------------------------------------------------------------------
// BeliefSource
// ---------------------------------------------------------------------------

impl BeliefSource for ShardStore {
    /// Evaluate against the resident graph, faulting in home shards on a miss.
    ///
    /// The retry is single-shot by construction: `fault_in_seeds` only reports
    /// progress when it actually loaded a shard, and a shard cannot be loaded twice.
    fn evaluate<'a>(
        &'a self,
        package: &'a mut QueryPackage,
    ) -> BoxFuture<'a, Result<(), BuildonomyError>> {
        Box::pin(async move {
            if self.fault_in_seeds(package) {
                tracing::trace!("faulted in shard(s) before evaluate");
            }
            self.inner.read().base.evaluate_query(package)
        })
    }

    fn submap<'a>(
        &'a self,
        network_bid: Bid,
        path: &'a str,
        depth: u8,
        include_index: bool,
    ) -> BoxFuture<'a, SubmapResult> {
        Box::pin(async move {
            let bref = network_bid.bref();
            let _ = self.ensure_loaded(&bref);
            Ok(self
                .inner
                .read()
                .base
                .paths()
                .submap(&bref, path, depth, include_index))
        })
    }

    fn submap_by_bid<'a>(
        &'a self,
        network_bid: Bid,
        entry: Option<Bid>,
        depth: u8,
        include_index: bool,
    ) -> BoxFuture<'a, SubmapResult> {
        Box::pin(async move {
            let bref = network_bid.bref();
            let _ = self.ensure_loaded(&bref);
            Ok(self
                .inner
                .read()
                .base
                .paths()
                .submap_by_bid(&bref, entry, depth, include_index))
        })
    }

    /// Export the resident graph.
    ///
    /// Only loaded shards contribute; call [`ShardStore::load_all`] first when the
    /// caller needs the whole corpus (the export path does).
    fn export_beliefgraph(&self) -> BoxFuture<'_, Result<BeliefGraph, BuildonomyError>> {
        Box::pin(async move {
            // Evaluate synchronously under the guard rather than awaiting while
            // holding it: `inner` is a `parking_lot::RwLock`, which is not
            // async-aware, so holding its guard across an await can deadlock the
            // executor. `evaluate_query` is the synchronous core that
            // `BeliefBase::export_beliefgraph` wraps, so this is the same work
            // without the future.
            let guard = self.inner.read();
            let all_bids: Vec<Bid> = guard.base.states().keys().copied().collect();
            let mut package = QueryPackage::new(QuerySpec::seed(TapeFn::Bids(all_bids)));
            guard.base.evaluate_query(&mut package)?;
            Ok(package.into_graph())
        })
    }

    /// Not implemented, deliberately.
    ///
    /// `DbConnection` answered this from its `file_mtimes` table, which existed
    /// because there was no durable prior generation to compare against. There is one
    /// now: "did this file change?" is answered by comparing `source_hashes` in the
    /// shard manifest, which also catches additions and deletions by key-set
    /// comparison. The default empty map is the correct answer here, not a stub.
    fn get_file_mtimes(&self) -> BoxFuture<'_, Result<BTreeMap<PathBuf, i64>, BuildonomyError>> {
        Box::pin(async { Ok(BTreeMap::new()) })
    }
}

// ---------------------------------------------------------------------------
// BeliefSink
// ---------------------------------------------------------------------------

impl BeliefSink for ShardStore {
    /// Apply a batch to the resident graph and mark the touched networks dirty.
    ///
    /// Writes are deferred: nothing reaches disk here. The caller re-exports dirty
    /// shards at `finalize_html`, which matches `generational_archive.md` §9.1's
    /// single `STAGED` generation overwritten every parse. Deferring costs no
    /// correctness because the resident graph is authoritative while the process runs.
    async fn apply_batch(&mut self, events: &[BeliefEvent]) -> Result<(), BuildonomyError> {
        let mut guard = self.inner.write();
        for event in events {
            tracing::trace!("{event:?}");
            let _ = guard.base.process_event(event);
        }
        // Attribute after applying, so newly-created nodes have a path entry.
        let mut touched = BTreeSet::new();
        for event in events {
            event_networks(&guard.base, event, &mut touched);
        }
        guard.dirty.extend(touched);
        Ok(())
    }
}
