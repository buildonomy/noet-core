# Issue XX: Consolidate Path Manipulation to Use WASM AnchorPath Functions

**Priority**: MEDIUM  
**Estimated Effort**: 1-2 days  
**Dependencies**: None  
**Blocks**: None

## Summary

Consolidate manual path manipulation in `viewer.js` to use WASM-exposed `AnchorPath` functions. Currently, JavaScript performs ad-hoc string operations (`substring`, `indexOf`, `split`) for path parsing and manipulation, which duplicates logic already implemented in Rust's `AnchorPath` and is error-prone.

## Goals

- Replace manual path string operations with WASM `AnchorPath` methods
- Eliminate duplicate path manipulation logic between Rust and JavaScript
- Improve maintainability (single source of truth for path semantics)
- Reduce bugs from inconsistent path parsing
- Leverage well-tested Rust path utilities

## Architecture

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

### 1. Audit Current Path Manipulation (1-2 hours)
- [x] Identify all manual path operations in `viewer.js`
- [ ] Categorize by operation type (parse, join, strip anchor, etc.)
- [ ] Map to corresponding `AnchorPath` method

**Locations Identified**:
- `navigateToLink()` - L524-530, L541-554, L556-569
- `navigateToSection()` - L597-611
- `handleHashChange()` - L653-657, L662-668
- `processLoadedContent()` - L898-915
- `getBidFromPath()` - L1006-1008
- `getActiveBid()` - L1334-1338, L1350-1356

### 2. Add Helper Functions (2-3 hours)
- [ ] Create `getHashPath()` helper using `pathParts()`
- [ ] Create `stripAnchor()` helper using `pathParent()`
- [ ] Create `splitPath()` helper using `pathParts()`
- [ ] Document helpers with JSDoc

**Example Helpers**:
```javascript
/**
 * Get current document path from hash without anchor
 * @returns {string} Document path (e.g., "net1_dir1/doc.html")
 */
function getCurrentDocPath() {
    const hash = window.location.hash.substring(1);
    if (!hash || !wasmModule) return "";
    return wasmModule.BeliefBaseWasm.pathParent(hash);
}

/**
 * Parse hash into document path and anchor
 * @param {string} hash - Hash string (with or without leading #)
 * @returns {{path: string, anchor: string|null}}
 */
function parseHashPath(hash) {
    const cleanHash = hash.startsWith("#") ? hash.substring(1) : hash;
    if (!cleanHash || !wasmModule) {
        return { path: cleanHash, anchor: null };
    }
    
    const parts = wasmModule.BeliefBaseWasm.pathParts(cleanHash);
    const path = parts.path ? `${parts.path}/${parts.filename}` : parts.filename;
    const anchor = parts.anchor ? `#${parts.anchor}` : null;
    
    return { path, anchor };
}
```

### 3. Refactor `navigateToLink()` (1 hour)
- [ ] Replace `indexOf("#")` / `substring()` with `pathParts()`
- [ ] Use `pathParent()` for relative path resolution
- [ ] Test with document links, section anchors, relative paths

### 4. Refactor `navigateToSection()` (30 min)
- [ ] Replace `substring(1)` with `pathParts()`
- [ ] Use `pathParent()` to preserve document path
- [ ] Test section navigation preserves document path

### 5. Refactor `handleHashChange()` (1 hour)
- [ ] Replace manual anchor extraction with `pathParts()`
- [ ] Use helper functions for parsing
- [ ] Test hash change handling with various formats

### 6. Refactor `processLoadedContent()` (30 min)
- [ ] Use `pathParent()` for current document path
- [ ] Simplify header anchor href generation
- [ ] Test anchor links include correct document path

### 7. Refactor `getBidFromPath()` (15 min)
- [ ] Use `pathParts()` or helper to clean path
- [ ] Remove manual leading slash stripping

### 8. Refactor `getActiveBid()` (30 min)
- [ ] Use `pathParts()` for hash parsing
- [ ] Simplify section ID extraction

## Testing Requirements

**Manual Testing**:
- [ ] Navigate between documents
- [ ] Click section anchors (should preserve document path)
- [ ] Navigate with relative links (../other.html)
- [ ] Navigate with anchors (doc.html#section)
- [ ] Use browser back/forward
- [ ] Test with edge cases:
  - Empty hash
  - Hash with only anchor (#section)
  - Nested paths (dir/subdir/doc.html)
  - Paths with multiple # (should not occur, but test gracefully)

**Regression Testing**:
- [ ] All existing navigation flows work
- [ ] Two-click pattern unaffected
- [ ] Metadata panel links work
- [ ] Image modal links work
- [ ] Header anchor links work

## Success Criteria

- [ ] All manual `indexOf("#")` replaced with `pathParts()`
- [ ] All manual `substring()` for paths replaced with helpers
- [ ] Helper functions documented with JSDoc
- [ ] No regressions in navigation behavior
- [ ] Code is more readable and maintainable
- [ ] Consistent path handling across all functions

## Risks

**Risk 1**: Edge case differences between manual parsing and `AnchorPath`
- **Mitigation**: Test thoroughly with edge cases; `AnchorPath` is well-tested in Rust

**Risk 2**: Performance overhead from WASM calls
- **Mitigation**: Path operations are infrequent (only on navigation); negligible impact

**Risk 3**: Introducing regressions in critical navigation flows
- **Mitigation**: Comprehensive manual testing; consider adding automated tests

## Open Questions

- Should we add additional `AnchorPath` methods to WASM if gaps are found?
- Should we create a JavaScript wrapper module (`pathUtils.js`) for these helpers?
- Do we need polyfills for older browsers that don't support WASM well?

## References

- `src/wasm.rs` - Lines 255-350 (WASM path methods)
- `src/paths/path.rs` - `AnchorPath` implementation
- `assets/viewer.js` - Current manual path manipulation
- `docs/design/interactive_viewer.md` - Navigation architecture