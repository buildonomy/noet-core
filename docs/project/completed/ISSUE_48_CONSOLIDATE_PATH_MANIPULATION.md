# Issue XX: Consolidate Path Manipulation to Use WASM AnchorPath Functions

**Priority**: MEDIUM  
**Estimated Effort**: 1-2 days  
**Dependencies**: None  
**Blocks**: None

## Summary

Consolidate manual path manipulation in `viewer.js` to use WASM-exposed `AnchorPath` functions. Currently, JavaScript performs ad-hoc string operations (`substring`, `indexOf`, `split`) for path parsing and manipulation, which duplicates logic already implemented in Rust's `AnchorPath` and is error-prone.

**Critical Bug**: Navigation to subnet documents is broken. NavTree paths for subnet documents are missing the subnet directory prefix (e.g., storing `subnet1_file1.html` instead of `subnet1/subnet1_file1.html`), causing 404 errors when navigating to documents in subnets.

## Goals

- **Fix subnet navigation**: Ensure NavTree paths include subnet directory prefix
- Replace manual path string operations with WASM `AnchorPath` methods
- Eliminate duplicate path manipulation logic between Rust and JavaScript
- Improve maintainability (single source of truth for path semantics)
- Reduce bugs from inconsistent path parsing
- Leverage well-tested Rust path utilities

## Architecture

### Subnet Path Prefix Problem (CRITICAL BUG)

**Issue**: When building the NavTree in `src/wasm.rs::get_nav_tree()`, documents inside subnets have incorrect paths.

**Root Cause**:
- Each network (including subnets) has its own `PathMap` with paths relative to that network
- Subnet documents are in the subnet's PathMap with paths like `subnet1_file1.html`
- Parent network's PathMap has subnet entry with path like `subnet1/`
- `get_nav_tree()` iterates through each PathMap independently without prepending subnet prefix

**Example**:
```
Network structure:
  network_1/
    subnet1/BeliefNetwork.toml  (subnet network)
    subnet1/subnet1_file1.md

Parent PathMap (network_1):
  - "subnet1/" → Bid(subnet1 network)

Subnet PathMap (subnet1):
  - "subnet1_file1.html" → Bid(subnet1_file1.md)

Current NavTree (WRONG):
  path: "subnet1_file1.html"  ❌ Missing subnet1/ prefix

Expected NavTree (CORRECT):
  path: "subnet1/subnet1_file1.html"  ✅
```

**Solution Options**:

1. **Add subnet prefix during NavTree construction** (Recommended):
   - In `get_nav_tree()`, when iterating subnet PathMaps, prepend the subnet's entry path from parent
   - Requires looking up subnet's path in parent network's PathMap

2. **Add prefix field to NavNode**:
   - Store subnet prefix separately in NavNode struct
   - JavaScript concatenates during rendering
   - More complex, adds runtime overhead

3. **Store full paths in PathMap** (Not recommended):
   - Would require changing core PathMap architecture
   - Paths are intentionally relative to network for modularity

### Current State: Manual Path Manipulation

JavaScript performs manual string operations scattered throughout `viewer.js`:

```javascript
// Extracting anchor manually
const anchorIndex = path.indexOf("#");
if (anchorIndex > 0) {
    sectionAnchor = path.substring(anchorIndex);
    path = path.substring(0, anchorIndex);
}

// Removing leading hash manually
const currentHash = window.location.hash.substring(1);

// Splitting paths manually
const parts = currentPath.split("/");
```

**Problems**:
- Inconsistent handling of edge cases (empty strings, missing separators)
- Duplicate logic (Rust has same logic in `AnchorPath`)
- Harder to test (manual operations vs. tested utility functions)
- Error-prone (easy to forget edge cases)

### Target State: WASM AnchorPath Functions

Use WASM-exposed methods from `AnchorPath`:

```javascript
// Available WASM methods (already exposed):
BeliefBaseWasm.pathParts(path)       // → { path, filename, anchor }
BeliefBaseWasm.pathJoin(base, end)   // → joined path
BeliefBaseWasm.pathParent(path)      // → parent (strips anchor or gets dir)
BeliefBaseWasm.pathExtension(path)   // → extension
BeliefBaseWasm.pathFilestem(path)    // → filename without extension
BeliefBaseWasm.normalizePath(path)   // → normalized path
```

**Example Refactor**:
```javascript
// Before:
const anchorIndex = path.indexOf("#");
if (anchorIndex > 0) {
    sectionAnchor = path.substring(anchorIndex);
    path = path.substring(0, anchorIndex);
}

// After:
const parts = wasmModule.BeliefBaseWasm.pathParts(path);
const sectionAnchor = parts.anchor ? `#${parts.anchor}` : null;
path = parts.path ? `${parts.path}/${parts.filename}` : parts.filename;
```

## Implementation Steps

### 0. Fix Subnet Path Prefix Bug (2-3 hours) **PRIORITY** ✅ COMPLETE
- [x] Understand subnet structure in PathMapMap
- [x] Identify how to get subnet's entry path from parent PathMap
- [x] Add `recursive_map()` to PathMap for recursive subnet traversal
- [x] Modify `get_nav_tree()` to use recursive_map with path prefixes
- [x] Test with `tests/network_1/subnet1/subnet1_file1.md`
- [x] Verify navtree links work for subnet documents

**Implementation (COMPLETED)**:

Added `PathMap::recursive_map()` in `src/paths/pathmap.rs`:
```rust
pub fn recursive_map(
    &self,
    nets: &PathMapMap,
    visited: &mut BTreeSet<Bid>,
) -> Vec<(String, Bid, Vec<u16>)> {
    // Recursively traverse subnets, prepending their path prefixes
    // Returns flattened list of (path, bid, order) with full paths
}
```

Modified `get_nav_tree()` in `src/wasm.rs`:
```rust
// Skip subnet networks in outer loop (handled recursively)
if subnet_prefixes.contains_key(&net_bid) {
    continue;
}

// Use recursive_map instead of pm.map() to get paths with subnet prefixes
for (path, bid, order_indices) in pm.recursive_map(&paths, &mut visited).iter() {
    let html_path = Self::normalize_path_extension(path);
    // ... rest of NavNode construction
}
```

**Result**: NavTree paths now include subnet directory prefix (e.g., `subnet1/subnet1_file1.html` instead of `subnet1_file1.html`)

**Locations Identified**:
- `navigateToLink()` - L524-530, L541-554, L556-569
- `navigateToSection()` - L597-611 (already uses pathParts ✅)
- `handleHashChange()` - L653-657, L662-668
- `processLoadedContent()` - L898-915
- `getBidFromPath()` - L1006-1008
- `getActiveBid()` - L1334-1338, L1350-1356

**Operation Categories**:

1. **Extract anchor from path** (split on #):
   - `navigateToLink()` L541-554: `indexOf("#")` + `substring()`
   - `handleHashChange()` L650-657: `indexOf("#")` + `substring()`
   - `processLoadedContent()` L900-903: `indexOf("#")` + `substring()`
   - `getBidFromPath()` L1006: `split("#")[0]`
   - **Map to:** `pathParts(path)` → access `parts.anchor`

2. **Reconstruct path with anchor**:
   - `navigateToLink()` L556-561: Manual string concatenation
   - `navigateToSection()` L599-605: Already uses `pathParts()` ✅
   - `processLoadedContent()` L913: String template
   - **Map to:** Combine `pathParts()` output with new anchor

3. **Strip leading character** (`substring(1)`):
   - `navigateToLink()` L528: Remove leading # (but then uses pathParts)
   - `getActiveBid()` L1338: Remove leading # for section ID
   - **Map to:** May not need WASM call - simple string operation

4. **Path comparison**:
   - `getActiveBid()` L1352: `endsWith()` comparison
   - **Map to:** No refactor needed - string methods appropriate

**Priority Refactors** (4 locations with manual indexOf/substring):
1. `navigateToLink()` L541-554
2. `handleHashChange()` L650-657
3. `processLoadedContent()` L900-903
4. `getBidFromPath()` L1006

### 1. Audit Current Path Manipulation (1-2 hours) ✅ COMPLETE
- [x] Identify all manual path operations in `viewer.js`
- [x] Categorize by operation type (parse, join, strip anchor, etc.)
- [x] Map to corresponding `AnchorPath` method

**See `.scratchpad/path_operations_audit.md` for detailed audit findings**

### 2. Add Helper Functions (2-3 hours) ✅ COMPLETE
- [x] Create `getCurrentDocPath()` helper using `pathParts()`
- [x] Create `parseHashPath()` helper using `pathParts()`
- [x] Create `stripAnchor()` helper using `pathParts()`
- [x] Document helpers with JSDoc

**Implementation (COMPLETED)**:

Added three helper functions in `assets/viewer.js` after State section (lines 139-188):

1. **`getCurrentDocPath()`** - Get current document path from hash without anchor
2. **`parseHashPath(hash)`** - Parse hash into `{path, anchor}` object
3. **`stripAnchor(path)`** - Remove anchor from path

All helpers use `wasmModule.BeliefBaseWasm.pathParts()` for consistent path parsing.

### 3. Refactor `navigateToLink()` (1 hour) ✅ COMPLETE
- [x] Replace `indexOf("#")` / `substring()` with `parseHashPath()`
- [x] Simplified anchor detection and path reconstruction
- [x] Relative path resolution already uses `pathParts()` correctly

**Implementation (COMPLETED)**:

Replaced lines 595-601 in `assets/viewer.js`:
- **Before**: `indexOf("#")` + `substring()` to split path and anchor
- **After**: `parseHashPath(resolvedPath)` → access `parsed.path` and `parsed.anchor`

Result: Cleaner code, consistent with other path operations.

### 4. Refactor `navigateToSection()` (30 min) ✅ COMPLETE
- [x] Already uses `pathParts()` correctly
- [x] No refactoring needed

**Status**: This function was already refactored in a previous session. Lines 647-655 correctly use `wasmModule.BeliefBaseWasm.pathParts(currentHash)` to parse path and reconstruct with new anchor.

### 5. Refactor `handleHashChange()` (1 hour) ✅ COMPLETE
- [x] Replace manual anchor extraction with `parseHashPath()`
- [x] Use helper function for parsing
- [x] Simplified path and anchor extraction

**Implementation (COMPLETED)**:

Replaced lines 699-704 in `assets/viewer.js`:
- **Before**: `indexOf("#")` + `substring()` to extract anchor and path
- **After**: `parseHashPath(path)` → access `parsed.path` and `parsed.anchor`

Result: Consistent path parsing, cleaner code with 5 fewer lines.

### 6. Refactor `processLoadedContent()` (30 min) ✅ COMPLETE
- [x] Use `getCurrentDocPath()` for current document path
- [x] Simplify header anchor href generation
- [x] Removed 7 lines of manual path manipulation

**Implementation (COMPLETED)**:

Replaced lines 940-947 in `assets/viewer.js`:
- **Before**: Manual `substring(1)` + `indexOf("#")` + `substring()` to strip anchor
- **After**: `getCurrentDocPath()` helper function

Result: 8 lines reduced to 1 line, cleaner and more maintainable.

### 7. Refactor `getBidFromPath()` (15 min) ✅ COMPLETE
- [x] Use `stripAnchor()` helper to clean path
- [x] Keep leading slash stripping (required by PathMap keys)

**Implementation (COMPLETED)**:

Replaced line 1047 in `assets/viewer.js`:
- **Before**: `path.split("#")[0]` to remove anchor
- **After**: `stripAnchor(path)` helper function

Result: Consistent with other path operations, more readable.

### 8. Refactor `getActiveBid()` (30 min) ✅ COMPLETE
- [x] No refactoring needed
- [x] Simple `substring(1)` is appropriate for removing leading `#`
- [x] `endsWith()` comparison is appropriate for path matching

**Status**: This function uses minimal string operations that don't benefit from WASM helpers. The `substring(1)` call on line 1388 is the simplest way to remove the leading `#` character.

## Testing Requirements

**Test Environment Setup**:
```bash
# Build WASM and binary
./scripts/build-full.sh

# Generate test HTML
./target/debug/noet parse tests/network_1 --html-output /tmp/noet-test-output

# Start local server
cd /tmp/noet-test-output
python3 -m http.server 8888

# Open in browser: http://localhost:8888
```

**Manual Testing** (in browser at http://localhost:8888): ✅ COMPLETE
- [x] Navigate between documents (click navtree items)
- [x] Navigate to subnet document (`subnet1/subnet1_file1.html`)
- [x] Click section anchors (should preserve document path)
- [x] Navigate with relative links (../other.html)
- [x] Navigate with anchors (doc.html#section)
- [x] Use browser back/forward
- [x] Test with edge cases:
  - Empty hash
  - Hash with only anchor (#section)
  - Nested paths (dir/subdir/doc.html)
  - Paths with multiple # (should not occur, but test gracefully)

**Regression Testing**: ✅ COMPLETE
- [x] All existing navigation flows work
- [x] Two-click pattern unaffected
- [x] Metadata panel links work
- [x] Image modal links work
- [x] Header anchor links work

**Console Verification**: ✅ COMPLETE
- [x] No JavaScript errors in browser console
- [x] `parseHashPath()` calls show correct `{path, anchor}` objects
- [x] Navigation logs show proper path resolution

## Success Criteria

- [x] **Subnet navigation works**: Can navigate to `subnet1/subnet1_file1.html` from navtree
- [x] **NavTree paths include subnet prefix** for subnet documents
- [x] All manual `indexOf("#")` replaced with `pathParts()`
- [x] All manual `substring()` for path/anchor parsing replaced with helpers
- [x] Helper functions documented with JSDoc
- [x] No regressions in navigation behavior
- [x] Code is more readable and maintainable
- [x] Consistent path handling across all functions

**Refactoring Complete Summary**:
- ✅ Created 3 helper functions: `getCurrentDocPath()`, `parseHashPath()`, `stripAnchor()`
- ✅ Refactored 4 functions: `navigateToLink()`, `handleHashChange()`, `processLoadedContent()`, `getBidFromPath()`
- ✅ Eliminated ~20 lines of manual path manipulation code
- ✅ All path operations now use WASM `pathParts()` consistently
- ✅ Build successful: WASM and CLI binary compile without errors
- ✅ HTML generation successful: Test output generated at `/tmp/noet-test-output`
- ✅ Manual browser testing complete: All navigation flows working correctly

## Risks

**Risk 1**: Subnet prefix logic complexity ✅ RESOLVED
- **Resolution**: Implemented `PathMap::recursive_map()` which cleanly handles subnet traversal and path joining
- **Result**: Subnets work correctly with full path prefixes in NavTree

**Risk 2**: Edge case differences between manual parsing and `AnchorPath`
- **Mitigation**: Test thoroughly with edge cases; `AnchorPath` is well-tested in Rust

**Risk 3**: Performance overhead from WASM calls
- **Mitigation**: Path operations are infrequent (only on navigation); negligible impact

**Risk 4**: Introducing regressions in critical navigation flows
- **Mitigation**: Comprehensive manual testing; consider adding automated tests

## Open Questions

- ~~How to efficiently look up subnet's entry path from parent PathMap?~~ ✅ RESOLVED: Used `recursive_map()` to traverse and prepend prefixes
- ~~Should subnet prefix be stored in NavNode or computed during construction?~~ ✅ RESOLVED: Computed during construction via `recursive_map()`
- ~~Should we add additional `AnchorPath` methods to WASM if gaps are found?~~ ✅ RESOLVED: Existing methods sufficient for all use cases
- ~~Should we create a JavaScript wrapper module (`pathUtils.js`) for these helpers?~~ ✅ RESOLVED: Helper functions inline in viewer.js is cleaner
- Do we need polyfills for older browsers that don't support WASM well? (Defer to future browser compatibility issue)

## Session Notes

### Session 1 (2025-02-18) - Implementation Complete ✅

**Work Completed**:
1. ✅ Step 0: Fixed subnet path prefix bug (already done)
2. ✅ Step 1: Audited manual path operations (created `.scratchpad/path_operations_audit.md`)
3. ✅ Step 2: Added 3 helper functions to `viewer.js` (lines 139-188)
4. ✅ Step 3: Refactored `navigateToLink()` to use `parseHashPath()`
5. ✅ Step 4: Verified `navigateToSection()` already correct
6. ✅ Step 5: Refactored `handleHashChange()` to use `parseHashPath()`
7. ✅ Step 6: Refactored `processLoadedContent()` to use `getCurrentDocPath()`
8. ✅ Step 7: Refactored `getBidFromPath()` to use `stripAnchor()`
9. ✅ Step 8: Verified `getActiveBid()` needs no refactoring
10. ✅ Build verification: Compiled successfully with `./scripts/build-full.sh`
11. ✅ HTML generation: Test output at `/tmp/noet-test-output`
12. ✅ Started test server: `http://localhost:8888` (PID 2180785)
13. ✅ Manual testing complete: All navigation flows verified working
    - Subnet navigation works correctly
    - Section anchors preserve document path
    - Browser back/forward navigation works
    - Two-click pattern unchanged
    - Metadata panel functions correctly
    - No JavaScript errors in console

**Result**: Issue complete! All success criteria met.

**Files Modified**:
- `assets/viewer.js` - Added helpers, refactored 4 functions
- `docs/project/ISSUE_48_CONSOLIDATE_PATH_MANIPULATION.md` - Progress tracking

## References

- `src/wasm.rs` - Lines 255-350 (WASM path methods)
- `src/paths/path.rs` - `AnchorPath` implementation
- `assets/viewer.js` - Lines 139-188 (new helper functions)
- `docs/design/interactive_viewer.md` - Navigation architecture
- `.scratchpad/path_operations_audit.md` - Detailed audit findings