# Issue 18: Procedure Execution — Aspirational Stub

**Priority**: LOW
**Estimated Effort**: Not estimable — nothing here is designed yet
**Dependencies**: Issue 104 (annotation record field set), Issue 105 (record
store), Issue 105 (run bracketing and folding) — these are the annotation model
this issue must be re-designed against. Issue 17 **narrowly**, for the procedure
codec and step-type semantics only.
**Blocks**: None

> [!WARNING]
> **Aspirational stub. Nothing below is a design commitment.**
>
> This issue's prior draft design has been **withdrawn**. It was built on Issue
> 17's three-piece as-run data model (`TemplateRef`, `ExecutorContext`,
> `AsRunRecord`), which Issue 17 withdrew entirely — those types will not exist.
> The draft cited them throughout, so patching the citations would have preserved
> a design whose foundation is gone. **This issue must be re-designed from the
> annotation model before any implementation.** Nothing below is a specification,
> a scope commitment, or a schedule — it records why the issue still exists, not
> what it will build.

## Summary

Something is still needed above Issue 17's procedure codec and Issue 105's run
bracketing — some way to organize, advance, or check a procedure as it is
executed. What that thing is has not been decided. This stub holds the framing
that survives the withdrawal and the questions that block re-design.

## Resuming this issue

**Issue 17 step 4 consolidates the procedural design-doc space and leaves named
stubs for this issue's territory.** Read those stubs first — they are the entry
point, and they define the shape of the space this issue fills.

**Issue 17 step 4 has landed.** Three named stubs mark this issue's territory,
each stating what a component there would have to account for:

| Stub | Question it holds open |
|---|---|
| [`procedure_model.md` §9](../../design/procedures/procedure_model.md#execution-stub) | How is a procedure advanced, checked, or organized while it runs? The primary entry point |
| [`observation_model.md` §Consuming a detection](../../design/procedures/observation_model.md#consumer-stub) | What watches a channel, evaluates a pattern, and decides a match occurred? |
| [`deviation_model.md` §6](../../design/procedures/deviation_model.md#detection-stub) | Who notices a deviation, and decides it is worth surfacing? |

Read `procedure_model.md` in full first — it is the entry point for the whole
group, and the three stubs only make sense against the model they bound.

- [x] Stub locations recorded here (filled in by Issue 17 step 4)
- [ ] Confirm the stubs still describe territory this issue wants — the
      candidate directions below are undecided, so a stub may turn out to be for
      work this issue declines

**Do not begin by writing design docs.** Issue 17 owns the procedural design-doc
space and consolidated it precisely so it reads as one coherent architecture.
Work here extends that set at the stub points; it does not start a parallel one.

**Historical note belongs here, not in design docs.** Issue 17 step 4's mandate
is that design docs are pedagogic about what *is*. Where the withdrawn execution
model needs recording — for provenance, or to avoid re-deriving a rejected
approach — it is recorded in this issue. The relevant history: the prior draft
defined `ProcedureRun`, `ExecutionRecord`, `CorrectionEvent`, `DeviationReport`,
and `ObservationEvent` as bespoke types, and their withdrawal is explained in
§"Why the Prior Design Was Withdrawn" below. `CorrectionType` and
`DeviationType` survive as **payload vocabulary** for the redline `record_kind`
that Issue 104 registers — they enumerate distinctions a redline payload must
express, and are not event types. `DeviationType` in particular belongs to the
as-run comparison (`docs/design/procedures/deviation_model.md` §3.1), not to the
redline kind itself.

## Why the Prior Design Was Withdrawn

The draft assumed it would define runtime run types, event payloads, and a
serialization format for completed runs. Under the living-corpus model it does
not need to: **an annotation *is* an as-run record**
(`docs/design/annotation/living_corpus.md` §2), and a procedure instance is the set of
annotations sharing a common `RunStart` ancestor — a *query* over the annotation
store, not a new type (Issue 105). The record field set is Issue 104's, the store
and its bracketing and folding are Issue 105's. Redline promotion into a
source edit is **Issue 106**, over the write-back path in **Issue 107**.

Also withdrawn: the run-index and claim/evidence persistence design. It described
how to query "runs of procedure X" from run-header records, but what a run header
*is* is now Issue 105's to define (`RunStart` is the header; `run_id` is its
`EventId`). Re-deriving it here would fork that definition.

## Withdrawn Component Inventory

Issue 17 step 4 removed `procedure_execution.md`, whose sound passages were
harvested into the consolidated design docs. What follows is the part that was
**not** harvested: the components of the withdrawn execution design. It is
recorded here because this issue owns the territory, and because a replacement
should account for each item or explicitly decline it — not because any of it is
a specification.

**A five-state machine** (Inactive → Triggered → Active → Completed/Aborted).
Withdrawn as a *shape*, not merely as a detail: a fixed enum of engine-observed
states forecloses the thing the current model requires, which is that **an
authored document is the lifecycle definition**. Do not reintroduce a hardcoded
state set. Step state is a marking, not a position.

**Three engine components.** A *Trigger Watcher* evaluating context conditions
to activate procedures; an *Active Monitor* matching events to expected steps
and maintaining a run record; a *Hypothesis Generator* formatting a completed
run for confirmation. The responsibilities are plausible requirements; the
decomposition is not a commitment. Note that "create and update the run record"
is not an operation the annotation model has — records are immutable.

**Context-based triggering.** Time-of-day, day-of-week, and during-action
conditions that would activate a procedure. Unowned. Note the constraint: a
scheduled *activation* is not the same as a wall-clock *transition*, which the
purity rule bars (`procedure_model.md` §4.3). Something that proposes a run at
08:00 is fine; something that changes a run's state because time passed is not.

**A confirmation flow.** Presenting an inferred run to its actor with
confirm / modify / wrong-procedure options. The user-facing shape survives in
`deviation_model.md` §3.3; what is undesigned is who presents it and when.

**A query API of typed accessors** (`get_runs`, `average_duration`,
`executor_success_rate`, and similar). Explicitly **declined**, not deferred:
Issue 17 step 4 decided these are queries over the record store expressed in
`query_model.md`, with aggregation belonging to a `View`. Do not reintroduce a
parallel typed API for this domain.

**Learning and prediction extensions** — probabilistic step matching, duration
prediction, adaptive templates. Out of scope as downstream-product concerns, and
still out of scope.


## What Survives

**A prompt is not a step type.** Every procedure step advances via an observation
event, whether the observer is a sensor, a system, or a human. A "prompt" is
therefore an observable action with a channel, a producer, and a response
configuration — not a distinct kind of step. One state machine for all
observations, identical recording regardless of source, and multi-modal patterns
(automatic reading *or* manual entry) that fall out rather than being
special-cased.

But it should **fall out of Issue 17's generalized design rather than be
specified here.** A procedure is a potentialized annotation — $S_P$ awaiting a
`P` (`living_corpus.md` §5). A participant-channel step is one specific, and
probably over-specified, case of that projection: a potentialized procedural
content designator with a response shape attached. If the general mechanism is
right, this is an instance of it, not a parallel schema.

**Runs live in the sidecar** — in-progress and completed alike. There is no
separate execution database and no second source of truth.

## Candidate Directions

Not decided — alternatives to choose between, not a list to build:

- A **semantic organizer/editor over an annotation queue** — a surface for
  reviewing, grouping, and revising pending records before they are folded.
- The **fold executor** — the component that actually runs Issue 105's fold.
- An **annotation-queue linter** — a "meta-linter" checking a queue for
  incoherence (records citing missing runs, steps discharged out of order,
  contradictory claims) rather than executing anything.

## What This Issue Must Not Do

- **Do not define record types.** Re-specifying run, step, or deviation records
  here would recreate exactly the coupling that caused this withdrawal. Field set
  → Issue 104. Store, run identity, and folding → Issue 105.
- **The execution-engine scope boundary still holds.** UI rendering, delivery
  strategy, sensor integrations, and behavior prediction remain
  downstream-product concerns.

## Open Questions

- **How do runs propagate to upstream sidecars?** The real unresolved problem. A
  run recorded in a local scope must reach a shared one in some form; the
  candidate mechanism is the scoped-queue/percolation model in
  `docs/design/annotation/federated_belief_network.md` §1.2 — transmitting a folded `RunEnd`
  summary rather than the raw record set. Whether that is a transport filter or a
  read-time policy is open there, and this issue is one of its consumers.
- **What is this issue, actually?** Until one of the candidate directions is
  chosen, it has no scope.

## References

- `ISSUE_17_NOET_PROCEDURES_EXTRACTION.md` — procedure codec, step-type
  combinator semantics; "What Was Removed and Why" for the withdrawn model
- `ISSUE_104_ANNOTATION_VOCABULARY.md` — annotation record field set (authoritative)
- `ISSUE_105_RECORD_STORE_AND_FOLD.md` — the record store, `RunStart`/`RunEnd`, `run_id`, nesting, folding
- `ISSUE_106_SOURCE_WRITE_BACK.md`, `ISSUE_107_CODEC_WRITE_BACK.md` — redline
  promotion and the write-back path, formerly drafted here
- `../../design/procedures/procedure_model.md` — the consolidated model these
  stubs bound; read first
- `../../design/annotation/living_corpus.md` §2 (annotations as a privileged subset of `R`),
  §5 (conduits; $S_P$ and potentialized annotations)
- `../../design/annotation/federated_belief_network.md` §1.2 — scoped queues and percolation
