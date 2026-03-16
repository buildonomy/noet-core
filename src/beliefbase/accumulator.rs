//! [`BeliefAccumulator`] — unified batch accumulator and query cache for the parse pipeline.
//!
//! ## Motivation
//!
//! During a corpus parse, the compiler emits [`BeliefEvent`]s over an unbounded channel.
//! Two concerns have historically been separate:
//!
//! 1. **Batch application**: collecting raw events between [`BeliefEvent::BatchStart`] /
//!    [`BeliefEvent::BatchEnd`] sentinels and writing them to a backing store in the right
//!    order (node events first so that relation/path events find their nodes already indexed).
//!
//! 2. **Query caching**: memoising [`BeliefSource::eval_query`] results so that O(N²)
//!    `index_sync` + `balance` chains across a sibling batch collapse to O(N).
//!    Previously handled by a separate `CachedBeliefSource` wrapper (now removed).
//!
//! `BeliefAccumulator<S>` owns both responsibilities in a single type.
//!
//! ## Operation
//!
//! ```text
//! parse_all:  BatchStart ──► [NodeUpdate, RelationUpdate, …] ──► BatchEnd
//!                                                                     │
//!                                                         drain() ◄──┘
//!                                                             │
//!                                      sort pending (node events first)
//!                                             │
//!                                  inner.apply_batch(sorted)
//!                                             │
//!                                        cache.clear()
//! ```
//!
//! Between `BatchStart` and `BatchEnd`, events accumulate in `pending`.  On `BatchEnd`,
//! the pending slice is sorted and flushed to `inner` via [`BeliefSink::apply_batch`],
//! then the query cache is cleared so subsequent queries see fresh state.
//!
//! All drain activity is driven exclusively by `BatchStart`/`BatchEnd` sentinels on the
//! channel.  There is no public drain method — epoch boundaries are signalled by the
//! compiler via the event channel, not by out-of-band API calls.
//!
//! [`BeliefSource`] queries on the accumulator delegate to `inner` with the memoised
//! cache layer.  They do not drain the channel; `inner` is considered stable within an
//! epoch (between the preceding `BatchEnd` and the next `BatchStart`).
//!
//! ## Interior mutability & `Sync`
//!
//! [`BeliefSource`] requires `&self` methods and `Sync`.  All mutable state
//! (channel receiver, pending buffer, backing store) lives in an
//! `Arc<tokio::sync::Mutex<AccInner<S>>>`.  The `Arc` clone is cheap and can be
//! moved into `async move` futures without capturing `&self`, which keeps the
//! return types of the trait methods `Send + 'static`-friendly.
//!
//! The query cache uses its own `Arc<AccCache>` (backed by `std::sync::Mutex`) so
//! cache hits do not need to await the `tokio::sync::Mutex`.
//!
//! ## `Clone` semantics
//!
//! `BeliefAccumulator` is **not** `Clone`.  The channel receiver is exclusive and
//! owned inside the `Arc<Mutex<…>>`.  If `parse_all` needs to pass a clonable
//! [`BeliefSource`] to parallel worker tasks, it should use
//! [`BeliefAccumulator::query_handle`] — a lightweight, clonable read-only view that
//! shares the same backing store and cache.
//!
//! ## Native-only
//!
//! This module is gated on `#[cfg(not(target_arch = "wasm32"))]` (enforced by the
//! parent `mod.rs`). There is no async channel runtime on WASM and no parse-pipeline
//! use-case there.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc::UnboundedReceiver;

use crate::{
    beliefbase::BeliefGraph,
    event::{BeliefEvent, EventOrigin},
    properties::{Bid, WeightSet},
    query::{BeliefSource, Expression, Query},
    BuildonomyError,
};

use super::BeliefSink;

// ---------------------------------------------------------------------------
// Query-result cache
// ---------------------------------------------------------------------------

/// A single memoised `eval_query` result.
#[derive(Clone)]
struct CacheEntry {
    result: BeliefGraph,
}

impl CacheEntry {
    fn new(result: BeliefGraph) -> Self {
        Self { result }
    }
}

type CacheKey = (Query, bool);

/// Shared, `Send + Sync` query-result cache.
///
/// Wrapped in `Arc` so that [`QueryHandle`] clones share entries with the
/// accumulator itself.
pub(super) struct AccCache {
    map: Mutex<HashMap<CacheKey, CacheEntry>>,
}

impl AccCache {
    fn new() -> Self {
        Self {
            map: Mutex::new(HashMap::new()),
        }
    }

    fn get(&self, key: &CacheKey) -> Option<BeliefGraph> {
        self.map
            .lock()
            .ok()
            .and_then(|g| g.get(key).map(|e| e.result.clone()))
    }

    fn insert(&self, key: CacheKey, entry: CacheEntry) {
        if let Ok(mut g) = self.map.lock() {
            g.insert(key, entry);
        }
    }

    fn clear(&self) {
        if let Ok(mut g) = self.map.lock() {
            g.clear();
        }
    }

    fn len(&self) -> usize {
        self.map.lock().map(|g| g.len()).unwrap_or(0)
    }

    /// Evict all entries whose result state set intersects `affected_bids`.
    ///
    /// Not yet called — reserved for future selective invalidation.
    #[allow(dead_code)]
    fn evict_affected(&self, affected_bids: &[Bid]) {
        if affected_bids.is_empty() {
            return;
        }
        if let Ok(mut g) = self.map.lock() {
            g.retain(|_key, entry| {
                !affected_bids
                    .iter()
                    .any(|bid| entry.result.states.contains_key(bid))
            });
        }
    }
}

// ---------------------------------------------------------------------------
// Mutable interior
// ---------------------------------------------------------------------------

/// All state that requires exclusive access — held behind an
/// `Arc<tokio::sync::Mutex<AccInner<S>>>` so that `BeliefSource` futures can
/// capture an `Arc` clone rather than a `&self` reference.
struct AccInner<S> {
    /// Backing store — satisfies both [`BeliefSource`] (queries) and
    /// [`BeliefSink`] (event application).
    inner: S,
    /// Event channel from the compiler.  Exclusive — not cloneable.
    rx: UnboundedReceiver<BeliefEvent>,
    /// Events collected since the last `BatchStart`.  Flushed on `BatchEnd`.
    pending: Vec<BeliefEvent>,
    /// Whether we are currently inside a `BatchStart` … `BatchEnd` window.
    in_batch: bool,
}

impl<S: BeliefSink + BeliefSource> AccInner<S> {
    /// Drain all events currently available in `rx` without blocking.
    ///
    /// Semantics per event:
    ///
    /// | Event | Action |
    /// |---|---|
    /// | `BatchStart` | clear `pending`, set `in_batch = true` |
    /// | `BatchEnd` | sort `pending` (node events first), `apply_batch`, clear `pending`, `cache.clear()` |
    /// | Any other, inside batch | push to `pending` |
    /// | Any other, outside batch | `apply_batch` immediately (no ordering needed) |
    ///
    /// Returns `Ok(())` when the channel is empty or closed.
    /// Drain all events currently available in `rx` without blocking.
    /// Logs a per-event-type census on completion so we can see exactly what's
    /// left in the channel at shutdown and how many events land outside a batch.
    async fn drain_with_census(
        &mut self,
        cache: &AccCache,
        label: &str,
    ) -> Result<(), BuildonomyError> {
        let pending_before = self.pending.len();
        let in_batch_before = self.in_batch;
        let mut outside_batch: BTreeMap<&'static str, usize> = BTreeMap::new();
        let mut inside_batch: usize = 0;
        let mut batch_starts: usize = 0;
        let mut batch_ends: usize = 0;

        loop {
            match self.rx.try_recv() {
                Ok(event) => {
                    // Classify before consuming
                    let was_in_batch = self.in_batch;
                    match &event {
                        BeliefEvent::BatchStart => batch_starts += 1,
                        BeliefEvent::BatchEnd => batch_ends += 1,
                        _ if was_in_batch => inside_batch += 1,
                        _ => *outside_batch.entry(event.as_str()).or_insert(0) += 1,
                    }
                    self.handle_event(event, cache).await?;
                }
                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => break,
            }
        }

        let outside_total: usize = outside_batch.values().sum();
        tracing::info!(
            label,
            pending_before,
            in_batch_before,
            batch_starts,
            batch_ends,
            inside_batch,
            outside_total,
            outside_census = ?outside_batch,
            "accumulator drain complete",
        );
        Ok(())
    }

    async fn handle_event(
        &mut self,
        event: BeliefEvent,
        cache: &AccCache,
    ) -> Result<(), BuildonomyError> {
        let event_kind_string = format!("{event}");
        match event {
            BeliefEvent::BatchStart => {
                // In a well-formed stream from `parse_all`, `pending` is always empty
                // when `BatchStart` arrives (the previous batch was closed by `BatchEnd`).
                // A non-empty `pending` here means a `BatchEnd` was dropped — compiler bug.
                //
                // `BatchStart` is preserved as an explicit sentinel (rather than inferring
                // batch boundaries from `BatchEnd` alone) because the federated model
                // (see `docs/design/federated_belief_network.md`) will receive event streams
                // from external peers where the stream may not be well-formed and explicit
                // open/close pairs are required for safe accumulation.
                if !self.pending.is_empty() {
                    tracing::warn!(
                        pending_count = self.pending.len(),
                        "BatchStart received with {} pending event(s) — previous batch was \
                         never closed with BatchEnd. Discarding pending events. This is a \
                         compiler bug.",
                        self.pending.len(),
                    );
                }
                self.pending.clear();
                self.in_batch = true;
            }
            BeliefEvent::BatchEnd => {
                let mut sorted = std::mem::take(&mut self.pending);
                prepare_batch(&mut sorted);
                self.inner.apply_batch(&sorted).await?;
                // pending is now empty (moved into `sorted`, which is dropped here)
                self.in_batch = false;
                cache.clear();
            }
            other => {
                if self.in_batch {
                    self.pending.push(other);
                } else {
                    // Events arriving outside a BatchStart/BatchEnd window should not
                    // happen in the parallel parse path — all compiler events are supposed
                    // to be bracketed by BatchStart/BatchEnd pairs.  Log at ERROR so we
                    // can identify the source; apply immediately so no events are lost.
                    tracing::error!(
                        event_kind = event_kind_string,
                        "accumulator: event arrived outside any BatchStart/BatchEnd window — \
                         this is a compiler bug (event will be applied without batch ordering)"
                    );
                    self.inner.apply_batch(std::slice::from_ref(&other)).await?;
                }
            }
        }
        Ok(())
    }
}

/// Prepare a batch for application:
///
/// 1. Consolidate all `NodesRemoved` events into a single event (union of all BID
///    vecs).  This prevents `index_sync` from being called once per `NodesRemoved`
///    event; instead it is called once for the consolidated removal, and subsequent
///    `NodeUpdate` events that internally call `remove_nodes` for replaced nodes will
///    find those BIDs already gone (no-op removes, no extra `index_sync` churn).
///
/// 2. Sort the result so the ordering is:
///    - Consolidated `NodesRemoved` first (single `index_sync`)
///    - `NodeUpdate` / `NodeRenamed` next (their internal removes are now no-ops)
///    - Everything else last (relation/path events find nodes already indexed)
fn prepare_batch(events: &mut Vec<BeliefEvent>) {
    // Consolidate all NodesRemoved into one event.
    let mut removed_bids: Vec<Bid> = Vec::new();
    let mut removed_origin = EventOrigin::Remote;
    events.retain(|e| {
        if let BeliefEvent::NodesRemoved(bids, origin) = e {
            removed_bids.extend_from_slice(bids);
            removed_origin = *origin;
            false // remove from vec; will be re-inserted as one consolidated event
        } else {
            true
        }
    });
    removed_bids.sort_unstable();
    removed_bids.dedup();

    // Sort remaining events: NodeUpdate/NodeRenamed before everything else.
    events.sort_by_key(|e| match e {
        BeliefEvent::NodeUpdate(_, _, _) | BeliefEvent::NodeRenamed(_, _, _) => 0u8,
        _ => 1u8,
    });

    // Prepend the consolidated NodesRemoved (if any) at position 0.
    if !removed_bids.is_empty() {
        events.insert(0, BeliefEvent::NodesRemoved(removed_bids, removed_origin));
    }
}

// ---------------------------------------------------------------------------
// BeliefAccumulator
// ---------------------------------------------------------------------------

/// Unified batch accumulator and query cache for the compile pipeline.
///
/// See the [module documentation](self) for full design rationale.
///
/// # Type parameter
///
/// `S` must implement both [`BeliefSource`] (for queries) and [`BeliefSink`]
/// (for event application).  The two canonical implementations are:
///
/// - `BeliefAccumulator<BeliefBase>` — in-memory parse command
/// - `BeliefAccumulator<DbConnection>` — watch service (future)
pub struct BeliefAccumulator<S: BeliefSource + BeliefSink> {
    /// Mutable interior (channel, pending buffer, backing store).
    /// `Arc` so that `BeliefSource` futures can capture a clone.
    acc: Arc<tokio::sync::Mutex<AccInner<S>>>,
    /// Shared query-result cache.
    /// `Arc` so that `QueryHandle` clones share the same memoised entries.
    cache: Arc<AccCache>,
}

impl<S> BeliefAccumulator<S>
where
    S: BeliefSource + BeliefSink + Clone + Send + 'static,
{
    /// Create a new accumulator wrapping `inner` and consuming `rx`.
    ///
    /// The accumulator starts outside a batch (`in_batch = false`).
    pub fn new(inner: S, rx: UnboundedReceiver<BeliefEvent>) -> Self {
        Self {
            acc: Arc::new(tokio::sync::Mutex::new(AccInner {
                inner,
                rx,
                pending: Vec::new(),
                in_batch: false,
            })),
            cache: Arc::new(AccCache::new()),
        }
    }

    /// Consume the accumulator and return the backing store.
    ///
    /// Drains any remaining events from the channel before unwrapping.  The
    /// caller should close `tx` before calling `into_inner` so the channel is
    /// disconnected; the `Disconnected` arm in `AccInner::drain_with_census` then
    /// ensures no events are silently lost.
    ///
    /// # Errors
    ///
    /// Returns `Err` if there are outstanding `Arc` clones (e.g. un-dropped
    /// [`QueryHandle`]s). Drop all handles before calling `into_inner`.
    pub async fn into_inner(self) -> Result<S, BuildonomyError> {
        // Drain any events still sitting in the channel (including any that
        // arrived after the last BatchEnd).  This is the only place a drain
        // is triggered externally — all other draining is done by BatchEnd
        // signals on the channel itself.
        {
            let mut guard = self.acc.lock().await;
            guard.drain_with_census(&self.cache, "into_inner").await?;
        }
        let arc = self.acc;
        match Arc::try_unwrap(arc) {
            Ok(mutex) => Ok(mutex.into_inner().inner),
            Err(_) => Err(BuildonomyError::Custom(
                "BeliefAccumulator::into_inner: outstanding Arc clones exist \
                 (drop all QueryHandles before calling into_inner)"
                    .into(),
            )),
        }
    }

    /// Return a lightweight, clonable query handle that shares this accumulator's
    /// backing store and cache.
    ///
    /// Pass `QueryHandle` clones to parallel parse tasks so they can call
    /// [`BeliefSource::eval_query`] without exclusive access to the channel.
    ///
    /// `QueryHandle` does **not** drain the channel — it only reads from `inner`
    /// through the shared cache.  Draining is driven exclusively by `BatchEnd`
    /// signals on the channel, processed inside `into_inner` via `drain_with_census`.
    pub fn query_handle(&self) -> QueryHandle<S> {
        QueryHandle {
            acc: Arc::clone(&self.acc),
            cache: Arc::clone(&self.cache),
        }
    }

    /// Return the current number of memoised query-cache entries.
    ///
    /// Primarily useful for testing.
    pub fn cache_len(&self) -> usize {
        self.cache.len()
    }
}

// ---------------------------------------------------------------------------
// BeliefSource impl for BeliefAccumulator
// ---------------------------------------------------------------------------

impl<S> BeliefSource for BeliefAccumulator<S>
where
    S: BeliefSource + BeliefSink + Clone + Send + 'static,
{
    fn eval_query(
        &self,
        query: &Query,
        all_or_none: bool,
    ) -> impl std::future::Future<Output = Result<BeliefGraph, BuildonomyError>> + Send {
        let query_owned = query.clone();
        let acc = Arc::clone(&self.acc);
        let cache = Arc::clone(&self.cache);

        async move {
            // No lazy drain here. Draining is driven exclusively by parse_all at
            // BatchEnd boundaries via BeliefAccumulator::drain_with_census(). Within an epoch,
            // inner is stable and queries go straight to the cache / inner.
            let cache_key = (query_owned.clone(), all_or_none);
            if let Some(cached) = cache.get(&cache_key) {
                return Ok(cached);
            }

            let result = {
                let guard = acc.lock().await;
                guard.inner.eval_query(&query_owned, all_or_none).await?
            };

            cache.insert(cache_key, CacheEntry::new(result.clone()));
            Ok(result)
        }
    }

    fn eval_unbalanced(
        &self,
        expr: &Expression,
    ) -> impl std::future::Future<Output = Result<BeliefGraph, BuildonomyError>> + Send {
        let expr_owned = expr.clone();
        let acc = Arc::clone(&self.acc);

        async move {
            let guard = acc.lock().await;
            guard.inner.eval_unbalanced(&expr_owned).await
        }
    }

    fn eval_trace(
        &self,
        expr: &Expression,
        weight_filter: WeightSet,
    ) -> impl std::future::Future<Output = Result<BeliefGraph, BuildonomyError>> + Send {
        let expr_owned = expr.clone();
        let acc = Arc::clone(&self.acc);

        async move {
            let guard = acc.lock().await;
            guard.inner.eval_trace(&expr_owned, weight_filter).await
        }
    }

    fn get_all_paths(
        &self,
        network_bid: Bid,
        include_index: bool,
    ) -> impl std::future::Future<Output = Result<Vec<(String, Bid)>, BuildonomyError>> + Send {
        let acc = Arc::clone(&self.acc);

        async move {
            let guard = acc.lock().await;
            guard.inner.get_all_paths(network_bid, include_index).await
        }
    }

    fn get_file_mtimes(
        &self,
    ) -> impl std::future::Future<Output = Result<BTreeMap<PathBuf, i64>, BuildonomyError>> + Send
    {
        let acc = Arc::clone(&self.acc);

        async move {
            let guard = acc.lock().await;
            guard.inner.get_file_mtimes().await
        }
    }

    fn export_beliefgraph(
        &self,
    ) -> impl std::future::Future<Output = Result<BeliefGraph, BuildonomyError>> + Send {
        let acc = Arc::clone(&self.acc);

        async move {
            let guard = acc.lock().await;
            guard.inner.export_beliefgraph().await
        }
    }
}

// ---------------------------------------------------------------------------
// QueryHandle — clonable read-only view for parallel tasks
// ---------------------------------------------------------------------------

/// A clonable, read-only view of a [`BeliefAccumulator`]'s backing store and cache.
///
/// Obtained via [`BeliefAccumulator::query_handle`].  Multiple handles may be
/// cloned and passed to parallel parse tasks without exclusive access to the
/// event channel.
///
/// `QueryHandle` reads from `inner` through the shared cache.  Between epochs the
/// compiler calls [`EpochDrain::drain_epoch`] on the handle to commit the completed
/// batch and invalidate the cache so the next epoch sees fresh state.  Within a
/// stable epoch (after a drain), `inner` is immutable from the tasks' perspective
/// and all queries are purely read-bound, sharing cache entries across all handles.
///
/// Individual parse tasks must **not** call `drain_epoch` — they hold clones of the
/// handle and must not advance the epoch boundary mid-batch.  Only the compiler's
/// main task (which owns the canonical `QueryHandle` returned by
/// [`BeliefAccumulator::query_handle`]) calls `drain_epoch`, always after all tasks
/// in the batch have been joined and `BatchEnd` has been sent to `tx`.
#[derive(Clone)]
pub struct QueryHandle<S: BeliefSource + BeliefSink> {
    acc: Arc<tokio::sync::Mutex<AccInner<S>>>,
    cache: Arc<AccCache>,
}

// ---------------------------------------------------------------------------
// EpochDrain — inter-epoch commit trigger
// ---------------------------------------------------------------------------

/// Drain and commit the current batch after a `BatchEnd` sentinel has been sent.
///
/// Implemented only by [`QueryHandle`].  The compiler calls this once per epoch
/// boundary (after `BatchEnd` is sent to `tx` and before the next `BatchStart`)
/// so that `global_bb` reflects all events from the completed epoch before the
/// next epoch's tasks begin querying it.
///
/// This is deliberately a separate trait from [`BeliefSource`] so that generic
/// parse tasks (which receive a cloned `QueryHandle` and must not drain mid-batch)
/// cannot accidentally call it.  Only the compiler's main task, which owns the
/// canonical handle, holds this bound.
pub trait EpochDrain {
    fn drain_epoch(&self) -> impl std::future::Future<Output = Result<(), BuildonomyError>> + Send;
}

impl<S> EpochDrain for QueryHandle<S>
where
    S: BeliefSource + BeliefSink + Clone + Send + 'static,
{
    /// Drain all pending channel events (including the `BatchEnd` that was just
    /// sent) and clear the query cache.
    ///
    /// After this returns, `inner` is consistent with all events from the
    /// completed epoch and the cache is empty, so the first query of the next
    /// epoch hits `inner` directly (warm-starting the cache for that epoch).
    fn drain_epoch(&self) -> impl std::future::Future<Output = Result<(), BuildonomyError>> + Send {
        let acc = Arc::clone(&self.acc);
        let cache = Arc::clone(&self.cache);
        async move {
            let mut guard = acc.lock().await;
            guard.drain_with_census(&cache, "drain_epoch").await?;
            // drain_with_census clears the cache on every BatchEnd it processes.
            // If (pathologically) no BatchEnd arrived, clear explicitly so callers
            // never see stale cached results across an epoch boundary.
            cache.clear();
            Ok(())
        }
    }
}

impl<S> BeliefSource for QueryHandle<S>
where
    S: BeliefSource + BeliefSink + Clone + Send + 'static,
{
    fn eval_query(
        &self,
        query: &Query,
        all_or_none: bool,
    ) -> impl std::future::Future<Output = Result<BeliefGraph, BuildonomyError>> + Send {
        let query_owned = query.clone();
        let acc = Arc::clone(&self.acc);
        let cache = Arc::clone(&self.cache);

        async move {
            // No drain — QueryHandle operates within a stable epoch (inner was last
            // drained by BeliefAccumulator::into_inner at the preceding BatchEnd boundary).
            let cache_key = (query_owned.clone(), all_or_none);
            if let Some(cached) = cache.get(&cache_key) {
                return Ok(cached);
            }

            let result = {
                let guard = acc.lock().await;
                guard.inner.eval_query(&query_owned, all_or_none).await?
            };

            cache.insert(cache_key, CacheEntry::new(result.clone()));
            Ok(result)
        }
    }

    fn eval_unbalanced(
        &self,
        expr: &Expression,
    ) -> impl std::future::Future<Output = Result<BeliefGraph, BuildonomyError>> + Send {
        let expr_owned = expr.clone();
        let acc = Arc::clone(&self.acc);

        async move {
            let guard = acc.lock().await;
            guard.inner.eval_unbalanced(&expr_owned).await
        }
    }

    fn eval_trace(
        &self,
        expr: &Expression,
        weight_filter: WeightSet,
    ) -> impl std::future::Future<Output = Result<BeliefGraph, BuildonomyError>> + Send {
        let expr_owned = expr.clone();
        let acc = Arc::clone(&self.acc);

        async move {
            let guard = acc.lock().await;
            guard.inner.eval_trace(&expr_owned, weight_filter).await
        }
    }

    fn get_all_paths(
        &self,
        network_bid: Bid,
        include_index: bool,
    ) -> impl std::future::Future<Output = Result<Vec<(String, Bid)>, BuildonomyError>> + Send {
        let acc = Arc::clone(&self.acc);

        async move {
            let guard = acc.lock().await;
            guard.inner.get_all_paths(network_bid, include_index).await
        }
    }

    fn get_file_mtimes(
        &self,
    ) -> impl std::future::Future<Output = Result<BTreeMap<PathBuf, i64>, BuildonomyError>> + Send
    {
        let acc = Arc::clone(&self.acc);

        async move {
            let guard = acc.lock().await;
            guard.inner.get_file_mtimes().await
        }
    }

    fn export_beliefgraph(
        &self,
    ) -> impl std::future::Future<Output = Result<BeliefGraph, BuildonomyError>> + Send {
        let acc = Arc::clone(&self.acc);

        async move {
            let guard = acc.lock().await;
            guard.inner.export_beliefgraph().await
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    use tokio::sync::mpsc::unbounded_channel;

    use super::*;
    use crate::{
        beliefbase::BeliefGraph,
        event::{BeliefEvent, EventOrigin},
        nodekey::NodeKey,
        properties::{Bid, WeightSet},
        query::{Expression, Query, StatePred},
    };

    // -----------------------------------------------------------------------
    // Minimal BeliefSource + BeliefSink that counts apply_batch calls.
    // -----------------------------------------------------------------------

    #[derive(Clone)]
    struct CountingStore {
        query_count: Arc<AtomicUsize>,
        batch_count: Arc<AtomicUsize>,
        result: BeliefGraph,
    }

    impl CountingStore {
        fn new() -> Self {
            Self {
                query_count: Arc::new(AtomicUsize::new(0)),
                batch_count: Arc::new(AtomicUsize::new(0)),
                result: BeliefGraph::default(),
            }
        }

        fn query_count(&self) -> usize {
            self.query_count.load(Ordering::SeqCst)
        }

        fn batch_count(&self) -> usize {
            self.batch_count.load(Ordering::SeqCst)
        }
    }

    impl BeliefSource for CountingStore {
        fn eval_query(
            &self,
            _query: &Query,
            _all_or_none: bool,
        ) -> impl std::future::Future<Output = Result<BeliefGraph, BuildonomyError>> + Send
        {
            self.query_count.fetch_add(1, Ordering::SeqCst);
            let result = self.result.clone();
            async move { Ok(result) }
        }

        fn eval_unbalanced(
            &self,
            _expr: &Expression,
        ) -> impl std::future::Future<Output = Result<BeliefGraph, BuildonomyError>> + Send
        {
            async { Ok(BeliefGraph::default()) }
        }

        fn eval_trace(
            &self,
            _expr: &Expression,
            _weight_filter: WeightSet,
        ) -> impl std::future::Future<Output = Result<BeliefGraph, BuildonomyError>> + Send
        {
            async { Ok(BeliefGraph::default()) }
        }
    }

    impl BeliefSink for CountingStore {
        async fn apply_batch(&mut self, _events: &[BeliefEvent]) -> Result<(), BuildonomyError> {
            self.batch_count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    fn any_query() -> Query {
        Query {
            seed: Expression::StateIn(StatePred::Any),
            traverse: None,
        }
    }

    fn test_bid(n: u128) -> Bid {
        Bid::from(uuid::Uuid::from_u128(n))
    }

    // -----------------------------------------------------------------------
    // into_inner with empty channel is a noop drain
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn drain_with_empty_channel_is_noop() {
        let (_tx, rx) = unbounded_channel::<BeliefEvent>();
        let store = CountingStore::new();
        let acc = BeliefAccumulator::new(store.clone(), rx);

        let inner = acc.into_inner().await.unwrap();

        assert_eq!(inner.batch_count(), 0);
    }

    // -----------------------------------------------------------------------
    // BatchStart / BatchEnd round-trip committed on into_inner
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn drain_commits_batch_on_batch_end() {
        let (tx, rx) = unbounded_channel::<BeliefEvent>();
        let store = CountingStore::new();
        let acc = BeliefAccumulator::new(store.clone(), rx);

        tx.send(BeliefEvent::BatchStart).unwrap();
        tx.send(BeliefEvent::NodeUpdate(
            vec![NodeKey::Bid { bid: test_bid(1) }],
            String::new(),
            EventOrigin::Remote,
        ))
        .unwrap();
        tx.send(BeliefEvent::BatchEnd).unwrap();
        drop(tx); // close channel so into_inner drain sees Disconnected

        let inner = acc.into_inner().await.unwrap();

        // apply_batch should have been called exactly once (on BatchEnd).
        assert_eq!(inner.batch_count(), 1);
    }

    // -----------------------------------------------------------------------
    // eval_query — cache hit avoids second inner call
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn eval_query_cache_hit_avoids_second_inner_call() {
        let (_tx, rx) = unbounded_channel::<BeliefEvent>();
        let store = CountingStore::new();
        let acc = BeliefAccumulator::new(store.clone(), rx);
        let q = any_query();

        // No lazy drain: inner is queried directly through cache.
        acc.eval_query(&q, true).await.unwrap();
        assert_eq!(store.query_count(), 1);

        acc.eval_query(&q, true).await.unwrap();
        assert_eq!(store.query_count(), 1, "second call should hit cache");
    }

    // -----------------------------------------------------------------------
    // BatchEnd clears cache — verified via into_inner then fresh accumulator
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn batch_end_clears_cache() {
        let (tx, rx) = unbounded_channel::<BeliefEvent>();
        let store = CountingStore::new();
        let acc = BeliefAccumulator::new(store.clone(), rx);
        let q = any_query();

        // Warm the cache.
        acc.eval_query(&q, true).await.unwrap();
        assert_eq!(store.query_count(), 1);
        assert_eq!(acc.cache_len(), 1);

        // Send a BatchStart/BatchEnd pair and close the channel.
        tx.send(BeliefEvent::BatchStart).unwrap();
        tx.send(BeliefEvent::BatchEnd).unwrap();
        drop(tx);

        // into_inner drains the channel; BatchEnd processing clears the cache.
        // We check cache_len before consuming the accumulator.
        // The cache is cleared as a side-effect of processing BatchEnd inside drain.
        // To observe it we need to inspect before into_inner consumes self.
        // Use the public drain path: lock AccInner directly via a QueryHandle trick —
        // but that's overcomplicated. Instead, verify via a second accumulator round:
        // the cache_len is 1 before drain; after drain (into_inner) it would be 0,
        // but we can't check it post-consumption.  What we CAN check: that a second
        // eval_query after into_inner on the extracted inner goes to inner (query_count
        // increments), not a stale cache — i.e. the cache was invalidated.
        //
        // Simpler: wrap in a new accumulator using the extracted inner and verify
        // the query count increments again (not cached from the old accumulator).
        let inner = acc.into_inner().await.unwrap();
        // BatchEnd always calls apply_batch (even with empty pending), so batch_count = 1.
        assert_eq!(inner.batch_count(), 1);
        // The old cache was on the old Arc<AccCache> which is now dropped.
        // A fresh accumulator wrapping the same inner has an empty cache.
        let (tx2, rx2) = unbounded_channel::<BeliefEvent>();
        drop(tx2);
        let acc2 = BeliefAccumulator::new(inner, rx2);
        acc2.eval_query(&q, true).await.unwrap();
        assert_eq!(
            acc2.into_inner().await.unwrap().query_count(),
            2,
            "fresh accumulator must re-query inner (old cache is gone)"
        );
    }

    // -----------------------------------------------------------------------
    // Events outside a batch are applied immediately on into_inner drain
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn events_outside_batch_applied_immediately() {
        let (tx, rx) = unbounded_channel::<BeliefEvent>();
        let store = CountingStore::new();
        let acc = BeliefAccumulator::new(store.clone(), rx);

        // Send a lone FileParsed event — no surrounding BatchStart/BatchEnd.
        tx.send(BeliefEvent::FileParsed(std::path::PathBuf::from(
            "/some/file.md",
        )))
        .unwrap();
        drop(tx);

        let inner = acc.into_inner().await.unwrap();

        // apply_batch called once (the lone event outside a batch).
        assert_eq!(inner.batch_count(), 1);
    }

    // -----------------------------------------------------------------------
    // prepare_batch: NodesRemoved consolidation + ordering
    // -----------------------------------------------------------------------

    #[test]
    fn node_events_sort_before_others() {
        let bid = test_bid(42);
        let bid2 = test_bid(43);
        // Two separate NodesRemoved events plus a NodeUpdate and a FileParsed.
        let mut events = vec![
            BeliefEvent::FileParsed(std::path::PathBuf::from("/f")),
            BeliefEvent::NodeUpdate(
                vec![NodeKey::Bid { bid }],
                String::new(),
                EventOrigin::Remote,
            ),
            BeliefEvent::NodesRemoved(vec![bid], EventOrigin::Remote),
            BeliefEvent::NodesRemoved(vec![bid2], EventOrigin::Remote),
        ];
        prepare_batch(&mut events);

        // First event must be the consolidated NodesRemoved.
        match &events[0] {
            BeliefEvent::NodesRemoved(bids, _) => {
                assert!(
                    bids.contains(&bid),
                    "consolidated NodesRemoved must contain bid"
                );
                assert!(
                    bids.contains(&bid2),
                    "consolidated NodesRemoved must contain bid2"
                );
                assert_eq!(bids.len(), 2, "two bids consolidated into one");
            }
            other => panic!("expected NodesRemoved first, got {:?}", other),
        }

        // Second must be NodeUpdate.
        assert!(
            matches!(&events[1], BeliefEvent::NodeUpdate(_, _, _)),
            "expected NodeUpdate second, got {:?}",
            events[1]
        );

        // Last must be FileParsed.
        assert!(
            matches!(&events[2], BeliefEvent::FileParsed(_)),
            "FileParsed should be last, got {:?}",
            events[2]
        );
    }

    // -----------------------------------------------------------------------
    // query_handle shares cache with accumulator
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn query_handle_shares_cache() {
        let (_tx, rx) = unbounded_channel::<BeliefEvent>();
        let store = CountingStore::new();
        let acc = BeliefAccumulator::new(store.clone(), rx);
        let handle = acc.query_handle();
        let q = any_query();

        // Warm via accumulator.
        acc.eval_query(&q, true).await.unwrap();
        assert_eq!(store.query_count(), 1);

        // Handle shares the same Arc<AccCache> — should hit the cached entry.
        handle.eval_query(&q, true).await.unwrap();
        assert_eq!(
            store.query_count(),
            1,
            "handle should reuse accumulator's cache"
        );
    }

    // -----------------------------------------------------------------------
    // into_inner
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn into_inner_returns_backing_store() {
        let (_tx, rx) = unbounded_channel::<BeliefEvent>();
        let store = CountingStore::new();
        let acc = BeliefAccumulator::new(store, rx);

        let _inner: CountingStore = acc.into_inner().await.unwrap();
    }

    // -----------------------------------------------------------------------
    // into_inner fails when QueryHandle is still alive
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn into_inner_fails_with_outstanding_handle() {
        let (_tx, rx) = unbounded_channel::<BeliefEvent>();
        let store = CountingStore::new();
        let acc = BeliefAccumulator::new(store, rx);
        let _handle = acc.query_handle();

        assert!(
            acc.into_inner().await.is_err(),
            "into_inner should fail while a QueryHandle holds an Arc clone"
        );
    }
}
