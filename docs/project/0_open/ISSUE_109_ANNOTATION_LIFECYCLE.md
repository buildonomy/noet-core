---
version = "0.1"
title = "Issue 109: Annotation Lifecycle — Runs Over an Append-Only Log"
---

# Issue 109: Annotation Lifecycle — Runs Over an Append-Only Log

**Priority**: MEDIUM
**Estimated Effort**: 3 days (RELATIVE COMPARISON ONLY)
**Dependencies**: Requires Issue 17 **narrowly** — only for the step-type
combinator semantics (`all_of` / `any_of` / `sequence` as combining predicates)
and stable step BIDs; not for the `.procedure` codec, and not for any record
type. Requires Issue 105 (the store the records live in, and the
version-anchored node reference a `RunStart` cites).
Blocks nothing directly, but Issue 104's fold assumes the mechanism this issue
defines, as would anything eventually built over an annotation queue (Issue 18).
**Design**: `docs/design/annotation/living_corpus.md` §4 (annotations are stateful),
`docs/design/annotation/attestation_fabric.md` §6 (protocol registry — the recorded gap)

## Summary

Annotation state is currently ownerless. `living_corpus.md` §4 says annotations
are stateful and that each kind has its own lifecycle; Issue 104 ships a
`protocol_id`-parameterized fold with three built-in tables and no way to define
a fourth.

**This issue owns the fold, not the lifecycle format.** A lifecycle *is* a
procedure — Issue 17 owns that definition, and a `.procedure` document's `steps`
field with its `sequence` / `any_of` / `all_of` types is the state machine. What
is missing here is the semantics for deriving state from an unordered,
append-only record log *against* such a template.

This issue defines **how a multi-step stateful operation is expressed in an
append-only, order-independent record log** — the framing mechanism, not any
particular lifecycle.

## The Problem

Derived state is a fold over records (`living_corpus.md` §3). That works for a
single-record claim: a `{todo}` opens, a later record closes it, the fold reports
`closed`.

It does not obviously work for an operation with *duration*. Consider executing a
review procedure: the reviewer starts it, works through steps over an hour,
records observations, and finishes. That is one logical operation producing
several records, and three properties of the store make it non-trivial:

1. **Records are immutable.** No record can be updated as the operation
   progresses.
2. **Merge is set union.** Records may arrive out of order, or interleaved with
   another actor's operation on the same node.
3. **Records are individually addressable.** Nothing currently groups them.

Without grouping, two concurrent executions of the same procedure on the same
node are indistinguishable in the log — their step records interleave and the
fold cannot tell which belongs to which run.

## Approach

**Bracket the operation and give it an identity.** The bracketed thing is a
**run** — the term the ontology already uses (`R` as-run records), and the term
Issue 17 used before its as-run model was withdrawn. It is deliberately not
`Session` (collides with `session_bb`, the compiler's per-compilation cache) and
not `State` (names the category, not the thing — a derived condition is also
state).

- A **`RunStart`** record opens a run. It carries a **version-anchored node
  reference** (Issue 105) to the procedure template being instantiated, and
  mints a **`run_id`**.
- Every subsequent record in that run carries the `run_id`.
- A **`RunEnd`** record closes it, referencing the same `run_id`.

The `run_id` is the grouping key the fold needs. It behaves like a transaction
token: the `RunStart` mints it, participants present it, and the fold partitions
the log by it before deriving state.

### `RunStart` is the run header, not a fourth object

A procedure execution splits into a *claim* (the run header — "this ran, by whom,
against which template") and *evidence* (step observations, which are `R` and
live outside the annotation store per `living_corpus.md` §2). The bracket is not
a third thing alongside those.

**The `RunStart` record *is* the run header claim**, and `run_id` is its
`EventId`. One object, two roles: it asserts that a run began, and its identity
groups everything that follows. This matters because the alternative — a header
record plus a separate bracket marker — would put two records in the log for one
event and leave open which of them a later record cites.

This composes with the existing model rather than extending it:

- `run_id` can be the `RunStart` record's own `EventId` — already
  globally unique, already `(actor, sequence)`, no new identity scheme.
- Bracketing is already the shape `BeliefEvent::BatchStart`/`BatchEnd` uses to
  frame a coherent epoch (`beliefbase_architecture.md` §4.3). Same idea at
  Layer 3.
- The template reference makes the state machine *data*: what states exist and
  which transitions are legal comes from the procedure the `RunStart` cites. A
  registry entry references a template by that version-anchored reference rather
  than embedding a state machine — one lifecycle grammar, owned by Issue 17
  (`attestation_fabric.md` §6).

### Runs nest

A `RunStart` may cite an existing `run_id` as its **parent**, designating a
sub-effort spawned from an in-progress run. This falls out of the design rather
than being bolted onto it: the `RunStart` already mints an identity and cites a
template, so a parent reference is one more optional field.

```
RunStart   run_id=A                       template: hazard-review
  RunStart   run_id=B  parent=A           template: fault-tree-check
    record     run_id=B
  RunEnd     run_id=B
  RunStart   run_id=C  parent=A           template: peer-review
  RunEnd     run_id=C
RunEnd     run_id=A
```

This is worth having for three reasons:

- **It matches how procedures actually decompose.** A step in a template can
  itself be a procedure. Nesting lets the log mirror that without inventing a
  separate sub-step mechanism.
- **The parent link is Epistemic, not structural.** A child run *draws from* its
  parent's context; it is not contained by it in the Section sense. The
  projection should emit the parent reference as an Epistemic edge, consistent
  with `caused_by` (`living_corpus.md` §5).
- **Partial completion becomes expressible.** A parent that ends with one child
  closed and another abandoned is a legitimate, readable outcome — not an error
  state needing repair.

The fold partitions on `run_id` as before; nesting is a tree over partitions, not
a change to partitioning.

#### Whether a child counts toward its parent is answered by the template slot

A child run does **not** need a policy flag saying whether its parent should fold
it in. The `RunStart` already carries the answer, provided it references *which
step of the parent's template it satisfies* — call it the task reference.

```
RunStart  run_id=B  parent=A  task=<step in A's template>
```

A child that fills a step of the parent's template is part of that procedure, so
the parent folds it in. A child spawned during the parent's execution but
mapped to no step is a related effort, not a constituent one — it hangs off the
parent for provenance and does not affect the parent's derived state.

The rule is: **a parent folds in a child iff the child's `task` maps to a step in
the parent's template.** No lifecycle-definition flag, no per-template policy
declaration — the mapping is the declaration.

This is better than a policy knob for three reasons:

- **The combining semantics come from the step type, not a new vocabulary.**
  Issue 17 already defines `sequence`, `parallel`, `any_of`, and `all_of` as step
  types. A parent whose children fill an `all_of` needs them all to succeed; an
  `any_of` needs one. The template already says how to combine, because that is
  what those step types mean.

  > **Unverified assumption.** See Issue 17 **Risk 1**: it is not yet established
  > that the step-type grammar can express a *cycle* (e.g. `draft → peer-reviewed
  > → draft`), because the operators form a nesting tree and a tree has no
  > back-edge. This issue assumes the grammar suffices for the lifecycles it must
  > fold; Issue 17 step 2 answers whether it does. If the answer is no, revisit
  > this section — do not introduce a second lifecycle grammar here.
- **It generalizes past nesting.** A task reference is what any
  template-anchored annotation needs in order to say *which part* of the template
  it discharges — nested run, single-record completion, or a step observation.
  §Open Questions carries whether this belongs on annotation records generally.
- **Unmapped children stay legible.** A sub-effort that is not part of the
  procedure is a real and useful thing to record. Making it structurally distinct
  from a constituent step — rather than distinguishing them by a flag — means the
  fold cannot conflate them.

The reference must be stable across template revision, which is why it is a step
identity rather than a positional index. **Resolved in Issue 17**: a step is a
node, so its **BID is the step reference** — derived from a stable string via
`Bid::codec_namespace` (`src/properties.rs:336`), never from a positional index,
so it survives template revision. No new reference type is needed here.

Nesting adds failure modes, listed in §Failure Modes: cycles, orphaned parents,
and the ordering hazard that a child's `RunStart` may merge before its parent's.

## Is State Procedure-Specific?

**Probably not, and the design should not assume it is.**

The argument that it is: an operation with duration and ordered steps *is* a
procedure, and the annotation model already describes one — an annotation record
*is* the as-run record, its `Envelope` supplies who and when
(`beliefbase_architecture.md` §4.3), and a `.procedure` document supplies the
template. Reusing that avoids a second mechanism.

The argument that it is not: `{todo}` has a lifecycle (`open → closed`,
`open → abandoned`) and is a degenerate one-step procedure, so the two are
already unified. A sign-off going `stale` is a lifecycle transition that is
**not** actor-driven at all — it is induced by the anchor hash changing
(`living_corpus.md` §4). That case has no procedure, no steps, and no
`run_id`, yet it is unambiguously state.

So there appear to be **two distinct state mechanisms**, and conflating them
would be an error:

| | Bracketed operation | Derived condition |
|---|---|---|
| Example | executing a review procedure | a sign-off going `stale` |
| Driven by | actor records | the world changing underneath |
| Needs `run_id` | yes | no |
| Needs a template | yes | no |
| Transition trigger | a record arrives | a hash comparison at fold time |

The fold must handle both. Recommend: `run_id` and the bracket markers are
for the first; the second stays a function of `(record chain, current node
hash)`, as Issue 104 already specifies. Do not force staleness into the bracket
model.

## Goals

1. Define `RunStart` / `RunEnd` records and the `run_id` they carry
2. Specify how the fold partitions a log by `run_id` before deriving state
3. Define where a lifecycle definition lives and how the fold finds it
4. Handle the failure modes an append-only unordered log makes possible
5. Keep derived-condition transitions (staleness) out of the bracket mechanism

## Failure Modes to Specify

These are the reason this needs an issue rather than a paragraph. Each must have
a defined behaviour, and none may be a refused write — the store is append-only
with set-union merge, so rejection is not available (`living_corpus.md` §4).

- **Unterminated operation.** A `RunStart` with no `RunEnd`. Is the operation
  in-progress indefinitely, or does it expire? An abandoned review and a review
  still underway look identical in the log.
- **Orphaned member.** A record citing a `run_id` whose `RunStart` has
  not arrived (or never will). Must remain readable; must not corrupt the fold.
- **Duplicate `RunEnd`.** Two closings of one operation, possibly from
  different actors.
- **Interleaved operations.** Two `RunStart`s on the same node by the same
  actor. Legal, and the partition must keep them separate.
- **Illegal transition.** A record whose transition the template forbids. A
  diagnostic via `check_consistency`, never a rejected write — and note it can
  only be *judged* illegal after a merge the writer could not have observed.
- **Unknown template.** The cited procedure is unavailable. Per
  `attestation_fabric.md` §6.2, the records stay readable and mergeable with no
  derived state; they are never dropped.

Nesting adds one more, and it is narrower than it first appears:

- **Unresolvable parent reference.** `parent=X` where no `RunStart` with
  `run_id=X` is in the reader's record set. The child must fold as a valid run in
  its own right and attach to its parent if X later becomes available — never be
  held in a pending state, and never dropped. The dangling reference is a
  `check_consistency` diagnostic.

  **This is not a merge race.** Citing `parent=A` requires the writer to have had
  A in hand, so the child's `lamport` strictly exceeds A's and the causal edge is
  genuine — within a single writer's log a child cannot precede its parent. What
  makes the case real is that *the writer had it* and *this reader has it* are
  different claims:

  1. **Partial scope.** Issue 105 resolves three scopes (repo / user / shared)
     and treats a missing scope as empty rather than an error. A parent written
     to `shared` and a child written to a colleague's `user` scope means a reader
     with only one mounted sees the child alone.
  2. **Incomplete sync.** A sidecar git repo fetched shallowly, or an Issue 65
     peer mid-transfer, yields a valid prefix of someone's log — not necessarily
     a causally closed one.
  3. **Retention.** Issue 105 defers garbage collection but names it as real. A
     compacted parent with a surviving child dangles permanently.

  So this is the **orphaned member** case above with a different field name, and
  should share its implementation: a citation into a record set that does not
  contain the target is normal under union semantics, not exceptional.

**Cyclic parentage cannot occur** and needs no runtime guard. `run_id` is the
`RunStart`'s own `EventId`, and citing a parent requires having it, so any cycle
would require a record to have existed before itself. Assert it in a debug build
if cheap; do not write recovery logic for it.

## Implementation Steps

1. **Record shape** (0.5 days)
   - [ ] `RunStart { template: <version-anchored node ref, Issue 105>,
         parent: Option<EventId>, task: Option<Bid>, .. }`; `run_id` = the
         record's own `EventId`. The `RunStart` **is** the run header claim — not
         a marker alongside one.
   - [ ] `task` names which step of the **parent's** template this run
         discharges. It is the **step's BID** (Issue 17: a step is a node, its BID
         derived stably via `Bid::codec_namespace`), so it is stable across
         template revision and is not a positional index.
   - [ ] `RunEnd { run_id, result, .. }`
   - [ ] `run_id: Option<EventId>` on annotation records generally —
         `None` for single-record claims, which stay the common case

2. **Partitioned fold** (1 day)
   - [ ] Group records by `run_id` before deriving state
   - [ ] Records with `run_id: None` fold as they do today — this must not
         regress Issue 104's three built-in kinds
   - [ ] Ordering within a partition by `(lamport, observed_at, id)`, consistent
         with Issue 105

3. **Lifecycle definition lookup** (1 day)
   - [ ] The `RunStart`'s template reference resolves to the states and
         transitions
   - [ ] Wire to the `protocol_id` transition table Issue 104 already
         parameterizes over — one lookup path, not two
   - [ ] Unknown template degrades to no-derived-state
   - [ ] A parent folds in a child **iff** the child's `task` maps to a step in
         the parent's template; combining semantics come from that step's type
         (`all_of` / `any_of` / `sequence`), not from a policy field
   - [ ] A child with no `task`, or one mapping to no step, attaches for
         provenance and does not affect the parent's derived state

4. **Failure modes + tests** (0.5 days)
   - [ ] One test per case in §Failure Modes
   - [ ] Two interleaved runs on one node fold to two correct states
   - [ ] A child run whose parent is **absent from the record set** — the partial
         scope case, not a merge race — folds correctly on its own, and attaches
         when the parent scope is mounted
   - [ ] A `{todo}` (no `run_id`) is unaffected by any of this

## Success Criteria

- [ ] A multi-record run folds to one coherent state
- [ ] Two concurrent runs on the same node do not interfere
- [ ] A nested run folds independently when its parent is not in the record set,
      and attaches when it is
- [ ] A child mapped to a parent template step counts toward the parent's state
      per that step's type; an unmapped child attaches without affecting it
- [ ] Every failure mode has a defined, tested behaviour; none is a refused write
- [ ] Staleness still works and does **not** route through the bracket mechanism
- [ ] Single-record annotation kinds are unchanged — no `run_id` required
- [ ] A lifecycle is definable as data, not code

## Risks

- **Two state mechanisms confuse implementers** → **Mitigation**: the table in
  §Is State Procedure-Specific is the contract; bracketed operations and derived
  conditions are named differently throughout.
- **`run_id` becomes mandatory** and every `{note}` carries transaction
  machinery → **Mitigation**: `Option<EventId>`, defaulting `None`; the
  single-record path must stay the simple one.
- **Unterminated operations accumulate** and the fold degrades → **Mitigation**:
  decide the expiry question in step 1; an in-progress operation older than some
  bound is reportable via `check_consistency`.
- **This is really transactions**, and transactional semantics over a CRDT is a
  known-hard problem → **Mitigation**: scope is *grouping and lifecycle*, not
  atomicity or isolation. A partially-recorded operation is a legitimate state
  (someone stopped halfway), not a rollback case. Do not let ACID vocabulary in.

## Open Questions

- **Does an unterminated operation expire?** A time bound is arbitrary but the
  alternative is unbounded in-progress state. Recommend: no automatic expiry,
  surface age via `check_consistency`, let a policy decide.
- **Can an operation span nodes?** A review covering five sections is plausibly
  one operation. If so, `run_id` groups across anchors, and the fold
  partitions before grouping by `(bid, version)` rather than after. Recommend
  yes, but confirm against a real review workflow.
- **Where do lifecycle definitions live?** Same unresolved question as Issue 105
  §Open Questions — source (normative, version-controlled) or sidecar (travels
  with the records). Note definitions are *override* data, not G-Set data; the
  scope precedence differs from records.
- **Is `RunStart` an annotation kind or a distinct payload?** As a
  `protocol_id` it costs nothing and inherits the registry. But it asserts
  nothing about a node — it opens a bracket — so it may not be an annotation at
  all in the §4 sense.

- **Does `task` belong on annotation records generally, not just `RunStart`?**
  Any record anchored to a template plausibly needs to say *which part* of it the
  record discharges — a nested run filling a step, a single-record completion of
  a one-step procedure, a step observation. If so, `task: Option<Bid>` (a step
  BID) is a peer of `run_id` on the record rather than a `RunStart` field, and the
  annotation/procedure equivalence gets sharper. Note the direction: the
  **annotation record is the primitive** — an annotation *is* an as-run record —
  and "a degenerate procedure" is the gloss on it, not the other way round. Under
  that reading a `{todo}` is a record whose `task` is the single step of its
  template.
  Recommend hoisting it, but confirm against Issue 104's three kinds first — if
  none of them needs it, leave it on `RunStart` until one does.

## References

- `docs/design/annotation/living_corpus.md` §4 — annotations are stateful; the per-kind
  state machine and the claim/assertion boundary this must not violate
- `docs/design/annotation/attestation_fabric.md` §6 — the protocol registry, and the
  recorded gap that entries declare no lifecycle
- `docs/design/core/beliefbase_architecture.md` §4.3 — `BatchStart`/`BatchEnd` as the
  Layer 2 precedent for bracketing; `Envelope` and `EventId`
- Issue 17 — the `.procedure` codec and `steps` schema; the step-type combining
  predicates this fold consumes, and the stable step BIDs `task` names
- Issue 104 — the `protocol_id`-parameterized fold this extends
- Issue 105 — the store; ordering by `(lamport, observed_at, id)`; the
  definition-location question
- Issue 18 — an aspirational stub; whatever is eventually built over an
  annotation queue (see its candidate directions) is a consumer of this
  mechanism, not a source of it
