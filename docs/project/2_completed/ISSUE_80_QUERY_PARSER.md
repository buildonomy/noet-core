# Issue 80: Query Parser — Textual Grammar for Surface Syntax

**Version**: 0.1
**Priority**: HIGH
**Estimated Effort**: 5 days (RELATIVE COMPARISON ONLY)
**Dependencies**: Requires Issue 79 (QuerySpec types) and Issue 83
(BeliefSource refactoring — clean `eval` API). Blocks Issue 81 (`{query}`
Directive).

## Summary

`query_model.md` §9.5 defines a single textual grammar shared across three
surfaces: viewer URLs (`?q=`), MyST directives (`{query}`), and MCP tool
arguments. This issue implements the recursive descent parser and serializer for
that grammar, producing `QuerySpec` instances (from Issue 79) from text strings
and serializing them back for URL round-tripping.

This is Phase 4b from Issue 70, pulled forward because the `{query}` directive
(Issue 81) needs it. The parser also enables the viewer's `?q=` URL parameter
(wired in this issue) and the MCP `query` tool to accept textual query
strings directly (wired in step 4b).

## Goals

- Implement a recursive descent parser in `src/query_parser.rs` that parses the
  §9.5 grammar into `QuerySpec`
- Implement a serializer that produces canonical query strings from `QuerySpec`
  (round-trip fidelity)
- Support the full grammar: anchors, NodeFilter expressions, Traversal
  expressions, pipeline (`THEN`), composition (`AND`/`OR`/`NOT`), sort/instrument
  suffixes (`| sort:...`, `| project:...`)
- Human-readable parse errors with position information
- URL-safe output (no characters requiring percent-encoding, except `%22` for
  quoted title anchors)

## Architecture

The parser follows the grammar defined in `query_model.md` §9.5.3–§9.5.8.

**Lexer**: Single-pass tokenization producing a token stream. Token types: `Word`,
`Kind` (section/epistemic/pragmatic), `RoleSigil` (s/k/o/n), `Dash`, `Paren`,
`Colon`, `Pipe`, `Star`, `QuestionMark`, `Arrow` (->/<-), `And`, `Or`, `Not`,
`Then`, `Fold`, `Terminal`, `Orphan`, `Anchor` (id://...), `QuotedTitle` ("..."),
`SortPrefix` (sort:), `ProjectPrefix` (project:), `Eof`. The keywords `THEN`,
`FOLD`, `TERMINAL`, `ORPHAN` are all pipeline operator tokens (CONST_CASE).

**Parser**: Recursive descent, single-token lookahead, no backtracking.
Productions:

```
query           = pipeline (composition_op pipeline)*
pipeline        = [anchor] stage (TAPE_FN? stage)*
TAPE_FN         = 'THEN' | 'FOLD' | 'TERMINAL' | 'ORPHAN'
stage           = traversal | node_filter
traversal       = full_traversal | shorthand_traversal
full_traversal  = INPUT_ROLES '-' KIND_SET '-' OUTPUT_ROLES ['(' DEPTH ')'] ['?']
shorthand       = ARROW KIND ['(' DEPTH ')']
node_filter     = or_expr
suffix          = '|' (sort_spec | project_spec)
```

Stage boundary detection: a role sigil (`s`/`k`/`o`/`n`) followed by `-` opens a
traversal; `->` or `<-` opens a shorthand; all other tokens enter NodeFilter
parsing.

**NodeFilter predicate syntax**: NodeFilter expressions use a unified property
predicate model (Issue 79's `PropertyPredicate`). Two forms:

- **Property predicates** (hard boolean): `path op value` syntax.
  `schema == "procedure"`, `kind in {Document, Symbol}`,
  `payload.status == "open"`, `payload.priority > 3`,
  `metadata.git.branch exists`. Dotted paths follow TOML key syntax.
  Operators: `==`, `!=`, `in`, `matches`, `contains`, `exists`, `>`, `<`,
  `>=`, `<=`.
- **Text search** (soft scored): `path:term` colon syntax (no operator).
  `title:authentication`, `content:oauth`. Produces TF-IDF score.

The colon form is sugar for `TextMatch`; the operator form is a
`PropertyPredicate`. Both compose with `AND`/`OR`/`NOT`.

**Subject from anchor**: When no `id://` or quoted title anchor appears, the
parser produces `Subject::Implicit`. Callers are responsible for resolving
`Implicit` before evaluation — the directive (Issue 81) injects the current
document's BID; the viewer injects the SPA route; MCP tools may reject it as an
error. This keeps the parser context-free.

**Suffix → partial `InstrumentConfig`**: The `| sort:` and `| project:` suffixes
populate fields of `InstrumentConfig`. The parser sets only the fields present in
the suffix; unspecified fields use `InstrumentConfig::default()`. When a query
string is used inside a `{query}` directive (Issue 81), the directive's options
(`:columns:`, `:sort:`, `:render:`) take precedence — the directive merges its
options over the parsed suffix. The precedence rule: **directive options override
query string suffixes** (more specific context wins). The `| project:` suffix
maps to `params` entries in `InstrumentConfig`: `project:edge_count` →
`params.display = "edge_count"`, `project:o-k` → `params.display = "maps_to"`.

**Serializer**: `QuerySpec` → canonical string. The round-trip invariant:
`parse(serialize(spec)) == spec` for all valid `QuerySpec` values. Named
shorthands are expanded to full form during parsing and re-emitted as shorthands
during serialization when the full form matches a known pattern.

**Parse errors**: Each error carries a byte offset and a human-readable message.
The parser does not attempt recovery — it fails fast on the first error. Error
messages include the problematic token and what was expected.

## Implementation Steps

1. Lexer (0.5 day)
   - [ ] Create `src/query_parser.rs`
   - [ ] Implement `Token` enum and `Lexer` struct
   - [ ] Handle whitespace skipping, keyword detection (AND/OR/NOT/THEN/FOLD/
        TERMINAL/ORPHAN are uppercase-only; lowercase triggers a helpful error,
        not silent fallback)
   - [ ] Handle `id://` anchor prefix detection
   - [ ] Handle quoted title strings (`"..."`)
   - [ ] Handle `->` and `<-` arrow tokens
   - [ ] Handle `|` pipe for suffix separation
   - [ ] Unit tests for lexer

2. Parser (1.5 days)
   - [ ] Implement `parse(input: &str) -> Result<QuerySpec, ParseError>`
   - [ ] Implement anchor parsing: `id://bref` → `Subject::Anchor`,
     `"Quoted Title"` → `Subject::Anchor`, absent → `Subject::Implicit`
   - [ ] Implement NodeFilter property predicate parsing:
     `path op value` → `PropertyPredicate` (Issue 79).
     Operators: `==`, `!=`, `in`, `matches`, `contains`, `exists`,
     `>`, `<`, `>=`, `<=`. Dotted paths for nested keys.
   - [ ] Implement NodeFilter text search parsing: `path:term` (colon
     syntax) → `TextMatch`. Bare words → `TextMatch` on content.
   - [ ] Implement NodeFilter boolean composition: AND/OR/NOT, parentheses
   - [ ] Implement Traversal expression parsing: full form
     `s-pragmatic-k(1)`, existence test `?`
   - [ ] Implement shorthand expansion: `->section(N)` → `s-section-k(N)`,
     `<-pragmatic(N)` → `k-pragmatic-s(N)`,
     `->owner(N)` → `o-pragmatic-k(N)`
   - [ ] Implement pipeline parsing: sequential stages with
      `TapeFn` variants (`Then`/`Fold`/`Terminal`/`Orphan` with
      optional `StepRef` range scoping). Surface keywords are
      CONST_CASE: `THEN`, `FOLD`, `TERMINAL`, `ORPHAN` (see
      `query_model.md` §5 and §9.5.6). Each stage produces a
      `ProjectionStep` with `label: String` (defaults to step
      index `"0"`, `"1"`, …)
   - [ ] Implement composition parsing: `AND`/`OR`/`NOT` between pipelines
   - [ ] Implement suffix parsing: `| sort:field` → `SortSpec::from_str()`
     (Issue 79), `| project:edge_count` → `params.display = "edge_count"`
   - [ ] Parse error type with byte offset and message

3. Serializer (0.5 day)
   - [ ] Implement `serialize(spec: &QuerySpec) -> String`
   - [ ] Canonical form: emit shorthands when the full form matches a known
     pattern, full form otherwise
   - [ ] `Subject::Implicit` serializes as empty (no anchor prefix)
   - [ ] URL-safe output validation

4. Viewer `?q=` URL integration (0.5 day)
   - [ ] Wire the parser into `traceability.js`: on page load, if `?q=` is
     present, parse it via `parse()` and use the resulting `QuerySpec` to
     set control state (`buildQuerySpec()` from Issue 79 Step 5 in reverse)
   - [ ] On control state change, serialize the current `QuerySpec` via
     `serialize()` and update `?q=` via `history.replaceState`
   - [ ] When `?q=` is present on load, auto-open the traceability panel
   - [ ] `Subject::Implicit` in `?q=` resolves to the current SPA route
     document (the `#/path` fragment)
   - [ ] `localStorage` stores the raw query string as fallback; `?q=`
     takes precedence on reload

4b. MCP `query` tool: query-string input (0.5 day)
   - [ ] Add an optional `query_string: Option<String>` field to `QueryInput`
     in `src/mcp/types.rs`. When present, parse it via `parse()` to produce
     a `QuerySpec`; when absent, fall back to deserializing `expression` as
     `QuerySpec` JSON (current behavior).
   - [ ] Update `tools::query()` to check `query_string` first:
     if `Some(qs)` → `parse(&qs)?` → `QuerySpec`;
     else → `serde_json::from_value(input.expression)?`.
     Parse errors produce `McpError::invalid_params` with the parse error
     message (including byte offset).
   - [ ] Update MCP tool description in `mod.rs` to mention both input
     modes: structured `expression` JSON and textual `query_string`.
   - [ ] Update `noet://docs/orientation` resource if it references the
     query tool schema.
   - [ ] Test: round-trip a few representative query strings through
     the MCP tool (parse → evaluate → verify result count).

5. User-facing query language reference (0.5 day)
   - [ ] Create `docs/design/query_language.md` — the authoring reference
     that directive users and MCP agents actually read. NOT the formal
     model (`query_model.md`) — that's the design spec. This is the
     practical guide with examples-first structure.
   - [ ] Structure: quick-start (3 examples covering the common cases),
     then reference sections for each construct
   - [ ] Document property predicates with examples:
     `schema == "procedure"`, `kind in {Document, Symbol}`,
     `payload.status == "open"`, `payload.priority > 3`,
     `metadata.git.branch exists`, `payload.* contains "auth"`
   - [ ] Document text search: `title:authentication`, bare words,
     implicit OR between terms
   - [ ] Document traversals: full form `s-pragmatic-k(1)`, shorthands
     `->section(N)`, `<-pragmatic(N)`, `->owner(N)`, existence test `?`
   - [ ] Document composition: `AND`, `OR`, `NOT` between pipelines,
     with gap analysis example
   - [ ] Document anchors: `id://node-id`, `"Quoted Title"`, implicit
     (current document in directives, SPA route in viewer)
   - [ ] Document `{query}` directive syntax with all options
     (`:columns:`, `:sort:`, `:render:`, `:max-rows:`, `:caption:`)
   - [ ] Document `| sort:` and `| project:` suffixes
   - [ ] Include a "cookbook" section with 5-6 real-world patterns:
     "show my linked requirements", "find all documents with schema X",
     "gap analysis: what's not covered", "cross-network edges",
     "filter by payload field"
   - [ ] Cross-reference from `myst_directive_architecture.md` and
     `network_authoring.md`

6. Tests (0.5 day)
   - [ ] Round-trip: `parse(serialize(parse(input))) == parse(input)` for all
     grammar examples from §9.5
   - [ ] Parse each example from §9.5.3 (NodeFilter), §9.5.4 (Traversal),
     §9.5.6 (Full Query)
   - [ ] Verify named shorthand expansion: `->pragmatic(1)` parses to same
     `QuerySpec` as `s-pragmatic-k(1)`
   - [ ] Verify `Subject::Implicit` when no anchor present
   - [ ] Verify `| project:edge_count` maps to `params.display = "edge_count"`
   - [ ] Verify `| sort:tfidf` maps to correct `SortSpec`
   - [ ] `->roots(*)`/`->leaves(*)` shorthands: expand to traversal
     + `TapeFn::Terminal` on the following stage (see
     `query_model.md` §5 `TapeFn`, §6.2 `terminal_bids`,
     §9.5.4 provisional note)
   - [ ] Error cases: `s-s-s` (degenerate self-loop rejected), unknown kind,
     malformed depth, unclosed paren
   - [ ] Error cases: lowercase `and` produces helpful error ("did you mean
     AND?")
   - [ ] Edge cases: leading `NOT` without positive term, bare `|` without
     suffix, empty input

## Testing Requirements

- Parser tests are pure (no BeliefBase needed — parsing produces `QuerySpec`,
  not evaluated results)
- Round-trip fidelity is the primary correctness criterion
- All examples from `query_model.md` §9.5 must parse successfully
- Error messages must include byte offset and be human-readable

## Success Criteria

- [ ] All §9.5 grammar examples parse correctly
- [ ] `parse(serialize(spec)) == spec` for all valid inputs (round-trip)
- [ ] Degenerate expressions (`s-*-s`) produce clear parse errors
- [ ] Parser handles implicit `OR` between bare words: `foo bar` ≡ `foo OR bar`
- [ ] Property predicates parse: `schema == "procedure"`, `kind in {Document}`,
  `payload.status == "open"`, `metadata.git.branch exists`
- [ ] Named shorthands expand correctly
- [ ] `Subject::Implicit` produced when no anchor present
- [ ] Suffix parsing produces correct `InstrumentConfig` fields
  (`SortSpec`, `params`)
- [ ] No percent-encoding needed in output except `%22` for quoted titles
- [ ] Viewer `?q=` URL round-trips: control state → serialize → URL → parse
  → same control state

## Risks

- **Grammar ambiguity at stage boundaries**: A word like `section` could be a
  NodeFilter text term or the start of a traversal kind. → **Mitigation**: Stage
  boundary detection is well-defined in §9.5.8 — role sigils followed by `-` open
  traversals; everything else is NodeFilter.
- **`THEN`/`FOLD` vs juxtaposition**: Omitting the operator (or using `THEN`)
  produces `TapeFn::Then`; `FOLD` produces `TapeFn::Fold`.
  The parser must auto-detect stage boundaries between adjacent stages.
  → **Mitigation**: The lookahead rule (role sigil + `-` = traversal) handles this.

## Open Questions

- Should the canonical serialized form prefer shorthands (`->pragmatic(1)`) or
  full form (`s-pragmatic-k(1)`)? Recommend: shorthands when pattern matches.
- Should the parser accept case-insensitive keywords (`and`/`And`/`AND`)?
  Recommend: strict uppercase per spec, with helpful error for lowercase.

## Implementation Notes

- **`EnumSet<Role>` ↔ sigil string**: Issue 83 migrated `TraversalSpec`
  role fields from `BTreeSet<Role>` to `EnumSet<Role>`. The 3-bit set maps
  directly to the `sko` sigil syntax in the grammar: `Role::Source` → `s`,
  `Role::Sink` → `k`, `Role::Owner` → `o`. The serializer iterates the
  set and emits one character per role; the parser `|`-folds characters
  into an `EnumSet<Role>`. No intermediate representation needed.

## References

- `docs/design/query_model.md` §9.5 — the full grammar specification
- `docs/design/query_model.md` §9.5.8 — parser rules
- `docs/design/query_model.md` §9.5.9 — surface bindings (URL, MyST, MCP)
- Issue 79 — `QuerySpec`, `Subject`, `InstrumentConfig`, `SortSpec` types
- Issue 81 — `{query}` directive that consumes parsed queries (defines
  precedence: directive options override query string suffixes)
- Issue 70 (completed/OBE) — original unified query UI issue
- Issue 82 — viewer query UI enhancements (search mode, graph, Explore)
