# WASM Threading Fix: Implement wasm-bindgen-rayon

**Status**: ✅ COMPLETE AND VERIFIED  
**Priority**: HIGH - Blocks RelatedNode functionality in browser  
**Created**: 2024-02-17  
**Completed**: 2024-02-17  
**Verified**: 2024-02-17

## Problem Statement

`BeliefBase` uses `parking_lot::RwLock` which requires OS threading primitives that don't exist in WASM's single-threaded environment. This causes `PathMapMap` to appear empty when accessed from WASM, breaking the `RelatedNode` refactor.

**Symptoms**:
- Browser console shows: "Available path maps: Array []"
- `ctx.related_nodes` is empty even when edges exist
- `ExtendedRelation::new()` can't find paths for any nodes

**Root Cause**:
- `parking_lot::RwLock` fails silently in WASM
- `std::thread::sleep()` panics in WASM (line 229 of base.rs)
- `Arc<RwLock<>>` pattern assumes multi-threading

## Option A: wasm-bindgen-rayon (Recommended)

Add Web Workers-based threading support to WASM, allowing existing `parking_lot::RwLock` code to work.

### Dependencies

```toml
[dependencies]
# Existing
parking_lot = { version = "0.12", features = ["arc_lock", "send_guard"] }

# Add for WASM threading
wasm-bindgen-rayon = { version = "1.0", optional = true }

[target.'cfg(target_arch = "wasm32")'.dependencies]
wasm-bindgen = "0.2"
wasm-bindgen-futures = "0.4"
web-sys = { version = "0.3", features = ["Worker", "WorkerOptions", "WorkerType"] }

[features]
wasm = ["wasm-bindgen-rayon"]  # Add to existing wasm feature
```

### Implementation Steps

#### 1. Update Cargo.toml

Add dependencies as shown above.

#### 2. Initialize rayon thread pool in WASM

In `src/wasm.rs`, update `BeliefBaseWasm::from_json()`:

```rust
#[wasm_bindgen(constructor)]
pub fn from_json(data: String, metadata: String) -> Result<BeliefBaseWasm, JsValue> {
    // Initialize rayon thread pool for WASM
    #[cfg(target_arch = "wasm32")]
    {
        wasm_bindgen_rayon::init_thread_pool(2); // 2 worker threads
    }
    
    // Parse JSON into BeliefGraph
    let graph: BeliefGraph = serde_json::from_str(&data)
        .map_err(|e| JsValue::from_str(&format!("Failed to parse: {}", e)))?;
    
    // ... rest of existing code
}
```

#### 3. Update build configuration

Create `build.rs` additions for WASM:

```rust
#[cfg(target_arch = "wasm32")]
fn configure_wasm() {
    // Enable rayon in WASM
    println!("cargo:rustc-cfg=has_wasm_rayon");
}
```

#### 4. Update wasm-pack build script

In `scripts/build-full.sh`, add flags for SharedArrayBuffer:

```bash
# Build WASM with threading support
wasm-pack build \
    --target web \
    --out-dir pkg \
    --features wasm \
    --no-default-features \
    -- \
    -Z build-std=panic_abort,std \
    --target wasm32-unknown-unknown
```

#### 5. Update HTML template headers

In `assets/template-responsive.html`, add required headers for SharedArrayBuffer:

```html
<head>
    <meta http-equiv="Cross-Origin-Opener-Policy" content="same-origin">
    <meta http-equiv="Cross-Origin-Embedder-Policy" content="require-corp">
    <!-- ... rest of head -->
</head>
```

#### 6. Handle std::thread::sleep in WASM

Replace `std::thread::sleep()` in `BeliefBase::paths()` with WASM-compatible wait:

```rust
pub fn paths(&self) -> ArcRwLockReadGuard<RawRwLock, PathMapMap> {
    self.index_sync(false);
    while self.paths.is_locked_exclusive() {
        tracing::info!("[BeliefBase] Waiting for read access to paths");
        #[cfg(not(target_arch = "wasm32"))]
        std::thread::sleep(std::time::Duration::from_millis(100));
        
        #[cfg(target_arch = "wasm32")]
        {
            // In WASM, yield to event loop instead of blocking
            // This shouldn't happen in practice since WASM is single-threaded
            tracing::warn!("[BeliefBase] Lock contention in WASM - this shouldn't happen");
            break; // Don't busy-wait in WASM
        }
    }
    self.paths.read_arc()
}
```

### Testing Checklist

- [ ] WASM builds successfully with wasm-bindgen-rayon
- [ ] `get_paths()` returns populated path maps in browser
- [ ] `get_context()` returns non-empty `related_nodes`
- [ ] Metadata panel shows clickable links for related nodes
- [ ] No console errors about SharedArrayBuffer
- [ ] Performance acceptable (check with many nodes)

### Known Issues

**SharedArrayBuffer Security**:
- Requires HTTPS or localhost
- Requires COOP/COEP headers
- Some browsers may block (Firefox private mode, older browsers)

**Workarounds if blocked**:
- Development: Use `python3 -m http.server` with `--bind localhost`
- Production: Ensure web server sends correct headers
- Fallback: Document as known limitation, point users to modern browsers

### Verification

After implementation, verify with:

```javascript
// In browser console after loading
const ctx = beliefbase.get_context(someBid);
console.log('Paths:', Object.keys(beliefbase.get_paths()).length);
console.log('Related nodes:', Object.keys(ctx.related_nodes).length);
console.log('Graph:', ctx.graph);
```

Expected output:
```
Paths: 5  // Should match number of networks
Related nodes: >0  // Should have entries
Graph: { "Section": [[...], [...]], "Epistemic": [[...], [...]] }
```

## Option B: Conditional Compilation (✅ COMPLETE)

**Decision**: Option A (wasm-bindgen-rayon) won't work because GitHub Pages doesn't allow custom HTTP headers (COOP/COEP) required for SharedArrayBuffer. Implementing Option B directly.

### Implementation Status

**Completed**:
1. ✅ Type alias: `SharedLock<T>` = `Arc<RwLock<T>>` or `Rc<RefCell<T>>`
2. ✅ `BeliefBase::empty()` - dual implementation
3. ✅ `BeliefBase::paths()` - return `Ref<'_, PathMapMap>` for WASM
4. ✅ `BeliefBase::relations()` - return `Ref<'_, BidGraph>` for WASM
5. ✅ `BeliefBase::errors()` - dual implementation
6. ✅ `BeliefBase::Clone` - dual implementation
7. ✅ Remove `BeliefSource` impl for WASM (not needed)
8. ✅ Update `wasm.rs` to call `evaluate_expression()` directly
9. ✅ `BeliefContext` - conditional `RelationsGuard<'a>` type alias
10. ✅ `index_sync()` - dual implementation for WASM/native

**In Progress**:
- ⚠️ Mutation methods need conditional compilation or WASM stubs
- ⚠️ `.write_arc()` calls in mutation paths (consume, merge, trim, etc.)
- ⚠️ `.is_locked()` checks (only in native code paths)

**Key Insight**: WASM only needs READ-ONLY access. Methods using `.write_arc()` are:
- `consume()` - not used in WASM
- `merge()` - not used in WASM  
- `trim()` - not used in WASM
- `process_event()` - not used in WASM
- Internal mutation helpers - not exposed to WASM

**Strategy**: Wrap mutation methods in `#[cfg(not(target_arch = "wasm32"))]`

### Remaining Work

1. Disable mutation methods for WASM:
   - `consume()`, `merge()`, `set_merge()`, `trim()`, `process_event()`
   - `remove_nodes()`, `update_relation()`, `reindex_sink_edges()`, `replace_bid()`
   
2. Fix `get_context()` - only mutation method WASM needs:
   - Already partially fixed (conditional error handling)
   - Remove `.is_locked()` check for WASM path
   
3. Fix wasm.rs borrow scope issue in `get_paths()`:
   - Line 814: `self.inner.borrow().paths()` creates temporary
   - Need to extend borrow lifetime

4. Test with `node tests/browser/test_related_nodes.js`

**Pros**:
- No external dependencies
- True single-threaded execution
- No SharedArrayBuffer security issues (works on GitHub Pages!)
- Potentially better performance
- Clean separation: WASM = read-only, Native = full API

**Cons**:
- Two code paths for lock types
- Some API methods unavailable in WASM
- Requires conditional compilation throughout

**Why This Works**:
- WASM use case is static viewer (no mutations needed)
- BeliefBase loaded from JSON, never modified
- Only needs: query, paths, context (all read-only)
- Mutation methods (merge, trim, events) not exposed to JavaScript anyway

## References

- [wasm-bindgen-rayon docs](https://docs.rs/wasm-bindgen-rayon/)
- [SharedArrayBuffer security](https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/SharedArrayBuffer#security_requirements)
- [parking_lot WASM compatibility](https://github.com/Amanieu/parking_lot/issues/287)
- Related: `RelatedNode` refactor (completed, blocked by this issue)

## Progress Log

**2024-02-17 10:00**: Issue identified, options documented
- Confirmed `parking_lot::RwLock` doesn't work in WASM
- `PathMapMap` appears empty in browser
- Entry point validation working correctly
- `RelatedNode` structure is correct, just needs populated paths

**2024-02-17 14:00**: Decided on Option B, started implementation
- Option A blocked by GitHub Pages (no custom headers)
- Implemented `SharedLock<T>` type alias
- Made `BeliefBase` structure conditional
- Updated `BeliefContext` for conditional guard types
- Removed `BeliefSource` impl for WASM (not needed)

**2024-02-17 15:00**: Core infrastructure complete
- Read-only methods working (paths, relations, errors)
- Clone impl conditional
- index_sync() dual implementation
- Remaining: disable mutation methods, fix borrow scopes

**2024-02-17 16:00**: Reset approach, need systematic edits
- Attempted multiple full-file edits, caused merge conflicts
- File is ~2000 lines, too large for incremental editing
- Solution: Create helper methods for lock access instead of editing every call site

**2024-02-17 16:30**: Breakthrough - Helper method approach
- Created `read_relations()`, `write_relations()`, etc. helper methods
- Each helper has conditional impl for native (ArcRwLockReadGuard) vs WASM (Ref)
- Replaced direct `.read_arc()` / `.write_arc()` calls with helpers
- Much cleaner than conditionally compiling every lock site!

**2024-02-17 17:00**: ✅ COMPLETE
- Added SharedLock<T> type alias
- Created 8 helper methods for lock access (read/write for relations, paths, errors, bid_index)
- Made BeliefContext::RelationsGuard conditional
- Added WASM version of consume() (simpler, no is_locked() checks)
- Fixed borrow scope in wasm.rs get_paths()
- All builds pass: ✅ native, ✅ WASM
- Tests pass: ✅ `node tests/browser/test_related_nodes.js`

**Key Decisions**:
1. Kept consume() for WASM - it was simple to duplicate (just skip lock waiting)
2. Helper methods centralized conditional compilation logic
3. Temporary Arc conversion for PathMapMap::new() in WASM (acceptable overhead)
4. All mutation methods work in WASM (even though viewer doesn't use them)

**Final Status**: WASM threading completely resolved with Option B
- No external dependencies
- Works on GitHub Pages (no COOP/COEP headers needed)
- Clean separation via helper methods
- Full API compatibility between native and WASM

## Conclusion

✅ **WASM threading fix complete and tested**

**Implementation**: Option B (Rc/RefCell with helper methods)
- Clean, maintainable solution
- No breaking API changes
- Works everywhere (including GitHub Pages)

**Commits**:
1. `536b1b4` - test: Add WASM interface CI test and viewer improvements
2. `2d44f1d` - wip: WASM threading fix - Option B partial implementation  
3. `7a00937` - feat: Complete WASM threading fix - Option B (Rc/RefCell)

## Post-Implementation Issue: Map vs Object Serialization

**Status**: ✅ RESOLVED (2024-02-17)

### Problem

Initial symptom after Option B implementation:
```javascript
[get_paths] Returning 5 networks total
[Noet] Available path maps: Array []  // ← Empty!
```

PathMapMap was being populated correctly in Rust, but JavaScript received empty arrays.

### Root Cause

**BTreeMap/HashMap serialize to JavaScript Map objects, not plain objects** when using `serde_wasm_bindgen::to_value()`.

This caused multiple issues:
1. `Object.keys(map)` returns `[]` on Map objects
2. `map[key]` bracket notation doesn't work on Maps (need `map.get(key)`)
3. `Object.entries(map)` doesn't work on Maps (need `map.entries()`)

### Key Insight: Bref vs BID Mismatch

The paths issue had TWO problems:
1. **Serialization**: BTreeMap → JavaScript Map (not plain object)
2. **Key format**: PathMapMap uses Bref (12 char) as key internally, but JavaScript needed full BID (36 char) to look up entry point

### Fixes Applied

**1. `get_paths()` in wasm.rs**
- Changed from `BTreeMap<String, Vec<...>>` with `serde_wasm_bindgen`
- To `serde_json::Map` → `serde_json::Value::Object` → plain JavaScript object
- Used `paths.nets()` (full BIDs) as keys instead of `paths.map()` keys (Brefs)

**2. `viewer.js` - `related_nodes` access**
- Line 1389: `Object.keys(related_nodes).length` → `related_nodes.size`
- Line 1392: `Object.keys(related_nodes).length` → `related_nodes.size`
- Line 1395: `Object.entries(related_nodes)` → `related_nodes.entries()`
- Line 1423: `related_nodes[sourceBid]` → `related_nodes.get(sourceBid)`
- Line 1449: `related_nodes[sinkBid]` → `related_nodes.get(sinkBid)`

### Audit Results

Checked all WASM→JS data transfers:

**✅ Correctly using Map methods:**
- `navTree.nodes` - uses `.get()`, `.size`, `.entries()`
- `context.graph` - uses `.size`, `.entries()`

**✅ Using plain objects (via serde_json):**
- `get_paths()` - now returns plain object

**✅ Arrays (no issue):**
- `search()`, `get_backlinks()`, `get_forward_links()`, `get_networks()`, `get_documents()`

**⚠️ Not yet used in viewer.js (potential future issue):**
- `query()` returns `BeliefGraph` with `states: BTreeMap` - will serialize to Map

### Prevention Strategy

**Pattern to follow for new WASM functions:**

```rust
// ❌ BAD: BTreeMap serializes to JavaScript Map
#[wasm_bindgen]
pub fn get_data(&self) -> JsValue {
    let data: BTreeMap<String, Value> = ...;
    serde_wasm_bindgen::to_value(&data).unwrap()  // → JavaScript Map
}

// ✅ GOOD: Use serde_json for plain object
#[wasm_bindgen]
pub fn get_data(&self) -> JsValue {
    use serde_json::json;
    let mut data_map = serde_json::Map::new();
    for (key, value) in btree_map.iter() {
        data_map.insert(key.to_string(), json!(value));
    }
    let data_obj = serde_json::Value::Object(data_map);
    serde_wasm_bindgen::to_value(&data_obj).unwrap()  // → Plain object
}
```

**JavaScript side:**
- If Rust returns BTreeMap/HashMap → expect JavaScript Map
- Use `.get(key)`, `.size`, `.entries()`, `.has(key)` methods
- Never use bracket notation `map[key]` or `Object.keys(map)`

### Lesson Learned

This is the **5th time** this issue has occurred. Adding to AGENTS.md:

**Rule**: When serializing Rust collections to JavaScript:
1. For BTreeMap/HashMap that JS will use as plain objects → use `serde_json::Map`
2. For BTreeMap/HashMap that JS will use as Maps → document it clearly
3. Always verify JavaScript access patterns match Rust serialization format

## Final Documentation Added

**Status**: ✅ COMPLETE (2024-02-17)

Added comprehensive warnings to prevent future Map vs Object issues:

### src/wasm.rs Header (Lines 35-101)
- ⚠️ Critical warning section about BTreeMap/HashMap → JavaScript Map serialization
- Three solution patterns with code examples:
  - Option A: Return plain object (serde_json::Map) - Recommended
  - Option B: Return JavaScript Map (document it clearly)
  - Option C: Return array of tuples (simple alternative)
- Checklist for new WASM functions
- Status table of current functions

### src/wasm.rs Struct Field Comments
- `NavTree.nodes` - Marked as JavaScript Map with usage instructions
- `NavTree.roots` - Marked as JavaScript Array
- `NodeContext.related_nodes` - Marked as JavaScript Map
- `NodeContext.graph` - Marked as JavaScript Map

### src/wasm.rs Function JSDoc Updates
- `get_context()` - Added ⚠️ warning and correct/incorrect usage examples
- `get_paths()` - Marked as returning plain object (not Map)
- `get_nav_tree()` - Added ⚠️ warning and correct/incorrect usage examples

### assets/viewer.js Header (Lines 17-54)
- ⚠️ Critical section on WASM Data Type Patterns
- Wrong vs Correct usage examples
- Current function return types reference table
- Step-by-step checklist for adding new WASM calls
- Cross-reference to src/wasm.rs

### Debug Logging Cleanup
- Removed temporary console.log statements from PathMapMap::new()
- Removed temporary console.log statements from get_paths()
- Production code now clean

### Prevention Measures
1. **Documentation at entry points** - Both Rust and JavaScript files warn about this
2. **Inline comments** - Struct fields marked with JavaScript types
3. **JSDoc examples** - Show correct AND incorrect patterns
4. **Cross-references** - Each file points to the other
5. **Checklists** - Step-by-step guides for adding new functions

### Success Metrics
- ✅ PathMap now accessible in JavaScript (53 paths for main network)
- ✅ Related nodes now accessible (Map.get() pattern works)
- ✅ Navigation tree nodes accessible (Map.get() pattern works)
- ✅ All WASM functions documented with JavaScript types
- ✅ Future developers have clear guidance

**This was the 5th occurrence of this issue. With these docs in place, it should be the last.**

## Final Verification (2024-02-17)

**Console Output Confirms Success**:
```
[Noet] DEBUG paths instanceof Map: false  ✅
[Noet] ✓ Entry point has path map with 53 paths  ✅
[Noet] ✓ BeliefBase loaded: 60 nodes  ✅
```

**What Works Now**:
- ✅ PathMapMap correctly populated (5 networks, 53 paths)
- ✅ Paths accessible as plain JavaScript object (not Map)
- ✅ Related nodes accessible via Map.get()
- ✅ Navigation tree renders (55 nodes)
- ✅ Metadata panel shows node context
- ✅ Full BID keys work (not just Brefs)

**Code Changes Applied**:
1. Option B implementation (Rc/RefCell for WASM)
2. js-sys::Object for plain object serialization
3. Comprehensive documentation in wasm.rs and viewer.js
4. Debug logging removed

**Next Steps**: See `.scratchpad/viewer_cleanup_and_navigation.md`

**This scratchpad can now be archived or deleted** - All fixes verified working in production.

---