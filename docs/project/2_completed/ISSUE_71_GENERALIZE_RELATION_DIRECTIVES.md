# Issue 71: Generalize Relation Directives — Unified Codespan Syntax

**Status**: COMPLETE (code implementation); corpus migration and `myst_directive_architecture.md` update pending

## Completion Notes

All code implementation is complete and committed. Remaining open items:
- **Step 5 (corpus migration)**: fenced `{implements}` blocks in the downstream corpus not yet converted to codespan toggle form — author action required.
- **`myst_directive_architecture.md`**: needs update to reflect codespan toggle, derived sentinels, and removed `promote_markers`/`marker` — documentation-only, no blocking code change.
- **Open Questions**: `ReferenceRole` new enum chosen (done). `{consists_of}`/`{component_of}` Section warning not yet implemented (deferred). Shadow warning on custom verb override implemented.

**Priority**: HIGH
**Estimated Effort**: 2 days (RELATIVE COMPARISON ONLY)
**Dependencies**: Issue 55 complete (MyST directive framework). Issue 58 subsumed by this issue; mark 58 as duplicate/closed.

## Summary

The `{implements}` block directive is hardwired, broken (block body never sub-parsed for
links), and uses a fenced-code-block syntax that is more complex than necessary. This issue
replaces the fenced block form with a unified **codespan toggle** model, generalizes relation
directives to all six WeightKind × reference_role combinations, introduces a precise
`{relation}kind=K, ref=R` form, and supports per-document custom verb registration. Issue 58
(inline `{implements}` role) is subsumed here.

**Breaking change**: the existing fenced `{implements}` / `{end}` block syntax is removed.
All corpora using it must migrate to the new codespan toggle form. The author has control
over all affected documents and will migrate them as part of this effort.

## Goals

1. Replace the fenced block directive syntax with a codespan toggle: `` `{implements}` ``
   opens a relation context; `` `{end}` `` closes it. All links encountered while the
   context is open are recorded with the active `(WeightKind, ReferenceRole)`.

2. Generalize to all six WeightKind × reference_role combinations with canonical verb names
   and a precise synonym form `{relation}kind=K, ref=R` parsed via the existing
   `parse_directive_info` signature (name=`"relation"`, args=`"kind=K, ref=R"`).

3. Support per-document custom verb registration: `` `{relation}name=mitigates, kind=pragmatic, ref=source` ``
   registers `mitigates` as a document-local alias. The per-document registry is
   pre-populated from the static verb table; document declarations overwrite (last-one-wins).

4. Implement a **relation context stack** so nested contexts restore the previous context
   on `{end}` rather than always reverting to default.

5. Implicit close on node boundary (new heading / new node) to contain damage from
   forgotten `` `{end}` `` tags.

6. Subsume Issue 58: the codespan toggle replaces the proposed inline role form. No
   separate inline-only implementation is needed.

## Architecture

### Reference Role Model

Every relation directive places the **referenced nodes** into a slot: either the **source**
slot (referenced nodes feed into the IR node as sink) or the **sink** slot (referenced nodes
are sinks the IR node flows into as source).

Verb aliases and their canonical meanings:

| Verb            | Ref nodes role | IR field     | Direction  |
|-----------------|----------------|--------------|------------|
| `{uses}`        | source         | `upstream`   | `Incoming` |
| `{implements}`  | source         | `upstream`   | `Incoming` |
| `{used_by}`     | sink           | `downstream` | `Outgoing` |
| `{draws_from}`  | source         | `upstream`   | `Incoming` |
| `{underlies}`   | sink           | `downstream` | `Outgoing` |
| `{consists_of}` | source         | `upstream`   | `Incoming` |
| `{component_of}`| sink           | `downstream` | `Outgoing` |

`{implements}` is retained as a legacy alias for `{uses}`.

Default link behavior (no open context) is unchanged: `WeightKind::Epistemic`, `ref=source`
→ `upstream`. The toggle only overrides the active kind and reference role.

### Codespan Toggle Syntax

A bare codespan whose content is a recognized verb, precise form, or custom verb opens a
relation context. `` `{end}` `` closes it.

```markdown
`{uses}`
- [[REQ-001]] recorded as Pragmatic upstream
- [[REQ-002]] recorded as Pragmatic upstream
`{end}`

Back to default: [[Concept-A]] is Epistemic upstream again.
```

Precise form using `{relation}` — parsed by the existing `parse_directive_info` as
`name="relation"`, `args="kind=pragmatic, ref=source"`:

```markdown
`{relation}kind=pragmatic, ref=source`
- [[REQ-001]]
`{end}`
```

Custom verb declaration — registers `mitigates` in the document-local verb registry:

```markdown
`{relation}name=mitigates, kind=pragmatic, ref=source`

`{mitigates}`
- [[HAZARD-001]]
`{end}`
```

Toggle rules:
- `` `{verb}` `` or `` `{relation}kind=K, ref=R` `` — push `(WeightKind, ReferenceRole, label)` onto stack
- `` `{relation}name=X, kind=K, ref=R` `` — register `X` in document-local registry (last-one-wins); do **not** push onto stack
- `` `{end}` `` — pop the stack; if empty, emit warning "unmatched {end}"
- New node boundary (heading) — drain the stack, emit one warning per unclosed entry
- Links while stack non-empty → routed per stack top
- Links while stack empty → default (Epistemic, source)

### Directive Registry — Two-Tier Lookup

`DIRECTIVES` remains the single source of truth and the **global tier** for all directive
names, both fenced-block and codespan. It gains `weight_kind` and `ref_role` fields. No
clone or pre-population into a session structure is needed — the global is always available
by reference via a free function:

```rust
fn global_verb_context(name: &str) -> Option<(WeightKind, ReferenceRole)> {
    DIRECTIVES.iter()
        .find(|d| d.name == name)
        .and_then(|d| d.weight_kind.zip(d.ref_role))
}
```

`MdCodec` gains a `session_verb_registry: HashMap<String, (WeightKind, ReferenceRole)>`
field, **starting empty**. It is only populated when a document declares
`` `{relation}name=X, ...` ``. Lookup is session-first, global-fallback:

```rust
fn directive_context(
    &self,
    name: &str,
    args: &str,
) -> Option<(WeightKind, ReferenceRole)> {
    if name == "relation" {
        // parse args: "kind=K, ref=R" or "name=X, kind=K, ref=R"
        parse_relation_args(args).map(|(_, kind, role)| (kind, role))
    } else {
        self.session_verb_registry.get(name).copied()
            .or_else(|| global_verb_context(name))
    }
}
```

The `Code` arm gate — deciding whether a `{...}` codespan should be treated as a
directive at all — uses `DIRECTIVES` lookup plus explicit checks for `"relation"` and
`"end"`. Unrecognized `{...}` spans (e.g. `` `{variable}` `` in prose) pass through
silently with no warning:

```rust
// In the Code arm of parse():
if let Some((name, args)) = parse_directive_info(content.trim()) {
    let is_known = lookup(name).is_some() || name == "relation" || name == "end";
    if is_known {
        // dispatch to directive_context / stack / registry logic
    }
    // else: silent passthrough — not a directive
}
```

`parse_relation_args` extracts key-value pairs from the args string, returning
`(Option<String>, WeightKind, ReferenceRole)`. Returns `None` on malformed input (warn +
no-op).

### Relation Context Stack

```rust
/// Active relation context stack. Each entry is pushed by a recognized codespan
/// directive and popped by `{end}` or an implicit node-boundary close.
/// The label is the directive text as written, used in diagnostic messages.
relation_context_stack: Vec<(WeightKind, ReferenceRole, String)>,
```

`ReferenceRole` is a new two-variant enum:

```rust
pub enum ReferenceRole { Source, Sink }
```

`ReferenceRole` describes the slot occupied by the **referent** (the referenced node),
not the IR node. The IR node is always the subject/owner; it takes the complementary slot
automatically. `Source` → referent is source, IR node is sink → `upstream`. `Sink` →
referent is sink, IR node is source → `downstream`. This matches the subject/verb/referent
model in `dag_model.md` §3.

Link routing:

```rust
let (kind, ref_role) = self.relation_context_stack.last()
    .map(|(k, r, _)| (*k, *r))
    .unwrap_or((WeightKind::Epistemic, ReferenceRole::Source));

match ref_role {
    // Referent is source → IR node (subject) is sink → push to upstream
    ReferenceRole::Source => current.upstream.push(relation.with_kind(kind)),
    // Referent is sink → IR node (subject) is source → push to downstream
    ReferenceRole::Sink   => current.downstream.push(relation.with_kind(kind)),
}
```

### `myst.rs` Changes

Add `weight_kind: Option<WeightKind>` and `ref_role: Option<ReferenceRole>` to
`DirectiveDef`. Register all seven verb entries. The `DIRECTIVES` entries serve as the
seed for the per-document verb registry.

`is_block_opener` becomes a derived convenience: `directive_context(name, "").is_some()`.
The `{relation}` name itself is not registered in `DIRECTIVES` — it is handled as a
special case in `directive_context`.

### Removed: Fenced Block Form

The `CodeBlock` info-string arm of `parse()` handling for relation directives is removed.
`{maps_to}` retains its fenced block form (third-party ownership semantics differ from
self-owned relation directives). `in_implements_block: bool` is removed entirely.

## Implementation Steps

1. **Add `ReferenceRole`, extend `DirectiveDef`, implement `parse_relation_args`** (0.2 days)
   - [x] Add `pub enum ReferenceRole { Source, Sink }` to `src/codec/myst.rs`.
   - [x] Add `weight_kind: Option<WeightKind>` and `ref_role: Option<ReferenceRole>` to
         `DirectiveDef`; populate all seven verb entries.
   - [x] Implement `pub(crate) fn parse_relation_args(args: &str) -> Option<(Option<String>, WeightKind, ReferenceRole)>` — splits on `,`, parses `key=value` pairs, returns `None` on malformed input.
   - [x] Unit tests: all seven verb entries have correct values; `parse_relation_args`
         handles well-formed and malformed inputs.

2. **Session verb registry on `MdCodec`** (0.1 days)
   - [x] Add `session_verb_registry: HashMap<String, (WeightKind, ReferenceRole)>` to
         `MdCodec`; initialize empty in `new()`, clear in `parse()` reset.
   - [x] Add free function `global_verb_context(name: &str) -> Option<(WeightKind,
         ReferenceRole)>` in `myst.rs`.
   - [x] `dispatch_relation_directive` free function handles session registry, stack, and
         `{relation}` dispatch; avoids borrow conflict with `MdParser`.
   - [x] `{relation}name=X, ...` arm: registers in session registry, warns on shadow, warns
         on malformed args, does not push stack.

3. **Replace `in_implements_block` with relation context stack** (0.3 days)
   - [x] Remove `in_implements_block: bool` from `MdCodec`.
   - [x] Add `relation_context_stack: Vec<(WeightKind, ReferenceRole, String)>`.
   - [x] `Code` arm: gates on `lookup(name).is_some() || name == "relation" || name == "end"
         || session_verb_registry.contains_key(name)` — silent passthrough otherwise.
   - [x] On heading boundary: drain stack, emit one warning per unclosed entry.
   - [x] Link routing uses stack top; `Source` → upstream, `Sink` → downstream.
   - [x] `CodeBlock` relation-directive arm removed; `{maps_to}` arm intact.
   - [x] All existing `{maps_to}` and `{network_children}` tests pass.

4. **Render: codespan directives suppressed from HTML output** (0.1 days)
   - [x] `render_html_body`: recognized codespan directives suppressed (no `<code>` tag).
   - [x] Pipeline directives in codespan form emit their sentinel per the invariant.
   - [x] Links inside active context render as normal `<a>` tags with `noet-rel-*` CSS class.

5. **Corpus migration** (0.2 days)
   - [ ] Search corpus for fenced `` ```{implements} `` blocks; convert to codespan toggle.
   - [ ] Verify no fenced relation directives remain (other than `{maps_to}`).

6. **Collapse `marker` into `sentinel` — remove `promote_markers` pass** (0.2 days)
   - [x] `marker` field removed from `DirectiveDef`; sentinel derived from name as
         `<!--@@noet-{name}@@-->` (underscores → hyphens) via `sentinel()` function.
   - [x] `render_html_body` and `NetworkCodec::generate_html` emit sentinel directly.
   - [x] `promote_markers`, `marker()`, `mapping_table_marker()` deleted from `myst.rs`.
   - [ ] Update `myst_directive_architecture.md` §3.1 and §4 to reflect single-field design.
   - [x] All tests pass; no regressions.

7. **Relation type decoration on rendered links** (bonus)
   - [x] `build_title_attribute` extended with `rel` parameter; encoded in JSON config blob
         as space-separated `"kind:role"` pairs (e.g. `"pragmatic:source epistemic:source"`).
   - [x] `parse_title_attribute` extracts `rel` from JSON config.
   - [x] `check_for_link_and_push` derives `rel` from `relation.weight.weights.keys()`.
   - [x] `render_html_body`: links with `rel` emit `<a class="noet-rel-pragmatic-source">`.

8. **Integration verification** (0.3 days)
   - [x] Integration test: `` `{uses}` `` toggle → Pragmatic upstream; reverts after `{end}`.
   - [x] Integration test: nested contexts → stack restores outer context after inner `{end}`.
   - [x] Integration test: missing `{end}` → warning at node boundary, no panic.
   - [x] Integration test: `` `{relation}kind=epistemic, ref=sink` `` → Epistemic downstream.
   - [x] Integration test: custom verb registration and use.
   - [x] Integration test: custom verb override warns (last-one-wins).
   - [x] Integration test: empty stack `{end}` → warning, no panic.
   - [ ] Run `noet build` on migrated corpus; confirm edges in `get_traceability` output.

## Testing Requirements

**`parse_relation_args`**:
- `"kind=pragmatic, ref=source"` → `Some((None, Pragmatic, Source))`
- `"kind=epistemic, ref=sink"` → `Some((None, Epistemic, Sink))`
- `"name=mitigates, kind=pragmatic, ref=source"` → `Some((Some("mitigates"), Pragmatic, Source))`
- `"kind=unknown, ref=source"` → `None` + warning
- `""` → `None` + warning
- `"garbage"` → `None` + warning

**`directive_context` (via verb registry)**:
- `"implements"` → `Some((Pragmatic, Source))`
- `"uses"` → `Some((Pragmatic, Source))`
- `"used_by"` → `Some((Pragmatic, Sink))`
- `"draws_from"` → `Some((Epistemic, Source))`
- `"underlies"` → `Some((Epistemic, Sink))`
- `"consists_of"` → `Some((Section, Source))`
- `"component_of"` → `Some((Section, Sink))`
- `"end"` → `None`; unknown name → `None`
- After `{relation}name=mitigates, kind=pragmatic, ref=source`: `"mitigates"` →
  `Some((Pragmatic, Source))` (session registry hit)
- After re-declaration with different kind: last-one-wins (session overwrites)
- `global_verb_context("uses")` → `Some((Pragmatic, Source))` (no session needed)
- Unrecognized `{variable}` codespan → silent passthrough, no warning, no stack push

**Parse — codespan toggle**:
- `` `{uses}` `` → pushes `(Pragmatic, Source)` onto stack
- `` `{end}` `` → pops stack
- `` `{end}` `` on empty stack → warning, no panic
- Links while stack non-empty → routed per stack top
- Links while stack empty → default Epistemic upstream (unchanged)
- Nested: `` `{uses}` `` then `` `{draws_from}` `` then `` `{end}` `` → restores `{uses}` context
- Heading while stack non-empty → stack drained, one warning per unclosed entry
- `` `{relation}kind=pragmatic, ref=source` `` → same effect as `` `{uses}` ``
- `` `{relation}name=mitigates, kind=pragmatic, ref=source` `` → registers alias, does not push stack

**Render**:
- `` `{uses}` `` → no visible HTML output
- `` `{end}` `` → no visible HTML output
- `` `{relation}...` `` → no visible HTML output
- Links inside active context → normal `<a>` tags

**Regression**:
- All existing `{maps_to}` and `{network_children}` tests pass unchanged
- Default link behavior (Epistemic upstream) unchanged outside toggle scope

## Success Criteria

- [x] `ReferenceRole { Source, Sink }` defined; all seven `DIRECTIVES` entries carry
      correct `weight_kind` and `ref_role`.
- [x] `parse_relation_args` correctly parses well-formed args; returns `None` + warning
      on malformed input.
- [x] Per-document verb registry starts empty; `{relation}name=X, ...` overwrites
      (last-one-wins); single lookup path (session-first, global-fallback).
- [x] Relation context stack implemented; `{end}` pops; nested contexts restore correctly.
- [x] Implicit close on node boundary with diagnostic; no panic on unmatched `{end}`.
- [x] Links routed to correct `upstream`/`downstream` with correct `WeightKind`; default
      unchanged outside context.
- [x] Fenced relation directive syntax removed; `{maps_to}` fenced form unaffected.
- [ ] Corpus migrated; `noet build` confirms edges in `get_traceability`.
- [x] `marker` field removed from `DirectiveDef`; `promote_markers` deleted; pipeline
      directives emit derived sentinel directly from both `CodeBlock` and `Code` detection paths.
- [x] No regression on `{maps_to}`, `{network_children}`, or default link behavior.

## Risks

- **Breaking syntax change**: all corpora using fenced `{implements}` blocks must be
  migrated. **Mitigation**: author controls all affected documents; migration is Step 5.
- **Stack discipline**: forgotten `` `{end}` `` silently annotates excess links until the
  next heading. **Mitigation**: implicit close + warning at heading boundary limits blast
  radius to one section.
- **`promote_markers` removal**: `generate_html` currently calls `promote_markers` after
  `render_html_body`. Removing it requires verifying no caller depends on the intermediate
  marker strings surviving into the HTML body. **Mitigation**: grep for all `marker()`
  call sites and `promote_markers` call sites before deleting; confirm the sentinel strings
  are unique enough to survive template wrapping without the promotion step.
- **`{maps_to}` separation**: `{maps_to}` retains the fenced block form. Care needed to
  not accidentally remove its `CodeBlock` arm. **Mitigation**: keep `{maps_to}` tests
  running throughout; remove only relation-directive handling from the `CodeBlock` arm.
- **`parse_directive_info` reuse**: `Code` event content is the full span text including
  braces (e.g. `"{uses}"` or `"{relation}kind=pragmatic, ref=source"`). `parse_directive_info`
  already handles this form correctly — no signature change needed.

## Open Questions

- ~~Should `ReferenceRole` be a new enum or reuse `Direction`?~~ **Resolved**: new enum
  with `From` impls to/from `petgraph::Direction`.
- Should `{consists_of}` and `{component_of}` emit a parse-time warning about Section
  edges (which heading hierarchy normally manages)? **Deferred** — no concrete use case yet.
- ~~Should custom verb declarations warn when they shadow a built-in?~~ **Resolved**: yes,
  warning implemented.

## References

- `src/codec/myst.rs` — `DirectiveDef`, `DIRECTIVES`, `ReferenceRole`, `global_verb_context`,
  `parse_relation_args`, `sentinel` (derived), `parse_directive_info`, `is_block_opener`
- `src/codec/md.rs` — `MdCodec`, `dispatch_relation_directive`, `session_verb_registry`,
  `relation_context_stack`, `build_title_attribute` (extended with `rel`), `render_html_body`
- `src/codec/builder.rs` — `push_relation`, `Direction::Incoming/Outgoing` semantics
- `docs/design/dag_model.md` — source/sink/reference_role terminology (corrected §2, §3)
- `docs/design/myst_directive_architecture.md` — needs update (pending)
- `docs/design/query_model.md` §4.2 — projection direction semantics
- `docs/mcp.md` — WeightKind × direction × verb table
- Issue 58: Inline `{implements}` Role — subsumed/closed