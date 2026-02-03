# Step 6: Export BeliefGraph to JSON - Implementation Summary

**Date**: 2026-02-03  
**Status**: ✅ COMPLETE  
**Estimated Effort**: 2 days (actual: ~2 hours)

## What Was Implemented

Added automatic BeliefGraph export to JSON for client-side use (Phase 2 WASM viewer preparation).

## Architecture

### 1. Added `export_beliefgraph()` to `BeliefSource` trait
- **File**: `src/query.rs`
- **Location**: Added after `get_file_mtimes()` method
- **Default implementation**: Uses `eval_unbalanced(&Expression::StateIn(StatePred::Any))`
- **Return type**: `Result<BeliefGraph, BuildonomyError>`
- **Purpose**: Consistent API across all BeliefSource implementations

### 2. Implemented for `BeliefBase`
- **File**: `src/beliefbase.rs`
- **Implementation**: `self.clone().consume()` - clones and consumes to get complete graph
- **Also implemented for**: `&BeliefBase` (reference version)
- **Use case**: `noet parse` command (in-memory graph)

### 3. Implemented for `DbConnection`
- **File**: `src/db.rs`
- **Implementation**: Direct SQL queries:
  - `SELECT * FROM beliefs` → states
  - `SELECT * FROM relations` → relations
- **Use case**: `noet watch` command (database-backed)

### 4. Added `export_beliefbase_json()` to DocumentCompiler
- **File**: `src/codec/compiler.rs`
- **Parameters**: Takes `BeliefGraph` to serialize
- **Output**: `{html_output_dir}/beliefbase.json` (pretty printed)
- **Features**:
  - File size calculation and logging
  - Warning if JSON exceeds 10MB threshold
  - Logs: file size (MB), state count, relation count

### 5. Integrated into CLI
- **Parse command** (`src/bin/noet/main.rs`):
  - Calls `session_bb().export_beliefgraph().await`
  - Runs after asset hardlinks are created
  - Errors are non-fatal (warning only)

- **Watch command** (`src/watch.rs`):
  - Calls `db_connection.export_beliefgraph().await`
  - Runs after HTML generation and asset hardlinks
  - Uses database state (not session_bb)

## Key Decisions

### Why BeliefSource trait method?
- Consistent with other query operations (`eval_unbalanced`, `eval_trace`, etc.)
- Allows different implementations for in-memory vs database
- Future-proof for other BeliefSource implementations

### Why different sources for parse vs watch?
- **Parse**: Uses `session_bb` - complete in-memory graph built during parsing
- **Watch**: Uses `DbConnection` - current database state (may be incomplete if parse in progress)
- This matches the lifecycle: parse builds complete graph, watch queries persisted state

### Why no pagination in v1?
- Simple warning-based approach first
- 10MB threshold is reasonable for most documentation sets
- Pagination can be added later if needed (query state is already serializable)
- Allows immediate Phase 2 work without complexity

### Format: Native BeliefGraph serialization
- Uses existing `BeliefGraph` struct (already derives `Serialize`)
- Format:
  ```json
  {
    "states": { "bid": { ...BeliefNode } },
    "relations": { "nodes": [...], "edges": [...] }
  }
  ```
- Relations use petgraph's native serialization (efficient, standard)
- No custom export format needed (YAGNI)

## Test Results

```bash
./target/debug/noet parse tests/network_1 --html-output test-output

# Output:
# Exported BeliefGraph to test-output/beliefbase.json (0.03 MB, 57 states, 66 relations)

# File created: 33KB
```

**Verification**:
- File contains all 57 states from network_1 test set
- Includes 66 relations (edges between nodes)
- JSON is valid and pretty-printed
- States include: Documents, Sections, Networks
- Relations include: Section hierarchy, document links

## Files Modified

1. `src/query.rs` - Added trait method (15 lines)
2. `src/beliefbase.rs` - Two implementations (10 lines total)
3. `src/db.rs` - DbConnection implementation (43 lines)
4. `src/codec/compiler.rs` - Export method (52 lines)
5. `src/bin/noet/main.rs` - Parse command integration (4 lines)
6. `src/watch.rs` - Watch service integration (14 lines)

**Total**: ~138 lines of new code

## Phase 1.5 Status

✅ **PHASE 1.5 COMPLETE**

All deliverables finished:
- Static HTML generation with metadata
- CSS theming with embedded stylesheet
- Network index pages
- Asset management
- Dev server with live reload
- **BeliefGraph JSON export** ← This step

## Next Steps (Phase 2)

Ready to begin WASM integration:
1. Step 7: Compile noet-core to WASM
2. Step 8: JavaScript viewer with WASM
3. Client-side search and navigation
4. Progressive enhancement (static HTML → interactive SPA)

## Notes

- All tests pass (cargo test --features service)
- No breaking changes to existing APIs
- Export is automatic with HTML generation (no separate CLI command needed)
- Warning-only approach for large files (no hard limits)
- Ready for immediate Phase 2 work