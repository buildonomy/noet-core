---
version = "0.1"
title = "Issue 104: Annotation Vocabulary — receipt, gap, redline, ask"
---

# Issue 104: Annotation Vocabulary — receipt, gap, redline, ask

**Priority**: HIGH
**Estimated Effort**: 2.5 days (RELATIVE COMPARISON ONLY)
**Dependencies**: Requires Issue 110 (the channel that emits these) and Issue 105
(the store and fold that hold and derive them). **Related**: Issue 17 step 2a
supplies the *observed field sets and lifecycles* for `redline` and `ask`, mined
from 25 hand-written records, and verifies its grammar can express them. **This
issue owns all four registry entries** — `redline` and `ask` are general
annotation kinds, not procedural ones
(`docs/design/annotation/redline_model.md`). Issue 17's directive layer is not a
dependency — none of the four kinds needs an authored lifecycle document.
**Blocks**: the W4 receipt pilot, then the W1 redline pilot.
**Follow-on**: Issue 112 (credentials) extends this registry with a role-annotation kind; it needs the enumeration and transition machinery first.

## Summary

A reader of a compiled network can query it but cannot mark it. This issue
delivers the vocabulary that lets them — as a **rendering and query surface over
records the channel already emits and the fold already derives**, not as a new
storage layer.

The vocabulary is **evidence-backed, in order of demand**:

| Kind | Claim | Source of demand | Truth conditions |
|---|---|---|---|
| **`receipt`** | "I read this at version *v*" | W4 — the need under every other workflow | none; latest wins |
| **`gap`** | "nothing satisfies *P* within scope *S*" | 18 of 25 change plans cite one; already in the corpus as `ext-gap` | **falsifiable** — closed the moment the section exists |
| **`redline`** | "this node should say *X* instead" | 25 hand-written records | closes at hand-off to the owning process |
| **`ask`** | "person *P* must decide *Q*" | 11 drafted asks | expires when answered |

Two of these were derived from the dig-out and independently confirmed:
`review_plan.md`'s eight disposition codes, authored before any of this design,
land on exactly these four kinds (`planning/reference/process_annotation_model.md`
§3). Two independent derivations converging is the strongest evidence the
vocabulary has.

**`{note}` has no demand** and is not in Phase 1. It is a comment with no truth
conditions and no state; nothing in W1–W4 needed one. It stays registrable as a
kind — the registry is open — but nothing here builds it. `{todo}` is W2's small
layer only and is subsumed by `redline` with an empty proposed text.

**`gap` is not a note subtype.** A note is commentary and cannot be wrong; a gap
is a negative existential claim that is *falsified* when its absence ends. That
falsifiability is why closure is derivable rather than authored, and it splits
the kind by **who can close it** (`process_annotation_model.md` §2): a *derived*
gap (closed by recomputation) must never be persisted — a stored absence is
actively false once filled; a *judged* gap (closed by human re-assessment) must
be. Only the judged form is a record here. The derived form is a query result.

This issue is authoritative for the annotation record's field set (Design
Authority below), with `beliefbase_architecture.md` §4.3 supplying the
`Envelope`; Issue 105 persists and folds it; Issue 110 emits it.

## Design Authority

> **This issue is authoritative for the annotation record's field set.**
> `beliefbase_architecture.md` §4.3 carries a *sketch* of the envelope — `id`,
> `actor`, `observed_at`, `caused_by` — explicitly marked as intent rather than
> schema. Settle it here, then update that section to match what was built.
>
> Three questions it leaves open, all of which this issue must answer:
>
> 1. **Is a logical clock needed? No.** Records merge by set union, so ordering
>    only matters to the fold, and the fold orders by `(observed_at, id)`.
>    `sequence` totally orders one session's records; between sessions the
>    records are genuinely concurrent (`collector_model.md` §5.1). `caused_by`
>    carries causality explicitly as a DAG, so a Lamport clock would be a weaker
>    implicit encoding of a fact the records already state. There is no
>    `lamport` field.
> 2. **`caused_by` vs. `provenance`.** `attestation_fabric.md` §4.2a already calls
>    this relation `provenance`. Two names for one thing is the divergence this
>    whole effort exists to prevent — pick one and propagate.
> 3. **Does `task` belong on every record** or only on `RunStart` (Issue 105)?
>    See Open Questions.
>
> Envelopes wrap **annotations only**. `BeliefEvent`s are compiler-generated,
> single-writer, and already ordered by the epoch structure; they carry no actor
> and no clock. Do not widen the envelope to cover them.

## Goals

1. **Validate the `Annotation` field set against every known consumer** — see
   §"The primitive census" below. This issue is authoritative for the field set,
   and the set has never been checked against the full demand.
2. Four `record_kind` values — `noet:receipt:v1`, `noet:gap:v1`,
   `noet:redline:v1`, `noet:ask:v1` — over the unified `Annotation` type; zero
   new schema. `redline`/`ask` field sets come from Issue 17 step 2a's data;
   all four are **general** kinds — none is procedure-specific
3. **A record carries only what its author knows.** Fields derivable from the
   anchor or the target are computed, never authored: `target_shape` from
   whether the anchor resolves, `destination_process` from the target document
   (`process_annotation_model.md` §6.4 — hand-authoring both produced 2 errors
   in 25)
4. A query surface for presence *and absence*, including the compliance gap
   query and "unread or changed since read, under scope *S*, for me"
5. Viewer rendering: per-node badges and the halo in the metadata panel
6. All state derived by the Issue 105 fold, never stored; this issue owns only
   the four built-in transition tables

## `record_kind` is one open enumeration, shared with Issue 108

The field a record carries to say what it is. It resolves through **one
registry** to the code object holding that kind's structure and logic — the
mapping the fold, the query surface, and the projection all look up.

**Issue 108's `RecordSource` registrations belong in the same enumeration.** Its
Decision 5 already describes this registry without naming it: sources are
"configured, not discovered… in the same shape as codec registration," with a
few generic built-ins and anything domain-specific out-of-tree. That is this
registry's shape exactly. Two open registries differing only in what an entry
resolves to is the fragmentation the `Envelope` unification exists to prevent.

**One enumeration, discriminated entries.** The entry's *fields* differ by what
the kind is for:

| Kind family | Example | Entry carries | Owner |
|---|---|---|---|
| **claim** | `noet:receipt:v1`, `noet:gap:v1`, `noet:redline:v1`, `noet:ask:v1` | payload schema, lifecycle/transition table, `caused_by` → `WeightKind` translation, anchor scope | this issue |
| **bracket** | `noet:run-start:v1`, `noet:run-end:v1` | payload schema; no lifecycle of its own | Issue 105 |
| **attestation** | `noet:bounds-check:v1` | the above **plus** a check specification, credential and independence requirements | `attestation_fabric.md` §6 |
| **evidence source** | `ext:ci-runs:v1` | `RecordSource` implementation + connection details; **no** lifecycle | Issue 108 |

Do not give an evidence-source entry a transition table, or a claim entry a
connection string. The discriminant is the family; a consumer that needs a
field the family does not carry has the wrong kind.

**This resolves Issue 108's namespace question.** Its `IMPORTANT` block asks
whether an external record node and a folded annotation node share the reserved
`Record` namespace. They do — both are nodes standing for something that is not
corpus content — and `record_kind` is what distinguishes them. One namespace,
one enumeration, two families.

- [ ] Confirm the family discriminant with Issue 108 before either registers a
      kind; agree the id-prefix convention (`noet:` built-in, `ext:` external
      evidence, `local:<team>:` per-deployment)

## The primitive census

**Do this first.** The `Envelope` sketch in
`docs/design/core/beliefbase_architecture.md` §4.3 was written against annotation
records, then Issue 110 established that *compiler observations are annotations
too*. The field set has not been checked against that widened demand, and a
review of the design docs could not confirm it matches.

> [!IMPORTANT]
> **Field naming follows `dag_model.md` §2 "Directionality", which is
> authoritative**: no `parent` / `child`, and no tree terms. An edge runs
> `source → sink`, meaning the sink derives from the source; `parent_*` inverts
> that reading and `leaf`/`root` inverts it while additionally presupposing a
> tree this graph is not.
>
> Applied to the kinds this issue registers:
>
> | Field | Not |
> |---|---|
> | `enclosing_run` (`RunStart`, Issue 105) | `parent` |
>
> Unbuilt, so the name is free. Check any new field set against the rule before
> adding it.
>
> This binds the field names **this issue registers**. It does not rename
> `Bid::parent_bref` / `adopt_into` (`src/properties.rs`), which describe BID
> *lineage* — a genuine ancestry relation where the word is accurate.

The census below is what this issue must reconcile. Each row is something the
system already banks on holding as an annotation.

| Candidate | Source | Actor | Scope | Anchors to |
|---|---|---|---|---|
| `receipt` | this issue | human | local | `(QuerySpec, tape_hash)` — the document's Section submap |
| `redline` | this issue; field set from Issue 17 step 2a | human, **or the parser** (a source `{todo}`) | local / shared | **`(path, base_corpus_version)`** — a redline's payload is a path→content map, and a path needs no referent, so the 17 whose target does not exist yet anchor the same way as the 8 that do. A `gap` may be *cited* alongside but is a separate claim (`redline_model.md` §4) |
| **judged `gap`** | this issue | human | shared | the requirement node; **must persist** |
| **derived `gap`** | query result, **not a record** | recomputation | none | `(query_identity, query_version, node_bid)` — computable without running the query, deterministic, possibly non-materializing. **Must not persist** |
| `ask` | this issue; field set from Issue 17 step 2a | human | shared | its own run's anchor; it `caused_by`-cites the redline run it blocks, and the addressee is **payload**. The blocking is a fold predicate, **not** a projected edge (`ISSUE_105` §Project) |
| Diagnostics | Issue 103 Decision 4, Issue 110 | compiler | regenerated | node + **source range** |
| `source_url` | `builder.rs` Phase 4 | compiler | shipped | node |
| `git` status | `builder.rs:2478` | compiler | shipped | **network node only** |
| Layout (`render_position`, `assembly_index`, `structural_weight`) | `layout.rs:216-235` | compiler | shipped | node |
| Source ranges | Issue 103 | compiler | repo | **`(path, file_hash)`**, not `(bid, version)` |
| Content / identity hashes | Issue 105, `content_identity.md` | compiler | shipped | node |
| **Cursor / focus** | **Issue 15** | a PII surface | **regenerated, per-consumer** | a *query*, not a node |
| Run brackets (`RunStart`/`RunEnd`) | Issue 105 | any | scope of the run | template ref + `enclosing_run` |

### What the census forces

Four things the current `Envelope` does not obviously accommodate:

1. **The anchor is a `QuerySpec`, not a pair and not an enum.** Every annotation
   anchors to `(QuerySpec, tape_hash)` (`content_versioning.md` §4), evaluated
   lazily; a single node is the degenerate spec `id://x`. Two rows above still
   need care: a source range anchors to `(path, file_hash)` (Issue 103), and a
   **derived gap** anchors to `(query_identity, query_version, node_bid)` — the
   query *constitutes* the finding, so it is in the key, and `query_version` is
   mandatory because otherwise "the corpus moved" and "the query moved" are
   indistinguishable. Whether either is a `QuerySpec` in disguise or a genuine
   second anchor form is this census's first job.
2. **`actor` must admit non-humans.** The compiler is an actor
   (`engineering_model_ontology.md` §3.4). `ActorId` is already described as
   opaque; confirm nothing downstream assumes a person, and decide what a
   compiler actor's identity *is* — "the compiler at version X" would let the
   codec-regression detector (`content_versioning.md` §7.2) attribute drift.

   **`actor` and `session` answer different questions and both are in
   `EventId`** (`core/beliefbase_architecture.md` §4.3). `actor` is *who is
   accountable* and is the unit a reader attributes a claim to; `session` is
   *which process wrote it* and exists only to keep concurrent writers from
   colliding. A parse run is one session of the compiler actor — which is what
   lets "the latest parse" be named without a timestamp comparison, and is worth
   checking against Issue 110's regenerated-observation case.
3. **A record can outlive its author's scope and change hands.** The `{todo}`
   lifecycle is the worked example: **opened by the parser** when it encounters a
   `{todo}` in source, **closed by whoever discharges it** through their PII
   surface, and the fold's effect is a source edit *removing it*. One logical
   record, two actors, terminating in a write-back. Confirm `caused_by` carries
   this without a mutable owner field.
4. **Scope is per-record, and the census spans all four.** Regenerated
   (diagnostics, cursors), shipped (layout, `source_url` — regenerated *and*
   written to the shard export, which is why "in-memory" was the wrong name for
   this scope), repo (ranges, receipts), shared (sign-offs). Whether scope is a field on the record or a
   property of the store it lives in is Issue 105's, but the census is what makes
   it a real question rather than a hypothetical.

### Two candidates that may not be annotations

Recorded so they are ruled in or out deliberately:

- **Directive caches** (`_query_specs`, `_maps_to_specs`) are a *parse of source
  content*, not an observation about a node — `content_versioning.md` §5.1a is
  explicit. They stay node content. Do not sweep them in.
- **Telemetry and test results** are `R` but not *privileged* `R`: they are not
  human-scale and must not be stored per-record. Issue 108 addresses them by
  *reference* (`RecordSource`), which is the boundary that keeps the store from
  becoming a log index. `living_corpus.md` §2 owns the rule.

> **Issue 15 is probably defining the cursor annotation.** Its
> `EventSubscription { id, query, tx }` is a per-consumer, regenerated-scope,
> mutable-by-its-owner filter over the graph — which is an annotation subtype,
> not a bespoke registry. If so, "every PII surface has a focus filter it fully
> owns" is a *structural* property of the layer model rather than a convention.
> Confirm with Issue 15 before finalising the field set; do not annex it
> unilaterally.

## Architecture

### Annotations are degenerate procedure execution events

This is the argument that removes most of the work from this issue.

A procedure is three things that are already accounted for elsewhere: a
**template** (a compiled lifecycle document — Issue 17), an **executor
context** (the `Envelope`'s `actor` and `observed_at`,
`docs/design/core/beliefbase_architecture.md` §4.3), and a **record of what happened**
(the annotation itself — an annotation *is* an as-run record). Collapse pieces of
that triple and the annotation vocabulary falls out:

| Directive | Procedural reading |
|---|---|
| `{note}` | A **zero-step** procedure. Carries text; nothing is executed; no result. |
| `{todo}` | A **one-step** procedure, **open**. The step exists; no completion record yet. |
| `{reviewed}` | A **completed** run — the envelope supplies the executor context and the payload optionally carries a credential claim. |

Nothing here is structurally new. A todo is not a different kind of thing from a
procedure run — it is a procedure run with one step and no completion. A sign-off
is not a different kind of thing from an attestation — it *is* an attestation
whose protocol happens to be "a human read this".

**`{todo}` is a potentialized annotation.** `docs/design/annotation/living_corpus.md` §5
frames a Pragmatic edge as a *declared conduit* — $S_P$, an expected act with no
actor bound — and an annotation record as the `R` proving an actor traversed it.
A todo is exactly that: a declared act awaiting an actor. Closing it binds the
actor and produces the evidence.

This is not a reframing for its own sake; it makes the query surface principled.
"Which todos are open?" and "which nodes lack a required sign-off?" are the same
question — *which declared conduits have no actualization* — which is why §3's
gap query and the todo list are one mechanism rather than two.

**One boundary to respect.** These are all *claims*: authored, owned, and
disputable. They are not `R` — as-run *observations* in the sense of
`../../essays/engineering_model_ontology.md` §3.4, which are reads of the world
with no owner, unbounded in volume, and valuable in aggregate rather than
individually. The annotation vocabulary is human-scale by construction: one
record per human act. A `{todo}` closed by a person is a claim; a thousand
telemetry samples are not, and must not arrive here. Note the boundary is
**subject and volume**, not kind: an annotation *is* an `R` record, privileged
because it observes a node in this graph at human scale. See
`../../design/annotation/living_corpus.md` §2.

The consequence is the point of the issue: **no new persistence model and no new
schema.** Per `docs/design/core/beliefbase_architecture.md` §4.3, the three
directives are `record_kind` values over one `Annotation` type:

| Directive | `record_kind` | Fields that matter |
|---|---|---|
| `{todo}` | `noet:todo:v1` | `result` absent while open; text; optional assignee |
| `{note}` | `noet:note:v1` | text only; no `result` |
| `{reviewed}` | `noet:signoff:v1` | `attester_id`, `result`, optional credential |

Adding a fourth annotation kind later is a protocol-registry entry
(`attestation_fabric.md` §6), not a code change. That property is the whole
reason to route through the registry rather than defining three bespoke types.

### The directives are mostly a rendering surface — with one real exception

> [!IMPORTANT]
> **Scope note.** The directives are a *secondary* feature — a rendering
> vocabulary the projection targets. They should not be read as the primary
> consumer of the annotation model, and the field set must not be designed around
> them. The census above is the real demand.

> **Most annotations are not authored in source**, and a reader should not
> conclude from this issue that typing ` ```{todo} ` into a Markdown file is the
> normal path. **But the boundary is genuinely softer than "never"** — see the
> parser-authored case below.

Annotations mostly exist **on top of** the source, not **as** the source. That
shapes most of the design:

- Source files are the artifact under review. An annotation is a claim *about*
  that artifact at a version. Writing the claim into the artifact changes the
  artifact and invalidates the version it was anchored to.
- Annotations are per-reader and often ephemeral. Source is shared and
  version-controlled. Issue 105's three-scope precedence exists precisely
  because these have different lifetimes.
- A reader with no write access to the repository must still be able to annotate.

#### The parser-authored case, which softens this

A `{todo}` written in a source file **is** an annotation — and the actor who
authored it is **the parser that surfaced it**. The parser emits it on the
annotation channel's **record lane** (`annotation_channel.md` §3), deliberately:
unlike a diagnostic, a source `{todo}` must outlive the parse and be citable by
whoever closes it. That is what distinguishes it from the diagnostic lane, and it
is why the compiler's parse log clearing each run does not touch it — the parse
agent's *diagnostics* clear; a record it chose to emit is a record like any other,
with an `EventId` that survives. Whether the parser re-emits the same `{todo}`
next parse or recognizes the prior record is Issue 110 step 3's open-time
reconciliation question, and the anchor — the `{todo}`'s source range once Issue
103 lands — is what makes "same" decidable.

Two consequences worth designing for:

- **Attribution can be recovered.** Where source attribution exists, `git blame`
  on the annotation's source range maps it to the commit author — so a
  parser-authored record can carry a *human* attribution without a human having
  used a PII surface. Not Phase 1, but do not foreclose it.
- **The lifecycle spans two actors and terminates in a source edit.** A source
  `{todo}` is **opened by the parser**; it is **closed by whoever discharges it**
  through their PII surface, whose `RunEnd` is theirs, not the parser's; and the
  fold's effect is a **write-back that deletes the todo from source**
  (Issues 106/107). The record outlives the parse that created it and changes
  hands.

This is the sharpest test of the field set: one logical record, two actors, three
layers touched. If the `Envelope` cannot express it without a mutable owner
field, the census has found something.

So the *typical* flow is:

```
PII surface (viewer / LSP code action / CLI)
        │  creates a Record
        ▼
Issue 105 sidecar store  (.noet/annotations/, gitignored)
        │  compiler reads on parse
        ▼
Record → BeliefEvent projection  (attestation_fabric.md §12.3)
        │
        ▼
graph — renders as if a directive had been authored inline
```

For records created through a PII surface — the majority — the directives are how
a record **renders and is queried**, not a thing an author types. The
parser-authored case above runs the same pipeline from the other end: source text
becomes a record, and the record renders. See
`attestation_fabric.md` §13 for the PII surface taxonomy — the viewer, the LSP,
the MCP server, and the CLI are four write paths into the same store.

### The exception: template annotations *are* source-authored

One directive form runs the other way, and it resolves the tension the warning
above creates — that a corpus can express what it *contains* but not what work it
*expects*.

A **template annotation** declares that an act is expected on some node, without
asserting that it happened. `docs/design/annotation/living_corpus.md` §5: a Pragmatic edge is
a *declared conduit* ($S_P$) awaiting an actor; an annotation record is the `R`
proving one traversed it. The conduit is normative — a statement about what the
corpus requires — so it belongs in version control. The traversal is evidence, so
it belongs in the sidecar.

| | Template annotation | Annotation record |
|---|---|---|
| Says | "a review is expected here" | "a review happened here" |
| Kind | normative — $S_P$ | evidence — `R` |
| Authored in | **source** | never source — sidecar only |
| Has an actor | no — that is the point | yes, always |
| Lifecycle owner | the declaring node | the actor |

**The mechanism already exists.** `{maps_to}` lets a node own edges between two
other nodes without being either endpoint (`mapping_node_architecture.md` §1),
with ownership carried by `WEIGHT_OWNED_BY` (`src/properties.rs:626`). A template
annotation is exactly that shape: a declaring node owning a
`template_annotation → target` Pragmatic edge.

```markdown
## Review Plan §3

````{expects} signoff
target = ["id://req-014", "id://req-015"]
credential = "safety-reviewer"
````
```

This inherits authoring shape, lifecycle (delete the declaration, the conduits go
with it — so a conduit is never orphaned), and query surface
(`get_maps_to_traceability` already walks owner → sink → sources) from machinery
that is production-tested. Do not build a parallel mechanism.

A template carries `record_kind` and actor constraints but not `actor`,
`observed_at`, or `result` — the same record schema minus what only execution can
supply. Actualization fills the holes.

The payoff is §3's gap query: **"which declared conduits have no actualization?"**
Without templates that question has no ground truth, because nothing states what
*should* have been reviewed. With them, an unreviewed node and a node nobody ever
expected to review are distinguishable — which is the difference between a
coverage report and a list.

**Open**: the directive name. `{expects}` reads well and does not collide, but
`{requires}`, `{awaits}`, and `{expects_annotation}` are candidates. Settle before
implementation; the name is user-facing and hard to change later.

### Directive registration

Follow `docs/design/codecs/myst_directive_architecture.md` §3 and §8 exactly. These are
ordinary `DirectiveDef` entries — no new syntax form, no new dispatch path:

- **Fenced-block form**, 4 backticks, TOML body — the same shape as `{maps_to}`
  (`mapping_node_architecture.md` §2). The body carries the record fields.
- `weight_kind: None`, `ref_role: None` — these are not relation verbs, so per
  §2.2's bare-codespan rule a prose mention of `` `{todo}` `` renders as
  `<code>` and is not a directive invocation. That is the desired behaviour.
- `builder: Some(..)` and a derived sentinel, because rendering is deferred:
  the record set is not known until the sidecar has been read.
- Each record anchors to a node BID, and optionally to a source byte range
  (Issue 103) so an annotation on a paragraph survives edits elsewhere in the
  file.
- The template directive (`{expects}`) is the one form that **is** authored, and
  it follows `{maps_to}`'s owned-edge path rather than the deferred-render path:
  it emits a real Pragmatic edge at parse time with `WEIGHT_OWNED_BY` set to the
  declaring node's bref.

### Derived state via fold

Open/closed and signed/unsigned are **computed, never stored**. Records are
immutable and append-only — a constraint owned by Issue 105 (which owns the
store) and `docs/design/core/beliefbase_architecture.md` §4.3, inherited from
`attestation_fabric.md` §4.2. Closing a todo appends a new record whose
`caused_by` cites the original; the fold over a BID's records yields current state.

This is what makes the store a G-Set — merge is set union, and two users closing
the same todo independently converge without a conflict resolution rule.

#### Each kind has its own state machine

Annotations are **stateful**, and the three kinds do not share a lifecycle:

| Kind | States |
|---|---|
| `{note}` | none — a note is a fact, not a process |
| `{todo}` | `open → closed`, plus `open → abandoned` |
| `{reviewed}` | `signed`; `→ revoked` by a later record; `→ stale` when the anchor hash changes |

Note that `stale` is not actor-driven — it is induced by the anchor moving, which
means the fold depends on both the record chain *and* the current node hash. Do
not model transitions as record-driven only.

**Which hash the anchor reads is per-`record_kind`.** A node carries a family of
hashes, and the record kind selects the scope it asserted about — `content_hash`
for a proofread, a Section-scoped hash for a section review, an Epistemic-scoped
one for a verification claim (`docs/design/identity/content_versioning.md` §5, and §5.5
for why a verification claim cannot anchor to the Section member). The staleness check
is therefore "has *my* anchor hash changed", not "has the node changed". The
transition table for a kind must name its hash alongside its states.

**A fold needs a transition function, and Phase 1 must not hardcode one per
directive.** Three kinds with bespoke `match` arms is tolerable; the fourth is
not, and custom team-defined kinds are a stated direction
(`docs/design/annotation/living_corpus.md` §4). Structure the fold as `(state, record) →
state` parameterized by a transition table looked up by `record_kind`, even
though Phase 1 ships only three tables and they are built in. The cost now is a
lookup; the cost later of not having done it is rewriting the fold.

The eventual home for those tables is **a procedure template** — Issue 17 owns
the single definition of what a lifecycle is: a step declares an **exit
predicate** (`all` / `any` / `ordered` / `n-of`) over a resolved node set, a
discriminated **outcome**, and an **effect** on the marking. A registry entry
references a template by a version-anchored node reference (Issue 105); it does
not embed a second state-machine grammar. The four kinds here are degenerate
templates.

Two consequences for this issue's tables. **State is a marking, not a
position** — there is no program counter, so a table is a set of legal
outcome-to-effect mappings rather than a transition graph. And **a cycle is an
outcome that clears marks**, which is how `redline: rejected → back to draft`
is expressed: no back-edge, no second grammar. Write the four tables in that
shape from the start.

**Issue 105 owns the fold** — deriving state from a record log against a
template, partitioning by run, and projecting one run to one graph node. This
issue owns only the **three built-in transition tables** the fold looks up. Shape
those so that supplying a table from a resolved template rather than a constant
is a substitution, not a refactor.

All three kinds here are **single-record or single-chain** — a `{todo}` opens and
a later record closes it. That is the simple case and it must stay simple. An
operation with *duration* (executing a review procedure over an hour, emitting
several records) needs grouping the log does not provide, which is Issue 105's
`RunStart`/`RunEnd` bracket and `run_id`. Leave room for an
`run_id: Option<EventId>` on the record, defaulting `None`; do not require
it here.

Two behaviours are required regardless:

- **Unknown `record_kind` degrades, never fails.** §6.2 already mandates this for
  attestations with unrecognized `local:` protocols. A record whose transition
  table is unavailable stays readable, still merges, and reports no derived
  state — it must not be dropped.
- **An illegal transition is a diagnostic, not a rejected write.** The store is
  append-only with set-union merge; refusing a write would break both, and a
  transition can only be judged illegal *after* a merge that the writer could not
  have seen. Surface it through `check_consistency`.

### Query surface

Presence queries are ordinary `{query}` expressions over `schema:<record_kind>`
(`attestation_fabric.md` §6.1 — `record_kind` *is* the `schema:` filter value):

```
schema:noet:todo:v1                        # every todo record
schema:noet:todo:v1 AND actor:<id>         # todos for one actor
```

The **absence** query is the compliance-relevant one: *which nodes have no
`{reviewed}` record at the current version?* This is the noet complement
operation described in `attestation_fabric.md` §12.1 — nodes reachable by
Section traversal from the network root, minus nodes reachable by Pragmatic
traversal from the sign-off records. It needs no new query primitive; it is the
same gap analysis the `{maps_to}` traceability machinery already performs
(§12.3), pointed at annotation records instead of requirement coverage.

**Template annotations give this query a denominator.** Complementing against
*every node reachable from the root* answers "what is unreviewed?" only if the
implicit expectation is that everything should be reviewed — which is rarely
true and makes the result mostly noise. Complementing against **declared
conduits** instead asks the question that has an actionable answer:

```
declared conduits (owned template_annotation edges)
  MINUS
actualized ones (conduits with a citing annotation record)
  = the real gap
```

This distinguishes *unreviewed* from *never expected to be reviewed*, which is
the difference between a coverage report and a list of everything. Both forms
should be available — the root-relative one for exploration, the conduit-relative
one for compliance.

Whether "open todos" is expressible as a filter or needs the fold evaluated
first is the one query-layer question this issue must answer in Step 1.

### Viewer rendering

- **Badges** on annotated nodes in the node listing — a todo count, a review
  state marker, a note indicator.
- **Per-node annotation panel** in the metadata card, listing records with
  actor and timestamp, and offering the create/close actions.

Issue 65's `noet-collab.js` overlay is the **deployed-static-site equivalent of
this same surface**. Both render the same `Annotation` type; they differ only in
where they read it from — the local viewer reads the Issue 105 sidecar through
the running server, the overlay reads the attestation server over HTTP. Keep
the rendering logic shaped so it takes a record list and does not care about the
source.

## Implementation Steps

0. **The census** (0.5 days) — do this first
   - [ ] Reconcile the field set against every row in §The primitive census,
         including the two non-node anchors (derived gap; ask → redline)
   - [ ] Decide which fields are **computed** rather than authored
         (`target_shape`, `destination_process`) and reject any field that is a
         second account of a fact the anchor or target already carries
   - [ ] Reject narrative fields (`revised`, `previous_text`, `change_note`) —
         the log is the history (`process_annotation_model.md` §2.6).
         `superseded_by` is a relation and is legitimate
   - [ ] **A `tape_hash` must be resolvable back to its member set.** A hash over
         a sorted set cannot be diffed against; answering *what changed* (Issue
         74 §Archive) needs the members. Decide here whether that is a tree
         object in the archive (preferred — receipts share trees) or a manifest
         in the receipt payload. Cheap now, expensive once receipts exist

1. **Protocol registry entries** (0.5 days), in this order
   - [ ] `noet:receipt:v1` — anchor spec, no payload, stateless; latest wins
   - [ ] `noet:gap:v1` — the **judged** form only; anchor = the requirement
         node; payload = rationale + `asserted_by`; closes by re-assessment
   - [ ] `noet:redline:v1` — field set from Issue 17 step 2a's 25 records;
         **stored** content is a file map (`generational_archive.md` §6.1) even
         where the authoring surface presents a section edit;
         `caused_by` → the gap it closes; `target` → the spec it changes;
         lifecycle `ready → packaged → submitted` with `blocked` / `superseded`
   - [ ] `noet:ask:v1` — `caused_by` → the redline(s) it unblocks; addressee in
         payload; expires on `answered`
   - [ ] Each entry declares its `caused_by` → `WeightKind` translation
         (Issue 105 §Project) and its `protocol.graph_roles`

2. **Directive registration** (0.25 days)
   - [ ] `{reviewed}` (renders a receipt) and `{todo}` (renders a redline with
         empty proposed text) as `DirectiveDef` entries — a rendering surface,
         not new kinds
   - [ ] Verify prose mentions still render as `<code>`

3. **Queries** (0.5 days)
   - [ ] "Unread or changed since read under scope *S*, for me" — the W4 query;
         receipts with `actor = me` whose `tape_hash` ≠ current
   - [ ] The compliance gap query — conduits with no actualizing record
   - [ ] The derived-gap query as a **worklist generator**: candidates a human
         converts to judged gaps or suppresses. Its results are never stored
   - [ ] Verify `schema:` filters resolve for the four protocol IDs

4. **Viewer surface** (0.75 days)
   - [ ] Badges on annotated nodes
   - [ ] Per-node annotation panel with create / close actions
   - [ ] Actions write through the PII surface to the Issue 105 store
   - [ ] Show unactualized conduits distinctly from actualized ones — an expected
         review that has not happened is the state worth surfacing

4a. **The four built-in transition tables** (0.25 days) — the fold itself is
    Issue 105's; this issue supplies what it looks up
   - [ ] receipt (stateless), gap (`open → closed` by re-assessment), redline
         (step 2a's observed lifecycle: `ready → packaged → submitted`, with
         `blocked` / `superseded`), ask (`sent → answered`)
   - [ ] The redline `packaged` transition carries a **cross-run precondition**
         on its cited ask reaching `answered`. **No new syntax** — it is an
         `over:` query with a payload predicate on the cited run's projected
         state (Issue 17 §Cross-run guards; Issue 105 §Project)
   - [ ] No table may reference wall-clock time. "Default if unanswered" is
         payload, not a transition (Issue 17 §Purity)
   - [ ] Receipt staleness is a lazy anchor comparison, not a transition
   - [ ] Unknown `record_kind`: record readable and mergeable, no derived state,
         no drop
   - [ ] Illegal transition surfaces via `check_consistency`, write still accepted

5. **Template annotations** (0.5 days)
   - [ ] Register the `{expects}` directive (name unsettled — see Architecture)
         following `{maps_to}`'s owned-edge path, not the deferred-render path
   - [ ] Emit a Pragmatic edge per target with `WEIGHT_OWNED_BY` set to the
         declaring node's bref
   - [ ] Carry `record_kind` and actor constraints; no `actor` / `observed_at` /
         `result`
   - [ ] Conduit-relative gap query: declared conduits minus actualized ones

## Documentation Obligation

- [ ] Update `beliefbase_architecture.md` §4.3's envelope sketch to the field set
      actually implemented, and remove its "Issue 104 is authoritative" banner
- [ ] Resolve `caused_by` / `provenance` naming across §4.3,
      `attestation_fabric.md` §4.2a, and `living_corpus.md`
- [ ] Confirm no `lamport` field is present in the shipped `Envelope`

## Testing Requirements

- A record in the sidecar renders on the correct node without any source edit
- Closing a todo appends a record and flips the folded state; the original
  record is unchanged on disk
- Two independently-created closing records for the same todo merge to one
  closed state (G-Set union, no conflict)
- Gap query returns exactly the nodes with no current-version sign-off, and
  drops a node from the result the moment one is added
- A `{expects}` directive produces owned Pragmatic edges; deleting the declaring
  section removes them, leaving no orphaned conduits
- The conduit-relative gap query counts only declared conduits — a node nobody
  declared an expectation for does not appear, while an unreviewed declared one
  does
- A prose mention of a directive name in a Markdown file produces no annotation
- A record with an unrecognized `record_kind` round-trips through the store and
  merges correctly while reporting no derived state
- A signoff goes `stale` when its anchor node's hash changes, with no new record
  written — the fold reflects the anchor, not just the chain
- Two records asserting conflicting transitions on the same chain both persist;
  the fold is deterministic and `check_consistency` reports the conflict
- An annotation anchored to a source range survives an unrelated edit elsewhere
  in the same file

## Success Criteria

- [ ] Three directives registered; no new syntax form introduced
- [ ] Zero new persistence types — every record is the
      `docs/design/core/beliefbase_architecture.md` §4.3 `Annotation`, discriminated by
      `record_kind` and persisted in Issue 105's store
- [ ] Open/closed and signed/unsigned are computed by fold; no stored state flag
- [ ] The gap query is expressible with existing query primitives
- [ ] Viewer shows badges and a working annotation panel
- [ ] No annotation is written to a source file by this issue

## Risks

- **Readers assume directives are authored.** → **Mitigation**: the directive
  documentation leads with the non-authoring rule; a source-authored `{todo}`
  is a no-op in Phase 1 rather than a half-working feature.
- **Fold cost on large corpora.** Folding every record on every render is
  O(records) per node. → **Mitigation**: annotation volumes are small relative
  to node counts; if it bites, cache the fold per `(bid, version)` — a derived
  cache, never a second store (`docs/design/annotation/living_corpus.md` §2 — the
  annotation store is sized for deliberate acts and must not be duplicated or
  flooded).
- ~~**`version` anchoring is unresolved upstream.**~~ **Resolved**: `version` is
  a `sha256` content hash over the node's non-metadata content. What remains open
  upstream is narrower — *which member of the hash family* a given `record_kind`
  anchors to (`docs/design/identity/content_versioning.md` §5.5). That choice is a per-kind semantic
  decision, not a blocker on this issue.

## Open Questions

- **Source-authored template vs instance.** A `{todo}` typed into source could
  reasonably mean "this is a checklist item the author intends" — a *template* —
  which is categorically different from an *instance* ("someone opened this
  todo"). The template reading is coherent under the degenerate-procedure model:
  it is the one-step procedure definition, and an instance is a run of it. But
  it reintroduces authoring into source and needs its own render treatment.
  **Flagged, not decided.** Needs a human call before Step 2.
- **Threading on `{note}`.** Replies would make notes a discussion surface.
  **Recommend no for Phase 1** — `caused_by` already expresses "this record
  responds to that one", so the tree structure needs no schema change whenever it
  is wanted. A reply cites the parent's `EventId`, never a collector-assigned id,
  so an offline or browser-authored reply has a stable target.

  **What threading *would* add is sibling ordering**, and that is a field-set
  question this issue owns. Sibling order is **`observed_at`** — collector
  arrival order must not be used, because it is per-collector, reflects
  connectivity rather than authorship, and is not reproducible
  (`docs/design/annotation/collector_model.md` §5.1).

  The optional refinement: `caused_by` loses the fact that an author who saw
  three replies and added a fourth is causally *after* all three, not concurrent
  with them. A reply may carry, **in its protocol payload**, a reference to the
  sibling it follows — optional, citation-shaped rather than an integer position
  (which hits the classic two-authors-claim-position-4 problem), and never part
  of `EventId`. Weigh it against the cost: it makes that push *contextual*, since
  the author must have read the parent's current siblings. Decide with the census.
- **Annotations on a node whose BID changed.** Section BID migration (Issue 36)
  can change a node's identity; records anchored to the old BID orphan. Whether
  they follow the migration, orphan visibly, or fall back to the source range
  (Issue 103) is unresolved. Cross-reference Issue 36 before implementing.

## References

- `docs/design/core/beliefbase_architecture.md` §4.3 — the `Annotation` schema decision
- `docs/design/annotation/attestation_fabric.md` §4.2 (record), §6 (protocol registry),
  §12.1 (gap analysis), §12.3 (edge projection), §13 (PII surfaces)
- `docs/design/codecs/myst_directive_architecture.md` §2, §3, §8 — directive conventions
- `docs/design/codecs/mapping_node_architecture.md` §2 — fenced-block TOML body precedent
- Issue 17 — the lifecycle grammar: exit predicates over an `over:` queryset,
  discriminated outcomes, clearing effects, stable step BIDs, and the purity rule
- Issue 103 — node source ranges (annotation anchoring)
- Issue 105 — the record store and the fold that reads these tables
- Issue 65 — `noet-collab.js`, the deployed-site equivalent of the viewer panel
