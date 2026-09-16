---
version = "0.1"
title = "Overlay Model — How the Annotation Layer Composes With the Compiled Graph"
authors = ["Andrew Lyjak"]
status = "Target architecture — the halo mechanism is shipped; layer composition is recommended"
dependencies = [
  "annotation/living_corpus.md",
  "core/beliefbase_architecture.md",
  "annotation/federated_belief_network.md",
]
---

# Overlay Model

## 1. Purpose

`living_corpus.md` §2 establishes three layers and says Layer 3's live projection
composes with the compiled graph. This document answers: **what is the resulting
object, and how is it read?**

It is the shared specification for:

| Consumer | Composes |
|---|---|
| Annotation store (Issue 105) | corpus + local annotation records |
| Browser SPA | shipped corpus + locally-authored annotations |
| Federation (`federated_belief_network.md` §3.6) | local + remote peers |
| Compiler observations | corpus + records the parse itself emitted |

**Compiler observations are a consumer, and the producer decides which ones.**
The annotation channel carries two lanes — records, which have an `EventId` and
may be cited, and diagnostics, which do not and are served by `tracing`
(`annotation_channel.md` §3). An observation becomes an annotation when the
producer emits it on the record lane; nothing promotes it implicitly. So "are
compiler observations annotations?" has the answer *some are, by the producer's
choice of lane*.

What distinguishes them from the other three rows is **lifetime, not kind**.
Parse-emitted records occupy the *regenerated* scope: the producing actor
replaces the whole set on its next run, so nothing outside that actor may depend
on one surviving (`living_corpus.md` §6). They compose, project, and render
exactly like durable records — an overlay reader cannot tell the difference and
does not need to.

**Scope boundary.** This document defines what an overlay *is* and how it is
read. It does not specify any particular read: the **halo query** — the first
thing built on this model, with its render surfaces and its client-side
viability constraint — is Issue 105 step 5.

## 2. An annotation owns edges; it does not patch nodes

**Annotation projection never produces a *stored* node that shares a BID with a
corpus node**, so composing annotations with a corpus involves no node merge. A
rendered before/after view does construct same-BID content, but ephemerally and
outside any store — §2.5.

`living_corpus.md` §4 maps a folded annotation to a `NodeUpsert` for **the record
itself**, whose BID is derived from its `EventId`
(`identity/identity_derivation.md` §6.1), plus `RelationUpdate`s connecting it to
what it concerns. The concerned nodes are not written to. They gain incoming
edges.

**This is the `{maps_to}` idiom, and it is the most-exercised structure in the
corpus.** A third-party node owns edges between nodes it is neither source nor
sink of:

| Piece | Where | What it does |
|---|---|---|
| `WEIGHT_OWNED_BY` | `src/properties.rs` | any value other than `"source"`/`"sink"` is a third-party owner bref |
| `owner_edges` | `src/beliefbase/base.rs` | `BTreeMap<Bref, BTreeSet<EdgeIndex>>`, maintained incrementally |
| `graph_for_owner` | `src/beliefbase/base.rs` | owner bref → `BeliefGraph` of its edges and their endpoints |
| `Role::Owner` | `src/query/spec.rs`, sigil `o` | traversable as both input and output role, in-memory **and in SQL** (`src/db.rs`) |

The run-derived node owns its edges in **two modes**, both already expressed by
`WEIGHT_OWNED_BY`:

| Mode | Edge | `owned_by` | Case |
|---|---|---|---|
| **Self-owned** | run node → concerned node | `"source"` | the common case — a receipt or claim *about a node* |
| **Third-party** | node A → node B, neither is the run node | the run node's bref | a claim *about a relationship* — the `{maps_to}` shape |

Annotating relationships directly is a real need ("this traceability edge is
wrong", "these two should be linked"), and the third-party mode is how it is
expressed without touching either endpoint.

**The halo is the union of both.** `graph_for_owner` returns only the
third-party set — `owner_edges` deliberately excludes `"source"`/`"sink"`
owners (`src/beliefbase/base.rs`). A run's self-owned edges are its
ordinary outgoing adjacency. The halo query (Issue 105 step 5) must take both; a single
`graph_for_owner` call misses every node-level annotation. Either way the result
is a `BeliefGraph` — a transport package, endpoints marked `Trace` because it is
a partial view. No merge, no patch, no wholesale replacement.

### 2.2 An annotation's scope is a queryset, not a node

The decisive property. An annotation is anchored by a `QuerySpec`
(`identity/content_versioning.md` §4), so it applies to **a set of nodes** —
potentially many within one document. There is no single node to patch, and the
halo is the tape's output set with owned edges into each member.

The **anchor** (what the annotation is about, and how staleness is evaluated) and
the **halo** (what a reader sees around the focused content) are therefore the
same set.

### 2.3 What this leaves

`BeliefGraph` is a **transport** form, not an at-rest one. So the right output of
reading an annotated corpus is what every `BeliefSource::evaluate` already
returns: a `QueryPackage` carrying a `BeliefGraph`. `graph_for_owner` produces
exactly that shape today.

### 2.4 Why not patch the node instead

Three variants of "put the annotation in the node" were considered and each
fails for a different reason.

**Patch fields at read time.** The at-rest structures do not support it
uniformly, and one case is decisive: `DbConnection` runs traversal **in SQL**
(`src/db.rs`), so a patch applied to returned rows cannot affect what
the query *selected*. That is not a view of an annotated corpus; it is
post-processing that silently disagrees with its own query. `Role::Owner`
traversal is implemented in both backends and composes with selection rather
than after it.

**Reserve a key inside `BeliefNode`.** `BeliefNode.metadata` (Issue 26) took this
route, and it produced the diff cascade, the hashing ambiguity, and the merge
question — `identity/content_versioning.md` §5.1a calls the result "three tables
wearing one name", and Issue 105 step 3a-pre reclassifies them. A key inside the
node puts two classes of data under **one** ownership rule — the node's — which
is exactly what needs separating, so the lesson outlives the cleanup.

**Add a fourth merge semantics to `BeliefGraph`**, which already carries three
(`union_mut`, `union_mut_from`, `union_mut_with_trace`). Every future change to
node structure would then have to be correct in four places.

**The prior art agrees.** Union filesystems (lower/upper dir), git (tree versus
index+worktree), copy-on-write B-trees, and an LSP's file-versus-dirty-buffer all
converge on **read-through with copy-on-write** — and none represents the overlay
as another instance of the base type. The overlay is a distinct type satisfying
the base's *read interface*, which is what §3 specifies.

### 2.5 Redlines: an ephemeral before/after, never a stored node

The halo says *which* nodes an annotation concerns. A **redline** — an annotation
proposing a change — additionally carries what those nodes should say, and a
reader needs the two juxtaposed: current content beside proposed content.

That juxtaposition **looks like** a node sharing a BID with a corpus node, and
the distinction that keeps it safe is *where it exists*:

| | Stored | Rendered |
|---|---|---|
| Lives in | a `BeliefSource` — `BeliefBase`, `DbConnection`, shards | one `QueryPackage`, discarded after the read |
| Participates in selection | yes — later queries see it | no — it is the *output* of a query |
| Survives a reparse | yes | no |
| Shares a BID with a corpus node | **must not** | may, because nothing merges it back |

A proposed-content node is a **rendering artifact**. It never enters a store,
never reaches `union_mut`, and cannot be selected by a subsequent query — so the
wholesale-replacement constraint (§2.6) does not reach it. What is *stored* is
the redline record: its own node, its own BID, owning edges into the queryset it
concerns, with the proposed content in its payload.

**The rule is one-directional and worth stating plainly**: proposed content may
be *materialized into a package* for reading, and must never be *written to a
source*. The moment it is stored, it stops being a proposal and starts being a
second account of the node's content that nothing reconciles.

The construction, given a set of redlines and the document they target:

1. Resolve the halo — the concerned node set (§2).
2. Build a candidate package: those nodes, carrying the redlines' proposed
   content instead of their current content. Identity resolves through the
   ordinary key-matching path, so a node pairs by whichever `NodeKey` variant
   matches and carries the corpus BID once paired.
3. Compare against the current state. `BeliefBase::compute_diff(old, new, scope)`
   (`src/beliefbase/base.rs`) already produces this as ordered
   `BeliefEvent`s — `NodesRemoved`, `NodeUpdate`, `RelationRemoved`,
   `RelationUpdate`, `RelationChange` — which covers changes to **properties and
   relations alike**, not only prose.
4. Render the delta.

Three properties fall out of doing it this way:

- **A set of redlines renders as one diff.** The unit that crosses an
  organizational boundary is a change package per target document, not one
  proposal at a time, so the set is the general case and the singleton the
  degenerate one.
- **Identity pairing is what makes it a diff rather than two documents.** Both
  sides must carry matching `NodeKey`s — not necessarily `NodeKey::Bid`. A
  candidate built by parsing proposed source acquires corpus identity the way any
  parse does: `cache_fetch` resolves a node's key list against the existing graph
  (`src/codec/builder.rs`), so a `Path` or `Id` match binds the candidate
  node to the corpus node it revises, and the BID follows. That matters for the
  case a redline exists to serve — proposed text whose heading was retitled still
  pairs by path, where BID-only matching would read as a delete plus an add.

  What the BID *does* determine is safety: once paired, a candidate node holds
  the corpus node's BID, which is why the candidate must never be stored (§2.5).
- **Rendering needs no write authority.** The delta can be rendered to HTML for a
  reader who will carry the change to whoever owns the target
  (`living_corpus.md` §7). Turning the same delta into *source text* is a
  separate operation with separate prerequisites — byte ranges (Issue 103), codec
  round-trip fidelity (Issue 107), and declared write authority (Issue 106).

Issue 74 owns the diff and its render modes; Issues 106 and 107 own the source
exit. This section fixes only the constraint they share: **the candidate side is
ephemeral, and no code path may persist it.**

### 2.6 The constraint both of these avoid

Anything that **merges** two graphs sharing BIDs inherits a hard limit.
`BeliefGraph::union_mut` (`src/beliefbase/graph.rs`) replaces nodes
outright:

```rust
for node in rhs.states.values().filter(|node| node.kind.is_complete()) {
    self.states.insert(node.bid, node.clone());
}
```

A node for BID *X* carrying only partial data replaces the corpus node entirely,
losing `title`, `payload`, and everything else — so a merge-based composition
must hold **complete copies** of every node it touches. `union_mut` assumes both
sides are `BeliefGraph`s of equal authority, so "merge" can only mean "one side
wins."

Neither construction above reaches this limit, and for different reasons.
Annotation projection (§2) produces records with their own BIDs, so nothing
collides. A redline render (§2.5) does construct same-BID content, but it is
never merged into anything — it is one side of a comparison, and the comparison's
output is a delta rather than a combined graph.

**The trap to watch for**: a same-BID candidate is *safe to build* and *unsafe to
store*, and nothing in the type system distinguishes the two. Any future
mechanism that needs genuine same-BID composition must confront `union_mut`
directly rather than assuming these constructions established a precedent.

## 3. Composing a source with a record set

A reader wanting the corpus *with annotations live* needs two things together:

```
base:    the compiled source (BeliefBase, BeliefAccumulator, or DbConnection)
records: the annotation store (Issue 105)
```

The base is never mutated. A reparse replaces it; the records survive because
they anchor to a `(QuerySpec, tape_hash)` rather than to a graph instance
(`identity/content_versioning.md` §4).

### 3.1 The read interface

`BeliefSource` (`src/query/mod.rs`) is the abstraction the query layer reads
through, and a composed source implementing it means existing consumers work
without knowing composition is happening.

Two measured facts about the trait and its consumers:

- The trait has five methods — `submap`, `submap_by_bid`, `get_file_mtimes`,
  `export_beliefgraph`, `evaluate` — and **no `get_node`/`get_edges`**. Those
  were deliberately removed in favour of the free functions
  `lookup_node`/`lookup_edges` (`src/query/mod.rs`), which build a
  `QueryPackage` per call.
- `BeliefGraph.states`/`.relations` are `pub` and read directly in ~155 places
  outside `src/beliefbase/`. Those are **consumers of a returned package**, not
  bypasses: a `BeliefGraph` is a transport form (§2.3), so composition has
  already happened by the time one exists. The compiler's own uses
  (`builder.rs`, `compiler.rs` — two thirds of the count) are Layer 1 → 2 and
  upstream of annotation entirely.

**`evaluate` is the only method an annotated read needs.** An annotation anchors
to a `QuerySpec` (§2.2), so reading one *is* evaluating a query — and
`Role::Owner` is implemented in both the in-memory and SQL evaluators, which
means an annotation participates in **selection** rather than being applied to a
query's results. Everything else on the trait is sugar over that: the two
`submap` methods are path-index conveniences (`base.rs:4092`), and
`get_file_mtimes` and `export_beliefgraph` carry default bodies.

That is what makes composition tractable: a composed source has **one method it
must get right**, and everything downstream consumes the package that method
returns.

### 3.2 Writes do not go through the composed source

**Mutation through the projection is forbidden.** The projection is *produced by*
folding records; writes go to the record store, and the projection is recomputed.

This is not a style preference. `beliefbase_architecture.md` §4.3 establishes
that a `BeliefEvent` is an instruction applied directly while an `Annotation` is
a claim requiring interpretation, and that folding an annotation *emits*
BeliefEvents but never the reverse. Making the projection read-only turns that
boundary into a **type constraint** rather than a convention someone must
remember.

## 4. Composition: layers stack

Three scopes (regenerated, repo/user, shared) are three layers, not three special
cases. Reads walk the stack from most-local to least, exactly as a union
filesystem walks its layers.

**Scope precedence becomes layer order** rather than a resolution rule that has
to be specified and maintained separately.

### 4.1 Federation is the same construction

`federated_belief_network.md` §3.6 specifies a `FederatedBeliefSource` wrapping a
local `DbConnection` plus N peers, implementing `BeliefSource`, resolving
conflicts with "local wins." That is this construction with different parameters:

> **`FederatedBeliefSource` does not exist in the codebase** — `grep` returns
> four comments referencing an unimplemented design. The parallel below is
> between two specifications, not between a design and a shipped precedent.

| | `FederatedBeliefSource` | composed source |
|---|---|---|
| Composes | local + N peers | base + N annotation layers |
| Precedence | "local wins" | layer order |
| Layers differ by | *who owns* the scope | *what scope* the records are in |
| Interface | `BeliefSource` | `BeliefSource` |

§1.2 of that document already states the generalization: *"a peer's scope is
another in the same union."* So `[shared, user, repo, regenerated]` on one machine
extends to `[peer-B, peer-A, local-shared, user, repo, regenerated]` across a
federation with no new mechanism. **"Local wins" stops being a rule and becomes
an ordering fact.**

**A remote layer needs no new machinery.** `BeliefSource::evaluate` already
returns a `BoxFuture<Result<...>>`, so slowness and failure are both expressible
on the interface as it stands — a layer that must go over a network is an
ordinary layer whose evaluation takes longer and may return an error. Nothing
about the composition changes.

What a remote layer does raise is a **partial-result policy**: when one layer of
a union is unreachable, does the read degrade to the layers it has or fail whole?
That is a federation question rather than an overlay one — the same question a
`FederatedBeliefSource` faces for a peer that times out — and
`federated_belief_network.md` owns it. The overlay contributes only the constraint
that a degraded read must be *legible as degraded*, for the reason §5 of
`living_corpus.md` gives about gap counts: silently computing against a smaller
evidence set produces a confident wrong answer.

### 4.2 Percolation is layer promotion

`federated_belief_network.md` §1.2 describes percolation: working records stay in
a local scope, and only a folded `RunEnd` summary crosses into a shared one.

Under a layer stack that is a record being **written to a higher layer** — the
same operation as Issue 105's flush. Two consequences:

- **§1.2's open question dissolves.** It asks whether percolation is *policy over
  general sync* or a *transport-level filter*. In a layered read-through model
  these are not alternatives: filtering on read is what a layer stack does, and
  percolation is a decision about which layer a record is *written* to. Read
  composition and write placement are orthogonal, so both halves hold.
- **A layer is a candidate permission boundary.** "My drafts are mine until I
  finish" becomes structural rather than an access-control feature bolted on.
  Closed Issue 16 reached the same conclusion from the focus side. **Open** — the
  async-overlay successor to Issue 110 owns it.

## 5. References

- `annotation/living_corpus.md` §2 — the three-layer model; this document
  specifies the projection's concrete form
- `core/beliefbase_architecture.md` §4.3 — `Envelope`/`Annotation`, and the
  assert-vs-mutate boundary §4.2 makes structural
- `annotation/federated_belief_network.md` §1.2, §3.6 — percolation and the
  federated read path; the same construction at remote scope
- `identity/content_versioning.md` §5.1a — why a reserved key inside the node is
  the rejected alternative
- `src/beliefbase/graph.rs` — `union_mut`, whose wholesale replacement
  constrains anything merging same-BID nodes (§2.6)
- `src/properties.rs` — `WEIGHT_OWNED_BY`, third-party edge ownership
- `src/beliefbase/base.rs` — `owner_edges`, the O(1) owner → edges index
- `src/beliefbase/base.rs` — `graph_for_owner`, the halo primitive
- `src/beliefbase/base.rs` — `compute_diff`, the two-way structural diff
  (Issue 74's need, not the halo's)
- `src/query/spec.rs`, `src/db.rs` — `Role::Owner` in both evaluators
- `src/query/mod.rs` — `BeliefSource`, the read interface
- `src/wasm.rs` — `BeliefBaseWasm`, the browser's existing graph host
- Issue 110 — ratifies or rejects the recommendation; owns steps 1 and 1a
- Issue 105 — the record store the overlay reads
- Issue 16 (closed) — the capability model the permission-boundary question
  inherits
