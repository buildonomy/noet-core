# Issue 34 Session Notes

## Session 1 (2026-01-31): Root Cause Investigation

### What We Did

1. **Added diagnostic logging** to understand cache behavior:
   - `DbConnection.eval_unbalanced` - logs query results and edge counts
   - `BeliefBase::from(BeliefGraph)` - logs states/edges when creating BeliefBase
   - `PathMapMap::new` - warns when nodes in relations aren't in states

2. **Created reproduction test** `test_belief_set_builder_with_db_cache`:
   - Uses real DbConnection with SQLite database
   - Commits events via Transaction.execute()
   - Verified DB has data: 57 nodes, 66 edges, 39 paths
   - Test FAILS on second parse with duplicate content ✓ (reproduces issue)

3. **Created unit tests** in `src/beliefbase.rs`:
   - `test_beliefgraph_with_orphaned_edges` - ❌ FAILS
   - `test_pathmap_with_incomplete_relations` - ❌ FAILS
   - `test_detect_orphaned_edges` - ✅ PASSES (helper function)
   - `test_orphaned_edges_behavior` - ❌ FAILS

### Root Cause Found

**Problem**: `DbConnection.eval_unbalanced` returns incomplete BeliefGraphs

When querying cache by Path/Title/Id:
1. Query finds the specific node requested
2. Loads that node's relations (incoming/outgoing edges) from DB
3. **BUT**: Other nodes referenced in those relations are NOT loaded
4. Result: Relations graph has edges pointing to BIDs not in states
5. `BeliefBase::from(BeliefGraph)` tries to build PathMap from this incomplete data
6. PathMap construction fails (DFS can't traverse with missing nodes)
7. `BeliefBase::get()` by Path/Title/Id relies on PathMap → lookup fails
8. cache_fetch treats as cache miss → generates new BID → duplicates!

**Evidence from logs**:
```
[DbConnection.eval_unbalanced] Query returned 1 states
[DbConnection.eval_unbalanced] Loaded 13 edges from DB for 1 states
[PathMapMap::new] 8 nodes in relations but NOT in states
neither lhs or rhs contains node with source id: ...
```

**Concrete example**: Query for Node A returns Node A + edge (A→B), but Node B is missing from states.

### Next Session: Implement Fix

**Recommended approach**: Transitive node loading in `DbConnection.eval_unbalanced`

When loading a node's relations, also load any nodes referenced in those relations:
- After loading relations, check for BIDs in edges that aren't in states
- Load those missing nodes from DB
- Repeat until all referenced nodes are included
- Add depth limit to prevent infinite recursion

**Alternative**: Modify `BeliefBase::get()` to search states directly for Path/Title/Id instead of using PathMap (simpler but may impact performance).

### Files Modified
- `src/db.rs` - added logging to eval_unbalanced
- `src/beliefbase.rs` - added logging + 4 unit tests
- `src/paths.rs` - added logging to PathMapMap::new
- `src/codec/builder.rs` - added detailed logging to cache_fetch
- `tests/codec_test.rs` - added test_belief_set_builder_with_db_cache

All changes are diagnostic/testing only - no fixes implemented yet.