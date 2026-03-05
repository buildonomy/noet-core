# Issue 53: Deterministic Synchronization for Cache Invalidation Tests

**Priority**: HIGH
**Estimated Effort**: 2 days (RELATIVE COMPARISON ONLY)
**Dependencies**: None (partially unblocked by the path-canonicalization fix in `src/db.rs`)
**See Also**: [`docs/design/federated_belief_network.md`](../design/federated_belief_network.md) — the `commit_generation` counter introduced here is the seed of the Layer 2 sequence number described in that design doc.

## Summary

The five tests in `tests/cache_invalidation_test.rs` used `std::thread::sleep` as their only
synchronization mechanism against three async pipeline stages inside `WatchService`. This
caused intermittent failures on Linux CI and consistent failures on Windows. The core
infrastructure (pipeline idle signaling, debouncer hold-off, TTL removal) has been
implemented and the tests pass locally on Linux. Windows CI validation and the transaction
poll-loop conversion remain open.

## Goals

- [x] Replace sleep-based synchronization in cache invalidation tests with a deterministic
      idle signal from `WatchService`.
- [x] Eliminate the TTL/debounce-window race in the compiler-rewrite suppression mechanism.
- [ ] Ensure all five tests pass reliably on Linux, macOS, and Windows CI.
- [x] Keep test intent clear: each test should express *what* it waits for, not *how long*.

## What Was Implemented

### Path canonicalization fix (`src/db.rs`)

`track_file_mtime` now calls `path.canonicalize()` before storing the path string. This
resolves the Windows 8.3 short-name aliasing bug where the same physical file was stored
under two different keys (e.g. `RUNNER~1` vs `runneradmin`), causing every subsequent mtime
lookup to miss or produce phantom duplicates.

### Pipeline idle signaling (`src/watch.rs`)

Two fields were added to `FileUpdateSyncer`:

**`commit_generation: Arc<AtomicU64>`** — incremented by the transaction task after each
successful `execute()` call that staged at least one event. This is a monotonically
advancing sequence number: waiting for it to exceed a snapshot taken before `enable_network_syncer`
guarantees that a full compile+commit cycle has completed since the snapshot.

**`compiler_idle: Arc<AtomicBool>`** — set `true` when both compiler queues drain to empty
(`parse_next` returns `None`), set `false` when the compiler wakes and finds work. Used by
the debouncer hold-off (see below).

`WatchService::wait_for_idle(timeout: Duration)` was added. It snapshots `commit_generation`
for each active syncer, drops the watchers lock, then calls `self.runtime.block_on` to wait
until the generation advances past the snapshot. The lock must be dropped before `block_on`
— holding a `parking_lot::MutexGuard` across `block_on` deadlocks because the calling thread
parks and cannot re-acquire the lock needed by other code paths.

`BuildonomyError::Timeout(String)` was added to `src/error.rs`.

### Debouncer hold-off and TTL removal (`src/watch.rs`)

The debouncer callback now checks `compiler_idle` as a coarse precondition before doing
anything. If the compiler is active, the callback returns immediately — notify-debouncer-full
buffers events internally and will re-deliver them after the next quiet window. This replaces
the previous 3-second TTL race as the primary guard against compiler-write false positives.

When the compiler transitions to idle (`parse_next` → `None`), it now flushes
`ignored_write_paths` entirely instead of relying on per-path TTL timers. The TTL removal
task (previously a `runtime.spawn(sleep(3000ms))` per parsed file) has been removed. This
is safe because `tokio::fs::write(...).await` completes only after the write syscall returns,
so the file-watcher notification for any compiler write is already queued in the OS before
the compiler declares itself idle.

`ignored_write_paths` is kept as the fine-grained per-file guard for the edge case where the
compiler is idle but a write event for a compiler-written file shares a debounce window with
a genuine user edit.

### Test refactor (`tests/cache_invalidation_test.rs`)

All sleep-based synchronization that preceded DB assertions was replaced with
`service.wait_for_idle(Duration::from_secs(30)).unwrap()`.

Two sleeps were intentionally kept:
- `test_stale_file_detection_and_reparse`: a 3-second sleep between the initial parse and
  the file modification ensures the new mtime is strictly greater than the cached one.
  This is logically required (mtime resolution on some platforms is 1–2 seconds), not a
  synchronization hack. The test has a comment to that effect.
- `test_deleted_file_handling`: the post-deletion wait uses `sleep(7s)` because file
  deletion emits no `FileParsed` event and therefore no transaction commit.
  `wait_for_idle` would wait forever. The test only verifies the service does not panic
  on deletion; a bounded sleep is the correct tool here.

## Pipeline Architecture (for reference)

```
test thread
  └─ writes files
  └─ calls enable_network_syncer()
       ├─ spawns debouncer  (notify-debouncer-full, fires after 2 s quiet period)
       │    └─ checks compiler_idle; if busy, defers to next window
       │    └─ if idle: filters paths, checks ignored_write_paths, enqueues
       ├─ spawns compiler task  (async, tokio)
       │    └─ sets compiler_idle=false on wake
       │    └─ emits FileParsed events into accum channel
       │    └─ sets compiler_idle=true + flushes ignored_write_paths on queue drain
       └─ spawns transaction task  (async, tokio, polls every 1 s)
            └─ executes SQL INSERT OR REPLACE INTO file_mtimes
            └─ increments commit_generation + notifies commit_notify
  └─ wait_for_idle(30s)   ← snapshots commit_generation, blocks until it advances
  └─ opens DB connection and queries
```

## Remaining Work

### Step 1: CI validation
- [ ] Confirm green on `ubuntu-latest` for at least 3 consecutive CI runs.
- [ ] Confirm green on `windows-latest` for at least 3 consecutive CI runs.
      The path-canonicalization fix should resolve the Windows failures, but this
      needs to be verified in CI — it has not been run on Windows since the fix landed.
- [ ] Remove any `#[ignore]` annotations added as temporary workarounds (none currently
      exist, but verify before closing).

### Step 2 (optional): Convert transaction task to notification-driven loop
The transaction task currently polls with `sleep(Duration::from_secs(1))`. This introduces
up to 1 second of latency between the compiler emitting `FileParsed` events and
`wait_for_idle` unblocking. For integration tests this is acceptable, but converting to a
`Notify`-driven loop would reduce latency to near-zero and simplify the idle logic.
This is a separate improvement and does not block closing this issue.

## Success Criteria

- [x] `cargo test --features service --test cache_invalidation_test` passes 5/5 on Linux.
- [ ] CI passes on both `ubuntu-latest` and `windows-latest` for at least 3 consecutive
      runs after the fix lands.
- [x] The 3-second TTL removal task in `FileUpdateSyncer::new` is gone; `ignored_write_paths`
      is flushed on compiler-idle transition instead.
- [x] `wait_for_idle` is implemented and gated on `feature = "service"` only.
- [x] `BuildonomyError::Timeout` variant exists.
- [x] No `std::thread::sleep` used as a synchronization primitive in
      `cache_invalidation_test.rs` (two logically-required sleeps remain with explanatory
      comments; see above).

## Risks

- Risk: Windows CI may still fail if the path-canonicalization fix does not fully cover all
  alias forms. → **Mitigation**: Run CI and inspect failure output; the mtime lookup now
  uses canonical paths end-to-end so this should be resolved.
- Risk: `test_deleted_file_handling` sleep(7s) remains timing-sensitive on very slow CI
  runners. → **Mitigation**: The test only checks for absence of panics, not DB state, so
  false negatives are unlikely. If it becomes a problem, the test can be restructured to
  check a different signal (e.g. a `FilesRemoved` event counter).
- Risk: The transaction task's 1-second poll loop causes `wait_for_idle` to be slow.
  → **Mitigation**: Acceptable for integration tests. Document the bound in the API comment
  (already done). See optional Step 2 above for the long-term fix.

## Decisions Made

- **`wait_for_idle` uses `commit_generation` not a bitmask**: An earlier design used a
  3-bit `AtomicU8` idle bitmask (one bit per pipeline stage). This was implemented and
  abandoned because the transaction stage's bit reset to "idle" on every 1-second poll
  iteration regardless of whether work had been done, causing `wait_for_idle` to return
  before the compile cycle completed. The generation counter is strictly monotonic and
  immune to this spurious-idle problem.

- **Lock must be dropped before `block_on`**: `wait_for_idle` collects `Arc` clones while
  holding `watchers.0.lock()`, then drops the lock before calling `self.runtime.block_on`.
  Holding the `parking_lot::MutexGuard` across `block_on` would park the calling thread
  while holding the lock, deadlocking any other code path that needs it.

- **`notify_one` must not be called inside `FileUpdateSyncer::new`**: The initial
  `work_notifier.notify_one()` was moved entirely to `enable_network_syncer`, after the
  network root is enqueued and after `commit_generation` bits are cleared. Calling it
  inside `new` caused the compiler task to wake against an empty queue, immediately declare
  itself idle, and allow `wait_for_idle` to return before any real work was done.

- **`wait_for_idle` is a public API, not test-only**: It is gated on `feature = "service"`
  consistent with the rest of `WatchService`. It is genuinely useful for CLI tools that
  need to ensure a parse completes before exiting (e.g. `noet build`).

- **OS write-back race is not a concern**: The compiler uses `tokio::fs::write(...).await`
  for all file writes. The `.await` completes only after the syscall returns, so the
  file-watcher notification is already queued in the OS before `parse_next` returns and
  before the compiler declares itself idle. Full flush of `ignored_write_paths` on
  compiler-idle transition is therefore safe.