# SCRATCHPAD - Procedures Migration Plan

**Status**: Planning  
**Created**: 2025-01-XX  
**Related Issue**: ISSUE_17_NOET_PROCEDURES_EXTRACTION.md

## Purpose

Track the migration of procedure-related documents from the noet product workspace into noet-core's design docs, in preparation for eventual extraction to a separate `noet-procedures` crate.

## Documents Currently in noet-core Root

These were copied from the product workspace and need to be processed:

1. **procedures.md** - Procedure schema specification
2. **procedure_engine.md** - Runtime execution and tracking
3. **redline_system.md** - Adaptive procedure matching
4. **action_inference_engine.md** - Multi-modal sensor inference

## Migration Strategy

### Phase 1: Design Doc Migration (Before Extraction)

Move general-purpose concepts to `docs/design/`:

#### procedures.md → docs/design/procedure_schema.md
- **Keep**: Schema definitions (sections 3-5)
- **Keep**: Semantic inference algorithm (section 2.1)
- **Keep**: Integration points (section 6)
- **Strip**: Product-specific motivation examples
- **Status**: ✅ COMPLETE - Created `docs/design/procedure_schema.md`

#### procedure_engine.md → docs/design/procedure_execution.md
- **Keep**: Core concepts (section 2-3)
- **Keep**: Event log architecture (section 4.1)
- **Keep**: Run index structure (section 4.2)
- **Keep**: Redline feedback loop basics (section 7.5)
- **Strip**: Motivation tracking (section 7.2-7.4, 7.8)
- **Strip**: Practice maturity metrics (section 7.1, 7.3, 7.6-7.7)
- **Status**: ✅ COMPLETE - Created `docs/design/procedure_execution.md`

#### redline_system.md → docs/design/redline_system.md
- **Keep**: Core "as-run" model concept
- **Keep**: Template vs. reality gap (conceptual)
- **Keep**: CorrectionEvent schema
- **Keep**: Lattice promotion mechanism
- **Keep**: Deviation recording
- **Strip**: LearnedParameters (probabilistic adaptation)
- **Strip**: HMM/Baum-Welch learning algorithms
- **Strip**: Dwelling point integration
- **Strip**: Automatic behavior prediction
- **Status**: ✅ COMPLETE - Created `docs/design/redline_system.md`

#### action_inference_engine.md → STAYS IN PRODUCT
- **Do not migrate**: Entirely product-specific
- This is sensor-driven behavior inference
- No general-purpose applicability
- **Status**: Leave in product workspace

### Phase 2: Extraction (During Issue 17)

Once Issue 1 (Schema Registry) is complete:
1. Create `noet-procedures` crate structure
2. Implement schemas in Rust (not TOML in repo)
3. Register schemas via noet-core API
4. Move execution tracking code
5. Remove `procedures.md`, `procedure_engine.md`, `redline_system.md` from noet-core root
6. Keep design docs in `docs/design/` as reference

## Key Insight: Procedures are Not Built-in

**Original assumption**: Procedure schema is core to noet-core  
**New understanding**: With Issue 1's schema registry, procedures are just another schema that gets registered at runtime

**Implication**: 
- noet-core has NO procedure-specific code
- `noet-procedures` crate registers schema via `SCHEMAS.register()`
- Other crates can define their own procedural schemas
- Clean separation of concerns

## Boundary Clarification

### noet-core (data structure library)
- Schema registry API
- Lattice primitives
- TOML parsing infrastructure
- **NO** procedure-specific code

### noet-procedures (procedural runtime library)
- Registers procedure schema at initialization
- Execution tracking (runs, steps, timing)
- Deviation recording (as-run vs template)
- Template promotion
- **NO** behavior prediction or learning

### noet product (behavior change application)
- Action inference engine (sensor → action)
- Learned parameters (probabilistic adaptation)
- Motivation tracking
- Practice maturity metrics
- HMM-based procedure matching

## Files to Create in docs/design/

1. `procedure_schema.md` - Clean schema specification
2. `procedure_execution.md` - Runtime tracking design
3. `redline_system.md` - As-run model and deviation tracking

## Files to Eventually Remove from noet-core Root

After extraction complete:
- `procedures.md` → deleted
- `procedure_engine.md` → deleted  
- `redline_system.md` → deleted
- `action_inference_engine.md` → moved back to product repo
- `pitch.md` → moved back to product repo

## Next Actions

1. ✅ Review this plan with human
2. ✅ Create cleaned design docs in `docs/design/` - **PHASE 1.1 COMPLETE**
3. Wait for Issue 1 completion
4. Execute Issue 17 extraction
5. Clean up root directory

## Phase 1.1 Completion Summary (2025-01-XX)

Successfully migrated three design documents to `docs/design/`:

1. **procedure_schema.md** (388 lines)
   - General-purpose schema specification
   - Removed all product-specific references
   - Emphasized runtime registration via schema registry
   - Applicable to manufacturing, labs, deployment, cooking, etc.

2. **procedure_execution.md** (514 lines)
   - Core execution infrastructure (state machine, event log, run index)
   - Removed motivation tracking and practice maturity metrics
   - Kept general deviation handling
   - Emphasized executor confirmation and correction flows

3. **redline_system.md** (440 lines)
   - As-run deviation tracking (template vs. reality)
   - Removed probabilistic learning and HMM algorithms
   - Kept correction events and template promotion
   - Emphasized explicit deviation recording over prediction

**Total**: ~1,342 lines of cleaned, general-purpose design documentation

**Next**: Wait for Issue 1 (Schema Registry) completion before proceeding with Issue 17 extraction.

---

**Note**: This is a scratchpad file - not permanent documentation. Delete after extraction complete.