---
version = "0.1"
title = "Issue 110: The Annotation Channel — One Write API for Parse, Browser, and Agent"
---

# Issue 110: The Annotation Channel

**Priority**: HIGH — the write side every other annotation issue assumes
**Estimated Effort**: 2 days design + 3 days implementation (RELATIVE COMPARISON ONLY)
**Dependencies**: Requires `docs/design/annotation/annotation_channel.md` (this
issue implements it), `beliefbase_architecture.md` §4.3 (`EventId =
(actor, session, sequence)`), and the transport decision in
`docs/project/trades/TRADE_ANNOTATION_CHANNEL_TRANSPORT.md`. Requires Issue 105
for a durable sink; **does not require it to be started** — the in-memory sink
is enough to exercise the API.
**Blocks**: Issue 105 (run brackets are `open`/`close`), Issue 104 (the first
record kinds need something to emit them), the W4 receipt skeleton.
**Design doc**: `docs/design/annotation/annotation_channel.md`

## Summary

Every annotation producer — the parse pipeline, a browser session, an MCP agent,
a background layout crawler — needs the same thing: a handle opened with an
`ActorId`, an `emit` that takes a typed payload and returns the `EventId` it
minted, and a `close`. The producer never sees the store. This issue builds that
handle, its in-memory sink, and the routing that lets several handles coexist in
one process.

The compiler is one client of the channel, not its definition. Which of its
current `metadata` observations become records is decided per observation, by
the compiler choosing a lane (`annotation_channel.md` §3), and this issue
performs that classification as its second half.

## Goals

1. `AnnotationChannel::open(actor, sink) -> Handle`, `Handle::emit(..) -> EventId`,
   `Handle::close()`, per `annotation_channel.md` §2
2. Two handles in one process route independently — a parse and an MCP agent
   never share a sequence or a sink
3. **Free when disabled**: `emit` on a handle with no sink is a branch and a
   counter increment, nothing else; assert it with a benchmark
4. **Never fails the producer**: sink failure degrades to a bounded buffer and
   one diagnostic; the parse completes
5. Every current `metadata` key classified into record lane, diagnostic lane, or
   node content — with the layout keys as the first record-lane producer

## Steps

1. **The handle** (1 day)
   - [ ] `Handle` holds `ActorId`, a 64-bit `session` drawn at open, a
         process-local `sequence`, and `Option<Sender<Envelope>>`
   - [ ] `emit` mints `EventId`, builds the `Envelope`, sends, returns the id
   - [ ] `open` also opens a `tracing::span!` carrying `actor`/`session`/corpus
         version so diagnostic-lane output inside the run is attributable
   - [ ] `close` emits `RunEnd` and closes the span (coordinate with Issue 105 on
         whether `open` emits `RunStart` or *is* it)
   - [ ] Decide and record: does `Event::Annotation(Envelope)` share the
         `BeliefEvent` `Sender` or take its own? (trade study leaves this open)

2. **The in-memory sink, and the failsafe** (0.5 days)
   - [ ] A `Receiver` draining into `Vec<Envelope>` — enough for tests and for
         the browser before IndexedDB exists
   - [ ] Bounded failsafe buffer on sink error; one diagnostic; producer proceeds
   - [ ] Benchmark: `emit` with `None` sink vs. a no-op function call

3. **Classify the compiler's observations** (0.5 days)
   For each current `metadata` key, choose a lane and record the reason:

   | Key | Lane | Reason |
   |---|---|---|
   | layout ×4 (`render_position`, `assembly_index`, `structural_weight`, `structural_depth`) | **record** — one payload per layout run | pure function of the graph; naturally lazy; its consumer (Issue 85) is unbuilt so nothing migrates |
   | `content_profile` | record or node content — **decide** | cheap, per node, feeds layout; may belong with it |
   | `source_url`, `git` | **decide** | parse-natural, cheap; the case for a record is provenance, the case against is that nothing else can compute them |
   | `_query_specs`, `_query_options`, `_query_texts`, `_maps_to_specs` | **node content** — unchanged | a parse of source, consumed by rendering; must not depend on L3 (`content_versioning.md` §5.1a) |
   | `ParseDiagnostic` | **diagnostic lane** | uncited, disposable; promoted to a record only once Issue 103 gives it a node |

   - [ ] Write the decided table into `annotation_channel.md` §7
   - [ ] Correct Issue 105 step 3a-pre's destination for whichever keys leave
         `metadata`

4. **Layout as the first record-lane producer** (1 day)
   - [ ] `compute_layout_metadata` opens a channel with the compiler actor and
         emits **one** record: anchor = the layout scope's `QuerySpec`, payload =
         `{bid → position, ..}`; stops writing the four per-node keys
   - [ ] Confirm nothing in-tree reads them (`assets/viewer/graph3d.js` does not
         exist; Issue 85 is parked) — so this is a removal, not a migration
   - [ ] Note for Issue 85: the viewer reads the layout record via the halo query
         rather than per-node `metadata`

5. **Tests** (0.5 days)
   - [ ] Two handles, same actor, concurrent: disjoint `EventId`s, independent
         sinks
   - [ ] `emit` returns an id that a second `emit` may cite in `caused_by`
   - [ ] Sink error mid-run: parse completes, one diagnostic, buffer bounded
   - [ ] Disabled-cost benchmark within noise of a no-op

## Done When

- [ ] A parse and an in-process MCP agent each hold a channel and neither can
      observe the other's records or sequence
- [ ] The layout pipeline emits one record per run and writes no per-node
      `metadata` keys
- [ ] Every `metadata` key has a recorded lane in `annotation_channel.md` §7
- [ ] `emit` with no sink benchmarks as a branch
- [ ] Issue 105 can define `RunStart`/`RunEnd` as `open`/`close` without a
      second identity scheme

## Risks

- **Two lanes drift.** A diagnostic that should have become a record stays a log
  line because nobody promoted it. → **Mitigation**: step 3's table is the
  contract; a new `metadata` key without a lane entry fails review.
- **Easy-to-open channels produce write-only stores.** The layout keys are the
  in-tree cautionary case — four fields on every node, serialized into every
  shard, for a consumer never finished. A channel makes emitting *easier*. →
  **Mitigation**: the sink selector is explicit at `open`, and a store's halo
  entry (`annotation_channel.md` §6) is enumerable, so "which stores have no
  reader" is a query rather than an archaeology exercise.
- **Open-time reconciliation is unspecified.** A second layout run against a
  store already holding one: supersede, accumulate, or refuse? → **Mitigation**:
  Phase 1 supersedes by `(actor, anchor)` and records that as the provisional
  rule; the general protocol is `collector_model.md` §4's and stays deferred.

## Open Questions

- Does the compiler actor's identity include the binary version
  (`content_versioning.md` §7.2 wants drift attributable)? Issue 104 census
  item 2 asks the same question; answer once.
- Is a store identified only by the `(actor, session)` pairs it holds, or does
  it need its own `ActorId` (`collector_model.md` §9)?

## References

- `docs/design/annotation/annotation_channel.md` — the design this implements
- `docs/project/trades/TRADE_ANNOTATION_CHANNEL_TRANSPORT.md` — why a typed
  handle plus a `tracing` span
- `src/codec/compiler.rs:279`, `:2606` — the existing typed `BeliefEvent`
  channel the handle may share
- `src/layout.rs:216-234` — the four per-node writes step 4 replaces
- `src/properties.rs:1073-1086` — `metadata`'s current doc comment and
  lifecycle
- Issue 105, Issue 104, Issue 103, Issue 85, Issue 92
