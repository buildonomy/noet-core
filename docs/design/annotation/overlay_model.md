---
version = "0.1"
title = "Overlay Model — How the Annotation Layer Composes With the Compiled Graph"
authors = ["Andrew Lyjak"]
status = "Target architecture — the composition rule is recommended, not yet ratified"
dependencies = [
  "annotation/living_corpus.md",
  "core/beliefbase_architecture.md",
  "annotation/federated_belief_network.md",
]
---

# Overlay Model

> [!NOTE]
> **Target architecture.** This document specifies how Layer 3's projection
> composes with the compiled graph. The composition *rule* is settled; the
> concrete type is a recommendation that **Issue 110 step 1 ratifies or rejects**.
> Where this document and `living_corpus.md` §2 differ on the projection's
> concrete form, this document is the more specific and §2 will narrow to match.

## 1. Purpose

`living_corpus.md` §2 establishes three layers and says Layer 3's live projection
is an overlay applied to the compiled graph. It does not say *how* that
application works, and the obvious answer is wrong in a way that is easy to miss.

This document answers one question: **when annotations are projected onto a
compiled corpus, what is the resulting object and how is it read?**

It is the shared specification for four things that turn out to be one thing:

| Consumer | Composes |
|---|---|
| Annotation store (Issue 105) | corpus + local annotation records |
| Parse-time observations (Issue 110) | corpus + compiler observations |
| Browser SPA | shipped corpus + locally-authored annotations |
| Federation (`federated_belief_network.md` §3.6) | local + remote peers |

## 2. The problem: wholesale replacement destroys the base

`BeliefGraph::union_mut` (`src/beliefbase/graph.rs:706-714`) replaces nodes
outright:

```rust
for node in rhs.states.values().filter(|node| node.kind.is_complete()) {
    self.states.insert(node.bid, node.clone());
}
```

An overlay node for BID *X* carrying only observations would therefore replace
the corpus node entirely, losing `title`, `payload`, and everything else. To
survive that, the overlay would have to hold **complete copies** of every
annotated node — which makes it a shadow corpus rather than a diff, doubles
memory, and goes stale the instant the corpus reparses.

The root cause is a type error rather than a merge bug: `union_mut` assumes both
sides are `BeliefGraph`s of equal authority, so "merge" can only mean "one side
wins."

## 3. Prior art: read-through with copy-on-write

Two structures that are *sort-of the same thing* with *different ownership rules*
is a well-trodden problem:

| System | Base | Overlay | Resolution |
|---|---|---|---|
| Union filesystems (OverlayFS) | lower dir, read-only | upper dir, writable | Read walks upper, falls through to lower |
| Git | committed tree | index + worktree | Status is a three-way read; the tree is never mutated |
| Copy-on-write B-trees | shared pages | modified pages | A page is copied only when written |
| LSP | file on disk | dirty buffer | Read the buffer if present, the file otherwise |

All four converge on **read-through with copy-on-write**, and — the load-bearing
observation — **none represents the overlay as another instance of the base
type.** The overlay is a distinct type that satisfies the base's *read
interface*.

## 4. The model: `OverlayGraph`

```
OverlayGraph {
    base:    Arc<BeliefGraph>,          // shared, never mutated
    records: RecordStore,               // the annotations (Issue 105)
    patches: HashMap<Bid, NodePatch>,   // memoized projection of records
}
```

- **`base` is immutable and shared.** Nothing is copied. A reparse replaces the
  `Arc`; the overlay's records survive because they anchor to
  `(bid, content_version)` rather than to a graph instance.
- **`patches` is a memoized fold.** Records are the source of truth; patches are
  a derived cache, invalidated when the record set or the base changes.
- **`NodePatch` is field-level.** This is what makes the overlay a diff rather
  than a shadow corpus. Reading an annotated node returns the base node with its
  patch applied; reading an unannotated node returns the base node untouched, at
  no cost.

### 4.1 The read interface already exists

`BeliefSource` (`src/query/mod.rs:40`) is already the abstraction the query layer
reads through. An `OverlayGraph` implementing it means existing query consumers
work over an annotated corpus without knowing the overlay is there.

That an interface factored for an unrelated reason fits this type is the
strongest available evidence the shape is right.

### 4.2 Writes do not go through the overlay

**Mutation through the overlay is forbidden.** The overlay is *produced by*
folding records; writes go to the record store, and the projection is recomputed.

This is not a style preference. `beliefbase_architecture.md` §4.3 establishes
that a `BeliefEvent` is an instruction applied directly while an `Annotation` is
a claim requiring interpretation, and that folding an annotation *emits*
BeliefEvents but never the reverse. Making the projection read-only turns that
boundary into a **type constraint** rather than a convention someone must
remember.

### 4.3 Rejected alternatives

- **Full-copy overlay** — doubles memory; every corpus reparse invalidates every
  copy; and "is this node annotated?" becomes a field-by-field comparison against
  the base.
- **A field-wise `apply_overlay` on `BeliefGraph`** — workable, but it adds a
  fourth merge semantics to a type that already has three (`union_mut`,
  `union_mut_from`, `union_mut_with_trace`). Every future change to node structure
  would then have to be correct in four places. A distinct type keeps overlay
  semantics somewhere they cannot be confused for the base.
- **A reserved key inside `BeliefNode`** — this is the current `metadata` design,
  and it is what produced the diff cascade, the hashing ambiguity, and the merge
  question. A key inside the node puts both classes of data under *one* ownership
  rule — the node's — which is precisely what needs separating. See
  `identity/content_versioning.md` §5.1a.

## 5. Composition: layers stack

Three scopes (in-memory, repo/user, shared) are three layers, not three special
cases. Reads walk the stack from most-local to least, exactly as a union
filesystem walks its layers.

**Scope precedence becomes layer order** rather than a resolution rule that has
to be specified and maintained separately.

### 5.1 Federation is the same construction

`federated_belief_network.md` §3.6 specifies a `FederatedBeliefSource` wrapping a
local `DbConnection` plus N peers, implementing `BeliefSource`, resolving
conflicts with "local wins." That is this construction with different parameters:

| | `FederatedBeliefSource` | `OverlayGraph` |
|---|---|---|
| Composes | local + N peers | base + N overlay layers |
| Precedence | "local wins" | layer order |
| Layers differ by | *who owns* the scope | *what scope* the records are in |
| Interface | `BeliefSource` | `BeliefSource` |

§1.2 of that document already states the generalization: *"a peer's scope is
another in the same union."* So `[shared, user, repo, in-memory]` on one machine
extends to `[peer-B, peer-A, local-shared, user, repo, in-memory]` across a
federation with no new mechanism. **"Local wins" stops being a rule and becomes
an ordering fact.**

### 5.2 Percolation is layer promotion

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

## 6. Open questions

- **Does `BeliefSource` cover an annotated read?** It was factored for the query
  layer and may not span everything a consumer needs. If it falls short, the cost
  rises from "implement a trait" to "widen a trait and update its implementors."
  Issue 110 step 1 audits this.
- **Which consumers hold `&BeliefGraph` directly?** Those bypass the read
  interface and are the migration cost.
- **What may a `NodePatch` patch, and what happens when two records patch the
  same field?** Layer order answers the cross-layer case. Within one layer it is
  unspecified — last-write-wins by `observed_at`, or a surfaced conflict.
- **Is the patch a `BeliefEvent` variant, an overlay-only type, or one
  representation with two application paths?** The edge side already has both
  granularities — `RelationUpdate` replaces a whole `WeightSet`, `RelationChange`
  changes one `WeightKind` — but the node side has only whole-node forms. The
  asymmetry matters: with only whole-node events, the fold must *construct
  complete nodes*, so it needs read access to the base to avoid clobbering fields
  it knows nothing about. A field-level representation decouples them. Note a new
  variant touches ~10 exhaustive `match` sites, and one treating it as a no-op
  fails silently.
- **Cost of the indirection.** A read becomes patch-lookup → miss → base. Cheap,
  but `source_url` sits on essentially every node in a mid-size corpus, so it
  should be measured rather than assumed.
- **Async layers.** Remote peers are fallible and slow; local layers are neither.
  Availability should be a *layer property* rather than forcing `async` onto every
  local read. Owned by the async-overlay successor to Issue 110.

## 7. References

- `annotation/living_corpus.md` §2 — the three-layer model; this document
  specifies the projection's concrete form
- `core/beliefbase_architecture.md` §4.3 — `Envelope`/`Annotation`, and the
  assert-vs-mutate boundary §4.2 makes structural
- `annotation/federated_belief_network.md` §1.2, §3.6 — percolation and the
  federated read path; the same construction at remote scope
- `identity/content_versioning.md` §5.1a — why a reserved key inside the node is
  the rejected alternative
- `src/beliefbase/graph.rs:706-714` — `union_mut`, the wholesale replacement this
  document routes around
- `src/query/mod.rs:40` — `BeliefSource`, the read interface
- `src/wasm.rs:480-502` — `BeliefBaseWasm`, the browser's existing graph host
- Issue 110 — ratifies or rejects the recommendation; owns steps 1 and 1a
- Issue 105 — the record store the overlay reads
- Issue 16 (closed) — the capability model the permission-boundary question
  inherits
