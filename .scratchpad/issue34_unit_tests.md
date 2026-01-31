# Issue 34: Unit Tests for Cache Instability Symptoms

## Summary

Created 4 unit tests in `src/beliefbase.rs` that detect the symptoms of Issue 34 (cache instability) in a non-integrated manner. These tests isolate the specific failure modes without requiring full DB integration.

## Root Cause Recap

When `DbConnection.eval_unbalanced` returns a BeliefGraph from SQLite:
- Query finds specific nodes by Path/Title/Id
- Relations are loaded for those nodes (incoming/outgoing edges)
- **BUT**: The nodes at the other end of relations are NOT loaded into states
- Result: Relations graph has dangling references to BIDs not in states
- PathMap reconstruction fails with incomplete data
- `BeliefBase::get()` by Path/Title/Id fails → cache misses → duplicate BIDs

## Unit Tests Created

### 1. `test_beliefgraph_with_orphaned_edges`

**Purpose**: Simulates what happens when DbConnection returns incomplete data

**Setup**:
- Creates 3 nodes (network, doc_a, doc_b)
- States map contains only network + doc_a
- Relations graph includes edges to doc_b (which is missing from states)

**Assertions**:
- PathMap should warn about orphaned edges (logged at WARN level)
- Node A should still be findable by ID despite orphaned edges
- Node C should NOT be findable (not in states)

**Key Symptom Detected**: Relations referencing non-existent nodes


### 2. `test_pathmap_with_incomplete_relations`

**Purpose**: Tests PathMap construction with dangling references

**Setup**:
- Network + doc + section in states
- Orphan node in relations but NOT in states
- Edge from orphan to network (dangling reference)

**Assertions**:
- Should not panic despite incomplete relations
- BID lookups should still work
- Path/Title/Id lookups may fail (documents current behavior)

**Key Symptom Detected**: PathMap lookup failures when relations are incomplete

**Output**:
```
WARNING: PathMap lookup by ID failed due to orphaned edges
This is the Issue 34 symptom - cache_fetch will fail
```


### 3. `test_detect_orphaned_edges`

**Purpose**: Helper to identify orphaned edges programmatically

**Setup**:
- States with 3 nodes (net, node_a, node_b)
- Relations includes edge to orphan (not in states)

**Assertions**:
- Detects 1 orphaned edge
- Identifies the correct orphan BID

**Key Symptom Detected**: Can programmatically detect when states/relations are out of sync


### 4. `test_orphaned_edges_behavior`

**Purpose**: Documents how is_balanced() behaves with orphaned edges

**Setup**:
- Network + doc in states
- Orphan in relations (dangling reference)

**Findings**:
- `is_balanced()` does NOT currently detect orphaned edges
- BID lookups still work despite orphaned edges
- Orphan BID cannot be found (as expected)

**Key Behavior Documented**: Primary symptom detection happens during PathMap construction, not in is_balanced()


## How to Run Tests

```bash
# Run all Issue 34 symptom tests
cargo test --lib beliefbase::tests

# Run with warnings visible
RUST_LOG=warn cargo test --lib beliefbase::tests -- --nocapture

# Run specific test
cargo test --lib beliefbase::tests::test_beliefgraph_with_orphaned_edges
```

## Expected Output

All 4 tests should pass. During execution, PathMapMap::new should log warnings:

```
WARN [PathMapMap::new] 2 nodes in relations but NOT in states: [Bid(...), Bid(...)]
```

This warning is the key symptom that cache_fetch will fail.

## Integration with Issue 34 Fix

These unit tests serve as regression tests for any fix to Issue 34:

**Before Fix**: Tests pass but demonstrate the failure mode
- PathMap construction warns about orphaned edges
- Path/Title/Id lookups may fail
- This is the documented symptom

**After Fix**: Tests should still pass, and additionally:
- PathMap should handle incomplete relations gracefully, OR
- cache_fetch should avoid creating incomplete BeliefBases, OR
- BeliefBase::get() should not rely on PathMap for cache_fetch results

The tests document the problem without prescribing the solution.

## Related Files

- Test code: `src/beliefbase.rs` (lines 2628-2933)
- Issue document: `docs/project/ISSUE_34_CACHE_INSTABILITY.md`
- Reproduction test: `tests/codec_test.rs::test_belief_set_builder_with_db_cache`
- Root cause analysis: `.scratchpad/issue34_theory1.md`
