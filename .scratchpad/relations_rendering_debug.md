# SCRATCHPAD - Relations Rendering Debug Session

**Date**: 2026-02-16
**Status**: IN PROGRESS - PathMap issue identified, needs further investigation

## Problem Statement

Relations metadata panel shows "Node Information" but the "Relations" section never renders, even though the Rust WASM side shows the data exists:
- Console shows: `Graph weight kinds: 2`, `Epistemic: 0 sources, 1 sinks`, `Section: 1 sources, 1 sinks`
- But JavaScript never receives the graph data
- `get_context()` returns None

## Root Cause Identified

**PathMap doesn't exist for root network BID**

Console output shows:
```
PathMap exists for root 1f10bf09-6b3b-6f6e-87ca-6ace77cb4a7d: false
⚠️ Node not found in context: 1f10b9a6-33a8-6daf-96a9-ff4c9aea61e2
Entry point: 1f10bf09-6b3b-6f6e-87ca-6ace77cb4a7d
```

The entry point BID (from `builder.repo()`) doesn't have a PathMap entry, so `get_context()` fails at line 1178 of beliefbase.rs:

```rust
let Some(root_pm) = self.paths().get_map(&root_net.bref()) else {
    tracing::debug!("[get_context] network {root_net} is not loaded");
    return None;
};
```

## Investigation Path

### How PathMapMap is Created

Call chain when loading from JSON:
1. `BeliefBase::from(graph)` → `new_unbalanced()`
2. `new_unbalanced()` line 1027 → calls `index_sync(false)`
3. `new_unbalanced()` line 1028 → creates PathMapMap:
   ```rust
   *bs.paths.write() = PathMapMap::new(bs.states(), bs.relations.clone());
   ```

### PathMapMap::new() Behavior

From `src/paths/pathmap.rs` lines 260-320:
- Iterates through states to collect network BIDs (lines 282-284)
- Adds BID to `pmm.nets` if `node.kind.is_network()` returns true
- PathMaps are built **lazily** when accessed via `get_map()`
- The root network BID is NOT in the `nets` set

### Why Root Network Missing?

**Hypothesis**: The root network node either:
1. Doesn't exist in the exported `beliefbase.json` states
2. Exists but doesn't have `kind` field marking it as Network
3. Exists with Network kind, but the BID doesn't match what's in the SPA metadata

The entry point BID comes from the SPA shell's metadata (index.html), which comes from `builder.repo()`.

## Code Locations

### WASM Side (src/wasm.rs)
- Line 645-750: `get_context()` method
- Line 659: Check if node exists in states ✓ (passes)
- Line 663-670: Check if PathMap exists for root ✗ (fails)
- Line 675: Call `inner.get_context(&self.entry_point_bid, &bid)` - returns None

### BeliefBase Side (src/beliefbase.rs)
- Line 1166-1195: `get_context()` implementation
- Line 1178-1181: Checks if `root_pm` exists - this is where it fails

### PathMapMap (src/paths/pathmap.rs)
- Line 260-320: `PathMapMap::new()` - collects network BIDs
- Line 282-284: Adds to `nets` if `node.kind.is_network()`

### Export Location (src/codec/compiler.rs)
- Line 966-970: Export beliefbase to JSON
  ```rust
  let graph = global_bb.export_beliefgraph().await?;
  self.export_beliefbase_json(graph).await?;
  ```

### Entry Point Setup (src/wasm.rs)
- Line 274-319: `from_json()` extracts entry point from metadata
- Line 304-312: Gets `bid` from metadata, stores as `entry_point_bid`

## Debug Logging Added

### In src/wasm.rs `get_context()`:
```rust
// Line 659: Check if node in states
console::log_1(&format!("   Node {} in states: {}", bid, node_exists).into());

// Line 663-670: Check if PathMap exists
let root_bref = self.entry_point_bid.bref();
let pathmap_exists = inner.paths().get_map(&root_bref).is_some();
console::log_1(&format!("   PathMap exists for root {}: {}", self.entry_point_bid, pathmap_exists).into());

// Line 672-683: Show networks in PathMapMap
let nets: Vec<String> = inner.paths().nets().iter().map(|bid| bid.to_string()).collect();
console::log_1(&format!("   Networks in PathMapMap: {} networks", nets.len()).into());
```

### In src/wasm.rs `get_context()` (Rust side):
```rust
// Line 732-746: Show graph data collected
console::log_1(&format!("✅ Got context for node: {}", node_context.node.title).into());
console::log_1(&format!("   Related nodes: {}", node_context.related_nodes.len()).into());
console::log_1(&format!("   Graph weight kinds: {}", node_context.graph.len()).into());
for (kind, (sources, sinks)) in &node_context.graph {
    console::log_1(&format!("   {:?}: {} sources, {} sinks", kind, sources.len(), sinks.len()).into());
}
```

## Next Steps

1. **Check which networks ARE in PathMapMap**
   - Added logging to show first 5 networks in `nets` set
   - Compare to entry point BID
   - Determine if root network is missing or BID mismatch

2. **Verify root network in beliefbase.json**
   - Check if `builder.repo()` BID exists in exported JSON
   - Check if it has Network kind
   - Verify BID matches between compiler export and SPA metadata

3. **Fix root network registration**
   - If missing: Ensure root network is in states before export
   - If kind missing: Ensure Network kind is set on root
   - If BID mismatch: Fix metadata generation to use correct BID

## Related Issues

- Two-click navigation broke temporarily during debugging (bref resolution issues)
- Fixed by clean rebuild - brefs in HTML must match beliefbase.json
- Asset links now open directly (not through SPA) - working correctly

## Files Modified This Session

- `assets/viewer.js`: 
  - Lines 1366-1430: Added Relations rendering with sources/sinks lists
  - Lines 421-442: Added asset detection to open PDFs/images directly
  - Lines 472-492: Fixed anchor navigation to preserve document path using PathParts
  - Lines 1280-1286: Added debug logging (to be removed)

- `src/wasm.rs`:
  - Lines 645-750: get_context() debug logging
  - Lines 659-683: Check node exists, PathMap exists, show networks
  - Lines 732-746: Show graph data collected

- `src/codec/compiler.rs`: (prior session)
  - Added base_url support throughout
  
- `assets/template-simple.html`: (prior session)
  - Added nav link, article wrapper, link rewriting script

## Success Criteria

- [ ] Relations section renders in metadata panel
- [ ] Shows categorized sources and sinks by WeightKind
- [ ] Related nodes are clickable and navigatable
- [ ] Works for all documents in all networks

## Notes

- The graph data IS being collected correctly in Rust (console confirms)
- The issue is purely that `get_context()` returns None before serialization
- PathMap is the gatekeeper - without it, no context can be retrieved
- PathMapMap Display impl shows all paths - could use for debugging