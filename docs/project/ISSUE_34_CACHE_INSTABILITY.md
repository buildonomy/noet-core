# Issue 34: Cache Instability - Duplicate Nodes and Orphaned Edges

**Priority**: CRITICAL
**Estimated Effort**: 3-5 days
**Dependencies**: None
**Blocks**: Production use, multi-session workflows

## Evidence (Log Files)

**Test setup**: Parse same directory multiple times with `noet watch`

**Without `--write` flag** (catastrophic growth):
- [`initial_run.log`](../../initial_run.log) - Run 1: 0 → 29 cached nodes (clean)
- [`second_run.log`](../../second_run.log) - Run 2: 29 → 56 cached nodes (DOUBLED!)
- [`third_run.log`](../../third_run.log) - Run 3: 56 cached nodes (continued growth)

**With `--write` flag** (more stable but still problematic):
- [`first_write_run.log`](../../first_write_run.log) - Run 1: 0 → 29 cached nodes (clean)
- [`second_write_run.log`](../../second_write_run.log) - Run 2: 29 → 29 nodes, 21 edges, 68 warnings
- [`third_write_run.log`](../../third_write_run.log) - Run 3: 29 → 30 nodes, 28 edges, 120 warnings

**Key observations**:
- Warnings nearly doubled: 68 → 120
- Edge count grew: 21 → 28
- "Why didn't we get our node?" warnings present in second/third runs
- "neither lhs nor rhs contains" merge warnings growing

**Note**: Log files are git-ignored. Keep until issue resolved.

## Summary

The SQLite cache accumulates duplicate nodes and orphaned edges across multiple parse sessions, leading to cache bloat and merge warnings. Without `--write` flag, node count **doubles each run** (0→29→56→112...). With `--write`, node count is more stable but edge count grows and merge warnings accumulate (68→120 warnings), indicating orphaned edge data.

**Root cause**: When `DbConnection.eval_unbalanced` returns a BeliefGraph from SQLite cache:
1. Query finds specific nodes by Path/Title/Id
2. Loads those nodes' relations (incoming/outgoing edges)
3. **BUT**: Nodes at the other end of relations are NOT loaded into states
4. Result: Relations graph has dangling references to BIDs not in states
5. PathMap reconstruction fails with incomplete data → `BeliefBase::get()` by Path/Title/Id fails
6. cache_fetch treats nodes as new → generates duplicate BIDs

**Key symptom**: `[PathMapMap::new] X nodes in relations but NOT in states`

## Goals

1. Achieve **zero cache growth** on unchanged content (nodes and edges stable)
2. Eliminate "neither lhs nor rhs contains" merge warnings on repeat parses
3. Maintain BID stability across parse sessions (with or without `--write`)
4. Preserve existing BID generation semantics for new content

## Architecture

### Current Architecture (Multi-ID Triangulation)

From `beliefbase_architecture.md` § 2.2.3:

**Identity Resolution Hierarchy** (should prevent BID instability):
1. **BID** - Most explicit, globally unique
2. **Bref** - Compact, collision-resistant  
3. **ID** - User-controlled semantic identifier
4. **Title** - Auto-generated from heading text
5. **Path** - Filesystem location

**BID Generation** (UUIDv6 time-based):
```rust
// src/properties.rs:205
pub fn new<U: AsRef<Bid>>(parent: U) -> Self {
    Bid(Uuid::now_v6(&parent.as_ref().namespace_bytes()))
}
```

**The Problem**: Despite time-based BIDs creating new UUIDs on each parse, the fallback lookups (Path, Title, ID) should find cached nodes. Instead, `cache_fetch` (builder.rs:1370-1450) queries return results but fail to match keys, logging: *"Why didn't we get our node? The query returned results. our key: {...}. query results: {...}"*

### Expected Behavior (from `test_belief_set_builder_bid_generation_and_caching`)

```rust
// Second parse of unchanged files should:
assert!(parse_result.rewritten_content.is_none());  // No changes to write
assert!(parse_result.dependent_paths.is_empty());   // No new dependencies
assert!(received_events.is_empty());                // No new BeliefEvents
```

### Cache Architecture

```
┌─────────────────────────────────────────────────────────────┐
│ Parse Session 1 (no cache)                                  │
│  ├─ Parse files → generate BIDs (time-based)                │
│  ├─ Process events → session_bb                             │
│  └─ Save to SQLite cache                                    │
│     Result: 29 nodes, 21 edges                              │
└─────────────────────────────────────────────────────────────┘
                          ↓
┌─────────────────────────────────────────────────────────────┐
│ Parse Session 2 (load from cache)                           │
│  ├─ Load 29 nodes from cache → global_bb                    │
│  ├─ Parse files → proto nodes with Path/Title keys          │
│  ├─ cache_fetch tries: doc_bb → session_bb → global_bb      │
│  ├─ Query returns nodes BUT key matching FAILS!             │
│  ├─ System treats as new nodes → generate new BIDs          │
│  └─ Save BOTH old and new nodes to cache                    │
│     Result: 56 nodes (DOUBLED!), orphaned edges             │
│     Warning: "Why didn't we get our node?"                  │
└─────────────────────────────────────────────────────────────┘
```

### With `--write` (Partial Mitigation)

When BIDs are written to frontmatter:
- First parse: Generate BIDs → write to files
- Second parse: **Read BIDs from frontmatter** → stable!
- Result: Nodes stable (29→29→30), but edges still grow, merge warnings persist

**Why partial**: Edges/relations may reference nodes that get recreated with slightly different state, causing merge issues.

## Investigation Steps

### Phase 1: Diagnose Identity Resolution Failure ✅ COMPLETE

**Root Cause Confirmed**:

1. ✅ Added comprehensive logging to `DbConnection.eval_unbalanced`, `BeliefBase::from`, and `PathMapMap::new`
2. ✅ Created reproduction test `test_belief_set_builder_with_db_cache` that successfully reproduces the issue
3. ✅ Identified the exact problem:
   - `DbConnection.eval_unbalanced` loads nodes by query (e.g., by Path/Title/Id)
   - It loads relations for those nodes (incoming/outgoing edges)
   - **BUT**: The nodes at the other end of relations are NOT loaded into states
   - When `BeliefBase::from(BeliefGraph)` is called, it tries to build PathMap
   - PathMap construction fails because relations reference non-existent nodes
   - PathMap ends up empty or incomplete
   - `BeliefBase::get()` by Path/Title/Id relies on PathMap → FAILS
   - cache_fetch can't find cached nodes → treats them as new → duplicate BIDs

**Evidence**:
```
[DbConnection.eval_unbalanced] Query returned 1 states for expr: ...
[DbConnection.eval_unbalanced] Loaded 13 edges from DB for 1 states
[PathMapMap::new] 8 nodes in relations but NOT in states: [Bid(...), ...]
neither lhs or rhs contains node with source id: ...
```

**Unit Tests Created** (in `src/beliefbase.rs`):
- `test_beliefgraph_with_orphaned_edges` - ❌ FAILS (detects orphaned edges)
- `test_pathmap_with_incomplete_relations` - ❌ FAILS (detects PathMap failure)
- `test_detect_orphaned_edges` - ✅ PASSES (helper function)
- `test_orphaned_edges_behavior` - ❌ FAILS (documents impact)

These tests will pass once Issue 34 is fixed.

### Phase 2: Design Solution

**Fix Options**:

**Option A: Load related nodes transitively**
- When loading node A's relations, also load nodes B, C that appear in those relations
- Modify `DbConnection.eval_unbalanced` to recursively fetch referenced nodes
- Ensures BeliefGraph is complete before PathMap construction
- **Pros**: Clean, maintains architecture
- **Cons**: More complex queries, potentially expensive

**Option B: Don't rely on PathMap for cache_fetch**
- Modify `BeliefBase::get()` to search states directly for Path/Title/Id matches
- PathMap only needed for rendering/UI, not identity resolution
- **Pros**: Simple fix, fast
- **Cons**: Duplicates logic, O(n) search vs O(log n) PathMap lookup

**Option C: Cache lookup uses BID directly**
- Store BID in frontmatter during first parse (already done with --write)
- Cache lookup uses BID key instead of Path/Title/Id
- **Pros**: This is why `--write` flag provides stability!
- **Cons**: Doesn't help without --write flag

**Option D: PathMap handles incomplete graphs gracefully**
- Modify PathMapMap::new to skip nodes not in states
- Allow PathMap construction even with dangling references
- **Pros**: Tolerant of incomplete data
- **Cons**: PathMap may be incomplete, lookups may still fail

**Recommended**: Implement Option A (transitive loading) for correctness, with Option B as fallback for performance.

### Phase 3: Implementation

**Next Session Tasks**:

1. Implement transitive node loading in `DbConnection.eval_unbalanced`:
   - When relations reference BIDs not in states, load those nodes too
   - Recursively load until all referenced nodes are in states
   - Set depth limit to prevent infinite recursion

2. Add validation to `BeliefBase::from(BeliefGraph)`:
   - Assert no orphaned edges before constructing PathMap
   - Clear error message if validation fails

3. Update unit tests to pass after fix

4. Test with reproduction case

### Phase 4: Testing

**Unit Tests (in `src/beliefbase.rs`)**:
- [x] `test_beliefgraph_with_orphaned_edges` - Currently FAILS, will pass after fix
- [x] `test_pathmap_with_incomplete_relations` - Currently FAILS, will pass after fix  
- [x] `test_detect_orphaned_edges` - PASSES (helper function)
- [x] `test_orphaned_edges_behavior` - Currently FAILS, will pass after fix

**Integration Tests**:
- [x] `test_belief_set_builder_with_db_cache` - Currently FAILS, reproduces issue
- [ ] Extend to test 3 iterations after fix
- [ ] Assert node/edge counts stable across runs

**Manual Validation**:
- [ ] Run on actual test repository 3 times
- [ ] Verify: `Cached node count: 29 → 29 → 29` (stable)
- [ ] Zero "Why didn't we get our node?" warnings
- [ ] Zero "neither lhs nor rhs contains" warnings

## Testing Requirements

### Unit Tests (src/beliefbase.rs)

- [x] `test_beliefgraph_with_orphaned_edges` - Detects orphaned edges symptom
- [x] `test_pathmap_with_incomplete_relations` - Detects PathMap failure symptom
- [x] `test_detect_orphaned_edges` - Helper to identify orphans
- [x] `test_orphaned_edges_behavior` - Documents behavior with orphans

**Status**: 3 of 4 tests currently FAIL (expected - they detect the bug). Will PASS after fix.

### Integration Tests (tests/codec_test.rs)

- [x] `test_belief_set_builder_with_db_cache` - Reproduces Issue 34 with DbConnection
- [ ] Test 3-iteration parse stability after fix
- [ ] Test with/without `--write` flag
- [ ] Test cache loading and merging

### Regression Tests

- [ ] `test_belief_set_builder_bid_generation_and_caching` must pass
- [ ] All existing `codec_test.rs` tests must pass  
- [ ] Manual test: parse same repo 3 times, check SQLite counts stable

## Success Criteria

- [ ] **Zero cache growth on unchanged content**: Nodes and edges stable across runs
- [ ] **Zero merge warnings on repeat parses**: "neither lhs nor rhs" eliminated
- [ ] **All tests pass**: Including extended `test_belief_set_builder_bid_generation_and_caching`
- [ ] **Manual validation**: Test repo shows stable cache (29→29→29 nodes)

## Risks

### Risk 1: BID Migration for Existing Caches

**Impact**: Users with existing SQLite caches may have orphaned data

**Mitigation**:
- Add cache repair command: `noet cache validate --repair`
- Prune orphaned edges on cache load
- Document migration process

### Risk 2: Breaking Change to BID Semantics

**Impact**: Changing from UUIDv6 to content-hash breaks existing BIDs

**Mitigation**:
- Use **Option B** (cache-first) instead of Option A (content-hash)
- Preserve UUIDv6 generation, only add cache lookup
- Existing BIDs remain valid

### Risk 3: Performance Impact of Cache Lookups

**Impact**: Checking cache before every BID generation may slow parsing

**Mitigation**:
- Cache lookups are read-only (fast)
- Use PathMap for O(log n) lookup by path
- Only lookup when node has no explicit BID in frontmatter

## Open Questions

### Q1: Should we migrate existing caches?

**Context**: Existing SQLite caches may contain duplicate nodes and orphaned edges

**Options**:
- A) Auto-repair on load (prune duplicates/orphans)
- B) Warn user, provide repair command
- C) Leave as-is, only fix new data

**Decision**: TBD after Phase 1 investigation reveals extent of corruption

### Q2: Should we change to content-addressed BIDs long-term?

**Context**: UUIDv6 time-based generation should be fine if multi-ID triangulation works

**Options**:
- A) Keep UUIDv6, fix identity resolution (Path/Title/ID lookups)
- B) Migrate to UUIDv5 only if resolution can't be fixed
- C) Add explicit pre-generation cache lookup as workaround

**Decision**: Fix identity resolution first (Option A). UUIDv6 is not the problem if Path lookups work correctly.

### Q3: What about assets?

**Context**: Assets use content-addressing already (`buildonomy_asset_bid`)

**Question**: Are asset BIDs stable across parses?

**Investigation**: Check if asset cache stability is better than document cache

## References

### Related Issues

- Issue 29: Static Asset Tracking (content-addressed BIDs for assets)
- Issue 14: Naming Improvements (BID/Bref/NodeKey semantics)

### Architecture References

- `docs/design/beliefbase_architecture.md` § 2.2 (Identity Management)
- `docs/design/beliefbase_architecture.md` § 3.4 (BeliefBase vs BeliefGraph)

### Test References

- `tests/codec_test.rs::test_belief_set_builder_bid_generation_and_caching`
- `tests/codec_test.rs::test_asset_content_addressing`

## Implementation Progress

### Session 1 (2026-01-31): Root Cause Identified

**Investigation Results**:

1. ✅ Added comprehensive diagnostic logging to:
   - `DbConnection.eval_unbalanced` (db.rs)
   - `BeliefBase::from(BeliefGraph)` (beliefbase.rs)
   - `PathMapMap::new` (paths.rs)

2. ✅ Created reproduction test `test_belief_set_builder_with_db_cache`:
   - Uses real DbConnection with SQLite
   - Commits events via Transaction
   - Second parse fails with duplicate content (reproduces issue)

3. ✅ **ROOT CAUSE CONFIRMED**:
   - `DbConnection.eval_unbalanced` queries for specific nodes (e.g., by Path)
   - Loads relations (edges) for those nodes from DB
   - **Relations reference other nodes NOT in the query results**
   - When `BeliefBase::from(BeliefGraph)` constructs PathMap, it fails
   - PathMap has orphaned edges → DFS can't build paths → lookups fail
   - cache_fetch can't find nodes → generates duplicates

4. ✅ Created 4 unit tests that detect the symptom:
   - `test_beliefgraph_with_orphaned_edges` - ❌ FAILS
   - `test_pathmap_with_incomplete_relations` - ❌ FAILS
   - `test_detect_orphaned_edges` - ✅ PASSES (helper)
   - `test_orphaned_edges_behavior` - ❌ FAILS

**Key Evidence**:
```
[DbConnection.eval_unbalanced] Query returned 1 states
[DbConnection.eval_unbalanced] Loaded 13 edges from DB
[PathMapMap::new] 8 nodes in relations but NOT in states
ISSUE 34: Found 1 orphaned edges in relations but not in states
```

**Next Session**: Implement fix (transitive node loading in eval_unbalanced)

## Notes

- This issue blocks production use of noet in watch mode or multi-session workflows
- The cache instability compounds over time (warnings nearly doubled: 68→120)
- Asset BIDs use content-addressing and may not have this issue
- The architecture is sound (multi-ID triangulation should work), but implementation has a bug
- Most likely cause: PathMap not synced when loading from SQLite, or Path keys don't match format
- UUIDv6 time-based BIDs are a red herring - the fallback lookups should handle this
- Consider adding `--reset-cache` flag for users to start fresh if corruption occurs