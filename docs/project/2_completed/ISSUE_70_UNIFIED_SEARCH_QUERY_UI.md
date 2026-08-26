# Issue 70: Unified Search, Query, and Graph Visualization UI

**Priority**: MEDIUM
**Estimated Effort**: 5 days (RELATIVE COMPARISON ONLY)
**Dependencies**: Issue 63 (Traceability View — complete), Issue 54 (Full-Text Search MVP — complete)
**Supersedes**: Issue 41 (Query Builder UI), Issue 49 (Full-Text Search Production backlog), Issue 42 (Graph Visualization)
**Status**: CLOSED — Overtaken by Events

## OBE Summary

This issue defined the unified three-axis query model (Subject / Projection /
Instrument) and planned a 7-phase implementation spanning the viewer UI, query
parser, graph visualization, and compile-time directives. The conceptual model
proved sound and was formalized in `docs/design/query_model.md`.

The scope has been decomposed into focused issues:

- **Issue 79** (Query Infrastructure) — `QuerySpec` types, `Score` primitive,
  evaluation bridge to `Expression`/`eval_query`. Covers the backend that this
  issue's Phases 1–5 all depended on.
- **Issue 80** (Query Parser) — Recursive descent parser for the §9.5 textual
  grammar. Extracts Phase 4b from this issue.
- **Issue 81** (`{query}` Directive) — Compile-time query rendering in MyST
  documents. New scope not originally in this issue, but motivated by the same
  infrastructure.

### Viewer UI work

The following viewer-side work is tracked in **Issue 82** (Viewer Query UI
Enhancements):

- **Phase 1**: Search mode in traceability panel — `[Submap | Search]` toggle,
  `bb.search()` integration, network-scope dropdown, debounced input, TfIdfScore
  column
- **Phase 2**: Graph render mode — D3.js lazy-load, `[Table | Graph]` toggle,
  force-directed layout, SVG/PNG export, 500-node cap
- **Phase 3**: Nav-tree search "Explore" affordance — event bridge between
  `search.js` and `traceability.js`
- **Phase 4**: Field-scoped search and boolean operators in `query_search_index`
  (`src/shard/search.rs`)
- **Phase 5**: Filter by Relation Membership and Complement View
- **Shard halo fix**: Cross-network MapsTo edges missing from shard halo
  (`src/shard/export.rs`)
- **Phase 6**: Revise `query_model.md` post-implementation
- **Phase 7**: Query & Graph Invariants Cheatsheet

### Key design artifacts preserved

- `docs/design/query_model.md` — full formal model (three-component QuerySpec,
  Score algebra, textual grammar §9.5, surface bindings)
- `docs/design/dag_model.md` §4 — the video camera conceptual model
- `docs/design/myst_directive_architecture.md` — directive pipeline that
  Issue 81 extends

## Original Goals (for reference)

1. Text search control in the traceability panel
2. Corpus-wide mode (camera at SPA root)
3. Nav-tree search results native to traceability panel
4. Graph render mode (D3.js, lazy-loaded)
5. Field-scoped search (`title:`, `schema:`, `kind:` prefixes)
6. Boolean operators (`AND`, `NOT`) in `query_search_index`

## References

- Issue 79 — Query Infrastructure (QuerySpec, Score, Evaluator + viewer validation)
- Issue 80 — Query Parser (textual grammar + `?q=` URL integration)
- Issue 81 — `{query}` Directive (compile-time rendering)
- Issue 82 — Viewer Query UI Enhancements (search, graph, Explore, Share/Embed)
- `docs/design/query_model.md` — the formal model this issue originated
- Issue 63 — Traceability View (complete, prerequisite)
- Issue 54 — Full-Text Search MVP (complete, prerequisite)
- Issue 41, 42, 49 — superseded by this issue
