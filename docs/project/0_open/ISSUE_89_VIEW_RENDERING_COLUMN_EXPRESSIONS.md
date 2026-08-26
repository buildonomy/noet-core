# Issue 89: View Rendering and Column Expressions

**Priority**: HIGH
**Estimated Effort**: 5 days
**Dependencies**: Extends `{query}` directive and ViewRenderer architecture.
Issue 90 (categorical fields) provides graph-native category nodes that
Layer 2 topological columns consume — without Issue 90, topological
columns require ad-hoc comma-splitting in column expressions.

## Summary

Three surfaces consume query results — the browser viewer, the MCP `query`
tool, and the `{query}` MyST directive — but only the browser has view
rendering. The MCP tool returns a raw graph blob; the directive has
compile-time column support but no runtime equivalent. This issue wires
the ViewRenderer infrastructure through all surfaces, then extends it
with per-column traversal expressions for bespoke tabular reports.

## Goals

- MCP `query` returns view-rendered tables (headers + rows), not graph blobs
- Agents can select a view key (`depth0`, `connectivity`, `maps_to`, etc.)
- Tape summary metadata included so agents can see query structure
- Column expression grammar for bespoke reports (per-row traversals and
  payload extractions as additional pipeline steps)
- Node-centric and edge-centric result modes both supported

## Architecture

### Layer 1: View-rendered table output (immediate)

Wire existing `ViewRenderer` infrastructure through the MCP `query` tool.

**Input:**
```json
{
  "query_string": "id:network-a composed_of(*) NOT id:network-b composed_of(*)",
  "view": "depth0",
  "sort": "section_order"
}
```

**Output:**
```json
{
  "display": "Node Intrinsics",
  "headers": ["Title", "Schema", "Kind"],
  "rows": [
    { "bid": "...", "cells": ["Widget Design", null, "Document"] }
  ],
  "row_count": 15,
  "tape_summary": [
    { "label": "0.L", "type": "Nodes", "count": 52 },
    { "label": "0.R", "type": "Nodes", "count": 80 },
    { "label": "0", "type": "Compose", "op": "Not", "count": 15 }
  ],
  "query_canonical": "id:network-a composed_of(*) NOT id:network-b composed_of(*)"
}
```

The `tape_summary` gives agents visibility into query structure (labels,
types, counts) without flooding them with BIDs. The view renders the final
result entry by default.

**Node-centric vs edge-centric results.** Not all queries produce node sets.
Ownership/traceability queries (`covers(1)`, `maps_to`) produce edge sets —
the row is an edge or traceability claim, not a node. The view selection
(`depth0` vs `maps_to`) determines whether rows represent nodes or edges.
The output schema (`{ headers, rows }`) is the same — only row semantics
differ.

### Layer 2: Column expressions via traversal (medium term)

Each column definition is a traversal or payload extraction evaluated
per row. Column expressions become additional labeled steps in the same
QuerySpec pipeline — one evaluation produces a tape with all data. The
view renderer reads specific labeled entries to populate each column.

**Column expression types:**
- Payload field path: `payload.status` → extract value
- Traversal: `uses(1)` → list of neighbor titles
- Traversal + filter: `uses(1) kind == "category-tag"` → filtered list
- Categorical membership: `uses(1) id:category-x` → boolean per category
- Edge count: `k-pragmatic-s(1) count` → integer

**MCP JSON form:**
```json
{
  "query_string": "id://sample-network composed_of(*)",
  "view": "columns",
  "columns": [
    { "header": "Title", "field": "title" },
    { "header": "Priority", "field": "payload.priority" },
    { "header": "Dependencies", "traversal": "uses(1)", "filter": "kind == widget" },
    { "header": "Reviewers", "traversal": "covers(1)", "filter": "kind == review" }
  ]
}
```

**Evaluation strategy:** Column expressions are compiled into additional
pipeline steps in the same QuerySpec, not N×M independent traversals. The
single evaluation produces a tape; the view renderer reads labeled entries
to fill columns. This is the same pattern the `{query}` directive already
uses for compile-time rendered HTML tables.

**Topological columns:** The column set itself can come from a traversal
when the categories form a closed set. Column headers are node titles from
the category traversal; each cell is a boolean or value indicating whether
the row node participates in that category. This is the `{query}`
directive's "category columns" pattern.

**Integration with Issue 90 categorical fields:** When a schema declares
categorical fields (Issue 90), `traverse_schema` auto-generates pragmatic
edges from content nodes to category value nodes under `schema_namespace`.
Topological columns can consume these directly — the value nodes form a
closed set discoverable via `id://schema-namespace composed_of(*)`
traversal, and each row's membership is a standard edge check. This
eliminates the need for column expressions to parse comma-separated
payload text at query time.

### Layer 3: Multi-join pivot reports (longer term)

Some analyses require traversing multiple paths from different starting
points, then pivoting the results by a grouping key. For example:
- Starting from items in a hierarchy, traverse to their categories
- From the same items, traverse to their review coverage
- Pivot by category to show coverage gaps per category

This requires either:
- **A.** Composition + column expressions (Layer 2 extended) — the query
  gives the row set, column expressions do per-row lookups, the agent or
  a dedicated view does the pivot.
- **B.** Report templates — a named definition specifying row query,
  column definitions, grouping key, sort order, and filters. Stored as
  TOML/YAML in the corpus, executable by both viewer and MCP.

Approach A is more general; Approach B is more ergonomic for recurring
reports. Defer decision until Layer 2 usage patterns stabilize.

## Implementation Steps

1. Layer 1: MCP view rendering (2 days)
   - [ ] Add `view`, `sort` to `QueryInput` (`max_rows` deferred to BACKLOG pagination)
   - [ ] Build tape summary serialization (label, type, count per entry)
   - [ ] Wire `ViewRenderer::render()` through `query` tool handler
   - [ ] Update `QueryOutput` to table structure (headers, rows, tape_summary)
   - [ ] Verify all existing view keys work (`depth0`, `connectivity`, `maps_to`)
   - [ ] Update `orientation.md` with view parameter documentation

2. Layer 2: Column expressions (3 days)
   - [ ] Design column expression grammar (field path, traversal, filter)
   - [ ] Compile column expressions into additional QuerySpec pipeline steps
   - [ ] Implement column-aware ViewRenderer that reads labeled tape entries
   - [ ] Support topological columns (category set from a traversal)
   - [ ] Wire through MCP `query` tool and `{query}` directive

## Testing Requirements

- Layer 1: MCP `query` with `view: "depth0"` returns correct row count
  (not full graph node count) for composition queries
- Layer 1: Edge-centric view (`maps_to`) returns edge rows
- Layer 1: `tape_summary` accurately reflects tape entry structure
- Layer 2: Column expression with `component_of(2) payload.field` correctly
  traverses 2 hops and extracts payload

## Success Criteria

- [ ] MCP `query` returns table with row count matching browser viewer
- [ ] Agents can select view key and get formatted tabular output
- [ ] Column expressions enable bespoke reports without new Rust code
- [ ] Same query + view produces same result via MCP and `{query}` directive

## Risks

- ViewRenderers may not all work in native Rust context (some may assume
  WASM/browser environment) → **Mitigation**: verify each renderer works
  with `BeliefBase` as backing store before wiring through MCP
- Column expression compilation into pipeline steps may conflict with
  tape label semantics → **Mitigation**: use distinct label namespace
  for column steps (e.g., `col:0`, `col:1`)
- Topological columns from categorical fields (Issue 90) depend on
  schema registration happening before query evaluation →
  **Mitigation**: Issue 90 guarantees schema files parse in Phase 1
  before content; category value nodes exist by query time

## Open Questions

- Should the current graph-blob output be preserved as `view: "graph"` for
  backward compatibility?
- Should tape entry BID lists be includable on demand (e.g., a
  `include_bids: true` flag), or always omitted from tape_summary?
- Should `network` filter parameter restrict query scope to a single
  network's submap?
