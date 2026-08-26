# Issue 87: RawTapeView and Covers Fix

**Priority**: HIGH
**Estimated Effort**: 6 days
**Dependencies**: Requires Issue 82 step 9 (tape redesign, result lens — complete)

## Summary

The viewer's maps_to rendering mode (`mapsToMode` in `traceability.js`) is a
hard-coded JS rendering path that duplicates logic better placed in the Rust
view layer. This issue replaces it with a general-purpose `RawTapeView` that
walks the tape entry-by-entry and renders each entry according to its
`TapeContent` variant. It also fixes the `covers` traversal shorthand to
correctly represent owner-edge semantics, and adds `ViewOutput::Json` so the
WASM/JS viewer can consume structured view data instead of doing its own
rendering.

## Goals

- Fix `covers` shorthand to output both edge endpoints (`Source|Sink`)
- Fix dag_model.md §3.1 diagram to show owned edges pointing at edges, not nodes
- Add `ViewOutput::Json` to the `ViewRenderer` trait
- Implement `RawTapeView` that renders the tape per-entry
- Replace `mapsToMode` JS code with `RawTapeView`-backed rendering
- Exercise and mature the `ViewRenderer` trait through a second implementation

## Architecture

### `covers` shorthand

`covers` currently parses to `o-pragmatic-k{N}` (Owner input, Sink output
only). When `input_roles=Owner`, the traversal finds *edges*, not nodes. The
edge index is the fundamental result — owner annotates an edge, not a node.
`-k` artificially discards the source endpoint; `-sk` is the natural
representation. Change to `o-pragmatic-sk{N}`.

### Owner-edge semantics (dag_model.md)

The §3.1 diagram shows `Review §3.1 → REQ-001` labeled "Pragmatic (maps_to)".
This is wrong — the arrow should point at the `Pragmatic(uses)` *edge* between
Design Doc and REQ-001. Owned edges are metadata about relationships, not about
nodes.

### `ViewOutput::Json`

Replace `ViewOutput::Rows(Vec<Vec<String>>)` with
`ViewOutput::Json(serde_json::Value)`. The JSON variant carries structured,
typed data that WASM, MCP, and tests can consume. The shape is
view-specific — `TableView` emits `{ headers, rows }`, `RawTapeView` emits
a vec of entry objects.

Current callers of `ViewOutput::Rows`:
- `compiler.rs`: rejects it with an error message (dead branch)
- `render_rows()`: called only by tests, is a `TableView`-specific method
  (not trait-level), can stay as a test helper

### `RawTapeView`

A new `ViewRenderer` implementation that walks the tape and renders each
entry according to its `TapeContent` variant:

| TapeContent | Rendering |
|-------------|-----------|
| `Edges` (with owner) | Row per edge: owner, sink, source |
| `Edges` (no owner) | Row per edge: source, sink, kind |
| `Nodes` | Row per BID: node title/bref/kind |
| `Compose` | **Needs design** — see Open Questions |
| `Corpus` | Summary marker |

**Html output**: serial tables, one per tape entry. Each table has a caption
showing the step label and entry index.

**Json output**: vec of entry objects, each containing:
- `step_label`: string
- `entry_index`: number
- `content_type`: `"edges"` | `"nodes"` | `"compose"` | `"corpus"`
- `headers`: column names for this entry
- `rows`: array of row objects with typed cell values (BIDs, strings, etc.)

The JS viewer receives the JSON, picks which entry to display (via a
tape-entry selector when tape length > 1), and renders the table. This makes
`traceability.js` a thin interactive wrapper around the Rust view layer.

### Viewer consolidation

Once `RawTapeView` is functional:
- Remove `mapsToMode`, `buildMapsToOwners`, `collectMapsToEndpointBids`,
  `renderMapsToTable`, `buildMapsToExportRows`, `hasActiveQuery()` from
  `traceability.js`
- The panel's view region gets a display-mode selector that includes the
  existing modes (connectivity, depth0) plus `raw_tape`

## Implementation Steps

1. Fix `covers` shorthand (0.5 day)
   - [x] Change output_roles from `Sink` to `Source|Sink` in
         `parse_named_shorthand` (`src/query/parser.rs`)
   - [x] Update `serialize_traversal` to emit `covers` for
         `Owner, Pragmatic, Source|Sink`
   - [x] Fix dag_model.md §3.1 diagram
   - [x] Update query_model.md §9.5.4 covers equivalence

2. `ViewOutput::Json` (1 day)
   - [x] Replace `ViewOutput::Rows` with `ViewOutput::Json(serde_json::Value)`
   - [x] Update `compiler.rs` to handle the new variant
   - [x] Add `render_json()` default method to `ViewRenderer` trait
         (returns error by default; concrete views opt in)
   - [x] Implement `render_json()` on `TableView` — emits
         `{ display, headers, rows: [{ bid, cells }] }`
   - [x] `render_rows()` kept as `TableView`-specific test helper (unchanged)
   - [x] Expose via WASM `queryView(spec, viewKey, params)` binding

3. `RawTapeView` implementation (2 days)
   - [x] New `RawTapeView` struct implementing `ViewRenderer`
   - [x] Html rendering: serial tables per tape entry
   - [x] Json rendering: vec of entry objects
   - [x] Register as `"raw_tape"` in `ViewRegistry`
   - [x] Handle `Edges`, `Nodes`, `Corpus` content types
   - [x] `Compose` handling: simplified Side column (Gap/Both/Merged)
   - [x] Unit tests for each content type (7 tests)
   - [x] Removed `TableDisplayMode::MapsTo` and `maps_to_factory` —
         replaced by `RawTapeView`'s `Edges` (with owner) rendering
   - [x] Converted `test_table_instrument_maps_to` integration test
         to `test_raw_tape_view_maps_to` using `covers` traversal
   - [ ] Browser validation of Compose rendering using a production
         corpus dataset or `tests/network_1` fixture

4. Viewer integration (1.5 days)
   - [x] WASM `queryView(spec, viewKey, params)` binding
   - [x] JS tape-entry selector with ◀/▶ nav arrows (rendered
         inline above the table, flexed to full width)
   - [x] JS rendering of RawTapeView JSON data
   - [x] Remove `mapsToMode` and related dead code from `traceability.js`
   - [x] Edge cells render as `KIND(s)`, `KIND(k)`, `KIND(@)`, or
         `KIND(⚠)` with clickable role links
   - [x] `serde_wasm_bindgen::Serializer::serialize_maps_as_objects`
         for JSON → plain JS objects (not Maps)
   - [x] Edge index remap in `materialize_graph` so tape edge indices
         reference the package graph
   - [x] Seed BIDs included in `materialize_graph` `all_bids`
   - [x] Kind filter applied in `edges_rows` from step's TraversalSpec
   - [x] `owned_by: "source"/"sink"` excluded from third-party owner detection
   - [x] Missing `owned_by` renders as `KIND(⚠)` error indicator
   - [x] Step operation annotation in tape entry JSON and captions
   - [x] Tape view context resolution: bulk-fetch BIDs from tape entries
         not in `currentContextMap`
   - [x] Keyboard navigation (up/down/left/right) works in tape mode
   - [x] Click on role links opens metadata panel
   - [x] Display mode resets to Connectivity on panel open
   - [x] Renamed "Edge Count" → "Connectivity", "Raw Tape" → "Tape"
   - [x] Network filter moved from View to Query section
   - [x] Verify CSV/XLSX export works with new view
   - [x] Unified `queryView()` as single WASM entry point for panel
   - [x] Connectivity `render_json()` produces per-edge BID cells
   - [x] `nodes` lookup map in both view JSON outputs
   - [x] Multi-sheet XLSX export for tape mode (`export_xlsx_multi`)
   - [x] CSV tape export with delimiter rows per entry
   - [x] Removed `query()`, `get_context_bulk`, `computeEdgeLists`,
         `buildNormalExportRows`, `currentContextMap`, `currentRows`
         from `traceability.js`

5. Gutter column and step editor UX (1 day)
   - [x] Add a gutter column before "Node" in connectivity view.
         Displays query depth (tape index) and/or pathmap order.
         Pathmap order: look up each BID in `PathMapMap::indexed_path`,
         render as dot-separated indices (e.g. `0.3.1`). Controlled
         by Depth and Order checkboxes in a gutter-controls `<span>`.
         Order/depth maps injected into view JSON by `query_view()`
         in `wasm.rs` post-processing.
   - [x] Add a named-traversal dropdown to the traversal step editor.
         Pre-populated with the canonical shorthands (`consists_of`,
         `component_of`, `uses`, `used_by`, `draws_from`, `underlies`,
         `covers`, `halo`) plus a "Custom" option. Selecting a
         shorthand sets `input_roles`, `kind_filter`, `output_roles`
         to the corresponding bitfields. Auto-detects current
         shorthand from bitfields on render.

## Testing Requirements

- `covers` shorthand round-trip: `parse(serialize(covers_spec)) == covers_spec`
- `RawTapeView` renders correct columns per content type
- Owner-annotated edges produce (owner, sink, source) columns
- Non-owner edges produce (source, sink, kind) columns
- `ViewOutput::Json` serializes cleanly through WASM
- JS viewer correctly renders each tape content type

## Success Criteria

- [ ] `covers(1)` returns both source and sink endpoints of owned edges
- [ ] `RawTapeView` renders all `TapeContent` variants
- [ ] `mapsToMode` and related JS code removed
- [ ] `ViewOutput::Json` usable from WASM and (future) MCP
- [ ] dag_model.md diagram correctly shows owner→edge relationship
- [x] Gutter column with depth/order checkboxes in connectivity view
- [x] Named-traversal dropdown in step editor populates bitfields

## Risks

- **Compose rendering**: `TableView::render_with_tape` is untested in the
  browser. → **Mitigation**: validate in browser with a production corpus or
  network_1 fixture before porting logic to `RawTapeView`.
- **WASM serialization overhead**: JSON serialization of large edge tables
  could be expensive. → **Mitigation**: profile, consider pagination or
  lazy rendering.

## Additional changes (not in original scope)

- **WeightKind enum reordered** to Section, Pragmatic, Epistemic
  (structural priority order). All bitfield constants, From/TryFrom
  impls, and JS TRAVERSAL_SHORTHANDS updated.
- **Tape edge ordering**: `apply_traversal_to_tape` sorts hop edges
  by `WeightSet::sort_key()` (new method) before recording.
  `extract_entries` preserves tape order instead of collecting into
  `BTreeSet`. JS `sortRowsByOrder()` sorts connectivity rows by
  pathmap order for depth-first structural display.
- **Data-driven keyboard navigation**: replaced DOM-walking model
  (`focusedRowBid`/`collectColEntries`/`collectRowCellBids`) with
  `focusedRow`/`focusedCol` indexing into `getNavRows()`. Unified
  across connectivity and tape modes.
- **Issue 88 drafted**: `ISSUE_88_EDGE_OWNER_TYPE.md` — type-safe
  `EdgeOwner` enum to replace stringly-typed `WEIGHT_OWNED_BY`.

## Open Questions

(None currently — Compose rendering resolved, see step 3 notes.)
