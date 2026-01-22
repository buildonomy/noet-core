# Issue 5: Core Library Documentation

**Priority**: CRITICAL - Blocks soft open source  
**Estimated Effort**: 
- **Soft Open Source**: 1-2 days (minimal viable docs)
- **Full Completion**: 3-4 days (comprehensive docs, post-soft-open-source)

**Dependencies**: 
- **Soft Open Source**: None (can proceed immediately)
- **Full Completion**: Issue 10 (needs working examples for comprehensive tutorials)

**Context**: Part of [`ROADMAP_NOET-CORE_v0.1.md`](./ROADMAP_NOET-CORE_v0.1.md) - preparation for extracting `noet-core` into standalone git repository. This issue has two completion stages: minimal docs for soft open source, then comprehensive docs for v0.1.0 announcement.

## Summary

Create documentation for `noet-core` open source library in two stages:

**Stage 1: Soft Open Source (1-2 days)** - Minimal viable documentation to make repository public:
- Migrate `beliefset_architecture.md` with product references removed
- Create basic `docs/architecture.md` 
- Update core README with library purpose and basic usage
- **Clean Cargo.toml** (remove product crate dependencies)
- Verify standalone build works

**Stage 2: Full Completion (post-soft-open-source, 2-3 additional days)** - Comprehensive documentation:
- Comprehensive tutorials (codec implementation, querying, file watching, database integration)
- BID deep dive documentation
- FAQ
- Working examples requiring Issue 10 (daemon, file watching)
- Unified navigation between rustdoc and manual docs

**Post-Migration**: After this issue completes, the entire `rust_core/crates/core/` directory (including its `docs/`) will be moved to a new git repository. Documentation must be self-contained and reference-complete for standalone use.

**Verification**: "Documentation compiles" means `cargo doc` passes without errors and all cross-references resolve correctly.

## Goals

1. Migrate `beliefset_architecture.md` to core library design docs, removing lattice references
2. Create architecture overview explaining core concepts (BID, BeliefSet, Codec)
3. Write codec implementation tutorial
4. Provide working examples (parsing, querying, custom codecs)
5. Document public API clearly via rustdoc (`lib.rs`, module docs)
6. Unify manual documentation with rustdoc for seamless developer navigation
7. Update shared design README to reference core library docs
8. Establish library vs. product boundary in docs

## Architecture

### Documentation Structure


**Core Library Docs** (`rust_core/crates/core/docs/`):

```
docs/
├── architecture.md     # Migrated from docs/design/beliefset_architecture with lattice references removed.
├── codecs.md           # DocCodec trait, implementation tutorial
├── ids_and_refs.md     # Multi-ID system and NodeKey multi reference system deep dive (material mainly derived from docs/design/design_architecture.md and docs/design/intention_lattice.md)
├── faq.md              # Common questions
└── tutorials/          # These wrap and explain the executable examples, giving pointers on how to change and manipulate them.
    ├── basic_parsing.md
    ├── querying.md
    ├── custom_codec.md
    ├── file_watching.md
    └── database_integration.md
```

**Primary Rustdoc** (`src/lib.rs`):
- Overview and relationship to prior art
- Core capabilities summary
- Multi-pass compilation explanation
- Basic usage examples (must compile)
- Links to detailed tutorial docs

**Integration Strategy**:
- `lib.rs` provides entry point and high-level overview
- Tutorial docs (`docs/*.md`) provide deep dives and step-by-step guides
- Design docs (`docs/design/*.md`) provide full specifications
- Each tutorial doc links back to relevant rustdoc sections
- Rustdoc examples link to tutorial docs for detailed walkthroughs

### What Library Does (Document This)
- Graph parsing and querying infrastructure
- BID injection and identity management
- Multi-format document parsing (Markdown, TOML)
- Event-driven synchronization
- Codec extensibility
- schema extensibility

### What Product Does (Don't Document)
- Action inference from sensors
- Dwelling point detection
- Procedure execution engine
- Motivation tracking
- Mobile app UI
- "Intention Lattice" schema/ontology

## Implementation Steps

### Stage 1: Soft Open Source (1-2 days)

0. **Migrate Core Design Document** (0.5 days) ⭐ REQUIRED FOR SOFT OPEN SOURCE ✅ COMPLETE
   - [x] Copy `docs/design/beliefset_architecture.md` to `rust_core/crates/core/docs/design/`
   - [x] **Remove all LatticeService references** (Section 3.5, lines 550-679) - this is product-specific orchestration
   - [x] Remove "Relationship to Intention Lattice" section (lines 681-708) - product-specific
   - [x] Verify `cargo doc` passes and cross-references resolve
   - [x] Ensure no external references - doc must be self-contained for standalone repo

**Result**: Created `rust_core/crates/core/docs/design/beliefset_architecture.md` (747 lines) with product sections removed, version updated to 0.2, library-focused terminology

0b. **Clean Cargo.toml Dependencies** (0.25 days) ⭐ REQUIRED FOR SOFT OPEN SOURCE ✅ COMPLETE
   - [x] Audit `rust_core/crates/core/Cargo.toml` dependencies
   - [x] Remove any references to product-specific crates (e.g., `lattice_service`, product schemas)
   - [x] Verify `cargo build --all-features` works in isolation
   - [x] Verify `cargo test --all-features` passes
   - [x] Check feature flags are appropriate for library usage
   - [x] Document any workspace dependencies that need to be versioned for standalone repo

**Result**: Cargo.toml is clean - no product crate dependencies found. Build status:
- `cargo build --all-features`: ✅ SUCCESS (33.83s)
- `cargo test --all-features`: ⚠️ PARTIAL (3 doctest failures expected for Stage 2)
- `cargo doc --all-features`: ✅ SUCCESS (3 warnings acceptable)

1. **Create Basic Architecture Overview** (0.5 days) ⭐ REQUIRED FOR SOFT OPEN SOURCE ✅ COMPLETE
   - [x] Create `rust_core/crates/core/docs/architecture.md` with:
     - High-level overview of core concepts (BID, BeliefSet, Codec)
     - Multi-pass compilation explanation (extracted from `lib.rs`)
     - Relationship to prior art (brief summary)
     - Link to full design doc for details
   - [x] Keep beginner-friendly, defer deep technical details
   - [x] Ensure no product-specific references

**Result**: Created `rust_core/crates/core/docs/architecture.md` (275 lines) with core concepts, architecture overview, comparisons to other tools

6a. **Update Core README** (0.25 days) ⭐ REQUIRED FOR SOFT OPEN SOURCE ✅ COMPLETE
   - [x] Update `rust_core/crates/core/README.md`:
     - Clear explanation of what noet-core is and its purpose
     - Basic usage example (reference `examples/basic_usage.rs`)
     - Installation instructions
     - Link to documentation (`docs/architecture.md`, `cargo doc`)
     - Clarify library vs. product boundary
     - Note: Pre-1.0, API may change
   - [x] Update `docs/design/README.md`:
     - Add pointer to `noet-core` library documentation
     - Note that `noet-core` will be extracted to standalone repository

**Result**: Complete rewrite of core README for library focus, added noet-core section to parent design README

**✅ SOFT OPEN SOURCE CHECKPOINT COMPLETE** - Repository ready to be made public after Steps 0, 0b, 1, 6a complete

**Completion Date**: 2025-01-17  
**Public Release Date**: 2025-01-18  
**Public Repository**: https://gitlab.com/buildonomy/noet-core

**Additional Work Completed**:
- [x] Refactored `lib.rs` rustdoc to be concise "Getting Started" guide (~110 lines vs 240 lines)
- [x] Created `docs/project/DOCUMENTATION_STRATEGY.md` explaining documentation hierarchy and single source of truth approach
- [x] All documentation follows Rust ecosystem best practices (tokio, serde, diesel patterns)
- [x] Fixed all doc tests in `lib.rs` (2025-01-18) - all 4 tests now passing
- [x] Updated `.gitignore` with comprehensive open-source Rust project template (2025-01-18)

### Stage 2: Full Completion (post-soft-open-source, 2-3 days)

1b. **Extract Full Architecture Content** (0.5 days)
   - [ ] From `lib.rs` rustdoc extract and expand:
     - Multi-pass compilation explanation (currently lines 49-97)
     - Relationship to prior art (currently lines 99-180)
     - Unique features (currently lines 182-210)
   - [ ] From migrated `beliefset_architecture.md` extract:
     - Compilation Model (lines 29-55)
     - Identity Management (lines 56-79)
     - Graph Structure (lines 113-168)
     - BeliefSet vs Beliefs API (lines 395-446)
   - [ ] Remove remaining product references (focus on library concepts)
   - [ ] Focus on public API
   - [ ] Add beginner-friendly examples
   - [ ] Link back to `lib.rs` rustdoc sections

2. **Write Comprehensive Codec Tutorial** (1 day)
   - [ ] Explain DocCodec trait role (link to `src/codec/mod.rs` rustdoc)
   - [ ] Step-by-step: Build JSON codec
   - [ ] Document built-in codecs (Markdown, TOML)
   - [ ] Best practices (error handling, testing)
   - [ ] Integration with BeliefSetParser (reference `Parser::simple()` convenience constructor)
   - [ ] Link back to relevant rustdoc sections

3. **Create BID Deep Dive** (0.5 days)
   - [ ] What are BIDs (UUID-based stable identifiers)
   - [ ] Why they matter (forward refs, cross-doc links)
   - [ ] BID lifecycle (generation, injection, resolution)
   - [ ] Usage patterns and best practices

4. **Write Comprehensive Examples and Tutorials** (1 day) - **REQUIRES ISSUE 10**
   - [ ] Verify `examples/basic_usage.rs` compiles (already fixed with `Parser::simple()`)
   - [ ] Fix code examples in `lib.rs` rustdoc (lines 220-310) - must compile
   - [ ] `basic_parsing.md` - Step-by-step tutorial (reference `examples/basic_usage.rs`)
   - [ ] `querying.md` - Graph query patterns
   - [ ] `custom_codec.md` - Full codec implementation
   - [ ] `file_watching.md` - Live sync example (requires daemon examples from Issue 10)
   - [ ] `database_integration.md` - Persistence patterns (requires daemon examples from Issue 10)
   - [ ] All code examples must compile and run (critical for standalone repo)
   - [ ] Each tutorial doc links to corresponding rustdoc sections

5. **Create FAQ** (0.5 days)
   - [ ] General: What is noet used for?
   - [ ] Technical: Multi-pass compilation, unresolved refs
   - [ ] Performance: Parsing speed, memory usage
   - [ ] Common patterns and solutions

6b. **Complete Documentation Navigation** (0.25 days)
   - [ ] Enhance `rust_core/crates/core/README.md`:
     - Add comprehensive "Documentation" section linking to all docs
     - Link to rustdoc (`cargo doc`), tutorial docs, design docs
   - [ ] Add navigation to tutorial docs:
     - Header linking back to main README and rustdoc
     - Footer linking to related tutorial docs
     - Consistent cross-linking strategy
   - [ ] Ensure seamless navigation between rustdoc and manual documentation

## Testing Requirements

### Soft Open Source Requirements ✅ ALL MET
- [x] `cargo build --all-features` works in `rust_core/crates/core/` directory isolation (33.83s)
- [x] `cargo test --all-features` passes (all tests passing, including doc tests - fixed 2025-01-18)
- [x] `cargo doc` passes (3 warnings acceptable)
- [x] No dependencies on product crates in `Cargo.toml`
- [x] `examples/basic_usage.rs` compiles and runs
- [x] Basic documentation exists and is readable
- [x] No product-specific references in migrated docs
- [x] Documentation is self-contained (no cross-repo references)
- [x] `.gitignore` suitable for open-source Rust project (updated 2025-01-18)

### Full Completion Requirements (post-soft-open-source)
- [ ] All code examples compile and run (including comprehensive tutorials)
- [ ] Cross-references resolve correctly (test by following links manually)
- [ ] Links between docs work correctly (relative paths only)
- [ ] Beginner can follow tutorials successfully
- [ ] Navigation between rustdoc and manual docs is clear and bidirectional
- [ ] All examples from Issue 10 integrated and documented

## Success Criteria

### Soft Open Source Success Criteria ✅ ALL MET (2025-01-18)
- [x] `cargo build --all-features` works standalone (no product dependencies)
- [x] `cargo test --all-features` passes (all tests including doc tests - fixed 2025-01-18)
- [x] `cargo doc` passes (3 warnings acceptable)
- [x] `beliefset_architecture.md` migrated with LatticeService/product references removed
- [x] Basic `docs/architecture.md` explains core concepts
- [x] Core README clearly explains library purpose
- [x] `examples/basic_usage.rs` compiles and runs
- [x] No product-specific references (no LatticeService, Intention Lattice)
- [x] Documentation self-contained (no cross-repo references)
- [x] `.gitignore` comprehensive and suitable for open-source project
- [x] Repository made public (2025-01-18): https://gitlab.com/buildonomy/noet-core

**STATUS**: ✅ SOFT OPEN SOURCE RELEASED (2025-01-18)

### Full Completion Success Criteria (post-soft-open-source)
- [ ] Comprehensive codec tutorial enables custom format implementation
- [ ] Examples cover common use cases (all code examples compile)
- [ ] FAQ answers frequent questions
- [ ] Tutorial docs reference Issue 10 daemon examples
- [ ] Clear library/product boundary established
- [ ] Core library README links to all documentation
- [ ] Shared design README directs developers to core library docs
- [ ] Seamless navigation between rustdoc and manual documentation
- [ ] `lib.rs` examples compile and run

## Risks

**Risk**: Accidentally exposing product details  
**Mitigation**: Review checklist - no LatticeService, smartphone features, Intention Lattice, sensor processing, ML inference, dwelling points, procedure execution

**Risk**: Cargo.toml still has hidden product dependencies  
**Mitigation**: Audit all dependencies carefully; test build in isolation; check workspace dependencies

**Risk**: Documentation too minimal for soft open source  
**Mitigation**: Basic architecture.md + README should be sufficient; defer comprehensive tutorials to post-soft-open-source

**Risk**: Documentation too technical for beginners  
**Mitigation**: Include step-by-step tutorials in Stage 2; explain concepts before diving into API

**Risk**: Examples become stale as code evolves  
**Mitigation**: Keep examples minimal, focus on concepts not implementation details

**Risk**: Documentation has cross-repository references after extraction  
**Mitigation**: Use only relative paths within `rust_core/crates/core/`, verify all links resolve locally

**Risk**: API changes break examples (e.g., `BeliefSetParser::new()` signature)  
**Mitigation**: Already added `Parser::simple()` convenience constructor; keep examples focused on common use cases; note API is pre-1.0 and evolving

**Risk**: Rustdoc and manual docs become out of sync  
**Mitigation**: Cross-link bidirectionally; make `cargo doc` part of testing requirements; review both during updates

## Open Questions

1. Include API reference generation (rustdoc)? (Yes, via `cargo doc`)
2. Create video tutorials? (Defer to Phase 2)
3. Visual architecture diagrams? (Nice to have, ASCII acceptable for v0.1.0)

## References

- **Soft Open Source**: ✅ COMPLETE (2025-01-17) - No blockers, ready for repository extraction
- **Full Completion**: [`ISSUE_10_DAEMON_TESTING.md`](./ISSUE_10_DAEMON_TESTING.md) - provides daemon examples for comprehensive tutorials
- **Roadmap Context**: [`ROADMAP_NOET-CORE_v0.1.md`](./ROADMAP_NOET-CORE_v0.1.md) - overall open source preparation plan
- **Future Work**: [`ROADMAP_HTML_RENDERING.md`](./ROADMAP_HTML_RENDERING.md) - will be part of extracted repo
- **Primary Source**: `src/lib.rs` - extensive rustdoc with overview, prior art comparison, multi-pass compilation
- **Migrate**: `docs/design/beliefset_architecture.md` → `rust_core/crates/core/docs/design/beliefset_architecture.md`
  - Remove Section 3.5 "LatticeService" (lines 550-679) - product-specific
  - Remove Section 4 "Relationship to Intention Lattice" (lines 681-708) - product-specific
- **Extract from**: 
  - `lib.rs` rustdoc (primary source for architecture, capabilities, examples)
  - Migrated `beliefset_architecture.md` (specification details)
- **Update**: `docs/design/README.md` (add pointer to core library docs)
- **Note**: `docs/design/README.md` philosophy already captured in `rust_core/crates/core/AGENTS.md`
- **Pattern**: Other Rust libraries (tokio, serde) for doc structure and rustdoc integration
- **Examples**: 
  - `examples/basic_usage.rs` - now compiles with `Parser::simple()`
  - `lib.rs` examples (lines 220-310) - need fixing to compile
- **Parser API**: `src/codec/parser.rs` - `Parser::simple()` added as convenience constructor

## Stage 1 Completion Notes

**Completed**: 2025-01-18  
**Public Release**: 2025-01-18  
**Repository**: https://gitlab.com/buildonomy/noet-core

**Files Created**:
- `rust_core/crates/core/docs/design/beliefset_architecture.md` (747 lines)
- `rust_core/crates/core/docs/architecture.md` (275 lines)
- `rust_core/crates/core/docs/project/DOCUMENTATION_STRATEGY.md` (320 lines)

**Files Modified**:
- `rust_core/crates/core/README.md` (complete rewrite for library focus)
- `rust_core/crates/core/src/lib.rs` (refactored to ~110 lines, concise "Getting Started" guide; doc tests fixed 2025-01-18)
- `docs/design/README.md` (added noet-core library section)
- `.gitignore` (comprehensive open-source Rust project template - 2025-01-18)

**Doc Test Fixes (2025-01-18)**:
- Fixed missing semicolon in Basic Parsing example
- Changed tokio runtime to `current_thread` flavor (works without `rt-multi-thread` feature)
- Added missing context and imports to standalone examples
- Removed non-existent `ParseDiagnostic::SinkDependency` variant
- Marked examples as `no_run` to prevent file system dependencies
- **Result**: All 4 doc tests passing, 2 ignored (as expected)

**Known Issues (Deferred to Stage 2)**:
- 3 documentation warnings for private item links in `src/codec/mod.rs` (acceptable)

**Milestone Achieved**: 🎉 Soft open source release completed

**Next Steps**:
1. ✅ Repository extraction - COMPLETE
2. ✅ Make repository public without announcement - COMPLETE (2025-01-18)
3. Complete Issue 10 (Daemon Testing & CLI)
4. Complete Issue 5 Stage 2 (Comprehensive Documentation)
5. Public announcement (Issue 11) after Stage 2 complete
