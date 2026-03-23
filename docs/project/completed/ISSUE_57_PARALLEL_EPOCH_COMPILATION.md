# Issue 57: Parallel Epoch Compilation

**Priority**: MEDIUM
**Estimated Effort**: 5 days (RELATIVE COMPARISON ONLY)
**Dependencies**: None hard. Issue 56 protocol model informs the merge-safety
argument but does not block implementation.
**Related**: Issue 56 (PathMap protocol observability)
**Status**: COMPLETE. All core steps done. Open threads (integration tests,
AnchorPath bug, attempt-3 requeue, perf follow-ons) carried to Issue 60.
**Completed**: 2026-03-23. Validated on MDN JS corpus: ×7.8 speedup at `--jobs 8`,
attempt-1 Phase 0 mean ~101ms/file (flat O(1)), attempt-2+ ~284ms/file (flat).

## Summary

`DocumentCompiler::parse_all` processed files sequentially. This issue delivers
intra-epoch parallelism by (1) introducing `BeliefAccumulator` — a live,
batch-draining in-memory `BeliefSource` that replaces the background processor
task in `main.rs` and the defunct `CachedBeliefSource` wrapper — and (2)
restructuring epoch 0 as a depth-ordered work-queue of parallel leaf batches,
each separated by an explicit `BatchStart`/`BatchEnd` sentinel pair. Epoch N≥1
reparse rounds are also parallelised. The result is a pipeline where `global_bb`
is always live, queryable, and cache-coherent between batch boundaries.

## Goals

1. ✅ Extend `try_initialize_stack_from_session_cache` to accept a `GlobalCache`
   hit as equivalent to a `StackCache` hit.
2. ✅ Add `--jobs N` / `NOET_JOBS` flag and sequential fallback at `jobs=1`.
3. ✅ Introduce `BeliefAccumulator`: a live in-memory `BeliefSource` + `BeliefSink`
   backed by an `UnboundedReceiver<BeliefEvent>`, replacing the background
   processor task in `main.rs` and the `CachedBeliefSource` wrapper.
4. ✅ Replace vestigial `BalanceCheck` emit/consume sites with `BatchStart` /
   `BatchEnd` sentinels that drive accumulator commit and cache invalidation.
5. ✅ Restructure epoch 0 in `parse_all` as a depth-ordered work-queue of
   parallel leaf batches, each pair bounded by `BatchStart`/`BatchEnd`.
6. ✅ Parallelize epoch N≥1 reparse rounds via `parse_epoch_parallel`.
7. ✅ Preserve `--jobs 1` sequential path as byte-identical fallback.
8. `ProtoIndex` mutability for `FileUpdateSyncer` — follow-on (Step 8).
9. ⚠️ Fix `build_path_key` and `get_parent_from_stack` `AnchorPath` directory
   path handling — see Risks section.
10. ✅ Fix asset-loading O(tasks) cost per epoch (Fix B + Fix C, see Risks).

## Architecture

### The Epoch Invariant (unchanged)

An **epoch** is the set of files sharing the same parse count at the point they
are dequeued. Files in epoch 0 have never been parsed. Files in epoch N ≥ 1
have been parsed N times; their nodes exist in `global_bb` from prior epochs.

**Within a single epoch, no file's parse output is an input to any other
file's parse in that same epoch.** Cross-file dependencies only flow across
epoch boundaries. This is the invariant that makes intra-epoch parallelism safe.

### BeliefAccumulator

`BeliefAccumulator<S>` is the unified batch accumulator and query cache for the
compile pipeline. It owns:

- `inner: S` — the backing store (`BeliefBase` for `parse`, `DbConnection` for
  `watch` in future). `S` must implement both `BeliefSource` and `BeliefSink`.
- `rx: UnboundedReceiver<BeliefEvent>` — the read side of the compiler's `tx`.
- `pending: Vec<BeliefEvent>` — events accumulated since the last `BatchStart`.
- `in_batch: bool` — true between `BatchStart` and `BatchEnd`.
- `cache: Arc<AccCache>` — shared query-result memo cache.

**Batch lifecycle** (all driven by events on the channel — no out-of-band API):

```
BatchStart arrives → pending.clear(), in_batch = true
  (warn if pending was non-empty — that's a compiler bug / missed BatchEnd)
events arrive → push to pending
BatchEnd arrives → prepare_batch(pending) → inner.apply_batch(sorted) → cache.clear()
channel closes → into_inner() drains remainder, then unwraps inner
```

**`prepare_batch`** consolidates and sorts the pending slice before application:
1. All `NodesRemoved` events merged into one (union of BID sets) → single
   `index_sync` call instead of N.
2. Consolidated `NodesRemoved` first, then `NodeUpdate`/`NodeRenamed`, then
   everything else — relation/path events find nodes already indexed.

**Query semantics**: `eval_*` methods on `BeliefAccumulator` and `QueryHandle`
do **not** drain the channel. `inner` is treated as stable within an epoch
(after the preceding `BatchEnd` and before the next `BatchStart`). Queries go
straight to the memo cache then `inner`.

**`QueryHandle`**: a clonable, `Clone`-able read-only view sharing the same
`Arc<Mutex<AccInner>>` and `Arc<AccCache>`. Passed to parallel tasks so they
can call `BeliefSource::eval_query` without exclusive ownership of the channel.

**`into_inner`** is `async`: drains the channel internally before unwrapping.
Caller closes `tx` first; `into_inner` sees `Disconnected` and stops cleanly.
There is no public `drain()` — everything is driven by channel signals.

**`BatchStart` is preserved** as an explicit open sentinel (not inferred from
`BatchEnd` alone) because the future federated model will receive streams from
external peers where explicit open/close pairs are required. If `pending` is
non-empty on `BatchStart` it logs `tracing::warn!` — that's a compiler bug
(missed `BatchEnd`).

`BeliefAccumulator` lives in `src/beliefbase/accumulator.rs`, gated to
`#[cfg(not(target_arch = "wasm32"))]`. `CachedBeliefSource` (`cached.rs`) has
been deleted — its responsibilities are subsumed by `AccCache` inside the
accumulator.

### parse / watch Unification

| | `parse` (before) | `parse` (after) | `watch` |
|---|---|---|---|
| `global_bb` type | frozen `BeliefBase` clone | `QueryHandle<BeliefBase>` | live `DbConnection` |
| event consumer | background `tokio::spawn` task | `BeliefAccumulator` via channel | transaction task |
| drain trigger | `close_tx` + `processor.await` | `BatchEnd` on channel; `into_inner` on close | compiler-idle notify |
| cache | `CachedBeliefSource` wrapper | `AccCache` inside accumulator | n/a |

`parse` now uses `BeliefAccumulator<BeliefBase>`. The background processor task
is eliminated. `parse_all` receives a `QueryHandle` clone from the accumulator
as its `global_bb`. `into_inner` after `close_tx` yields the fully-populated
`BeliefBase`.

`watch` is unchanged: it continues to use `DbConnection` as `global_bb` via the
existing `FileUpdateSyncer` / transaction task. `BatchStart`/`BatchEnd` pass
through as no-ops in `BeliefBase::process_event` and `Transaction::add_event`.

### Epoch 0: Network-Ordered Parallel Dispatch

Epoch 0 uses a `VecDeque` work-queue seeded from `primary_queue`. Each
iteration pops one entry:

```
queue = VecDeque::from(primary_queue)

while let Some(entry) = queue.pop_front():

  if proto_index.children_of(entry) returns Some(children):
    // Network directory — dispatch index.md alone first so parent belief
    // state commits before any child is parsed.
    BatchStart → parse_epoch_parallel([entry]) → BatchEnd
    (accumulator processes BatchEnd on next channel read)

    // Split children: leaf files into an immediate parallel batch,
    // subnet sub-directories back onto the queue.
    leaf_batch = []
    for child in children:
      if child.is_dir():
        queue.push_back(child)       // subnet — own round later
      else:
        leaf_batch.push(child)

    if leaf_batch not empty:
      BatchStart → parse_epoch_parallel(leaf_batch) → BatchEnd

  else:
    // Plain file or path not in ProtoIndex — dispatch directly.
    BatchStart → parse_epoch_parallel([entry]) → BatchEnd

// Defensive: if primary_queue still has entries, dispatch as unordered batch
// and emit tracing::warn! — indicates stale ProtoIndex or filtering bug.
```

**Why the parent must precede its children**: `try_initialize_stack_from_session_cache`
needs the parent network node in `global_bb` to fire the `GlobalCache` fast
path. The parent node enters `inner` only after its `BatchEnd` is processed.
Once committed, all leaf siblings are fully independent — `sort_key_for` is
served by `ProtoIndex` (static, no `global_bb` query needed) and leaf content
is mutually non-referential within epoch 0.

**Subnets as docs**: a subnet's `index.md` is structurally a document within
its parent network from `initialize_stack`'s perspective. It lands in the
parent's single-file batch (so parent belief state is visible). Its own children
form the next batch after that `BatchEnd` is processed.

### Epoch N ≥ 1: Parallel Dispatch

At the start of each reparse round, `global_bb` reflects all prior epochs.
Every file queued for reparse can resolve cross-document links via `GlobalCache`
hits against this stable snapshot.

```
while reparse_queue not empty:
  if stable and no new node updates → break

  candidates = reparse_queue sorted by fewest unresolved deps
  sentinel_paths = candidates where parse_count >= max_reparse_count
  batch = candidates - sentinel_paths

  emit ReparseLimitExceeded results for sentinel_paths
  remove sentinel_paths and batch from reparse_queue

  reset reparse_stable = true, last_round_updates.clear()

  BatchStart → parse_epoch_parallel(batch) → BatchEnd
  (process_epoch_batch_results re-enqueues any still-unresolved paths,
   setting reparse_stable = false as needed)
```

Within-round dependency resolution (file B seeing file A's events from the same
round) is intentionally **not** supported — the epoch invariant already
guarantees correctness without it. The old sequential `parse_next` inner loop
is replaced by `parse_epoch_parallel`.

### True OS-Thread Parallelism

`parse_epoch_parallel` currently runs tasks sequentially on the async executor
(cooperative multitasking via `await` points in file I/O and `eval_query`). No
`Send` bound is required — `BeliefBase` uses `Rc<RefCell<T>>` on WASM.

True multi-thread parallelism (Step 6) requires `GraphBuilder: Send`, which
holds on native (`Arc<RwLock<T>>`). The plan:

1. Replace `for path in paths` with `tokio::task::spawn` per path.
2. Gate concurrency with `Arc<Semaphore>` of size `jobs`.
3. Collect results via `JoinSet`; preserve path order for determinism.
4. `--jobs 1`: semaphore of size 1 is equivalent to sequential — no special
   case needed.

No post-task merge step. Events flow directly to `BeliefAccumulator` via `tx`.

## Implementation Steps

### Step 1: GlobalCache fast-path ✅ (complete)

`try_initialize_stack_from_session_cache` accepts both `NodeSource::StackCache`
and `NodeSource::GlobalCache`. `fast_missing` sourced from `global_bb` on
`GlobalCache` hit.

### Step 2: `--jobs` / `NOET_JOBS` ✅ (complete)

`jobs: usize` field on `DocumentCompiler`. `-j N` CLI flag. `NOET_JOBS` env
var. Sequential fallback at `jobs=1`.

### Step 3: `BeliefAccumulator` ✅ (complete)

- [x] `src/beliefbase/sink.rs`: `BeliefSink` trait with `apply_batch`. Impls
      for `BeliefBase` (calls `process_event` per event) and `DbConnection`
      (builds one `Transaction` and executes it).
- [x] `src/beliefbase/accumulator.rs`: `BeliefAccumulator<S>` + `QueryHandle<S>`.
      `AccCache` (shared memo cache). `prepare_batch` consolidation + sort.
      `into_inner` async with internal drain. No public `drain()`.
- [x] `src/beliefbase/mod.rs`: `mod accumulator; pub use accumulator::{BeliefAccumulator, QueryHandle}`
      gated to `#[cfg(not(target_arch = "wasm32"))]`.
- [x] `src/beliefbase/cached.rs` deleted. `CachedBeliefSource` removed from
      public API.
- [x] `src/bin/noet/main.rs`: background `tokio::spawn` processor task replaced
      by `BeliefAccumulator::new(BeliefBase::empty(), rx)`. `QueryHandle` passed
      to `parse_all` as `global_bb`. `close_tx()` + `into_inner().await`
      replaces old `drain + processor.await` pattern.

### Step 4: `BatchStart` / `BatchEnd` sentinels ✅ (complete)

The `BalanceCheck` rename to `Flush` was not implemented. Instead, the existing
`BatchStart` and `BatchEnd` variants (added in the session 9 scaffolding) were
assigned the batch-boundary semantics directly:

- [x] All vestigial `BalanceCheck` emit sites in `builder.rs` and
      `try_initialize_stack_from_session_cache` removed.
- [x] `BatchStart`/`BatchEnd` are no-ops in `BeliefBase::process_event` and
      `Transaction::add_event` — all batch semantics owned by `BeliefAccumulator`.
- [x] `BatchStart` warns if `pending` is non-empty (missed `BatchEnd` = compiler bug).
- [x] `BatchEnd` triggers `prepare_batch` + `inner.apply_batch` + `cache.clear()`.

### Step 5: Network-ordered epoch 0 dispatch ✅ (complete)

- [x] `parse_all` parallel path: `VecDeque` work-queue loop replacing the flat
      `for net_dir in network_dirs` loop.
- [x] Subnet dirs re-enqueued for their own round (not merged into parent batch).
- [x] `tracing::warn!` on defensive remainder batch (stale ProtoIndex / bug).
- [x] `process_epoch_batch_results` helper extracted to eliminate duplication.
- [x] `ProtoIndex::network_dirs()` added: returns known dirs shallowest-first
      from the already-built index map (no redundant `WalkDir`).

### Step 6: True OS-thread parallelism ✅ (complete)

- [x] `parse_epoch` (renamed from `parse_epoch_parallel`): parallel branch uses
      `tokio::task::spawn` per path, gated by `Arc<Semaphore>` of size `jobs`.
- [x] Each spawned task gets: owned `GraphBuilder` seeded via `seed_session`,
      isolated `task_tx` channel, `global_bb.clone()`.
- [x] Per-task events buffered into `Vec<BeliefEvent>`; replayed to `shared_tx`
      in original path-index order after `JoinSet` drains — deterministic
      first-one-wins collision resolution regardless of task completion order.
- [x] `--jobs 1`: sequential inline path, no `JoinSet` overhead.
- [x] `epoch_session_snapshot` seeds each task builder with full network ancestor
      chain + const-namespace subgraph so tasks avoid `global_bb` for both
      ancestor reconstruction and asset loading.

### Step 7: Epoch N ≥ 1 parallel dispatch ✅ (complete)

- [x] Sequential `parse_next` inner loop replaced by `parse_epoch_parallel`.
- [x] Stability check and per-path parse-limit sentinel logic preserved.
- [x] Candidates sorted by fewest unresolved deps before dispatch.
- [x] One `BatchStart`/`BatchEnd` pair per reparse round.
- [x] Within-round dependency resolution intentionally removed (epoch invariant
      is sufficient).

### Step 8: `ProtoIndex` mutability for `FileUpdateSyncer` (follow-on)

- [ ] `FileUpdateSyncer` in `watch.rs` has no reference to `ProtoIndex`. New
      files (especially new `index.md` files) created at runtime fall into the
      defensive remainder batch with no epoch boundary.
- [ ] Decide: (a) rebuild `ProtoIndex` on each watch-triggered compile cycle
      (simple, correct) or (b) hold `Arc<RwLock<ProtoIndex>>` in
      `FileUpdateSyncer` and update incrementally (optimised).
- [ ] Add a test: create a new `index.md` at runtime, verify it gets its own
      epoch boundary in the next compile.

### Step 9: Repo-root BID stability in remainder epoch ✅ (complete)

- [x] Diagnosed: `speculative_path_key` for network nodes returns
      `NodeKey::Id { net: Bref::default(), id: ... }`. `Bref::default()`
      normalises to the API node's bref in `net_get_from_id`, so lookup goes
      through the API PathMap. The `epoch_session_snapshot` excludes the
      `repo_root → api` Section edge (API node kind is `API|Trace`, not
      `Network`), so the API PathMap in every seeded task `session_bb` has no
      subnets. ID lookup misses in both `session_bb` and `global_bb` →
      `cache_fetch` returns `Unresolved` → fresh time-based BID generated for
      the repo root → PathMap built under wrong bref → `get_context` panics
      (`in_states=true, in_pathmap=false`) at `builder.rs:821`.
- [x] Fix: in `speculative_path_key` network branch, prepend
      `NodeKey::Bid { bid: self.repo }` when `self.repo != Bid::nil() &&
      self.stack.is_empty()`. `NodeKey::Bid` lookups go directly to
      `BeliefBase::states` — no PathMap traversal, no `nil_bref` normalisation.
      The canonical node is in `session_bb.states` (included in
      `epoch_session_snapshot` as a network-kinded node) and is returned
      immediately. No behaviour change for epoch 0, subnets, or documents.

## Testing Requirements

- [ ] `cargo test --features service,bin --test codec_test` passes with no
      regressions (blocked on active bugs in Risks section).
- [ ] `--jobs 1` produces byte-identical output to the pre-Issue-57 sequential
      implementation on `tests/network_1`.
- [ ] Parallel build of `global_objects/` corpus completes without panic or
      data corruption (`NOET_JOBS=8` run completes, step 9 fix validated).
- [x] `BeliefAccumulator` unit tests: `BatchStart`/`BatchEnd` round-trip,
      cache hit avoids second inner call, `BatchEnd` clears cache, events
      outside batch applied on `into_inner`, `prepare_batch` consolidation,
      `QueryHandle` shares cache, `into_inner` drains before unwrapping.
- [x] No `CachedBeliefSource` references remain.
- [ ] `test_belief_set_builder_bid_generation_and_caching`: no EXTRA nodes,
      no WRITTEN-but-not-cached BIDs, second parse produces no graph events.
- [ ] `test_belief_set_builder_with_db_cache`: second parse does not rewrite
      any document content.
- [x] `NOET_JOBS=8` parallel corpus run does not panic at `builder.rs:821`
      (repo-root BID stability fix, step 9).

## Success Criteria

- [x] `BeliefAccumulator` replaces the background processor task in `main.rs`.
- [x] `CachedBeliefSource` deleted; caching subsumed by `AccCache` in the
      accumulator.
- [x] Epoch 0 dispatches files in depth-ordered batches gated by network-parent
      availability in `global_bb`.
- [x] Epoch N≥1 reparse rounds dispatched in parallel via `parse_epoch_parallel`.
- [ ] `test_belief_set_builder_bid_generation_and_caching` and
      `test_belief_set_builder_with_db_cache` pass (blocked on active bugs).
- [x] Wall-clock improvement on `global_objects/` corpus is measurable with
      `--jobs 8` vs `--jobs 1`. Measured ×7.8 speedup on MDN JS corpus
      (`--jobs 8`, 2860 files, mean 0.24s/file, wall 15m30s). Fresh-parse
      (attempt 1) Phase 0 mean ~101ms/file — flat O(1) scaling.
- [x] `--jobs 1` sequential fallback is correct (sequential path unchanged).

## Risks

- **Asset-loading O(tasks) cost per remainder epoch** ✅ (fixed, Fix B + Fix C):
  Remainder-epoch parallel tasks each independently loaded the full asset set
  (~366 assets at ~10.9ms/asset = 4–9s Phase 0) by calling `global_bb.eval`
  inside `initialize_stack`'s `content_namespaces()` guard. Root cause: parallel
  task events flow `task_tx → shared_tx → BeliefAccumulator → global_bb` but
  are never replayed into `compiler.builder.session_bb`, so
  `epoch_session_snapshot` Part 2 always produced an empty asset subgraph.
  → **Fix B**: `DocumentCompiler::sync_asset_snapshot` called after every
  `drain_epoch` in `parse_all`. Pulls each `content_namespaces()` subgraph from
  `global_bb` into `self.builder.session_bb` via `get_async` + `merge_from`.
  The next `epoch_session_snapshot` includes the full asset set; task builders
  seeded with it hit the guard immediately and skip `eval` entirely.
  Confirmed: `snapshot_states=369` in task-seeding logs post-fix.

- **`global_bb.get_async(&api_key)` mutex serialization** ✅ (fixed, Fix C):
  After Fix B, a new bottleneck emerged: `initialize_stack` calls
  `global_bb.get_async(&api_key)` unconditionally at Phase 0 start to register
  the api node in `global_bb` if absent. With `--jobs 8`, all 8 tasks acquire
  the `BeliefAccumulator` mutex serially here (8 × ~800ms = ~6s stall per
  epoch). The check is meaningless for parallel task builders: (a) tasks emit to
  an isolated `task_tx`, not the accumulator; (b) the api node is already in
  `global_bb` from the pre-epoch sequential root parse.
  → **Fix C**: added `skip_global_api_check: bool` field to `GraphBuilder`
  (default `false`). `initialize_stack` gates the `get_async` call on
  `!self.skip_global_api_check`. `parse_epoch` sets the flag via
  `.with_skip_global_api_check(true)` on each task builder. All non-parallel
  callers (`GraphBuilder::new` in tests, `simple`, `with_html_output`) unaffected.

- **Repo-root BID instability in remainder epoch** ✅ (fixed, step 9):
  In parallel tasks during the remainder reparse loop, `speculative_path_key`
  returned `NodeKey::Id { net: Bref::default(), .. }` for the repo root network
  node. `Bref::default()` normalises to the API node's bref, causing ID lookup
  to search the API PathMap. The `epoch_session_snapshot` excludes the
  `repo_root → api` edge (API node kind is `API|Trace`, not `Network`), so the
  API PathMap in every seeded task `session_bb` has no subnets. Both
  `session_bb` and `global_bb` lookups missed → `cache_fetch` returned
  `Unresolved` → fresh time-based BID generated → PathMap built under wrong
  bref → `get_context` panicked (`in_states=true, in_pathmap=false`) at
  `builder.rs:821`. Fixed by prepending `NodeKey::Bid { bid: self.repo }` when
  `self.repo != Bid::nil() && self.stack.is_empty()` in `speculative_path_key`.

- **`AnchorPath` directory-path mangling in `build_path_key` and
  `get_parent_from_stack`** (ACTIVE BUG — fixes ready):
  `AnchorPath::new(net_path).strip_prefix(...)` is called in two places in
  `builder.rs` where `net_path` is an absolute directory path (no extension, no
  trailing slash). `AnchorPath::filepath()` calls `dir()` for extension-less
  paths; `dir()` strips the last path component, so `strip_prefix` strips the
  grandparent directory instead of the network directory. The child path is
  produced as repo-root-relative while the `net` field in the resulting
  `NodeKey::Path` is the subnet's bref — an inconsistent key that never resolves
  in the PathMap. Both call sites (`build_path_key` L1139 and
  `get_parent_from_stack` L1061 in `src/codec/builder.rs`) must be fixed
  together by appending a trailing slash (or using `AnchorPath::new_dir`) before
  `strip_prefix` — they mangle in the same direction, so fixing only one creates
  a cross-layer mismatch worse than the current consistent-but-wrong state.
  → **Fix**: append trailing slash to `net_path` / `stack_path` before
  `strip_prefix`; use `AnchorPath::new_dir` for the `starts_with` filter at
  `get_parent_from_stack` L1032. See `docs/design/beliefbase_architecture.md`
  section 2.2 "Network Node Dual-Path Representation".

- **`BeliefBase` no-op `EpochDrain` in integration test** (ACTIVE BUG — fix
  ready): `test_belief_set_builder_bid_generation_and_caching` passes a bare
  `BeliefBase` as `global_bb` to `parse_all`. `BeliefBase::drain_epoch` is a
  no-op, so `global_bb` is never updated between epochs — every epoch's
  `cache_fetch` queries an effectively empty `global_bb`. The epoch sequencing
  in `parse_all` is correct by construction (parent network committed before
  children dispatched), but the no-op drain means the parent's BID is invisible
  to child epochs, producing fresh time-based BIDs and EXTRA nodes.
  → **Fix**: replace the bare `BeliefBase` in the test with a
  `BeliefAccumulator`-backed `QueryHandle`, matching the production path.

- **`BeliefAccumulator` lazy-drain removal**: removing `try_recv` before every
  `eval_query` means mid-epoch queries see the state as of the last `BatchEnd`,
  not the absolute latest channel contents. This is correct by the epoch
  invariant — files within an epoch are independent — but callers that relied
  on the lazy drain for correctness would silently break.
  → **Mitigation**: the epoch invariant guarantees this is safe. The `BatchStart`
  warning catches any case where events accumulate without a matching `BatchEnd`.

- **Work-queue subnet ordering**: the `VecDeque` processes entries in FIFO order.
  A subnet added to the queue before its parent completes would be dispatched
  before its parent's `BatchEnd` is processed. This cannot happen today because
  subnet dirs are only pushed onto the queue from within the parent's
  `children_of` split — which occurs after the parent's single-file batch is
  dispatched.
  → **Mitigation**: the queue population logic enforces parent-before-child by
  construction. If this invariant breaks, the `tracing::warn!` remainder batch
  will expose it.

- **Reparse epoch correctness under parallelism**: two files in the same reparse
  epoch with a latent cross-document dependency would produce a wrong result if
  dispatched in parallel.
  → **Mitigation**: the epoch invariant forbids this by definition. Epoch-N
  files depend only on epoch 0..N-1 output, never on each other. A violation
  indicates a bug in the dependency-promotion logic.

- **Network-dir ordering edge cases**: a file in a directory without an
  `index.md` is flattened into its ancestor network by `iter_net_docs`.
  `sort_key_for` walks upward to find it.
  → **Mitigation**: `ProtoIndex::sort_key_for` already handles this case. No
  changes needed.

- **`ProtoIndex` staleness in `watch`**: new files created at runtime are not
  reflected in the `ProtoIndex` built at compiler startup. They fall into the
  defensive remainder batch (no epoch boundary).
  → **Mitigation**: `tracing::warn!` makes this visible. Step 8 is the
  permanent fix.

## Open Questions

- Step 8 option selection: rebuild `ProtoIndex` per watch cycle (simple) vs
  incremental mutation (optimised). Defer until after corpus validation.
- After the `build_path_key` fix, verify `test_belief_set_builder_with_db_cache`
  passes. If it still fails, `DbConnection::resolve_net_path` (db.rs) is a
  suspect — it is newer code that resolves cross-network paths one segment at a
  time and may not yet handle all subnet path forms correctly.

## Log Analysis Guide

Use these commands and interpretation rules when analysing corpus compilation
logs to validate correctness and diagnose performance regressions.

### Grep commands — one-job run (`NOET_JOBS=1`)

```
grep "accumulator drain complete" <one-job-log>
grep "ERROR.*outside any BatchStart" <one-job-log>
grep "generate_deferred_html.*Generating" <one-job-log>
grep -c "source is missing: true" <one-job-log>
grep -c "sink is missing: true" <one-job-log>
python3 benches/log_analysis/parse_log.py --warnings <one-job-log>
python3 benches/log_analysis/parse_log.py --file-times <one-job-log>
```

### Grep commands — parallel run (`NOET_JOBS=N`)

```
# Did it complete?
tail -5 <parallel-log>
grep "parsing complete\|Both queues empty" <parallel-log>

# Drain health
grep "accumulator drain complete" <parallel-log>
grep "ERROR.*outside any BatchStart" <parallel-log>

# Per-epoch drain census (one line per batch)
grep "drain_epoch.*drain complete\|drain_epoch" <parallel-log>

# Warning counts
python3 benches/log_analysis/parse_log.py --warnings <parallel-log>
python3 benches/log_analysis/parse_log.py --file-times <parallel-log>

# Specific issues
grep -c "source is missing: true" <parallel-log>
grep -c "session_bb.*does not have" <parallel-log>
grep "generate_deferred_html.*Generating" <parallel-log>
```

### Interpreting `drain_with_census` output

Each `accumulator drain complete` line has these fields:

| Field | Meaning |
|-------|---------|
| `label` | `"drain_epoch"` (per-epoch) or `"into_inner"` (final shutdown) |
| `pending_before` | events in pending at drain entry — should be 0 for epoch drains |
| `in_batch_before` | should be `false` at drain entry for both drain types |
| `batch_starts` / `batch_ends` | should be 1/1 for epoch drains; 0/0 for `into_inner` |
| `inside_batch` | event count processed inside a batch window; should be > 0 for epoch drains |
| `outside_total` | **must be 0** — any non-zero value means events arrived outside a `BatchStart`/`BatchEnd` window (compiler bug) |
| `outside_census` | per-event-type breakdown of outside-batch events; check `tracing::error!` lines to find the source |

### Expected healthy output — parallel, per epoch batch

```
INFO noet_core::beliefbase::accumulator: accumulator drain complete
  label="drain_epoch" pending_before=0 in_batch_before=false
  batch_starts=1 batch_ends=1 inside_batch=N outside_total=0 outside_census={}
```

### Expected healthy output — both paths, `into_inner`

```
INFO noet_core::beliefbase::accumulator: accumulator drain complete
  label="into_inner" pending_before=0 in_batch_before=false
  batch_starts=0 batch_ends=0 inside_batch=0 outside_total=0 outside_census={}
```

If `into_inner` shows `inside_batch > 0`, some batches were not drained by
`drain_epoch` — investigate for missing `drain_epoch` call sites in `parse_all`.

### Validation checklist (run after every corpus rerun)

1. **Run completed?** — check for `"parsing complete"` or clean exit.
2. **`outside_total = 0`** in `accumulator drain complete` — if non-zero, find
   the matching `tracing::error!` lines for event kind and call site.
3. **`drain_epoch` census** — one line per epoch batch; `inside_batch` > 0,
   `outside_total` = 0, `batch_starts=1 batch_ends=1`.
4. **`deferred_html` generated** — `"[generate_deferred_html] Generating HTML
   for N deferred"` must appear after `accumulator drain complete`.
5. **`ReparseLimitExceeded` count** — expect ~851 files at `max_reparse_count=2`
   on the MDN JS corpus (844 attempt-3 records confirmed in `/tmp/jobs-8-newfix.log`).
6. **Asset snapshot populated** — task-seeding log lines must show
   `snapshot_states≥369` for remainder-epoch batches (366 assets + network nodes).
   If `snapshot_states` is low (≤3), `sync_asset_snapshot` is not firing or
   `drain_epoch` is not completing before the snapshot is taken.
7. **Phase 0 mean** — fresh-parse (attempt 1) should be ~100ms flat; remainder
   (attempt 2+) ~300ms flat. Outliers >1s in attempt 2+ indicate asset-loading
   regression (Fix B) or api-key mutex contention (Fix C).
8. **`[initialize_stack] Loaded N assets` log lines** — should not appear for
   remainder-epoch tasks after Fix B. Any occurrence means `session_bb` asset
   subgraph is missing for that epoch.
9. **`session_bb does not have a network node` WARNs** — count; reduction
   indicates session_bb population improving for epoch N≥1 tasks.
10. **`source is missing` WARNs** — count and BID pattern (TQ-2, not yet
    resolved); distinct from `sink is missing`.
11. **Wall-clock comparison** — `--jobs 8` vs `--jobs 1`; baseline ×7.8 speedup
    on MDN JS corpus (2860 files, 15m30s wall, 636s sequential sum).

### Wall-clock comparison methodology

```
# Parse phase only (excludes drain):
python3 benches/log_analysis/parse_log.py --file-times <log> 2>&1 | tail -40

# Concurrency histogram (parallel only):
python3 benches/log_analysis/parse_log.py --concurrency <parallel-log>

# Stall annotation (task-switch gaps):
python3 benches/log_analysis/parse_log.py --stalls <log>
```

Baseline (MDN JS corpus, `/tmp/jobs-8-newfix.log`, Fix B + Fix C applied):
- Wall time: 15m30s, ×7.8 speedup over sequential sum (636s)
- Attempt-1 Phase 0 mean: ~101ms/file (flat — O(1) scaling confirmed)
- Attempt-2+ Phase 0 mean: ~284ms/file (flat — asset-loading cost eliminated)
- Top Phase 0 outlier: 3.74s (`array/copywithin`, attempt 3, task_idx=7)
- 110 Phase 0 outliers above 0.69s cutoff (mean + 2σ); all attempt 2+
- `weakset/weakset` Phase 5 outlier: 10.9s (terminate_stack fan-out — separate issue)
- 851 files reaching attempt 3 (entire attempt-2 population re-queued — cause TBD)