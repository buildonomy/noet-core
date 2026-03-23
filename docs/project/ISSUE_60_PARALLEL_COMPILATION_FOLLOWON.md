# Issue 60: Parallel Compilation Follow-On

**Priority**: HIGH
**Estimated Effort**: 3 days (RELATIVE COMPARISON ONLY)
**Dependencies**: Issue 57 (complete)
**Related**: Issue 56 (PathMap protocol observability)
**Status**: Active

## Summary

Issue 57 delivered ×7.8 parallel speedup on the MDN JS corpus. Four threads
remain: two failing integration tests with known fixes, an `AnchorPath`
directory-path bug causing inconsistent `NodeKey::Path` lookups, and a
systematic attempt-3 requeue affecting ~85% of the attempt-2 population whose
root cause is unknown. This issue also tracks the ongoing pattern of perf
regressions as each fix exposes the next bottleneck.

## Goals

1. Fix `AnchorPath` directory-path mangling in `build_path_key` and
   `get_parent_from_stack` — fix documented, mechanical to apply.
2. Fix `test_belief_set_builder_bid_generation_and_caching` — fix documented
   (replace bare `BeliefBase` with `BeliefAccumulator`-backed `QueryHandle`).
3. Fix `test_belief_set_builder_with_db_cache` — blocked on goal 1; verify
   after AnchorPath fix.
4. Diagnose and fix systematic attempt-3 requeue (~851 files on MDN JS corpus).
5. Diagnose `weakset/weakset` Phase 5 fan-out outlier (10.9s `terminate_stack`).

## Architecture

### Known Bug: AnchorPath Directory-Path Mangling

`AnchorPath::new(net_path)` classifies extension-less paths as files and
`filepath()` calls `dir()`, stripping the last path component. Two call sites
in `builder.rs` construct an `AnchorPath` from a known directory path and then
call `strip_prefix`:

- `build_path_key` (~L1139): `AnchorPath::new(net_path).strip_prefix(...)`
- `get_parent_from_stack` (~L1061): same pattern with `stack_path`

Both produce a repo-root-relative child path paired with the subnet's bref as
`net` — a `NodeKey::Path` that never resolves in the PathMap. Fix: use
`AnchorPath::new_dir(net_path)` (or append a trailing slash) at both sites.
They must be fixed together — fixing only one creates a cross-layer mismatch
worse than the current consistent-but-wrong state.

See `docs/design/beliefbase_architecture.md` section 2.2.

### Known Bug: Integration Test `global_bb` Mismatch

`test_belief_set_builder_bid_generation_and_caching` passes a bare `BeliefBase`
as `global_bb` to `parse_all`. `BeliefBase::drain_epoch` is a no-op, so
`global_bb` is never updated between epochs — the parent network's BID is
invisible to child epochs, producing fresh time-based BIDs and EXTRA nodes.

Fix: replace bare `BeliefBase` with `BeliefAccumulator::new(BeliefBase::empty(),
rx)` and pass its `QueryHandle` as `global_bb`, matching the production path.

### Unknown: Systematic Attempt-3 Requeue

On the MDN JS corpus (`/tmp/jobs-8-newfix.log`), the attempt distribution is:

```
attempt 1:  1008 records
attempt 2:   851 records
attempt 3:   851 records
```

Every attempt-2 file is also reaching attempt-3 — the entire remainder
population is being requeued a second time. This is not link-resolution churn
(which would produce a sparse, file-specific pattern). Suspects:

- **Gateway-tier depth-change warnings** (8,915 occurrences in the log): the
  warning text says "dependents NOT re-queued" but something may be. Check
  whether `process_one_parse_result` has a code path that requeues on
  `PathOrderDepthChanged`.
- **`ReparseLimitExceeded` count**: with `max_reparse_count=2`, a file reaching
  attempt 3 means it was enqueued for a third parse. Verify whether this is
  `ReparseLimitExceeded` sentinel files being silently re-enqueued or genuine
  third parses completing.
- **Asset requeueing in remainder loop**: `parse_all` seeds `remainder_queue`
  with cached assets from `session_bb` after epoch 0. If assets produce
  downstream `process_asset_reference` → requeue events during attempt-2 passes,
  those could systematically re-enqueue the full population.

Diagnosis approach: add attempt-count logging to `process_one_parse_result` and
correlate with the depth-change and asset-requeue warning sites.

### Unknown: `weakset/weakset` Phase 5 Fan-Out

`weakset/weakset/index.md` shows 10.9s and 6.87s Phase 5 gaps across two
attempts. Phase 5 is `terminate_stack` + event fan-out. 10 `RelUpdates` is low
— the cost is not relation count. Suspect: the node sits at a position in the
PathMap that triggers an O(N) walk during `update_relation` (e.g. a deeply
nested subnet whose ancestor chain is re-traversed on each edge update). Needs
profiling or targeted logging in `terminate_stack` / `PathMap::update_relation`.

### Perf Hardening Pattern

Issue 57's fix sequence exposed three sequential bottlenecks:
- **Bug 1** (Phase 4 panic) → fixed
- **Bug 2** (DB "no such table") → fixed
- **Fix B** (asset O(tasks) per epoch) → fixed
- **Fix C** (api_key mutex serialization O(tasks) per epoch) → fixed

Each fix reduced the dominant cost and revealed the next. The remaining Phase 0
outliers (3.74s top, mean 0.20s) suggest further contention points remain.
Deferred items from Issue 57 that may be relevant:

- **Fix A** (epoch-0 `global_bb` stub): pass `BeliefBase::empty()` to epoch-0
  parallel tasks. Blocked on type-system: `parse_epoch<B>` uses one concrete `B`
  throughout; cannot swap it at the task-closure boundary without a second type
  parameter or trait-object. Implement only if `parse_log.py --warnings` shows
  epoch-0 `eval_unbalanced` hits.
- **`sync_asset_snapshot` scope-narrowing**: currently issues one `global_bb.eval`
  per namespace per epoch boundary (O(1) per epoch). Could be narrowed to assets
  reachable from the current epoch's parent network node via an upstream-1 query.
  Not blocking; revisit if `sync_asset_snapshot` appears in profiles.

## Implementation Steps

1. Fix `AnchorPath` mangling (0.5 day)
   - [ ] Replace `AnchorPath::new(net_path)` with `AnchorPath::new_dir(net_path)`
         at `build_path_key` (~L1139) and `get_parent_from_stack` (~L1061) in
         `src/codec/builder.rs`.
   - [ ] Verify `get_parent_from_stack` `starts_with` filter also uses
         `AnchorPath::new_dir` at ~L1032.
   - [ ] Run `cargo test`; confirm no new failures.

2. Fix integration test `global_bb` (0.5 day)
   - [ ] In `test_belief_set_builder_bid_generation_and_caching`: replace bare
         `BeliefBase` with `BeliefAccumulator`-backed `QueryHandle`.
   - [ ] Run test; confirm no EXTRA nodes, no WRITTEN-but-not-cached BIDs,
         second parse produces no graph events.
   - [ ] Run `test_belief_set_builder_with_db_cache`; if it still fails,
         investigate `DbConnection::resolve_net_path` as suspect.

3. Diagnose attempt-3 requeue (1 day)
   - [ ] Add per-attempt logging to `process_one_parse_result` at the requeue
         decision point; re-run corpus.
   - [ ] Correlate with gateway-tier depth-change warnings and asset-requeue
         events.
   - [ ] Identify the code path triggering systematic requeue; fix or document
         as expected behaviour with a counter in the validation checklist.

4. Diagnose `weakset/weakset` Phase 5 outlier (0.5 day)
   - [ ] Add Phase 5 entry/exit timing to `terminate_stack` in `builder.rs`.
   - [ ] Identify whether cost is in `update_relation`, PathMap traversal, or
         event fan-out; file targeted follow-on or fix inline.

5. Step 8: `ProtoIndex` mutability for `FileUpdateSyncer` (follow-on, deferred)
   - [ ] Decide rebuild-per-cycle vs incremental mutation; see Issue 57 Step 8.

## Testing Requirements

- `test_belief_set_builder_bid_generation_and_caching` passes: no EXTRA nodes,
  no WRITTEN-but-not-cached BIDs, second parse produces no graph events.
- `test_belief_set_builder_with_db_cache` passes: second parse rewrites no
  document content.
- `cargo test --features service,bin --test codec_test` clean.
- MDN JS corpus `--jobs 8`: attempt distribution shows no attempt-3 population
  (or a documented, bounded count if some requeue is intentional).

## Success Criteria

- [ ] `AnchorPath` mangling fixed; both call sites use `new_dir`.
- [ ] `test_belief_set_builder_bid_generation_and_caching` passes.
- [ ] `test_belief_set_builder_with_db_cache` passes.
- [ ] Attempt-3 requeue root cause identified and either fixed or documented
      with an expected count in the validation checklist.
- [ ] `weakset/weakset` Phase 5 outlier understood; fix or backlog entry filed.

## Risks

- **AnchorPath fix cross-layer mismatch**: fixing only one of the two
  `build_path_key` / `get_parent_from_stack` sites makes things worse, not
  better. Both must change atomically.
  → **Mitigation**: fix both in a single commit; run the full test suite before
  merging.

- **Attempt-3 requeue may be intentional**: if assets discovered in attempt-2
  legitimately require a third pass for their referencing documents, attempt-3
  is correct behaviour. The validation checklist should document the expected
  count rather than targeting zero.
  → **Mitigation**: diagnosis step 3 will distinguish intentional from
  accidental requeue.

- **Ongoing perf regression pattern**: each fix exposes the next bottleneck.
  Phase 0 mean is now ~200ms; further gains require profiling rather than
  log-based diagnosis.
  → **Mitigation**: if Phase 0 mean rises above ~500ms on a subsequent corpus
  run, open a dedicated profiling issue before spending time on log analysis.

## Open Questions

- Is the 851-file attempt-3 population the same files every run (deterministic
  requeue) or variable (timing-dependent)? Deterministic → structural bug.
  Variable → accumulator ordering artifact.
- After the AnchorPath fix, does `test_belief_set_builder_with_db_cache` pass
  without further changes, or does `DbConnection::resolve_net_path` need work?
- Should Fix A (epoch-0 `global_bb` stub) be implemented now that the type wall
  has been identified, or deferred until `parse_log.py --warnings` shows actual
  epoch-0 `eval_unbalanced` hits?