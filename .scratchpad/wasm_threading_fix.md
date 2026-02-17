# WASM Threading Fix: Implement wasm-bindgen-rayon

**Status**: In Progress - Implementing Option B  
**Priority**: HIGH - Blocks RelatedNode functionality in browser  
**Created**: 2024-02-17  
**Updated**: 2024-02-17

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

## Option B: Conditional Compilation (ACTIVE)

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

**Next**: 
1. Wrap mutation methods in `#[cfg(not(target_arch = "wasm32"))]`
2. Fix `get_context()` and `get_paths()` borrow issues
3. Build and test WASM

---

**Note**: This scratchpad should be deleted once the fix is implemented and tested. Move relevant documentation to permanent docs if needed.