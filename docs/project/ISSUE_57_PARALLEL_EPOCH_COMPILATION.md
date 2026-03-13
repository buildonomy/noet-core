# Issue 57: Parallel Epoch Compilation

**Priority**: MEDIUM
**Estimated Effort**: 5 days (RELATIVE COMPARISON ONLY)
**Dependencies**: None hard. Issue 56 protocol model informs the merge-safety
argument but does not block implementation.
**Related**: Issue 56 (PathMap protocol observability)

## Summary

`DocumentCompiler::parse_all` processes files sequentially today. This issue
delivers true intra-epoch parallelism by (1) introducing a `BeliefAccumulator`
— a live, drainable in-memory `BeliefSource` that unifies the `parse` and
`watch` event-processing architectures — and (2) restructuring epoch 0 as a
network-ordered sequence of parallel leaf batches, each separated by an
explicit flush. The result is a `parse` pipeline structurally identical to
`watch`, where `global_bb` is always live and queryable between sub-epoch
boundaries.

## Goals

1. ✅ Extend `try_initialize_stack_from_session_cache` to accept a `GlobalCache`
   hit as equivalent to a `StackCache` hit.
2. ✅ Add `--jobs N` / `NOET_JOBS` flag and sequential fallback at `jobs=1`.
3. Introduce `BeliefAccumulator`: a live in-memory `BeliefSource` backed by an
   `UnboundedReceiver<BeliefEvent>` that unifies `parse` and `watch` event
   handling.
4. Repurpose `BalanceCheck` → `Flush`: remove all vestigial emit/consume sites,
   assign new sub-epoch-boundary semantics consumed by `BeliefAccumulator`.
5. Restructure epoch 0 in `parse_all` as a network-ordered sequence of parallel
   leaf batches gated by `Flush`.
6. Implement true OS-thread parallelism for leaf batches via
   `tokio::task::spawn` + bounded semaphore.
7. Preserve `--jobs 1` sequential path as byte-identical fallback.

## Architecture

### The Epoch Invariant (unchanged)

An **epoch** is the set of files sharing the same parse count at the point they
are dequeued. Files in epoch 0 have never been parsed. Files in epoch N ≥ 1
have been parsed N times; their nodes exist in `global_bb` from prior epochs.

**Within a single epoch, no file's parse output is an input to any other
file's parse in that same epoch.** Cross-file dependencies only flow across
epoch boundaries. This is the invariant that makes intra-epoch parallelism safe.

### BeliefAccumulator

`BeliefAccumulator` is the in-memory analog of `DbConnection`. It owns:

- An internal `BeliefBase` (the accumulated state)
- An `UnboundedReceiver<BeliefEvent>` (the read side of the compiler's `tx`)

It implements `BeliefSource` by querying the internal `BeliefBase` directly.
Before each query it lazily drains any pending events from the receiver into the
internal `BeliefBase` — cheap when the channel is already empty, free when
nothing has changed.

It exposes one additional method:

```rust
fn drain(&mut self)
```

`drain()` performs a complete synchronous drain of all pending channel events
into the internal `BeliefBase` and then calls `invalidate()` on any wrapped
`CachedBeliefSource`. This is the explicit flush point called by `parse_all` at
sub-epoch boundaries.

`BeliefAccumulator` lives in `src/beliefbase/accumulator.rs`, gated to
`#[cfg(not(target_arch = "wasm32"))]` (same as `CachedBeliefSource`).

### parse / watch Unification

Today `parse` and `watch` differ in how `global_bb` is wired:

| | `parse` (today) | `watch` | target |
|---|---|---|---|
| `global_bb` type | frozen `BeliefBase` clone | live `DbConnection` | live `BeliefAccumulator` |
| event consumer | background processor task | transaction task | `BeliefAccumulator::drain()` |
| drain trigger | `close_tx` + `processor.await` | compiler-idle notify | explicit `drain()` at sub-epoch boundaries |

After this issue, `parse` uses `BeliefAccumulator` as `global_bb`. The
background processor task in `main.rs` is eliminated. `parse_all` owns the
drain loop. `finalize_html` receives the same `BeliefAccumulator` (fully
drained) that was used during parsing — no separate `final_bb` needed.

`watch` is unchanged: it continues to use `DbConnection` as `global_bb`. The
compiler task in `FileUpdateSyncer` continues to call `parse_next` against the
`DbConnection` exactly as today.

### Epoch 0: Network-Ordered Parallel Dispatch

`ProtoIndex::discover_network_dirs()` returns all network directories sorted
by depth (shallowest first — already implemented). This ordering defines the
sub-epoch sequence for epoch 0:

```
For each network_dir in discover_network_dirs() (depth order):
  1. Parse network_dir/index.md  (single task, slow-path push())
  2. accumulator.drain()         (flush: index.md's events enter global_bb)
  3. Dispatch all leaf children of network_dir in parallel
     (each fires GlobalCache fast-path against now-populated global_bb)
  4. accumulator.drain()         (flush: leaf events enter global_bb;
                                   CachedBeliefSource invalidated)
```

**Why the parent must precede its leaves**: `try_initialize_stack_from_session_cache`
needs the parent network node in `global_bb` to fire the `GlobalCache` fast
path. The parent node enters `global_bb` only after its `terminate_stack` events
are drained. Once drained, all leaves are fully independent — `sort_key_for` is
served by `ProtoIndex` (static, no `global_bb` query needed) and leaf content
is mutually non-referential within epoch 0.

**Sub-networks**: `discover_network_dirs()` returns them in depth order. A
sub-network (`array/`) appears after its parent (`reference/global_objects/`)
in the sequence, so by the time `array/index.md` is dispatched, its parent
network is already in `global_bb`. The leaf batch for `array/` fires after
`array/index.md` is drained. Correct by construction.

**`CachedBeliefSource` scope**: one `CachedBeliefSource` wraps `global_bb` for
the entire `parse_all` invocation. After each `drain()`, `invalidate()` is
called so stale parent-network query results are evicted. Within a leaf batch
(between two drains), the cache is stable — all siblings query the same parent
key and share the memoised result.

### Epoch N ≥ 1: Standard Parallel Dispatch

At the start of epoch N ≥ 1, `global_bb` (the `BeliefAccumulator`) is fully
drained from epoch N-1. All parent networks and all previously-parsed leaf
nodes are present. Wrap in a fresh `CachedBeliefSource`.

Every file queued for reparse can resolve cross-document links via `GlobalCache`
hits against this complete snapshot. Files within epoch N are still mutually
independent (same epoch invariant). Dispatch all epoch-N files in parallel,
then `drain()` before epoch N+1.

The sequential `parse_next` reparse state machine (max_reparse_count,
reparse_stable, pending_dependencies) is preserved unchanged — the parallel
dispatch just replaces the inner `parse_next` call, not the outer retry logic.

### Flush Event (`BalanceCheck` → `Flush`)

`BalanceCheck` is currently vestigial at every consumption site:

- `BeliefBase::process_event` — explicit no-op (comment explains it was
  de-fanged in run 11)
- `db.rs Transaction::add_event` — commented-out no-op
- `terminate_stack` match arm — `BeliefEvent::BalanceCheck => {}`
- All emit sites in `builder.rs` and
  `try_initialize_stack_from_session_cache` — fire-and-forget calls that
  accomplish nothing

**Plan**: rename `BalanceCheck` → `Flush` and simultaneously:

1. Remove all vestigial emit sites in `builder.rs` and
   `try_initialize_stack_from_session_cache`.
2. Remove the no-op match arms in `BeliefBase::process_event`, `db.rs`, and
   `terminate_stack`.
3. Assign new semantics: `Flush` is emitted by `parse_all` onto the `tx`
   channel at sub-epoch boundaries as a sentinel. `BeliefAccumulator::drain()`
   drains until it sees `Flush` (or the channel empties), then invalidates
   `CachedBeliefSource`.

`Flush` is not emitted by `terminate_stack` or any per-file code — it is
exclusively a `parse_all`-level signal. Per-file `BalanceCheck` emissions are
simply deleted.

### True OS-Thread Parallelism

Steps 1 and 2 of this issue already added `Send` bounds to `B` in
`parse_next`, `parse_all`, and `parse_epoch_parallel`. `BeliefBase` on native
is `Send + Sync` (uses `Arc<RwLock<T>>`). `GraphBuilder` (which contains
`BeliefBase`) is therefore `Send` on native.

Leaf batch tasks can be dispatched via `tokio::task::spawn` with a
`Arc<Semaphore>` pool bounded to `--jobs N`. Each task:

1. Constructs a fresh `GraphBuilder` (fresh `session_bb`, shared `tx` clone)
2. Calls `parse_content` + `terminate_stack`
3. Returns `ParseContentWithCodec`

No post-task merge step. Events flow directly to `BeliefAccumulator` via `tx`.
The semaphore ensures at most N tasks run concurrently regardless of batch size.

## Implementation Steps

### Step 1: GlobalCache fast-path ✅ (complete)

`try_initialize_stack_from_session_cache` accepts both `NodeSource::StackCache`
and `NodeSource::GlobalCache`. `fast_missing` sourced from `global_bb` on
`GlobalCache` hit.

### Step 2: `--jobs` / `NOET_JOBS` ✅ (complete)

`jobs: usize` field on `DocumentCompiler`. `-j N` CLI flag. `NOET_JOBS` env
var. Sequential fallback at `jobs=1`.

### Step 3: `BeliefAccumulator` (1 day)

- [ ] `src/beliefbase/accumulator.rs`: `BeliefAccumulator` struct with internal
      `BeliefBase` + `UnboundedReceiver<BeliefEvent>`.
- [ ] `impl BeliefSource for BeliefAccumulator`: drain lazily before each
      query (cheap `try_recv` loop), then delegate to internal `BeliefBase`.
- [ ] `fn drain(&mut self)`: synchronous full drain + `CachedBeliefSource`
      invalidation. Takes a `&mut CachedBeliefSource<Self>` or invalidates via
      a shared `Arc<Inner>` reference.
- [ ] `pub fn new(rx: UnboundedReceiver<BeliefEvent>) -> Self`
- [ ] `src/beliefbase/mod.rs`: `mod accumulator; pub use accumulator::BeliefAccumulator`
      gated to `#[cfg(not(target_arch = "wasm32"))]`.
- [ ] `src/bin/noet/main.rs`: replace background processor task with
      `BeliefAccumulator::new(rx)`. Pass accumulator as `global_bb` to
      `parse_all`. After `parse_all` returns, call `close_tx()` then
      `accumulator.drain()` for the final flush before `finalize_html`.
- [ ] Tests: construct `BeliefAccumulator` from a channel, send events, confirm
      `BeliefSource` queries reflect them after drain.

### Step 4: `BalanceCheck` → `Flush` (0.5 days)

- [ ] `src/event.rs`: rename `BalanceCheck` → `Flush`. Update doc comment.
- [ ] Remove all vestigial emit sites in `builder.rs` (all
      `process_event(&BeliefEvent::BalanceCheck)` calls on `session_bb`,
      `doc_bb`) and in `try_initialize_stack_from_session_cache`.
- [ ] Remove no-op match arms for `BalanceCheck` in `BeliefBase::process_event`,
      `db.rs Transaction::add_event`, `terminate_stack`.
- [ ] `BeliefAccumulator::drain()`: emit `Flush` from `parse_all` onto `tx`
      as the drain sentinel; `drain()` reads until `Flush` or channel empty.
- [ ] Update `Display`, `origin()`, `with_origin()` impls in `event.rs`.
- [ ] `s/BalanceCheck/Flush/g` across all remaining match arms (compile-driven).

### Step 5: Network-ordered epoch 0 dispatch (1.5 days)

- [ ] `parse_all`: replace the flat `primary_queue` batch with a loop over
      `proto_index.discover_network_dirs()` in depth order.
- [ ] For each `network_dir`:
  - Parse `network_dir/index.md` as a single task (reuse `parse_epoch_parallel`
    with a one-element slice, or inline).
  - Call `cached_global_bb.invalidate()` + `accumulator.drain()`.
  - Collect `children_of(network_dir)` that are leaves (not themselves network
    dirs). Dispatch as a parallel batch via `parse_epoch_parallel`.
  - Call `cached_global_bb.invalidate()` + `accumulator.drain()`.
- [ ] Handle the case where a child of `network_dir` is itself a network dir
      (sub-network): it will appear later in `discover_network_dirs()` and be
      handled in its own iteration — skip it in the leaf batch.
- [ ] Verify: after epoch 0 completes, every file in `primary_queue` has been
      dispatched exactly once.
- [ ] `--jobs 1` path: same loop structure, but each leaf batch runs
      sequentially (reuse existing `parse_epoch_parallel` sequential loop or
      `parse_next`).

### Step 6: True OS-thread parallelism (1 day)

- [ ] `parse_epoch_parallel`: replace sequential `for path in paths` loop with
      `tokio::task::spawn` per path, gated by `Arc<Semaphore>` of size `jobs`.
- [ ] Each spawned task gets: owned `GraphBuilder`, `tx.clone()`,
      `global_bb.clone()` (cheap — `CachedBeliefSource` is `Arc`-wrapped).
- [ ] Collect results via `JoinSet` or `FuturesUnordered`; preserve path order
      for deterministic result ordering.
- [ ] `--jobs 1`: semaphore of size 1 is equivalent to sequential — no special
      case needed.

### Step 7: Epoch N ≥ 1 parallel dispatch (0.5 days)

- [ ] After epoch 0 completes, the reparse queue contains files with unresolved
      cross-document links. Group by epoch number (existing `processed` map).
- [ ] For each reparse epoch N: wrap `accumulator` in a fresh
      `CachedBeliefSource`, dispatch all epoch-N files via
      `parse_epoch_parallel`, drain.
- [ ] The existing reparse state machine (max_reparse_count, reparse_stable,
      pending_dependencies, last_round_updates) is preserved; only the inner
      dispatch changes from sequential `parse_next` to parallel batch.

## Testing Requirements

- [ ] `cargo test --features service` passes with no regressions.
- [ ] `--jobs 1` produces byte-identical output to the current sequential
      implementation on `tests/network_1`.
- [ ] Parallel build of `global_objects/` corpus completes without panic or
      data corruption.
- [ ] `BeliefAccumulator` unit tests: drain reflects events, lazy query drain
      works, `Flush` sentinel triggers invalidation.
- [ ] No `BalanceCheck` references remain after Step 4.

## Success Criteria

- [ ] `BeliefAccumulator` replaces the background processor task in `main.rs`.
- [ ] `parse` and `watch` share the same event-processing pattern: compiler
      emits to `tx`; a live `BeliefSource` (accumulator or DB) drains and
      answers queries.
- [ ] Epoch 0 dispatches leaf files in parallel, gated by network-parent
      availability in `global_bb`.
- [ ] `BalanceCheck` is gone; `Flush` is emitted only at sub-epoch boundaries
      by `parse_all`.
- [ ] Wall-clock improvement on `global_objects/` corpus is measurable with
      `--jobs 4` vs `--jobs 1`.
- [ ] `--jobs 1` sequential fallback is correct and byte-identical.

## Risks

- **`BeliefAccumulator` lazy drain cost**: if `try_recv` before every
  `eval_query` adds measurable latency even when the channel is empty, switch
  to an explicit pre-batch drain only.
  → **Mitigation**: `try_recv` on an empty `mpsc` channel is a single atomic
  check — negligible. Profile if it shows up.

- **Network-dir ordering edge cases**: a file that lives in a directory without
  an `index.md` is flattened into its ancestor network by `iter_net_docs`. Its
  sort key is computed by `sort_key_for` walking upward. If
  `discover_network_dirs` doesn't cover this directory, the file appears in the
  ancestor network's leaf batch — correct, since the ancestor network was
  already parsed in its own iteration.
  → **Mitigation**: `ProtoIndex::sort_key_for` already handles this case
  (walk-up loop). No changes needed to `ProtoIndex`.

- **`Flush` sentinel race**: if `parse_all` sends `Flush` onto `tx` while a
  parallel batch task is still sending its own events, the drain could stop at
  `Flush` before all task events have arrived.
  → **Mitigation**: `parse_all` sends `Flush` only after `JoinSet` / all task
  handles have been awaited — all task `tx` sends are complete before `Flush`
  is enqueued. MPSC ordering guarantees `Flush` arrives after all preceding
  sends from any task that has already completed.

- **`CachedBeliefSource` invalidation granularity**: invalidating the full
  cache on every drain evicts entries that are still valid (e.g. the repo-root
  ancestor chain never changes). `invalidate_for_events` would be more precise
  but requires threading the event list through `drain()`.
  → **Mitigation**: full invalidation is correct and cheap (cache is small —
  tens of entries per epoch). Selective invalidation is a follow-on optimisation
  if profiling shows it matters.

- **Reparse epoch correctness under parallelism**: two files in the same reparse
  epoch that have a latent cross-document dependency (file A's epoch-1 output
  is needed by file B's epoch-1 parse) would produce a wrong result if
  dispatched in parallel.
  → **Mitigation**: the epoch invariant forbids this by definition. Epoch-N
  files depend only on epoch 0..N-1 output, never on each other. If a violation
  is found empirically, it indicates a bug in the dependency-promotion logic
  (which promotes unresolved references to the *next* epoch), not in the
  parallel dispatch.

## Open Questions

- None. Architecture is settled; implementation is sequenced above.