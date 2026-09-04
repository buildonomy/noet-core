---
version = "0.1"
title = "Trade Study: Event and Record Schema Unification, and Node Content Hashing"
---

# Trade Study: Event and Record Schema Unification, and Node Content Hashing

**Status**: **Accepted and migrated — superseded by the design documents below.**
**Decision owner**: human

> [!IMPORTANT]
> **Do not cite this document.** Option C was accepted and its content now lives
> in the design documentation. This copy is retained only as the decision record:
> the rejected alternatives, and the reasoning that produced the accepted design.
>
> | Material | Now lives in |
> |---|---|
> | `Envelope`, `Event::Annotation`, assert-vs-mutate | [`beliefbase_architecture.md`](../../../design/core/beliefbase_architecture.md) §4.3 |
> | Layer model, claims vs. evidence, annotation semantics | [`living_corpus.md`](../../../design/annotation/living_corpus.md) |
> | `version` anchor, hash family, staleness scope | [`content_versioning.md`](../../../design/identity/content_versioning.md) |
> | The forward-compatibility exercise rule | [`LESSONS_LEARNED.md`](../../LESSONS_LEARNED.md) §Design constraints |
>
> **The hashing half is two generalizations behind.** It reaches a two-hash
> design and then a `hash(n, kind, radius)` family; `content_versioning.md`
> supersedes both with `(QuerySpec, tape_hash)`, of which the family is the
> precomputed cache. It also carries claims that design review found false —
> notably that per-kind acyclicity is an *enforced* invariant (it is reported,
> not enforced, so a visited set is required) and that `pathmap_order` lives in
> `base.rs`. Those are corrected in the design doc and **not** corrected here.
>
> Options A and B are retained because they record *why* the level separation
> exists. They were rejected on structural grounds — conflating instructions with
> assertions, and multiplying streams — that do not change with circumstance.
> Re-litigation is unlikely; the reasoning is still worth having.

## Problem

Five schemas currently describe overlapping notions of "something happened to a
node". No document owns the reconciliation, and four of the five are unimplemented
— so the cost of unifying is paid now in design review rather than later in
migration.

| Schema | Home | Status | Carries |
|---|---|---|---|
| `BeliefEvent` | `src/event.rs` | **implemented** | graph mutations; `EventOrigin` |
| `ActivityEvent` | Issue 16 | design only | `(device_id, seq)`, `lamport_clock`, `producer`, `target`, `payload` |
| Attestation record | `attestation_fabric.md` §4.2 | design only | `(path, version)`, `attester_id`, `result`, `evidence_hash`, `provenance` |
| As-run record | Issues 17/18 | design only | template + executor context + what happened |
| Annotation record | Issue 105 | unwritten | — |

Concrete evidence of the divergence: Issue 16 defines an event type
`procedure_correction` carrying a `participant_note`. That is a redline, which is
Issue 106's subject, expressed in a schema Issue 106 does not reference. Issue
16's Use Case 2 is Issue 18's execution tracking. Issue 17's three-piece as-run
model is field-for-field congruent with the attestation record and neither
document cites the other.

## Findings from the implementation

Two facts constrain the solution space and are not reflected in any design doc.

**`EventOrigin` is not a provenance field.** It has two variants, `Local` and
`Remote`, and appears at 127 call sites. Its documented meaning is *"has this
already been applied to my state?"* — a local dispatch concern. It does not
identify a producer. Widening it into an identity carrier would touch every one
of those sites and conflate two questions.

**There is no peer identity in the codebase.** `grep` for `PeerId`, `peer_id`,
`device_id` returns nothing outside design docs. Distributed identity is entirely
greenfield; nothing constrains its shape.

**The outer `Event` enum is nearly unused.** `Event { Ping, Belief(BeliefEvent) }`
has two production call sites, both in `src/watch.rs`. It is the natural
extension point and carries almost no migration cost.

## Options

### Option A — Add attestation variants to `BeliefEvent`

Add `BeliefEvent::Attestation(..)`, `BeliefEvent::Annotation(..)`, etc.

- **For**: one enum, no new types, existing plumbing carries it.
- **Against**: conflates levels. Every existing variant is a *mutation instruction*
  the accumulator applies directly. An attestation is an *assertion* that must be
  interpreted before it becomes a mutation. Every `match` on `BeliefEvent` —
  including the accumulator's apply path — would need arms for events it cannot
  apply. Rejected.

### Option B — Separate parallel stream for attestations

Leave `BeliefEvent` untouched; build a second channel for attestation records.

- **For**: zero risk to the compiler.
- **Against**: this is the status quo that produced five schemas. Two streams
  means two orderings, two sync protocols, two consumer registries. Issue 102's
  consumer registry would have to multiplex them anyway. Rejected.

### Option C — Envelope + sibling payload kinds (recommended)

Keep `BeliefEvent` as the graph-mutation vocabulary. Add `Annotation` as a sibling
of `Belief` in the outer `Event` enum. Wrap both in an envelope carrying identity,
ordering, and causality.

```rust
pub struct Envelope {
    pub id: EventId,            // (actor, sequence) — globally unique, no coordination
    pub lamport: u64,           // logical clock; total order with (timestamp, id) tiebreak
    pub actor: ActorId,         // who/what produced this — opaque; JWT sub, device, pipeline
    pub observed_at: Timestamp, // wall clock, display only, may drift
    pub causes: Vec<EventId>,   // causality / provenance chain
    pub payload: Event,
}

pub enum Event {
    Ping,
    Belief(BeliefEvent),        // graph mutation — UNCHANGED
    Annotation(Annotation),     // assertion about a node, without changing it
}
```

`Annotation` is the record schema from `attestation_fabric.md` §4.2, with the
record *kind* discriminated by `protocol_id` per §6 rather than by a Rust enum
variant. This is what collapses the remaining four schemas into one:

| Concept | `protocol_id` | Notes |
|---|---|---|
| `{note}` | `noet:note:v1` | text only; no result, no identity requirement |
| `{todo}` | `noet:todo:v1` | open claim; closed by a later record citing it |
| `{reviewed}` / sign-off | `noet:signoff:v1` | + `attester_id`, optional credential |
| Attestation | `noet:attest:v1` | + credential, policy evaluation, `evidence_hash` |
| Redline | `noet:redline:v1` | proposes a source change; `causes` cites its evidence |

A new record kind is a registry entry, not a code change. That is the property
worth buying.

### Annotations are claims; `R` records are evidence

An earlier version of this table listed an "as-run record" (`noet:asrun:v1`) as a
sibling of the others. That was a category error, and correcting it clarifies
what `Event::Annotation` is for.

`docs/essays/engineering_model_ontology.md` §3.4 and §6.1 distinguish `R` — as-run
records — from the content types `N`, `S`, `P`. `R` is *"not authored content —
it is a read of `s_t`"*, it *"has no model owner and cannot be wrong in the way
`N` or `S` can be wrong"*, and it is the **time dimension** of model-space rather
than a fourth spatial axis: "an observation event — a packet with a specific
space-time context, analogous to a log message or a git commit."

An annotation is not that. An annotation is an **authored claim about a node**,
with an owner, which can be wrong. "I reviewed this section" is a claim; it can
be mistaken, disputed, or withdrawn. "Test T-42 measured 5.03 volts at 14:32" is
not a claim about a node — it is a reading of the world, and it cannot be wrong,
only mis-contextualised.

The two differ in every operational property that matters here:

| | Annotation (claim) | `R` record (evidence) |
|---|---|---|
| Authored by | a person or agent, about a node | a process, about the world |
| Anchored to | one `(bid, version)` | a production context (`N`/`S` active, `P` that produced it) |
| Can be wrong | yes — that is why it carries identity | no — only mis-contextualised |
| Value comes from | the individual record | **accumulation and statistics** |
| Volume | one per human act | unbounded — machine-generated |
| Projects into the graph as | a node plus edges | a distribution, not a node per record |

The last two rows are the practical reason to keep them apart. §3.4 puts it
directly: *"statistics on `R` across the operating domain — how much of the
constraint surface has been checked, against what provenance of `R`, with what
margin — constitute the model's credibility evidence."* The power is in the
aggregate. Projecting each telemetry sample or build log into a `BeliefEvent`
would flood the graph with nodes whose individual identity nobody queries, and
would make the annotation store's volume unbounded and machine-driven rather than
bounded and human-driven.

**The relationship is citation, not identity.** An attestation *claims* something
and cites `R` as its evidence:

```
R records (evidence trail)          Annotation (claim)
  test run 41  ─┐
  test run 42  ─┼──  causes  ──>  noet:attest:v1
  analysis 7   ─┘                  "this requirement is verified"
```

This is exactly the provenance chain `attestation_fabric.md` §4.2a already
specifies, and it is what `causes` and `evidence_hash` are for. The attestation
is in the annotation store because it is a claim with an owner. The `R` it cites
lives wherever `R` lives — a test result database, a telemetry store, a CI
artifact archive — and is referenced, not absorbed.

**What this means for the as-run model in Issues 17 and 18.** A procedure
execution produces `R`. The *record that a procedure was executed and by whom* is
plausibly an annotation; the *observations the execution produced* are `R`. Issue
18 should not assume both live in the annotation store. This is flagged there as
an open question rather than decided here, because it needs the procedure work to
be concrete before the boundary can be drawn well.

**How a citation is addressed is specified in Issue 108**, not here. In outline:
`R` stays in whatever system produces it; the graph carries a *record node* in a
reserved namespace that addresses a cited *span* (one node per span, not per
observation) and holds an aggregate — a coverage statistic, a surprise
distribution — which is the §9.3 "credibility as a typed floor map" shape. A
`RecordSource` interface, shaped like `DocCodec`, resolves and verifies the
address against the owning store. Notably this requires **no new `WeightKind`**:
the citation is an Epistemic edge, because "draws from" is what Epistemic already
means. See `docs/design/annotation/living_corpus.md` §8.

### Why `Annotation` and not `Attest`

`Attest` names a *subtype*, not the base operation. An attestation is an
annotation that additionally carries attester identity, a credential claim, and a
policy evaluation — it is the most constrained member of the family, not its
root. Sign-offs, redlines, and bare notes are its siblings; none
of them is an attestation, and a `{note}` in particular carries no identity
requirement at all. Naming the base after its strictest member would force every
unauthenticated comment through a type called `Attest`.

The base operation is **annotation**: saying something *about* a node without
changing it. That is exactly the boundary this option is drawing — annotations
assert, `BeliefEvent`s mutate. The rows in the table above are then read as
increasing constraint down the list, with `protocol_id` carrying the constraint
rather than the Rust type.

This also keeps the naming aligned with the surfaces users touch: Issue 104 is
the annotation vocabulary, Issue 105 is the annotation sidecar store. Only Issue
65 is about attestation specifically, and it is one consumer among several.

## Why the levels must stay separate

`attestation_fabric.md` §12.3 already specifies the projection from records to
graph mutations:

- annotation record → `NodeUpsert`
- `provenance` / `causes` link → `RelationUpdate` (Epistemic)
- coverage assertion → `RelationUpdate` (Pragmatic)

So folding an `Annotation` event **emits** `BeliefEvent`s. This is the
unification: one stream, two payload kinds, one direction of derivation.
Consumers that only care about graph state subscribe to the projection and never
observe an `Annotation` payload. Consumers that need annotation semantics
(compliance queries, sign-off summaries, open-todo lists) read the records
directly.

Option A destroys this by making the two levels indistinguishable at the type
level.

## Relationship to `EventOrigin`

`EventOrigin` stays exactly as it is, on `BeliefEvent` variants. It answers
"already applied?"; `Envelope.actor` answers "produced by whom?". They are
orthogonal and both are needed: a locally-generated event from a remote actor's
record is `EventOrigin::Local` with a remote `actor`.

Renaming `EventOrigin` to something like `Applied` would be clearer but touches
127 sites for no functional gain. Out of scope; note it in BACKLOG.

## Relationship to Issue 16 (Automerge)

The envelope's `id`, `lamport`, and `actor` fields are Issue 16's `ActivityEvent`
identity and ordering fields. Carrying them now is what makes Issue 16 a
**storage swap** rather than a schema migration.

Issue 16 remains 3-4 weeks, v0.4.0+, blocked on Keyhive (pre-release). It must
**not** become a dependency of Issue 105. The relationship is forward
compatibility, not sequencing.

### The forward-compatibility field is a liability unless exercised

"Costs nothing" is false. A field carried but never read is not free — it is
unverified data that accumulates, and on the day the second writer arrives it is
as likely to be garbage as to be useful. A logical clock is only correct if
*every* writer maintains it correctly, and a single-writer deployment never
exercises that property. The failure mode is silent: the field looks populated,
nothing rejects it, and the ordering it implies is wrong.

The fields are not equally exposed to this. Auditing the downstream issues shows
a clean split:

| Field | Exercised in Phase 1 | By what |
|---|---|---|
| `id` (`actor`, `sequence`) | **yes** | sidecar filename; G-Set uniqueness (Issue 105) |
| `actor` | **yes** | scope-conflict resolution (Issue 105) |
| `causes` | **yes** | close/revoke, redline chains (Issues 104, 105, 106) |
| `observed_at` | **yes** | display; staleness surfacing |
| `lamport` | **no** | no Phase 1 consumer |

Four of the five are load-bearing from day one and are therefore verified by
ordinary use — a bug in them breaks something visible. `lamport` is the sole
exception, and it is exactly the field whose correctness depends on multi-writer
discipline that Phase 1 cannot provide.

**Therefore `lamport` must be either exercised or omitted — not carried inert.**
Three options:

1. **Omit until a second writer exists.** Cheapest, and the honest reading of
   YAGNI. Cost: retrofitting an ordering onto an existing log is not possible,
   because historical records have no clock value and none can be reconstructed.
   The log written before the retrofit is permanently unorderable relative to the
   log written after.
2. **Carry and exercise it.** Include it, and make the fold that derives state
   *use* it as the ordering key rather than sorting by `observed_at`. On a single
   writer this is a monotonic counter and the derived order is trivially checkable
   against insertion order — a property a test can assert. This is the option that
   makes the field self-verifying.
3. **Carry it inert.** Rejected. This is the case the objection identifies.

**Recommend option 2**, with a specific obligation attached: the derived-state
fold in Issue 105 orders by `(lamport, observed_at, id)` — never by wall clock
alone — and a test asserts that for a single writer the `lamport` order matches
insertion order across a process restart. That test is cheap, and it is the thing
that converts "forward compatibility" from an assertion into a checked property.

The same obligation generalizes: **any field justified by forward compatibility
must name the Phase 1 mechanism that exercises it.** If no such mechanism exists,
the field is speculative and should be omitted. Applied to this envelope, all
five fields pass — four by ordinary use, `lamport` by the fold-ordering rule
above.

Note also that an append-only set of immutable records with globally unique IDs is
a **G-Set** — a state-based CRDT whose merge is set union. Conflict-freedom for
the Phase 1 file-per-record sidecar (Issue 105) follows from the schema, not from
Automerge. Automerge earns its place only for character-level concurrent text
editing, in-place mutation of records, or efficient delta sync at scale — none of
which are Phase 1 concerns. Issue 65's existing decision that *revocation is
prospective, not retroactive* is what keeps records immutable; that decision is
load-bearing for CRDT semantics and must not be softened.

## Migration path

Additive throughout. No existing call site changes behavior.

1. Add `Envelope`, `EventId`, `ActorId`; add `Event::Annotation(Annotation)`.
2. Existing producers construct an `Envelope` with a constant local `actor` and a
   monotonic counter. `Event::Belief` payloads are unchanged.
2a. Make the derived-state fold order by `(lamport, observed_at, id)`, and add
   the single-writer ordering test described above. This is not optional polish —
   it is what keeps `lamport` from becoming inert data (see §The
   forward-compatibility field is a liability unless exercised).
3. Define `Annotation` per `attestation_fabric.md` §4.2 plus the `protocol_id`
   registry from §6.
4. Implement the §12.3 projection `Annotation -> Vec<BeliefEvent>`.
5. Issue 105 persists `Envelope`s; Issue 102 routes them; Issue 65's server syncs
   them.

## The `version` anchor

> [!IMPORTANT]
> **Superseded by [`../../design/identity/content_versioning.md`](../../../design/identity/content_versioning.md).**
> Everything from this heading through §Staleness radius has been migrated to
> that design document, which supersedes it in two ways:
>
> - The general model is **`(QuerySpec, tape_hash)`** — a scope expressed as a
>   query, paired with a hash over what that query returned. The
>   `hash(n, kind, radius)` family developed below is the **precomputed cache of
>   its most common instances**, not the model. Scopes this study could not
>   express — filtered ("all Class-A items under §3"), composed, mixed-kind — are
>   ordinary queries.
> - Several claims below were corrected during design review: termination
>   requires a visited set (invariant 0 is reported, not enforced),
>   `BeliefKind::Trace` must be excluded from the hashed `kind`, storage is per
>   node **per kind**, and `pathmap_order` is at `src/paths/pathmap.rs:476`.
>
> This material is retained for the decision record — the rejected alternatives
> and the reasoning that produced the family are not repeated in the design doc.
> **Cite `content_versioning.md`, not this section.**

**Resolved: `version` is a content hash over the node's non-metadata content.**

Three definitions were in circulation — `attestation_fabric.md`'s
`sha256(content)`, the collaboration overlay's `asset_version` (FNV-1a over the
whole beliefbase), and Issue 105's undefined `(bid, version)` key. The
whole-beliefbase option is rejected outright: any edit anywhere would invalidate
every annotation anchor in the corpus, so a typo fix in one document would mark
every sign-off in the corpus stale.

### What is hashed

`BeliefNode` (`src/properties.rs:1059`) has seven fields. The hash covers the six
that carry the node's meaning and excludes `metadata`:

| Field | In hash | Rationale |
|---|---|---|
| `bid` | no | identity, not content — it is the other half of the anchor |
| `kind` | **yes** | changing a node's kind changes what was reviewed |
| `title` | **yes** | user-visible content |
| `schema` | **yes** | changes validation semantics |
| `payload` | **yes** | the structured content |
| `id` | **yes** | user-authored identity; a rename is a meaningful change |
| `metadata` | **no** | see below |

This is the node's `content_hash`. Every node additionally carries a
`section_hash` covering its Section-edge sources — see §Staleness radius below
for why structural containment needs its own hash and why computing it is
O(V+E).

`metadata` is excluded because it carries per-parse runtime annotation that
changes without the node's meaning changing: git commit/branch/dirty status
(`properties.rs:2033`), 3D layout coordinates (`layout.rs:216`), and content
profiles (`layout.rs:1071`). Including it would invalidate every annotation on
every node on each commit, or whenever layout is recomputed — reintroducing the
whole-beliefbase problem through a side door.

Note this makes the version hash's field set **almost but not exactly**
`BeliefNode`'s `PartialEq`, which *does* include `metadata` (`properties.rs:1099`)
and excludes nothing. The divergence is deliberate and must be commented at both
sites: equality asks "is this the same node state?" (merges and `compute_diff`
need metadata propagated); the version hash asks "is this the same node
*content*?" (annotation staleness must not fire on a git commit). Implementing
the hash by reusing `PartialEq`'s field set would be wrong.

### Consequences

- Editing node A does not stale annotations on node B. This is the property the
  whole-beliefbase hash could not provide.
- A node that is edited and then reverted returns to its prior `version`, and
  annotations anchored there become current again. This is correct — the content
  is genuinely the same — and is a strictly better outcome than the timestamp- or
  build-based alternatives.
- `asset_version` is retained as a separate **informational** field on records
  (which build observed this), never as the anchor. Issue 65's
  `collaboration_overlay.md` mapping must be updated accordingly: the overlay's
  `(site_url, asset_version, bid)` triple becomes `(site_url, bid, version)`.
- The hash must be stable across processes and platforms: fixed field order, a
  canonical serialization of `payload`/`id`, and no `HashMap` iteration order in
  the input. `Table` is `toml_edit`'s ordered table, so this is achievable, but it
  needs an explicit test asserting the same node hashes identically across runs.
- Algorithm: `sha256`, per `attestation_fabric.md` §4.2, rather than FNV-1a.
  Annotation anchors are long-lived compliance artifacts and collision resistance
  matters more than hashing speed here.

### Staleness radius: the general form

A node's `content_hash` is not the only version anchor an annotation can want,
and which one it wants is **a property of the annotation kind, not of the node**.
That is the observation the rest of this section generalizes.

#### An annotation selects the staleness semantics it asserts against

An annotation does not merely *have* a version anchor. It **selects which kind of
version it asserts against**, and that selection is semantic:

| Claim | Anchors to | Because |
|---|---|---|
| "I proofread this paragraph" | `content_hash` (radius 0) | only the node's own text was read; a distant change is noise |
| "I reviewed this section" | `section_hash` (radius ∞, Section) | structural containment is what was reviewed |
| "I verified this requirement is satisfied" | `epistemic_hash` (radius ∞, Epistemic) | the verification rests on a reasoning chain; if upstream evidence moved, the verification is stale even though the requirement's own text is untouched |
| "I signed off on this interface and its immediate dependencies" | radius 1 | the neighbourhood is the scope |

So `protocol_id` selects not only a record schema and a set of graph roles but a
**staleness semantics**. Two annotations on the same node, by the same actor, at
the same instant, can legitimately stale differently, because they assert about
different scopes.

This is why the parameterized family is *required* rather than merely elegant.
With two hardcoded hashes every annotation kind is forced into one of two
staleness models, and the epistemic-chain case — the one that carries the weight
for verification and compliance claims — cannot be expressed at all. It is not
that the third hash would be nice to have later; it is that a design admitting
only two has already decided that "my evidence changed" is not a thing an
annotation may say.

The caveat is that this argument establishes the *shape*, not the build order.
Which instances Phase 1 computes is a separate question, settled below.

#### The motivating case: containment is structural, so content hashes cannot see it

Most relations are *expressed in content*. A `{maps_to}` directive is text in the
owner's body, so the owner's `content_hash` moves when the directive text moves
— the content hash captures it for free.

Section edges are not like this. Containment is **structural**: it is derived
from document position, not from a string inside the parent. A heading node's own
fields (`kind`, `title`, `schema`, `payload`, `id`) are unchanged when a child is
added, removed, reordered, or edited, because the child's prose belongs to the
child's node, not the parent's. So a pure `content_hash` reports "unchanged" for
a section whose entire body was rewritten — which makes "I reviewed this section"
unanchorable, since the thing the reviewer read is not what the hash covers.

> [!NOTE]
> This holds cleanly for heading nodes. It is weaker for **document** nodes,
> whose `payload` carries the `sections` table written by `MdCodec::finalize`
> (child `bid`/`id`/`schema` per section). A document node's `content_hash`
> therefore already moves when a child is added, removed, or renamed — though
> *not* when a child's prose is edited in place. The argument for a separate
> structural hash survives, but the claim "a parent's own fields are entirely
> unchanged" is false for document nodes and should not be restated as an
> absolute.

Radius ∞ over Section edges is the hash that answers the reviewer's actual
question, and it is the first non-trivial member of the family.

#### The family

The hashes are not separate features. They are **radii of one parameterized
construction**, and naming the parameter is what makes the design ergonomic
rather than ad hoc:

> `hash(n, r)` = a Merkle hash over `n` and everything reachable from `n` within
> `r` hops along a chosen edge set.

| Radius | Answers | Status |
|---|---|---|
| **0** | Did this node's own content change? | **build now** — `content_hash` |
| **1** | Did this node or anything it directly relates to change? | **defer** — see below |
| **∞** | Did anything in this node's dependency closure change, *along one edge kind*? | **build now** for Section (`section_hash`); same function, other kinds, when consumed |

Radius ∞ is **stratified by edge kind** — one hash per `WeightKind`, each
confined to its own subgraph. This is what makes it terminate; see below.

Radius 0 and ∞ are the two that Phase 1 consumers actually anchor to, and they
bound the useful range. Radius 1 is the interesting middle and is deliberately
not built yet — not because it is hard, but because nothing consumes it. Under
the rule in §The forward-compatibility field is a liability unless exercised, a
third hash with no Phase 1 reader is exactly the inert data that rule rejects.
The construction is parameterized, so adding it later is an argument change, not
a redesign.

#### The constraint that shapes this: cycles, and how stratification removes them

**Radius ∞ is only well-defined over an acyclic edge set, and the noet graph is
acyclic *per edge kind*, not in the union.** `BeliefBase`'s invariant 0
(`src/beliefbase/base.rs:237`) is checked by three independent SCC passes — one
each for Section, Epistemic, and Pragmatic (`base.rs:1198-1223`). Nothing forbids
a cycle that alternates kinds: section A contains B, B `{maps_to}` A is legal and
unremarkable. A naive radius-∞ hash over the *union* of edge kinds would not
terminate on real corpora.

**Stratifying by edge kind removes the problem rather than working around it.**
Compute one radius-∞ hash *per edge kind*, each traversing only its own
subgraph:

```
content_hash(n)        = H( own fields )                       // radius 0
section_hash(n)        = H( content_hash(n) ‖ section_hash(s)  for s ∈ sources_section(n) )
epistemic_hash(n)      = H( content_hash(n) ‖ epistemic_hash(s) for s ∈ sources_epistemic(n) )
pragmatic_hash(n)      = H( content_hash(n) ‖ pragmatic_hash(s) for s ∈ sources_pragmatic(n) )
```

Each recursion is confined to a single `WeightKind` subgraph, and the mixed-kind
cycle is unreachable by construction because no recursion ever crosses kinds.
The infrastructure already exists: `BeliefGraph::as_subgraph(kind, reverse)`
(`src/beliefbase/graph.rs:164`) yields exactly these per-kind views.

> [!WARNING]
> **Stratification removes the mixed-kind cycle. It does not license omitting a
> visited set.** Invariant 0 is *checked* by `built_in_test`, not *enforced*: the
> SCC passes only append to `diagnostics` and clear the `balanced` flag
> (`base.rs:1356`), and `built_in_test` is expensive by its own docstring and is
> not run on every parse. More decisively, `PathMap::new_indexed` maintains a
> `loops: BTreeSet<(Bid, Bid)>` populated from `DfsEvent::BackEdge` while walking
> the **Section** subgraph, and then works around the entries it finds
> (`src/paths/pathmap.rs:2017-2036, 2146-2156`). Same-kind Section cycles
> therefore do occur in practice and the pathfinder is written to survive them.
> A hash that assumes acyclicity would hang on exactly the corpora that
> motivated that code. Carry a visited set, and treat a back edge the way
> `PathMap` does — skip the contribution and record a diagnostic — so the hash
> is total.

#### Cost: O(V+E), not quadratic

The intuition that a second hash is expensive on deep section graphs assumes the
naive algorithm: for each node, walk its whole subtree and hash the contents.
That is O(n × average subtree size) — quadratic in the worst case, and it is the
right thing to reject. A Merkle DAG is not that. Computed bottom-up with
memoization, **each node is hashed exactly once** — one post-order traversal,
O(V+Eₖ) per kind. Sources contribute their already-computed hashes, not their
contents. This is precisely what git does for tree objects on every commit.
Computing all three kinds is O(V+E) over the whole multigraph, since the kinds
partition the edge set.

- **Compute**: one extra pass at export, alongside the existing shard walk.
- **Storage**: 32 bytes per node per kind. ~1 MB per kind on a 32k-node corpus.
- **Incremental**: editing one node invalidates only its ancestors along that
  kind — O(depth), not O(n).

Nodes may have multiple Section parents (a document included from several
places), so each per-kind subgraph is a DAG rather than a tree. Memoization
handles that without change; a shared subtree is hashed once and contributes to
each parent.

The genuine cost is **semantic, not computational**: in a deep chain, one leaf
edit stales every ancestor's radius-∞ annotation on that kind. That is inherent
to the question "did anything beneath me change?" and not an artifact of the
implementation. It is also why radius 0 must remain available — a reviewer who
only attested a heading should not be disturbed by a change three levels down.

#### Requirements on a radius-∞ hash

- **Source order is included.** Section children have a deterministic
  `order: Vec<u16>` compared by `pathmap_order`
  (`src/paths/pathmap.rs:476`; `src/beliefbase/base.rs:986` is a call site, not
  the definition). Reordering sections changes meaning and must change the hash.
  Epistemic and Pragmatic sources carry `WEIGHT_SORT_KEY` and must likewise fold
  in a deterministic order.
- **Source identity and count are included** via their hashes, so additions and
  removals are detected — the case a pure content hash misses entirely.
- **Only the one kind's edges are traversed.** A debug assertion that the
  traversal never leaves `kind` makes a future violation fail loudly.
- **Subnet boundaries are traversed** like any other Section edge; a network index
  node's `section_hash` covers its network.

**The base case falls out for free.** A node with no sources on a given kind has
nothing to fold in, so its hash for that kind degenerates to its `content_hash`.
No special-casing, no sentinel value — a leaf's `section_hash` *is* its
`content_hash`, which is also the right semantics: for a node that contains
nothing, "did anything under me change?" and "did I change?" are the same
question.

The Section member of this family is `section_hash`. An earlier draft of this
study called it `subtree_hash` and presented it as one of exactly two hashes;
that name is retired, because it hid the parameter that makes the design
coherent. `epistemic_hash` and `pragmatic_hash` are the same function with a
different `WeightKind` argument.

**A cross-kind hash remains ill-defined and is out of scope.** "Did anything at
all change anywhere upstream of me, along any edge?" cannot be answered by this
construction, because that closure genuinely can contain cycles.

> [!NOTE]
> A composition such as `H(section_hash ‖ epistemic_hash ‖ pragmatic_hash)` is
> well-defined and cheap, but it is **not** equivalent to the cross-kind closure
> and must not be described as answering the same question. It misses every
> mixed-kind path: if `A` contains `B` via a Section edge and `B` draws on `C`
> via an Epistemic edge, `C` is in neither of `A`'s per-kind closures, so no
> combination of them detects a change to `C`. The composition also folds
> `content_hash(n)` in three times, which is harmless but should be stated
> rather than discovered. If the cross-kind question is ever genuinely wanted,
> it needs its own construction — a bounded walk, or an explicitly cycle-tolerant
> traversal — not a product of the three.

Radius 1 is unaffected by any of this — a bounded walk terminates regardless of
cycles, so it may span edge kinds freely.

#### Why radius 1 is the interesting middle

Sections are the acute case of a broader gap: **a node's content hash cannot see
its edges at all**, in either direction. A requirement that gains an incoming
`{maps_to}` claim has unchanged content, yet what is true *about* it has changed
— plausibly relevant to whoever signed off on it. Radius 1 over
Section+Epistemic+Pragmatic is precisely the "has my immediate neighbourhood
shifted?" question, and it is the one a reviewer most often means.

What blocks it is not computation but **policy**: which edge kinds count, whether
direction matters (incoming `{maps_to}` almost certainly should; outgoing may
not), and whether a neighbour's radius-0 or radius-∞ hash is the input. Those
choices are unanswerable without real annotation data showing which staleness
signals people act on and which they learn to ignore. Guessing now risks building
the always-fires signal that `asset_version` already taught us to avoid.

Recorded as future work with a named parameter, not as a gap.

This also resolves Issue 103's parallel question about section *source ranges* —
the same distinction applies (heading line vs. whole subtree span), and it should
track the same radius-0 / radius-∞ split.

## Open questions

- ~~Is `lamport` worth carrying before a second writer exists?~~ **Resolved**:
  yes, but only under the exercise obligation in §The forward-compatibility field
  is a liability unless exercised. Carrying it inert is rejected.
- Should `causes` be `Vec<EventId>` or a single `Option<EventId>`? Fabric §4.2a
  provenance chains are DAGs, so `Vec`. Confirm against Issue 18's redline needs.
- **Does `epistemic_hash` have a Phase 1 consumer?** If `protocol_id` selects a
  staleness semantics, then `noet:attest:v1` and `noet:signoff:v1` on a
  requirement plausibly want the Epistemic closure, not the Section one — which
  would make the hash load-bearing in Phase 1 rather than speculative. Computing
  it is an argument change, so the cost is near zero; the real question is
  whether the always-fires risk that deferred radius 1 also applies here.
- **Does hashing `kind` break shard-hydration stability?** `BeliefKindSet`
  carries `BeliefKind::Trace`, which is a load-state flag removed on merge
  (`properties.rs:1335`), not content. A node hashed while Trace and rehashed
  when complete would produce two versions and stale every annotation on it.
  Either exclude `Trace` from the hashed `kind`, or hash only the content-bearing
  kinds.
- **What happens to an anchor when the cited node is deleted?** A deleted Section
  source changes its parents' `section_hash`, which is correct. But an annotation
  anchored directly to the deleted node's `(bid, version)` orphans, and nothing
  in this design says whether that surfaces as stale, as broken, or silently.
  Issue 104 already flags the parallel BID-migration case; these should be
  answered together.
- **Is `payload` a stable hash input?** `payload["text"]` is regenerated from the
  markdown event stream and only rewritten when `inject_context` fires
  (`md.rs:2380-2395`). A hash over `payload` inherits whatever instability that
  regeneration has, and the required cross-process stability test must cover the
  round trip, not just repeated hashing of one in-memory node.
- **`{maps_to}` is not fully content-expressed.** The resolved directive spec
  lives in `metadata["_maps_to_specs"]`, which the hash excludes, and the edge is
  third-party-owned via `WEIGHT_OWNED_BY` — so it appears in neither endpoint's
  `content_hash`. The claim that content-expressed relations are "captured for
  free" holds only for the directive's prose, not its resolution.

## Decision

**Option C accepted.** Envelope + sibling payload kinds; `Event::Annotation` as
the base operation with `protocol_id` discriminating record kinds.

**`version` accepted** as a `sha256` content hash over the node's non-metadata
content (`kind`, `title`, `schema`, `payload`, `id`), excluding `bid` and
`metadata`.

**A kind-stratified, radius-parameterized hash family accepted** as the model.
`hash(n, kind, radius)` is the surface; the named instances are `content_hash`
(radius 0) and `section_hash` / `epistemic_hash` / `pragmatic_hash` (radius ∞,
one per `WeightKind`). `protocol_id` selects which a given record anchors to —
and that selection is a **staleness semantics**, not merely a key choice: a
proofreading claim, a section review, and a verification claim assert about
different scopes and must stale independently. A node with no sources on a kind
has that kind's hash equal to its `content_hash`.

**Phase 1 builds `content_hash` and `section_hash`**, via one kind-parameterized
function rather than two hardcoded passes. Stratification is what makes radius ∞
well-defined — the mixed-kind cycle is unreachable when no recursion crosses
kinds — but it does **not** remove the need for a visited set: invariant 0 is
reported by `built_in_test`, not enforced, and `PathMap` already detects and
works around Section-subgraph back edges. See §Staleness radius.

**`lamport` accepted** under the exercise obligation: the derived-state fold
orders by `(lamport, observed_at, id)` and Issue 105 carries the single-writer
ordering test.

Remaining before implementation: the `BeliefNode.metadata` field conflict
recorded in Issue 103 Decision 2.

Deliberately deferred: radius 1, which needs real annotation data before its
direction and edge-kind policy can be chosen.

**Reopened by this revision**: whether `epistemic_hash` is Phase 1 rather than
deferred. The forward-compatibility rule asks for a Phase 1 consumer, and
verification and sign-off annotations (Issues 104, 105, 65) are Phase 1 — a
verification claim whose evidence moved is exactly the staleness signal
`epistemic_hash` exists to carry, and `noet:attest:v1` has nowhere else to
anchor it. See §Open questions.

### Downstream obligations created by this decision

| Document | Required change |
|---|---|
| Issue 105 | `version` defined; hash-stability test; anchor no longer open |
| Issue 65 | overlay anchor `(site_url, asset_version, bid)` → `(site_url, bid, version)`; `asset_version` demoted to informational |
| `collaboration_overlay.md` §3.1–3.2 | §3.2 argues `asset_version` *is* the right fingerprint — now superseded; needs rewriting |
| Issue 103 | section *source range* takes the same two-value shape as the two hashes (heading span vs. subtree span) |
| Issue 66 | export computes `content_hash` + `section_hash` per node via one kind-parameterized function; both must reach the shards |
| `attestation_fabric.md` §4.1 | confirm the noet `path`/`version` mapping matches this definition |
