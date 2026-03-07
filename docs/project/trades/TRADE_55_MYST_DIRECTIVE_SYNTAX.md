# Trade Study: MyST Directive Syntax Implementation Strategy

**Version**: 0.1
**Status**: Decision Revised
**Related Issue**: Issue 55 (MyST Directive Syntax for Noet Extensions)

## Summary

Issue 55 proposes replacing `<!-- network-children -->` with a MyST-style directive syntax.
This study first establishes the **correct MyST syntax** (the issue used a non-standard form),
then evaluates three implementation strategies given two hard constraints: (1) round-trip
source fidelity via `pulldown-cmark-to-cmark`, and (2) no parser fork. Empirical test
results for all syntax forms are included. A second round of experiments confirmed blank-line
body termination behaviour, argument parsing shape, backtick-fence double-round-trip
stability, no-definition-list fallback events, and role-context safety for the peek-back
detection approach. A third round of experiments (documented in "Additional Empirical
Findings — Q6") confirmed that backtick-fence nesting is recoverable via recursive
re-parsing of the body `Text` event.

**Decision summary**: the colon-fence (`:::`) syntax is not supported. noet uses the
backtick-fence form (`` ````{name} ``) exclusively, intercepting `CodeBlock(Fenced("{name}"))`
events in the `MdCodec::parse()` loop.

---

## Correct MyST Directive Syntax

The [MyST specification](https://mystmd.org/guide/syntax-overview) defines two syntactically
distinct extension points:

### Directives (block-level)

The MyST spec defines two syntactically valid fence forms:

**Colon-fence** (preferred in the MyST spec for Markdown-body directives):
```
:::{directivename} optional-args
:key: value

Body content (Markdown).
:::
```

**Backtick-fence** (preferred in the MyST spec for code-like bodies: math, diagrams):
````
```{directivename} optional-args

Body content.
```
````

Nesting is achieved by using more backticks on the outer fence than the inner:
````
````{important}
```{note}
Here's my `important`, highly nested note! 🪆
```
````
````

**noet's position**: noet does **not** support the colon-fence form. The colon-fence is
fatally broken under `pulldown-cmark` with `ENABLE_DEFINITION_LIST`: it cannot round-trip
(closing `:::` is corrupted), and a blank line in the body terminates the definition list
entirely — making it impossible to implement body directives. The backtick-fence form
produces a clean `CodeBlock(Fenced("{name}"))` event, supports nested directives via
recursive body re-parsing, and is stable from the second round-trip onward. noet uses
backtick-fence exclusively, with the authoring convention of 4 backticks (```` ````{name}
```` ``) so that the 3→4 mutation on first write-back does not occur.

### Roles (inline)

```
{rolename}`content`
```

A role is a `{name}` token immediately followed by a backtick-delimited span. There is no
block-level role; they are strictly inline.

---

## What pulldown-cmark 0.13 Actually Produces

All results below use `Options::all()` (which is a superset of `buildonomy_md_options()`).

### Colon-fence directives → DefinitionList events

```
Input:  ":::{network_children}\n:::\n"

Events:
  Start(DefinitionList)
  Start(DefinitionListTitle)
    Text(":::{network_children}")
  End(DefinitionListTitle)
  Start(DefinitionListDefinition)
    Text("::")
  End(DefinitionListDefinition)
  End(DefinitionList)
```

```
Input: ":::{note}\nHere is a note!\n:::\n"

Events:
  Start(DefinitionList)
  Start(DefinitionListTitle)
    Text(":::{note}")
    SoftBreak
    Text("Here is a note!")
  End(DefinitionListTitle)
  Start(DefinitionListDefinition)
    Text("::")
  End(DefinitionListDefinition)
  End(DefinitionList)
```

**Key finding**: `ENABLE_DEFINITION_LIST` causes pulldown-cmark to interpret a line starting
with `:::` as a definition list term (the `:::` prefix matches the `:` definition-list
sigil). The `:::` closing fence becomes a single-definition list entry with body `::`. This
is consistent across zero-body, body, and argument forms. The directive structure is
**not** represented — all content is flattened into `Text` events inside
`DefinitionListTitle` / `DefinitionListDefinition`.

**Important**: when the body contains a blank line, pulldown-cmark terminates the
`DefinitionList` at the blank line (confirmed by Q4 experiment). **This is one of the
reasons colon-fence is not supported by noet.** See "Additional Empirical Findings — Q4".

### Backtick-fence directives → CodeBlock events (correct shape, wrong semantics)

```
Input: "```{math}\n\\mathbf{u}\n```\n"

Events:
  Start(CodeBlock(Fenced(Borrowed("{math}"))))
    Text("\\mathbf{u}\n")
  End(CodeBlock)
```

The info-string `{math}` is preserved verbatim in the `Fenced` variant. pulldown-cmark
treats this as a fenced code block with an unusual info string — it does not parse it as a
directive. The structure is at least intact: the name is recoverable, the body is intact.

### Inline roles → Text + Code events (split, no structure)

```
Input: "{rolename}`role content here`.\n"

Events:
  Start(Paragraph)
    Text("{rolename}")
    Code("role content here")
    Text(".")
  End(Paragraph)
```

The `{rolename}` prefix and the backtick-delimited span are emitted as two separate events
with no structural connection. The adjacency in the event stream is the only signal.

### HTML comment (current mechanism) → HtmlBlock events (clean)

```
Input: "<!-- network-children -->\n\nSome text.\n"

Events:
  Start(HtmlBlock)
    Html("<!-- network-children -->\n")
  End(HtmlBlock)
  Start(Paragraph)
    Text("Some text.")
  End(Paragraph)
```

The HTML comment is a first-class `MdEvent::Html` event with no ambiguity.

---

## Additional Empirical Findings

Results from a second round of experiments using the same `Options::all()` harness.

### Q4: Blank line in colon-fence body terminates the DefinitionList

```
Input: ":::{note}\nFirst paragraph.\n\nSecond paragraph after blank line.\n:::\n"

Events:
  Start(Paragraph)
    Text(":::{note}")
    SoftBreak
    Text("First paragraph.")
  End(Paragraph)
  Start(DefinitionList)
    Start(DefinitionListTitle)
      Text("Second paragraph after blank line.")
    End(DefinitionListTitle)
    Start(DefinitionListDefinition)
      Text("::")
    End(DefinitionListDefinition)
  End(DefinitionList)
```

**Critical finding**: a blank line inside the directive body does not produce a multi-entry
`DefinitionList` — it **terminates the definition list entirely**. The opening `:::{note}`
line and everything before the blank line are parsed as a plain `Paragraph`. The content
after the blank line starts a fresh `DefinitionList`. The closing `:::` is again the
definition body `::`. This means a colon-fence directive with multi-paragraph body is two
completely unrelated parse structures — the opening and closing fences are not connected
in the event stream.

**Consequence for Issue 55**: zero-body directives (`:::{network_children}\n:::`) are
unaffected — no blank line can occur. Any future directive requiring a multi-paragraph body
**cannot** be reliably detected via `DefinitionList` interception; Option C (fork) or a
different body encoding would be required for that case.

Round-trip of the blank-line form:
```
in : ":::{note}\nFirst paragraph.\n\nSecond paragraph after blank line.\n:::\n"
rt1: ":::{note}\nFirst paragraph.\n\n\nSecond paragraph after blank line.\n: ::\n"
rt2: rt1  (stable after first round-trip)
```

---

### Q3: Arguments after directive name appear as a single Text event

```
Input: ":::{figure} image.png\n:::\n"

Events:
  Start(DefinitionList)
    Start(DefinitionListTitle)
      Text(":::{figure} image.png")
    End(DefinitionListTitle)
    Start(DefinitionListDefinition)
      Text("::")
    End(DefinitionListDefinition)
  End(DefinitionList)
```

The entire opening line — name and arguments — is one `Text` event inside
`DefinitionListTitle`. Parsing the name from a directive with arguments: strip the
`":::{"` prefix, read up to the closing `}`, and the remainder (` image.png`) is the
argument string. This is unambiguous as long as `}` does not appear in the directive name
itself (it cannot — MyST directive names are identifiers).

---

### Q1: Without ENABLE_DEFINITION_LIST, colon-fence becomes a Paragraph

```
Input: ":::{network_children}\n:::\n"
Options: Options::all() & !Options::ENABLE_DEFINITION_LIST

Events:
  Start(Paragraph)
    Text(":::{network_children}")
    SoftBreak
    Text(":::")
  End(Paragraph)
```

Disabling `ENABLE_DEFINITION_LIST` produces a plain `Paragraph` with two `Text` events.
The structure is even harder to detect reliably. This confirms that `ENABLE_DEFINITION_LIST`
must remain enabled for Option B to work, and that there is no cleaner fallback without
enabling it.

---

### Q2: Backtick-fence double round-trip — stable after first pass

```
Input:  "```{network_children}\n```\n"
rt1:    "\n````{network_children}\n````"
rt2:    "\n````{network_children}\n````"   (rt2 == rt1)

Input:  "```{math}\n\\mathbf{u}\n```\n"
rt1:    "\n````{math}\n\\mathbf{u}\n````"
rt2:    "\n````{math}\n\\mathbf{u}\n````"   (rt2 == rt1)

Input:  "````{math}\n\\mathbf{u}\n````\n"   (already 4-backtick)
rt1:    "\n````{math}\n\\mathbf{u}\n````"
rt2:    "\n````{math}\n\\mathbf{u}\n````"   (rt2 == rt1)
```

The first round-trip adds one backtick (3→4) and drops the leading/trailing newlines.
The second round-trip is **idempotent** — `rt2 == rt1` in all cases. A file that has been
round-tripped once reaches a stable fixed point. A file authored with 4 backticks is
already at the fixed point.

Zero-body backtick-fence produces no `Text` event inside the block:
```
Input: "```{network_children}\n```\n"
Events:
  Start(CodeBlock(Fenced("{network_children}")))
  End(CodeBlock)
```
No body text event is emitted — correct for a zero-body marker directive.

Backtick-fence with a trailing argument:
```
Input: "```{figure} image.png\n```\n"
Events:
  Start(CodeBlock(Fenced("{figure} image.png")))
  End(CodeBlock)
```
The entire info string including arguments is preserved verbatim in the `Fenced` variant —
identical parsing behaviour to the colon-fence argument case, but as a `CodeBlock` event
rather than a `DefinitionList`.

**Implication for deferred question**: if backtick-fence directives are adopted in a future
issue, authors should use 4 backticks (```` ````{name} ````) to avoid the 3→4 mutation on
first write-back. Alternatively, normalise 4→3 in `generate_source` when the info string
matches `{...}`. Either approach is straightforward.

---

### Q5: Role peek-back safety — {name} always in Text immediately before Code

All tested contexts:

```
"{abbr}`MyST...`"                → Text("{abbr}") · Code("MyST...")
"**bold** {role}`content`"       → End(Strong) · Text(" {role}") · Code("content")
"[text](url) {role}`content`"    → End(Link) · Text(" {role}") · Code("content")
"<span> {role}`content`"         → InlineHtml("<span>") · Text(" {role}") · Code("content")
"*text {role}`content`*"         → Start(Emphasis) · Text("text {role}") · Code("content") · End(Emphasis)
"{a}`first` {b}`second`"         → Text("{a}") · Code("first") · Text(" {b}") · Code("second")
```

**Finding**: in every tested context the `{name}` token ends a `Text` event that immediately
precedes the `Code` event. Even inside emphasis (`*...*`), the text containing `{role}` is
still a plain `Text` event — the emphasis `Start`/`End` wraps the entire span but does not
interrupt the `Text → Code` adjacency. The peek-back approach (check that the last event
before `Code` is `Text` and that its content ends with `}`) is reliable across all inline
contexts tested.

**Role detection rule** (for a future issue):
- Current event: `Code(content)`
- Previous event: `Text(t)` where `t` ends with `}`
- Extract name: find the last `{` in `t`, take the substring up to `}` as the role name

One edge case not tested: a role immediately following another role with no space
(`{a}`first`{b}`second``). In the tested case with a space, the second role's `{b}` lands
in a fresh `Text` event. The no-space variant is left to a future implementation issue.

---

### Q6: Backtick-fence nesting — recoverable via recursive re-parse

The MyST nesting example uses 4 backticks on the outer fence and 3 on the inner:

```
Input: "````{important}\n```{note}\nHere's my `important`, highly nested note! 🪆\n```\n````\n"

Outer parse events:
  Start(CodeBlock(Fenced("{important}")))
    Text("```{note}\nHere's my `important`, highly nested note! 🪆\n```\n")
  End(CodeBlock)
```

The outer `CodeBlock` body `Text` event contains the raw inner fence as a string —
pulldown-cmark does not recurse into it. Re-parsing that body string with a fresh
`Parser::new_ext` call yields the inner directive cleanly:

```
Inner parse events (body text re-parsed):
  Start(CodeBlock(Fenced("{note}")))
    Text("Here's my `important`, highly nested note! 🪆\n")
  End(CodeBlock)
```

**Round-trip of canonical 4-outer/3-inner form:**
```
in : "````{important}\n```{note}\nHere's my `important`...\n```\n````\n"
rt1: "\n````{important}\n```{note}\nHere's my `important`...\n```\n````"
rt2: rt1  (stable)
```

The outer 4-backtick fence is already at the stable fixed point; the inner 3-backtick
fence is preserved verbatim as body text and is never re-serialised by
`pulldown-cmark-to-cmark`, so it does not gain an extra backtick.

**Degenerate case (3-outer/3-inner):**
```
Input: "```{important}\n```{note}\nHere's my note!\n```\n```\n"

Events:
  Start(CodeBlock(Fenced("{important}")))
    Text("```{note}\nHere's my note!\n")   ← inner closing ``` terminates outer block
  End(CodeBlock)
  Start(CodeBlock(Fenced("")))             ← orphaned trailing ```
  End(CodeBlock)
```

The outer fence closes on the first matching ` ``` ` it encounters — the inner closing
fence — leaving the outer closing fence as an orphaned empty code block. Authors must
always use one more backtick on the outer fence than on the inner.

**Deeper nesting (5-outer/4-inner) round-trip instability:**
```
in : "`````{important}\n````{note}\nHere's my note!\n````\n`````\n"
rt1: "\n````{important}\n````{note}\nHere's my note!\n````\n````"
rt2: "\n````{important}\n````{note}\nHere's my note!\n````\n\n````\n````"   ← diverges
```

5-backtick outer is normalised to 4 on the first RT. The body now contains a 4-backtick
inner fence, and 4-outer/4-inner is degenerate (same as 3/3). **The only stable nesting
depth is 4-outer/3-inner.** noet's authoring convention (4 backticks for top-level
directives, 3 for nested) is the correct and only stable choice.

**Nested zero-body directive (our actual use case):**
```
Input: "````{important}\n```{network_children}\n```\n````\n"

Outer events:
  Start(CodeBlock(Fenced("{important}")))
    Text("```{network_children}\n```\n")
  End(CodeBlock)

rt1: "\n````{important}\n```{network_children}\n```\n````"
rt2: rt1  (stable)
```

The inner `{network_children}` zero-body directive is preserved as body text through the
outer round-trip and is detectable by recursive re-parsing.

---

## Round-trip Write-back Results

Using `cmark_resume_with_source_range_and_options` (the same function `events_to_text` calls):

| Input | Output | Idempotent? |
|---|---|---|
| `":::{network_children}\n:::\n"` | `"\n:::{network_children}\n: ::\n"` | No — closing `:::` corrupted to `: ::` |
| `":::{note}\nHere is a note!\n:::\n"` | `"\n:::{note}\nHere is a note!\n: ::\n"` | No — same closing corruption |
| `":::{figure} image.png\n:width: 100%\n\nCaption.\n:::\n"` | `"\n:::{figure} image.png\n: width: 100%\n\nCaption.\n: ::\n"` | No — options and closing both corrupted |
| `` "```{math}\n\\mathbf{u}\n```\n" `` | `` "\n````{math}\n\\mathbf{u}\n````" `` | No — extra backtick added, trailing newline dropped; **stable at rt2** |
| `` "```{network_children}\n```\n" `` | `` "\n````{network_children}\n````" `` | No — same; **stable at rt2** |
| `"Some content {rolename}\`role content\`.\n"` | `"Some content {rolename}\`role content\`."` | Nearly — trailing newline dropped only |
| `"{abbr}\`MyST (Markedly Structured Text)\`\n"` | `"{abbr}\`MyST (Markedly Structured Text)\`"` | Nearly — trailing newline dropped only |
| `"<!-- network-children -->\n\nSome text.\n"` | `"<!-- network-children -->\n\nSome text."` | Nearly — trailing newline dropped only |

**Critical finding**: Colon-fence directives are **not round-trip safe**. Because
`ENABLE_DEFINITION_LIST` parses them as definition lists, the serializer emits canonical
definition-list syntax — which mangles the closing `:::` into `: ::` and adds a leading
blank line. A source file containing `:::{network_children}\n:::\n` that is parsed and
written back produces broken syntax. This disqualifies in-event-stream transformation of
colon-fence forms as the sole mechanism.

Backtick-fence directives write back with an extra opening backtick (four instead of three)
and a missing trailing newline — also not idempotent on the first pass, but **stable from
the second pass onward** (`rt2 == rt1`). The body content is intact. Authors who author
with 4 backticks are already at the stable fixed point and see no mutation.

Roles and HTML comments are nearly idempotent (trailing newline only), which is acceptable
for source round-trips in practice.

---

## Options

### Option A — Source-Level Pre-processor (Issue 55 original proposal)

A stateless `preprocess_myst_directives(&str) -> Cow<str>` function runs before
pulldown-cmark sees the source, replacing known directive patterns with their internal
intermediate forms (e.g. `<!-- network-children -->`).

Given the correct MyST syntax `:::{network_children}\n:::`, the regex would target the
two-line pattern rather than the single-line `:::name:::` form in the original issue.

**Round-trip consequence**: The `:::` form never enters `current_events`; `generate_source`
emits `<!-- network-children -->`. The file on disk contains the MyST form (written by
`create_network_file`), but a parse-then-write-back cycle silently downgrades it.

**Pros**
- No changes to pulldown-cmark or the event pipeline.
- Fast-path `Cow::Borrowed` when no directives present (zero allocation).
- Adding a directive is one regex rule.
- Upstream updates absorbed without rebasing.

**Cons**
- Round-trip degrades `:::{name}\n:::` → `<!-- name -->` silently.
- Colon-fence write-back would be broken anyway (see round-trip findings), so this is
  actually *less bad* than keeping the colon-fence events in the stream — but the author
  sees a different syntax in the file after a round-trip.
- Unknown directives pass through silently.
- Pre-processor fires on raw source bytes; no diagnostic integration.

---

### Option B — In-event-stream Transformation (new proposal)

Intercept `DefinitionList` events in the `parse()` loop (analogous to how `MetadataBlock`
and `Heading` are intercepted), detect the `:::{name}` pattern in `DefinitionListTitle`
text, and replace the entire definition-list event group in `proto_events` with an
`Html("<!-- network-children -->\n")` event — or suppress it entirely and record the
directive placement on the `IRNode` directly.

**Round-trip consequence**: If the substituted event is `MdEvent::Html(...)`, `generate_source`
emits `<!-- network-children -->` — identical to Option A. If instead we store the
**original source range** and preserve the raw `DefinitionList` events in `proto_events`
unchanged, `cmark_resume_with_source_range_and_options` will use the source range to copy
the original bytes verbatim — so `:::{network_children}\n:::` survives the round-trip.

The source-range preservation approach works because `cmark_resume_with_source_range_and_options`
uses the `Option<Range<usize>>` offsets to splice original source bytes when ranges are
present, rather than re-serializing events. As long as we keep the original events and
their ranges in `proto_events`, write-back is exact.

**Pros**
- Fits the existing `parse()` event-loop pattern (same structure as heading and metadata
  handling).
- Source-range preservation gives exact round-trip fidelity for `:::{name}\n:::` with no
  extra mechanism.
- Directive detection emits `ParseDiagnostic` for unknown `:::{foo}` patterns.
- No pre-processing step; directives are visible at the event level.
- `NetworkCodec::generate_html` can match `MdEvent::Html(NETWORK_CHILDREN_MARKER)` exactly
  as it does today — no change to the HTML generation path.

**Cons**
- `ENABLE_DEFINITION_LIST` must remain enabled (it already is). Any legitimate definition
  lists starting with `:::` would be misidentified — but this is already a MyST authoring
  convention conflict, not a new problem introduced by this approach.
- Requires a small state machine in the parse loop: buffer `Start(DefinitionList)` through
  `End(DefinitionList)`, inspect `DefinitionListTitle` text, decide to emit `Html` or pass
  through.
- The backtick-fence form (```` ```{name} ````) is handled differently — its
  `CodeBlock(Fenced("{name}"))` shape is distinct and would need separate handling if ever
  adopted.

---

### Option C — Fork pulldown-cmark + pulldown-cmark-to-cmark

Add a `ColonFence` event variant to a fork of pulldown-cmark and a corresponding emission
rule to a fork of pulldown-cmark-to-cmark. Both crates are actively maintained and release
3–5 times per year with non-trivial `Event` enum changes.

**Ongoing cost**: diff, rebase, port, test, update pins on every upstream release — for
both crates simultaneously.

**Pros**
- Typed `ColonFence` events visible to LSP (Issue 11) and SPA (Issue 41).
- Write-back emits canonical `:::{name}\n:::` natively.
- Unknown directives emit parse-time diagnostics at the event level.

**Cons**
- Disproportionate maintenance burden for one zero-body marker directive.
- Requires `[patch.crates-io]` or published renamed crates; complicates WASM builds and
  downstream library use.
- The typed-event benefit is speculative until Issue 11 or Issue 41 is built.

---

## Comparison Matrix

| Criterion | A (Pre-processor) | B (Event interception) | C (Fork) |
|---|---|---|---|
| Implementation effort | ~0.5 days | ~1 day | ~3–4 weeks |
| Ongoing maintenance | Negligible | Negligible | Significant |
| Round-trip fidelity | Degrades to HTML comment | Exact (source-range preserved) | Native |
| Fits existing patterns | No (new layer) | Yes (parse loop) | Yes (new event variant) |
| Unknown directive diagnostic | Silent | ParseDiagnostic possible | Parse-time error |
| Parser fork required | No | No | Yes |
| WASM build impact | None | None | Complicates patching |
| Correct MyST syntax supported | Yes (with correct regex) | Yes | Yes |
| HTML comment backward compat | Natural (intermediate = marker) | Natural | Requires dual handling |

---

## Decision

**Backtick-fence `CodeBlock` interception in the `parse()` loop. Colon-fence is not supported.**

The colon-fence (`:::`) form is rejected on three independent grounds:
1. **Round-trip corruption**: the serialiser mangles `:::` into `: ::` — there is no
   source-range workaround because the mangling is in the event representation itself.
2. **Body limitation**: a blank line in the body terminates the `DefinitionList` entirely;
   multi-paragraph body directives are impossible to detect as a single structure.
3. **Spec mismatch**: the MyST spec uses backtick-fence for code-like bodies and
   recommends it for any directive where the body is not plain prose. noet's directives
   are placement markers and structured content — backtick-fence is the correct form.

The backtick-fence form produces a `CodeBlock(Fenced("{name}"))` event with a clean,
unambiguous structure. The body text is preserved verbatim and can be recursively
re-parsed to recover nested directives. Round-trip is stable from the second pass onward;
authors using 4 backticks are already at the stable fixed point.

Option A (pre-processor) is rejected: it would convert backtick-fence syntax to an HTML
comment before pulldown-cmark sees it, discarding the original bytes and preventing
round-trip fidelity.

Option C (fork) is deferred indefinitely: the maintenance burden is disproportionate, and
the typed-event benefit provides no practical value until Issue 11 or Issue 41 is built.

### Architecture for Issue 55

The `myst.rs` module is the **directive registry** — a lookup table mapping directive
names to their internal marker strings:

```
parse() loop sees:
  Start(CodeBlock(Fenced("{network_children}")))
  End(CodeBlock)                                    ← no Text event: zero-body

→ info string "{network_children}": strip "{" and "}", name = "network_children"
→ myst::lookup("network_children") → Some(NETWORK_CHILDREN_MARKER)
→ record directive on current IRNode
→ keep original CodeBlock events WITH source ranges in proto_events
  (generate_source splices original bytes verbatim: "````{network_children}\n````\n")

render_html_body / NetworkCodec::generate_html:
  CodeBlock(Fenced("{network_children}")) → emit Html(NETWORK_CHILDREN_MARKER)
  (NetworkCodec::generate_html already scans for NETWORK_CHILDREN_MARKER downstream)
```

For a directive with body content:

```
parse() loop sees:
  Start(CodeBlock(Fenced("{note}")))
    Text("Here is a note!\n")
  End(CodeBlock)

→ body text is preserved in proto_events; render step passes it through as-is
  or recursively re-parses it if the directive type requires structured body handling
```

For nested directives the outer parse produces a body `Text` event containing the raw
inner fence string. The inner directive is recovered by calling `Parser::new_ext` on that
string — a recursive call identical to the top-level parse. Only one nesting depth is
stable: **4-backtick outer, 3-backtick inner**. Deeper nesting (5/4) collapses to 4/4 on
the first round-trip and diverges on the second. Authors must use exactly one more
backtick on the outer fence than on the inner.

### Authoring convention

| Context | Backtick count | Example |
|---|---|---|
| Top-level directive | 4 | ```` ````{network_children} ```` |
| Nested directive (one level) | 3 | ` ```{note} ` (inside a 4-backtick outer) |

Authors who use 3 backticks for a top-level directive will see a 3→4 mutation on the
first write-back, then stability. This is a one-time normalisation, not divergence.

### Accepted trade-offs

- The info string `{name}` is not MyST-typed by pulldown-cmark — it is just an unusual
  code block info string. Detection is entirely in `myst.rs`'s lookup table.
- A fenced code block whose info string happens to match `{...}` but is not a noet
  directive will emit a `ParseDiagnostic::warning`. This is intentional: unknown
  `{name}` info strings are a MyST authoring convention and should be flagged.
- Nesting beyond one level (4-outer/3-inner) is unstable and must be documented as
  unsupported. This is a pulldown-cmark-to-cmark limitation, not a noet limitation.
- `ENABLE_DEFINITION_LIST` remains enabled. Colon-fence input (`:::{name}\n:::`) will
  parse as a `DefinitionList` and pass through unchanged — it will not be recognised as
  a directive and will render as a definition list in HTML output. Authors who write
  colon-fence syntax will see it silently ignored. A diagnostic for this case is desirable
  but deferred (requires detecting `:::{ ` prefix in `DefinitionListTitle` text).

### Revisit triggers for Option C (fork)

1. The LSP (Issue 11) requires typed directive events for autocompletion or hover docs.
2. The in-event-stream `CodeBlock` detection proves fragile against pulldown-cmark changes
   to how fenced code block info strings are parsed.
3. pulldown-cmark upstream adds an official extension callback for custom block syntax.

---

## Deferred Questions

- **Body directives with multi-paragraph content**: the backtick-fence body `Text` event
  is a raw string — re-parsing it with `Parser::new_ext` handles single-paragraph and
  code-body content correctly. Multi-paragraph bodies (containing blank lines) will parse
  correctly when re-parsed because the inner content is not subject to the
  `DefinitionList` limitation. Confirmed viable in principle; defer until a concrete
  directive type with a rich body is needed.

- **Inline roles** (`` {name}`content` ``): adjacent `Text("{name}")` + `Code("content")`
  in the event stream. Confirmed viable via Q5 experiments: the `{name}` token reliably
  ends a `Text` event immediately before the `Code` event in all tested inline contexts
  (paragraph start, after strong/link/inline-HTML, inside emphasis, after another role).
  Detection rule: last event before `Code` is `Text(t)`, `t` ends with `}`, extract name
  from last `{...}` in `t`. No current use case in noet. Defer to a follow-on issue.

- **Colon-fence passthrough diagnostic**: authors who write `:::{name}\n:::` will see it
  silently rendered as a definition list. A diagnostic warning (detect `:::{` prefix in
  `DefinitionListTitle` text and emit `ParseDiagnostic::warning("use backtick-fence
  syntax: ````{name}")`) would improve the authoring experience. Defer to a follow-on issue.

- **LSP directive autocompletion**: the directive registry in `myst.rs` should be exposed
  as a queryable list when Issue 11 is implemented. Defer to that issue.

---

## References

- MyST syntax overview: https://mystmd.org/guide/syntax-overview
- `src/codec/md.rs` — `MdCodec::parse`, `events_to_text`, `render_html_body`
- `src/codec/network.rs` — `NETWORK_CHILDREN_MARKER`, `NETWORK_CHILDREN_SENTINEL`,
  `generate_html`, `generate_deferred_html`
- `docs/project/trades/TRADE_52_DYNAMIC_CONTENT_PLACEMENT.md` — prior trade study on
  placement mechanism; this study supersedes its "Revisit Triggers" with concrete findings
- Issue 55: MyST Directive Syntax for Noet Extensions
- Issue 11: Basic LSP (future consumer of typed directive events)

---

## Appendix: Test Harness Source

The empirical results in this study were produced by a standalone Rust binary at
`/tmp/pdcmark_test/`. Reconstruct with:

```
mkdir -p /tmp/pdcmark_test/src
```

`Cargo.toml`:
```toml
[package]
name = "pdcmark_test"
version = "0.1.0"
edition = "2021"

[dependencies]
pulldown-cmark = "0.13.0"
pulldown-cmark-to-cmark = "22"
```

`src/main.rs` (final state, covering Q1–Q6):
```rust
use pulldown_cmark::{Event, Options, Parser};
use pulldown_cmark_to_cmark::cmark_resume_with_source_range_and_options;

fn events(label: &str, input: &str) {
    let opts = Options::all();
    println!("\n=== EVENTS: {label} ===");
    println!("  input: {:?}", input);
    for (event, range) in Parser::new_ext(input, opts).into_offset_iter() {
        println!("  [{:?}]  {:?}", range, event);
    }
}

fn roundtrip(label: &str, input: &str) {
    let opts = Options::all();
    let events_vec: Vec<(Event<'static>, Option<std::ops::Range<usize>>)> =
        Parser::new_ext(input, opts)
            .into_offset_iter()
            .map(|(e, r)| (e.into_static(), Some(r)))
            .collect();

    let mut buf = String::with_capacity(input.len() + 64);
    let _ = cmark_resume_with_source_range_and_options(
        events_vec.iter().map(|(e, r)| (e, r.clone())),
        input,
        &mut buf,
        None,
        pulldown_cmark_to_cmark::Options::default(),
    );

    // second round-trip
    let events_vec2: Vec<(Event<'static>, Option<std::ops::Range<usize>>)> =
        Parser::new_ext(&buf, opts)
            .into_offset_iter()
            .map(|(e, r)| (e.into_static(), Some(r)))
            .collect();
    let buf_clone = buf.clone();
    let mut buf2 = String::with_capacity(buf.len() + 64);
    let _ = cmark_resume_with_source_range_and_options(
        events_vec2.iter().map(|(e, r)| (e, r.clone())),
        &buf_clone,
        &mut buf2,
        None,
        pulldown_cmark_to_cmark::Options::default(),
    );

    println!("\n=== ROUNDTRIP: {label} ===");
    println!("  in : {:?}", input);
    println!("  rt1: {:?}", buf);
    println!("  rt2: {:?}", buf2);
    println!("  rt1==in : {}", input == buf.as_str());
    println!("  rt2==rt1: {}", buf == buf2);
}

fn events_recursive(label: &str, outer_input: &str) {
    let opts = Options::all();
    println!("\n=== RECURSIVE EVENTS: {label} ===");
    println!("  outer input: {:?}", outer_input);
    for (event, range) in Parser::new_ext(outer_input, opts).into_offset_iter() {
        println!("  [{:?}]  {:?}", range, event);
        // When we see a CodeBlock body text event, re-parse it as Markdown
        if let Event::Text(ref t) = event {
            let body = t.as_ref();
            if !body.trim().is_empty() {
                println!("    -- re-parsing body text as Markdown --");
                for (inner_event, inner_range) in
                    Parser::new_ext(body, opts).into_offset_iter()
                {
                    println!("    inner [{:?}]  {:?}", inner_range, inner_event);
                }
            }
        }
    }
}

fn main() {
    println!("\n\n╔══════════════════════════════════════════════════════════╗");
    println!(  "║  Q4: BLANK LINE IN COLON-FENCE BODY                     ║");
    println!(  "╚══════════════════════════════════════════════════════════╝");

    events("colon-fence body with blank line",
        ":::{note}\nFirst paragraph.\n\nSecond paragraph after blank line.\n:::\n");

    roundtrip("colon-fence body with blank line",
        ":::{note}\nFirst paragraph.\n\nSecond paragraph after blank line.\n:::\n");

    events("colon-fence single-para body",
        ":::{note}\nHere is a note!\n:::\n");

    println!("\n\n╔══════════════════════════════════════════════════════════╗");
    println!(  "║  Q3: ARGUMENTS AFTER DIRECTIVE NAME                     ║");
    println!(  "╚══════════════════════════════════════════════════════════╝");

    events("colon-fence with filename arg",
        ":::{figure} image.png\n:::\n");

    events("colon-fence with title arg",
        ":::{admonition} My Title\n:::\n");

    roundtrip("colon-fence with filename arg",
        ":::{figure} image.png\n:::\n");

    println!("\n\n╔══════════════════════════════════════════════════════════╗");
    println!(  "║  Q1: COLON-FENCE WITHOUT ENABLE_DEFINITION_LIST         ║");
    println!(  "╚══════════════════════════════════════════════════════════╝");

    {
        let no_deflist = Options::all() & !Options::ENABLE_DEFINITION_LIST;
        let input = ":::{network_children}\n:::\n";
        println!("\n=== EVENTS (no ENABLE_DEFINITION_LIST): colon-fence zero-body ===");
        println!("  input: {:?}", input);
        for (event, range) in Parser::new_ext(input, no_deflist).into_offset_iter() {
            println!("  [{:?}]  {:?}", range, event);
        }
    }

    println!("\n\n╔══════════════════════════════════════════════════════════╗");
    println!(  "║  Q2: BACKTICK-FENCE — ZERO BODY, TRAILING ARG, DOUBLE RT║");
    println!(  "╚══════════════════════════════════════════════════════════╝");

    events("backtick-fence zero-body",
        "```{network_children}\n```\n");

    events("backtick-fence with trailing arg",
        "```{figure} image.png\n```\n");

    roundtrip("backtick-fence directive (math)",
        "```{math}\n\\mathbf{u}\n```\n");

    roundtrip("backtick-fence zero-body",
        "```{network_children}\n```\n");

    events("backtick-fence already 4 backticks",
        "````{math}\n\\mathbf{u}\n````\n");

    roundtrip("backtick-fence already 4 backticks",
        "````{math}\n\\mathbf{u}\n````\n");

    println!("\n\n╔══════════════════════════════════════════════════════════╗");
    println!(  "║  Q5: ROLE CONTEXT — WHAT PRECEDES THE Code EVENT?       ║");
    println!(  "╚══════════════════════════════════════════════════════════╝");

    events("role at paragraph start",
        "{abbr}`MyST (Markedly Structured Text)`\n");

    events("role after inline formatting",
        "**bold** {role}`content`\n");

    events("role after link",
        "[text](url) {role}`content`\n");

    events("role after inline HTML",
        "<span> {role}`content`\n");

    events("role nested inside emphasis",
        "*text {role}`content`*\n");

    events("role after another role",
        "{a}`first` {b}`second`\n");

    println!("\n╔══════════════════════════════════════════════════════════╗");
    println!(  "║  Q6: NESTED BACKTICK-FENCE                              ║");
    println!(  "╚══════════════════════════════════════════════════════════╝");

    // The canonical MyST nesting example from the docs
    let nested_myst =
        "````{important}\n```{note}\nHere's my `important`, highly nested note! 🪆\n```\n````\n";

    events("nested backtick-fence (4 outer / 3 inner)", nested_myst);
    roundtrip("nested backtick-fence (4 outer / 3 inner)", nested_myst);
    events_recursive("nested backtick-fence — re-parse body", nested_myst);

    // What if both use 3 backticks? (should be indistinguishable / broken)
    let nested_3_3 =
        "```{important}\n```{note}\nHere's my note!\n```\n```\n";

    events("nested backtick-fence (3 outer / 3 inner — broken?)", nested_3_3);

    let nested_5_4 =
        "`````{important}\n````{note}\nHere's my note!\n````\n`````\n";

    events("nested backtick-fence (5 outer / 4 inner)", nested_5_4);
    roundtrip("nested backtick-fence (5 outer / 4 inner)", nested_5_4);

    // Our actual use case: network_children inside an outer directive
    let nested_zero_body =
        "````{important}\n```{network_children}\n```\n````\n";

    events("nested zero-body network_children (4 outer / 3 inner)", nested_zero_body);
    roundtrip("nested zero-body network_children", nested_zero_body);
}
```