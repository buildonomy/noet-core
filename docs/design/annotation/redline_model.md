---
title = "The Redline Model: Proposing a Change to the Corpus"
authors = "Andrew Lyjak, Claude"
last_updated = "2026-09-14"
status = "Draft"
version = "0.1"
dependencies = ["living_corpus.md", "overlay_model.md", "generational_archive.md"]
---

# The Redline Model

> [!NOTE]
> **This document describes a target architecture.** The `redline` record kind
> is registered by Issue 104; the candidate-state render is Issue 74's; the
> source exit is Issue 106/107's. None of it is built. §9 maps each element to
> its owner.

## 1. Purpose

A reader who spots something wrong can say so. A **redline** is the record of
that: an annotation whose payload proposes what the content should say instead.

The name comes from editorial practice — marking revisions on a draft rather
than silently producing a new version. The essential property is that the
proposal is **separable from the thing proposed about**: it is attributable,
disputable, and reviewable before anything changes, and it never modifies the
source it concerns.

**A redline is a general annotation kind, not a procedural one.** Any proposed
change to any part of a corpus is a redline: a reviewer's correction to a
requirement, a gap analysis proposing a missing section, an agent's suggested
fix. Procedures *compose* with redlines rather than containing them — §7.

**Scope.** This document owns what a redline is, what shape its payload takes,
how it anchors, and how it is read and exits. It does not own the record
envelope (`beliefbase_architecture.md` §4.3), the store (Issue 105), the diff
machinery ([`../identity/generational_archive.md`](../identity/generational_archive.md)),
or the write path (Issue 106/107).

## 2. A Redline Is a Record Kind

A redline is an ordinary annotation carrying a registered `record_kind` and a
payload schema. It is not a type, not a graph node of its own kind, and not an
entry in a separate log.

Four properties, all inherited from the annotation model rather than designed
here:

- **It is immutable.** A revision of a redline is a *new record citing the prior
  one* via `caused_by`. The current reading is what the fold produces
  ([`living_corpus.md`](./living_corpus.md) §4).
- **It names an actor.** A proposed change with no attributable author is a
  claim nobody made ([`living_corpus.md`](./living_corpus.md) §5).
- **It anchors to what it concerns**, at a content version — so a redline
  against a section that has since moved or changed is detectably stale rather
  than silently wrong (§4).
- **It asserts; it does not mutate.** A redline is a claim requiring
  interpretation, never an instruction the store applies. Assertions project
  into mutations via the fold, and never the reverse
  ([`living_corpus.md`](./living_corpus.md) §4).

**There is no redline node in the corpus.** The proposed content is payload. The
projection creates a node for the *record* — its own BID, owning edges into the
node set it concerns — and nothing else. This is why a redline blocked on an
unanswered question is a fold predicate rather than a graph edge (Issue 105
§Project).

## 3. The Payload Is a Map of File Path to Content

**The unit of storage is the file.** A redline's proposed content is
`BTreeMap<corpus-relative path, content string>`, parsed through the ordinary
`GraphBuilder` to produce a candidate graph state.

> **The file constrains storage, not presentation.** Authoring, diffing, and
> rendering are all section-aware: a surface offers an edit to one section, the
> diff pairs nodes by BID so the delta is per-node, and the viewer marks the
> halo where it intersects rendered prose (Issue 105 step 5). What the
> *record* holds is the file. This is the split git makes between hunks and
> blobs, for the same reason.

Two independent arguments force the file as the storage unit, and they converge.

**Determinism.** Computing hashes for proposed text by any route other than the
real codec is a second parse implementation, which the determinism contract
forbids (`codec_determinism_contract.md` G1/G2). Proposed content must go
through the same machinery, or its hashes are not comparable with the corpus's.

**Anchoring.** A section's identity depends on its document context — its parent
stack, its sort key, its resolved links. Parsing a fragment yields a node whose
identity hash differs from the same text in situ, purely from absent context.
**The file is the smallest unit at which a parse is total**, and therefore the
smallest unit at which the resulting diff is well-anchored. A payload *stored*
at section granularity would be claiming an anchor it cannot compute — whereas a
section-level edit stored as its enclosing file parses in context and anchors
correctly.

[`../identity/generational_archive.md`](../identity/generational_archive.md) §6.1
is authoritative for the payload shape.

> **Why unparsed text is the hard case.** A redline's payload has no BID, no
> content hash, no identity hash, and no edges, because nothing has parsed it.
> The failure is subtle rather than loud: the link-collapse rule resolves each
> link to a target key *when the compiler located the target*, so proposed text
> identical to an existing section can hash differently purely because its links
> were never resolved.

## 4. Anchoring and Staleness

**A redline anchors by path, not by query.** Its payload is a path→content map
(§3), so the anchor is `(path, base_corpus_version)` — the files it proposes to
change, and the corpus generation it was written against. That differs from the
general annotation anchor, `(QuerySpec, tape_hash)`
([`../identity/content_versioning.md`](../identity/content_versioning.md) §4), and
the difference is the point: a query selects what exists, while a redline may
name a file that does not.

The general anchor is unchanged. A kind needing more declares the extra fields in
its **own payload** rather than hoisting them onto the envelope, which is why a
redline's path anchor costs the other three kinds nothing.

**Staleness is a look-time comparison, not a stored flag.** When the anchored
content changes underneath a pending redline, the redline does not become
invalid — it becomes *stale*, and a reader is told so. Nothing is evaluated at
fold time and nothing needs to run when the corpus changes; the comparison
happens when someone looks
([`../identity/content_versioning.md`](../identity/content_versioning.md) §4.4).

This matters more for redlines than for most kinds, because a redline has the
longest natural latency in the system: it can sit in a change package awaiting
an external approval process for weeks while its target moves.

**Staleness is a flag; a three-way diff is the answer.** Because a redline
records the `corpus_version` it was written against, the archive can supply that
generation as a **merge base**
([`../identity/generational_archive.md`](../identity/generational_archive.md)
§6.4). With it, drift that does not touch the proposed span is distinguishable
from a genuine conflict — which matters precisely because of the latency above:
over weeks most movement will be compatible, and flagging all of it as stale is
noise. Without the base generation the comparison degrades to two sides, which
cannot tell a proposal from drift.

**Greenfield needs no special case.** Because a path needs no referent, a new
document is a key absent from the base map and a new network is
`subnet/index.md`; both anchor exactly as a revision to an existing file does
([`living_corpus.md`](./living_corpus.md) §5).

A `gap` record may still *accompany* such a redline — "nothing satisfies this
requirement" is a claim worth making in its own right, and 18 of 25 observed
change plans make it (Issue 104). But it is a **separate claim that a redline may
cite**, not the thing the redline anchors to. The two answer different questions:
a gap says the absence is a defect, a redline says what should fill it.

## 5. Reading a Redline: the Candidate State

A reader needs current content beside proposed content. That juxtaposition is
produced by building a **candidate graph state** and diffing it against the
corpus:

1. Resolve the concerned node set — the halo (`overlay_model.md` §2)
2. Build a candidate package by parsing the proposed files; identity resolves
   through the ordinary key-matching path, so each node pairs with the corpus
   node it revises and carries that BID once paired
3. Compare against current state — `compute_diff` already produces ordered
   `BeliefEvent`s covering properties and relations alike, not only prose
4. Render the delta

**Identity pairing is what makes this a diff rather than two documents.** Both
sides must carry matching `NodeKey`s — not necessarily `NodeKey::Bid`. A candidate
built by parsing proposed source resolves identity the way any parse does,
through `cache_fetch`'s key list (`src/codec/builder.rs:4026`), so a node pairs
by `Path` or `Id` when its BID is not known up front and carries the corpus BID
once paired. A retitled heading therefore still pairs, where BID-only matching
would read it as a delete plus an add.

> **The candidate must never be stored.** It is a rendering artifact: it lives
> in one query package, is discarded after the read, and is never merged back.
> Persisting it would put two accounts of one node's content in a store with
> nothing to reconcile them, and `union_mut`'s wholesale replacement would
> resolve the collision by destroying the base node.
>
> The rule is one-directional: proposed content may be **materialized into a
> package for reading**, and must never be **written to a store**. Nothing in
> the type system distinguishes a safe candidate from an unsafe one, so it must
> be asserted by test (`overlay_model.md` §2.5, §2.6).

**Rendering needs no write authority at all**, which is what makes this useful
in the common case where noet cannot touch the target.

### 5.1 The unit is a set, not a single redline

Several redlines against one target document are read, packaged, and promoted as
**one** unit. The singleton is the degenerate case of the set, not the other way
round.

Three consequences, none of which follow from the singleton case:

- **Byte ranges invalidate after the first edit.** Applying an edit shifts every
  later offset in the file, so edits to one document must be applied in a single
  pass, or sorted descending by offset. Applying them sequentially against
  ranges recorded beforehand is silently corrupting. This bites *enactment*
  (Issue 106) rather than reading — a candidate state is built by parsing whole
  files, so it never holds a stale offset.
- **Conflicting redlines become representable.** Two redlines proposing
  different text for the same span is a real state that a set-based operation
  detects and a one-at-a-time operation hides — each succeeds, last writer wins,
  and nothing says so.
- **State belongs to the package.** Redlines crossing a boundary together
  succeed or fail together.

## 6. Exits: Hand-off and Enactment

A redline proposes a change. Two things can happen to it, and they are different
machinery.

| | Hand-off — the common case | Enactment |
|---|---|---|
| Produces | a rendered change package | a source edit |
| When | another process controls the target | noet holds declared write authority |
| Built by | **Issue 74** — a diff with provenance, wholly on the read side | **Issue 106**, over **Issue 107**'s write-back path |
| Needs write authority | no | yes — declared per network and per codec |

**The loop closes at a hand-off, not at a write.** A redline is a claim about a
*future* state of a document, and the authority to enact it belongs to that
document's owner — usually not noet, and often not the redline's author. What
closes the loop is a rendered package delivered in the form the owning process
consumes: proposed text, rationale, the requirement driving it, and the impact
set ([`living_corpus.md`](./living_corpus.md) §7).

Promotion is therefore **one operation with a variable boundary cost**, not two
operations. What varies is who owns the target — a matrix the author maintains
is a commit; a controlled procedure is a change request with named approvers; an
upstream requirement is a comment and a negotiation. **Codec write-back is the
special case where that boundary is free**, and it is the exit fewest real
targets qualify for.

**An unenactable redline is not a failure state.** It reaches `packaged` and
awaits a human carrying it across — the same shape as a question whose target is
a person. Modelling the hand-off explicitly is what lets the lifecycle represent
"done, awaiting someone else" rather than stalling.

Two properties any exit must preserve:

- **Promotion appends; it does not consume.** A promotion is a record whose
  `caused_by` cites the redline. The redline is unmodified afterwards, because
  no record is ever modified.
- **The summary states what is, not how it got there.** A run that drafted a
  claim, found it wrong, and corrected it promotes the *corrected* claim, not
  the correction history (Issue 105 §Promotion reads the fold).

## 7. Composition with Procedures

Redlines and procedures compose in both directions, and neither contains the
other.

**A procedure can expect redlines.** A step may declare that its discharge *is*
an authored redline — a procedure for drafting a new procedure, or a review
whose output is a set of proposed corrections. The step is a potentialized
annotation and the redline is its actualization
([`living_corpus.md`](./living_corpus.md) §5); nothing about this is special-cased.

**A procedure's execution can motivate redlines.** Comparing what a template
declared against the records that discharged it yields deviation observations,
and a systematic deviation is evidence that the template is wrong. Turning that
evidence into a proposal produces an ordinary redline. That comparison is
[`../procedures/deviation_model.md`](../procedures/deviation_model.md);
its output is an *input* to this document, not a subtype of it.

**Most redlines involve no procedure at all.** A reviewer correcting a
requirement, an agent proposing a fix, a gap analysis proposing a missing
section — none of these has a run, a template, or a marking.

## 8. Design Principles

- **A proposal is not a change.** A redline is separable from its target,
  reviewable before anything happens, and inert until someone acts.
- **Immutable records.** A revision cites; it never edits.
- **The actor is named.** Every proposal is attributable.
- **Rendering before authority.** A redline is useful where it can never be
  enacted; the package is the deliverable.
- **The candidate is ephemeral.** Proposed content is materialized for reading
  and never stored.
- **Store the file, present the section.** Storage granularity is fixed by what
  can compute an anchor; presentation granularity is fixed by what a reader
  needs. They differ, and that is not a compromise.
- **The set is the unit.** The singleton is the degenerate case.

## 9. Architecture Map

| Element | Owner |
|---|---|
| The `redline` `record_kind` and its payload schema | **Issue 104** — registered as `noet:redline:v1` alongside `receipt`, `gap`, and `ask` |
| The observed field set and lifecycle it is derived from | **Issue 17** step 2a — evidence only; it registers nothing |
| The record store and the fold | **Issue 105** |
| The candidate graph state and the diff render | **Issue 74** |
| Move-aware comparison and the archive | [`../identity/generational_archive.md`](../identity/generational_archive.md) |
| Anchoring and staleness | [`../identity/content_versioning.md`](../identity/content_versioning.md) |
| The ephemerality constraint | [`overlay_model.md`](./overlay_model.md) §2.5 |
| Three-way diff against the base generation | **Issue 74**, over the archive (`generational_archive.md` §6.4) |
| Client-side candidate build | **Issue 103** Part D — the wasm32 builder |
| Source enactment | **Issue 106**, over **Issue 107** |
| Deviation observations that motivate a redline | [`../procedures/deviation_model.md`](../procedures/deviation_model.md) |

## 10. Open Questions

1. **What produces the candidate state?** Its *shape* is settled — a candidate
   graph, not a two-payload comparison
   ([`../identity/generational_archive.md`](../identity/generational_archive.md)
   §6.3) — but nothing owns *building* one from a redline. It needs a parse of
   the payload's files against the base corpus, which is Issue 74's second side
   and is echoed in Issue 107's banner.
2. **Client-side parse.** A static-site redline surface would need to build the
   candidate state in a browser. Measured: `builder.rs` and `md.rs` have no
   target-specific errors, and one `Send` bound plus one `File::open` stand in
   the way — **Issue 103 Part D** owns it
   ([`../identity/generational_archive.md`](../identity/generational_archive.md) §7).
3. **Supplanting reorders siblings.** Proposed content may move a section rather
   than only revise it, so a promotion's write set is not always one contiguous
   range, and the rendered diff must say *moved* rather than *removed + added*
   (Issue 74). Authoring new content raises no separate anchoring question — a
   new document is a path absent from the base map, a new network is
   `subnet/index.md` ([`living_corpus.md`](./living_corpus.md) §5).
4. **Partial supersession.** One redline can absorb part of another. Which
   claims moved is a payload concern; how it is expressed is unspecified.
5. **What is the promotion record's `record_kind`?** Promotion is a distinct act
   from the redline it promotes, so it likely wants its own registry entry
   (Issue 106).

## 11. References

- [`living_corpus.md`](./living_corpus.md) §4 (assert vs. mutate; stateful annotations), §5 (conduits; authoring new content), §7 (the loop and the hand-off)
- [`overlay_model.md`](./overlay_model.md) §2.5 — the ephemeral candidate; §2.6 — the `union_mut` constraint
- [`../identity/generational_archive.md`](../identity/generational_archive.md) §6 — the file-map shape and why unparsed text is hard
- [`../identity/content_versioning.md`](../identity/content_versioning.md) §4 — the general `(QuerySpec, tape_hash)` anchor a redline departs from (§4), and §4.4 lazy staleness
- [`../procedures/deviation_model.md`](../procedures/deviation_model.md) — as-run comparison, which motivates redlines
- [`../core/query_model.md`](../core/query_model.md) — how a redline corpus is queried
- `ISSUE_74_CROSS_VERSION_DIFF.md` — the diff and the change package
- `ISSUE_104_ANNOTATION_VOCABULARY.md` — registers `noet:redline:v1`; authoritative for the field set
- `ISSUE_17_NOET_PROCEDURES_EXTRACTION.md` step 2a — derives the observed field set and lifecycle from real records
- `ISSUE_105_RECORD_STORE_AND_FOLD.md` — the store and the fold
- `ISSUE_106_SOURCE_WRITE_BACK.md`, `ISSUE_107_CODEC_WRITE_BACK.md` — enactment
