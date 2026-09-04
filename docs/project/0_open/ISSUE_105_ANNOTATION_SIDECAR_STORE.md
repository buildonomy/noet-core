---
version = "0.1"
title = "Issue 105: Annotation Sidecar Store"
---

# Issue 105: Annotation Sidecar Store

**Priority**: HIGH
**Estimated Effort**: 5.25 days (RELATIVE COMPARISON ONLY) — 3 for the store,
+1.5 for the node hash family (radius-0 `_content_hash` **and** the closures,
moved here from Issue 66), +0.75 for the `payload`/`metadata` reclassification
that hashing depends on (see Record schema §)
**Dependencies**: Requires the event/record schema decision (`docs/design/core/beliefbase_architecture.md` §4.3); **Requires Issue 110 (parse as an annotation server) — it settles overlay-application semantics and where observations live, which this issue's store and step 3a-pre both depend on**; Requires Issue 102 (`noet serve`); Requires Issue 66 **step 1c only** (`payload["text"]` as a pure function of file content — a prerequisite for hashing `payload` at all). Blocks Issue 104 (annotation vocabulary), Issue 65 (attestation server as sync peer), Issue 109 (the records a run brackets live here), Issue 74 (cross-version diff — consumes `_content_hash` as its join key).

> [!IMPORTANT]
> **Issue 110 changes this issue's shape and must land first.**
>
> 110 reframes a parse run as an **annotation server instance**: compiler
> observations (diagnostics, `git`, `source_url`, layout, hashes, ranges) are
> annotations, not node fields. Three consequences land here:
>
> 1. **This store is not annotation-only.** It becomes the store for *all*
>    observations, at three scopes — in-memory (parse), local (browser/repo), and
>    shared (collab). The three-scope model below already anticipates this; what
>    changes is that the compiler is one of the actors writing into it.
> 2. **Step 3a-pre's destination is in question.** It currently plans to move
>    derived keys from `payload` **into `metadata`**. Under 110 the destination
>    may be the **overlay**, with `metadata` shrinking or disappearing from the
>    corpus node. Do not execute 3a-pre until 110 step 2 confirms the target.
> 3. **Envelope semantics apply to observations too.** A compiler observation
>    carries `actor`, `observed_at`, and `caused_by` like any record. This closes
>    the provenance gap that made `metadata` merge semantics unanswerable (see
>    `.scratchpad/merge_metadata_drop.md` §3), and it means the store must accept
>    a non-human `actor`.
>
> **The projection's form is specified in
> `docs/design/annotation/overlay_model.md`** and ratified by 110 step 1. It is
> not a `BeliefGraph`; what this issue hands records to is a read-through
> overlay. That does not change what this issue *builds* — it reads records and
> folds them — only the target.

> **This issue owns the per-node content hash family**, including radius-0
> `metadata["_content_hash"]`. It moved here from Issue 66 when 66's scope was
> narrowed to per-file hashes; this is the first issue in the sequence that
> actually needs it. See the note in §Record schema.

## Summary

Annotations, sign-offs, and redlines need somewhere to live
before any server exists. This issue delivers the local-first persistence layer:
an append-only, file-per-record store inside the corpus, hydrated into the live
graph at startup and updated as records arrive.

The store is deliberately boring. One immutable record per file, filename
derived from a globally-unique `EventId`, no index, no database, no merge logic.
That shape is what makes multi-writer convergence a non-problem rather than a
subsystem.

## Goals

- A `.noet/annotations/` sidecar store: file-per-record, immutable, append-only,
  TOML, prefix-sharded by BID
- Three-scope resolution (repo / user / shared) with **union** record semantics
  and precedence confined to write targeting and derived-state conflicts
- Hydration into the live in-memory graph as `Envelope`-wrapped
  `Event::Annotation`, projected via `attestation_fabric.md` §12.3
- Derived state (`is this todo open?`, `is this node signed off?`) computed as a
  fold over the record set, never stored
- Zero required infrastructure: annotation works with no server and no network

## Architecture

### File-per-record, immutable, append-only

One record per file. The filename is derived from the record's `EventId`, which
per `docs/design/core/beliefbase_architecture.md` §4.3 is `(actor, sequence)` —
globally unique with no coordination. The directory is sharded by BID prefix,
mirroring git's object store, so a corpus with tens of thousands of annotations
does not produce one flat directory:

```
.noet/annotations/<bid-prefix>/<event-id>.toml
```

Two concurrent writers cannot produce the same filename, so their write sets
never overlap. The store is therefore a **G-Set** — a grow-only set CRDT — and
merge is directory union. Conflicts are impossible by construction, not by
resolution policy.

This is the entire reason Automerge (Issue 16) is not needed for Phase 1.
Conflict-freedom here follows from the schema, not from a CRDT library.
Automerge earns its place only for character-level concurrent text editing,
in-place record mutation, or efficient delta sync at scale — none of which are
Phase 1 concerns. Issue 65's decision that *revocation is prospective, not
retroactive* is what keeps records immutable, and is load-bearing for this
property.

### Gitignored by default

`.noet/` is added to the corpus `.gitignore`.

**State this plainly, because it is the sharpest edge in the design: a fresh
clone has no annotations, and a CI run has no annotations.** The sidecar is a
**working store, not a ledger.** Nothing about a green CI build says anything
about whether a node was reviewed.

Durability is opt-in, two ways, neither required for single-user local work:

- **`git init` inside the annotation directory**, making it its own nested repo.
  Because the store is a G-Set, `git push` / `git pull` between peers is a
  zero-infrastructure multi-writer sync protocol — union merge is exactly what
  git does with non-overlapping file sets.
- **The Issue 65 collaboration server as a sync peer**, replicating records
  bidirectionally rather than owning them.

### Three-scope precedence, modeled on git config

| Scope | Location |
|---|---|
| repo | `<corpus>/.noet/annotations/` |
| user | `~/.config/noet/annotations/` |
| shared | operator-configured path (e.g. a shared or synced directory) |

**The critical subtlety, which is where this design diverges from git config:**
git config precedence is *override* semantics — the narrowest scope wins and
shadows the broader ones. Annotation records are *union* semantics. If a narrow
scope shadowed a broad one, records would be **silently lost**.

Therefore:

- **The record set is always the union of all three scopes.** Records are never
  hidden by precedence. This is not negotiable; it is what preserves the G-Set.
- Precedence governs only two things:
  1. **Which scope new writes land in** by default.
  2. **Which scope wins when derived state genuinely conflicts** — e.g. two
     `{reviewed}` records for the same `(bid, version)` from the same actor. The
     narrower scope is authoritative for the derived answer; both records remain
     in the set and both remain readable.

A `--scope <repo|user|shared>` flag and a matching config key select the write
target.

Motivating case for the shared scope: a user who annotates across several
corpora wants **one** syncable annotation store. Per-corpus scoping alone cannot
provide that — the records would be scattered across repos the user may not own.

### Record schema

The record is the `Envelope` + `Annotation` from
`docs/design/core/beliefbase_architecture.md` §4.3. **This issue defines no new
schema.** The fields the store itself keys on:

| Field | Used for |
|---|---|
| `EventId` (`actor`, `sequence`) | filename; uniqueness |
| anchor `(bid, version)` | prefix shard directory; fold grouping (`version` = `content_hash` or `section_hash` per `protocol_id`, below) |
| `protocol_id` | record kind dispatch during the fold |
| `caused_by` | close / revoke chains; provenance |
| `actor` | scope-conflict resolution |
| `lamport` | **fold ordering key** — see below |

Serialization is TOML — consistent with the rest of the corpus, human-readable,
and diffable, which matters when the durability path is a git repo.

**`version` is a `sha256` content hash over the node's non-metadata content** —
`kind`, `title`, `schema`, `payload`, `id`; excluding `bid` (that is the other
half of the anchor) and `metadata` (git status, layout coordinates, and content
profiles all change without the node's meaning changing). Settled in
`docs/design/identity/content_versioning.md` §5.1; this issue consumes the definition
rather than restating its rationale.

> **This issue owns the version-anchored node reference.** The `(bid,
> content_version)` pair above — call it `NodeVersionRef` — is a named type, not
> two loose fields, because every record this store keys on uses it and other
> issues cite it: Issue 109's `RunStart` names the procedure template it ran
> against with exactly this reference (it was formerly Issue 17's `TemplateRef`,
> which is withdrawn — the anchor is general, not procedure-specific). Two
> components only; nothing else belongs on it.

**There are two hashes, and `protocol_id` selects which one a record anchors
to.** Section containment is *structural* — a parent's own fields do not change
when its children do — so a content hash alone would report "unchanged" for a
section whose entire body was rewritten. Every node therefore carries:

| Hash | Radius | Covers | Anchors the claim |
|---|---|---|---|
| `content_hash` | 0 | the node's own fields | "I reviewed this heading/node" |
| `section_hash` | ∞, Section kind | Merkle over itself + Section-edge sources, in pathmap order | "I reviewed this section" |

For a node with no Section sources the two are equal by construction — a leaf's
`section_hash` *is* its `content_hash`. The store needs no special case for this;
it is the correct semantics, not a degenerate one.

The store keys on whichever the record's `protocol_id` designates; both are
opaque strings to the store.

> **This issue computes the node hash and its closures — the whole family.**
>
> **Including radius 0.** `metadata["_content_hash"]` was previously expected
> from Issue 66, but Issue 66's scope is now **per-file `source_hashes` only** —
> skip logic never needed a per-node hash, and carrying one made the issue that
> gates everything else larger than it had to be. This issue is the first in the
> sequence that genuinely requires a per-node hash, so it owns it.
>
> Every member belongs here: `_content_hash` (radius 0, no traversal) and the
> closures (`_section_hash`, plus the Epistemic and Pragmatic siblings), per
> `../../design/identity/content_versioning.md` §5.1 and §7.1.
>
> The split from Issue 66 is by consumer. A closure recurses to fixpoint and
> crosses subnet boundaries, so editing one leaf changes every ancestor's hash to
> the network root. That is right for anchoring a claim and wrong for skip logic,
> which exists to avoid exactly that cascade. Node hashing is annotation
> infrastructure; only this issue knows which `protocol_id` needs which scope.
>
> **Two other consumers inherit from here**, and neither should compute its own:
> Issue 74 (cross-version diff) uses `_content_hash` as a join key, and the
> codec-regression harness (`content_versioning.md` §7.2) compares it across
> binaries. Both are downstream of this issue rather than of Issue 66.
>
> **Prerequisite: Issue 66 step 1c.** `payload["text"]` must be a pure function
> of file content before anything hashes `payload`. It is conditionally written
> today (`src/codec/md.rs:2380-2384`), so on a steady-state re-parse the stored
> text is a fossil of an earlier parse. Hashing a field that depends on parse
> *history* is unsound — do not start this work until 1c has landed.
>
> **Two specification gaps remain open** in `content_versioning.md` §8 and must
> be closed here: the hash input encoding (it must be length-delimited, or
> `title="ab",id="c"` and `title="a",id="bc"` collide), and whether
> `BeliefKind::External` is content-bearing for hashing purposes. §5.2's
> exclusion of `BeliefKind::Trace` is already settled and is a hard correctness
> requirement — hashing it stales every annotation on shard hydration.
>
> Concretely this issue inherits from `content_versioning.md` §5.3–5.4:
> stratification by `WeightKind` (`as_subgraph`, `src/beliefbase/graph.rs:164`),
> the **visited-set requirement** (invariant 0 is reported, not enforced —
> `base.rs:1350-1360` — and same-kind Section cycles occur in practice, per
> `PathMap`'s `loops` guard at `src/paths/pathmap.rs:1852`), `pathmap_order`
> (`src/paths/pathmap.rs:476`) as the Section fold order, and the requirement to
> specify a fold order for Epistemic and Pragmatic. Storage follows Issue 66:
> `metadata`, underscore-prefixed.

**The store must therefore treat the anchor hash as an opaque tagged value, not
as one of exactly two known kinds.** Record which hash kind the anchor refers to
as an explicit field rather than inferring it from the record's shape, so
additional kinds do not require a format migration. This is cheap now and
otherwise becomes a rewrite of every stored record.

Two implementation consequences land here:

- The hash must be **stable across processes and platforms** — fixed field order,
  canonical serialization, no map-iteration-order dependence. A test asserting
  that the same node hashes identically across separate runs is required; without
  it, every restart silently stales every annotation.
- Do **not** implement the hash by reusing `BeliefNode`'s `PartialEq` field set.
  Equality includes `metadata` (`src/properties.rs:1099`) and the version hash
  must not. The two answer different questions and the divergence is deliberate.

**`lamport` carries an exercise obligation.**
`docs/design/core/beliefbase_architecture.md` §4.3 rejects carrying it as inert
forward-compatibility data. A logical clock is only correct if every
writer maintains it, and a single-writer deployment never tests that — the
failure is silent, and surfaces only when a second writer arrives and the
ordering turns out to be garbage.

This issue is where the obligation is discharged: **the derived-state fold orders
by `(lamport, observed_at, id)`, never by wall clock alone.** On a single writer
`lamport` is a monotonic counter, so the resulting order is checkable against
insertion order — which makes the field self-verifying under ordinary use rather
than on the day it first matters. See Testing Requirements for the assertion.

The counter must survive process restart (persist the high-water mark alongside
the store, or recover it by scanning the max `lamport` at load). A counter that
resets to zero on restart produces duplicate ordering keys and silently breaks
the property.

### Hydration into the live graph

Loading the sidecar is a fold over the record set that produces `BeliefEvent`s
via the `attestation_fabric.md` §12.3 projection. Issue 102 owns the projection
step itself; this issue owns reading the records and handing them over.

**What the projection lands in is an overlay** — applied on top of the compiled
graph rather than written into it. `docs/design/annotation/overlay_model.md` is
authoritative for its shape; `living_corpus.md` §2 for the layer model it serves.

**What this issue builds is independent of that decision**: it reads records and
folds them. Only the target of the fold's output is 110's to settle — so do not
harden against any particular application mechanism here.

**Architectural observation worth stating explicitly**: this is the same
durable-store → live-projection pattern as Issue 66 (shards → in-memory DB) and
Issue 16 (Automerge logs → SQLite indices). Three instances of one shape.

**Scope, not system.** Issue 65's shared server is this mechanism instantiated at
a broader scope — same records, same fold, same union — not a separate system
this one hands off to. The three scopes here and a peer's scope are members of
one union (`federated_belief_network.md` §1.2).

### Derived state is a fold

"Is this todo open?" and "is this node signed off?" are computed by folding the
record log, not stored anywhere. A close or a revoke is a **new record** citing
the prior one via `caused_by` — never an edit, never a delete. This is what makes
the store append-only in practice and not just in intent.

## Implementation Steps

1. **Store layout and writer** (0.75 days)
   - [ ] `.noet/annotations/<bid-prefix>/<event-id>.toml` path derivation
   - [ ] Atomic write (temp + rename) of a single record
   - [ ] Append `.noet/` to the corpus `.gitignore` on first write
   - [ ] Reject any code path that mutates or deletes an existing record file

2. **Scope resolution** (0.75 days)
   - [ ] Discover repo / user / shared scopes; missing scopes are empty, not errors
   - [ ] Reader returns the **union** across scopes
   - [ ] Derived-state conflict resolution by scope narrowness
   - [ ] `--scope` flag and config key for the write target

3. **Reader and fold** (0.75 days)
   - [ ] Enumerate and deserialize records; malformed record warns and is skipped
   - [ ] Group by anchor; fold `caused_by` chains into derived state
   - [ ] Expose "open todos", "sign-off status" queries over the fold

3a-pre. **Reclassify `payload` and `metadata`** (0.75 days)

   > **Blocks 3a. Requires Issue 110 step 2 first — do not start without it.**
   > §5.1 hashes `payload` **wholesale** and excludes `metadata` wholesale. Both
   > are only sound if each table holds one kind of thing, and neither currently
   > does. Hashing before this lands means hashing derived data — including, on
   > every asset node, a node's own hash.
   >
   > **The destination below says "→ `metadata`", which Issue 110 may change to
   > "→ the overlay".** The *classification* is settled and survives either way;
   > only the target moves. Read 110 step 2's outcome before executing, and treat
   > "move to `metadata`" as the fallback if 110 concludes observations stay on
   > the node.

   `content_versioning.md` §5.1a is authoritative for the classification. The
   rule: **`payload` is for content whose authoritative record is itself;
   anything cached, computed, or observed belongs in `metadata`.**

   - [ ] **Move out of `payload` → `metadata`** (derived, must not be hashed):
         asset/directory `content_hash` (`src/codec/builder.rs:4344`, `:4493`,
         `:4799`), `listing` / `truncated` (`:4800-4805`), and
         `remote_url` / `branch` / `network_prefix` (`:4813-4815` — same class as
         `metadata["git"]`)
   - [ ] **Leave in `payload`, keep hashed**: `text`, and the **`sections`
         table** — that table is *authoritative for assigning section-heading
         BIDs*, i.e. L1 authoring state in transit, **not** a cache. Read
         `parse_sections_metadata` (`src/codec/md.rs:1223-1226`) before touching
         anything here; it is the boundary where authoring state and cached
         derivation are hardest to tell apart, and deleting it would destabilize
         every section BID.
   - [ ] **Leave in `metadata`, keep excluded**: the observations (`git`,
         `content_profile`, `assembly_index`, `render_position`,
         `structural_weight`, `structural_depth`).
   - [ ] **Leave in `metadata` — but note they are *not* observations**: the
         parse-time directive caches `_query_specs`, `_query_texts`,
         `_query_options`, `_maps_to_specs` (`md.rs:2500-2528`). These are a
         parse of `{maps_to}` and query directives written *in source*, consumed
         at parse time by the HTML generator (`compiler.rs:4669`, `:4753`,
         `:4759`). **Do not move them to the annotation layer** — that would make
         an L2 parse-time operation depend on L3, so the corpus would not render
         without the annotation server. Excluding them from the hash is correct
         for a different reason: the directive *text* in `payload["text"]` is the
         source of truth and is already hashed, so hashing the cache would
         double-count and would couple node versions to a cache's encoding.
   - [ ] Test: no node carries a derived value in `payload` after the move;
         asset nodes still resolve their content-addressed output paths
         (`compiler.rs:5026-5051`) and dedup still works (`:5084-5096`)

3a. **Per-node `_content_hash`** (0.5 days) — *moved from Issue 66*

   > **Requires Issue 66 step 1c and step 3a-pre above.** Do not start before
   > `payload["text"]` is a pure function of file content, and before `payload`
   > holds no derived data. `content_versioning.md` §5.1-§5.2 is authoritative
   > for the field set; where this checklist and that document disagree, the
   > document wins.

   - [ ] Hash the content-bearing fields per §5.1: `title`, `schema`, `payload`,
         `id`, and content-bearing `kind`. **Not** `bid` (identity, not content)
         and **not** `metadata` (derived — §5.1a)
   - [ ] **Exclude `BeliefKind::Trace`** (§5.2) — a node hashed while Trace and
         rehashed once complete yields two versions, silently staling every
         annotation on it. Hard correctness requirement, not an optimization
   - [ ] **Close §8's two open gaps**: length-delimit the hash input encoding, so
         `title="ab",id="c"` and `title="a",id="bc"` cannot collide; and decide
         whether `BeliefKind::External` is content-bearing
   - [ ] Store as `metadata["_content_hash"]`; persist through shard export
   - [ ] Determinism test across a process restart **and** a shard round-trip —
         parse → DB → export → hydrate must agree, not merely repeated in-memory
         hashing (§6). The hash must also be stable **across binaries**
         (§7.2), so nothing binary-specific may enter the input

3b. **Closure hashes** (0.5 days)
   - [ ] `_section_hash` and the Epistemic/Pragmatic siblings, per §5.3-§5.4
   - [ ] Visited-set on the recursion — same-kind Section cycles occur in real
         corpora and `PathMap` already guards for them
   - [ ] Leaf collapse (§5.5): a Section leaf's `_section_hash` equals its
         `_content_hash`. Assert it, and note the consequence for open item E

4. **Hydration into the live graph** (0.5 days)
   - [ ] Emit `Envelope`-wrapped `Event::Annotation` for Issue 102 to project
   - [ ] Startup warning when the annotation dir is neither a git repo nor
         configured with a sync peer

5. **Tests** (0.25 days)

## Testing Requirements

- Two writers with different `actor` values writing concurrently to the same
  anchor produce two files and zero conflicts; the union contains both
- Copying one scope's directory into another and re-reading is idempotent —
  the G-Set property, asserted directly
- A record present only in the user scope is visible when the repo scope also
  has records for the same anchor (the shadowing regression test)
- Two `{reviewed}` records for the same `(bid, version)` from the same actor in
  different scopes: the narrower wins the derived answer, **both** remain readable
- A close record citing a prior todo via `caused_by` flips the folded state without
  modifying or removing the original file
- Hydration emits one `Envelope`-wrapped `Event::Annotation` per folded record,
  in `(lamport, observed_at, id)` order
- A corpus with no `.noet/` directory starts cleanly with an empty record set
- **`lamport` ordering (the exercise obligation)**: for a single writer, the fold
  order by `(lamport, observed_at, id)` matches insertion order — asserted
  **across a process restart**, so a counter that resets to zero fails the test
- **`lamport` ordering under clock skew**: two records whose `observed_at` values
  are out of order relative to their `lamport` values fold in `lamport` order,
  proving the wall clock is not the ordering key
- **`version` hash stability**: the same node hashes identically across separate
  processes and on a rebuild from shards — an unstable hash stales every
  annotation on every restart
- **`section_hash` sensitivity**: editing a child changes the parent's
  `section_hash` and stales a section-anchored sign-off, while leaving the
  parent's `content_hash` — and any heading-anchored note — current. The two
  claims must not stale together.
- **`version` excludes `metadata`**: mutating a node's `metadata` (git status,
  layout coordinates) leaves `version` unchanged and annotations current;
  mutating `title` or `payload` changes it and surfaces them as stale

## Success Criteria

- [ ] Records persist as one immutable TOML file per `EventId`, prefix-sharded
- [ ] No code path mutates or deletes a record file
- [ ] `.noet/` is gitignored by default and documented as a working store
- [ ] Reader returns the union of all three scopes; no record is ever hidden
- [ ] Derived-state fold orders by `(lamport, observed_at, id)`; the single-writer
      ordering test passes across a restart (discharges the exercise obligation
      in `docs/design/core/beliefbase_architecture.md` §4.3 — without this, `lamport`
      is inert data)
- [ ] `--scope` selects the write target; precedence affects only writes and
      derived-state conflicts
- [ ] Derived state is computed by fold; nothing derived is persisted
- [ ] `noet serve` warns once at startup when the annotation directory has
      neither git nor a configured sync peer

## Risks

- **Sidecar and corpus drift when the corpus is edited without the server
  running.** Records are anchored to a `version` that no longer exists.
  → **Mitigation**: records are never *invalid*, only *stale*. Stale records
  surface in the UI as "annotated at a prior version" rather than being hidden
  or discarded. A record that cannot be anchored is still a record.
- **Many small files stress some filesystems.** Tens of thousands of sub-kilobyte
  TOML files in one tree is a known bad shape for directory enumeration.
  → **Mitigation**: BID-prefix sharding, the same fix git applies for the same
  reason.
- **Users mistake a gitignored working store for durable review evidence.** This
  is the risk that produces an audit failure rather than a bug report.
  → **Mitigation**: document it loudly in the user-facing docs, and have
  `noet serve` warn once at startup when the annotation directory is neither a
  git repo nor configured with a sync peer.
- **Scope configuration becomes a support burden.** Three scopes is three places
  a record can fail to appear. → **Mitigation**: a `noet` subcommand that prints
  the resolved scopes and per-scope record counts, so "where did my annotation
  go" is one command.

## Open Questions

- ~~**`version` semantics in the `(bid, version)` key are unresolved and
  blocking.**~~ **Fully resolved** — see the Record schema section above and
  `docs/design/identity/content_versioning.md` §5.1 (what is hashed) and §5.5 (the leaf
  collapse, and what it breaks). `version` is a `sha256`
  hash over the node's non-metadata content; nodes carry both a `content_hash`
  and a Merkle `section_hash`, and `protocol_id` selects which a record anchors
  to. Nothing about the anchor remains open.
- **Which `protocol_id` values anchor to which hash?** The principle is settled
  (heading-scoped claims → `content_hash`; section-scoped claims →
  `section_hash`), but the per-kind assignment is a table someone must write.
  Recommend: `noet:note:v1` and `noet:todo:v1` → `content_hash`;
  `noet:signoff:v1` and `noet:attest:v1` → `section_hash` when the target is a
  section, `content_hash` otherwise. Non-blocking; settle with Issue 104.

  **The `noet:attest:v1 → section_hash` half of that recommendation is wrong and
  must be revisited.** For a requirement *leaf* — the commonest attestation
  target — `section_hash` equals `content_hash` by the Merkle base case (a node
  with no Section sources folds in nothing). An attestation anchored there cannot
  detect that the evidence supporting it moved, only that the requirement's own
  text changed. A verification claim asserts about its *reasoning chain*, so its
  natural anchor is an Epistemic-scoped hash, which Issue 66 does not currently
  compute. See `docs/design/identity/content_versioning.md` §5.5 (the leaf collapse) and
  its §8 Open Questions, where which closure members Issue 105 computes is still
  open — this is a correctness issue, not a preference.
- **Should the scope hierarchy extend downward to an in-memory scope?**
  `living_corpus.md` §6 argues yes: compiler diagnostics, inference findings,
  cursors, and presence are annotations that must never persist, and modelling
  them as a *scope below repo* is cheaper than a parallel ephemeral class. They
  read, project, and render like any other record; they simply have no file.

  Two consequences if adopted: the reader's union gains a fourth scope (trivial),
  and **flush** — promotion from a narrower scope to a broader one — becomes a
  real operation this store must support. Flush is also how federation
  percolation works (`federated_belief_network.md` §1.2), so it should be one
  mechanism, not two. Issue 109 is the likely owner of the semantics; this issue
  owns the scope plumbing.

  Related: a **staleness policy** per `protocol_id` — discard, retain-and-mark,
  or re-derive when the anchor version changes. Diagnostics discard; sign-offs
  retain and mark. Not blocking Phase 1 (all three built-in kinds retain), but it
  determines whether the in-memory scope needs eviction logic.

- **Do custom annotation-kind definitions live in the sidecar?** Annotation kinds
  are stateful and each has its own lifecycle
  (`docs/design/annotation/living_corpus.md` §4); team-defined kinds are a stated direction,
  and their state machines have to be stored somewhere. Two candidates pull
  opposite ways: **source** (a definition is normative — "this is what a hazard
  review *is*" — which by the §5 template/record argument means version control)
  versus **the sidecar** (definitions travel with the annotations, and a reviewer
  without commit access can still define a workflow).

  If sidecar: note that definitions are **not** G-Set data. Records union;
  definitions *override*. The three-scope precedence in this issue governs which
  scope a conflicting definition wins in, and that is genuinely override
  semantics — the one place in this design where narrowing a scope hides
  something. Flag it explicitly rather than inheriting the record rule by
  accident.

  Not blocking Phase 1: three built-in kinds ship with built-in tables. Blocking
  the first custom kind.

- **Does the shared scope need a lock or lease** for concurrent multi-process
  writes to the same directory? **Recommend: unnecessary.** Filenames are unique
  per `(actor, sequence)`, so concurrent writers never contend for the same file,
  and atomic temp+rename covers partial writes. Revisit only if a real
  multi-process shared-directory deployment appears.
- **Garbage collection and retention.** An append-only store grows without bound.
  A long-lived corpus with heavy annotation traffic will accumulate records that
  no fold ever reaches (superseded todos, revoked sign-offs). This is a real
  concern, not a hypothetical, but compaction interacts with sync and with
  audit-evidence semantics in ways that deserve their own analysis. **Deferred to
  a later issue**; noted here so it is not discovered in production.
- Should the record kind (`protocol_id`) participate in the directory layout, or
  is BID-prefix sharding alone sufficient? Recommend BID-only — kind-based
  partitioning would fragment the per-anchor fold across directories.

## References

- `docs/design/core/beliefbase_architecture.md` §4.3 — the authoritative
  `Envelope` / `Annotation` schema and the unresolved `version` question
- `docs/design/annotation/attestation_fabric.md` §4.2 (record), §6 (protocol registry),
  §12.3 (record → edge-type projection)
- Issue 102 (`noet serve`) — owns event routing and the §12.3 projection step
- Issue 66 (incremental parse) — supplies **only** per-file `source_hashes` and
  step 1c (`payload["text"]` purity). The per-node hash family moved here
- Issue 104 (annotation vocabulary) — `{todo}` / `{note}` / `{reviewed}`
  directives whose records this store persists
- Issue 17 (procedure codec and steps schema) — compiles the `.procedure`
  templates that records anchored here cite; defines no record type
- Issue 18 (procedure execution — aspirational stub) — records produced by
  whatever it becomes land in this store; it defines no record type of its own
- Issue 65 (attestation server) — this mechanism at shared scope; sync peer, not
  system of record
- `docs/design/annotation/living_corpus.md` §2 — Layer 3's live projection as a held-out
  BeliefBase
- Issue 16 (Automerge integration) — forward-compatible, explicitly **not** a
  dependency
