---
title: "Redline System: As-Run Deviation Tracking"
authors: "Andrew Lyjak, Claude Code"
last_updated: "2025-01-XX"
status: "Active"
version: "0.1"
dependencies: ["procedure_schema.md (v0.1)", "procedure_execution.md (v0.1)", "intention_lattice.md (v0.1)"]
---

# Redline System: As-Run Deviation Tracking

## 1. Purpose

The **redline system** bridges template procedures to execution reality by tracking, recording, and analyzing deviations. It solves a fundamental problem in procedural systems: **Templates specify idealized behavior, but executors exhibit systematic variations.**

**Name Origin**: Like editorial redlines that mark revisions on drafts, the redline system marks the delta between template and reality, making variations explicit and analyzable.

**Key Design Principle**: This system is **general-purpose**. It applies to any domain where procedures are executed: manufacturing SOPs, lab protocols, deployment runbooks, emergency response, cooking recipes, etc.

**Out of Scope**: Behavior prediction and learning algorithms are product-specific extensions. This document covers only the core deviation tracking infrastructure.

## 2. The Template vs. Reality Gap

### 2.1 Core Problem

**Template (as-written):**
```toml
[procedure]
[[procedure.steps]]
type = "action"
id = "preheat_oven"

[[procedure.steps]]
type = "action"
id = "mix_ingredients"
duration_minutes = [5, 10]

[[procedure.steps]]
type = "action"
id = "pour_batter"

[[procedure.steps]]
type = "action"
id = "bake"
duration_minutes = [25, 30]
```

**Reality (as-run):**
```
Events: 
- preheat_oven (10:00)
- mix_ingredients (10:15, duration: 18min)  # Took longer than template
- bake (10:35, duration: 28min)             # Skipped "pour_batter" step
```

**Redline (recorded deviation):**
```
Procedure: "recipe_banana_bread"
Run ID: run_789
Deviations:
  - Step 2 "mix_ingredients": duration 18min (template: 5-10min)
  - Step 3 "pour_batter": SKIPPED
  - Step 4 "bake": executed (no deviation)
Match Quality: Loose (2 deviations)
```

### 2.2 Why Deviations Matter

Systematic deviations reveal:
- **Shortcuts**: Steps consistently skipped → template may be overly detailed
- **Extensions**: Extra steps consistently added → template missing critical operations
- **Timing differences**: Durations consistently different → unrealistic estimates
- **Equipment substitutions**: Different resources used → template assumes unavailable tools
- **Reordering**: Steps performed out of order → dependencies may not be strict

## 3. The Three Components

The redline system consists of three distinct responsibilities:

### 3.1 Deviation Detection

**Input**: 
- Template procedure (as-written)
- Execution record (as-run events)

**Output**: Deviation report

**Deviation Types:**

1. **Step Skipped**: Expected step not executed
2. **Step Reordered**: Steps executed in different sequence
3. **Step Added**: Extra step not in template
4. **Duration Mismatch**: Step took longer/shorter than template range
5. **Resource Substitution**: Different resources used than specified
6. **Quality Violation**: Prompt response outside expected range

### 3.2 Deviation Recording

**Storage**: Correction events in unified event log (see `procedure_execution.md`)

**Schema**:
```rust
pub struct ProcedureCorrectionEvent {
    pub event_id: String,
    pub timestamp: String,
    pub source: String,                // Always "executor"
    pub event_type: String,            // Always "procedure_correction"
    pub payload: CorrectionPayload,
}

pub struct CorrectionPayload {
    pub run_id: String,                // References completed run
    pub correction_type: CorrectionType,
    pub deviations: Vec<Deviation>,
    pub executor_note: Option<String>,
}

pub enum CorrectionType {
    Confirmed,          // "Yes, that's what I did"
    WrongProcedure,     // "No, I was actually doing X"
    PartialMatch,       // "Sort of, but I modified it"
    Rejected,           // "No, that didn't happen"
}

pub struct Deviation {
    pub step_index: usize,
    pub deviation_type: DeviationType,
    pub expected: Option<String>,      // What template specified
    pub actual: Option<String>,        // What really happened
}

pub enum DeviationType {
    Skipped,
    Reordered,
    Added,
    DurationMismatch,
    ResourceSubstitution,
    QualityViolation,
}
```

### 3.3 Deviation Analysis

**Purpose**: Aggregate deviations across multiple runs to identify patterns.

**Queries**:
- "Which steps are most often skipped?"
- "How does actual duration compare to template?"
- "Who deviates most from template?"
- "Which templates have highest deviation rates?"

**Output**: Statistical summaries and insights for template improvement.

## 4. Executor Confirmation Flow

### 4.1 Confirmation Prompt

After procedure completion, executor is shown:

```
Procedure: Daily Equipment Startup
Duration: 18 minutes (template: 15-20 min)

Detected steps:
✓ Power on reactor (2 min)
✓ Verify temperature < 200°F (1 min)
⚠ Circulation pump startup (SKIPPED)
✓ Record baseline readings (3 min)

Does this match what you did?
[Confirm] [Modify] [Wrong Procedure]
```

### 4.2 Correction Workflow

**Option 1: Confirm**
- System records as-run as accurate
- No further action needed

**Option 2: Modify**
- Executor edits specific deviations
- System updates run record with corrections
- Generates corrected deviation report

**Option 3: Wrong Procedure**
- Executor specifies actual procedure performed
- System updates run record with correct procedure_id
- Original procedure run marked as false positive

### 4.3 Correction Event Storage

All confirmations and corrections are stored in event log:

```json
{
  "event_id": "evt_corr_123",
  "timestamp": "2025-06-01T10:30:00Z",
  "source": "executor",
  "event_type": "procedure_correction",
  "payload": {
    "run_id": "run_456",
    "correction_type": "PartialMatch",
    "deviations": [
      {
        "step_index": 2,
        "deviation_type": "Skipped",
        "expected": "start_circulation_pump",
        "actual": null
      }
    ],
    "executor_note": "Pump was already running from yesterday"
  }
}
```

## 5. Template Promotion

### 5.1 When to Promote

A consistent as-run pattern can become a new template:

**Criteria**:
- Same deviations across ≥10 runs
- Low variance in deviation pattern
- Executor explicitly requests promotion

**Example**:
- Template "recipe_banana_bread" consistently executed without "pour_batter" step
- After 15 runs, system suggests: "Create variant 'recipe_banana_bread_simplified'?"

### 5.2 Promotion Mechanism

**Process**:
1. Identify stable deviation pattern
2. Generate new procedure definition incorporating deviations
3. Present to executor for review
4. Create new lattice node with `promoted_from` metadata

**New Procedure Metadata**:
```toml
id = "recipe_banana_bread_simplified"
title = "Banana Bread (Simplified)"

promoted_from_redline = true
original_template = "recipe_banana_bread"
promoted_at = "2025-06-15T10:00:00Z"
deviation_pattern = "step_3_skipped"

[procedure]
# Steps reflect learned as-run pattern
[[procedure.steps]]
type = "action"
id = "preheat_oven"

[[procedure.steps]]
type = "action"
id = "mix_ingredients"
duration_minutes = [15, 20]  # Learned actual duration

# "pour_batter" step removed - consistently skipped

[[procedure.steps]]
type = "action"
id = "bake"
```

### 5.3 Lattice Integration

Promoted procedures maintain connection to original:

```toml
[[parent_connections]]
parent_id = "recipe_banana_bread"
weight_kind = "Pragmatic"
note = "Simplified variant derived from as-run analysis"
```

**Benefits**:
- Track template evolution
- Maintain attribution
- Enable version comparison
- Support rollback if needed

## 6. Deviation Analysis Queries

### 6.1 Frequency Analysis

```rust
// Steps most often skipped
fn most_skipped_steps(procedure_id: &str) -> Vec<(StepId, f64)>;
// Returns: [(step_id, skip_rate), ...]

// Steps most often reordered
fn reordering_frequency(procedure_id: &str) -> Vec<(StepId, StepId, u32)>;
// Returns: [(step_a, step_b, swap_count), ...]
```

### 6.2 Timing Analysis

```rust
// Actual vs. template duration
fn duration_comparison(procedure_id: &str) -> DurationStats {
    template_mean: Duration,
    actual_mean: Duration,
    actual_variance: Duration,
    runs_analyzed: u32,
}

// Per-step timing
fn step_duration_stats(procedure_id: &str, step_id: &str) -> StepTiming;
```

### 6.3 Executor Patterns

```rust
// Executor deviation rates
fn executor_deviation_rate(procedure_id: &str) -> HashMap<ExecutorId, f64>;

// Executor-specific patterns
fn executor_deviation_patterns(
    procedure_id: &str, 
    executor_id: &str
) -> Vec<Deviation>;
```

## 7. Use Cases

### 7.1 Manufacturing

**Scenario**: SOP consistently performed with steps reordered

**Redline Action**: 
- Record deviation: "Steps 3 and 4 swapped in 12/15 runs"
- Analysis: Steps 3-4 have no strict dependency
- Recommendation: Update template to use `all_of` instead of `sequence`

### 7.2 Lab Protocols

**Scenario**: Reagent substitution consistently used

**Redline Action**:
- Record deviation: "Item 'reagent_A' substituted with 'reagent_B' in 8/10 runs"
- Analysis: Reagent_A often out of stock
- Recommendation: Add reagent_B to template as alternative

### 7.3 Deployment Runbooks

**Scenario**: Manual verification step consistently skipped

**Redline Action**:
- Record deviation: "Step 'verify_health_check' skipped in 18/20 runs"
- Analysis: Health check automated in recent update
- Recommendation: Remove manual step, rely on automation

### 7.4 Emergency Response

**Scenario**: Extra communication step consistently added

**Redline Action**:
- Record deviation: "Additional step 'notify_supervisor' added in 9/10 runs"
- Analysis: Recent policy change not reflected in template
- Recommendation: Add supervisor notification to template

### 7.5 Cooking

**Scenario**: Baking time consistently longer than template

**Redline Action**:
- Record deviation: "Step 'bake' duration 35min (template: 25-30min)"
- Analysis: Oven temperature calibration issue
- Recommendation: Update template range or note oven variability

## 8. Implementation Checklist

For libraries implementing redline system:

- [ ] Deviation detection algorithm
- [ ] Correction event schema
- [ ] Executor confirmation UI/prompts
- [ ] Deviation storage in event log
- [ ] Frequency analysis queries
- [ ] Timing analysis queries
- [ ] Executor pattern analysis
- [ ] Template promotion mechanism
- [ ] Lattice integration for promoted templates
- [ ] Audit trail for all corrections

## 9. Extension Points

This architecture supports downstream extensions:

### 9.1 Predictive Matching (Product-Specific)

Products can add:
- Probabilistic step matching (HMM, edit distance)
- Skip probability learning
- Duration prediction models
- Automatic deviation detection

**Integration**: Read from event log, write learned parameters to separate tables.

### 9.2 Adaptive Templates (Product-Specific)

Products can add:
- Automatic template adjustment based on patterns
- Personalized procedure variants per executor
- Context-aware template selection

**Integration**: Use deviation analysis to inform template generation.

### 9.3 Compliance Checking (Implementation-Specific)

Applications can add:
- Critical step validation (never skip)
- Deviation thresholds (abort if too many)
- Approval workflows for deviations
- Audit reports

**Integration**: Query deviation records, enforce policies.

## 10. Design Principles

- **Record deviations explicitly**: Template vs. as-run delta is first-class data
- **Executor as authority**: Corrections always override system inference
- **No automatic learning**: Deviation patterns inform humans, don't silently change templates
- **Traceable evolution**: Promoted templates maintain attribution and history
- **General-purpose**: Applicable to any procedural domain
- **Extensible**: Products can add learning without forking

## 11. Open Questions

1. **Deviation Severity**: Should some deviations (e.g., skipping safety checks) be flagged differently?
2. **Promotion Thresholds**: How many consistent runs before suggesting promotion?
3. **Multi-Executor Patterns**: How to handle different executors with different deviation patterns?
4. **Template Versioning**: How to handle deviations when template has been updated?
5. **Rollback**: How to demote a promoted template if it proves problematic?

---

**Status**: This design defines core deviation tracking infrastructure. Predictive matching and learning algorithms are out of scope (product-specific extensions).