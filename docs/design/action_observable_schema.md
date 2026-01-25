---
title = "Observable Action Schema"
authors = "Andrew Lyjak, Claude"
last_updated = "2025-01-24"
status = "Active"
version = "0.2"
dependencies = ["procedure_schema.md (v0.1)", "procedure_execution.md (v0.1)", "beliefbase_architecture.md"]
---

# Observable Action Schema

## Purpose

This document defines the schema for marking procedure steps as **observable** - detected from external event streams rather than explicitly triggered. It specifies:

- The `inference_hint` property for action steps
- Recursive pattern structure (grouping and transition events)
- Temporal and confidence constraints
- Participant channel for interactive observations
- Integration with procedure execution

This schema is **general-purpose**: it defines *what patterns to match*, not *how to observe* them. Observation producers (sensors, monitors, scanners, participant input) are product-specific implementations.

## Guiding Principle: Unified Observation Model

All procedure steps advance via observation events - whether from sensors, systems, or participants. This unified model treats:
- Sensor readings (passive observation)
- System metrics (automatic detection)
- Participant input (active observation)

...as variations of the same pattern: the procedure waits for a matching event on a specified channel.

**Use cases**:
- **Manufacturing**: Barcode scans, sensor readings, operator confirmations
- **Lab protocols**: Instrument readings, reagent tracking, manual measurements
- **Deployment runbooks**: Service health, configuration state, operator verification
- **Quality control**: Automated measurements, visual inspections, defect logging

## Architecture

Every observable action step is a **BeliefNode** with:
- `bid`: Unique identifier
- `title`: Human-readable step name
- Markdown text: Explanation of why this step exists (rendered as description/prompt)
- `inference_hint`: The observation pattern to match

```
┌─────────────────────────────────────────────────┐
│ BeliefNode (Procedure Step)                     │
│ ┌─────────────────────────────────────────────┐ │
│ │ bid: "step_scan_part"                       │ │
│ │ title: "Scan Assembly Part"                 │ │
│ │ text: "Use barcode scanner to identify..."  │ │
│ │                                             │ │
│ │ [inference_hint]                            │ │
│ │ channel = "Barcode"                         │ │
│ │ producer = "Scanner"                        │ │
│ │ to = ["part_123"]                           │ │
│ └─────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────┘
                       │
                       ▼
┌─────────────────────────────────────────────────┐
│ Observation Pipeline (Product-Specific)         │
│ • Barcode scanner emits observation events      │
│ • Inference engine matches to hint              │
│ • action_detected event emitted                 │
└─────────────────────────────────────────────────┘
                       │
                       ▼
┌─────────────────────────────────────────────────┐
│ Procedure Engine                                │
│ • Matches event to waiting step                 │
│ • Advances state machine                        │
│ • Records as-run execution                      │
└─────────────────────────────────────────────────┘
```

## Schema Definitions

### Observable Action Steps

A procedure step (BeliefNode) can include an `inference_hint` property in its payload:

```toml
---
bid = "step_verify_temp"
title = "Verify Incubator Temperature"

[inference_hint]
channel = "Temperature"
producer = "Incubator_A"
to = ["37C"]
min_duration_minutes = 5
---

Confirm the incubator is maintaining 37°C ±0.5°C for at least 5 minutes before proceeding.
```

The markdown text provides context (why this step exists), while the `inference_hint` defines the observation pattern.

### Inference Hint Schema

```toml schema:action_observable.inferenceHint
# Root schema for observable patterns - recursive union type

[[oneOf]]
"$ref" = "#/$defs/groupingEvent"

[[oneOf]]
"$ref" = "#/$defs/transitionEvent"
```

An inference hint is a **recursive structure** that can be:
1. **Grouping Event**: Logical combination of multiple patterns
2. **Transition Event**: Single observation event pattern

### Grouping Events

Logical operators for combining multiple observation patterns.

```toml schema:action_observable.groupingEvent
type = "object"
required = ["operator", "events"]

[properties.operator]
type = "string"
enum = ["any_of", "all_of", "none_of"]
description = "Logical operator combining child events"

[properties.events]
type = "array"
description = "Child inference events (grouping or transition)"
minItems = 1

[properties.min_duration_minutes]
type = "number"
description = "Minimum duration all conditions must hold"

[properties.max_duration_minutes]
type = "number"
description = "Maximum duration before pattern expires"

[properties.confidence_threshold]
type = "number"
minimum = 0.0
maximum = 1.0
description = "Minimum confidence required for match"

[properties.time_of_day]
type = "array"
description = "Time windows when pattern is valid (24-hour format)"

[properties.day_of_week]
type = "array"
description = "Days when pattern is valid"

[properties.calendar_date]
type = "string"
format = "date"
description = "Specific date when pattern is valid"

[properties.events.items]
[[properties.events.items.oneOf]]
"$ref" = "#/$defs/groupingEvent"

[[properties.events.items.oneOf]]
"$ref" = "#/$defs/transitionEvent"

[properties.time_of_day.items]
type = "string"
pattern = "^([01]?[0-9]|2[0-3]):[0-5][0-9]$"
examples = ["09:00", "17:30"]

[properties.day_of_week.items]
type = "string"
enum = ["monday", "tuesday", "wednesday", "thursday", "friday", "saturday", "sunday"]
```

**Operator semantics**:
- `any_of`: Match if ANY child event matches
- `all_of`: Match if ALL child events match simultaneously
- `none_of`: Match if NONE of the child events match

### Transition Events

Single observation event pattern.

```toml schema:action_observable.transitionEvent
type = "object"
required = ["channel", "producer"]

[properties.channel]
type = "string"
description = "Observation channel (domain-specific namespace)"
examples = ["Location", "Barcode", "Temperature", "SystemMetrics", "Participant"]

[properties.producer]
type = "string"
description = "Specific producer within channel"
examples = ["Scanner", "GPS", "Thermometer", "HealthCheck", "Prompt", "Measurement"]

[properties.from]
type = "array"
description = "Semantic labels for source state (empty = wildcard)"

[properties.to]
type = "array"
description = "Semantic labels for target state (empty = wildcard)"

[properties.min_duration_minutes]
type = "number"
description = "Minimum duration in target state"

[properties.max_duration_minutes]
type = "number"
description = "Maximum duration in target state"

[properties.confidence_threshold]
type = "number"
minimum = 0.0
maximum = 1.0
description = "Minimum confidence required"

[properties.time_of_day]
type = "array"
description = "Time windows when pattern is valid"

[properties.day_of_week]
type = "array"
description = "Days when pattern is valid"

[properties.calendar_date]
type = "string"
format = "date"

[properties.response_config]
type = "object"
description = "Response capture configuration (for Participant channel)"

[properties.from.items]
type = "string"

[properties.to.items]
type = "string"

[properties.time_of_day.items]
type = "string"
pattern = "^([01]?[0-9]|2[0-3]):[0-5][0-9]$"

[properties.day_of_week.items]
type = "string"
enum = ["monday", "tuesday", "wednesday", "thursday", "friday", "saturday", "sunday"]

# Response configuration schema (for Participant channel)
[properties.response_config.properties.form_element]
type = "string"
enum = ["text", "textarea", "number", "select", "multi_select", "checkbox", "radio", "slider"]
description = "UI element type for input capture"

[properties.response_config.properties.stores_in_variable]
type = "string"
description = "Variable name to store response in run context"

[properties.response_config.properties.options]
type = "array"
description = "Options for select/multi_select/radio types"

[properties.response_config.properties.min]
type = "number"
description = "Minimum value for number/slider"

[properties.response_config.properties.max]
type = "number"
description = "Maximum value for number/slider"

[properties.response_config.properties.step]
type = "number"
description = "Increment step for slider"

[properties.response_config.properties.default_value]
description = "Default value (type varies by form_element)"

[properties.response_config.properties.required]
type = "boolean"
default = true
description = "Whether response is required to proceed"

[properties.response_config.properties.validation_pattern]
type = "string"
description = "Regex pattern for text validation (text/textarea only)"

[properties.response_config.properties.validation_message]
type = "string"
description = "Error message when validation fails"

[properties.response_config.properties.options.items]
type = "string"
```

## Participant Channel: Interactive Observations

The `Participant` channel enables procedures to request human input. These are **active observations** - the procedure engine requests an observation, and the participant provides it.

### How Participant Observations Work

1. **Procedure reaches Participant channel step**
2. **Engine emits `observation_requested` event** with the step's BeliefNode data
3. **Product's UI renderer** subscribes to these events:
   - Uses step `title` as prompt title
   - Uses step markdown text as prompt description
   - Uses `response_config` to render form element
4. **Participant responds** via UI
5. **ObservationEvent emitted** with response data
6. **Inference engine matches** observation to hint
7. **Procedure advances**, stores response in run context

### Participant Producers

Standard producers on the Participant channel:

- `Participant.Prompt` - Interactive prompts with form elements
- `Participant.Confirmation` - Simple acknowledgments (checkbox)
- `Participant.Measurement` - Manual measurements/readings
- `Participant.Note` - Freeform notes/annotations
- `Participant.Choice` - Decision points (select/radio)

### Response Configuration

For Participant channel observations, `response_config` specifies how to capture input:

**Form element types**:
- `text` / `textarea`: String input
- `number` / `slider`: Numeric input with optional min/max/step
- `checkbox`: Boolean (confirmation)
- `select` / `radio`: Single choice from options
- `multi_select`: Multiple choices from options

**Response value types** (polymorphic):
- `text`, `textarea`: String
- `number`, `slider`: Number
- `checkbox`: Boolean
- `select`, `radio`: String (selected option)
- `multi_select`: Array of strings

### Participant Channel Examples

#### Lab Protocol: Manual Temperature Entry

```toml
---
bid = "step_record_sample_temp"
title = "Record Sample Temperature"

[inference_hint]
channel = "Participant"
producer = "Measurement"

[inference_hint.response_config]
form_element = "number"
stores_in_variable = "sample_temp"
min = -20
max = 100
step = 0.1
---

Enter the current temperature of Sample A using the digital thermometer. This reading will be used to verify incubation conditions.
```

#### Manufacturing: Safety Confirmation

```toml
---
bid = "step_safety_check"
title = "Safety Equipment Confirmation"

[inference_hint]
channel = "Participant"
producer = "Confirmation"

[inference_hint.response_config]
form_element = "checkbox"
stores_in_variable = "safety_confirmed"
---

Confirm all required safety equipment is in place:
- Safety glasses
- Gloves
- Lab coat
- Closed-toe shoes
```

#### Deployment: Environment Selection

```toml
---
bid = "step_select_environment"
title = "Select Deployment Environment"

[inference_hint]
channel = "Participant"
producer = "Choice"

[inference_hint.response_config]
form_element = "radio"
stores_in_variable = "target_environment"
options = ["development", "staging", "production"]
---

Select the target environment for this deployment. Production deployments require additional approval.
```

#### Quality Control: Measurement with Validation

```toml
---
bid = "step_measure_diameter"
title = "Measure Component Diameter"

[inference_hint]
channel = "Participant"
producer = "Measurement"

[inference_hint.response_config]
form_element = "number"
stores_in_variable = "diameter_mm"
min = 24.5
max = 25.5
step = 0.01
validation_message = "Diameter must be within specification (25.0mm ±0.5mm)"
---

Use the digital caliper to measure the diameter of the machined component at its widest point. Specification requires 25.0mm ±0.5mm.
```

#### Contemplative Prompt (No Response)

```toml
---
bid = "step_review_work"
title = "Review Checklist"

[inference_hint]
channel = "Participant"
producer = "Confirmation"

[inference_hint.response_config]
form_element = "checkbox"
stores_in_variable = "review_complete"
---

Take a moment to review the completed checklist before continuing. Ensure all steps were performed correctly and all measurements are within specification.
```

## Event Schema

When an observable action is detected, the inference engine emits an `action_detected` event:

```toml schema:events.actionDetected
type = "object"
required = ["event_id", "timestamp", "source", "event_type", "payload"]

[properties.event_id]
type = "string"
format = "uuid"

[properties.timestamp]
type = "string"
format = "date-time"

[properties.source]
type = "string"
description = "Source of observation (inference, participant, sensor, system)"

[properties.event_type]
type = "string"
const = "action_detected"

[properties.payload]
type = "object"
required = ["node_bid", "confidence"]

[properties.payload.properties.node_bid]
type = "string"
description = "BeliefNode BID of the matched step"

[properties.payload.properties.confidence]
type = "number"
minimum = 0.0
maximum = 1.0
description = "Confidence score for the match"

[properties.payload.properties.duration_minutes]
type = "number"
description = "Duration of matched pattern"

[properties.payload.properties.captured_value]
description = "Captured value (for Participant channel responses, sensor readings, etc.)"

[properties.payload.properties.stored_in_variable]
type = "string"
description = "Variable name where value was stored (if applicable)"

[properties.payload.properties.supporting_data]
type = "array"
description = "Observation events that contributed to match"

[properties.payload.properties.metadata]
type = "object"
description = "Additional product-specific metadata"

[properties.payload.properties.supporting_data.items]
type = "object"
description = "Reference to ObservationEvent (product-specific schema)"
```

## Multi-Modal Observations

Complex steps can combine multiple observation sources using grouping events:

### Either Automatic OR Manual

```toml
---
bid = "step_verify_temperature"
title = "Verify Incubator Temperature"

[inference_hint]
operator = "any_of"

[[inference_hint.events]]
# Automatic sensor reading
channel = "Temperature"
producer = "Incubator_A"
to = ["37C"]
min_duration_minutes = 5

[[inference_hint.events]]
# Manual verification fallback
channel = "Participant"
producer = "Confirmation"

[inference_hint.events.response_config]
form_element = "checkbox"
stores_in_variable = "temp_verified_manual"
---

Confirm the incubator is maintaining 37°C ±0.5°C. System will auto-verify from sensor, or you can manually confirm if sensor is unavailable.
```

### Automatic AND Manual (Both Required)

```toml
---
bid = "step_dual_verification"
title = "Dual Verification Required"

[inference_hint]
operator = "all_of"

[[inference_hint.events]]
# System must detect correct state
channel = "SystemMetrics"
producer = "ConfigChecker"
to = ["production_config"]

[[inference_hint.events]]
# Operator must explicitly confirm
channel = "Participant"
producer = "Confirmation"

[inference_hint.events.response_config]
form_element = "checkbox"
stores_in_variable = "operator_confirmed"
---

Production deployment requires both automated config verification AND explicit operator confirmation. Confirm you have reviewed the deployment plan.
```

## Integration with Procedure Execution

### Lifecycle

1. **Template Loading**: Procedure engine loads BeliefNodes representing steps
2. **Pattern Registration**: Inference hints registered with observation pipeline
3. **Step Execution**: When step is reached, procedure state becomes "awaiting_observation"
4. **Observation Request**: For Participant channel, engine emits `observation_requested` event
5. **Event Matching**: Observation events matched against registered patterns
6. **Action Detection**: Inference engine emits `action_detected` events
7. **Variable Storage**: For responses with `stores_in_variable`, value stored in run context
8. **Procedure Advancement**: State machine advances to next step
9. **As-Run Recording**: Execution recorded with observed data

### State Machine Behavior

When a procedure reaches an observable action step:
- **Awaiting Observation**: Pattern registered, waiting for matching event
- **Observed**: Event received, confidence above threshold
- **Completed**: Action confirmed, procedure continues
- **Skipped**: Explicitly bypassed by executor

### Variable Scoping

Response variables are **run-scoped**: stored in the `ProcedureRun` context and accessible to subsequent steps:

```toml
# Step 1: Capture temperature
[inference_hint]
channel = "Participant"
producer = "Measurement"
[inference_hint.response_config]
stores_in_variable = "sample_temp"

# Step 2: Use temperature in conditional
[[procedure.steps]]
type = "if"
condition = "sample_temp < 0"
# ... handle cold sample
```

## Extension Points

Products implement observable actions by providing:

### 1. Observation Producers

Components that emit observation events:
```rust
trait ObservationProducer {
    fn channel(&self) -> &str;
    fn producer(&self) -> &str;
    fn start(&mut self);
    fn stop(&mut self);
}
```

Examples:
- Barcode scanner emitting scan events
- GPS emitting location transitions
- Thermometer emitting temperature readings
- System monitor emitting health checks
- UI framework emitting participant responses

### 2. Inference Engine

Component that matches observation streams to inference hints:
```rust
trait InferenceEngine {
    fn register_pattern(&mut self, node_bid: Bid, hint: InferenceHint);
    fn process_observation(&mut self, event: ObservationEvent);
    fn emit_action_detected(&self, detection: ActionDetection);
}
```

Responsibilities:
- Pattern matching (grouping/transition logic)
- Confidence scoring
- Semantic label resolution
- Event correlation

### 3. UI Renderer (for Participant Channel)

Component that displays prompts and captures responses:
```rust
trait ParticipantRenderer {
    fn render_observation_request(&self, step: &BeliefNode) -> Result<()>;
    fn collect_response(&self) -> Result<ObservationEvent>;
}
```

Uses BeliefNode fields:
- `title` → Prompt title
- Markdown text → Prompt description
- `inference_hint.response_config` → Form element configuration

### 4. Semantic Label Mapping

Products map semantic labels to concrete values:
```
"home" → GPS coordinates (37.7749, -122.4194)
"part_123" → Barcode value "0012345678905"
"37C" → Temperature range (36.5°C - 37.5°C)
```

## Examples Across Domains

### Manufacturing: Barcode Scan

```toml
---
bid = "step_scan_assembly_part"
title = "Scan Assembly Part"

[inference_hint]
channel = "Barcode"
producer = "Scanner"
to = ["part_assembly_123"]
---

Use the barcode scanner to identify the assembly part. Ensure the part number matches the work order.
```

### Lab Protocol: Automated Instrument Reading

```toml
---
bid = "step_spectrophotometer_reading"
title = "Spectrophotometer Reading"

[inference_hint]
channel = "Instrument"
producer = "Spectrophotometer_A"
min_duration_minutes = 1
confidence_threshold = 0.95
---

Place sample in spectrophotometer and wait for absorbance reading to stabilize. Reading will be automatically captured.
```

### Deployment: Service Health Check

```toml
---
bid = "step_service_health"
title = "Confirm Service Health"

[inference_hint]
operator = "all_of"
min_duration_minutes = 5

[[inference_hint.events]]
channel = "SystemMetrics"
producer = "HealthCheck"
to = ["healthy"]

[[inference_hint.events]]
channel = "SystemMetrics"
producer = "LoadAverage"
to = ["normal"]
---

Wait for all microservices to report healthy status with normal load for at least 5 minutes before proceeding with deployment.
```

## Boundary: General vs. Product-Specific

### General-Purpose (This Schema)

- `inference_hint` structure definition
- Grouping/transition event schemas
- Temporal and confidence constraints
- Participant channel conventions
- Response configuration schema
- `action_detected` event schema
- Concept of observable action steps

### Product-Specific (Downstream)

- Observation event producers (hardware/software integrations)
- ObservationEvent schema (product defines structure)
- Inference engine implementation (pattern matching algorithms)
- Confidence scoring formulas
- Semantic label resolution (mapping labels to concrete values)
- Channel/producer namespaces (product defines vocabulary)
- UI rendering for Participant channel (modal dialogs, forms, notifications)
- Delivery strategy (immediate, deferred, scheduled)
- Attention windows (psychological guardrails - product-specific extension)

## Design Rationale

### Why Unified Model?

Treating all observations (sensors, systems, participants) with the same schema:
1. **Conceptual simplicity**: One state machine model for all steps
2. **Consistent as-run recording**: All observations recorded the same way
3. **Natural multi-modal patterns**: Easy to combine automatic + manual verification
4. **Extension friendly**: Adding new observation channels is uniform

### Why Channel/Producer Namespace?

Two-level namespace enables:
- **Organization**: Group related observation types
- **Disambiguation**: Multiple GPS sources, multiple barcode scanners
- **Extension**: Products define custom channels without conflicts

### Why BeliefNodes for Steps?

Each step is a BeliefNode, which provides:
- Unique identity (`bid`)
- Human-readable name (`title`)
- Contextual explanation (markdown text)
- Type safety (schema validation)
- Graph relationships (part of lattice structure)

## Validation Rules

### Schema Validation

- `operator` must be one of: `any_of`, `all_of`, `none_of`
- `events` array must contain at least 1 item
- `from`/`to` arrays of strings (empty = wildcard)
- `confidence_threshold` must be 0.0-1.0
- Duration constraints must be positive numbers
- Time constraints must use valid formats

### Runtime Validation

- Observable action steps must have unique `bid`
- Referenced channels/producers must be registered
- Semantic labels must be resolvable
- Circular grouping references prohibited
- For Participant channel:
  - `response_config` required if capturing input
  - `form_element` must match response value type
  - Numeric values within `min`/`max` if specified
  - Selected options must be in `options` list

## Migration from Product Workspace

**Source**: 
- `noet/docs/design/action_interface.md` (observable actions)
- `noet/docs/design/prompt_interface.md` (participant prompts)

**Migrated**:
- `inference_hint` schema definition
- Grouping/transition event structures
- Temporal constraints
- Participant channel patterns (merged from prompt_interface.md)
- Response capture configuration
- `action_detected` event schema

**Stays in Product**:
- Action Inference Engine implementation
- ObservationEvent schema (product-specific)
- Semantic label mapping and resolution
- Confidence scoring algorithms
- Sensor-specific examples (GPS, screen activity)
- Attention window system (psychological guardrails, body budget, crisis override)
- Window selection policies and frequency management
- `opens_window` step type (bridge to attention windows)

## References

- **procedure_schema.md** - Core procedure schema
- **procedure_execution.md** - Execution lifecycle
- **redline_system.md** - As-run deviation tracking
- **beliefbase_architecture.md** - BeliefNode structure
- **ISSUE_17_NOET_PROCEDURES_EXTRACTION.md** - Implementation plan
- **ISSUE_18_EXTENDED_PROCEDURE_SCHEMAS.md** - Extended schemas

## Version History

- **v0.1** (2025-01-24): Initial schema definition, migrated from product workspace
- **v0.2** (2025-01-24): Unified model - merged participant prompts into observable actions, clarified BeliefNode structure
