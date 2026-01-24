# Issue 17: Extract noet-procedures Crate

**Priority**: MEDIUM  
**Estimated Effort**: 2-3 weeks  
**Dependencies**: Issue 1 (Schema Registry)  
**Blocks**: None (enables future procedural features)

## Summary

Extract procedural execution functionality into a separate `noet-procedures` crate. This crate will provide general-purpose runtime infrastructure for executing, tracking, and adapting procedures defined in noet lattices. The extraction establishes a clean boundary: `noet-core` provides data structures and schemas, `noet-procedures` provides execution and tracking, and the noet product provides behavior-specific learning and inference.

## Goals

1. Create `noet-procedures` crate with clear API boundaries
2. Define procedure schema as runtime-registered extension (not built into noet-core)
3. Implement "as-run" tracking (template + context + execution record)
4. Provide core redline system (deviation recording, not prediction)
5. Enable downstream products to extend with learning/adaptation

## Architecture

### The Three-Piece "As-Run" Model

Every procedure execution consists of:
1. **Template** (as-written): The procedure definition from lattice
2. **Executor Context** (who/when/where): Metadata about execution
3. **As-Run Record** (reality): What actually happened, including deviations

This is analogous to paper procedures in aviation, manufacturing, and lab protocols where practitioners mark up the procedure as they execute it.

### Crate Structure

```
noet-procedures/
├── src/
│   ├── schema/           # Procedure schema definitions
│   │   ├── mod.rs       # Schema registration with noet-core
│   │   ├── procedure.rs # Core procedure schema
│   │   └── steps.rs     # Step types (action, prompt, logical operators)
│   ├── execution/        # Runtime execution tracking
│   │   ├── mod.rs
│   │   ├── run.rs       # ProcedureRun, ExecutionRecord
│   │   └── context.rs   # ExecutorContext (who/when/where)
│   ├── redlines/         # As-run deviation tracking
│   │   ├── mod.rs
│   │   ├── correction.rs # CorrectionEvent schema
│   │   ├── deviation.rs  # Deviation analysis (template vs as-run)
│   │   └── promotion.rs  # Promote as-run to new template
│   └── lib.rs           # Public API
├── examples/            # Usage examples
└── tests/              # Integration tests
```

### Boundary with noet-core

**noet-core provides:**
- Schema registry API (Issue 1)
- Lattice data structure and query primitives
- Node/edge storage and manipulation
- TOML parsing infrastructure

**noet-procedures provides:**
- Procedure schema (registered at runtime via schema registry)
- Execution tracking (runs, steps, timing)
- Deviation recording (as-run vs template)
- Template/as-run comparison
- Lattice promotion (as-run → new procedure node)

**noet-procedures does NOT provide:**
- Behavior prediction (that's product-specific)
- Sensor integration (dwelling points, screen monitoring)
- Learning algorithms (HMM, probabilistic matching)
- Motivation tracking (intrinsic reward, efficacy scores)

### Boundary with noet Product

The noet product will extend `noet-procedures` with:
- Action inference engine (sensor → action mapping)
- Learned parameters (probabilistic adaptation)
- Predictive matching (HMM-based procedure recognition)
- Motivation tracking and behavior change features
- Privacy-sensitive observation infrastructure

## Implementation Steps

### Phase 1: Design Document Migration (1 week)

1. **Migrate Core Procedure Documents to noet-core** (3 days)
   - [x] Create `docs/design/procedure_schema.md` (from `procedures.md`) - COMPLETE
   - [x] Create `docs/design/procedure_execution.md` (from `procedure_engine.md`) - COMPLETE
   - [x] Create `docs/design/redline_system.md` (from `redline_system.md`) - COMPLETE
   - [x] Strip product-specific sections (motivation tracking, sensor integration) - COMPLETE
   - [x] Keep general-purpose concepts (as-run model, deviation tracking) - COMPLETE

2. **Extract and Unify Observable Action Schema** (3 days)
   - [x] Review `action_interface.md` and `prompt_interface.md` in product workspace - COMPLETE
   - [x] Extract `inference_hint` schema (groupingEvent, transitionEvent) - COMPLETE
   - [x] Merge prompt patterns into Participant channel observations - COMPLETE
   - [x] Create `docs/design/action_observable_schema.md` (unified model) - COMPLETE
   - [x] Document that prompts are observations on Participant channel - COMPLETE
   - [x] Add `response_config` to transitionEvent for Participant channel - COMPLETE
   - [x] Document BeliefNode structure (title, text already provide prompt content) - COMPLETE
   - [x] Mark boundary: schema (general) vs inference engine + UI rendering (product-specific) - COMPLETE
   - [x] Mark what stays in product: Action Inference Engine, sensor fusion, UI rendering, attention windows - COMPLETE

3. **Define Schema Separation** (2 days)
   - [ ] Document which schema elements belong in `noet-procedures`
   - [ ] Identify integration points with noet-core schema registry
   - [ ] Define public API contracts
   - [ ] Create sequence diagrams for execution flow
   - [ ] Clarify extension points for products (observation producers, UI renderers)

4. **Update ROADMAP** (1 day)
   - [ ] Add noet-procedures extraction to backlog
   - [ ] Clarify versioning strategy (separate from noet-core)
   - [ ] Document dependencies on Issue 1
   - [ ] Note relationship to Issue 18 (Participant Channel implementation)

### Phase 2: Crate Scaffolding (3 days)

4. **Create Crate Structure** (1 day)
   - [ ] Initialize cargo project
   - [ ] Set up workspace dependencies
   - [ ] Add noet-core as dependency
   - [ ] Configure CI/CD

5. **Schema Registration** (2 days)
   - [ ] Implement schema definitions in Rust
   - [ ] Register with noet-core's schema registry
   - [ ] Write registration tests
   - [ ] Verify TOML parsing with registered schema

### Phase 3: Execution Tracking (1 week)

6. **Core Execution Infrastructure** (3 days)
   - [ ] `ProcedureRun` struct (run metadata, status)
   - [ ] `ExecutionRecord` struct (steps executed, timing, context)
   - [ ] `ExecutorContext` struct (who, when, where)
   - [ ] Step-by-step execution tracking

7. **Event Schemas** (2 days)
   - [ ] `ProcedureStartedEvent`
   - [ ] `StepExecutedEvent`
   - [ ] `ProcedureCompletedEvent`
   - [ ] Event log integration

8. **Storage Layer** (2 days)
   - [ ] Run index table (SQLite)
   - [ ] Step execution table
   - [ ] Query API (list runs, get run details)

### Phase 4: Redline System (1 week)

9. **Deviation Recording** (3 days)
   - [ ] `CorrectionEvent` schema
   - [ ] `DeviationReport` (template vs as-run diff)
   - [ ] Manual correction API
   - [ ] Deviation storage

10. **Template Comparison** (2 days)
    - [ ] Compute diff between template and as-run
    - [ ] Identify skipped steps, reordering, timing differences
    - [ ] Report generation

11. **Lattice Promotion** (2 days)
    - [ ] Create new procedure node from as-run
    - [ ] Preserve reference to original template
    - [ ] Metadata tracking (promoted_from, promoted_at)

### Phase 5: Documentation & Examples (3 days)

12. **API Documentation** (1 day)
    - [ ] Rustdoc for all public APIs
    - [ ] Module-level documentation
    - [ ] Usage examples in doc comments

13. **Tutorial Examples** (1 day)
    - [ ] Basic procedure execution
    - [ ] Recording deviations
    - [ ] Promoting as-run to template
    - [ ] Querying execution history

14. **Integration Guide** (1 day)
    - [ ] How to register custom schemas
    - [ ] How to extend with learning (for products)
    - [ ] Best practices for execution tracking

## Testing Requirements

### Unit Tests
- Schema registration and retrieval
- Execution record creation and storage
- Deviation computation
- Template promotion logic

### Integration Tests
- End-to-end procedure execution
- Correction event handling
- Multi-step procedures with logical operators
- Query API correctness

### Examples as Tests
- All examples compile and run
- Doctests pass
- Tutorial code is valid

## Success Criteria

- [ ] `noet-procedures` crate compiles independently
- [ ] Procedure schema registered via noet-core API (no hardcoding)
- [ ] Can execute and track procedure runs
- [ ] Can record deviations from template
- [ ] Can promote as-run to new template
- [ ] Documentation complete
- [ ] Integration tests pass
- [ ] Examples demonstrate key features
- [ ] No product-specific code in crate

## Design Principles

### Clean Boundaries
- **If it's about recording reality** → noet-procedures
- **If it's about predicting behavior** → noet product
- **If it's core data structure** → noet-core

### General-Purpose Design
Avoid behavior-change assumptions. Design for:
- Manufacturing SOPs (deviation tracking, equipment substitutions)
- Lab protocols (reagent variations, timing adjustments)
- Deployment runbooks (commands executed vs planned)
- Emergency response (reality vs training)
- Cooking recipes (ingredient substitutions, technique variations)

### Extensibility
Products should be able to:
- Add learning algorithms without forking
- Register additional event types
- Extend schema with custom fields
- Plug in custom storage backends

## Open Questions

1. **Schema Versioning**: How should `noet-procedures` handle schema evolution?
   - Likely: Semantic versioning, migration utilities
   
2. **Storage Backend**: Should storage be pluggable (SQLite, Postgres, custom)?
   - Phase 1: SQLite only
   - Phase 2: Consider abstraction

3. **Event Log**: Should `noet-procedures` maintain its own event log or integrate with external?
   - Likely: Provide event schemas, let caller handle storage

4. **Concurrent Execution**: How to handle multiple simultaneous procedure runs?
   - Design: Each run has unique `run_id`, no shared state

5. **Step IDs**: Should step IDs be globally unique or procedure-scoped?
   - Design: Procedure-scoped, qualified by `procedure_id` + `step_index`

6. **Event.rs Redesign for Procedure Execution** (HIGH PRIORITY)
   - **Context**: Current `src/event.rs` was designed before LSP diagnostics and procedure execution
   - **Problem**: May not fully support message passing needed for:
     - Procedure execution events (see `procedure_execution.md`)
     - Observable action detection (see `action_observable_schema.md`)
     - LSP-style diagnostics and progress notifications
     - Bidirectional communication (executor responses, corrections)
   - **Current Event enum**: Only `Ping`, `Belief(BeliefEvent)`, `Focus(PerceptionEvent)`
   - **Missing event types** (from procedure_execution.md):
     - `proc_triggered`, `step_matched`, `proc_completed`, `proc_aborted`
     - `prompt_response`, `deviation_detected`, `procedure_correction`
     - `action_detected` (from action_observable_schema.md)
   - **Questions**:
     - Should Event enum expand to include procedure-specific variants?
     - Should noet-procedures define its own Event type?
     - How to integrate with LSP notifications (textDocument/publishDiagnostics pattern)?
     - How to handle bidirectional events (server → client, client → server)?
   - **Decision needed before**: Phase 2 (noet-procedures crate implementation)
   - **Related**: Issue 15 (Filtered Event Streaming), Issue 11 (LSP), Issue 18 (Participant channel)
   - **Status**: Needs design document or architectural decision before proceeding

## Migration Path

### Material to Move from Product Docs

**From `procedures.md`:**
- Section 1: Purpose → `docs/design/procedure_schema.md`
- Section 2: Role of procedure property → Schema design doc
- Section 3-5: Schema definitions → Rust implementation
- Section 6: Examples → Tutorial examples

**From `procedure_engine.md`:**
- Section 2-3: Core concepts, architecture → `docs/design/procedure_execution.md`
- Section 4: Event log structure → Event schemas
- Section 4.2: Run index → Execution tracking
- Section 7.1-7.3: **Exclude** (motivation tracking is product-specific)
- Section 7.5: Redlines → Redline system design

**From `redline_system.md`:**
- Core concepts (template vs reality) → `docs/design/redline_system.md`
- Correction feedback loop → Deviation recording API
- Lattice promotion → Promotion implementation
- **Exclude**: LearnedParameters, HMM matching (product-specific)

**From `action_interface.md`:**
- **Migrated**: `inference_hint` schema (section 3.2) → `docs/design/action_observable_schema.md` ✅
- **Migrated**: Observable action step patterns (section 3.1) ✅
- **Migrated**: Concept of action steps being "by reference" or "inline" ✅
- **Keep in product**: Action Inference Engine implementation (section 4)
- **Keep in product**: ObservationEvent stream processing
- **Keep in product**: Semantic label mapping and resolution
- **Keep in product**: Confidence scoring algorithms
- **Keep in product**: Sensor-specific examples (dwelling points, screen activity)

**From `action_inference_engine.md`:**
- **Do not migrate** - entirely product-specific (sensor fusion, HMM, behavior prediction)

**From `prompt_interface.md`:**
- **Migrated**: Response capture schema → merged into Participant channel section of `action_observable_schema.md` ✅
- **Migrated**: Form elements and validation → `response_config` property ✅
- **Key insight**: Prompts are observations on Participant channel, not a separate step type ✅
- **Key insight**: BeliefNode title and markdown text provide prompt content (no `prompt_text` needed) ✅
- **Keep in product**: Attention window schema with guardrails (section 3)
- **Keep in product**: Prerequisites (body budget, crisis override, complexity level)
- **Keep in product**: Window selection policy and frequency management
- **Keep in product**: UI rendering implementation (modal dialogs, forms, notifications)
- **Keep in product**: `opens_window` step type (bridge to attention windows)

### What Stays in Product

- Motivation tracking (intrinsic reward, efficacy, relatedness)
- Practice maturity metrics (rhythm analysis, consistency scores)
- Learned parameters (skip probabilities, duration variance)
- Probabilistic matching (HMM, Baum-Welch)
- Action inference engine implementation (sensor fusion, confidence scoring)
- ObservationEvent producers (dwelling points, screen activity monitors)
- Semantic label resolution and mapping
- Behavior prediction algorithms
- Privacy-sensitive observation infrastructure
- Attention window system with psychological guardrails
- Body budget integration
- Crisis override logic

## Risks

**Risk**: Schema registry (Issue 1) incomplete  
**Mitigation**: Block on Issue 1 completion, validate API first

**Risk**: Over-generalizing makes API awkward  
**Mitigation**: Design for concrete use cases first, generalize incrementally

**Risk**: Tight coupling with noet product  
**Mitigation**: Code review for product assumptions, write non-behavior-change examples

**Risk**: Performance overhead from abstraction  
**Mitigation**: Benchmark execution tracking, optimize hot paths

**Risk**: Observable action schemas too abstract  
**Mitigation**: Document clear extension points, provide concrete examples for non-behavior-change use cases (manufacturing SOPs, lab protocols)

**Risk**: Boundary between general and product-specific unclear  
**Mitigation**: Explicitly mark in design docs what stays in product and why

**Risk**: Confusion about unified observable model (prompts as observations)  
**Mitigation**: Clear documentation explaining why prompts are Participant channel observations, show multi-modal examples

## References

- **ISSUE_01_SCHEMA_REGISTRY.md** - Dependency
- **ISSUE_14_NAMING_IMPROVEMENTS.md** - Likely to affect API
- **ISSUE_18_EXTENDED_PROCEDURE_SCHEMAS.md** - Participant channel implementation
- **ROADMAP.md** - Versioning strategy
- **AGENTS.md** - Document structure guidelines
- Product docs (migrated): `procedures.md`, `procedure_engine.md`, `redline_system.md`
- Product docs (partial migration, unified): `action_interface.md`, `prompt_interface.md` (merged into single observable model)

## Next Steps After Completion

1. Noet product can import `noet-procedures` as dependency
2. Product implements learning/adaptation as extension
3. Other applications (lab protocols, manufacturing) can use procedures
4. Community contributions to procedural features
5. Potential future: Publish to crates.io separately from noet-core
