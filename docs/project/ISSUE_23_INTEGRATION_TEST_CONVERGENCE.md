# Issue 23: Fix Integration Test Convergence and Cache Utilization

**Priority**: CRITICAL
**Estimated Effort**: 3-5 days
**Dependencies**: Issue 04 (complete)

## Summary

The integration test `test_belief_set_builder_bid_generation_and_caching` is failing because files require multiple parse attempts even on the second parse run with a fully populated `global_bb`. This suggests fundamental issues with parse convergence, cache utilization, or event propagation that prevent the system from reaching a stable state efficiently.

This is our most realistic test environment for how noet-core should function in production, making it critical path for reliability.

## Goals

- Achieve single-pass parsing on second run with populated cache
- Eliminate unnecessary reparsing cycles (hitting 3-attempt limit)
- Ensure `dependent_paths` is empty on cached parse runs
- Maintain correct handling of legitimately unresolved references
- Preserve semantic information in `WEIGHT_SORT_KEY` gaps (intentional for unresolved refs)

## Current Behavior

**First Parse (empty global_bb)**:
- Multiple files hit 3-attempt reparse limit:
  - `/tmp/.tmp*/` (root network)
  - `link_manipulation_test.md`
  - `repeating_references.md`
- Files contain intentionally unresolved references (e.g., `[[Another Node Title]]`, `subnet1/file2.md`)
- Warnings about `WEIGHT_SORT_KEY` gaps (now fixed in Issue 04)

**Second Parse (populated global_bb)**:
- Test expects: no rewrites, no dependent_paths, no events
- **Actual**: Still hits reparse limits, `dependent_paths` non-empty
- **Assertion fails**: `assert!(parse_result.dependent_paths.is_empty())`

## Root Cause Hypotheses

### 1. Reindexing Generates Unnecessary Derivatives
When relations are removed/reindexed, `RelationUpdate` events are generated for sort key changes. These may trigger:
- PathMap updates even when structure unchanged
- Path change events that mark files as needing reparse
- Cascading reparsing of dependent files

**Evidence**: Issue 04 fixed `RelationRemoved` to call `update_relation()`, which generates reindex events.

### 2. Cache Misses Despite Populated global_bb
The second parse may not be properly utilizing cached nodes:
- Cache lookup logic may have gaps
- Node keys may not match exactly between cache and parse
- `cache_fetch()` returning Unresolved when it should return Resolved

**Evidence**: If cache worked perfectly, second parse should be trivial (no new nodes, no new relations).

### 3. Convergence Logic Issues
The reparse loop may not be detecting when system has converged:
- `dependent_paths` populated even when no substantive changes
- Parse results differ even when BeliefBase state is identical
- Diff computation between session_bb and doc_bb may be overly sensitive

### 4. Event Propagation Creates Cycles
Derivative events from one parse may trigger unnecessary updates:
- Path events cause files to be marked dirty
- Reindexing events propagate through network
- Global_bb updates trigger local_bb invalidation

## Architecture Context

**Relevant Components**:
- `DocumentCompiler::parse_all()` - orchestrates multi-pass parsing
- `GraphBuilder::cache_fetch()` - resolves nodes from cache
- `BeliefBase::update_relation()` - manages edges and reindexing
- `PathMap::process_event()` - generates path change events
- Reparse queue logic in `DocumentCompiler`

**Key Insight from Issue 04**:
Gaps in `WEIGHT_SORT_KEY` indices are INTENTIONAL - they track unresolved references in source material. These gaps should NOT trigger reparsing.

## Investigation Steps

### Phase 1: Understand What Triggers Reparsing (0.5 days)

1. Add detailed logging to `DocumentCompiler::parse_next()`:
   - What populates `dependent_paths`?
   - Which unresolved references create dependencies?
   - Which files get enqueued for reparse and why?

2. Trace second parse run with populated global_bb:
   - Which cache lookups succeed vs. fail?
   - What events are generated during "no-op" parse?
   - What causes `dependent_paths` to be non-empty?

### Phase 2: Identify Convergence Blockers (1 day)

1. Compare BeliefBase state before/after second parse:
   - Are nodes/relations actually changing?
   - Are only paths/indices changing (non-substantive)?
   - What diff events are generated?

2. Check event propagation:
   - Do derivative events from first parse affect second parse?
   - Are reindex events necessary or can they be suppressed?
   - Should path-only changes trigger reparsing?

3. Validate cache utilization:
   - Are nodes in global_bb being found during cache_fetch()?
   - Are node keys matching correctly between cache and parse?
   - Is there a systematic cache miss pattern?

### Phase 3: Implement Fix (1-2 days)

**Option A: Suppress Non-Substantive Events**
- Don't generate `RelationUpdate` for index-only changes
- Filter path events when structure unchanged
- Mark reindex derivatives as non-triggering

**Option B: Improve Cache Hit Rate**
- Fix cache_fetch() to better utilize global_bb
- Ensure node key regularization is consistent
- Pre-populate session_bb from global_bb more thoroughly

**Option C: Refine Convergence Detection**
- Distinguish "needs reparse" from "has unresolved refs"
- Don't add to dependent_paths if target doesn't exist
- Only trigger reparse if BeliefBase content actually changed

**Option D: Decouple Reindexing from Reparsing**
- Allow sort key gaps without path updates
- Generate reindex events but mark as non-actionable
- Only trigger reparse if new nodes/relations discovered

### Phase 4: Validate and Test (0.5-1 day)

1. Verify `test_belief_set_builder_bid_generation_and_caching` passes
2. Ensure other integration tests still pass
3. Confirm unresolved references handled correctly
4. Check that legitimate updates still trigger reparsing

## Success Criteria

- [ ] `test_belief_set_builder_bid_generation_and_caching` passes consistently
- [ ] Second parse with populated global_bb completes in single pass per file
- [ ] No files hit 3-attempt reparse limit on cached parse
- [ ] `dependent_paths` empty on second parse (unless new content added)
- [ ] Intentionally unresolved references don't trigger reparsing
- [ ] All other integration tests still pass
- [ ] Performance: second parse <10% time of first parse

## Testing Requirements

**Integration Tests**:
- Existing `test_belief_set_builder_bid_generation_and_caching` must pass
- Add test for cache hit rate measurement
- Add test for convergence with unresolved references

**Unit Tests**:
- Test cache_fetch() with populated global_bb
- Test event filtering (substantive vs. non-substantive)
- Test dependent_paths population logic

**Scenarios to Cover**:
1. Parse with empty cache → populate → reparse with cache (should be fast)
2. Parse with unresolved references → reparse → still unresolved (should not loop)
3. Parse with relation removal → reindex → reparse (should converge)
4. Parse with path changes only → reparse (should recognize no structural change)

## Risks

**High**: This touches core parsing/caching logic - breaking changes could ripple widely
**Mitigation**: Comprehensive test suite, incremental changes with validation

**Medium**: Fix may reveal deeper architectural issues with event propagation
**Mitigation**: Be prepared to refactor event types or propagation logic

**Low**: Performance regression if cache logic becomes too conservative
**Mitigation**: Benchmark parse times before/after

## Related Issues

- **Issue 04**: Fixed `RelationRemoved` reindexing, removed confusing warnings about index gaps
- **Issue 10**: Daemon testing relies on stable convergence behavior
- **Issue 15**: Filtered event streaming needs clean event semantics

## Notes

- WEIGHT_SORT_KEY gaps are intentional (track unresolved references) - don't "fix" them
- Test data in `tests/network_1/` has intentional unresolved references:
  - `repeating_references.md` → `[[Another Node Title]]` (doesn't exist)
  - `link_manipulation_test.md` → `subnet1/file2.md` (doesn't exist)
- These are valid test cases, not bugs to eliminate

## References

- `tests/codec_test.rs::test_belief_set_builder_bid_generation_and_caching` (line 73)
- `src/codec/compiler.rs::parse_next()` (dependent_paths population)
- `src/codec/builder.rs::cache_fetch()` (cache utilization)
- `src/beliefbase.rs::update_relation()` (reindexing logic)