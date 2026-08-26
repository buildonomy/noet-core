# Issue 100: Split the Accumulator's Global Mutex for Concurrent SQL Reads — ✅ COMPLETE

**Priority**: MEDIUM
**Status**: COMPLETE
**Estimated Effort**: 1-2 days (RELATIVE COMPARISON ONLY) (Actual: ~1 session)
**Dependencies**: Related to Issue 99 (large-corpus perf investigation) —
that issue's preliminary (unverified) findings included a hypothesis that a
global mutex serializes all `--jobs N` parallel tasks behind a single
in-flight SQL traversal. Goal 1 below (root-cause investigation) confirmed
the hypothesis independent of Issue 99's specific stall numbers, which
remain unverified.

## Summary

`BeliefAccumulator<S>` and `QueryHandle<S>` (`src/beliefbase/accumulator.rs`)
both implement `BeliefSource::evaluate` (and every other `BeliefSource`
method) by acquiring a single `tokio::sync::Mutex<AccInner<S>>` and holding
the guard for the **entire** duration of the delegated call:

```rust
// beliefbase/accumulator.rs — both BeliefAccumulator::evaluate and
// QueryHandle::evaluate follow this identical pattern:
let guard = acc.lock().await;
guard.inner.evaluate(package).await?;
```

When `S = DbConnection` (the production `--jobs N render` / `noet parse`
path), `evaluate()` drives a multi-hop SQL traversal (up to `MAX_TRAVERSAL =
10` hops, each a `fetch_all` round-trip against `relations`) plus a final
bulk state fetch — all of it read-only SQL. Holding the mutex for that whole
traversal means **every other `--jobs N` task attempting any query blocks
until the one long-running query finishes**, collapsing effective read
parallelism to 1 for the duration.

**Goal 1 finding (see below): this serialization is a self-imposed
Rust-level constraint, not something SQLite requires for reader-vs-reader
concurrency.** The investigation also surfaced a separate, pre-existing
correctness bug in `Transaction::execute` (missing `BEGIN`/`COMMIT`
wrapping) that should be fixed as part of this issue regardless of the lock
redesign.

## Goal 1 Findings (root-cause investigation — complete)

**Scope note**: this investigation is scoped to the `db_init_memory` path
(the in-memory, shared-cache DB used by `--jobs N` parse/render), which is
the actual production hot path Issue 99 is concerned with. The file-backed
`db_init` path (watch service) has different locking characteristics and is
out of scope here — see the WAL finding below.

1. **`db_init_memory` uses SQLite shared-cache mode, not the plain
   rollback-journal model.** It opens
   `file:noet_parse?mode=memory&cache=shared`. Shared-cache has its own
   three-tier lock model (transaction / table / schema level). At the table
   level — the relevant one here — **any number of connections may hold a
   concurrent read-lock on a table; only a single write-lock may be held at
   a time**, and a write-lock excludes all read-locks on that table for its
   duration. This means concurrent `evaluate()` calls (100% read-only SQL —
   confirmed by reading `DbConnection::evaluate`, `resolve_seed`,
   `apply_traversal_sql`, `apply_filter_sql`, `get_states_by_bids`) are
   **not serialized by SQLite itself**. The `tokio::Mutex` held across the
   whole delegated call is a pure Rust-level constraint for the read/read
   case.
2. **WAL mode is inapplicable to `db_init_memory` — drop it from scope.**
   Per SQLite docs: "the journal_mode for an in-memory database is either
   MEMORY or OFF and can not be changed to a different value." The original
   Context section's discussion of `PRAGMA journal_mode=WAL` as a fix or
   prerequisite does not apply to this DB. (It remains a legitimate,
   separately-scoped question for the file-backed `db_init`/watch-service
   path, which is out of scope for this issue.)
3. **A real, pre-existing correctness bug**: `Transaction::execute` (`db.rs`
   L103-161) issues multiple sequential statements against the pool — the
   main `qb`, any overflow chunks (bind-limit flushes), the `is_net` fixup
   UPDATE, and mtime `INSERT OR REPLACE` batches — with **no SQL-level
   `BEGIN`/`COMMIT` wrapping them**. Today, atomicity of a whole
   `apply_batch` call is provided *only* by the Rust mutex being held for
   its entire duration, despite the doc comment on
   `db_init_memory`/`DbConnection::apply_batch` claiming "a single
   WAL-style transaction per epoch." If any statement partway through
   `execute()` fails, everything already executed is permanently committed
   with no rollback. **This should be fixed by wrapping the whole
   `Transaction::execute` body in `BEGIN`/`COMMIT`, independent of the lock
   redesign** — it is a small, targeted, high-value fix (see Implementation
   Steps).
4. **A chunk-pause approach (releasing the Rust lock between overflow
   chunks mid-write to let readers interleave) was considered and
   rejected.** SQLite's shared-cache write-lock is held for the entire SQL
   transaction, not per-statement. Releasing only the *Rust* lock between
   chunks would not let readers through — they would immediately hit
   `SQLITE_LOCKED_SHAREDCACHE` (or block, given a `busy_timeout`) against
   the same tables until the SQL transaction commits. This adds complexity
   and a correctness surface (reasoning about interrupted/resumed writer
   state) without buying real concurrency.
5. **Reads and writes do not currently overlap in time, by construction of
   the epoch structure.** In `parse_epoch`/`parse_all`, each epoch fully
   joins all parallel read-only tasks *before* `BatchEnd` is sent and
   `drain_epoch()` triggers the one `apply_batch` write for that epoch. So
   the actual, measurable win available today is **reader-vs-reader**
   concurrency within a single epoch — which a plain `RwLock` captures
   directly, with no writer-side chunking tricks needed. (Caveat: this
   "reads and writes never overlap" property is a byproduct of today's
   sequential epoch structure, not an enforced invariant — future
   pipelining work, e.g. Issue 97, could change this. The `RwLock` design
   below does not depend on it for correctness, only benefits from it
   today.)
6. **No `busy_timeout` is configured anywhere** (confirmed via grep — zero
   matches for `busy_timeout`/`busy_handler`/`SQLITE_BUSY` in `src/`). This
   is currently invisible because exactly one SQL statement is ever in
   flight against the pool at a time. Once the lock is split and/or
   `BEGIN`/`COMMIT` is added, add `PRAGMA busy_timeout` as cheap insurance
   against a `SQLITE_LOCKED_SHAREDCACHE`/`SQLITE_BUSY` surfacing as a hard
   error instead of a bounded wait.

**Conclusion: root cause confirmed over-broad for the read/read case.**
Proceed to lock-granularity redesign (Goal 2 below), scoped to the
`db_init_memory` path.

## Goals

1. ~~Confirm whether SQLite requires serializing `evaluate()` calls~~ —
   **done, see Findings above.**
2. Add `BEGIN`/`COMMIT` around `Transaction::execute`'s body (`db.rs`) so
   `apply_batch` is atomic at the SQL level, independent of the Rust lock.
   Small, targeted change — do this first, since the lock redesign in Goal
   3 will depend on writes being genuinely atomic once no longer
   Rust-lock-guarded for the reader case.
3. Design and implement a lock split — `tokio::sync::RwLock<AccInner<S>>` in
   place of `tokio::sync::Mutex<AccInner<S>>` — with:
   - **Shared** guard for all `BeliefSource` methods (`evaluate`, `submap`,
     `submap_by_bid`, `get_file_mtimes`, `export_beliefgraph`).
   - **Exclusive** guard for `AccInner::handle_event` /
     `drain_with_census` as a whole (covers `BatchStart`/`BatchEnd`
     bookkeeping, `pending` mutation, and the `apply_batch` call as one
     atomic unit).
   - Preserve all existing ordering guarantees:
     - `BatchStart`/`BatchEnd` bracketing and `pending` buffer semantics.
     - Read-after-write consistency across `drain_epoch` boundaries.
     - `QueryHandle` clone semantics (`Arc<RwLock<AccInner<S>>>` clones
       share the same lock).
4. Add `PRAGMA busy_timeout` to `db_init_memory` (and `db_init`) as
   insurance against shared-cache lock contention surfacing as a hard
   error.
5. Prototype and benchmark with the microbenchmark described under Testing
   Requirements (a real large-corpus run is not required to validate this
   change — see Issue 99 for that separate investigation).

## Architecture Notes / Where to Start

- `src/beliefbase/accumulator.rs`:
  - `AccInner<S>` struct and its fields (`inner`, `rx`, `pending`, `in_batch`,
    `drain_count`).
  - `BeliefAccumulator::evaluate` / `submap` / etc. and `QueryHandle`'s
    equivalents — currently identical lock-then-delegate implementations;
    all become shared-guard acquisitions.
  - `AccInner::handle_event` / `drain_with_census` — where `apply_batch` is
    actually invoked, and where `in_batch`/`pending` are mutated. This is
    the code that needs the exclusive guard.
  - `AccCache` — already independently locked (`std::sync::Mutex`); not
    affected by this redesign, just confirm it stays correct.
- `src/db.rs`:
  - `Transaction::execute` — add `BEGIN`/`COMMIT` around the existing
    sequence of `query.execute(connection)` calls.
  - `db_init` / `db_init_memory` — add `PRAGMA busy_timeout = <N>ms` via
    `after_connect` (mirroring the existing `after_connect` hook already
    used in `db_init` for `regexp` registration).
  - `DbConnection`'s `BeliefSource`/`BeliefSink` impls — no changes
    expected; confirmed safe to invoke concurrently via `&Pool<Sqlite>` for
    the read case.

## Testing Requirements

- Existing accumulator/parallel-epoch test suite must continue to pass
  unchanged — event ordering guarantees are load-bearing for many tests.
- New concurrency test: spawn multiple concurrent `evaluate()` calls against
  a `QueryHandle<DbConnection>` while no write is in flight, and assert they
  complete without serializing on wall-clock time (e.g. N concurrent
  1-second-simulated queries should take ~1s total, not ~N seconds). Write
  this test **first**, against the current `Mutex`-based code (it should
  currently fail/serialize), so it becomes the regression gate for the
  `RwLock` change.
- New ordering test: a write (`apply_batch` via `BatchStart`/`BatchEnd`)
  concurrent with reads must still produce correct read-after-write
  semantics — no reader should observe a partially-applied batch.
- New atomicity test: inject a mid-`execute()` failure (e.g. a bad chunk)
  and confirm no partial writes are committed once `BEGIN`/`COMMIT` wraps
  `Transaction::execute`.

## Resolution

All goals implemented and tested in a single session, following the Goal 1
investigation above.

**`src/db.rs`**:
- `Transaction::execute` now wraps its full body in `connection.begin()` /
  `tx.commit()`, with every statement (main `qb`, overflow chunks, `is_net`
  fixup, mtime batch) executed against the transaction handle instead of
  the bare pool.
- `db_init` and `db_init_memory` both register `PRAGMA busy_timeout = 5000;`
  via `after_connect`, applied per-connection on the pool.
- Fixed two pre-existing inline `use` statements inside `db_init` and
  `db_init_memory` (moved to module top) per the "No Inline Imports" hard
  rule in AGENTS.md, encountered incidentally while editing these
  functions.
- Added `db::tests::execute_rolls_back_valid_statements_on_later_failure`,
  which forces a valid statement to run before a deliberately invalid one
  in the same `execute()` call and asserts the valid statement's effect is
  rolled back. Verified this test fails (partial write persists) against
  the pre-fix code and passes against the fix.

**`src/beliefbase/accumulator.rs`**:
- `AccInner<S>` is now held behind `Arc<tokio::sync::RwLock<AccInner<S>>>`
  (was `Arc<tokio::sync::Mutex<AccInner<S>>>`) in both `BeliefAccumulator`
  and `QueryHandle`.
- All `BeliefSource` methods (`evaluate`, `submap`, `submap_by_bid`,
  `get_file_mtimes`, `export_beliefgraph`) on both types now take a shared
  (`read()`) guard.
- `AccInner::drain_with_census` (invoked from `into_inner` and
  `QueryHandle::drain_epoch`, and internally covering `handle_event`/
  `apply_batch`) takes the exclusive (`write()`) guard, preserving
  `BatchStart`/`BatchEnd` bookkeeping exclusivity and read-after-write
  consistency across epoch boundaries.
- `AccCache` (the separate `std::sync::Mutex`-backed query cache) is
  unaffected.
- Added two regression tests:
  - `concurrent_evaluates_do_not_serialize` — spawns 8 concurrent
    `QueryHandle::evaluate()` calls (each against a distinct seed BID to
    avoid cache hits) with an artificial 200ms delay in the test
    `BeliefSource`, and asserts they overlap (`max_in_flight > 1`) and
    complete well under `N * delay` wall-clock time. Verified this test
    fails (`max_in_flight = 1`, ~1.6s for 8x200ms) against the pre-fix
    `Mutex`-based code and passes (`max_in_flight` > 1, ~0.2s) against the
    fix.
  - `write_excludes_concurrent_reads` — starts a delayed `apply_batch` via
    `drain_epoch` concurrently with a burst of `evaluate()` calls, and
    asserts no reader ever observes a `write_in_progress` flag set by the
    writer (i.e. the exclusive guard genuinely excludes concurrent reads).

**Validation**: `cargo test --lib --features service` (721 passed, 0
failed); `cargo test --test belief_source_test --test cache_invalidation_test
--test service_integration --test schema_migration_test --features service`
(all pass except `test_stale_file_detection_and_reparse`, which was
independently confirmed to fail identically against the unmodified `HEAD`
baseline — a pre-existing filesystem-watcher timing flake, unrelated to
this change); `cargo test --test codec_test --features service
test_sequential_db` and `test_parallel_db` (both pass, exercising the exact
sequential and `--jobs 4` parallel `DbConnection` parse paths this issue
targets).

**Not done / explicitly out of scope**: a large-corpus benchmark (Goal 5's
"benchmark against a production corpus with `--jobs 4`") was not run —
Issue 99 is the correct home for that, once a clean (sleep-free) baseline
log exists. The
microbenchmark-style regression tests above are sufficient to validate this
issue's correctness and concurrency claims per the Testing Requirements.
The file-backed `db_init`/watch-service path's WAL-mode question (mentioned
in Goal 1 Finding #2) also remains unexplored — not needed for this issue's
scope but flagged for anyone picking up watch-service performance work.

## Success Criteria

- [x] Root cause determined: the current full-duration mutex hold is
      over-broad for the read/read case; not required by SQLite's
      shared-cache table-lock model.
- [x] `BEGIN`/`COMMIT` added around `Transaction::execute`; atomicity test
      passes.
- [x] `RwLock` redesign implemented, tested, and benchmarked (via
      microbenchmark-style regression tests, not a full corpus run — see
      Resolution) showing improved concurrent-read throughput without
      correctness regressions.
- [x] `PRAGMA busy_timeout` added to both `db_init` and `db_init_memory`.
- [x] All existing accumulator/parallel-epoch tests still pass.

## Risks

- Risk: Splitting the lock introduces a subtle read-after-write ordering bug
  that only manifests under specific interleavings on large corpora →
  **Mitigation**: write the concurrency/ordering tests described above
  *before* the redesign, and keep the change as small and well-isolated as
  possible (`RwLock` swap-in, not a broader restructuring).
- Risk: Adding `BEGIN`/`COMMIT` changes failure/rollback behavior in a way
  existing tests don't cover → **Mitigation**: add the atomicity test
  before making the change.
- Risk: The actual bottleneck in Issue 99 turns out to be fully explained by
  system sleep, making this investigation lower-value than currently
  believed → **Mitigation**: not a blocker — a global mutex held across full
  SQL traversals, plus a missing transaction wrapper, are correctness- and
  performance-adjacent design smells worth fixing regardless of Issue 99's
  outcome.

## References

- `noet-core/src/beliefbase/accumulator.rs` — `AccInner`,
  `BeliefAccumulator::evaluate`, `QueryHandle::evaluate`,
  `AccInner::handle_event`, `AccCache`.
- `noet-core/src/db.rs` — `DbConnection`, `Transaction::execute`, `db_init`,
  `db_init_memory`, `apply_traversal_sql`.
- `noet-core/docs/project/0_open/ISSUE_99_LARGE_CORPUS_PERF_INVESTIGATION.md`
  — the investigation that surfaced this hypothesis; its stall numbers are
  unverified (confounded by system sleep) but the architectural observation
  about lock scope is independently confirmed by this issue's Goal 1.
- [SQLite Shared-Cache Mode](https://sqlite.org/sharedcache.html) — table-
  level read/write lock model referenced in Findings #1 and #4.
- [SQLite WAL](https://sqlite.org/wal.html) — confirms in-memory DBs cannot
  use WAL (Finding #2).
- [SQLite Result Codes](https://www.sqlite.org/rescode.html#locked) —
  `SQLITE_LOCKED_SHAREDCACHE` semantics referenced in Finding #6.
