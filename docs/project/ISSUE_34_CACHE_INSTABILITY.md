# Issue 34: DbConnection vs BeliefBase Equivalence

**Priority**: CRITICAL
**Estimated Effort**: 2-3 days
**Dependencies**: None
**Blocks**: Production use, multi-session workflows

## Summary

`DbConnection` (SQLite cache) does not return equivalent results to `BeliefBase` (in-memory) for the same queries, causing cache instability, duplicate nodes, and orphaned edges across parse sessions.

**Core Issue**: `DbConnection.eval_unbalanced` and `DbConnection.eval_trace` have multiple bugs that cause them to return incomplete or incorrectly-marked BeliefGraphs compared to `BeliefBase` reference implementation.

**Impact**: Without fixes, SQLite cache accumulates duplicates (node count doubles: 0→29→56→112), PathMap reconstruction fails due to orphaned edges, cache lookups miss, and duplicate BIDs are generated.

**Status**: Phases 1-5 COMPLETE ✅ (orphaned edges, relation duplication, Trace marking for RelationIn). Remaining: Fix eval_trace SQL error + manual validation.

## Evidence (Log Files)

**Test setup**: Parse same directory multiple times with `noet watch`

**Without `--write` flag** (catastrophic growth):
- `initial_run.log` - Run 1: 0 → 29 cached nodes (clean)
- `second_run.log` - Run 2: 29 → 56 cached nodes (DOUBLED!)
- `third_run.log` - Run 3: 56 cached nodes (continued growth)

**With `--write` flag** (more stable but still problematic):
- `first_write_run.log` - Run 1: 0 → 29 cached nodes (clean)
- `second_write_run.log` - Run 2: 29 → 29 nodes, 21 edges, 68 warnings
- `third_write_run.log` - Run 3: 29 → 30 nodes, 28 edges, 120 warnings

**Key symptom in logs**:
```
[PathMapMap::new] 8 nodes in relations but NOT in states
neither lhs or rhs contains node with source id: ...
Why didn't we get our node? The query returned results.
```

**Note**: Log files are git-ignored. Keep until issue resolved.

## Goals

1. **DbConnection equivalence**: Ensure `DbConnection` returns identical BeliefGraphs to `BeliefBase` for all query types
2. **Zero cache growth**: Nodes and edges stable on unchanged content
3. **Zero warnings**: Eliminate "neither lhs nor rhs contains" merge warnings
4. **BID stability**: Maintain consistent BIDs across parse sessions
5. **Comprehensive test coverage**: Equivalence tests validate all Expression types

## Root Cause Analysis

### The Bug in DbConnection

**Current Implementation** (`src/db.rs:318-381`):

```rust
async fn eval_unbalanced(&self, expr: &Expression) -> Result<BeliefGraph, BuildonomyError> {
    let states = self.get_states(expr).await?;  // Query for specific nodes
    
    // Load ALL relations where sink OR source is in states
    let relations = if !states.is_empty() {
        let state_set = states.keys().map(|bid| format!("\"{bid}\"")).join(", ");
        
        // Query 1: WHERE sink IN (state_set)
        let sink_relations = query_relations_by_sink(&state_set);
        
        // Query 2: WHERE source IN (state_set)
        let source_relations = query_relations_by_source(&state_set);
        
        BidGraph::from_edges(sink_relations + source_relations)
    };
    
    Ok(BeliefGraph { states, relations })  // ← BUG: relations reference nodes NOT in states!
}
```

**Problem**: Relations reference BIDs not in `states`. Example:
- Query returns Node A
- Loads relations including edge (A→B)
- **Node B is NOT loaded into states**
- PathMap construction fails with orphaned edges
- cache_fetch can't find nodes → generates duplicates

## BeliefSource Equivalence Test Harness

**File**: `tests/belief_source_test.rs` (343 lines)

Validates that `DbConnection` and `BeliefBase` return identical BeliefGraphs for same queries.

**Test Coverage**:
1. `Expression::StateIn(StatePred::Any)` ✅ PASSES
2. `Expression::StateIn(StatePred::Bid([...]))` ✅ PASSES
3. `Expression::StateIn(StatePred::Schema("..."))` ✅ PASSES
4. `Expression::RelationIn(RelationPred::Any)` ✅ PASSES
5. `eval_trace(..., WeightSet::from(WeightKind::Section))` ❌ FAILS

**Test Validation**:
- Compares ALL states (including Trace nodes)
- Verifies consistent Trace marking between sources
- Compares relation structure (edge count and source→sink pairs)
- Detailed error logging shows exact divergence

**Test 5 Failure**:
```
SQL error: no such column: section
Query: SELECT * FROM relations WHERE source IN (...) AND section IS NOT NULL;
```

## Implementation Status

### Phases 1-5: COMPLETE ✅

**Phase 1**: Helper function `BeliefGraph::find_orphaned_edges()` added
- File: `src/beliefbase.rs:701-718`
- Returns sorted, deduplicated list of BIDs in relations but not in states

**Phase 2**: Fixed `DbConnection.eval_unbalanced`
- File: `src/db.rs:318-403`
- Loads missing nodes referenced in relations (orphaned edge fix)
- Uses single SQL query with OR (eliminates relation duplication)
- Marks RelationIn results as Trace (Session 5 fix)

**Phase 3**: Fixed `DbConnection.eval_trace` direction
- File: `src/db.rs:407-481`
- Marks all queried states as Trace
- Fixed query direction (source IN, not sink IN)
- Loads missing sink nodes

**Phase 4**: PathMap validation upgraded to ERROR
- File: `src/paths.rs:344-351`
- Changed orphaned edge warning to ERROR level
- Added Issue 34 reference to message
- Graceful degradation in place

**Phase 5**: BeliefSource equivalence test created
- File: `tests/belief_source_test.rs`
- Tests 1-4 passing (StateIn, RelationIn)
- Test 5 fails with SQL error (Phase 6)

## Remaining Work

### Phase 6: Fix eval_trace WeightSet Filter ✅ COMPLETE

**Goal**: Make Test 5 pass - fix SQL error "no such column: section"

**Problem**:
```
SQL error: no such column: section
Query: SELECT * FROM relations WHERE source IN (...) AND section IS NOT NULL
```

**Root Cause**: `eval_trace` tries to filter relations by WeightSet in SQL, but:
- WeightSet contains `WeightKind::Section`
- SQL query translates this to `AND section IS NOT NULL`
- But `relations` table doesn't have a `section` column - weights are stored in edge weight data

**Solution**: Match BeliefBase behavior
1. Read `src/beliefbase.rs:2370-2443` - `evaluate_expression_as_trace`
2. Load ALL relations from DB (no weight filter in SQL)
3. Filter relations by WeightSet **in-memory** after loading
4. Build filtered BidGraph from matching edges only

**Implementation** (`src/db.rs:407-481`):
```rust
async fn eval_trace(&self, expr: &Expression, weight_filter: WeightSet) 
    -> Result<BeliefGraph, BuildonomyError> 
{
    // Get states and mark as Trace
    let mut states = self.get_states(expr).await?;
    for node in states.values_mut() {
        node.kind.insert(BeliefKind::Trace);
    }
    
    // Load ALL relations (no weight filter in SQL)
    let state_set = states.keys().map(|bid| format!("\"{bid}\"")).join(", ");
    let query = format!("SELECT * FROM relations WHERE source IN ({state_set});");
    let all_relations: Vec<BeliefRelation> = /* execute query */;
    
    // Filter relations by WeightSet IN-MEMORY (matches BeliefBase)
    let filtered_relations: Vec<BeliefRelation> = all_relations
        .into_iter()
        .filter(|rel| rel.weights.intersects(&weight_filter))
        .collect();
    
    let relations = BidGraph::from_edges(filtered_relations);
    
    // Load missing sink nodes and mark as Trace
    // (same pattern as eval_unbalanced)
    
    Ok(BeliefGraph { states, relations })
}
```

**Success Criteria**:
- Test 5 PASSES
- All 5 equivalence tests passing
- `cargo test --test belief_source_test` shows 5/5 passed

### Phase 7: Manual Validation (~1 hour)

**Goal**: Verify cache stability in real-world usage

**Steps**:
1. Delete `.noet/` cache directory
2. Run `noet watch` on test repository (Run 1)
   - Record: node count, edge count
3. Run `noet watch` again (Run 2)
   - Verify: same node count, same edge count
4. Run `noet watch` again (Run 3)
   - Verify: still stable

**Expected Results**:
- Node count stable: 29 → 29 → 29
- Edge count stable: 21 → 21 → 21
- Zero "Why didn't we get our node?" warnings
- Zero "neither lhs nor rhs contains" warnings
- Zero "X nodes in relations but NOT in states" errors

**Success**: Issue 34 complete, ready to move to `docs/project/completed/`



## Testing Requirements

### Unit Tests ✅

**src/beliefbase.rs**:
- [x] `test_detect_orphaned_edges` ✅ PASSES
- [x] `test_beliefgraph_with_orphaned_edges` ✅ PASSES
- [x] `test_pathmap_with_incomplete_relations` ✅ PASSES
- [x] `test_orphaned_edges_behavior` ✅ PASSES

### Equivalence Tests

**tests/belief_source_test.rs**:
- [x] Test 1: `StateIn(Any)` ✅ PASSES
- [x] Test 2: `StateIn(Bid([...]))` ✅ PASSES
- [x] Test 3: `StateIn(Schema("..."))` ✅ PASSES
- [x] Test 4: `RelationIn(Any)` ✅ PASSES
- [ ] Test 5: `eval_trace(..., WeightSet)` ❌ FAILS (SQL error - Phase 6)

## Success Criteria

- [x] **Orphaned edge loading**: DbConnection loads missing nodes referenced in relations (Phases 2-3 ✅)
- [x] **Relation duplication fixed**: Single SQL query eliminates 2x duplication (Phase 5.5 ✅)
- [x] **RelationIn Trace marking**: All nodes marked as Trace for RelationIn queries (Session 5 ✅)
- [x] **PathMap validation**: Orphaned edges detected with ERROR level (Phase 4 ✅)
- [x] **StateIn equivalence**: Tests 1-3 all passing ✅
- [x] **RelationIn equivalence**: Test 4 passing ✅
- [x] **eval_trace equivalence**: Test 5 passing - schema column fix applied (Phase 6 ✅)
- [ ] **Manual validation**: Stable cache with zero warnings (Phase 7 - REMAINING)

## Open Questions

### Q1: Depth limit for transitive loading?

**Context**: Current proposal only loads nodes 1-hop away (directly referenced in relations)

**Question**: Should we recursively load relations of missing nodes?

**Recommendation**: No. Single hop is sufficient. Missing nodes are marked as `Trace` to indicate incomplete relation set. If caller needs full graph, they should call `balance()`.

### Q2: Cache repair for existing corrupted databases?

**Context**: Users with existing `.noet/` caches may have orphaned data

**Options**:
- A) Auto-detect and prune on load
- B) Add `noet cache validate --repair` command
- C) Document "delete .noet/ if you see warnings"

**Recommendation**: Option C for v0.1 (simple), Option B for v1.0 (production-ready).

### Q3: Should BeliefGraph::is_balanced check for orphaned edges?

**Context**: Currently `is_balanced()` only checks for external sinks

**Recommendation**: Yes, add orphaned edge check:
```rust
pub fn is_balanced(&self) -> bool {
    self.build_balance_expr().is_none() && self.find_orphaned_edges().is_empty()
}
```

## Implementation Estimate

- Phase 1 (helper function): 30 min
- Phase 2 (fix eval_unbalanced): 1-2 hours
- Phase 3 (fix eval_trace): 1 hour
- Phase 4 (PathMap validation): 1 hour
- Phase 5 (equivalence tests): 2 hours
- Phase 6 (update existing tests): 1 hour
- Phase 7 (manual validation): 1 hour

**Total**: ~8 hours (~1 day)

## Implementation Progress

### Session 1-4 Summary (2026-01-31 to 2026-02-01)

**Phases Complete**: 1, 2, 3, 4, 5 ✅
**Status**: Core bug FIXED, defensive measures in place, test harness validates equivalence

### Session 1 (2026-01-31): Root Cause Identified ✅

**Investigation**:
1. Added diagnostic logging to `DbConnection.eval_unbalanced`, `BeliefBase::from`, and `PathMapMap::new`
2. Created reproduction test `test_belief_set_builder_with_db_cache` (FAILS - reproduces issue)
3. Created 4 unit tests that detect symptoms (3 FAIL, 1 PASSES)

**Root Cause Confirmed**:
- `DbConnection.eval_unbalanced` loads relations for queried nodes
- Relations reference other nodes NOT in query results
- PathMap construction fails with orphaned edges
- cache_fetch can't find nodes → generates duplicates

**Key Evidence**:
```
[DbConnection.eval_unbalanced] Query returned 1 states
[DbConnection.eval_unbalanced] Loaded 13 edges from DB
[PathMapMap::new] 8 nodes in relations but NOT in states
```

### Session 2 (2026-01-31): Solution Design ✅

**Code Analysis**:
- Reviewed `BeliefBase::evaluate_expression` - shows correct pattern
- Reviewed `BeliefBase::evaluate_expression_as_trace` - shows correct trace pattern
- Identified discrepancies in `DbConnection::eval_trace`:
  1. Missing orphaned node loading
  2. Doesn't mark nodes as Trace
  3. Query used `sink IN` instead of `source IN` (wrong direction for downstream trace)

**Solution Finalized**: Option A + C (Defense in Depth)
- Fix DbConnection to match BeliefBase behavior
- Add PathMap validation for safety
- Create equivalence test harness

### Session 3 (2026-02-01): Implementation Phases 1-3 ✅

**Phase 1 Completed**: Added `BeliefGraph::find_orphaned_edges()` helper method
- Returns sorted, deduplicated list of orphaned BIDs
- Updated `test_detect_orphaned_edges` to use new public method
- Updated other unit tests to verify graceful handling instead of panicking
- All 4 beliefbase unit tests now PASS ✅
- Also added `BeliefBase::find_orphaned_edges()` for direct use (e.g., in `built_in_test()`)

**Phase 2 Completed**: Fixed `DbConnection.eval_unbalanced`
- Added imports: `BeliefKind`, `StatePred`
- After loading relations, detect orphaned edges with `find_orphaned_edges()`
- Load missing nodes with single query: `Expression::StateIn(StatePred::Bid(missing))`
- Mark missing nodes with `BeliefKind::Trace`
- Extend states with missing nodes
- Compiles successfully ✅

**Phase 3 Completed**: Fixed `DbConnection.eval_trace`
- Mark all queried states as `BeliefKind::Trace` (matches in-memory behavior)
- **Fixed critical bug**: Changed `sink IN` to `source IN` (correct direction for downstream trace)
- Load missing sink nodes and mark as Trace
- Compiles successfully ✅

**Integration Test Analysis**:
- `test_belief_set_builder_with_db_cache` still fails on second parse
- DB populated correctly: 57 nodes, 66 edges, 39 paths after first parse
- But all queries on second parse return 0 results
- **Hypothesis**: Network BID regeneration on second parse causes NetPath query mismatches
- **Better Approach**: Use Phase 5 equivalence test harness to systematically compare session_bb vs db after parse_all

**Next Session**: Pivot to Phase 5 (BeliefSource equivalence tests) using `test_belief_set_builder_with_db_cache` as foundation.

**Additional Work**: Added backlog items for BeliefBase trait abstraction (Option 2) and beliefbase.rs module splitting.

### Session 4 (2026-02-01): Phase 5 + Relation Duplication Fix ✅

**Phase 5 Completed**: Created BeliefSource Equivalence Test
- New test module: `tests/belief_source_test.rs` (343 lines)
- Manually builds test BeliefBase (5 nodes, 4 relations)
- Uses `compute_diff()` to generate events, populates DB via Transaction
- Runs identical queries on BeliefBase and DbConnection
- Compares BeliefGraph results using `assert_belief_graphs_equivalent()` helper

**Test Coverage**:
- Expression::StateIn(StatePred::Any) ✅ PASSES
- Expression::StateIn(StatePred::Bid([...])) ✅ PASSES
- Expression::StateIn(StatePred::Schema("...")) ✅ PASSES
- Expression::RelationIn(RelationPred::Any) ✅ PASSES (after Session 5 fix)
- BeliefSource::eval_trace() ❌ FAILS (SQL error - separate from Issue 34)

**Critical Bug Found & FIXED**:
Test revealed DbConnection.eval_unbalanced() returned 2x relations (session=4, db=8).

**Root Cause**: Two separate SQL queries (`sink IN` + `source IN`) appended together. When edge has both source AND sink in result set, it appears twice.

**Fix Applied** (`src/db.rs:334-360`):
- Changed from two queries + append to single query with OR
- `SELECT * FROM relations WHERE sink IN (...) OR source IN (...);`
- Matches BeliefBase semantics (EITHER source OR sink in set)
- All tested queries now PASS ✅
- Phase 5 COMPLETE ✅

**Files Modified**:
1. `tests/belief_source_test.rs` - NEW FILE (equivalence test harness)
2. `src/db.rs` - FIXED eval_unbalanced relation duplication bug

### Session 5 (2026-02-01): Phase 4 Complete + Test 4 Fix ✅

**Phase 4: PathMap Validation** (30 min)
- Updated `src/paths.rs:344-351` to upgrade orphaned edge warning to ERROR
- Added Issue 34 reference: "ISSUE 34 VIOLATION: DbConnection should have loaded these"
- Validation already runs before PathMap construction
- Graceful degradation already in place
- Scratchpad files cleaned up (issue34_solution_design.md, session3/4 summaries)

**Test 4 Fix - RelationIn Trace Marking** (30 min):
- **Bug Found**: DbConnection wasn't marking nodes as Trace for RelationIn queries
- **Root Cause**: Only orphaned nodes were marked as Trace, but ALL nodes in RelationIn should be Trace
- **Fix Applied** (`src/db.rs:335-342`):
  - Added check for `Expression::RelationIn(_)` queries
  - Mark all returned nodes as Trace (matches BeliefBase behavior)
  - Comment: "we don't guarantee complete relation sets for returned nodes"
- **Test Result**: Test 4 now PASSES ✅

**Test Framework Improvement**:
- Updated `assert_belief_graphs_equivalent()` to compare ALL nodes (including Trace)
- Verifies same BID sets returned by both sources
- Verifies consistent Trace marking between sources
- Fixed comparison to include Trace nodes (they're critical to validate)

**Test 5 Failure Identified**:
- SQL error: "no such column: section"
- Query: `SELECT * FROM relations WHERE source IN (...) AND section IS NOT NULL`
- Root cause: WeightSet filter generates invalid SQL column reference
- Next session: Fix eval_trace to match BeliefBase (load all, filter in-memory)

### Session 6 (2026-02-01): Phase 6 Complete - Schema Column Fix ✅

**Phase 6: Fix eval_trace WeightSet Filter** (15 min - simpler than expected!)

**Root Cause Identified**:
- Test 5 SQL error: "no such column: section"
- **Actual bug**: Schema column name mismatch from WeightKind rename
- WeightKind::SubSection was renamed to WeightKind::Section
- Database schema CREATE TABLE had `subsection` column
- INSERT query in `update_relation()` also had `subsection`
- Tests never ran against fresh DB, so old schema persisted

**Fix Applied**:
1. `src/db.rs:552-555` - Schema already updated to `section` column (done earlier)
2. `src/db.rs:201` - Fixed INSERT query: `subsection` → `section`
3. Deleted any cached `belief_cache.db` files (none existed)
4. Reran tests against fresh schema

**Test Results**:
- Test 1 (StateIn Any): ✅ PASS
- Test 2 (StateIn Bid): ✅ PASS  
- Test 3 (StateIn Schema): ✅ PASS
- Test 4 (RelationIn Any): ✅ PASS
- Test 5 (eval_trace Section filter): ✅ PASS

**All 5 equivalence tests PASSING** ✅

**Code Cleanup**:
- Removed unused import in `tests/belief_source_test.rs:339` (petgraph::visit::EdgeRef)

**Trace Semantics Verified**:
- Reviewed `eval_unbalanced` Trace marking against BeliefBase reference
- Confirmed: Missing sink/source nodes correctly marked as Trace (relations partially loaded)
- Confirmed: RelationIn queries correctly mark ALL nodes as Trace
- DbConnection matches BeliefBase semantics ✅

**Key Insight**:
The planned implementation (load all relations, filter in-memory) was correct architectural direction, but unnecessary for this bug. The real issue was a simple schema inconsistency from incomplete rename. However, the investigation validated that DbConnection's current approach is equivalent to BeliefBase.

## Next Session Plan

### Phase 7: Manual Validation (~1 hour)

**Goal**: Verify cache stability in real-world usage

**Steps**:
1. Delete `.noet/` cache directory
2. Run `noet watch` 3 times on test repository
3. Verify stable counts (29→29→29 nodes, 21→21→21 edges)
4. Verify zero warnings
5. Mark Issue 34 COMPLETE
6. Move to `docs/project/completed/`

**Success**: Ready to mark Issue 34 complete

## References

### Test File
- `tests/belief_source_test.rs` - Equivalence test harness (343 lines)

### Implementation Files (FIXED)
- `src/beliefbase.rs:701-718` - `find_orphaned_edges()` helper
- `src/db.rs:318-403` - `eval_unbalanced` (orphaned edges + relation dedup + RelationIn Trace)
- `src/db.rs:201` - `update_relation` (schema column name fixed: subsection → section)
- `src/db.rs:407-481` - `eval_trace` (working correctly, schema fix resolved Test 5)
- `src/paths.rs:344-351` - PathMap validation (ERROR on orphaned edges)

### Reference Implementation
- `src/beliefbase.rs:2445-2594` - `evaluate_expression` (correct pattern)
- `src/beliefbase.rs:2370-2443` - `evaluate_expression_as_trace` (correct weight filtering)