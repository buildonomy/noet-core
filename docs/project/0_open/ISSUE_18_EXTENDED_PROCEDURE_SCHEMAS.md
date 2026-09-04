# Issue 18: Procedure Execution — Aspirational Stub

**Priority**: LOW
**Estimated Effort**: Not estimable — nothing here is designed yet
**Dependencies**: Issue 104 (annotation record field set), Issue 105 (record
store), Issue 109 (run bracketing and folding) — these are the annotation model
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

Something is still needed above Issue 17's procedure codec and Issue 109's run
bracketing — some way to organize, advance, or check a procedure as it is
executed. What that thing is has not been decided. This stub holds the framing
that survives the withdrawal and the questions that block re-design.

## Resuming this issue

**Issue 17 step 4 consolidates the procedural design-doc space and leaves named
stubs for this issue's territory.** Read those stubs first — they are the entry
point, and they define the shape of the space this issue fills.

When Issue 17 step 4 lands, this section must be updated to name the specific
stub locations it created. If it has landed and this list is still empty, that
is a defect in Issue 17, not a reason to start designing here:

- [ ] Stub locations recorded here (filled in by Issue 17 step 4)
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
`DeviationType` survive as **payload vocabulary** for the redline `protocol_id`
that Issue 17 step 2a registers — they enumerate distinctions a redline payload
must express, and are not event types.

## Why the Prior Design Was Withdrawn

The draft assumed it would define runtime run types, event payloads, and a
serialization format for completed runs. Under the living-corpus model it does
not need to: **an annotation *is* an as-run record**
(`docs/design/annotation/living_corpus.md` §2), and a procedure instance is the set of
annotations sharing a common `RunStart` ancestor — a *query* over the annotation
store, not a new type (Issue 109). The record field set is Issue 104's, the store
is Issue 105's, bracketing and folding are Issue 109's. Redline promotion into a
source edit is **Issue 106**, over the write-back path in **Issue 107**.

Also withdrawn: the run-index and claim/evidence persistence design. It described
how to query "runs of procedure X" from run-header records, but what a run header
*is* is now Issue 109's to define (`RunStart` is the header; `run_id` is its
`EventId`). Re-deriving it here would fork that definition.

## What Survives

**A prompt is not a step type.** Every procedure step advances via an observation
event, whether the observer is a sensor, a system, or a human. A "prompt" is
therefore an observable action with a channel, a producer, and a response
configuration — not a distinct kind of step. One state machine for all
observations, identical recording regardless of source, and multi-modal patterns
(automatic reading *or* manual entry) that fall out rather than being
special-cased.

But it should **fall out of Issue 17's generalized design rather than be
specified here.** A procedure is a potentialized annotation — `S(P)` awaiting a
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
- The **fold executor** — the component that actually runs Issue 109's fold.
- An **annotation-queue linter** — a "meta-linter" checking a queue for
  incoherence (records citing missing runs, steps discharged out of order,
  contradictory claims) rather than executing anything.

## What This Issue Must Not Do

- **Do not define record types.** Re-specifying run, step, or deviation records
  here would recreate exactly the coupling that caused this withdrawal. Field set
  → Issue 104. Store → Issue 105. Run identity and folding → Issue 109.
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
- `ISSUE_105_ANNOTATION_SIDECAR_STORE.md` — the record store
- `ISSUE_109_ANNOTATION_LIFECYCLE.md` — `RunStart`/`RunEnd`, `run_id`, nesting, folding
- `ISSUE_106_SOURCE_WRITE_BACK.md`, `ISSUE_107_CODEC_WRITE_BACK.md` — redline
  promotion and the write-back path, formerly drafted here
- `../../design/annotation/living_corpus.md` §2 (annotations as a privileged subset of `R`),
  §5 (conduits; `S(P)` and potentialized annotations)
- `../../design/annotation/federated_belief_network.md` §1.2 — scoped queues and percolation
