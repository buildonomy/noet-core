---
version = "0.1"
title = "Mapping Node Architecture"
---

# Mapping Node Architecture

**Version**: 0.1  
**Status**: Implemented (Issue 61)  
**Key files**: `src/query/spec.rs`, `src/codec/belief_ir.rs`, `src/codec/md.rs`,
`src/codec/myst.rs`, `src/codec/builder.rs`, `src/beliefbase/base.rs`,
`src/wasm.rs`, `assets/viewer/metadata.js`

---

## 1. Purpose

The `{maps_to}` directive lets any section or document node own directed edges
between two other nodes without being either endpoint. The owning node is a
third-party observer — it declares a relationship between a `source` and a
`sink` and is responsible for the lifecycle of those edges.

**Motivating use case**: a traceability matrix section that records which
implementation nodes satisfy which requirements, without modifying either the
requirement nodes or the implementation nodes.

Edge direction follows the same convention as all other edges in the graph:

- **source** — the upstream node (e.g. a requirement). Sources are agnostic;
  they do not know about their sinks.
- **sink** — the downstream node (e.g. an implementor). Sinks depend on their
  sources.

This mirrors the `{implements}` block: an implementing section is the sink; the
requirements it links to are the sources.

---

## 2. Authoring Convention

A `{maps_to}` directive is a MyST fenced block placed inside any section or
document node. The directive body is parsed as TOML (with YAML/JSON fallback
via `parse_with_fallback`). An optional weight kind may be supplied as an
info-string argument.

```markdown
## Trace Mapping

This section owns the edges below. It is neither a source nor a sink.

````{maps_to} Pragmatic
source = ["id://req-alpha", "id://req-beta"]
sink = "id://impl-one"
````
```

### 2.1 Field reference

| Field | Type | Description |
|---|---|---|
| `source` | string or array | One or more source node keys. |
| `sink` | string or array | One or more sink node keys. |
| `weight_kind` | string | Edge weight kind. Overridden by info-string arg when present. Defaults to `Pragmatic`. |
| *(any other key)* | scalar | Stored as extra payload on every emitted edge. |

Both `source` and `sink` accept either a single node key string or an array of
node key strings. One edge is emitted for every element of the Cartesian
product `sources × sinks`.

### 2.2 Info-string shorthand

The weight kind may be placed directly in the info string:

```markdown
````{maps_to} Pragmatic
source = "id://req-alpha"
sink = "id://impl-one"
````
```

The info-string value is parsed via `WeightKind::try_from(&str)` and takes
precedence over a `weight_kind` key in the body. When neither is present the
kind defaults to `Pragmatic`.

### 2.3 Multiple directives per section

Multiple `{maps_to}` directives may appear in a single section. Each directive
produces one `IntermediateMappingRelation` on the enclosing section's `IRNode`.
All are processed independently during Phase 2b.

### 2.4 NodeKey format

`source` and `sink` values follow the standard NodeKey URL format:

- `id://anchor-slug` — section by anchor
- `bid://bref` — node by bref
- `path://net/file.md` — node by path
- `bref://hexstring` — node by bref (alternate form)

Relative path keys are resolved against the owning document's path via
`NodeKey::resolve_against`.

---

## 3. Dropped from original design

The following concepts appeared in earlier drafts and were deliberately removed:

- `schema = "noet.mapping"` front-matter and standalone mapping document
- `[[mappings]]` TOML array-of-tables authoring convention
- Schema registry entry for `"noet.mapping"`
- `is_mapping_schema: bool` on `MdCodec`
- Tombstone scan in `terminate_stack` (replaced by `owner_index` + `compute_diff`)

---

## 4. Intermediate Representation

### 4.1 `IntermediateMappingRelation`

Defined in `src/codec/belief_ir.rs`:

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct IntermediateMappingRelation {
    /// Source node keys (upstream, e.g. requirements). Accepts scalar or array in TOML body.
    pub sources: Vec<NodeKey>,
    /// Sink node keys (downstream, e.g. implementors). Accepts scalar or array in TOML body.
    pub sinks: Vec<NodeKey>,
    /// Edge weight kind (from info-string arg or "weight_kind" body field).
    pub kind: WeightKind,
    /// Extra payload fields (all body keys except source, sink, weight_kind).
    pub weight: Option<Weight>,
    /// Byte offset in the source document, for diagnostics.
    pub location: Option<usize>,
}
```

One `IntermediateMappingRelation` is produced per `{maps_to}` directive.
`push_mapping` then emits one `RelationChange` per `(source_i, sink_j)` pair.

### 4.2 `IRNode.mappings`

```rust
pub struct IRNode {
    // ... existing fields ...
    pub mappings: Vec<IntermediateMappingRelation>,
}
```

`mappings` is included in `IRNode`'s hand-implemented `PartialEq`. It defaults
to an empty `Vec` so all existing `IRNode` construction sites are unaffected
(they use `..Default::default()`).

### 4.3 Parsing in `MdCodec::parse`

Three fields are added to `MdCodec`:

```rust
in_maps_to_block: bool,
maps_to_body_accum: String,
maps_to_weight_kind_override: Option<WeightKind>,
```

State machine in the parse event loop:

1. `Start(CodeBlock(Fenced("{maps_to} ...")))` — set `in_maps_to_block = true`,
   parse info-string arg via `WeightKind::try_from`, set `has_deferred_render = true`.
2. `Text(...)` while `in_maps_to_block` — accumulate into `maps_to_body_accum`.
   This accumulator is **separate** from `current.accumulator` (which is used for
   heading titles and frontmatter).
3. `End(CodeBlock)` while `in_maps_to_block` — call
   `parse_with_fallback(&body, MetadataFormat::Toml)`, extract `source`, `sink`,
   `weight_kind`, and extra payload keys. Resolve node keys via
   `NodeKey::from_str` + `resolve_against`. Push to `current.mappings`.
4. `Start(Heading(...))` — auto-clears `in_maps_to_block` (same as
   `in_implements_block`).

`parse_with_fallback` is `pub(crate)` to allow access from `md.rs`.

The directive body helper `parse_node_keys(doc, field, base_path)` handles both
scalar and array forms for `source` and `sink` symmetrically.

---

## 5. Query Layer: Owner Traversal

Owned-edge queries use `TraversalSpec` with `Role::Owner` as the input role.
The traversal walks edges whose `WEIGHT_OWNED_BY` payload matches the input
node’s bref, returning the source and sink endpoints of those edges.

### 5.1 In-Memory (`BeliefBase`)

The in-memory evaluator iterates `raw_edges()` and checks
`weight.get::<String>(WEIGHT_OWNED_BY)` against the input node’s bref.
Matching edges’ source and sink BIDs are collected as the traversal output.

### 5.2 SQL (`DbConnection`)

The DB schema stores weights in per-kind columns (`epistemic TEXT`, `section
TEXT`, `pragmatic TEXT`), each holding a TOML-serialized `Weight` payload. The
SQL form for an Owner-input traversal checks all three columns:

```sql
SELECT DISTINCT source, sink FROM relations
WHERE json_extract(epistemic, '$.owned_by') = ?
   OR json_extract(pragmatic, '$.owned_by') = ?
   OR json_extract(section,   '$.owned_by') = ?
```

This returns the participating BID set. The evaluator then fetches full states
for discovered endpoints via a bulk `SELECT` in the same `evaluate` call.

---

## 6. `WEIGHT_OWNED_BY` Extension

`WEIGHT_OWNED_BY = "owned_by"` (defined in `src/properties.rs`) previously
accepted only `"source"` or `"sink"`. It now accepts a third form: a **bref
hex string** identifying a third-party owner node.

### 6.1 Extended contract

| Value | Meaning |
|---|---|
| `"source"` | Edge owned by the source node (e.g. `parent_connections`). |
| `"sink"` or absent | Edge owned by the sink node (default). |
| *bref hex string* | Edge owned by a third-party section node (a `{maps_to}` owner). |

### 6.2 `compute_diff` bref arm

`compute_diff` in `src/beliefbase/base.rs` has two symmetric sites (new
relations and old relations) that attribute each edge to its owner for GC
scoping. A third match arm was added to both:

```rust
match weight.get::<String>(WEIGHT_OWNED_BY).as_deref() {
    Some("source") => (&source, "+"),
    Some("sink") | None => (&sink, "-"),
    Some(bref_str) => {
        // Resolve bref → BID via the appropriate set's bref map.
        // Site 1 (new_relations): uses new_set.brefs()
        // Site 2 (old_relations): uses old_set.brefs()
        owner_bid_buf = Bref::try_from(bref_str)
            .ok()
            .and_then(|bref| set.brefs().get(&bref).copied());
        owner_bid_buf
            .as_ref()
            .map(|b| (b, "o"))
            .unwrap_or((&sink, "-"))
    }
}
```

If the bref does not resolve (owner node was deleted in the same pass), the
edge falls back to sink-owned behavior. It will be retracted by the
`owner_index` GC scan in `terminate_stack` anyway.

### 6.3 `display_contents` arm

`src/beliefbase/graph.rs` `display_contents` renders bref-owned edges with `"o"`:

```
Kind[o]   ← third-party owner
Kind[+]   ← source-owned
Kind[-]   ← sink-owned (default)
```

---

## 7. Builder Protocol

All changes are in `src/codec/builder.rs`.

### 7.1 `owner_index`

A new field on `GraphBuilder`:

```rust
owner_index: HashMap<Bref, Vec<(Bid, Bid, WeightKind)>>
```

Key: owner section bref. Value: list of `(source_bid, sink_bid, kind)` tuples
emitted by `push_mapping` during the current `parse_content` call. Used
exclusively by `terminate_stack` for within-session node-deletion GC. Not
persisted across compiler restarts.

### 7.2 `fetch_owned_edges`

```rust
async fn fetch_owned_edges<B: BeliefSource + Clone>(
    &self,
    owner_bref: Bref,
    global_bb: B,
) -> Result<BeliefGraph, BuildonomyError>
```

Three-tier lookup:

1. `session_bb` owner traversal — synchronous, fast. Evaluates a
   `QuerySpec` with `Role::Owner` input against the local `session_bb`.
   Hits when the owner’s edges are already in `session_bb` from a prior parse.
2. `global_bb.evaluate(&mut package)` — async, authoritative. Evaluates
   the same `QuerySpec` against the global `BeliefSource`. Required for
   parallel epoch tasks where `session_bb` is seeded only from network
   ancestors and will not contain mapping edges from sibling leaf documents
   parsed in the same epoch.
3. Empty `BeliefGraph` — valid on first parse (no prior edges exist). Not an
   error.

### 7.3 `push_mapping`

```rust
async fn push_mapping<B: BeliefSource + Clone>(
    &mut self,
    mapping: &IntermediateMappingRelation,
    owner_bid: &Bid,
    index: usize,
    _content: &str,
    global_bb: B,
    update_queue: &mut Vec<BeliefEvent>,
    missing_structure: &mut BeliefGraph,
    diagnostics: &mut Vec<ParseDiagnostic>,
    parse_number: usize,
) -> Result<(), BuildonomyError>
```

1. Resolves all `sources` keys via `cache_fetch`. Unresolvable keys emit
   `ParseDiagnostic::UnresolvedReference` and are skipped individually.
2. Resolves all `sinks` keys via `cache_fetch`. Same skip-on-unresolved logic.
3. If either resolved set is empty after all keys are attempted, returns early.
4. Emits one `RelationChange(source_bid, sink_bid, kind, Some(weight), Remote)`
   per `(source_i, sink_j)` pair in the Cartesian product.
5. Each weight carries `WEIGHT_SORT_KEY` (encoding `mapping_index * 256 +
   pair_index` as `u16`) and `WEIGHT_OWNED_BY` (owner bref string).
6. Records each emitted pair in `owner_index` under the owner bref.

### 7.4 Phase 2b: per-node mapping loop

Inside the Phase 2 `for (proto, bid)` loop in `parse_content`, after
`upstream` and `downstream` relations are processed:

```rust
if !proto.mappings.is_empty() {
    let owner_bref = bid.bref();
    let previously_owned = self
        .fetch_owned_edges(owner_bref, global_bb.clone())
        .await?;
    if !previously_owned.is_empty() {
        self.doc_bb.merge(&previously_owned);
    }
    for (mapping_idx, mapping) in proto.mappings.iter().enumerate() {
        self.push_mapping(mapping, bid, mapping_idx, ...).await?;
    }
}
```

**Why `doc_bb.merge` and not `merge_from`**: `merge` with no seed restriction
includes only what is in the `previously_owned` graph itself. `merge_from`
performs a DFS through `session_bb` following Section edges — that would pull
in the source/sink node neighborhoods and corrupt `compute_diff`'s scope.

**Why this gives correct GC**: after pre-loading `previously_owned` into
`doc_bb` and emitting new `RelationChange` events via `push_mapping`:
- Dropped mappings exist in `session_bb` and `doc_bb` (pre-loaded) but are
  absent from the new emissions → `compute_diff` produces `RelationRemoved`.
- New mappings appear only in the new emissions → `RelationChange`.
- Unchanged mappings appear in both → no event.

### 7.5 GC: `terminate_stack`

Before `compute_diff` runs, `terminate_stack` scans `owner_index` for entries
whose owner BID is in `removed_nodes` (collision-removed nodes from the current
pass). For each such entry, `RelationRemoved` events are emitted directly into
`tx_events`.

This handles the within-session node-deletion case (a heading with a `{maps_to}`
directive is removed from the source file in the same compile pass). The reparse
GC path (changed mappings, not deleted heading) is handled independently by the
Phase 2b pre-load + `compute_diff` cycle and does not require `owner_index`.

---

## 8. Rendering Pipeline

The `{maps_to}` directive renders **in-place** (at the directive's source
position), unlike `{requirements_table}` which appends to the document end.

```
[doc.md] containing ````{maps_to} Pragmatic ...````
        │
        ▼  MdCodec::parse()
   sets has_deferred_render = true
   sets in_maps_to_block = true; accumulates body; parses on End(CodeBlock)
   populates IRNode.mappings
        │
        ▼  MdCodec::render_html_body()
   second pass over stored events; fenced block replaced by:
       MdEvent::Html("<!-- noet-mapping-table -->")
        │
        ▼  MdCodec::generate_html() → promote_markers()
   "<!-- noet-mapping-table -->" → "<!--@@noet-mapping-table@@-->"
        │
        ▼  compiler.rs: generate_html_for_path()
   mapping_table_query: Owner traversal via TraversalSpec(Role::Owner) → graphs[0]
   build_mapping_table_html: renders Source / Kind / Sink table
   splice_sentinels: replaces sentinel with table HTML
```

### 8.1 `DirectiveDef` entry (`src/codec/myst.rs`)

```rust
DirectiveDef {
    name: "maps_to",
    marker: "<!-- noet-mapping-table -->",
    sentinel: "<!--@@noet-mapping-table@@-->",
    directive: "",
    is_block_opener: false,
    queries: &[mapping_table_query],
    builder: Some(build_mapping_table_html),
},
```

### 8.2 `mapping_table_query`

```rust
fn mapping_table_query(ctx: &BeliefContext, _graphs: &[BeliefGraph]) -> QuerySpec {
    QuerySpec {
        subject: Subject::Bids(vec![ctx.node.bid]),
        projection: vec![ProjectionStep::traverse(TraversalSpec {
            input_roles: Role::Owner.into(),
            kind_filter: EnumSet::all(),
            output_roles: Role::Source | Role::Sink,
            depth: TraversalDepth::count(1),
        })],
    }
}
```

### 8.3 `build_mapping_table_html`

Collects edges from `graphs[0]` where `WEIGHT_OWNED_BY == owner_bref_str`.
Renders an HTML table with columns: Source / Kind / Sink, plus one additional
column per extra payload key that appears on any row (sparse; blank when absent).
Empty result renders `<p><em>No mappings declared.</em></p>`.

---

## 9. Viewer Integration (`src/wasm.rs`, `assets/viewer/metadata.js`)

### 9.1 `EdgeEntry`

```rust
#[cfg(feature = "wasm")]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EdgeEntry {
    /// BID of the source or sink node at the other end of this edge.
    pub bid: Bid,
    /// BID of the section node that owns this edge via a `{maps_to}` directive.
    /// `None` for standard source-owned or sink-owned edges.
    pub owner_bid: Option<Bid>,
}
```

### 9.2 `NodeContext.graph`

Changed from `HashMap<WeightKind, (Vec<Bid>, Vec<Bid>)>` to
`HashMap<WeightKind, (Vec<EdgeEntry>, Vec<EdgeEntry>)>`.

### 9.3 `extract_node_context`

The `brefs` map is snapshotted before calling `get_context()` to avoid a borrow
conflict (`get_context` holds a mutable borrow on `inner`; the closure needs an
immutable borrow for bref resolution):

```rust
let brefs_snapshot = inner.brefs().clone();

inner.get_context(ns, bid).map(|ctx| {
    let resolve_owner_bid = |weight: &Weight| -> Option<Bid> {
        match weight.get::<String>(WEIGHT_OWNED_BY).as_deref() {
            Some("source") | Some("sink") | None => None,
            Some(bref_str) => Bref::try_from(bref_str)
                .ok()
                .and_then(|bref| brefs_snapshot.get(&bref).copied()),
        }
    };
    // ... populate EdgeEntry with owner_bid from resolve_owner_bid ...
})
```

### 9.4 `renderRelationGroup` (`metadata.js`)

The function now accepts `Array<{bid, owner_bid}>` instead of `string[]`. It
remains backward-compatible with plain string BIDs (legacy path). When
`owner_bid` is present, a "via `<title link>`" annotation is appended to the
list item:

```javascript
if (ownerBid) {
  const ownerNode = related_nodes.get(ownerBid);
  if (ownerNode) {
    viaHtml = ` <span class="noet-via-owner">via <a ...>${ownerTitle}</a></span>`;
  }
}
html += `<li>${itemHtml}${viaHtml}</li>`;
```

---

## 10. Testing

### 10.1 Unit tests

- `{maps_to}` with TOML body: `IntermediateMappingRelation` parsed with correct
  `sources`, `sinks`, `kind`, extra payload.
- Info-string weight kind takes precedence over body `weight_kind` field.
- `source` and `sink` each accept scalar string or array.
- Missing `source` or empty `sink` → `ParseDiagnostic::Warning`, directive skipped.
- Owner traversal via `TraversalSpec` with `Role::Owner` input: matches
  bref-valued `WEIGHT_OWNED_BY`; does not match `"source"` or `"sink"`.

### 10.2 Integration tests (`tests/codec_test/mapping_tests.rs`)

The fixture `tests/network_1/mapping_test.md` exercises the full pipeline:

```markdown
````{maps_to} Pragmatic
source = ["id://req-alpha", "id://req-beta"]
sink = "id://impl-one"
````
```

- `test_maps_to_produces_owned_pragmatic_edges`: verifies two Pragmatic edges
  exist with `WEIGHT_OWNED_BY` set to the "Trace Mapping" section's bref (not
  `"source"` or `"sink"`).
- `test_maps_to_directive_survives_rewrite_round_trip`: verifies the directive
  body is preserved verbatim in the rewritten Markdown source.
- `test_maps_to_one_edge_per_sink`: verifies exactly 2 edges (Cartesian product
  of 2 sources × 1 sink).

### 10.3 Rendering tests

- `{maps_to}` directive position in rendered HTML is replaced by the
  mapping-table sentinel.
- `build_mapping_table_html` produces a table with correct Source / Kind / Sink
  columns and sparse extra-payload columns.
- Empty owned edges → `<p><em>No mappings declared.</em></p>`.

### 10.4 Viewer tests

- `EdgeEntry.owner_bid` is `Some` for bref-owned edges; `None` for
  `"source"` / `"sink"` owned edges.
- "via `<section title>`" renders correctly in `metadata.js`.

---

## 11. File Map

| File | Change |
|---|---|
| `src/query/spec.rs` | Owner traversal via `TraversalSpec` with `Role::Owner` input role (replaces former `RelationPred::OwnedBy`) |
| `src/codec/belief_ir.rs` | Added `IntermediateMappingRelation`; added `IRNode.mappings`; made `parse_with_fallback` `pub(crate)` |
| `src/codec/md.rs` | Added `in_maps_to_block` state machine; `parse_node_keys` helper; `{maps_to}` body parsing |
| `src/codec/myst.rs` | Added `maps_to` `DirectiveDef`; `mapping_table_query`; `build_mapping_table_html` |
| `src/codec/builder.rs` | Added `owner_index`; `fetch_owned_edges`; `push_mapping`; Phase 2b loop; `terminate_stack` GC |
| `src/beliefbase/base.rs` | Extended `compute_diff` with bref arm at both sites |
| `src/beliefbase/graph.rs` | Added `"o"` display arm in `display_contents` |
| `src/properties.rs` | Updated `WEIGHT_OWNED_BY` doc comment |
| `src/wasm.rs` | Added `EdgeEntry`; updated `NodeContext.graph`; updated `extract_node_context` |
| `assets/viewer/metadata.js` | Updated `renderRelationGroup` to accept `EdgeEntry`; render "via" annotation |
| `tests/network_1/mapping_test.md` | Integration test fixture |
| `tests/codec_test/mapping_tests.rs` | Three integration tests |