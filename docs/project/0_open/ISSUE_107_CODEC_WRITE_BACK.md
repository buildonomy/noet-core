---
version = "0.1"
title = "Issue 107: Generalized Codec Write-Back — BeliefEvent to Source"
---

# Issue 107: Generalized Codec Write-Back — BeliefEvent to Source

**Priority**: MEDIUM
**Estimated Effort**: 4 days (RELATIVE COMPARISON ONLY)
**Dependencies**: Requires Issue 103 (node source ranges — a codec must know which
byte span a `Bid` occupies). Blocks Issue 106 (enactment interface) and the
`L3 → L1` **enact** arrow in `docs/design/annotation/living_corpus.md` §7 — which
is the conditional arrow, not the arrow that closes the loop. **Blocks nothing
on the pilot path.**
**Design doc**: `docs/design/annotation/living_corpus.md` §What Is Not Yet Bidirectional

## Summary

The compile pipeline is bidirectional only *within a single parse*. A codec can
rewrite the file it just parsed, because it still holds that file's event vector
— but no codec can accept a `BeliefEvent` for a node it did not just parse and
turn it into a source edit. This issue builds that capability.

> [!IMPORTANT]
> **This issue is the write half only, and it does not gate loop closure.**
>
> The annotate → change loop closes at a **hand-off**: a rendered change package
> delivered to whatever process owns the target (`living_corpus.md` §7). In most
> real cases that target is a controlled document, another team's repository, or
> a generated file, and noet holds no authority to write it.
>
> **The change package does not pass through this issue.** It is produced
> entirely on the read side — project the redline into a graph state, diff it
> against the corpus, render the result with the record's provenance (Issue 74).
> No source text is written, and none needs to be. Do not treat this issue as a
> narrower version of that path; it is different machinery serving a different
> purpose. What lives here is byte ranges, cmark event vectors, atomic writes,
> and conflict detection — none of which the render needs.
>
> Two consequences for this issue's scope:
>
> - **Round-trip fidelity is a per-codec property that must be declarable**, not
>   assumed of every codec. Issue 106 gates on it together with a per-network
>   authority declaration.
> - **This issue is the *last* step of enactment, not the first.** Its input is a
>   `BeliefEvent` stream that some upstream step already produced. If that stream
>   comes from folding a redline, the fold is Issue 105's and the projection is
>   Issue 110's — see the unowned step noted in Issue 74.

## The current asymmetry

`DocCodec` already has write-back methods (`generate_source`,
`generate_source_bytes`, `set_node_bid`), so it is easy to assume the return path
exists. It does not, in the general case.

```
FORWARD (general):
  source ──parse──> IRNodes ──GraphBuilder──> BeliefEvent ──BeliefSink──> store

REVERSE (parse-scoped only):
  source <──generate_source── current_events
                                   ↑
                        same codec instance, same parse
```

`MdCodec::generate_source` (`src/codec/md.rs:2543`) renders
`self.current_events` — the pulldown-cmark event vector captured during *this
instance's* parse of *this* file. Mutations are applied to that vector during
`inject_context` (`md.rs:2208-2537`) by compiler-internal machinery: BID
injection, link normalization, frontmatter merge. The file is then re-rendered
from the mutated events.

This is a **parse-scoped round-trip**, not a general write path. Three
consequences:

1. A codec cannot act on a node it did not just parse — `current_events` is empty.
2. Nothing routes a `BeliefEvent` to the codec owning that node's file.
3. `BeliefEvent` never reaches a codec at all. `BeliefSink` has exactly two impls
   (`BeliefBase`, `DbConnection` — `src/beliefbase/sink.rs`), both stores.

## Why `BeliefSink` cannot be reused unchanged

`BeliefSink::apply_batch(&mut self, events: &[BeliefEvent])` is shaped for
stores, and a codec sink violates three of its assumptions:

| `BeliefSink` assumes | A codec sink requires |
|---|---|
| Stateless apply — any event at any time | Must load and parse the target file first to populate its event vector |
| Events span all networks | Events must be routed to the one codec owning that node's file |
| Mutates opaque whole-store state | Must map `Bid` → source byte range to know what to edit |

The third is why Issue 103 is a hard dependency: without a `Bid → Range`, a codec
receiving `NodeUpdate(bid, node)` has no way to locate the text to replace. This
is also why Issue 103 is not merely an LSP convenience — it is load-bearing for
write-back.

## Goals

1. A codec-side write interface that accepts `BeliefEvent`s for nodes the codec
   did not just parse and produces a source edit.
2. Routing from a `BeliefEvent` to the codec instance owning the target node's
   file, via the existing `CodecMap` / two-registry dispatch.
3. A defined subset of `BeliefEvent` variants that are writable, and explicit,
   non-silent rejection of the rest.
4. Atomic, conflict-detecting writes (the mechanism Issue 106 consumes).
5. `MdCodec` as the reference implementation; other codecs opt in.

## Architecture

### `SourceSink`: a sibling trait, not a `BeliefSink` impl

```rust
pub trait SourceSink {
    /// Load the file backing `path` and prepare for edits.
    fn open(&mut self, path: &Path, content: &str) -> Result<(), BuildonomyError>;

    /// Apply one event. Returns Unsupported for variants this codec cannot express.
    fn apply(&mut self, event: &BeliefEvent) -> Result<WriteOutcome, BuildonomyError>;

    /// Render the mutated state back to source text. None = no change.
    fn render(&self) -> Option<String>;
}
```

`open` + `render` are the load/store bookends that `BeliefSink` lacks. For
`MdCodec`, `open` is a parse that populates `current_events` and `render` is the
existing `generate_source` — so the reference implementation is mostly wiring,
not new rendering logic.

Keeping this separate from `BeliefSink` avoids widening a trait that two stores
implement correctly today.

### Writable event subset

Not every graph mutation has a source expression. Be explicit rather than
best-effort:

| Event | Writable | Notes |
|---|---|---|
| `NodeUpdate` / `NodeUpsert` | **yes** | title → heading text; payload → frontmatter |
| `RelationUpdate` / `RelationChange` | **yes** | emit or rewrite a link / relation directive |
| `RelationRemoved` | **yes** | remove the link that expressed it |
| `NodesRemoved` | **partial** | removing a section removes its text — destructive; require opt-in |
| `NodeRenamed` | **no** | a BID change is compiler-internal, not source-expressible |
| `PathAdded` / `PathUpdate` / `PathsRemoved` | **no** | file moves are a filesystem concern, not a codec's |
| `FileParsed` / `Batch*` / `BuiltInTest` | **no** | control flow |

`WriteOutcome::Unsupported` must be a visible, diagnosable outcome — never a
silent no-op. A caller that expected a write and got nothing should be able to
tell.

### Structural vs. content-expressed relations

The same distinction that shaped the hashing decision applies here, and it is the
main source of difficulty.

- **Content-expressed** relations (`{maps_to}`, links, `{implements}`) have a
  textual site. Writing them means editing or inserting that text.
- **Structural** relations (Section containment) have no directive to edit —
  containment is derived from document position. Expressing "node X is now a
  child of Y" means *moving text*, potentially across files.

**Phase 1 handles content-expressed relations only.** Structural mutation via
write-back is out of scope; it is a document-restructuring operation, not an
edit, and it needs its own treatment. Reject it explicitly with
`WriteOutcome::Unsupported`.

### Conflict detection

A codec writes text it parsed at some earlier moment. Between the parse and the
write, the file may have changed on disk. Before writing, re-hash the target span
and compare against the hash recorded at `open`; on mismatch, refuse and surface
a conflict rather than clobbering. This reuses the `content_hash` machinery from
Issue 66 step 1a.

### Atomicity and the watcher

Writes are temp-file + rename. The write must register with the server
(Issue 102) so the file watcher does not treat it as an external change and
re-parse in a loop. `ignored_write_paths` (`src/watch.rs`) suppresses only writes
made by the *same* `WatchService` instance, so it is insufficient once a second
writer exists. Issue 106 §2 covers the same ground from the caller's side —
coordinate rather than solving it twice.

## Implementation Steps

1. **Define `SourceSink`** (0.5 days)
   - [ ] Trait, `WriteOutcome` enum (`Written` / `Unsupported` / `Conflict`)
   - [ ] Document the writable-subset table above as the trait contract

2. **Implement for `MdCodec`** (1.5 days)
   - [ ] `open`: parse the file into `current_events`, record span hashes
   - [ ] `apply` for `NodeUpdate` / `NodeUpsert`: locate the node's range
         (Issue 103), rewrite heading text and/or frontmatter in `current_events`
   - [ ] `apply` for relation events: insert, rewrite, or remove the expressing
         link or directive
   - [ ] `render`: delegate to the existing `generate_source`
   - [ ] Reject structural and path events with `Unsupported`

3. **Routing** (1 day)
   - [ ] Resolve a `BeliefEvent`'s target `Bid` to its owning file via `PathMap`
   - [ ] Instantiate or reuse the owning codec via `CodecMap`
   - [ ] Group events by target file so each file is opened, mutated, and
         rendered once per batch
   - [ ] **Apply multiple edits to one file in a single pass**, or sorted
         descending by offset. A batch is the normal case, not an optimisation:
         one change package routinely carries several edits to one document
         (Issue 106 §Redline promotion). Applying them sequentially against
         ranges recorded before the first edit is **silently corrupting** — the
         first splice shifts every later offset in the file
   - [ ] Detect two events targeting overlapping spans and surface it as a
         conflict rather than applying both
   - [ ] Honour the two-registry dispatch rules (`beliefbase_architecture.md`
         §3.2) — a claimed file must route to its claiming codec

4. **Atomic write + conflict detection** (0.5 days)
   - [ ] Temp-file + rename
   - [ ] Span-hash check before write; `Conflict` on mismatch
   - [ ] Register the write with the watcher so it does not re-trigger

5. **Tests** (0.5 days)
   - [ ] Round-trip: parse a document, emit a `NodeUpdate` changing a title,
         apply, render — the heading text changes and nothing else does
   - [ ] Relation write: a new `{maps_to}` appears in source and re-parses to the
         same edge
   - [ ] Unsupported: a `NodeRenamed` returns `Unsupported`, writes nothing, and
         is diagnosable
   - [ ] Conflict: mutate the file on disk after `open`; the write is refused
   - [ ] Idempotence: applying the same event twice produces one change

## Testing Requirements

- A document written by `SourceSink` and then re-parsed produces a graph
  equivalent to applying the same events directly to a `BeliefBase` — the write
  path and the store path must agree.
- No write loop: a `SourceSink` write during a watch session does not trigger a
  re-parse that generates further writes.
- A file with no applicable events is not rewritten at all (no mtime churn — this
  matters for Issue 66's incremental skip).

## Success Criteria

- [ ] `SourceSink` exists with a documented writable-event subset
- [ ] `MdCodec` implements it; a `NodeUpdate` for a node parsed in an *earlier*
      session produces a correct source edit
- [ ] Events route to the owning codec via `PathMap` + `CodecMap`
- [ ] Unsupported variants are explicit and diagnosable, never silent no-ops
- [ ] Stale-span writes are refused with a conflict, not applied
- [ ] Writes are atomic and do not re-trigger the watcher
- [ ] Issue 106 can build redline promotion on this without further primitives

## Risks

- **Round-trip fidelity.** `events_to_text` (`md.rs:1588`) reconstructs Markdown
  from cmark events; a mutation may perturb formatting elsewhere in the file.
  → **Mitigation**: the no-applicable-events test asserts byte-identical output;
  extend it to assert that a single-node edit changes only that node's span.
- **Scope creep into document restructuring.** Structural relation writes look
  adjacent but are a different problem (moving text across files).
  → **Mitigation**: explicitly `Unsupported` in Phase 1; revisit only with a
  concrete consumer.
- **Divergence from `BeliefSink`.** Two apply-shaped traits may drift.
  → **Mitigation**: `SourceSink` deliberately does not extend `BeliefSink`; the
  writable-subset table is the contract that keeps the difference legible.
- **Codecs that cannot write.** Binary codecs (XLSX) return `None` from
  `generate_source` today. → **Mitigation**: `SourceSink` is opt-in; a codec that
  does not implement it simply never receives write events.

## Open Questions

- Should `SourceSink` be a separate trait or a defaulted extension of `DocCodec`?
  A defaulted extension keeps codecs in one trait and lets non-writers inherit
  `Unsupported`. A separate trait keeps `DocCodec` focused on reading. Recommend
  the defaulted extension for discoverability; decide at implementation time.

  **One constraint on that choice**: `living_corpus.md` §11 records a plausible
  future where the annotation store stages a batch of changes and commits it
  outward to *several* substrates — source, an issue tracker, a spreadsheet — not
  just to source files. A tracker sink would be this same trait against a
  different substrate, mirroring what `RecordSource` (Issue 108) does for reads.
  Nothing here should be built to require a `DocCodec`, a file path, or a byte
  range in ways a non-file substrate could not satisfy. `open`/`apply`/`render`
  is already substrate-neutral in shape; keep it that way. This is a
  don't-foreclose constraint, not a requirement to generalize now.
- Where does a *new* node get written? **A redline names the file**, so there is
  no placement policy to choose: a new document is a path absent from the base
  map, a new section is content written in place, and a new network is
  `subnet/index.md` (`living_corpus.md` §5,
  `identity/generational_archive.md` §6.1). Phase 1 may still restrict itself to
  edits of existing files, but that is a scope decision rather than a gap in the
  model.

  What this issue does inherit: proposed content may **supplant** an existing
  position, so a write can reorder siblings rather than splice into a span. **Do
  not assume the write set is always a single contiguous range.**
- Does write-back belong in the codec at all, or in a layer above it that owns
  source text and calls codecs only for rendering? The codec has the event vector
  and the range mapping, so it is the natural site — but revisit if the routing
  logic grows past the codec boundary.

## References

- `src/codec/md.rs:2543` — `generate_source`, the parse-scoped write path
- `src/codec/md.rs:1588` — `events_to_text`, the cmark re-render
- `src/codec/md.rs:2208` — `inject_context`, where compiler-internal mutations
  are applied to `current_events`
- `src/beliefbase/sink.rs` — `BeliefSink` and its two store impls
- `src/event.rs` — `BeliefEvent` variants; the writable-subset table above
- `docs/design/core/beliefbase_architecture.md` §3.2 — two-registry codec dispatch,
  which routing must honour
- `docs/design/core/beliefbase_architecture.md` §3.6 — `DocCodec` trait surface
- Issue 103 — node source ranges; `Bid → Range` is required to locate edits
- Issue 106 — source write-back and redline promotion; the primary consumer
- Issue 102 — `noet serve`; owns the idle boundary and watcher coordination
- `docs/design/annotation/living_corpus.md` — the `L3 → L1` arrow this issue implements
