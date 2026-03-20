---
title: "MyST Directive Architecture"
version: "0.1"
---

# MyST Directive Architecture

**Version**: 0.1
**Status**: Current implementation

## 1. Purpose

This document specifies the architecture for noet's MyST directive system — the mechanism
by which authors embed structured, context-dependent content (child document listings,
requirements tables, and future extensions) into Markdown source files using a standard
backtick-fence syntax.

**Scope**: `src/codec/myst.rs`, `src/codec/compiler.rs` (deferred phase), and the
`DocCodec` trait's `should_defer` / `generate_html` methods as they relate to directives.

**Out of scope**: The broader codec system and multi-pass compilation model are specified
in [`beliefbase_architecture.md`](./beliefbase_architecture.md).

---

## 2. Syntax and Authoring Convention

noet extends Markdown with a small set of block directives using the **backtick-fence**
form from the [MyST spec](https://mystmd.org/guide/syntax-overview):

```
````{directive_name}
````
```

**Rules:**

- Use **4 backticks** for top-level directives. A 3-backtick directive is normalized to 4
  on the first write-back and is then stable.
- Nested directives use 3 backticks inside a 4-backtick outer fence — the only stable
  nesting depth under pulldown-cmark.
- The **colon-fence form** (`:::`) is **not supported**. Under pulldown-cmark with
  `ENABLE_DEFINITION_LIST`, the serializer corrupts `:::` to `: ::` on every write-back,
  and a blank line in the body terminates the underlying `DefinitionList` entirely. See
  `docs/project/trades/TRADE_55_MYST_DIRECTIVE_SYNTAX.md` for the full empirical analysis.

**Detection**: pulldown-cmark represents any fenced code block whose info string matches
`{...}` as an ordinary `CodeBlock(Fenced("{name}"))` event — no special directive mode.
`myst::parse_directive_info` extracts the name and optional arguments from the info string.
Unknown names pass through unchanged and emit a `ParseDiagnostic::Warning`.

---

## 3. The `DirectiveDef` Registry

`src/codec/myst.rs` is the **single source of truth** for all directive metadata. Every
directive is declared as a `DirectiveDef` entry in the static `DIRECTIVES` array. No
per-directive logic lives outside this file except in `generate_html_for_path`
(the async pipeline runner) and the codec `generate_html` implementations that emit
markers.

```rust
pub struct DirectiveDef {
    /// Name used in the backtick-fence info string, e.g. `"network_children"`.
    pub name: &'static str,

    /// Render-time intermediate HTML comment emitted by `render_html_body` in place of
    /// the directive's CodeBlock events. Empty = suppressed from HTML output entirely
    /// (e.g. `{implements}`, `{end}`).
    pub marker: &'static str,

    /// Collision-safe placeholder that survives template wrapping. Replaces `marker` in
    /// `generate_html` and is later replaced by the deferred pipeline. Empty = no
    /// deferred phase.
    pub sentinel: &'static str,

    /// Opening-line source form written to new files by `noet init`
    /// (e.g. `"````{network_children}"`). Empty for directives never written
    /// programmatically.
    pub directive: &'static str,

    /// Whether this directive opens a parse-behaviour block (changes link WeightKind
    /// until `{end}` or the next heading).
    pub is_block_opener: bool,

    /// Async query pipeline, run by `generate_html_for_path` before the sync builder.
    ///
    /// `graphs[0]` is always the node-resolution graph. Each refiner receives the full
    /// `&[BeliefGraph]` slice accumulated so far and returns the next Expression to
    /// evaluate. The result is appended before the next refiner is called.
    ///
    /// Empty slice = no deferred phase.
    pub queries: &'static [fn(&[BeliefGraph]) -> Expression],

    /// Sync deferred-render builder. Receives the full accumulated `&[BeliefGraph]`
    /// slice. Builders **must filter by edge kind** — the slice contains everything
    /// fetched by all prior pipeline steps, not only what this directive queried for.
    ///
    /// `None` for parse-only or marker-only directives.
    pub builder: Option<fn(&[BeliefGraph]) -> Result<String, BuildonomyError>>,
}
```

### 3.1. Registered Directives

| Name | Marker | Sentinel | Opener | Pipeline steps |
|------|--------|----------|--------|----------------|
| `network_children` | `<!-- network-children -->` | `<!--@@noet-network-children@@-->` | No | 1 |
| `implements` | _(empty — suppressed)_ | _(empty)_ | **Yes** | 0 |
| `end` | _(empty — suppressed)_ | _(empty)_ | No | 0 |
| `requirements_table` | `<!-- noet-requirements-table -->` | `<!--@@noet-requirements-table@@-->` | No | 2 |

**Marker vs. Sentinel distinction:**

- **Marker** — an HTML comment emitted at render time (`render_html_body` /
  `NetworkCodec::generate_html`). Used internally as an intermediate signal; not
  collision-safe enough to survive template wrapping.
- **Sentinel** — a longer, namespaced placeholder (`<!--@@noet-…@@-->`) substituted for
  the marker in `generate_html`. Survives the `write_fragment` template wrap intact.
  Replaced by real HTML during the deferred pipeline.

**Backward compatibility**: The legacy `<!-- network-children -->` HTML comment in
existing files is treated identically to the MyST backtick-fence form. `NetworkCodec`
detects either form and emits the same sentinel.

### 3.2. Helper Functions

All helper functions derive their behaviour from iterating `DIRECTIVES` — no separate
lookup tables exist:

| Function | Purpose |
|----------|---------|
| `myst::lookup(name)` | Map directive name → marker, `None` for unknown |
| `myst::marker(name)` | Render-time HTML comment for a directive name |
| `myst::sentinel(name)` | Collision-safe placeholder for a directive name |
| `myst::directive(name)` | Author-facing opening-line source form |
| `myst::is_block_opener(name)` | Whether this directive opens a parse block |
| `myst::parse_directive_info(info)` | Extract `(name, args)` from a `Fenced` info string |
| `myst::promote_markers(body)` | Replace all markers with their sentinels in an HTML body |
| `myst::splice_sentinels(path, replacements)` | Replace sentinels in an on-disk HTML file |

---

## 4. Three-Phase Processing Model

Each directive with a non-empty sentinel participates in three phases. Directives with an
empty sentinel (e.g. `{implements}`, `{end}`) participate only in Phase 1.

```
Phase 1 — Parse
  MdCodec::parse()
    ↓ detect CodeBlock(Fenced("{name}")) events
    ↓ myst::parse_directive_info → (name, args)
    ↓ myst::lookup → marker or warning
    ↓ record on IRNode (has_deferred_render, has_network_children, in_implements_block)
    ↓ original CodeBlock events kept in proto_events for round-trip fidelity

Phase 2 — Immediate HTML (generate_html)
  MdCodec / NetworkCodec
    ↓ render_html_body: CodeBlock directive events → Html(marker)
    ↓ myst::promote_markers: marker → sentinel in output HTML body
    ↓ write_fragment: body wrapped in page template, written to pages/<path>.html
    ↓ sentinel now embedded in on-disk HTML, ready for splicing

Phase 3 — Deferred Pipeline (generate_html_for_path)
  DocumentCompiler::generate_html_for_path
    ↓ eval_query(nodekey) → graphs[0]  (node-resolution graph)
    ↓ for each directive whose sentinel appears in the on-disk HTML:
        for each refiner in d.queries:
          expr = refiner(&graphs)
          graphs.push(eval_query(expr))
        html = d.builder(&graphs)
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
with a non-empty sentinel was encountered during parse). `NetworkCodec::should_defer()`
returns `self.0.should_defer() || self.0.has_network_children`. The compiler uses this
signal to enqueue the document's source path into `deferred_html` for Phase 3 processing.

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
graphs[0]     — node-resolution graph (always present; produced by the initial
                eval_query for the document's NodeKey before any directive runs)
graphs[1..]   — one entry per eval_query call, in the order directives appear
                in DIRECTIVES and their queries slices
```

Each refiner receives `&graphs[..]` — the full slice accumulated so far. It returns an
`Expression`; the result of `eval_query(expr)` is appended before the next refiner or
builder is called.

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

> **Why not `.next()`?** A seed-only `eval_query` for a document node returns that node
> plus Trace neighbours pulled in via immediate edges (e.g. the parent network node from
> the Section edge). `BTreeMap` is ordered by `Bid` value — a Trace neighbour whose BID
> sorts earlier would be returned first, yielding the wrong node. The non-Trace filter is
> the correct guard.

### 5.4. Sentinel-Present Optimization

Before running any pipeline, `generate_html_for_path` reads the existing on-disk HTML
file (if present). Directives whose sentinel is **not found** in that content are skipped
entirely — no DB round-trips, no builder calls. When the file does not yet exist (first
compile, or `html_output_dir` not configured), all directives with non-empty sentinels are
run unconditionally.

### 5.5. Cost

Each entry in `queries` is one `eval_query` call — one DB round-trip in production (or
one in-memory `BeliefBase::evaluate_expression` call in tests). The pipeline runs once
per document per compile, not per request.

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
graphs[1]  — result of: RelationIn(SinkIn([node_bid]))
             → all edges whose sink is the network node
             → sources of those edges are the network's child documents
```

**Refiner**:
```rust
fn network_children_query(graphs: &[BeliefGraph]) -> Expression {
    let node_bid = node_bid_from_graphs(graphs);
    Expression::RelationIn(RelationPred::SinkIn(vec![node_bid]))
}
```

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
graphs[1]  — result of: StateIn(NetPathIn(home_net_bid))
             → all nodes whose path is registered under the home network
graphs[2]  — result of: RelationIn(SourceIn(all_home_bids)) ∩ RelationIn(Kind(Pragmatic))
             → all Pragmatic edges from home-network nodes to external requirements
```

**Refiners**:
```rust
fn req_table_step1(graphs: &[BeliefGraph]) -> Expression {
    // Find the home network BID from graphs[0]. The node-resolution graph may contain
    // a network-kind ancestor node; fall back to node_bid itself (covers the network
    // index page case where the document IS the network).
    let node_bid = node_bid_from_graphs(graphs);
    let home_net_bid = graphs[0].states.values()
        .find(|n| n.kind.is_network())
        .map(|n| n.bid)
        .unwrap_or(node_bid);
    Expression::StateIn(StatePred::NetPathIn(home_net_bid))
}

fn req_table_step2(graphs: &[BeliefGraph]) -> Expression {
    // graphs[1] is the home-network node set from step 1.
    let all_bids: Vec<Bid> = graphs.get(1)
        .map(|g| g.states.keys().copied().collect())
        .unwrap_or_else(|| vec![node_bid_from_graphs(graphs)]);
    Expression::Dyad(
        Box::new(Expression::RelationIn(RelationPred::SourceIn(all_bids))),
        SetOp::Intersection,
        Box::new(Expression::RelationIn(RelationPred::Kind(WeightKind::Pragmatic.into()))),
    )
}
```

**Builder** (`build_requirements_table_html`):
1. Collects all BIDs from `graphs[1]` (home network) as the "internal" set.
2. Iterates Pragmatic edges from `graphs[2]`, grouping by sink (requirement) → sources
   (implementors), excluding sinks that are themselves in the home network.
3. Builds a temporary `BeliefBase` from all three graphs for title/URL resolution.
4. Renders a two-column HTML table: `| Requirement | Implemented By |`.
5. Returns an empty-state message when no Pragmatic edges are found.

### 6.3. `{implements}` and `{end}` (parse-only)

`{implements}` is a **block opener**: all Markdown links inside the block until the next
`{end}` or heading are recorded as `WeightKind::Pragmatic` upstream relations instead of
the default `WeightKind::Epistemic`. Both directives and the block's content are
suppressed from HTML output entirely (empty marker, empty sentinel, no pipeline).

```
````{implements}
[Requirement XYZ](../requirements/xyz.md)
````{end}
```

Nesting `{implements}` inside another `{implements}` emits a warning and implicitly
closes the first block. A stray `{end}` with no open block also emits a warning.

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

To add a new directive:

1. **Define the `DirectiveDef`** in the `DIRECTIVES` array in `myst.rs`:
   - Choose a unique `name`, `marker`, and `sentinel`.
   - If the directive needs deferred content, write one or more refiner functions and a
     builder function; set `queries` and `builder`.
   - If it is parse-only (e.g. a block opener), leave `queries: &[]` and
     `builder: None`.

2. **Emit the marker at render time**: In `render_html_body` (for inline directives in
   `MdCodec`) or `NetworkCodec::generate_html` (for network-specific directives), detect
   the `CodeBlock(Fenced("{name}"))` event and emit `Html(marker)`. `promote_markers`
   (called from `generate_html`) converts the marker to the sentinel automatically.

3. **Register parse effects** (if any): Set flags on `MdCodec` (e.g.
   `has_deferred_render = true`, `in_implements_block`) to signal `should_defer` and
   alter parse behaviour for subsequent events.

4. **No changes required** in `compiler.rs`, `mod.rs`, `md.rs`, or `network.rs` for
   directives that fit the standard pipeline pattern. The `DIRECTIVES` array iteration
   handles discovery automatically.

**Cost guideline**: Keep `queries` to 1–3 entries. Each entry is one DB round-trip.
Document the pipeline layout (which graph index holds what) in the refiner's doc comment.

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

**Rationale**: the deferred phase is fundamentally async (it calls `eval_query`) and
graph-aware (it iterates `DIRECTIVES`). Putting it on a sync trait required threading a
`BeliefBase` snapshot through the codec — leaking async concerns into a sync interface
and forcing a full-graph fetch. With Option B, codecs remain simple and sync; the
compiler's async context handles all query work.

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
| `src/codec/md.rs` | `MdCodec::parse`: directive detection; `render_html_body`: marker emission; `generate_html`: `promote_markers` call; `should_defer` |
| `src/codec/network.rs` | `NetworkCodec::generate_html`: marker/sentinel handling for `network_children`; `should_defer` override |
| `src/codec/mod.rs` | `DocCodec` trait: `should_defer`, `generate_html` |

---

## 11. References

- `docs/project/trades/TRADE_55_MYST_DIRECTIVE_SYNTAX.md` — empirical analysis of
  backtick-fence vs colon-fence, nesting stability, round-trip safety
- `docs/project/ISSUE_55_MYST_DIRECTIVE_SYNTAX.md` — original implementation issue
- `docs/design/beliefbase_architecture.md` § 3.5 — `DocCodec` trait specification
- `docs/design/architecture.md` § 11 — codec system overview
- MyST specification: https://mystmd.org/guide/syntax-overview