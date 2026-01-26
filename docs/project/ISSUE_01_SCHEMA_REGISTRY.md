# Issue 1: Schema Registry Singleton Pattern

**Priority**: CRITICAL - Blocks Issues 2, 3, 4  
**Estimated Effort**: 3-4 days  
**Dependencies**: None (foundational)

## Summary

Refactor `schema_registry.rs` from static code-generated functions to a dynamic singleton pattern matching `CodecMap`. Enables downstream libraries to register custom schemas at runtime without modifying `noet-core`.

## Goals

1. Create `SchemaRegistry` singleton with thread-safe `HashMap`
2. Allow runtime registration via `SCHEMAS.register()`
3. Support both built-in and downstream schemas
4. Enable plugin architecture for extensibility

## Architecture

```rust
pub static SCHEMAS: Lazy<SchemaRegistry> = Lazy::new(SchemaRegistry::create);

pub struct SchemaRegistry(Arc<RwLock<HashMap<String, SchemaDefinition>>>);

impl SchemaRegistry {
    pub fn create() -> Self;
    pub fn register(&self, schema_name: String, definition: SchemaDefinition);
    pub fn get(&self, schema_name: &str) -> Option<SchemaDefinition>;
}
```

Pattern matches `CodecMap` in `codec/mod.rs`.

## Implementation Steps

1. **Create Registry Struct** (1 day) ✅
   - [x] Define wrapper around `Arc<RwLock<HashMap<...>>>`
   - [x] Implement `Clone` (Arc semantics)
   - [x] Add thread-safety tests

2. **Implement Methods** (1 day) ✅
   - [x] `register()` with write lock
   - [x] `get()` with read lock + clone
   - [x] `list_schemas()` for introspection
   - [x] Logging for registrations (overwrites)

3. **Register Built-ins** (0.5 days) ✅
   - [x] Move existing schemas into `create()`
   - [x] Initialize `intention_lattice.intention`
   - [x] Verify all schemas present

4. **Update Call Sites** (0.5 days) ✅
   - [x] Replace `get_schema_definition()` with `SCHEMAS.get()`
   - [x] Remove old `get_schema_definition()` function
   - [x] Update `belief_ir.rs`
   - [x] Export `SCHEMAS` from `codec/mod.rs`
   - [x] Add module documentation example

5. **Update `compile_schema.py`** (1 day, Optional Phase 2 - DEFERRED)
   - [ ] Script doesn't exist yet in codebase
   - [ ] Generate initialization code instead of match
   - [ ] Call from `SchemaRegistry::create()`
   - [ ] Note: This is optional and not blocking downstream issues

## Testing Requirements ✅

- [x] Register/retrieve schemas
- [x] Concurrent reads and writes
- [x] Parse TOML with registered schema (via existing integration tests)
- [x] Downstream registration pattern (global SCHEMAS test)
- [x] Arc clone efficiency (pointer equality test)

## Success Criteria ✅

- [x] `SCHEMAS` global available
- [x] Existing schemas work unchanged
- [x] Downstream libraries can register
- [x] No performance regression (Arc clone is O(1))
- [x] Tests pass (45/45 tests passing)

## Risks

**Risk**: Thread-safety issues  
**Mitigation**: Use `parking_lot::RwLock`, comprehensive tests

**Risk**: Performance degradation  
**Mitigation**: RwLock read locks are very cheap, benchmark if needed

## Open Questions

1. ~~Should `SchemaDefinition` be `Clone`?~~ ✅ **RESOLVED**: Yes, Arc<SchemaDefinition> clones
2. Schema versioning? (Defer to Phase 2)
3. Validation on registration? (Defer - "last one wins" with logging is sufficient)

## Implementation Summary

**Status**: ✅ **COMPLETE** (2025-01-24)

All core objectives achieved:
- Singleton pattern implemented matching `CodecMap`
- Runtime schema registration enabled
- Thread-safe with `parking_lot::RwLock`
- Comprehensive test coverage (6 schema registry tests)
- Module documentation and usage examples added
- Backward compatible (all existing tests pass)

**Changes**:
- `schema_registry.rs`: Refactored to singleton with `SCHEMAS` global
- `belief_ir.rs`: Updated to use `SCHEMAS.get()`
- `codec/mod.rs`: Exported `SCHEMAS`, added documentation

**Next**: Ready to unblock Issues 2, 3, 4

## References

- Pattern: `CodecMap` in `codec/mod.rs`
- Implementation: `codec/schema_registry.rs`
- Generator: `utilities/compile_schema.py` (doesn't exist yet - Phase 2)
