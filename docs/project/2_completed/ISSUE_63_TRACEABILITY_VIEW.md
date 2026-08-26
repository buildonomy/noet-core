---
version = "0.1"
title = "Issue 63: Traceability View"
---

# Issue 63: Traceability View - ✅ COMPLETE

**Priority**: HIGH
**Status**: COMPLETE (2025-07-10)
**Estimated Effort**: 5 days (RELATIVE COMPARISON ONLY) (Actual: ~5 days)
**Dependencies**: Mapping Node Architecture (completed, see `docs/design/mapping_node_architecture.md`)

---

## Summary

Users need a structured, tabular view of how any selected node relates to the
rest of the beliefbase via `{maps_to}`-owned edges and direct graph edges. The
current metadata panel shows flat edge lists; a traceability modal exposes a
sortable, filterable, exportable matrix keyed on `submap` order — the canonical
network ordering of sibling/descendant nodes.

---

## Goals

1. Add a **Traceability modal** triggered from the metadata drawer for the
   `selectedNodeBid`, displaying a table whose rows are `submap` entries and
   whose columns are incoming/outgoing edge counts grouped by `WeightKind`.
2. Add a **`maps_to` mode toggle** that replaces the row set with the *sink*
   nodes reached via owned edges of each row node's `{maps_to}` directives.
3. Add a **`WeightKind` column filter** (Section / Epistemic / Pragmatic, each
   with in/out) so users can focus the matrix.
4. Expose a **`get_submap` WASM endpoint** (non-async, mirroring `get_nav_tree`)
   and a **`get_context_bulk` WASM endpoint** so the frontend can load the full
   row set with two calls rather than N+1.
5. Export the displayed table to **CSV** (Phase 1) and **ODS/XLSX** (Phase 2).

---

## Architecture

### Data flow

```
selectedNodeBid (state.js)
  │
  ▼
metadata.js: renderNodeContext()
  └── "Traceability" button appended after "Node Information" <details> block
      (visible whenever home_net is non-null, which it always is when the
       drawer is open — home_net is already in the get_context response)
  │
  ▼
traceability.js: openTraceabilityModal(bid, home_net_bid)
  │  1. bb.get_submap(home_net_bid, "", 0, false)
  │     → [{path, bid, order, is_network}, ...]   ← ordered row set
  │  2. bb.get_context_bulk([...row bids])
  │     → Map<bid, NodeContext>                   ← edge matrix for all rows
  │  3. if maps_to mode:
  │        for each row bid, filter context.graph to edges where
  │        owner_bid == row_bid; collect sink bids; de-dup + sort
  │        re-call get_context_bulk([...sink bids]) for the sink set
  │
  ▼
renderTraceabilityTable(rows, columns, context_map)
  │
  ▼
<dialog> modal  (HTML5 dialog element, ARIA labelled)
  ├── controls bar: WeightKind checkboxes, maps_to toggle, depth spinner, Export CSV button
  └── <table>: rows × columns, cells = edge count
```

### `get_submap` WASM endpoint

Mirrors `get_nav_tree`: synchronous, reads `paths().submap(...)` directly.
Returns a `JsValue` array of `{ path: string, bid: string, order: number[], is_network: bool }`.

```rust
#[wasm_bindgen]
pub fn get_submap(
    &self,
    network_bid: String,
    path: String,       // "" = entire network; network-relative path OR BID of any node
    depth: u8,          // 0 = this level only (subnets opaque); 255 = full recursion
    include_index: bool,
) -> JsValue { ... }
```

The frontend passes `home_net` from the already-available `get_context` response
as `network_bid` — no additional WASM call is required to discover it.

**`path` scopes to any node, not just directories**: the underlying
`PathMap::submap` resolves `path` to a BID via `pm.get(path, self)` and then
emits only the subtree rooted at that node. If the resolved BID lives in a
subnet, the implementation delegates to that subnet's `PathMap` and re-prefixes
the returned paths with the subnet's own path and order. Passing `""` returns
the full network. The WASM endpoint exposes this parameter directly; the UI
defaults to `""` (full network view) but can pass any network-relative path to
scope the traceability table to a specific node and its descendants.

**`depth: u8` rationale**: `u8::MAX = 255` is an astronomical recursion depth
for any real graph. Using `u8` instead of `u32` makes the sentinel value
self-evidently bounded and avoids magic-number ambiguity. The type is also
narrower at the WASM boundary.

**`depth: 0` default**: subnets appear as opaque single rows (leaf entries)
rather than being expanded inline. This prevents combinatorial row explosion when
a network contains many subnets. Rows whose `is_network` flag is `true` may get
an expand affordance in the UI; the depth spinner lets the user expand all subnets
uniformly to a chosen level by re-calling `get_submap` with a higher depth value.

**Rust-layer change required**: the current `PathMap::submap` / `PathMapMap::submap`
signatures take `recurse: bool` and propagate it uniformly with no depth tracking.
Replacing this with `depth: u8` (decrement on each subnet recursion, stop at 0)
requires modifying both `PathMap::submap`, `PathMapMap::submap`, and the
`BeliefSource::submap` trait method. All existing call sites that pass `recurse:
true` become `depth: u8::MAX`; `recurse: false` becomes `depth: 0`. This is a
contained change but touches the trait boundary.

### `OwnedEdge` and `BeliefContext::owned_edges`

Owned-edge awareness moves into `context.rs` as a first-class native capability,
not a WASM-only concern.

```rust
// context.rs
pub struct OwnedEdge {
    pub owner_bid: Bid,    // the {maps_to} section node that owns this edge
    pub source_bid: Bid,
    pub sink_bid: Bid,
    pub weight_kind: WeightKind,
}
```

`BeliefContext` gains:

```rust
pub fn owned_edges(&self) -> Vec<OwnedEdge>
```

This walks `self.sources()` and `self.sinks()`, resolves `WEIGHT_OWNED_BY` bref
strings against `self.bb.brefs()` (a plain `&BTreeMap` — no lock conflict with
the `relations_guard` already held), and emits only third-party-owned entries
(i.e. excludes `"source"`, `"sink"`, and absent `WEIGHT_OWNED_BY` values).

The `resolve_owner_bid` closure currently inlined in `extract_node_context`
(`wasm.rs`) is removed in favour of this method. `NodeContext` (the serializable
WASM DTO) gains a corresponding `owned_edges: Vec<SerializedOwnedEdge>` field
populated by calling `ctx.owned_edges()`, making the maps_to derivation
client-side: no separate `get_owned_edges` endpoint is needed.

### `get_context_bulk` WASM endpoint

Replaces the O(N) `get_context` call loop with a single bulk fetch.

```rust
// wasm.rs
#[wasm_bindgen]
pub fn get_context_bulk(
    &self,
    bids: Vec<String>,   // JS string array of BID strings
) -> JsValue             // JS Map<bid_string, NodeContext>
```

Holds a single `borrow_mut()` across all BIDs, calls `extract_node_context` per
BID, and collects results into a JS `Map`. Each entry is structurally identical
to a `get_context` response for that BID. The frontend indexes into the map per
row to populate edge counts; the `owned_edges` field on each `NodeContext` drives
maps_to mode without any additional WASM calls.

### Column schema

Six logical columns, two per `WeightKind`:

| Column key | Label |
|---|---|
| `section_in` | Section ↓ |
| `section_out` | Section ↑ |
| `epistemic_in` | Epistemic ↓ |
| `epistemic_out` | Epistemic ↑ |
| `pragmatic_in` | Pragmatic ↓ |
| `pragmatic_out` | Pragmatic ↑ |

The WeightKind filter checkboxes show/hide column pairs. Default: all visible.

### `maps_to` mode

The maps_to toggle changes what the rows represent:

- **Off (default)**: rows = submap entries (nodes in the selected node's network,
  in path/sort order). Columns show direct in/out edges for each row node,
  sourced from `get_context_bulk` over the submap BID list.
- **On**: for each submap entry, inspect its `NodeContext.graph` for edges where
  `owner_bid == row_bid` (i.e. edges *owned* by that row's `{maps_to}` directive).
  Collect all *sink* BIDs from those owned edges. The row set becomes this sink
  union, de-duplicated and ordered by `WEIGHT_SORT_KEY` ascending. Call
  `get_context_bulk` on the sink set; columns show direct in/out edges for each
  sink node (not the mapping owner).

This is equivalent to: "given that each row node in my network owns some
mappings, show me what those mappings point *to*, and the relationships *those*
nodes have with the rest of the beliefbase."

The `owner_bid` field on `EdgeEntry` (already populated by `extract_node_context`
and present in every `get_context` / `get_context_bulk` response) is the primary
mechanism. No separate `get_owned_edges` endpoint is needed — the bulk context
already carries this information.

### Button placement in `metadata.js`

`renderNodeContext()` builds the drawer HTML string. The "Traceability" button is
inserted immediately after the closing `</div>` of the "Node Information"
`<details>` block (before the Source backlink section). Because `home_net` is
destructured from `context` at the top of `renderNodeContext` and is always
present when the drawer is open, no conditional guard beyond `home_net` being
non-null is required. The button carries `data-bid` and `data-home-net`
attributes; `attachMetadataLinkHandlers()` wires the click handler to
`openTraceabilityModal(bid, home_net)`.

### Export surface (Phase 2 design constraint)

The table data must be serializable as a flat array of row objects keyed by
column ID so that any spreadsheet library (e.g. SheetJS/`xlsx`) can consume it
without additional transformation:

```js
[
  { path: "req/alpha.md", bid: "...", label: "Alpha Req",
    section_in: 0, section_out: 1, pragmatic_in: 2, pragmatic_out: 0,
    epistemic_in: 0, epistemic_out: 0 },
  ...
]
```

A `buildExportRows(rows, visibleColumns)` helper in `traceability.js` produces
this structure; Phase 1 serializes it to CSV directly; Phase 2 passes it to a
spreadsheet library. This decoupling means Phase 2 requires no API changes.

---

## Implementation Steps

### Phase 1 — Core traceability table (3 days)

1. **`OwnedEdge` + `BeliefContext::owned_edges` + `get_submap` + `get_context_bulk`** (1.5 days)
   - [x] Add `OwnedEdge { owner_bid, source_bid, sink_bid, weight_kind }` to
         `src/beliefbase/context.rs`
   - [x] Add `BeliefContext::owned_edges(&self) -> Vec<OwnedEdge>` to `context.rs`;
         walks `sources()` + `sinks()`, resolves `WEIGHT_OWNED_BY` bref strings
         against `self.bb.brefs()`, emits only third-party-owned entries
   - [x] Add `owned_edges: Vec<OwnedEdge>` field to `NodeContext` in `wasm.rs`;
         remove the inline `resolve_owner_bid` closure from `extract_node_context`
         and replace with `ctx.owned_edges()`
   - [x] Modify `PathMap::submap`, `PathMapMap::submap`, and the
         `BeliefSource::submap` trait to replace `recurse: bool` with `depth: u8`;
         existing `recurse: true` callers pass `u8::MAX`, `recurse: false` callers
         pass `0` — mechanical substitution, one pass across all implementors
         (`BeliefBase`, `DbConnection`, `BeliefAccumulator`, test mocks)
   - [x] Add `pub fn get_submap(&self, network_bid, path, depth, include_index) -> JsValue`
         to `src/wasm.rs`; non-async, calls `self.inner.borrow().paths().submap(...)`
   - [x] Return array of `{ path, bid, order, is_network }` plain objects
   - [x] Add `pub fn get_context_bulk(&self, bids: Vec<String>) -> JsValue` to
         `src/wasm.rs`; single `borrow_mut()` across all BIDs, calls
         `extract_node_context` per BID, returns JS `Map<bid_string, NodeContext>`

2. **`traceability.js` module** (1.5 days)
   - [x] `openTraceabilityModal(bid, home_net_bid)`: call `get_submap(home_net_bid,
         "", 0, false)`, then `get_context_bulk([...row bids])` to build the
         internal data model in two WASM calls
   - [x] Depth control: spinner (min 0, max 10) re-calls `get_submap` with the
         new depth and re-renders; `depth: 0` keeps subnets opaque
   - [x] `maps_to` mode: for each row's `NodeContext.graph`, collect edges where
         `owner_bid == row_bid`; de-dup sink BIDs; call `get_context_bulk` on the
         sink set; replace row set with results sorted by `WEIGHT_SORT_KEY`
   - [x] `renderTraceabilityTable(rows, columns)`: emit `<table>` HTML
   - [x] `buildExportRows(rows, visibleColumns)`: flat array for export
   - [x] CSV export: `exportToCsv(rows)` using `Blob` + `URL.createObjectURL`
   - [x] Modal lifecycle: open/close, focus trap, Escape key, ARIA

3. **`metadata.js` integration** (0.5 days)
   - [x] In `renderNodeContext`, append a "Traceability" button immediately after
         the "Node Information" `<details>` closing `</div>`, carrying
         `data-bid="${node.bid}"` and `data-home-net="${home_net}"` attributes
   - [x] In `attachMetadataLinkHandlers`, wire the button's click handler to
         `openTraceabilityModal(bid, home_net)` via the `callbacks` object or
         direct import from `traceability.js`

4. **CSS** (0.25 days)
   - [x] Modal overlay + dialog styles (reuse existing noet design tokens)
   - [x] Table: sticky header row, zebra rows, column-group highlights,
         indented subnet child rows (toggled by expand button)
   - [x] Controls bar: checkbox group, maps_to toggle, depth spinner, export button

5. **`viewer.js` wiring** (0.25 days)
   - [x] Import `traceability.js`, expose `openTraceabilityModal` on `callbacks`
         so `metadata.js` can call it without a circular import

### Phase 2 — Spreadsheet export (2 days, separate session)

6. **Library selection trade study** (0.5 days)
   - [x] Evaluated SheetJS, ExcelJS, and native ODS alternatives; selected
         `rust_xlsxwriter` (MIT OR Apache-2.0) — pure Rust, first-class
         `wasm` feature flag, `save_to_buffer()` returns `Vec<u8>` directly.
         No trade study doc needed: single clear winner with no meaningful tradeoffs.

7. **ODS/XLSX export** (1 day)
   - [x] Added `rust_xlsxwriter = { version = "0.94", features = ["wasm"], optional = true }`
         to `Cargo.toml`; appended `"dep:rust_xlsxwriter"` to the `wasm` feature.
   - [x] Added `BeliefBaseWasm::export_xlsx(headers, rows) -> Uint8Array` static
         WASM endpoint in `src/wasm.rs`; delegates to private `build_xlsx_bytes`
         helper; uses `buildExportRows` output shape (same as CSV path).
   - [x] UI: added "Export XLSX" button to the controls bar in `renderSkeleton`;
         wired click handler in `attachControlHandlers` to `exportToXlsx()`.
   - [x] Added `exportToXlsx()` in `assets/viewer/traceability.js`; calls
         `state.wasmModule.BeliefBaseWasm.export_xlsx(keys, rowsArray)` and
         triggers download via `Blob` + `URL.createObjectURL` (same pattern as CSV).

8. **Polish** (0.5 days)
   - [ ] Column sort on header click — **deferred to BACKLOG** (not needed for current use cases)
   - [ ] Loading indicator for large submaps — **deferred to BACKLOG**
   - [ ] Empty-state messaging when no edges exist for visible columns — **deferred to BACKLOG**

---

## Testing Requirements

- Submap returns nodes in correct path/sort order for a known test network.
- `get_submap` WASM endpoint returns correct `bid`, `order`, and `is_network`
  fields; `depth: 0` keeps subnets as opaque leaf rows, `depth: 1` expands one
  level, `depth: u8::MAX` is equivalent to full recursion.
- `PathMap::submap` depth semantics: at depth 0, subnet entries are emitted as
  opaque leaves; at depth N > 0, subnet entries are expanded and depth N-1 is
  passed to recursive calls.
- `BeliefContext::owned_edges()` returns only third-party-owned edges (excludes
  `WEIGHT_OWNED_BY == "source"`, `"sink"`, or absent); `owner_bid`, `source_bid`,
  `sink_bid`, and `weight_kind` are all correctly populated.
- `NodeContext.owned_edges` matches what `ctx.owned_edges()` would return for
  that BID — i.e. `get_context` and `get_context_bulk` produce identical
  `owned_edges` for the same BID.
- `get_context_bulk([bid1, bid2, ...])` returns a JS `Map` with one entry per
  requested BID; each entry is structurally identical to a `get_context` response
  for that BID, including the `owned_edges` field.
- maps_to mode: sink row set equals de-duped union of `sink_bid` values from
  `NodeContext.owned_edges` in the first bulk context result; order matches
  `WEIGHT_SORT_KEY` ascending per owner.
- WeightKind column filter: hiding a kind hides both in/out columns without
  re-querying WASM.
- CSV export: output has header row + one data row per table row; cells are
  quoted to prevent injection via embedded commas/newlines.
- Modal: opens on button click, closes on Escape and close button, focus
  returns to trigger element on close.

---

## Success Criteria

- [x] `bb.get_submap(net_bid, "", 0, false)` returns ordered path entries with
      subnets as opaque leaves; `depth: 1` expands one subnet level; `depth:
      u8::MAX` is equivalent to full recursion and matches the pre-existing
      `recurse: true` behavior.
- [x] `PathMap::submap` and `BeliefSource::submap` trait compile and pass
      existing tests after `recurse: bool` → `depth: u8` migration.
- [x] `BeliefContext::owned_edges()` compiles on both native and wasm32 targets
      and passes unit tests for third-party-owned edge detection.
- [x] `bb.get_context_bulk(bids)` returns a JS `Map` with one entry per requested
      BID; each entry is structurally identical to a `get_context` response for
      that BID, including `owned_edges`.
- [x] Traceability modal opens from the metadata drawer for any node with a
      known home network. *(validated by human browser smoke test)*
- [x] Table rows match submap order; columns reflect correct in/out edge counts
      per WeightKind. *(validated by human browser smoke test)*
- [x] maps_to toggle replaces rows with the sink set and updates column data
      via a second `get_context_bulk` call. *(validated by human browser smoke test)*
- [x] WeightKind checkboxes show/hide column pairs without re-querying WASM.
- [x] "Export CSV" produces a valid, quoted CSV file downloadable in browser.
- [x] "Export XLSX" produces a valid `.xlsx` file downloadable in browser via
      `rust_xlsxwriter` WASM endpoint; bold header row, one data row per export row.
- [x] Modal is keyboard-navigable and screen-reader accessible (ARIA dialog).

---

## Risks

- **Row explosion from subnet expansion**: high depth values on large networks
  can yield hundreds of rows. → **Mitigation**: `depth: 0` default; spinner
  capped at 10 in the UI; WASM call is synchronous so a runaway depth blocks
  the JS thread — document the cap clearly.
- **`BeliefSource::submap` trait migration**: changing `recurse: bool` to
  `depth: u8` is a breaking trait change — all implementors (`BeliefBase`,
  `DbConnection`, `BeliefAccumulator`, test mocks) must be updated in one pass.
  → **Mitigation**: mechanical substitution (`recurse: true` → `u8::MAX`,
  `recurse: false` → `0`); covered by existing submap integration tests.
- **`get_context_bulk` cost for large sink sets**: maps_to mode may produce a
  large sink set requiring a second bulk fetch. → **Mitigation**: the second
  call is still a single pass; if the sink set exceeds a practical threshold
  (e.g. 500 BIDs), surface a warning in the UI and offer to cap results.
- **maps_to direction ambiguity**: `{maps_to}` edges have a defined
  source→sink direction; the maps_to mode always follows to the *sink* set. If
  a use case requires the *source* set, that is a separate feature. →
  **Mitigation**: document the direction constraint clearly in the UI tooltip.
- **SheetJS licensing**: the original SheetJS (xlsx) changed to a non-OSS
  license; must use the Apache-2.0 community fork or an alternative. →
  **Resolved**: selected `rust_xlsxwriter` (MIT OR Apache-2.0) — pure Rust
  WASM-native, no licensing concerns.

---

## Open Questions

- Spreadsheet column filters (pre-applied filter state in the `.xlsx` document)
  are a reach goal — out of scope, not tracked.

## Resolution

All Phase 1 and Phase 2 goals met. Phase 2 polish items (column sort, loading
indicator, empty-state messaging) deferred to `BACKLOG.md` — not needed for
current use cases. Issue closed.
