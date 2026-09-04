---
version = "0.1"
title = "Issue 110: Parse as an Annotation Server — Unifying Compiler Observations with the Annotation Layer"
---

# Issue 110: Parse as an Annotation Server

**Priority**: MEDIUM — architectural clarification; unblocks the `metadata` rework
**Estimated Effort**: 2 days design + investigation (RELATIVE COMPARISON ONLY) —
implementation not scoped until the design questions below are settled
**Dependencies**: Requires `docs/design/annotation/living_corpus.md` §2 (the
held-out BeliefBase model) and Issue 105 (the annotation store and its scopes).
Informs Issue 105 step 3a-pre (the `payload`/`metadata` reclassification), Issue
103 (source ranges), and Issue 65 (attestation server as a scope).
**Blocks**: Nothing yet — this reframes work already owned elsewhere.

## Summary

`BeliefNode.metadata` is a grab bag because we have been treating compiler
observations as *node fields* when they are *annotations*. Diagnostics, git
status, `source_url`, layout coordinates, content hashes, and source ranges are
all observations made **about** a node **by** the compiler — which is precisely
the Layer 3 definition (`living_corpus.md` §2).

The reframe: **a parse run is an annotation server instance.** The compiler is an
actor; its observations are records; the overlay it produces is Layer 3's live
projection. We already do this — we just express it as an opaque `Table` bolted
onto the domain model instead of through the annotation API.

Two consequences follow, and the second is the reason this is worth an issue
rather than a note: the browser SPA can host the same thing, which is what makes
a static site annotatable without a server.

## The unification

The ontology already licenses this. `docs/essays/engineering_model_ontology.md`
§3.4: "an automated test pipeline is as much a `P`-identity as a human reviewer."
A compiler is an actor. What it emits about a node is an `R` record.

So there is one mechanism at three scopes — the "scope, not system" framing
Issues 105 and 65 already use:

| Instance | Actor | Produces | Persistence |
|---|---|---|---|
| **Parse** | the compiler | diagnostics, git, `source_url`, layout, hashes, ranges | in-memory; re-derived each parse |
| **Browser SPA** | the reader | notes, todos, sign-offs | IndexedDB, local |
| **Collab server** | many | shared attestations | the Issue 105 store, shared scope |

None of these is a different kind of thing. They differ in *who acts*, *what
scope the records live in*, and *whether the records outlive the process*.

### What this dissolves

**The layering objection.** An earlier analysis held that `_query_specs` and
`_maps_to_specs` could not move to Layer 3, because the HTML generator consumes
them at parse time (`src/codec/compiler.rs:4669`, `:4753`, `:4759`) and a
Layer-2 parse-time operation must not depend on Layer 3. **If parse *is* an
annotation server, the server is running during parse and the inversion
disappears.** The corpus remains renderable because rendering happens inside a
session that has the overlay.

**The merge question.** `BeliefNode::merge` deliberately does not touch
`metadata`, and the investigation in `.scratchpad/merge_metadata_drop.md`
established that the incoming side is always empty
(`BeliefNode::try_from(&IRNode)` hard-codes `metadata: Table::new()`). Under this
model the corpus node carries no metadata at all, so the question does not need
an answer — the field is not there to merge.

**The provenance gap.** That same investigation flagged that `metadata` has no
provenance: nothing records which phase wrote a key or against which content
revision, which is *why* "should the incoming side win?" had no principled
answer. An annotation carries an `Envelope` — `actor`, `observed_at`, `caused_by`
(`docs/design/core/beliefbase_architecture.md` §4.3). Modelling observations as
annotations supplies the freshness signal that was missing.

## The browser instance

`BeliefBaseWasm` (`src/wasm.rs:480-502`) already holds a real `BeliefBase`, loads
shards incrementally, tracks which BIDs came from which shard, and merges via
`self.inner.borrow_mut().merge(&graph)` (`:902`). The viewer already persists
smaller state to `localStorage` (`assets/viewer/network-selector.js:519`,
`panels.js:40`, `resize.js:80`).

So the browser is not "a static site with no server." It is a BeliefBase host
that currently loads one class of graph. An annotation overlay is a second
load-shaped call over an IndexedDB-backed store.

**Two populations must be distinguished**, and conflating them is the main risk:

- **Shipped overlay** — compiler-authored observations (`source_url`, layout,
  hashes) computed at build time and exported alongside the shards. The browser
  cannot derive these; there is no repo and no compiler.
- **Local overlay** — reader-authored annotations, created in the browser,
  persisted locally, and eventually promotable to a shared scope.

They have different trust, different lifetimes, and different sync behaviour.
One store, two scopes.

## The hard problem: overlay application is not `union_mut`

`living_corpus.md` §2 says Layer 3's projection is a held-out BeliefBase merged
onto the compiled graph. **The existing merge does not do what that requires.**

`BeliefGraph::union_mut` (`src/beliefbase/graph.rs:706-714`) replaces nodes
wholesale:

```rust
for node in rhs.states.values().filter(|node| node.kind.is_complete()) {
    self.states.insert(node.bid, node.clone());
}
```

If an overlay node for BID *X* carries only observations, `union_mut` replaces
the corpus node entirely — losing `title`, `payload`, everything. The overlay
would have to hold **complete copies** of every annotated node, which makes it a
shadow corpus rather than a diff, doubles memory, and goes stale the moment the
corpus reparses.

### The recommended resolution lives in a design doc

**`docs/design/annotation/overlay_model.md` is authoritative** for the overlay's
shape, the prior art it follows, the rejected alternatives, and how it unifies
with federation. It was extracted from this issue so that four consumers — the
annotation store, parse-time observations, the browser SPA, and federation — read
one specification rather than each re-deriving it.

In brief: **`OverlayGraph`**, a read-through wrapper holding
`Arc<BeliefGraph>` + records + memoized field-level patches, implementing the
existing `BeliefSource` trait (`src/query/mod.rs:40`). Writes are forbidden
through the overlay — they go to the record store — which makes §4.3's
assert-vs-mutate boundary a type constraint rather than a convention.

**This issue's job is to ratify or reject that recommendation**, not to restate
it. What the issue owns is in Steps below.

**Consequence for `living_corpus.md` §2**: if ratified, "held-out BeliefBase"
narrows to "BeliefBase-*like*" — it satisfies the read interface but is not one.
The *model* is unaffected; only the claim about the projection's concrete type.
This also matches Issue 103's source-range decision (an anchored side table,
explicitly not node fields), making the two one pattern rather than two.

## Steps

1. **Settle the overlay-application semantics** (0.75 days)
   - [ ] Confirm or reject the `OverlayGraph` recommendation above
   - [ ] **Audit `BeliefSource` coverage**: does its method set actually cover
         what an annotated read needs, or was it factored for query only? This
         decides whether the wrapper is cheap or requires widening the trait
   - [ ] Enumerate consumers holding `&BeliefGraph` directly rather than going
         through `BeliefSource` — they are the migration cost
   - [ ] Define `NodePatch`: which fields an annotation may patch, and what
         happens when two records patch the same field (last-write-wins by
         `observed_at`? scope precedence? conflict surfaced?)
   - [ ] Decide mutation-through-overlay. Recommend **forbidden** — writes go to
         the record store, which makes §4.3's assert-vs-mutate boundary a type
         constraint rather than a convention
   - [ ] Decide whether the field-level patch is a `BeliefEvent` variant, an
         overlay-only type, or one representation with two application paths.
         Note `RelationChange` is the existing precedent for a field-level event,
         and that adding a variant touches ~10 exhaustive `match` sites
   - [ ] If not a `BeliefGraph`, correct `living_corpus.md` §2's phrasing

1a. **Open the federated follow-on issue** (0.25 days)
   - [ ] Once the synchronous overlay stack is settled, open an issue to revise
         `docs/design/annotation/federated_belief_network.md` against it:
         §3.6 (whole-node dedup → field-level, per-layer), §1.2 (percolation as
         layer promotion; its open question dissolves), §3.7 (layer-model position)
   - [ ] That issue owns the **async/distributed** extension: remote layers,
         unreachable layers, and how availability becomes a layer property rather
         than forcing `async` onto local reads
   - [ ] **It also inherits authorization**, which nothing currently owns. Closed
         Issue 16 (`docs/project/2_completed/ISSUE_16_AUTOMERGE_INTEGRATION.md`)
         worked out a capability model — actions
         (`Read`/`Write`/`Append`/`Subscribe`), scopes (`AllEvents`/`UserEvents`/
         `FocusEvents`/`EventType`/`TargetBid`), and constraints (time window,
         rate limit, approval-required). The rest of that issue is OBE; the
         authorization thinking is not, and it becomes live the moment an overlay
         layer is a **remote peer** rather than a local scope. Carry it forward
         rather than re-deriving it.
   - [ ] Carry Issue 16's **Decision 3** insight: a focus is a *permission
         boundary*, not only a query scope. Under the layered model that reads as
         **a layer is a permission boundary** — which is what makes percolation
         (`federated_belief_network.md` §1.2) a privacy mechanism rather than
         only volume control
   - [ ] Cross-reference it from `federated_belief_network.md` so the design doc
         names its own successor

2. **Classify the observations by scope** (0.5 days)
   - [ ] **Diagnostics have no migration owner — take it or place it.** Issue 103
         Decision 4 retires the *positional gap* in `BeliefBase.diagnostics` (it
         attaches ranges to drained diagnostics) but leaves the field a
         `Vec<ParseDiagnostic>`. This issue says diagnostics **are** annotations.
         Nobody owns turning them into records. Decide here whether that is this
         issue's step, a follow-on, or explicitly deferred — do not leave it
         unassigned a third time
   - [ ] Which compiler observations are in-memory-only, and which must ship with
         the export? `source_url` is on 1342/1342 nodes in a mid-size corpus, so
         this decides the artifact size
   - [ ] Confirm the in-memory scope does **not** mean per-record files. Issue
         105's store is file-per-record, immutable, append-only — 1342 files per
         parse would be pathological. In-memory observations are re-derived, not
         persisted individually
   - [ ] Reconcile with Issue 105 step 3a-pre, which currently plans to move
         derived keys from `payload` into `metadata`. **If observations leave the
         node entirely, that step's destination changes.**

3. **Define the wrapper / access API** (0.5 days)
   - [ ] How a consumer reads "node plus observations" without every call site
         learning about the overlay
   - [ ] Keep the cost proportionate: `source_url` is dense (every node), `git`
         is sparse (4 nodes, network-level, injected wholesale at
         `src/codec/builder.rs:2478`). Same class in the current taxonomy, very
         different access patterns

4. **Scope the browser instance** (0.25 days)
   - [ ] Confirm IndexedDB is reachable from the existing WASM surface
   - [ ] Decide whether the shipped overlay is a separate artifact or rides in
         the existing shards
   - [ ] Do not build it here — this step sizes it and hands off

## Done When

- [ ] Overlay application semantics are chosen and written into
      `docs/design/annotation/living_corpus.md`
- [ ] Every current `metadata` key is classified: in-memory observation, shipped
      observation, or genuine node content
- [ ] Issue 105 step 3a-pre's destination is confirmed or corrected
- [ ] The browser instance is scoped well enough to become its own issue
- [ ] No new record type; observations reuse the `Envelope` + `Annotation` schema

## Risks

- **Scope creep into rebuilding the compiler's data flow.** The reframe is
  conceptual; the parse pipeline works. → **Mitigation**: this issue changes
  *where observations live* and *how they are addressed*, not when they are
  computed. Phase ordering stays as-is.
- **Per-node cost.** An observation on 1342/1342 nodes routed through a record
  API pays indirection 1342 times per parse. → **Mitigation**: step 2's in-memory
  scope, and measure before committing. The current direct-field access is the
  performance baseline to beat or match.
- **The directive caches are not observations.** `_query_specs` and
  `_maps_to_specs` are a *parse of source content*, not an observation about a
  node — `content_versioning.md` §5.1a is explicit. The layering objection
  dissolves, but the classification does not change. → **Mitigation**: do not
  sweep them into the overlay just because the inversion argument went away.
- **§2 may need to weaken.** If the projection is a wrapper rather than a graph,
  a recently-written design statement becomes partly wrong. → **Mitigation**:
  better to narrow it now than to build toward phrasing that cannot hold.
- **`BeliefSource` may not be the interface it appears to be.** The recommendation
  leans on it being *the* read abstraction, but it was factored for the query
  layer and may not cover every read an annotated corpus needs. If it does not,
  the wrapper's cost rises from "implement a trait" to "widen a trait and update
  its implementors." → **Mitigation**: step 1 audits this before the shape is
  committed to.

## Open Questions

- **Is `parse` one annotation server instance, or one per network?** Sharding and
  incremental parse (Issue 66) operate per-network; an overlay that spans
  networks may not evict cleanly. Note this interacts with the `Arc<BeliefGraph>`
  base: per-network overlays over a shared base is the natural shape, and would
  mirror `BeliefBaseWasm`'s existing per-shard BID tracking (`src/wasm.rs:483-487`).
- **Do overlays compose?** Three scopes (in-memory, local, shared) suggest
  stacking `OverlayGraph`s, each reading through to the next — which is exactly
  what union filesystems do with multiple lower layers. If that works, scope
  precedence becomes layer order rather than a resolution rule. Attractive;
  unverified. It would also give federation percolation
  (`federated_belief_network.md` §1.2) a concrete representation.
- **Is a layer a permission boundary?** If overlays compose, the natural place to
  attach read/write authority is the layer — which would make "my drafts are
  mine until I finish" a structural property rather than an access-control
  feature. Closed Issue 16 reached the same conclusion from the focus side
  (its Decision 3). Not this issue's to settle; flagged so the successor inherits
  a question rather than rediscovering it.
- **Do compiler observations get `caused_by`?** A diagnostic caused by an
  unresolved reference could cite the reference. That would make diagnostics
  traversable rather than just displayable — appealing, and unproven.
- **Does this subsume `BeliefBase.diagnostics`?** Issue 103 already plans to
  retire that stepping-stone. If diagnostics are annotations, the field's
  replacement is the overlay rather than a better `Vec`.
- **What is the actor identity for a compiler observation?** `Envelope.actor` is
  designed for accountable parties. "The compiler at version X" is a defensible
  actor and would make the codec-regression detector
  (`content_versioning.md` §7.2) able to attribute drift.

## References

- `docs/design/annotation/living_corpus.md` §2 — the held-out BeliefBase model
  this issue tests and may narrow
- `docs/design/core/beliefbase_architecture.md` §4.3 — `Envelope`/`Annotation`;
  the assert-vs-mutate boundary the overlay must not violate
- `docs/design/identity/content_versioning.md` §5.1a — the three-way `metadata`
  classification this reframes
- `docs/essays/engineering_model_ontology.md` §3.4 — an automated pipeline is a
  `P`-identity; the licence for treating the compiler as an actor
- `src/beliefbase/graph.rs:706-714` — `union_mut`, wholesale node replacement
- `src/wasm.rs:480-502`, `:902` — `BeliefBaseWasm`, shard loading, graph merge
- `src/codec/compiler.rs:4669`, `:4753`, `:4759` — parse-time directive-cache
  consumption
- `src/codec/builder.rs:2478` — `metadata["git"]` whole-table override
- `.scratchpad/merge_metadata_drop.md` §2-§3 — the metadata occupancy census and
  the provenance gap this model closes
- Issue 105 — the annotation store and its scopes; step 3a-pre's destination
  depends on this issue
- Issue 103 — source ranges as anchored side-table data; the precedent for
  option 3
