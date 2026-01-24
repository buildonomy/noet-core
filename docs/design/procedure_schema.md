---
title: "Procedure Schema: Operational Definitions for Lattice Nodes"
authors: "Andrew Lyjak, Gemini 2.5 Pro, Claude Code"
last_updated: "2025-01-XX"
status: "Active"
version: "0.1"
dependencies: ["intention_lattice.md (v0.1)"]
---

# Procedure Schema

## 1. Purpose

This document specifies the schema for the `procedure` property, which can be registered with noet-core's schema registry to give lattice nodes operational definitions. A procedure connects abstract intentions to concrete steps, defining **how** something is done rather than just **what** it means.

**Key Design Principle**: This schema is **general-purpose**. It applies to any domain requiring procedural knowledge: manufacturing SOPs, lab protocols, cooking recipes, deployment runbooks, emergency response procedures, project workflows, etc.

**Implementation Note**: As of noet-core Issue 1 (Schema Registry), procedure schemas are **runtime-registered** via the `SCHEMAS.register()` API. They are not built into noet-core. Libraries like `noet-procedures` will register this schema at initialization.

## 2. Architectural Role

The `procedure` property provides a "downward-facing" view toward concrete execution, complementing the "upward-facing" view provided by `parent_connections` in the intention lattice.

- **Without a procedure**: A node is abstract, defined only through relationships
- **With a procedure**: A node is operational, with explicit execution steps

### 2.1 Generating Lattice Relationships

Procedure structure can generate edges in the lattice graph. The semantic meaning of these edges (constitutive, instrumental, expressive, exploratory) can be inferred from structural patterns:

**Inference Patterns:**

- **Constitutive**: Steps that use resources never released, steps that are part of recurring patterns
- **Instrumental**: Steps that produce outputs or release resources for subsequent use
- **Expressive**: Steps followed by reflective prompts, culminating actions
- **Exploratory**: Steps within `any_of` blocks, early steps with many conditional branches

**Note**: Exact inference rules are implementation-specific. Downstream libraries can implement different heuristics.

## 3. Top-Level Schema

The procedure schema defines:

```toml
[procedure]
metadata = { ... }      # Optional: tags, version, description
produces = { ... }      # Optional: expected outcome
variables = [ ... ]     # Optional: dynamic parameters
inventory = [ ... ]     # Optional: required resources
context = { ... }       # Optional: when this procedure applies
steps = [ ... ]         # Required: sequence of operations
```

### 3.1 Metadata

Optional descriptive information for documentation and discovery:

```toml
[procedure.metadata]
tags = ["cooking", "breakfast"]
version = 1.2
description = "Standard pancake recipe for 4 servings"
```

### 3.2 Produces

Optional specification of expected outcomes:

```toml
[procedure.produces]
type = "deliverable"
description = "Batch of 12 pancakes"
```

### 3.3 Variables

Declares dynamic parameters that can be set at runtime:

```toml
[[procedure.variables]]
name = "batch_size"
type = "number"
default = 12
description = "Number of units to produce"

[[procedure.variables]]
name = "quality_level"
type = "string"
default = "standard"
description = "Quality tier: 'draft', 'standard', 'precision'"
```

**Supported types**: `string`, `number`, `boolean`

### 3.4 Inventory

Resources required for execution. Can be:
- Simple item IDs: `inventory = ["mixing_bowl", "whisk"]`
- Quantified items: `{ item = "flour", quantity = 2, units = "cups" }`

**Item Lifecycle**:
- Items in `uses` blocks are consumed/occupied
- Items in `releases` blocks become available again
- Items in `produces` blocks are created

**Role Inference**:
- Constitutive items: Used but never released
- Instrumental items: Used and later released

### 3.5 Context

Optional conditions defining when this procedure applies:

```toml
[procedure.context]
time_of_day = ["08:00", "09:00"]  # Active during this window
day_of_week = ["monday", "friday"]  # Active on these days
```

**Use Cases**:
- Scheduled maintenance procedures (run monthly)
- Context-specific protocols (during emergency conditions)
- Time-sensitive workflows (business hours only)

## 4. Steps Schema

The heart of a procedure is its `steps` array, defining the work to be done.

### 4.1 Step Types

Each step must have a `type`:

#### `type: action`

Observable, real-world operation:

```toml
[[procedure.steps]]
type = "action"
id = "mix_batter"           # Optional: document-unique identifier
reference = "act_mix"       # Optional: reference to another procedure
uses = ["mixing_bowl", "flour", "eggs"]
produces = ["batter"]
releases = ["mixing_bowl"]
```

**Properties**:
- `id`: Optional identifier for reference by other steps
- `reference`: Link to another procedure or step to duplicate
- `uses`: Resources consumed/occupied
- `produces`: Resources created
- `releases`: Resources made available again

#### `type: prompt`

Interactive communication with executor:

```toml
[[procedure.steps]]
type = "prompt"
prompt_text = "Verify temperature is below 200°F"
response = { type = "boolean", stores_in_variable = "temp_ok" }
```

**Use Cases**:
- Quality checkpoints ("Does it look right?")
- Data collection ("How many units?")
- Decision points ("Continue or abort?")

#### `type: opens_window`

Programmatic trigger for external subsystem (implementation-defined):

```toml
[[procedure.steps]]
type = "opens_window"
window_id = "safety_checklist"
```

**Use Cases**:
- Trigger inspection forms
- Launch sub-applications
- Activate specialized interfaces

### 4.2 Logical Operators

Steps can be nested within logical blocks:

```toml
[[procedure.steps]]
type = "sequence"        # Execute in order
steps = [ ... ]

[[procedure.steps]]
type = "parallel"        # Can execute simultaneously
steps = [ ... ]

[[procedure.steps]]
type = "all_of"         # All required, any order
steps = [ ... ]

[[procedure.steps]]
type = "any_of"         # At least one required
selection_variable = "path_choice"
steps = [ ... ]

[[procedure.steps]]
type = "avoid"          # Counter-productive patterns
steps = [ ... ]
```

**Default**: If no operator specified for a list, assume `parallel`

### 4.3 Step References

Steps can reference other procedures or steps for reusability:

```toml
# Simple reference (duplicates entire procedure)
[[procedure.steps]]
reference = "act_preheat_oven"

# Reference with variable passing (like function call with args)
[[procedure.steps]]
reference = "act_brew_beverage"
[procedure.steps.uses]
beverage_type = "coffee"
brew_strength = "{{user_preference}}"  # Pass from parent variable
```

**Use Cases**:
- Reusable sub-procedures
- Parameterized templates
- Modular procedure composition

## 5. Resource Management

### 5.1 Items vs. Relationships

**CRITICAL**: The inventory system is for **material resources only** (objects, tools, consumables, spaces). Never use it for people or relationships.

**Why**: Human relationships require mutual co-creation and participatory knowing, not consumption/release cycles.

**Wrong**:
```toml
inventory = ["colleague", "mixing_bowl"]  # ❌ NEVER
```

**Right**:
```toml
inventory = ["meeting_room", "laptop"]    # ✅ Material resources only
# Human relationships belong at aspiration level, not inventory
```

### 5.2 Tension Management

Items can accumulate "tension" over time (wear, debt, state changes):

```toml
# In item definition file
tensions = [
  { name = "usage_debt", type = "usage_based", threshold = 100 },
  { name = "cleanliness", type = "state_based", states = ["clean", "dirty"] }
]
```

Steps can resolve tensions:

```toml
[[procedure.steps]]
type = "action"
id = "clean_oven"
[[procedure.steps.resolves_tension]]
item = "oven"
tension = "cleanliness"
to_state = "clean"
```

**Use Cases**:
- Maintenance procedures (usage-based triggers)
- Quality control (state-based checks)
- Resource lifecycle tracking

## 6. Dynamic Procedures

Variables enable runtime flexibility:

```toml
[procedure]
variables = [
  { name = "mode", type = "string", default = "standard" }
]

# Prompt populates variable
[[procedure.steps]]
type = "prompt"
prompt_text = "Select mode"
response = { type = "choice", options = ["draft", "standard"], stores_in_variable = "mode" }

# Variable controls path
[[procedure.steps]]
type = "any_of"
selection_variable = "mode"
[[procedure.steps.steps]]
id = "draft"           # Matches variable value
reference = "act_draft_mode"
[[procedure.steps.steps]]
id = "standard"
reference = "act_standard_mode"
```

## 7. Example: Manufacturing SOP

```toml
id = "sop_widget_assembly"
title = "Widget Assembly Procedure"

[procedure]
variables = [
  { name = "batch_id", type = "string" },
  { name = "inspector", type = "string" }
]

inventory = [
  { item = "widget_base", quantity = 1 },
  { item = "fasteners", quantity = 4 }
]

[[procedure.steps]]
type = "sequence"

[[procedure.steps.steps]]
type = "prompt"
prompt_text = "Enter batch ID"
response = { type = "string", stores_in_variable = "batch_id" }

[[procedure.steps.steps]]
type = "action"
id = "attach_fasteners"
uses = ["widget_base", "fasteners", "torque_wrench"]
releases = ["torque_wrench"]

[[procedure.steps.steps]]
type = "prompt"
prompt_text = "Verify torque: 15-20 ft-lbs"
response = { type = "boolean" }

[[procedure.steps.steps]]
type = "action"
id = "quality_check"
produces = ["completed_widget"]
releases = ["widget_base"]
```

## 8. Integration Points

This schema is **data only**. Runtime execution requires:

1. **Execution Engine**: Tracks procedure runs, step progress, timing
2. **Event Log**: Records as-run history (what actually happened)
3. **Redline System**: Compares template to reality, records deviations

See `procedure_execution.md` and `redline_system.md` for runtime architecture.

## 9. Implementation Checklist

For libraries implementing this schema:

- [ ] Register schema with noet-core via `SCHEMAS.register()`
- [ ] Parse TOML into internal representation
- [ ] Validate step references resolve correctly
- [ ] Handle variable substitution
- [ ] Support nested logical operators
- [ ] Provide runtime execution tracking
- [ ] Record as-run history

## 10. Design Principles

- **General-purpose**: Applicable to any procedural domain
- **Declarative**: Define structure, not execution strategy
- **Composable**: Steps reference other procedures
- **Dynamic**: Variables enable runtime parameterization
- **Traceable**: Structure enables as-run comparison
- **Extensible**: Downstream libraries can add custom fields

---

**Status**: This design defines the schema specification. Runtime execution is out of scope for this document.