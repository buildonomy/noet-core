---
title = "Noet Procedure Schema: Bridging Intentions to Actions"
authors = "Andrew Lyjak, Gemini 2.5 Pro, Claude Code"
last_updated = "2025-10-28"
status = "Active"
version = "0.1"
dependencies = [ "intention_lattice.md (v0.1)", "procedure_engine.md (v0.1)", "action_interface.md (v0.1)", "prompt_interface.md (v0.1)", "action_modality_framework.md (v0.1)", "redline_system.md (v0.1)" ]
---
# Noet Procedure Schema

## 1. Purpose

This document specifies the master schema for the `procedure` property, which can be added to any Intention Lattice node to give it a concrete, operational definition. It defines the structure that connects abstract intentions to observable actions and participant interactions.

This document serves as a central hub, defining the overall structure of a procedure while relying on other specialized design documents for the definitive schemas of its components. It describes *how* the components fit together, while the other documents define *what* those components are.

## 2. The Role of the `procedure` Property

The `procedure` property gives any node a "downward-facing" view towards the infimum (the ground of action), just as `parent_connections` give it an "upward-facing" view towards the supremum (the horizon of intention).

-   **Without a `procedure` block**, a node is considered **abstract**. It is only operationalized through upwards relationships (the `parent_connections` property of intention nodes.
-   **With a `procedure` block**, a node is considered **operational**. Its objective shape(s) in the world can be partially constituted by the pattern of steps it contains.

### 2.1. Generating Lattice Relationships from Procedures

A `procedure` block is a blueprint for generating the intention lattice graph. It defines the parent-child relationships between the current node (the parent) and the action nodes referenced within its `steps` (the children). The system uses the following algorithm to determine the `relationship_profile` (type and intensity) for each of these connections.

#### 2.1.1. Relationship Generation from Procedures

**ARCHITECTURAL NOTE (2025-11-19):** Step magnitude is NOT stored in the lattice. It's computed at runtime by **ProcedureStructureOracle** (see design_decisions_consolidated.md section 7.7) for visualization and attention allocation purposes only.

The system generates Pragmatic edges from procedure parent nodes to the action nodes referenced in procedure steps. This algorithm runs during **schema parsing** (implemented in `codec/schema_parsers.rs`) when converting TOML → BeliefNode graph.

**Edge Generation:**

1. **Create edges** between procedure parent node → each action step referenced
2. **Infer semantic kinds** for each edge based on structural patterns (populates `EnumSet<PragmaticKind>` in edge payload)
3. **No edge weights are stored** - the lattice contains only boolean semantic flags

#### 2.1.2. Semantic Kind Inference Algorithm

**Note on Legacy Terminology:** This document historically used relationship type names like "constitutive", "instrumental", "expressive" as **relationship types**. As of the 2025-11-19 Advisory Council session, these are now **semantic kinds** (boolean flags) stored as `EnumSet<PragmaticKind>` in the Pragmatic edge payload. The core inference logic remains the same.

---

##### For steps with `type: action`

The system infers which semantic kinds to include in the edge's `EnumSet<PragmaticKind>` payload:

**Constitutive Inference:**
- Action uses resources (including "self") that are NEVER released across entire procedure
- Action produces no instrumental outputs (no `produces`, no `releases` blocks)
- Action is referenced by nodes with explicit `embodies` archetype property
- Action is part of recurring high-frequency pattern (detected from temporal context)

**Instrumental Inference:**
- Action has `produces` block (creates tangible output)
- Action has `releases` block (makes resources available to subsequent steps)
- Action is within `sequence` where output flows to next step
- Action is deeply nested (more than 2 levels) within procedure structure

**Expressive Inference:**
- Action is immediately followed by step with `type: prompt` that has `expresses` property
- Action is final/culminating step in `sequence` block
- Action references archetype nodes
- Action is within procedure that includes `opens_window` steps

**Exploratory Inference:**
- Action is within `any_of` block (multiple paths under exploration)
- Action is within `parallel` block (experimental/uncertain ordering)
- Action is early in procedure with many subsequent conditional decision points
- Procedure has high variable count (many unknowns being resolved)

**Note on Tensions and Trade-offs:**

`tensions_with` and `trades_off` are **NOT** inferred from procedure structure. Per Advisory Council Agreement A1, these are **computed by the Action Modality Framework** from somatic channel overlap analysis. See `action_modality_framework.md` for details on how channel requirements generate resource conflicts.

The `avoid` block in procedures serves a different purpose: it documents counter-productive patterns for the Redline System to detect and surface for reflection, but does not directly generate `tensions_with` edges.

**Default:** If no indicators match, assume `Instrumental` only.

**Example Inference:**

```toml
[[steps]]
type = "sequence"
[[steps.steps]]
  id = "sit_in_meditation"
  uses = ["self", "meditation_cushion"]
  releases = ["meditation_cushion"]  # But NOT "self"
[[steps.steps]]
  type = "prompt"
  prompt_text = "What quality of mind emerged?"
  expresses = "asp_awareness"
```

**Inferred semantic kinds for "sit_in_meditation" edge:**
- `Constitutive` ✓ (uses "self" never released)
- `Instrumental` ✓ (releases cushion for subsequent use)
- `Expressive` ✓ (followed by reflective prompt)
- `Exploratory` ✗ (not in any_of, established pattern)

**Result:** `EnumSet<PragmaticKind> = [Constitutive, Instrumental, Expressive]`

**Implementation Note:** The detailed inference heuristics are documented in `design_decisions_consolidated.md` section 2a. The implementation lives in `codec/schema_parsers.rs` as part of the TOML → BeliefNetwork compilation process.

---

##### For steps with `type: opens_window`

1.  The system performs a graph descent from the `window_id` referenced in the step to find all accessible `prompt` nodes within that Attention Window.
2.  For each discovered prompt, a Pragmatic edge is created between the prompt node and the procedure's parent node.
3.  Semantic kind: `EnumSet<PragmaticKind>` contains only `Expressive` (prompts express values/intentions)

---

##### Explicit Override of Inferred Semantics

Users can manually specify semantic kinds in `parent_connections` which **override** the inference:

```toml
[[parent_connections]]
parent_id = "health_aspiration"
weight_kind = "Pragmatic"

[relationship_semantics]
constitutive = true      # Agrees with inference
exploratory = true       # USER OVERRIDE - adds meaning inference missed
instrumental = false     # USER OVERRIDE - removes inferred flag
```

When explicit `relationship_semantics` exist, they take precedence over procedure-inferred semantics for that specific parent connection.



## 3. Top-Level Schema

```toml schema:procedures.procedure
# This is the master schema for the `procedure` property.
type = "object"
[properties]
  [properties.metadata]
    type = "object"
    [properties.metadata.properties]
      [properties.metadata.properties.tags]
        type = "array"
        [properties.metadata.properties.tags.items]
          type = "string"
      [properties.metadata.properties.version]
        type = "number"
    description = "Optional metadata for search, discovery, and sharing."
  [properties.produces]
    type = "object"
    [properties.produces.properties]
      [properties.produces.properties.type]
        type = "string"
      [properties.produces.properties.description]
        type = "string"
    description = "Optional definition of the expected outcome of the procedure."
  [properties.variables]
    type = "array"
    description = "Declares variables that can be populated by prompts and used for dynamic control."
    [properties.variables.items]
      type = "object"
      [properties.variables.items.properties]
        [properties.variables.items.properties.name]
          type = "string"
        [properties.variables.items.properties.type]
          type = "string"
          enum = ["string", "number", "boolean"]
        [properties.variables.items.properties.description]
          type = "string"
        [properties.variables.items.properties.default]
          oneOf = [
            { type = "string" },
            { type = "number" },
            { type = "boolean" },
          ]
      required = ["name", "type"]
  [properties.inventory]
    type = "array"
    description = "Defines the resources expected to be used in the procedure."
    [properties.inventory.items]
      oneOf = [
        { type = "string" }, # Simple item ID
        { "$ref" = "#/$defs/quantifiedItem" },
      ]
  [properties.context]
    type = "object"
    description = "Defines the conditions under which this procedure is active."
    [properties.context.properties]
      [properties.context.properties.time_of_day]
        type = "array"
        [properties.context.properties.time_of_day.items]
          type = "string"
          pattern = "^([0-1]?[0-9]|2[0-3]):[0-5][0-9]$"
      [properties.context.properties.day_of_week]
        type = "array"
        [properties.context.properties.day_of_week.items]
          type = "string"
          enum = ["monday", "tuesday", "wednesday", "thursday", "friday", "saturday", "sunday"]
  [properties.steps]
    type = "array"
    description = "The sequence of steps that constitute the procedure."
    [properties.steps.items]
      "$ref" = "#/$defs/procedureStep"
required = ["steps"]
```

## 4. Inventory and Item Management

### 4.0. Critical Distinction: Items vs. Persons

**IMPORTANT:** The `inventory` system is designed exclusively for **material resources** (objects, tools, consumables, spaces). It must **never** be used to model people or human relationships.

**Why this matters:**
- The inventory lifecycle (`uses`, `releases`, `produces`) assumes **unidirectional consumption**, which is appropriate for objects but fundamentally incompatible with human relationships
- Treating persons as "inventory items" violates their inherent dignity and reduces them to means rather than ends
- Human relationships are characterized by **mutual co-creation** and **participatory knowing**—they cannot be "consumed" or "released" without objectification

**What NOT to do:**
```toml
# ❌ WRONG - Never do this
inventory = ["spouse", "flour", "car"]
[[steps]]
type = "action"
uses = ["spouse", "mixing_bowl"]
```

**What to do instead:**
- Human relationships should be modeled at the **aspiration level** of the intention lattice (see [intention_lattice.md](./intention_lattice.md))
- Procedures that involve other people should reference the **relationship aspiration** they serve, not list people as dependencies
- Use natural language like `with: spouse` in documentation, but avoid treating persons as procedure properties

**Example of proper approach:**
```toml
# ✅ CORRECT - Procedure serves a relationship
id = "goal_date_night"
title = "Evening Connection with Spouse"

[procedure]
# Material resources only
inventory = ["restaurant_reservation", "car"]

# Reference the relationship aspiration this procedure serves
serves = "asp_communion_connection_spouse"

[[procedure.steps]]
type = "action"
id = "share_dinner"
# Actions that involve others should express relational intentions
expresses = "asp_communion_connection_spouse"
```

For the complete rationale and alternative approaches to modeling relational practices, see the Advisory Council session report at `docs/council/sessions/2025-10-29_relationship_language/`.

---

The procedure schema treats resource management as a primary concern, modeling how a participant uses and maintains **material items** over time.

### 4.1. Optional Inventory and Quantity Tracking

A procedure's `inventory` block is **optional**. If it is omitted, the system will infer the required inventory by aggregating all unique items listed in the `uses` blocks of its steps. Providing an explicit `inventory` can be useful for clarity or to define items that may not be used in all possible branches of a complex procedure.

To track amounts, items can be defined as simple IDs or as objects with quantities and units.

```toml schema:procedures.quantifiedItem
# Schema for an item with a specified quantity and unit.
type = "object"
[properties]
  [properties.item]
    type = "string"
  [properties.quantity]
    oneOf = [
      { type = "number" },
      { type = "string" }, # To allow for dynamic expressions like "{{num_guests}}"
    ]
  [properties.units]
    type = "string"
required = ["item", "quantity"]
```

### 4.2. Inferred Item Roles

The procedure schema treats resource management as a primary concern, modeling how a participant uses and maintains items over time.

A procedure's `inventory` is a simple list of item IDs. The system determines an item's role—`constitutive` (consumed) or `instrumental` (reused)—based on its lifecycle within the `steps` block.

-   An item that appears in a `uses` list but **never** in a `releases` list is considered **constitutive**.
-   An item that appears in both a `uses` list and a later `releases` list is considered **instrumental**.

This behavioral definition simplifies the schema and removes redundancy.

### 4.3. Item Definition Files and Tension Management

While a procedure defines how items are used *in context*, the core properties of an item are defined in centralized **Item Definition Files** (e.g., in a `/participant/inventory/` directory). These files contain metadata about an item, including different forms of "tension" it can accumulate over time.

The `procedure_engine` is responsible for tracking this tension across multiple procedures.

An item can define a `tensions` block, which is a list of tension models.

**Example Item Definition (`/participant/inventory/oven.md`):**
```toml
# Defines the types of tension the oven can accumulate.
tensions = [
  { name = "usage_debt", type = "usage_based", threshold = 100, description = "Accumulated wear from use, reset by maintenance." },
  { name = "cleanliness_debt", type = "state_based", states = ["clean", "dirty"], default = "clean", description = "Grime buildup from cooking." }
]
```

When a procedure `uses` an item, the engine updates its tension state. When a `threshold` is reached or a `state` changes, this can trigger prompts or contextual suggestions. Actions can then be used to resolve this tension (see `resolves_tension` below), creating a feedback loop for long-term sustainability.

```toml schema:procedures.tensionResolution
# Schema for the 'resolves_tension' property within an action step.
type = "object"
[properties]
  [properties.item]
    type = "string"
    description = "The ID of the item whose tension is being resolved. Can use 'self' for the participant."
  [properties.tension]
    type = "string"
    description = "The name of the tension being resolved, from the item's definition file."
  [properties.to_state]
    type = "string"
    description = "For state_based tensions, the new state after resolution."
  [properties.description]
    type = "string"
    description = "A description of how the tension is resolved."
required = ["item", "tension"]
```

## 5. The `steps` Block Schema

The `steps` block is the core of the procedure, defining the work to be done. It is a list of step objects that can be nested within logical operators to create complex workflows.

Each step can have a document-unique `id` to allow its execution logic to be referenced or duplicated by other steps. These `id` are part of the relative id structure defined in `intention_lattice.md`, subsection *Identifier and Referencing Strategy*. To reference particular steps between lattice nodes, concatenate the node ID with the step ID, e.g. `act_take_a_walk.detect_walk`.

If a step does not have an explicitly defined ID, it receives an enumerated ID based on it's location within the surrounding block.

```toml schema:procedures.procedureStep
# A single step in a procedure. It can be a reference, a logical operator, or a specific action/prompt.
oneOf = [
  { type = "string" }, # A simple string is a reference to another node's procedure or a step ID.
  { "$ref" = "#/$defs/logicalOperatorBlock" },
  { "$ref" = "#/$defs/actionStep" },
  { "$ref" = "#/$defs/promptStep" },
  { "$ref" = "#/$defs/opensWindowStep" },
]
```

### 5.1. Logical Operators

To structure complex procedures, steps can be nested within a logical operator. If no operator is specified for a list of steps, it is treated as a `parallel` by default.

-   **`type: sequence`**: A list of steps that must be performed in order.
-   **`type: parallel`**: A list of steps that can be performed simultaneously.
-   **`type: all_of`**: A list of steps that must *all* be completed, but in *any order*.
-   **`type: any_of`**: A list of steps where *at least one* must be completed. Can be paired with a `selection_variable` to allow a prompt to determine the path.
-   **`type: avoid`**: A list of steps that are counter-productive or in tension with the goal. The execution of any step in this list actively works against the parent's intention.

```toml schema:procedures.logicalOperatorBlock
# Defines a block that groups other steps with a logical operator.
type = "object"
[properties]
  [properties.type]
    type = "string"
    enum = ["sequence", "parallel", "all_of", "any_of", "avoid"]
  [properties.steps]
    type = "array"
    [properties.steps.items]
      "$ref" = "#/$defs/procedureStep"
  [properties.selection_variable]
    type = "string"
    description = "For 'any_of', the variable that determines which path to take."
required = ["type", "steps"]
```

### 5.2. Step Types

Each object in a `steps` list must have a `type`.

#### `type: action`

This step represents an observable, real-world action. It is the primary way procedures interact with the world and manage resources.

Actions are the default step type. When a procedure block includes a step that is just a string, it is interpreted as an action reference. See the reference section below for an example.

An action can have the following properties related to item management:
-   `uses`: A list of items that are required for the action to initiate.
-   `produces`: A list of items that are created once the action is complete.
-   `releases`: A list of instrumental items that are available for other actions once this action is complete.
-   `resolves_tension`: A list of tensions on items that are resolved once this action is complete.

In addition, an action step includes either a `reference` or a `inference_hint` property. The `inference_hint` connects the procedure into the `action_inference_engine.md` through the `action_interface.md`. The reference block is explained below.

```toml schema:procedures.actionStep
type = "object"
[properties]
  [properties.type]
    type = "string"
    const = "action"
  [properties.id]
    type = "string"
    pattern = "^[a-zA-Z0-9_-]+$"
    description = "A document-unique ID for the step."
  [properties.reference]
    type = "string"
    description = "A reference to another node's procedure or a step ID within the same procedure."
  [properties.uses]
    type = "array"
    [properties.uses.items]
      oneOf = [
        { type = "string" },
        { "$ref" = "#/$defs/quantifiedItem" },
      ]
  [properties.produces]
    type = "array"
    [properties.produces.items]
      oneOf = [
        { type = "string" },
        { "$ref" = "#/$defs/quantifiedItem" },
      ]
  [properties.releases]
    type = "array"
    [properties.releases.items]
      type = "string"
  [properties.resolves_tension]
    type = "array"
    [properties.resolves_tension.items]
      "$ref" = "#/$defs/tensionResolution"
  [properties.inference_hint]
    # The full schema for 'inference_hint' is defined in 'action_interface.md'.
    "$ref" = "action_interface.md#/$defs/inferenceHint"
  [properties.sentence]
    # The full schema for 'sentence' is defined in 'action_modality_framework.md'.
    "$ref" = "action_modality_framework.md#/$defs/sentence"
required = ["type"]
```

**Example Action Steps:**
```toml
[[steps]]
type = "action"
id = "mix_batter"
reference = "act_mix_ingredients"
```

**Action References**

An action can be defined by reference using the `reference` property. When a reference is invoked, the step's contents duplicate the entire procedure of the referenced node (`reference = "/path/to/node.md"`), or the entire contents of the referenced step (`reference = "#step-id"`).

These references form the **Procedural Hierarchy** of the lattice, creating `Pragmatic` relationships between the referencing step and the referenced target. This is distinct from the **Structural Hierarchy** created by Markdown headings. For a complete definition of this dual-hierarchy system, see the "Dual Hierarchies: Structural vs. Procedural" section in `intention_lattice.md`.

**Example Reference:**
```toml
[[steps]]
type = "action"
id = "step-1"
## action definition here

[[steps]]
type = "action"
reference = "#step:step-1"
## step-1 action definition is duplicated here

[[steps]]
# This single string argument is equivalent to the preceeding step.
reference = "#step:step-1"
```

**Passing Variables to References**

To increase reusability, you can pass variables from a parent procedure into a referenced action. This is analogous to calling a function with arguments. The `uses` block referenced below maps variable names in the child action to values or variables in the parent.

If a variable is passed via `uses`, any `prompt` designed to populate that same variable in the child action will be skipped. If the `uses` block is omitted or does not include a specific variable, the child action's own prompt mechanism will function as usual.

```toml
[[steps]]
type = "reference"
id = "brew_morning_coffee"
ref = "act_brew_beverage"

# Pre-populates variables inside the 'act_brew_beverage' procedure.
[steps.uses]
beverage_type = "coffee"
brew_strength = "{{user_preference_strength}}"
```

**Schema for `resolves_tension`:**
```toml
[[resolves_tension]]
item = "<item_id>"
tension = "<tension_name_from_item_definition>"
# (Optional) For state_based tensions, sets the new state.
to_state = "<new_state>"
# (Optional) A description of how the tension is resolved.
description = "Addresses the biological need for energy and nutrients."

```

#### `type: prompt`

This step type creates an interactive communication channel with the participant. It can be used for pure contemplation (`expresses`), for capturing data to control the procedure (`stores_in_variable`), or both.

```toml schema:procedures.promptStep
type = "object"
[properties]
  [properties.type]
    type = "string"
    const = "prompt"
  [properties.prompt_text]
    type = "string"
  [properties.expresses]
    type = "string"
    description = "The ID of a higher-level intention this prompt expresses."
  [properties.response]
    # The full schema for 'response' is defined in 'prompt_interface.md'.
    "$ref" = "prompt_interface.md#/$defs/response"
required = ["type", "prompt_text"]
```

#### `type: opens_window`

This step type programmatically triggers an Attention Window, presenting the participant with a curated set of prompts for reflection. It serves as a bridge between a procedural context and a contemplative one.

When this step is executed, the system initiates the specified Attention Window. For the purposes of graph generation, this step creates `expresses` relationships to all the prompts contained within that window, as described in Section 2.1.2.

```toml schema:procedures.opensWindowStep
type = "object"
[properties]
  [properties.type]
    type = "string"
    const = "opens_window"
  [properties.window_id]
    type = "string"
    description = "The ID of the Attention Window node to be opened."
required = ["type", "window_id"]
```


## 6. Example: Dynamic Procedure

This example for "Plan a Weekend Meal" demonstrates how variables and prompts can create a flexible, interactive procedure.

```toml
id = "goal_plan_weekend_meal"
title = "Plan a Weekend Meal"

[procedure]
variables = [
  { name = "meal_choice", type = "string", default = "dinner" },
  { name = "num_guests", type = "number", default = 1 },
]

[[procedure.steps]]
type = "sequence"
[[procedure.steps.steps]]
  type = "prompt"
  id = "ask_meal_type"
  prompt_text = "What kind of meal do you want to make?"
  [procedure.steps.steps.response]
    type = "choice"
    options = ["dinner", "brunch"]
    stores_in_variable = "meal_choice"
[[procedure.steps.steps]]
  type = "prompt"
  id = "ask_guest_count"
  prompt_text = "How many guests are you expecting?"
  [procedure.steps.steps.response]
    type = "number"
    stores_in_variable = "num_guests"
[[procedure.steps.steps]]
  type = "any_of"
  # The path taken here depends on the value of 'meal_choice'.
  selection_variable = "meal_choice"
  [[procedure.steps.steps.steps]]
    type = "action"
    # This step's id matches a possible value of 'meal_choice'.
    id = "dinner"
    reference = "act_cook_chicken_dinner"
    uses = [
      # The quantity is calculated dynamically from a variable.
      { item = "chicken_breast", quantity = "{{num_guests}}" }
    ]
  [[procedure.steps.steps.steps]]
    type = "action"
    id = "brunch"
    reference = "act_make_pancakes"
    uses = [
      { item = "eggs", quantity = "2 * {{num_guests}}" }
    ]
[[procedure.steps.steps]]
  type = "prompt"
  id = "final_reflection"
  prompt_text = "How do you hope this meal will make your guests feel?"
  # This step is purely for reflection, expressing the core intention.
  expresses = "asp_be_a_good_host"
```

## 6. Integration Points

The `procedure` schema is a static definition. It is brought to life by two key dynamic components of the Noet architecture.

-   **[procedure_engine.md](./procedure_engine.md)**: The Procedure Engine is the runtime that consumes this schema. It is the state machine that tracks the progress of active procedures by matching incoming events against the defined `steps`. It is the literal interpreter of the procedure.

-   **[redline_system.md](./redline_system.md)**: The Redline System provides adaptive, probabilistic matching. While the Procedure Engine checks for direct matches, the Redline System uses this schema as a "template" to find "close enough" matches in the messy reality of the event stream. It handles variations, skipped steps, and reordered actions, learning the participant's unique patterns over time.
