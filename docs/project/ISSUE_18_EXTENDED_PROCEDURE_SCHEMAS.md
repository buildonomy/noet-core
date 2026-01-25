# Issue 18: Extended Procedure Schemas (Participant Channel)

**Priority**: MEDIUM  
**Estimated Effort**: 1 week  
**Dependencies**: Issue 1 (Schema Registry), Issue 17 (noet-procedures extraction)  
**Blocks**: None (enables richer procedural features)

## Summary

Extend the noet-procedures crate to support **Participant channel observations** - treating operator input as just another observation source in the unified observable action model. This eliminates the need for a separate "prompt" step type by recognizing that participant responses are observations on the `Participant` channel, using the same `inference_hint` mechanism as sensor readings, barcode scans, and system metrics.

## Goals

1. Define `Participant` channel conventions within observable action schema
2. Extend `inference_hint` with `response_config` for input capture
3. Leverage BeliefNode `title` and markdown text for prompt content
4. Provide unified event model (`action_detected` for all observations)
5. Keep schemas general-purpose (applicable beyond behavior change)

## Architecture

### Unified Observable Model

Every procedure step advances via observation events, whether from:
- **Sensors**: Temperature probes, accelerometers
- **Systems**: Health checks, configuration state
- **Participants**: Measurements, confirmations, choices

All use the same pattern: `inference_hint` defines what to observe, `channel` + `producer` identify the source.

```
┌─────────────────────────────────────────────────┐
│ BeliefNode (Procedure Step)                     │
│ • bid: unique identifier                        │
│ • title: "Record Sample Temperature"            │
│ • text: "Enter the current temperature..."      │
│ • inference_hint.channel = "Participant"        │
│ • inference_hint.response_config = {...}        │
└────────────────┬────────────────────────────────┘
                 │
                 ▼
┌─────────────────────────────────────────────────┐
│ Procedure Engine                                │
│ • Reaches step with Participant channel         │
│ • Emits observation_requested event             │
└────────────────┬────────────────────────────────┘
                 │
                 ▼
┌─────────────────────────────────────────────────┐
│ Product's UI Renderer (subscribes to requests)  │
│ • Reads BeliefNode title → Prompt title         │
│ • Reads BeliefNode text → Prompt description    │
│ • Reads response_config → Renders form element  │
└────────────────┬────────────────────────────────┘
                 │
                 ▼
┌─────────────────────────────────────────────────┐
│ Participant responds                            │
│ • ObservationEvent emitted                      │
│ • Inference engine matches to hint              │
│ • action_detected event (same as all channels)  │
└────────────────┬────────────────────────────────┘
                 │
                 ▼
┌─────────────────────────────────────────────────┐
│ Procedure advances, stores value in run context │
└─────────────────────────────────────────────────┘
```

### Key Insight: Prompts ARE Observations

A "prompt" is not a separate step type - it's an observable action where:
- `channel = "Participant"`
- `producer = "Prompt"` (or "Measurement", "Confirmation", etc.)
- `response_config` defines how to capture the observation
- BeliefNode `title` and markdown text provide context

This unifies the conceptual model: all steps wait for observation events.

## Implementation Steps

### Phase 1: Schema Extension (3 days)

1. **Extend `transitionEvent` Schema** (1 day)
   - [ ] Add `response_config` property (optional, for Participant channel)
   - [ ] Define response configuration schema (form elements, validation)
   - [ ] Update `action_observable_schema.md` with Participant channel section
   - [ ] Document that BeliefNode title/text provide prompt content

2. **Define Participant Channel Conventions** (1 day)
   - [ ] Document standard producers: `Prompt`, `Measurement`, `Confirmation`, `Choice`, `Note`
   - [ ] Define form element types: text, number, checkbox, select, slider, etc.
   - [ ] Specify response value polymorphism (string, number, boolean, array)
   - [ ] Document validation constraints (min/max, pattern, required)

3. **Update Event Schema** (1 day)
   - [ ] Extend `action_detected` event to include captured values
   - [ ] Add `stored_in_variable` field for run context storage
   - [ ] Document that Participant channel uses same event as all channels
   - [ ] Specify confidence scoring for participant responses (validation-based)

### Phase 2: Integration with Procedure Execution (2 days)

4. **Observation Request Mechanism** (1 day)
   - [ ] Define `observation_requested` event schema
   - [ ] Procedure engine emits request when reaching Participant channel step
   - [ ] Include full BeliefNode data in request (title, text, response_config)
   - [ ] Document lifecycle: request → render → respond → detect

5. **Variable Storage** (1 day)
   - [ ] Store responses in run context under `stores_in_variable` name
   - [ ] Enable subsequent steps to reference stored values
   - [ ] Document variable scoping (run-level)
   - [ ] Provide access API for conditional logic

### Phase 3: Documentation & Examples (2 days)

6. **Design Documentation** (1 day)
   - [ ] Complete Participant channel section in `action_observable_schema.md`
   - [ ] Document relationship to BeliefNode structure
   - [ ] Mark boundaries: schema (general) vs UI rendering (product-specific)
   - [ ] Explain why prompts are observations

7. **Tutorial Examples** (1 day)
   - [ ] Lab protocol: Manual temperature entry
   - [ ] Manufacturing: Safety confirmation
   - [ ] Deployment: Environment selection
   - [ ] Quality control: Measurement with validation
   - [ ] Multi-modal: Either automatic sensor OR manual entry

## Schema Additions

### Response Configuration Schema

Added to `transitionEvent` as optional property:

```toml
[properties.response_config]
type = "object"
description = "Response capture configuration (for Participant channel)"

[properties.response_config.properties.form_element]
type = "string"
enum = ["text", "textarea", "number", "select", "multi_select", "checkbox", "radio", "slider"]

[properties.response_config.properties.stores_in_variable]
type = "string"
description = "Variable name to store response in run context"

[properties.response_config.properties.options]
type = "array"
description = "Options for select/multi_select/radio"

[properties.response_config.properties.min]
type = "number"

[properties.response_config.properties.max]
type = "number"

[properties.response_config.properties.step]
type = "number"

[properties.response_config.properties.required]
type = "boolean"
default = true

[properties.response_config.properties.validation_pattern]
type = "string"

[properties.response_config.properties.validation_message]
type = "string"
```

## Example: Lab Protocol

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

Enter the current temperature of Sample A using the digital thermometer. This reading will be used to verify incubation conditions are maintained.
```

When the procedure reaches this step:
1. Engine emits `observation_requested` with BeliefNode data
2. UI renderer displays:
   - Title: "Record Sample Temperature"
   - Text: "Enter the current temperature of Sample A..."
   - Form: Number input, -20 to 100, step 0.1
3. Participant enters: 22.5
4. ObservationEvent emitted with value
5. Inference engine matches, emits `action_detected`
6. Procedure advances, stores 22.5 in run context as `sample_temp`

## Testing Requirements

### Schema Tests
- Parse `inference_hint` with `response_config`
- Validate form element types
- Validate response value types match form elements

### Integration Tests
- Execute procedure with Participant channel step
- Emit observation event with response
- Verify value stored in run context
- Verify procedure advancement
- Test validation (min/max, pattern, required)

### Multi-Modal Tests
- Either automatic sensor OR manual entry (any_of)
- Both sensor AND manual confirmation (all_of)
- Verify correct event matching

## Success Criteria

- [ ] Participant channel documented in `action_observable_schema.md`
- [ ] `response_config` schema defined
- [ ] BeliefNode title/text usage documented
- [ ] Unified event model (all channels use `action_detected`)
- [ ] Variable storage in run context working
- [ ] Multi-modal examples demonstrate flexibility
- [ ] Non-behavior-change examples across domains
- [ ] Documentation complete
- [ ] Tests pass

## Boundary: General vs. Product-Specific

### General-Purpose (noet-procedures)

**Observable Actions (all channels)**:
- `inference_hint` schema structure
- Participant channel conventions
- Response configuration schema
- `action_detected` event schema
- Variable storage mechanism

**What's NOT included**:
- No `prompt` step type (it's just `action` with `channel = "Participant"`)
- BeliefNode structure used as-is (title, text already exist)

### Product-Specific (Downstream)

**UI Rendering**:
- Modal dialogs, forms, notifications
- Platform-specific controls (iOS, Android, web)
- Styling and branding

**Delivery Strategy**:
- Timing (immediate, deferred, scheduled)
- Frequency management
- Attention windows (psychological guardrails)
- Body budget integration
- Crisis override logic

**Observation Producers**:
- Sensor integrations (GPS, temperature, barcode)
- System monitors (health checks, metrics)
- UI frameworks (form submission handlers)

## Design Rationale

### Why Unified Model?

1. **Conceptual simplicity**: One state machine for all observations
2. **Consistent as-run recording**: All observations recorded the same way
3. **Natural multi-modal patterns**: Easy to combine automatic + manual
4. **Extension friendly**: Adding observation channels is uniform

### Why Reuse BeliefNode Fields?

BeliefNodes already have:
- `title`: Human-readable name
- Markdown text: Explanation/context

No need to add `prompt_text` or `prompt_title` - they're redundant. The UI renderer just reads the BeliefNode.

### Why Not Separate `prompt` Step Type?

Prompts are observations. Treating them differently:
- Creates conceptual split (observable vs. interactive)
- Duplicates event handling logic
- Makes multi-modal patterns awkward
- Adds unnecessary schema complexity

Unified model is simpler and more powerful.

## Use Cases (Non-Behavior-Change Examples)

### Manufacturing SOP
```toml
[inference_hint]
channel = "Participant"
producer = "Confirmation"
[inference_hint.response_config]
form_element = "checkbox"
stores_in_variable = "safety_confirmed"
```

### Lab Protocol
```toml
[inference_hint]
channel = "Participant"
producer = "Measurement"
[inference_hint.response_config]
form_element = "number"
stores_in_variable = "ph_level"
min = 0
max = 14
step = 0.1
```

### Deployment Runbook
```toml
[inference_hint]
channel = "Participant"
producer = "Choice"
[inference_hint.response_config]
form_element = "radio"
stores_in_variable = "rollback_strategy"
options = ["immediate", "gradual", "manual"]
```

### Quality Control
```toml
[inference_hint]
operator = "any_of"
[[inference_hint.events]]
# Automatic measurement
channel = "Instrument"
producer = "Caliper"

[[inference_hint.events]]
# Manual measurement fallback
channel = "Participant"
producer = "Measurement"
[inference_hint.events.response_config]
form_element = "number"
stores_in_variable = "diameter_mm"
```

## Risks

**Risk**: Confusion about BeliefNode structure  
**Mitigation**: Document clearly that steps are BeliefNodes, fields already exist

**Risk**: UI rendering expectations unclear  
**Mitigation**: Provide detailed sequence diagram, reference BeliefNode fields explicitly

**Risk**: Response value type mismatches  
**Mitigation**: Runtime validation, clear documentation of form_element → type mapping

## References

- **action_observable_schema.md** - Unified observable action schema (includes Participant channel)
- **procedure_execution.md** - Execution lifecycle
- **beliefbase_architecture.md** - BeliefNode structure
- **ISSUE_01_SCHEMA_REGISTRY.md** - Foundation
- **ISSUE_17_NOET_PROCEDURES_EXTRACTION.md** - Companion issue

## Next Steps After Completion

1. Products implement UI renderers for Participant channel
2. Renderer subscribes to `observation_requested` events
3. Reads BeliefNode title/text for prompt content
4. Renders form element based on `response_config`
5. Emits `ObservationEvent` on response
6. Procedure advances via standard `action_detected` flow

## Notes

**Key simplification**: By recognizing that prompts are observations, we:
- Eliminate separate `prompt` step type
- Reuse BeliefNode title/text (no `prompt_text` property needed)
- Unify event model (all channels use `action_detected`)
- Enable natural multi-modal patterns (automatic OR manual)

This is a significant architectural insight that simplifies the model considerably.
