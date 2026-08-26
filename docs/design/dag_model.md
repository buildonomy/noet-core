---
title = "Documentation as a Dependency Graph"
authors = "Andrew Lyjak, Claude"
last_updated = "2026-04-23"
status = "Draft"
version = "0.1"
---

# Documentation as a Dependency Graph

How noet turns documents into queryable structure.

## 1. The Problem with Flat Documentation

Every engineered system has a dependency structure. Code has imports. Build systems
have dependency graphs. Hardware has bills of materials. But documentation — process
docs, design rationale, coverage matrices — has historically been flat: a table in a
spreadsheet, a section in a PDF, a checkbox in a form.

Flat documentation goes stale because nothing enforces coherence. When a design
changes, the impact on downstream documents is invisible. Prose rationale is
disconnected from the claims it supports. You cannot query a PDF. You cannot diff a
spreadsheet meaningfully. When structure changes, nobody knows what else broke.

noet treats documentation the same way a build system treats code: as a directed
acyclic graph (DAG) where every claim of coverage, dependency, or relationship is an
*explicit edge*, not an assertion buried in prose. The result is documentation you can
query, diff, and validate structurally.

## 2. The noet Model: Documents as Graph Nodes

### Nodes

Every named entity in the system is a **node** with:

- **Identity** — a unique BID (belief identifier, a UUID) and a human-readable bref
  (short hex alias)
- **Kind** — Document, Section, Network, External, etc.
- **Schema** — user-defined classification (e.g. "procedure", "requirement", "concept")
- **Content** — the text payload, if any

Nodes are the atoms of the knowledge graph. A document is a node. A heading within
that document is a node. A network — a collection of related documents — is a node.

### Directionality: Source, Sink, and Owner

Every edge has three participant roles:

- **Source** (`<`) — the child, dependent, or more-discrete end
- **Sink** (`>`) — the parent, depended-upon, or more-interconnected end
- **Owner** (`@`) — the node that *declared* the edge

In the common case, the owner is the source or sink itself. In cross-document
traceability claims, the owner is a third-party node that declares a relationship
between two nodes it does not structurally contain. This three-party ownership model
is what makes cross-cutting traceability possible without modifying the documents
being traced.

A design document (sink) *uses* a requirement (source) to constrain its design — the
requirement is the more foundational node, the document the more discrete one. A
review entry (source) *maps to* an external standard section (sink). Source is always
the more-discrete end; sink is the more-interconnected end.

### A Minimal Example

Consider a project with a design document, a requirement, and a review section,
all contained within a shared project network:

```
                              ┌────────────────┐     Pragmatic      ┌───────────┐
                              │  Design Doc    │◀────(uses)─────────│  REQ-001  │
                              │                │        ▲           │           │
                              └────────────────┘        │           └───────────┘
                                     ▲                  │ Pragmatic
                                     │ Epistemic        │ (maps_to)
                                     │ (draws from)     │ owner: Review §3.1
                                     │                  │
                              ┌────────────────┐  ┌────────────────┐
                              │  Concept-A     │  │  Review §3.1   │
                              └────────────────┘  └────────────────┘
Arrows point from source (tributary) to sink (ocean). The Design Doc is the sink of
the Pragmatic `{uses}` edge — it depends on REQ-001 as a source constraint.
Review §3.1 *owns* the `{uses}` edge (arrow points at the edge, not at a node) —
it asserts coverage of the relationship between Design Doc and REQ-001.
All four nodes are Section-children of a shared Project Network (not shown).
```

The design document *draws from* Concept-A (epistemic provenance — the concept is the
more foundational source, the design doc the sink). The design document *uses* REQ-001
(pragmatic dependency — REQ-001 is the source constraint, the design doc the sink that
depends on it). The review section declares a third-party `{maps_to}` edge claiming
coverage of the `{uses}` relationship without modifying either document. The owned
edge points at the *edge* between Design Doc and REQ-001, not at either node directly. Section edges (omitted for
clarity) encode the containment hierarchy. Three edge types, three distinct structural
questions, one coherent graph.

### Why DAG, Not Tree

A tree allows each node exactly one parent. A DAG allows multiple parents — a node
can be the source of edges to multiple sinks across different edge types. This is
essential because real knowledge has multi-dimensional structure:

- A concept can be *contained in* one document (Section edge) while being *cited by*
  another (Epistemic edge) and *classified by* a third (Pragmatic edge).
- A single item can appear as a waypoint in multiple traversal paths, connecting
  different parts of the graph.

The DAG structure is what makes composed queries possible: you can ask questions that
span multiple relationship dimensions simultaneously.

## 3. Three Dimensions of Structure

The three edge types — Section, Epistemic, Pragmatic — are orthogonal axes, not
names for specific relationships.

| Kind      | Axis      | Source-verb     | Sink-verb       | Encodes                    |
|-----------|-----------|-----------------|-----------------|----------------------------|
| Section   | Structure | *consists of*   | *component of*  | What counts as "one thing" |
| Epistemic | Knowledge | *draws from*    | *underlies*     | Where claims come from     |
| Pragmatic | Action    | *uses*          | *used by*       | What covers/depends on what|

Every relation declared in a document has three participants: the **subject** (the
authoring document, always the owner of the relation), the **verb** (the directive),
and the **referent** (the referenced node). The verb determines which graph slot the
referent occupies; the subject takes the other slot automatically.

For example: `I {composed_of} [foo]` → the verb places `foo` as the **source**,
so the subject (`I`) becomes the **sink**: `source: foo, sink: I, kind: section`.
(`{consists_of}` is a backward-compatible alias for `{composed_of}`.)
Conversely: `I {component_of} [foo]` → the verb places `foo` as the **sink**,
so the subject becomes the **source**: `source: I, sink: foo, kind: section`.

The **Source-verb** column names verbs that place the referent in the source slot
(subject is sink). The **Sink-verb** column names verbs that place the referent in
the sink slot (subject is source). Authors only need to choose the verb that describes
their relationship — the graph wiring follows. This is the same ownership model used
by `{maps_to}`: the declaring document is always the owner; the verb and referent
determine the rest.

**Section** edges are the mesh topology: the connectivity that turns isolated nodes
into bounded entities. They define containment — a network consists of documents, a
document consists of sections.

**Epistemic** edges encode provenance — the chain of reasoning from foundational
concepts to derived claims. They are the pedagogical structure of the domain: the
path a newcomer follows when learning the system.

**Pragmatic** edges encode actionable claims — coverage assertions, dependency
declarations, classification. When someone says "this review covers that requirement,"
that is a pragmatic edge. These are the only edges that encode normative, executable
relationships.

Orthogonality matters because any query can filter, traverse, or compose along any
combination of these axes independently. A Section traversal can be followed by a
Pragmatic filter, or an Epistemic trace can be intersected with a Pragmatic coverage
check. The axes do not interfere with each other.

## 4. The Video Camera Model

### Graph as Mesh, Query as Rendering Pass

The beliefbase graph is to knowledge what a physics engine mesh is to geometry: a
discretized approximation that preserves the properties that matter for traversal —
structure, provenance, coverage — and discards the rest. The mesh is not the object;
the graph is not the knowledge. It is an approximation good enough to answer
structural questions.

A query is a **rendering pass** — like a video camera moving through the dataset. The
query has two independent controls, plus a consumer-side rendering concern:

- **Position** (seed) — which region of the graph to measure. This is the camera's
  starting point: a specific anchor node, a search result set, or the entire corpus.
  Expressed as a seed `TapeFn` on the first step of the query pipeline.
- **Orientation + Lens** (step pipeline) — which edge axis to face, in which direction,
  how deep to focus, and what filters to apply. Pointing the camera at Section edges
  versus Pragmatic edges is facing a fundamentally different structural dimension.
  A text search filter is a graduated lens — signal passes through at reduced
  intensity proportional to match quality.
- **View** — how the captured frame is recorded. Table (static exposure), graph
  (spatial rendering), CSV (exported stills). The view reads the pipeline's output
  and formats it for human or machine consumption. The view is NOT part of the
  query specification — it is a consumer-side concern (see `query_model.md` §7).

The output of each position is a set of captured paths — nodes that the camera's field
of view reached, at what intensity, via what route. When the camera moves to a new
position, the previous frame's content becomes the coordinates for the next shot.

### Traversal Depth as Taylor Series

Each traversal step is a discrete linear transformation that incrementally
approximates the local neighborhood structure — analogous to a term in a Taylor
series expansion. At depth 0 you have the point value (the anchor node itself). At
depth 1 you have the gradient (immediate neighbors). At depth 2 you have curvature
(neighbors of neighbors). Each additional term captures more of the mesh's structure
at the cost of resolution and computation.

The depth parameter is not merely a performance guard — it bounds the approximation
order. For most practical queries, depth 2–3 captures the relevant structural context.

### Two Modes of Engagement

The same rendering pass serves two purposes:

- **When the model is accurate**, the render tells you something true: coverage holds,
  citations are coherent, dependencies are satisfied.
- **When the model is wrong**, the render makes the incongruities *visible*: gaps,
  orphaned nodes, contradictions, missing edges.

Both modes are necessary. You need the render to see where the mesh is wrong, and you
need the mesh to be approximately right before the render tells you anything useful.
Gap analysis, coverage reports, and consistency checks are all diagnostic renders —
they exist to find where the model needs refinement.

## 5. Composed Queries: Stereoscopic Vision

When two projections view the same region from different orientations, composing them
produces multi-ocular measurement:

- **And** (intersection) is **stereoscopic vision** — two cameras looking at the same
  region from different orientations. The binocular overlap reveals structural depth
  that neither monocular view captures alone. Nodes visible from more vantage points
  are structurally more central.

- **Difference** is **blind spot detection** — what one eye sees that the other
  cannot. A coverage gap list is one camera's visual field minus the other's: items
  present in the graph but unreachable from the second vantage point. This is the
  complement operator, and it answers "what should be connected but isn't?"

- **Or** (union) is **panoramic vision** — the combined field of view of both
  cameras, covering more of the graph than either alone.

**Example**: "Which items in category X have review coverage from document Y?" is a
stereoscopic query — Camera A oriented along the Pragmatic axis from the category
node, Camera B oriented along the coverage axis from the review document, with only
the binocular overlap returned. "Which items in category X have NO coverage?" is
blind spot detection — Camera A's field minus Camera B's.

See `docs/design/query_model.md` §5.3 for the formal composition algebra and §9 for
a worked example of a category-filtered coverage gap query.

## 6. Reachability and Queries

Given any anchor node, you can ask:

- **"What does this node depend on?"** → traverse toward roots (source → sink)
- **"What depends on this node?"** → traverse toward leaves (sink → source)
- **"What is this node classified as?"** → follow Pragmatic edges outward
- **"Are there gaps?"** → find nodes with expected but missing edge connections
- **"What is uncovered?"** → compute the complement: nodes in set A but not set B

This transforms structural review from "read every document and check manually" to
"run a graph query." The complement operation is particularly powerful: it answers
"what should be connected but isn't?" — the gap list.

A query specification composes these primitives into an **optical bench**: a
configurable rig of positions, orientations, and views that can be saved,
shared, and re-executed as the underlying graph evolves. The same query run today and
next week will reveal what changed structurally — not because someone remembered to
check, but because the query's structure encodes what matters.

See `docs/design/query_model.md` for the full formal treatment of query
specifications, the score algebra, path recording, and the textual query surface.

## 7. Getting Started

### How Nodes and Edges Are Declared

noet source files are authored in Markdown (or TOML). The compiler parses these files,
identifies structural elements (headings become Section nodes, documents become
Document nodes), and builds the graph automatically. Identity (BIDs) is injected into
source files on first compilation, so nodes gain stable identity that survives renames
and reorganization.

### The `{maps_to}` Directive

Cross-document edges are declared using block directives in Markdown. The most common
is `{maps_to}`, which creates a third-party pragmatic edge:

```markdown
## Review of Safety Requirements

{maps_to}
- [REQ-001](../requirements/req-001.md)
- [REQ-002](../requirements/req-002.md)
{end}
```

This declares that the "Review of Safety Requirements" section (the owner) asserts
coverage of REQ-001 and REQ-002 (the sinks) — without modifying either requirement
document. The six canonical relation directives (`{composed_of}`, `{component_of}`,
`{draws_from}`, `{underlies}`, `{uses}`, `{implements}`) cover all three edge types
in both directions. See `docs/design/query_model.md` §9.5 and Issue 71 for the full
directive surface.

### How the Compiler Builds the Graph

```
Source Files (*.md, *.toml)
    ↓
[Parse] → DocCodec (per-format lexer/parser)
    ↓
IRNode (intermediate representation)
    ↓
[Link] → GraphBuilder (multi-pass reference resolution)
    ↓
BeliefBase (compiled graph)
    ↓
[Query / Traverse] → MCP tools, WASM viewer, CLI
```

The multi-pass compiler resolves forward references, injects BIDs, and iterates until
all resolvable links are wired. Unresolvable references are tracked as diagnostics,
not fatal errors — the graph is always available, even when incomplete. See
`docs/design/beliefbase_architecture.md` for the full compilation model.

---

## Further Reading

- **[Query Model](query_model.md)** — Formal specification of the query
  algebra: score primitives, traversal, composition, sort, views, and textual
  query syntax.
- **[BeliefBase Architecture](beliefbase_architecture.md)** — Detailed
  technical specification of identity management, the compilation pipeline,
  graph invariants, and incremental updates.
- **[MCP Server](../mcp.md)** — Agent-facing tool documentation for querying a
  compiled BeliefBase via the Model Context Protocol.
- **[README](../../README.md)** — Project overview, installation, and quick start.
