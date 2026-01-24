---
title: "Procedure Execution: Runtime Tracking and As-Run Recording"
authors: "Andrew Lyjak, Gemini 2.5 Pro, Claude Code"
last_updated: "2025-01-XX"
status: "Active"
version: "0.1"
dependencies: ["procedure_schema.md (v0.1)", "intention_lattice.md (v0.1)"]
---

# Procedure Execution Architecture

## 1. Purpose

This document specifies the runtime architecture for executing and tracking procedures defined by the procedure schema. It defines:

- Procedure lifecycle state machine (Inactive → Triggered → Active → Completed/Aborted)
- Event log architecture (unified, append-only audit trail)
- Run index (queryable as-run history)
- Concurrency and nesting handling
- Integration points for downstream extensions

**Key Design Principle**: This architecture is **general-purpose**. It provides execution tracking for any procedural domain: manufacturing SOPs, lab protocols, deployment runbooks, cooking recipes, project workflows, etc.

**Out of Scope**: Behavior prediction, learning algorithms, and sensor integration are product-specific extensions. This document covers only the core execution infrastructure.

## 2. The Three-Piece "As-Run" Model

Every procedure execution consists of three components:

1. **Template (As-Written)**: The procedure definition from the lattice
2. **Executor Context (Who/When/Where)**: Metadata about the execution
3. **As-Run Record (Reality)**: What actually happened, including deviations

This model is fundamental to procedural systems in aviation, manufacturing, and lab protocols where practitioners mark up procedures as they execute them.

## 3. Procedure Lifecycle

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

**Update Responsibility**: Active Monitor creates run record on first step, updates as procedure progresses.

### 4.3 Executor Context

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

## 7. Deviation Handling

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

**Integration Point**: Corrections feed into redline system for learning (see `redline_system.md`).

## 8. Query API

Execution history enables powerful analysis:

### 8.1 Basic Queries

```rust
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

- **Record reality, don't predict**: Engine tracks what happens, learning is separate
- **Immutable history**: Event log never edited, only appended
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

**Status**: This design defines runtime execution infrastructure. Learning and adaptation are out of scope (see `redline_system.md` for adaptive features).