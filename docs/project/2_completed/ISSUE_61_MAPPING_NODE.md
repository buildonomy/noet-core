# Issue 61: Mapping Node Implementation

**Priority**: MEDIUM
**Estimated Effort**: 6 days (RELATIVE COMPARISON ONLY)
**Dependencies**: Requires Issue 55 (MyST directive syntax), Blocks Issue 18 (extended procedure schemas)

## Summary

Implement the `{maps_to}` directive — a MyST fenced-block directive that lets any
section or document node own directed edges between two other nodes (source and sink)
without being either endpoint. The owning node is identified by its bref in
`WEIGHT_OWNED_BY`, extending that field beyond its current `"source"` / `"sink"` values.
The full spec will be written to `docs/design/mapping_node_architecture.md` as the final
step of this issue, once the working implementation is in place.

## Goals

- Parse `{maps_to}` directive bodies into `IRNode.mappings` via a new `IntermediateMappingRelation` struct
- Register third-party-owned edges through a new `push_mapping` builder method (Phase 2b)
- Correctly GC owned edges on reparse (changed mappings) and node deletion (removed section)
- Extend `WEIGHT_OWNED_BY` to accept a bref string; add `RelationPred::OwnedBy(Bref)` to the query layer
- Render owned edges as an HTML table in-place of the directive via the deferred-render pipeline
- Surface `owner_bid` in the viewer via `EdgeEntry` and a "via <link>" annotation in `metadata.js`
- Rewrite `docs/design/mapping_node_architecture.md` to reflect the implemented design

## Architecture

### Authoring Convention

Any section or document node may contain a `{maps_to}` fenced directive. The directive
body is parsed as TOML (with YAML/JSON fallback via the existing `parse_with_fallback`).
The fenced info string may carry the `weight_kind` as a shorthand:

```markdown
## req abc trace

These elements satisfy req abc for xyz reasons.

````{maps_to} Pragmatic
source = "id://req_abc"
sink = ["id://int_a", "id://int_b", "id://int_c"]
````
```

Or with weight kind in the body (format-agnostic):

```markdown
````{maps_to}
weight_kind = "Pragmatic"
source = "id://req_abc"
sink = ["id://int_a", "id://int_b", "id://int_c"]
````
```

Multiple `{maps_to}` directives may appear in a single section. Each directive produces
one or more `IntermediateMappingRelation` entries on the enclosing section's `IRNode`.
The owning node is the section node; `WEIGHT_OWNED_BY` is set to the section's bref.

**Dropped from original design**: `schema = "noet.mapping"` front-matter, `[[mappings]]`
TOML array-of-tables, and the standalone mapping document concept. The directive is
self-contained and requires no schema declaration.

### IR Layer

New struct `IntermediateMappingRelation` in `belief_ir.rs`:

```
source: NodeKey        // resolved from directive body "source" field
sink: Vec<NodeKey>     // resolved from directive body "sink" field (array)
kind: WeightKind       // from info-string arg or body "weight_kind" field
weight: Option<Weight> // extra payload fields (all keys except source/sink/weight_kind)
location: Option<usize>
```

New field `pub mappings: Vec<IntermediateMappingRelation>` on `IRNode`. Populated by
`MdCodec::parse` when a `{maps_to}` fenced block is encountered. `IRNode::PartialEq`
must include `mappings`.

### Query Layer: `RelationPred::OwnedBy`

New variant in `query.rs`:

```rust
RelationPred::OwnedBy(Bref)  // matches edges where WEIGHT_OWNED_BY == bref.to_string()
```

`match_ref` inspects `rel.weights` for `WEIGHT_OWNED_BY` and compares to the bref string.
`AsSql` implementation queries `json_extract(payload, '$.owned_by')` (modify initial
migration — all DBs are ephemeral, no migration versioning needed).

Used by:
- `fetch_owned_edges` (new builder helper) for the Phase 2b pre-load
- `mapping_table_query` refiner in `myst.rs` for deferred HTML rendering

### Builder Protocol

**`fetch_owned_edges`** — new async helper on `GraphBuilder`:

```rust
async fn fetch_owned_edges<B: BeliefSource + Clone>(
    &self,
    owner_bref: Bref,
    global_bb: B,
) -> Result<BeliefGraph, BuildonomyError>
```

Three-tier lookup (mirrors `cache_fetch` pattern):
1. `session_bb.evaluate_expression(RelationIn(OwnedBy(owner_bref)))` — synchronous
2. `global_bb.eval_unbalanced(RelationIn(OwnedBy(owner_bref)))` — async, authoritative
3. Empty `BeliefGraph` — valid on first parse, no edges previously registered

Returns a `BeliefGraph` of previously-owned edges. An empty result is not an error.

**Phase 2b pre-load** (in `parse_content`, before new mapping emissions):

For each section node with non-empty `mappings`:
1. Call `fetch_owned_edges(section_bid.bref(), global_bb)` → `previously_owned`
2. Merge into `doc_bb` via `doc_bb.union_mut(&previously_owned)` — direct union only,
   no `merge_from` DFS expansion, to avoid contaminating `doc_bb` with source/sink
   neighborhoods
3. Emit new `RelationChange` events via `push_mapping` into `relation_event_queue`

This gives `compute_diff` the correct baseline: dropped mappings appear in `session_bb`
and `doc_bb` (pre-loaded) but not in the new emissions → `RelationRemoved`. New mappings
appear only in the new emissions → `RelationChange`. Unchanged mappings appear in both
→ no event.

**`push_mapping`** — new async method on `GraphBuilder`:

For each `(source_key, sink_key)` pair in a mapping entry:
1. `cache_fetch` source key (Trace hit acceptable — only BID needed)
2. `cache_fetch` sink key (Trace hit acceptable — only BID needed)
3. If either is `Unresolved`, emit `ParseDiagnostic::UnresolvedReference` and skip
4. Build `Weight`: start from extra payload, set `WEIGHT_SORT_KEY = index`,
   set `WEIGHT_OWNED_BY = owner_bid.bref().to_string()`
5. Push `RelationChange(source_bid, sink_bid, kind, weight)` to `relation_event_queue`
6. Record `(source_bid, sink_bid, kind)` in `owner_index` entry for `owner_bref`

**`owner_index: HashMap<Bref, Vec<(Bid, Bid, WeightKind)>>`** — builder-local field on
`GraphBuilder`. Written by `push_mapping`. Used only by `terminate_stack` for the node
deletion case (where no reparse fires for the deleted section).

**GC — node deletion** (in `terminate_stack`):

For each BID in `removed_nodes` whose bref appears in `owner_index`:
- Emit `RelationRemoved(source_bid, sink_bid, origin)` for each recorded entry
- Remove the entry from `owner_index`

**GC — reparse (changed mappings)**:

Handled automatically by the Phase 2b pre-load + `compute_diff`. No tombstone scan needed.

### `compute_diff` Fix

`base.rs` `compute_diff` currently maps `WEIGHT_OWNED_BY` to source/sink node for
ownership scoping. The bref form falls to the `else → sink` arm, misattributing
mapping-owned edges to their sink. Add a third arm:

```rust
match weight.get::<String>(WEIGHT_OWNED_BY).as_deref() {
    Some("source") => &source,
    Some("sink") | None => &sink,
    Some(bref_str) => {
        // Third-party owned: attribute to the owner node identified by bref.
        // Include in diff scope only if the owner BID is in parsed_nodes.
        // Resolved via session_bb bref index.
        owner_by_bref(bref_str, session_bb, parsed_nodes)
            .unwrap_or(&sink)  // fallback: treat as sink-owned if owner unknown
    }
}
```

### Rendering: `{maps_to}` / `{mapping_table}` Directive

The `{maps_to}` directive is replaced in-place in the rendered HTML by a `{mapping_table}`
sentinel (not appended to the document end). This differs from `requirements_table` which
appends. The marker is emitted at the directive's position during `render_html_body`.

`DirectiveDef` for `mapping_table` in `DIRECTIVES`:

```rust
DirectiveDef {
    name: "mapping_table",
    marker: "<!-- noet-mapping-table -->",
    sentinel: "<!--@@noet-mapping-table@@-->",
    directive: "",
    is_block_opener: false,
    queries: &[mapping_table_query],
    builder: Some(build_mapping_table_html),
}
```

`mapping_table_query` refiner:
```rust
fn mapping_table_query(ctx: &BeliefContext, _graphs: &[BeliefGraph]) -> Expression {
    Expression::RelationIn(RelationPred::OwnedBy(ctx.node.bid.bref()))
}
```

`build_mapping_table_html`: collects edges from `graphs[0]` filtered to
`owned_by == ctx.node.bref()`, resolves source/sink titles and URLs from the merged
belief base, renders an HTML table with Source / Kind / Sink columns plus any extra
payload keys (sparse, blank when absent). Empty result renders
`<p><em>No mappings declared.</em></p>`.

`MdCodec::parse` sets `has_deferred_render = true` when a `{maps_to}` directive is
encountered (same flag used by `requirements_table`). The `{maps_to}` directive marker
is emitted at the directive's source position in `render_html_body`; `promote_markers`
converts it to the sentinel.

### Viewer Integration

`EdgeEntry { bid: Bid, owner_bid: Option<Bid> }` replaces raw `Bid` in
`NodeContext.graph`. `extract_node_context` populates `owner_bid` by reading
`WEIGHT_OWNED_BY` from each edge's weight: if the value is a bref string (neither
`"source"` nor `"sink"`), resolve to a `Bid` via `session_bb.brefs().get(&bref)`.
`metadata.js` `renderRelationGroup` renders "via \<title link\>" when `owner_bid` is
present.

## Implementation Steps

1. **Query layer: `RelationPred::OwnedBy`** (0.25 days)
   - [x] Add `OwnedBy(Bref)` variant to `RelationPred` in `src/query.rs`
   - [x] Implement `match_ref` for `OwnedBy`: inspect `rel.weights` payload for `WEIGHT_OWNED_BY`
   - [x] Implement `AsSql` for `OwnedBy`: `json_extract(epistemic/pragmatic/section, '$.owned_by') = ?`
   - [x] Update initial DB migration to include `owned_by` in the payload index if needed

2. **IR layer** (0.5 days)
   - [x] Add `IntermediateMappingRelation` struct to `src/codec/belief_ir.rs`
   - [x] Add `pub mappings: Vec<IntermediateMappingRelation>` field to `IRNode`; update `PartialEq` and `Default`
   - [x] Add `{maps_to}` directive parsing in `MdCodec::parse` (`src/codec/md.rs`):
     - Detect fenced block with `{maps_to}` info string
     - Extract weight_kind from info-string arg or body field (via `WeightKind::try_from`)
     - Parse body via `parse_with_fallback` (TOML-first; made `pub(crate)`)
     - Populate `current.mappings`; set `self.has_deferred_render = true`
     - Emit `marker("mapping_table")` at directive position in `render_html_body`

3. **`WEIGHT_OWNED_BY` extension** (0.25 days)
   - [x] Update doc comment on `WEIGHT_OWNED_BY` in `src/properties.rs` to document bref-string case
   - [x] Add bref arm to `compute_diff` ownership attribution in `src/beliefbase/base.rs` (both sites; uses `weight.get::<String>()` + `Bref::try_from`)
   - [x] Add `'o'` display arm to `display_contents` in `src/beliefbase/graph.rs` for bref-owned edges

4. **Builder: `fetch_owned_edges` + `push_mapping` + Phase 2b** (1.5 days)
   - [x] Add `owner_index: HashMap<Bref, Vec<(Bid, Bid, WeightKind)>>` field to `GraphBuilder`
   - [x] Add `fetch_owned_edges` async helper to `impl GraphBuilder` in `src/codec/builder.rs`
   - [x] Add `push_mapping` async method to `impl GraphBuilder`
   - [x] Add Phase 2b loop in `parse_content`: pre-load via `fetch_owned_edges` + `doc_bb.merge` (not union_mut — that's on BeliefGraph, not BeliefBase), then emit via `push_mapping`
   - [x] Add GC scan in `terminate_stack`: emit `RelationRemoved` for each `owner_index` entry whose bref matches a removed node

5. **Rendering: `{mapping_table}` directive** (1 day)
   - [x] Add `DirectiveDef` entry for `maps_to` to `DIRECTIVES` in `src/codec/myst.rs`
   - [x] Add `mapping_table_query` refiner: `RelationIn(OwnedBy(ctx.node.bid.bref()))`
   - [x] Add `build_mapping_table_html` builder (sparse extra-payload columns, empty-state message)

6. **Viewer wiring** (0.75 days)
   - [x] Add `EdgeEntry { bid, owner_bid }` struct to `src/wasm.rs`
   - [x] Update `NodeContext.graph` type from `Vec<Bid>` to `Vec<EdgeEntry>`
   - [x] Update `extract_node_context` to populate `owner_bid` from bref-valued `WEIGHT_OWNED_BY` (brefs snapshot before get_context borrow to avoid E0502)
   - [x] Update `assets/viewer/metadata.js`: `renderRelationGroup` accepts `EdgeEntry`; render "via \<link\>" when `owner_bid` is present; backward-compatible with plain string bids

7. **Rewrite design doc** (0.5 days)
   - [x] Rewrite `docs/design/mapping_node_architecture.md` to reflect the implemented directive-first design, correct GC mechanics, `fetch_owned_edges` three-tier pattern, `RelationPred::OwnedBy`, and `compute_diff` bref arm

## Testing Requirements

- Unit: `IntermediateMappingRelation` parsed correctly from TOML, YAML, and JSON directive bodies; weight_kind from info string takes precedence over body field
- Unit: `RelationPred::OwnedBy` `match_ref` matches edges with bref-valued `WEIGHT_OWNED_BY`; does not match `"source"` / `"sink"` valued edges
- Unit: `fetch_owned_edges` returns empty graph on first parse; returns prior edges from `session_bb` on reparse; falls back to `global_bb` when `session_bb` is empty (parallel task scenario)
- Unit: `push_mapping` emits `RelationChange` with correct `WEIGHT_OWNED_BY` bref; returns `Unresolved` for missing endpoints; records in `owner_index`
- Integration: a section with `{maps_to}` compiles and registers the correct edges in `session_bb` with `WEIGHT_OWNED_BY` set to the section's bref
- Integration: removing a `{maps_to}` directive (edit reparse) removes the owned edges from `session_bb`
- Integration: deleting the section node (heading removed) removes the owned edges via `owner_index` GC in `terminate_stack`
- Integration: endpoints that are unresolved on first parse trigger requeue and resolve on second pass
- Rendering: `{maps_to}` directive position in rendered HTML is replaced by the `mapping_table` sentinel; `build_mapping_table_html` produces a table with correct Source / Kind / Sink columns and sparse extra-payload columns
- Viewer: `EdgeEntry.owner_bid` is set for bref-owned edges; `None` for `"source"` / `"sink"` owned edges; "via" link renders correctly in `metadata.js`

## Success Criteria

- [ ] A section containing `{maps_to} Pragmatic` with a `source` and multiple `sink` entries compiles without errors and registers one `RelationChange` per sink in `session_bb`, each with `WEIGHT_OWNED_BY` set to the section's bref
- [ ] Editing the `{maps_to}` body (adding or removing a sink) on reparse emits the correct `RelationChange` and `RelationRemoved` events — no stale edges persist
- [ ] Deleting the section heading removes the owned edges in the same compile pass via `owner_index` GC
- [ ] The compiled HTML for a section with `{maps_to}` contains a rendered mapping table at the directive's source position
- [ ] The viewer shows "via \<section title\>" for edges owned by a section node
- [ ] All existing tests pass; new unit and integration tests cover the above scenarios
- [ ] `docs/design/mapping_node_architecture.md` is rewritten to match the implemented design

## Risks

- **Parallel epoch task `session_bb` isolation**: each parallel task builder has a fresh `session_bb` seeded only from the epoch snapshot (network ancestors + Section edges). Previously-owned mapping edges from sibling leaf documents will not be in `session_bb`. `fetch_owned_edges` must reliably fall back to `global_bb` in this case. → **Mitigation**: `fetch_owned_edges` always checks `global_bb` when `session_bb` returns empty; covered by unit test.
- **`compute_diff` bref-owner attribution**: the new bref arm in `compute_diff` must resolve bref → BID via `session_bb`'s bref index. If the owner node has been removed in the same pass, its bref may not resolve. → **Mitigation**: fall back to sink-owned behavior (existing default); edge will be GC'd by `terminate_stack` owner_index scan anyway.
- **`owner_index` lost on compiler restart**: the index is builder-local and not persisted. On a fresh compiler start, if a section with `{maps_to}` is not in the parse queue (mtime hit), the index is empty and `terminate_stack` cannot GC via it. → **Mitigation**: the Phase 2b pre-load via `fetch_owned_edges` + `doc_bb.union_mut` + `compute_diff` handles the reparse GC path independently; `owner_index` is only needed for the within-session node-deletion path, which always fires a reparse of the affected file.
- **`WEIGHT_OWNED_BY` bref vs `"source"` / `"sink"` ambiguity**: bref strings are 8 hex chars; no collision with the literal strings `"source"` or `"sink"` is possible. → **Mitigation**: no action needed beyond the third match arm.