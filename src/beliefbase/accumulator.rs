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
//! 2. **Query caching**: memoising [`BeliefSource::evaluate`] results so that O(N²)
//!    `index_sync` + traversal chains across a sibling batch collapse to O(N).
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
//! `Arc<tokio::sync::RwLock<AccInner<S>>>`.  The `Arc` clone is cheap and can be
//! moved into `async move` futures without capturing `&self`, which keeps the
//! return types of the trait methods `Send + 'static`-friendly.
//!
//! `BeliefSource` methods (`evaluate`, `submap`, `submap_by_bid`,
//! `get_file_mtimes`, `export_beliefgraph`) take a **shared** (`read`) guard —
//! they delegate to `inner`, which for `DbConnection` is read-only SQL against
//! a pool that supports concurrent readers (see noet-core Issue 100). Batch
//! application (`AccInner::handle_event` / `drain_with_census`, which is where
//! `apply_batch` is actually invoked) takes an **exclusive** (`write`) guard,
//! since it mutates `pending`/`in_batch` and must not interleave with any
//! concurrent read of `inner`'s state mid-write.
//!
//! The query cache uses its own `Arc<AccCache>` (backed by `std::sync::Mutex`) so
//! cache hits do not need to await the `tokio::sync::RwLock`.
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

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc::UnboundedReceiver;

use crate::{
    beliefbase::BeliefGraph,
    event::{BeliefEvent, EventOrigin},
    nodekey::NodeKey,
    properties::{Bid, WeightSet},
    query::{spec::QueryPackage, BeliefSource, BoxFuture, SubmapResult},
    BuildonomyError,
};

use super::BeliefSink;

// ---------------------------------------------------------------------------
// Query-result cache
// ---------------------------------------------------------------------------

/// A single memoised evaluate result.
#[derive(Clone)]
struct CacheEntry {
    package: QueryPackage,
}

impl CacheEntry {
    fn new(package: QueryPackage) -> Self {
        Self { package }
    }
}

/// Cache key: serialized `QuerySpec` JSON string.
///
/// We use `serde_json::to_string` rather than `Hash`/`Eq` on `QuerySpec`
/// because `QuerySpec` contains types that don't implement `Hash` (`f32`
/// in `SortSpec`, `toml::Value` in filter predicates). The JSON
/// serialization is deterministic for identical structs and avoids the
/// need for manual `Hash`/`Eq` impls across the entire type tree.
type CacheKey = String;

/// Shared, `Send + Sync` query-result cache.
///
/// Wrapped in `Arc` so that [`QueryHandle`] clones share entries with the
/// accumulator itself.
pub(super) struct AccCache {
    /// `evaluate` cache (keyed on serialized `QuerySpec` JSON).
    entries: Mutex<HashMap<CacheKey, CacheEntry>>,
}

impl AccCache {
    fn new() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
        }
    }

    fn get(&self, key: &str) -> Option<QueryPackage> {
        self.entries
            .lock()
            .ok()
            .and_then(|g| g.get(key).map(|e| e.package.clone()))
    }

    fn insert(&self, key: CacheKey, entry: CacheEntry) {
        if let Ok(mut g) = self.entries.lock() {
            g.insert(key, entry);
        }
    }

    fn clear(&self) {
        if let Ok(mut g) = self.entries.lock() {
            g.clear();
        }
    }

    fn len(&self) -> usize {
        self.entries.lock().map(|g| g.len()).unwrap_or(0)
    }

    /// Evict all entries whose result state set intersects `affected_bids`.
    ///
    /// Not yet called — reserved for future selective invalidation.
    #[allow(dead_code)]
    fn evict_affected(&self, affected_bids: &[Bid]) {
        if affected_bids.is_empty() {
            return;
        }
        if let Ok(mut g) = self.entries.lock() {
            g.retain(|_key, entry| {
                // Check if the cached package's graph intersects
                // with the affected BIDs.
                if let Some(graph) = entry.package.graph() {
                    !affected_bids
                        .iter()
                        .any(|bid| graph.states.contains_key(bid))
                } else {
                    true // no graph populated; keep the entry
                }
            });
        }
    }
}

// ---------------------------------------------------------------------------
// Mutable interior
// ---------------------------------------------------------------------------

/// All state that requires synchronized access — held behind an
/// `Arc<tokio::sync::RwLock<AccInner<S>>>` so that `BeliefSource` futures can
/// capture an `Arc` clone rather than a `&self` reference. Reads take a shared
/// guard; batch application takes an exclusive guard. See the module-level
/// "Interior mutability & `Sync`" section for the full rationale.
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
    /// Absorbed BID -> the claimant that replaced it, accumulated across every
    /// batch of this compile.
    ///
    /// Absorption has to outlive the batch that performed it because the BIDs it
    /// retires are **deterministic**: an href stub's BID is UUID v5 of the URL.
    /// When a second document cites a URL that an earlier epoch already resolved,
    /// `ensure_href_entry` mints a stub with the very same BID again, and that
    /// batch contains no claim to re-absorb it — the claiming document was parsed
    /// in the earlier epoch. Without this map the resurrected stub sits alongside
    /// its claimant and the duplicate path returns.
    ///
    /// Retaining it lets any later batch redirect references to an absorbed BID
    /// without re-resolving anything. Bounded by the number of absorptions in a
    /// compile (tens on a corpus of ~1,100 nodes), so it is not worth evicting.
    absorbed_to_claimant: BTreeMap<Bid, Bid>,
    /// Number of calls to drain_with_census
    drain_count: usize,
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
        tracing::debug!(
            label,
            drain_count = self.drain_count,
            pending_before,
            in_batch_before,
            batch_starts,
            batch_ends,
            inside_batch,
            outside_total,
            outside_census = ?outside_batch,
            "accumulator drain complete",
        );
        self.drain_count += 1;
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
                resolve_merge_keys(&mut sorted, &self.inner, &mut self.absorbed_to_claimant).await;
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

/// Resolve `NodeUpdate` merge keys into explicit `NodeRenamed` + `NodesRemoved`
/// events, so that node absorption happens for **every** [`BeliefSink`], not
/// just the in-memory one.
///
/// # Why this lives here
///
/// A `NodeUpdate` carries merge keys that mean "this node is also whatever these
/// keys name". [`BeliefBase::insert_state`] honours that: it resolves the keys,
/// builds a `to_replace` set, and emits `NodeRenamed` -> `replace_bid` ->
/// `NodesRemoved`, which re-points third-party edges before deleting the absorbed
/// node. That is how a content node claiming a URL retires the `External|Trace`
/// stub that was minted for it by `ensure_href_entry`.
///
/// `Transaction::add_event` — the other `BeliefSink` — destructures the same
/// event and drops the keys on the floor. So absorption depended on which backing
/// store you happened to be running, and the parse command defaults to an
/// *ephemeral in-memory DB*, making the non-absorbing path the normal one. The
/// symptom is a path with two claimants: the retired stub and the content node
/// that should have replaced it.
///
/// Resolving here rather than in either sink makes absorption a property of the
/// event stream instead of a property of the backend. Both sinks then see the
/// same explicit `NodeRenamed`/`NodesRemoved` events and need no merge-key
/// support of their own. `Transaction` in particular stays write-only and
/// batched — no SELECT per key inside transaction assembly.
///
/// # Relationship to `insert_state`
///
/// [`BeliefBase::insert_state`] performs the same absorption against its own
/// graph, and keeps doing so. That is *local consistency*: `GraphBuilder` drives
/// `doc_bb`/`session_bb` through `process_event` directly, never through an
/// accumulator, and a single-file parse can mint a stub and claim it later in
/// the same file. Its derivatives are `EventOrigin::Local` and are not forwarded
/// to `tx`.
///
/// This function owns the *authoritative* version — the events every sink
/// applies — because only it can see the pending batch and carry absorptions
/// across batch boundaries. See the note above the `to_replace` loop in
/// `beliefbase/base.rs` for the other side of this split.
///
/// # Two resolution scopes
///
/// A key may name a node that is already committed to the backing store, or one
/// created by *this same batch* (nothing has been applied yet at this point).
/// Both are checked; a measured census over one corpus found 131 keys resolving
/// against the store and a further 24 resolving only against the pending batch,
/// so consulting the store alone would miss ~15% of real absorptions.
///
/// Pending BIDs are matched only for [`NodeKey::Bid`] keys. Path/Id keys would
/// require replicating PathMap resolution against un-applied events; the href
/// case that motivates this always emits a `Bid` key alongside its `Path` key
/// (the stub's BID is UUID v5 of the URL, so it is computable without having
/// seen the stub), which is what makes the pending scope reachable at all.
///
/// # Ordering
///
/// Runs *before* [`prepare_batch`], so the `NodesRemoved` events synthesized here
/// are folded into that function's consolidated removal and land at the head of
/// the batch — ahead of the `NodeUpdate` that claims the path, which is the order
/// absorption requires.
///
/// # Carry-over across batches
///
/// `absorbed` accumulates for the whole compile rather than being rebuilt per
/// batch; see the field documentation on [`AccInner::absorbed_to_claimant`] for
/// why a deterministic BID makes that necessary.
async fn resolve_merge_keys<S: BeliefSource>(
    events: &mut Vec<BeliefEvent>,
    inner: &S,
    absorbed_to_claimant: &mut BTreeMap<Bid, Bid>,
) {
    // Cheap pre-pass: most batches carry no absorbing key at all. A key that
    // names its own node is a self-reference (the overwhelmingly common case —
    // `compute_diff` emits `NodeUpdate` with a single self-BID key), not a claim
    // on another node, so it cannot absorb anything and is skipped.
    //
    // A batch with no claims of its own may still need the rewrite below, if it
    // references a BID some earlier batch absorbed.
    let has_candidate = events.iter().any(|e| match e {
        BeliefEvent::NodeUpdate(keys, node, _) => keys.iter().any(|k| !names_self(k, node.bid)),
        _ => false,
    });
    if !has_candidate && absorbed_to_claimant.is_empty() {
        return;
    }

    // BIDs this batch creates. Checked before the store, since a key naming a
    // node born in this batch will not be found in `inner`.
    let pending_bids: HashSet<Bid> = events
        .iter()
        .filter_map(|e| match e {
            BeliefEvent::NodeUpdate(_, node, _) => Some(node.bid),
            BeliefEvent::NodeUpsert(bid, _, _) => Some(*bid),
            _ => None,
        })
        .collect();

    // Memoise key -> resolution across the batch. The same URL is frequently
    // claimed by several sections in one epoch, and each miss otherwise costs a
    // full `evaluate` round-trip (a SQL query under `DbConnection`).
    let mut resolved_cache: HashMap<NodeKey, Option<Bid>> = HashMap::new();

    // Candidate (claimant, absorbed) pairs in batch order. Conflicts are
    // resolved in a second pass, once every claim in the batch is known.
    let mut claims: Vec<(Bid, Bid)> = Vec::new();

    for event in events.iter() {
        let BeliefEvent::NodeUpdate(keys, node, _) = event else {
            continue;
        };
        for key in keys {
            if names_self(key, node.bid) {
                continue;
            }
            let target = match resolved_cache.get(key) {
                Some(hit) => *hit,
                None => {
                    // Pending first: a node created by this batch is not yet in
                    // the store, and this check is free.
                    let pending_hit = match key {
                        NodeKey::Bid { bid } if pending_bids.contains(bid) => Some(*bid),
                        _ => None,
                    };
                    let hit = match pending_hit {
                        Some(bid) => Some(bid),
                        None => crate::query::lookup_node(inner, key)
                            .await
                            .ok()
                            .flatten()
                            .map(|n| n.bid),
                    };
                    resolved_cache.insert(key.clone(), hit);
                    hit
                }
            };
            // Resolving back to the claimant is a self-reference by another
            // name (e.g. a Path key for a path this node already holds).
            let Some(target) = target.filter(|t| *t != node.bid) else {
                continue;
            };
            claims.push((node.bid, target));
        }
    }

    // The same pair can be claimed twice over: an href alias emits both a `Bid`
    // key (v5 of the URL) and a `Path` key for one URL, and both resolve to the
    // same stub. That is one absorption asserted twice, not a conflict, so
    // collapse it before conflict detection runs — otherwise the second copy is
    // counted as a double-absorption of a node the first copy just claimed, and
    // the conflict counter reports a problem that does not exist.
    //
    // Deduped in place to preserve batch order, which is what makes
    // first-claim-wins below deterministic.
    let mut seen_pairs: HashSet<(Bid, Bid)> = HashSet::new();
    claims.retain(|pair| seen_pairs.insert(*pair));

    // Absorption is destructive — the absorbed BID's row is deleted and its
    // edges re-pointed at the claimant — so the surviving and deleted sets must
    // be kept disjoint. Three conflicts are possible within one batch, and all
    // three are resolved first-claim-wins, in batch order, for determinism:
    //
    // - **double absorption** (A and B both claim S): the second claim is
    //   dropped. Applying both would emit two `NodeRenamed`s for one node, and
    //   the second would re-point edges off a BID that no longer exists.
    // - **mutual claims** (A claims B, B claims A): the second is dropped.
    //   Applying both deletes both rows and orphans every edge they held.
    // - **chains** (A claims B, B claims C): the second is dropped, so B is not
    //   simultaneously deleted and used as a rename destination.
    //
    // Dropping a claim is safe: the merge key it came from is a *hint* that two
    // nodes are the same, and the duplicate simply survives to be resolved on a
    // later parse. Applying a conflicting claim is not safe.
    // Prior batches' claimants count as claimants here too, so a node that
    // already absorbed something cannot itself be absorbed by a later batch.
    let mut claimants: BTreeSet<Bid> = absorbed_to_claimant.values().copied().collect();
    let mut renames: Vec<BeliefEvent> = Vec::new();
    let mut removals: Vec<Bid> = Vec::new();
    let mut conflicts = 0usize;

    for (claimant, target) in claims {
        if absorbed_to_claimant.contains_key(&target)
            || claimants.contains(&target)
            || absorbed_to_claimant.contains_key(&claimant)
        {
            conflicts += 1;
            tracing::debug!(
                target: "noet_core::db::merge_keys",
                claimant = %claimant,
                absorbed = %target,
                "conflicting absorption claim dropped; duplicate survives this batch",
            );
            continue;
        }
        claimants.insert(claimant);
        absorbed_to_claimant.insert(target, claimant);
        tracing::debug!(
            target: "noet_core::db::merge_keys",
            claimant = %claimant,
            absorbed = %target,
            "merge key resolved: absorbing node into claimant",
        );
        // Remote, so every sink applies it. `BeliefBase::process_event` treats
        // NodeRenamed as a graph-level no-op (insert_state already did the work
        // when it saw the same merge keys) but still routes it through
        // PathMapMap, which re-points index entries off the absorbed BID.
        // `Transaction::rename_node` performs the equivalent UPDATEs in SQL.
        renames.push(BeliefEvent::NodeRenamed(
            target,
            claimant,
            EventOrigin::Remote,
        ));
        removals.push(target);
    }

    if !removals.is_empty() {
        tracing::debug!(
            target: "noet_core::db::merge_keys",
            new_absorptions = removals.len(),
            known_absorptions = absorbed_to_claimant.len(),
            conflicts,
            batch_len = events.len(),
            "merge-key resolution complete for this batch",
        );
    }

    // Nothing absorbed, now or previously — no rewrite to do.
    if absorbed_to_claimant.is_empty() {
        return;
    }

    // Re-point this batch off every absorbed BID, including ones absorbed by an
    // *earlier* batch.
    //
    // Two distinct failures are covered here.
    //
    // Within this batch: `NodeRenamed` fixes up edges already in the store, but
    // the batch still holds its own relation events naming the absorbed node —
    // the very events the claiming document just emitted. Applying those
    // unchanged re-creates the node as a relation row with no `beliefs` row
    // behind it, undoing the absorption inside the batch that performed it.
    //
    // Across batches: href stub BIDs are UUID v5 of the URL, so when a *second*
    // document cites a URL an earlier epoch already resolved, `ensure_href_entry`
    // mints the identical BID again. That batch contains no claim — the claiming
    // document was parsed earlier — so without the carry-over map the stub
    // silently returns. Measured: 17 of 41 duplicate URLs on one corpus were
    // cited by exactly two files and survived batch-local resolution for this
    // reason.
    //
    // `BeliefBase` happens to survive the in-batch case: `update_relation` skips
    // an edge whose source is missing. The SQL sink has no such guard and inserts
    // the row. So this rewrite is also what keeps the two sinks agreeing.
    fn redirect(bid: &mut Bid, map: &BTreeMap<Bid, Bid>) {
        if let Some(claimant) = map.get(bid) {
            *bid = *claimant;
        }
    }
    let mut rewritten = 0usize;
    // (source, sink) pairs that redirection *landed on*. Only these can have
    // collapsed two events into one, so only these need unioning below.
    let mut redirected_pairs: HashSet<(Bid, Bid)> = HashSet::new();
    for event in events.iter_mut() {
        match event {
            BeliefEvent::RelationUpdate(source, sink, _, _)
            | BeliefEvent::RelationChange(source, sink, _, _, _)
            | BeliefEvent::RelationRemoved(source, sink, _) => {
                let (before_src, before_snk) = (*source, *sink);
                redirect(source, absorbed_to_claimant);
                redirect(sink, absorbed_to_claimant);
                if before_src != *source || before_snk != *sink {
                    rewritten += 1;
                    redirected_pairs.insert((*source, *sink));
                }
            }
            BeliefEvent::PathAdded(_, _, target, _, _)
            | BeliefEvent::PathUpdate(_, _, target, _, _) => {
                let before = *target;
                redirect(target, absorbed_to_claimant);
                if before != *target {
                    rewritten += 1;
                }
            }
            _ => {}
        }
    }

    // A relation event may now be a self-loop (both endpoints absorbed into the
    // same claimant, or an edge that ran between claimant and absorbed node).
    // Neither sink has a use for one, and the SQL unique index rejects the pair.
    //
    // Drop the absorbed node's own `NodeUpdate` in the same pass.
    //
    // Most absorptions resolve against the *pending* set, which means the batch
    // still carries the event that creates the node being absorbed. `prepare_batch`
    // orders `NodesRemoved` first and node events after it — deliberately, so that
    // a later update's internal removes are no-ops — with the result that the
    // removal deletes the stub and its own `NodeUpdate`, two positions later,
    // immediately puts it back. The absorption then appears to have happened
    // (the events are all there) while the duplicate quietly survives the batch.
    events.retain(|e| match e {
        BeliefEvent::RelationUpdate(source, sink, _, _)
        | BeliefEvent::RelationChange(source, sink, _, _, _)
        | BeliefEvent::RelationRemoved(source, sink, _) => source != sink,
        BeliefEvent::NodeUpdate(_, node, _) => !absorbed_to_claimant.contains_key(&node.bid),
        BeliefEvent::NodeUpsert(bid, _, _) => !absorbed_to_claimant.contains_key(bid),
        _ => true,
    });

    // Redirecting can collapse two events onto one (source, sink) pair -- the
    // absorbed node and its claimant both had an edge to the same neighbour.
    // Union them rather than letting the later one win: the whole point of
    // absorption is that the claimant inherits what the absorbed node held, and
    // a positional last-writer-wins here would silently drop any weight kind
    // carried only by the absorbed node's edge.
    let mut merged_pairs: BTreeMap<(Bid, Bid), WeightSet> = BTreeMap::new();
    for event in events.iter() {
        if let BeliefEvent::RelationUpdate(source, sink, ws, _) = event {
            if !redirected_pairs.contains(&(*source, *sink)) {
                continue;
            }
            merged_pairs
                .entry((*source, *sink))
                .and_modify(|acc| *acc = acc.union(ws))
                .or_insert_with(|| ws.clone());
        }
    }
    let mut pair_emitted: HashSet<(Bid, Bid)> = HashSet::new();
    events.retain_mut(|event| {
        let BeliefEvent::RelationUpdate(source, sink, ws, _) = event else {
            return true;
        };
        let Some(merged) = merged_pairs.get(&(*source, *sink)) else {
            return true;
        };
        // Keep the first occurrence of a collapsed pair, carrying the unioned
        // weights; drop the rest.
        if !pair_emitted.insert((*source, *sink)) {
            return false;
        }
        *ws = merged.clone();
        true
    });

    tracing::debug!(
        target: "noet_core::db::merge_keys",
        rewritten,
        "re-pointed in-batch events off absorbed BIDs",
    );

    // `prepare_batch` runs next: it consolidates every NodesRemoved into one
    // event at position 0 and sorts NodeRenamed in with the node events, so the
    // final order is removal -> rename/update -> relations. Appending here is
    // enough; no ordering is imposed at this point.
    events.append(&mut renames);
    if !removals.is_empty() {
        events.push(BeliefEvent::NodesRemoved(removals, EventOrigin::Remote));
    }

    // Expand the renames just synthesized, plus any the producer already put in
    // this batch (`terminate_stack` emits its own).
    //
    // The rewrite above only touches events this batch happens to carry. Edges
    // that live solely in the backing store are invisible to it, and re-pointing
    // those is what the expansion does.
    expand_renames(events, inner).await;
}

/// Rewrite every `NodeRenamed` in a batch into the explicit edge events it
/// implies, so no sink has to work out edge re-pointing for itself.
///
/// # Why
///
/// `NodeRenamed(from, to)` means "re-point everything at `from` onto `to`, then
/// delete `from`". Both sinks used to implement that separately, and they
/// disagreed on the case that matters:
///
/// | | in-memory (`replace_bid`) | SQL (`rename_node`) |
/// |---|---|---|
/// | colliding edge | `w_to.union(w_from)` | kept `w_to`, dropped `w_from` |
/// | self-loop | skipped | deleted before the update |
/// | unique index | n/a | `UNIQUE(sink, source)` had to be dodged |
///
/// The SQL side could not be fixed in place: weights live in serialized TEXT
/// columns, so the union is not expressible there. Computing it **once**, here,
/// and emitting ordinary `RelationRemoved` + `RelationUpdate` events makes both
/// sinks reach the same result through code they already had —
/// `update_relation`, whose `INSERT OR REPLACE` also resolves the unique-index
/// collision natively.
///
/// Same principle as [`resolve_merge_keys`]: absorption belongs to the event
/// stream, not to the backing store.
///
/// # One query per batch
///
/// Edge lookup is batched across every rename rather than issued per rename:
/// collect the endpoints, then a single [`lookup_edges`] covers all of them.
/// Two queries for a whole corpus parse, against one per absorption.
///
/// [`lookup_edges`]: crate::query::lookup_edges
async fn expand_renames<S: BeliefSource>(events: &mut Vec<BeliefEvent>, inner: &S) {
    let renames: Vec<(Bid, Bid)> = events
        .iter()
        .filter_map(|e| match e {
            BeliefEvent::NodeRenamed(from, to, _) => Some((*from, *to)),
            _ => None,
        })
        .collect();
    if renames.is_empty() {
        return;
    }

    // Both endpoints: `from` for the edges being moved, `to` because the union
    // needs the claimant's existing weights wherever the two overlap.
    //
    // `lookup_edges` is balanced, so it returns edges incident to these BIDs
    // even when the node at the other end is not in the list. A seed-only
    // package would return no edges at all here — see its documentation.
    let endpoints: Vec<Bid> = renames
        .iter()
        .flat_map(|(from, to)| [*from, *to])
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();

    let stored = match crate::query::lookup_edges(inner, &endpoints).await {
        Ok(g) => g,
        Err(e) => {
            tracing::warn!(
                target: "noet_core::db::merge_keys",
                error = %e,
                "rename expansion could not read incident edges; \
                 falling back to per-sink rename handling",
            );
            return;
        }
    };

    // (source, sink) -> weights, seeded from the store then overlaid with this
    // batch's own edges, which are the fresher statement of the same fact.
    let mut edges: BTreeMap<(Bid, Bid), WeightSet> = BTreeMap::new();
    {
        let graph = stored.relations.as_graph();
        for edge_idx in graph.edge_indices() {
            if let Some((src_idx, snk_idx)) = graph.edge_endpoints(edge_idx) {
                if let Some(ws) = graph.edge_weight(edge_idx) {
                    edges.insert((graph[src_idx], graph[snk_idx]), ws.clone());
                }
            }
        }
    }
    for event in events.iter() {
        if let BeliefEvent::RelationUpdate(source, sink, ws, _) = event {
            edges.insert((*source, *sink), ws.clone());
        }
    }

    let mut expanded: Vec<BeliefEvent> = Vec::new();
    // Pairs this expansion emits an authoritative RelationUpdate for. Only these
    // may be dropped from the batch below — a batch-wide dedup would discard
    // unrelated events that merely share an endpoint with a rename.
    let mut superseded: HashSet<(Bid, Bid)> = HashSet::new();
    let (mut n_moved, mut n_unioned, mut n_selfloop) = (0usize, 0usize, 0usize);

    for (from, to) in &renames {
        // Snapshot before mutating, so a batch containing several renames that
        // touch one neighbourhood still reads a stable view per rename.
        let incident: Vec<((Bid, Bid), WeightSet)> = edges
            .iter()
            .filter(|((s, k), _)| s == from || k == from)
            .map(|(pair, ws)| (*pair, ws.clone()))
            .collect();

        for ((source, sink), ws) in incident {
            let new_source = if source == *from { *to } else { source };
            let new_sink = if sink == *from { *to } else { sink };

            // An edge between the two renamed nodes collapses to a self-loop:
            // meaningless to either sink, and rejected by the unique index.
            if new_source == new_sink {
                expanded.push(BeliefEvent::RelationRemoved(
                    source,
                    sink,
                    EventOrigin::Remote,
                ));
                superseded.insert((source, sink));
                edges.remove(&(source, sink));
                n_selfloop += 1;
                continue;
            }

            // Union with whatever the claimant already holds between the same
            // endpoints. This is the case the sinks disagreed on.
            let merged = match edges.get(&(new_source, new_sink)) {
                Some(existing) if (new_source, new_sink) != (source, sink) => {
                    n_unioned += 1;
                    existing.union(&ws)
                }
                _ => {
                    n_moved += 1;
                    ws
                }
            };

            expanded.push(BeliefEvent::RelationRemoved(
                source,
                sink,
                EventOrigin::Remote,
            ));
            expanded.push(BeliefEvent::RelationUpdate(
                new_source,
                new_sink,
                merged.clone(),
                EventOrigin::Remote,
            ));
            superseded.insert((source, sink));
            superseded.insert((new_source, new_sink));

            edges.remove(&(source, sink));
            edges.insert((new_source, new_sink), merged);
        }
    }

    if expanded.is_empty() {
        return;
    }

    // Drop only the batch events the expansion supersedes. Anything else stays
    // exactly as it was.
    events.retain(|e| match e {
        BeliefEvent::RelationUpdate(source, sink, _, _)
        | BeliefEvent::RelationChange(source, sink, _, _, _)
        | BeliefEvent::RelationRemoved(source, sink, _) => !superseded.contains(&(*source, *sink)),
        _ => true,
    });

    tracing::debug!(
        target: "noet_core::db::merge_keys",
        renames = renames.len(),
        moved = n_moved,
        unioned = n_unioned,
        self_loops = n_selfloop,
        "expanded NodeRenamed into explicit edge events",
    );

    events.append(&mut expanded);
}

/// Does this key simply name `bid` itself?
///
/// Self-referential keys are the norm — `compute_diff` emits every `NodeUpdate`
/// with its own BID as the sole key — and can never absorb anything, so they are
/// filtered before any lookup is attempted.
fn names_self(key: &NodeKey, bid: Bid) -> bool {
    match key {
        NodeKey::Bid { bid: k } => *k == bid,
        NodeKey::Bref { bref } => *bref == bid.bref(),
        _ => false,
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
        BeliefEvent::NodeUpdate(_, _, _)
        | BeliefEvent::NodeUpsert(_, _, _)
        | BeliefEvent::NodeRenamed(_, _, _) => 0u8,
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
/// See the module documentation for full design rationale.
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
    acc: Arc<tokio::sync::RwLock<AccInner<S>>>,
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
            acc: Arc::new(tokio::sync::RwLock::new(AccInner {
                inner,
                rx,
                pending: Vec::new(),
                in_batch: false,
                absorbed_to_claimant: BTreeMap::new(),
                drain_count: 0,
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
            let mut guard = self.acc.write().await;
            guard.drain_with_census(&self.cache, "into_inner").await?;
        }
        let arc = self.acc;
        match Arc::try_unwrap(arc) {
            Ok(rwlock) => Ok(rwlock.into_inner().inner),
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
    /// [`BeliefSource::evaluate`] without exclusive access to the channel.
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
    fn submap<'a>(
        &'a self,
        network_bid: Bid,
        path: &'a str,
        depth: u8,
        include_index: bool,
    ) -> BoxFuture<'a, SubmapResult> {
        let acc = Arc::clone(&self.acc);
        let path = path.to_owned();

        Box::pin(async move {
            let guard = acc.read().await;
            guard
                .inner
                .submap(network_bid, &path, depth, include_index)
                .await
        })
    }

    fn submap_by_bid<'a>(
        &'a self,
        network_bid: Bid,
        entry: Option<Bid>,
        depth: u8,
        include_index: bool,
    ) -> BoxFuture<'a, SubmapResult> {
        let acc = Arc::clone(&self.acc);
        Box::pin(async move {
            let guard = acc.read().await;
            guard
                .inner
                .submap_by_bid(network_bid, entry, depth, include_index)
                .await
        })
    }

    fn get_file_mtimes(&self) -> BoxFuture<'_, Result<BTreeMap<PathBuf, i64>, BuildonomyError>> {
        let acc = Arc::clone(&self.acc);

        Box::pin(async move {
            let guard = acc.read().await;
            guard.inner.get_file_mtimes().await
        })
    }

    fn export_beliefgraph(&self) -> BoxFuture<'_, Result<BeliefGraph, BuildonomyError>> {
        let acc = Arc::clone(&self.acc);

        Box::pin(async move {
            let guard = acc.read().await;
            guard.inner.export_beliefgraph().await
        })
    }

    fn evaluate<'a>(
        &'a self,
        package: &'a mut QueryPackage,
    ) -> BoxFuture<'a, Result<(), BuildonomyError>> {
        // Serialize the original spec as cache key before evaluation
        // (evaluation mutates the spec via ensure_graph_context).
        let cache_key = serde_json::to_string(package.original_spec()).unwrap_or_default();
        let acc = Arc::clone(&self.acc);
        let cache = Arc::clone(&self.cache);

        Box::pin(async move {
            // Cache hit: restore the full package state (spec, tape, output)
            // from the cached entry. No re-evaluation needed.
            if let Some(cached) = cache.get(&cache_key) {
                *package = cached;
                return Ok(());
            }

            // Cache miss — delegate to inner. Shared guard: concurrent evaluate()
            // calls (and other BeliefSource reads) may proceed in parallel; only
            // batch application (handle_event/drain_with_census) needs exclusivity.
            {
                let guard = acc.read().await;
                guard.inner.evaluate(package).await?;
            }

            // Cache the completed package.
            if package.graph().is_some() {
                cache.insert(cache_key, CacheEntry::new(package.clone()));
            }
            Ok(())
        })
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
    acc: Arc<tokio::sync::RwLock<AccInner<S>>>,
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
            let mut guard = acc.write().await;
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
    fn submap<'a>(
        &'a self,
        network_bid: Bid,
        path: &'a str,
        depth: u8,
        include_index: bool,
    ) -> BoxFuture<'a, SubmapResult> {
        let acc = Arc::clone(&self.acc);
        let path = path.to_owned();

        Box::pin(async move {
            let guard = acc.read().await;
            guard
                .inner
                .submap(network_bid, &path, depth, include_index)
                .await
        })
    }

    fn submap_by_bid<'a>(
        &'a self,
        network_bid: Bid,
        entry: Option<Bid>,
        depth: u8,
        include_index: bool,
    ) -> BoxFuture<'a, SubmapResult> {
        let acc = Arc::clone(&self.acc);
        Box::pin(async move {
            let guard = acc.read().await;
            guard
                .inner
                .submap_by_bid(network_bid, entry, depth, include_index)
                .await
        })
    }

    fn get_file_mtimes(&self) -> BoxFuture<'_, Result<BTreeMap<PathBuf, i64>, BuildonomyError>> {
        let acc = Arc::clone(&self.acc);

        Box::pin(async move {
            let guard = acc.read().await;
            guard.inner.get_file_mtimes().await
        })
    }

    fn export_beliefgraph(&self) -> BoxFuture<'_, Result<BeliefGraph, BuildonomyError>> {
        let acc = Arc::clone(&self.acc);

        Box::pin(async move {
            let guard = acc.read().await;
            guard.inner.export_beliefgraph().await
        })
    }

    fn evaluate<'a>(
        &'a self,
        package: &'a mut QueryPackage,
    ) -> BoxFuture<'a, Result<(), BuildonomyError>> {
        let cache_key = serde_json::to_string(package.original_spec()).unwrap_or_default();
        let acc = Arc::clone(&self.acc);
        let cache = Arc::clone(&self.cache);

        Box::pin(async move {
            if let Some(cached) = cache.get(&cache_key) {
                *package = cached;
                return Ok(());
            }

            // Shared guard: concurrent evaluate() calls across QueryHandle clones
            // (the `--jobs N` parallel-task fan-out) may proceed in parallel; only
            // batch application takes the exclusive guard (see drain_epoch above).
            {
                let guard = acc.read().await;
                guard.inner.evaluate(package).await?;
            }

            if package.graph().is_some() {
                cache.insert(cache_key, CacheEntry::new(package.clone()));
            }
            Ok(())
        })
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
        properties::Bid,
        query::spec::{QueryPackage, QuerySpec, TapeFn},
    };

    // -----------------------------------------------------------------------
    // Minimal BeliefSource + BeliefSink that counts apply_batch calls.
    // -----------------------------------------------------------------------

    #[derive(Clone)]
    struct CountingStore {
        query_count: Arc<AtomicUsize>,
        batch_count: Arc<AtomicUsize>,
        result: BeliefGraph,
        /// Optional artificial delay applied inside `evaluate`, used to make
        /// concurrency (vs. serialization) observable via wall-clock timing.
        evaluate_delay: Option<std::time::Duration>,
        /// Concurrent-evaluate() tracking: incremented on entry, decremented on
        /// exit, and the running max recorded — lets a test assert that more
        /// than one `evaluate()` call was in flight at the same time.
        in_flight: Arc<AtomicUsize>,
        max_in_flight: Arc<AtomicUsize>,
        /// Optional artificial delay applied inside `apply_batch`, giving a
        /// concurrent `evaluate()` call a window in which to (incorrectly)
        /// observe a partially-applied batch if the exclusive guard is broken.
        apply_batch_delay: Option<std::time::Duration>,
        /// Set to `true` for the duration of `apply_batch`; a concurrent
        /// `evaluate()` call records whether it ever observed this as `true`
        /// (which would mean it ran without mutual exclusion against the write).
        write_in_progress: Arc<std::sync::atomic::AtomicBool>,
        /// Sticky flag: true if any `evaluate()` call observed `write_in_progress`
        /// set while it ran.
        observed_write_in_progress: Arc<std::sync::atomic::AtomicBool>,
    }

    impl CountingStore {
        fn new() -> Self {
            Self {
                query_count: Arc::new(AtomicUsize::new(0)),
                batch_count: Arc::new(AtomicUsize::new(0)),
                result: BeliefGraph::default(),
                evaluate_delay: None,
                in_flight: Arc::new(AtomicUsize::new(0)),
                max_in_flight: Arc::new(AtomicUsize::new(0)),
                apply_batch_delay: None,
                write_in_progress: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                observed_write_in_progress: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            }
        }

        fn with_evaluate_delay(delay: std::time::Duration) -> Self {
            Self {
                evaluate_delay: Some(delay),
                ..Self::new()
            }
        }

        fn with_apply_batch_delay(delay: std::time::Duration) -> Self {
            Self {
                apply_batch_delay: Some(delay),
                ..Self::new()
            }
        }

        fn query_count(&self) -> usize {
            self.query_count.load(Ordering::SeqCst)
        }

        fn batch_count(&self) -> usize {
            self.batch_count.load(Ordering::SeqCst)
        }

        fn max_in_flight(&self) -> usize {
            self.max_in_flight.load(Ordering::SeqCst)
        }

        fn observed_write_in_progress(&self) -> bool {
            self.observed_write_in_progress.load(Ordering::SeqCst)
        }
    }

    impl BeliefSource for CountingStore {
        fn evaluate<'a>(
            &'a self,
            package: &'a mut QueryPackage,
        ) -> crate::query::BoxFuture<'a, Result<(), BuildonomyError>> {
            self.query_count.fetch_add(1, Ordering::SeqCst);
            let delay = self.evaluate_delay;
            let in_flight = Arc::clone(&self.in_flight);
            let max_in_flight = Arc::clone(&self.max_in_flight);
            let result = self.result.clone();
            let write_in_progress = Arc::clone(&self.write_in_progress);
            let observed_write_in_progress = Arc::clone(&self.observed_write_in_progress);
            Box::pin(async move {
                let current = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                max_in_flight.fetch_max(current, Ordering::SeqCst);
                if let Some(delay) = delay {
                    tokio::time::sleep(delay).await;
                }
                if write_in_progress.load(Ordering::SeqCst) {
                    observed_write_in_progress.store(true, Ordering::SeqCst);
                }
                in_flight.fetch_sub(1, Ordering::SeqCst);
                package.set_graph(result);
                Ok(())
            })
        }

        fn submap<'a>(
            &'a self,
            _network_bid: Bid,
            _path: &'a str,
            _depth: u8,
            _include_index: bool,
        ) -> crate::query::BoxFuture<'a, crate::query::SubmapResult> {
            unimplemented!("CountingStore is a test stub and does not support submap")
        }

        fn submap_by_bid<'a>(
            &'a self,
            _network_bid: Bid,
            _entry: Option<Bid>,
            _depth: u8,
            _include_index: bool,
        ) -> crate::query::BoxFuture<'a, crate::query::SubmapResult> {
            unimplemented!("CountingStore is a test stub and does not support submap_by_bid")
        }
    }

    impl BeliefSink for CountingStore {
        async fn apply_batch(&mut self, _events: &[BeliefEvent]) -> Result<(), BuildonomyError> {
            self.write_in_progress.store(true, Ordering::SeqCst);
            if let Some(delay) = self.apply_batch_delay {
                tokio::time::sleep(delay).await;
            }
            self.batch_count.fetch_add(1, Ordering::SeqCst);
            self.write_in_progress.store(false, Ordering::SeqCst);
            Ok(())
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
            crate::properties::BeliefNode {
                bid: test_bid(1),
                ..Default::default()
            },
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
    // evaluate — cache hit avoids second inner call
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn evaluate_cache_hit_avoids_second_inner_call() {
        let (_tx, rx) = unbounded_channel::<BeliefEvent>();
        let store = CountingStore::new();
        let acc = BeliefAccumulator::new(store.clone(), rx);

        let spec = QuerySpec::seed(TapeFn::Corpus);

        // No lazy drain: inner is queried directly through cache.
        let mut pkg = QueryPackage::new(spec.clone());
        acc.evaluate(&mut pkg).await.unwrap();
        assert_eq!(store.query_count(), 1);

        let mut pkg2 = QueryPackage::new(spec.clone());
        acc.evaluate(&mut pkg2).await.unwrap();
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

        let spec = QuerySpec::seed(TapeFn::Corpus);

        // Warm the cache.
        let mut pkg = QueryPackage::new(spec.clone());
        acc.evaluate(&mut pkg).await.unwrap();
        assert_eq!(store.query_count(), 1);
        assert_eq!(acc.cache_len(), 1);

        // Send a BatchStart/BatchEnd pair and close the channel.
        tx.send(BeliefEvent::BatchStart).unwrap();
        tx.send(BeliefEvent::BatchEnd).unwrap();
        drop(tx);

        // into_inner drains the channel; BatchEnd processing clears the cache.
        // Verify via a second accumulator round: the query count increments
        // again (not cached from the old accumulator).
        let inner = acc.into_inner().await.unwrap();
        // BatchEnd always calls apply_batch (even with empty pending), so batch_count = 1.
        assert_eq!(inner.batch_count(), 1);
        // The old cache was on the old Arc<AccCache> which is now dropped.
        // A fresh accumulator wrapping the same inner has an empty cache.
        let (tx2, rx2) = unbounded_channel::<BeliefEvent>();
        drop(tx2);
        let acc2 = BeliefAccumulator::new(inner, rx2);
        let mut pkg2 = QueryPackage::new(spec.clone());
        acc2.evaluate(&mut pkg2).await.unwrap();
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
                crate::properties::BeliefNode {
                    bid,
                    ..Default::default()
                },
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
    // Concurrent evaluate() calls run in parallel, not serialized (Issue 100)
    // -----------------------------------------------------------------------
    //
    // Regression test for the RwLock split: with the old full-duration
    // tokio::Mutex, N concurrent 1-evaluate-delay-long QueryHandle::evaluate()
    // calls would serialize and take ~N * delay wall-clock time. With the
    // shared (read) guard, they should all run within roughly one delay's
    // worth of wall-clock time.

    #[tokio::test]
    async fn concurrent_evaluates_do_not_serialize() {
        let delay = std::time::Duration::from_millis(200);
        let (_tx, rx) = unbounded_channel::<BeliefEvent>();
        let store = CountingStore::with_evaluate_delay(delay);
        let acc = BeliefAccumulator::new(store.clone(), rx);

        // Cache would defeat this test (second+ evaluate() calls would hit the
        // cache and never enter CountingStore::evaluate at all), so use N
        // distinct QuerySpecs — one per task — to force N real evaluate() calls.
        const N: usize = 8;
        let start = std::time::Instant::now();
        let mut tasks = Vec::with_capacity(N);
        for i in 0..N {
            let handle = acc.query_handle();
            tasks.push(tokio::spawn(async move {
                let spec = QuerySpec::seed(TapeFn::Bids(vec![test_bid(i as u128)]));
                let mut pkg = QueryPackage::new(spec);
                handle.evaluate(&mut pkg).await.unwrap();
            }));
        }
        for task in tasks {
            task.await.unwrap();
        }
        let elapsed = start.elapsed();

        assert_eq!(
            store.query_count(),
            N,
            "all N evaluate() calls must run (no cache hits)"
        );
        assert!(
            store.max_in_flight() > 1,
            "expected concurrent evaluate() calls to overlap; max_in_flight = {}",
            store.max_in_flight()
        );
        assert!(
            elapsed < delay * (N as u32 / 2),
            "N={N} concurrent {delay:?} evaluate() calls took {elapsed:?} — looks serialized, \
             expected roughly one delay's worth of wall-clock time"
        );
    }

    // -----------------------------------------------------------------------
    // A write (apply_batch) still excludes concurrent reads (Issue 100)
    // -----------------------------------------------------------------------
    //
    // Regression test for the RwLock split: the exclusive (write) guard taken
    // by AccInner::handle_event/drain_with_census around apply_batch must still
    // block concurrent evaluate() calls from observing a partially-applied
    // batch — i.e. no reader should ever see write_in_progress = true.

    #[tokio::test]
    async fn write_excludes_concurrent_reads() {
        let write_delay = std::time::Duration::from_millis(200);
        let (tx, rx) = unbounded_channel::<BeliefEvent>();
        let store = CountingStore::with_apply_batch_delay(write_delay);
        let acc = BeliefAccumulator::new(store.clone(), rx);
        let handle = acc.query_handle();

        // Start a batch and queue events, but don't send BatchEnd/drain yet.
        tx.send(BeliefEvent::BatchStart).unwrap();
        tx.send(BeliefEvent::NodeUpdate(
            vec![NodeKey::Bid { bid: test_bid(1) }],
            crate::properties::BeliefNode {
                bid: test_bid(1),
                ..Default::default()
            },
            EventOrigin::Remote,
        ))
        .unwrap();
        tx.send(BeliefEvent::BatchEnd).unwrap();

        // Drive the write (drain_epoch takes the exclusive guard and blocks in
        // apply_batch for write_delay) concurrently with a burst of reads.
        let write_task = {
            let handle = handle.clone();
            tokio::spawn(async move { handle.drain_epoch().await.unwrap() })
        };
        // Give the write a head start so it is holding the exclusive guard
        // when the reads below attempt to acquire the shared guard.
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        let mut read_tasks = Vec::new();
        for i in 0..4 {
            let handle = handle.clone();
            read_tasks.push(tokio::spawn(async move {
                let spec = QuerySpec::seed(TapeFn::Bids(vec![test_bid(100 + i as u128)]));
                let mut pkg = QueryPackage::new(spec);
                handle.evaluate(&mut pkg).await.unwrap();
            }));
        }
        for task in read_tasks {
            task.await.unwrap();
        }
        write_task.await.unwrap();

        assert!(
            !store.observed_write_in_progress(),
            "a concurrent evaluate() observed write_in_progress = true — the exclusive \
             guard around apply_batch did not exclude concurrent reads"
        );
        assert_eq!(store.batch_count(), 1);
        drop(tx);
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

        let spec = QuerySpec::seed(TapeFn::Corpus);

        // Warm via accumulator.
        let mut pkg = QueryPackage::new(spec.clone());
        acc.evaluate(&mut pkg).await.unwrap();
        assert_eq!(store.query_count(), 1);

        // Handle shares the same Arc<AccCache> — should hit the cached entry.
        let mut pkg2 = QueryPackage::new(spec.clone());
        handle.evaluate(&mut pkg2).await.unwrap();
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

    // -----------------------------------------------------------------------
    // resolve_merge_keys
    // -----------------------------------------------------------------------

    /// Build a `NodeUpdate` for `bid` carrying `keys` as merge keys.
    fn node_update(bid: Bid, keys: Vec<NodeKey>) -> BeliefEvent {
        BeliefEvent::NodeUpdate(
            keys,
            crate::properties::BeliefNode {
                bid,
                ..Default::default()
            },
            EventOrigin::Remote,
        )
    }

    /// Run `resolve_merge_keys` on a single batch with no prior absorptions.
    ///
    /// Tests that span batches use `resolve_merge_keys` directly so they can
    /// carry one map across calls.
    async fn resolve_one_batch<S: BeliefSource>(events: &mut Vec<BeliefEvent>, store: &S) {
        let mut absorbed = BTreeMap::new();
        resolve_merge_keys(events, store, &mut absorbed).await;
    }

    /// Collect (from, to) pairs from every `NodeRenamed` in a batch.
    fn renames_in(events: &[BeliefEvent]) -> Vec<(Bid, Bid)> {
        events
            .iter()
            .filter_map(|e| match e {
                BeliefEvent::NodeRenamed(from, to, _) => Some((*from, *to)),
                _ => None,
            })
            .collect()
    }

    /// Collect every BID named by a `NodesRemoved` in a batch.
    fn removals_in(events: &[BeliefEvent]) -> Vec<Bid> {
        events
            .iter()
            .filter_map(|e| match e {
                BeliefEvent::NodesRemoved(bids, _) => Some(bids.clone()),
                _ => None,
            })
            .flatten()
            .collect()
    }

    /// The self-referential key that `compute_diff` puts on every `NodeUpdate`
    /// must not be mistaken for a claim on another node.
    #[tokio::test]
    async fn self_referential_keys_produce_no_absorption() {
        let store = CountingStore::new();
        let bid = test_bid(1);
        let mut events = vec![
            node_update(bid, vec![NodeKey::Bid { bid }]),
            node_update(bid, vec![NodeKey::Bref { bref: bid.bref() }]),
        ];
        let before = events.len();

        resolve_one_batch(&mut events, &store).await;

        assert_eq!(
            events.len(),
            before,
            "a key naming its own node is not a claim on another node and must \
             not synthesize absorption events; got {events:?}"
        );
    }

    /// A key naming a node created earlier in the *same* batch resolves against
    /// the pending set. Nothing has been applied to the store at this point, so
    /// consulting `inner` alone would miss it.
    #[tokio::test]
    async fn key_resolving_against_pending_batch_absorbs() {
        // CountingStore::evaluate returns an empty graph, so every `lookup_node`
        // misses. Any absorption here therefore came from the pending set.
        let store = CountingStore::new();
        let stub = test_bid(1);
        let claimant = test_bid(2);

        let mut events = vec![
            node_update(stub, vec![NodeKey::Bid { bid: stub }]),
            node_update(claimant, vec![NodeKey::Bid { bid: stub }]),
        ];

        resolve_one_batch(&mut events, &store).await;

        assert_eq!(
            renames_in(&events),
            vec![(stub, claimant)],
            "the claimant should absorb the stub born in this same batch"
        );
        assert_eq!(
            removals_in(&events),
            vec![stub],
            "the absorbed node should be removed"
        );
    }

    /// A key that resolves to nothing at all is inert — the overwhelmingly
    /// common case, where a claim is simply the first registration of that path.
    #[tokio::test]
    async fn unresolvable_key_produces_no_absorption() {
        let store = CountingStore::new();
        let claimant = test_bid(2);
        let mut events = vec![node_update(
            claimant,
            vec![NodeKey::Bid { bid: test_bid(99) }],
        )];
        let before = events.len();

        resolve_one_batch(&mut events, &store).await;

        assert_eq!(
            events.len(),
            before,
            "a key naming a node that exists nowhere has nothing to absorb"
        );
    }

    /// Two nodes claiming each other must not both be deleted.
    ///
    /// Absorption is destructive: the absorbed BID's row is dropped and its
    /// edges re-pointed. If A absorbs B *and* B absorbs A, applying both leaves
    /// neither node in the graph and orphans every edge they held. One side has
    /// to lose.
    #[tokio::test]
    async fn mutual_claims_do_not_delete_both_nodes() {
        let store = CountingStore::new();
        let a = test_bid(1);
        let b = test_bid(2);

        let mut events = vec![
            node_update(a, vec![NodeKey::Bid { bid: b }]),
            node_update(b, vec![NodeKey::Bid { bid: a }]),
        ];

        resolve_one_batch(&mut events, &store).await;

        let removed = removals_in(&events);
        assert_eq!(
            removed.len(),
            1,
            "exactly one of the mutually-claiming pair may be absorbed, else \
             both rows vanish; got {removed:?}"
        );
        let renames = renames_in(&events);
        assert_eq!(renames.len(), 1, "one rename to match the one removal");
        // The surviving claimant must not itself be on the removal list.
        let (from, to) = renames[0];
        assert!(
            !removed.contains(&to),
            "the surviving claimant {to} must not also be removed"
        );
        assert_eq!(from, removed[0], "the rename must name the removed node");
    }

    /// One claimant naming one stub by two different keys is a single
    /// absorption, not a conflict.
    ///
    /// An href alias emits both a `Bid` key (v5 of the URL) and a `Path` key for
    /// the same URL; both resolve to the same stub. Counting the second as a
    /// double-absorption would drop it *and* report a conflict that does not
    /// exist.
    #[tokio::test]
    async fn duplicate_claim_of_same_pair_is_one_absorption() {
        let store = CountingStore::new();
        let stub = test_bid(1);
        let claimant = test_bid(2);

        let mut events = vec![
            node_update(stub, vec![NodeKey::Bid { bid: stub }]),
            // Two keys, same claimant, both resolving to the same stub.
            node_update(
                claimant,
                vec![NodeKey::Bid { bid: stub }, NodeKey::Bid { bid: stub }],
            ),
        ];

        resolve_one_batch(&mut events, &store).await;

        assert_eq!(
            renames_in(&events),
            vec![(stub, claimant)],
            "one pair claimed twice is one rename"
        );
        assert_eq!(
            removals_in(&events),
            vec![stub],
            "one pair claimed twice is one removal"
        );
    }

    /// Two claimants naming the same stub must not both absorb it.
    ///
    /// This is the realistic conflict: several documents cite one URL and each
    /// declares it as an alias, so every one of them emits a merge key naming
    /// the same `External|Trace` stub. Emitting two `NodeRenamed`s for that stub
    /// would re-point the second batch of edges onto a BID already deleted by
    /// the first.
    #[tokio::test]
    async fn one_stub_claimed_twice_is_absorbed_once() {
        let store = CountingStore::new();
        let stub = test_bid(1);
        let first = test_bid(2);
        let second = test_bid(3);

        let mut events = vec![
            node_update(stub, vec![NodeKey::Bid { bid: stub }]),
            node_update(first, vec![NodeKey::Bid { bid: stub }]),
            node_update(second, vec![NodeKey::Bid { bid: stub }]),
        ];

        resolve_one_batch(&mut events, &store).await;

        assert_eq!(
            renames_in(&events),
            vec![(stub, first)],
            "only the first claimant may absorb the stub"
        );
        assert_eq!(
            removals_in(&events),
            vec![stub],
            "the stub must be removed exactly once"
        );
    }

    /// Relation events later in the same batch must be re-pointed at the
    /// claimant, not left naming a BID this batch is about to delete.
    ///
    /// Regression: the claiming document emits its own Section edge from the
    /// stub in the same epoch that absorbs it. Applying that edge unchanged
    /// re-creates the stub as a relation row with no `beliefs` row behind it
    /// ("nodes in relations but NOT in states"), undoing the absorption inside
    /// the batch that performed it.
    #[tokio::test]
    async fn in_batch_relations_are_repointed_off_absorbed_nodes() {
        let store = CountingStore::new();
        let stub = test_bid(1);
        let claimant = test_bid(2);
        let neighbour = test_bid(3);

        let mut events = vec![
            node_update(stub, vec![NodeKey::Bid { bid: stub }]),
            node_update(claimant, vec![NodeKey::Bid { bid: stub }]),
            BeliefEvent::RelationUpdate(
                stub,
                neighbour,
                crate::properties::WeightSet::default(),
                EventOrigin::Remote,
            ),
        ];

        resolve_one_batch(&mut events, &store).await;

        let edges: Vec<(Bid, Bid)> = events
            .iter()
            .filter_map(|e| match e {
                BeliefEvent::RelationUpdate(s, k, _, _) => Some((*s, *k)),
                _ => None,
            })
            .collect();
        assert_eq!(
            edges,
            vec![(claimant, neighbour)],
            "the edge should now originate from the claimant, not the absorbed stub"
        );
    }

    /// The absorbed node's own `NodeUpdate` must be dropped from the batch.
    ///
    /// Regression: absorptions usually resolve against the *pending* set, so the
    /// batch still contains the event that creates the stub. `prepare_batch` puts
    /// `NodesRemoved` first and node events after it, so the removal deletes the
    /// stub and its own `NodeUpdate` re-inserts it two positions later — the
    /// absorption looks complete in the event log while the duplicate survives.
    #[tokio::test]
    async fn absorbed_nodes_own_update_is_dropped_from_batch() {
        let store = CountingStore::new();
        let stub = test_bid(1);
        let claimant = test_bid(2);

        let mut events = vec![
            node_update(stub, vec![NodeKey::Bid { bid: stub }]),
            node_update(claimant, vec![NodeKey::Bid { bid: stub }]),
        ];

        resolve_one_batch(&mut events, &store).await;

        let surviving_updates: Vec<Bid> = events
            .iter()
            .filter_map(|e| match e {
                BeliefEvent::NodeUpdate(_, n, _) => Some(n.bid),
                _ => None,
            })
            .collect();
        assert_eq!(
            surviving_updates,
            vec![claimant],
            "the absorbed stub's own NodeUpdate must not survive to re-insert it \
             after the removal; got {surviving_updates:?}"
        );
    }

    /// An in-batch edge between the two absorbed-together nodes becomes a
    /// self-loop once re-pointed, and must be dropped rather than emitted.
    ///
    /// `relations` is `UNIQUE(sink, source)` and a self-edge is meaningless to
    /// either sink.
    #[tokio::test]
    async fn in_batch_self_loops_are_dropped() {
        let store = CountingStore::new();
        let stub = test_bid(1);
        let claimant = test_bid(2);

        let mut events = vec![
            node_update(stub, vec![NodeKey::Bid { bid: stub }]),
            node_update(claimant, vec![NodeKey::Bid { bid: stub }]),
            BeliefEvent::RelationUpdate(
                claimant,
                stub,
                crate::properties::WeightSet::default(),
                EventOrigin::Remote,
            ),
        ];

        resolve_one_batch(&mut events, &store).await;

        assert!(
            !events
                .iter()
                .any(|e| matches!(e, BeliefEvent::RelationUpdate(..))),
            "an edge between claimant and absorbed node collapses to a self-loop \
             and must be dropped; got {events:?}"
        );
    }

    /// An absorbed BID stays absorbed in later batches.
    ///
    /// Regression, and the reason the absorption map is accumulator state rather
    /// than a per-batch local. An href stub's BID is UUID v5 of the URL, so a
    /// second document citing a URL an earlier epoch already resolved re-mints
    /// the *identical* BID. That later batch carries no merge key — the claiming
    /// document was parsed in the earlier epoch — so batch-local resolution has
    /// nothing to match on and the stub silently returns, restoring the duplicate
    /// path the first batch removed.
    ///
    /// Measured on one corpus: 17 of 41 duplicated URLs were cited by exactly two
    /// files and survived for precisely this reason.
    #[tokio::test]
    async fn absorption_carries_over_to_later_batches() {
        let store = CountingStore::new();
        let stub = test_bid(1);
        let claimant = test_bid(2);
        let neighbour = test_bid(3);
        let mut absorbed = BTreeMap::new();

        // Epoch 1: the claiming document absorbs the stub.
        let mut batch1 = vec![
            node_update(stub, vec![NodeKey::Bid { bid: stub }]),
            node_update(claimant, vec![NodeKey::Bid { bid: stub }]),
        ];
        resolve_merge_keys(&mut batch1, &store, &mut absorbed).await;
        assert_eq!(renames_in(&batch1), vec![(stub, claimant)]);

        // Epoch 2: a second document cites the same URL. `ensure_href_entry`
        // re-mints the same deterministic BID and draws an edge from it. No
        // merge key is present anywhere in this batch.
        let mut batch2 = vec![
            node_update(stub, vec![NodeKey::Bid { bid: stub }]),
            BeliefEvent::RelationUpdate(
                stub,
                neighbour,
                crate::properties::WeightSet::default(),
                EventOrigin::Remote,
            ),
        ];
        resolve_merge_keys(&mut batch2, &store, &mut absorbed).await;

        assert!(
            !batch2
                .iter()
                .any(|e| matches!(e, BeliefEvent::NodeUpdate(_, n, _) if n.bid == stub)),
            "the re-minted stub must not be re-inserted; got {batch2:?}"
        );
        let edges: Vec<(Bid, Bid)> = batch2
            .iter()
            .filter_map(|e| match e {
                BeliefEvent::RelationUpdate(s, k, _, _) => Some((*s, *k)),
                _ => None,
            })
            .collect();
        assert_eq!(
            edges,
            vec![(claimant, neighbour)],
            "the second document's edge should attach to the surviving claimant"
        );
    }

    /// The accumulator's absorption and `insert_state`'s must not fight.
    ///
    /// `BeliefBase::insert_state` performs the *same* absorption independently
    /// when it sees the merge keys, so with `BeliefBase` as the sink both run.
    /// This is a real configuration (`cli.rs`, non-`service` builds). Pins that
    /// the doubled work converges: stub gone, claimant present, its edge intact.
    #[tokio::test]
    async fn absorption_agrees_with_insert_state_on_a_beliefbase_sink() {
        use crate::beliefbase::BeliefBase;

        let stub = test_bid(1);
        let claimant = test_bid(2);
        let neighbour = test_bid(3);

        // A non-empty WeightSet: an empty one means "remove this edge" to both
        // sinks, so it would not exercise re-pointing at all.
        let mut ws = crate::properties::WeightSet::default();
        ws.set(
            crate::properties::WeightKind::Section,
            crate::properties::Weight::default(),
        );

        let mut events = vec![
            node_update(stub, vec![NodeKey::Bid { bid: stub }]),
            node_update(neighbour, vec![NodeKey::Bid { bid: neighbour }]),
            node_update(claimant, vec![NodeKey::Bid { bid: stub }]),
            BeliefEvent::RelationUpdate(stub, neighbour, ws, EventOrigin::Remote),
        ];

        let mut base = BeliefBase::empty();
        resolve_one_batch(&mut events, &base).await;
        prepare_batch(&mut events);
        base.apply_batch(&events).await.unwrap();

        assert!(
            !base.states().contains_key(&stub),
            "the absorbed stub must not survive in a BeliefBase sink"
        );
        assert!(
            base.states().contains_key(&claimant),
            "the claimant must survive"
        );
        let has_edge = base
            .bid_to_index(&claimant)
            .zip(base.bid_to_index(&neighbour))
            .and_then(|(s, k)| base.relations().as_graph().find_edge(s, k))
            .is_some();
        assert!(
            has_edge,
            "the re-pointed edge should attach the claimant to the neighbour"
        );
    }

    /// Both sinks must agree on the weights of a colliding edge.
    ///
    /// This is the assertion whose absence let the sinks diverge: in-memory
    /// `replace_bid` unioned the two `WeightSet`s while SQL `rename_node` kept
    /// the claimant's and discarded the absorbed node's, because weights are
    /// serialized TEXT there. `expand_renames` now computes the union once,
    /// upstream, so both sinks apply the same edge events.
    ///
    /// Drives one `BeliefBase` through the full pipeline and asserts the merged
    /// edge carries **both** weight kinds -- the Section kind the claimant already
    /// had, and the Epistemic kind that existed only on the absorbed node's edge.
    #[tokio::test]
    async fn absorption_unions_weights_of_a_colliding_edge() {
        use crate::beliefbase::BeliefBase;
        use crate::properties::{Weight, WeightKind, WeightSet};

        let stub = test_bid(1);
        let claimant = test_bid(2);
        let neighbour = test_bid(3);

        // The stub's edge carries a kind the claimant's edge does not. A union
        // keeps it; a discard loses it.
        let mut stub_ws = WeightSet::default();
        stub_ws.set(WeightKind::Epistemic, Weight::default());
        let mut claim_ws = WeightSet::default();
        claim_ws.set(WeightKind::Section, Weight::default());

        let mut events = vec![
            node_update(stub, vec![NodeKey::Bid { bid: stub }]),
            node_update(neighbour, vec![NodeKey::Bid { bid: neighbour }]),
            node_update(claimant, vec![NodeKey::Bid { bid: stub }]),
            BeliefEvent::RelationUpdate(stub, neighbour, stub_ws, EventOrigin::Remote),
            BeliefEvent::RelationUpdate(claimant, neighbour, claim_ws, EventOrigin::Remote),
        ];

        let mut base = BeliefBase::empty();
        resolve_one_batch(&mut events, &base).await;
        prepare_batch(&mut events);
        base.apply_batch(&events).await.unwrap();

        let ws = base
            .bid_to_index(&claimant)
            .zip(base.bid_to_index(&neighbour))
            .and_then(|(s, k)| base.relations().as_graph().find_edge(s, k))
            .and_then(|e| base.relations().as_graph().edge_weight(e).cloned())
            .expect("claimant should hold the merged edge");

        assert!(
            ws.get(&WeightKind::Section).is_some(),
            "the claimant's own weight kind must survive; got {ws:?}"
        );
        assert!(
            ws.get(&WeightKind::Epistemic).is_some(),
            "the absorbed node's weight kind must be unioned in, not discarded; \
             got {ws:?}"
        );
    }

    /// The expansion must supersede the batch's own edge event, not duplicate it.
    ///
    /// Both the in-batch rewrite and `expand_renames` have an opinion about an
    /// edge naming a renamed node. If both are emitted the sink sees the same
    /// pair twice, and a stale copy can win. Asserts exactly one
    /// `RelationUpdate` survives per pair.
    ///
    /// Regression: the first attempt at expansion deduped batch-wide rather than
    /// by superseded pair, which dropped unrelated events and drove corpus
    /// duplicate warnings from 34 to 187.
    #[tokio::test]
    async fn expansion_supersedes_rather_than_duplicates_batch_edges() {
        use crate::properties::{Weight, WeightKind, WeightSet};

        let store = CountingStore::new();
        let stub = test_bid(1);
        let claimant = test_bid(2);
        let neighbour = test_bid(3);
        let unrelated_a = test_bid(4);
        let unrelated_b = test_bid(5);

        let mut ws = WeightSet::default();
        ws.set(WeightKind::Section, Weight::default());

        let mut events = vec![
            node_update(stub, vec![NodeKey::Bid { bid: stub }]),
            node_update(claimant, vec![NodeKey::Bid { bid: stub }]),
            BeliefEvent::RelationUpdate(stub, neighbour, ws.clone(), EventOrigin::Remote),
            // Touches neither endpoint of the rename; must pass through intact.
            BeliefEvent::RelationUpdate(unrelated_a, unrelated_b, ws, EventOrigin::Remote),
        ];

        resolve_one_batch(&mut events, &store).await;

        let updates: Vec<(Bid, Bid)> = events
            .iter()
            .filter_map(|e| match e {
                BeliefEvent::RelationUpdate(s, k, _, _) => Some((*s, *k)),
                _ => None,
            })
            .collect();

        assert_eq!(
            updates
                .iter()
                .filter(|(s, k)| *s == claimant && *k == neighbour)
                .count(),
            1,
            "exactly one RelationUpdate for the re-pointed pair; got {updates:?}"
        );
        assert!(
            !updates.iter().any(|(s, k)| *s == stub || *k == stub),
            "no surviving event may still name the absorbed stub; got {updates:?}"
        );
        assert!(
            updates.contains(&(unrelated_a, unrelated_b)),
            "an event unrelated to the rename must survive untouched; got {updates:?}"
        );
    }

    /// Absorption must survive `prepare_batch`'s reordering: the removal has to
    /// land ahead of the `NodeUpdate` that claims the path, or the claim is
    /// written first and then deleted along with the stub.
    #[tokio::test]
    async fn absorption_events_are_ordered_removal_first() {
        let store = CountingStore::new();
        let stub = test_bid(1);
        let claimant = test_bid(2);

        let mut events = vec![
            node_update(stub, vec![NodeKey::Bid { bid: stub }]),
            node_update(claimant, vec![NodeKey::Bid { bid: stub }]),
        ];
        resolve_one_batch(&mut events, &store).await;
        prepare_batch(&mut events);

        let removal_idx = events
            .iter()
            .position(|e| matches!(e, BeliefEvent::NodesRemoved(..)))
            .expect("a consolidated NodesRemoved should be present");
        let claim_idx = events
            .iter()
            .position(|e| matches!(e, BeliefEvent::NodeUpdate(_, n, _) if n.bid == claimant))
            .expect("the claiming NodeUpdate should still be present");

        assert!(
            removal_idx < claim_idx,
            "removal (idx {removal_idx}) must precede the claim (idx {claim_idx})"
        );
    }
}
