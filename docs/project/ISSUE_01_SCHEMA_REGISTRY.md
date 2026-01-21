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

1. **Create Registry Struct** (1 day)
   - [ ] Define wrapper around `Arc<RwLock<HashMap<...>>>`
   - [ ] Implement `Clone` (Arc semantics)
   - [ ] Add thread-safety tests

2. **Implement Methods** (1 day)
   - [ ] `register()` with write lock
   - [ ] `get()` with read lock + clone
   - [ ] `list_schemas()` for introspection
   - [ ] Logging for registrations

3. **Register Built-ins** (0.5 days)
   - [ ] Move existing schemas into `create()`
   - [ ] Initialize `intention_lattice.intention`
   - [ ] Verify all schemas present

4. **Update Call Sites** (0.5 days)
   - [ ] Replace `get_schema_definition()` with `SCHEMAS.get()`
   - [ ] Remove old `get_schema_definition()` function
   - [ ] Update `lattice_toml.rs`

5. **Update `compile_schema.py`** (1 day, Optional Phase 2)
   - [ ] Generate initialization code instead of match
   - [ ] Call from `SchemaRegistry::create()`

## Testing Requirements

- Register/retrieve schemas
- Concurrent reads and writes
- Parse TOML with registered schema
- Downstream registration pattern

## Success Criteria

- [ ] `SCHEMAS` global available
- [ ] Existing schemas work unchanged
- [ ] Downstream libraries can register
- [ ] No performance regression
- [ ] Tests pass

## Risks

**Risk**: Thread-safety issues  
**Mitigation**: Use `parking_lot::RwLock`, comprehensive tests

**Risk**: Performance degradation  
**Mitigation**: RwLock read locks are very cheap, benchmark if needed

## Open Questions

1. Should `SchemaDefinition` be `Clone`? (Likely yes)
2. Schema versioning? (Defer to Phase 2)
3. Validation on registration? (Nice to have)

## References

- Pattern: `CodecMap` in `codec/mod.rs`
- Current: `codec/schema_registry.rs`
- Generator: `utilities/compile_schema.py`
