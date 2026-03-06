# Issue 53: Deterministic Synchronization for Cache Invalidation Tests

**Priority**: HIGH
**Estimated Effort**: 2 days (RELATIVE COMPARISON ONLY)
**Dependencies**: None (partially unblocked by the path-canonicalization fix in `src/db.rs`)
**See Also**: [`docs/design/federated_belief_network.md`](../design/federated_belief_network.md) — the `commit_generation` counter introduced here is the seed of the Layer 2 sequence number described in that design doc.

## Summary

The five tests in `tests/cache_invalidation_test.rs` used `std::thread::sleep` as their only
synchronization mechanism against three async pipeline stages inside `WatchService`. This
caused intermittent failures on Linux CI and consistent failures on Windows. The path
canonicalization fix and pipeline idle signaling infrastructure were implemented in a first
pass (tests pass on Linux), but the Windows failures revealed a deeper design flaw: the
transaction task's polling model meant `wait_for_idle` could unblock after the *first* partial
batch rather than after the full compile cycle. This issue tracks the complete fix.

## Goals

- [x] Replace sleep-based synchronization in cache invalidation tests with a deterministic
      idle signal from `WatchService`.
- [x] Eliminate the TTL/debounce-window race in the compiler-rewrite suppression mechanism.
- [x] Fix the `wait_for_idle` race where it unblocks prematurely on a partial batch commit.
- [ ] Ensure all five tests pass reliably on Linux, macOS, and Windows CI.
- [x] Keep test intent clear: each test should express *what* it waits for, not *how long*.

## Root Cause of Windows Failures

All five tests failed on Windows at the first DB assertion. The error message for
`test_mtime_tracking` was diagnostic:

```
test.md should have mtime tracked.
Found mtimes: {"C:\\Users\\runneradmin\\AppData\\Local\\Temp\\.tmp...\\test_network\\index.md": ...}
```

Only `index.md` was in the DB — the other files the test had written were not. The pipeline
had completed for `index.md` only.

**The sequence that causes this:**

1. `enable_network_syncer` enqueues the network directory root and fires `notify_one`.
2. Compiler wakes, parses `index.md` (the network root). This emits `FileParsed(index.md)`
   into the `mpsc` channel and enqueues `test.md`, `doc1.md`, etc. as dependents.
3. The transaction task was polling with `sleep(1s)`. It wakes, finds `FileParsed(index.md)`
   in the channel, drains and commits it, increments `commit_generation` to 1, and calls
   `notify_waiters`.
4. `wait_for_idle` unblocks (generation advanced past snapshot of 0).
5. The test queries the DB — only `index.md` is there. The compiler is still parsing the
   remaining files.

**The fundamental problem:** `commit_generation` advanced after the *first partial batch*,
not after the full compile cycle. The polling transaction task has no way to know whether
the compiler is still producing events. The `compiler_idle` flag exists but the transaction
task never checked it before signalling.

## What Was Implemented (Phase 1 — now superseded in part)

### Path canonicalization fix (`src/db.rs`)

`track_file_mtime` now calls `path.canonicalize()` before storing the path string. This
resolves the Windows 8.3 short-name aliasing bug where the same physical file was stored
under two different keys (e.g. `RUNNER~1` vs `runneradmin`). **This fix is correct and
complete; it is not being revisited.**

### Compiler idle flag (`src/watch.rs`)

`compiler_idle: Arc<AtomicBool>` was added to `FileUpdateSyncer`. It is set `false` when
the compiler wakes and finds work, and `true` when `parse_next` returns `None` (queues
drained). The debouncer hold-off uses this flag to defer file-watcher events while the
compiler is active. **This mechanism is correct and complete.**

`ignored_write_paths` is flushed entirely on compiler-idle transition rather than relying
on per-path TTL timers. The TTL removal task is gone. **This is correct and complete.**

`wait_for_idle` was added to `WatchService`. **The interface is correct; the implementation
is being corrected.**

### Test refactor (`tests/cache_invalidation_test.rs`)

Sleep-based synchronization replaced with `service.wait_for_idle(Duration::from_secs(30))`.
Two logically-required sleeps remain (see below). **These are correct and not changing.**

## Target Architecture (Phase 2 — this fix)

The transaction task polling model is replaced with a notification-driven design. The
`RwLock<UnboundedReceiver>` shared between the compiler task and the transaction task is
replaced with two unshared channels plus a dedicated idle notification.

### Pipeline topology

```
DocumentCompiler
    │
    │  mpsc::unbounded_channel<BeliefEvent>   (exclusively owned by transaction task)
    ▼
Transaction Task ──→ DbConnection (local SQLite, reliable delivery)
    │
    └──→ broadcast::Sender<BeliefEvent>       (best-effort, for LSP / future peers)
              ↓
         broadcast::Receiver  (LSP, peer replication — Issue 11, federated_belief_network.md)

Compiler Task
    └── on queue drain: sets compiler_idle=true, fires compiler_idle_notify
              ↓
         Transaction Task wakes via select!, drains channel, commits, signals wait_for_idle
```

### Key design decisions

**`mpsc` for DB, `broadcast` for subscribers**: The DB path uses `tokio::sync::mpsc`
(reliable, no drop). The broadcast channel is best-effort — a slow LSP client that falls
behind loses events but can re-query the DB. This is the correct reliability split: DB
consistency is mandatory, LSP freshness is not.

**`compiler_idle_notify: Arc<Notify>`**: A new `Notify` primitive added alongside
`compiler_idle: Arc<AtomicBool>`. The compiler fires this when it sets `compiler_idle =
true`. The transaction task `select!`s on `mpsc::recv()` OR `compiler_idle_notify.notified()`.
This eliminates the 1-second poll latency and the need for any task to inspect another
task's channel.

**Transaction task owns `UnboundedReceiver` exclusively**: The `RwLock<UnboundedReceiver>`
wrapper is removed. The receiver moves directly into the transaction task closure. No other
task inspects it. This eliminates the lock contention between the compiler task's
`is_empty()` check and `perform_transaction`'s `try_recv()` loop.

**Idle signal ownership**: `commit_generation` is incremented and `commit_notify` is fired
by the transaction task *only after* draining the channel in a state where
`compiler_idle == true`. Specifically:

- If after committing a batch the channel is empty AND `compiler_idle == true`: signal.
- If the `compiler_idle_notify` fires (compiler just went idle): drain any remaining
  events, commit if any were staged, then signal.
- Neither case fires on a partial batch mid-compile.

**`broadcast` is additive, not a breaking change**: `FileUpdateSyncer` gains a
`broadcast::Sender<BeliefEvent>` field. The transaction task calls
`broadcast_tx.send(event)` after processing each event (before or alongside the DB write).
The `broadcast::Sender` can be cloned out of `FileUpdateSyncer` by callers (e.g. LSP
setup) via a new accessor. If no `broadcast::Receiver` exists, sends are no-ops.

**`perform_transaction` is inlined or simplified**: The current `perform_transaction`
free function uses a write-lock loop to drain the channel. With the receiver exclusively
owned by the task, it simplifies to a straightforward drain loop with no locking.

### What `wait_for_idle` does

```
1. Snapshot commit_generation for each active syncer (under watchers lock).
2. Drop the lock.
3. For each syncer: block_on(timeout(remaining, async {
       loop {
           if generation > snapshot { return; }
           commit_notify.notified().await;
       }
   }))
```

This is unchanged from Phase 1. The fix is upstream: the transaction task now only fires
`commit_notify` at true pipeline idle, so `wait_for_idle` returning means the full cycle
is complete.

## Intentionally Retained Sleeps in Tests

Two `std::thread::sleep` calls remain in `tests/cache_invalidation_test.rs`:

- **`test_stale_file_detection_and_reparse`**: a 3-second sleep between the initial parse
  and the file modification. This is logically required — filesystem mtime resolution can
  be 1–2 seconds on some platforms (NTFS has 2-second granularity), so without a pause the
  re-written file could carry the same mtime as the original and not be detected as stale.
  This is not a synchronization hack. The test comment says so.

- **`test_deleted_file_handling`**: the post-deletion wait uses `sleep(7s)`. File deletion
  emits no `FileParsed` event and therefore no transaction commit. `wait_for_idle` would
  wait until timeout. The test only verifies the service handles deletion without panicking,
  not that specific DB state results. A bounded sleep is the correct tool.

## Implementation Steps

- [x] Remove `Arc<RwLock<UnboundedReceiver<BeliefEvent>>>`. Move `accum_rx` directly into
      the transaction task closure (no wrapper).
- [x] Add `compiler_idle_notify: Arc<tokio::sync::Notify>` to `FileUpdateSyncer`. Fire it
      in the compiler task immediately after `compiler_idle_flag.store(true, ...)`.
- [x] Add `broadcast::channel<BeliefEvent>` in `FileUpdateSyncer::new`. Store the
      `broadcast::Sender` in `FileUpdateSyncer`. Transaction task holds a `broadcast::Sender`
      clone; it sends each event to the broadcast channel alongside the DB write.
- [x] Rewrite the transaction task loop: `select!` on `accum_rx.recv()` (process event,
      accumulate batch) and `compiler_idle_notify.notified()` (drain remainder, commit,
      signal idle if `compiler_idle == true` and channel now empty).
- [x] Update the post-commit idle check: only increment `commit_generation` and fire
      `commit_notify` when the channel is empty AND `compiler_idle == true`.
- [x] Remove `compiler_accum_rx`, `compiler_commit_generation`, `compiler_commit_notify`
      clones from the compiler task (the compiler no longer inspects the transaction channel).
- [x] Remove the `transaction_compiler_idle`-gated `notify_waiters` call added in the
      partial Phase 1 fix.
- [x] Verify `cargo test --features service --test cache_invalidation_test` passes 5/5
      locally on Linux.
- [ ] Push to CI and confirm green on `ubuntu-latest` and `windows-latest` for 3
      consecutive runs.

## Success Criteria

- [x] `cargo test --features service --test cache_invalidation_test` passes 5/5 on Linux.
- [ ] CI passes on both `ubuntu-latest` and `windows-latest` for at least 3 consecutive
      runs after the fix lands.
- [x] The 3-second TTL removal task in `FileUpdateSyncer::new` is gone.
- [x] `wait_for_idle` is implemented and gated on `feature = "service"` only.
- [x] `BuildonomyError::Timeout` variant exists.
- [x] No `std::thread::sleep` used as a synchronization primitive in
      `cache_invalidation_test.rs` (two logically-required sleeps remain with comments).
- [x] `RwLock<UnboundedReceiver>` is gone; receiver is exclusively owned by transaction task.
- [x] Transaction task is notification-driven (`select!`), not polling (`sleep(1s)`).
- [x] `broadcast::Sender<BeliefEvent>` is accessible from `FileUpdateSyncer` for LSP use.
- [x] `commit_generation` / `commit_notify` are only fired at true pipeline idle
      (channel empty AND `compiler_idle == true`).

## Risks

- Risk: `broadcast` channel capacity. If the LSP receiver is slow and the ring buffer fills,
  `broadcast::send` returns `RecvError::Lagged` on the receiver side — the sender never
  blocks. → **Mitigation**: This is the intended behavior (best-effort delivery). Document
  it in the accessor API comment. LSP should handle `Lagged` by re-querying the DB.
- Risk: `test_deleted_file_handling` sleep(7s) remains timing-sensitive on very slow CI
  runners. → **Mitigation**: The test only checks for absence of panics, not DB state, so
  false negatives are unlikely. If it becomes a problem, restructure to check a
  `FilesRemoved` event counter via the broadcast channel.
- Risk: `select!` branch fairness. If the `mpsc::recv()` branch is always ready, the
  `compiler_idle_notify` branch may starve. → **Mitigation**: `tokio::select!` is
  pseudo-random when both branches are ready; in practice the compiler goes idle well
  before the transaction task drains the last event.

## Decisions Made

- **`wait_for_idle` uses `commit_generation` not a bitmask**: An earlier design used a
  3-bit `AtomicU8` idle bitmask (one bit per pipeline stage). Abandoned because the
  transaction stage's bit reset to "idle" on every 1-second poll iteration regardless of
  whether work had been done, causing `wait_for_idle` to return before the compile cycle
  completed. The generation counter is strictly monotonic and immune to spurious-idle.

- **Lock must be dropped before `block_on`**: `wait_for_idle` collects `Arc` clones while
  holding `watchers.0.lock()`, then drops the lock before calling `self.runtime.block_on`.
  Holding the `parking_lot::MutexGuard` across `block_on` would park the calling thread
  while holding the lock, deadlocking any other code path that needs it.

- **`notify_one` must not be called inside `FileUpdateSyncer::new`**: The initial
  `work_notifier.notify_one()` is fired by `enable_network_syncer`, after the network root
  is enqueued. Calling it inside `new` caused the compiler task to wake against an empty
  queue, immediately declare itself idle, and allow `wait_for_idle` to return before any
  real work was done.

- **`wait_for_idle` is a public API, not test-only**: Gated on `feature = "service"`
  consistent with the rest of `WatchService`. Genuinely useful for CLI tools that need to
  ensure a parse completes before exiting (e.g. `noet build`).

- **OS write-back race is not a concern**: The compiler uses `tokio::fs::write(...).await`.
  The `.await` completes only after the syscall returns, so the file-watcher notification
  is already queued in the OS before `parse_next` returns and before the compiler declares
  itself idle. Full flush of `ignored_write_paths` on compiler-idle transition is safe.

- **`mpsc` for DB, `broadcast` for LSP/peers**: The DB must never drop events. Broadcast
  is best-effort — the correct reliability split. This also maps directly onto the
  federated model in `federated_belief_network.md` where `DbConnection` is the reliable
  replication target and LSP/peer subscribers are best-effort consumers of the event stream.

- **Transaction task owns receiver exclusively**: Sharing `UnboundedReceiver` behind an
  `RwLock` between the compiler task (for `is_empty()`) and the transaction task (for
  `try_recv()`) created lock contention and required spin-wait loops. Exclusive ownership
  by the transaction task eliminates this entirely; the compiler signals via
  `compiler_idle_notify` instead of inspecting the channel.