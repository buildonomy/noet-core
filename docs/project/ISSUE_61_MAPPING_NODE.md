# Issue 61: Mapping Node Implementation

**Priority**: MEDIUM
**Estimated Effort**: 4 days (RELATIVE COMPARISON ONLY)
**Dependencies**: Requires Issue 55 (MyST directive syntax), Blocks Issue 18 (extended procedure schemas)

## Summary

Implement the mapping node — a belief node that owns directed edges between two other
nodes without being either endpoint. A mapping node declares `schema = "noet.mapping"`
and a `[[mappings]]` TOML array-of-tables, and the compiler registers each entry as a
`RelationChange` with `WEIGHT_OWNED_BY` set to the mapping node's bref. The full spec
is in `docs/design/mapping_node_architecture.md`.

## Goals

- Parse `[[mappings]]` front-matter into `IRNode.mappings` via a new `IntermediateMappingRelation` struct
- Register third-party-owned edges through a new `push_mapping` builder method (Phase 2b)
- Extend `WEIGHT_OWNED_BY` to accept a bref string in addition to `"source"` / `"sink"`
- Render a mapping node's declared edges as an HTML table via the deferred-render pipeline
- Surface `owner_bid` in the viewer via `EdgeEntry` and a "via <link>" annotation in `metadata.js`

## Architecture

See `docs/design/mapping_node_architecture.md` for the complete spec. Key design decisions
already resolved:

- **Schema**: `"noet.mapping"` with `[[mappings]]` TOML array-of-tables (§2)
- **IR**: new `IntermediateMappingRelation` struct; new `IRNode.mappings: Vec<IntermediateMappingRelation>` field (§4.1–4.2)
- **Builder**: new `push_mapping` async method; Phase 2b loop after the downstream relations loop in `parse_content` (§6.1–6.2)
- **GC**: tombstone scan in `terminate_stack` for deleted mapping nodes — scan `session_bb.relations()` for edges whose `WEIGHT_OWNED_BY` matches the removed bref (§6.3)
- **Rendering**: `{mapping_table}` directive via the existing `DirectiveDef` + `should_defer` + sentinel pipeline; auto-injected when `schema = "noet.mapping"` (§4b)
- **Viewer**: `EdgeEntry { bid, owner_bid }` replaces raw `Bid` in `NodeContext.graph`; `metadata.js` renders "via \<mapping node link\>" when `owner_bid` is present (§8)
- **Open Q1** (extra payload columns): union all keys, blank cell when absent — confirm before implementing table builder
- **Open Q3** (round-trip fidelity): verify `toml_edit::DocumentMut` preserves `[[mappings]]` array-of-tables ordering with arbitrary extra keys

## Implementation Steps

1. **IR layer** (0.5 days)
   - [ ] Add `IntermediateMappingRelation` struct to `src/codec/belief_ir.rs`
   - [ ] Add `pub mappings: Vec<IntermediateMappingRelation>` field to `IRNode`
   - [ ] Add extraction logic in `MdCodec::parse` — scan front-matter for `[[mappings]]` and populate `proto.mappings`

2. **Schema registry** (0.25 days)
   - [ ] Register `"noet.mapping"` in `src/codec/schema_registry.rs` with empty `graph_fields` (mappings are declared explicitly, not via graph traversal)

3. **Builder: `push_mapping` + Phase 2b** (1 day)
   - [ ] Add `push_mapping` to `impl GraphBuilder` in `src/codec/builder.rs` (see §6.1 for full signature and steps)
   - [ ] Add Phase 2b mapping loop in `parse_content` after the downstream relations loop (see §6.2)
   - [ ] Add tombstone scan in `terminate_stack` for deleted mapping nodes (see §6.3)

4. **`WEIGHT_OWNED_BY` extension** (0.25 days)
   - [ ] Extend `WEIGHT_OWNED_BY` handling in `src/properties.rs` doc comment to document the bref-string case
   - [ ] Update any callers that pattern-match on `"source"` / `"sink"` only to handle arbitrary bref strings

5. **Rendering: `{mapping_table}` directive** (1 day)
   - [ ] Add `mapping_table_query` refiner and `build_mapping_table_html` builder to `src/codec/myst.rs`
   - [ ] Add `DirectiveDef` entry for `mapping_table` to `DIRECTIVES`
   - [ ] Add `is_mapping_schema` detection in `MdCodec`; set `has_deferred_render = true` and auto-append `{mapping_table}` marker in `generate_html` when schema matches (§4b.5)

6. **Viewer wiring** (1 day)
   - [ ] Add `EdgeEntry { bid, owner_bid }` struct to `src/wasm.rs`
   - [ ] Update `NodeContext.graph` type from `Vec<Bid>` to `Vec<EdgeEntry>`
   - [ ] Update `extract_node_context` to populate `owner_bid` from `WEIGHT_OWNED_BY` via `inner.brefs().get(&bref).copied()` (O(log n))
   - [ ] Update `assets/viewer/metadata.js`: `renderRelationGroup` accepts `EdgeEntry`; render "via \<link\>" when `owner_bid` is present

## Testing Requirements

- Unit: `IntermediateMappingRelation` round-trips through `IRNode` for valid and invalid TOML
- Unit: `push_mapping` resolves both endpoints, emits `RelationChange` with correct `WEIGHT_OWNED_BY` bref, returns `Unresolved` for missing endpoints
- Integration: a mapping node file produces the expected edges in `session_bb`; deleting the file removes the owned edges
- Rendering: `{mapping_table}` sentinel is present in `generate_html` output for mapping-schema nodes; `build_mapping_table_html` produces a table with correct source/sink/weight columns
- Viewer: `EdgeEntry.owner_bid` is set for third-party-owned edges; absent for direct edges; "via" link renders correctly in `metadata.js`

## Success Criteria

- [ ] A `schema = "noet.mapping"` document with two `[[mappings]]` entries compiles without errors and registers two `RelationChange` events with `WEIGHT_OWNED_BY` set to the mapping node's bref
- [ ] Deleting the mapping node file removes the owned edges in the next compile pass
- [ ] The HTML output for a mapping node contains a rendered table of its declared edges
- [ ] The viewer shows "via \<mapping node title\>" for edges owned by a mapping node
- [ ] All existing tests pass; new tests cover the above scenarios

## Risks

- **`WEIGHT_OWNED_BY` bref collision with `"source"` / `"sink"` strings**: bref strings are hex (12 chars), so no collision is possible in practice — but callers that pattern-match the field need updating. → **Mitigation**: grep all `WEIGHT_OWNED_BY` read sites before shipping.
- **GC scan cost for large sessions**: O(session edges) scan per deleted mapping node. → **Mitigation**: acceptable for typical use; document and profile if needed (§6.3 notes this explicitly).

## Open Questions

- Confirm Open Q1: union-all-keys sparse table is the agreed default for `build_mapping_table_html` — verify before starting §5
- Confirm Open Q3: manually verify `toml_edit::DocumentMut` round-trips `[[mappings]]` with mixed extra payload keys before shipping `generate_source()`
