---
title = "The Deviation Model: Comparing As-Run Against Template"
authors = "Andrew Lyjak, Claude"
last_updated = "2026-09-14"
status = "Draft"
version = "0.1"
dependencies = ["procedure_model.md", "redline_model.md"]
---

# The Deviation Model

> [!NOTE]
> **This document describes a target architecture.** The comparison it defines
> is a fold output (Issue 105); nothing performs it yet, and what *notices* a
> deviation is undesigned (§6).

## 1. Purpose

Templates specify idealized behaviour. Executors exhibit systematic variation.
A **deviation** is the recorded difference between what a template declared and
what the records show.

This document defines what a deviation is, the vocabulary it needs, and how a
corpus of them is analysed. It is general across domains: manufacturing SOPs,
lab protocols, deployment runbooks, emergency response, and editorial review all
produce the same object.

**A deviation is an observation, not a proposal.** It says what differed. When a
pattern of deviations is judged to mean the *template* is wrong, that judgement
is expressed as a redline — a general annotation kind that has nothing to do
with procedures ([`../annotation/redline_model.md`](../annotation/redline_model.md)).
§5 is the seam. Keeping the two separate is what lets a deviation be recorded
without implying anyone has yet decided what to do about it.

## 2. The Template-versus-Reality Gap

A template says:

```
preheat_oven → mix_ingredients (5-10 min) → pour_batter → bake (25-30 min)
```

The records say:

```
preheat_oven          10:00
mix_ingredients       10:15   (18 min — longer than the template allows)
bake                  10:35   (28 min)
```

`pour_batter` has no discharging record, and `mix_ingredients` ran past its
declared bound.

### 2.1 Why the delta is worth keeping

A single deviation is an incident. A *systematic* deviation is information about
the template:

| Pattern | What it suggests |
|---|---|
| A step is consistently skipped | the template is over-specified, or the step has been automated |
| A step is consistently added | the template is missing a real operation |
| Durations consistently differ | the declared estimates are wrong |
| A resource is consistently substituted | the template assumes something unavailable |
| Steps are consistently reordered | a declared dependency is not real |

Each reading converts an apparent compliance failure into a document defect.
That inversion is the point: the corpus learns from execution instead of merely
auditing it.

## 3. The Comparison Is a Fold Output

A deviation is not separately computed. It falls out of the marking a template
produces against the records that discharged it
([`procedure_model.md`](./procedure_model.md) §4).

The fold already partitions records by run, evaluates each step's exit predicate
over its declared set, and derives state. A step whose predicate is unsatisfied
when the run closes, a discharge the `ordered` predicate declares illegal, a
record whose captured value falls outside a declared range — each is already
visible to `derive`. Issue 105 requires an illegal transition to be *recorded*
rather than refused, which is precisely the deviation record.

**This is why no deviation-detection subsystem is needed.** The information is a
byproduct of deriving state. What is undesigned is the component that reads it
and decides something should be surfaced (§6).

### 3.1 Deviation vocabulary

The kinds of delta the comparison must be able to express:

| Deviation | Meaning |
|---|---|
| `skipped` | a declared step has no discharging record |
| `reordered` | steps were discharged in an order the predicate declares illegal |
| `added` | an act was recorded that no declared step covers |
| `duration_mismatch` | a step ran outside its declared bound |
| `resource_substitution` | a resource other than the declared one was drawn from |
| `quality_violation` | a captured value fell outside its declared range |

This is **payload vocabulary**, not a type and not a set of event kinds. The
list is open: an unrecognized value is structurally valid and fails loudly at
fold time, per the open-enum rule in
[`lifecycle_grammar.md`](./lifecycle_grammar.md) §2.

A deviation references the step it concerns by **stable step BID**, never by
positional index, so the reference survives revision of the template
([`procedure_model.md`](./procedure_model.md) §2).

### 3.2 Attribution conclusions are outcomes, not payload

When an inferred run is put to the actor who performed it, the reply is one of a
small set: *that is what I did*; *I was doing a different procedure*; *roughly,
but I changed things*; *that did not happen*.

**These are outcome discriminants, not payload fields.** They are concluded
about a run rather than described in a record, and their determinants are
plural — an actor's assertion, a stale anchor under an in-progress run, or the
state of a cited run can each produce one
([`procedure_model.md`](./procedure_model.md) §4.4).

The distinction is exact: **a deviation describes a delta; an attribution
conclusion concludes something about a run.** The first is payload because only
the record's author knows it. The second is derived because more than the
record's author determines it.

### 3.3 Confirmation is a record, not an edit

Putting an inferred run to its actor produces **a new record**, whichever way
the actor replies. Nothing updates a run in place, because no such operation
exists. "Marking the original run a false positive" is a later record citing it,
and the reading a user sees is the fold over the chain.

This is what makes the flow auditable without extra machinery: a disagreement
between an inference and its subject is preserved, attributable on both sides,
and queryable afterwards.

## 4. Analysis Is a Query, Not an API

The questions a deviation corpus answers are the reason to keep one:

- Which steps are most often skipped?
- How does actual duration compare to what the template declares?
- Which templates have the highest deviation rates?
- Do deviation patterns differ between actors?

**These are answered by [`../core/query_model.md`](../core/query_model.md), and
there is no bespoke Rust API.** The fold projects each run to a graph node
carrying its derived state in its payload (Issue 105 §Project), and
`resolve_property_path` (`src/query/spec.rs:580`) reaches payload from the query
grammar. So:

| Question | How it is answered |
|---|---|
| Runs of a procedure | traversal from the template node to the run nodes Pragmatic toward it |
| Runs in a time range | that set, filtered on the envelope's `observed_at` |
| Runs that deviated | that set, filtered on a payload predicate over derived state |
| Frequency of a deviation per step | the same set, grouped by the step BID a deviation names |
| Comparison across actors | the same set, grouped on the envelope's `actor` |

Selection is the query model's; aggregation and presentation belong to a `View`
(`query_model.md` §7). A parallel API of typed accessors would fork the query
layer for one domain, and would need re-deriving every time the record shape
moved.

> **Measuring duration is not a wall-clock transition.** Reporting how long a
> run took compares two recorded `observed_at` values, which is a pure function
> of the record set. The purity rule bars time as an *input to state*
> ([`procedure_model.md`](./procedure_model.md) §4.3), not as a reported field.

## 5. From Deviation to Redline

A deviation observes. A redline proposes. The step between them is a
**judgement**, and it is deliberately not automatic.

```
records + template ──fold──> deviations ──judgement──> a redline ──> a change package
                                           (a human decides
                                            the template is wrong)
```

The judgement is where the information changes direction. Up to that point the
data says "execution differed from the document"; afterwards it says "the
document should change." Those are different claims with different authors, and
collapsing them would make the system silently rewrite templates to match
whatever people actually did — including the mistakes.

So a redline motivated by deviations is an **ordinary redline**: same record
kind, same file-map payload, same anchoring, same exits
([`../annotation/redline_model.md`](../annotation/redline_model.md)). What the
deviation history supplies is the *evidence*, cited via `caused_by`. Nothing
about the redline is procedural, and nothing in the redline model needs to know
a procedure was involved.

**What makes a pattern worth promoting is unowned.** How many consistent runs,
how much variance, whether an actor must request it — no document answers this,
and §7 records it as open.

## 6. Detection Is Not Designed {#detection-stub}

> **Stub — Issue 18.** This section marks a boundary, not a design.

§3 says the comparison is a fold output, so the information exists. What is
undesigned is the component that reads it, decides a deviation is worth
surfacing, and puts it to someone.

What such a component would have to account for:

- Distinguishing a deviation from a run still in progress: an undischarged step
  is only "skipped" relative to a judgement that the run is over, and a run has
  no automatic expiry (Issue 105 §Failure modes)
- Severity. A skipped safety check and a skipped formatting pass are not the
  same event, and nothing currently distinguishes them
- Which actor a deviation is put to, and when
- Aggregating across template revisions, given that a template can change
  between runs

**Issue 18 owns this and has chosen none of its candidate directions.** The
related stubs are
[`procedure_model.md` §Execution is not designed](./procedure_model.md#execution-stub)
and
[`observation_model.md` §Consuming a detection](./observation_model.md#consumer-stub).

Note that [`observation_model.md`](./observation_model.md)'s `inference_hint`
machinery is the natural tool for part of this: a deviation is a pattern over an
observation stream, and the schema for declaring "what to match" already exists.

## 7. Open Questions

1. **Severity.** Should some deviations be distinguishable as critical? Nothing
   in the vocabulary does so today.
2. **Template revision.** How should analysis aggregate runs against a template
   that changed between them? Step BIDs are stable, so the join is possible; the
   presentation question is open.
3. **Multi-actor patterns.** When actors deviate differently, is that a template
   defect, a training signal, or both — and can the corpus tell?
4. **When is a pattern worth promoting?** Unowned (§5). A threshold on
   consistent runs is the obvious shape, but nothing establishes the number, and
   an actor-requested promotion may make a threshold unnecessary.

## 8. Design Principles

- **The delta is first-class data.** A deviation is information about the
  template, not a compliance failure to suppress.
- **Observation is not proposal.** Recording a difference commits no one to
  changing anything.
- **The actor is the authority.** A human assertion overrides an inference.
- **No silent learning.** Deviation patterns inform humans; they never
  automatically rewrite a template.
- **Immutable records.** A correction cites; it never edits.

## 9. References

- [`procedure_model.md`](./procedure_model.md) — what a procedure is; §4.4 on derived outcomes
- [`lifecycle_grammar.md`](./lifecycle_grammar.md) — exit predicates, whose evaluation produces the comparison
- [`observation_model.md`](./observation_model.md) — declaring what discharges a step
- [`../annotation/redline_model.md`](../annotation/redline_model.md) — the general proposal kind a deviation may motivate
- [`../core/query_model.md`](../core/query_model.md) — how the analysis questions are answered
- `ISSUE_105_RECORD_STORE_AND_FOLD.md` — the fold that produces the comparison
- `ISSUE_18_EXTENDED_PROCEDURE_SCHEMAS.md` — detection, undesigned
