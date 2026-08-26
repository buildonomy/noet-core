# Issue 90: Data-Driven Schema Definitions and Categorical Fields

**Priority**: HIGH
**Estimated Effort**: 8 days
**Dependencies**: None (Issue 89 column expressions *consume* categorical edges but are not a prerequisite)

## Summary

Schema definitions today are Rust-only: registered in code via
`SCHEMAS.register()` with `&'static str` field names. This makes them
inaccessible to corpus authors who can't modify the Rust binary. This
issue introduces **corpus-level schema files** (`.schema.toml`) that are
parsed as first-class graph nodes, extends schemas with **categorical
field definitions** that auto-generate queryable category nodes, and
establishes a **`schema_namespace` const network** so all schema-derived
structure is discoverable from a single graph entry point.

## Goals

- Schema definitions are fully declarable in `.schema.toml` files within
  a corpus — no Rust code required for new schemas
- A three-tier registration model (built-in → app-shim → corpus file)
  with last-one-wins override semantics and provenance tracking
- Schema nodes are dual-sink: structural children of both their owning
  network and the `schema_namespace` const network
- Categorical fields on schemas auto-generate a tree of category nodes
  (schema → field → value) with pragmatic edges from content nodes
- Category values are discoverable via standard traversal queries

## Architecture

### Schema namespace const network

A new `schema_namespace()` const BID (following the `href_namespace`,
`asset_namespace` pattern). Seeded in `seed_session` with a Section edge
to `buildonomy_namespace()`. Added to `const_namespaces()`.

All schema nodes — whether from built-in registration, app-shim code,
or corpus `.schema.toml` files — get a Section edge to
`schema_namespace()`. This provides a single entry point for enumerating
all active schemas: `id://schema-namespace composed_of(*)`.

### Dual-sink schema nodes

Corpus-level schema files produce nodes with **two** structural parents:

1. **The owning network** — normal `ProtoIndex` child ownership (the file
   lives in that directory, subject to whitelist/blacklist filtering)
2. **`schema_namespace()`** — explicit Section edge emitted by the schema
   codec, like `process_asset` emits for `asset_namespace()`

Built-in and app-shim schemas have only `schema_namespace()` as parent
(no filesystem path, `BeliefKind::External`).

```
buildonomy_namespace
└── schema_namespace          (const network)
    ├── myapp.task            (corpus schema — also child of task-tracker network)
    │   ├── tags              (categorical field node)
    │   │   ├── alpha         (value node)
    │   │   ├── beta          (value node)
    │   │   └── gamma         (value node)
    │   └── priority          (categorical field node)
    │       ├── high          (value node)
    │       └── low           (value node)
    └── intention_lattice.intention  (built-in schema — schema_namespace parent only)
```

Content nodes get pragmatic edges to value nodes via `traverse_schema`:

```
task-001 ──pragmatic──▶ alpha
task-001 ──pragmatic──▶ gamma
task-002 ──pragmatic──▶ beta
```

### Three-tier registration model

| Tier | Mechanism | When | Priority |
|------|-----------|------|----------|
| Built-in | `SchemaRegistry::create()` | `Lazy` init | Lowest |
| App-shim | Rust `SCHEMAS.register()` | Before `DocumentCompiler::new` | Middle |
| Corpus | `.schema.toml` file parsed in Phase 1 | During parse | Highest (overrides) |

Last-one-wins on the same schema name key. Each registration records
its `SchemaSource` for diagnostics:

```rust
pub enum SchemaSource {
    BuiltIn,
    AppShim(String),
    CorpusFile(PathBuf),
}
```

Override emits `tracing::info!` with old and new source, not silent
`debug!`. No merging — full replacement. If a corpus author wants to
extend an app-shim schema, they copy the base definition and add to it.

### `&'static str` → `String` migration

`GraphField::field_name`, `GraphField::payload_fields`, and the new
`CategoricalField` fields must be `String` (or `Arc<str>`) to support
deserialization from files. Built-in schemas use `.to_string()` at
registration. `traverse_schema` already calls `.as_str()` on these
values, so the functional change is minimal.

### Schema file format

`.schema.toml` files are discovered by a `SchemaWalkCodec` and parsed
by a `SchemaCodec`. Format:

```toml
name = "myapp.task"
title = "Task Schema"

[[graph_fields]]
field_name = "blocking_tasks"
direction = "downstream"
weight_kind = "pragmatic"
required = false
payload_fields = ["priority", "reason"]

[[categorical_fields]]
field_name = "tags"
separator = ", "
direction = "upstream"
weight_kind = "pragmatic"

[[categorical_fields]]
field_name = "priority"
separator = ", "
direction = "upstream"
weight_kind = "pragmatic"
namespace = "shared.priority"  # optional: share category nodes across schemas
```

### Parse ordering constraint

`traverse_schema` runs during Phase 1 `MdCodec::parse`. Schema files
must be registered before content files that reference them.
`NetworkCodec::parse` already controls claim order — schema files
(identified by extension) are claimed and parsed before `.md` content
files within the same network. This is a localized change to the
existing claim loop in `NetworkCodec::parse`.

### Categorical field definition

```rust
pub struct CategoricalField {
    pub field_name: String,
    pub separator: String,
    pub direction: EdgeDirection,
    pub weight_kind: WeightKind,
    /// Optional override: share category nodes across schemas.
    /// Default: schema-prefixed (e.g. "myapp.task.high").
    pub namespace: Option<String>,
}
```

### Parse-time categorical emission

When `traverse_schema` processes a node whose schema has categorical fields:

1. Read the payload field value (e.g., `"Alpha, Beta, Gamma"`)
2. Split by separator → `["Alpha", "Beta", "Gamma"]`
3. For each value, emit a relation to the value node under
   `schema_namespace`: `NodeKey::Id { net: schema_namespace().bref(),
   id: "{schema}.{field}.{to_anchor(value)}" }`
4. Track observed values in a memo on the schema registry (thread-safe
   `HashMap<String, HashSet<String>>` keyed by `"{schema}.{field}"`)

### Deferred node emission

After the main parse loop (same pattern as `process_asset`):

1. Iterate observed categorical values across all schemas
2. For each schema with categorical fields, emit:
   - Schema node (if corpus-level, already emitted by `SchemaCodec`;
     if built-in/app-shim, emit as `BeliefKind::External`)
   - Categorical field nodes as Section children of the schema node
   - Value nodes as Section children of the field node
3. All category structure nodes get Section edges to their parent in
   the schema tree AND the schema root gets its Section edge to
   `schema_namespace()`

### Uniqueness across schemas

Schema-prefixed by default: `"{schema}.{field}.{to_anchor(value)}"`.
The optional `namespace` field on `CategoricalField` allows sharing:
when set, the value node ID uses `"{namespace}.{to_anchor(value)}"`
instead, so multiple schemas can map to the same category nodes.

## Implementation Steps

1. `&'static str` → `String` migration (0.5 days)
   - [ ] Change `GraphField::field_name` and `payload_fields` to `String`
   - [ ] Update built-in schema registrations to use `.to_string()`
   - [ ] Update `traverse_schema` (should be no-op — already uses `.as_str()`)
   - [ ] Add `#[derive(Deserialize)]` to `GraphField`, `EdgeDirection`,
         `SchemaDefinition`

2. Schema namespace const and seeding (0.5 days)
   - [ ] Add `UUID_NAMESPACE_SCHEMA` const and `schema_namespace()` fn
   - [ ] Add to `const_namespaces()` array
   - [ ] Seed `schema_namespace` network in `epoch_session_snapshot`
   - [ ] Add `BeliefNode::schema_network()` constructor

3. `SchemaSource` provenance tracking (0.5 days)
   - [ ] Add `SchemaSource` enum and `SchemaEntry` wrapper struct
   - [ ] Update `SchemaRegistry` to store `SchemaEntry` instead of
         `SchemaDefinition`
   - [ ] Upgrade override log from `debug!` to `info!` with source context

4. `SchemaWalkCodec` and `SchemaCodec` (1.5 days)
   - [ ] Implement `SchemaWalkCodec` tracking `.schema.toml` files
   - [ ] Register in `WALK_CODECS`
   - [ ] Implement `SchemaCodec` that deserializes `.schema.toml`,
         calls `SCHEMAS.register()`, and emits dual-sink Section edges
   - [ ] Register in `CODECS`
   - [ ] Ensure `NetworkCodec::parse` claims schema files before `.md` files

5. Categorical field definition (0.5 days)
   - [ ] Add `CategoricalField` struct with `Deserialize`
   - [ ] Add `categorical_fields: Vec<CategoricalField>` to `SchemaDefinition`
   - [ ] Preserve backward compat (default empty vec)

6. Parse-time categorical edge emission (1 day)
   - [ ] Extend `traverse_schema` to process categorical fields
   - [ ] Split field values, emit relations to category value node IDs
   - [ ] Track observed values in thread-safe memo on registry

7. Deferred category node emission (1.5 days)
   - [ ] Post-parse step to emit schema/field/value node tree
   - [ ] Schema node dual-sink edges (network parent + schema_namespace)
   - [ ] Built-in/app-shim schema nodes emitted as `BeliefKind::External`
   - [ ] Wire into compiler's deferred processing pipeline

8. Marginalia and HTML (1 day)
   - [ ] Generate marginalia HTML for categorical variables on schema
         node pages
   - [ ] Render observed values with counts on schema node HTML
   - [ ] Wire into `generate_deferred_html` pipeline

## Testing Requirements

- `.schema.toml` file produces a schema node with dual Section parents
- Schema override: corpus file replaces app-shim registration, logs info
- `GraphField` with `String` fields round-trips through TOML deserialization
- Categorical field splits correctly on separator, produces expected edges
- Category value nodes created with correct IDs in schema namespace tree
- `id://schema-namespace composed_of(*)` returns all schema nodes
- Traversal `id:myapp.task.tags.alpha used_by(1)` finds tagged content
- Two schemas with same-name field but different namespaces produce
  distinct value nodes
- Two schemas sharing a namespace via `namespace` override produce
  shared value nodes
- Parse ordering: schema file parsed before content file in same network
- Built-in schema with no corpus override produces `External` node under
  `schema_namespace` only

## Success Criteria

- [ ] Corpus authors can define schemas via `.schema.toml` without Rust code
- [ ] Three-tier registration with provenance tracking works end-to-end
- [ ] Schema nodes are discoverable from `schema_namespace` entry point
- [ ] Categorical payload fields auto-generate queryable graph structure
- [ ] Category values traversable via standard query grammar
- [ ] Column expressions (Issue 89) can consume categorical edges for
      topological columns
- [ ] No manual edge authoring required for categorical relationships

## Risks

- Parse ordering between schema files and content files within a network
  is implicit (extension-based priority in `NetworkCodec::parse`) →
  **Mitigation**: document the ordering contract; add a diagnostic if
  `traverse_schema` encounters an unregistered schema name that matches
  a `.schema.toml` file in the same network (suggests ordering bug)
- High-cardinality categorical fields (hundreds of unique values) →
  **Mitigation**: log warning when a field exceeds a configurable
  threshold; consider not creating nodes for values seen only once
- Separator ambiguity (value contains the separator) →
  **Mitigation**: document separator semantics; allow regex separator

## Open Questions

- Should the memo of observed values be persisted across incremental
  parses, or rebuilt each time? Incremental parse would need to handle
  category nodes whose source values were removed.
- Should `.schema.toml` files support a `key_field` override for
  `GraphField` (currently hardcoded to `parent_id` in `traverse_schema`)?
  This would make the schema file fully self-describing for graph field
  extraction logic.
