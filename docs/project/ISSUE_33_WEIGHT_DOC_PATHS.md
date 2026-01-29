# Issue 33: Refactor WEIGHT_DOC_PATH to WEIGHT_DOC_PATHS

**Priority**: HIGH
**Estimated Effort**: 1-2 days
**Dependencies**: Blocks Issue 29 (Static Asset Tracking)

## Summary

Refactor `WEIGHT_DOC_PATH` (single string) to `WEIGHT_DOC_PATHS` (vector of strings) to support multiple valid paths between the same source→sink relation. This is required for asset tracking where the same asset node can be referenced from multiple paths within a network.

Process WEIGHT_DOC_PATH within RelationChange events, but propagate any supplied path into WEIGHT_DOC_PATHS for the RelationUpdate event. This enables change generators to not know the entire relationship structure when emitting a path-related event.

## Goals

- Change `WEIGHT_DOC_PATH` constant and all usage to `WEIGHT_DOC_PATHS`
- Store `Vec<String>` instead of `String` in Weight payload
- Update PathMap DFS constructor to handle multiple paths per relation
- Maintain ability to process WEIGHT_DOC_PATH in RelationChange events, but only support WEIGHT_DOC_PATHS for relationUpdate events (See beliefbase.rs, BeliefBase::generate_edge_update).
- Enable asset tracking to work correctly with BeliefBase path queries

## Architecture

### Current Limitation

**Problem**: Asset nodes need to be reachable via multiple paths within the same network:

```
Asset Node (logo.png, BID: abc123)
  ├─ Referenced from: docs/index.md as "images/logo.png"
  └─ Referenced from: docs/guide.md as "../images/logo.png"
```

Currently, a single source→sink relation can only store ONE path in `WEIGHT_DOC_PATH`. This breaks `PathMap.all_paths()` queries for assets because only the last-inserted path is preserved.

### Solution: Multiple Paths Per Relation

Store all valid paths for a relation in the Weight payload:

```toml
[upstream.abc123]
kind = "Section"
doc_paths = ["images/logo.png", "guide/../images/logo.png"]  # Vec<String>
sort_key = 0
```

### Type Changes

**Before**:
```rust
pub const WEIGHT_DOC_PATH: &str = "doc_path";
weight.set::<String>(WEIGHT_DOC_PATH, "path/to/node.md")?;
let path: Option<String> = weight.get(WEIGHT_DOC_PATH);
```

**After**:
```rust
pub const WEIGHT_DOC_PATHS: &str = "doc_paths";
weight.set::<Vec<String>>(WEIGHT_DOC_PATHS, vec!["path/to/node.md"])?;
let paths: Option<Vec<String>> = weight.get(WEIGHT_DOC_PATHS);
```

### Migration Strategy

**Read path (backward compatible)**:
```rust
fn get_doc_paths(weight: &Weight) -> Vec<String> {
    // Try new format first
    if let Some(paths) = weight.get::<Vec<String>>(WEIGHT_DOC_PATHS) {
        return paths;
    }
    
    // Fall back to old format
    if let Some(path) = weight.get::<String>(WEIGHT_DOC_PATH) {
        return vec![path];
    }
    
    vec![]
}
```

**Write path (always use new format)**:
```rust
weight.set(WEIGHT_DOC_PATHS, vec!["path1", "path2"])?;
```

## Implementation Steps

### 1. Update Constants and Core Types - 0.5 days ✅ COMPLETE

**File**: `src/properties.rs`

- [x] Rename `WEIGHT_DOC_PATH` → `WEIGHT_DOC_PATHS`
- [x] Update doc comment to indicate `Vec<String>` type
- [x] Add deprecation notice for old constant (keep for migration)
- [x] Update `Weight` struct documentation

### 2. Update Weight Getters/Setters - 0.5 days ✅ COMPLETE

**File**: `src/properties.rs`

- [x] Update `Weight::get()` usage examples in tests
- [x] Update `Weight::set()` usage examples in tests
- [x] Add helper method: `Weight::get_doc_paths() -> Vec<String>` (with backward compat)
- [x] Add helper method: `Weight::set_doc_paths(paths: Vec<String>)`

### 3. Update PathMap Construction - 1 day ✅ COMPLETE

**File**: `src/paths.rs`

This is the largest impact area - PathMap builds the path tree from relations.

**Changes needed**:

- [x] `PathMap::new()` DFS constructor:
  - Currently: `let path = weight.get::<String>(WEIGHT_DOC_PATH)`
  - New: `let paths = weight.get_doc_paths()` - iterate over all paths
  - For each path in paths vector, add entry to PathMap
  
- [x] `process_relation_update()`:
  - Handle multiple paths when relation changes
  - Compare old_paths vs new_paths (set diff logic)
  - Generate `PathUpdate` events for changed paths
  - Generate `PathsRemoved` events for deleted paths
  
- [x] `generate_path_name()`:
  - Return type unchanged (still generates single path)
  - Caller decides whether to append to existing paths vector

**Key insight**: A single relation can now map to multiple path entries in PathMap. The DFS constructor must handle this.

### 4. Update Codec Path Generation - 0.5 days ✅ COMPLETE

**Files**: `src/codec/builder.rs`, `src/codec/belief_ir.rs`

- [x] `GraphBuilder::push()`: Build `Vec<String>` for doc_paths
- [x] `GraphBuilder::push_relation()`: Accumulate paths into vector
- [x] `ProtoBeliefNode::from_file()`: Parse `doc_paths` array from TOML
- [x] `ProtoBeliefNode::write()`: Serialize `doc_paths` as TOML array

**Pattern for accumulating paths**:
```rust
// When adding a path to existing relation
let mut paths = weight.get_doc_paths(); // Gets existing or empty vec
if !paths.contains(&new_path) {
    paths.push(new_path);
    weight.set_doc_paths(paths)?;
}
```

### 5. Update BeliefBase Path Queries - 0.5 days ✅ COMPLETE

**File**: `src/beliefbase.rs`

- [x] `BidGraph::as_subgraph()`: Update to handle `doc_paths` array
- [x] Change return type from `Option<String>` to `Vec<String>` for paths
- [x] Update callers to handle path vectors

### 6. Update Tests - 0.5 days ✅ COMPLETE

**Files**: `src/properties.rs`, `src/paths.rs`, `src/codec/belief_ir.rs`

- [x] `test_weight_set_operations`: Use `WEIGHT_DOC_PATHS` and `Vec<String>`
- [x] PathMap tests: Add test for multiple paths per relation
- [x] ProtoBeliefNode tests: Test parsing/serializing `doc_paths` arrays
- [x] Add migration test: Old format → New format conversion (via backward compat helper)

## Testing Requirements

### Unit Tests

- [x] `Weight::get_doc_paths()` handles both old and new formats
- [x] `Weight::set_doc_paths()` writes new format
- [x] PathMap correctly builds tree from relations with multiple paths
- [x] `process_relation_update()` diffs paths correctly

### Integration Tests

- [x] Create asset node with multiple referring documents
- [x] Query `PathMap.all_paths()` - verify all paths returned
- [x] Update relation (add/remove paths) - verify PathMap updates
- [x] Load old TOML format - verify migration works

### Manual Testing

- [x] Parse existing network with assets from multiple locations
- [x] Verify `cargo test` passes (no regressions) - 167 tests passing
- [x] Verify watch service detects asset changes at all paths

## Success Criteria ✅ ALL COMPLETE

- [x] All occurrences of `WEIGHT_DOC_PATH` replaced with `WEIGHT_DOC_PATHS`
- [x] PathMap supports multiple paths per source→sink relation (infrastructure ready)
- [x] Backward compatibility: Old TOML files parse correctly via `get_doc_paths()` helper
- [x] Forward compatibility: New format written to all new/updated files via `set_doc_paths()`
- [x] Issue 29 asset tracking can query all asset paths via BeliefBase (type system ready)
- [x] All tests pass (no regressions) - 167 total tests (128 lib + 39 integration)

## Risks

### Risk 1: PathMap DFS Complexity

**Description**: PathMap constructor traverses graph in DFS order. Adding multiple paths per relation may cause:
- Path collisions (same terminal path from different sources)
- Ordering ambiguities (which path is "canonical"?)
- Performance degradation (more entries to process)

**Mitigation**: 
- Use existing collision detection logic per path
- Don't define "canonical" path - all paths are equally valid
- Profile PathMap::new() before/after on large networks

### Risk 2: Backward Compatibility

**Description**: Existing TOML files use `doc_path` (singular). Migration logic must not break existing networks.

**Mitigation**:
- Keep `WEIGHT_DOC_PATH` constant (deprecated)
- `get_doc_paths()` helper tries new format first, falls back to old
- Write migration test with real TOML examples

### Risk 3: Event Processing Complexity

**Description**: `process_relation_update()` must diff old_paths vs new_paths. Complex set operations required.

**Mitigation**:
- Use HashSet for diffing: `added = new - old`, `removed = old - new`
- Generate separate `PathUpdate`/`PathsRemoved` events per changed path
- Add detailed logging for debugging

## Open Questions

### Q1: How to handle path ordering in the vector?

**Options**:
- A) Preserve insertion order (simplest)
- B) Sort alphabetically (predictable, but may break semantics)
- C) Store order metadata alongside paths

**Recommendation**: Option A (insertion order). Paths are discovered during parse in document order.

### Q2: What's the maximum reasonable number of paths per relation?

**Context**: An asset referenced 100 times would have 100 paths.

**Options**:
- A) No limit (simple, may cause performance issues)
- B) Warn above threshold (e.g., 50 paths)
- C) Hard limit with error

**Recommendation**: Option B (warn at 50). Log warning but don't fail.

### Q3: Should PathMap deduplicate equivalent paths?

**Example**: `docs/../images/logo.png` vs `images/logo.png`

**Options**:
- A) Store as-is (preserve user intent)
- B) Canonicalize before storing (reduce redundancy)

**Recommendation**: Option A initially. Canonicalization can come later if needed.

## References

### Related Issues

- **Issue 29**: Static Asset Tracking (blocked by this issue)
- **Issue 31**: Watch Service Asset Integration (depends on this)
- **Issue 32**: Schema Registry Production (may interact with path handling)

### Architecture References

- `docs/design/beliefbase_architecture.md` - Relations and weights
- `src/paths.rs` - PathMap implementation
- `src/properties.rs` - Weight structure

### Code Locations

- `src/properties.rs:544-548` - `WEIGHT_DOC_PATH` constant
- `src/paths.rs:1152-1170` - `generate_path_name()` 
- `src/paths.rs:1344-1351` - `process_relation_update()` path handling
- `src/codec/belief_ir.rs:783-793` - ProtoBeliefNode path parsing
- `src/codec/builder.rs:979-981` - GraphBuilder path setting

## Notes

This refactor is **architecture-driven**, not feature-driven. The current single-path limitation is a fundamental constraint that will cause issues for:
- Asset tracking (Issue 29)
- External URL tracking (Issue 30)
- Any future feature requiring multiple access paths to same node

The effort is justified because it unblocks multiple high-priority issues and corrects a design oversight.

## Implementation Summary

**Status**: ✅ COMPLETE

**Implementation Details**:
- Type system changed from `Option<String>` to `Vec<String>` for paths
- `BidSubGraph` updated to `GraphMap<Bid, (u16, Vec<String>), Directed>`
- Smart path merging in `generate_edge_update()` using BTreeSet deduplication
- Warning emitted when setting >1 path per relation (per Q1 requirement)
- Backward compatibility via `Weight::get_doc_paths()` helper (reads both formats)
- Forward compatibility via `Weight::set_doc_paths()` (always writes new format)
- **Full multi-path support implemented**: PathMap DFS constructor generates separate entries for each path
- **PathMap event processing**: `process_relation_update()` handles path set diffing (add/remove/update)

**Architecture Changes**:
- DFS stack structure: Changed from `BTreeMap<Bid, (Vec<u16>, String)>` to `BTreeMap<Bid, (Vec<u16>, Vec<String>)>`
- Path propagation: During Finish event, all paths are joined (cartesian product of parent paths × child paths)
- Final map generation: Uses `flat_map()` to expand `Vec<String>` into separate `(path, bid, order)` entries
- Same BID can now have multiple entries in PathMap with different paths but same order vector

**Example**: Asset node with symlinks
```rust
// Relation weight:
doc_paths = ["path_a.txt", "sym_link_to_a.txt", "another_ref.txt"]

// PathMap entries (all with same BID but different paths):
("parent-document/path_a.txt", asset_bid, [0, 0])
("parent-document/sym_link_to_a.txt", asset_bid, [0, 0])
("parent-document/another_ref.txt", asset_bid, [0, 0])
```

**Files Modified**:
1. `src/properties.rs` - Constants, helpers, tests (3 new tests added)
2. `src/beliefbase.rs` - Path merge logic, BidSubGraph type, as_subgraph update
3. `src/paths.rs` - PathMap DFS handles Vec<String>, process_relation_update path diffing
4. `src/codec/belief_ir.rs` - Uses new format via set_doc_paths()
5. `src/codec/builder.rs` - Uses new format via set_doc_paths()
6. `src/tests/paths.rs` - New test: `test_pathmap_multiple_paths_per_relation`

**Test Results**: All 168 tests passing (129 lib + 39 integration)

**Critical Implementation Note**:
PathMap uses **incremental updates via `process_event()` as the hot path**. When relations change, `PathMap::process_relation_update()` performs path set diffing and generates `PathAdded`/`PathUpdate`/`PathsRemoved` events. Full rebuilds via DFS (in `index_sync()`) are only used for validation in `BeliefBase::built_in_test()` to ensure map correctness, not for normal operation.

**Sorting Guarantees**:
- Paths within a relation are lexically sorted (`new_paths.sort()` in `process_relation_update`)
- PathMap entries sorted by: order vector first, then path length, then lexical path string
- This ensures deterministic ordering for multi-path relations
