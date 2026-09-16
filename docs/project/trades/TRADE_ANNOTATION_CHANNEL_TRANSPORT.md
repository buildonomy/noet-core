# Trade Study: Transport for the Annotation Channel

**Version**: 0.1
**Status**: Open — options enumerated, decision pending
**Related**: Issue 110 (the annotation channel), Issue 105 (record store),
Issue 109 (run brackets), `docs/design/annotation/living_corpus.md` §4

## Summary

The annotation layer needs a **write-side channel**: a handle a producer opens
with an `ActorId`, pushes typed records into, and closes — with the records
routed to a store the producer never touches directly. Both the parse pipeline
and the WASM runtime open one. Whoever is listening decides what happens;
nobody listening costs a filter check.

This study chooses the **transport** under that handle. Three constraints
eliminate most of the space:

1. **Typed payloads end to end.** Records carry `Envelope { EventId, ActorId,
   observed_at, caused_by, payload }` plus a `QuerySpec` anchor. Round-tripping
   that through a string field is a cost paid on every record and a
   deserialization failure mode on every read.
2. **Records must return their identity.** A producer needs the `EventId` back
   so a later record can cite it via `caused_by`. Fire-and-forget is
   insufficient.
3. **Concurrent channels in one process.** One human with a viewer, an MCP
   agent, and a CLI is one actor with three writers (P1); a parse and an MCP
   agent are two actors in one process. Routing is per-channel, not global.

Two further requirements shape the choice without eliminating options: **free
when disabled** (`LESSONS_LEARNED.md` §Instrumentation must be free when
disabled), and **never fail the producer** — an unwritable store must not fail a
parse.

## Prior art: the earlier instrumentation crate

An earlier iteration of this project built exactly this shape on `tracing`
(`capture_data<D: Serialize>` → `tracing::event!` with `header`/`data` string
fields → a routing `Layer` classifying on the header prefix → per-type CSV
writers, with a session bracket opened over FFI and a ring-buffer failsafe when
file handles could not be opened).

What it demonstrated, and what transfers:

| Property | Mechanism | Transfers? |
|---|---|---|
| Producer decoupled from consumer | global dispatcher; producer never sees the sink | **yes** — this is the whole point |
| Free when disabled | level filter (`TRACE` for data, `INFO` for annotations) | **yes** |
| Never fails the producer | ring-buffer failsafe on I/O failure | **yes** — should be a stated design rule |
| Session bracket | `start_capture_session` / `stop_capture_session` | **yes** — it is `RunStart`/`RunEnd` (Issue 109), and the session id is `EventId.session` (P1) |
| One global capture state | `Lazy<Arc<Mutex<CaptureStateInner>>>`, one `active` flag | **no** — constraint 3 |
| Payload as JSON string field | `serde_json::to_string` at emit, parse at receive | **no** — constraint 1 |
| Fire-and-forget | `tracing::event!` returns `()` | **no** — constraint 2 |

The three non-transfers are exactly the three constraints. They are properties
of the *transport chosen*, not of the design, which is why this study exists.

## Options

### A — `tracing` as-is (events with string payloads)

The prior-art shape, ported.

- **For**: mature; `tracing-wasm` already in the dependency tree; level filtering
  and swappable `Layer`s are exactly the disabled-cost and routing story;
  interoperates with existing diagnostic output.
- **Against**: violates constraint 1 outright. `tracing`'s field model is
  primitives-and-`Debug` (`record_str`, `record_i64`, ...); a struct payload
  must be stringified. Violates constraint 2 — no return value; the handle would
  mint the `EventId` *before* emitting and hope the event landed. Constraint 3
  is workable (route on an `actor` field) but the dispatcher is process-global.
- **Rejected as-is.** Retained as the baseline the others are measured against.

### B — `tracing` with structured values (`valuable`)

`tracing` 0.1.x has an unstable `valuable` integration
(`RUSTFLAGS="--cfg tracing_unstable"`) that records structured data without
stringification: a `Visit` impl receives `&dyn Valuable` and can walk fields
typed.

- **For**: satisfies constraint 1 at the transport level while keeping A's
  routing and filtering. Same dependency.
- **Against**: unstable cfg flag on a public crate's build — every downstream
  consumer inherits it, and noet-core is destined for public release. Still
  fire-and-forget (constraint 2). `valuable` visits are borrowed; a subscriber
  that wants to *keep* the payload must clone it out field-by-field, which is a
  second, hand-written serializer.
- **Assessment**: better than A on paper; the unstable flag is likely
  disqualifying for a library.

### C — A dedicated typed channel (the `BeliefEvent` pattern)

noet already moves typed events through
`tokio::sync::mpsc::UnboundedSender<BeliefEvent>` (`src/codec/compiler.rs:279`,
`:2606`) with no serialization, and the outer `Event` enum
(`src/event.rs:192`) has the `Belief(BeliefEvent)` variant with `Annotation`
designed as its sibling (`living_corpus.md` §4). The channel would be a handle
wrapping an `mpsc::Sender<Envelope>` bound to an actor and a session:

```
AnnotationChannel::open(actor, session_cfg, sink_selector) -> Handle
Handle::emit(payload, anchor, caused_by) -> EventId     // mints and returns
Handle::close()                                         // RunEnd
```

- **For**: constraints 1–3 satisfied by construction — the payload is a Rust
  value the whole way, `emit` returns the id it minted, each handle is its own
  channel. Reuses the plumbing the compiler already runs. `Event::Annotation`
  is the slot the design already reserved.
- **Against**: routing, filtering, and "free when disabled" must be built
  rather than inherited — a `Sender` with no `Receiver` still allocates.
  Cheapest form: the handle holds `Option<Sender>` and `emit` on `None` is a
  branch. Loses interop with `tracing`'s subscriber ecosystem (no
  `RUST_LOG`-style control, no free console output for debugging).
- **Assessment**: strongest on the hard constraints; weakest on the soft ones.

### D — Hybrid: typed channel for records, `tracing` span for context

C carries the records. A `tracing::span!` opened alongside the handle carries
the *ambient* context — actor, session, corpus version — so that ordinary
`tracing::info!` diagnostics emitted inside the span are attributable to the
same run without being annotation records themselves.

- **For**: separates the two things the prior art fused. Records (typed,
  identified, cited) go down C. Log lines (untyped, uncited, disposable) stay in
  `tracing` and gain attribution for free via span fields. This is the
  "diagnostics are not annotations in Issue 104's sense" answer from Issue 110's
  open question, made structural.
- **Against**: two mechanisms to keep in step. A diagnostic that *should* be a
  record (an `UnresolvedReference` once Issue 103 gives it a BID) has to be
  promoted deliberately from the span to the channel.
- **Assessment**: likely the right split, and it dissolves the "is a diagnostic
  an annotation?" argument by giving each answer its own lane.

### E — A journald-style structured log

The question raised: is there a `tracing` equivalent closer to `systemd-journald`
— append-only, structured fields, indexed by field, seekable, with the
identity/cursor semantics a journal has?

Candidates in the Rust ecosystem, and why none fits directly:

| Candidate | What it is | Why not |
|---|---|---|
| `tracing-journald` | a `Layer` that *writes to* journald | Linux-only; a sink, not a transport; fields still go through `tracing`'s primitive model |
| `sled` / `redb` / `fjall` | embedded ordered KV / log-structured stores | a *store*, not a channel — the right layer for Issue 105, not for this question |
| `okaywal` / `rio` | write-ahead logs | ordered append with cursors, but bytes-in/bytes-out; serialization is the caller's |
| Custom | `Vec<Envelope>` + `session` cursor | trivially satisfies constraints; is C with a different name |

**Finding**: the journald properties that matter — append-only, field-indexed,
cursor-resumable — are properties of the **store** (Issue 105) and of the
`(session, sequence)` cursor P1 already provides. None of them is a *transport*
property. A journal is what a subscriber to C writes into, not an alternative
to C. Option E therefore collapses into "C, with a journal-shaped sink," which
is compatible with every option above.

## Comparison

| | A | B | C | D | E |
|---|---|---|---|---|---|
| Typed end to end (1) | ✗ | ✓ (unstable) | ✓ | ✓ | (store property) |
| Returns `EventId` (2) | ✗ | ✗ | ✓ | ✓ | — |
| Per-channel routing (3) | partial | partial | ✓ | ✓ | — |
| Free when disabled | ✓ inherited | ✓ inherited | must build | ✓ / must build | — |
| Never fails producer | ✓ (ring buffer) | ✓ | must build | must build | — |
| WASM | ✓ (`tracing-wasm`) | ? | ✓ (`mpsc` works; no tokio needed for unbounded) | ✓ | — |
| Public-release safe | ✓ | ✗ (cfg flag) | ✓ | ✓ | — |
| Reuses existing plumbing | diagnostics only | diagnostics only | **`BeliefEvent` channel** | both | — |

## Recommendation

**D — a typed handle for records, a `tracing` span for ambient context.** The
typed channel (C) is the only shape that meets all three hard constraints
without an unstable flag, and it is the pattern the compiler already uses. The
span keeps what `tracing` is genuinely good at — cheap, filterable,
attributable diagnostics — without forcing records through a string field.

Two design rules to carry from the prior art regardless of option:

- **An unwritable store never fails the producer.** The prior art's ring-buffer
  failsafe generalizes: on sink failure the handle degrades to an in-memory
  buffer and emits *one* diagnostic, and the parse completes.
- **Disabled means a branch, not a filter pass.** `Option<Sender>` on the
  handle; `emit` on `None` returns a minted id and does nothing else.

## What this does not decide

- **Promotion between stores.** The channel selects a sink at open time; how a
  record later moves from a local store to a shared one is the next layer
  (`collector_model.md` §4), deliberately deferred so the channel API stays
  small.
- **Store format.** Issue 105's file-per-record shape stands for durable
  records; whether a journal-shaped store (E's candidates) is better for
  high-volume regenerated records is Issue 105's to weigh once a producer exists.
- **Whether `Event::Annotation` is the channel's item type or a projection of
  it.** `living_corpus.md` §4 says folding an annotation *emits* `BeliefEvent`s;
  the channel carries `Envelope`s. Whether `Event::Annotation(Envelope)` lands
  on the same `Sender` as `Event::Belief` or on its own is an implementation
  choice the design doc should record once made.

## References

- `src/codec/compiler.rs:279`, `:2606` — the existing typed `BeliefEvent` channel
- `src/event.rs:192` — `Event { Ping, Belief }`, with `Annotation` as the
  designed sibling (`living_corpus.md` §4)
- `docs/design/annotation/living_corpus.md` §4 — assert vs. mutate; the
  `Envelope`
- `docs/design/core/beliefbase_architecture.md` §4.3 — `EventId =
  (actor, session, sequence)`; the handle mints it
- `docs/project/LESSONS_LEARNED.md` §Instrumentation must be free when disabled
- Earlier-iteration instrumentation crate (`capture_data`,
  `CaptureStateRouterLayer`, `start_capture_session`, `FailsafeBuffer`) — the
  prior art assessed above
