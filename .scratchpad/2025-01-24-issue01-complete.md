# SCRATCHPAD - Issue 1 Complete: Schema Registry Singleton

**Date**: 2025-01-24  
**Issue**: ISSUE_01_SCHEMA_REGISTRY.md  
**Status**: ✅ COMPLETE

## What We Built

Refactored `schema_registry.rs` from static code-generated functions to a dynamic singleton pattern, enabling runtime schema registration for downstream libraries.

### Core Implementation

**Pattern**: Matches `CodecMap` exactly
- `Arc<RwLock<HashMap<String, Arc<SchemaDefinition>>>>`
- Global singleton: `pub static SCHEMAS: Lazy<SchemaRegistry>`
- Thread-safe with `parking_lot::RwLock`
- Arc clones for O(1) retrieval

**Key API**:
```rust
SCHEMAS.register(name, definition);  // Register/overwrite with logging
SCHEMAS.get(name);                    // Returns Option<Arc<SchemaDefinition>>
SCHEMAS.list_schemas();               // Returns Vec<String>
```

### Changes Made

1. **schema_registry.rs** (~280 lines):
   - Created `SchemaRegistry` struct with singleton pattern
   - Implemented `register()`, `get()`, `list_schemas()`
   - Added `#[derive(Clone)]` to `SchemaDefinition` and `GraphField`
   - Moved built-in schemas to `create()` initialization
   - Added 6 comprehensive tests (registration, overwrite, concurrency, Arc efficiency)
   - Removed old `get_schema_definition()` function

2. **belief_ir.rs**:
   - Updated import: `get_schema_definition` → `SCHEMAS`
   - Updated call site: `get_schema_definition(&schema_name)` → `SCHEMAS.get(&schema_name)`

3. **codec/mod.rs**:
   - Exported `SCHEMAS` for public API
   - Added `SchemaRegistry` to module documentation
   - Added usage example for downstream libraries

### Test Results

✅ All 45 tests passing
- 6 schema_registry tests (new)
- 39 existing tests (unchanged)

**New test coverage**:
- Schema registration and retrieval
- Overwrite behavior with logging
- Global singleton access
- Concurrent read/write
- Arc pointer equality (clone efficiency)

### Design Decisions

1. **Arc<SchemaDefinition> not reference**: Enables cheap clones matching `CodecMap` pattern
2. **Last one wins**: Overwrites log with `tracing::info!()`, no errors
3. **No validation on registration**: Type safety is sufficient, logging handles conflicts
4. **compile_schema.py deferred**: Script doesn't exist yet, marked as Optional Phase 2

### Unblocks

- ✅ Issue 2 (Multi-Node TOML Parsing)
- ✅ Issue 3 (Heading Anchors) 
- ✅ Issue 4 (Link Manipulation)
- ✅ Issue 21 (JSON Fallback Parsing)

## Next Steps

**Immediate**: Begin Issue 2 (Multi-Node TOML Parsing)
- Depends on schema registry for typed section payloads
- ~4-5 days estimated effort

**Phase 1 Critical Path**:
```
Issue 1 ✅ → Issue 2 → Issue 4 → Migration Tool → v0.1.0 → Open Source
             Issue 3 ↗
```

## Notes

- No diagnostics/warnings
- Backward compatible (all existing tests pass)
- Clean separation: schemas registered at runtime, not compile-time
- Ready for downstream library extensions