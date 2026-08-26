# Issue 81: `{query}` Directive — Compile-Time Query Rendering

**Version**: 0.2
**Priority**: HIGH
**Estimated Effort**: 2.5 days (RELATIVE COMPARISON ONLY)
**Dependencies**: Requires Issue 79 (QuerySpec types), Issue 80 (Query
Parser), and Issue 83 (BeliefSource refactoring — clean `eval` API).
Related to Issue 70 (the viewer-side query UI).

## Summary

Authors need to embed query results directly in their documents — a design doc
should show its linked requirements as an inline table, generated at compile time,
not maintained by hand. This issue adds a `{query}` MyST directive that evaluates a
query string (Issue 80's grammar) against the compiled BeliefBase and renders results
as HTML in the document.

The integration test fixture and test harness are already committed (Issue 80 scope)
as a TDD contract. This issue makes those tests pass.

## Current API Surface (post-Issues 79, 80, 83)

Key types available for the builder implementation:

- **Parser**: `noet_core::query_parser::parse(input: &str) -> Result<QuerySpec, ParseError>`
- **Subject injection**: use `Subject::Bids(vec![bid])` — `Subject::AnchorBid` does not exist
- **Evaluation**: `BeliefSource::evaluate(&mut QueryPackage)` — async, mutates in place
- **View dispatch**: `VIEWS.get(key: &str) -> Option<ViewFactory>` and `ViewRegistry::default_view()`
- **ViewFactory**: `fn(&toml::Table) -> Result<Box<dyn ViewRenderer>, BuildonomyError>`
- **Rendering**: `ViewRenderer::render(&QueryPackage) -> Result<ViewOutput, BuildonomyError>`
- **Output**: `ViewOutput::Html(String)` — splice into document at sentinel position
- **No `Instrument` enum**: view config is a `toml::Table` (the opaque params bag)
- **No query string suffixes**: view config travels as directive options, not embedded in
  the query string

## Goals

- Register a `{query}` directive in the `DIRECTIVES` array (`src/codec/myst.rs`)
- Parse the directive body as a query string (via `query_parser::parse()`)
- Parse directive options (`:view:`, `:sort:`, `:max-rows:`, `:caption:`, `:columns:`)
  into a `toml::Table` params bag
- Evaluate the query at compile time via `BeliefSource::evaluate(&mut QueryPackage)`
- Render results via `VIEWS.get(view_key).render(&package)` — `ViewOutput::Html`
- Support implicit anchor: when `Subject::Implicit`, inject the current document's
  BID as `Subject::Bids(vec![current_doc_bid])`

## Architecture

### Three-Phase Model

The `{query}` directive follows `myst_directive_architecture.md` §4:

**Phase 1 — Parse**: `MdCodec::parse` detects `CodeBlock(Fenced("{query}"))`,
extracts the body (raw query string) and options. The body must be stored in the
node's `payload` or `metadata` so that the Phase 3 refiner can retrieve it.
One approach: `node.metadata["_query_body"] = toml::Value::String(body)` during
parsing. `has_deferred_render = true`.

**Phase 2 — Immediate HTML**: Emits sentinel `<!--@@noet-query@@-->` at the
directive's position.

**Phase 3 — Deferred Pipeline**: The refiner and builder share a document node's
`BeliefContext`. The refiner reads the stored query body, parses it to a `QuerySpec`,
and returns it. The evaluator runs the spec. The builder renders the `BeliefGraph`
output using the `ViewRenderer` selected by the `:view:` directive option.

### Deferral Pattern: per-instance sentinels (mirrors `maps_to`)

`generate_html_for_path` in `compiler.rs` is the deferred engine. It runs the
`DIRECTIVES` pipeline for static directives, then handles `maps_to` with its own
special block (per-section unique sentinels `<!--@@noet-mapping-table:ANCHOR@@-->`
rather than the global `<!--@@noet-maps-to@@-->`). `{query}` follows the same
pattern as `maps_to`:

**Phase 1 (`MdCodec::parse`)** — `query_parser::parse()` is pure, call it
immediately for each `{query}` block. Store each resulting `QuerySpec` (with
`Subject::Implicit`) serialised into `node.metadata["_query_specs"]` (a TOML array,
one entry per block in document order). Syntax errors are caught here.

**Phase 2 (immediate HTML)** — emit a per-instance sentinel that encodes the block
index: `<!--@@noet-query:N@@-->` where N is 0-based. Multiple `{query}` blocks in
one document each get their own unique sentinel, enabling independent splicing.

**Phase 3 (`generate_html_for_path`)** — add a special-case block parallel to the
`maps_to` block:
1. Scan the on-disk HTML for `<!--@@noet-query:N@@-->` patterns.
2. For each sentinel found, retrieve `spec = ctx.node.metadata["_query_specs"][N]`
   (deserialise from TOML).
3. Resolve `Subject::Implicit → Subject::Bids(vec![ctx.node.bid])`.
4. Parse the directive options from `ctx.node.metadata["_query_options"][N]` to
   build the `toml::Table` params bag.
5. Evaluate: `global_bb.evaluate(&mut QueryPackage::balanced(spec)).await?`.
6. Render via `VIEWS.get(view_key)(&params)?.render(&package)` → `ViewOutput::Html`.
7. Push `(sentinel, html)` onto `splice_pairs`.

**`DIRECTIVES` entry** — `{query}` registers with `queries: &[]` and `builder: None`
(no static pipeline). The entry is needed so `sentinel("query")` returns non-empty
and the block is recognised as a fenced directive. The per-instance sentinel scheme
is opt-in (the standard `<!--@@noet-query@@-->` sentinel is NOT emitted).

### Directive Syntax

The body is the raw query string; options control view configuration:

````md
```{query}
:view: depth0
:sort: section_order
:max-rows: 50
:caption: Applicable Requirements
k-pragmatic-s(1)
```
````

Directive options map directly to the `toml::Table` params bag passed to
`ViewFactory`. Known option keys:

| Directive option | Params key   | Default  |
|-----------------|--------------|----------|
| `:view:`        | `"display"`  | `"depth0"` |
| `:sort:`        | `"sort"`     | (none)   |
| `:max-rows:`    | `"max_rows"` | `200`    |
| `:caption:`     | `"caption"`  | (none)   |
| `:columns:`     | `"columns"`  | (none)   |

The view key from `:view:` is looked up in `VIEWS` to get the factory. If absent or
unknown, `ViewRegistry::default_view()` is used (depth0 TableView).

### Implicit Anchor

When `Subject::Implicit` is produced by the parser (no `id://` anchor in body),
the builder replaces it before evaluation:

```rust
if matches!(spec.subject, Subject::Implicit) {
    spec.subject = Subject::Bids(vec![ctx.node.bid]);
}
```

### Evaluation and Rendering

```rust
// Build the view params table from directive options
let mut params = toml::Table::new();
// ... populate from directive options ...

// Look up the view factory
let factory = VIEWS.get(view_key).unwrap_or(ViewRegistry::default_view_factory());
let renderer = factory(&params)?;

// Evaluate
let mut package = QueryPackage::balanced(spec);
ctx.bb().evaluate(&mut package)?;  // or however BeliefBase is accessed

// Render
let html = match renderer.render(&package)? {
    ViewOutput::Html(h) => h,
    ViewOutput::Rows(_) => return Err(...), // should not happen
};
```

### Error Handling

Parse errors and evaluation errors should produce a visible HTML error block:

```html
<div class="noet-query-error">
  <strong>Query error:</strong> parse error at offset 3: ...
</div>
```

Never silently fail or emit the raw sentinel.

## Implementation Steps

1. Phase 1: parse and store (0.5 day)
   - [x] In `MdCodec::parse`, detect each `{query}` fenced block
   - [x] Call `query_parser::parse(body)` immediately (pure — no BeliefBase needed);
         on parse error store the error string instead of a QuerySpec
   - [x] Serialise each `QuerySpec` to JSON; push into
         `node.metadata["_query_specs"]` (array, order-preserving)
   - [x] Serialise each directive options table; push into
         `node.metadata["_query_options"]` (parallel array)
   - [x] Emit per-instance sentinel `<!--@@noet-query:N@@-->` at each block site
         where N is the 0-based index into `_query_specs`
   - [x] Verify metadata survives the BeliefBase round-trip

2. Register directive in DIRECTIVES (0.25 day)
   - [x] Add `DirectiveDef { name: "query", queries: &[], builder: None, .. }` —
         minimal entry so the block is recognised; no static pipeline

3. Phase 3: special-case block in `generate_html_for_path` (0.75 day)
   - [x] Add a block parallel to the `maps_to` block in compiler.rs
   - [x] Scan `existing_html` for `<!--@@noet-query:N@@-->` patterns
   - [x] For each: retrieve and deserialise `_query_specs[N]` from `ctx.node.metadata`
   - [x] Resolve `Subject::Implicit → Subject::Bids(vec![node_bid])`
   - [x] Assemble params `toml::Table` from `_query_options[N]`
   - [x] Look up factory via `VIEWS.get(view_key)` or `ViewRegistry::default_view()`
   - [x] Evaluate: `global_bb.evaluate(&mut QueryPackage::balanced(spec)).await?`
   - [x] Render: `renderer.render(&package)` → `ViewOutput::Html`
   - [ ] Apply `max_rows` truncation with visible notice
   - [x] On any error: emit `<div class="noet-query-error">` block
   - [x] Push `(sentinel, html)` onto `splice_pairs`

4. Make TDD tests pass (0.5 day)
   - [x] Run `cargo test --features service query_directive` — make all non-ignored tests pass
   - [x] Remove `#[ignore]` from tests in `tests/codec_test/query_directive_tests.rs`
   - [x] Verify `{requirements_table}` regression test still passes

5. Documentation (0.5 day)
   - [x] Document `{query}` in `docs/design/myst_directive_architecture.md` §3.1 and §6
   - [x] Document `_query_specs` / `_query_options` metadata keys in code
         (internal contract, not user-facing — documented at write site in
         `md.rs::inject_context`)

## Testing Requirements

The TDD fixture and harness are already in `tests/network_1/query_directive_test.md`
and `tests/codec_test/query_directive_tests.rs`. All `#[ignore]` tests there define
the success criteria. Running `cargo test --features service -- --include-ignored`
shows exactly what needs to pass.

## Success Criteria

- [x] All `#[ignore]` tests in `query_directive_tests.rs` pass without `--include-ignored`
- [x] Implicit anchor resolves to current document
- [x] Parse error body produces visible `<div class="noet-query-error">` block
- [x] Empty result set renders gracefully (empty table, not a panic)
- [x] `{requirements_table}` continues to work unchanged (regression test)
- [x] Directive documented in `myst_directive_architecture.md`

## Risks

- **Storing directive body in node metadata** creates a semi-public contract. Any
  future schema migration must preserve `_query_body`. → **Mitigation**: Document
  the key explicitly; consider a dedicated `DirectivePayload` type in a future cleanup.
- **Static refiner reads from metadata** — if the metadata key is absent (e.g. the
  node was reconstructed from the DB), the refiner must handle `None` gracefully
  (emit a parse error block rather than panicking).
- **Performance for large result sets**: unbounded traversal could produce thousands
  of rows. → **Mitigation**: Default `:max-rows: 200` with visible truncation notice.

## Open Questions

- Should Phase 1 do a syntax-only parse to catch errors early? The parser is pure
  (no BeliefBase needed), so this is feasible. Defer — Phase 3 error rendering is
  sufficient for MVP.
- Should the `_query_body` metadata key be namespaced (e.g. `noet:query_body`) to
  avoid collisions with user payload? Defer — confirm with metadata schema design.

## Addendum: View Rendering Toolbox (depth0 link resolution)

The depth0 view's `render_depth0_list` emits `<a data-bid="UUID">` links that
don't navigate — they match neither the SPA's two-click contract
(`<a href="..." title="bref://...">`) nor static HTML expectations. This
addendum adds the minimal infrastructure to produce correct links, designed
for reuse across all directive builders.

### Problem

Node link rendering is duplicated across `build_listing_html`,
`build_requirements_table_html`, `build_mapping_table_html` (all in myst.rs),
and now `render_depth0_list` (table.rs). Each re-implements PathMap lookup →
extension rewrite → `AnchorPath::path_to` → `<a>` tag formatting.

Node content rendering (`payload.text` → markdown → HTML) is similarly
duplicated between `render_depth0_list`, `BeliefBaseWasm::render_markdown`,
and `metadata.js`'s `renderNodeContext`.

### Approach: extend existing types

No new structs. Add methods to existing types + a utility function + a trait.

**`resolve_node_href(bb, bid, from_path) -> Option<String>`** (context.rs):
Standalone function extracting the path-resolution logic duplicated in the
three myst.rs builders. PathMap lookup → CODECS extension check → `path_to`.

**`BeliefNode::render_text_html() -> String`** (properties.rs):
Render `payload.text` via `render_markdown_snippet`. Unifies the identical
text rendering in table.rs, wasm.rs, and metadata.js.

**`LinkResolver` trait** (query/view/mod.rs):
```rust
pub trait LinkResolver {
    fn resolve_href(&self, bid: &Bid) -> Option<String>;
    fn resolve_anchor(&self, node: &BeliefNode, content: &str) -> String;
}
```
Implemented by a simple struct wrapping `&BeliefBase` + `current_doc_path`.
Platform-independent — works native, WASM, and MCP.

**`ViewRenderer::render` signature change**:
```rust
fn render(&self, package: &QueryPackage, links: Option<&dyn LinkResolver>)
    -> Result<ViewOutput, BuildonomyError>;
```
All callers updated mechanically. `None` = plain text fallback (MCP/contexts
without path context). `Some` = navigable `<a href title="bref://...">` links
matching the two-click contract.

### Implementation Steps

6. Link resolver infrastructure (0.5 day)
   - [x] Add `resolve_node_href(bb, bid, from_path)` to `beliefbase/context.rs`
   - [x] Add `BeliefNode::render_text_html()` to `properties.rs`
   - [x] Add `LinkResolver` trait to `query/view/mod.rs`
   - [x] Add `links: Option<&dyn LinkResolver>` parameter to `ViewRenderer::render`
   - [x] Implement `BeliefBaseLinkResolver` wrapping `&BeliefBase` + path
   - [x] Update `render_depth0_list` to use resolver + `render_text_html`
   - [x] Update all `render` call sites (compiler.rs, tests) — mechanical
   - [x] Clone `node_bb` + merge query graph for link resolution (avoids
         deadlock with `BeliefContext`'s read lock on `node_bb`)

### Success Criteria (addendum)

- [x] depth0 links produce `<a href="..." title="bref://...">` (two-click contract)
- [x] depth0 links navigate in both static HTML and SPA
- [x] `BeliefNode::render_text_html` used by depth0 renderer
- [x] No HTML string post-processing for link resolution

### Future: query metadata div + SPA search integration

Tracked in Issue 82 Step 5: embed hidden query metadata in output, add
"Open in Search" button, cap static HTML at `MAX_QUERY_ROWS`.

### Future: unify myst.rs builders (separate issue)

Once `resolve_node_href` and `LinkResolver` exist, the three myst.rs builders
can be refactored to use them, eliminating ~150 lines of duplicated resolve
closures. Also: `ExtendedRelation::render_anchor` method for the common
`<a href title="bref://">` pattern. This is Phase 2 — not in Issue 81 scope.

## References

- `docs/design/query_model.md` §9.5.9 — MyST directive surface binding
- `docs/design/myst_directive_architecture.md` §4, §8 — three-phase model
- `docs/design/myst_directive_architecture.md` §6.2 — `{requirements_table}` as prior art
- `src/codec/myst.rs` — `DIRECTIVES`, `DirectiveDef`, `DirectiveRefiner`, `BeliefContext`
- `src/query_parser.rs` — `parse()`, `ParseError` (Issue 80)
- `src/query/view/mod.rs` — `VIEWS`, `ViewRegistry`, `ViewFactory`, `ViewRenderer`
- `src/query/view/table.rs` — `TableView`, `from_params()` (Issue 79/80)
- `tests/network_1/query_directive_test.md` — TDD fixture (committed in Issue 80)
- `tests/codec_test/query_directive_tests.rs` — TDD harness (committed in Issue 80)
- Issue 79 — `QuerySpec` types, `QueryPackage`, evaluator
- Issue 80 — query string parser, `ViewRegistry`
- Issue 70 (completed/OBE) — original unified query UI issue
- Issue 82 — viewer query UI enhancements
