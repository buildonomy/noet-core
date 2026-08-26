---
title: "MyST Directive Architecture"
version: "0.1"
---

# MyST Directive Architecture

**Version**: 0.2
**Status**: Current implementation

## 1. Purpose

This document specifies the architecture for noet's MyST directive system — the mechanism
by which authors embed structured content and declare semantic relations in Markdown source
files using two directive syntax forms.

**Scope**: `src/codec/myst.rs`, `src/codec/md.rs`, `src/codec/compiler.rs` (deferred
phase), and the `DocCodec` trait's `should_defer` / `generate_html` methods as they
relate to directives.

**Out of scope**: The broader codec system and multi-pass compilation model are specified
in [`beliefbase_architecture.md`](./beliefbase_architecture.md).

---

## 2. Syntax and Authoring Convention

noet extends Markdown with directives in two syntax forms. The two forms are
**syntax-form-agnostic** at the registry level: both are detected via
`myst::parse_directive_info` and dispatched through the same `DIRECTIVES` lookup.

### 2.1. Fenced-block directives

Detected as `CodeBlock(Fenced("{name}"))` events. Any directive can use this form
regardless of whether it has a deferred render pipeline — the pipeline is determined
solely by whether `builder` is set in the `DirectiveDef` registration:

```
````{network_children}
````
```

- Use **4 backticks** for top-level directives. A 3-backtick directive is normalized to 4
  on the first write-back and is then stable.
- Nested directives use 3 backticks inside a 4-backtick outer fence — the only stable
  nesting depth under pulldown-cmark.
- The **colon-fence form** (`:::`) is **not supported**. Under pulldown-cmark with
  `ENABLE_DEFINITION_LIST`, the serializer corrupts `:::` to `: ::` on every write-back.
  See `docs/project/trades/TRADE_55_MYST_DIRECTIVE_SYNTAX.md` for the full empirical
  analysis.

### 2.2. Codespan directives

Detected as `Code("{name}")` or `Code("{relation}args")` events. Used for
relation context directives that open, close, or configure a relation context affecting
how links are recorded:

```markdown
`{uses}`
- [[REQ-001]] recorded as Pragmatic upstream
- [[REQ-002]] recorded as Pragmatic upstream
`{end}`
```

- Detected as `Code("{name}")` or `Code("{relation}args")` events in `MdCodec::parse`.
- Unrecognized `{...}` code spans (e.g. `` `{variable}` `` in prose) pass through silently.

**Bare-codespan rule**: A codespan with **no arguments** (e.g. `` `{query}` ``,
`` `{network_children}` ``) is treated as a directive invocation **only** if the name is
a relation verb (`weight_kind.is_some()`), `{end}`, `{relation}`, or a session-registered
custom verb. All other bare codespans — including pipeline directives like `{query}`,
`{maps_to}`, `{network_children}`, and `{requirements_table}` — render as ordinary
`<code>` elements. This prevents prose mentions of directive names from being
misinterpreted as directive invocations.

Codespans **with arguments** (e.g. `` `{maps_to} Pragmatic` ``) are always dispatched
through the directive system for any known name.

**Codespan directive forms:**

| Form | Example | Effect |
|------|---------|--------|
| Named verb | `` `{uses}` `` | Push `(Pragmatic, Source)` onto relation context stack |
| Precise form | `` `{relation}kind=pragmatic, ref=source` `` | Push explicit context |
| Custom verb registration | `` `{relation}name=mitigates, kind=pragmatic, ref=source` `` | Register alias in session registry; does not push stack |
| Closer | `` `{end}` `` | Pop relation context stack |

**Subject/verb/referent model**: the authoring document is always the **subject** (owner).
The **verb** (directive name) determines which graph slot the **referent** (referenced node)
occupies; the subject takes the complementary slot automatically. See
`docs/design/dag_model.md` §3 for the full model.

**Detection**: `myst::parse_directive_info` handles both forms — the same function parses
`{network_children}` from a fenced info string and `{uses}` or `{relation}kind=…` from a
codespan. Unknown names pass through unchanged.

---

## 3. The `DirectiveDef` Registry

`src/codec/myst.rs` is the **single source of truth** for all directive metadata. Every
directive is declared as a `DirectiveDef` entry in the static `DIRECTIVES` array. No
per-directive logic lives outside this file except in `generate_html_for_path`
(the async pipeline runner) and the codec `generate_html` implementations that emit
sentinels.

```rust
pub struct DirectiveDef {
    /// Name used in the backtick-fence info string or codespan, e.g. `"network_children"`.
    pub name: &'static str,

    /// Canonical fenced-block source form written to new files by `noet init`
    /// (e.g. `"````{network_children}"`). Empty for directives never written
    /// programmatically by the tool.
    pub directive: &'static str,

    /// For codespan relation verbs: the role of the referent (referenced node).
    /// `Some(Source)` → referent is graph source, subject is sink → links go to `upstream`.
    /// `Some(Sink)` → referent is graph sink, subject is source → links go to `downstream`.
    /// `None` for pipeline directives and `{end}`.
    pub ref_role: Option<ReferenceRole>,

    /// For codespan relation verbs: the WeightKind of edges produced while this context
    /// is active. `None` for pipeline directives and `{end}`.
    pub weight_kind: Option<WeightKind>,

    /// Async query pipeline, run by `generate_html_for_path` before the sync builder.
    /// Empty slice = no deferred phase (parse-only directives).
    pub queries: &'static [DirectiveRefiner],

    /// Sync deferred-render builder. `None` for parse-only directives.
    /// Builders **must filter by edge kind** — the slice contains everything fetched by
    /// all prior pipeline steps.
    pub builder: Option<DirectiveBuilder>,
}
```

**Sentinel derivation**: there is no `sentinel` field on `DirectiveDef`. Sentinels are
derived on demand from the directive name as `<!--@@noet-{name}@@-->` (underscores
replaced with hyphens) by `myst::sentinel(name)`. Only directives with `builder: Some(…)`
produce a sentinel; parse-only directives return `""`.

**`is_block_opener` is derived**: `myst::is_block_opener(name)` is equivalent to
`myst::global_verb_context(name).is_some()` — a directive is a block opener if and only
if it is a registered codespan relation verb.

### 3.1. Registered Directives

**Pipeline directives** (fenced-block form, deferred render):

| Name | Derived sentinel | Pipeline steps |
|------|-----------------|----------------|
| `network_children` | `<!--@@noet-network-children@@-->` | 1 |
| `requirements_table` | `<!--@@noet-requirements-table@@-->` | 2 |
| `maps_to` | `<!--@@noet-mapping-table:ANCHOR@@-->` (per section) | 1 |
| `query` | `<!--@@noet-query:N@@-->` (per instance) | special¹ |

¹ `{query}` uses `builder: None` and `queries: &[]`. It does not use the standard
`DIRECTIVES` pipeline; instead, a special-case block in `generate_html_for_path`
reads serialized `QuerySpec` objects from the document node's `metadata["_query_specs"]`
and evaluates them at splice time. See §6.2.

**Codespan relation verbs** (parse-only, no sentinel):

| Name | `weight_kind` | `ref_role` | Meaning |
|------|--------------|-----------|---------|
| `uses` | Pragmatic | Source | I consume/depend on these |
| `implements` | Pragmatic | Source | I satisfy these (legacy alias for `uses`) |
| `used_by` | Pragmatic | Sink | These consume/depend on me |
| `draws_from` | Epistemic | Source | I cite/derive from these |
| `underlies` | Epistemic | Sink | These derive from me |
| `composed_of` | Section | Source | I contain these parts (preferred) |
| `consists_of` | Section | Source | I contain these parts (backward-compatible alias for `composed_of`) |
| `component_of` | Section | Sink | I am part of these wholes |
| `end` | — | — | Close relation context (pop stack) |

**Backward compatibility**: The legacy `<!-- network-children -->` HTML comment in
existing files sets `has_network_children` in `MdCodec::parse` so `NetworkCodec`
defers correctly, but is **not** converted to a sentinel — authors must migrate to the
fenced-block form for deferred rendering.

### 3.2. Helper Functions

All helper functions derive their behaviour from iterating `DIRECTIVES`:

| Function | Purpose |
|----------|---------|
| `myst::lookup(name)` | `Some(sentinel)` for known names, `None` for unknown |
| `myst::sentinel(name)` | Derived sentinel string, `""` for parse-only directives |
| `myst::directive(name)` | Author-facing fenced-block source form |
| `myst::is_block_opener(name)` | `true` iff the directive is a codespan relation verb |
| `myst::global_verb_context(name)` | `Some((WeightKind, ReferenceRole))` for relation verbs |
| `myst::parse_relation_args(args)` | Parse `{relation}` args into `(Option<name>, WeightKind, ReferenceRole)` |
| `myst::parse_directive_info(info)` | Extract `(name, args)` from any `{name}args` string |
| `myst::parse_directive_options(body)` | Parse `:key: value` options from fenced block body |
| `myst::query_sentinel(index)` | Per-instance sentinel `<!--@@noet-query:N@@-->` |
| `myst::query_sentinel_indices(html)` | Scan HTML for query sentinel indices |
| `myst::splice_sentinels(path, replacements)` | Replace sentinels in an on-disk HTML file |

---

## 4. Three-Phase Processing Model

Directives with `builder: Some(…)` participate in three phases. Parse-only directives
(relation verbs, `{end}`) participate only in Phase 1.

Both syntax forms (fenced-block and codespan) feed into the same phases — the forms are
syntax-agnostic at the registry level.

```
Phase 1 — Parse
  MdCodec::parse()
    ↓ CodeBlock arm: detect CodeBlock(Fenced("{name}")) events
    ↓ Code arm: detect Code("{name}") and Code("{relation}args") events
    ↓ Both arms: myst::parse_directive_info → (name, args)
    ↓ Both arms: myst::lookup → known directive or warning
    ↓ Pipeline directives: set has_deferred_render / has_network_children flags
    ↓ Relation verbs / {end}: dispatch_relation_directive → update relation_context_stack
      and session_verb_registry on MdCodec
    ↓ Link events: routed per relation_context_stack top
      (WeightKind + ReferenceRole → upstream or downstream on IRNode)
    ↓ Original events kept in proto_events for round-trip fidelity

Phase 2 — Immediate HTML (generate_html)
  MdCodec / NetworkCodec
    ↓ render_html_body:
        CodeBlock directive events → Html(sentinel)   [pipeline directives]
        Code directive events → suppressed (no <code> tag)  [relation verbs, {end}]
        Code directive events → Html(sentinel)  [if builder.is_some(), e.g. future use]
    ↓ write_fragment: body wrapped in page template, written to pages/<path>.html
    ↓ sentinel now embedded in on-disk HTML, ready for splicing

Phase 3 — Deferred Pipeline (generate_html_for_path)
  DocumentCompiler::generate_html_for_path
    ↓ evaluate(QueryPackage::balanced(DocumentNodes)) → doc_nodes_graph
    ↓ build BeliefContext from doc_nodes_graph
    ↓ for each directive with builder.is_some() whose sentinel appears in on-disk HTML:
        sentinel = myst::sentinel(d.name)  ← derived, not stored
        for each refiner in d.queries:
          spec = refiner(&ctx, &graphs)
          evaluate(QueryPackage::new(spec)) → graph
          graphs.push(graph)
        html = d.builder(&ctx, &graphs)
        collect (sentinel, html) pairs
    ↓ myst::splice_sentinels(existing_html_path, pairs)
      OR write_fragment(fallback_body) if file does not exist yet
```

### 4.1. Round-Trip Fidelity

`MdCodec::parse` keeps the original `CodeBlock` events (with their source byte ranges) in
`proto_events` unchanged. `generate_source` calls `cmark_resume_with_source_range` which
splices those original bytes verbatim back into the output — preserving the exact
backtick-fence form the author wrote. No re-serialization of directive events occurs.

### 4.2. `should_defer` Signal

`MdCodec::should_defer()` returns `true` when `has_deferred_render` is set (any directive
with `builder: Some(…)` was encountered during parse). `NetworkCodec::should_defer()`
returns `self.0.should_defer() || self.0.has_network_children`. The compiler uses this
signal to enqueue the document's source path into `deferred_html` for Phase 3 processing.

### 4.3. Relation Context Stack

`MdCodec` maintains a `relation_context_stack: Vec<(WeightKind, ReferenceRole, String)>`
during parse. Each codespan relation verb pushes a context entry; `` `{end}` `` pops one.
The stack label (third element) is the directive text as written, used in diagnostics.

- **Heading boundary**: the stack is drained with one warning per unclosed entry.
- **Nested contexts**: fully supported — `{end}` restores the outer context.
- **Default**: when the stack is empty, links are recorded as `(Epistemic, Source)` →
  `upstream` (the historical default, unchanged).
- **`session_verb_registry`**: a per-parse `HashMap<String, (WeightKind, ReferenceRole)>`
  allows documents to register custom verb aliases via
  `` `{relation}name=mitigates, kind=pragmatic, ref=source` ``. Session-first, global-
  fallback lookup via `global_verb_context`. Last-one-wins; shadows built-in verbs with
  a warning.

---

## 5. The `Vec<BeliefGraph>` Query Pipeline

### 5.1. Motivation

`DocCodec` is a **sync** trait; `BeliefSource` (the DB connection) is **async**. Builders
need query results that can only be obtained asynchronously. Passing a full `BeliefBase`
snapshot (the previous approach) required fetching the entire graph — expensive, and
impossible against a live `DbConnection`.

The solution: each `DirectiveDef` declares a declarative, data-only pipeline of **query
refiners**. The async runner in `generate_html_for_path` executes the pipeline, and the
sync builder receives the pre-fetched results.

### 5.2. The Accumulated Slice

`generate_html_for_path` maintains a single `Vec<BeliefGraph>` across all directive
pipelines for one document:

```
graphs[0]     — first refiner’s result (e.g. net_path_in for {network_children})
graphs[1..]   — one entry per subsequent refiner in the directive’s queries
                slice, in order
```

Each refiner receives the resolved node’s `BeliefContext` and `&graphs[..]` — the full
slice accumulated so far. It returns a `QuerySpec`; the result of
`evaluate(QueryPackage::new(spec))` is appended before the next refiner or builder is
called.

**Index conventions for refiner authors:**
- `graphs[0]` — the resolved document node (use `node_bid_from_graphs` to extract the BID)
- `graphs[graphs.len()-1]` — the immediately preceding step's result

**Builder contract:** builders receive the full slice. They **must filter by edge kind** —
earlier pipeline steps for other directives may have appended graphs with unrelated edges.
Both `build_listing_html` (filters `WeightKind::Section`) and
`build_requirements_table_html` (filters `WeightKind::Pragmatic`) satisfy this contract.

### 5.3. Extracting the Resolved Node BID

```rust
fn node_bid_from_graphs(graphs: &[BeliefGraph]) -> Bid {
    // graphs[0] contains the document node as the sole non-Trace state,
    // plus Trace neighbours pulled in via its immediate edges.
    // BTreeMap ordering may put a Trace neighbour before the document node,
    // so we search specifically for the non-Trace state.
    graphs[0]
        .states
        .values()
        .find(|n| !n.kind.contains(BeliefKind::Trace))
        .or_else(|| graphs[0].states.values().next())
        .map(|n| n.bid)
        .expect("graphs[0] is the node-resolution graph and must be non-empty")
}
```

> **Why not `.next()`?** A balanced query for a document node returns that node plus
> Trace neighbours pulled in via halo edges (e.g. the parent network node from the
> Section edge). `BTreeMap` is ordered by `Bid` value — a Trace neighbour whose BID
> sorts earlier would be returned first, yielding the wrong node. The non-Trace filter is
> the correct guard.

### 5.4. Sentinel-Present Optimization

Before running any pipeline, `generate_html_for_path` reads the existing on-disk HTML
file (if present). Directives whose sentinel is **not found** in that content are skipped
entirely — no DB round-trips, no builder calls. When the file does not yet exist (first
compile, or `html_output_dir` not configured), all directives with non-empty sentinels are
run unconditionally.

### 5.5. Cost

Each entry in `queries` is one `evaluate(QueryPackage)` call — one DB round-trip in
production (or one in-memory `BeliefBase::evaluate` call in tests). The pipeline runs
once per document per compile, not per request.

| Directive | Round-trips |
|-----------|-------------|
| `network_children` | 1 |
| `requirements_table` | 2 |

Keep pipelines to 1–3 steps. Deep pipelines increase compile time proportionally.

---

## 6. Built-In Directive Pipelines

### 6.1. `{network_children}`

**Purpose**: Renders an HTML `<ul>` listing the direct child documents of a network node,
sorted by `WEIGHT_SORT_KEY`. Used in network index pages to auto-generate navigation.

**Author syntax**:
```
````{network_children}
````
```

**Backward compat**: `<!-- network-children -->` in existing files produces identical
output. No migration is required.

**Pipeline** (1 step):

```
graphs[0]  — network node (document being rendered; must have kind.is_network())
graphs[1]  — result of: Section traversal from home_net_bid with depth=Max
             → all nodes reachable via Section edges from the network root
             → these are the network's child documents
```

**Refiner** (`net_path_in`):

The `{network_children}` directive uses `net_path_in` as its refiner, which constructs
a `QuerySpec` with `Subject::Bids(vec![home_net_bid])` and a Section-edge traversal
at unbounded depth to collect all nodes belonging to the document's home network.

**Builder** (`build_listing_html`):
1. Builds a temporary `BeliefBase` from `graphs[0]` ∪ `graphs[1]` for
   `ExtendedRelation::new` and bref lookups.
2. Iterates `raw_edges()` filtering for `sink == node_bid` with `WeightKind::Section`.
3. Sorts by `WEIGHT_SORT_KEY`, groups by subdirectory, renders linked `<li>` items.
4. Returns `"<p><em>No documents in this network yet.</em></p>"` when the child list is
   empty.

### 6.2. `{requirements_table}`

**Purpose**: Renders an HTML table summarizing `WeightKind::Pragmatic` edges — i.e.,
`{implements}` links — from nodes in the document's home network to external requirement
nodes. Used to generate traceability matrices.

**Author syntax**:
```
````{requirements_table}
````
```

**Pipeline** (2 steps):

```
graphs[0]  — document node (the page containing the directive)
graphs[1]  — result of: Section traversal from home_net_bid with depth=Max
             → all nodes reachable via Section edges from the network root
graphs[2]  — result of: Pragmatic traversal from all_home_bids with depth=1
             → all Pragmatic edges from home-network nodes to external requirements
```

**Refiners**:

Step 1 (`net_path_in`): Constructs a `QuerySpec` with
`Subject::Bids(vec![home_net_bid])` and a Section-edge traversal at unbounded
depth to collect all nodes in the home network. This is the same refiner used
by `{network_children}`.

Step 2 (`req_table_step2`): Takes the home-network node set from `graphs[0]`
(the `net_path_in` result) and constructs a `QuerySpec` that traverses
Pragmatic edges from those nodes. Uses a `TraversalSpec` with
`Role::Source` input, `WeightKind::Pragmatic` kind filter, and
`Role::Sink` output to find external requirement nodes.

```rust
fn req_table_step2(ctx: &BeliefContext, graphs: &[BeliefGraph]) -> QuerySpec {
    let home_net_bid = if ctx.node.kind.is_network() {
        ctx.node.bid
    } else {
        ctx.home_net
    };
    let all_net_bids: Vec<Bid> = /* collect BIDs from graphs[0] via section subgraph */;
    QuerySpec {
        subject: Subject::Bids(all_net_bids),
        projection: vec![ProjectionStep::traverse(TraversalSpec {
            input_roles: Role::Source.into(),
            kind_filter: WeightKind::Pragmatic.into(),
            output_roles: Role::Sink.into(),
            depth: TraversalDepth::count(1),
        })],
    }
}
```

**Builder** (`build_requirements_table_html`):
1. Collects all BIDs from `graphs[1]` (home network) as the "internal" set.
2. Iterates Pragmatic edges from `graphs[2]`, grouping by sink (requirement) → sources
   (implementors), excluding sinks that are themselves in the home network.
3. Builds a temporary `BeliefBase` from all three graphs for title/URL resolution.
4. Renders a two-column HTML table: `| Requirement | Implemented By |`.
5. Returns an empty-state message when no Pragmatic edges are found.

### 6.2. `{query}` — Compile-Time Query Rendering

The `{query}` directive evaluates a query string against the compiled BeliefBase
and renders the results as an inline HTML table. Unlike other pipeline directives,
`{query}` uses `builder: None` and `queries: &[]` — it bypasses the standard
`DIRECTIVES` pipeline entirely and is handled by a special-case block in
`generate_html_for_path`.

**Syntax** (fenced-block only — bare codespans render as `<code>`):

````markdown
```{query}
:view: depth0
:caption: Applicable Requirements
k-pragmatic-s(1)
```
````

The body is a raw query string (parsed by `query_parser::parse`). Directive
options (`:key: value` lines before the body) control view configuration:

| Directive option | Params key | Default |
|-----------------|------------|----------|
| `:view:` | `"display"` | `"depth0"` |
| `:sort:` | `"sort"` | (none) |
| `:max-rows:` | `"max_rows"` | `200` |
| `:caption:` | `"caption"` | (none) |
| `:columns:` | `"columns"` | (none) |

**Three-phase processing**:

**Phase 1 (parse)**: `MdCodec::parse` detects each `{query}` fenced block,
accumulates the body text, calls `query_parser::parse()` (pure — no BeliefBase
needed), and stores the result in `MdCodec::query_blocks`. On parse error, the
error message is stored instead.

**Phase 2 (HTML emission)**: `render_html_body` emits a per-instance sentinel
`<!--@@noet-query:N@@-->` where N is the 0-based block index across the document.
A `Cell<usize>` counter tracks indices across the `flat_map` over proto events.

**Phase 2b (metadata injection)**: `inject_context` serializes the accumulated
specs and options into the document node's `BeliefNode.metadata`:
- `metadata["_query_specs"]` — JSON-serialized `QuerySpec` array (or `{"error":"msg"}`)
- `metadata["_query_options"]` — JSON-serialized `toml::Table` array (directive options)

**Phase 3 (deferred evaluation)**: `generate_html_for_path` contains a special-case
block (parallel to the `maps_to` block) that:
1. Reads `_query_specs` and `_query_options` from the document node's metadata
2. Scans on-disk HTML for `<!--@@noet-query:N@@-->` sentinels
3. For each sentinel: deserializes the spec, resolves `Subject::Implicit` →
   `Subject::Bids(vec![node_bid])`, looks up the view factory via `VIEWS.get()`,
   evaluates the query, renders to HTML, wraps with caption if specified
4. On any error: emits `<div class="noet-query-error">` instead of panicking

**Why JSON in TOML metadata?** `QuerySpec` contains Rust enums with data
(`Subject::Bids(Vec<Bid>)`, `StepOperation::Traverse(TraversalSpec)`, etc.) that
cannot round-trip through TOML natively (no null, no heterogeneous arrays, no
externally-tagged enum support). JSON serialization via `serde_json` is used
instead, stored as string values in the TOML metadata table.

### 6.3. Codespan Relation Directives (parse-only)

Relation verbs are codespan toggle directives — they push and pop a relation context
stack in `MdCodec`. All nine built-in verbs plus the `{end}` closer are registered in
`DIRECTIVES` with `ref_role` and `weight_kind`; they produce no HTML output.

**Verb table** (see `docs/design/dag_model.md` §3 for the subject/verb/referent model;
`docs/design/engineering_model_ontology.md` §7.2 for normative coupling on the epistemic axis):

| Verb | `weight_kind` | `ref_role` | Referent slot | Subject slot |
|------|--------------|-----------|--------------|-------------|
| `{composed_of}` | Section | Source | source | sink (preferred) |
| `{consists_of}` | Section | Source | source | sink (alias for `{composed_of}`) |
| `{component_of}` | Section | Sink | sink | source |
| `{constrained_by}` | Epistemic | Source | source | sink (preferred for normative coupling) |
| `{constrains}` | Epistemic | Sink | sink | source (preferred for normative coupling) |
| `{draws_from}` | Epistemic | Source | source | sink (alias for `{constrained_by}`) |
| `{underlies}` | Epistemic | Sink | sink | source (alias for `{constrains}`) |
| `{uses}` | Pragmatic | Source | source | sink |
| `{implements}` | Pragmatic | Source | source | sink (alias for `{uses}`) |
| `{used_by}` | Pragmatic | Sink | sink | source |

**Usage**:

```markdown
`{uses}`
- [[REQ-001]] recorded as Pragmatic upstream (referent is source)
- [[REQ-002]] recorded as Pragmatic upstream
`{end}`

`{relation}kind=epistemic, ref=sink`
- [[Derived-Claim]] recorded as Epistemic downstream (referent is sink)
`{end}`
```

A stray `` `{end}` `` with no open context emits a warning. Nested verbs push additional
stack entries; each `` `{end}` `` pops one and restores the outer context.

**Custom verbs**: `` `{relation}name=mitigates, kind=pragmatic, ref=source` `` registers
`mitigates` in the per-document session registry. Subsequent `` `{mitigates}` `` codespans
push the registered context. Shadowing a built-in verb emits an info-level warning.

---

### 6.4. `{#__continue}` — Heading Continuation (parse-only)

`{#__continue}` is a **magic heading anchor** that folds a heading back into the
preceding section node instead of creating a new one. It uses the standard Markdown
explicit-anchor syntax (`{#id}`) rather than the backtick-fence directive form, and is
therefore **not** registered in the `DIRECTIVES` array. It is detected entirely within
`MdCodec::parse` at the `End(Heading)` event.

**Syntax:**

```markdown
## My Section

First paragraph.

## Continued {#__continue}

Second paragraph that belongs to "My Section".
```

**Behaviour:**

- At `Start(Heading)`, the explicit anchor `__continue` is captured and stored in the
  new `IRNode`'s `document["id"]` field, exactly like any other `{#anchor}`.
- At `End(Heading)`, before the title-match and empty-title merge checks, the parser
  tests whether `document["id"] == "__continue"`. If so, the new proto is popped from
  the event stack and its event stream is appended to the prior node's event stream —
  the same merge path used for empty-title and repeated-title headings.
- The heading tag itself (e.g. `## Continued {#__continue}`) is preserved in the
  merged event stream. The annotation is **never stripped** on write-back; this ensures
  idempotency across multiple parse cycles.
- `seen_ids` collision tracking is **not** triggered for `__continue` headings, because
  the merge fires before a title is committed and the duplicate-anchor check only runs
  for section nodes that were actually created.
- The constant `MAGIC_CONTINUE_ID = "__continue"` is exported from `src/codec/md.rs`
  as the single source of truth.

**Use cases:**

- Breaking a long section into multiple visually-separated subsections in source while
  keeping them as a single belief node in the graph.
- Adding a thematic heading mid-section for readability without introducing a new
  addressable anchor.

**What it does not do:**

- `{#__continue}` does not merge across document boundaries or across different heading
  levels — it only merges with whatever node was most recently pushed onto
  `current_events`, which is always within the same document parse pass.
- It does not suppress the heading from HTML output. Authors who want the heading
  visible in rendered HTML use `{#__continue}` normally; authors who want to suppress it
  entirely should use a comment or omit the heading.

---

## 7. Splicing: `splice_sentinels`

After builders produce HTML, `generate_html_for_path` splices the results into the
on-disk HTML file:

```rust
pub(crate) fn splice_sentinels(
    path: &std::path::Path,
    replacements: &[(&str, &str)],  // (sentinel, html) pairs
) -> Result<bool, BuildonomyError>
```

- Reads the file, applies all replacements, writes back if any sentinel was found.
- Returns `true` if at least one replacement was made.
- Sentinels absent from the file are silently skipped (author opt-out / conditional
  rendering).
- If the file does not yet exist (no `html_output_dir` at parse time, or first compile
  before Phase 2), `generate_html_for_path` falls back to `write_fragment` with the
  concatenated builder outputs.

All async/query logic lives in `compiler.rs`. `splice_sentinels` is pure sync I/O.

---

## 8. Extension Point: Adding a New Directive

### Adding a fenced-block pipeline directive

1. **Define the `DirectiveDef`** in `DIRECTIVES` in `myst.rs` with a unique `name`,
   `directive` string, `ref_role: None`, `weight_kind: None`, and implement `queries` /
   `builder`.
2. **Emit the sentinel at render time**: `render_html_body` detects
   `CodeBlock(Fenced("{name}"))` events for all known directives and emits
   `Html(myst::sentinel(name))` — the sentinel is derived automatically. No `marker`
   field or `promote_markers` call needed.
3. **Set parse flags** (if needed): set `has_deferred_render = true` or
   `has_network_children = true` on `MdCodec` to signal `should_defer`.
4. **No changes required** in `compiler.rs` — the `DIRECTIVES` loop gates on
   `d.builder.is_some()` and derives the sentinel via `myst::sentinel(d.name)`.
5. **Bare-codespan mentions** of the directive name (e.g. `` `{my_directive}` `` in
   prose) automatically render as ordinary `<code>` elements — the bare-codespan rule
   (§2.2) ensures only relation verbs and control keywords are treated as directive
   invocations in codespan form without arguments.

**Cost guideline**: Keep `queries` to 1–3 entries. Each entry is one DB round-trip.
Document the pipeline layout (which graph index holds what) in the refiner's doc comment.

### Adding a codespan relation verb

1. **Define the `DirectiveDef`** in `DIRECTIVES` with `weight_kind: Some(…)`,
   `ref_role: Some(…)`, `directive: ""`, `queries: &[]`, `builder: None`.
2. `myst::global_verb_context(name)` and `myst::is_block_opener(name)` pick it up
   automatically — no other changes needed.
3. `MdCodec::parse` detects it in the `Code` arm via the bare-codespan rule (§2.2):
   relation verbs (`weight_kind.is_some()`) are always treated as directive invocations
   in bare codespan form. Dispatched to `dispatch_relation_directive`.
4. Document the verb in `myst.rs` module doc and in `docs/design/dag_model.md` §3.

---

## 9. Design Decisions and Rationale

### 9.1. Backtick-fence over colon-fence

Colon-fence (`:::`) is disqualified by a `pulldown-cmark` serialization bug: closing
`:::` becomes `: ::` on every write-back, making the syntax unusable for round-tripping.
Backtick-fence survives write-back unchanged. See
`docs/project/trades/TRADE_55_MYST_DIRECTIVE_SYNTAX.md` for full empirical evidence.

### 9.2. `Vec<BeliefGraph>` over accumulated `BeliefGraph`

An earlier design accumulated pipeline results via `merge_from` into a single growing
`BeliefGraph`. The `Vec<BeliefGraph>` design is strictly better:

- Refiners can distinguish "what step N-1 returned" vs "what step 0 returned" by index,
  without filtering out content from other steps.
- `req_table_step2` uses `graphs[1].states.keys()` to get exactly the home-network BIDs
  — with accumulation it would need to subtract `graphs[0]`'s states.
- Builders can still union graphs when they need a `BeliefBase` for lookups (temporary,
  ephemeral BB constructed inside the builder).
- The slice is self-documenting: each index maps to a named query result.

### 9.3. Compiler owns splicing (Option B)

`generate_deferred_html` was removed from the `DocCodec` trait entirely. The compiler's
`generate_html_for_path` owns all sentinel splicing. Codecs produce HTML only via the
synchronous `generate_html` method.

**Rationale**: the deferred phase is fundamentally async (it calls
`BeliefSource::evaluate`) and graph-aware (it iterates `DIRECTIVES`). Putting it on a
sync trait required threading a `BeliefBase` snapshot through the codec — leaking async
concerns into a sync interface and forcing a full-graph fetch. With Option B, codecs
remain simple and sync; the compiler’s async context handles all query work.

### 9.4. `BeliefBase::merge` inside builders

Builders construct a temporary `BeliefBase` by calling `bb.merge(graph)` (`pub(crate)`)
to assemble pipeline results for `ExtendedRelation::new` and path/bref lookups. This
`pub(crate)` method performs an unbounded-seed DFS merge — acceptable here because the
input graphs are small (pipeline results only, not the full session BB) and the `BeliefBase`
is ephemeral (dropped at the end of the builder call).

---

## 10. File Map

| File | Role |
|------|------|
| `src/codec/myst.rs` | `DirectiveDef`, `DIRECTIVES`, all helper functions, query refiners, builders, `splice_sentinels` |
| `src/codec/compiler.rs` | `generate_html_for_path`: async pipeline runner, sentinel splicing dispatcher |
| `src/codec/md.rs` | `MdCodec::parse`: directive detection (both `CodeBlock` and `Code` arms), `dispatch_relation_directive`, `relation_context_stack`, `session_verb_registry`, `{#__continue}` merge logic; `render_html_body`: sentinel emission and codespan suppression; `should_defer`; `MAGIC_CONTINUE_ID` constant |
| `src/codec/network.rs` | `NetworkCodec::generate_html`: sentinel emission for `network_children`; `should_defer` override |
| `src/codec/mod.rs` | `DocCodec` trait: `should_defer`, `generate_html` |

---

## 11. References

- `docs/project/trades/TRADE_55_MYST_DIRECTIVE_SYNTAX.md` — empirical analysis of
  backtick-fence vs colon-fence, nesting stability, round-trip safety
- `docs/project/ISSUE_55_MYST_DIRECTIVE_SYNTAX.md` — original implementation issue
- `docs/project/completed/ISSUE_71_GENERALIZE_RELATION_DIRECTIVES.md` — codespan toggle,
  derived sentinels, `ReferenceRole`, `promote_markers` removal
- `docs/design/dag_model.md` §3 — subject/verb/referent model, `ReferenceRole` semantics
- `docs/design/beliefbase_architecture.md` § 3.5 — `DocCodec` trait specification
- `docs/design/architecture.md` § 11 — codec system overview
- MyST specification: https://mystmd.org/guide/syntax-overview
- `src/codec/md.rs` — `MAGIC_CONTINUE_ID` constant and `{#__continue}` merge logic in `MdCodec::parse`