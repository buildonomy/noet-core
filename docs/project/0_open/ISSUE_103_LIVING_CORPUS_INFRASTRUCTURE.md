---
version = "0.1"
title = "Issue 103: Living-Corpus Infrastructure — Source Ranges, Content Hashes, Network Child Order"
---

# Issue 103: Living-Corpus Infrastructure

**Priority**: HIGH — gates Issue 11 (LSP), Issue 106 (write-back), and every
annotation anchor
**Estimated Effort**: 9 days (RELATIVE COMPARISON ONLY) — 2.5 source ranges,
1.5 the node hash family, 1.5 the network `documents` table, 2 the wasm32
builder, 1.5 set-valued edge ownership
**Dependencies**: Requires Issue 66's per-file `source_hashes` (the range
anchor's validity key — see Decision 2) and step 1c (`payload["text"]` a pure
function of file content — a prerequisite for hashing `payload` at all).
Informs Issue 110 step 3 (whether a range is a record or node content is a lane
decision). Blocks Issue 11, Issue 106, Issue 107, Issue 105 (anchors need
`_content_hash`), Issue 74 (join key and, via Part D, a browser-side candidate
graph state).

> **This issue is the infrastructure holding area for the living-corpus
> campaign.** It collects the primitives that several annotation issues need and
> none should own: source ranges (Part A), the content hash family (Part B), the
> network child-order table (Part C), the wasm32 builder (Part D), and
> set-valued edge ownership (Part E). All five are compiler-level with no
> annotation semantics, which is what qualifies them to live here rather than in
> the issues that consume them.
>
> **Parts C and E each fix a live defect and should not wait for the rest.**
> Adding a document to a network currently changes no node's `content_hash`, so a
> receipt on a network node does not stale when its contents change — a false
> negative in the W4 pilot. And two sections claiming the same `{maps_to}` edge
> silently overwrite each other's ownership, so a traceability matrix reports one
> claimant where two exist (Part E).

## Part A — Source Ranges

### Summary

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
  parallel hash decision in `docs/design/identity/content_versioning.md` §5.5
  (when a closure collapses to the content hash).

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

## Part B — The Node Content Hash Family

### Summary

Every annotation anchor is `(QuerySpec, tape_hash)`, and `tape_hash` is a
sorted fold over member nodes' `content_hash`es
(`docs/design/identity/content_versioning.md` §4.3). Nothing computes a per-node
`content_hash` today. Issue 66 computes per-*file* `source_hashes` for skip
logic, which is a different consumer with a different scope (§7.1). This part
owns the per-node family: radius-0 `_content_hash` and the per-kind closures.

`content_versioning.md` §5 is authoritative for the field set and the fold;
where this checklist and that document disagree, the document wins.

### Steps

6. **Per-node `_content_hash`** (0.75 days)
   - [ ] **Requires Issue 66 step 1c** — do not start before `payload["text"]`
         is a pure function of file content
   - [ ] **Requires Issue 110 step 3's lane table** — `payload` must hold no
         derived data before it is hashed. Asset/directory `content_hash`,
         `listing`/`truncated`, and `remote_url`/`branch`/`network_prefix`
         currently sit in `payload` (`src/codec/builder.rs:4344`, `:4493`,
         `:4799-4815`) and must move out first; where they go (node `metadata`
         or a record) is 110's call. The **`sections` table stays in `payload`
         and stays hashed** — it is authoritative for section-heading BIDs, L1
         authoring state in transit, not a cache (`src/codec/md.rs:1223`)
   - [ ] Hash the content-bearing fields per §5.1: `title`, `schema`, `payload`,
         `id`, content-bearing `kind`. **Not** `bid` (identity, not content),
         **not** `metadata` (derived, §5.1a)
   - [ ] **Exclude `BeliefKind::Trace`** (§5.2) — a node hashed while Trace and
         rehashed once complete yields two versions, silently staling every
         annotation on it. Correctness, not optimization
   - [ ] **Name the encoding, and hash a projection rather than the node.**
         `BeliefNode`'s own `Serialize` emits `bid` and `metadata` and uses
         `skip_serializing_if`, so an empty field is indistinguishable from an
         absent one — define a content-bearing projection with fixed field
         order instead. Pin **one** codec: `rmp_serde::to_vec_named` is the
         candidate, since shards round-trip it already and it must match the
         archive form (below). Both in-tree codecs are self-delimiting, so no
         separate length-delimiting scheme is required; TOML carries the
         `preserve_order` hazard (`content_versioning.md` §6)
   - [ ] **`External` is content-bearing** — do not exclude it (§5.2). It is
         load-state only when paired with `Trace`, which already excludes it;
         `External` alone marks asset and directory nodes carrying a real
         fingerprint. Excluding it would drop every asset from hashing
   - [ ] Store as `metadata["_content_hash"]`; persist through shard export
   - [ ] **The canonical serialization used to compute the hash is the
         serialization an archive would store** (Issue 74 §Archive). One form,
         not two — a blob store keyed by `content_hash` whose values were
         serialized differently cannot be verified against its own keys
   - [ ] Determinism test across a process restart **and** a shard round-trip —
         parse → DB → export → hydrate must agree (§6), and the hash must be
         stable **across binaries** (§7.2), so nothing binary-specific enters it

7. **Closure hashes** (0.75 days)
   - [ ] `_section_hash` and the Epistemic/Pragmatic siblings as **a flat set
         fold**: sort the closure's member `content_hash`es and hash the sorted
         sequence (§5.4). Not a Merkle tree — sibling order is derived from
         source text `content_hash` already covers
   - [ ] **Terminate by visited set, not depth.** Closures are exempt from
         `MAX_TRAVERSAL` (§4.4); per-kind acyclicity is reported, not enforced
   - [ ] Prefer implementing via the query API's `TapeFn::Fold { op: Union }`
         over a second traversal (§5.4); the one blocker is `max_hops()`
         clamping, which needs an uncapped variant
   - [ ] Closure collapse (§5.5): a node with no Section sources has a
         `_section_hash` covering exactly what its `_content_hash` covers.
         Assert, and note that an attestation on such a requirement must
         therefore anchor to the Epistemic closure
   - [ ] `_content_hash` sensitivity test: mutating `metadata` leaves it
         unchanged; mutating `title` or `payload` changes it
   - [ ] `_section_hash` sensitivity test: editing a child changes the parent's
         closure hash and leaves its `_content_hash` unchanged — the two must
         not stale together

### Consumers of Part B

- **Issue 105** — every anchor's `tape_hash` is built from these
- **Issue 74** — `_content_hash` as the cross-version join key
- **Codec-regression detection** (`content_versioning.md` §7.2) — a hash
  manifest over a fixture corpus, compared across binaries

## Part C — The Network `documents` Table

### Summary

A document node's `payload` carries a `sections` table naming each child
heading's `bid`/`id`/`schema`, written by `MdCodec::finalize`
(`src/codec/md.rs:2634`). A **network** node carries no equivalent for its
document children. Two consequences follow, and the first is a defect:

1. **Network membership is invisible to content hashing.** Adding, removing, or
   reordering a document changes no node's `content_hash` — the network node's
   payload is untouched. A receipt anchored to a network therefore **does not
   stale when the network's contents change**. That is a silent false negative in
   exactly the W4 case, and it is the same defect `content_versioning.md` §8
   records for document nodes, which the `sections` table is what prevents.
2. **Network children have no authored order.** `net_dir_partition` sorts
   lexicographically and `emit_group` walks that order
   (`src/codec/proto_index.rs:115`, `:317`), so the only way to order documents
   in a network is to rename files with numeric prefixes.

A third consequence matters for the archive design (Issue 74 §Archive), and it
is a pattern rather than an exception list. **Containment order always lives in
the *parent's* payload, never the child's.** A section node's own content says
nothing about which document contains it or where in the order it sits; that is
in the parent document's `sections` table. Epistemic/Pragmatic and `{maps_to}`
differ — those *are* in the citing node's own `payload.text`.

So Section containment is reconstructible from blobs **only when the parent is in
the reconstructed set**, and network → document is not reconstructible at all,
because the network node has no table to hold it. Part C does not patch an
exception; it **completes the pattern**, giving a network node the same role over
its documents that a document node already has over its sections.

Two consequences for a reader reconstructing a scope:

- A tree covering subsections but not their parent document loses their order.
  The `tape_hash` sorts member hashes lexically by design
  (`content_versioning.md` §4.3), so order cannot come from the hash — it comes
  from the parent's table, or from the tree preserving tape order (Issue 74
  §Archive).
- After Part C, "include the parent" is a uniform rule at every level of
  containment rather than one that silently fails at the network boundary.

### Design

Mirror the section mechanism rather than inventing one. `NetworkCodec` wraps
`MdCodec` and delegates `finalize` (`src/codec/network.rs:694`), and
`prepare_proto_relations` (`:376`) already enumerates and filters the child set
— the two integration points exist.

```toml
# index.md frontmatter, authored or injected
[documents.overview]
bid = "0195..."
order = 0

[documents.subnet-a]
bid = "0195..."
order = 1
```

**Written by the codec, authoritative for order.** Like `sections`, the table is
*authoring state in transit*, not a cache of the filesystem: where it names an
order, that order wins; a child absent from the table is appended in
lexicographic order after the named ones. This keeps the table optional — a
network with no table behaves exactly as today — while making a declared order
durable and reviewable in the frontmatter diff.

> **The precedent carries a warning.** Issue 105 flagged `sections` as "the
> boundary where authoring state and cached derivation are hardest to tell
> apart," and deleting it "would destabilize every section BID." A `documents`
> table inherits that: it must be authoritative for *order*, never a second
> account of *membership*. Membership stays derived from the filesystem plus
> `whitelist`/`blacklist`.

### Steps

8. **Emit the table** (0.75 days)
   - [ ] In `NetworkCodec::finalize` (delegating to `MdCodec::finalize`,
         `src/codec/network.rs:694`), build a `documents` table from the child
         set `prepare_proto_relations` already computed and filtered (`:376`)
   - [ ] One entry per child: `bid`, and `order` when authored. Key on the
         child's `id` where it has one, its slug otherwise — mirroring
         `parse_sections_metadata`'s key preference (`src/codec/md.rs:1275`)
   - [ ] Round-trip through `update_or_insert_frontmatter` (`:1167`) so the
         table persists in `index.md`, exactly as `sections` does
   - [ ] Subnet children appear in the table too — a subnet is a Section child
         of its parent network

9. **Consume the order** (0.5 days)
   - [ ] `prepare_proto_relations` assigns `WEIGHT_SORT_KEY` from the table's
         `order` where present; unnamed children keep lexicographic order after
         the named ones
   - [ ] Test: reordering two entries in the table reorders the network's
         children in the rendered nav, with no file rename
   - [ ] Test: a child present on disk but absent from the table still appears
   - [ ] Test: a table entry naming a child that `blacklist` excludes is ignored
         and reported as a diagnostic — the table does not override membership

10. **Confirm the hash gap closes** (0.25 days)
   - [ ] Test: adding a document to a network changes the network node's
         `content_hash` (this is the defect — it currently does not)
   - [ ] Test: reordering documents changes it; editing a document's *prose*
         does not (that is the closure hash's job, Part B step 7)
   - [ ] Confirm a receipt anchored to a network stales on membership change

### Consumers of Part C

- **Issue 105 / the W4 pilot** — without it, a network-scoped receipt is
  silently wrong
- **Issue 74's archive** — makes network→document the last relation derivable
  from blob content, so a content-addressed archive can reconstruct the full
  graph shape
- **Anyone authoring a network** — document order without filename prefixes

## Part D — The wasm32 Builder

### Summary

`src/codec/builder.rs` and `src/codec/md.rs` are gated off the wasm32 build by
`#[cfg(not(target_arch = "wasm32"))]`, but neither produces target-specific
compile errors. The gate is inherited caution. Behind it sit one real barrier and
one mechanical fix; clearing both lets a browser parse content into a graph with
the same codec the server uses.

**Why it belongs here.** A redline is a map of corpus-relative path to content,
parsed by the real `GraphBuilder`
(`docs/design/identity/generational_archive.md` §6.1) — anything else is a second
parse implementation and a determinism-contract violation. Building that
candidate state in a browser is what a static-site redline surface needs. Like
Parts A–C this is compiler-level plumbing with no annotation semantics.

**Design authority**: `docs/design/identity/generational_archive.md` §7.

### What the measurement showed

Compiling with the codec gates lifted (`--target wasm32-unknown-unknown
--no-default-features --features wasm`):

- **All 24 `tokio::fs` errors land in `compiler.rs`.** Zero in `builder.rs` or
  `md.rs` — which corroborates the layering the archive design depends on: the
  compiler is the filesystem layer, the builder is the pure parse layer.
- With `compiler` and `assets` re-gated, one root cause remains:

```
error[E0277]: `Rc<RefCell<BidGraph>>` cannot be sent between threads safely
    --> src/codec/builder.rs:3611:40
     |  BeliefSource::evaluate(&self.session_bb, &mut package).await?;
```

`BeliefBase` is `Rc<RefCell<…>>` on wasm32 and `Arc<RwLock<…>>` natively (the
`SharedLock` alias, `src/beliefbase/base.rs:72-78`), while
`BeliefSource::evaluate` is an `async fn` carrying a `Send` bound. The builder is
single-threaded by nature, so this is a bound too strong for the target rather
than a design conflict.

Secondary: `MdCodec::proto` opens the file itself (`src/codec/md.rs:2196`) and
needs a content-taking entry point. Out of scope: `process_asset_dir`'s
`read_dir` (`builder.rs:4765`) — a redline never reaches it and directory
listing has no browser analogue.

### Steps

11. **Locate and resolve the `Send` bound** (1 day)
    - [ ] Determine whether `Send` is required by `BeliefSource::evaluate`'s
          signature, by the async desugaring, or by the caller's executor — the
          fix differs, and `spawn_local` suffices only in the third case
    - [ ] Apply one of: `#[cfg]` the bound (smallest diff, two signatures to keep
          in step), a `?Send` variant (explicit, duplicated surface), or
          `spawn_local` at the call site
    - [ ] Confirm the native build's public signatures and bounds are unchanged

12. **Content-taking `proto`** (0.5 days)
    - [ ] Add an entry point that takes frontmatter content rather than a path
    - [ ] Keep the existing `File::open` signature for native callers

13. **Ungate and verify** (0.5 days)
    - [ ] Remove the `cfg` from `pub mod builder`, `pub mod md`, and the siblings
          they pull in (`belief_ir`, `myst`, `network`, `proto_index`)
    - [ ] Keep `compiler` and `assets` gated — `compiler.rs` is the filesystem
          layer and has no business in a browser
    - [ ] `cargo check --target wasm32-unknown-unknown --no-default-features --features wasm`
    - [ ] Test: parse a two-file map into a `BeliefBase` on wasm32; assert node
          count and one resolved cross-file link
    - [ ] **Test: hash parity across targets.** The same input parsed natively
          and on wasm32 yields identical `_content_hash` and `_identity_hash` for
          every node. This is the test that matters — if hashes diverge by
          target, a client-side candidate state can never be compared against a
          server-built corpus. A failure here is a determinism-contract defect
          (`codec_determinism_contract.md` G1) worth fixing regardless of WASM

### Risks specific to Part D

- **The `Send` fix leaks into native signatures** → **Mitigation**: the `#[cfg]`
  approach leaves the native bound exactly as it is; assert the native public API
  is unchanged.
- **Hash parity fails across targets** — float formatting, map ordering, or a
  `cfg`-divergent path → **Mitigation**: it is the headline test, not a
  secondary one.

### Open questions for Part D

- Does the browser need `ProtoIndex`, or can a redline's file map substitute?
  A redline names its own files, so the pre-scan may be unnecessary.
- Is a parsed candidate state cheap enough to rebuild per keystroke, or does the
  surface need explicit "preview" semantics?

### Consumers of Part D

- **Issue 74's redline diff** — a candidate graph state parsed client-side,
  which is what makes a static-site redline surface possible rather than
  read-mostly
- **Issue 65 / the collaboration overlay** — the same argument: a browser that
  can parse can compose, not only read
- **The determinism contract** — cross-target hash parity is a G1 check nothing
  currently performs

## Part E — Set-Valued Edge Ownership

### Summary

`WEIGHT_OWNED_BY` holds one value, and two owners claiming one edge silently
overwrite each other. This is a live correctness defect in `{maps_to}`
traceability, and it is the reason the credential edge in Issue 112 cannot
represent more than one attester.

The graph is keyed on `(source, sink)` — `find_edge(src_idx, snk_idx)`
(`BeliefBase::generate_edge_update`) — with one `Weight` per `WeightKind`. Two
sections in different documents declaring the same `(source, sink, kind)` triple
emit two `RelationChange` events for that one edge, and the payload merge
overwrites on key conflict. `GraphBuilder::push_mapping` sets ownership
unconditionally, so the last document parsed wins.

### Why this is a defect and not a limitation

Three consequences, in increasing severity:

1. **Output is parse-order dependent.** Which owner survives depends on document
   traversal order. Two builds of an unchanged corpus can disagree.
2. **The owner memo faithfully records the wrong answer.** `update_relation`
   maintains `owner_edges` correctly across an update — `OwnerEdgeDelta::Updated`
   removes the old weight's brefs and adds the new ones. So
   `graph_for_owner(displaced_owner)` returns an empty graph, and the losing
   claim is absent from the traceability index with no diagnostic.
3. **GC can delete a live claim.** `terminate_stack` clears `owner_index` and
   falls through to `graph_for_owner` for edges whose owning section no longer
   declares them. Once the displaced owner's link to the edge is severed, a
   later single-file reparse cannot distinguish "this section withdrew its
   claim" from "this section's claim was overwritten".

**The compliance consequence is the point.** In a gap-analysis corpus, two
reviewers independently asserting the same coverage is *evidence of agreement*.
`get_maps_to_traceability` exists to answer "who claims to cover this" and
structurally cannot report two claimants.

### The fix follows `WEIGHT_DOC_PATHS`, which is the same migration

`WEIGHT_DOC_PATH` → `WEIGHT_DOC_PATHS` was a single-valued key discovered to be
multi-valued, and its mechanics are the template: `generate_edge_update` already
special-cases that one key to *merge* rather than overwrite, and
`Weight::get_doc_paths` reads plural-then-singular so old shards keep working.
Apply the same shape to ownership.

**Separate the sentinel from the owner list.** `WEIGHT_OWNED_BY` currently means
three things in one field: `"source"`, `"sink"`, or a third-party bref. A *set*
of owners one of whose elements is `"source"` is incoherent. The sentinel is a
statement about which endpoint owns the edge; a bref list is a statement about
which third parties claim it. Only the latter is multi-valued.

### Steps

14. **Set-valued third-party ownership** (1 day)
    - [ ] Add `WEIGHT_OWNED_BY_ALL` holding a **sorted** `Vec<String>` of owner
          brefs; keep `WEIGHT_OWNED_BY` readable for the endpoint sentinel and
          for existing shards
    - [ ] `Weight::get_owners()` reads plural-then-singular, mirroring
          `get_doc_paths`; `Weight::set_owners()` sorts and deduplicates
    - [ ] **Sorted, never append-ordered.** The weight is serialized into shards
          and reachable from hash inputs, so an owner list whose order depends on
          parse order would defeat Part B's determinism tests
    - [ ] Add the key to `generate_edge_update`'s merge branch beside
          `WEIGHT_DOC_PATHS` — union, not overwrite
    - [ ] `BeliefBase::third_party_owner_brefs` already returns an iterator, so
          its signature survives; make it yield every owner
    - [ ] `push_mapping` unions its owner into the existing set rather than
          calling `set`

15. **Propagate to the owner-indexed read paths** (0.5 days)
    - [ ] `owner_edges` is already `bref → Vec<EdgeIndex>` and needs no reshape;
          verify a single edge can be indexed under several brefs and that
          `update_relation`'s delta handling removes only the departing owner
    - [ ] `compute_diff`'s owner resolution picks one owner to attribute the
          edge to. Decide and state the rule: an edge survives while **any**
          owner still declares it, and is GC'd only when the last one withdraws
    - [ ] `collect_output_bids` and `apply_traversal_to_tape` already loop over
          weights collecting owner brefs — confirm they collect all of a set
    - [ ] Test: two documents declaring the same `{maps_to}` triple produce one
          edge with two owners, and `graph_for_owner` returns it for **both**
    - [ ] Test: reparsing one of the two documents with its claim removed leaves
          the edge alive, owned by the remaining claimant
    - [ ] Test: reparsing both with the claim removed GCs the edge
    - [ ] Test: parse order does not affect the resulting owner set

### Risks specific to Part E

- **Shard format change.** The weight payload round-trips through msgpack, so
  this interacts with Issue 66 hydration. → **Mitigation**: the plural key is
  additive and `get_owners` falls back to the singular, exactly as
  `get_doc_paths` does; no existing shard becomes unreadable.
- **The incidence is unmeasured.** Whether real corpora contain duplicate
  `{maps_to}` claims is unknown. → **Mitigation**: count before building — scan
  a large application corpus for `(source, sink, kind)` triples claimed by more
  than one owner. Zero means this is latent; a large number means existing
  traceability matrices are under-reporting and Part E should lead the issue.

### Consumers of Part E

- **`{maps_to}` traceability** — the immediate defect; two reviewers claiming one
  coverage relation is agreement, and must read back as two claims
- **Issue 112's credential edge** — a role grant attested by several peers is the
  same shape. Issue 112's web-of-trust model is unimplementable without this,
  since every attester after the first would be overwritten
- **Issue 105's fold** — a projected edge asserted by several records needs the
  same union semantics

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
- `docs/design/annotation/annotation_channel.md` §3 — whether a range or a
  diagnostic is a record or node content is a lane decision
- `docs/design/identity/content_versioning.md` §4.3–§4.4, §5, §8 — the hash
  family Part B implements; §5.1a the `metadata` classification
- [`ISSUE_66_INCREMENTAL_PARSE.md`](./ISSUE_66_INCREMENTAL_PARSE.md) — per-file
  `source_hashes`, the range anchor's validity key; step 1c gates Part B
- [`ISSUE_105_RECORD_STORE_AND_FOLD.md`](./ISSUE_105_RECORD_STORE_AND_FOLD.md)
  — the store whose anchors consume Part B; the eventual home for the range table
- [`ISSUE_110_ANNOTATION_CHANNEL.md`](./ISSUE_110_ANNOTATION_CHANNEL.md) — step
  3's lane table gates Part B step 6
- [`ISSUE_11_BASIC_LSP.md`](./ISSUE_11_BASIC_LSP.md) — Open Question 6, Decision 3
  (superseded by this issue)
- `docs/design/identity/generational_archive.md` §6.1 (redline as a file map),
  §7 (the WASM boundary) — design authority for Part D
- `docs/design/codecs/codec_determinism_contract.md` G1, G2 — the parity Part D
  step 13 tests
- `src/codec/mod.rs` — the target gates Part D removes
- `src/properties.rs` — `WEIGHT_OWNED_BY`, `WEIGHT_DOC_PATHS`,
  `Weight::get_doc_paths` (the plural-then-singular precedent Part E follows)
- `src/beliefbase/base.rs` — `generate_edge_update` (the payload merge and its
  `WEIGHT_DOC_PATHS` special case), `third_party_owner_brefs`, `graph_for_owner`,
  `update_relation`'s `OwnerEdgeDelta`
- `src/codec/builder.rs` — `GraphBuilder::push_mapping` (the unconditional
  ownership write), `owner_index`, `terminate_stack`'s GC fall-through
- [`ISSUE_112_CREDENTIALS_AND_PROMOTION.md`](./ISSUE_112_CREDENTIALS_AND_PROMOTION.md)
  — the credential edge Part E unblocks
- `src/beliefbase/base.rs:72-78` — the `SharedLock` alias that diverges by target
- `src/codec/builder.rs:3611` — the failing call; `src/codec/md.rs:2196` — the
  frontmatter read
