# Issue 17: Procedure Lifecycle Grammar

**Priority**: HIGH
**Estimated Effort**: 3.25 days (RELATIVE COMPARISON ONLY) — 0.5 for the
directive registrations, 1.25 for the field-set derivation, 1.5 for consolidating
the procedural design-doc space
**Dependencies**: Issue 1 (Schema Registry), Issue 2 (Section Metadata), Issue
91B (inline anchor nodes — **completed**; supplies step identity)
**Blocks**: Issue 105 (record store and fold) — for the exit-predicate semantics
and the marking model below
**Related**: Issue 104 (annotation vocabulary) is a *sibling*, not a dependant —
it registers `redline` and `ask`; this issue supplies their field sets (step 2a);
Issue 95 owns the eventual crate split; Issue 106 owns promotion into a source
edit

> [!IMPORTANT]
> **No `.procedure` codec.** The lifecycle grammar is a set of **MyST
> directives** over ordinary markdown. Issue 91B's inline anchor nodes already
> give every step a node with a stable BID, so the codec this issue was named
> for has no work left to do. See "What Was Removed and Why".

## Summary

Deliver a **lifecycle grammar**: directive registrations that let an authored
markdown document define a state machine, plus the **evidence** — field sets and
observed lifecycles mined from real records — that Issue 104 needs to register
the kinds which drive one.

The grammar factors the old nesting operators into their two halves — an **exit
predicate** over a resolved node set, and a discriminated **outcome** with an
effect. That factoring makes a cycle expressible without a second grammar, makes
a cross-run guard fall out of the existing query language, and dissolves the
codec.

## Goals

1. Register `{exit}` and `{outcome}` as directives; define the marking semantics
   a fold applies
2. Let exit predicates range over an arbitrary **queryset**, defaulting to
   containment children
3. Express a cycle as an outcome that **clears marks**, with no back-edge and no
   transition construct
4. Keep `derive` a **pure function of (record set, corpus version)** — no wall
   clock
5. Supply Issue 104 with the **`redline` and `ask` field sets**, derived from
   real records, and verify the grammar can express their lifecycles
6. Add **no new record primitives** and **no new codec**

## Scope: what this issue is not

| Concern | Owner |
|---|---|
| Annotation record field set | **Issue 104** — authoritative |
| Record storage, `(bid, version)` anchoring, retention | **Issue 105** |
| Run bracketing, nesting, folding a log into state | **Issue 105** |
| Envelope (`actor`, `observed_at`, `caused_by`) | `beliefbase_architecture.md` §4.3 |
| Execution loop, deviation analysis | **Issue 18** |
| Promotion of a record into a source edit | **Issue 106** |

Issue 105's dependency is narrow: it needs the **exit-predicate semantics** and a
**stable step reference**. Both are supplied without a codec.

## Architecture

### What Was Removed and Why

Two removals, in two rounds. The first withdrew a three-piece "as-run data
model" (template / executor context / as-run record):

| Withdrawn piece | Where it went |
|---|---|
| **Template** | **Survives, demoted.** Not a data model — an authored markdown document plus a two-field reference to it. That reference is a general "node at a content version", owned by Issue 105. |
| **Executor Context** | **Collapses into `Envelope`.** `executor_id` → `actor`; `timestamp` → `observed_at` (`beliefbase_architecture.md` §4.3). `environment` had no named reader and is dropped. |
| **As-Run Record** | **Collapses into a query.** The set of annotations sharing a `RunStart` ancestor (Issue 105). |

The second removes the **`.procedure` codec** and most of the `steps` TOML
schema. A step is a node; Issue 91B makes any `{#anchor}` block a node with a
stable BID at the right depth, round-tripped and tested. Everything the codec was
specified to provide — node generation, parent-child structure, stable step
identity, lossless round-trip — is shipped. A second parser producing the same
node shape from TOML would be a second authoring surface for one concept.

From the prior step schema, these do not survive: `parallel` (once a predicate is
about *completion of the parent*, "unordered" and "all required, any order" are
the same operator), `variables` / `selection_variable` / `stores_in_variable`
(engine state — a prompt response *is* a record, so a predicate reads the
discharging record's payload, not an environment), and `avoid` (a negative
predicate with no stated semantics and no observed use).

> **Do not resurrect `AsRun`.** A struct of that name with `AsRunState` existed
> as dead code in `src/properties.rs` and has been deleted. Its state space is
> not the space a fold produces.

### The factored core

A step declares what satisfies it; records discharge it. Three concepts:

- **Exit predicate** — what makes this step complete, evaluated over a resolved
  node set: `all` / `any` / `ordered` / `n-of`.
- **Outcome** — exit is *discriminated*, not boolean. A review exits `approved`
  or `rejected`. The discharging record names the outcome in its payload.
- **Effect** — what an outcome does to the marking. Default: mark this step with
  that outcome. The only other effect is `clears`.

**State is a marking, not a position.** There is no program counter and nothing
enables anything; the template is a constraint system. `ordered` is a *legality
predicate on each record* — an out-of-order discharge is Issue 105's "illegal
transition" diagnostic, never a scheduler.

**Marks live on leaves; interior state is computed.** A parent's state is its
exit predicate applied to its `over:` set, evaluated at read time. This is what
makes the next point cheap.

**A cycle is an outcome that clears marks.** `rejected` on a review step clears
the marks of the draft steps. Not a jump, not a back-edge, not a traversal —
which is why it needs no transition construct and cannot be a second grammar.

Clearing is the template-scale form of the **squash** that `living_corpus.md`
§Movement defines at record scale. Both are lossy in the same direction: the log
keeps the journey, the current state says what is. A third rejection produces a
marking identical to the first, so **the state space is bounded by the template
regardless of how many times a cycle runs.** That is the termination argument,
and it is stronger than a fuel guard.

### Combinators range over a queryset

The `over:` option is a query string; the default is `composed_of(1)`. Authored
children become the degenerate case.

| `over:` | Reads as |
|---|---|
| `composed_of(1)` (default) | containment children — the old nesting operators |
| `constrained_by(1)` | the step's declared normative inputs |
| `uses(1)` | the step's declared material inputs (`inventory`) |
| a role query | the actors expected to act — the **agential** case, below |
| any query | a set discovered after authoring — an N-ary join |

This answers step 2's standing question about `inventory` versus `caused_by`:
**yes, they are the same relation in two tenses — but they are not merged.** A
step declares inputs ($S_P$, the conduit); records cite what they drew from ($R$,
the traversal); the exit predicate is the comparison. The general primitive is
**coverage of a declared set by a discharged set**. `procedure_model.md` §6.1's
material-resources boundary survives untouched, because nothing is collapsed —
`uses` stays material, `constrained_by` stays normative, and the combinator does
not care which relation it ranges over.

**The same primitive covers actors, which is how multi-signature sign-off
works.** A conduit may declare a **role** rather than a node — "a safety reviewer
is expected to act here" — and "2 of 3 reviewers" is then
`{exit} n-of :count: 2` over a queryset resolving that role. No conduit-level
state machine is needed, and no new grammar: the declared set is the role, the
discharged set is the actors who emitted runs against it, and the exit predicate
is the same comparison.

This is the material/normative pattern in its **agential** tense:

| Tense | Declares | Discharged by | Kind |
|---|---|---|---|
| material (`uses`) | an input a step needs | records citing what they used | Pragmatic |
| normative (`constrained_by`) | a constraint a step answers to | records citing what constrained them | Epistemic |
| **agential** (a role) | an actor *class* expected to act | actor → run edges from actors who acted | **Pragmatic** |

The agential case is Pragmatic for the same reason `uses` is: a role declares an
**expected act** ($S_P$), and a run proves one happened ($R$). An actor is a graph
node with a derived BID, and the edge is
`(source: actor, sink: run, WEIGHT_OWNED_BY: actor)` — see
`docs/design/annotation/living_corpus.md` §5. A *record* may likewise cite another
actor to pull them into the run's scope, which is the agential counterpart of
citing an input and is Pragmatic for the same reason.

**Selection comes from `query_model.md`; combination comes from nesting.** The
predicate leaf therefore gets **no `and` / `or` / `not`** — `all`/`any` over a
nested set already *is* the boolean algebra. This is the structural reason the
`over:` slot cannot grow into a general condition language.

> **Claim to verify, not to build on**: declared-set versus discharged-set, per
> owner, is what `{maps_to}` and the traceability matrix already compute. If it
> holds, a coverage matrix and a procedure state are one operation at two scales.

### Purity: no wall clock

> **`derive` must be a pure function of (record set, corpus version).**

Anchor resolution passes — a function of corpus state at a version. Staleness
passes — a content-hash comparison. **A timeout fails**: two readers folding an
identical record set would derive different states, breaking the set-union merge
the store rests on. A timeout is not a weak transition, it is a
*non-deterministic* one.

Observed "default if unanswered" dispositions are therefore **payload** — a
standing instruction to whoever looks — and the transition happens when a human
asserts it. **Time is a sort key for attention, never an input to state.** Stuck
runs surface on a dashboard ordered by age.

### Cross-run guards need no syntax

Issue 105 §Cross-run guards assigns this issue "the syntax". There is none to
add. The fold derives runs in `caused_by`-topological order and puts derived
state in the run node's payload (105 §Project); `resolve_property_path`
(`src/query/spec.rs:580`) serializes the whole `BeliefNode` and walks it, so
`payload` is reachable from the query grammar and `payload.priority > 3`
round-trips today (`src/query/parser.rs:2838`).

A guard is therefore an `over:` query with a payload predicate on projected run
state. **Issue 105's evaluation half is unchanged; this issue's syntax half
dissolves into the existing grammar.** Confirm the exact spelling of a traversal
composed with a payload predicate against `query_model.md` §9.5 during step 2.

### Authoring surface

Three new directive names over existing machinery: fenced directives with `:key:`
options (`parse_directive_options`, `src/codec/myst.rs:659`), query shorthands
(`src/query/parser.rs:414`), inline-anchor nodes (91B), Section containment.

````markdown
## Change plan lifecycle {#plan-lifecycle}

```{exit} ordered
```

- {#drafted} Proposed text is complete.
- {#placed} Target section is pinned.
- {#packaged} Rendered as a change request.
  ```{exit} all
  :over: constrained_by(1)
  ```
- {#submitted} Handed to the process owner.
  ```{outcome} rejected
  :clears: #drafted
  ```
````

`#packaged` does not complete until every node it declares itself constrained by
is discharged — a blocking relation expressed as a queryset rather than a status
value.

**Open-enum rule**: an unknown combinator or outcome validates structurally and
**fails loudly at fold time**. Same rule for an unknown `record_kind`.

**The one place a guard is genuinely needed**: because `over:` is an arbitrary
query, the *over-graph* can cycle. Exit evaluation and `clears` propagation both
walk it and need a visited set; a cycle in the over-graph is a template lint.
This does **not** apply to the record fold, which is a linear pass over records
and cannot diverge.

### Module Layout

```
src/procedures/
├── lifecycle.rs   # exit predicates, outcomes, marking semantics
└── mod.rs         # directive registration, public API
```

No `codec/`, no `as_run/`. Issue 95 owns crate extraction; do not attempt it
here.

## Implementation Steps

### 1. Directive registration and marking semantics (1 day)

- [ ] Register `{exit}` and `{outcome}` in `DIRECTIVES` (`src/codec/myst.rs`).
      Both are fenced-block, parse-only (`builder: None`) — they configure a
      node, they do not render
- [ ] Parse the combinator argument (`all` / `any` / `ordered` / `n-of` with
      `:count:`) and options (`:over:`, `:outcome:`, `:clears:`)
- [ ] Store the parsed lifecycle on the step node's `payload` so the fold reads
      it without re-parsing source
- [ ] Implement exit evaluation over a resolved `over:` set, with a visited set
      against over-graph cycles
- [ ] Implement `clears`: resolve to leaf marks, clear transitively through
      `over:` sets
- [ ] Confirm the traversal-plus-payload-predicate spelling against
      `query_model.md` §9.5; if it is not expressible, **that** is the one
      genuine grammar gap and it belongs to the query model, not here
- [ ] Acceptance: `draft → peer-reviewed → board-approved` with a rejection path
      back to `draft` parses, validates, and folds. Notify Issue 105

### 2. Schema registration (0.25 days)

- [ ] Register the step payload shape via `SCHEMAS`; validate combinator and
      outcome discriminants
- [ ] Unknown combinator or outcome: structurally valid, loud at fold time
- [ ] Document each combinator as a predicate over a **resolved node set**,
      which is the form Issue 105 consumes

### 2a. Derive the `redline` and `ask` field sets from data (1.25 days) — **may start before step 1**

> [!IMPORTANT]
> **`redline` and `ask` are general annotation kinds, not procedural subtypes,
> and this issue does not register them.** A redline is any proposed change to
> any content — `overlay_model.md` §2.5 defines it with no procedure involved,
> and a redline against content that does not exist yet anchors to a path like
> any other (`redline_model.md` §4). **Issue 104
> owns the registry entries** (`noet:redline:v1`, `noet:ask:v1`) alongside
> `receipt` and `gap`.
>
> What this step owns is the **evidence**: the field sets and observed
> lifecycles, mined from real records, which Issue 104 consumes. Plus one thing
> only this issue can do — **check that the lifecycle grammar can express those
> lifecycles**. If it cannot, the grammar is wrong, and that is a finding about
> steps 1–2 rather than about the vocabulary.
>
> The design homes are
> `docs/design/annotation/redline_model.md` (the general kind) and
> `docs/design/procedures/deviation_model.md` (the as-run comparison that may
> motivate one).

> **This step has real input data and must be done against it.** A corpus of ~25
> hand-written change-plan records with state-bearing frontmatter exists in a
> sibling repository (ask the human for the location). Derive the field sets
> **from** those files. Where a designed schema and the hand-written frontmatter
> disagree, the frontmatter wins: it has the evidence. **Do not cite that
> corpus, its documents, or its organization in noet-core** — describe it by
> structural properties only (`AGENTS.md` § Application-Neutral Content).

**What the corpus already establishes.** Its flat 7-value status enum conflates
three different determinants, and the factoring separates them:

| Authored value | Actually determined by | Share |
|---|---|---|
| "blocked" | state of a **cited run** | ~9/25 |
| "needs-placement" | **anchor does not resolve** — the target section is absent | ~4/25 |
| "ready" | absence of both of the above | ~6/25 |
| terminal values | a **record** asserting a discharge | ~6/25 |

Only the last row is a state. The other three are derived conditions, and the
flat enum conflated them *because it had no predicate language*. This maps onto
Issue 105's bracketed-operation versus derived-condition split, so the fold
needs no new machinery.

**The predicate vocabulary follows** — three subjects, one evaluation-time split:

| Predicate | Subject | Evaluated |
|---|---|---|
| combinator over `over:` | a resolved node set | at fold |
| payload predicate on a cited run | projected run state | at fold |
| anchor resolves | the anchor `QuerySpec`'s result | at look time |

- [ ] **Derive the field sets and hand them to Issue 104** — do not register
      them here:
      - **redline** — proposes a change to the content it anchors. Observed
        fields: target document and section, proposed text (before/after),
        rationale, driving external requirement, destination process, owner
        contact, blocked-on. These are **authoring-surface fields**, and the
        observed section granularity is correct at that level — storage is a
        file map (`generational_archive.md` §6.1), the same split git makes
        between hunks and blobs. Record the fields as observed; do not flatten
        them to the storage shape
      - **ask** — a question whose answer unblocks another record. Its
        `caused_by` cites what it unblocks; its target is an **actor or process,
        not a node**. Issue 104's primitive census has no row for that — hand it
        the finding
- [ ] `destination_process` is **derivable from the target document** in ~22 of
      25 observed records, because document control defines the route from the
      document class. Computed, not authored
- [ ] **Lifecycle authoring check, on paper** (0.5 days) — **the part only this
      issue can do**. Write the observed redline and ask lifecycles using only
      the grammar from steps 1–2, including the cross-run guard. Expected:
      expressible, with the guard as a payload predicate and no new syntax. A
      lifecycle that cannot be written is a **defect in the grammar**, not a
      special case for the vocabulary
- [ ] **Partial supersession is payload, not lifecycle.** One observed record was
      absorbed in half by another. Which claims moved is a redline-payload
      concern; do not build it into the predicate logic
- [ ] Test: an unknown `record_kind` fails loudly at fold time

**What this step does *not* do**: register a `record_kind` (Issue 104), define
the redline payload's storage shape (`redline_model.md` §3), or design the
promotion path (Issue 74 for the package, Issue 106/107 for enactment).

### 3. Documentation (0.25 days)

- [ ] Rustdoc for public APIs; module-level docs
- [ ] **Fix `myst_directive_architecture.md` §3.1** — the registered-verb table
      lists `draws_from`/`underlies` and omits the canonical
      `constrained_by`/`constrains` pair. `src/codec/myst.rs:307` and
      `src/query/parser.rs:1176` are correct; the table is stale
- [ ] Add `{exit}` / `{outcome}` to §3.1 and §8 (extension point)

### 4. Consolidate the procedural design-doc space (1.5 days) — **partially complete**

> **Done ahead of steps 1–2a, deliberately and in reduced scope.** The
> consolidation was to describe what was implemented; nothing here is
> implemented yet. What has landed is everything that does not require the
> implementation to exist: the target document set, the harvest, the removals,
> the corrections, and a named stub where the grammar will be specified. The
> remaining boxes need built code to describe.

**Landed document set** (3,459 lines → ~1,900, five documents → five):

| Document | State |
|---|---|
| `procedure_model.md` | **New.** Entry point: a procedure is a document, a potentialized annotation, a marking derived from records. Absorbs `procedure_schema.md` |
| `lifecycle_grammar.md` | **New — the named stub.** Settled decisions for steps 1–2a; no live specification |
| `observation_model.md` | Renamed from `action_observable_schema.md`; `inference_hint` schema preserved, integration half stubbed |
| `deviation_model.md` | **New.** The as-run-versus-template comparison and its vocabulary |
| `../annotation/redline_model.md` | **New, and outside this group.** A redline is a *general* annotation kind — any proposed change to any content — so it lives in `annotation/`. `redline_system.md`'s procedure-specific framing was an error inherited from the withdrawn model |
| `procedures_vs_alternatives.md` | Renamed from `noet_procedures_readme.md`; heavily cut, layering corrected |

`procedure_execution.md` was removed. Four passages were harvested first (§4.1's
append-only-log reasoning, §6.1 concurrency, §6.2 nesting, §11 principles); its
withdrawn component inventory is recorded in Issue 18.

> **A redline is not a procedural concept, and this issue's framing of it was
> wrong.** `redline_system.md` scoped redlines to procedure execution, and the
> consolidation initially inherited that. It does not hold: `overlay_model.md`
> §2.5 defines a redline as "an annotation proposing a change" with no procedure
> involved, `generational_archive.md` §6 fixes its payload as a file map, and
> a redline against absent content anchors to a path like any other
> (`redline_model.md` §4). Most redlines
> involve no run at all.
>
> The split: **`annotation/redline_model.md`** owns the general kind — payload
> shape, anchoring, the candidate-state read, and the two exits.
> **`procedures/deviation_model.md`** owns the as-run-versus-template
> comparison. A deviation is an *observation*; turning a pattern of them into a
> proposal is a human judgement that produces an ordinary redline. Procedures
> compose with redlines in both directions — a step can expect a redline as its
> discharge (a procedure for drafting a procedure), and a run's deviations can
> motivate one.

- [x] **Decide the target document set first** and record the shape before
      editing
- [x] **Remove the withdrawal banners by making them unnecessary** — no
      procedural design doc carries one
- [x] **Preserve what was assessed as sound**: the `inference_hint` schema is
      intact in `observation_model.md`; the append-only-log-plus-derived-state
      reasoning is `procedure_model.md` §4.1; run nesting is §5.2; the design
      principles are §10; the deviation taxonomy is `deviation_model.md` §3.1
- [x] **`CorrectionType` / `DeviationType` become payload vocabulary** —
      *resolved with a correction.* `DeviationType` is payload vocabulary
      (`deviation_model.md` §3.1). `CorrectionType` is **not**: its values are
      *concluded* about a run rather than described in a record, so they are
      **outcome discriminants**. This generalized into a claim the model now
      carries — **an outcome is a derived enumeration** with at least three
      determinant sources (an actor's assertion; a derived condition such as an
      anchor going stale under an in-progress run; a cited run's state), all of
      which are functions of (record set, corpus version) and so preserve the
      purity rule. See `procedure_model.md` §4.4 and `deviation_model.md` §3.2
- [x] **Fix the inverted layering** — corrected in
      `procedures_vs_alternatives.md` §5.1, which states that records project
      into a **held-out** BeliefBase via the fold, and that no channel is
      privileged with direct write access
- [x] **Resolve the bespoke query APIs** — *decided: `query_model.md` wins.* No
      bespoke Rust API. A run is a node whose payload carries derived state
      (Issue 105 §Project) and `resolve_property_path` reaches payload, so the
      analysis questions are traversals with payload predicates. Selection is
      the query model's; aggregation belongs to a `View` (§7).
      `deviation_model.md` §4 holds the question → answer table
- [x] **Do not delete history — relocate it.** *Archive convention decided: no
      `docs/design/superseded/`.* Git history is the archive and the owning
      issue is the design note, per `DOCUMENTATION_STRATEGY.md` Rule 4's routing
      table. A superseded design doc retained in the tree is exactly the
      on-topic-plausible-and-false noise Rule 4 exists to prevent; a directory
      name is weaker insulation than readers assume
- [x] Named stub locations for Issue 18's scope, cross-referenced from Issue 18,
      and Issue 18's task list updated to match
- [x] Repoint inbound references: `ISSUE_93` and `ISSUE_16` (filename only — it
      is completed, so its content stands)
- [ ] **Revisit once steps 1–2a land.** `lifecycle_grammar.md` is a stub by
      construction; it becomes the live specification when there is built code
      to describe. The `{exit}` / `{outcome}` option spellings and the marking
      semantics are its content — **not** the `redline` / `ask` payload schemas,
      which are Issue 104's
- [ ] **Re-verify the architecture map** in `procedure_model.md` §7 against the
      tree when steps 1–2a land

**Both open questions are settled.** `procedure_execution.md` was superseded
rather than salvageable — four passages survived and were harvested. The archive
convention is git history plus an issue note, with no superseded directory.

## Testing Requirements

**Directives**: registration and dispatch for both forms; `:key:` option parsing;
unknown combinator validates structurally and fails loudly at fold time;
round-trip through `generate_source` is lossless.

**Exit predicates**: each combinator returns the correct verdict over a synthetic
record set (`all` with one missing child fails; `any` with one present child
passes; `ordered` flags an out-of-order discharge as a diagnostic, not a
refusal). No execution engine — evaluate against a fixture.

**Queryset ranging**: default `composed_of(1)` matches the old nesting behaviour;
a non-default `over:` resolves and evaluates; an over-graph cycle is caught by
the visited set and reported as a template lint.

**Cycles**: the acceptance workflow folds correctly; a second and third rejection
produce a marking **identical** to the first.

**Purity**: the same record set folded twice at one corpus version yields
identical state. No test may depend on wall-clock time.

**Step identity**: covered by Issue 91B's shipped tests — a step's BID is stable
across an edit that does not touch it. Re-verify at the lifecycle level only.

**Negative**: no new record type. A grep for `AsRunRecord`, `ExecutorContext`,
`ProcedureRun`, `ExecutionRecord`, `CorrectionEvent`, `DeviationReport`,
`ObservationEvent`, or a resurrected `AsRun` finds nothing. **No `.procedure`
codec**: `CODECS` gains no entry.

## Success Criteria

- [ ] `{exit}` and `{outcome}` registered; a lifecycle is authorable in ordinary
      markdown with **no new codec and no new file extension**
- [ ] Exit predicates range over an arbitrary queryset, defaulting to containment
- [ ] **A cycle is expressible** as an outcome that clears marks; the acceptance
      workflow parses, validates, and folds, with termination by construction
- [ ] **A cross-run guard is expressible with no syntax added by this issue**
- [ ] `derive` is pure over (record set, corpus version); no wall-clock input
- [ ] Combinators and outcomes are open enums, loud on the unknown
- [ ] The **`redline` and `ask` field sets are derived from data and handed to
      Issue 104**, which registers them; their observed lifecycles are shown to
      be expressible in this issue's grammar
- [ ] **Zero new record types**; no addition to `src/properties.rs` or
      `src/event.rs`
- [x] **No procedural design doc carries a withdrawal banner**, and none
      describes a type or a codec that does not exist
- [x] **A reader new to the codebase can learn the procedural architecture from
      the design docs alone** — `procedure_model.md` is the entry point
- [x] **Issue 18's territory is stubbed, not designed** — three named stubs,
      listed in Issue 18
- [ ] Module layout is extraction-ready for Issue 95
- [ ] No execution engine, deviation analysis, or persistence layer

## Open Questions

1. **Is a traversal composed with a payload predicate expressible in
   `query_model.md` §9.5 today?** `payload.x > n` parses and resolves; what needs
   confirming is the composed form. If it is not expressible, the gap belongs to
   the query model and this issue files it there.
2. **Does the coverage reading hold?** Declared-set versus discharged-set per
   owner is what the traceability matrix computes. **Still open** — step 4
   recorded it as `procedure_model.md` §8 question 1 rather than settling it,
   because deciding it needs the predicate evaluator that step 1 builds.
3. **Can `Bid::codec_namespace` carry schema versions?** The open combinator enum
   makes the type set a versioning surface. Broader evolution is Issue 32's; this
   is only whether the namespace mechanism is the right carrier.

## Risks

**Risk 1 — resolved.** The step grammar needed a cycle construct. Resolved by
§The factored core: a cycle is an outcome that clears marks. No transition
construct, no back-edge, no second grammar. The five constraints that were
written against a transition construct are superseded; note in particular that
**constraint 4 conflated graph traversal with record folding** — the fold is a
linear pass over records and cannot diverge. A visited set is needed for
over-graph evaluation, which is a different thing.

**Risk 1a — resolved, and smaller than stated.** Cross-run guards need no syntax;
see §Cross-run guards. The genuinely harder case the original framing missed is
the **N-ary join over a dynamically discovered set** — observed as several
independent records that must ship as one package, grouped after authoring.
Dissolved by §Combinators range over a queryset, not by a new construct.

**Risk**: the `over:` option grows into a general condition language →
**Mitigation**: the predicate leaf has no boolean combinators; nesting is the
boolean algebra. Selection is `query_model.md`'s, combination is the tree's.

**Risk**: a wall-clock transition reappears as "just a convenience" →
**Mitigation**: the purity rule and the no-clock test. A timeout is
non-determinism, not a weak transition.

**Risk**: the withdrawn as-run model creeps back as "just a small struct" →
**Mitigation**: the zero-new-record-types criterion and the negative grep.

**Risk**: a `.procedure` codec creeps back for "schema validation TOML gives us
free" → **Mitigation**: the negative `CODECS` test. Validation is the `SCHEMAS`
registry's job and does not require a file format.

**Risk**: scope creeps toward an execution engine → **Mitigation**: any run loop
belongs to Issue 18.

## References

- `docs/design/core/beliefbase_architecture.md` §4.3 — `Envelope` and `Annotation`
- `docs/design/annotation/living_corpus.md` §2 — annotations as a privileged subset of `R`
- `docs/design/annotation/living_corpus.md` §5 — the conduit model; a step is $S_P$, a record is `R`
- `docs/design/annotation/living_corpus.md` §Movement — squash; the record-scale form of clearing marks
- `docs/design/codecs/myst_directive_architecture.md` §2, §3, §8 — directive registry and extension point
- `docs/design/core/query_model.md` §9.5 — the `over:` predicate language
- `docs/design/procedures/procedure_model.md` — the consolidated model (step 4)
- `docs/design/procedures/lifecycle_grammar.md` — the stub this issue fills
- `src/query/spec.rs:580` — `resolve_property_path`; why payload is query-reachable
- `src/codec/myst.rs:659` — `parse_directive_options`
- `ISSUE_91B_INLINE_ANCHOR_NODES.md` — **completed**; supplies step identity
- `ISSUE_104_ANNOTATION_VOCABULARY.md` — authoritative for the record field set
- `ISSUE_105_RECORD_STORE_AND_FOLD.md` — consumes the exit-predicate semantics
- `ISSUE_01_SCHEMA_REGISTRY.md`, `ISSUE_18_EXTENDED_PROCEDURE_SCHEMAS.md`,
  `ISSUE_95_WORKSPACE_DECOMPOSITION.md`

## Next Steps After Completion

1. **Issue 105** — folds a record set against a template using the exit
   predicates and step BIDs defined here; the immediate consumer
2. **Issue 18** — execution loop, participant-channel observations, deviation
   analysis
3. **Issue 95** — extracts `src/procedures/` into its own crate

Issues 104 and 105 are **not** downstream of this issue and need not wait for it.
