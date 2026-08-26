# Issue 55: MyST Directive Syntax for Noet Extensions

**Priority**: MEDIUM
**Estimated Effort**: 1 day
**Dependencies**: None (orthogonal to active issues; should be done before Issue 7 testing pass)

## Summary

Replace the opaque `<!-- network-children -->` HTML comment marker with the standard MyST
backtick-fence directive `` ````{network_children} ``. This establishes `src/codec/myst.rs`
as the single documented extension point for all future noet directives, implemented by
intercepting `CodeBlock(Fenced("{name}"))` events in the existing `MdCodec::parse()` loop —
the same pattern already used for `MetadataBlock` and `Heading` processing. The colon-fence
(`:::`) form is not supported.

## Goals

- Author-facing syntax is standard MyST backtick-fence: `` ````{network_children} `` instead of a raw HTML comment
- Old `<!-- network-children -->` in existing files continues to work (backward compat)
- `noet init` and `noet init --children-marker` write the MyST backtick-fence form
- `src/codec/myst.rs` exists as the single, documented extension point for all future noet directives
- Round-trip fidelity: a file containing `` ````{network_children} `` that is parsed and
  written back by `generate_source` preserves the original MyST syntax unchanged
- No change to `NETWORK_CHILDREN_SENTINEL` or `generate_deferred_html`

## Architecture

### Correct MyST syntax for noet

The [MyST spec](https://mystmd.org/guide/syntax-overview) defines two fence forms for
block directives. noet uses the **backtick-fence** form exclusively:

````
````{network_children}
````
````

The colon-fence form (`:::{name}\n:::`) is **not supported**. It is fatally broken under
`pulldown-cmark` with `ENABLE_DEFINITION_LIST`: the serialiser corrupts the closing `:::` to
`: ::` on every write-back, and a blank line in the body terminates the underlying
`DefinitionList` structure entirely, making body directives impossible to detect as a
single parse unit. See `docs/project/trades/TRADE_55_MYST_DIRECTIVE_SYNTAX.md` for the
full empirical analysis.

**Authoring convention**: use 4 backticks for top-level directives. A 3-backtick directive
will be normalised to 4 on the first write-back and is then stable. Nested directives use 3
backticks inside a 4-backtick outer fence (the only stable nesting depth).

### How pulldown-cmark parses `` ````{name} ``

With `Options::all()` (a superset of `buildonomy_md_options()`), pulldown-cmark treats any
fenced code block whose info string matches `{...}` as an ordinary fenced code block —
it does not parse it as a directive. The structure is clean and unambiguous:

```
Input: "````{network_children}\n````\n"

Events:
  Start(CodeBlock(Fenced("{network_children}")))
  End(CodeBlock)                               ← no Text event: zero-body
```

For a directive with body content:

```
Input: "````{note}\nHere is a note!\n````\n"

Events:
  Start(CodeBlock(Fenced("{note}")))
    Text("Here is a note!\n")
  End(CodeBlock)
```

The info string `{name}` is preserved verbatim. Name extraction: strip the leading `{`
and trailing `}`, optionally splitting on the first space for arguments (e.g.
`{figure} image.png` → name `figure`, arg `image.png`).

### Event-interception approach

Intercept `CodeBlock` events in the `MdCodec::parse()` loop. When the `Fenced` info string
matches `{...}`, extract the name and call `myst::lookup`. Keep the **original** events
with their source ranges in `proto_events` unchanged.

Because `cmark_resume_with_source_range_and_options` uses `Option<Range<usize>>` offsets to
splice original source bytes (rather than re-serialising events), preserving the original
events + ranges gives exact write-back fidelity at zero extra cost.

The `→ NETWORK_CHILDREN_MARKER` substitution happens at **render time** in
`render_html_body` / `NetworkCodec::generate_html`, not at parse time — identical to how
heading rewriting is deferred to `render_html_body`.

```
parse() loop:
  Start(CodeBlock(Fenced("{network_children}"))) ?
    → info string starts with "{" and ends with "}" (possibly with args)
    → extract name: "network_children"
    → myst::lookup("network_children") → Some(NETWORK_CHILDREN_MARKER)
    → record directive on IRNode; keep original CodeBlock events in proto_events as-is
    → myst::lookup("unknown_foo") → None
    → emit ParseDiagnostic::warning("unknown noet directive: {unknown_foo}")
    → keep original CodeBlock events in proto_events unchanged (graceful passthrough)

render_html_body / NetworkCodec::generate_html:
  CodeBlock(Fenced("{network_children}")) → emit Html(NETWORK_CHILDREN_MARKER)
  (NetworkCodec::generate_html already scans the HTML string for NETWORK_CHILDREN_MARKER)
```

### Module layout

```
src/codec/
  myst.rs       NEW — directive registry: lookup(), directive name constants
  md.rs         MODIFIED — intercept CodeBlock(Fenced("{...}")) events in parse() loop
  network.rs    MODIFIED — add NETWORK_CHILDREN_DIRECTIVE const, update create_network_file,
                           update render step to substitute CodeBlock → Html marker
```

### Constants after this change

| Constant | Value | Visibility | Purpose |
|---|---|---|---|
| `NETWORK_CHILDREN_DIRECTIVE` | `"````{network_children}"` | `pub` | Author-facing directive name (opening line) |
| `NETWORK_CHILDREN_MARKER` | `"<!-- network-children -->"` | `pub` | Internal intermediate emitted at render time |
| `NETWORK_CHILDREN_SENTINEL` | `"<!--@@noet-network-children@@-->"` | `pub` | Two-phase write placeholder (unchanged) |

`NETWORK_CHILDREN_MARKER` is preserved as the internal render-time signal and as the
backward-compat path for existing files using the old HTML comment syntax.

## Implementation Steps

### Step 1: Create `src/codec/myst.rs` (0.25 days)
- [ ] New file with module-level doc comment explaining the `CodeBlock` interception approach,
      why colon-fence is rejected (round-trip corruption, body limitation), and the
      4-backtick authoring convention
- [ ] `pub fn lookup(directive_name: &str) -> Option<&'static str>` — maps a known directive
      name (e.g. `"network_children"`) to its internal marker string; returns `None` for
      unknown directives
- [ ] `pub fn parse_directive_info(info: &str) -> Option<(&str, &str)>` — given a `Fenced`
      info string, returns `(name, args)` if it matches `{name}` or `{name} args`; returns
      `None` if the string does not start with `{` or has no closing `}`
- [ ] Commented-out placeholder entries in `lookup` for anticipated future directives
      (e.g. `// "toc" => TOC_MARKER,  // TODO(Issue N)`), making the extension point
      concrete for the next contributor
- [ ] Re-export from `src/codec/mod.rs`
- [ ] Unit tests in `myst.rs`:
  - `lookup("network_children")` → `Some(NETWORK_CHILDREN_MARKER)`
  - `lookup("unknown")` → `None`
  - `lookup("")` → `None`
  - `parse_directive_info("{network_children}")` → `Some(("network_children", ""))`
  - `parse_directive_info("{figure} image.png")` → `Some(("figure", "image.png"))`
  - `parse_directive_info("rust")` → `None` (plain code block, not a directive)
  - `parse_directive_info("")` → `None`

### Step 2: Intercept CodeBlock events in `MdCodec::parse` (0.5 days)
- [ ] In the `match event.borrow()` arm of `parse()`, add handling for
      `MdEvent::Start(MdTag::CodeBlock(MdCodeBlockKind::Fenced(info)))`:
      call `myst::parse_directive_info(info)` to test whether this is a directive
- [ ] If `parse_directive_info` returns `Some((name, _args))`:
  - Call `myst::lookup(name)`:
    - Known directive → push the original `Start(CodeBlock(...))`, any body `Text`, and
      `End(CodeBlock)` events (with source ranges) to `proto_events` as-is; also record
      the directive name on the current `IRNode` so `NetworkCodec` can act on it at render time
    - Unknown directive → emit `ParseDiagnostic::warning("unknown noet directive: {name}")`;
      push original events to `proto_events` unchanged (graceful passthrough)
- [ ] If `parse_directive_info` returns `None`: push events to `proto_events` unchanged
      (ordinary code block, no action)
- [ ] Verify existing unit tests still pass: `cargo test`

### Step 3: Substitute directive at render time in `NetworkCodec` (0.1 days)
- [ ] In `render_html_body` (or in `NetworkCodec::generate_html` for the
      `NetworkCodec`-specific path), when iterating `proto_events`, detect a
      `CodeBlock(Fenced("{network_children}"))` event and emit
      `MdEvent::Html(NETWORK_CHILDREN_MARKER)` in its place before calling `push_html`;
      the existing sentinel-injection logic downstream is then unchanged
- [ ] Alternatively (simpler): in `render_html_body`, replace the `CodeBlock` directive
      events with the `Html` marker before calling `push_html`; `NetworkCodec::generate_html`
      inherits this via the shared render path

### Step 4: Update `network.rs` and `compiler.rs` (0.1 days)
- [ ] Add `pub const NETWORK_CHILDREN_DIRECTIVE: &str = "````{network_children}";` to
      `network.rs` (the opening-line form written to source files)
- [ ] Update `DocumentCompiler::create_network_file`: write
      `"````{network_children}\n````\n"` when `insert_children_marker` is true
- [ ] Update `main.rs` CLI `--children-marker` / `--no-children-marker` help text

### Step 5: Update test fixtures and docs (0.1 days)
- [ ] `tests/network_1/index.md` — replace `<!-- network-children -->` with
      ```` ````{network_children}\n```` ````
- [ ] `tests/network_1/subnet1/index.md` — same
- [ ] Update `network.rs` unit tests that use `<!-- network-children -->` as source input
      to use the backtick-fence form (tests exercising the full parse path); tests that
      directly exercise `NETWORK_CHILDREN_MARKER` string handling may keep it
- [ ] Update doc comments in `network.rs` that show the author-facing syntax

## Testing Requirements

- `myst::lookup` and `myst::parse_directive_info` unit tests (see Step 1)
- `MdCodec` integration: a document containing `` ````{network_children}\n```` `` produces
  HTML with `NETWORK_CHILDREN_SENTINEL` at the correct position
- Round-trip: a document containing `` ````{network_children}\n```` `` that is parsed and
  written back by `generate_source` preserves `` ````{network_children}\n```` `` unchanged
- Backward compat: a document containing `<!-- network-children -->` still produces HTML
  with `NETWORK_CHILDREN_SENTINEL` (no regression for existing files)
- Unknown directive `` ````{foo}\n```` `` passes through unchanged and emits a warning diagnostic
- Plain fenced code block (e.g. ` ```rust\n...\n``` `) is unaffected — no diagnostic, no substitution
- `create_network_file` with `insert_children_marker = true` writes `` ````{network_children}\n```` ``
- `noet parse --html-output` on `tests/network_1` produces `pages/index.html` with the
  sentinel replaced by child links (Bug 3 must also be fixed for this to pass end-to-end)

## Success Criteria

- [ ] `` ````{network_children}\n```` `` in source produces correct child listing in HTML output
- [ ] `<!-- network-children -->` in source still works (backward compat, no migration required)
- [ ] `noet init --children-marker` writes `` ````{network_children}\n```` `` to `index.md`
- [ ] Round-trip: parse + `generate_source` preserves `` ````{network_children}\n```` `` verbatim
- [ ] `src/codec/myst.rs` exists with documented directive registry
- [ ] All existing tests pass
- [ ] No change to `NETWORK_CHILDREN_SENTINEL` or `generate_deferred_html` logic

## Risks

- **Risk**: `CodeBlock` interception in the parse loop fires on legitimate fenced code
  blocks whose info string happens to match `{...}` (e.g. a hypothetical `` ```{json} ``
  used as a language tag by some editor).
  **Mitigation**: this is precisely the MyST authoring convention — `{name}` info strings
  are the directive syntax. Emit a diagnostic for unknown names so authors can identify
  unexpected matches. Plain language tags (`rust`, `python`, `json` without braces) are
  unaffected.

- **Risk**: pulldown-cmark upstream changes how `Fenced` info strings are parsed or
  represented.
  **Mitigation**: the detection is confined to `myst::parse_directive_info`. A regression
  surfaces immediately in the round-trip and integration tests added here.

- **Risk**: interaction between `CodeBlock` interception and link accumulation or other
  parse-loop state.
  **Mitigation**: `CodeBlock` events do not nest inside `Heading` or `MetadataBlock`
  contexts; the interception is a single-event check with no buffering required for
  zero-body directives. Body text events pass through the existing accumulation logic
  unchanged.

## Open Questions

None — all architectural questions resolved by empirical experiment. See trade study.

## References

- Trade study: `docs/project/trades/TRADE_55_MYST_DIRECTIVE_SYNTAX.md` — full empirical
  analysis including colon-fence disqualification, backtick-fence round-trip stability,
  nesting behaviour, and role peek-back safety
- MyST syntax overview: https://mystmd.org/guide/syntax-overview
- `src/codec/network.rs` — `NETWORK_CHILDREN_MARKER`, `NETWORK_CHILDREN_SENTINEL`,
  `generate_html`, `generate_deferred_html`
- `src/codec/md.rs` — `MdCodec::parse`, `events_to_text`, `render_html_body`
- `src/codec/compiler.rs` — `create_network_file`, `DocumentCompiler`
- `src/bin/noet/main.rs` — `--children-marker` / `--no-children-marker` CLI flags
- Issue 7: Comprehensive Testing (this issue should be complete before the testing pass)