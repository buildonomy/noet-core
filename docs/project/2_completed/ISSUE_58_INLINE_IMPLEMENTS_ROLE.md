# Issue 58: Inline `{implements}` Role

> **STATUS: SUBSUMED BY ISSUE 71** — The unified codespan toggle design in Issue 71
> covers all inline relation role functionality and supersedes this issue. Mark as
> duplicate/closed. Do not implement separately.

**Priority**: MEDIUM
**Estimated Effort**: 0.5 days (RELATIVE COMPARISON ONLY)
**Dependencies**: Requires Issue 55 complete (MyST directive framework in place)

## Summary

Add an inline `{implements}` role using a code-span syntax: `` `{implements}[[Some Requirement]]` ``.
A single `Code` event whose content starts with `{implements}` is sub-parsed as Markdown,
all links extracted as `WeightKind::Pragmatic` relations, and the links rendered as normal
`<a class="noet-implements">` tags. This gives authors a compact, single-line citation form
without opening a full `{implements}` / `{end}` block.

## Goals

- `` `{implements}[[Requirement Title]]` `` records a `WeightKind::Pragmatic` relation and
  renders as `<a href="..." class="noet-implements">Requirement Title</a>`
- Full pulldown-cmark link handling on the sub-parsed content: wikilinks, standard Markdown
  links, and multiple links in one span all work
- Round-trip fidelity: `generate_source` preserves the original code span verbatim
- Warning diagnostic when content contains no recognisable link
- Warning diagnostic when an inline `{implements}` appears inside an open `{implements}` block
- Plain code spans and unknown `{name}` spans are unaffected

## Architecture

### Detection

In the `Code(content)` arm of `MdCodec::parse()`, call `myst::parse_directive_info(content)`.
If it returns `Some(("implements", rest))`, sub-parse `rest` with a fresh
`MdParser::new_with_broken_link_callback` (same options as the outer parse). Process the
resulting `Link` events exactly as the outer parse loop does — `link_to_relation` →
`WeightKind::Pragmatic` → push to `current.upstream`. Keep the original `Code` event and
its source range in `proto_events` unchanged (write-back fidelity via source-range splicing).

### Render

In `render_html_body`, detect `Code` events whose content starts with `{implements}`:
sub-parse the remainder, then for each link emit raw HTML instead of passing through the
`Code` event:

```
<a href="{resolved_url}" class="noet-implements">{link_text}</a>
```

The `href` is the resolved URL produced by `rewrite_md_links_to_html` logic (same
`.md` → `.html` rewriting already applied to normal links). Non-link content between links
in the sub-parse (plain `Text` events) is emitted as-is. If the sub-parse produces no
links, fall through to rendering the original `Code` span unchanged (matching the
warning-and-passthrough behaviour from parse time).

### `myst.rs` additions

```rust
pub fn parse_code_span(content: &str) -> Option<(&str, &str)>
```

Identical logic to `parse_directive_info` — strips `{name}` prefix, returns
`(name, rest)`. Kept as a separate function so the module doc comment can distinguish
block directives (fenced `CodeBlock` info strings) from inline roles (code span content).
Reuse `parse_directive_info` internally or duplicate the two-line body — either is fine.

A new `pub fn lookup_role(name: &str) -> bool` (or extend the existing `lookup` with a
role namespace) indicates whether a name is a recognised inline role. Initially only
`"implements"` returns `true`. Unknown role names emit `ParseDiagnostic::warning`.

### Interaction with block `{implements}`

The inline form is always point-in-time — it does not set or read `in_implements_block`.
If `in_implements_block` is already `true` when the inline role is encountered, emit:

```
ParseDiagnostic::warning("inline {implements} used inside an {implements} block — redundant")
```

Then process the inline role normally (record the Pragmatic edge, render the link with the
CSS class). The warning is the signal; it does not suppress the edge.

### Event shape summary

```
parse() loop sees:
  Code("{implements}[[Some Requirement]]")
  → parse_code_span → Some(("implements", "[[Some Requirement]]"))
  → sub-parse "[[Some Requirement]]" → Link event → WeightKind::Pragmatic
  → if in_implements_block → emit warning (redundant)
  → if no links found in sub-parse → emit warning + leave Code event in proto_events as-is
  → keep original Code event + source range in proto_events unchanged

render_html_body sees:
  Code("{implements}[[Some Requirement]]")
  → sub-parse remainder → links present
  → emit: <a href="some-requirement.html" class="noet-implements">Some Requirement</a>
  → Code event consumed; no <code> tag emitted

  Code("{implements}just plain text")   ← no links in sub-parse
  → fall through: emit <code>{implements}just plain text</code> unchanged

  Code("{unknown_role}content")
  → parse_code_span → Some(("unknown_role", "content"))
  → lookup_role("unknown_role") → false
  → fall through: emit <code>{unknown_role}content</code> unchanged (warning already at parse time)
```

### Round-trip

The original `Code` event's source range is preserved in `proto_events` unchanged.
`cmark_resume_with_source_range_and_options` splices the original bytes verbatim.
`` `{implements}[[Some Requirement]]` `` survives any number of parse-write cycles.

## Implementation Steps

1. **`myst.rs`: add `parse_code_span` and `lookup_role`** (0.1 days)
   - [ ] `pub fn parse_code_span(content: &str) -> Option<(&str, &str)>` — same logic as
         `parse_directive_info`; separate entry point for inline role detection
   - [ ] `pub fn lookup_role(name: &str) -> bool` — returns `true` for `"implements"`;
         `false` for all others (including unknown names)
   - [ ] Update module doc comment: add "Inline roles" section parallel to "Directives"
         section; document the `{name}content` code-span form and note the deliberate
         deviation from MyST spec (`{name}\`content\`` peek-back form)
   - [ ] Unit tests: `parse_code_span`, `lookup_role` (see Testing Requirements)

2. **`md.rs` `parse()`: detect inline role in `Code` arm** (0.2 days)
   - [ ] In the `MdEvent::Text | MdEvent::InlineHtml | MdEvent::Code` match arm, add a
         `Code`-specific branch: call `myst::parse_code_span(cow_str)`
   - [ ] If `Some(("implements", rest))`: sub-parse `rest`, extract links via
         `link_to_relation`, push `WeightKind::Pragmatic` relations to `current.upstream`
   - [ ] Emit `ParseDiagnostic::warning` when `in_implements_block` is true (redundant)
   - [ ] Emit `ParseDiagnostic::warning` when sub-parse yields no links
   - [ ] Emit `ParseDiagnostic::warning` for unknown role names (`lookup_role` returns false)
   - [ ] Keep original `Code` event + source range in `proto_events` unchanged in all cases
   - [ ] `cargo test` — no regressions

3. **`md.rs` `render_html_body`: emit `<a class="noet-implements">` for inline roles** (0.1 days)
   - [ ] In the event-substitution loop (after heading rewrite, before `push_html`), detect
         `Code` events whose content matches `parse_code_span` with a known role name
   - [ ] Sub-parse remainder, apply `rewrite_md_links_to_html` URL rewriting to each link
   - [ ] Emit raw `Html` event: `<a href="{url}" class="noet-implements">{text}</a>` per link;
         emit any inter-link `Text` events as plain `Html` text nodes
   - [ ] If no links in sub-parse: fall through, emit the original `Code` event unchanged
   - [ ] `cargo test` — no regressions

## Testing Requirements

- `myst::parse_code_span("{implements}[[Req]]")` → `Some(("implements", "[[Req]]"))`
- `myst::parse_code_span("{implements}")` → `Some(("implements", ""))` (empty rest)
- `myst::parse_code_span("rust")` → `None` (plain code span)
- `myst::parse_code_span("")` → `None`
- `myst::lookup_role("implements")` → `true`
- `myst::lookup_role("unknown")` → `false`
- Parse: `` `{implements}[[Req]]` `` in paragraph body → `WeightKind::Pragmatic` relation on `current.upstream`
- Parse: `` `{implements}[[A]] and [[B]]` `` → two `WeightKind::Pragmatic` relations
- Parse: `` `{implements}just text` `` → `WeightKind::Pragmatic` relation count = 0,
  warning diagnostic emitted, `Code` event preserved in `proto_events`
- Parse: inline role inside open `{implements}` block → warning diagnostic emitted,
  relation still recorded
- Parse: unknown role `` `{unknown}content` `` → warning diagnostic, no relation, `Code` event preserved
- Parse: plain `` `rust code` `` → no diagnostic, no relation, `Code` event unchanged
- Render: `` `{implements}[[Req]]` `` → HTML contains `<a` and `class="noet-implements"`
- Render: `` `{implements}just text` `` → HTML contains `<code>{implements}just text</code>`
- Round-trip: parse + `generate_source` preserves `` `{implements}[[Req]]` `` verbatim
- Round-trip: parse + `generate_source` preserves `` `{implements}just text` `` verbatim

## Success Criteria

- [ ] `` `{implements}[[Requirement Title]]` `` produces a `WeightKind::Pragmatic` relation
      and renders `<a href="..." class="noet-implements">Requirement Title</a>`
- [ ] Multiple links in one span: all recorded as Pragmatic, all rendered with CSS class
- [ ] No-link content: warning emitted, `<code>` span rendered unchanged
- [ ] Redundant inline inside open block: warning emitted, relation still recorded
- [ ] Round-trip fidelity: code span preserved verbatim across parse-write cycles
- [ ] All existing tests pass

## Risks

- **Risk**: the `Code` arm of `parse()` currently handles title accumulation for all three
  event types (`Text | InlineHtml | Code`) in one branch. Adding role detection for `Code`
  requires splitting this arm carefully to avoid breaking title accumulation.
  **Mitigation**: accumulation logic runs first (unchanged); role detection is an additive
  check on the same `cow_str` after accumulation. Covered by existing title-accumulation tests.

- **Risk**: sub-parsing `rest` in `render_html_body` runs a fresh `Parser::new_ext` on a
  short string — fine for correctness but called once per inline role per render. Not a
  performance concern in practice (documents have O(10s) of inline roles at most).
  **Mitigation**: no mitigation needed; document the sub-parse in the function comment.

## Open Questions

None — design fully resolved. See `docs/project/trades/TRADE_55_MYST_DIRECTIVE_SYNTAX.md`
Q5 for inline role detection experiments and the "Deferred Questions" section for the
decision to use code-span-internal `{name}content` form rather than the MyST peek-back form.

## References

- Issue 55: MyST Directive Syntax (`src/codec/myst.rs` extension point)
- `docs/project/trades/TRADE_55_MYST_DIRECTIVE_SYNTAX.md` — Q5 (inline role detection),
  Q6 (sub-parse pattern), Deferred Questions (inline role form decision)
- `src/codec/myst.rs` — `parse_directive_info`, `lookup`, `is_block_opener`
- `src/codec/md.rs` — `MdCodec::parse` (Code arm ~L1860), `render_html_body` (~L1178),
  `rewrite_md_links_to_html` (~L1179), `check_for_link_and_push` (~L350)
- `src/codec/network.rs` — `NetworkCodec` (inherits all `MdCodec` render behaviour via Deref)