---
title = "Mapping Node Architecture"
version = "0.1"
---

# Mapping Node Architecture

## 1. Purpose

This document specifies the **mapping node** — a first-class belief node that *owns*
directed edges between two other nodes (source and sink) without being either endpoint.
It is a reification of an edge: a node that says "A relates to B in manner M, and I
declare and maintain that relationship."

**Why reify edges?**
- A third-party document (an audit record, a traceability matrix, a cross-cutting
  concern) needs stable, versioned ownership of connections it did not author.
- `WEIGHT_OWNED_BY` currently accepts only `"source"` or `"sink"`. This design extends
  it to accept a bref, enabling any node to claim ownership.
- The mapping node gets its own BID, title, section parent, and prose content — all the
  normal machinery — so it participates in search, history, and the viewer like any
  other document.

**Scope of this document:**

- TOML schema for declaring mappings (`noet.mapping`)
- IR extension (`IntermediateMappingRelation`, `IRNode.mappings`)
- Builder protocol (`push_mapping`, Phase 2b, GC semantics)
- `WEIGHT_OWNED_BY` extension to accept a bref string
- Viewer wiring (`EdgeEntry` struct, `metadata.js` rendering)

**Out of scope:**

- MyST directive shorthand for inline mapping declarations (deferred)
- Query language extensions for traversing ownership chains
- Bidirectional / undirected edge semantics (all edges remain directed)

**Rendering** — a mapping node renders its declared edges as an HTML table, using the
same deferred-render pipeline (`DirectiveDef` + `should_defer` + sentinel splicing) that
`{requirements_table}` uses. The table is injected automatically; no directive needs to
be written in the mapping node's Markdown body.

---

## 2. Authoring Convention

A mapping node is a normal Markdown or TOML file. Its TOML front-matter declares
`schema = "noet.mapping"` and a `[[mappings]]` array-of-tables. Each entry specifies
one owned edge.

### 2.1. Minimal example

```toml
title = "Authentication → Audit requirement link"
schema = "noet.mapping"

[[mappings]]
source = "auth-module"        # NodeKey: resolves by id, bref, title, or path
sink   = "audit-req-7"
weight_kind = "Pragmatic"
```

This mapping node:
1. Is a first-class document with its own BID and section parent.
2. Owns a `Pragmatic` edge from `auth-module` to `audit-req-7`.
3. Appears in `network_children` listings — not hidden.

### 2.2. Multiple owned edges

```toml
title = "Sprint 3 traceability"
schema = "noet.mapping"

[[mappings]]
source = "feature-login"
sink   = "req-001"
weight_kind = "Pragmatic"
notes = "implements core login flow"

[[mappings]]
source = "feature-login"
sink   = "req-002"
weight_kind = "Pragmatic"
notes = "partially satisfies security requirement"

[[mappings]]
source = "feature-session"
sink   = "req-003"
weight_kind = "Epistemic"
```

### 2.3. NodeKey resolution for `source` and `sink`

The `source` and `sink` fields resolve using the standard `NodeKey` hierarchy
(see `beliefbase_architecture.md` §2.2). Accepted forms:

| Form | Example |
|------|---------|
| User-defined ID | `"auth-module"` |
| Bref (8 hex chars) | `"1a2b3c4d"` |
| Relative path | `"../requirements/req-001.md"` |
| Title | `"Authentication Module"` |

Resolution follows the same rules as Markdown link targets: relative paths are
resolved against the mapping node's own file location; bare strings are tried as
ID, then bref, then title.

### 2.4. Supported `weight_kind` values

Any `WeightKind` variant name (case-insensitive):

| Value | Meaning |
|-------|---------|
| `"Epistemic"` | Knowledge / reference link (default in normal Markdown) |
| `"Pragmatic"` | Intent / traceability link (as used by `{implements}`) |
| `"Section"` | Structural containment (rarely declared manually) |

### 2.5. Optional weight payload fields

Any additional key-value pairs in a `[[mappings]]` entry are carried as payload on
the edge's `Weight`. They are preserved through round-trips. Example:

```toml
[[mappings]]
source = "component-a"
sink   = "req-x"
weight_kind = "Pragmatic"
notes  = "justification text"
confidence = 0.85
```

The `notes` and `confidence` keys are stored in `Weight.payload` and surfaced in the
viewer's relation detail panel.

---

## 3. Schema Registry Entry

`SchemaRegistry::create()` in `src/codec/schema_registry.rs` registers:

```rust
registry.register(
    "noet.mapping".to_string(),
    SchemaDefinition {
        // [[mappings]] is handled explicitly in belief_ir.rs (not via GraphField),
        // because each entry contains two NodeKey fields rather than one.
        graph_fields: vec![],
    },
);
```

The `[[mappings]]` array is **not** handled through the generic `GraphField` mechanism.
`GraphField` assumes a single `NodeKey` per field (upstream or downstream). Mapping
entries have two (`source` AND `sink`) and a third-party owner, so they are extracted
ad-hoc in `belief_ir.rs` when `schema == "noet.mapping"`.

---

## 4. Intermediate Representation

### 4.1. New struct: `IntermediateMappingRelation`

Add to `src/codec/belief_ir.rs`:

```rust
/// A single owned-edge declaration from a mapping node.
///
/// Distinct from [`IntermediateRelation`]: carries two target keys (source AND sink)
/// rather than one, and the owning node is a third party (neither source nor sink).
/// Populated by codec parsers when they encounter `schema = "noet.mapping"`.
#[derive(Debug, Clone, PartialEq)]
pub struct IntermediateMappingRelation {
    /// NodeKey identifying the source of the owned edge.
    pub source: NodeKey,
    /// NodeKey identifying the sink of the owned edge.
    pub sink: NodeKey,
    /// WeightKind for the owned edge.
    pub kind: WeightKind,
    /// Optional extra weight payload declared in the [[mappings]] entry
    /// (e.g. "notes", "confidence"). Keys "source", "sink", and "weight_kind"
    /// are consumed during parsing and do not appear here.
    pub weight: Option<Weight>,
    /// Byte offset into the source document where this entry was declared, if known.
    pub location: Option<usize>,
}

impl IntermediateMappingRelation {
    pub fn new(source: NodeKey, sink: NodeKey, kind: WeightKind, weight: Option<Weight>) -> Self {
        Self { source, sink, kind, weight, location: None }
    }

    pub fn with_location(mut self, byte_offset: usize) -> Self {
        self.location = Some(byte_offset);
        self
    }
}
```

### 4.2. New field on `IRNode`

```rust
pub struct IRNode {
    // ... existing fields ...
    /// Mapping relations declared by this node (populated when schema = "noet.mapping").
    /// Each entry owns a directed edge source → sink with this node as the third-party owner.
    pub mappings: Vec<IntermediateMappingRelation>,
}
```

`IRNode::PartialEq` must include `mappings` in its comparison.

`IRNode::default()` initialises `mappings` as `Vec::new()`.

### 4.3. Codec extraction

In `belief_ir.rs`, when processing a node whose `document["schema"] == "noet.mapping"`,
iterate the `[[mappings]]` array-of-tables:

```
for each entry in document["mappings"]:
    source_key = NodeKey::from_str(entry["source"])   // existing resolution logic
    sink_key   = NodeKey::from_str(entry["sink"])
    kind       = WeightKind::try_from(entry["weight_kind"])  // error → ParseDiagnostic
    weight     = extract_payload_fields(entry, skip=["source","sink","weight_kind"])
    mappings.push(IntermediateMappingRelation::new(source_key, sink_key, kind, weight))
```

Unrecognised `weight_kind` values emit a `ParseDiagnostic::Warning` and skip the entry
(do not abort the parse).

Missing `source` or `sink` keys emit `ParseDiagnostic::Warning` and skip the entry.

---

## 4b. Rendering: `{mapping_table}` Directive

A mapping node automatically includes a rendered table of its owned edges in the
compiled HTML. This is wired through the existing deferred-render pipeline.

### 4b.1. Automatic emission

When `MdCodec` parses a document whose TOML front-matter contains
`schema = "noet.mapping"`, it sets `has_deferred_render = true` unconditionally — the
same flag that `{requirements_table}` sets when its directive is encountered. No
`{mapping_table}` directive needs to be written by the author.

The table is appended at the end of the document's rendered HTML body (after all other
content). If the author prefers a specific placement, they may write
`` ````{mapping_table} `` in the Markdown body to control position; the codec will detect
it and skip the auto-append.

### 4b.2. `DirectiveDef` entry

Add to `DIRECTIVES` in `src/codec/myst.rs`:

```rust
DirectiveDef {
    name: "mapping_table",
    marker: "<!-- noet-mapping-table -->",
    sentinel: "<!--@@noet-mapping-table@@-->",
    directive: "",          // not written programmatically; auto-injected
    is_block_opener: false,
    queries: &[mapping_table_query],
    builder: Some(build_mapping_table_html),
},
```

### 4b.3. Query refiner: `mapping_table_query`

```rust
/// Refiner for `mapping_table`: fetch all edges where `WEIGHT_OWNED_BY` matches
/// the mapping node's bref.
///
/// Uses `RelationPred::NodeIn([mapping_bid])` to fetch all edges incident to the
/// mapping node — then the builder filters to those actually owned by it (i.e. where
/// `owned_by == mapping_bref`), discarding the Section parent edge.
fn mapping_table_query(ctx: &BeliefContext, _graphs: &[BeliefGraph]) -> Expression {
    Expression::RelationIn(RelationPred::NodeIn(vec![ctx.node.bid]))
}
```

This is a single-step pipeline (`queries: &[mapping_table_query]`). The resulting
`graphs[0]` contains every edge incident to the mapping node. The builder then filters
to owned edges only.

### 4b.4. Builder: `build_mapping_table_html`

```
fn build_mapping_table_html(ctx, graphs) -> Result<String, BuildonomyError>
```

**Algorithm:**

1. Build a unified `BeliefBase` from `ctx.beliefbase()` merged with `graphs[0]`.
2. Collect edges from `graphs[0].relations.as_graph().raw_edges()`.
3. Filter to edges where `weight.get::<String>(WEIGHT_OWNED_BY) == Some(mapping_bref)`.
4. For each qualifying edge `(source_bid, sink_bid, kind, weight)`:
   - Resolve `source_bid` → `(title, Option<html_url>)` using `bb`.
   - Resolve `sink_bid` → `(title, Option<html_url>)` using `bb`.
   - Collect extra payload fields (keys other than `WEIGHT_OWNED_BY`, `WEIGHT_SORT_KEY`,
     `WEIGHT_DOC_PATHS`) as supplementary columns, if any are present.
5. Sort rows by `WEIGHT_SORT_KEY` (the mapping declaration order).
6. Render an HTML table:

```html
<table class="noet-mapping-table">
  <thead>
    <tr>
      <th>Source</th>
      <th>Kind</th>
      <th>Sink</th>
      <!-- one <th> per extra payload key, if any -->
    </tr>
  </thead>
  <tbody>
    <tr>
      <td><a href="…">Source Title</a></td>
      <td>Pragmatic</td>
      <td><a href="…">Sink Title</a></td>
    </tr>
    …
  </tbody>
</table>
```

When `source_bid` or `sink_bid` cannot be resolved to a title, fall back to
`bid.bref().to_string()`. When no HTML URL is available (e.g. external or unrooted
node), render the title as plain text.

Empty mapping node (no owned edges yet): render
`<p><em>No mappings declared.</em></p>`.

### 4b.5. Auto-injection in `MdCodec`

In `MdCodec::generate_html` (or a new `finalize_html` hook), when
`self.is_mapping_schema` is true:

- If the rendered body already contains `marker("mapping_table")`, it was placed
  explicitly by the author — do nothing.
- Otherwise, append `marker("mapping_table")` to the body before `promote_markers`
  runs.

`promote_markers` converts the marker to the sentinel, and the deferred pipeline
replaces the sentinel with the rendered table at `generate_html_for_path` time.

This requires a new `is_mapping_schema: bool` field on `MdCodec`, set during `parse()`
when `document["schema"] == "noet.mapping"`.

### 4b.6. `should_defer` interaction

`MdCodec::should_defer()` already returns `self.has_deferred_render`. Setting
`has_deferred_render = true` when `is_mapping_schema = true` is sufficient to enqueue
the document for deferred HTML generation. No other changes to the `should_defer` path
are needed.

---

## 5. `WEIGHT_OWNED_BY` Extension

### 5.1. Current contract

```rust
pub const WEIGHT_OWNED_BY: &str = "owned_by";
// Values: "source" | "sink"  (default when absent: sink-owned)
```

Written by `push_relation` (builder.rs):

```rust
let owner = match direction {
    Direction::Incoming => "sink",
    Direction::Outgoing => "source",
};
weight.set(WEIGHT_OWNED_BY, owner).ok();
```

### 5.2. Extended contract

`WEIGHT_OWNED_BY` now accepts three value classes:

| Value | Meaning |
|-------|---------|
| `"source"` | Edge owned by the source node |
| `"sink"` | Edge owned by the sink node (default) |
| 8-char hex string | Edge owned by the node whose bref matches |

The bref form is written exclusively by `push_mapping`. All existing callers that
write `"source"` or `"sink"` are unchanged.

**Consumer impact** — every site that reads `WEIGHT_OWNED_BY` and branches on
`"source"` vs `"sink"` needs an `else` arm:

```rust
match weight.get::<String>(WEIGHT_OWNED_BY).as_deref() {
    Some("source") => { /* source owns */ }
    Some("sink") | None => { /* sink owns (default) */ }
    Some(bref_str) => { /* third-party owner identified by bref_str */ }
}
```

Known read sites (grep `WEIGHT_OWNED_BY`): `push_relation` (write), `metadata.js`
(display). Update both when implementing.

---

## 6. Builder Protocol

### 6.1. New method: `push_mapping`

Add to `impl GraphBuilder` in `src/codec/builder.rs`:

```rust
/// Resolve and register a third-party-owned edge declared by a mapping node.
///
/// Resolves `mapping.source` and `mapping.sink` independently via `cache_fetch`,
/// then emits a `RelationChange(source_bid, sink_bid, kind, weight)` event onto
/// `update_queue` with `WEIGHT_OWNED_BY` set to the owner's bref string.
///
/// Returns `(source_result, sink_result)`. Either side may be `Unresolved`; the
/// caller should push a `ParseDiagnostic::UnresolvedReference` for each.
async fn push_mapping<B: BeliefSource + Clone>(
    &mut self,
    mapping: &IntermediateMappingRelation,
    owner_bid: &Bid,
    index: usize,
    source: &str,
    global_bb: B,
    update_queue: &mut Vec<BeliefEvent>,
    missing_structure: &mut BeliefGraph,
) -> Result<(GetOrCreateResult, GetOrCreateResult), BuildonomyError>
```

**Internal steps:**

1. Regularize `mapping.source` key against repo root (same logic as `push_relation`).
2. Regularize `mapping.sink` key against repo root.
3. Call `cache_fetch(&[source_key], global_bb.clone(), true, missing_structure)` → `source_result`.
4. Call `cache_fetch(&[sink_key], global_bb.clone(), true, missing_structure)` → `sink_result`.
5. If either side is `Unresolved`, return early with both results (no edge emitted for a
   half-resolved pair).
6. Build `weight`:
   - Start from `mapping.weight.clone().unwrap_or_default()`
   - `weight.set(WEIGHT_SORT_KEY, index as u16)?`
   - `weight.set(WEIGHT_OWNED_BY, owner_bid.bref().to_string()).ok()`
7. Push `BeliefEvent::RelationChange(source_bid, sink_bid, mapping.kind, Some(weight), EventOrigin::Remote)`.
8. Return `(source_result, sink_result)`.

Both resolved nodes have `BeliefKind::Trace` inserted (same as `push_relation`'s
treatment of external nodes).

### 6.2. Phase 2b: mapping loop in `parse_content`

Insert after the existing downstream loop in Phase 2 of `parse_content`:

```rust
// Phase 2b: mapping relations (third-party-owned edges)
for (index, mapping) in proto.mappings.iter().enumerate() {
    let (source_result, sink_result) = self
        .push_mapping(
            mapping,
            bid,
            index,
            &content,
            global_bb.clone(),
            &mut relation_event_queue,
            &mut missing_structure,
        )
        .await?;

    for result in [source_result, sink_result] {
        match result {
            GetOrCreateResult::Resolved(node, source) => {
                relation_seeds.insert(node.bid);
                if source.is_from_cache() {
                    inject_context = true;
                }
            }
            GetOrCreateResult::Unresolved(unresolved) => {
                diagnostics.push(ParseDiagnostic::UnresolvedReference(unresolved));
            }
        }
    }
}
```

The `missing_structure` flush and `relation_event_queue` drain that already follow the
downstream loop absorb the mapping relations without additional changes.

### 6.3. GC semantics: mapping node deletion

When a mapping node file is deleted, `terminate_stack` removes all edges *incident to*
the mapping node's BID (edges where source == mapping_bid or sink == mapping_bid).
However, the owned edge `source_bid → sink_bid` has `WEIGHT_OWNED_BY = mapping_bref`
and neither endpoint is the mapping node — it will **not** be cleaned up by the
standard mechanism.

**Required tombstone scan** — `terminate_stack` (or a dedicated helper
`remove_mapping_owned_edges`) must, for each BID in `removed_nodes` whose
`BeliefNode.schema == "noet.mapping"`, scan `session_bb.relations()` for edges whose
`Weight.get::<String>(WEIGHT_OWNED_BY)` matches the removed node's bref, and emit
`BeliefEvent::RelationRemoved(source, sink, EventOrigin::Remote)` for each found edge.

**Scope bound**: the scan is limited to `session_bb` (the current compilation session
graph), not the global BeliefBase. For large corpora this is O(session edges), which
is acceptable. Optimize with an inverse index if profiling shows it matters.

**Alternative (deferred)**: on every compile pass, a mapping node re-emits all its
owned edges. If the file is absent from the file tree, no parse fires and the edges
persist until the next full recompile explicitly detects the missing file. The tombstone
scan is preferable for interactive editing where files can be deleted mid-session.

---

## 7. Rendering Pipeline Summary

```
[mapping.md] (schema = "noet.mapping")
        │
        ▼  MdCodec::parse()
   sets is_mapping_schema = true
   sets has_deferred_render = true
   populates IRNode.mappings (Phase 2b → RelationChange events)
        │
        ▼  MdCodec::generate_html()
   render_html_body() produces HTML from Markdown events
   if no mapping_table marker in body:
       append marker("mapping_table")  →  "<!-- noet-mapping-table -->"
   promote_markers():
       "<!-- noet-mapping-table -->" → "<!--@@noet-mapping-table@@-->"
        │
        ▼  compiler.rs: generate_html_for_path()
   mapping_table_query refiner runs → graphs[0] (all incident edges)
   build_mapping_table_html() filters to owned edges, renders table HTML
   splice_sentinels() replaces "<!--@@noet-mapping-table@@-->" with table
```

---

## 8. Viewer Integration

### 8.1. `WEIGHT_OWNED_BY` surfacing in `wasm.rs`

#### 8.1.1. New struct: `EdgeEntry`

In `src/wasm.rs`, add alongside `RelatedNode`:

```rust
/// A single entry in a NodeContext relation group (sources or sinks for one WeightKind).
///
/// Replaces raw `Bid` in `NodeContext.graph` to carry per-edge metadata.
#[cfg(feature = "wasm")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeEntry {
    /// BID of the source or sink node.
    pub bid: Bid,
    /// BID of the mapping node that owns this edge, if any.
    /// `None` when `WEIGHT_OWNED_BY` is `"source"`, `"sink"`, or absent.
    pub owner_bid: Option<Bid>,
}
```

#### 8.1.2. Updated `NodeContext.graph` type

Change `NodeContext.graph` from:

```rust
pub graph: HashMap<WeightKind, (Vec<Bid>, Vec<Bid>)>,
```

to:

```rust
/// Relations by weight kind: Map<WeightKind, (sources, sinks)>
/// Each EdgeEntry carries the endpoint BID and the optional third-party owner BID.
/// ⚠️ JavaScript: This is a Map object! Use `.get(weightKind)`, `.size`, `.entries()`
pub graph: HashMap<WeightKind, (Vec<EdgeEntry>, Vec<EdgeEntry>)>,
```

#### 8.1.3. `extract_node_context` changes

In the source and sink collection loops in `extract_node_context`, replace the current
`bid`-only collection with `EdgeEntry` construction:

```rust
// Resolve WEIGHT_OWNED_BY to a BID when it is a bref string
let owner_bid: Option<Bid> = edge_weight
    .and_then(|w| w.get::<String>(WEIGHT_OWNED_BY))
    .and_then(|owned_by| match owned_by.as_str() {
        "source" | "sink" => None,
        bref_str => {
            // Resolve bref → BID via BeliefBase::brefs() — O(log n) BTreeMap lookup.
            Bref::try_from(bref_str).ok().and_then(|bref| {
                inner.brefs().get(&bref).copied()
            })
        }
    });

let entry = EdgeEntry { bid: other_node.bid, owner_bid };
```

When `owner_bid` is `Some(mapping_bid)`, the mapping node must also be added to
`related_nodes` so the JS layer can render its title and path:

```rust
if let Some(mapping_bid) = owner_bid {
    if !related_nodes.contains_key(&mapping_bid) {
        if let Some(mapping_node) = inner.states().get(&mapping_bid).cloned() {
            // compute home_net and root_path for the mapping node
            related_nodes.insert(mapping_bid, RelatedNode { ... });
        }
    }
}
```

### 8.2. `metadata.js` changes

#### 8.2.1. `renderRelationGroup` signature

Change:

```javascript
function renderRelationGroup(bids, label, related_nodes)
```

to:

```javascript
function renderRelationGroup(entries, label, related_nodes)
```

where `entries` is an array of `EdgeEntry`-shaped objects `{ bid, owner_bid }`.

#### 8.2.2. Rendering owned edges

Inside the `for` loop, after rendering the primary relation link for `entry.bid`, check
`entry.owner_bid`:

```javascript
for (const entry of entries) {
  const bid = entry.bid;
  const ownerBid = entry.owner_bid ?? null;

  // ... existing logic to render the primary relation link for bid ...

  if (ownerBid) {
    const ownerNode = related_nodes.get(ownerBid);
    if (ownerNode && ownerNode.root_path) {
      const ownerTitle = escapeHtml(
        ownerNode.node.title || ownerNode.link_title || brefFromBid(ownerBid)
      );
      const ownerPath = ownerNode.root_path.startsWith("/")
        ? ownerNode.root_path
        : `/${ownerNode.root_path}`;
      html +=
        `<span class="noet-mapping-owner"> via ` +
        `<a href="${escapeHtml(ownerPath)}" class="noet-metadata-link" ` +
        `data-bid="${ownerBid}">${ownerTitle}</a></span>`;
    } else {
      html +=
        `<span class="noet-mapping-owner"> via ` +
        `<code>${escapeHtml(brefFromBid(ownerBid))}</code></span>`;
    }
  }
}
```

#### 8.2.3. Call-site updates

The two `renderRelationGroup(sources, ...)` and `renderRelationGroup(sinks, ...)` calls
in `renderNodeContext` receive the updated `graph` entries directly:

```javascript
// Before
for (const [weightKind, [sources, sinks]] of graph.entries()) {
  html += renderRelationGroup(sources, "Dependencies", related_nodes);
  html += renderRelationGroup(sinks, "Referenced by", related_nodes);
}

// After — no call-site change needed; renderRelationGroup now accepts EdgeEntry arrays
```

The change is backwards-compatible in the call sites: only the `renderRelationGroup`
body changes to destructure `{ bid, owner_bid }` instead of treating each element as a
raw BID string.

---

## 9. Data Flow Summary

```
[mapping.md]
  schema = "noet.mapping"
  [[mappings]]
    source = "X"
    sink   = "Y"
    weight_kind = "Pragmatic"
        │
        ▼
  belief_ir.rs: extract [[mappings]]
        │
        ▼
  IRNode.mappings = [IntermediateMappingRelation { source: Key(X), sink: Key(Y), kind: Pragmatic }]
        │
        ▼
  parse_content Phase 2b: push_mapping(mapping, owner_bid=mapping_BID)
        │   ├── cache_fetch(Key(X)) → source_BID
        │   ├── cache_fetch(Key(Y)) → sink_BID
        │   └── weight: { sort_key=0, owned_by="<mapping_bref>" }
        │
        ▼
  BeliefEvent::RelationChange(source_BID, sink_BID, Pragmatic, weight)
        │
        ▼
  session_bb / global_bb: edge stored with owned_by = mapping_bref
        │
        ▼
  BeliefBaseWasm::extract_node_context:
    EdgeEntry { bid: source_BID, owner_bid: Some(mapping_BID) }
        │
        ▼
  metadata.js renderRelationGroup:
    "X → Y  via [mapping.md]"
```

---

## 10. Testing Requirements

### 10.1. Unit tests (`belief_ir.rs`)

- Parse a TOML document with `schema = "noet.mapping"` and a single `[[mappings]]` entry.
  Assert `IRNode.mappings` has one element with correct `source`, `sink`, `kind`.
- Parse two `[[mappings]]` entries. Assert count = 2.
- Missing `source` field → `ParseDiagnostic::Warning`, entry skipped.
- Unknown `weight_kind` value → `ParseDiagnostic::Warning`, entry skipped.
- Extra payload fields (e.g. `notes`) → preserved in `weight.payload`.

### 10.2. Integration tests (`builder.rs`)

- A mapping node file with two `[[mappings]]` entries compiles to two
  `RelationChange` events with `WEIGHT_OWNED_BY = mapping_bref`.
- The owned edges (`source → sink`) are present in `session_bb` after `terminate_stack`.
- Deleting the mapping node (removing from `removed_nodes`) tombstones the owned edges.
- Unresolvable `source` or `sink` yields `ParseDiagnostic::UnresolvedReference` and no
  edge is emitted for that entry.
- A mapping node with zero `[[mappings]]` entries compiles cleanly (no edges, no errors).

### 10.3. Rendering tests (`myst.rs`)

- `build_mapping_table_html` with zero owned edges returns the empty-state paragraph.
- `build_mapping_table_html` with two owned edges renders a table with two rows,
  each containing linked source and sink titles.
- Edges in `graphs[0]` that are NOT owned by the mapping node (e.g. the Section edge
  to its parent network) are excluded from the table.
- Extra payload fields (`notes`, `confidence`) appear as additional columns when present
  on at least one edge.
- `MdCodec::generate_html` auto-appends the marker when `is_mapping_schema = true` and
  no explicit `{mapping_table}` directive is present in the body.
- `MdCodec::generate_html` does NOT double-append when the author wrote
  `{mapping_table}` explicitly.

### 10.4. Viewer tests (`metadata.js`)

- A node that is the target of a mapping-owned edge displays a "via [mapping node]" link
  in its Relations panel.
- Clicking the "via" link opens the mapping node's metadata.
- Edges owned by `"source"` or `"sink"` render without a "via" annotation.

---

## 11. Open Questions

1. **Extra payload columns**: when different `[[mappings]]` entries have different extra
   payload keys, `build_mapping_table_html` must decide whether to union all keys as
   columns (sparse table, empty cells) or only include keys present on every row.
   **Proposed default**: union of all keys present on any row, blank cell when absent.
   Confirm before implementing.

2. **Bref resolution in `extract_node_context`**: use `inner.brefs().get(&bref)` —
   `BeliefBase::brefs()` exposes `&BTreeMap<Bref, Bid>` directly, giving O(log n)
   lookup with no scan. Resolved; no further decision needed.

3. **Round-trip fidelity of `[[mappings]]`**: `toml_edit::DocumentMut` preserves
   array-of-tables ordering and unknown keys. Verify this holds for `[[mappings]]` with
   arbitrary extra payload fields before shipping `generate_source()`.

4. **GC scan granularity**: the tombstone scan described in §6.3 fires in
   `terminate_stack`. If multiple mapping nodes are deleted in one session, the scan
   cost is O(deleted_mapping_nodes × session_edges). Acceptable for typical use; add a
   note in the implementation issue to profile if needed.

---

## 12. File Map

| File | Change |
|------|--------|
| `src/codec/belief_ir.rs` | Add `IntermediateMappingRelation`; add `IRNode.mappings`; add extraction logic |
| `src/codec/schema_registry.rs` | Register `"noet.mapping"` with empty `graph_fields` |
| `src/codec/builder.rs` | Add `push_mapping`; add Phase 2b loop in `parse_content`; add GC scan in `terminate_stack` |
| `src/properties.rs` | Document extended `WEIGHT_OWNED_BY` contract in doc comment |
| `src/codec/myst.rs` | Add `mapping_table_query` refiner; add `build_mapping_table_html` builder; add `DirectiveDef` entry |
| `src/codec/md.rs` | Add `is_mapping_schema` field; set `has_deferred_render` when mapping schema detected; auto-append marker in `generate_html` |
| `src/wasm.rs` | Add `EdgeEntry`; update `NodeContext.graph` type; update `extract_node_context` |
| `assets/viewer/metadata.js` | Update `renderRelationGroup` to accept `EdgeEntry`; render "via" link |

---

## 13. References

- `docs/design/beliefbase_architecture.md` §2.2 — Identity and NodeKey resolution
- `docs/design/myst_directive_architecture.md` §6.1–6.2 — `{network_children}` and
  `{requirements_table}` deferred-render pipelines (direct precedents for §4b)
- `docs/design/myst_directive_architecture.md` §6.3 — `{implements}` block opener
  (precedent for parse-only relation modifiers)
- `docs/design/myst_directive_architecture.md` §8 — Extension point checklist
- `src/codec/schema_registry.rs` — `SchemaDefinition`, `GraphField`, `EdgeDirection`
- `src/codec/belief_ir.rs` — `IRNode`, `IntermediateRelation`
- `src/codec/builder.rs` — `push_relation`, `parse_content` Phase 2
- `src/codec/myst.rs` — `DirectiveDef`, `DIRECTIVES`, `build_requirements_table_html`
  (reference implementation for `build_mapping_table_html`)
- `src/codec/md.rs` — `MdCodec::parse`, `has_deferred_render`, `generate_html`
- `src/properties.rs` — `WEIGHT_OWNED_BY`, `Weight`, `WeightKind`
- `src/wasm.rs` — `NodeContext`, `RelatedNode`, `extract_node_context`
- `assets/viewer/metadata.js` — `renderRelationGroup`, `renderNodeContext`
