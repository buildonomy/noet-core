# Issue 32: Schema Registry Productionization

**Priority**: MEDIUM
**Estimated Effort**: 3-4 days
**Dependencies**: None
**Blocks**: Future schema-driven features (custom content types, advanced querying)

## Summary

The Schema Registry (`schema_registry.rs`) currently exists as scaffolding but lacks the capability to automatically translate structured payload fields into BeliefGraph edges. This issue tracks productionizing the registry to enable schema-driven edge generation, reducing bespoke codec logic and enabling extensible content types.

On top of that, ProtoBeliefNode is the wrong place to perform traversal! A NodeUpdate event should generate the RelationChange events that its schema-defined payload represents. Otherwise, the `GraphBuilder::parse_content` is the only way that will generate the defined relation events, which is likely to break idempotency in strange ways.

## Goals

1. **Define GraphField semantics** for common payload-to-edge patterns
2. **Implement automatic edge generation** in `ProtoBeliefNode::traverse_schema()`
3. **Support map-based references** (e.g., Network's `asset_manifest` field)
4. **Enable reverse translation** (edges → payload during context injection)
5. **Register built-in schemas** (Network, Document, Section) in `SchemaRegistry::create()`
6. **Document schema definition patterns** for future extensibility

## Architecture

### Problem Statement

Currently, when a BeliefNode has structured payload fields that reference other nodes, codecs must manually:
1. Parse payload structure
2. Extract BIDs/NodeKeys
3. Create `ProtoBeliefNode.upstream` or `downstream` entries
4. Reverse process during `inject_context()` to rebuild payload

**Example (Issue 29 - Asset Manifest):**
```rust
// Network BeliefNode payload
{
    "asset_manifest": {
        "docs/logo.png": "bid:asset-ns:sha256-abc",
        "images/chart.png": "bid:asset-ns:sha256-def"
    }
}

// Must manually translate to ProtoBeliefNode edges:
network_proto.upstream.push((
    NodeKey::Bid("bid:asset-ns:sha256-abc"),
    WeightKind::Pragmatic,
    Some(payload_with_path),
));
```

This manual translation is **repeated for every schema** with graph-structured fields.

### Desired Behavior

**Schema definition should declaratively specify:**
- Which payload fields represent graph edges
- How to extract NodeKeys from field values
- Edge direction (upstream/downstream)
- WeightKind for the edge
- What metadata to preserve in edge payload

**`traverse_schema()` should automatically:**
- Read GraphField definitions from registry
- Iterate over payload fields
- Generate upstream/downstream entries
- Preserve structured metadata in edge payloads

**`inject_context()` should automatically:**
- Rebuild payload fields from BeliefBase edges
- Restore original structure (maps, lists, single refs)

### GraphField Patterns to Support

**Pattern 1: Map References (Network asset_manifest)**
```toml
# Payload field
[payload.asset_manifest]
"docs/logo.png" = "bid:asset-ns:sha256-abc"
"images/chart.png" = "bid:asset-ns:sha256-def"

# Should generate edges:
# upstream: [(NodeKey::Bid, Pragmatic, {"path": "docs/logo.png"}), ...]
```

**Pattern 2: Single Reference**
```toml
# Payload field
parent_bid = "bid:section-123"

# Should generate edge:
# upstream: [(NodeKey::Bid("bid:section-123"), Semantic, None)]
```

**Pattern 3: List References**
```toml
# Payload field
related_docs = ["bid:doc-1", "bid:doc-2"]

# Should generate edges:
# downstream: [(NodeKey::Bid("bid:doc-1"), Semantic, None), ...]
```

**Pattern 4: Structured List**
```toml
# Payload field
[[contributors]]
person_bid = "bid:person-1"
role = "author"

[[contributors]]
person_bid = "bid:person-2"
role = "reviewer"

# Should generate edges with role in payload:
# upstream: [(NodeKey::Bid("bid:person-1"), Semantic, {"role": "author"}), ...]
```

### Schema Registry Enhancements Needed

1. **GraphField type system** to express extraction patterns
2. **Bidirectional translation** (payload ↔ edges)
3. **Schema registration API** for built-in and custom schemas
4. **Preferred serialization format** per schema (TOML/JSON/YAML)
5. **Validation rules** (required fields, type constraints)

## Implementation Steps

### 1. Design GraphField Type System (1 day)
- [ ] Define `GraphFieldType` enum variants for common patterns
- [ ] Specify extraction semantics (how to get NodeKey from field)
- [ ] Specify edge payload construction (what metadata to preserve)
- [ ] Document pattern matching logic

### 2. Implement Forward Translation (1 day)
- [ ] Update `ProtoBeliefNode::traverse_schema()` to iterate GraphFields
- [ ] Implement extraction logic for each GraphFieldType
- [ ] Generate upstream/downstream entries with correct WeightKind
- [ ] Preserve metadata in edge payloads

### 3. Implement Reverse Translation (1 day)
- [ ] Add reverse traversal in `inject_context()` or new method
- [ ] Rebuild payload fields from BeliefBase edges
- [ ] Handle missing edges gracefully (optional fields)
- [ ] Validate roundtrip consistency

### 4. Register Built-in Schemas (0.5 days)
- [ ] Define Network schema with `asset_manifest` GraphField
- [ ] Define Document schema (if graph fields exist)
- [ ] Define Section schema (parent references?)
- [ ] Set preferred serialization formats

### 5. Testing and Documentation (0.5 days)
- [ ] Unit tests for each GraphFieldType pattern
- [ ] Roundtrip tests (payload → edges → payload)
- [ ] Document schema definition patterns
- [ ] Example custom schema registration

## Testing Requirements

### Unit Tests
- `test_graph_field_map_extraction()` - Map pattern parsing
- `test_graph_field_single_ref()` - Single BID extraction
- `test_graph_field_list_refs()` - List pattern parsing
- `test_roundtrip_consistency()` - payload → edges → payload identity

### Integration Tests
- `test_network_asset_manifest_automatic()` - Network schema with assets
- `test_custom_schema_registration()` - User-defined schema
- `test_edge_payload_preservation()` - Metadata survives roundtrip

## Success Criteria

- [ ] GraphField definitions express common payload-to-edge patterns
- [ ] `traverse_schema()` automatically generates edges from payload
- [ ] `inject_context()` rebuilds payload from edges
- [ ] Network schema registered with `asset_manifest` support
- [ ] Codecs can rely on schema traversal instead of manual edge construction
- [ ] Documentation explains how to define custom schemas
- [ ] All tests passing

## Risks

### Risk 1: Complex Nested Structures
**Impact**: MEDIUM  
**Likelihood**: MEDIUM  
**Mitigation**: 
- Start with flat patterns (maps, lists, single refs)
- Defer deeply nested object graphs to future iterations
- YAGNI - don't over-engineer type system

### Risk 2: Backward Compatibility
**Impact**: LOW  
**Likelihood**: LOW  
**Mitigation**:
- Existing manual edge construction continues to work
- Schema traversal is additive, not replacing
- Codecs opt-in by defining schemas

### Risk 3: Edge Payload Serialization
**Impact**: MEDIUM  
**Likelihood**: LOW  
**Mitigation**:
- Edge payloads already support TOML values
- Ensure payload reconstruction matches original structure
- Test roundtrip extensively

## Open Questions

### Q1: Should schemas be versioned?
**Context**: If schema definitions evolve, how do we handle old documents?

**Options**:
- A) Schema versioning (e.g., "Network/v1", "Network/v2")
- B) Schema migration logic
- C) Ignore for now (YAGNI)

**Recommendation**: C for v0.1, revisit when breaking changes needed.

### Q2: How to handle custom WeightKinds?
**Context**: Future schemas may need domain-specific edge types.

**Options**:
- A) Hardcode WeightKind in GraphField
- B) Allow string-based custom weights
- C) Enum + extensibility pattern

**Recommendation**: A for now (Semantic/Pragmatic sufficient).

### Q3: GraphField evaluation order?
**Context**: If multiple fields reference same BID, which edge wins?

**Options**:
- A) Last-defined wins
- B) Error on conflict
- C) Merge edge payloads

**Recommendation**: A (simple, predictable).

## References

### Related Issues
- **Issue 29**: Static Asset Tracking (motivating use case - Network asset_manifest)
- **Issue 30**: External URL Tracking (similar manifest pattern)

### Architecture References
- `src/codec/schema_registry.rs` - Current scaffolding
- `src/codec/belief_ir.rs:ProtoBeliefNode::traverse_schema()` - Integration point
- `docs/design/beliefbase_architecture.md` - Schema vs Kind distinction

### Future Enhancements
- **Custom Content Types**: User-defined schemas for domain-specific nodes
- **Schema Validation**: Enforce required fields, type constraints
- **Schema Migrations**: Handle evolving schema definitions
- **Query Optimization**: Index by schema for fast filtering

## Notes

**YAGNI Principle**: Start with minimal GraphField variants needed for Issue 29 (map references). Extend as new patterns emerge.

**Separation of Concerns**: Schema registry defines **structure**, not **behavior**. Codecs still control parsing/serialization logic.

**Opt-In Design**: Existing codecs continue working without schema definitions. Schemas enable automation but aren't required.

**Pattern Library**: As schemas are defined, common patterns emerge that can be extracted into reusable GraphField templates.
