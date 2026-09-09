---
title = "Query Model"
version = "0.1"
---

# Query Model

Design document formalizing the query, search, and traceability model for noet-core's beliefbase.

## 1. Problem Statement

The pre-implementation system has three separate query surfaces that share structure but had no unified model:

**`query.rs`** — `Expression` / `SetOp` / `NeighborsExpression` / `Query`: an unordered node-set evaluator backed by SQLite and an in-memory `BeliefGraph`. Returns a `BeliefGraph` with no ordering guarantee. Property predicates and set compositions are well-modeled; structural traversal is limited to a single bidirectional, WeightKind-unfiltered `NeighborsExpression`.

**MCP tools** — `get_submap`, `get_context`, `get_maps_to_traceability`, `get_traceability`, and friends: each tool encodes a specific predicate composition plus ordering hard-coded in Rust. The API surface grew pragmatically without a unifying model. Adding a new traversal pattern requires a new tool, new types, new handler — even when the new traversal differs from an existing one only in WeightKind or direction.

**Viewer / search** — `bb.search()` WASM endpoint returning TF-IDF ranked results; the traceability panel using submap order; maps-to mode with its own ordering. These three orderings are not composable from the viewer side.

These surfaces expose lines and planes in a higher-dimensional relation space. The missing model is what lets us express **manifolds** — composed, multi-hop, multi-kind queries with principled ordering. Without it, every new use case requires a new special-purpose code path. With it, new use cases are compositions of existing primitives.

> See [`docs/design/core/dag_model.md`](dag_model.md) for the full treatment
> of the mesh/camera metaphor, the three WeightKind axes, the Taylor series depth
> model, and the validation vs. diagnosis modes of engagement.

The concrete trigger for this document is the category-filtered coverage gap query (§9): a
query that requires joining a structural Pragmatic traversal with a maps-to traceability
traversal, filtering to the intersection, and computing the complement (uncovered items).
This query cannot be expressed in any current surface without custom code.

---

## 2. Core Insight: Paths Are the Primitive Unit

**The primitive unit of a query result is a path, not a node.**

A path is an ordered sequence of `(node, edge_kind, direction)` steps through the beliefbase graph. A node set is the projection of a path set onto its members.

This distinction matters because:

- A node can appear as the **start**, a **waypoint**, or the **terminus** of a path. These are different structural facts about why the node is in the result.
- Sort functions, filters, and projections operate on paths first; nodes are derived.
- The same node reached by two different paths carries different `path_info` — path length, edge kinds traversed, projection scores matched at each step.

**Concrete example** — a category-filtered coverage gap query:
- A coverage source node **starts** a path
- An item node is a **waypoint** (sink of the review document's maps-to claim, start of the Pragmatic edge to the category)
- The category classification node is the **terminus**

The traceability table is one rectangular projection of this path set: rows = review document owner nodes, columns = item waypoints, cells = coverage source starts, filter criterion = "waypoint has a Pragmatic edge to the category terminus." The graph view renders the same path set without collapsing it.

**Why this matters for ordering**: a node that appears as a waypoint in 12 paths and a terminus in 3 is structurally more central than a node that is a terminus in 1 path and a waypoint in 0. Sorting by node-intrinsic properties alone (TF-IDF score, schema) cannot capture this. Sort must operate on the path set.

---

## 3. The Unified Pipeline QuerySpec

A complete query is a single pipeline of steps:

```
QuerySpec {
    steps: Vec<ProjectionStep>,    // the full pipeline: seed + transformations
}
```

Each `ProjectionStep` has an input (specified by a `TapeFn`) and a `StepOperation` (its
transformation). The first step's `TapeFn` determines the seed — the origin to begin
measuring from. Subsequent steps chain transformations via `TapeFn::Then(None)` (the
default: "output of the previous step"). Seed `TapeFn` variants can also appear
mid-pipeline, replacing the current BID set with a fresh starting point.

The **view** (§7) is not part of the `QuerySpec`. A view is a consumer-side rendering
trait that reads the evaluated `QueryPackage` (spec + tape + graph) and produces output.
The evaluator does not know or care how results will be displayed — this separation
allows the same spec to be rendered by different views without re-evaluation.

The step pipeline is independent of presentation — it produces a structural result
(the tape). The view is independent of how that result was produced — it reads the
tape and renders.

Current hardcoded combinations are special cases of this general model:

| Current tool / endpoint      | seed (first step TapeFn)      | operations                    | view (consumer-side)              |
|------------------------------|-------------------------------|-------------------------------|-----------------------------------|
| `get_submap`                 | `Bids([anchor])`              | `[Traverse(section, depth=N)]`| EdgeCount, SectionOrder, Table    |
| `get_maps_to_traceability`   | `Bids([anchor])`              | `[Traverse(maps_to, depth=N)]`| MapsToPath, SectionOrder, Table   |
| `get_traceability`           | `Bids([anchor])`              | `[Traverse(section, depth=N)]`| EdgeCount, SectionOrder, Table    |
| `bb.search(query)`           | `Corpus`                      | `[Filter(TextMatch(query))]`  | depth-0, TfIdfScore, Table        |

---

## 4. Seed (TapeFn as Scan Set)

The seed is the starting coordinate in the manifold: a `TapeFn` variant on a step
that produces a `Set<Bid>` independently of any upstream tape entry. The seed is
the initial camera position in the video camera model (§8).

Seed `TapeFn` variants:
- **`TapeFn::Bids(Vec<Bid>)`** — explicit resolved BIDs. The most common case for
  programmatic queries. A single BID (anchor mode) is `Bids(vec![bid])`.
- **`TapeFn::Keys(Vec<NodeKey>)`** — unresolved node keys (`id://`, `bref:`,
  `bid:`, path). Resolved to BIDs at evaluation time. The standard form
  produced by the query parser for anchored queries.
- **`TapeFn::Corpus`** — all loaded nodes. The widest-angle starting point.
- **`TapeFn::DocumentNodes(bref, path)`** — all nodes belonging to a document
  (root + sections). Used by compile-time query directives.

A seed `TapeFn` can appear on **any** step, not just the first. When the evaluator
encounters a seed `TapeFn` mid-pipeline, it discards the upstream BID set and
produces a fresh set from the seed. This enables composition branches with
independent anchors:

```
-- Two independently-seeded pipelines composed:
KEYS(bref:abc) composed_of(1)
AND
KEYS(bref:def) uses(1)
```

When no seed is given (first step has `TapeFn::Then(None)`), the query is
**context-dependent** — the caller must inject a concrete seed `TapeFn` before
evaluation:
- **Directive**: injects `TapeFn::Bids([doc_bid])` (current document)
- **Viewer**: injects `TapeFn::Bids([route_bid])` or `TapeFn::Corpus`
- **MCP**: rejects as an error or defaults to `TapeFn::Corpus`

The `BeliefGraph` is threaded through all steps as a read-only reference.
The manifold does not change during evaluation — only the ranked node set transforms.

---

## 5. Projection (Transformation Chain)

A projection is a chain of declarative steps, each a pure function:

```
(Set<(Bid, Score)>, &BeliefGraph) → (Set<(Bid, Score)>, &BeliefGraph)
```

The `BeliefGraph` is fixed; only the ranked node set transforms. Each step is either a
`NodeFilter` (zero-hop, property-based) or a `Traversal` (graph walk). Compositions
(`And`/`Or`/`Difference`) combine results from parallel projection chains —
stereoscopic vision (see §5.3).

Each `ProjectionStep` pairs a step operation with a `TapeFn` that selects its
input from the tape, plus a label that names its tape entries:

```
struct ProjectionStep {
    label:     String,           // names tape entries for this step (see §5.5, §6)
    input:     TapeFn,           // selects input from tape or seed (see below)
    operation: StepOperation,    // Filter | Traverse | Compose | Identity
}

enum StepOperation {
    Filter(NodeFilter),          // zero-hop property filter (§5.1)
    Traverse(TraversalSpec),     // graph walk (§5.2)
    Compose(Composition),        // set-algebraic composition (§5.3)
    Identity,                    // pass-through: output = input (useful for seed-only queries)
}

/// Input source for a projection step. Either selects from the tape (Then, Fold,
/// Terminal, Orphan) or produces a fresh BID set from a seed (Bids, Keys, Corpus,
/// DocumentNodes). Seed variants ignore any upstream tape state.
enum TapeFn {
    // ── Tape accessors ─────────────────────────────────────────

    /// Output BIDs of a single entry.
    /// `None` (default) = previous entry's output (sequential pipeline).
    /// `Some(ref)` = output of the referenced entry.
    Then(Option<StepRef>),

    /// Fold a set operation across a range of entries.
    /// Applies `op` to the output BID sets: e[a] op e[a+1] op ... op e[b-1].
    /// `None` range = all entries for the previous projection step.
    Fold {
        op: SetOp,
        range: Option<(StepRef, StepRef)>,
    },

    /// Boundary/terminal nodes: output BIDs that never appear as input
    /// BIDs within the range.  Roots in a Section traversal, leaf nodes
    /// in a downward walk.  Internally: union(outputs) \ union(inputs).
    /// `None` range = previous projection step's entries.
    Terminal(Option<(StepRef, StepRef)>),

    /// Orphan nodes: input BIDs that produced no output BIDs within
    /// the range.  Nodes with no edges of the traversed kind.
    /// Internally: union(inputs) \ union(outputs).
    /// `None` range = previous projection step's entries.
    Orphan(Option<(StepRef, StepRef)>),

    // ── Seed variants (see §4) ─────────────────────────────────

    /// Explicit resolved BIDs. Ignores upstream tape.
    Bids(Vec<Bid>),

    /// Unresolved node keys. Resolved at evaluation time.
    Keys(Vec<NodeKey>),

    /// All loaded nodes.
    Corpus,

    /// All nodes belonging to a document (root + heading sections).
    DocumentNodes(Bref, String),
}

enum SetOp {
    Union,           // A ∪ B
    Intersection,    // A ∩ B
    LeftDiff,        // A \ B  (in A, not in B)
    RightDiff,       // B \ A  (in B, not in A)
    SymmetricDiff,   // A △ B  (in exactly one)
}

/// Reference to a tape position.
enum StepRef {
    Label(String),  // by step label name
    Index(usize),   // by absolute tape index
}
```

The `label` field is always present. When not user-specified, it defaults to the step's
zero-based index in the projection chain (e.g., `"0"`, `"1"`, `"2"`). During
evaluation, each expanded tape entry carries a `StepLabel { name: label, hop: N }` —
the label plus the hop index within the step (see §6). Labels are optimization fences
(see §5.5).

`Then(None)` (the default) is standard sequential composition: each step sees only the
previous entry's output. `Fold { op: Union, range: None }` unions all prior tape
entries — the step sees the original seed set combined with every BID emitted so far.
This is useful when a traversal needs to act on the accumulated frontier rather than
the most recent slice (e.g., collecting all pragmatic neighbors of every node visited
across a multi-step section descent). Other `SetOp` variants (`Intersection`,
`LeftDiff`, `RightDiff`, `SymmetricDiff`) enable derived set computations over tape
entry ranges.

The `Score` algebra within compositions (`min`/`max`/mask) is implicit sort — it
determines which nodes survive and at what rank. This is load-bearing for correctness,
not just presentation.

The output of evaluating a projection chain is a **tape**: `Vec<TapeEntry>` — the
ordered record of atomic projection results, one entry per hop/filter/compose (see
§6). The tape is the sole interface between projection and view.

### 5.0 Unified Primitive: Score

Every projection step — whether filtering nodes by intrinsic properties or traversing relations
— produces a `Score` for each candidate node:

```
type Score = Option<f32>

None        — node excluded from result
Some(1.0)   — hard inclusion, full weight
Some(s)     — soft inclusion, attenuated weight  (s ∈ (0.0, 1.0))
```

This is the single output type for all projection steps in the camera pipeline. Hard
steps (`SchemaFilter`, `KindFilter`, `ExplicitSet`) emit `Some(1.0)`. Soft steps
(`TextMatch`) emit `Some(tfidf_score)`. Structural traversals emit `Some(1.0)` for each
reachable node (the `path_info` record carries the structural detail; the score carries
presence). `None` means the node is not illuminated by this stage and is excluded from the
output fed to the next stage.

The `Score` type forms a valid semiring under `(min, max)` — exact on f32, since `min`
and `max` select operands without arithmetic. The `(×, +)` provenance semiring (see §13)
is the mathematical model but only approximately valid on f32: float multiplication and
addition are not associative, so `(a × b) × c` may differ from `a × (b × c)` by ULP
error. For the `min`/`max`/mask algebra used by `And`/`Or`/`Difference` (§5.3), this is
irrelevant — no arithmetic occurs. For chains that arithmetically combine scores (see
float stability note below), the approximation is acceptable for short chains with scores
in `[0, 1]`, but the quantizer step is the mechanism for recovering exact boundaries when
drift accumulates.

> **Note — float stability**: The `min`/`max`/mask algebra used by `And`/`Or`/`Difference`
> is float-stable: these operations select or propagate scores without arithmetic, so a
> score that enters a composition chain as `0.73` exits as `0.73` regardless of chain
> depth. `Some(1.0)` is exactly representable in IEEE 754 and safe as a hard-inclusion
> sentinel.
>
> However, multi-step projection chains may want to **arithmetically combine** scores —
> e.g., attenuating a structural result by a TF-IDF match quality (`score × tfidf`),
> boosting survivors of multiple steps (`score + bonus`), or decaying with traversal
> depth (`score × decay_factor`). These operations introduce float drift: after enough
> `× 0.95` steps, a score that started at `1.0` degrades to `0.77...`, and the clean
> `1.0` sentinel no longer distinguishes hard inclusion from strong-but-soft. When a
> projection chain needs to recover clean score boundaries after arithmetic composition,
> add an explicit **quantizer** step:
>
> ```
> Quantize(threshold: f32)
>     score >= threshold → Some(1.0)    // promote to hard inclusion
>     score <  threshold → Some(score)  // preserve soft score
>     None               → None         // still excluded
> ```
>
> The quantizer is a declared projection step, not hidden rounding — the threshold is
> part of the projection chain's specification. This keeps the algebra honest: arithmetic
> composition is allowed, and boundary recovery is explicit.

### 5.1 NodeFilter (zero-hop step)

A `NodeFilter` is a projection step that down-selects the current node set based on
node-intrinsic properties, with no relation traversal. It is a zero-hop optical stage:
direction, WeightKind, and depth do not apply.

```
NodeFilter(input: Set<Node>, predicate: Node -> Score) -> Set<(Node, Score)>
```

Named NodeFilters:

- **`TextMatch(field, query)`** — soft filter. Emits `Some(tfidf_score)` for each node
  whose `field` matches `query` via the compile-time inverted index. `field` is `title`,
  `content`, or `payload[key]`. Nodes with no match emit `None` and are excluded. The
  TF-IDF score is carried as `path_info.scalar` on each result path.

- **`SchemaFilter(schema)`** — hard filter. Emits `Some(1.0)` for nodes where
  `node.schema == schema`, `None` otherwise. Maps to `StatePred::Schema`.

- **`KindFilter(kinds)`** — hard filter. Emits `Some(1.0)` for nodes where
  `node.kind ∈ kinds`, `None` otherwise. Maps to `StatePred::Kind`.

- **`IdMatch(id)`** — hard filter, at most one result. Emits `Some(1.0)` for the node
  whose `id` field equals the given `id://...` string within a network. The standard way
  to resolve a human-readable identifier to a BID for use as an anchor in a downstream
  `Traversal` stage. Maps to `StatePred::NetId`.

- **`ExplicitSet(bids)`** — hard filter. Emits `Some(1.0)` for nodes whose BID is in the
  given set, `None` otherwise. The fundamental **composition bridge**: the output node set
  of any prior stage can be materialized as BIDs and passed as `ExplicitSet` input to the
  next stage. Maps to `StatePred::Bid`.

> **Note — shard scope and TextMatch**: `TextMatch` is a zero-hop stage with no depth
> parameter. In the WASM viewer, all search indices (`.idx.msgpack` files) are loaded
> eagerly on init — full-corpus search is available immediately, even before any data
> shard is loaded (`shard-manager.js` loads all indices in `_loadAllSearchIndices()`).
> This means `TextMatch` scope is always corpus-wide regardless of which data shards
> are loaded. Data shards (the `BeliefGraph` node/edge data needed for structural
> traversal) are loaded on-demand under a memory budget — but this does not affect
> search index availability. When `TextMatch` is composed with a structural projection
> step via `And(SectionSubmap(anchor, depth), TextMatch(...))`, the text search
> operates over all indices while the structural stage operates over loaded data shards.
> The two loading strategies are independent: search indices are partitioned by network
> but loaded eagerly; data shards are partitioned by network and loaded on-demand.

### 5.2 Traversal (graph walk step)

A `Traversal` is a projection step that maps the current node set to a new node set by
traversing one hop through relations, selecting which relations to follow based on their
properties, and resolving to a specified endpoint of each matched relation.

```
Traversal(
    input:      Set<Node>,
    via:        RelationFilter,   // which relations to include
    resolve_to: Endpoint,         // which node to extract from each matched relation
    depth:      TraversalDepth,   // how to iterate: count or guided
) -> Set<(Node, Score, PathInfo)>
```

Where:

```
RelationFilter = predicate over {
    weight_kind:            WeightKind,         // Section | Epistemic | Pragmatic
    edge_weight_properties: Weight,             // WEIGHT_OWNED_BY, WEIGHT_SORT_KEY, etc.
    source ∈ input | sink ∈ input | owned_by ∈ input.brefs,  // connection to current set
}

Endpoint = Source | Sink | Owner

struct TraversalDepth {
    count:       DepthCount,             // how many hops
    edge_filter: Option<EdgePredicate>,  // which edges per hop
}

enum DepthCount {
    N(u8),                               // fixed iteration count
    Max,                                 // unbounded (*)
}

struct EdgePredicate {
    path:  PropertyPath,                 // dotted key into edge Weight payload
    op:    CompareOp,                    // ==, !=, contains, matches, etc.
    value: PropertyValue,                // comparison operand
}
```

Count and edge filter are orthogonal and compose freely: `Count(3)` alone
follows any matching edge for 3 hops. `EdgePredicate` alone implies
`Count(1)`. Together, the filter applies at every hop for the specified
count. `Count(Max) + edge_filter` = "chase edges matching the filter
until exhaustion or `MAX_TRAVERSAL`."

**Terminal identification** — computing boundary nodes (the frontier where no
further matching edges exist) is a `TapeFn` variant, not a `TraversalSpec`
field.  `TapeFn::Terminal(range)` identifies nodes that appear as outputs
of some hop but never as inputs of any hop within the range.  This is exact
for DAGs (Section trees) and conservative for cyclic edge kinds.  The
complement — `TapeFn::Orphan(range)` — finds input nodes that produced no
outputs.  See §5 for the full `TapeFn` enum.

```
struct TraversalSpec {
    input_roles:    EnumSet<Role>,
    kind_filter:    EnumSet<WeightKind>,
    output_roles:   EnumSet<Role>,       // per-hop: which endpoints to collect
    depth:          TraversalDepth,
}
```

At each hop, `output_roles` determines which endpoints to collect. Those become
the next frontier AND are added to the accumulated result. Multi-hop traversals
expand into per-hop tape entries — one entry per atomic hop, not one per
`ProjectionStep` (see §6). This per-hop granularity enables `TapeFn::Terminal`
and topological sort directly from the tape.

Example: `s-section-k(*)` — "walk Section edges toward roots (from
dependencies to consumers), accumulate all ancestors." To identify roots (nodes with no parent),
use `TapeFn::Terminal(None)` on the traversal's entries — this computes
output BIDs that never appear as input BIDs across the labeled entries.
Since source=child in the Section model, a node that is never a source
has no parent — it is a root.

`EdgePredicate` reuses the same `PropertyPath` / `CompareOp` / `PropertyValue`
types as `PropertyPredicate` (node-property filters, §5.1), but resolves against
the edge `Weight.payload` table rather than the `BeliefNode` TOML representation.
This means any edge weight property — `WEIGHT_SORT_KEY`, `WEIGHT_DOC_PATHS`,
`WEIGHT_OWNED_BY`, `WEIGHT_LINK_TITLE`, or custom payload keys — can be used
as a per-hop filter.

Multi-hop path patterns (e.g., `path:a/b#c`) desugar into a sequence of
single-hop `TraversalSpec` steps, each with `Count(1)` and a different
`edge_filter`. Glob `**` desugars to an intermediate `Count(Max)` step.
The parser handles this expansion; the evaluator sees only primitive steps.

**Path sugar**: filesystem-style path resolution is sugar over a guided section
traversal. The path `"doc.md#section-a"` desugars to:

```
Traversal(
    via: { weight_kind: Section, sink ∈ input },
    resolve_to: Source,
    depth: Guided([
        EdgePredicate { path: ["doc_paths"], op: Eq, value: "doc.md" },
        EdgePredicate { path: ["doc_paths"], op: Eq, value: "section-a" },
    ])
)
```

The `doc_paths` key is `WEIGHT_DOC_PATH` — the slugified path segment
stored on each section edge. A `path://network-id/doc.md#section-a`
reference resolves to `TapeFn::Keys([id:network-id])` + the guided
traversal above. The `path:` prefix in the textual grammar (`docs/query_language.md` §5)
auto-splits on `/` and `#` to produce per-hop `EdgePredicate`s, and
supports `*` (single wildcard) and `**` (recursive descent) globs.

**Index sugar**: `idx:N` selects the Nth edge by `WEIGHT_SORT_KEY`
position. `idx:0` = first child. `idx:1..3` = edges at positions 1
and 2. This desugars to an `EdgePredicate` matching the sort key value.

Common `StatePred` variants map to guided traversals:

| Legacy `StatePred`         | QuerySpec equivalent                                    |
|----------------------------|---------------------------------------------------------|
| `NetPath(net, path)`       | `Anchor(net)` + `Guided([doc_paths==seg1, ...])`        |
| `NetPathIn(net)`           | `Anchor(net)` + `Count(MAX)` section traversal           |
| `DocumentNodes(net, path)` | `Anchor(net)` + `Guided(path segs)` + `Count(MAX)`      |
| `Path(paths)`              | API-root anchor + `Guided(path segs)`                    |
| `NetId(net, id)`           | `Anchor(net)` + filter `id == id`                        |

**Performance**: Guided traversals through section edges can be accelerated
by `PathMapMap`, which is the materialized index of all section-edge traces.
At the start of `eval`, the evaluator scans the `QuerySpec` for guided
section traversals, identifies which network PathMaps are needed, and
resolves against the cached index in O(1) per path instead of walking the
graph. The `PathMapMap` is always available on `BeliefBase`; `DbConnection`
has an equivalent `paths` table.

`Source` and `Sink` are the structural endpoints of the relation. `Owner` resolves to the
node identified by the `WEIGHT_OWNED_BY` edge property — the node that declared the edge
via a `{maps_to}` directive — rather than either structural endpoint.

All named structural predicates are `Traversal` instances with specific parameters:

| Named predicate              | RelationFilter                              | Endpoint | Score      |
|------------------------------|---------------------------------------------|----------|------------|
| `SectionSubmap(anchor, d)`   | `weight_kind=Section, source ∈ input`       | `Source` | `Some(1.0)`|
| `NeighborSet(a, k, Out, d)`  | `weight_kind=k, source ∈ input`             | `Sink`   | `Some(1.0)`|
| `NeighborSet(a, k, In, d)`   | `weight_kind=k, sink ∈ input`               | `Source` | `Some(1.0)`|
| `MapsToTraversal(anchor, d)` | `weight_kind=Pragmatic, owned_by ∈ input.brefs` | `Sink` | `Some(1.0)`|

For `Count(N)`: the traversal is applied iteratively, feeding each output set as
the next input, up to `N` times. A node visited at iteration `d` is not re-visited
at `d+1` (cycle prevention). For `Guided(preds)`: each iteration consumes one
predicate; cycle prevention is implicit (the predicate sequence terminates).
The full path from initial input to terminus is recorded in `path_info.steps` —
one `PathStep` per iteration.

`path_info` records for the named structural predicates:

```noet-core/docs/design/core/query_model.md#L1-14
// SectionSubmap — depth-2 section descendant:
PathInfo {
    steps:     [(anchor_bid, Section, Out), (child_bid, Section, Out), (grandchild_bid, _, _)],
    sort_keys: [anchor_sort_key, child_sort_key, grandchild_sort_key],
    scalar:    Some(1.0),
}

// MapsToTraversal — single owned claim:
PathInfo {
    steps:     [(review_doc_root, WEIGHT_OWNED_BY, Owner), (owner_section, Pragmatic, Out), (item_sink, _, _)],
    sort_keys: [owner_section_key],
    scalar:    Some(1.0),
}
```



> **Note — depth bounds and decidability**: The `depth` parameter is not merely a
> performance guard. Depth-bounded `Traversal` stages correspond to bounded-length regular
> expressions over the edge relation — a decidable fragment (bounded CRPQs, Calvanese et
> al. 2000). Composing unbounded-depth stages with `Not` or `Difference` (§4.3) would
> enter territory where query equivalence is undecidable (Trakhtenbrot 1950). The hard cap
> `MAX_TRAVERSAL` in `query.rs` is load-bearing for decidability; see §11 and §13.4.

> **Note — depth monotonicity**: `Traversal` result sets grow monotonically with depth.
> A future query optimizer can exploit this: `And(Traversal(..., depth=d1), Traversal(...,
> depth=d2))` with identical filter and endpoint and `d1 ≤ d2` simplifies to the shallower
> traversal — it is the binding constraint.

### 5.3 Compositions

Compositions combine the `Score`-bearing outputs of `NodeFilter` and `Traversal` stages.
Because `Score = Option<f32>` forms a semiring, the composition operators follow standard
fuzzy logic — no special-casing needed in the evaluator.

- **`And(p1, p2)`** — intersection. Score: `min(s1, s2)`. Both stages must illuminate a
  node; the weaker signal is the binding constraint. `None` from either side excludes the
  node. `path_info` is the concatenation of both stages' records — the sort function sees
  both traversal histories.

- **`Or(p1, p2)`** — union. Score: `max(s1, s2)`. Either stage suffices; the stronger
  signal wins. For nodes illuminated by both, `path_info` is concatenated. For nodes
  illuminated by one only, `path_info` is that stage's record alone.

- **`Difference(p1, p2)`** — exclusion. Score: `if s2.is_some() { None } else { s1 }`.
  Nodes illuminated by `p2` are masked out; surviving nodes carry `p1`'s score and
  `path_info` unchanged. `p2` is used only as a mask; its `path_info` records are
  discarded. This is the coverage gap operator: "nodes illuminated by stage A but not
  by stage B."

  > **Note — why-not provenance and tractability**: Explaining why a node is absent from
  > a query result is the **why-not provenance** problem (Chapman & Jagadish, SIGMOD 2009),
  > exponential in the general open case. Our `Difference` avoids this because both `p1`
  > and `p2` are bounded, finite, already-evaluated sets — the result is plain set
  > subtraction, not open negation. Caching of `Difference` results must be invalidated
  > on any change to `p2`.

- **`Not(p)`** — complement against the full corpus. Score: `if s.is_some() { None } else
  { Some(1.0) }`. Expensive for large corpora; prefer `Difference(p1, p2)` when the
  positive set is bounded.

The `And`/`Or` score operators (`min`/`max`) are the standard fuzzy logic T-norm and
T-conorm. They satisfy associativity, commutativity, and the distributivity law
`And(p, Or(q, r)) = Or(And(p, q), And(p, r))` — algebraic laws inherited from the
semiring structure that a future query optimizer can exploit for rewriting.

> See [`docs/design/core/dag_model.md` §5](dag_model.md#5-composed-queries-stereoscopic-vision)
> for the full stereoscopic/blind-spot/panoramic metaphor.

The current `QuerySpec` + `SetOp` + composition steps encode a Boolean (hard-only)
version of this model. Migration path: introduce `Score` as the unified output type,
replace `SetOp::Intersection/Union/Difference` evaluations with the `min/max/mask`
operators above, and add `path_info` to query results (see §10). The `TraversalSpec`
already supports `weight_kind` filtering, `Direction`, and role-based endpoint
specification.

#### 5.3.1 Composition Decomposition via Fold

After a composition step, subsequent pipeline stages receive the operator's
`result` by default (via `TapeFn::Then`). To extract a specific decomposition,
use `Fold(op, branch_labels)` where the branch labels reference the composition's
left and right tape entries (labeled `{step}.L` and `{step}.R` by the evaluator):

- **`Fold(Intersection, "N.L", "N.R")`** — BIDs present in both branches.
  Optimized: reuses the stored `intersection` from the Compose tape entry
  when available.
- **`Fold(LeftDiff, "N.L", "N.R")`** — BIDs only in the left branch.
- **`Fold(RightDiff, "N.L", "N.R")`** — BIDs only in the right branch.
- **`Fold(SymmetricDiff, "N.L", "N.R")`** — BIDs in exactly one branch.

Example: find items in category A that have no review coverage (the gap set),
then traverse their pragmatic edges:

```
id://category-a k-pragmatic-s(1)
NOT
id://review-doc o-pragmatic-k(2)
FOLD(LDIFF, "0.L", "0.R") k-pragmatic-s(1)
```

This unifies composition decomposition with the existing `Fold` mechanism —
no separate `ComposeLens` type is needed. The `intersection` field on
`TapeContent::Compose` provides an optimization for the `Intersection` case
but is not required for correctness.

---

### 5.4 Axis Rotation

Any of the three WeightKinds can serve as the traversal axis:

- `SectionSubmap` — Section-axis: traverses Section edges, uses section sort keys, the current default for `get_submap` and `get_traceability`.
- `NeighborSet(wk=Epistemic, ...)` — Epistemic-axis: traverses Epistemic edges, uses path length.
- `NeighborSet(wk=Pragmatic, ...)` — Pragmatic-axis: traverses Pragmatic edges, uses path length.

The depth parameter retains the same meaning (hop count) regardless of axis. The projection WeightKind is independent per step: a Pragmatic-axis traversal step can be followed by a step that counts Section and Epistemic connectivity. This is the "rotation" through relation space referenced in §8. The `PathMap::new(kind, ...)` constructor already accepts any `WeightKind`; the extension needed in `PathMapMap` is an on-demand construction path that builds without permanent registration (see §10).

### 5.5 Labels as Optimization Fences

**Labels are the user's declaration that intermediate results are observable.** The
evaluator must produce tape entries for every labeled step. This constrains query
optimization.

**Intra-step optimization: always safe.** A `Count(Max)` traversal labeled `"balance"`
still produces N hop entries. The evaluator can use `WITH RECURSIVE` SQL, PathMap
acceleration, or any other strategy — as long as it produces the same per-hop edge sets.
Labels don't constrain execution strategy within a step.

**Inter-step optimization: constrained by labels.** Labels are optimization fences
between steps. If steps A and B have labels `"roots"` and `"descendants"` respectively,
the evaluator cannot merge them — a consumer may reference the tape entries for `"roots"`
via `tape.entries_for("roots")`, terminal computation, or sort spec. Merging would
destroy that observable checkpoint.

Auto-generated labels (step index strings) are semantically equivalent to user-supplied
labels — the evaluator treats them identically. This means every step is an optimization
fence by default. This is conservative but correct: no consumer can be surprised by
missing intermediate results.

**Practical impact.** The constraint is mild: PathMap acceleration (the main optimization
today) operates *within* a single guided traversal step. DB-side `WITH RECURSIVE`
operates within a single `Count(Max)` traversal. Neither is affected. Redundant step
elimination (theoretical today) IS constrained — you can't merge labeled steps — but
redundant steps in user-authored queries are rare; the optimizer mostly benefits
auto-generated specs (like `with_graph_context()` appending halo + balance_map).

> **Future**: An `optimizable: bool` flag on `ProjectionStep` (default `false`) could
> relax the fence for auto-generated context steps that no consumer references by name.
> YAGNI for initial implementation.

---

## 6. The Tape

The output of evaluating a projection chain is a **tape**: an ordered record of
atomic projection results. One `TapeEntry` per atomic operation — one hop of a
traversal, one filter application, one composition. A `Count(3)` traversal
expands to 3 tape entries, each recording the edges discovered at that hop depth.
The tape records in discovery order; reversal is a consumer concern.

### 6.1 Data Structures

```
struct TapeEntry {
    /// Step label: user-supplied name, or auto-generated from step
    /// index (e.g. "0", "1", "halo", "balance"). Multi-hop
    /// traversals produce multiple entries sharing the same label;
    /// hop index is derived from position within the label group
    /// via `entries_for(label)` — no explicit hop field needed.
    label:   String,
    content: TapeContent,
    /// Optional parallel payload, 1:1 indexed with edges or BIDs
    /// in `content`. Only present when the step produces non-trivial
    /// sort/score data (TF-IDF, etc.).
    payload: Option<Vec<SortPayload>>,
}

enum TapeContent {
    /// Traversal hop: edge indices into the package graph plus
    /// self-contained output BIDs (so the tape is readable without
    /// graph access). Ordered by WEIGHT_SORT_KEY (sibling order).
    Edges {
        edges:       Vec<EdgeIndex>,
        output_bids: Vec<Bid>,
    },

    /// Filter: BIDs that survived the predicate.
    /// Order inherited from input.
    Nodes(Vec<Bid>),

    /// Compose result: set operation applied to branch outputs.
    /// `left`/`right` are entry ranges for each branch.
    Compose {
        op:     CompositionOp,
        left:   Range<usize>,
        right:  Range<usize>,
        result: Vec<Bid>,
        /// BIDs present in both branches. Enables diff rendering:
        /// `left_unique  = fold_bids(left)  - intersection`
        /// `right_unique = fold_bids(right) - intersection`
        intersection: Vec<Bid>,
    },

    /// Corpus-wide seed: all loaded nodes implicitly in scope.
    /// Zero allocation — not enumerable from the tape.
    Corpus,
}

struct SortPayload {
    /// Scalar score (TF-IDF, decay, boosting). None = hard inclusion.
    score: Option<f32>,
}
```

For traversal entries, `WEIGHT_SORT_KEY` is derivable from the edge (look up
`graph[edge_idx]` → `WeightSet` → sort key). No need to duplicate it in the
payload. The payload is reserved for data NOT in the graph — primarily TF-IDF
scores from `TextMatch`. For most traversal-only queries, `payload` is `None`
(zero allocation overhead).

For `TapeContent::Edges`, `output_bids` stores the resolved output endpoints
directly, making the tape self-contained for BID extraction.  Input BIDs for a
hop are derived from the previous entry's output (or the seed entry for hop 0).

Combined with per-hop granularity, concatenation of entries for a traversal label
produces a topological sort with sibling ordering:

```
// leaves() from api (walks parent → child via Section edges):
  Hop 0: edges [api→net]               output: [net]
  Hop 1: edges [net→doc_a, net→doc_b]  output: [doc_a, doc_b]
  Hop 2: edges [doc_a→sec1, doc_a→sec2, doc_b→sec3]
                                        output: [sec1, sec2, sec3]

// Concatenated outputs = [net, doc_a, doc_b, sec1, sec2, sec3]
// = valid topological order with sibling ordering
```

The tape is the **sole interface** between projection and view. The projection
produces it; the view consumes it via the tape API below and the package graph
(§6.3).

### 6.2 Tape API

The tape provides access and extraction primitives. Set operations (`difference`,
`intersection`, `union`) live on `BTreeSet` — the consumer extracts BID sets from
the entries they care about, then applies whatever set algebra they need.

```
impl Tape {
    // ── Access ────────────────────────────────────
    fn len(&self) -> usize;
    fn get(&self, idx: usize) -> Option<&TapeEntry>;

    /// All entries sharing a label name, in tape order.
    fn entries_for(&self, label: &str)
        -> impl Iterator<Item = (usize, &TapeEntry)>;

    /// Last non-empty entry for a label.
    fn last_entry_for(&self, label: &str)
        -> Option<(usize, &TapeEntry)>;

    // ── BID extraction ─────────────────────────────
    // These need the graph (for Edges entries) and the spec
    // (for role resolution). For Nodes/Compose entries the
    // graph and spec are unused.

    /// Output BIDs for a single entry.
    fn output_bids(&self, idx: usize, graph: &BidGraph,
        spec: &TraversalSpec) -> Vec<Bid>;

    /// Input BIDs for a single entry.
    fn input_bids(&self, idx: usize, graph: &BidGraph,
        spec: &TraversalSpec) -> Vec<Bid>;

    // ── TapeFn evaluation ──────────────────────────────

    /// Evaluate a TapeFn against this tape.  This is the general-purpose
    /// method — all TapeFn variants (Then, Fold, Terminal, Orphan) are
    /// evaluated here.
    fn eval(&self, f: &TapeFn, graph: &BidGraph,
        spec: &TraversalSpec) -> BTreeSet<Bid>;
}
```

Consumers compose these primitives with standard set operations:

```
let a: BTreeSet<Bid> = tape.output_bids(2, graph, spec).into_iter().collect();
let b: BTreeSet<Bid> = tape.output_bids(5, graph, spec).into_iter().collect();
let diff = a.difference(&b).copied().collect();
```

For `TapeFn::Fold { op: Union, range: None }`, `tape.eval(...)` returns the union
of all output BIDs across all prior entries.

### 6.3 QueryPackage and Graph Ownership

The `QueryPackage` carries the tape and an optional `BeliefGraph` that serves as
the tape's backing store for edge-indexed entries:

```
struct QueryPackage {
    original_spec: QuerySpec,
    spec:          QuerySpec,
    tape:          Tape,
    /// Populated during async evaluation. `None` on the sync path
    /// (tape indexes into the source's own graph; caller holds
    /// the source). No lifetime parameter on QueryPackage.
    graph:         Option<BeliefGraph>,
}
```

Both the sync path (`BeliefBase::evaluate_query`) and async path
(`DbConnection::evaluate`, MCP tools) populate `graph` during evaluation.
Each traversal hop adds discovered edges and endpoint nodes into this graph.
The tape records `EdgeIndex` values valid against this owned graph.

Each step in `spec.steps` produces tape entries from its *output*.
The step's `TapeFn` input is resolved by the evaluator and fed into
the step; only what the step produces goes into the tape. An Identity
step passes its input through as `TapeContent::Nodes`. A traversal
produces one `TapeContent::Edges` entry per hop.

**Result extraction** uses `TapeFn` as a lens over the tape:
- `TapeFn::Then(None)` → last entry's output ("final frontier")
- `TapeFn::Fold { op: Union, range: None }` → union of all entries
  ("full tree" — all nodes discovered at any depth)
- `TapeFn::Terminal(None)` → leaf/root nodes of the traversal
- A label or index reference → entries for that specific step

Views specify which lens to use when extracting the display set from the
tape. `materialize_graph` uses the union of all user-step entries as the
primary set (full state); graph context entries (halo/balance) are Trace.

The package graph + tape together ARE the evaluation output. No separate
`EvalOutput` enum or `materialize_graph` step.

**Constructors:**

- `QueryPackage::new(spec)` — bare evaluation. Tape records BID sets only. Suitable
  for table views, search results, and any consumer that doesn't need edge data.
- `QueryPackage::balanced(spec)` — appends halo + section-root traversal steps to
  the effective spec. After evaluation, the package graph contains a balanced,
  self-contained graph with ancestor chains and edge-endpoint context. Replaces
  the old `View::Graph` evaluator branch. Use this when the consumer needs edge
  data (graph rendering, `to_event_stream`, node context lookups).

---

## 7. View

A view is a **consumer-side trait** that reads an evaluated `QueryPackage` (spec +
tape + graph) and produces rendered output. The view is not part of the `QuerySpec`
— it is external to the query model. The evaluator does not know or care how results
will be displayed.

```noet-core/docs/design/core/query_model.md#L1-7
trait View {
    type Output;
    fn render(&self, package: &QueryPackage) -> Result<Self::Output, Error>;
}

// QuerySpec has no `view` field. It is purely structural.
```

This means:

- The same `QueryPackage` can be rendered by multiple views without re-evaluation.
- The evaluator never branches on display intent. All optimization information comes
  from the spec itself (via `TapeFn` variants, §5).
- `EvalOutput` is deleted — the package graph + tape IS the evaluation output.
- `produce_output` and `materialize_graph` move from the evaluator into view
  implementations (or convenience methods on `QueryPackage`).

### 7.0 Balanced Packages

Consumers that need a self-contained graph (balanced, with ancestor chains and
edge-endpoint context) call `QueryPackage::balanced()` before evaluation:

```noet-core/docs/design/core/query_model.md#L1-3
let mut package = QueryPackage::balanced(spec);
source.evaluate(&mut package).await?;
// package now contains a balanced graph with halo + ancestry
```

This is a `QueryPackage` constructor — not a spec method, not a view concern — that
appends halo and section-root traversal steps to the effective spec. It replaces the
old `View::Graph` evaluator branch. Consumers that only need BID sets (table views,
search results) use `QueryPackage::new(spec)` instead and skip the graph context
overhead entirely.

### 7.1 The View Trait

A view encompasses four responsibilities:

- **Result lens** — which BIDs from the tape constitute "the result."
  Expressed as a `TapeFn`: `Then(None)` for the final frontier,
  `Fold(Union, None)` for the full tree, a label reference for a
  specific step's output. The lens determines which nodes the view
  iterates over.
- **Display schema** — what to show per node (edge counts, maps-to paths, flat list)
- **Sort** — display ordering of the result set for human consumption
- **Render mode** — how to composite tape frames into output

The view reads node BIDs from the tape via its result lens and reads
relation context from the package graph (§6.3) to populate its columns,
rows, or graph edges.

**Concrete implementations:**

**`TableView`** — static composite. Rows are nodes from the tape's result set, ordered
by a `SortSpec`. Columns are the display schema (edge counts, maps-to paths, etc.).
Static views flatten multiple tape frames into a single output — a cubist rendering,
superimposing simultaneous perspectives into one frame. Multi-path nodes use
intra-table back-references.

**`ListView`** — flat node list with title, kind, schema, score. No relation columns.
Suitable for search results and simple BID-set queries.

Future implementations:

- **`GraphView`** — dynamic spatial rendering (force-directed). Renders the package
  graph spatially. Multi-path nodes have multiple edges rendered naturally. Interactive
  mode can animate transitions between tape frames or overlay them with edge color
  encoding which projection step contributed each edge.
- **`DiffView`** — side-by-side or inline diff of two tape states. Could be a
  standalone view or a mode within `traceability.js`.
- **Interactive viewer** (`traceability.js`) — the existing JavaScript viewer is
  already a view implementation: it takes a `QueryPackage` (via WASM/MCP), reads
  the graph context and edge lists, and renders either a normal edge-count table
  or a maps-to traceability matrix. The viewer's control bar is a spec editor:
  the subject selector maps to the first step's seed `TapeFn`, the step cards
  map to the step pipeline, and `renderMode` / `sortOrder` map to view
  configuration.

### 7.2 Display Schema

The display schema defines what relation properties to expose per node in the rendered
output.

- **Depth 0** — just the node itself (title, kind, schema, score from the tape). A flat
  node list with no relation columns. This is what `TextMatch` search returns.

- **EdgeCount(weight_kinds, directions)** — for each node in the tape's final result set,
  count edges per WeightKind per direction from the package graph. The column set
  is the cross-product of `weight_kinds × directions`. Example:

  | Node          | Sec.In | Sec.Out | Ep.In | Ep.Out | Pr.In | Pr.Out |
  |---------------|--------|---------|-------|--------|-------|--------|
  | item-001      | 1      | 0       | 3     | 0      | 0     | 2      |
  | item-002      | 1      | 0       | 1     | 0      | 0     | 1      |

- **MapsToPath** — the specialized `WEIGHT_OWNED_BY → Pragmatic → sink` display layout.
  For each node in the tape, the view traverses: owner → WEIGHT_OWNED_BY edges →
  intermediate claim nodes → Pragmatic edges → sink (item) nodes. The resulting table has
  owner nodes as rows, item sink nodes as column headers, and intermediate claim nodes
  (coverage sources) as cell values.

  Example layout:

  | Owner section  | item-001       | item-002  | item-003       |
  |----------------|----------------|-----------|----------------|
  | Review §4.1    | src-A, src-B   | —         | src-C          |
  | Review §4.2    | —              | src-D     | → (§4.1, item-003) |

  The `→ (§4.1, item-003)` cell is an intra-table back-reference: src-C was already
  placed at row §4.1 / col item-003. The view maintains a
  `visited: HashMap<Bid, (row, col)>` to detect and render these.

- **RelationPath([(wk1, dir1), (wk2, dir2), ...])** — the general case. Each hop in the
  sequence adds a level of nesting to the column schema. Depth > 2 is not yet
  implemented (see §11).

### 7.3 Sort (Display Ordering)

Sort is a **view concern**, not a spec concern. The `SortSpec` lives on the view
implementation, not on `QuerySpec`.

Sort has a dual role:

- **Within projection** (implicit): the `Score` algebra (`min`/`max`/mask) during
  `And`/`Or`/`Difference` compositions determines which nodes survive and at what rank.
  This is load-bearing for correctness, not just presentation.
- **Within view** (explicit): display ordering of the tape's final result set for
  human consumption. This is the sort configured on the view.

#### Tape-derived sort dimensions

The tape (§6) already captures multi-dimensional ordering information that the view
can interrogate without a separate sort pass:

| Dimension           | Tape source                                      | How to read it                                              |
|---------------------|--------------------------------------------------|-------------------------------------------------------------|
| **Topological**     | `TapeContent::Edges` concatenated across hops    | Entry order within a traversal label is a valid topological sort (§6.1). Parent appears before child. |
| **Sibling**         | `WEIGHT_SORT_KEY` on edges in `TapeContent::Edges` | Edges within a single hop are ordered by `WEIGHT_SORT_KEY`. Concatenated hop outputs give topological + sibling ordering. |
| **Hop depth**       | Position within `entries_for(label)` group       | Index 0 = directly connected to anchor; higher indices = more distant hops. |
| **Arbitrary score** | `SortPayload.score` on `TapeEntry.payload`       | TF-IDF scores, decay factors, or boosting weights from `TextMatch` or future scoring steps. |
| **Composition**     | `TapeContent::Compose { left, right, result }`   | How many branches contribute a node; which branch produced it. `IntersectionCardinality` counts branches. |

A view selects its sort function by reading the relevant dimension from the tape.
`SectionOrder` reads topological + sibling order. `TfIdfScore` reads the payload
score. `PathLength` reads the entry's position within its label group. `IntersectionCardinality` counts compose
branches. The tape is the sort's input — no auxiliary data structures needed.

#### Named sort functions

A sort function maps each node in the result to a comparable key. For nodes that
appear in multiple tape entries, the sort function receives all records for that
node and must aggregate them into a single key.

**`SectionOrder`** — sort by tape entry order (topological + sibling). The
concatenated output BIDs of a traversal label's entries IS a valid section order
(§6.1). Position-based; ignores path quality. Aggregation: earliest appearance
in the tape.

**`TfIdfScore`** — sort descending by `SortPayload.score` from `TextMatch` entries.
Aggregation: maximum score across all records.

**`PathLength`** — sort ascending by entry position within the label group
(traversal depth from anchor). Shorter = more directly related. Aggregation:
minimum position.

**`IntersectionCardinality`** — sort descending by the number of distinct compose
branches contributing to this node. Most meaningful for `And`/`Or` compositions.

**`Composite([(sort_fn, weight), ...])`** — weighted linear combination. Each component
normalized to `[0.0, 1.0]` via min-max normalization before weighting.

### 7.4 Render Mode

Any display schema and sort can be combined with any view implementation.

**Sequence rendering requirement**: A composed projection (`And`, `Or`, `Difference`)
produces a tape with multiple entries — each a measurement from a different orientation
(see §5.3, stereoscopic vision). The view must support compositing:

- **Static** (Table, List, CSV): composite all frames into a single output.
- **Dynamic** (Graph, timeline, interactive viewer): present frames individually or
  as an interactive sequence.

The `QuerySpec` does not prescribe compositing strategy — it provides the tape, and the
view decides how to render it.

---

## 8. The Video Camera

The query model uses a video camera metaphor: the query is a camera moving through
the fixed beliefbase graph, recording dimensionally-reduced frames at each position.
See [`docs/design/core/dag_model.md` §4](dag_model.md#4-the-video-camera-model)
for the conceptual introduction. This section specifies the formal mapping.

A `QuerySpec` configures the camera rig with two independent controls:

- **Position** (seed `TapeFn` on the first step) — the scan set coordinate: which
  region of the graph to measure. `TapeFn::Bids` positions the camera at specific
  nodes; `TapeFn::Corpus` samples across all loaded networks.
- **Orientation + Lens** (`StepOperation` chain) — which WeightKind axis to face,
  in which direction (In / Out / Both), how deep to focus, and what filters to
  apply. A property filter with TF-IDF scoring is a **graduated lens filter** —
  signal passes through at reduced intensity proportional to match quality. The
  depth parameter is the zoom: shallow depth gives a sharp, local frame; deep
  depth captures more structure at the cost of resolution.

The **view** (§7) is the third control but is NOT part of the `QuerySpec` — it is
a consumer-side concern: how the projected frame is recorded and composited (display
schema, sort order, render mode). Table = static exposure. Graph = spatial exposure.
CSV/XLSX = exported stills.

The output of each camera position is a set of captured paths — nodes that the camera's
field of view reached, at what intensity, via what route. When the camera moves to a new
position, the previous frame's content becomes the scan set coordinates for the next
shot. The ordering of shots is physically meaningful: the same two positions visited in
different sequence produce different `path_info` records, even when the final node set
is similar, because the intermediate frames differ.

**`path_info` is the full shot log** — not just the final frame, but the complete record
of which positions the camera visited, in what order, at what exposure. This is why
start / waypoint / terminus are positional facts, not output labels: they record a
node's position in the shot sequence that captured it.

The set operations (`And`, `Or`, `Difference`) combine footage from multiple cameras
(see §5.3, stereoscopic vision):

- `And` — **stereoscopic**: two cameras filming the same region from different
  orientations. Only subjects visible from both vantage points appear in the composite.
  The binocular overlap reveals structural depth.
- `Or` — **panoramic**: the combined field of view of both cameras, covering more of
  the dataset than either alone.
- `Difference` — **blind spot detection**: one camera's footage with everything the
  second camera also captured masked out. What remains is what the first camera sees
  but the second cannot — the coverage gap.

The result — the composite footage — is always a subset of paths through the fixed
graph. What the camera rig defines is the criterion for which paths are captured, not
a new structure. The complexity of the criterion grows with the number of cameras and
compositions; the underlying dataset does not change.

**Single shot** — one position, no composition:

```
QuerySpec { steps: [
    { input: Corpus, operation: Filter(TextMatch(content, "authentication")) },
]}
// view: Table { sort: TfIdfScore, display: depth-0 }
// grammar: CORPUS() :authentication
```

Camera positioned at the corpus with a graduated lens filter (TF-IDF). Output: a
flat frame graded by signal intensity (relevance score). No structural traversal; the
zoom is at minimum depth.

**Two-shot sequence** — structural traversal + depth-1 projection:

```
QuerySpec { steps: [
    { input: Bids([anchor]), operation: Traverse(section, depth=2) },
]}
// view: Table { sort: SectionOrder, display: EdgeCount([Epistemic, Pragmatic], [In, Out]) }
// grammar: bref:anchor composed_of(2)
```

Camera positioned at the anchor node, oriented along the Section axis, zoom opened to
depth 2. The captured frame is then re-projected through a depth-1 edge-counting lens
that tallies connections per WeightKind × direction. Result: the current traceability
table. Two shots, two dimensions captured.

**Stereoscopic composite** — two cameras combined:

```
QuerySpec { steps: [
    { input: Then(None), operation: Compose(And,
        left:  [{ input: Keys([id://priority-high]), operation: Traverse(k-pragmatic-s, depth=1) }],
        right: [{ input: Keys([id://review-doc]),    operation: Traverse(o-pragmatic-k, depth=2) }],
    )},
]}
// view: Graph { sort: IntersectionCardinality, display: MapsToPath }
// grammar: id://priority-high k-pragmatic-s(1) AND id://review-doc o-pragmatic-k(2)
```

Each arm of the `And` embeds its own seed `TapeFn` — these are the "camera positions"
for each half of the stereoscopic pair. Camera A is seeded at `id://priority-high`,
oriented inward along the Pragmatic axis at depth 1. Camera B is seeded at
`id://review-doc`, oriented outward along the MapsTo axis at depth 2. The stereoscopic
composite passes only nodes visible from both vantage points — items that are both
Pragmatic in-neighbors of the category and waypoints in the review traceability.
`path_info` records both cameras' shot logs for each such node.
`IntersectionCardinality` sorts by how many distinct shots from both cameras converge
on each node — the subjects with the deepest binocular overlap.

The viewer UI is a point-and-click camera configurator. MCP tools are the programmatic
interface to the same camera rig.

---

## 9. Concrete Use Case: Category-Filtered Coverage Gap Query

**Goal**: Starting from a review document, show only traceability rows where the reviewed
item belongs to a specific category. Additionally, show which items in that category have
*no* review coverage — the coverage gap.

This use case requires joining two traversals and computing a set complement. It cannot be
expressed in any current query surface without custom code.

**Example domain**: A project has an "Items" network containing item nodes (`item-001`,
`item-002`, etc.). Each item has Pragmatic out-edges to category nodes it belongs to (e.g.,
`id://priority-high`). A separate "Review Document" network contains sections that declare
`{maps_to}` claims covering specific items. The question: which priority-high items have
review coverage, and which are uncovered?

### Step 1 — Define Set A: Items in the Target Category

```noet-core/docs/design/core/query_model.md#L1-6
set_a = NeighborSet(
    anchor      = "id://priority-high",  // BID resolved via IdMatch
    weight_kind = Pragmatic,
    direction   = In,
    depth       = 1,
)
```

Implementation: resolve `"id://priority-high"` to a BID using `IdMatch`. Then call
`PathMap::new(Pragmatic, category_bid, ...)` to build a temporary Pragmatic PathMap rooted
at the category node. Traverse depth-1 in-neighbors: all item nodes `n` such that there is
a Pragmatic out-edge `n → priority_high`.

`set_a` produces paths of the form:

```noet-core/docs/design/core/query_model.md#L1-5
PathInfo {
    steps: [(item_bid, Pragmatic, Out), (category_bid, _, _)],
    sort_keys: [],
    scalar: None,
}
```

The start is `item_bid`; the terminus is `category_bid`. To use `set_a` as a filter on item
nodes, materialize the **starts**: `ExplicitSet(set_a.start_nodes())`. The terminus (the
category node) is the same for all paths and is not a useful filter.

### Step 2 — Define Set B: Review Coverage

```noet-core/docs/design/core/query_model.md#L1-5
set_b = MapsToTraversal(
    anchor = review_doc_bid,   // BID of the review document network root
    depth  = 2,
)
```

This is the existing `get_maps_to_traceability` traversal: for each owner section in the
review document, follow WEIGHT_OWNED_BY edges to claim nodes, then Pragmatic edges to item
sinks. Each path has the structure:

```noet-core/docs/design/core/query_model.md#L1-7
PathInfo {
    steps: [
        (review_doc_root, WEIGHT_OWNED_BY, Out),
        (owner_section,   Pragmatic,        Out),
        (item_sink,       _, _),
    ],
    sort_keys: [owner_section_key],
    scalar: None,
}
```

The item sink is the terminus (and also a waypoint in the full category query). Materialize
the item sinks as waypoints: `ExplicitSet(set_b.terminus_nodes())`.

### Step 3 — Join: Intersection of Coverage with Category Membership

```noet-core/docs/design/core/query_model.md#L1-12
// Item nodes that belong to the target category
items_from_a = ExplicitSet(set_a.start_nodes())

// Filter set_b to rows whose item terminus is in items_from_a
items_from_b = ExplicitSet(set_b.terminus_nodes() ∩ set_a.start_nodes())

join_result = QuerySpec { steps: [
    { input: Corpus, operation: Compose(And,
        left:  [items_from_a],
        right: [items_from_b],
    )},
]}
// view: Table { sort: IntersectionCardinality, display: MapsToPath }
```

Result: the traceability table showing only category-member item rows, with their review
coverage populated in cells, sorted by how many distinct paths reach each item node
(most-covered items first).

The `And` composition operates on two `ExplicitSet` predicates derived from the
materialized results of steps 1 and 2. The `MapsToPath` projection then traverses from
each matching item node back through the `set_b` path records to populate the review source
cells. `IntersectionCardinality` sort ranks items by how many distinct review sources cover
them — the item with the most coverage appears first.

**Evaluator pseudocode** for this step:

```noet-core/docs/design/core/query_model.md#L1-16
fn eval_join(set_a, set_b, graph) -> Vec<(Bid, PathInfo)> {
    let nodes_a: HashSet<Bid> = set_a.start_nodes().collect();
    let nodes_b: HashSet<Bid> = set_b.terminus_nodes().collect();
    let intersection = nodes_a.intersection(&nodes_b);

    intersection.map(|item_bid| {
        let path_infos_from_a = set_a.paths_starting_at(item_bid);
        let path_infos_from_b = set_b.paths_ending_at(item_bid);
        let merged = PathInfo::merge(path_infos_from_a, path_infos_from_b);
        (item_bid, merged)
    }).collect()
}
```

### Step 4 — Complement: Uncovered Items (The Coverage Gap)

```noet-core/docs/design/core/query_model.md#L1-11
items_covered = ExplicitSet(set_b.terminus_nodes())

gap = QuerySpec { steps: [
    { input: Corpus, operation: Compose(Not,
        left:  [ExplicitSet(set_a.start_nodes())],   // category-member items
        right: [items_covered],                       // items with any review coverage
    )},
]}
// view: Table { sort: PathLength, display: EdgeCount([Pragmatic], [Out]) }
```

Result: a flat list of item nodes that belong to the target category but have no review
coverage, sorted by distance from the category anchor (distance 1 = direct Pragmatic
neighbor, as all nodes in `set_a` are at depth 1; within the same depth, ties broken by
section order). The `EdgeCount([Pragmatic], [Out])` projection shows each uncovered item's
outgoing Pragmatic edges — which other categories it belongs to, providing context for
prioritization.

This is the coverage gap list. It answers: "Which items in this category need review
coverage, and which ones are completely unaddressed?"

---

## 9.5 Query Surface: Textual Syntax

The textual query grammar is specified in the user-facing reference
[`docs/query_language.md`](../../query_language.md). It surfaces the `NodeFilter`
and `Traversal` primitives defined in §5 to three surfaces — the viewer `?q=`
parameter, the `{query}` MyST directive body, and the MCP `query` tool — and is the
canonical serialization of a `QuerySpec` step pipeline. View configuration is never
embedded in the query string; each surface supplies it separately.

The parser is `src/query/parser.rs`. Implementation-level disambiguation rules for
the parser remain in §10.3 below.

---

## 10. Implementation Notes

`QuerySpec` is the sole query primitive. There is no intermediate AST or expression
language — callers construct a `QuerySpec` directly (JSON, Rust struct, or future
textual parser) and pass it to `BeliefSource::evaluate` via a `QueryPackage`.

Implementation guidance for the evaluator, tape, and view trait is tracked in
`ISSUE_70_UNIFIED_SEARCH_QUERY_UI.md` (Phases 1–6) and
`ISSUE_83_BELIEF_SOURCE_REFACTOR.md` (Phases 7–9). Key mapping points:

- `src/query/mod.rs` — `BeliefSource` trait (5 methods, object-safe via `BoxFuture`),
  `BALANCE_CUTOFF`, `MAX_TRAVERSAL`.
- `src/query/spec.rs` — `QuerySpec`, `QueryPackage`, `TraversalSpec`,
  `ProjectionStep`, `TapeFn`, `Tape`, `SortSpec`, etc.
- `src/query/view/` — `View` trait (was `Instrument`), `TableView`, `InstrumentOutput`.
- `src/paths/pathmap.rs` — `PathMap::new(kind, ...)` accepts any `WeightKind`;
  `PathMapMap::build_for_kind` provides on-demand ephemeral traversal construction.
- MCP tools (`get_submap`, `get_maps_to_traceability`, etc.) are named shorthand
  compositions of `QuerySpec` primitives. Each tool constructs a `QuerySpec` internally
  and delegates to the unified evaluator. The raw `query` tool accepts `QuerySpec` JSON
  directly (`docs/query_language.md` §12).
- The viewer controls bar (`traceability.js`) is a `QuerySpec` editor: the subject
  selector maps to the first step's seed `TapeFn`, the step cards map to the step
  pipeline, and render mode / sort order map to view configuration. The control state
  is always synchronized with the URL's `q=` parameter (`docs/query_language.md` §12).
- **`localStorage` persistence**: the viewer persists the query string verbatim. On
  reload, the URL `q=` parameter takes precedence over `localStorage` if both are present.

### 10.1 BeliefSource Trait Simplification

With the tape recording edge indices into the package graph (§6.3), `get_edges` and
`get_node` are removed from the `BeliefSource` trait. After evaluation, the package
graph contains all nodes and edges the query touched — seed nodes as complete,
discovered endpoints as Trace. Consumers read directly from `package.graph`.

The trait is object-safe via boxed futures (`BoxFuture`), eliminating the need for
the former `McpBeliefSource` wrapper trait. The five remaining methods:

| Method | Required | Purpose |
|--------|----------|---------|
| `submap` | yes | Section-edge traversal by path; returns `(path, bid, order)` triples |
| `submap_by_bid` | yes | Section-edge traversal by entry BID |
| `get_file_mtimes` | no (default) | Cache invalidation; returns file → mtime map |
| `export_beliefgraph` | no (default) | Full graph serialization for client-side use |
| `evaluate` | no (default) | Execute a `QueryPackage` in place; the single query entry point |

```
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub trait BeliefSource: Send + Sync {
    fn submap<'a>(&'a self, network_bid: Bid, path: &'a str, depth: u8, include_index: bool)
        -> BoxFuture<'a, Result<Vec<(String, Bid, Vec<u16>)>, BuildonomyError>>;
    fn submap_by_bid<'a>(&'a self, network_bid: Bid, entry: Option<Bid>, depth: u8, include_index: bool)
        -> BoxFuture<'a, Result<Vec<(String, Bid, Vec<u16>)>, BuildonomyError>>;
    fn get_file_mtimes(&self) -> BoxFuture<'_, Result<BTreeMap<PathBuf, i64>, BuildonomyError>> { /* default */ }
    fn export_beliefgraph(&self) -> BoxFuture<'_, Result<BeliefGraph, BuildonomyError>> { /* default */ }
    fn evaluate<'a>(&'a self, package: &'a mut QueryPackage)
        -> BoxFuture<'a, Result<(), BuildonomyError>> { /* default */ }
}
```

`submap` and `submap_by_bid` are the only required methods — every backend must
provide section-edge traversal. The remaining three have default implementations
(error stubs or empty maps) that backends override as needed.

### 10.2 Deleted Constructs

Everything below has been removed from the codebase. The replacement (if any) is
noted in parentheses.

**Types and enums**:

- **`Expression` enum** — the former untyped query AST. Replaced by `QuerySpec`
  (unified step pipeline), a structured, composable pipeline.
- **`StatePred` enum** — state predicates are now `NodeFilter` steps within the
  `QuerySpec` step pipeline, or seed `TapeFn` variants for seed selection.
- **`SetOp` enum (tree form)** — the `Dyad(Expression, SetOp, Expression)` recursive
  composition tree is deleted. `SetOp` still exists as a flat combinator between
  pipeline-level projection steps.
- **`Query` struct** — replaced by `QuerySpec`.
- **`RelationPred` enum** — replaced by `TraversalSpec` steps with role-based
  input/output specification and `WeightKind` filtering.
- **`View` enum on `QuerySpec`** — removed from the spec. View is a consumer-side
  trait (§7), not a query component.
- **`EvalOutput`** — the package graph + tape IS the evaluation output (§6.3).
- **`QueryResult`** — consumers read tape entries directly from the package.
- **`OwnedBeliefContext`** — deleted. `BeliefContext<'a>` is the sole context type.

**Functions and methods**:

- **`eval_query`**, **`eval_unbalanced`**, **`eval_trace`**, **`eval_spec`** — all
  deleted. `BeliefSource::evaluate(&mut QueryPackage)` is the single entry point.
- **`balance()`** method — deleted. Graph balancing is handled by
  `QueryPackage::balanced(spec)` which appends halo + section-root traversal steps.
- **`materialize_graph`** — deleted. The graph is built incrementally during
  projection.
- **`ensure_graph_context`** — replaced by `QueryPackage::balanced(spec)` constructor
  (§7.0). Consumer calls it when they need a balanced, self-contained graph.
- **`produce_output`** — deleted. Output production is the view's responsibility.

**Traits**:

- **`McpBeliefSource` wrapper trait** — deleted. `BeliefSource` is now object-safe
  via `BoxFuture`, so no wrapper is needed for MCP tool dispatch.

### 10.3 Parser Implementation Rules

Full disambiguation rules for `src/query_parser.rs`. Implementors extending the
parser should read this section; authors only need `docs/query_language.md` §13.

- **Word characters**: any character except whitespace and
  `{: ( ) , | $ ? * " ! < > = - #}`. Dots allowed (dotted property paths `payload.status`).
  Hyphens in `id://` anchors are a special case in the anchor lexer.
- **Colon**: not a WORD character. Emitted as a pseudo-token `Word(":")`. Parsed
  contextually: `WORD:WORD` is field:term (TextMatch) or property:value (edge filter).
  A bare `:` at the start of a filter_atom expands to `text:term`.
- **Keyword normalisation (lexer)**: operator keywords are folded to canonical case at
  tokenisation time. `AND`/`and`/`And` → `"AND"`. `in`/`IN`/`In` → `"in"`. The rest of
  the parser only sees canonical forms. This is unambiguous because conjunctions are
  stop words — a text search for bare `"and"` scores zero after TF-IDF filtering.
- **Stage detection**: a `Word` consisting entirely of `{s,k,o,n}` followed by `Dash`
  opens a full traversal. `->` / `<-` open a named shorthand. All other tokens enter
  the filter parser. Single-token lookahead; no backtracking.
- **Filter AND/OR greediness**: `parse_filter_or`/`parse_filter_and` consume `AND`/`OR`
  greedily. Before consuming, they check that the next token can start a `filter_atom`
  (not a traversal start, tape-fn keyword, composition op, or Eof). If not, the
  `AND`/`OR` is left for the query-level composition parser.
- **Filter NOT**: unary prefix only within a filter stage. `NOT atom` →
  `Compose(pass_all, Not, atom)`. Binary `NOT` between two pipelines is always
  query-level.
- **Composition precedence**: the query-level parser uses four-level recursive descent:
  `parse_comp_or` → `parse_comp_and` → `parse_comp_not` → `parse_comp_atom`. OR binds
  loosest, AND and binary NOT share the same level, unary NOT binds tightest.
  `parse_comp_atom` handles `(` as a grouping paren (argument parens are consumed at
  lower levels).
- **Unified continuation**: `parse_continuation_stages` is the single implementation
  of the `CONTINUATION` production. It is called from `parse_query` (top level),
  `parse_comp_atom` (inside grouping parens), and `parse_pipeline` (multi-stage
  pipelines). A `TAPE_FN` without a following stage emits an Identity step.
- **Minimal parenthesization**: the serializer uses a `parent_prec: Option<u8>` parameter
  to emit parens only when a child composition has lower precedence than its parent.
  AND/NOT (prec=1) nested inside OR (prec=0) needs no parens; OR nested inside AND does.
- **Degenerate traversal**: single-role self-loops rejected (`s-...-s`, `k-...-k`,
  `o-...-o`). Multi-role `n-pragmatic-n` is valid.
- **Step labels**: assigned as `"0"`, `"1"`, … in flat projection order by the parser.
- **Idempotency**: `parse(serialize(parse(input))) == parse(input)`. Serialiser emits
  named shorthands when the traversal matches a known pattern; multi-word TextMatch
  terms are quoted.
- **Reserved sigil**: `$` reserved for future stage-reference syntax; must not appear
  in WORD tokens.

---

## 11. Open Questions

1. **`MAX_TRAVERSAL` is not purely a performance constant.** The hard cap on traversal
   depth in `query.rs` preserves decidability of query equivalence: depth-bounded
   structural projection steps (`NeighborSet`, `SectionSubmap`) correspond to a decidable
   bounded-CRPQ fragment (Calvanese et al. 2000, Barceló et al. 2012). Removing
   `MAX_TRAVERSAL` while retaining `Not`/`Difference` composition would enter undecidable
   territory (Trakhtenbrot 1950). Any proposal to raise or remove this cap must explicitly
   re-examine the decidability implications, not treat it as a tunable performance knob.

2. **Waypoint role weight defaults.** The `role_weight(Terminus/Waypoint/Start)` values
   in §7.2 are illustrative. The weights should be configurable per `QuerySpec` (as
   optional `View` variant parameters) rather than global constants. Empirical tuning
   against real corpora is needed before defaults are fixed.

3. **Depth monotonicity simplification rule.** `Traversal` result sets grow monotonically
   with depth. A query optimizer can simplify `And(Traversal(a, k, dir, d1),
   Traversal(a, k, dir, d2))` with `d1 ≤ d2` to the shallower traversal — it is always
   the binding constraint.

4. **Tape explosion at depth > 3.** The number of tape entries per node can grow
   exponentially with depth. `IntersectionCardinality` is implicitly a truncated
   provenance measure (it counts entries rather than enumerating them) — the correct
   pragmatic choice. A `max_witnesses_per_node` parameter with a documented truncation
   policy (e.g., top-k by weight) would formally close this gap.

5. **Composite sort normalization.** Min-max normalization compresses all scores into a
   narrow range when a single outlier dominates. Consider z-score normalization as an
   alternative for corpora with outlier score distributions.

6. **Composition arms cannot reference prior tape entries.** Index-based `TapeFn`
   operators (`THEN("0")`, `FOLD(UNION, "0", "1")`) are designed to reference prior
   tape entries by label. A natural expectation is that a composition arm could use
   a tape reference as its input — e.g., `id:a uses(1) AND THEN("0")` to compose a
   fresh pipeline with a prior step's output. This does not work today: composition
   branches are evaluated via `apply_projection_steps`, which has no tape access.
   Only the top-level `apply_projection_steps_to_package` path sees the tape.
   Supporting tape references in composition arms would require either (a) passing
   the parent tape read-only into branch evaluation, or (b) refactoring composition
   to use the tape-based evaluation path with a scoping mechanism to prevent
   branches from writing to the parent tape. This is the primary use case for
   index-based `TapeFn` operators inside compositions; without it, `THEN(ref)` and
   `FOLD(op, ref, ref)` are only useful in the post-composition continuation
   (`docs/query_language.md` §7), not within composition arms.

Implementation-level open questions (tape WASM serialization, depth > 2 table UX,
composite sort serialization, `BeliefGraph` interface migration, BM25 improvement path)
are tracked in `ISSUE_70_UNIFIED_SEARCH_QUERY_UI.md` Open Questions.

---

## 12. References

- `noet-core/src/query/mod.rs` — `BeliefSource` trait, `BoxFuture`, `BALANCE_CUTOFF`, `MAX_TRAVERSAL`
- `noet-core/src/query/spec.rs` — `QuerySpec`, `QueryPackage`, `TraversalSpec`, `ProjectionStep`, `SetOp`, `TapeFn`, `Tape`
- `noet-core/src/paths/pathmap.rs` — `PathMap::new(kind, ...)`, `PathMap::submap`, `PathMapMap`
- `noet-core/src/mcp/tools.rs` — `get_maps_to_traceability`, `get_maps_to`, `get_traceability`
- `noet-core/src/mcp/types.rs` — `GetMapsToTraceabilityInput`, `MapsToTraceabilityOutput`
- `noet-core/docs/project/ISSUE_70_UNIFIED_SEARCH_QUERY_UI.md` — viewer UI implementation plan
- `noet-core/docs/design/core/search_and_sharding.md` — compile-time search index; `query_search_index` MCP tool is the `TextMatch` predicate implementation
- `noet-core/assets/viewer/navigation.js` — `renderNavNode` visited-set / back-reference pattern (analogous to intra-table back-references in §5.2)
- Application-specific acceptance tests for the §9 use case live outside this
  repository, alongside the corpus they validate against

---

## 13. Prior Art and Literature Grounding

This section records the results of a structured literature review conducted across five parallel investigations: graph query languages (Task 1), provenance (Task 2), ranking and centrality (Task 3), SPARQL/GQL mapping (Task 4), and query equivalence (Task 5).

### 13.1 What Is Prior Art (Not Novel)

The following components of our model have direct, well-established analogues in the literature:

| Our model | Prior art analogue |
|---|---|
| `And`/`Or`/`Difference` compositions over structural projection steps | SPARQL BGP conjunctions / `FILTER NOT EXISTS` / GQL `WHERE NOT EXISTS`; formally: depth-bounded **Conjunctive Regular Path Queries (CRPQs)** over a typed directed graph (Barceló 2013 survey) |
| `TfIdfScore` with title-3x field boost | **BM25F** field weighting (Robertson et al. 1994, 2004); the 3× title multiplier is correct BM25F; TF saturation and length normalization are absent (low-impact for short payloads) |
| `PathLength` sort | Single-source graph distance ranking — basic graph theory; no special citation needed |
| `IntersectionCardinality` in the category-filtered use case | **s-t betweenness centrality** with fixed target (Freeman 1977, Brandes 2001); our implementation is a focal, depth-bounded, directed approximation |
| `Composite([(fn, weight)])` linear ranker | **Pointwise learning-to-rank** (Liu 2011); our static weights are the simplest instance of this family |
| Tape as ordered traversal record | **How-provenance / provenance polynomial** (Green, Karvounarakis, Tannen, PODS 2007); a single tape entry is a **witness** (monomial) for its terminus node's membership; the collection of entries for one node is the provenance polynomial. The tape extends provenance by recording step *order* — the sequence of camera positions, not just which witnesses exist |
| `Difference` gap query (why a node is absent) | **Why-not provenance** (Chapman & Jagadish, SIGMOD 2009); our bounded case (both sides are materialized finite sets) is plain set subtraction and avoids the general exponential complexity |
| Seed / pipeline / view separation | **Scientific visualization pipelines** (Schroeder, Martin, & Lorensen, *The Visualization Toolkit*, 1996; Moreland, 2013 survey). VTK's data → filter → mapper → renderer pipeline is structurally identical: data source (seed TapeFn), transformation chain (step pipeline), rendering backend (view). The separation is standard in scientific visualization but has no analogue in graph query languages, which conflate query evaluation and result presentation |
| Projection as dimensionality reduction | **Mathematical projection** in the linear algebra sense — reducing a high-dimensional manifold to a lower-dimensional representation. Standard in scientific computing; novel in application to graph query result shaping |
| Stereoscopic composition (`And` as binocular overlap) | **Multi-view reconstruction** in computer vision (Hartley & Zisserman, 2003). Combining multiple viewpoints to recover structure is standard in CV; applying it to graph query composition (two projection chains on a shared scan set) is non-standard |

### 13.2 What Is Genuinely Novel

The following aspects of the model have no direct prior art in SPARQL, GQL, Cypher, or
the standard graph query literature. The scientific visualization pipeline (§13.1) is the
closest structural analogue, but the specific application to graph queries is new.

The **three-axis WeightKind type system** (Section/Epistemic/Pragmatic) as orthogonal
projection parameters enabling axis rotation — no analogue in any existing graph query
language, which use flat edge-label namespaces. The **`WEIGHT_OWNED_BY` edge-metadata
ownership pattern** (`MapsToTraversal` 2-hop) is natively expressible in GQL as an edge
property but requires RDF reification in SPARQL; treating it as a *named projection step
type* (rather than an inline pattern) is a further novelty.

The **video camera model** (§8) — initial input as camera position, step pipeline as orientation +
lens + filter chain, tape as shot log, view as recording medium — unifies concepts
from scientific visualization (VTK pipeline), mathematical projection (dimensionality
reduction), computer vision (multi-view reconstruction), and graph query theory (CRPQs)
into a single coherent framework. No prior graph query system combines all four.

The **start/waypoint/terminus role weights** as continuous, composable view sort
components extend the hub/authority binary (Kleinberg 1999) to multi-hop typed paths with
no direct prior formulation. **`IntersectionCardinality` as focal depth-bounded directed
betweenness** — the combination of focal (query-scoped), depth-bounded, and directed is
non-standard; standard betweenness is all-pairs, undirected, and unbounded.

The **tape** (§6) as the sole interface between projection and view — an ordered
provenance record that captures not just which nodes are in the result but the full
sequence of transformations that produced them — extends standard how-provenance
(polynomials over witnesses) with temporal ordering of projection steps. Standard
provenance records *what* contributed to a result; the tape also records *in what order*
and *from which camera position*.

Each WeightKind sub-graph is a finite DAG — a partially ordered set. The `And`/`Or`
composition operators on traversal results are meet/join in the product lattice of two
WeightKind projections; this lattice structure is what makes depth-bounded CRPQs
decidable (Calvanese et al. 2000) and what makes `min`/`max` the correct score
composition operators — they are the meet and join of the score lattice.

### 13.3 GQL as Reference Formalism

The projection chain is best understood as a domain-specific instance of the GQL
(ISO/IEC 39075, 2024) property graph query model, with beliefbase WeightKinds as typed
edge labels. GQL is preferred over SPARQL as a reference formalism because GQL's property
graph model handles edge metadata (`WEIGHT_OWNED_BY`) natively without reification,
whereas SPARQL requires RDF-star or verbose reification patterns. The key remaining gaps:
GQL's `ORDER BY` operates on variables and aggregates, not on tape records — our
view sort model (§7.2), particularly `IntersectionCardinality`, `SectionOrder`, and
role-weighted `Composite`, extends beyond GQL's current capabilities. GQL also has no
analogue to the view layer — result presentation is outside the language scope.

### 13.4 Key References

**Load-bearing** — these directly ground design decisions in the spec:

- Calvanese, D., De Giacomo, G., Lenzerini, M., & Vardi, M. Y. (2000). "Containment
  of conjunctive regular path queries with inverse." *KR 2000*. — Why `MAX_TRAVERSAL`
  exists: bounded-depth CRPQs are decidable (polynomial for fixed depth); removing the
  bound while retaining `Difference` enters undecidable territory. Read the abstract.
- Green, T. J., Karvounarakis, G., & Tannen, V. (2007). "Provenance semirings."
  *PODS 2007*. — Why `Score = Option<f32>` with `min`/`max` works: the tape is a
  provenance polynomial, each entry a witness monomial. `And`/`Or` composition inherits
  the semiring's algebraic laws. Read §1–2.
- Robertson, S. E., et al. (1994). "Okapi at TREC-3." *TREC 1994*; Robertson, S. E.,
  Zaragoza, H., & Taylor, M. (2004). "Simple BM25 extension to multiple weighted
  fields." *CIKM 2004*. — Our TF-IDF with 3× title boost is simplified BM25F field
  weighting. The simplification (no TF saturation, no length normalization) is
  low-impact for short-payload corpora.
- Chapman, A., & Jagadish, H. V. (2009). "Why not?" *SIGMOD 2009*. — `Difference`
  avoids the why-not provenance problem (exponential in general) because both sides are
  materialized finite sets — plain set subtraction, not open negation.
- Brandes, U. (2001). "A faster algorithm for betweenness centrality." *Journal of
  Mathematical Sociology 25(2)*. — `IntersectionCardinality` is a focal, depth-bounded,
  directed variant of betweenness centrality.

**Further reading** — analogy sources and positioning references, not required for
understanding the implementation:

- Barceló, P. (2013). "Querying graph databases." *PODS 2013*. — CRPQ survey containing
  Calvanese's result in broader context.
- Schroeder, W., Martin, K., & Lorensen, B. (1996). *The Visualization Toolkit*.
  Prentice Hall. — VTK data→filter→mapper→renderer pipeline; structural analogue for
  the seed/pipeline/view separation.
- Hartley, R. & Zisserman, A. (2003). *Multiple View Geometry in Computer Vision*.
  Cambridge University Press. — multi-view reconstruction; analogue for stereoscopic
  `And` composition.
- ISO/IEC 39075 (2024). *GQL: Graph Query Language*. — reference formalism for
  positioning; we are a domain-specific GQL instance but do not implement GQL.
- Kleinberg, J. M. (1999). "Authoritative sources in a hyperlinked environment."
  *JACM 46(5)*. — HITS hub/authority; the start/terminus role weight concept extends
  this to multi-hop typed paths.
- Freeman (1977), Liu (2011), Moreland (2013). — Original betweenness centrality,
  learning-to-rank textbook, visualization pipeline survey. Standard background;
  cited for completeness.
