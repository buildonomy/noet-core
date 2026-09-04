---
version = "0.1"
title = "Issue 104: Annotation Vocabulary — {todo}, {note}, {reviewed}"
---

# Issue 104: Annotation Vocabulary — {todo}, {note}, {reviewed}

**Priority**: HIGH
**Estimated Effort**: 2 days (RELATIVE COMPARISON ONLY)
**Dependencies**: Requires Issue 105 (annotation sidecar store), Issue 102
(`noet serve`). **Related**: Issue 17 (procedure codec and steps schema) is a
*sibling*, not a dependency — this issue needs a `protocol_id` and a store, not a
`.procedure` parser; its three kinds are degenerate templates needing no template
file.
**Blocks**: the annotate/review pilot — the first workflow that puts a human
reader in front of a compiled network and lets them write something back.

## Summary

A reader of a compiled network can query it but cannot mark it. There is no way
to say "this needs work", "here is context", or "I reviewed this at this
version". This issue delivers that vocabulary — three directives, `{todo}`,
`{note}`, `{reviewed}` — as a **rendering and query surface over records that
already exist**, not as a new storage layer.

The vocabulary is deliberately thin. This issue is authoritative for the
annotation record's field set (see Design Authority below), with
`docs/design/core/beliefbase_architecture.md` §4.3 supplying the `Envelope` that wraps
it; Issue 105 persists it; Issue 102 routes it. This issue defines what the three
record kinds mean, how they render, and how you query for their absence.

## Design Authority

> **This issue is authoritative for the annotation record's field set.**
> `beliefbase_architecture.md` §4.3 carries a *sketch* of the envelope — `id`,
> `actor`, `observed_at`, `caused_by` — explicitly marked as intent rather than
> schema. Settle it here, then update that section to match what was built.
>
> Three questions it leaves open, all of which this issue must answer:
>
> 1. **Is a logical clock needed?** Records merge by set union, so ordering only
>    matters to the derived-state fold. `(observed_at, id)` may suffice. A
>    `lamport` field is justified only if the fold reads it — and per
>    `LESSONS_LEARNED.md` §Design constraints, a field with no reader is a
>    liability, not free forward compatibility.
> 2. **`caused_by` vs. `provenance`.** `attestation_fabric.md` §4.2a already calls
>    this relation `provenance`. Two names for one thing is the divergence this
>    whole effort exists to prevent — pick one and propagate.
> 3. **Does `task` belong on every record** or only on `RunStart` (Issue 109)?
>    See Open Questions.
>
> Envelopes wrap **annotations only**. `BeliefEvent`s are compiler-generated,
> single-writer, and already ordered by the epoch structure; they carry no actor
> and no clock. Do not widen the envelope to cover them.

## Goals

1. **Validate the `Annotation` field set against every known consumer** — see
   §"The primitive census" below. This issue is authoritative for the field set,
   and the set has never been checked against the full demand.
2. Three directives — `{todo}`, `{note}`, `{reviewed}` — registered in the
   existing `DIRECTIVES` table with no new syntax form
3. Three `protocol_id` values over the unified `Annotation` type; zero new schema
4. A query surface for presence *and absence* of annotations, including the
   compliance-relevant gap query
5. Viewer rendering: per-node badges and an annotation panel
6. Open/closed and signed/unsigned derived by fold, never stored

## The primitive census

**Do this first.** The `Envelope` sketch in
`docs/design/core/beliefbase_architecture.md` §4.3 was written against annotation
records, then Issue 110 established that *compiler observations are annotations
too*. The field set has not been checked against that widened demand, and a
review of the design docs could not confirm it matches.

The census below is what this issue must reconcile. Each row is something the
system already banks on holding as an annotation.

| Candidate | Source | Actor | Scope | Anchors to |
|---|---|---|---|---|
| `{todo}` / `{note}` / `{reviewed}` | this issue | human, **or the parser** | repo / shared | `(bid, version)` |
| Diagnostics | Issue 103 Decision 4, Issue 110 | compiler | in-memory | node + **source range** |
| `source_url` | `builder.rs` Phase 4 | compiler | shipped | node |
| `git` status | `builder.rs:2478` | compiler | shipped | **network node only** |
| Layout (`render_position`, `assembly_index`, `structural_weight`) | `layout.rs:216-235` | compiler | shipped | node |
| Source ranges | Issue 103 | compiler | repo | **`(path, file_hash)`**, not `(bid, version)` |
| Content / identity hashes | Issue 105, `content_identity.md` | compiler | shipped | node |
| **Cursor / focus** | **Issue 15** | a PII surface | **in-memory, per-consumer** | a *query*, not a node |
| Run brackets (`RunStart`/`RunEnd`) | Issue 109 | any | scope of the run | template ref + `parent` |
| Redline | Issue 17 step 2a | any | repo | node + replacement text |

### What the census forces

Four things the current `Envelope` does not obviously accommodate:

1. **Not every annotation anchors to `(bid, version)`.** A source range anchors
   to `(path, file_hash)` (Issue 103); a cursor anchors to a *query*
   (`content_versioning.md` §4's `(QuerySpec, tape_hash)`). If the anchor is a
   fixed pair, these are excluded and need their own home — which is the
   fragmentation this unification exists to prevent. **Likely resolution**: the
   anchor is a small enum, not a pair.
2. **`actor` must admit non-humans.** The compiler is an actor
   (`engineering_model_ontology.md` §3.4). `ActorId` is already described as
   opaque; confirm nothing downstream assumes a person, and decide what a
   compiler actor's identity *is* — "the compiler at version X" would let the
   codec-regression detector (`content_versioning.md` §7.2) attribute drift.
3. **A record can outlive its author's scope and change hands.** The `{todo}`
   lifecycle is the worked example: **opened by the parser** when it encounters a
   `{todo}` in source, **closed by whoever discharges it** through their PII
   surface, and the fold's effect is a source edit *removing it*. One logical
   record, two actors, terminating in a write-back. Confirm `caused_by` carries
   this without a mutable owner field.
4. **Scope is per-record, and the census spans all four.** In-memory
   (diagnostics, cursors), shipped (layout, `source_url`), repo (ranges,
   todos), shared (sign-offs). Whether scope is a field on the record or a
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
> `EventSubscription { id, query, tx }` is a per-consumer, in-memory-scoped,
> mutable-by-its-owner filter over the graph — which is an annotation subtype,
> not a bespoke registry. If so, "every PII surface has a focus filter it fully
> owns" is a *structural* property of the layer model rather than a convention.
> Confirm with Issue 15 before finalising the field set; do not annex it
> unilaterally.

## Architecture

### Annotations are degenerate procedure execution events

This is the argument that removes most of the work from this issue.

A procedure is three things that are already accounted for elsewhere: a
**template** (a compiled `.procedure` document — Issue 17), an **executor
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
frames a Pragmatic edge as a *declared conduit* — `S(P)`, an expected act with no
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
directives are `protocol_id` values over one `Annotation` type:

| Directive | `protocol_id` | Fields that matter |
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
authored it is **the parser that surfaced it**, in exactly the same class as
`git` data, `source_url`, and diagnostics. That is not a special case bolted on;
it is what Issue 110 establishes generally.

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
a *declared conduit* (`S(P)`) awaiting an actor; an annotation record is the `R`
proving one traversed it. The conduit is normative — a statement about what the
corpus requires — so it belongs in version control. The traversal is evidence, so
it belongs in the sidecar.

| | Template annotation | Annotation record |
|---|---|---|
| Says | "a review is expected here" | "a review happened here" |
| Kind | normative — `S(P)` | evidence — `R` |
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

A template carries `protocol_id` and actor constraints but not `actor`,
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

**Which hash the anchor reads is per-`protocol_id`.** A node carries a family of
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
state` parameterized by a transition table looked up by `protocol_id`, even
though Phase 1 ships only three tables and they are built in. The cost now is a
lookup; the cost later of not having done it is rewriting the fold.

The eventual home for those tables is **a procedure template** — Issue 17 owns
the single definition of what a lifecycle is, and a `.procedure` document's
`steps` field with its `sequence` / `any_of` / `all_of` types *is* a state
machine. A registry entry references one by a version-anchored node reference
(Issue 105); it does not embed a second state-machine grammar. The three kinds
here are degenerate templates.

**Issue 109 owns the fold semantics** — deriving state from a record log against
a template, including the harder half: multi-record operations. Shape the fold so
that supplying the transition table from a resolved template rather than a
constant is a substitution, not a refactor.

All three kinds here are **single-record or single-chain** — a `{todo}` opens and
a later record closes it. That is the simple case and it must stay simple. An
operation with *duration* (executing a review procedure over an hour, emitting
several records) needs grouping the log does not provide, which is Issue 109's
`RunStart`/`RunEnd` bracket and `run_id`. Leave room for an
`run_id: Option<EventId>` on the record, defaulting `None`; do not require
it here.

Two behaviours are required regardless:

- **Unknown `protocol_id` degrades, never fails.** §6.2 already mandates this for
  attestations with unrecognized `local:` protocols. A record whose transition
  table is unavailable stays readable, still merges, and reports no derived
  state — it must not be dropped.
- **An illegal transition is a diagnostic, not a rejected write.** The store is
  append-only with set-union merge; refusing a write would break both, and a
  transition can only be judged illegal *after* a merge that the writer could not
  have seen. Surface it through `check_consistency`.

### Query surface

Presence queries are ordinary `{query}` expressions over `schema:<protocol_id>`
(`attestation_fabric.md` §6.1 — `protocol_id` *is* the `schema:` filter value):

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

1. **Protocol registry entries** (0.25 days)
   - [ ] Define `noet:todo:v1`, `noet:note:v1`, `noet:signoff:v1` per
         `attestation_fabric.md` §6, including `protocol.graph_roles`
   - [ ] Confirm the projection each emits under §12.3

2. **Directive registration** (0.5 days)
   - [ ] Three `DirectiveDef` entries in `src/codec/myst.rs`
   - [ ] Deferred builders reading the projected records for the node
   - [ ] Verify prose mentions still render as `<code>`

3. **Fold and query** (0.5 days)
   - [ ] `fold(records) -> AnnotationState` for a BID
   - [ ] Verify `schema:` filters resolve for the three protocol IDs
   - [ ] Gap query: nodes with no `noet:signoff:v1` at the current version

4. **Viewer surface** (0.75 days)
   - [ ] Badges on annotated nodes
   - [ ] Per-node annotation panel with create / close actions
   - [ ] Actions write through the PII surface to the Issue 105 store
   - [ ] Show unactualized conduits distinctly from actualized ones — an expected
         review that has not happened is the state worth surfacing

4a. **Parameterized state fold** (0.25 days)
   - [ ] Fold shaped as `(state, record) → state` with the transition table
         looked up by `protocol_id` — not a `match` per directive
   - [ ] Three built-in tables: note (stateless), todo, signoff
   - [ ] Signoff's `stale` transition reads the current node hash, not only the
         record chain
   - [ ] Unknown `protocol_id`: record readable and mergeable, no derived state,
         no drop
   - [ ] Illegal transition surfaces via `check_consistency`, write still accepted

5. **Template annotations** (0.5 days)
   - [ ] Register the `{expects}` directive (name unsettled — see Architecture)
         following `{maps_to}`'s owned-edge path, not the deferred-render path
   - [ ] Emit a Pragmatic edge per target with `WEIGHT_OWNED_BY` set to the
         declaring node's bref
   - [ ] Carry `protocol_id` and actor constraints; no `actor` / `observed_at` /
         `result`
   - [ ] Conduit-relative gap query: declared conduits minus actualized ones

## Documentation Obligation

- [ ] Update `beliefbase_architecture.md` §4.3's envelope sketch to the field set
      actually implemented, and remove its "Issue 104 is authoritative" banner
- [ ] Resolve `caused_by` / `provenance` naming across §4.3,
      `attestation_fabric.md` §4.2a, and `living_corpus.md`
- [ ] Record whether a logical clock shipped, and if not, remove `lamport` from
      Issue 105's ordering requirement and its associated test

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
- A record with an unrecognized `protocol_id` round-trips through the store and
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
      `protocol_id` and persisted in Issue 105's store
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
  upstream is narrower — *which member of the hash family* a given `protocol_id`
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
  responds to that one", so threading can be derived later without a schema
  change if it turns out to be wanted.
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
- Issue 17 — procedure codec, `steps` schema, stable step BIDs, and the
  step-type combining predicates
- Issue 103 — node source ranges (annotation anchoring)
- Issue 105 — annotation sidecar store
- Issue 65 — `noet-collab.js`, the deployed-site equivalent of the viewer panel
