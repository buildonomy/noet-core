---
title = "The Annotation Channel — Issuing, Holding, and Moving Records"
authors = ["Andrew Lyjak", "Claude"]
status = "Target architecture — the write API; transport chosen by trade study"
version = "0.1"
dependencies = [
  "annotation/living_corpus.md",
  "annotation/overlay_model.md",
  "annotation/collector_model.md",
  "core/beliefbase_architecture.md",
]
---

# The Annotation Channel

## 1. Purpose

`living_corpus.md` describes what an annotation *is* and how it composes with
the corpus. `overlay_model.md` describes how it is *read*. This document
describes how it is **issued**: the write-side API through which any producer —
the parse pipeline, a browser session, an MCP agent, a background crawler —
emits records into a store it never touches directly.

The whole annotation layer can be read as **a system for issuing, moving, and
semantically enhancing log-type records.** The transport is log-like: a producer
pushes a typed payload into a channel and moves on. The semantics are not:
records carry identity, cite each other, anchor to a queryset, and fold into
state (`living_corpus.md` §4). The channel is where the log-like half lives.

## 2. The shape

```
AnnotationChannel::open(actor: ActorId, sink: SinkSelector) -> Handle

Handle::emit(payload: impl Into<Annotation>,
             anchor:  QuerySpec,
             caused_by: &[EventId]) -> EventId

Handle::close()
```

Three properties, each load-bearing:

**The handle mints identity.** `EventId = (actor, session, sequence)`
(`beliefbase_architecture.md` §4.3). The handle holds the actor it was opened
with, a `session` drawn at open, and a process-local counter — so `emit` can
mint the id and **return it** without consulting any store. That return is what
lets a producer cite its own earlier record in a later `caused_by`, which is the
difference between a log and a causal record.

**Open and close are the run bracket.** `open` is `RunStart`, `close` is
`RunEnd` (Issue 105), and the handle's `session` is the run's identity.

**An unbracketed `emit` is a `RunEnd`** — a run of length one, opened and closed
by the same record. It is not a record *outside* the run model; a bare receipt
is a complete claim, which is exactly what a `RunEnd` is. So `emit` has two
modes, determined by whether a bracket is open: inside one it produces a member,
outside one it produces a terminal record. A procedure needs the bracket; a
single receipt does not, and gets the same terminal semantics either way.

**The producer never sees the sink.** `SinkSelector` names a store; the handle
routes. A parse and a browser tab call the same `emit`; only `open` differs.
This is what makes the channel symmetric between the compiler and the WASM
runtime rather than making one an "instance" of the other.

## 3. Two lanes: records and diagnostics

The channel carries **records** — things with an `EventId` that another record
may cite. It does not carry **diagnostics** — parse warnings, progress, timing —
which are disposable, uncited, and already served by `tracing`.

The two lanes are kept in step by a `tracing::span!` the handle opens alongside
itself, carrying `actor`, `session`, and the corpus version as span fields. Any
ordinary `tracing::warn!` emitted inside that span is attributable to the run
without becoming a record.

| | Record lane | Diagnostic lane |
|---|---|---|
| Carried by | the channel, typed | `tracing`, as today |
| Has an `EventId` | yes | no |
| May be cited | yes | no |
| Persisted | per sink policy | per subscriber; default no |
| Cost when nobody listens | one branch (§5) | level filter |

A diagnostic that *should* be a record — an unresolved reference once Issue 103
gives it a node to anchor to — is promoted deliberately: the producer emits it on
the channel. Nothing promotes it implicitly. This is the answer to "are compiler
observations annotations?": **some are, and the producer says which by choosing
the lane.**

## 4. Payload shape is the producer's, not the node's

A record's payload is any `Annotation` the registry knows
(`attestation_fabric.md` §6). Nothing requires one record per node.

The compile-time layout pipeline is the motivating case. It currently writes
four fields onto every in-scope node (`src/layout.rs:216-234`) because
`metadata` is a node field and a node field is the only place a per-node value
can go. As a record it is **one emission**: a layout run over a scope, anchored
to that scope's `QuerySpec`, with `{bid → position}` in the payload. One record,
one actor, one corpus version, atomically supersedable — instead of ~128k
entries per parse with the provenance implied by position.

The rule: **a record carries what one run of one producer determined, at the
granularity that run naturally has.** A force simulation has one result. A
review has one disposition per section. A receipt has one document. Let the
payload follow the work.

## 5. Free when disabled, never fails the producer

Two rules inherited from the earlier instrumentation crate this design descends
from (`docs/project/trades/TRADE_ANNOTATION_CHANNEL_TRANSPORT.md`):

- **Disabled is a branch.** The handle holds `Option<Sender>`. With no sink
  configured, `emit` mints an id and returns. No allocation, no serialization,
  no filter pass.
- **An unwritable sink never fails a parse.** On sink failure the handle
  degrades to a bounded in-memory buffer, emits one diagnostic on the
  diagnostic lane, and continues. The parse completes; the records are what is
  lost, and the diagnostic says so.

The second rule is the `FailsafeBuffer` of the prior art, generalized. Its
justification is unchanged: instrumentation that can break the thing it observes
is worse than none.

## 6. The halo of stores

Opening a channel is cheap by design, and the intent is that **it is easy to
initiate a store** — a parse gets one, a browser session gets one, an agent gets
one. The consequence is many stores, and the design question becomes how they
relate rather than how one is structured.

Every store that a reader can reach forms that reader's **halo of stores**: the
set whose records may layer onto the graph they are looking at. The API and the
UX both expose this set directly — a reader can see which stores are local,
which are shared, which are someone else's, and filter the layered result by
actor.

What is fixed here is small:

- A store is **identified by the `(actor, session)` pairs it holds**, so the
  halo is enumerable without reading records.
- Stores are **ordered by a configured precedence**, and that order is what
  gives "narrower wins" its meaning when two stores hold claims about one node
  (`overlay_model.md` §4).
- Records **move between stores by promotion** — an explicit act, reviewed in
  the UX, taking a record from a local store to a more durable, non-local, or
  shared one. Some stores have network-sync conduits; to the reader they look
  like any other store with a different position in the precedence.

What is deferred, deliberately: **the promotion protocols themselves.** The
channel selects a sink at open time and stops there. How a record later moves,
what crosses a boundary, and what a receiving store may refuse are
`collector_model.md` §4 and §6 — the next layer, kept out of the write API so
the write API stays small enough to be easy to open.

## 7. What each existing piece becomes

| Piece | Under the channel |
|---|---|
| `BeliefNode.metadata` compiler observations | candidates for the record lane, one payload per run; the producer chooses lane by lane (§3, §4) |
| `BeliefNode.metadata` directive caches (`_query_specs`, `_maps_to_specs`) | **unchanged** — a parse of source content, not an observation; must not depend on L3 (`content_versioning.md` §5.1a) |
| `BeliefBase.diagnostics` | the diagnostic lane; retires as Issue 103 intended |
| `RunStart` / `RunEnd` (Issue 105) | `open` / `close` |
| `EventId.session` (P1) | the handle's session |
| Issue 105's store | one sink among several in the halo |
| `collector_model.md` §3.1 personal log | the sink a parse opens by default |

## 8. Open questions

- **Item type on the wire.** Whether `Event::Annotation(Envelope)` shares the
  compiler's existing `BeliefEvent` channel or rides its own `Sender`. Either
  satisfies §2; the trade study leaves it to implementation.
- **Open-time reconciliation.** A channel opening against a store that already
  holds this actor's records over this scope: supersede, accumulate, or refuse?
  The information to decide is present at `open` (actor and scope are both
  known), which is why the channel rather than the store should own the rule —
  but the rule itself is not chosen.
- **Store identity.** §6 identifies a store by the `(actor, session)` pairs it
  holds. Whether it also needs an identity of its own — a collector `ActorId`
  (`collector_model.md` §9) — is unresolved.
- **The halo query across stores.** Issue 105 step 5 specifies the
  single-store, single-actor halo. The union across a reader's store halo, with
  precedence applied, is the general form and is unspecified.

## 9. References

- `annotation/living_corpus.md` §4 — assert vs. mutate; the `Envelope`; why
  records are stateful
- `annotation/overlay_model.md` §2, §5, §6 — how records are read; layer
  precedence; the halo query
- `annotation/collector_model.md` §4, §6 — promotion and admission, the layer
  above this one
- `core/beliefbase_architecture.md` §4.3 — `EventId = (actor, session,
  sequence)`
- `docs/project/trades/TRADE_ANNOTATION_CHANNEL_TRANSPORT.md` — why a typed
  handle plus a `tracing` span, and the prior art it descends from
- Issue 105 — the store, run brackets, and fold; Issue 103 — the node anchor
  that lets a diagnostic become a record
