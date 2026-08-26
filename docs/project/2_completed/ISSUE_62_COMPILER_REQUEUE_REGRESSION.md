# Issue 62: Compiler Re-queue Regression — current_batch Sibling Awareness

**Priority**: HIGH (broke bid_tests idempotency suite — second parse generated spurious events)
**Status**: COMPLETE
**Estimated Effort**: 1 day (RELATIVE COMPARISON ONLY)
**Dependencies**: None

## Summary

A refactor of `process_one_parse_result`'s self-requeue condition introduced two
bugs that together prevented leaf documents in sibling-batched networks from
re-queuing themselves after Phase 2. The result: links stayed unresolved on pass
1, no `inject_context`, no stable BIDs written to disk, and `compute_diff` on
pass 2 emitted spurious `NodeUpdate(Remote)` + `RelationUpdate(Remote)` events —
failing the `bid_tests` idempotency assertion.

## Root Cause

The refactor replaced `!unresolved_refs.is_empty()` with `any_dependency_enqueued`
as the self-requeue condition. The intent was correct (avoid O(n) requeue churn for
permanent externals that can never resolve), but two bugs made the condition always
false for same-batch siblings:

### Bug 1 — Already-processed deps appear pre-incremented

In `parse_sequential`, all leaf files are pre-incremented to `processed=1` before
any of them runs (the pre-increment invariant). So when file A resolves a dependency
on sibling file B, `process_unresolved_reference` saw `already_processed=true` for B
and returned `false`. `any_dependency_enqueued` stayed false → no self-requeue →
file A was never re-parsed in Phase 3 of pass 1.

### Bug 2 — Id-keyed wikilinks invisible to re-queue logic

Wikilinks (`[[HSTP]]`, `[[The Spatial Web Standards]]`) produce
`NodeKey::Id { net: nil_bref, id: ... }` in `other_keys`. The re-queue path called
`as_unresolved_source()` which requires `NodeKey::Path` and returns `None` for
Id-keyed refs. The `process_unresolved_reference` call was never reached →
`any_dependency_enqueued` stayed false even for Incoming wikilink refs to
same-batch siblings.

## Fix

Three changes in `src/codec/compiler.rs`:

### 1. `current_batch: HashSet<PathBuf>` field on `DocumentCompiler`

Tracks the set of paths in the currently-dispatched batch. Set before each batch
loop (depth groups, leaf batch, remainder candidates in `parse_sequential`; batch
results in `process_epoch_batch_results`), cleared after. Allows distinguishing
same-batch siblings (pre-incremented but not yet parsed) from already-processed
deps from prior batches.

### 2. `process_unresolved_reference` tail return

Changed from `false` to `self.current_batch.contains(&canonical_dep_path)`:

| Case | Before | After |
|------|--------|-------|
| Newly enqueued dep | `true` (early return) | `true` (unchanged) |
| Same-batch sibling | `false` | `true` ← fixed |
| Already-processed dep from prior batch | `false` | `false` (correct) |
| Permanent external | `false` | `false` (unchanged) |

### 3. Id-keyed Incoming unresolved refs

For refs where `as_unresolved_source()` returns `None` (wikilinks produce
`NodeKey::Id`), added an `else if` branch: when `direction == Incoming`,
`other_keys` contains a `NodeKey::Id`, AND `current_batch` is non-empty →
set `any_corpus_dependency = true`. Any same-batch sibling could be the
Id target; re-queue self to resolve after the batch completes.

## Key Insight

The pre-increment invariant (`processed` is bumped for all batch members before
any file runs) is correct and intentional — it prevents redundant re-enqueuing of
deps already scheduled. The missing piece was batch-sibling awareness: a dep that
looks `already_processed` may actually be a same-batch sibling whose output is not
yet in `session_bb`. `current_batch` makes this distinction explicit without
changing the pre-increment invariant.

## Files Changed

| File | Change |
|------|--------|
| `src/codec/compiler.rs` | `current_batch` field; sibling-aware `process_unresolved_reference` return; Id-keyed Incoming branch |
| `src/codec/builder.rs` | `is_content_ns_key` moved to top of `cache_fetch` loop (independent cleanup landed in same session) |

## Testing Requirements

- `bid_tests` suite: all 4 tests pass (sequential/parallel × memory/db)
- Parse 2 of any corpus with sibling files and wikilinks must produce zero graph-modifying events
- Permanent externals (mailto:, out-of-corpus URLs) must NOT trigger self-requeue

## Success Criteria

- [x] `bid_tests::test_sequential_in_memory` — pass
- [x] `bid_tests::test_sequential_db` — pass
- [x] `bid_tests::test_parallel_in_memory` — pass
- [x] `bid_tests::test_parallel_db` — pass
- [x] Full `cargo test` — 409 passed, 0 failed, 3 ignored
- [x] `hsml.md` and sibling files in `net1_dir1/` correctly re-queued and re-parsed on pass 1
- [x] Wikilinks (`[[HSTP]]`, `[[The Spatial Web Standards]]`) resolve correctly on pass 2

## Notes

The regression was introduced in the same commit that added `permanently_unresolved`
to suppress re-parse storms for broken wikilinks. The two features (storm suppression
and sibling re-queue) interact through the same `any_corpus_dependency` flag; the
fix preserves both invariants by making the sibling check explicit via `current_batch`
rather than trying to infer it from `processed` counts alone.

The `is_content_ns_key` cleanup in `builder.rs` (moving the predicate to the top of
the `cache_fetch` loop for symmetric doc_bb/session_bb Trace filters) was an
independent correctness fix landed in the same session and committed together.