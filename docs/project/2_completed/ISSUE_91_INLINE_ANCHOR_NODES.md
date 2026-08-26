# Issue 91: Inline Anchor Nodes — Non-Heading Block Anchors as Named Nodes

**Priority**: MEDIUM
**Estimated Effort**: 3 days (RELATIVE COMPARISON ONLY)
**Dependencies**: None

## Summary

The markdown codec currently creates nodes only at heading boundaries. Inline
anchors (`{#some-id}`) appearing in non-heading blocks (paragraphs, list items)
are silently ignored. Adding support for these would allow individually-named
requirements, checklist items, or other block-level content to become first-class
graph nodes without requiring a heading for every item.

## Goals

- Detect `{#anchor-id}` in any paragraph or list-item block during `parse()`
- Push a new node onto the stack when one is encountered, using the anchor as
  the node's explicit ID
- Assign the new node a heading depth one greater than the enclosing section
  heading, so consecutive inline-anchor nodes are siblings
- Accumulate subsequent content into the new node until the next node boundary
  (next heading or next inline anchor)
- Round-trip: `generate_source` reproduces the inline anchor in its original
  position
- All existing tests continue to pass

## Architecture

### Where inline anchors appear

pulldown-cmark emits inline HTML for raw `{#id}` syntax only when it appears
inside a heading (via the `ENABLE_HEADING_ATTRIBUTES` extension). In non-heading
blocks the `{#id}` text is emitted as a plain `MdEvent::Text` or
`MdEvent::InlineHtml` event — it is not parsed by the library at all.

### Detection strategy: scan at block end

Rather than peeking at the first `Text` event after `Start(Paragraph)` (which
breaks when inline formatting precedes the anchor), accumulate all events for
the block normally and scan for `{#id}` at the block-closing event
(`End(Paragraph)` or `End(Item)`).

1. Track block nesting with a flag set on `Start(Paragraph)` or `Start(Item)`.
2. As events accumulate, buffer `Text` / `InlineHtml` content for anchor
   scanning (or scan lazily at the end).
3. On `End(Paragraph)` or `End(Item)`, call `extract_inline_anchor` on the
   buffered text. If an anchor `{#id}` is found:
   - Perform the full node-boundary reset (see §Boundary behaviour).
   - Push the current node to `current_events`.
   - Start a new `IRNode` with the extracted ID as both its explicit `id` and
     `title`, at depth `enclosing_section_heading + 1`.
   - Transfer the buffered block events into the new node's `proto_events`.
4. If no anchor is found, the block events stay with the current node as normal.

This allows `{#id}` to appear anywhere within the block — after emphasis,
links, or other inline markup — without special-casing event order. The entire
block (from `Start` to `End`) belongs to the anchor node.

Detection triggers on both `Paragraph` and `Item` blocks. Tight list items
(no `Paragraph` wrapper inside `Item`) are handled because `Start(Item)` /
`End(Item)` is itself a detection boundary. Table cells are deferred to a
follow-on issue.

### Node depth

Inline-anchor nodes use depth `enclosing_section_heading + 1`, where
`enclosing_section_heading` is the heading depth of the most recent
heading-created node. This is determined by reverse-searching
`current_events` for the last entry whose node was created at a heading
boundary (has a title and heading events in its proto_events), and reading
its `heading` value. If no heading node exists yet (anchor before any
heading), the document root's heading depth (2) is used.

This ensures consecutive inline-anchor paragraphs under the same heading
are **siblings**, not nested:

```markdown
## Section          ← heading 4
{#item-a} First     ← heading 5 (child of Section)
{#item-b} Second    ← heading 5 (sibling of item-a, child of Section)
### Subsection      ← heading 5 (heading boundary, sibling of items)
{#item-c} Third     ← heading 6 (child of Subsection)
```

`get_parent_from_stack` in `builder.rs` uses absolute heading depth for
parent selection (`stack_heading < proto.heading`), so correct absolute depth
is sufficient for correct tree structure.

### Boundary behaviour

An inline-anchor node boundary opens at the `Start` of the block containing
the anchor and closes at:
- The next heading start (existing behaviour)
- The next inline anchor in a subsequent block
- End-of-document

Content between two consecutive inline-anchor paragraphs (e.g. a plain
paragraph with no anchor) accumulates into the preceding anchor node, exactly
as plain paragraphs below a heading accumulate into that heading's node.

At the inline-anchor boundary, perform the **same full state reset** as at
heading boundaries:
- `relation_context_stack.drain(..)` with warning per unclosed entry
- `in_maps_to_block = false`, `maps_to_body_accum` cleared,
  `maps_to_weight_kind_override = None`
- `in_query_block = false`, `query_body_accum` cleared

Additionally:
- Insert the extracted anchor ID into `seen_ids` and emit a
  `ParseDiagnostic::warning` on collision (same as heading ID collision
  detection at `End(Heading)`)
- Call `traverse_schema()` on the pushed node (same as heading boundary)

### Round-trip

`generate_source` reproduces content verbatim from stored events. Because the
entire block (including the `{#id}` text) is stored in the new node's
`proto_events`, round-trip is automatic. The anchor text is not stripped; it
is both the node ID source and the round-trip marker.

## Implementation Steps

1. **Verify pulldown-cmark event shape** (0.25 days)
   - [x] Write a standalone test confirming `{#id}` in a paragraph body
         survives as `Text` or `InlineHtml` (not consumed by heading-attributes)
   - [x] Confirm event shape for tight and loose list items
   - [x] Confirm that `{#id}` after inline formatting (emphasis, links) is
         still present in a `Text` event

2. **Add block-end anchor detection** (0.75 days)
   - [x] Write `fn extract_inline_anchor(s: &str) -> Option<String>` using
         shared `ANCHOR_CHAR_CLASS` from `paths::path`
   - [x] Track block context: set a flag on `Start(Paragraph)` / `Start(Item)`,
         buffer text content for anchor scanning
   - [x] On `End(Paragraph)` / `End(Item)`: scan buffered text; if anchor found,
         perform full boundary reset (relation context, maps_to, query state),
         push current node, start new `IRNode` with extracted ID as both `id`
         and `title` at depth `enclosing_section_heading + 1`, transfer block
         events to new node
   - [x] Insert anchor ID into `seen_ids`; emit collision warning if duplicate
   - [x] Call `traverse_schema()` on the pushed node

3. **Accumulate remainder into anchor node** (0.25 days)
   - [x] Verify plain paragraphs following an inline-anchor block accumulate
         into the anchor node (same as plain paragraphs below a heading)
   - [x] Verify a subsequent heading closes the anchor node correctly

4. **Tests** (1 day)
   - [x] Paragraph with `{#para-id}` → child node with ID `para-id`, title
         `para-id`, at depth `section.heading + 1`
   - [x] Two consecutive anchor paragraphs under one heading → two sibling
         child nodes at the same depth
   - [x] Tight list item with `{#item-id}` → child node (no `Paragraph` wrapper)
   - [x] `{#id}` after inline formatting (`*text* {#id} more`) → detected
   - [x] Plain paragraph after anchor paragraph → content folds into anchor node
   - [x] Anchor paragraph followed by heading → heading closes anchor node
   - [x] Round-trip: source in → `generate_source` out → re-parse yields same
         node graph
   - [x] Relation context open at anchor boundary → warning emitted
   - [x] Duplicate anchor ID → collision warning emitted
   - [x] Paragraph without anchor → no node split (no regression)
   - [x] All existing heading-anchor tests pass unchanged (115/115)

5. **HTML rendering pass in `render_html_body`** (0.5 days)
   - [x] Detect inline-anchor protos (explicit ID, no `Start(Heading)` in events)
   - [x] Inject `<a id="{id}" class="noet-inline-anchor" title="bref://..."></a>`
         before the block's events, with bref from proto BID when available
   - [x] Strip `{#id}` from `Text` event (replace with empty, trim whitespace)
   - [x] Test: rendered HTML has anchor element, no literal `{#para-id}` visible

6. **`content.js` — extend `injectHeaderAnchors`** (0.25 days)
   - [x] Query `a.noet-inline-anchor` elements alongside existing `h1–h6` query
   - [x] Apply same bref lookup and 🔗 link injection as for headings
   - [x] Use `insertAdjacentElement("afterend", link)` (anchor element is empty)
   - [ ] Test: 🔗 appears, fragment URL works, bref resolves

7. **Source-line accuracy** (0.25 days)
   - [x] Verify `source_line` on inline-anchor nodes points to the block's
         opening line (set from first event's byte offset in splice)
   - [x] Byte-offset → line uses existing `byte_offset_to_location`

## Testing Requirements

- All existing `mod tests` pass unchanged
- New tests cover: paragraph anchor, list-item anchor, anchor after inline
  formatting, consecutive anchors as siblings, heading boundary interaction,
  round-trip fidelity, ID collision warning
- No domain-specific content in test fixtures (use generic "item-a", "req-001"
  style IDs)

## Success Criteria

- [x] `{#some-id}` in a paragraph body produces a node with ID `some-id` and title `some-id`
- [x] `{#some-id}` in a list item produces a node with ID `some-id` and title `some-id`
- [x] `{#id}` after inline formatting within a block is detected
- [x] Consecutive inline anchors under one heading are siblings, not nested
- [x] Heading-level anchors are unaffected
- [x] Round-trip: source in → source out with anchor intact
- [x] All existing tests pass (696/696 lib tests, 17 new)

## Risks

- **pulldown-cmark event shape**: `ENABLE_HEADING_ATTRIBUTES` processes `{#id}`
  only in headings; body blocks should pass `{#id}` through as raw `Text`.
  → **Mitigation**: Step 1 is a standalone verification test; no `parse()`
  changes until the event shape is confirmed.

- **Schema inheritance**: `traverse_schema()` is called on heading-boundary
  nodes to inherit the parent schema. Inline-anchor nodes need the same call.
  → **Mitigation**: Call `traverse_schema()` on the pushed node at the
  inline-anchor boundary, same as at heading boundaries.

- **Block-end detection and event buffering**: Scanning at `End(Paragraph)` /
  `End(Item)` means the events are already accumulated into the current node's
  `proto_events`. If an anchor is found, the block's events must be transferred
  from the current node to the new node. This is a splice operation on
  `proto_events` (remove from tail of current, prepend to new).
  → **Mitigation**: Track the event index at `Start(Paragraph)` /
  `Start(Item)` so the splice boundary is known exactly.

## HTML Rendering

Heading anchors are stripped and re-injected by pulldown-cmark's
`ENABLE_HEADING_ATTRIBUTES` extension. Inline-anchor blocks receive no such
treatment — the `{#id}` text renders as visible content unless handled.

**Approach: event-rewriting pass in `render_html_body`.** The existing
event-rewriting layer already handles rel-typed link injection and directive
marker replacement. Add one more pass that:

1. Detects inline-anchor protos (explicit ID, no heading event in proto_events).
2. Injects `<a id="{id}" class="noet-inline-anchor"></a>` before the block start.
3. Strips `{#id}` from the `Text` event so it doesn't appear in rendered output.

The `noet-inline-anchor` class serves as the detection hook for `content.js`,
which extends `injectHeaderAnchors` to query these elements and apply the same
bref-lookup and 🔗 link injection pattern used for headings.

## Open Questions

- Table cells (`Start(TableCell)`) have a different event shape and nesting
  model. Defer to a follow-on issue unless trivial during Step 1 verification.

- ~~**Code span / code block suppression**~~ **Resolved.** Code spans emit
  `MdEvent::Code` (not `Text`), so `extract_inline_anchor` never fires.
  Fenced code blocks are block-level siblings of paragraphs — their `Text`
  events fall outside the `Start(Paragraph)`…`End(Paragraph)` scan range,
  so no `in_plain_code_block` flag is needed. Regression tests
  `test_inline_anchor_not_in_code_span` and
  `test_inline_anchor_not_in_fenced_code_block` confirm both cases.
