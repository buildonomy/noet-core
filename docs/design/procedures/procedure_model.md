---
title = "The Procedure Model: Templates, Markings, and Runs"
authors = "Andrew Lyjak, Gemini 2.5 Pro, Claude"
last_updated = "2026-09-14"
status = "Draft"
version = "0.1"
dependencies = ["living_corpus.md", "beliefbase_architecture.md"]
---

# The Procedure Model

> [!NOTE]
> **This document describes a target architecture.** Step identity ships today
> (Issue 91B's inline anchor nodes). The lifecycle grammar that reads a marking
> off a record set is specified in
> [`lifecycle_grammar.md`](./lifecycle_grammar.md) and is not built. §7 maps
> every claim to either an implementation site or the issue that will build it.

## 1. Purpose

A procedure says how something is done. This document defines what a procedure
*is* in a belief graph, what its state is, and where the boundary falls between
the template and the record of running it.

Three claims carry the whole model:

1. **A procedure is an authored document, not a data structure.** Its steps are
   ordinary nodes with stable identities.
2. **A procedure is a potentialized annotation** — structural content whose
   subject is execution, awaiting an actor.
3. **A procedure's state is a marking derived from records, never stored.**

Everything else follows from these, including the absence of several things a
reader might expect: there is no procedure file format, no run object, no
execution database, and no as-run record type.

**Scope.** This document owns the model. The directive grammar that expresses a
lifecycle is [`lifecycle_grammar.md`](./lifecycle_grammar.md); how a step
declares the observation that discharges it is
[`observation_model.md`](./observation_model.md); the delta between a template
and what was done is [`deviation_model.md`](./deviation_model.md). The record
store, run brackets, and the fold are Issue 105's.

## 2. A Procedure Is a Document

A procedure is a markdown document. A step is a node in it.

This is not a simplification of a richer model — it is the whole mechanism.
Issue 91B makes any `{#anchor}` block a node with a stable BID at the correct
depth in the Section hierarchy, round-tripped losslessly. Everything a procedure
format would need to supply — node generation, parent-child structure, stable
step identity, source round-trip — is therefore already supplied by the markdown
codec.

````markdown
## Change plan lifecycle {#plan-lifecycle}

- {#drafted} Proposed text is complete.
- {#placed} Target section is pinned.
- {#packaged} Rendered as a change request.
- {#submitted} Handed to the process owner.
````

Four steps, four nodes, four stable BIDs, containment edges to the parent, and a
document a human can read. A second parser producing the same node shape from a
structured file body would be a second authoring surface for one concept.

**The consequences are worth stating explicitly**, because each is a feature
that would otherwise need designing:

| Property | Why it holds |
|---|---|
| A step is addressable | it is a node; it has a BID |
| A step can be annotated | annotations anchor to nodes (`living_corpus.md` §4) |
| A step can be reviewed, versioned, diffed | it is source under version control |
| A lifecycle is user-definable without a code change | authoring a document is not a code change |
| A procedure can be queried | `query_model.md` applies with no extension |

The last two matter most. `attestation_fabric.md` §6 resolves a `record_kind` to
a lifecycle by pointing at an authored document rather than embedding a state
machine in a registry entry, precisely so that a team can define
`local:safety:hazard-review:v1` as markdown and have the definition travel with
the corpus (`living_corpus.md` §4).

### 2.1 Steps declare, they do not sequence

A step declares **what satisfies it**. It does not name what runs next.

There is no program counter and nothing enables anything. A procedure is a
constraint system over its steps, and "ordered" is a legality predicate on each
arriving record rather than a scheduler. This is what lets the same template
describe a strictly sequential checklist and a set of independent obligations
without two grammars.

The predicate language that expresses this — exit predicates over a resolved
node set, discriminated outcomes, and the effects an outcome has on a marking —
is [`lifecycle_grammar.md`](./lifecycle_grammar.md).

## 3. A Procedure Is a Potentialized Annotation

`../../essays/engineering_model_ontology.md` §3.3 separates the causal agent `P`
from the structural content whose subject is execution, $S_P$. A written
procedure is not `P`; it is $S_P$. The actor acting is `P`, and that happens
outside the graph. The record the actor emits is the `R` proving it happened.

A compiled graph is inert, so it cannot contain `P`. What it can contain is a
**declared conduit** along which an actor is expected to act — and a procedure
is exactly that, with steps (`living_corpus.md` §5).

| State | Shape | Meaning |
|---|---|---|
| Potentialized | the authored procedure and its steps | "an actor of this kind is expected to act here" |
| Actualized | a record citing the step | "this actor did act, and here is what resulted" |

The payoff is that **gap analysis is a plain graph query**. Both endpoints are
graph-resident: the template arrives by parsing source, the actualization by the
record → `BeliefEvent` projection. "Which declared steps have no discharging
record?" is therefore the ordinary complement over Pragmatic edges that
`attestation_fabric.md` §12.1 describes for coverage — no joining of two data
sources at query time.

A corpus states not only what it contains but what work it expects, and the
difference is computable.

## 4. State Is a Marking, Derived from Records

### 4.1 The log is the truth; the state is a fold

A procedure's state is not stored. It is computed from an append-only record set
whenever someone looks.

This is the general pattern every layer of the system follows — a durable store
plus a live projection, with the projection always reconstructible
(`living_corpus.md` §3). For a procedure the store is the record set and the
projection is the marking.

Immutability is what makes it work. A record is never edited; a transition is a
*new record citing the prior one* via `caused_by`, and the current state is the
fold over that chain. Two properties fall out that a mutable status field cannot
offer:

- **The journey is recoverable and the state is clean.** The log keeps every
  step, correction, and reversal; the marking says only what is true now.
- **Merge is set union.** Records are immutable with globally unique IDs — a
  G-Set — so two people folding the same records derive the same state and
  concurrent writers cannot conflict (`living_corpus.md` §2).

Issue 105's `Fold` is where this is built: `partition` groups records by run,
`derive` folds a partition against a lifecycle, and `project` turns the result
into a graph node whose payload carries the derived state.

### 4.2 Marks live on leaves; interior state is computed

A parent step's state is its exit predicate applied to its declared set,
evaluated at read time. Only leaves carry marks.

This is what makes a **cycle** cheap. A rejection is not a jump or a back-edge:
it is an outcome whose effect is to **clear** the marks of some other steps. A
third rejection produces a marking identical to the first, so **the state space
is bounded by the template regardless of how many times a cycle runs**. That is
a termination argument by construction, and it is stronger than any fuel guard.

Clearing is the template-scale form of the squash that Issue 105 defines at
record scale. Both are lossy in the same direction, and deliberately: the log
keeps the journey, the current state says what is.

### 4.3 `derive` is pure over (record set, corpus version)

> **No wall clock.** Two readers folding an identical record set must derive an
> identical state.

Anchor resolution qualifies — it is a function of corpus state at a version. So
does staleness, which is a content-hash comparison. **A timeout does not.** Two
readers folding the same records at different moments would disagree, which
breaks the set-union merge the whole store rests on. A timeout is not a weak
transition; it is a non-deterministic one.

A "default if unanswered" disposition is therefore **payload** — a standing
instruction to whoever looks — and the transition happens when a human asserts
it. **Time is a sort key for attention, never an input to state.** Stuck runs
surface on a dashboard ordered by age; they do not change state by age.

### 4.4 An outcome is derived, not read off a field

Exit is *discriminated*, not boolean: a review exits `approved` or `rejected`,
and the discriminant selects the effect.

**The discriminant is a derived enumeration.** It is not a payload field that a
record sets, because more than one kind of thing determines it:

| Determinant | Example | Evaluated |
|---|---|---|
| An actor's assertion | a reviewer records `rejected` | at fold, from the record payload |
| A derived condition | the anchor went stale under an in-progress run | at look time, by hash comparison |
| A cited run's state | the question this run depends on came back unanswerable | at fold, in `caused_by`-topological order |

The first two are `living_corpus.md` §4's bracketed-operation/derived-condition
split; the third is Issue 105's cross-run guard. All three are functions of
(record set, corpus version), so the purity rule in §4.3 holds across the whole
set — which is the constraint that keeps the determinant list open without
making the fold non-deterministic.

Reading an outcome off a payload field would collapse this to the first row and
force the other two to be modelled as something else. The grammar for declaring
outcomes and their effects is
[`lifecycle_grammar.md`](./lifecycle_grammar.md).

## 5. Runs

### 5.1 A run is a query, not an object

An operation with duration produces several records, and in an unordered
append-only store nothing groups them. The grouping mechanism is a
`RunStart`/`RunEnd` bracket carrying a `run_id` that the fold partitions on.

So "the run record" is not a row to fetch and mutate. It is the result of
partitioning the record set by `run_id` and folding the partition. **Issue 105
owns run bracketing, `run_id`, nesting, and folding**; this document does not
define them.

There is no separate execution database. In-progress and completed runs alike
live in the same record store as every other annotation, and there is no second
source of truth to reconcile.

### 5.2 Runs nest

A `RunStart` may cite a parent `run_id` and a `task` — the step of the parent's
template this run discharges. A sub-procedure is therefore a child run, and its
relation to the parent projects as a **Section** edge: containment, not
citation.

**A parent folds in a child iff the child's `task` maps to a step in the
parent's template**, and the combining semantics come from that step's exit
predicate rather than from any policy field. A child with no `task` attaches for
provenance and does not affect the parent's state.

Because a step is a node with a stable BID, `task` is stable across template
revision. Issue 105 §Runs nest is authoritative.

### 5.3 Concurrent procedures are expected

Two procedures can be in progress at once, and one observed act can be
consistent with a step in each — powering on equipment may be the first step of
both a startup procedure and a commissioning procedure.

Nothing needs to disambiguate this at record time. Both runs partition cleanly
because their `run_id`s are distinct, and the actor's own later records resolve
which procedure was being executed. Ambiguity is a question for a reader, not an
error for a writer.

## 6. Resources

A step may declare the material resources it draws on. The declaration is a
queryset like any other, and the exit predicate can range over it — a step does
not complete until every resource it declares itself to use is discharged.

### 6.1 Material resources only

> **The inventory model is for material resources — objects, tools,
> consumables, spaces. Never for people.**

```toml
inventory = ["meeting_room", "laptop"]     # material resources
```

Human participation is not a resource that is consumed and released. Modelling a
colleague as inventory would make collaboration a scheduling problem over a pool
of interchangeable units, which misdescribes it badly enough to produce wrong
procedures. Where a procedure needs a person, it needs an *actor*.

### 6.1a Declaring which people, without treating them as inventory

The rule above is about the *relation*, not about vagueness. A multi-stakeholder
procedure must be able to state that a step needs a safety reviewer and a
structures engineer, or it cannot describe the work it exists to describe — and
"the record names who acted" is retrospective, which is the wrong tense for a
declaration.

**A step declares a role, and the exit predicate ranges over its holders.** The
declared set is the role ($S_P$, no actor bound); the discharged set is the
actors who emitted runs ($R$); the exit predicate compares them
([`../annotation/living_corpus.md`](../annotation/living_corpus.md) §5). "Two of
three reviewers" is `{exit} n-of :count: 2` over a queryset resolving the role —
§6.2's coverage primitive in its agential tense, not a second mechanism.

This differs from inventory in the way that matters:

| | Material resource | Role |
|---|---|---|
| Declared as | a specific thing, or a pool of equivalents | a **capability predicate** over people |
| Discharged by | allocation, then release | an actor choosing to act, and saying so |
| Who decides which one | the scheduler | the people holding the role |
| After the step | returned to the pool | nothing is returned; a record exists |

A role names *what competence the work requires*. It does not name people, does
not reserve them, and does not make them interchangeable — which of its holders
acts is theirs to settle, and the record says who did. That is the difference
between declaring a requirement and allocating a unit.

**Who holds a role is not this document's mechanism.** A role is a node, holders
reach it by Section edges, and the grant is a peer-attested credential
(`ISSUE_112_CREDENTIALS_AND_PROMOTION.md`). A procedure names the role and stops
there — which is what keeps a template portable between organizations whose
people differ.

> **The declaration is a floor, not a gate.** An unsatisfied role predicate is a
> gap in a coverage query, not a refused write: the store is append-only and
> someone without the credential can still act and still be recorded. What the
> corpus reports is that the act did not meet the declared standard — which is
> the finding, and is strictly more useful than having prevented the record.

### 6.2 Declared inputs versus drawn-from inputs

A step declares what it will use; a record cites what it actually drew from. The
exit predicate is the comparison between the two.

These are the same relation in two tenses, and they are deliberately **not**
merged. The declaration is $S_P$ — the conduit. The citation is `R` — the
traversal. Collapsing them would erase the gap that gap analysis measures.

The general primitive underneath is **coverage of a declared set by a discharged
set**, which is what `{maps_to}` and the traceability matrix already compute for
requirements. Whether a coverage matrix and a procedure marking are literally one
operation at two scales is an open question, recorded in §8.

### 6.3 Tension

A resource can accumulate state over time — wear, usage debt, cleanliness — and a
step can resolve it. This is what makes a maintenance procedure expressible: the
trigger is a threshold on accumulated tension rather than a date.

Tension is a property of the resource node, and resolving it is an effect of a
step. Nothing about it is procedure-specific machinery; it is ordinary node
payload read by an ordinary predicate.

## 7. Architecture Map

What exists today, and what is owed.

| Element | Status |
|---|---|
| A step is a node with a stable BID | **Implemented** — Issue 91B, inline anchor nodes |
| Section containment between steps | **Implemented** — markdown codec |
| Fenced directives with `:key:` options | **Implemented** — `parse_directive_options`, `src/codec/myst.rs:659` |
| Query shorthands for traversal | **Implemented** — `src/query/parser.rs:414` |
| Payload reachable from the query grammar | **Implemented** — `resolve_property_path`, `src/query/spec.rs:580` |
| Third-party edge ownership (the conduit mechanism) | **Implemented** — `WEIGHT_OWNED_BY`, `src/properties.rs:626` |
| `{exit}` / `{outcome}` directives, marking semantics | **Issue 17** — see [`lifecycle_grammar.md`](./lifecycle_grammar.md) |
| `redline` and `ask` record kinds | **Issue 104** — general annotation kinds, not procedural ones |
| Record store, run brackets, the fold | **Issue 105** |
| Role nodes, credentials, and who holds a role (§6.1a) | **Issue 112** — a procedure names a role; it never enumerates people |
| Annotation record field set | **Issue 104** |
| Rendering a redline as a change package | **Issue 74** — a diff with provenance; needs no write authority. See [`../annotation/redline_model.md`](../annotation/redline_model.md) |
| Enacting a redline as a source edit | **Issue 106**, over **Issue 107**'s write-back path |
| Anything that advances, checks, or organizes a run | **Issue 18** — undesigned; see §9 |

## 8. Open Questions

1. **Does the coverage reading hold?** Declared-set versus discharged-set per
   owner is what the traceability matrix computes (§6.2). If a coverage matrix
   and a procedure marking are one operation at two scales, the implementation
   should share a path; if they differ, the difference needs recording.
2. **Is a traversal composed with a payload predicate expressible in
   `query_model.md` §9.5 today?** `payload.x > n` parses and resolves; the
   composed form is what needs confirming. If it is not expressible, the gap
   belongs to the query model.
3. **May one actor discharge two roles in one step?** A person holding both
   `safety-reviewer` and `structures-engineer` could satisfy a two-role
   predicate alone. Sometimes correct — a small team — and sometimes exactly
   what the declaration meant to prevent, since independent review by one
   person is not independent. The predicate cannot tell which is intended, so
   the template must say. Whether that is a flag on `{exit}` or a distinct
   combinator is undecided; `attestation_fabric.md` §4.4's independence
   protocols are the neighbouring mechanism and may already cover it.

## 9. Execution Is Not Designed {#execution-stub}

> **Stub — Issue 18.** This section marks a boundary, not a design.

Sections 2–6 say what a procedure is, where its state comes from, and how a
record discharges a step. They do not say how a procedure is **advanced,
checked, or organized while it runs**. Nothing in this document presumes a
running engine, and no component of one is specified anywhere in
`docs/design/procedures/`.

What such a component would have to account for, drawn from what the model
already implies:

- Presenting a reader with the steps a marking leaves open, ordered by
  attention — with age as a sort key and never as a state input (§4.3)
- Noticing when an arriving record is one the lifecycle forbids, and surfacing
  it as a diagnostic rather than refusing the write (Issue 105 §Failure modes)
- Grouping and reviewing pending records before they are folded
- Resolving which of several in-progress procedures an ambiguous act belongs to
  (§5.3), given that the writer need not decide

**Issue 18 owns this and has chosen none of its candidate directions** — a
semantic organizer over a record queue, a fold executor, and a record-queue
linter are alternatives, not a list to build. The related stubs are
[`observation_model.md` §Consuming a detection](./observation_model.md#consumer-stub)
and
[`deviation_model.md` §Detection is not designed](./deviation_model.md#detection-stub).

Do not start by writing a design document here. This directory is one coherent
architecture; work extends it at the stub points.

## 10. Design Principles

- **Record reality, don't predict it.** What happened is data; what it means is
  a separate, later, and revisable judgement.
- **Immutable history.** A transition is a new record citing the prior one,
  never an edit.
- **Deviations are explicit.** The delta between template and reality is
  first-class data, not an error state ([`deviation_model.md`](./deviation_model.md)).
- **The actor is the authority.** A human assertion overrides an inference.
- **Derived, never stored.** Any state that can be computed from the record set
  is computed from the record set.
- **General-purpose.** Nothing here is specific to a domain: manufacturing SOPs,
  lab protocols, deployment runbooks, emergency response, and editorial review
  are the same object.

## 11. References

- [`lifecycle_grammar.md`](./lifecycle_grammar.md) — exit predicates, outcomes, marking semantics
- [`observation_model.md`](./observation_model.md) — how a step declares what observation discharges it
- [`deviation_model.md`](./deviation_model.md) — the template-versus-reality delta
- [`../annotation/redline_model.md`](../annotation/redline_model.md) — proposing a change; a deviation may motivate one
- [`procedures_vs_alternatives.md`](./procedures_vs_alternatives.md) — why this shape rather than a notebook or a DAG
- [`../annotation/living_corpus.md`](../annotation/living_corpus.md) §2 (annotations as a privileged subset of `R`), §4 (assert vs. mutate; stateful annotations), §5 (conduits; $S_P$)
- [`../core/beliefbase_architecture.md`](../core/beliefbase_architecture.md) §4.3 — `Envelope` and `Annotation`
- [`../core/query_model.md`](../core/query_model.md) — the query algebra a predicate ranges over
- [`../codecs/myst_directive_architecture.md`](../codecs/myst_directive_architecture.md) — the directive registry
- [`../../essays/engineering_model_ontology.md`](../../essays/engineering_model_ontology.md) §3.3–§3.4 — `P`, $S_P$, and `R`
- `ISSUE_17_NOET_PROCEDURES_EXTRACTION.md` — the lifecycle grammar
- `ISSUE_104_ANNOTATION_VOCABULARY.md` — the record field set
- `ISSUE_105_RECORD_STORE_AND_FOLD.md` — the store, run brackets, and the fold
- `ISSUE_18_EXTENDED_PROCEDURE_SCHEMAS.md` — execution, undesigned
