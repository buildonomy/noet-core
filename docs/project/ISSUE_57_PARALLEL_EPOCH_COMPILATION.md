# Issue 57: Parallel Epoch Compilation

**Priority**: MEDIUM
**Estimated Effort**: 3 days (RELATIVE COMPARISON ONLY)
**Dependencies**: None hard. Issue 56 protocol model informs the merge-safety argument but does not block implementation.
**Related**: Issue 56 (PathMap protocol observability)

## Summary

`DocumentCompiler::parse_all` processes files sequentially today. Files parsed
in the same epoch (same parse-count round) are mutually independent — no
cross-file state exists at the start of each epoch. Giving each file its own
`GraphBuilder` with a private `session_bb` and running epoch tasks concurrently
via a bounded thread pool should yield near-linear wall-clock speedup on the
I/O and codec-parse phases. On a corpus like MDN (~14 000 files), epoch 0 alone
represents ~14 000 independent tasks that currently run one at a time.

Epoch 0 gets an additional simplification: because no PathMap state exists yet,
the collision-avoidance logic in `generate_path_name_with_collision_check` is
both unnecessary and a source of non-determinism under parallel execution. The
fix is to skip it entirely during epoch-0 tasks and instead rebuild the
`PathMapMap` from scratch via `index_sync(true)` after the epoch-0 merge.
`PathMap::new` produces a fully deterministic result from the relation graph
alone — no event ordering dependency, no collision-check state, BID-ordered
tiebreaking built in.

## Goals

1. Define the epoch invariant formally and encode it as a doc-comment on `parse_all`.
2. Refactor `GraphBuilder` to accept a pre-seeded `session_bb` at construction time.
3. Implement parallel epoch dispatch in `parse_all` using a bounded thread pool (`--jobs N` / `NOET_JOBS`).
4. Merge per-task `session_bb` results back into the compiler's `session_bb` after each epoch.
5. After the epoch-0 merge, rebuild `PathMapMap` from scratch via `index_sync(true)` to produce a deterministic canonical PathMap without any event-ordering dependency.
6. Validate the post-merge event stream with the ownership invariant (see Architecture).
7. Preserve the existing sequential code path behind `--jobs 1` for debugging.

## Architecture

### The Epoch Invariant

An **epoch** is the set of files sharing the same parse count at the point they
are dequeued. Files in epoch 0 have never been parsed; their nodes do not yet
exist in any `session_bb`. Files in epoch N ≥ 1 have been parsed N times; their
nodes exist in the compiler's `session_bb` from prior epochs.

**Within a single epoch, no file's parse output is an input to any other file's
parse in that same epoch.** Cross-file dependencies only flow across epoch
boundaries (a file reparsed in epoch N reads nodes written in epochs 0..N-1).
This is the invariant that makes intra-epoch parallelism safe.

### Per-Task Isolation

Each concurrent task receives:

- An **owned `GraphBuilder`** constructed with its own fresh (epoch 0) or cloned
  (epoch N ≥ 1) `session_bb`.
- A **shared read reference to `global_bb`** — already `BeliefSource + Clone`,
  no changes needed.
- Exclusive ownership of its input file path — the 1:1 file-per-task invariant
  means no two tasks touch the same path, so `rewritten_content` write-back
  inside `terminate_stack` is safe without additional coordination.

`terminate_stack` is unchanged. It computes `compute_diff(session_bb, doc_bb,
parsed_nodes)` and sends diff events over `self.tx` to the global `BeliefBase`
channel. That channel is already the integration point; parallel tasks just
produce events concurrently rather than sequentially.

### Epoch Seed Strategy

| Epoch | Per-task `session_bb` seed |
|-------|---------------------------|
| 0     | Empty (same as today's `initialize_stack` starting state) |
| N ≥ 1 | Clone of compiler's `session_bb` post-merge from epoch N-1 |

After all tasks in an epoch complete, the compiler merges each task's
`session_bb` into its own via `BeliefBase::merge`. This produces the seed for
the next epoch.

### Merge Commutativity and the Ownership Invariant

`WEIGHT_SORT_KEY` in each `RelationChange` event is the document-position index
of the relation within a single file's parse (`index as u16` in
`push_relation`). It is locally unique and stable per file regardless of which
task processes it.

`compute_diff` already enforces ownership before emitting any `Relation*` event:
it only includes an edge in the diff when `parsed_content.contains(owner)`,
where `owner` is the endpoint indicated by `WEIGHT_OWNED_BY` on that edge. This
means each task's diff events are, by construction, restricted to edges owned by
nodes that task actually parsed.

**The ownership invariant** (to be validated on the unified post-merge event
stream): every `RelationChange` / `RelationUpdate` event emitted by a task must
have its `WEIGHT_OWNED_BY` endpoint be a BID that was in that task's
`parsed_content` set. This is the predicate that `compute_diff`'s filter
enforces locally, and the same predicate that the Issue 56 `push_relation` guard
protects at the `session_bb` boundary. Verifying it on the merged stream closes
the gap between "each task is individually correct" and "the merged result is
collectively correct."

### Epoch-0 PathMap Rebuild (Determinism Without Event Ordering)

During epoch 0, no PathMap state exists yet. The event-driven
`process_relation_update` path calls `generate_path_name_with_collision_check`,
which reads `self.map` at insertion time to decide between the title-anchor path
and the bref fallback. Under parallel execution this read is
arrival-order-dependent: whichever task's diff events are applied first
determines which sibling gets the clean path name and which gets the bref
fallback. This is a genuine non-determinism source, not a positional stability
risk.

**The fix**: epoch-0 tasks skip collision-avoidance entirely (they only record
the relation graph). After the epoch-0 merge, call `index_sync(true)` (or an
equivalent `BeliefBase::rebuild_paths`) to throw away the event-driven PathMap
and rebuild `PathMapMap` from scratch using `PathMap::new`. `PathMap::new`
already handles this correctly:

- It calls `generate_terminal_path` directly, with no collision-check read of
  `self.map`.
- It sorts the final `map` by `(pathmap_order, bid)` — the BID tiebreaker is
  unique and stable, so the result is fully deterministic regardless of which
  tasks completed in which order.
- Collisions between same-titled siblings are resolved by BID ordering, not by
  arrival order.

This is not a new code path: `index_sync(true)` already builds a `PathMapMap`
via `PathMapMap::new` and compares it against the event-driven one as a
correctness check. The change is to *use* the constructor-built PathMap as the
authoritative result after epoch 0, rather than just asserting they match.

**Epoch 1+**: tasks start from the canonical PathMap produced by the epoch-0
rebuild. `generate_path_name_with_collision_check` reads stable, deterministic
`self.map` contents for all tasks in the same epoch (they all receive the same
cloned `session_bb` seed). Collision outcomes are therefore order-independent
for all subsequent epochs without any additional sorting or rebuilding.

### Shared Read-Only Filesystem Access

`initialize_stack` calls `NetworkCodec::proto` on each ancestor directory,
which calls `iter_net_docs` — a `WalkDir` scan. Two concurrent tasks parsing
sibling files in the same network will both scan that directory. This is
read-only and safe; the redundancy is acceptable and can be addressed later
with a per-epoch directory-scan cache if profiling shows it matters.

### Thread Pool and Concurrency Bound

Parallelism is bounded by a pool sized to `--jobs N` (default: `num_cpus::get()`),
exposed as `NOET_JOBS` environment variable for CI control. Blindly opening one
task per file for 14 000 files would exhaust file descriptors and tokio thread
pool capacity.

Preferred primitive: `tokio::task::spawn_blocking` for the file-read + codec
parse work (CPU-bound, benefits from OS thread parallelism), feeding results
back to an async collection loop via a `FuturesUnordered` or a bounded channel.
Alternatively, `rayon` for the CPU-bound codec work with async I/O kept on the
tokio runtime. The choice between these is an implementation decision; the pool
bound is the non-negotiable constraint.

`--jobs 1` (or `NOET_JOBS=1`) must produce byte-identical output to the current
sequential path and serves as the debugging escape hatch.

## Implementation Steps

### 1. `GraphBuilder` seeded construction (0.5 days)
   - [ ] Add `GraphBuilder::with_session_bb(session_bb: BeliefBase, ...) -> Self`
         constructor (or extend `new` with an `Option<BeliefBase>` parameter).
   - [ ] Ensure `initialize_stack` does not unconditionally clear `session_bb`
         when it is pre-seeded. Today it clears `doc_bb` only; `session_bb` is
         preserved across calls on the same builder. Verify this invariant holds
         when the builder is constructed with a cloned seed.

### 2. Thread pool and job control (0.5 days)
   - [ ] Add `--jobs N` CLI flag and `NOET_JOBS` env var to `DocumentCompiler`.
   - [ ] Default to `num_cpus::get()`. `--jobs 1` falls back to the existing
         sequential `parse_next` loop without any other code changes.
   - [ ] Implement the bounded pool (tokio `spawn_blocking` + semaphore, or
         rayon); validate that file descriptor usage stays bounded on a 14k-file
         corpus.

### 3. Parallel epoch dispatch in `parse_all` (1 day)
   - [ ] Partition the current-epoch queue into a batch: all paths with
         `self.processed.get(path) == epoch_number` (or 0 for the primary queue).
   - [ ] Dispatch each path to the pool, each task constructing its own
         `GraphBuilder` and calling `parse_content` + `terminate_stack`.
   - [ ] Collect `(ParseResult, session_bb)` from each completed task.
   - [ ] Merge all task `session_bb`s into the compiler's `session_bb` in
         alphabetical-by-path order to keep the merge itself deterministic.
   - [ ] After the epoch-0 merge, call `BeliefBase::rebuild_paths()` (wrapping
         `index_sync(true)`) to replace the event-driven `PathMapMap` with the
         constructor-built one. This is the canonical determinism fix for epoch 0
         (see Architecture).
   - [ ] Apply the reparse-stability check (`reparse_stable`, `last_round_updates`)
         across the full batch result rather than per-file.

### 4. Ownership invariant validation (0.5 days)
   - [ ] After collecting all diff events from an epoch, assert (in debug builds)
         that every `RelationChange` / `RelationUpdate` event has its
         `WEIGHT_OWNED_BY` endpoint in the `parsed_content` set of the task that
         emitted it.
   - [ ] This is the same predicate `compute_diff` enforces locally; verifying it
         on the unified stream confirms the per-task isolation is preserved through
         the merge.
   - [ ] If any violation is found, it indicates a task's `session_bb` contained
         a stale cross-document edge — the Issue 56 class of bug — and the
         `push_relation` home-network guard failed to catch it.

### 5. Expose `rebuild_paths` on `BeliefBase` (0.25 days)
   - [ ] Add `pub fn rebuild_paths(&self)` to `BeliefBase` that calls
         `self.index_dirty.store(true, Ordering::SeqCst)` then `index_sync(true)`.
   - [ ] Add a test that verifies `rebuild_paths()` after a parallel-simulated
         out-of-order merge produces the same `PathMapMap` as a sequential build
         of the same corpus.

### 6. HTML generation (0.5 days)
   - [ ] `write_fragment` is already pure file I/O after codec state is
         captured. Confirm it can run inside the task (it currently does) and
         that concurrent writes to distinct output paths are safe (they are,
         by the 1:1 file-per-task invariant).
   - [ ] If `deferred_html` (network index generation) is touched by parallel
         tasks, ensure the `HashSet` insert is done post-merge in the compiler,
         not inside the task.

### 7. PathMap determinism validation (0.5 days)
   - [ ] Run parallel and sequential builds against `NOET_BENCH_CORPUS=web/javascript`.
   - [ ] Diff PathMap output (via a test that compares `get_nav_tree()` node sets
         and their ordering between the two runs).
   - [ ] The epoch-0 `rebuild_paths()` call should make this a non-issue; confirm
         empirically and document the result in this issue and in the `parse_all`
         doc-comment.
   - [ ] Verify epoch 1+ produces stable output without any additional sorting
         (expected, given the deterministic epoch-0 PathMap seed).

## Testing Requirements

- [ ] Existing test suite passes with parallel dispatch enabled (no regressions).
- [ ] `--jobs 1` produces byte-identical output to the current implementation
      on `tests/network_1`.
- [ ] Parallel build of `NOET_BENCH_CORPUS=web/javascript` completes without
      panic or data corruption.
- [ ] Ownership invariant `debug_assert` fires zero times on the MDN corpus.
- [ ] PathMap output is byte-identical between sequential and parallel builds
      (enforced by `rebuild_paths()` after epoch 0, not by event ordering).
- [ ] File descriptor usage stays bounded (no EMFILE errors at 14k files).
- [ ] `bench_parse_throughput` shows measurable improvement over the baseline
      saved before this issue.

## Success Criteria

- [ ] `parse_all` dispatches epoch-0 tasks concurrently via a bounded pool.
- [ ] Epoch N ≥ 1 tasks use a cloned `session_bb` seed from the post-merge
      compiler state.
- [ ] Per-task `session_bb`s are merged back into the compiler's `session_bb`
      after each epoch in deterministic (alphabetical-by-path) order.
- [ ] `BeliefBase::rebuild_paths()` called after epoch-0 merge; PathMap is
      constructor-built and fully deterministic.
- [ ] Ownership invariant validated on unified event stream post-merge.
- [ ] `--jobs 1` / `NOET_JOBS=1` sequential fallback exists and is correct.
- [ ] PathMap determinism confirmed empirically for both epoch 0 and epoch 1+.
- [ ] Wall-clock improvement on MDN `web/javascript` corpus is measurable
      (target: > 2× speedup on epoch 0 on a 4-core machine).

## Risks

- **PathMap ordering instability**: parallel task completion order could produce
  non-deterministic `recursive_map` output. → **Mitigation**: `rebuild_paths()`
  after epoch-0 merge replaces event-driven PathMap with the fully deterministic
  constructor-built one. Epoch 1+ is order-independent by construction (stable
  `session_bb` seed). Empirical gate (step 7) confirms both.

- **`session_bb` merge amplifies Issue 56 class of bugs**: if a task's
  `session_bb` contains a stale cross-document edge, merging it propagates
  the corruption to all subsequent epochs. → **Mitigation**: the Issue 47 fix
  (`push_relation` home-network guard) is already in place. The ownership
  invariant `debug_assert` (step 4) catches violations at the merge boundary
  without requiring Issue 56's full protocol model to be complete first.

- **`iter_net_docs` redundant scans**: N concurrent tasks parsing siblings in
  the same network directory each call `WalkDir` on that directory during
  `initialize_stack`. Read-only and safe; redundant work only.
  → **Mitigation**: acceptable for now; a per-epoch directory-scan cache can
  be added if profiling shows it matters.

- **File descriptor exhaustion**: bounded pool prevents EMFILE at scale.
  → **Mitigation**: pool size capped at `--jobs N` (default `num_cpus::get()`);
  validate on MDN corpus as part of step 6.

## Open Questions

- Should `NOET_JOBS` default to `num_cpus::get()` or to a fixed value for CI
  reproducibility? Recommendation: default to `num_cpus::get()` at runtime;
  CI can pin via `NOET_JOBS=1` if byte-identical output is required.