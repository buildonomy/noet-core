---
version = "0.1"
title = "Issue 103: Node Source Ranges"
---

# Issue 103: Node Source Ranges

**Priority**: HIGH — gates Issue 11 (LSP) and Issue 106 (write-back)
**Estimated Effort**: 2.5 days (RELATIVE COMPARISON ONLY)
**Dependencies**: Requires Issue 66's per-file `source_hashes` (the anchor's
validity key — see Decision 2). **Requires Issue 110** — ranges are compiler
observations, so they are annotations, and 110 settles where observations live
and how the overlay is read. Blocks Issue 11, Issue 106, Issue 107; informs
Issue 104 (its primitive census carries the range anchor).

## Summary

Position tracking in noet-core is half-built: `byte_offset_to_location` converts a
byte offset into a 1-based `(line, column)` pair, and diagnostics can carry that
pair. What exists is a **point**, attached to **diagnostics**. What is missing is a
**range** (`start..end`), attached to **nodes**, plus the reverse lookup from a
source position back to a BID.

Three downstream issues each need the same primitive and none of them owns it. This
issue owns it: every `BeliefNode` gains a source byte range, and each parsed document
gains a position → BID lookup. Issue 11 stops carrying position tracking as its
Step 1; Issue 106 gets the byte span it must overwrite; Issue 104 gets a span to
anchor annotations against.

## Goals

- Every node parsed from a source file has a resolvable byte range for that file
- A range is only ever served against the source text it was computed from —
  never silently applied to a newer file
- A position → BID lookup exists for a single document, sufficient for hover and
  go-to-definition
- `ParseDiagnostic` locations are derived from ranges rather than hand-plumbed points
- The `BeliefBase.diagnostics` stepping-stone is retired
- No change to source-file output: ranges never round-trip into authored markdown
- Range churn never produces a spurious `NodeUpdate`

## What Already Exists

Read `src/codec/diagnostic.rs` before starting. Do not rebuild any of this:

| Item | Location | State |
|---|---|---|
| `byte_offset_to_location(source, offset) -> (usize, usize)` | `codec/diagnostic.rs` | Complete, 9 unit tests, exported from `codec::` |
| `ParseDiagnostic::with_location` / `::location()` | `codec/diagnostic.rs` | Complete for `Warning`, `Info`, `ParseError` |
| `UnresolvedReference.reference_location: Option<(usize, usize)>` | `codec/diagnostic.rs` | Complete, plus `with_location()` |
| `IRNode.source_line: Option<usize>` | `codec/belief_ir.rs` | Line-only; populated by `MdCodec`, used for `#L<n>` backlinks |
| `pulldown_cmark` `into_offset_iter()` byte ranges | `codec/md.rs` | Already threaded through the event stream as `Option<Range<usize>>` |

The markdown codec already *has* byte ranges in hand — it discards all but the start
line (`md.rs` calls `byte_offset_to_location(&self.content, offset.start).0`). The
work is largely plumbing an existing value further, not deriving a new one.

## Architecture

### Decision 1: Store byte offsets, convert lazily

Store `Range<usize>` of **byte offsets**, not `(line, col)` pairs.

- The codec already produces byte offsets (`into_offset_iter`); storing them is free,
  while storing `(line, col)` costs a `byte_offset_to_location` call per node at parse
  time on the hot path.
- `byte_offset_to_location` already exists and is tested, so conversion at the LSP
  boundary — the only consumer that speaks in lines and columns — is a solved problem.
- Issue 106 (write-back) wants byte offsets directly: it splices a replacement string
  into `source[range]`. Converting to `(line, col)` and back would be a lossy detour.
- Rejected alternative: store both. Redundant state that can disagree after an edit.

### Decision 2: Ranges are anchored side-table data, not node fields

This supersedes Issue 11's Open Question 6 and Decision 3, which proposed adding
an ephemeral `metadata` field to `BeliefNode` carrying per-node source context.
That design has been overtaken twice — once by the code, once by the layer model.

**Overtaken by the code.** `BeliefNode` already has a `metadata: Table` field
(`src/properties.rs:1073`), but with the *opposite* properties to those OQ6
proposes: it **is** serialized via `toml()`, **is** persisted in the DB
`metadata` column, and **is** included in `PartialEq` (`:1091`, metadata compared
at `:1099`). OQ6 and Decision 3 both describe adding a field that exists.

**Overtaken by the layer model.** A source range is an *observation about* a
node — derived, per-parse, and true only of one file version. That is the
defining character of Layer 3 data (`docs/design/annotation/living_corpus.md` §2), not of
the belief graph. Storing it as a node field puts an observation inside the thing
observed.

> **The general argument is in `docs/design/annotation/overlay_model.md`.**
> Ranges are compiler observations, so they are annotations (Issue 110), and the
> reserved-key-in-node alternative is rejected there for the whole class — §4.3.
> What follows is the range-specific instance, kept because the diff cascade is
> the concrete mechanism and this issue is where it was found.

#### Why a node field cannot work: the diff cascade

The decisive argument is mechanical, not philosophical. Incremental parse
compares whole nodes to decide whether to emit `NodeUpdate`
(`src/beliefbase/base.rs:849` — `new_node != old_node_normalized`), and
`BeliefNode::PartialEq` includes `metadata` (`src/properties.rs:1099`).

So if ranges live in `metadata`, **a single whitespace edit at the top of a file
shifts every subsequent node's range and fires a `NodeUpdate` for every later
node in that file.** Event volume becomes proportional to file length rather than
to the size of the edit — which destroys the value of the very mechanism Issue 66
is building. This is not a tuning problem; it is what the design would mean.

Excluding `metadata` from `PartialEq` is not an escape either: `metadata` also
carries `_content_hash`, git status (`src/codec/builder.rs:1005-1008`), and
`content_profile` (`:1978-1983`), and a `_content_hash` change genuinely *is* a
content change that must produce a `NodeUpdate`.

#### The design: a `(path, file_hash)`-anchored side table

> **Under Issue 110 this table is an overlay-carried annotation on the document
> node**, not a bespoke structure. Its anchor — `(path, file_hash)` rather than
> `(bid, version)` — is one of the cases Issue 104's primitive census must
> accommodate, and is why the anchor is likely an enum rather than a fixed pair.
> The shape below is unchanged either way; what changes is who stores it.

Ranges live outside `BeliefNode`, in a per-document table keyed by the source
file *and the version of that file they were computed from*:

```
DocumentPositions {
    path:      PathBuf,
    file_hash: <Issue 66 source_hashes entry>,   // the validity key
    ranges:    Vec<(Range<usize>, Bid)>,          // sorted, disjoint
}
```

The `file_hash` is the whole point, and it is what distinguishes this from making
ranges merely *ephemeral*:

- **On lookup**, compare the stored `file_hash` against the current file's hash.
  Match → the ranges are valid, serve them. Mismatch → discard and reparse.
  A range is never served against source it was not computed from.
- **On hydration from shards** (Issue 66), the same comparison decides whether
  persisted ranges are usable. This **preserves the benefit** that argued for
  storing ranges in `metadata` in the first place: an LSP attached to a
  hydrated-from-shards graph can answer hover without a reparse, *when the files
  have not changed*. Purely in-memory ranges would forfeit that.
- **Merge semantics dissolve.** The table is keyed by path, so two nodes from two
  files never contend for one range slot. The `BeliefNode::merge` and
  `BeliefGraph::union` questions OQ6 raised simply do not arise.
- **`Clone` no longer carries stale positions.** Cloning a `BeliefNode` clones no
  range, so a range cannot outlive its parse by accident.
- **Cache eviction is automatic.** A resurrected range whose file has changed
  fails the hash comparison rather than needing a bespoke eviction hook.

**Scope**: repo-scoped in the annotation model's terms (`Issue 105`) — durable
across restarts, local to this checkout, never promoted to a shared store. Note
that Issue 105's scope machinery is not yet built; this issue may land the table
as a plain compiler-side structure and adopt 105's store when it exists. What is
not negotiable is the `(path, file_hash)` anchor.

#### What this leaves for the metadata refactor

This issue removes one prospective `metadata` key. The broader question — that
`metadata` currently mixes derived observations (git, layout, content profiles)
with parse-time source-directive caches (`_query_specs`, `_maps_to_specs`) — is
**not** this issue's to settle. See `docs/design/identity/content_versioning.md` §5.1a.
This issue must only avoid adding to the pile.

### Decision 3: Range → BID by binary search, not an interval tree

Issue 11 proposed a `PositionIndex` holding an `IntervalTree<Range, Bid>`. This is
over-built for the actual need.

Within a single document, node ranges are **non-overlapping** and **naturally sorted
by start offset** — the codec emits them in source order. Finding the node at a given
offset is therefore a `partition_point` over a `Vec<(Range<usize>, Bid)>`: O(log n),
no tree, no crate dependency, and the vector is built by pushing during the parse walk
with no sort step. For a document with a few hundred nodes the difference is not
measurable; the difference in code to maintain is.

An interval tree only earns its keep if ranges nest or overlap. Two cases could
introduce that later: section nodes containing their child headings (if we choose to
give a section the range of its whole subtree rather than its heading line), and
inline anchor nodes inside a paragraph. Note the interval tree as the escape hatch,
and add a debug assertion that ranges are disjoint and ascending so the day the
invariant breaks is the day we find out.

### Decision 4: Retire the `BeliefBase.diagnostics` stepping-stone

Issue 11 OQ6 describes `BeliefBase.diagnostics` as `SharedLock<Vec<String>>` that
"loses type structure and source position." Half of that is stale: the field is
already `SharedLock<Vec<ParseDiagnostic>>` (`src/beliefbase/base.rs`), so type
structure is intact. What remains true is the position loss — the three collision
warnings pushed from `insert_state` are constructed with `ParseDiagnostic::warning`
and carry no location, so the LSP cannot place them in the editor.

The wiring is: `insert_state` pushes warnings into `self.diagnostics`;
`GraphBuilder::push` drains them via `doc_bb.drain_diagnostics()` into the compiler's
result set. Once nodes carry ranges, `push()` knows the range of the node it just
inserted and can attach it to each drained diagnostic — at which point the field is a
plain per-`BeliefBase` accumulator with no positional gap, and the "stepping stone"
framing retires. Whether the field itself is removed or simply stops being a known
compromise is an implementation call; the *defect* it represents is closed here.

## Implementation Steps

1. **Define the anchored position table** (0.5 days)
   - [ ] `DocumentPositions { path, file_hash, ranges }` as specified in Decision 2
   - [ ] Source the `file_hash` from Issue 66's per-file `source_hashes` — do not
         introduce a second file-hashing path
   - [ ] `validate(current_hash) -> bool`; every accessor goes through it, so a
         stale table cannot be read by mistake
   - [ ] Document explicitly that no range is stored on `BeliefNode`

2. **Thread byte ranges from `MdCodec` into `IRNode`** (0.5 days)
   - [ ] Add `source_range: Option<Range<usize>>` to `IRNode` alongside `source_line`
   - [ ] Populate at the two sites in `md.rs` that already hold the offset range
         (heading end, inline anchor) instead of discarding all but `.0`
   - [ ] Leave `source_line` in place — it is derivable, but the `#L<n>` backlink
         path is unrelated churn

3. **Populate the table during `push()`** (0.25 days)
   - [ ] Append `(range, bid)` to the document's table when the codec supplied a
         range; skip otherwise
   - [ ] Stamp the table's `file_hash` at parse completion
   - [ ] Attach the range to diagnostics drained from `drain_diagnostics()`

4. **Lookup and persistence** (0.75 days)
   - [ ] `node_at_offset(offset) -> Option<Bid>` via `partition_point`
   - [ ] `range_of(bid) -> Option<Range<usize>>`
   - [ ] Both return `None` when `validate()` fails — stale never resolves
   - [ ] Debug assertion: ranges disjoint and ascending
   - [ ] Persist tables alongside shards so a hydrated graph can serve hover;
         discard on hash mismatch at load

5. **Tests** (0.5 days)
   - [ ] Round-trip: `source[range]` for a heading node equals its heading text
   - [ ] `node_at_offset` at range boundaries — first byte, last byte, gap between
   - [ ] `byte_offset_to_location(source, range.start)` matches the existing
         `IRNode.source_line` for the same node (guards against a regression in the
         backlink path)
   - [ ] **A whitespace-only edit at the top of a file produces exactly one
         `NodeUpdate`, not one per subsequent node** — the diff-cascade guard,
         and the test that would have failed under the node-field design
   - [ ] A range table whose `file_hash` does not match the file on disk resolves
         nothing — `node_at_offset` and `range_of` both return `None`
   - [ ] Hydrating from shards with unchanged files serves hover without a
         reparse; with changed files, ranges are discarded rather than served
   - [ ] Codecs that supply no range parse without panicking and report `None`

## Testing Requirements

- Ranges survive the parse → persist → hydrate round trip **and** are correctly
  invalidated when the underlying file changes between the two
- No `BeliefNode` in any test carries a source range in `metadata`
- A document with nested sections and inline anchors satisfies the disjoint-ascending
  assertion, or the assertion is relaxed with a documented reason and the lookup is
  upgraded

## Success Criteria

- [ ] Every markdown-parsed node has a resolvable byte range covering its source span
- [ ] `node_at_offset` returns the correct BID for any offset inside a node
- [ ] **No source range is stored on `BeliefNode`**, and node equality is
      unaffected by position changes
- [ ] A stale range is never served: hash mismatch resolves to `None`
- [ ] Collision diagnostics from `insert_state` reach the compiler carrying a position
- [ ] Issue 11 Step 1 is deletable: nothing in it remains unimplemented here
- [ ] No new dependency added

## Risks

- **Ranges go stale the instant a buffer is edited.** An LSP holds an unsaved buffer
  whose offsets no longer match the last parse. → **Mitigation**: the `file_hash`
  anchor makes staleness *detectable* rather than merely documented — a stale
  table resolves to `None` instead of returning a plausible wrong answer. The LSP
  still reparses on change (Issue 11 uses full-document sync); the anchor is the
  backstop for when it does not.
- **An unsaved buffer has no file hash.** The editor's in-memory text differs from
  disk by definition, so a disk-derived hash will mismatch continuously during
  editing. → **Mitigation**: Issue 11 owns buffer-versus-disk state; the table's
  contract is "valid for the text whose hash this is," and the LSP supplies the
  buffer's own hash when serving against a buffer. Confirm this interface with
  Issue 11 before Step 4.
- **Persisted range tables inflate shards.** → **Mitigation**: two integers per
  node plus one hash per document; measure on a large corpus before assuming it
  matters, and make persistence optional if it does. Note the fallback is
  degraded performance (reparse for hover), never wrong answers.
- **A second file-hashing path diverges from Issue 66's.** → **Mitigation**:
  consume `source_hashes` directly; do not compute an independent hash. If Issue
  66's hash is not yet available, this issue waits rather than forking.
- ~~**Range semantics for section nodes are unspecified**~~ — **Resolved: both.**
  Write-back (Issue 106) wants the subtree; hover wants the heading; they are
  different questions and neither is wrong. This mirrors the hash decision in
  `docs/design/identity/content_versioning.md` §5.5, which reached
  the same two-value conclusion for the same reason — section containment is
  *structural*, so a heading-scoped value cannot express what is beneath it.
  → **Store the heading range; derive the subtree extent** from the child
  ordering rather than storing a second range. The subtree span is contiguous in
  source and its end is the start of the next sibling (or the parent's end), so
  it is computable in O(1) from data already present. Consumers ask for whichever
  they need.

## Open Questions

- ~~Reserved key in `metadata`, or a new ephemeral field?~~ **Resolved: neither.**
  Ranges are anchored side-table data keyed by `(path, file_hash)`. See Decision 2.
- **What hash does the LSP supply for an unsaved buffer?** The table validates
  against a hash of the text the ranges came from; for a dirty buffer that is the
  buffer's content, not the file's. Interface question for Issue 11 — settle
  before Step 4.
- **Does the range table adopt Issue 105's repo-scope store, or stay a
  compiler-side structure?** 105's scope machinery does not exist yet. Recommend
  landing it compiler-side and migrating when 105 lands; the `(path, file_hash)`
  anchor is identical either way, so the migration is a relocation rather than a
  redesign.
- Should non-markdown codecs (TOML frontmatter, tabular) be in scope, or is markdown
  sufficient to unblock Issues 11, 104, and 106? Recommend markdown only; the range is
  `Option`al by construction, so other codecs opt in later at no cost.
- ~~Section range: heading only, or full subtree?~~ **Resolved: store the heading
  range, derive the subtree extent from child ordering.** See Risks, and the
  parallel hash decision in `docs/design/identity/content_versioning.md` §5.5 (the leaf
  collapse, and what it breaks).

## Consumers

- **Issue 11 (LSP)** — hover needs `node_at_offset`; `publishDiagnostics` needs a
  range per diagnostic, converted at the boundary via `byte_offset_to_location`
- **Issue 106 (source write-back)** — needs the byte span to replace when promoting a
  graph edit back into the source file
- **Issue 107 (generalized codec write-back)** — needs the `Bid → Range` mapping to
  locate the span a codec must edit for a node it did not just parse
- **Issue 104 (annotation vocabulary)** — needs a source span to anchor an annotation
  to, so it survives edits elsewhere in the document. Note the parallel: an
  annotation anchored to `(bid, content_version)` and a range anchored to
  `(path, file_hash)` are the same construction at different granularities.

## References

- `src/codec/diagnostic.rs` — `byte_offset_to_location`, `ParseDiagnostic::location`,
  `UnresolvedReference.reference_location`
- `src/codec/md.rs` — `into_offset_iter()` byte ranges already in the event stream
- `src/codec/belief_ir.rs` — `IRNode.source_line`
- `src/codec/builder.rs` — `GraphBuilder::push`, `drain_diagnostics()` call site
- `src/beliefbase/base.rs` — `insert_state` collision warnings, `diagnostics` field
- `src/properties.rs:1073` — `BeliefNode.metadata`; `:1091-1099` — `PartialEq`
  including `metadata`, the mechanism behind the diff cascade
- `src/beliefbase/base.rs:849` — whole-node comparison gating `NodeUpdate`
- `docs/design/annotation/living_corpus.md` §2 — the three-layer model placing observations
  about nodes in Layer 3
- `docs/design/identity/content_versioning.md` §5.1a — the `metadata` classification this
  issue's decision follows
- [`ISSUE_66_INCREMENTAL_PARSE.md`](./ISSUE_66_INCREMENTAL_PARSE.md) — per-file
  `source_hashes`, the anchor's validity key
- [`ISSUE_105_ANNOTATION_SIDECAR_STORE.md`](./ISSUE_105_ANNOTATION_SIDECAR_STORE.md)
  — annotation scopes; the eventual home for the range table
- [`ISSUE_11_BASIC_LSP.md`](./ISSUE_11_BASIC_LSP.md) — Open Question 6, Decision 3
  (superseded by this issue)
