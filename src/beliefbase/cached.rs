//! [`CachedBeliefSource`] — a shared, event-invalidated memoising wrapper around any
//! [`BeliefSource`].
//!
//! ## Motivation
//!
//! [`crate::query::BeliefSource::eval_query`] calls `balance()` internally, which walks
//! Section edges across the backing store and triggers `index_sync` on
//! [`crate::beliefbase::BeliefBase`]-backed sources.  When `global_bb` receives
//! `index_dirty = true` after every `terminate_stack`, each per-node `cache_fetch` call
//! in Phase 1 re-runs `index_sync` + `balance` over the entire accumulated graph —
//! producing O(N²) total cost across a corpus run.
//!
//! `CachedBeliefSource` eliminates this by memoising `eval_query` results.  The first
//! call for a given `(Query, all_or_none)` key pays the full cost; every subsequent call
//! within the **same epoch** returns a cheap clone.
//!
//! ## Sharing across files (cross-file speedup)
//!
//! The primary win comes from sharing the cache across all files in an epoch.  All
//! sibling files in a network (e.g. all ~50 `Array` method pages) query their parent
//! network with the **same** `parent_query`.  Without caching, each file independently
//! runs `eval_query` → `index_sync` + `balance` on the full `global_bb`.  With a shared
//! `CachedBeliefSource`, the first sibling pays the cost once and every subsequent
//! sibling gets a clone in O(1).
//!
//! One `CachedBeliefSource` is created per epoch and passed by clone (cheap — the inner
//! cache is `Arc`-wrapped) to every `parse_next` / `parse_epoch_parallel` task.
//!
//! ## Invalidation
//!
//! After each `terminate_stack`, new `BeliefEvent`s arrive in `global_bb` (NodeUpdate,
//! RelationUpdate, etc.).  Call [`CachedBeliefSource::invalidate_for_events`] with the
//! event slice to evict any cache entries whose results are now stale.
//!
//! ### What can go stale?
//!
//! A cached `eval_query` result for a query seeded on network node `N` is stale when:
//!
//! - A `NodeUpdate` arrives for a BID that is **in** the cached result (the node's state
//!   changed — title, kind, etc.).
//! - A `RelationUpdate` or `RelationRemoved` arrives where **either** `source` or `sink`
//!   is a BID that is in the cached result (the graph topology around `N` changed).
//!
//! In practice, within a single epoch the parent-network node itself never changes
//! (no file re-parses its own parent mid-epoch), so the dominant cached result — the
//! parent-network ancestor chain queried by `try_initialize_stack_from_session_cache` —
//! is stable for the entire sibling batch.  The `ancestors_only` filter further ensures
//! that even if new child-doc Section edges arrive (added as siblings are parsed), the
//! filtered output is unchanged: child-doc edges are stripped before use.
//!
//! ### Eviction strategy
//!
//! For each cached `(Query, bool) → BeliefGraph` entry, maintain the set of BIDs present
//! in the result graph.  On invalidation, evict every entry whose BID set intersects the
//! set of BIDs affected by the incoming events.  This is O(cache_entries × affected_bids)
//! — negligible for typical cache sizes (tens of entries, each covering a small ancestor
//! chain).
//!
//! [`CachedBeliefSource::invalidate`] provides a coarse full-clear escape hatch.
//!
//! ## `Clone` contract
//!
//! `clone()` produces a wrapper that **shares** the same inner cache (via `Arc`).  This
//! is intentional: clones handed to parallel tasks within the same epoch all benefit from
//! and contribute to the shared cache.
//!
//! ## `Send`
//!
//! `CachedBeliefSource` is native-only (`#[cfg(not(target_arch = "wasm32"))]`).
//! `BeliefSource for BeliefBase` is likewise native-only, so there is no WASM use-case
//! for this wrapper.  The inner cache uses `Mutex` so that `CachedBeliefSource` is
//! `Send + Sync`.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use crate::{
    beliefbase::BeliefGraph,
    event::BeliefEvent,
    properties::{Bid, WeightSet},
    query::{BeliefSource, Expression, Query},
    BuildonomyError,
};

// ---------------------------------------------------------------------------
// Inner cache entry
// ---------------------------------------------------------------------------

/// A single memoised `eval_query` result together with the set of BIDs present in the
/// result graph.  The BID set is used for selective invalidation.
#[derive(Clone)]
struct CacheEntry {
    result: BeliefGraph,
    /// All BIDs present in `result.states` — used to check whether an incoming event
    /// affects this entry.
    bids: HashSet<Bid>,
}

impl CacheEntry {
    fn new(result: BeliefGraph) -> Self {
        let bids: HashSet<Bid> = result.states.keys().copied().collect();
        Self { result, bids }
    }
}

// ---------------------------------------------------------------------------
// Shared inner state (Arc-wrapped so clones share the same cache)
// ---------------------------------------------------------------------------

type CacheMap = Mutex<HashMap<(Query, bool), CacheEntry>>;

struct Inner {
    cache: CacheMap,
}

impl Inner {
    fn new() -> Self {
        Self {
            cache: Mutex::new(HashMap::new()),
        }
    }

    fn get(&self, key: &(Query, bool)) -> Option<BeliefGraph> {
        self.cache
            .lock()
            .ok()
            .and_then(|g| g.get(key).map(|e| e.result.clone()))
    }

    fn insert(&self, key: (Query, bool), entry: CacheEntry) {
        if let Ok(mut g) = self.cache.lock() {
            g.insert(key, entry);
        }
    }

    fn clear(&self) {
        if let Ok(mut g) = self.cache.lock() {
            g.clear();
        }
    }

    fn len(&self) -> usize {
        self.cache.lock().map(|g| g.len()).unwrap_or(0)
    }

    /// Evict all entries whose BID set intersects `affected_bids`.
    fn evict_affected(&self, affected_bids: &HashSet<Bid>) {
        if affected_bids.is_empty() {
            return;
        }
        if let Ok(mut g) = self.cache.lock() {
            g.retain(|_key, entry| entry.bids.is_disjoint(affected_bids));
        }
    }
}

// ---------------------------------------------------------------------------
// CachedBeliefSource
// ---------------------------------------------------------------------------

/// A shared, event-invalidated memoising wrapper around any [`BeliefSource`].
///
/// See the [module documentation](self) for motivation, sharing semantics, and
/// invalidation strategy.
pub struct CachedBeliefSource<B: BeliefSource> {
    inner_source: B,
    cache: Arc<Inner>,
}

impl<B: BeliefSource + Clone> Clone for CachedBeliefSource<B> {
    /// Clones the wrapper, **sharing** the same inner cache.
    ///
    /// All clones within the same epoch contribute to and benefit from the same
    /// memoised results.
    fn clone(&self) -> Self {
        Self {
            inner_source: self.inner_source.clone(),
            cache: Arc::clone(&self.cache),
        }
    }
}

impl<B: BeliefSource> CachedBeliefSource<B> {
    /// Wrap `source` in a fresh, empty cache.
    pub fn new(source: B) -> Self {
        Self {
            inner_source: source,
            cache: Arc::new(Inner::new()),
        }
    }

    /// Discard **all** cached results unconditionally.
    ///
    /// Use this when the backing source has changed in a way that cannot be described
    /// by a `BeliefEvent` slice (e.g. a full epoch boundary reset).
    pub fn invalidate(&self) {
        self.cache.clear();
    }

    /// Selectively evict cache entries that are stale given `events`.
    ///
    /// For each event, extract the set of affected BIDs:
    ///
    /// - `NodeUpdate(keys, …)` → the BID resolved from `keys` if it is a `NodeKey::Bid`;
    ///   otherwise the BID is unknown at this point so we conservatively skip
    ///   (the node wasn't in any result yet if its BID is unknown).
    /// - `NodesRemoved(bids, …)` → all bids.
    /// - `RelationUpdate(source, sink, …)` / `RelationChange(source, sink, …)` /
    ///   `RelationRemoved(source, sink, …)` → both `source` and `sink`.
    /// - All other events → no cache effect.
    ///
    /// Any cache entry whose BID set intersects the affected BIDs is evicted.
    pub fn invalidate_for_events(&self, events: &[BeliefEvent]) {
        let mut affected: HashSet<Bid> = HashSet::new();
        for event in events {
            match event {
                BeliefEvent::NodeUpdate(keys, _, _) => {
                    for key in keys {
                        if let crate::nodekey::NodeKey::Bid { bid } = key {
                            affected.insert(*bid);
                        }
                    }
                }
                BeliefEvent::NodesRemoved(bids, _) => {
                    affected.extend(bids.iter().copied());
                }
                BeliefEvent::RelationUpdate(source, sink, _, _)
                | BeliefEvent::RelationRemoved(source, sink, _) => {
                    affected.insert(*source);
                    affected.insert(*sink);
                }
                BeliefEvent::RelationChange(source, sink, _, _, _) => {
                    affected.insert(*source);
                    affected.insert(*sink);
                }
                // NodeRenamed, PathAdded/Update/Removed, FileParsed, BatchEnd,
                // BuiltInTest: no direct cache effect on query results.
                _ => {}
            }
        }
        self.cache.evict_affected(&affected);
    }

    /// Return the number of entries currently held in the cache.
    pub fn cache_len(&self) -> usize {
        self.cache.len()
    }
}

// ---------------------------------------------------------------------------
// BeliefSource implementation
// ---------------------------------------------------------------------------

impl<B: BeliefSource + Clone + Send> BeliefSource for CachedBeliefSource<B> {
    // ------------------------------------------------------------------
    // Cached method: eval_query
    //
    // This is the only method that needs caching.  `eval_unbalanced` and
    // `eval_trace` are called from within `balance()` which is called from
    // `eval_query` — caching `eval_query` subsumes them.
    // ------------------------------------------------------------------

    fn eval_query(
        &self,
        query: &Query,
        all_or_none: bool,
    ) -> impl std::future::Future<Output = Result<BeliefGraph, BuildonomyError>> + Send {
        let query_owned = query.clone();
        // Clone Arc<Inner> and inner_source out of &self *before* the async block so
        // the future does not capture `&self` (which would require CachedBeliefSource: Sync,
        // broken on WASM where Inner uses RefCell).
        let cache = Arc::clone(&self.cache);
        let inner_source = self.inner_source.clone();

        async move {
            let cache_key = (query_owned.clone(), all_or_none);

            if let Some(cached) = cache.get(&cache_key) {
                return Ok(cached);
            }

            let result = inner_source.eval_query(&query_owned, all_or_none).await?;
            cache.insert(cache_key, CacheEntry::new(result.clone()));
            Ok(result)
        }
    }

    // ------------------------------------------------------------------
    // Pass-through methods
    // ------------------------------------------------------------------

    fn eval_unbalanced(
        &self,
        expr: &Expression,
    ) -> impl std::future::Future<Output = Result<BeliefGraph, BuildonomyError>> + Send {
        self.inner_source.eval_unbalanced(expr)
    }

    fn eval_trace(
        &self,
        expr: &Expression,
        weight_filter: WeightSet,
    ) -> impl std::future::Future<Output = Result<BeliefGraph, BuildonomyError>> + Send {
        self.inner_source.eval_trace(expr, weight_filter)
    }

    fn get_all_paths(
        &self,
        network_bid: Bid,
        include_index: bool,
    ) -> impl std::future::Future<Output = Result<Vec<(String, Bid)>, BuildonomyError>> + Send {
        self.inner_source.get_all_paths(network_bid, include_index)
    }

    fn get_file_mtimes(
        &self,
    ) -> impl std::future::Future<
        Output = Result<std::collections::BTreeMap<std::path::PathBuf, i64>, BuildonomyError>,
    > + Send {
        self.inner_source.get_file_mtimes()
    }

    fn export_beliefgraph(
        &self,
    ) -> impl std::future::Future<Output = Result<BeliefGraph, BuildonomyError>> + Send {
        self.inner_source.export_beliefgraph()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use super::*;
    use crate::beliefbase::BeliefGraph;
    use crate::event::{BeliefEvent, EventOrigin};
    use crate::nodekey::NodeKey;
    use crate::properties::{Bid, WeightSet};
    use crate::query::{Expression, Query, StatePred};

    /// Return a stable non-nil BID derived from a u128 literal.
    fn test_bid(n: u128) -> Bid {
        Bid::from(uuid::Uuid::from_u128(n))
    }

    // -----------------------------------------------------------------------
    // Minimal BeliefSource that counts eval_query calls and returns an
    // optionally pre-loaded graph.
    // -----------------------------------------------------------------------
    #[derive(Clone)]
    struct CountingSource {
        call_count: Arc<AtomicUsize>,
        result: BeliefGraph,
    }

    impl CountingSource {
        fn new() -> Self {
            Self {
                call_count: Arc::new(AtomicUsize::new(0)),
                result: BeliefGraph::default(),
            }
        }

        fn with_result(result: BeliefGraph) -> Self {
            Self {
                call_count: Arc::new(AtomicUsize::new(0)),
                result,
            }
        }

        fn count(&self) -> usize {
            self.call_count.load(Ordering::SeqCst)
        }
    }

    impl BeliefSource for CountingSource {
        async fn eval_unbalanced(
            &self,
            _expr: &Expression,
        ) -> Result<BeliefGraph, BuildonomyError> {
            Ok(BeliefGraph::default())
        }

        async fn eval_trace(
            &self,
            _expr: &Expression,
            _weight_filter: WeightSet,
        ) -> Result<BeliefGraph, BuildonomyError> {
            Ok(BeliefGraph::default())
        }

        fn eval_query(
            &self,
            _query: &Query,
            _all_or_none: bool,
        ) -> impl std::future::Future<Output = Result<BeliefGraph, BuildonomyError>> + Send
        {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            let result = self.result.clone();
            async move { Ok(result) }
        }
    }

    fn any_query() -> Query {
        Query {
            seed: Expression::StateIn(StatePred::Any),
            traverse: None,
        }
    }

    // -----------------------------------------------------------------------
    // Basic memoisation
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn cache_hit_avoids_second_inner_call() {
        let inner = CountingSource::new();
        let cached = CachedBeliefSource::new(inner.clone());
        let q = any_query();

        cached.eval_query(&q, true).await.unwrap();
        assert_eq!(inner.count(), 1);

        cached.eval_query(&q, true).await.unwrap();
        assert_eq!(inner.count(), 1, "inner should not be called on cache hit");
    }

    #[tokio::test]
    async fn different_all_or_none_produces_separate_cache_entries() {
        let inner = CountingSource::new();
        let cached = CachedBeliefSource::new(inner.clone());
        let q = any_query();

        cached.eval_query(&q, true).await.unwrap();
        cached.eval_query(&q, false).await.unwrap();
        assert_eq!(inner.count(), 2);

        cached.eval_query(&q, true).await.unwrap();
        cached.eval_query(&q, false).await.unwrap();
        assert_eq!(inner.count(), 2, "should not call inner again");
    }

    // -----------------------------------------------------------------------
    // Coarse invalidation
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn invalidate_clears_cache() {
        let inner = CountingSource::new();
        let cached = CachedBeliefSource::new(inner.clone());
        let q = any_query();

        cached.eval_query(&q, true).await.unwrap();
        assert_eq!(inner.count(), 1);

        cached.invalidate();
        cached.eval_query(&q, true).await.unwrap();
        assert_eq!(inner.count(), 2);
    }

    // -----------------------------------------------------------------------
    // Clone shares cache
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn clone_shares_cache() {
        let inner = CountingSource::new();
        let cached = CachedBeliefSource::new(inner.clone());
        let q = any_query();

        // Warm the cache on the original.
        cached.eval_query(&q, true).await.unwrap();
        assert_eq!(inner.count(), 1);

        // Clone shares the cache — should NOT call inner again.
        let cloned = cached.clone();
        cloned.eval_query(&q, true).await.unwrap();
        assert_eq!(inner.count(), 1, "clone must share the cache");
    }

    #[tokio::test]
    async fn clone_invalidation_is_shared() {
        let inner = CountingSource::new();
        let cached = CachedBeliefSource::new(inner.clone());
        let q = any_query();

        cached.eval_query(&q, true).await.unwrap();
        let cloned = cached.clone();

        // Invalidating via clone also clears the original's view.
        cloned.invalidate();
        cached.eval_query(&q, true).await.unwrap();
        assert_eq!(
            inner.count(),
            2,
            "invalidation via clone should evict shared cache"
        );
    }

    // -----------------------------------------------------------------------
    // Selective invalidation via events
    // -----------------------------------------------------------------------

    /// Build a minimal BeliefGraph containing a single node with the given BID.
    fn graph_with_bid(bid: Bid) -> BeliefGraph {
        use crate::properties::BeliefNode;
        use std::collections::BTreeMap;
        let mut node = BeliefNode::default();
        node.bid = bid;
        BeliefGraph {
            states: BTreeMap::from([(bid, node)]),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn invalidate_for_events_evicts_on_node_update() {
        let bid = test_bid(1);
        let inner = CountingSource::with_result(graph_with_bid(bid));
        let cached = CachedBeliefSource::new(inner.clone());
        let q = any_query();

        cached.eval_query(&q, true).await.unwrap();
        assert_eq!(inner.count(), 1);
        assert_eq!(cached.cache_len(), 1);

        // NodeUpdate for the BID that is in the cached result → should evict.
        let event = BeliefEvent::NodeUpdate(
            vec![NodeKey::Bid { bid }],
            String::new(),
            EventOrigin::Remote,
        );
        cached.invalidate_for_events(&[event]);
        assert_eq!(
            cached.cache_len(),
            0,
            "entry should be evicted on matching NodeUpdate"
        );

        cached.eval_query(&q, true).await.unwrap();
        assert_eq!(inner.count(), 2, "should re-fetch after eviction");
    }

    #[tokio::test]
    async fn invalidate_for_events_evicts_on_relation_update() {
        let source = test_bid(2);
        let sink = test_bid(3);
        // Result contains `source`; event mentions `source` in a RelationUpdate.
        let inner = CountingSource::with_result(graph_with_bid(source));
        let cached = CachedBeliefSource::new(inner.clone());
        let q = any_query();

        cached.eval_query(&q, true).await.unwrap();
        assert_eq!(inner.count(), 1);

        let event =
            BeliefEvent::RelationUpdate(source, sink, WeightSet::default(), EventOrigin::Remote);
        cached.invalidate_for_events(&[event]);
        assert_eq!(
            cached.cache_len(),
            0,
            "entry should be evicted on RelationUpdate touching a cached BID"
        );
    }

    #[tokio::test]
    async fn invalidate_for_events_keeps_unaffected_entries() {
        let cached_bid = test_bid(4);
        let other_bid = test_bid(5);
        let inner = CountingSource::with_result(graph_with_bid(cached_bid));
        let cached = CachedBeliefSource::new(inner.clone());
        let q = any_query();

        cached.eval_query(&q, true).await.unwrap();
        assert_eq!(cached.cache_len(), 1);

        // Event touches `other_bid` only — result contains only `cached_bid` → no eviction.
        let unrelated_source = other_bid;
        let unrelated_sink = test_bid(6);
        let event = BeliefEvent::RelationUpdate(
            unrelated_source,
            unrelated_sink,
            WeightSet::default(),
            EventOrigin::Remote,
        );
        cached.invalidate_for_events(&[event]);
        assert_eq!(
            cached.cache_len(),
            1,
            "unrelated event must not evict unaffected entries"
        );
    }

    #[tokio::test]
    async fn invalidate_for_events_evicts_on_nodes_removed() {
        let bid = test_bid(7);
        let inner = CountingSource::with_result(graph_with_bid(bid));
        let cached = CachedBeliefSource::new(inner.clone());
        let q = any_query();

        cached.eval_query(&q, true).await.unwrap();
        assert_eq!(cached.cache_len(), 1);

        let event = BeliefEvent::NodesRemoved(vec![bid], EventOrigin::Remote);
        cached.invalidate_for_events(&[event]);
        assert_eq!(cached.cache_len(), 0);
    }

    // -----------------------------------------------------------------------
    // cache_len
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn cache_len_reflects_stored_entries() {
        let inner = CountingSource::new();
        let cached = CachedBeliefSource::new(inner);
        let q = any_query();

        assert_eq!(cached.cache_len(), 0);
        cached.eval_query(&q, true).await.unwrap();
        assert_eq!(cached.cache_len(), 1);
        cached.eval_query(&q, false).await.unwrap();
        assert_eq!(cached.cache_len(), 2);
        cached.invalidate();
        assert_eq!(cached.cache_len(), 0);
    }
}
