---
version = "0.1"
title = "Collector Model — Store Topology, Admission, and Automatic Promotion"
authors = ["Andrew Lyjak"]
status = "Target architecture — unratified; Phase 1 is local-only"
dependencies = [
  "annotation/living_corpus.md",
  "annotation/overlay_model.md",
  "core/beliefbase_architecture.md",
]
---

# Collector Model

> [!NOTE]
> **Target architecture, and unratified.** The topology and admission model
> below are a recommendation that no implemented system exercises. Phase 1 is a
> single local collector with no network, no signatures, and the simplest
> admission rule that works — everything else is an extension point, deliberately
> left open. Treat the multi-collector material as a design sketch until a
> second store exists.

## 1. Purpose

`overlay_model.md` specifies how annotation records *compose* into a readable
graph. It does not say where records live, who may write them, how they move
between stores, or what may be discarded. This document answers those.

One question drives it: **an append-only store with union merge cannot reject
anything, yet some records must be replaced wholesale and some must not
propagate.** Both are true, and the resolution is topological rather than
policy-based.

## 2. The store/boundary distinction

The apparent contradiction dissolves by separating two objects:

- A **store** holds records. It never rejects a record it holds; merging two
  stores is union. Convergence lives here.
- A **boundary** is where a record moves from one store to another. It may
  decline. Integrity lives here.

Declining is not deletion. A push refused at a boundary leaves the record
authoritative in its origin store; what failed was a *copy*.

This reconciles two rules that read as contradictory:

- Issue 105: *"none may be a refused write — the store is append-only with
  set-union merge, so rejection is not available."* **True of stores.**
- A validated tier can only claim "everything here passed the checks" if there
  is exactly one way in. **True of boundaries.**

The general argument, with its antecedents (Clark-Wilson, Biba, relay
topologies), is in `docs/essays/trust_boundaries_and_admission.md`. This document
is the noet instantiation.

## 3. Topology

Three tiers. Each is an ordinary record store; they differ in who writes and what
admits.

| Tier | Writers | Admission | Promotes by |
|---|---|---|---|
| **Personal log** | exactly one `ActorId` | none needed — sole writer | configured push rules |
| **Subsystem collector** | pushes from configured sources | accepts from a configured endpoint set | automatic, on record-state predicates |
| **System collector** | pushes from configured subsystem collectors | same, one tier up | (higher tiers, later) |

### 3.1 Single-writer personal logs resolve retention

**Each `ActorId` owns one read-write log and has no write authority over any
other.** This is the topological fact from which retention follows.

The parse agent may **clear and rewrite its own log** each run. That is not an
exception to append-only semantics — it is the ordinary consequence of sole
ownership. Nobody else's records are in that log, so nobody else's guarantees
are violated.

This dissolves the problem that motivated the model. Parse diagnostics do not
supersede the previous batch record-by-record; the prior batch is simply gone,
because the observation stopped being true. Under a shared grow-only store that
is a violation. Under single-writer ownership it is unremarkable.

> **The G-Set property is a property of collectors, not of every store.** A
> collector receives from many actors and merges by union, so it is grow-only.
> A personal log has one writer with replace authority, so it is not — and does
> not need to be, because it never merges.

### 3.1a Where a write-cost mechanism would go, if one is ever needed

A `RunStart` could mint a **token** that later records in the run must present,
making the bracket an admission gate as well as a grouping construct. Nothing
needs this today, and the tier table says why: **the two tiers that accept
unsolicited writes are the two that do not exist yet as network services.**

| Tier | Accepts from | Exposure |
|---|---|---|
| Personal log | one `ActorId`, locally | none — a local file, sole writer |
| Subsystem collector | a *configured* endpoint set | bounded by configuration, not open |
| A future public store | anyone reachable | this is where a token earns its cost |

So the mechanism is correctly deferred, but the *shape* it would take is
constrained by decisions already made, and recording that is cheaper than
rediscovering it:

- **A token gates admission, never storage semantics.** The store is append-only
  with set-union merge. A token may make a write *not happen*; it must never make
  a stored record conditionally valid, or `derive` stops being a pure function of
  the record set.
- **It cannot be a fold input.** Two readers folding the same records must agree,
  and one of them may never have seen the token. A token is transport-layer
  state that expires; a record is durable. Putting a token in a payload would
  make replay a verification failure.
- **It is not the signature.** A `RunEnd` signature proves *who* authored a
  completed claim, retrospectively and durably (Issue 105 §If a record is ever
  signed). A `RunStart` token would prove *that a writer was admitted*,
  prospectively and ephemerally. Different questions, different lifetimes — a
  store may want both, and neither substitutes for the other.

**The real generalization is capability, not rate limiting.** Issue 16 parked an
authorization model — actions, scopes, and constraints including a rate limit —
with the note that the annotation model does not answer who may read or write
which scope, and assigned the question to the async-overlay successor of Issue
110. A `RunStart` token is one instantiation of that capability, scoped to a run.
It should be designed there rather than bolted onto the bracket, or the bracket
acquires an authorization role it cannot discharge for single-record writes,
which have no `RunStart` at all.

> **The singleton case is the shape test, and it passes for a reason worth
> keeping.** A singleton *is* a `RunEnd` (Issue 105 §Partition), so it has no
> `RunStart` to mint a token from — but it also needs none. A token exists to
> amortize one admission check over a stream of follow-on writes; a singleton is
> one write, and gating it is a per-write check with nothing to amortize. The
> token is therefore a **stream** mechanism, not a record mechanism, which is the
> honest scope for it. An admission model that cannot state that distinction
> would be gating the wrong thing.

### 3.2 Collectors are dumb about content

A collector **stores and serves records; it does not interpret them.** It does
not fold, does not derive state, does not rewrite payloads. Every reader folds
locally, which they already do.

Two things this permits without breaking the property:

- **Caches.** A collector may maintain derived indices over records it holds —
  "records anchored to BID *X*", "records citing `EventId` *Y*" — invalidated on
  arrival. A cache requires no interpretation of what a record *means*, so
  dumb-about-content survives. These are the same two filters the local fold
  needs, which is a good sign they are the right primitives.
- **Opinions about admission.** A collector may decline a push. That is a
  decision about *whether to store*, not about what the record means.

The distinction to hold: **dumb about content, potentially opinionated about
admission.** Both halves are load-bearing; a collector that interprets payloads
becomes a server with semantics, and the local/remote symmetry breaks.

### 3.3 The configured endpoint set

A collector accepts pushes only from sources it is **configured** to know. Three
consequences:

- **No N².** Collectors do not discover each other, and do not talk to
  collectors they were not told about. Fan-out is bounded by configuration.
- **The topology is a DAG.** A cycle would deadlock admission where predicates
  depend on other tiers. Acyclicity is a requirement, not a convention.
- **Admission stays locally computable.** The configured set is part of the
  receiving store's trusted state, so consulting it is not an external
  dependency in the sense that would defeat the purpose.

## 4. Should promotion be automatic and derived?

> **Strong opinion, loosely held — pressure-test before building on it.** This
> section argues that promotion should be derived rather than commanded. The
> argument is good enough to design against and not good enough to treat as
> settled: it has not met a real workflow, it is in tension with
> `annotation_channel.md` §6 (promotion as "an explicit act, reviewed in the
> UX"), and one of its two supporting arguments weakens once Issue 112's
> credentials exist — see below. §4.1's marker-record construction is what would
> reconcile the tension — a review gesture writes a marker, and the predicate
> fires on it — but
> whether that is a faithful account of reviewing or a re-description of a
> command is exactly what a pilot has to say. **The W4 and W1 pilots are the
> test** (planning Issue 29).

**The proposal: a subsystem collector pushes on record-state predicates
configured within it.** No actor requests a push; no actor is authorized to
request one.

Two arguments for it.

**It avoids an authorization question** — though this argument is weaker than it
first appears, and Issue 112 is why. The original form ran: actor-requested push
would require each collector to hold a list of who may push, which is the
top-down role assignment `collaboration_overlay.md` §4a rejects. But §4a rejects
*administrator-assigned* roles — a config panel, a privileged account — not
authorization as such. **A credential-based push rule is neither.** The
credential is peer-attested, it lives in the graph as an edge, and the rule reads
it the same way any other predicate reads state; no collector holds a list and no
administrator maintains one.

So with Issue 112 implemented, "who may push" has an answer that costs nothing
this design already objects to, and this argument reduces to a preference for
fewer moving parts rather than an objection to the alternative.

**It places promotion on the same side as everything else derived.** Derived
state is a fold, never stored. Promotion would likewise be **derived from record
state**, not commanded. You annotate; propagation is a consequence.

Where it is most likely to break: a human who wants to promote *this* set now,
having just decided it is ready, and for whom "write a marker record and wait for
a predicate" is a worse description of what they did than "I published it." If
that turns out to be the common case rather than the exception, the marker
construction is a workaround and the honest model has a command in it.

A push rule is a **predicate plus a target endpoint**. Note the shape: that is
also `EventSubscription { id, query, tx }` from Issue 15 — a filter plus a
delivery channel. Whether these are one mechanism is flagged in Issue 104's
primitive census; do not assume it here.

### 4.1 Explicit intent must be representable

"I am not finished; do not publish this yet" is a real requirement, and automatic
promotion appears to remove the ability to express it.

It does not — *if* the construction holds: make the predicate require a **marker
record**. Intent becomes state rather than a command, which keeps it inside the
model instead of adding a control channel beside it.

The construction is general enough to express holding back, releasing, and
releasing a subset. What it has not been tested against is whether authors
experience it as expressing intent or as operating a mechanism, which is the
§4 caveat in its sharpest form.

### 4.2 What crosses is per-relation configuration

When a subsystem collector promotes a folded summary, the receiving tier faces a
choice, and it is a genuine trade:

| Option | Receiving tier can re-verify | Cost |
|---|---|---|
| Summary only | No — trusts the sender's fold | Cheapest; transitive trust |
| Summary + constituent records | Yes, locally | Volume; weakens the privacy property percolation was designed for |
| Signed summary | Partially — authenticity, not derivation | Pulls signatures into the local-first path |

**This is configured per collector relation, not globally.** A relation inside
one trust domain can send summaries only; a relation crossing domains can require
evidence or signatures. Phase 1 implements the first row.

Recording this as configuration rather than as a decision is deliberate: the
right answer differs per deployment, and the interface should admit all three
before any of them is needed.

> **Consequence for Issue 105.** Under summary-only promotion, a summary in the
> destination cites constituents that are not present. Issue 105's "orphaned
> member" failure mode is therefore the **expected case** across a promotion
> boundary, not an error, and the fold must not treat it as one.

## 5. Clocks, causality, and citation closure

### 5.1 The clocks already exist; name them honestly

It is tempting to ask whether records need a logical clock added. They do not,
because the mechanisms are already present under other names:

| Mechanism | What it is |
|---|---|
| `EventId = (actor, session, sequence)` | a **monotonic per-session counter** — total order within one writing process's log |
| A collector's arrival order | the same, for the collector as an actor |
| A watermark per pushing source | a **vector clock** — one entry per source (`federated_belief_network.md` §3.3) |

**The counter's scope is a session, not an actor.** One actor may write from
several processes at once — a viewer, an MCP agent, and a CLI — and each mints
its own `session` and counts from zero within it
(`core/beliefbase_architecture.md` §4.3). So `sequence` totally orders one
process's log and says nothing between two processes of the same actor.

**And `sequence` is not a Lamport clock.** A Lamport clock's defining rule is
`clock = max(local, received) + 1` on receipt, which is what makes
`a → b ⇒ L(a) < L(b)` hold *across* actors. Our counters never advance because
another actor's record was seen. They order within a session and say nothing
between sessions.

That is correct rather than deficient, and the session scope sharpens the
argument rather than weakening it. Records merge by set union, so two
independent annotations on one node are genuinely **concurrent** — imposing a
total order would invent an ordering the world did not have. Two sessions of one
actor are the same case: a person annotating in a browser tab while an agent
writes on their behalf has produced concurrent records, and pretending otherwise
would require a coordination point the design deliberately does not have.

**So the honest framing is that pushes and folds synchronize clocks between
stores.** A push advances the destination's watermark for one source. A fold
reads a **cut** across those watermarks. That is a well-defined object with known
properties, and it answers a question left open elsewhere: "which records does
this fold see?" is *the cut*, not "everything, hopefully."

#### Worked example: a comment thread

Threading needs no schema addition. A reply is a record whose `caused_by` cites
the parent record's `EventId`; `caused_by` is a DAG, and a thread is its
tree-shaped projection. Issue 104 already records this.

Note what the reply cites: **the parent's `EventId`, not a collector-assigned
identifier.** `(actor, session, sequence)` is unique without coordination, so a
reply written offline — or in a browser, against a record not yet promoted
anywhere — has a stable target and converges when both land. Collector-assigned
identity would make the same logical reply a different object in each collector.

The browser case is why `session` exists rather than a persisted device or
writer identity: a tab has no durable counter to resume and no way to coordinate
with the desktop process writing as the same actor, yet it must mint ids that
will never collide with that process's.

**Sibling order is not given by the collector's arrival clock**, and this is the
part worth being explicit about, because reaching for it is the natural move.
Arrival order is:

- **per-collector** — two collectors that received the same replies in different
  orders would render different threads, with nothing to say which is right;
- **a fact about connectivity, not authorship** — a reply written offline at 9am
  and synced at 5pm arrives after one written at 4pm;
- **not reproducible** — re-importing the same records into a fresh collector can
  change it.

So arrival order serves *synchronization* (resumable pull, watermarks, what's
new) and must not serve *display*. **Sibling order is `observed_at`** — already
in the `Envelope`, already marked display-only, which is exactly this use. It is
wrong under clock skew, but wrong *identically everywhere*, which for a thread
beats being locally right and globally inconsistent.

> **Extension point, not settled here: observed-sibling ordering.** `caused_by`
> loses one real fact. An author who sees three replies and adds a fourth
> *observed* those three — their reply is causally after all of them, not
> concurrent with them, and `observed_at` recovers that only by accident of clock
> agreement.
>
> A reply may therefore optionally carry, **in its protocol payload**, a
> reference to the sibling it follows. Prefer that citation form over an integer
> position: it converges without a numeric tie-break (two authors both claiming
> position 4 is the classic sequence-CRDT failure), it reuses the vocabulary
> already in use, and it degrades cleanly to "unordered peer" when absent.
>
> Three constraints. It is **optional** — a reply written against a stale view
> carries nothing and sorts by `observed_at`, so threads render either way and
> better when the author had context. It is **payload, never `EventId`** — that
> pair is load-bearing for uniqueness and filename derivation, and an offline
> reply must be able to have an identity before it knows its position. And it
> makes that push **contextual rather than unconditional**, since the author must
> have read the parent's current siblings; §5.3's closure query already
> establishes that a push may involve a round trip, but the cost should be
> acknowledged rather than acquired quietly.
>
> This is Issue 104's to settle — the primitive census is where competing demands
> on the record are reconciled.

It also makes back-propagation precise. "Which upstream records cite mine?" is
answerable **at a stated cut**, so an agent knows exactly which state it checked
before clearing its log. Without a cut, that check races the next push.

### 5.2 Causality is `caused_by`, not a clock

`caused_by` is a real happens-before edge — a record naming what it reasons
*from*. It is the one relation where cross-actor ordering carries semantic
content, and it is **explicit in the data** rather than inferred from a counter.

This is the argument for not adding a `lamport` field, and it is stronger than
"it may not be needed" (`beliefbase_architecture.md` §4.3): a Lamport clock would
be a *weaker, implicit* encoding of a causal DAG the records already carry
explicitly. Clocks are for synchronization; `caused_by` is for causality. Keeping
them separate is what lets the clocks stay simple.

### 5.3 Citation closure across a boundary

A promoted record's `caused_by` may cite records the destination does not have.
Two relations behave differently here and must not be conflated:

| Relation | Points at | May dangle? |
|---|---|---|
| `caused_by` | another annotation **record** | **Yes** — the ordinary partial-scope case (`federated_belief_network.md` §1.2); Issue 105 requires an orphaned member stay readable |
| `RecordSource` citation | an external span, **addressed** | Not applicable — it is a payload field of the citing annotation, not a separate record |

**Closure is a per-relation policy, not a global invariant.** The same axis as
§4.2:

- **Within a trust domain** — *closed*: push the transitive `caused_by` set so
  the destination can re-verify locally.
- **Across a percolation boundary** — *open*: the summary crosses, working
  records stay local, citations dangle by design. This is what percolation
  *means*; requiring closure here would defeat it.

#### The algorithm

1. **Ask** the destination which of the transitive set it already holds — a
   set-membership test on `EventId`s.
2. **Push the difference incrementally.** Union merge makes retries idempotent,
   so a receipt is simply permission to stop tracking an entry. **No batch
   atomicity is required**: a partially-landed closure is a state the fold must
   already handle, since both `federated_belief_network.md` §1.2 and Issue 105
   treat a dangling citation as ordinary rather than corrupt.
3. **Bound the recursion.** Stop at the first record the destination already has
   — self-limiting, since the frontier shrinks as the destination fills — with a
   count cap as a backstop for a first push into an empty collector.
4. **Beyond the bound, summarize.** Emit **one** annotation citing the remaining
   chain as `RecordSource` references (Issue 108) rather than pushing the
   records. `RecordRange::Sequence { from, to }` fits a contiguous run of
   `EventId`s from one actor.

#### Why the summary boundary is not a special case

Issue 108 Decision 6 holds that a record node may exist **only** as the sink of a
citation from an annotation — never free-standing, never separately ingested. A
closure-boundary citation is exactly that: the record node comes into existence
when the summary annotation is folded.

So the citation and the thing it cites arrive as **one payload**, and no ordering
constraint arises. The "cited record must precede its citation" concern applies
to `caused_by` pointing at independent records — where dangling is already
accepted — and not here.

This also makes the boundary *legible*. A dangling `caused_by` says nothing; a
`RecordSource` citation says **where the chain continues**, and Issue 108's
`verify` (`Valid` / `Missing` / `Changed`) can check it without holding it.

## 6. Admission checks

What a boundary may check, independent of substrate:

| Kind | Question | Needs crypto? |
|---|---|---|
| Structural | Well-formed? Schema, required fields, version | no |
| Invariant | Within declared bounds? | no |
| Consistency | Compatible with the receiving store's state? | no |
| Correlation | Do independent sources agree? | no |
| Authenticity | From who it claims, unmodified? | **yes** |
| Reproducibility | Does re-deriving reproduce the result? | no |

**Only authenticity requires keys.** A boundary between two stores under one
operator's control can perform every other check with no cryptography at all —
which is what makes the local-only Phase 1 genuinely simple rather than a
degenerate case of a security architecture.

Which checks a boundary requires should follow from the record's `record_kind`,
so the policy lives in the registry Issue 104 already owns rather than becoming
a separate configuration surface.

## 7. Phase 1: one local collector

Deliberately minimal:

- **One collector, local filesystem.** No network, no discovery, no signatures.
- **The parse `ActorId` publishes to it**; the WASM viewer reads and writes it.
- **Admission is structural only** — is this a well-formed record?
- **Push rules exist but may be trivial** — the mechanism is present so the
  configuration interface is exercised, even if the first predicate is
  "everything".

This is enough to make the browser instance work (`overlay_model.md` §1) and to
give the parse agent somewhere to publish. Every extension — network endpoints,
signatures, richer predicates, tiering — attaches at an interface that already
exists, rather than requiring the model to change shape.

## 8. What this supersedes

`federated_belief_network.md` §3.3 (pull-based replication with per-peer
watermarks) and §3.5 (subscribe-on-unresolved-reference) describe peer-to-peer
L2 replication. Both are superseded for Layer 3 by this document, and that
document's own §1.1 already argues L2 replication is replicating a cache — L1 is
git's job, L2 is a pure function of L1, and **L3 is the only layer that cannot be
derived.**

Until that revision runs, this document governs Layer 3 record movement.

## 9. Open questions

- **Does a store need a write-cost mechanism, and is it a capability?** §3.1a
  records the shape a `RunStart` token would take and why nothing needs one
  until a store accepts unsolicited writes. The generalization is Issue 16's
  parked capability model, inherited by the async-overlay successor to Issue 110.
- **Does a collector have its own `ActorId`?** It stamps arrival order, which is
  an observation, which makes it an actor. §5.1 argues the ordering machinery is
  already sufficient either way — what the `ActorId` would add is the ability for
  the collector to *author* records (receipts, admission decisions), which is a
  separate question from ordering.
- **Can the parse agent clear when a collector is unreachable?** Recommend yes,
  fail-fast: `federated_belief_network.md` §1.2 already establishes an
  unresolvable citation as the ordinary partial-scope case, not corruption.
- **Does back-propagation stay a read?** An agent asking "what upstream records
  cite mine?" before clearing must be a *query*, never the collector writing into
  the personal log — otherwise single-writer ownership collapses and §3.1's
  retention argument with it.
- **Is transitive trust acceptable at the system tier?** With summary-only
  promotion, the system collector trusts the subsystem's judgment. It configured
  that endpoint deliberately, so this may be fine — but it should be a stated
  choice, since it is exactly what local computability otherwise avoids.
- **Where does separation of duty apply?** The party configuring a push predicate
  and the party whose records it promotes should plausibly differ. Not required
  for Phase 1; likely required for anything audited.

## 10. References

- `docs/essays/trust_boundaries_and_admission.md` — the general argument and its
  antecedents; this document is its noet instantiation
- `annotation/overlay_model.md` — how records compose into a readable graph;
  §5.2's percolation is this document's automatic promotion
- `annotation/living_corpus.md` §2 — the layer model
- `annotation/collaboration_overlay.md` §4a — peer-derived credentials; the
  reason promotion is automatic rather than requested
- `annotation/federated_belief_network.md` §1.1 (why L3 is the layer that
  matters), §1.2 (percolation), §3.3/§3.5 (superseded — see §7)
- `core/beliefbase_architecture.md` §4.3 — `Envelope`, `ActorId`, `EventId`
- Issue 105 — the store implementation; its G-Set claim is scoped by §3.1, and
  it owns the retention question this model's tiering implies
- Issue 105 — run lifecycle; §4.2 makes its orphaned-member case expected
- Issue 104 — the record-kind registry, where admission policy belongs
