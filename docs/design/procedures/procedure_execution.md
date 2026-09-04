---
title: "Procedure Execution: Runtime Tracking and As-Run Recording"
authors: "Andrew Lyjak, Gemini 2.5 Pro, Claude Code"
last_updated: "2025-01-XX"
status: "Withdrawn — retained for requirements only"
version: "0.1"
dependencies: ["procedure_schema.md (v0.1)"]
---

# Procedure Execution Architecture

> [!WARNING]
> **This document describes a withdrawn model.** Its execution mechanics are
> built on the three-piece "as-run" model — template / executor context / as-run
> record — as three bespoke types. **That model is withdrawn.** The types named
> below (`ProcedureRun`, `ExecutionRecord`, `TemplateRef`, `ExecutorContext`,
> `AsRunRecord`) do not exist and will not be built under those names.
>
> **The current model** is `docs/design/annotation/living_corpus.md` §2 (the three-layer
> Source / Belief Graph / Annotation model) and
> `docs/project/0_open/ISSUE_17_NOET_PROCEDURES_EXTRACTION.md` → "What Was
> Removed and Why". In short: **an annotation *is* an as-run record**, and a
> procedure instance is the set of annotations sharing a common `RunStart`
> ancestor — a *query* over the annotation store, not a type.
>
> **No replacement execution design exists yet.** Issue 18, which owned the
> execution loop and deviation analysis, has been reduced to an aspirational
> stub; the engine components, deviation handling, and query API below await a
> design it has not produced. This document is retained as a record of the
> requirements that design must satisfy — **not** as a specification to
> implement against.
>
> **§2 has been rewritten** to the current model. The remaining sections are
> otherwise unrevised — read them as requirements-gathering — but each passage
> that depends structurally on a withdrawn type now carries an inline marker
> naming what replaced it and who owns it.

## 1. Purpose

This document was written to specify the runtime architecture for executing and tracking procedures defined by the procedure schema. Under the withdrawn model it defined:

- Procedure lifecycle state machine (Inactive → Triggered → Active → Completed/Aborted)
- Event log architecture (unified, append-only audit trail)
- Run index (queryable as-run history)
- Concurrency and nesting handling
- Integration points for downstream extensions

Read these as **requirements a replacement design must address**, not as an
owned specification. Ownership has moved: the annotation record's field set is
Issue 104's, the store is Issue 105's, run bracketing and folding are Issue
109's, the procedure codec and the procedural annotation subtypes are Issue
17's, and the execution loop itself is undesigned (Issue 18).

**Key Design Principle**: This architecture is **general-purpose**. It provides execution tracking for any procedural domain: manufacturing SOPs, lab protocols, deployment runbooks, cooking recipes, project workflows, etc.

**Out of Scope**: Behavior prediction, learning algorithms, and sensor integration are product-specific extensions. This document covers only the core execution infrastructure.

## 2. The Execution Record Model

> This section has been rewritten. It previously described a "three-piece as-run
> model" — template, executor context, and as-run record as three bespoke types.
> That model is withdrawn; what follows is the current one.

The durable observation still holds: **practitioners mark up procedures as they
execute them**, and a procedural system in aviation, manufacturing, or a lab
must record the delta between what was written and what was done. What changed
is that recording this needs **no procedure-specific record types at all**.

### 2.1 An annotation is an as-run record

An annotation is an observation pinned to a moment, attributable to an actor,
and immutable once made (`living_corpus.md` §2). That is exactly what an as-run
entry is. There is no second category, so the three pieces collapse:

| Former piece | Where it now lives |
|---|---|
| **Template** (as-written) | A compiled `.procedure` document, plus a `NodeVersionRef` — a `(bid, content_version)` pair — naming the node and version that was run against. The reference is general, not procedure-specific, and is owned by **Issue 105**. |
| **Executor Context** (who/when/where) | The annotation `Envelope` (`beliefbase_architecture.md` §4.3): `executor_id` → `actor`, `timestamp` → `observed_at`, and `caused_by` for what the record reasons from. `credential_type` is a payload field of the sign-off protocol (Issue 104). |
| **As-Run Record** (reality) | **Not a type — a query.** A run is the set of annotations sharing a common `RunStart` ancestor. `status` is the fold's output, the step entries are member records carrying `run_id` and a step reference, and provenance is `caused_by`. |

### 2.2 A run is a query, not an object

An operation with duration produces *several* records, and in an unordered
append-only store nothing groups them. The grouping mechanism is a
`RunStart`/`RunEnd` bracket carrying a `run_id` that the fold partitions on; a
`RunStart` may cite a parent `run_id`, so runs nest. **Issue 109 owns run
bracketing, `run_id`, nesting, and folding** — this document does not define
them, and §4.2's "run index" below is not a substitute for that definition.

The consequence for everything downstream: "the run record" is not a row to
fetch and mutate. It is the result of partitioning the annotation store by
`run_id` and folding the partition. Sections 5–8 were written against the
fetch-and-mutate reading and have not been reworked.

### 2.3 An event is an annotation subtype

Run lifecycle events — step discharged, deviation noted, correction made — are
**not** a new enum and do not expand `src/event.rs`. Each is an annotation
carrying a registered `protocol_id` and a payload schema, the same mechanism
`{todo}` and `{reviewed}` already use. **Issue 17 step 2a** registers the
procedural subtypes, redline being the worked example.

This is why the withdrawn types had no home to return to: `ProcedureRun`,
`ExecutionRecord`, `CorrectionEvent`, `DeviationReport`, and `ObservationEvent`
were all procedure-specific spellings of things the annotation model already
expresses. The governing constraint is that **new primitives in core types must
be few and general** — a record type is general, an `AsRunRecord` is not.

### 2.4 What is not yet designed

The collapse above says where the *data* lives. It does not say how a procedure
is advanced, checked, or organized while it runs. **That is undesigned.** Issue
18 is an aspirational stub whose candidate directions (a semantic organizer over
an annotation queue, a fold executor, an annotation-queue linter) have not been
chosen between. Sections 3 and 5–10 of this document describe one such design;
it is the withdrawn one.

## 3. Procedure Lifecycle

> [!CAUTION]
> **A hardcoded five-state machine contradicts the current lifecycle model.**
> Under Issue 17, **the procedure schema *is* the lifecycle definition**: the
> `steps` field declares the states and the step types (`sequence`, `any_of`,
> `all_of`, `parallel`) declare the transition semantics, read as *combining
> predicates over a set of records*. A custom lifecycle is therefore an authored
> `.procedure` document, not a code change — which a fixed five-state enum
> forecloses. `attestation_fabric.md` §6.1 records the same decision from the
> registry side, and an embedded `[protocol.states]` block was withdrawn for the
> same reason.
>
> **Unresolved, and material.** Issue 17 Risk 1 records a verified gap: the step
> types are a *nesting tree* grammar and a tree has no back-edge, so a cycle such
> as `draft → reviewed → draft` is **not expressible as written**. Issue 17 step
> 2 owns designing a cycle construct to close it, under the constraint that the
> construct stays a combining predicate over a record set rather than something
> meaningful only to a running engine. **Do not pre-empt that design by
> implementing §3** — a fixed enum of engine-observed states is the shape it is
> explicitly required not to take.
>
> §3.2's triggering mechanisms are a separate concern and are not affected by the
> withdrawal.

Every procedure exists in one of five states:

### 3.1 State Machine

```
┌──────────┐
│ Inactive │ ◄─────────────────────┐
└────┬─────┘                       │
     │ context matches             │
     ▼                             │
┌───────────┐                      │
│ Triggered │                      │
└─────┬─────┘                      │
      │ first step detected        │
      ▼                            │
┌────────┐                         │
│ Active │                         │
└───┬────┘                         │
    │                              │
    ├─► Completed ─────────────────┤
    │                              │
    └─► Aborted ──────────────────┘
```

**States:**

1. **Inactive**: Default state. Trigger conditions not met. Not monitored.
2. **Triggered**: Context conditions met. Watching for first step.
3. **Active**: First step detected. Tracking progress through steps.
4. **Completed**: All steps matched. Generates confirmation prompt.
5. **Aborted**: Deviated from pattern (missed step, timeout, violation).

### 3.2 Triggering Mechanisms

Procedures activate based on **context** conditions defined in the schema:

**Time-Based Triggers:**
```toml
[procedure.context]
time_of_day = ["08:00", "09:00"]  # Activate at 8 AM
day_of_week = ["monday"]          # Only on Mondays
```

**Event-Based Triggers:**
```toml
[procedure.context]
during_action = "act_equipment_startup"  # When equipment starts
```

**Use Cases:**
- Scheduled maintenance (monthly procedures)
- Context-dependent protocols (emergency procedures)
- Sequential workflows (deploy after build)

## 4. Data Architecture

The execution engine maintains two durable data stores:

### 4.1 Unified Event Log

> [!NOTE]
> **Mechanics survive; the schema does not.** "Immutable, append-only, single
> source of truth, rebuild any derived state from it" is exactly the annotation
> store (`living_corpus.md` §3, Issue 105) — this section anticipated it
> correctly. But the log is **not procedure-specific**: there is one annotation
> store, not an execution database beside it. The bespoke event schema below
> maps onto the general record: `event_id` → `Envelope.id`, `timestamp` →
> `observed_at`, `source` → `actor`, `event_type` → `protocol_id`, and `payload`
> → the protocol's payload schema. The event *types* named in the examples
> (`proc_triggered`, `step_matched`, `prompt_response`, `proc_completed`) are
> candidate `protocol_id` values, not enum variants — **Issue 17 step 2a**
> registers procedural subtypes; run bracketing (`RunStart`/`RunEnd`) is Issue
> 109's.

**Design**: Immutable, append-only log of every event.

**Purpose**: Single source of truth for audit trail, debugging, and analytics.

**Event Schema:**
```json
{
  "event_id": "uuid",
  "timestamp": "ISO 8601 datetime",
  "source": "inference | scheduler | engine | executor",
  "event_type": "proc_triggered | step_matched | proc_completed | etc",
  "payload": { ... }
}
```

**Example Events:**

```json
// Context-based trigger
{
  "event_id": "evt_001",
  "timestamp": "2025-06-01T08:00:00Z",
  "source": "scheduler",
  "event_type": "proc_triggered",
  "payload": {
    "procedure_id": "sop_daily_startup",
    "trigger_reason": "time_of_day"
  }
}

// Step execution
{
  "event_id": "evt_002",
  "timestamp": "2025-06-01T08:05:00Z",
  "source": "engine",
  "event_type": "step_matched",
  "payload": {
    "procedure_id": "sop_daily_startup",
    "run_id": "run_123",
    "step_index": 0,
    "step_id": "power_on"
  }
}

// Executor response to prompt
{
  "event_id": "evt_003",
  "timestamp": "2025-06-01T08:10:00Z",
  "source": "executor",
  "event_type": "prompt_response",
  "payload": {
    "run_id": "run_123",
    "variable_name": "temperature_check",
    "value": true
  }
}

// Completion
{
  "event_id": "evt_004",
  "timestamp": "2025-06-01T08:20:00Z",
  "source": "engine",
  "event_type": "proc_completed",
  "payload": {
    "run_id": "run_123",
    "procedure_id": "sop_daily_startup",
    "duration_minutes": 20
  }
}
```

**Key Properties:**
- Immutable (never edited, only appended)
- Timestamp-ordered
- Includes all sources (scheduler, engine, executor)
- Enables rebuild of any derived state

### 4.2 Procedure Run Index

> [!CAUTION]
> **Withdrawn.** The run header is not this document's to define. `RunStart` is
> the header and `run_id` is its `EventId`; **Issue 109** owns both, along with
> nesting and the fold that derives run state. The schema below would fork that
> definition. Its `context` block is superseded by the `Envelope` (§2.1), and
> `event_ids` is superseded by partitioning the annotation store on `run_id`.
>
> **What survives** is the *shape of the requirement*: run state must be derived
> from an append-only log rather than stored, and it must be indexable for the
> queryable dimensions listed below. That is §3 of `living_corpus.md` — durable
> store plus live projection — and it is satisfied by the fold, not by a second
> derived table.

**Design**: Derived, indexed representation built from event log.

**Purpose**: Fast, queryable access to as-run history.

**Run Schema:**
```json
{
  "run_id": "uuid",
  "procedure_id": "uuid",
  "start_time": "ISO 8601 datetime",
  "end_time": "ISO 8601 datetime",
  "status": "completed | aborted",
  "event_ids": ["evt_001", "evt_002", ...],
  "executor_confirmation": "yes | no | partial | null",
  "context": {
    "executor_id": "user_alice",
    "location": "lab_3",
    "equipment_id": "reactor_02"
  }
}
```

**Queryable Dimensions:**
- All runs for a procedure: "Show me every time we ran 'Daily Startup'"
- Duration trends: "How long does this usually take?"
- Failure analysis: "Which steps are most often skipped?"
- Executor patterns: "Who performs this most consistently?"

**Update Responsibility**: Active Monitor creates run record on first step, updates as procedure progresses. *(Withdrawn: records are immutable and are never updated in place. A transition is a new record citing the prior one.)*

### 4.3 Executor Context

> [!CAUTION]
> **Withdrawn as a type.** `ExecutorContext` collapses into the annotation
> `Envelope` (`beliefbase_architecture.md` §4.3): `executor_id` → `actor`,
> `timestamp` → `observed_at`. The remaining keys below are **payload fields of
> a protocol**, not envelope fields — `location`, `equipment_used`, and
> `environmental_conditions` had no named reader and are dropped until something
> asks for them. The use cases underneath are the surviving part: they are what
> a payload schema would have to serve.

Metadata about who is executing and under what conditions:

```json
{
  "executor_id": "user_alice",
  "executor_role": "lab_technician",
  "location": "lab_3",
  "equipment_used": ["reactor_02", "thermometer_07"],
  "environmental_conditions": {
    "temperature_c": 22,
    "humidity_percent": 45
  },
  "notes": "First run after maintenance"
}
```

**Use Cases:**
- Compliance tracking (who performed this procedure)
- Pattern analysis (does location affect duration?)
- Training assessment (new executors vs. experienced)
- Environmental correlation (temperature affects outcomes?)

## 5. Engine Components

> [!CAUTION]
> **Undesigned, not merely stale.** The three components below describe the
> execution engine of the withdrawn model. No replacement has been designed:
> Issue 18 lists a semantic organizer over an annotation queue, a fold executor,
> and an annotation-queue linter as *candidate directions* and has chosen none.
> Read §5 as a statement of what such a component would have to do — the
> responsibilities are plausible requirements; the decomposition is not a
> commitment. Note in particular that "create/update the run record" is not an
> operation the annotation model has (§4.2).

### 5.1 Trigger Watcher

**Responsibility**: Monitor for procedures entering Triggered state.

**Inputs:**
- Scheduler events (time-based triggers)
- External events (context changes)
- Procedure context definitions

**Operation:**
1. Evaluate context conditions for all Inactive procedures
2. When conditions match, transition to Triggered
3. Pass to Active Monitor for step tracking

**Efficiency**: Only evaluates simple context matching (not complex step logic). Scales to hundreds of procedures.

### 5.2 Active Monitor

**Responsibility**: Track progress of Active procedures.

**Inputs:**
- Triggered procedures from Trigger Watcher
- Step execution events
- Executor responses

**Operation:**
1. Create run record when first step detected (Triggered → Active)
2. Match incoming events against expected steps
3. Update run record with event_ids
4. Detect completion or abortion
5. Generate confirmation prompt
6. Transition back to Inactive

**Key Functions:**
- Step matching (did this event match expected step?)
- Timeout detection (procedure stalled?)
- Deviation detection (unexpected step order?)
- Completion recognition (all steps matched?)

### 5.3 Hypothesis Generator

**Responsibility**: Format completed procedures for executor confirmation.

**Input**: Completed run record

**Output**: Confirmation prompt

**Example Prompt:**
```
Procedure: Daily Equipment Startup (20 minutes)

Detected steps:
✓ Power on reactor
✓ Verify temperature < 200°F
✓ Start circulation pump
✓ Record baseline readings

Does this match what you did?
[ Yes ] [ No ] [ Partially ]
```

**Purpose**: Executor feedback validates or corrects engine's interpretation.

## 6. Concurrency and Nesting

### 6.1 Concurrent Procedures

Multiple procedures can be Active simultaneously:

**Scenario**: Executor action "power_on_reactor" is first step in both:
- "Daily Startup Procedure"
- "Equipment Commissioning Procedure"

**Behavior**: Both procedures transition to Active. Engine tracks both independently.

**Resolution**: Executor confirmation disambiguates ("I was doing startup, not commissioning").

### 6.2 Nested Procedures

Steps can reference sub-procedures:

```toml
[[procedure.steps]]
type = "action"
reference = "sop_calibrate_sensor"  # Complete sub-procedure
```

**Behavior**:
1. Parent procedure pauses at reference step
2. Sub-procedure triggers and becomes Active
3. Engine tracks sub-procedure to completion
4. Parent procedure resumes

**Run Record**: Sub-procedure gets own run_id, linked to parent via event log.

> [!NOTE]
> **Nesting survives, with a different owner.** "Sub-procedure gets its own
> `run_id`, linked to the parent" is the current model too — a `RunStart` may
> cite a parent `run_id`, and the parent link projects as an Epistemic edge.
> **Issue 109 owns run nesting**; this section anticipates it rather than
> defining it. §6.1 (concurrent procedures resolved by executor confirmation) is
> independent of the withdrawn types and stands as written.

## 7. Deviation Handling

> [!CAUTION]
> **Depends on withdrawn types.** `DeviationReport` and `CorrectionEvent` were
> withdrawn with Issue 18's draft design. A deviation or a correction is an
> **annotation subtype** — a registered `protocol_id` with a payload schema,
> anchored to the step node it concerns and carrying the `run_id` of the run it
> belongs to — not a bespoke record type and not an addition to `src/event.rs`.
> Which subtypes exist, and their payload shapes, is **Issue 17 step 2a**;
> redline is the only one currently committed to. The deviation *taxonomy* below
> (reordering, omission, addition, timeout) is vocabulary a payload schema would
> need, and is preserved on that basis.
>
> Note also that "the engine detects a deviation" presumes a running engine that
> does not exist and has not been designed (§5).

Procedures rarely execute exactly as written. The engine must handle reality:

### 7.1 Deviation Types

**Reordering:**
- Expected: Step A → Step B → Step C
- Observed: Step A → Step C → Step B
- Behavior: Mark as "completed with deviations"

**Omission:**
- Expected: Step A → Step B → Step C
- Observed: Step A → Step C (Step B skipped)
- Behavior: Mark as "completed, Step B skipped"

**Addition:**
- Expected: Step A → Step B
- Observed: Step A → Step X → Step B
- Behavior: Mark as "completed with extra steps"

**Timeout:**
- Expected: Complete within 30 minutes
- Observed: Step A, then 45 minutes, then Step B
- Behavior: Mark as "aborted, timeout exceeded"

### 7.2 Deviation Recording

Deviations are logged as events:

```json
{
  "event_id": "evt_dev_001",
  "timestamp": "2025-06-01T08:15:00Z",
  "source": "engine",
  "event_type": "deviation_detected",
  "payload": {
    "run_id": "run_123",
    "deviation_type": "step_skipped",
    "expected_step": "verify_temperature",
    "actual_step": "start_pump"
  }
}
```

**Executor Correction:**

Executors can correct engine's interpretation:

```json
{
  "event_id": "evt_corr_001",
  "timestamp": "2025-06-01T08:25:00Z",
  "source": "executor",
  "event_type": "procedure_correction",
  "payload": {
    "run_id": "run_123",
    "correction_type": "wrong_procedure",
    "actual_procedure_id": "sop_emergency_shutdown",
    "note": "Alarm triggered, switched to emergency protocol"
  }
}
```

**Integration Point**: Corrections feed into redline system for learning (see `redline_system.md`). *(Note: redline as an annotation subtype is **Issue 17 step 2a**; promotion of a redline into a source edit is **Issue 106** over the write-back path in **Issue 107**. `redline_system.md` carries the same withdrawn framing as this document.)*

## 8. Query API

> [!CAUTION]
> **Every signature below returning `Vec<ProcedureRun>` depends on a withdrawn
> type.** `ProcedureRun` does not exist. Under the current model a run is not a
> value to return — it is a partition of the annotation store keyed on `run_id`,
> folded to derive state (§2.2, Issue 109). These signatures are retained
> because the *questions* they ask are the requirements a real query surface
> must answer; the return types are not.
>
> Note also that noet already has a query language (`query_model.md`). A
> replacement design should establish whether these questions need a bespoke
> Rust API at all, or whether they are queries over the annotation store
> expressed in the existing grammar. That is an open question, not a decision
> made here.

Execution history enables powerful analysis:

### 8.1 Basic Queries

```rust
// WITHDRAWN TYPE: `ProcedureRun` does not exist. See the section note.

// All runs for a procedure
fn get_runs(procedure_id: &str) -> Vec<ProcedureRun>;

// Runs in time range
fn get_runs_in_range(
    procedure_id: &str, 
    start: DateTime, 
    end: DateTime
) -> Vec<ProcedureRun>;

// Failed runs only
fn get_failed_runs(procedure_id: &str) -> Vec<ProcedureRun>;
```

### 8.2 Analytics Queries

```rust
// Average duration
fn average_duration(procedure_id: &str) -> Duration;

// Most skipped steps
fn skipped_steps_frequency(procedure_id: &str) -> HashMap<StepId, u32>;

// Executor comparison
fn executor_success_rate(procedure_id: &str) -> HashMap<ExecutorId, f64>;
```

### 8.3 Compliance Queries

```rust
// WITHDRAWN TYPE: `ProcedureRun` does not exist. See the section note.
// `executor_id` is `Envelope.actor` under the current model.

// All procedures executed by executor
fn procedures_by_executor(executor_id: &str) -> Vec<ProcedureRun>;

// Procedures on equipment
fn procedures_on_equipment(equipment_id: &str) -> Vec<ProcedureRun>;

// Audit trail for specific run
fn full_event_log(run_id: &str) -> Vec<Event>;
```

## 9. Extension Points

This architecture is designed for downstream extensions:

### 9.1 Learning Algorithms (Product-Specific)

Products can add:
- Probabilistic step matching (HMM, edit distance)
- Duration prediction models
- Failure prediction
- Personalized adaptations

**Integration**: Read from event log, write learned parameters to separate tables.

### 9.2 Sensor Integration (Product-Specific)

Products can add:
- Observation producers (location, activity, biometrics)
- Automatic step detection
- Context inference

**Integration**: Sensors emit events to unified log, engine matches to procedures.

### 9.3 Visualization (Implementation-Specific)

Applications can add:
- Live procedure dashboard
- Historical trend charts
- Executor leaderboards
- Compliance reports

**Integration**: Query run index and event log, render as needed.

## 10. Implementation Checklist

> [!CAUTION]
> **Do not implement this checklist.** It enumerates the withdrawn model's
> components, including the run index (§4.2) and the correction/deviation record
> types (§7), neither of which this document owns. It is retained as an
> inventory of concerns a replacement design must account for or explicitly
> decline.

For libraries implementing this architecture:

- [ ] State machine with five states (Inactive, Triggered, Active, Completed, Aborted)
- [ ] Unified event log (append-only, immutable)
- [ ] Procedure run index (derived from event log)
- [ ] Trigger Watcher component
- [ ] Active Monitor component
- [ ] Hypothesis Generator component
- [ ] Concurrent procedure tracking
- [ ] Nested procedure support
- [ ] Deviation detection and recording
- [ ] Executor correction mechanism
- [ ] Query API for run history
- [ ] Integration tests for concurrent/nested scenarios

## 11. Design Principles

These survive the withdrawal intact — they constrain the replacement design
rather than describing the withdrawn one, and every one of them is echoed by
the annotation model.

- **Record reality, don't predict**: Engine tracks what happens, learning is separate
- **Immutable history**: Event log never edited, only appended — a transition is a *new record citing the prior one*, never an edit (`living_corpus.md` §3)
- **Explicit deviations**: Template vs. as-run differences are first-class data
- **Executor as authority**: Corrections always override engine inference
- **Queryable patterns**: As-run history enables continuous improvement
- **General-purpose**: Applicable to any procedural domain

## 12. Use Cases

**Manufacturing**: SOP execution tracking, quality control, compliance auditing

**Lab Protocols**: Experiment documentation, reproducibility verification, safety compliance

**Deployment Runbooks**: Software release tracking, rollback detection, incident analysis

**Emergency Response**: Protocol adherence verification, training assessment, post-incident review

**Cooking**: Recipe execution timing, ingredient substitution tracking, technique variation

---

**Status**: **Withdrawn model, retained for requirements.** This document
described runtime execution infrastructure built on the three-piece as-run
model. §2 has been corrected to the current model; the remaining sections are
unrevised and are marked where they depend on withdrawn types. See
`docs/design/annotation/living_corpus.md` §2,
`docs/project/0_open/ISSUE_17_NOET_PROCEDURES_EXTRACTION.md` ("What Was Removed
and Why"), and `docs/project/0_open/ISSUE_18_EXTENDED_PROCEDURE_SCHEMAS.md`
(the undesigned replacement). Learning and adaptation remain out of scope (see
`redline_system.md`, which carries the same withdrawn framing).