# Issue 69: XLSX Codec — Structured Spreadsheet Ingestion

**Priority**: HIGH
**Status**: COMPLETE
**Supersedes**: Issue 71 (Schema v2), Issue 74 (Viewer Bugs)
**Version**: 0.1

## Summary

Implements end-to-end ingestion and HTML rendering of Excel workbooks (`.xlsx`, `.ods`) as
first-class noet nodes. The codec parses workbooks via `calamine`, maps columns through a
`ColumnEntry`-based unified column map, emits row nodes with stable IDs, and resolves
cross-shard relations in `inject_context`. A Tabulator v6 viewer (`xlsx-tabs.js`) renders
each workbook as a tab-switched table with two-click relation navigation. A write-back
pipeline annotates source files with `__noet_bid__` and `__noet_relation_{ir_key}__` hidden
columns, enabling round-trip relation authoring.

## What Was Built

### 1. XlsxCodec (`src/codec/xlsx/`)

Full `Codec` trait implementation: `proto()`, `parse()`, `inject_context()`,
`generate_html()`, and `generate_source_bytes()`.

### 2. Schema Grammar (`src/codec/xlsx/schema.rs`)

- `WorkbookSchema`, `TabSchema`, `ColumnRole` — roles: `title`, `text`, `relation`,
  `payload`
- `RelationKeyFormat` — controls how the relation IR key is derived from column name
- Lazy column defaults — unconfigured columns get a role inferred from content
- Wildcard tab (`name: "*"`) — applies a default schema to all unmatched sheets
- `text_template` — Jinja-style `{{ placeholder }}` interpolation (spaces inside braces
  required); `to_anchor` normalization applied to resolved values
- `ignore: true` — excludes a tab from the node graph
- `__noet_*__` reserved column namespace for internal write-back columns

### 3. ColumnEntry / ColumnKind

Unified column map replacing the old `effective_schema` tuple and `PropertySources`:

- `build_column_map` — five-pass scan: reserved detection → explicit schema match →
  wildcard match → header inference → fallback
- `ColumnKind::RelationBref` — handles `__noet_relation_{col}__` write-back columns
- `ir_key` collision validation at parse time — duplicate IR keys produce a hard error

### 4. Row Node Stable IDs

Format: `{workbook_prefix}-{tab_slug}-{row_number}`, set at parse time.
`inject_context` corrects `doc["id"]` from `ctx.node.id` when `GraphBuilder` detects a
collision and assigns a disambiguated ID.

### 5. `inject_context` Write-Back

- Syncs `doc["id"]` from `ctx.node.id`
- Populates `doc["{ir_key}_bids"]` from `ctx.sinks()` for cross-shard relation resolution
- Collects `RowRelation` entries for relation edge emission
- `generate_source_bytes()` writes `__noet_bid__` and `__noet_relation_{ir_key}__` hidden
  columns back into the workbook bytes via `write_annotated_sheet`

### 6. HTML Viewer

Files: `assets/viewer/xlsx-tabs.js`, `assets/noet-layout.css`,
`assets/template-xlsx-workbook.html`

- Tabulator v6 lazy-init per tab — columns and data built from JSON embedded at render time
- Tab switching with `_tab_order` JSON field preserving workbook sheet order
- Two-click navigation pattern via `handleNodeLinkClick`
- Parse-time BIDs in `{field}_bids` fields enable cross-shard relation links
- `__noetToAnchor` WASM static method used for ID normalization at link-click time
- Tabulator theme overrides via `--noet-*` CSS custom properties

### 7. SPA Routing Fixes

- Nav links emit `href="#/{path}"` (hash-safe routing)
- `scheduleBuildNavigation` debounce prevents redundant rebuilds
- `history.replaceState` / `pushState` use root-relative absolute URLs
- Unloaded-shard navigation loads the shard without triggering a confirmation dialog

### 8. Source Link

- `{{SOURCE_LINK}}` / `{{SOURCE_BINARY}}` template placeholders
- `#noet-source-link` element with `data-binary` attribute
- Metadata panel "View Source" / "Download Source" row
- Network index nodes correctly resolve to `index.md` (previously returned `None`)

## Architecture

**Parse-time data flow**: `calamine` opens the workbook → `build_column_map` runs the
five-pass scan for each sheet → the row loop emits `IntermediateNode` and
`IntermediateRelation` records → `inject_context` receives the resolved `ctx` and syncs BIDs
and relation brefs back into the document map → `generate_html` serializes Tabulator-ready
JSON into the HTML fragment → `generate_source_bytes` calls `write_annotated_sheet` to
produce an annotated workbook with hidden noet columns.

**Viewer pattern**: The SPA injects the workbook HTML fragment into the content area →
`noetInitXlsxTabs(callbacks)` registers the tab-bar click handler and Tabulator lazy-init →
on first tab activation Tabulator renders the column definitions and row data from embedded
JSON → cell clicks on relation values call `handleNodeLinkClick`, which applies the two-click
pattern (first click previews, second navigates) → navigation resolves through
`showMetadataPanel` and `navigateToLink` using the `{field}_bids` cross-shard lookup.

## Testing

53 xlsx unit tests and 27 integration tests, all passing.
`cargo clippy --all-features --all-targets -- -D warnings` clean.

## References

- `noet-core/src/codec/xlsx/` — codec implementation
- `noet-core/assets/viewer/xlsx-tabs.js` — Tabulator viewer
- `noet-core/docs/design/xlsx_codec_schema.md` — schema reference