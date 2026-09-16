---
version = "0.1"
title = "Issue 105: Record Store and Fold — Storage Trait, Semantic Enhancement, Shard Co-location"
---

# Issue 105: Record Store and Fold

**Priority**: HIGH
**Estimated Effort**: 5 days (RELATIVE COMPARISON ONLY) — 2 storage, 2 fold and
run brackets, 1 shard co-location and manifest
**Dependencies**: Requires Issue 110 (the annotation channel — this issue is the
sink it routes to, and the in-memory sink 110 builds is this issue's first
`RecordSink` impl). Requires `beliefbase_architecture.md` §4.3 (`Envelope`,
`EventId = (actor, session, sequence)`). Requires Issue 17 **narrowly** — the
exit predicates (`all` / `any` / `ordered` / `n-of`), the outcome/effect model,
and stable step BIDs that the fold consumes; not the directive parsing itself. Requires Issue 66 for BID
stability of anchors. **Does not require** the node hash family (moved to
Issue 103) or the `payload`/`metadata` reclassification (Issue 110 step 3).
**Blocks**: Issue 104 (the vocabulary needs a fold to test against), Issue 65
(sync peer), Issue 74 (the halo it renders), the W4 receipt skeleton.
**Absorbs**: Issue 105 (record store and fold) — see §Why one issue.
**Design**: `docs/design/annotation/annotation_channel.md` §6 (the halo of
stores), `overlay_model.md` §2 and §6 (how records are read), `living_corpus.md`
§4 (annotations are stateful).

## Summary

Three deliverables that cannot be validated apart:

1. **`RecordSink`** — the storage IO trait the annotation channel routes to.
   Append, cursor-read, enumerate the `(actor, session)` pairs held. In-memory
   and filesystem backends here; IndexedDB later.
2. **Semantic enhancement** — the trait that turns a record set into state.
   Partition by run, fold against a lifecycle, and **project one run to one
   graph node** whose owned edges form the halo. This is where records become
   `Bid`s.
3. **Shard co-location** — the filesystem backend lives beside the corpus's
   shards and is registered in its own manifest, so a reader discovers the stores
   in its halo the same way it discovers networks.

## Why one issue

A record store without a fold is a directory of files, and there is no test
that says the directory is right. A fold without a store folds over a `Vec` in
a unit test and never meets the append-only, out-of-order, multi-scope record
sets that make folding hard. The two were separate issues because the fold was
believed to be a *consumer* of the store. It is not — it is the half of the
store that makes it a store of *annotations* rather than of bytes.

The decision that fixes the seam: **one `Bid` per run, not per record.** A run
is the unit that projects into the graph. Its records are collated by the fold
into one node — the run header — whose owned edges into the anchor queryset are
the halo (`overlay_model.md` §2). Single-record claims (a receipt, a `{note}`)
are runs of length one, and a run of length one *is* a `RunEnd` (§Partition), so
no claim sits outside the model. That makes the fold the *only* path from
`EventId` space to `Bid` space, which is why it must be specified alongside the
store that holds the `EventId`s.

## Architecture

### `RecordSink`: what the channel writes to

```
trait RecordSink {
    fn append(&mut self, env: Envelope) -> Result<()>;
    fn read_from(&self, cursor: Cursor) -> impl Iterator<Item = &Envelope>;
    fn sessions(&self) -> impl Iterator<Item = (ActorId, SessionId)>;
    fn identity(&self) -> StoreId;
}
```

- **`append` is the only write.** No update, no delete. A record is immutable
  from the moment it lands (`living_corpus.md` §4). Merging two sinks is set
  union on `EventId`.
- **`Cursor` is `(actor, session, sequence)`** — the P1 identity scheme is
  already a resumable log position, per session. Contiguity within a session is
  what lets a reader resume; concurrency between sessions is what makes the
  cursor a small vector rather than one number.
- **`sessions()` is how a store describes itself** to the halo without being
  read. Whether a store also needs its own `ActorId` is open (`annotation_channel.md`
  §8).

Two impls here:

| Impl | Where | Purpose |
|---|---|---|
| `MemorySink` | `Vec<Envelope>` | Issue 110's first sink; tests; the browser before IndexedDB |
| `FsSink` | `beliefbase/annotations/<store-id>/` beside the shards | the durable local store |

### Filesystem layout, co-located with shards

The corpus's durable form is `beliefbase/manifest.json` +
`beliefbase/networks/{bref}.msgpack` (`src/shard/export.rs`). The annotation
stores sit beside it:

```
beliefbase/
  manifest.json                    # corpus shards — unchanged
  global.msgpack
  networks/{bref}.msgpack
  annotations/
    manifest.json                  # the store halo — separate, dynamic
    <store-id>/
      <actor>-<session>.log        # one append-only file per session
```

**Separate manifests, deliberately.** The corpus manifest describes a build
output that changes only on rebuild. The annotation manifest describes a set
that clients open, add to, remove, and reconfigure at will. Coupling them would
make every session open a corpus-manifest write, and would put user-dynamic
state under a file the build owns.

**One file per session, not per record.** A session is a contiguous
`sequence` run from one writer, so it is exactly the append-only unit: open
for append while the session is live, immutable once closed. This replaces the
earlier file-per-record shape — 128k records from one layout run would have
been 128k files; as one session it is one file with one record in it. Records
from concurrent sessions never share a file, so there is no lock.

**Partition by store, not by BID prefix.** A store is a unit of ownership and
precedence (§Halo); a BID prefix is neither.

### The annotation manifest

```
{
  "version": "1",
  "default_halo": ["<store-id>", "…"],
  "stores": [
    { "id": "…", "kind": "local",  "precedence": 0, "ships": false, "sessions": [...] },
    { "id": "…", "kind": "shared", "precedence": 1, "ships": true,  "sync": {...} }
  ]
}
```

- **`precedence`** is the configured order that gives "narrower wins" its
  meaning when two stores hold claims about one node (`overlay_model.md` §4).
- **`ships`** replaces the old "`.noet/` is gitignored" rule. Whether a store
  travels with the shards is a per-store property: a scratch store does not, a
  promoted shared store does. A fresh clone has exactly the stores that were
  marked to ship.
- **`default_halo`** is the corpus's declaration of which stores a reader should
  listen to — the team's agreed sources of truth. Without it a gap count is a
  property of each reader's configuration rather than of the corpus, and two
  people disagree with neither being wrong (`living_corpus.md` §5). A reader may
  still widen or narrow; what the default fixes is the *unmodified* reading.
  Because the manifest ships with the shards, the declaration is reviewable —
  the same argument `living_corpus.md` §10 makes for write authority.
- **`sync`** is where a network conduit attaches. To a reader the store looks
  like any other; the conduit is how records arrive in it. The protocol is
  `collector_model.md` §4's and is **not this issue's** — only the slot is.

### Semantic enhancement: records → runs → nodes

```
trait Fold {
    fn partition(&self, records: impl Iterator<Item = &Envelope>) -> Vec<Run>;
    fn derive(&self, run: &Run, lifecycle: &Lifecycle) -> RunState;
    fn project(&self, run: &Run, state: &RunState) -> (BeliefNode, Vec<BeliefRelation>);
}
```

**Partition.** Group by `run_id`. **The `run_id` is the `EventId` of the record
that opens the run** — one object, two roles: it asserts a run began, and its
identity groups what follows. Every later record in the run carries the
`run_id`.

Which record opens it depends on length, and this is the only place length
matters:

| | Opened by | Closed by | `run_id` |
|---|---|---|---|
| Bracketed run | `RunStart` | `RunEnd` | the `RunStart`'s `EventId` |
| Singleton | — | itself | its own `EventId` |

**A singleton is a `RunEnd`, not a record outside the run model.** A bare
receipt is a complete claim, which is what a `RunEnd` *is*; the bracket is what
a claim needs when it takes more than one record to make, not what makes it a
claim. Everything terminal therefore has one shape: the summary that crosses a
promotion boundary, the signature scope if one exists, and the projected node's
BID all key off `RunEnd` with no singleton special case.

Two interleaved runs on one node by one actor partition cleanly because
`run_id`s are distinct `EventId`s.

`open`/`close` on the annotation channel (Issue 110) **are** `RunStart`/`RunEnd`.
The channel's `session` is the run's identity component; no second scheme.

**Derive.** Fold the partition against the lifecycle the `RunStart` cites. Two
distinct mechanisms, and conflating them is an error:

| | Bracketed operation | Derived condition |
|---|---|---|
| Example | executing a review procedure | a sign-off going `stale` |
| Driven by | actor records arriving | the anchor's `tape_hash` changing |
| Needs `run_id` / lifecycle | yes | no |
| Evaluated | at fold | at look time, lazily (`content_versioning.md` §4.4) |

Staleness does **not** route through the bracket. It is a comparison between
the anchor's recorded hash and its current one, evaluated when someone looks.

> **`derive` must be a pure function of (record set, corpus version)**
> (Issue 17 §Purity). Anchor resolution and staleness both qualify — each is a
> function of corpus state at a version. **A timeout does not**: two readers
> folding an identical record set would derive different states, which breaks
> the set-union merge this store rests on. A timeout is not a weak transition,
> it is a non-deterministic one. "Default if unanswered" is therefore *payload*
> — a standing instruction to whoever looks — and the transition happens when a
> human asserts it. **Time is a sort key for attention, never an input to
> state**; stuck runs surface by age, they do not change state by age.
>
> Assert this: fold the same record set twice with the wall clock advanced
> between runs and require identical output.

**Project.** One run → one `BeliefNode` (BID derived from the `run_id`,
`identity_derivation.md` §6.1) plus owned edges into the anchor
queryset. The node carries the derived state in its payload. Edges from the run
node to concerned nodes are **self-owned** (`owned_by = "source"`); an
annotation *about a relationship* between two other nodes emits that edge with
`owned_by = <run node bref>` — the third-party `{maps_to}` mode
(`overlay_model.md` §2). Both together are the halo, and this is the only
mechanism by which a record reaches the graph. **Note `graph_for_owner` returns
only the third-party set**; step 5's halo query must union it with the run
node's outgoing adjacency or it misses every node-level annotation.

**A run emits all three edge kinds, and the fold assigns each.** Direction varies
by what the edge asserts: the run is the **source** of the edges reaching into
the nodes it concerns, and the **sink** of everything it derives from
(`living_corpus.md` §5, `dag_model.md` §2).

| Edge | Kind | Rationale |
|---|---|---|
| actor → run | **Pragmatic** | the actor performed it; the actor node is derived from its email address and created here (step 4) (`living_corpus.md` §5) |
| run → the nodes its anchor selected | **Epistemic** | the anchor is a `QuerySpec`; the claim draws from the set it returned, and reaches into each member — self-owned, `WEIGHT_OWNED_BY = "source"` (`overlay_model.md` §2). **Not Pragmatic** — an annotation over a 200-node scope must not manufacture 200 coverage assertions, and provenance must stay out of the coverage subgraph |
| the procedure template it follows → run | **Pragmatic** | the run is executing that template — the conduit the `RunStart` cites and actualizes (`living_corpus.md` §5) |
| nested run → the run enclosing it | **Section** | containment — the contained node is the source |
| cited record / prior annotation → run | **Epistemic** by default | provenance |

The first row is the one most easily got wrong, and getting it wrong is
expensive: Epistemic and Pragmatic have separate closure hashes
(`content_versioning.md` §5.3), so mis-kinding the anchor edges would put every
annotation's scope into the coverage closure and make "which conduits are
actualized" unanswerable.

**`caused_by` projects to an edge whose `WeightKind` is a record-kind
assertion.** The relation is one field on the `Envelope`; what it *means*
structurally depends on the kind of the citing record, and that translation is
part of the kind's definition, bundled into the fold alongside its transition
table:

For a citation the citing record is the **sink** — it cannot stand without what
it cites. Containment is the exception: a nested run is the **source** of its
`enclosing_run` edge, per the Section convention.

| Citing record | Cites | Projected kind | Reading |
|---|---|---|---|
| nested `RunStart` (`enclosing_run`) | the enclosing run | **Section** | the nested run is the source; containment, not citation |
| reply / close record | the record it answers | **Epistemic** | draws from |
| receipt | — | none | no citation |

A kind that declares no translation defaults to Epistemic. The run hierarchy is
therefore a Section subgraph over run nodes, each owned by its own `RunStart`;
nothing owns the hierarchy as a whole.

> **A blocking dependency is not an edge.** An `ask` gating a redline's
> `packaged` transition emits **no** relation for the gating. The ask is a
> sub-run of the redline's procedure, so its structural link is already the
> Section edge its `enclosing_run` field projects; the gate itself is a fold predicate over
> the cited run's derived state (§Cross-run guards). Note also that "redline" is
> a record kind — its proposed text is payload — so there is no redline node for
> an edge to target. The only nodes the projection creates are run nodes.

### Cross-run guards are predicates over projected state

The case: a redline may enter `packaged` only when the ask it cites has reached
`answered`. The ask is not a child of the redline's run — it is a separate run
the redline cites via `caused_by` — so an exit predicate ranging over the run's
own children cannot express the condition.

The projection rule above dissolves this rather than adding a mechanism.
**The fold derives runs in `caused_by`-topological order** — it must, since
citing a record requires having it, so the DAG is acyclic and the order exists.
By the time the redline's run is derived, the ask's run has already been
projected and its state (`answered` or not) sits in its node's payload. A
cross-run guard is then **a predicate over a cited run's projected state**,
evaluated during `derive` with no lookahead and no second grammar.

**No new syntax is needed on either side.** Issue 17 §Cross-run guards shows the
guard is an ordinary `over:` query with a payload predicate on projected run
state: this issue's §Project puts derived state in the run node's payload, and
`resolve_property_path` (`src/query/spec.rs:580`) already walks a serialized
`BeliefNode`, so `payload.state == "answered"` round-trips through the existing
grammar. **This issue's evaluation half is unchanged**; the syntax half
dissolves. Confirm the exact spelling of a traversal composed with a payload
predicate against `query_model.md` §9.5 when implementing.

### Promotion reads the fold: consistency is a predicate, and the summary is a squash

This issue stops short of the promotion layer, but promotion consumes two things
only the fold can produce. Naming them here keeps them from being reinvented
upstream.

**Consistency is a predicate over a run's own projected state.** A push rule is
a predicate plus a target endpoint (`collector_model.md` §4). §Cross-run guards
establishes the predicate class — a condition over a *cited* run's projected
state. The same class, pointed at the run itself, answers "did this run take an
illegal transition": `derive` already knows, because §Failure modes requires it
to record the illegal transition rather than refuse it.

That closes an otherwise open gap. An illegal transition is a diagnostic and
never a rejected write — correct, since the writer could not see the merge that
made it illegal. But nothing currently stops an inconsistent run from satisfying
a push predicate and promoting anyway. **The teeth belong at promotion, not at
admission**: the record still lands locally, it simply does not cross. The
append-only invariant and set-union merge are untouched, because what is gated
is *movement*, not *storage*.

Two scopes, different costs, and they must not be conflated:

| Predicate | Cost | When |
|---|---|---|
| Run-internal — did the lifecycle permit every transition | free; a `derive` byproduct | at fold |
| Anchor-live — is the anchor's `tape_hash` still current | a query per record | at look time, lazily |

A push predicate that reaches for the second turns promotion into a corpus-wide
re-evaluation. Phase 1 should offer the first.

**The summary is a squash, and the fold defines what it keeps.** §Failure modes
already states that only the `RunEnd` summary crosses a promotion boundary while
constituents stay local — so promotion *is* a squash; it has simply never been
named as one, and the function from chain to summary is unspecified.

The contract follows from a rule the vocabulary work already settled: **fields
derivable from anchor or target are computed, never authored.** Applied here, a
summary must not restate what its constituent chain determines. A run that
drafted a claim, found it wrong, and corrected it promotes **the corrected
claim** — not the correction history. The log keeps the journey; the summary
states what is. The `caused_by` chain gives the squash its boundary for free,
and the summary cites the constituents it replaced.

This is the same constraint the documentation rule states for prose, which is
why it is worth stating mechanically: a promoted record carrying its own
revision narrative is the record-shaped form of a superseded claim sitting next
to a current one.

> **Not owned here.** The push predicate's *syntax* and the endpoint
> configuration are `collector_model.md` §4's, and that document is unratified.
> The re-cut of Issues 74/106/107 owns the *rendered* change package — "the unit
> is a set of redlines per target document" — which is where a squash becomes
> visible to a reader. This issue owns only the two inputs: the consistency
> predicate as a `derive` output, and the constituent set a summary is squashed
> from. **If the squash function grows beyond that, calve it off** rather than
> letting this issue absorb the promotion layer it deliberately stops short of.

**If a record is ever signed, `RunEnd` is what carries it.** No signature field
exists and none should be added before the credential format settles
(Issue 112). But the *scope* a signature would cover constrains `RunEnd`'s
shape, so it is recorded here rather than discovered later.

`RunEnd` is the right carrier on three counts: it is the moment the claim is
complete, it is the only part of a run that crosses a promotion boundary
(§Failure modes), and one signature per run amortizes over a chain of any
length. It also matches the human act — a reviewer signs off once, not per
keystroke. This is git's signed-commit shape and in-toto's link metadata, both
already cited as prior art (`attestation_fabric.md` §3.2–§3.3).

**The signature must enumerate its members, not name the `run_id`.** "Everything
with this `run_id`" is not a well-defined set in a store that merges by set
union: a late-arriving member or a duplicate `RunEnd` changes the slice after
signing, and §Failure modes requires both to fold rather than be refused. A
signature over an open set is a signature over nothing. It must therefore cover
an explicit list of `(EventId, content_hash)` pairs plus the summary payload —
after which a late arrival is *visibly outside* the signed set, which is the
correct and useful outcome.

Three consequences follow, and each is a decision rather than a detail:

- **Verification must not require the constituents.** Only the summary crosses a
  promotion boundary, so a receiver holding just the `RunEnd` cannot re-hash the
  chain. Signing the *enumeration* rather than the concatenated content keeps
  verification local: the receiver checks the signature over the manifest, and
  checks constituents against it only if it later obtains them. Orphaned members
  (§Failure modes) stay verifiable on arrival.
- **Nested runs compose by signature, not by content.** An enclosing `RunEnd`
  covers a nested run's `RunEnd` — not the nested constituents — so no signer
  ever vouches for content another actor produced.
- **Unsigned runs remain legal.** An unterminated run has no `RunEnd` and
  therefore no signature. A singleton *is* a `RunEnd` and so is signable on the
  same path — its enumeration is empty and its payload is its own summary, with
  nothing restated. Signature presence is a *property a policy may require*,
  never an invariant of the store; requiring it would turn a missing signature
  into a refused write, which §Failure modes forbids.

> **This is also what makes an actor node tamper-evident**, which the frontmatter
> attack (Issue 112 §An actor node's payload) otherwise leaves open. If an
> actor's identity-bearing fields are projected *only* from signed records, a
> document edit claiming someone's address changes no projection — there is
> nothing to forge, because corpus content is not an input. The signature does
> not defend the actor node; it makes the actor node a derived artifact, which
> is stronger.

**Demotion is not the inverse.** Reaping is demotion, not deletion: a lowest
tier is genuinely ephemeral with a bounded generation (Issue 110). Nothing is
retracted from an append-only set-union store, so there is no operation that
undoes a promotion — only a tier that expires and a predicate that stops
re-promoting.

### Runs nest

A `RunStart` may cite an enclosing run via `enclosing_run` — named per
`dag_model.md` §2, which forbids `parent_*` for a field pointing at a sink —
plus a `task` — the step of
the enclosing run's template this run discharges. **An enclosing run folds in a
nested one iff the nested run's `task` maps to a step in the enclosing template**; the combining semantics come
from that step's **exit predicate** (`all` / `any` / `ordered` / `n-of`,
Issue 17), not from a policy field. A child with no `task` attaches for provenance and does not affect
the enclosing run's state. `task` is a step BID (Issue 17: a step is a node, BID
derived via `Bid::codec_namespace`), stable across template revision.

> **Cycles are resolved** (Issue 17 §The factored core): a cycle is an *outcome
> that clears marks*, not a back-edge — `rejected` on a review clears the draft
> steps' marks. Nothing here needs a transition construct, and the state space
> stays bounded by the template however many times a cycle runs. The cross-run
> guard is likewise syntax-free (§Cross-run guards above).

### Failure modes the fold must define

None may be a refused write — the store is append-only with set-union merge.

- **Unterminated run** — `RunStart`, no `RunEnd`. In-progress indefinitely;
  age surfaced by `check_consistency`; no automatic expiry.
- **Orphaned member** — a record citing a `run_id` whose `RunStart` this reader
  does not hold. Folds as a valid singleton; attaches when the enclosing run arrives.
  **Distinct from a genuine singleton**, which cites no `run_id` at all and is
  complete rather than waiting: the first is a fragment whose context may arrive,
  the second is a whole. Conflating them would make every orphan look finished.
  **This is the expected case across a promotion boundary**, where only the
  `RunEnd` summary crosses and the constituents stay local. It is also the
  partial-scope case (a store not mounted) and the incomplete-sync case. One
  implementation for all three.
- **Duplicate `RunEnd`** — two closings; fold takes the first by `observed_at`
  and reports the second.
- **Interleaved runs** — same node, same actor, two `RunStart`s. Legal;
  partition keeps them apart by `run_id`.
- **Illegal transition** — a record the lifecycle forbids. A diagnostic, never a
  rejection; it can only be judged after a merge the writer could not see.
- **Unknown lifecycle** — records stay readable with no derived state
  (`attestation_fabric.md` §6.2).
- **Cyclic nesting cannot occur** — citing an enclosing run requires holding it, and
  `run_id` is the `RunStart`'s own `EventId`. Debug-assert, no recovery logic.

### Fold ordering

Within a run, order by `(observed_at, id)`. `sequence` totally orders one
session's records; between sessions of one actor the records are concurrent and
`observed_at` is the display tiebreak (`collector_model.md` §5.1). There is no
`lamport` field — `caused_by` carries causality explicitly as a DAG, and a
Lamport clock would be a weaker implicit encoding of it.

## Implementation Steps

1. **`RecordSink` + `MemorySink`** (0.5 days)
   - [ ] Trait as above; `MemorySink` over `Vec<Envelope>`
   - [ ] Issue 110's handle routes to it; round-trip test: `emit` → `append` →
         `read_from(cursor)` yields the same `Envelope`

2. **`FsSink` and the annotation manifest** (1.5 days)
   - [ ] `beliefbase/annotations/<store-id>/<actor>-<session>.log`, append-only,
         one file per session; a closed session's file is never reopened
   - [ ] Decide whether `StoreId` is configured-only or may be derived from
         `(corpus, actor, purpose)` — see Open Questions. If derived, a
         self-provisioned store is `ships: false` and outside `default_halo`
         until a human adds it
   - [ ] `annotations/manifest.json` with `precedence`, `ships`, `sessions`,
         `default_halo`
   - [ ] Reader uses `default_halo` when the caller names no store set; decide
         whether a *reported* figure may be computed from a widened halo, and how
         a diverged halo is surfaced to the reader
   - [ ] Reader discovers stores from the manifest and constructs the halo
         ordered by precedence
   - [ ] `ships: false` stores excluded from `export_beliefbase`'s output
   - [ ] Test: two sessions of one actor writing concurrently produce two files
         and zero conflicts (P1 regression test)
   - [ ] Test: copying one store's directory into another and re-reading is
         idempotent (set union)

3. **Fold: partition and derive** (1.5 days)
   - [ ] `RunStart { lifecycle: <version-anchored ref>, enclosing_run: Option<EventId>,
         task: Option<Bid>, .. }`; `run_id` = its own `EventId`
   - [ ] `RunEnd { run_id, result, .. }` — for a singleton, `run_id` is the
         record's own `EventId`, so the field is always populated after partition
   - [ ] `run_id: Option<EventId>` **as authored**; `None` means "this record is
         its own run", resolved during partition rather than carried as a special
         case into `derive` or `project`
   - [ ] Partition by `run_id`, resolving `None` to the record's own `EventId`
   - [ ] Derive against the cited lifecycle; unknown lifecycle → no state
   - [ ] An enclosing run folds in a nested one iff `task` maps to a step of the
         enclosing template
   - [ ] Staleness computed separately, lazily, by anchor hash comparison
   - [ ] `RunState` exposes **whether every transition was legal** — a
         `derive` byproduct, not a second pass. This is the term a push
         predicate names (§Promotion reads the fold)
   - [ ] One test per failure mode above
   - [ ] **Purity**: folding one record set twice with the wall clock advanced
         between runs yields identical state (Issue 17 §Purity)
   - [ ] Test: a run containing an illegal transition still folds, still
         projects, and reports itself inconsistent — all three, since the
         diagnostic must not become a refused write

4. **Project: run → node + halo** (1 day)
   - [ ] One `BeliefNode` per run, BID derived from the `run_id` into the
         `Record` namespace (`identity_derivation.md` §6.1; coordinate the
         namespace split with Issue 108). For a singleton the `run_id` is the
         record's own `EventId` — one rule, no special case
   - [ ] Test: a singleton and a two-record bracketed run project through the
         same code path, differing only in constituent count
   - [ ] **One `BeliefNode` per distinct `ActorId`**, BID derived from the
         actor's email address into an `Actor` namespace — derived, never minted,
         so the same actor is the same node across corpora with no coordination.
         Created on demand when a run is projected. The namespace **also holds
         role nodes** (Issue 112) — actors and roles share the agent/authorization
         boundary and are told apart by `schema`, not by BID. **Two issues add a
         namespace to one array** (this one and 108's `Record`): whichever lands
         first adds both, per `identity_derivation.md` §5.1
   - [ ] **`Actor` joins `content_namespaces()`.** An `ActorId` is URL-shaped (an
         email, a DID), so `BeliefNode::keys` must emit `NodeKey::Path`; the
         `NodeKey::Id` branch slugifies via `to_anchor` and would corrupt a
         case-sensitive DID identifier. Test: an actor keyed by a `did:key:`
         identifier round-trips byte-identical
   - [ ] **Pragmatic** edge `(source: actor, sink: run, WEIGHT_OWNED_BY: actor)`
         — the actor performed the run (`living_corpus.md` §5). This is what makes
         "everything this person signed off" a traversal rather than a payload
         scan
   - [ ] Owned edges into the anchor queryset with `WEIGHT_OWNED_BY` = run node,
         kinded **Epistemic** (the anchor is a query; the run draws from what it
         returned)
   - [ ] **Pragmatic** edge to the procedure template the `RunStart` cites — the
         conduit actualization, and what makes "which declared reviews have
         happened" a coverage query
   - [ ] `enclosing_run` → **Section**; `caused_by` → the kind its record type declares,
         defaulting Epistemic
   - [ ] Test: a run over a 200-node anchor emits **zero** Pragmatic edges to
         those nodes — the mis-kinding guard
   - [ ] Test: `graph_for_owner(run_bref)` returns the run's halo; re-folding the
         same records yields the same BIDs (idempotent)
   - [ ] Test: a 1,000-record run projects to **one** node
   - [ ] Test: a run whose chain drafted, retracted, and re-stated a claim
         projects a node carrying **only the final claim** — the squash
         contract (§Promotion reads the fold). The constituents remain readable
         in the log

5. **Single-actor halo query** (0.5 days)

   A halo query is: given a focused node set and one or more owners, return the
   owned edges and their endpoints.

   - [ ] Resolve the focused document's node set — its Section submap, which is a
         `QuerySpec` and is also a receipt's anchor
   - [ ] Find annotation records whose halo intersects that set
   - [ ] Return the owned edges plus their endpoints as a `BeliefGraph` in a
         `QueryPackage` — `graph_for_owner`'s output shape
   - [ ] **Take both ownership modes.** `graph_for_owner` returns only
         third-party edges; `owner_edges` deliberately excludes `"source"`/
         `"sink"` owners (`src/beliefbase/base.rs:106-108`). A run's *self-owned*
         edges are its ordinary outgoing adjacency, so a single `graph_for_owner`
         call misses every node-level annotation

   **One owner is the simplest useful form**, and it is what W4 needs: one
   `owner_edges` lookup, no merge across authors, no precedence question, no
   conflicting claims about a single node. A workflow whose loop closes inside
   one author's boundary needs nothing more.

   **The multi-owner form is a union.** `graph_for_owner` takes one bref, so
   "every annotation touching this document" is N lookups combined. `owner_edges`
   is keyed by owner, so the extension is mechanically small — the query surface
   expressing it does not exist yet.

   **Where a halo renders** (viewer integration, not this step's build):

   | Surface | Renders | Hook |
   |---|---|---|
   | `metadata.js` | focused node's relations, attributed to the owning annotation | `renderRelationGroup` renders owner attribution (`viaHtml`, `ownerBid`/`ownerTitle`) |
   | `traceability.js` | rows over a queryset; `o` is a role sigil | existing diff decorations |
   | `content.js` | inline marks where the halo intersects rendered prose | `highlightExternalInContent` resolves brefs to elements |
   | `navigation.js` | per-subtree badge or count | node state classes |

   `metadata.js` is the closest fit: it already displays third-party-owned edges
   with attribution to their owner, which is the halo's display form. Annotation
   rendering extends that path rather than adding one.

   - [ ] **Client-side viability is the open constraint.** `BeliefBaseWasm` is
         read-only and the record store may be IndexedDB or absent. A halo query
         that runs in the browser against shards means annotation reads need no
         server; one that cannot means the static-site deployment cannot show
         annotations at all. Measure before assuming either
         (`identity/generational_archive.md` §7)

## Testing Requirements

- Append-only holds: no code path mutates or deletes a record or a closed
  session file
- Two sessions of one actor: disjoint files, disjoint `EventId`s, union merge
- Fold on a shuffled record set equals fold on the ordered set
- A run with its `RunStart` in an unmounted store folds as a singleton and
  attaches when mounted
- Interleaved runs on one node fold to two states
- An inconsistent run folds, projects, and reports its inconsistency
- Staleness fires on anchor change and does not route through `run_id`
- Re-fold idempotence: same records → same BIDs → same halo
- Manifest round-trip: write, read, precedence order preserved; `ships: false`
  store absent from export

## Success Criteria

- [ ] `RecordSink` has two impls and Issue 110's handle routes to either
- [ ] Records persist as one append-only file per session beside the shards,
      discovered via `annotations/manifest.json`
- [ ] A multi-record run folds to one state and projects to **one** node
- [ ] Two concurrent runs on one node do not interfere
- [ ] Every failure mode has a defined, tested behaviour; none is a refused write
- [ ] `RunState` reports transition legality, so a push predicate can require
      consistency without a second pass over the records
- [ ] Staleness works and does not use the bracket mechanism
- [ ] `graph_for_owner` on a run node returns its halo; the single-actor halo
      query returns the W4 receipt view
- [ ] A lifecycle is definable as data, not code

## Risks

- **This is really transactions**, and transactional semantics over a CRDT is
  hard → **Mitigation**: scope is *grouping and lifecycle*, not atomicity or
  isolation. A half-recorded run is a legitimate state, not a rollback case. No
  ACID vocabulary.
- **`run_id` becomes mandatory** and every receipt carries bracket machinery →
  **Mitigation**: `Option<EventId>`, default `None`; the singleton path stays the
  simple one and is the W4 case.
- **The lowest-precedence store accumulates** — a scratch store nobody reaps
  holds every session ever opened → **Mitigation**: reaping is *demotion, not
  deletion* (append-only is load-bearing), and a store's session list is
  enumerable so age is a query. The reap rule itself is `collector_model.md`
  §4's; Phase 1 ships without one and measures.
- **Two state mechanisms confuse implementers** → **Mitigation**: the table in
  §Derive is the contract; bracketed operations and derived conditions are named
  differently throughout.

## Open Questions

- **Actor aliasing is deferred, and `url_aliases` is not the answer.** One actor
  holding several email addresses should resolve to one node. The existing alias
  mechanism (`url_aliases` / `alias-template`, `codecs/network_authoring.md` §8)
  composes additively over one node and is the obvious reuse — but it is
  **document frontmatter**, so aliasing an actor that way lets anyone with commit
  access merge identities by editing a file, with no signature to break
  (`ISSUE_112_CREDENTIALS_AND_PROMOTION.md` §An actor node's payload). Phase 1
  may treat one address as one actor; if a pilot actor turns out to have two,
  bind them with an attested record rather than a frontmatter field. Distributed
  identity tokens (DIDs) are the later substitution the derivation rule already
  accommodates.
- **Should a `StoreId` be derivable rather than configured?** A store exists
  today only if `annotations/manifest.json` declares it, which is user-dynamic
  configuration. So a producer needing a scratch store — a parse agent, an MCP
  session, a test — cannot create one without a provisioning step, and two runs
  of the same agent agree on a store only because a human wrote the same id
  twice. Deriving `StoreId` from `(corpus, actor, purpose)` is the rule this
  codebase already applies to actor nodes and record nodes
  (`identity_derivation.md` §3): an agent self-provisions with no configuration,
  and two runs converge on one store with no coordination.
  - This changes nothing about **write authority**, which stays topological —
    one `ActorId`, one read-write log, clear-and-rewrite by sole ownership
    (`collector_model.md` §3.1). A derived id is a *name*, not a grant.
  - **The guard is `ships` and `default_halo`.** A self-provisioned store must
    default to `ships: false` and stay out of `default_halo`, or an agent
    silently inserts itself into the corpus's declared sources of truth. Joining
    the halo stays a human act, which is what makes the declaration reviewable.
  - Open sub-question: is `purpose` a free string, or an enumeration? A free
    string converges only if two producers spell it identically, which is the
    failure the derivation was meant to remove.
- **Does "singleton is a `RunEnd`" collapse *member* into *nested run of length
  one*?** A single record citing `enclosing_run` + `task` is now describable both
  ways, and the two fold differently — members by lifecycle transition, nested
  runs by the step's exit predicate (§Runs nest). Either the two are genuinely
  one thing and the fold has one path, or the distinction is load-bearing and
  must be stated in terms other than length. Settle before implementing
  `partition`; it is the one place this unification could over-reach.
- **Is `RunStart` also an admission gate?** It could mint a token later records
  must present — a write-cost mechanism for a store exposed to unsolicited
  writes. No tier in the current topology is so exposed, and the constraint it
  would have to satisfy is sharp: a token may gate whether a write happens, but
  must never enter the fold, or `derive` stops being pure. Shape and placement
  recorded in `collector_model.md` §3.1a; the generalization is Issue 16's
  parked capability model.
- **What binds a signing key to an actor?** A `RunEnd` signature proves one key
  signed a slice; it does not say whose key it is. That binding is a credential
  (Issue 112) — a peer-attested record peers can revoke — which means key
  material must be reachable from the graph, since `derive` is pure and cannot
  make a network call to check a key or a revocation.
- **Does a store need its own `ActorId`**, or is `(actor, session)` enumeration
  enough? (`collector_model.md` §9; `annotation_channel.md` §8)
- **Can a run span anchors?** A review over five sections is plausibly one run.
  Recommend yes — the run's anchor is a `QuerySpec` and can select five nodes —
  but confirm against a real review pass (planning Issue 33).
- **Does `task` belong on records generally**, not only `RunStart`? Any record
  anchored to a template plausibly says which step it discharges. Recommend
  hoisting once a second consumer appears.
- **Where do lifecycle definitions live** — source (normative,
  version-controlled) or a store (travels with records)? They are override
  data, not set-union data, so their precedence differs from records'.
- **Which cross-kind closures are worth caching versus walking.** "Did anything
  upstream of me change, along any edge?" is answerable two ways and neither is
  blocked: compose the cached per-kind hashes (free, but cannot express a chain
  that *alternates* kinds), or walk the union with a `Fold{Union}` tape (one
  O(V+E) traversal; the tape is the visited set, so cycles terminate). Prefer
  composition; the walk is the fallback for alternating chains
  (`content_versioning.md` §5.6, and §4.4 for the uncapped-traversal variant both
  need). A measurement question, not a design one.
- **Staleness propagation along Epistemic edges — deferred until the W4 pilot
  supplies data.** A node's content hash cannot see its incoming edges: a
  requirement that gains a `{maps_to}` claim has unchanged content and changed
  meaning. Which edge kinds propagate, how far, and in which direction is a
  policy question, and an Epistemic closure over a densely-linked corpus may fire
  constantly — `content_versioning.md` §8's always-fires failure in a new form.
  Computing the closure is the only way to get fire-rate data, so compute it and
  treat closure-scoped anchoring as provisional.
- **What does a surface show when two annotations disagree about one node?**
  Both edges exist — nothing merges them — so this is a rendering and fold
  question, not a conflict-resolution one. Unspecified.
- **An arbitrary-`QuerySpec` anchor is depth-capped; a cached closure is not.**
  §4.4 of `content_versioning.md` exempts closure evaluation from
  `MAX_TRAVERSAL`, but an anchor written as a general query runs through the
  ordinary evaluator and is bounded at 10 hops — so "everything within 12 hops
  along this path" silently becomes 10 and the annotation under-reports. Either
  extend the exemption to anchor-role evaluation (costing the decidability
  argument §4.4 sets aside) or **reject an over-deep anchor at emit time**, so a
  claim that cannot be evaluated faithfully is never made. Silent truncation is
  the option to rule out. Recommend the second for Phase 1: it is cheap and
  fails loudly.
- **Open-time reconciliation** — a new session against a store already holding
  this actor's records over this anchor. The channel owns the rule
  (`annotation_channel.md` §8); Phase 1 supersedes by `(actor, anchor)`.
- **Trail UX is a follow-on: naming and moving between cuts.** A reader resuming
  work wants a cursor and the trail to it. The mechanisms exist — a session's log
  ordered by `EventId`, a **cut** across watermarks (`collector_model.md` §5.1,
  §5.2), the regenerated scope, and promotion as a squash (§Promotion reads the
  fold) — so nothing needs building here. What a follow-on must settle is the
  *interaction* layer over a configured halo: what names a cut, who may name one,
  whether an unnamed cut survives a session, and how a reader moves records
  between stores. Note the asymmetry that makes this cheap: an append-only log
  discards nothing, so replaying to a cut reconstructs any prior state exactly —
  the question is which cuts are worth naming, not what to keep
  (`identity/generational_archive.md` §9.2).
- **Orphaned anchors are a follow-on, not this issue.** When a record's target is
  deleted — or its BID migrates (Issue 36) — the anchor is intact and resolves to
  nothing. That is a distinct condition from staleness: *stale* means re-read,
  *orphaned* means the subject is gone, and a surface that renders them alike
  will mislead. It arises only for **stores that accept promotion**, since a
  regenerated-scope record never outlives the parse that wrote it, so Phase 1's
  local store does not meet it. What a follow-on must settle: whether the fold
  marks an orphaned run, whether an orphan is promotable at all, and what a
  reader sees. Design context in `content_versioning.md` §8 and
  `content_identity.md` §8, which raise the same case from two sides.

## References

- `docs/design/annotation/annotation_channel.md` §6 — the halo of stores, and
  the promotion layer this issue deliberately stops short of
- `docs/design/annotation/overlay_model.md` §2, §5, §6 — owned-edge halos,
  precedence, the single-actor halo query
- `docs/design/annotation/living_corpus.md` §4, §5 — stateful records; which
  edge kinds a projection emits
- `docs/design/annotation/collector_model.md` §4, §5.1, §9 — promotion (the next
  layer); clocks; the collector-identity question
- `docs/design/core/beliefbase_architecture.md` §4.3 — `Envelope`, `EventId`
- `docs/design/identity/identity_derivation.md` §6.1 — run node BID from
  `EventId`
- `src/shard/export.rs`, `src/shard/manifest.rs` — the shard layout this
  co-locates with
- `src/beliefbase/base.rs:519` — `graph_for_owner`, the halo primitive
- Issue 110 (the channel), Issue 103 (hash family and source ranges), Issue 104
  (vocabulary), Issue 108 (`Record` namespace), Issue 17 (step grammar),
  planning Issue 33 (the real review pass)
