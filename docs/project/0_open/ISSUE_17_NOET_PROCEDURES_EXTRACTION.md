# Issue 17: Procedure Codec and Steps Schema

**Priority**: HIGH
**Estimated Effort**: 4.5-4.75 days (RELATIVE COMPARISON ONLY) — 2-2.5 for the
codec and schema, +0.75 for the annotation subtypes recovered from Issue 18,
+1.5 for consolidating the procedural design-doc space (step 4)
**Dependencies**: Issue 1 (Schema Registry), Issue 2 (Section Metadata)
**Blocks**: Issue 109 (annotation lifecycle) — for step-type combinator semantics and the annotation subtypes below
**Related**: Issue 104 (annotation vocabulary) is a *sibling*, not a dependant — see Scope; Issue 95 owns the eventual crate split; Issue 105 (annotation sidecar store) owns the record store; Issue 106 (source write-back) owns promotion into a source edit

> [!IMPORTANT]
> **Re-scoped.** This issue previously defined a three-piece "as-run data model"
> (template / executor context / as-run record) as bespoke types. That model is
> **withdrawn**. Under the living-corpus design, **an annotation *is* an as-run
> record**, and a procedure instance is the set of annotations sharing a common
> `RunStart` ancestor — a *query* over the annotation store, not a new type.
>
> What remains is a codec and a schema. See "What Was Removed and Why".

## Summary

Deliver two things: a `ProcedureCodec` for `.procedure` files, and a `steps`
schema extension registered with `SCHEMAS`. Together they let a procedure
document compile into a hierarchy of step nodes with stable BIDs and typed
combinators.

This supplies the **template** side of the annotation model — the as-written
definition that a run is folded against. It adds **no record types**. The record
side is already covered: `Envelope` + `Annotation`
(`docs/design/core/beliefbase_architecture.md` §4.3) carries who/when/why, Issue 105
stores it, and Issue 109 brackets a run and folds it.

## Goals

1. Implement `ProcedureCodec` for `.procedure` files, registered with the
   noet-core codec map
2. Define the procedure schema as a runtime-registered extension that validates
   the `steps` field structure
3. Generate nodes from the `steps` field — schema-driven, hierarchical, recursive
4. Give every step a **stable BID** so a run can name which step it discharges,
   surviving template revision
5. Establish whether step types can carry **transition semantics**, not merely
   execution order (see Risk 1 — this may fail, and failing is a valid outcome)
6. Define the **annotation subtypes** a procedural state machine needs to be
   driven by records — redline being the obvious one (see below)
7. Add **no new record primitives**; ship as a module inside noet-core with
   crate extraction deferred to Issue 95

## Scope: what this issue is not

The re-scope moved several things out. Named explicitly, because three other
issues previously waited on this one and no longer should:

| Concern | Owner |
|---|---|
| Annotation record field set (`{todo}`/`{note}`/`{reviewed}`) | **Issue 104** — authoritative |
| Record storage, `(bid, version)` anchoring, retention | **Issue 105** |
| Run bracketing, nesting, folding a log into state | **Issue 109** |
| Envelope (`actor`, `observed_at`, `caused_by`) | `beliefbase_architecture.md` §4.3 |
| Execution loop, deviation analysis | **Issue 18** |
| Promotion of a record into a source edit | **Issue 106** |

### What came back from Issue 18

Issue 18 was reduced to an aspirational stub, and its concrete types went with
it. **The subset needed to drive a procedural state machine belongs here**, not
in an undesigned issue: a template that no record can advance is inert, so the
template side and the annotation subtypes that discharge it are one deliverable.

This is a **vocabulary** obligation, not a record-schema one. Issue 104 owns the
annotation record's field set and Issue 105 owns the store; what this issue adds
is the set of `protocol_id` values — and their payload shapes — that let a run
synchronize with the state machine its template defines.

**Redline is the worked example.** A redline annotation proposes a change to the
node it anchors. Issue 106 promotes one into a source edit and already depends on
it existing (`ISSUE_106` §Redline promotion cites "an annotation whose
`protocol_id` marks it as proposing a change"), but no issue currently defines
that `protocol_id`. That gap closes here.

What this does **not** reintroduce: `ProcedureRun`, `ExecutionRecord`,
`CorrectionEvent`, `DeviationReport`, or `ObservationEvent`. Those were withdrawn
because they were bespoke record *types*. A subtype here is a registered
`protocol_id` plus a payload schema — the same mechanism `{todo}` and
`{reviewed}` already use, not a parallel one.

**An event is an annotation subtype.** Run lifecycle events are not a separate
enum and do not expand `src/event.rs`. A sequence of annotations sharing a
`RunStart` ancestor *is* what was formerly called an as-run log — which is why
the withdrawn types had no home to return to.

**Issue 17 does not block Issue 104.** The previous header claimed it did. Issue
104 needs a `protocol_id` and a store, not a `.procedure` parser; its three
built-in kinds are degenerate templates that need no template *file*. The two can
proceed in parallel.

Issue 109's dependency is real but narrow: it needs the **step-type combinator
semantics** (`all_of` / `any_of` / `sequence` decide whether a parent folds in a
child) and a **stable step reference**. It does not need the codec.

## Architecture

### Codec-First Design Principle

**Why a specialized codec?** Procedures prioritize structure and operations
(connections, execution order, logical operators) over text content. Generating
nodes from the `steps` field hierarchy conflicts with markdown's content-driven
generation from headings. Rather than merging three sources of truth, the file
extension signals the parsing strategy:

- `.md` → MdCodec → nodes from headings, `sections` as metadata, text is primary
- `.procedure` → ProcedureCodec → nodes from `steps`, text supplements structure

This gives clear semantics per file type and no authority conflicts — the codec
owns generation strategy. MdCodec may still handle text content within a
`.procedure` file, but ProcedureCodec orchestrates parsing.

> See `docs/design/core/beliefbase_architecture.md` §3.2 and §3.6 — "Two-Registry
> Codec Dispatch" — before implementing. `WALK_CODECS` and `CLAIM_MAP` have
> non-obvious ordering constraints.

### What Was Removed and Why

The withdrawn three-piece model was: **Template** (as-written), **Executor
Context** (who/when/where), **As-Run Record** (what happened). Each piece has a
home, and none of those homes is a new type here:

| Withdrawn piece | Where it went |
|---|---|
| **Template** | **Survives, demoted.** Not a data model — it is a compiled `.procedure` document plus a two-field reference to it. That reference is a general "node at a content version", not a procedure-specific type (see below). |
| **Executor Context** | **Collapses into `Envelope`.** `executor_id` → `actor`; `timestamp` → `observed_at` (`beliefbase_architecture.md` §4.3). `credential_type` is already a payload field of the sign-off protocol (Issue 104). `environment` had **no named reader** and is dropped until something asks for it. |
| **As-Run Record** | **Collapses into a query.** The set of annotations sharing a `RunStart` ancestor (Issue 109). `template` → the `RunStart`'s reference; `context` → its envelope; `status` → the fold's output; `steps` → member records carrying `run_id`/`task`; `provenance` → `caused_by`; `evidence_hash` → per-record, Issue 108's concern. |

The governing constraint: **new primitives in core types must be few and
general.** A `record type` or a `cause-ordering predicate` is general. An
`AsRunRecord` is not — it is a procedure-specific spelling of something the
annotation model already expresses.

> **Do not resurrect `AsRun`.** A struct of that name, with `AsRunState`
> (`Running`/`Failed`/`Redlined`/`Inventory`) and `RenderMode`, existed in
> `src/properties.rs` as dead code — orphaned when completed Issue 10 removed the
> client events that used it. It has been deleted. Its state space is **not** the
> `open`/`closed` space a fold produces, and it was never designed against one.

### The one reference type, and why it is not defined here

A run must name the template it ran against: a BID plus a content version.

That pair is **not procedure-specific** — Issue 105 needs the same
`(bid, content_version)` anchor for *every* record it stores, and
`content_versioning.md` §5.1 already defines what the version is. Defining it
here would put a general primitive behind a codec issue and make three issues
wait on a `.procedure` parser they do not need.

**Decision**: the version-anchored node reference is owned by Issue 105 (the
store that keys on it), named neutrally. This issue *consumes* it.

### The Procedure Schema Is the Lifecycle Definition

**This issue owns the single definition of what a lifecycle is.** Nothing else
may declare one.

Annotation kinds are stateful — a `{todo}` goes `open → closed`, a review
progresses through phases — and deriving that state from an immutable record log
requires knowing which states exist and which transitions are legal. An earlier
design put a `[protocol.states]` / `[[protocol.transitions]]` block in the
attestation protocol registry (`attestation_fabric.md` §6). **That is withdrawn**:
it would have been a second grammar for something this issue already defines.

A procedure *is* a state machine. The `steps` field declares ordered states; the
step types declare the transition semantics:

| Step type | Transition meaning |
| --------- | ------------------ |
| `sequence` | states advance in order |
| `any_of` | one branch satisfies the parent |
| `all_of` | every branch must be satisfied |
| `parallel` | branches progress independently |

So a lifecycle is an authored `.procedure` document, and a registry entry
references it rather than embedding a state machine. Three things follow:

- **A custom lifecycle is still no code change** — it is a `.procedure` file plus
  a registry entry pointing at it.
- **A lifecycle becomes a first-class graph node.** It can be versioned,
  reviewed, annotated, and traversed like any other content. An embedded TOML
  block could not.
- **The step types must be expressive enough to serve as transition semantics**,
  not merely as execution ordering. **This is not yet established — see Risk 1.**

Read the step types as **combining predicates over a set of records**, not as an
execution order. "Did this run satisfy its parent step?" is `all_of` / `any_of` /
`sequence` applied to the child records. That reading is what Issue 109 already
assumes, and it is what keeps the semantics general rather than tied to a running
engine this issue does not build.

**Step types must be an open enum.** Downstream vocabularies will add types, and
a closed enum forces a core change for each. Note the consequence: an open enum
makes the type set a **versioning surface** — a record folded against a template
using a step type the reader does not know must fail visibly rather than silently
mis-fold. Schema versioning is Issue 32's; see Open Question 2 for the namespace
mechanism.

Consumers: Issue 109 folds a record log against a template to derive state, and
uses a step reference (`task`) to decide whether a nested run counts toward its
parent. Issue 104's three built-in kinds are degenerate templates. Neither
defines a lifecycle format of its own.

### Relationship to the Attestation Record

The unification that motivated the withdrawn model still holds, and is now
someone else's to enforce: an as-run record and an attestation record in
`docs/design/annotation/attestation_fabric.md` §4.2 are **the same object described from two
ends** — one starts from "a procedure was executed", the other from "a claim was
made about an artifact".

Because this issue defines no record type, it cannot cause that divergence and
cannot prevent it. The append-only, anchoring, and one-schema-one-store
constraints belong to **Issue 105**, which owns the store, and the field set
belongs to **Issue 104**. They are noted here only so a reader arriving from the
old version of this issue knows where they went.

### Module Layout

Ship as a module in noet-core. **Issue 95 (Workspace Decomposition) owns
crate-splitting** — extraction of this module into a separate `noet-procedures`
crate is deferred to whenever Issue 95 lands, and should not be attempted here.
The layout below is designed to make that later extraction mechanical.

```
src/procedures/
├── codec/            # ProcedureCodec implementation
│   ├── mod.rs        # ProcedureCodec, registers with CODECS
│   ├── parse.rs      # Parse .procedure files, generate nodes from steps
│   └── generate.rs   # Generate .procedure source from nodes
├── schema/           # Procedure schema definitions
│   ├── mod.rs        # Schema registration with SCHEMAS
│   ├── procedure.rs  # Core procedure schema (validates steps field)
│   └── steps.rs      # Step types (open enum) + combining predicates
└── mod.rs            # Public API, initialization
```

Note the absent `as_run/` directory — it was the withdrawn model's home.

Out of scope: `execution/` and `redlines/` (Issue 18), `promotion/` (Issue 106),
any record type or store (Issues 104, 105, 109).

### ProcedureCodec Behavior

`ProcedureCodec::parse` reads TOML frontmatter into a document node, retrieves
the registered `Procedure` schema, and calls `generate_nodes_from_steps` on the
`steps` field. That function recursively walks the steps array: each step becomes
a node, substeps become child nodes, and the `heading` field carries the
parent-child relationship. This is the **opposite** of MdCodec — schema-driven,
not content-driven. Markdown after the frontmatter is optional documentation,
injected into step nodes as `text`; it does not define structure.

### Boundaries

**Provided here**: `ProcedureCodec`, the procedure schema, node generation from
the `steps` field with stable step BIDs, and the step-type combining predicates
— built on noet-core's existing codec registry, schema registry, and lattice
primitives.

**Not provided here**: any record type, a running execution engine or deviation
analysis (Issue 18), source write-back or template promotion (Issue 106), record
persistence (Issue 105), or behavior prediction, sensor integration, and learning
algorithms (all downstream-product concerns).

## Implementation Steps

### 1. Codec Infrastructure (2 days)

- [ ] Create `src/procedures/codec/mod.rs`, implement `DocCodec` for `ProcedureCodec`
- [ ] Register with `CODECS.insert("procedure", ...)` and `WALK_CODECS`; verify
      claim-time dispatch ordering against `beliefbase_architecture.md` §3.6
- [ ] Parse `steps` from TOML frontmatter; recursively generate `IRNode`s
- [ ] Set `heading` for parent-child relationships (substeps)
- [ ] Handle step types: action, prompt, sequence, parallel, any_of, all_of
- [ ] **Stable step BIDs** — a step is a node, so it already has a BID; that BID
      *is* the step reference Issue 109 needs for its `task` field. Derive it
      from a stable string (see `Bid::codec_namespace`, `src/properties.rs:336`),
      never from a positional index, so it survives template revision. This adds
      **no new primitive** and resolves what was Open Question 2.
- [ ] Inject optional post-frontmatter markdown as step `text`

### 2. Schema Registration and Step Semantics (1 day)

> Steps 2 and 2a together answer whether a template can actually be driven by
> records. Neither is complete without the other.

- [ ] Define the procedure schema in Rust; register via `SCHEMAS`
- [ ] Validate `steps` field structure and step-type discriminants
- [ ] Step types as an **open enum** — an unknown type validates structurally and
      fails loudly at fold time rather than being silently ignored
- [ ] Document each step type as a **combining predicate over a record set**,
      which is the form Issue 109 consumes
- [ ] **Design the cycle construct** (Risk 1) — a transition step type meeting
      the five constraints in Risks. Acceptance: `draft → peer-reviewed →
      board-approved` with a rejection path back to `draft`. Record the design
      here and notify Issues 104 and 109
- [ ] Registration tests; verify TOML parsing against the registered schema

### 2a. Procedural Annotation Subtypes (0.75 days)

- [ ] Register the `protocol_id` values a run needs to advance a state machine.
      Minimum: **redline** (proposes a change to the anchored node — Issue 106
      consumes it and currently has no definition to point at)
- [ ] Payload schema per subtype, in Issue 104's record shape — **no new record
      type**, no addition to `src/event.rs`
- [ ] Define how a subtype names the step it discharges: the `task` field is a
      step BID (step 1), so this is a reference convention, not a new mechanism
- [ ] Test: a record carrying an unknown `protocol_id` fails loudly at fold time
      rather than being silently dropped — same rule as the open step-type enum

### 3. Documentation (0.25 days)

- [ ] Rustdoc for public APIs; module-level docs
- [ ] Update `docs/design/procedures/procedure_schema.md` for ProcedureCodec behavior and
      the `.procedure` extension
- [ ] Record the withdrawal of the three-piece model wherever
      `procedure_schema.md`, `attestation_fabric.md` §4.2, or
      `docs/design/procedures/procedure_execution.md` still assumes it — the latter's §2 is
      titled "The Three-Piece 'As-Run' Model" and states it directly

### 4. Consolidate the procedural design-doc space (1.5 days) — **near-final step**

> **Do this last, after steps 1-2a are built.** The consolidation should describe
> what was implemented, not what was planned. Doing it first would mean
> documenting a design that the build may still change.

**The problem.** Five design documents describe procedural architecture, and they
currently total ~3,460 lines built substantially on the withdrawn as-run model:

| Document | Lines | State |
|---|---|---|
| `procedure_execution.md` | 724 | Banner-marked withdrawn; §2 rewritten; §8 API depends on withdrawn types |
| `action_observable_schema.md` | 938 | Banner-marked; `inference_hint` half is sound |
| `noet_procedures_readme.md` | 872 | Banner-marked; argument survives, API does not |
| `redline_system.md` | 543 | Banner-marked; §2, §3.3, §6, §7, §10, §11 stand |
| `procedure_schema.md` | 387 | Sound; the step grammar lives here |

Those banners were triage, not a fix. A reader arriving at any of these cannot
tell which parts describe the system and which describe a withdrawn draft.

**The mandate.** Design docs must be **pedagogic about what IS**, not archaeology
about what used to be. This step has full authority to **edit, move, merge,
split, delete, or create** documents in `docs/design/` so that the procedural
architecture reads as one coherent design.

- [ ] **Decide the target document set first**, and record the shape before
      editing. Consolidation is the expected outcome — five overlapping documents
      for one subsystem is the problem — but the split is a judgment call. A
      plausible target: one document for the procedure/annotation model as built,
      one for the schema reference, and stubs for what Issue 18 will own.
- [ ] **Remove the withdrawal banners by making them unnecessary.** A banner
      saying "this describes a withdrawn model" is a placeholder for this step.
      When it is done, no procedural design doc should carry one.
- [ ] **Preserve what was assessed as sound** rather than rewriting it:
      `action_observable_schema.md`'s `inference_hint` schema (grouping and
      transition events, temporal and confidence constraints, the Participant
      channel, `response_config`); `procedure_execution.md` §4.1's
      append-only-log-plus-derived-state design and §6.2's run nesting, both of
      which anticipated the current model; §11's design principles; and the
      deviation taxonomy in `redline_system.md`. These are requirements the
      current model relocates rather than answers.
- [ ] **`CorrectionType` / `DeviationType` become payload vocabulary** for the
      redline `protocol_id` registered in step 2a. They enumerate distinctions
      the payload schema must express; they are not event types.
- [ ] **Fix the inverted layering in `noet_procedures_readme.md`**: it says the
      redline channel "injects `BeliefEvent`s to modify the loaded BeliefBase".
      That violates assert-vs-mutate (`beliefbase_architecture.md` §4.3) —
      annotations project into events *via the fold*, never the reverse, and the
      result is a **held-out** BeliefBase (`living_corpus.md` §2).
- [ ] **Resolve the bespoke query APIs.** `procedure_execution.md` §8 and
      `redline_system.md` §6 define Rust signatures for questions that may be
      expressible in the existing grammar (`query_model.md`). Decide; do not
      leave both standing.
- [ ] **Do not delete history — relocate it.** Where a withdrawn design needs
      recording, put a note in this issue or Issue 18, not in a design doc.
      `AGENTS.md` forbids deleting documents outright: propose consolidation or
      archiving, and get agreement before removing any file.

**Stub, do not design, Issue 18's territory.** Execution loop, deviation
analysis, and the participant channel are Issue 18's. This step creates the
sections or documents those will occupy, with enough context that Issue 18 can
pick them up — and creates nothing more. A stub that quietly becomes a design is
this step failing.

- [ ] Named stub locations for Issue 18's scope, cross-referenced from Issue 18
- [ ] **Verify Issue 18's task list matches those stubs.** Issue 18 is currently
      an aspirational stub with three undecided candidate directions; it must
      contain the tasks and cross-references needed to resume, or the stubs will
      be orphaned.
- [ ] Check the remaining inbound references — `ISSUE_16_AUTOMERGE_INTEGRATION.md`
      (four `procedure_correction` citations plus a `redline_system.md`
      dependency) and `ISSUE_93_MODEL_MAP_INFERENCE_ENGINE.md` — and repoint them
      at whatever the consolidation produces

**Two open questions this step must settle, not inherit:**

- **Is `procedure_execution.md` salvageable or superseded?** It is the most
  withdrawn-dependent of the five. Merging its surviving parts into a new
  document may be cleaner than repairing it in place.
- **Where does `docs/design/` archive superseded material?** There is no
  convention — `docs/project/trades/superseded/` exists for trade studies, but
  design docs have no equivalent. If archiving rather than deleting, this step
  establishes the convention.

## Testing Requirements

**Codec**: registration with `CODECS` and claim-time dispatch; TOML frontmatter
parsing; hierarchical node generation from `steps`; recursive substeps produce
correct heading levels; optional markdown injected as `text` without altering
structure; round-trip parse → generate → parse yields an identical node set; all
step types (action, prompt, sequence, parallel, any_of, all_of).

**Schema**: registration and retrieval; `steps` validation including malformed
and deeply nested cases.

**Step identity**: a step's BID is stable across a template edit that does not
touch that step — specifically, inserting a step *before* it must not change it.
This is the test that a positional index would fail, and Issue 109 depends on it.

**Step semantics**: each combining predicate returns the correct verdict over a
synthetic record set (`all_of` with one missing child fails; `any_of` with one
present child passes; `sequence` respects order). No execution engine — evaluate
the predicate against a fixture.

**Integration**: GraphBuilder creates correct edges for substeps; all
`.procedure` examples parse; doctests pass.

**Negative**: no new record type is introduced. A grep for `AsRunRecord`,
`ExecutorContext`, `ProcedureRun`, `ExecutionRecord`, `CorrectionEvent`,
`DeviationReport`, `ObservationEvent`, or a resurrected `AsRun` finds nothing —
the annotation subtypes are `protocol_id` registrations, not types.

**Annotation subtypes**: a redline record round-trips through Issue 105's store
and resolves to the node it anchors; an unknown `protocol_id` fails loudly at
fold time rather than being dropped.

## Success Criteria

- [ ] `ProcedureCodec` registered and claiming `.procedure` files
- [ ] Procedure schema registered via the `SCHEMAS` API — no hardcoding in noet-core
- [ ] Nodes generated from the `steps` field, including recursive substeps
- [ ] Round-trip parse → generate → parse is lossless
- [ ] **Every step has a BID stable across template revision**; Issue 109 can use
      it as `task` without further work
- [ ] Step types are an open enum, documented as combining predicates
- [ ] **A cycle is expressible** — the acceptance workflow in Risk 1 parses,
      validates, and folds correctly, with termination guaranteed
- [ ] **No procedural design doc carries a withdrawal banner**, and none
      describes a type that does not exist
- [ ] **A reader new to the codebase can learn the procedural architecture from
      the design docs alone**, without needing to know which parts were withdrawn
- [ ] **Issue 18's territory is stubbed, not designed**, and Issue 18 carries the
      tasks and cross-references to resume from those stubs
- [ ] **Zero new record types**; no addition to `src/properties.rs` or
      `src/event.rs` — the annotation subtypes are registered `protocol_id`
      values with payload schemas, not new Rust types
- [ ] A **redline** `protocol_id` exists and is registered, so Issue 106's
      promotion path has a definition to resolve
- [ ] Module layout is extraction-ready for Issue 95 (no noet-core internals
      reached into from `src/procedures/`)
- [ ] No execution engine, deviation analysis, or persistence layer in this issue
- [ ] No product-specific code

## Open Questions

1. **Do `.procedure` templates need explicit frontmatter or a file-extension
   marker to bind to the right codec?** Extension-based dispatch handles the
   simple case, but a template embedded in or adjacent to other content may need
   an explicit declaration. Settle during step 1 against the two-registry
   dispatch rules (`beliefbase_architecture.md` §3.2).
2. **Can `Bid::codec_namespace` (or another reserved namespace) carry schema
   versions?** Networks already map into the buildonomy namespace by
   construction; the same mechanism could identify, link, and migrate
   *sub-schemas*, which the open step-type enum now requires. Promising and
   unproven. Broader schema evolution is Issue 32's; this question is only
   whether the namespace mechanism is the right carrier.
3. **Cross-referencing**: markdown docs referencing `.procedure` steps via BID
   URLs (`bid://procedures/example#step_two`). Largely dissolved by the
   stable-step-BID decision — a step is an addressable node like any other — but
   confirm the resolver handles the fragment form.

### Resolved or moved

- ~~Where does `version` come from for a template ref?~~ → `content_versioning.md`
  §5.1 defines the content version; Issue 105 owns the anchor.
- ~~Step IDs: globally unique or procedure-scoped?~~ → **Resolved**: a step is a
  node, so its BID is the reference. See step 1.
- ~~Schema versioning~~ → Issue 32 owns evolution; Issue 105 owns whether the
  store can resolve a superseded template version.
- ~~Event enum expansion~~ → **Stale as written**: `src/event.rs` has `Ping` and
  `Belief`; there is no `Focus` variant. The envelope question is settled in
  `beliefbase_architecture.md` §4.3.

## Risks

**Risk 1 — superseded by a design task: the step grammar needs a cycle
construct.** Not a risk to mitigate; a thing to design during step 2.

**The finding (verified).** `sequence` / `parallel` / `all_of` / `any_of` are a
*nesting tree* grammar (`docs/design/procedures/procedure_schema.md` §4.2 — operators
contain `steps = [ ... ]`), and a tree has no back-edge. A `grep` across that
document for `goto|next_state|transition|on_fail|on_reject|repeat|loop|retry`
returns **nothing** — no transition construct exists. A review lifecycle with a
rejection path (`draft → peer-reviewed → draft`) is a cycle, so as written the
grammar cannot express the workflow that motivated the lifecycle unification.
Issues 104 and 109 both assume it can; neither checked.

**The resolution is to extend the grammar.** This is our design and the open
step-type enum already accommodates an addition. What is **not** acceptable is a
second lifecycle grammar appearing elsewhere — a transition construct inside
`steps` is one grammar; a `[protocol.transitions]` block beside it is two, which
is exactly what this issue's single-definition rule exists to prevent.

**Constraints the construct must satisfy** — design against these:

1. **It names a target rather than nesting.** A transition is a real grammar
   addition, not a reinterpretation of the existing operators.
2. **The target is a stable step BID** (step 1's decision), never a positional
   index — a transition naming a position breaks on template revision.
3. **It stays a combining predicate over a record set.** Issue 109 folds records
   against the template; a construct meaningful only to a *running engine* would
   break that reading and re-couple lifecycle state to execution.
4. **Termination.** A cycle means the fold can revisit a step, so it needs a
   visited-set or an explicit iteration bound. Precedent: `PathMap` already
   carries a `loops` guard for same-kind Section cycles — cycles-with-guards is
   an established pattern here, not a new hazard.
5. **Unknown transition types fail loudly at fold time**, per the open-enum rule.

**Acceptance test**: express `draft → peer-reviewed → board-approved` with a
rejection path back to `draft`. Tell Issues 104 and 109 the outcome.

**Risk**: Codec dispatch ordering implemented incorrectly, silently wrong
behavior → **Mitigation**: read `beliefbase_architecture.md` §3.2/§3.6 first;
test claim-time dispatch explicitly

**Risk**: The withdrawn as-run model creeps back in as "just a small struct" →
**Mitigation**: the zero-new-record-types success criterion and the negative
grep test. If a record field seems to have no home, that is a question for
Issue 104, not a reason to define a type here.

**Risk**: Schema registry (Issue 1) incomplete → **Mitigation**: validate the
registry API before starting; block if it is not ready

**Risk**: Scope creeps back toward an execution engine → **Mitigation**: the
`execution/` and `redlines/` paths are explicitly out of scope; any run loop
belongs to Issue 18

**Risk**: Module boundaries leak, making the Issue 95 extraction painful →
**Mitigation**: treat `src/procedures/` as if it were already a separate crate

## References

- `docs/design/core/beliefbase_architecture.md` §4.3 — `Envelope` and `Annotation`;
  where the withdrawn executor context went
- `docs/design/annotation/living_corpus.md` §2 — annotations as a privileged subset of `R`;
  the reason an annotation *is* an as-run record
- `docs/design/annotation/living_corpus.md` §5 — the conduit model; a template step is
  `S(P)`, a record is the `R`
- `docs/design/procedures/procedure_schema.md` §4.2 — the step-type grammar; the nesting
  structure behind Risk 1
- `docs/design/core/beliefbase_architecture.md` §3.2, §3.6 — codec dispatch
- `src/properties.rs:336` — `Bid::codec_namespace`, the stable-BID-from-string
  pattern step identity uses
- `ISSUE_104_ANNOTATION_VOCABULARY.md` — authoritative for the record field set
- `ISSUE_109_ANNOTATION_LIFECYCLE.md` — run bracketing and folding; consumes the
  step semantics defined here
- `ISSUE_01_SCHEMA_REGISTRY.md`, `ISSUE_18_EXTENDED_PROCEDURE_SCHEMAS.md`,
  `ISSUE_95_WORKSPACE_DECOMPOSITION.md`

## Next Steps After Completion

1. **Issue 109** — folds a record set against a template using the step
   combinators and step BIDs defined here; the immediate consumer
2. **Issue 18** — execution loop, participant-channel observations, deviation
   analysis
3. **Issue 95** — extracts `src/procedures/` into its own crate

Issues 104 and 105 are **not** downstream of this issue and need not wait for it.
