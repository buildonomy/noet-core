---
title = "noet-procedures: Observable, Auditable, Event-Driven Procedures"
authors = "Andrew Lyjak, Claude"
last_updated = "2025-01-24"
status = "Active"
version = "0.1"
dependencies = []
---

# noet-procedures

**Observable, auditable, event-driven procedure execution for any domain.**

## What is this?

`noet-procedures` is a general-purpose library for executing procedures defined in noet lattices. It provides:

- **Observable action detection**: Procedures advance when observation events match defined patterns
- **As-run recording**: Complete audit trail of template vs. reality
- **Redline deviation tracking**: Systematic differences between intention and execution
- **Multi-modal observations**: Sensor data, system metrics, and participant input treated uniformly
- **Template promotion**: Convert consistent as-run patterns into new templates

Unlike embedded computation models (Jupyter notebooks), noet-procedures **separates intention from execution**, making every step observable and auditable.

## Why does this exist?

### The Problem: Jupyter/Binder Model

Jupyter notebooks embed computation **in** documents:

```python
# Cell 1: Load data
data = load_csv("sample.csv")

# Cell 2: Process
result = analyze(data)

# Cell 3: Visualize
plot(result)
```

**Problems**:
- **Opaque state**: Kernel holds state invisibly - which variables exist? what values?
- **Hidden execution order**: Cell 5 depends on cell 3, but you re-ran cell 4 first
- **"Trust me" reproducibility**: Papermill parameterizes, Binder containerizes, but execution is still a black box
- **No deviation tracking**: If something goes wrong, debug by re-running
- **Static output**: Binder gives you a snapshot of completed execution

### The Solution: Observable Event-Driven Execution

noet-procedures **separates** concerns:

```toml
# Lattice document (intention)
---
bid = "analyze_sample"
title = "Analyze Sample Data"

[inference_hint]
channel = "FileSystem"
producer = "FileCreated"
to = ["sample.csv"]
---

When sample.csv appears, load and analyze it.
```

**Execution**:
1. **BeliefSet** (compiled structure): WHAT to do, relationships between nodes
2. **Procedure Engine** (runtime): Watches event streams, matches patterns
3. **As-Run Record** (audit trail): What ACTUALLY happened vs. template

**As-Run Record**:
```
Template: "analyze_sample" procedure
Started: 2025-01-24T10:15:00Z

Step 1 (wait for file):
  Expected: sample.csv created
  Observed: sample_batch_b.csv created (DEVIATION)
  Matched: FileSystem.FileCreated → "sample_batch_b.csv"
  Confidence: 0.85 (filename variation)
  Timestamp: 10:15:23Z
  
Step 2 (process):
  Expected: analyze()
  Observed: analyze() with temperature_warning
  Event: AnalysisCompleted, warnings=["temp_range"]
  Timestamp: 10:16:45Z
```

## Key Differences

| Aspect | Jupyter/Binder | Airflow | Terraform | noet-procedures |
|--------|----------------|---------|-----------|-----------------|
| **Domain** | Data analysis | Data pipelines | Infrastructure | Any procedural domain |
| **Computation** | Embedded IN document | Python DAG tasks | HCL declarative config | Event-driven channels |
| **State** | Hidden in kernel | Task status (success/fail) | Desired vs. actual state | Template vs. as-run |
| **Mutability** | Edit cells, no history | Modify DAG code | Detect drift, reconcile | Redline deviations, learn |
| **Transparency** | Opaque execution | Task logs, lineage | Plan/apply output | Observable event matching |
| **Deviation tracking** | None | Task retries only | Drift detection | Complete as-run analysis |
| **Learning from reality** | None | None | None (reconciles back) | Template promotion |
| **Output** | Static snapshot | Task completion records | State file | As-run audit trail |
| **Reproducibility** | "Rerun the notebook" | "Rerun the DAG" | "Reapply the plan" | "Here's the event stream" |
| **Multi-modal** | One kernel | One task type | One provider type | Unified observable model |

## Comparisons to Other Tools

### vs. Apache Airflow (Workflow Orchestration)

**Airflow** is a popular workflow orchestration platform for data pipelines.

**What Airflow does well**:
- Schedule and orchestrate data pipelines
- Manage task dependencies (DAGs)
- Retry failed tasks
- Track task completion

**What Airflow doesn't do**:
```python
# Airflow DAG
task1 = PythonOperator(task_id='load_data', ...)
task2 = PythonOperator(task_id='process_data', ...)
task1 >> task2  # Define dependency

# Airflow knows: "Did task2 complete?"
# Airflow doesn't know: "How did task2 actually execute vs. the template?"
```

**Key differences**:

1. **Completion vs. Deviation**: Airflow tracks whether tasks completed; noet-procedures tracks how execution deviated from template
2. **No redline system**: Airflow has no concept of "we consistently skip this task" → promote to new workflow
3. **Code-centric**: DAGs defined in Python; noet uses lattice documents with observable patterns
4. **Single modality**: Tasks run Python/Bash; noet unifies sensors, systems, and participant input
5. **Data pipeline focus**: Airflow optimized for ETL; noet is domain-agnostic

**Example**: Lab protocol execution

**Airflow approach**:
```python
# Tasks succeed or fail
reagent_task = BashOperator(task_id='add_reagent', bash_command='...')
wait_task = TimeDeltaSensor(task_id='wait_10min', delta=timedelta(minutes=10))
measure_task = PythonOperator(task_id='measure', python_callable=measure_func)

# Can't track: "We actually used reagent B (not A)" or "We waited 12 minutes (not 10)"
```

**noet-procedures approach**:
```toml
# Template defines intention
[[procedure.steps]]
[inference_hint]
channel = "Barcode"
to = ["reagent_A"]

# As-run records reality
Deviation: Used reagent_B (expected: reagent_A)
Executor note: "Reagent A out of stock"
Recorded in audit trail, analyzable for patterns
```

**When to use Airflow**: Batch data pipelines, ETL workflows, scheduled jobs
**When to use noet-procedures**: Any domain requiring deviation analysis, compliance, template learning

---

### vs. Terraform (Infrastructure as Code)

**Terraform** manages infrastructure with declarative configuration and drift detection.

**What Terraform does well**:
- Declare desired infrastructure state
- Detect drift (actual vs. desired)
- Plan changes before applying
- Reconcile infrastructure to match configuration

**What Terraform doesn't do**:
```hcl
# Terraform config
resource "aws_instance" "web" {
  instance_type = "t3.medium"
  ami           = "ami-12345"
}

# Terraform detects: "Instance is t3.large (expected: t3.medium)"
# Terraform's response: Reconcile back to t3.medium
# Terraform doesn't: Learn that t3.large might be better, track why drift occurred
```

**Key differences**:

1. **Reconciliation vs. Learning**: Terraform forces reality back to desired state; noet-procedures records deviations and learns from them
2. **No template promotion**: Terraform doesn't say "we consistently use t3.large, let's update the template"
3. **Infrastructure-only**: Terraform is for servers/cloud; noet is for any procedural domain
4. **State file vs. event stream**: Terraform maintains state snapshots; noet maintains event streams
5. **No executor confirmation**: Terraform can't ask "did you intentionally change this?"

**Example**: Deployment procedure

**Terraform approach**:
```hcl
# Desired state
resource "kubernetes_deployment" "app" {
  replicas = 3
}

# Drift detected: replicas = 5 (someone scaled manually)
# Terraform: "Reconciling back to 3" (overwrites manual change)
# Lost: Why was it scaled? Was it intentional? Should we update the template?
```

**noet-procedures approach**:
```toml
# Template
[[procedure.steps]]
title = "Deploy with 3 replicas"
[inference_hint]
channel = "Kubernetes"
producer = "Deployment"
to = ["replicas_3"]

# As-run
Deviation: Deployed with 5 replicas (expected: 3)
Executor confirmation: "Scaled for high traffic event"
Redline: Record deviation, analyze if pattern emerges
If consistent: Promote to new template "Deploy (high traffic variant)"
```

**Conceptual overlap**:
- Both compare reality to template (drift vs. deviation)
- Both track what actually happened vs. what was planned

**Fundamental difference**:
- **Terraform**: "Force reality to match template"
- **noet-procedures**: "Record reality, learn from deviations"

**When to use Terraform**: Infrastructure provisioning, declarative state management
**When to use noet-procedures**: When deviations are signals (not errors), any procedural domain, learning from reality

---

## Architecture: Three Layers

### 1. Intention (Lattice Documents)

BeliefNodes define **WHAT** should happen:

```toml
---
bid = "step_measure_temp"
title = "Measure Sample Temperature"

[inference_hint]
channel = "Participant"
producer = "Measurement"

[inference_hint.response_config]
form_element = "number"
stores_in_variable = "sample_temp"
min = -20
max = 100
---

Enter the current temperature using the digital thermometer.
```

### 2. Execution (Procedure Engine)

Watches event streams, matches patterns:

```
Event Stream:
  [10:15:00] FileCreated: "sample_batch_b.csv"
  [10:15:23] Temperature: 22.5°C (Participant.Measurement)
  [10:16:45] AnalysisCompleted: warnings=["temp_range"]

Procedure Engine:
  ✓ Matched step_measure_temp (confidence: 1.0)
  → Stored sample_temp = 22.5 in run context
  → Advanced to next step
```

### 3. Reality (As-Run Record)

Complete audit trail with deviations:

```
Run ID: run_789
Template: "lab_protocol_A"
Started: 10:15:00
Completed: 10:25:30

Deviations:
  - Step 2: Duration 18min (expected: 5-10min)
  - Step 3: Used reagent_B (expected: reagent_A)
  - Step 5: SKIPPED
  
Executor Confirmation: "Partial match - reagent_A was out of stock"
```

## Unified Observable Model

All observations use the same pattern - whether from sensors, systems, or participants:

```toml
# Automatic sensor observation
[inference_hint]
channel = "Temperature"
producer = "Incubator_A"
to = ["37C"]

# System metric observation
[inference_hint]
channel = "SystemMetrics"
producer = "HealthCheck"
to = ["healthy"]

# Participant observation (interactive prompt)
[inference_hint]
channel = "Participant"
producer = "Measurement"
[inference_hint.response_config]
form_element = "number"
stores_in_variable = "temp_reading"
```

### Observation Channels Enable Arbitrary Computation

**Key insight**: Observation channels provide the same computational power as Jupyter cells, but with observability.

**Jupyter approach** (computation embedded):
```python
# Cell 1
data = load_data()

# Cell 2  
result = expensive_computation(data)

# Cell 3
plot(result)
```

**noet-procedures approach** (computation in observation channels):

```
Procedure Engine emits:
  → action_detected: "step_load_data" (Run 123)

Computation Channel subscribes to engine output:
  1. Filter: Listen for "step_load_data" events
  2. Trigger: Run_123.step_load_data detected
  3. Compute: expensive_computation(data)
  4. Output: ObservationEvent { 
       channel: "Analytics", 
       producer: "ComputationPipeline",
       value: result,
       references_run: "Run_123"
     }

Procedure Engine receives observation:
  → Matches to next step in Run 123
  → Links result to as-run state machine
  → Continues execution
```

**The pattern**:

```
┌─────────────────────────────────────────────────┐
│ Procedure Engine                                │
│ • Emits action_detected events                  │
│ • Maintains as-run state machines               │
└────────────────┬────────────────────────────────┘
                 │ (output stream)
                 ▼
┌─────────────────────────────────────────────────┐
│ Observation Channel (arbitrary computation)     │
│ • Subscribes to procedure engine output         │
│ • Filters for trigger events                    │
│ • Performs computation (plot, analyze, etc.)    │
│ • Emits ObservationEvent back to engine         │
└────────────────┬────────────────────────────────┘
                 │ (input stream)
                 ▼
┌─────────────────────────────────────────────────┐
│ Procedure Engine                                │
│ • Receives observation                          │
│ • Matches to waiting step                       │
│ • Links to as-run state machine                 │
│ • Records in audit trail                        │
└─────────────────────────────────────────────────┘
```

**Example: Data Analysis Pipeline**

```toml
# Procedure definition
---
bid = "step_analyze_results"
title = "Analyze Experimental Results"

[inference_hint]
channel = "Analytics"
producer = "StatisticalAnalysis"
# Waits for analysis to complete
---

Results will be analyzed and plotted automatically.
```

**Implementation**:

```rust
// Observation channel performs computation
struct AnalyticsChannel {
    engine_output: Subscriber<ActionDetectedEvent>,
}

impl AnalyticsChannel {
    fn run(&mut self) {
        for event in self.engine_output.iter() {
            // Filter: Only trigger on specific steps
            if event.node_bid == "step_load_data" {
                // Compute: Arbitrary expensive operation
                let data = load_from_event(&event);
                let results = expensive_statistical_analysis(data);
                let plot = generate_visualization(results);
                
                // Emit: Send back to procedure engine
                self.emit_observation(ObservationEvent {
                    channel: "Analytics",
                    producer: "StatisticalAnalysis",
                    value: serde_json::to_value(&results),
                    metadata: plot_path,
                    references_run: event.run_id,
                });
            }
        }
    }
}
```

**Advantages over Jupyter**:

1. **Observable**: Computation triggered by explicit events (not hidden cell execution)
2. **Auditable**: Input event and output event both recorded in as-run trail
3. **Decoupled**: Computation happens in separate process, doesn't block engine
4. **Reusable**: Same channel can serve multiple procedures
5. **Composable**: Chain channels (Analysis → Visualization → Notification)
6. **Testable**: Mock the observation channel, verify events

**Example: Complex Pipeline**

```
Step 1: Load Data
  → Triggers "DataProcessing" channel
  → Performs ETL, validation, transformation
  → Emits: ObservationEvent("data_ready")

Step 2: Analyze (waits for "data_ready")
  → Triggers "Analytics" channel  
  → Runs statistical models, generates plots
  → Emits: ObservationEvent("analysis_complete")

Step 3: Report (waits for "analysis_complete")
  → Triggers "Reporting" channel
  → Generates PDF, sends email
  → Emits: ObservationEvent("report_sent")

All events recorded in as-run trail with timestamps.
```

**This is how noet-procedures achieves Jupyter's computational flexibility while maintaining observability**: computation happens in observation channels that filter the procedure engine's output stream, compute, and emit results back as observations.

### Multi-Modal Verification

Combining multiple observation sources is natural:

```toml
# Either automatic sensor OR manual confirmation
[inference_hint]
operator = "any_of"

[[inference_hint.events]]
channel = "Temperature"
producer = "Sensor"
to = ["37C"]

[[inference_hint.events]]
channel = "Participant"
producer = "Confirmation"
[inference_hint.events.response_config]
form_element = "checkbox"
stores_in_variable = "temp_confirmed"
```

## Redline System: Mutable but Transparent

Unlike Jupyter (mutable but opaque), noet-procedures provides **mutable execution with complete transparency**.

### The Redline Channel

The redline system is a **privileged observable channel** that can inject BeliefEvents to modify the loaded BeliefSet:

```
Template (as-written):
  Step 1 → Step 2 → Step 3

Reality (as-run):
  Step 1 ✓
  Step 3 ✓ (Step 2 skipped)
  
Redline Event (recorded):
  CorrectionEvent {
    type: "step_skipped",
    step: "Step 2",
    executor_note: "Step 2 automated last week",
    timestamp: "2025-01-24T10:30:00Z"
  }
```

**Key features**:
- Every deviation recorded with attribution
- Executor confirms/corrects system inferences
- Consistent deviations can be promoted to new templates
- Complete audit trail of all changes

### Template Promotion

Systematic deviations become new templates:

```
Observation: Step 3 skipped in 15/15 runs over 2 weeks
Analysis: Step consistently unnecessary
Action: Promote to new template variant

New template: "lab_protocol_A_simplified"
  - Inherits from "lab_protocol_A"
  - Step 3 removed
  - Metadata: promoted_from_redline = true
```

**Jupyter**: Edit cells, no record of why
**noet-procedures**: Record deviations, analyze patterns, promote with attribution

## Stateful Execution: Local vs. Distributed

Unlike Binder (static snapshot), procedure engines are **stateful** - whoever controls the engine sees execution state in real-time.

### Local Execution

```
Your device:
├── Procedure Engine (local)
├── Event streams (local sensors, your input)
└── As-run logs (private)

Use case: Personal productivity, behavior tracking
State visibility: Private
```

### Enterprise Execution

```
Company infrastructure:
├── Central Procedure Engine
├── Distributed event streams
│   ├── Factory floor sensors
│   ├── Lab instruments
│   ├── Multiple operator confirmations
│   └── System metrics
└── Unified as-run repository

Use case: Manufacturing, compliance, coordination
State visibility: Real-time dashboard, auditable
```

**Configuration determines scope**:

```toml
# Local mode
[procedure_engine]
mode = "local"
event_sources = ["device_sensors", "participant_input"]
storage = "local_db"

# Distributed mode
[procedure_engine]
mode = "distributed"
event_sources = [
    "factory_sensors",
    "operator_stations",
    "system_monitors"
]
storage = "central_db"
authorization = "keyhive_capabilities"
```

## Use Cases

### Scientific Reproducibility

**Jupyter**: "I ran analysis v2, got result R"
- Which data version? Which packages? What order? What kernel state?

**noet-procedures**: "Procedure P matched events E₁, E₂, E₃"
- Complete audit: which file, which sensor, which params, all timestamped
- Deviations recorded: "Expected batch A, used batch B"

### Manufacturing SOPs

**Paper checklist**: ☑ Steps 1-5 completed
- When? How long? Any deviations?

**noet-procedures**:
- Step 1: Barcode scan at 14:32:15
- Step 2: Temp reached 37.2°C at 14:35:03, held 5min 12sec
- Deviation: 12 seconds longer than spec (recorded)

### Lab Protocols

**Lab notebook**: "Added 5ml reagent A, waited 10 min"
- Did they? When exactly? What was actual wait time?

**noet-procedures**:
- Template: Add 5ml reagent A → wait 10 min → measure
- As-run: Reagent B (deviation), waited 10:15 (deviation), measured 0.432, all timestamped

### Deployment Runbooks

**Runbook**: "Verify service health, deploy to production"
- How was health verified? When did deployment start?

**noet-procedures**:
- Step 1: SystemMetrics.HealthCheck matched (confidence: 0.95) at 15:30:00
- Step 2: Participant.Confirmation from operator_smith at 15:32:15
- Step 3: Deployment started at 15:32:30, completed 15:45:10

### Emergency Response

**Training manual**: "Assess situation, notify supervisor, execute protocol"
- Which protocol? What was the actual sequence?

**noet-procedures**:
- As-run: Assessed → Executed protocol (REORDERED - skipped notification)
- Redline: "Notified supervisor AFTER execution due to urgency"
- Analysis: Common pattern under time pressure → update template

## General-Purpose Design

noet-procedures is **domain-agnostic**. It does not assume:
- Behavior change (not just for habits/productivity)
- Personal use (works for enterprise too)
- Specific observation types (you define channels/producers)
- Prediction/learning (you can add this as an extension)

**Applicable to any domain with procedures**:
- Manufacturing SOPs
- Lab protocols  
- Deployment runbooks
- Quality control checklists
- Emergency response procedures
- Cooking recipes
- Construction checklists
- Medical protocols

## Extension Points

Products extend noet-procedures by implementing:

### 1. Observation Producers

```rust
trait ObservationProducer {
    fn channel(&self) -> &str;
    fn producer(&self) -> &str;
    fn start(&mut self);
    fn stop(&mut self);
}
```

Examples: Barcode scanners, temperature sensors, GPS, system monitors, UI frameworks

### 2. Inference Engine

```rust
trait InferenceEngine {
    fn register_pattern(&mut self, node_bid: Bid, hint: InferenceHint);
    fn process_observation(&mut self, event: ObservationEvent);
    fn emit_action_detected(&self, detection: ActionDetection);
}
```

Responsibilities: Pattern matching, confidence scoring, semantic label resolution

### 3. UI Renderer (for Participant channel)

```rust
trait ParticipantRenderer {
    fn render_observation_request(&self, step: &BeliefNode) -> Result<()>;
    fn collect_response(&self) -> Result<ObservationEvent>;
}
```

Uses BeliefNode title and text for prompt content

## Key Architectural Insights

### 1. Separation of Intention and Execution

**Jupyter**: Computation embedded in document (opaque)
**noet-procedures**: Intention (lattice) separate from execution (event-driven)

### 2. Observable Event Streams

**Jupyter**: Hidden kernel state
**noet-procedures**: All observations explicit, matchable, recordable

### 3. Mutable with Transparency

**Jupyter**: Edit cells, no history
**noet-procedures**: Redline deviations, complete audit trail

### 4. Stateful vs. Static

**Binder**: Static snapshot of completed execution
**noet-procedures**: Stateful engine, real-time visibility

### 5. Unified Observable Model

**All observations** (sensors, systems, participants) use the same schema - natural multi-modal patterns

## Getting Started

```rust
use noet_procedures::{ProcedureEngine, ObservationEvent};

// Load procedures from lattice
let engine = ProcedureEngine::new(belief_set);

// Register observation producers
engine.register_producer(TemperatureSensor::new());
engine.register_producer(ParticipantUI::new());

// Process observation events
let event = ObservationEvent {
    channel: "Temperature",
    producer: "Sensor_A",
    value: "37.2",
    timestamp: now(),
};
engine.process_observation(event);

// Query as-run records
let runs = engine.completed_runs("procedure_id");
for run in runs {
    println!("Deviations: {:?}", run.deviations);
}
```

## Philosophy

> "Jupyter notebooks put computation IN documents, making execution opaque. noet-procedures separates intention from execution, producing complete audit trails of what actually happened. It's not just reproducible - it's **observable**."

## Learn More

- **Observable Action Schema**: `docs/design/action_observable_schema.md`
- **Procedure Execution**: `docs/design/procedure_execution.md`
- **Redline System**: `docs/design/redline_system.md`
- **Procedure Schema**: `docs/design/procedure_schema.md`

## License

[To be determined - likely MIT/Apache-2.0 dual license]