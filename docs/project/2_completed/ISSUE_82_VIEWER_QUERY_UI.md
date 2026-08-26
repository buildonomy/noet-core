# Issue 82: Viewer Query UI Enhancements

**Version**: 0.1
**Priority**: MEDIUM
**Estimated Effort**: 13.5 days (RELATIVE COMPARISON ONLY)
**Dependencies**: Requires Issue 79 (QuerySpec types), Issue 80 (query
parser + `?q=` URL integration), Issue 81 (`{query}` directive +
`LinkResolver` infrastructure), and Issue 83 (BeliefSource refactoring).
All dependencies are complete.

## Summary

The traceability panel is a visual `QuerySpec` editor. Every panel state
(subject, projection steps, view configuration) maps to a `QuerySpec` that
serializes to `?q=` for sharing and round-trips through `parseQuery` /
`serializeQuery`. Graph visualization is tracked separately in Issue 85.

Steps 1–6 (complete) established the infrastructure: shard halo fix, unified
query evaluation via `bb.query()` with `TextMatch` support, field-scoped
search, Share/Embed, Explore affordance. Step 7 restructures the control
panel into a proper query builder with clear separation between query
construction and view configuration.

## Goals

- The traceability panel control area is a composable form builder that
  maps to the full `QuerySpec` data model (`spec.rs`): subject selection,
  projection step chain (filter, traverse, compose), and composition
  operators (AND/OR/NOT)
- The text query input synchronizes bidirectionally with the form builder:
  editing the text updates the form; editing the form updates the text
- Clear visual separation between **query construction** (subject, steps)
  and **view configuration** (WeightKind filters, display mode, export)
- `maps_to` mode is expressed as a query-level concern in the `?q=` string,
  not as a side-channel toggle (deferred to `ISSUE_87_RAW_TAPE_VIEW.md`)
- All panel states round-trip through `?q=` URLs
- Fix the cross-network MapsTo shard halo gap (done)

## Architecture

The control panel has two distinct regions:

**Query region** (top): Constructs the `QuerySpec`. Contains:
- **Text input**: Shows the current query grammar string. Editable.
  Changes are debounced, parsed via `BeliefBaseWasm.parseQuery()`, and
  evaluated via `bb.query(spec)`. Parse errors shown inline.
- **Subject selector**: Radio/dropdown for `CorpusWide` vs `Bids` (anchored
  to a specific node). Anchored mode shows a node picker or uses the
  current document's BID.
- **Projection step chain**: Visual cards for each `ProjectionStep`.
  Each card shows the step type (Filter / Traverse / Compose) and its
  parameters. Add/remove buttons. Drag to reorder.
- **Composition wiring**: When multiple pipelines exist, AND/OR/NOT
  selector between branches.

**View region** (bottom): Configures how results are displayed. Contains:
- **WeightKind checkboxes**: Section / Epistemic / Pragmatic visibility
- **Display mode**: Table columns, maps_to, depth0 list
- **Export**: CSV, XLSX, Share URL, Embed directive
- **Network filter**: Dropdown to filter results by home network

**maps_to**: Has both a query and a view component:
- **Query** (`?q=`): anchored subject + traversal step that walks owned
  edges from the submap. Replaces the current `mapsToMode` boolean.
- **View** (`&view=maps_to` or equivalent): renders the tape’s N-2
  element (owner BIDs before the final traversal) as the first column,
  producing the owner → sink → source three-level rowspan table.
  The view reads tape structure to determine column grouping — this is
  analogous to `display=edge_count` vs `display=depth0` in the existing
  `ViewRenderer` trait.

**Shard halo fix** (complete): Owner nodes embedded in shard halo for
cross-network MapsTo edge resolution.

## Implementation Steps

1. Shard halo fix (0.5 day)
   - [x] In `src/shard/export.rs` `export_sharded`: scan each shard's edges
         for `WEIGHT_OWNED_BY` bref values, resolve to BIDs, embed owner nodes
         in the shard halo alongside existing `referenced_extern_states`
   - [x] Test: `test_shard_halo_includes_owned_by_owner_nodes` — verified the
         test fails without the fix and passes with it

2. Search mode in traceability panel (2 days)
   - [x] Add `[Submap | Search]` radio toggle to controls bar
   - [x] In Search mode: call `bb.search(query, 200)`, map results to row
         shape, call `get_context_bulk()`, render table via `refreshSearchData()`
   - [x] Add TfIdfScore column (visible in Search mode only)
   - [x] Add network-scope dropdown (filter results client-side by
         `network_bref`); populated from nav tree roots
   - [x] Hide depth/maps_to/kind controls in Search mode; show in Submap mode
         via `syncModeControls()`
   - [x] Debounced input handler (300 ms) triggers `refreshSearchData()`
   - [x] Score column included in CSV/XLSX export in Search mode
   - [x] `openTraceabilitySearch(query)` public API for opening panel in
         search mode with pre-populated query

3. Nav-tree Explore affordance (0.5 day)
   - [x] Add Explore button (🔍) to `search.js::_renderResultItem`
   - [x] Click handler in `_attachGlobalListeners` intercepts `.noet-search-result__explore`
         before the general result-navigation handler
   - [x] `_openExplore(bid, networkBref)`: resolves network bref → BID via
         nav tree roots, loads shard if needed, calls `callbacks.openTraceabilityModal`
         in Submap mode anchored to the result node
   - [x] Explore button appears on hover/selection; does not interfere with
         keyboard navigation (uses `tabindex="-1"`)

4. Share, Embed, and query metadata integration (1 day)
   - [x] `buildQueryText()`: serialize current panel state to a query string
         (search mode → raw query text, submap mode → `id://` anchor)
   - [x] Share button: copy full URL with `?q=` param to clipboard, toast
   - [x] Embed button: copy `{query}` directive fence to clipboard, toast
   - [x] Both visible whenever traceability panel is open
   - [x] Toast notification system (`showToast`) with fade animation
   - [x] Compile-time: preserve raw query text in `_query_texts` metadata
         array (`QueryBlockData.query_text` in `md.rs`); emit
         `<div class="noet-query-meta" data-query="..." data-count="N" hidden>`
         from `generate_html_for_path` in `compiler.rs`
   - [x] "Open in Search" button: `attachQuerySearchButtons()` in `content.js`
         finds `.noet-query-meta` divs, creates button in top-right of
         `.noet-query-result`, calls `callbacks.openTraceabilitySearch(query)`
   - [x] CSS: `max-height: 60vh` + `overflow-y: auto` on `.noet-query-result`
         so long query results scroll within the block instead of truncating

5. WASM TextMatch integration and unified query execution (2 days)
   - [x] `TextSearchProvider` trait in `query/spec.rs`: defines the callback
         for evaluating `TextMatch` filters; returns `Vec<(Bid, f64)>`
   - [x] `BeliefBase::apply_filter()`: accepts `Option<&dyn TextSearchProvider>`;
         `TextMatch` delegates to provider when present, errors when absent
   - [x] `evaluate_query_with_search()`: new method on `BeliefBase` that
         threads the provider through the full evaluation pipeline
         (`apply_projection_steps_to_package` → `apply_filter` → `apply_composition`)
   - [x] `WasmTextSearchProvider` in `wasm.rs`: bridges loaded `search_indices`
         to `TextSearchProvider` trait via `query_search_index()`
   - [x] `BeliefBaseWasm::query()` now constructs `WasmTextSearchProvider` and
         calls `evaluate_query_with_search()` instead of `evaluate_query()`
   - [x] Bug fixes: kind filters visible in both modes; score column removed;
         keyboard nav column indices consistent
   - [x] `refreshFromQuery(queryText)` in `traceability.js`: unified query
         path that parses query grammar via `BeliefBaseWasm.parseQuery()`,
         evaluates via `bb.query(spec)`, maps results to rows + context
   - [x] `searchQueryToGrammar(raw)`: wraps bare user text as `text:"..."`;
         passes through query grammar strings (detects `id://`, `field:`,
         operators, traversal steps)
   - [x] Search mode now evaluates `TextMatch` via full `QuerySpec` pipeline
         instead of calling `bb.search()` directly
   - [x] `BeliefBaseWasm.parseQuery()` and `.serializeQuery()` WASM bindings
         exposed for query grammar ↔ QuerySpec JSON round-trip
   - [x] `?q=` URL round-trip: `viewer.js` reads `?q=` on page load,
         opens traceability panel in search mode, query text is parsed
         and evaluated as a `QuerySpec`

6. Field-scoped search and boolean operators (1.5 days)
   - [x] Inverted index keys changed from `term` to `field:term` format
         (e.g. `"title:thruster"`, `"text:thruster"`, `"*:thruster"`);
         `*:` catch-all prefix for unscoped queries
   - [x] `IndexedDoc`: added `schema` and `kind` fields
   - [x] `index_node`: indexes schema (as identifier, no stemming) and kind
         labels alongside title/text/id; uses `add_term` helper for
         field-scoped + catch-all dual indexing
   - [x] `parse_query_terms`: parses `field:term`, `AND`, `NOT` from query;
         schema/kind fields skip tokenization/stemming (identifiers)
   - [x] `query_search_index`: OR terms score normally; AND terms act as
         post-filters (require presence without scoring); NOT terms exclude
   - [x] `fuzzy_expand`: compares bare term portion after field prefix
   - [x] `search.js::tokenize`: strips query syntax (AND/OR/NOT, field:,
         id://, traversal syntax) before highlighting
   - [x] Unit tests: field-scoped title search, boolean AND, boolean NOT,
         unknown field prefix fallback, schema/kind indexing (5 new tests)

7. Visual query builder and panel restructure (3 days)
   - [x] Restructure `renderSkeleton` into two visual regions:
         **Query region** (subject + projection steps + text input) and
         **View region** (kind filters, display mode, export, network filter)
   - [x] View region: collapsible "View" section with toggle button,
         kind filters, network filter, export/share/embed buttons
   - [x] CSS: clear visual separation between query and view regions
         (border, background, section labels, step cards)
   - [x] Step cards: `renderStepCards(spec)` shows subject card + projection
         step cards with type labels (Search/Filter/Traverse/Compose) and
         parameter summaries. Rebuilds on every successful parse.
   - [x] Parse error display: `tryParseQuery` / `showParseError` /
         `clearParseError` show inline errors below the query input
   - [x] Anchored query display: `openTraceabilityModal` populates
         `searchQuery` with `id://{bref}` and renders step cards
   - [x] Removed maps_to toggle, depth spinner, submap-controls div;
         depth and maps_to are now expressed in the query grammar
         (e.g. `id://bref consists_of(2)`, `id://bref covers(1)`)
   - [x] maps_to as query: users type `id://bref covers(1)` to get
         maps_to results; evaluated via `refreshFromQuery`
   - [x] Editable step cards: ▲/▼ reorder, ✕ remove, "+ Step" add
         (adds default `consists_of(1)` traversal). All actions modify
         `currentSpec` and serialize back to text input via `syncSpecToText`.
   - [x] Form → text sync: `syncSpecToText()` serializes `currentSpec`
         via `BeliefBaseWasm.serializeQuery()`, updates text input, marks
         dirty. Does NOT auto-evaluate.
   - [x] Explicit execution: ▶ Run button and Enter key trigger
         `executeQuery()`. Text input changes only parse/preview step
         cards (debounced), not evaluate.
   - [x] Dirty state: `markDirty()` / `clearDirty()` toggle `.is-dirty`
         class on the Run button (full opacity when dirty, dimmed otherwise)
   - [x] Compose display: step cards show `AND`/`OR`/`NOT` with branch
         step counts. Compositions are entered via query grammar text.
   - [x] Subject selector: autocomplete multi-select backed by
         `bb.search()`, rendered inline in the step editor when a
         step's input TapeFn is `Keys`. Selected nodes shown as
         removable chips. Keyboard nav (↑/↓/Enter/Escape/Backspace).
   - [x] `syncSubjectFromSpec`: initializes chips from first seed
         step's TapeFn (Keys/Bids resolution)
   - [x] `onSubjectChanged`: chip add/remove updates step's
         `input.Keys` directly and calls `syncSpecToText`

8. Unify Subject and TapeFn — QuerySpec as Vec\<ProjectionStep\> (3 days)
   - [x] Collapse `QuerySpec { subject, projection }` into
         `QuerySpec { steps }`. The seed is a `TapeFn` variant on a
         step's input field, not a separate `Subject` type.
   - [x] Define seed `TapeFn` variants: `Bids(Vec<Bid>)`,
         `Keys(Vec<NodeKey>)`, `Corpus`, `DocumentNodes(Bref, String)`.
         Add `StepOperation::Identity` for seed-only queries.
   - [x] Remove `Subject` enum. Context-dependent queries use
         `TapeFn::Then(None)` on the first step; callers inject the
         concrete seed `TapeFn` before evaluation.
   - [x] Grammar: `KEYS(...)`, `BIDS(...)`, `CORPUS()` as explicit
         seed syntax. Bare single NodeKey is sugar for `KEYS(key)`.
         Multi-anchor: `KEYS(bref:abc,bref:def) consists_of(1)`.
   - [x] Update `query_parser.rs`: parse/serialize seed functions.
         Round-trip: `parse(serialize(spec)) == spec`. 9 new tests.
   - [x] Update evaluators (`base.rs`, `db.rs`): `eval_subject` →
         `eval_seed`, handles `TapeFn` variants. `Identity` operation
         is pass-through.
   - [x] Update `query_model.md` §3–4, §5, §8, §9.5.5–9.5.9, §10.2,
         §13. Also `dag_model.md` §4 and `architecture.md` NodeKey
         String Format.
   - [x] Update viewer JS: `spec.subject` → `spec.steps[0].input`,
         `spec.projection` → `spec.steps`. `rebuildQueryFromForm`
         emits `KEYS(bref:a,bref:b)` for multi-subject. Removed
         multi-subject override hack.
   - [x] WASM bindings: serde shape changes automatically.
   - [x] Backward compatible: bare anchors and `{query}` directives
         work unchanged.

9. Tape redesign and viewer consolidation (3 days)

   **Tape protocol** (see `query_model.md` §6.3):
   - [x] Remove `__seed` tape entry. The seed `TapeFn` is resolved by
         the evaluator and fed as *input* to the step; only the step's
         *output* goes into the tape. Identity steps produce
         `TapeContent::Nodes(seed_bids)`. Traversals produce per-hop
         `TapeContent::Edges` entries. No special entry 0.
   - [x] Remove `SEED_LABEL` constant and all `__seed` references in
         `evaluate_query_with_search`, `apply_projection_steps_to_package`,
         `PackageStage::stage()`, `bid_tape_indices`, `Tape::input_bids`.
   - [x] `materialize_graph` primary set = union of all user-step tape
         entries (everything before the graph context boundary). Graph
         context entries (halo/balance) are Trace-colored.

   **Result lens** (see `query_model.md` §7.1):
   - [x] Views specify a `TapeFn` as a "result lens" that extracts the
         display set from the tape. `Then(None)` = final frontier.
         `Fold(Union, None)` = full tree. Label ref = specific step.
         Implemented as `Tape::result_bids(lens, seed)` in `spec.rs`.
   - [x] WASM `query()` returns `{ graph, tape_indices }` where
         `tape_indices` uses `bid_tape_indices()` (no longer skips
         seed entry since seed entry is gone).
   - [x] `result_bids` test helper takes a `TapeFn` lens argument
         instead of always reading the last tape entry.

   **Viewer consolidation:**
   - [x] Remove `refreshData()` and the `get_submap` WASM code path.
         All panel data flows through `refreshFromQuery` using the
         unified query pipeline (`bb.query(spec)`).
   - [x] Remove `currentDepth` state variable — depth is part of the
         query grammar (`consists_of(N)`).
   - [x] Replace `brefFromBid` JS function (broken: computes
         `parent_bref` not `Bid::bref()`) with WASM-backed resolver.
         `setBrefResolver()` in `utils.js` injects WASM
         `get_bref_from_bid()` at initialization time.

   **Extracted to Issue 87 (RawTapeView and Covers Fix):**
   - `covers` shorthand fix (`o-pragmatic-k` → `o-pragmatic-sk`)
   - dag_model.md diagram correction (owner→edge, not owner→node)
   - `ViewOutput::Json` (replaces `ViewOutput::Rows`)
   - `RawTapeView` implementation (per-entry tape rendering)
   - `mapsToMode` removal from `traceability.js`

   **Remaining items extracted to Issue 87.**

## Testing Requirements

- Query input drives row set; edge columns render correctly;
  kind filter checkboxes visible in both modes
- Explore: opens traceability panel anchored to result BID with query input
  focused
- All existing traceability and search tests pass (no regression)
- Cross-network MapsTo edges resolve correctly after shard halo fix
- maps_to state round-trips through `?q=` URL
- Form controls and text input stay in sync bidirectionally

## Success Criteria

- [x] Shard halo fix: cross-network MapsTo edges render in WASM viewer
- [x] `TextMatch` filter steps evaluate correctly in WASM viewer
- [x] Field-scoped search and boolean operators work in `query_search_index`
- [x] Explore affordance opens panel anchored to selected result
- [x] Share copies URL; Embed copies `{query}` directive
- [x] Panel has clear query region / view region separation
- [x] Step cards with seed TapeFn editor, composition wiring present
- [x] Text input ↔ form controls sync bidirectionally
- [x] maps_to mode reflected in `?q=` string
- [x] All `?q=` URLs round-trip correctly through new control states
- [x] Multi-anchor subjects work in grammar: `KEYS(bref:a,bref:b) consists_of(1)`
- [x] `QuerySpec` is `{ steps: Vec<ProjectionStep> }` with seed TapeFn on first step
- [x] No regression in existing traceability, search, or export functionality
      (687 tests pass, 0 failures)

## Risks

- **`IndexedDoc` field additions increase index size**: → **Mitigation**: Measure
  on a real corpus before landing; use short enum codes if significant.
- **Bidirectional sync complexity**: → **Mitigation**: One-way text→form on
  parse success, one-way form→text on control change. Don't try to merge
  partial edits. Parse errors disable form sync until text is valid.
- **maps_to decomposition**: The query component (owned-edge traversal)
  and view component (owner→sink→source column grouping from tape[-2])
  are distinct concerns. → **Mitigation**: The query is a standard
  traversal step in `?q=`; the view mode is a `&view=` parameter that
  tells `renderMapsToTable` to read tape structure for column grouping.

## Resolved Questions

- **maps_to QuerySpec shape**: `Subject::Implicit` THEN submap traversal
  THEN owner→sink traversal on Pragmatic(1) THEN sink→source traversal
  on Pragmatic(1). Three projection steps total. View mode reads tape[-2]
  for the owner column grouping. Define as a free function that builds
  this QuerySpec given the anchor BID and depth.
- **Projection step card reordering**: up/down arrows + add/remove is
  sufficient. No drag-to-reorder needed for initial version.
- **Composition visualization**: nested/indented list. More space-efficient
  than tree layout for typical panel widths.

## Implementation Notes from Issue 81

- **`ViewRenderer::render` signature changed**: now takes
  `links: Option<&dyn LinkResolver>`. Any viewer-side WASM call to `render`
  needs updating. `BeliefBaseLinkResolver::new(&bb, from_path)` can be
  constructed from the WASM `BeliefBaseWasm`'s loaded shard data.
- **`BeliefNode::render_text_html()`** renders `payload.text` as HTML via
  `render_markdown_snippet`. Available for viewer-side node text rendering.
- **`resolve_node_href(bb, bid, from_path)`** resolves BIDs to relative HTML
  hrefs. Uses two-step PathMap lookup to avoid cross-network BID prefixes.
- **Empty-result state** already implemented: tables with no data show
  `<p class="noet-query-empty"><em>No results.</em></p>` instead of empty
  headers. No need to re-implement.
- **`processLoadedContent`** in `content.js` is the hook for attaching
  click handlers to "Open in Search" buttons in `.noet-query-result` blocks
  (Step 5). Same pattern as `injectHeaderAnchors`.
- **Compile-time query block** (`generate_html_for_path` in compiler.rs,
  ~L4210–4390) is well-structured for adding the `noet-query-meta` div
  (Step 5). The query string is available from `spec_json`, result count
  from `package.graph()`, truncation from `max_rows` comparison.

## References

- `docs/design/query_model.md` §9.5.9 — surface bindings (URL, MyST, MCP)
- `assets/viewer/traceability.js` — primary edit target
- `assets/viewer/search.js` — Explore button added here
- `assets/viewer/content.js` — `processLoadedContent` hook for query buttons
- `src/shard/search.rs` — field-scoped search and boolean operators
- `src/shard/export.rs` — shard halo fix
- `src/query/view/mod.rs` — `LinkResolver`, `BeliefBaseLinkResolver`, `ViewRenderer`
- `src/beliefbase/context.rs` — `resolve_node_href`, `ExtendedRelation`
- `src/properties.rs` — `BeliefNode::render_text_html`
- Issue 79 — `QuerySpec` types, `buildQuerySpec()` in viewer
- Issue 80 — query parser, `?q=` URL integration
- Issue 81 (completed) — `{query}` directive, `LinkResolver` infrastructure
- Issue 85 — Graph visualization mode (extracted from this issue)
- Issue 70 (completed/OBE) — original phases now tracked here
