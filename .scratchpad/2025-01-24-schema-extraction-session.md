# SCRATCHPAD - Schema Extraction Session Summary

**Date**: 2025-01-24  
**Purpose**: Address schema documentation approach and design document migration needs  
**Status**: COMPLETE (REVISED - Unified Observable Model)

## Session Summary

User identified two critical gaps in the noet-procedures extraction plan:

1. **Schema-in-docs approach**: Should the `compile_schema.py` pattern become a noet-core library feature?
2. **Missing design documents**: Action observable and prompt schemas need migration from product workspace

## Key Decisions

### 1. Schema Extraction is NOT a Core Library Feature

**Finding**: The `compile_schema.py` approach is a **documentation pattern**, not a runtime library feature.

**Rationale**:
- Issue 1 (Schema Registry) = runtime registration mechanism (HOW schemas are used)
- `compile_schema.py` = build-time extraction from markdown (WHERE schemas are defined)
- These are orthogonal concerns

**Action**: Keep as Python script for now, potentially migrate to Rust tool (`noet schema extract`) in v0.5.0+

### 2. Schema Adherence Principle for AGENTS.md

**Recommendation**: Integrate schema adherence guideline into AGENTS.md
- "Schemas must be defined in design docs with TOML blocks"
- Co-locate documentation with schema definitions
- Use compile_schema.py to ensure consistency

**Status**: Deferred to future session (user didn't request this in scope)

### 3. Design Document Migration Analysis

Analyzed three product design documents for migration needs:

#### instrumentation_design.md - NO MIGRATION
**Verdict**: Entirely product-specific
- Accelerometer data capture
- CSV export for calibration
- FFI control plane for mobile apps
- Stays in product workspace

#### action_interface.md - PARTIAL MIGRATION
**Split**:
- **Migrate to noet-core**: `inference_hint` schema (observable action patterns)
- **Stay in product**: Action Inference Engine implementation, sensor fusion, confidence scoring

**Boundary**: Schema defines WHAT to detect, product implements HOW to observe

#### prompt_interface.md - PARTIAL MIGRATION
**Split**:
- **Migrate to noet-core**: `prompt` step type, response capture schema
- **Stay in product**: Attention windows, psychological guardrails, body budget, crisis override

**Boundary**: Schema defines WHAT prompts look like, product implements WHEN/HOW to deliver

## Work Completed

### A) Updated Issue 17

File: `docs/project/ISSUE_17_NOET_PROCEDURES_EXTRACTION.md`

**Changes**:
- Updated Phase 1 timeline (1 week → 1.5 weeks)
- Added Step 2: Extract Observable Action Schema (2 days)
- Added Step 3: Extract Prompt/Response Schema (2 days)
- Updated migration plan to include `action_interface.md` and `prompt_interface.md`
- Documented boundary between general and product-specific for each source doc
- Added new risks around boundary clarity

**Phase 1 now includes**:
1. Core procedure docs (COMPLETE)
2. Observable action schema extraction
3. Prompt/response schema extraction
4. Schema separation definition
5. Roadmap update

### B) Created Issue 18

File: `docs/project/ISSUE_18_EXTENDED_PROCEDURE_SCHEMAS.md` (389 lines - REVISED)

**Scope**: Participant channel implementation (prompts as observations)
- Extends observable action schema with Participant channel
- No separate `prompt` step type - uses `action` with `channel = "Participant"`
- Response capture via `response_config` property in `inference_hint`
- Leverages BeliefNode title/text for prompt content
- Unified event model (`action_detected` for all channels)

**Key sections**:
- Participant channel conventions
- Response configuration schema
- Multi-modal examples (automatic OR manual)
- Extension points for UI renderers
- Non-behavior-change examples

**Dependencies**: Issue 1 (Schema Registry), Issue 17 (noet-procedures)

### C) Created Unified Design Document

#### docs/design/action_observable_schema.md (854 lines - v0.2)

**Content**:
- `inference_hint` recursive schema (grouping + transition events)
- Temporal constraints (duration, time_of_day, day_of_week)
- Confidence thresholds
- **Participant channel section** (merged from prompt_interface.md)
- Response capture via `response_config` property
- Unified `action_detected` event schema (all channels)
- Extension points (ObservationProducer, InferenceEngine, ParticipantRenderer traits)
- Multi-modal examples (automatic AND/OR manual)
- BeliefNode structure documentation (title, text provide prompt content)
- Examples across all domains

**Key architectural insight**: Prompts are observations on the Participant channel, not a separate step type

**Migrated from**: 
- `noet/docs/design/action_interface.md` (sections 3.1-3.2)
- `noet/docs/design/prompt_interface.md` (section 4.2, merged into Participant channel)

**Left in product**:
- Action Inference Engine implementation
- ObservationEvent producers (sensors, GPS, screen activity, UI frameworks)
- Semantic label mapping
- Confidence scoring algorithms
- UI rendering (modal dialogs, forms, notifications)
- Attention window system (psychological guardrails)
- Body budget integration
- Crisis override logic
- Window selection policies

#### docs/design/prompt_schema.md - DELETED

Merged into `action_observable_schema.md` as Participant channel section. Prompts are not a separate concept - they're observable actions.

### D) Updated Roadmap

File: `docs/project/ROADMAP.md`

**Changes**:
- Added Issue 18 to v0.5.0+ section
- Updated issue dependencies graph
- Issue count: 18 total issues now tracked

## Architecture Insights

### Three-Layer Separation

```
┌─────────────────────────────────────────────────┐
│ noet-core (data structures)                     │
│ • Schema registry API                           │
│ • Lattice primitives (BeliefNode, BeliefBase)    │
│ • TOML parsing                                  │
└─────────────────────────────────────────────────┘
                    ▲
                    │
┌─────────────────────────────────────────────────┐
│ noet-procedures (general-purpose runtime)       │
│ • Procedure schemas (registered at runtime)     │
│ • Execution tracking                            │
│ • Observable action schema (WHAT to detect)     │
│   - All channels: Sensors, Systems, Participant │
│   - Unified event model (action_detected)       │
│ • Deviation recording                           │
└─────────────────────────────────────────────────┘
                    ▲
                    │
┌─────────────────────────────────────────────────┐
│ noet product (behavior change app)              │
│ • Observation producers (HOW to observe)        │
│   - Sensors, GPS, barcode scanners              │
│   - UI frameworks (Participant channel)         │
│ • Inference engine (sensor fusion)              │
│ • UI renderer (Participant channel observations)│
│ • Attention windows (psychological guardrails)  │
│ • Learning algorithms                           │
└─────────────────────────────────────────────────┘
```

### Key Insight: Unified Observable Model

**All observations use the same pattern**:
- Schema (general): `inference_hint` with channel/producer
- Implementation (product): Observation producers + inference engine

**Participant channel** is not special:
- Same schema as Temperature, Barcode, SystemMetrics channels
- `response_config` defines how to capture the observation
- BeliefNode title/text provide prompt content (no `prompt_text` needed)
- Same `action_detected` event as all other channels

**This eliminates**:
- Separate `prompt` step type
- Separate `prompt_response` event
- Redundant prompt_text/prompt_title properties

## Files Created

1. `docs/project/ISSUE_18_EXTENDED_PROCEDURE_SCHEMAS.md` (389 lines - revised for unified model)
2. `docs/design/action_observable_schema.md` (854 lines - v0.2, includes Participant channel)

## Files Modified

1. `docs/project/ISSUE_17_NOET_PROCEDURES_EXTRACTION.md` (~100 lines changed)
2. `docs/project/ROADMAP.md` (added Issue 18 reference)

## Total Output

~1,400 lines of documentation created/modified (more concise due to unified model)

## Next Steps

### Immediate (Issue 17 Phase 1)
- [ ] Human review of Issue 17 updates
- [ ] Human review of Issue 18 scope
- [ ] Human review of design documents (observable/prompt schemas)
- [ ] Validate boundary decisions (what's general vs product-specific)

### After Issue 1 Complete (Schema Registry)
- [ ] Begin Issue 17 implementation (noet-procedures extraction)
- [ ] Integrate observable action schema
- [ ] Integrate prompt schema
- [ ] Define extension traits in Rust

### Future Considerations
- Schema adherence guidelines in AGENTS.md (optional)
- Migrate compile_schema.py to Rust tool (v0.5.0+, low priority)
- Example observation producers as separate crates (community contributions)

## Open Questions Resolved

1. **Schema extraction in Issue 1?** → No, it's a documentation pattern, not runtime feature
2. **instrumentation_design.md migration?** → No, entirely product-specific
3. **action_interface.md migration?** → Partial, schema only (not engine)
4. **prompt_interface.md migration?** → Partial, schema only (not windows)
5. **Issue 17 scope sufficient?** → No, needed Issue 18 for observables + prompts

## Key Principles Applied

From AGENTS.md:
- ✅ Succinct and reviewable (Issue 18: 389 lines, design doc: 854 lines but comprehensive)
- ✅ Schema as contract (all schemas defined in TOML blocks)
- ✅ Clear boundaries (explicitly marked what's general vs product)
- ✅ Non-behavior-change examples (manufacturing, labs, deployment)
- ✅ Extension over configuration (traits/interfaces for products)
- ✅ Unified conceptual model (prompts are observations, not separate concept)

## Risks Identified

1. **Boundary drift**: As products evolve, general/product boundary may blur
   - Mitigation: Document boundaries explicitly in design docs, reference in code comments

2. **Over-abstraction**: Observable/prompt schemas might be too abstract
   - Mitigation: Provide diverse concrete examples, document extension points clearly

3. **Schema adherence**: compile_schema.py pattern not enforced
   - Mitigation: Consider integrating into AGENTS.md, make it a convention

## Success Metrics

- [x] Issue 17 updated with unified observable model
- [x] Issue 18 created focusing on Participant channel
- [x] Design document created with unified schema (v0.2)
- [x] Boundaries documented (general vs product-specific)
- [x] Examples demonstrate non-behavior-change use cases
- [x] Roadmap updated
- [x] Architectural insight captured: prompts are observations
- [x] BeliefNode structure leveraged (no redundant fields)

## Major Architectural Decisions This Session

1. **Schema extraction is NOT a library feature** - it's a documentation pattern (compile_schema.py stays as build tool)

2. **Prompts are observations** - eliminated separate `prompt` step type in favor of unified observable model with Participant channel

3. **BeliefNode structure reused** - steps don't need `prompt_text`/`prompt_title` because BeliefNodes already have `title` and markdown text

4. **Single event model** - all observations emit `action_detected`, no special `prompt_response` event

5. **Multi-modal patterns natural** - "either automatic OR manual" is just `any_of` with multiple channels

## Impact on Scope

**Simplified**:
- One design document instead of two (action_observable_schema.md only)
- One step type for all observations (`action` with `inference_hint`)
- One event type for all detections (`action_detected`)
- Smaller Issue 18 (389 lines vs original 620 lines)

**Unified conceptual model makes system more powerful while being simpler**

---

**Status**: Session complete, ready for human review
**Delete When**: After Issue 17/18 implementation begins
