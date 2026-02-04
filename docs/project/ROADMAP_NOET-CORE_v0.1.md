# noet-core v0.1.0 Roadmap

**Status**: Phase 1-3 Complete, Phase 2 (Issue 6) In Progress  
**Target**: v0.1.0 - Public Announcement & crates.io Release  
**Created**: 2025-01-17  
**Updated**: 2025-01-28

**Parent Document**: [`ROADMAP.md`](./ROADMAP.md) - Main planning document with full backlog

## Overview

This roadmap covers the path from current state to v0.1.0 public announcement. It includes a **soft open source phase** where the repository is made public without announcement, followed by feature completion and official release.

**Two-Stage Approach**:
1. **Soft Open Source** ✅ COMPLETE (2025-01-20): Minimal docs, repo public, no announcement
2. **v0.1.0 Announcement** (IN PROGRESS): Feature complete, crates.io, public announcement
   - **Current Blocker**: Issue 6 (HTML Generation)

**Philosophy**: Quality over speed. A polished first release builds trust and adoption.

---

## Pre-Soft-Open-Source Phase ✅ COMPLETE (2025-01-24)

**Goal**: Make repository public with minimal viable documentation

**[Issue 5: Core Library Documentation](./completed/ISSUE_05_DOCUMENTATION.md)** ✅ COMPLETE

**Completed Deliverables**:
- [x] Migrated `beliefbase_architecture.md` from `docs/design/`
- [x] Removed all product-specific references
- [x] Created comprehensive architecture documentation
- [x] Updated README with clear library purpose and examples
- [x] Cleaned up `Cargo.toml` (no product dependencies)
- [x] Verified `cargo doc` passes without errors
- [x] Verified basic examples compile

**Success Criteria** ✅ ALL MET:
- [x] README clearly explains what noet-core is
- [x] No product-specific code or references
- [x] Cargo.toml has no product dependencies
- [x] Basic documentation exists
- [x] `cargo build` works in isolation
- [x] `cargo test` passes
- [x] `cargo doc` generates docs without errors

---

## Soft Open Source Checkpoint ✅ COMPLETE (2025-01-20)

**Action**: Make `noet-core` repository public on GitLab

**Completed**:
- [x] Repository is public and clonable
- [x] No announcement made
- [x] Not published to crates.io (deferred to Phase 5)
- [x] Breaking changes still acceptable
- [x] Available for early feedback from trusted users

**Completed Deliverables**:
- [x] Extracted `rust_core/crates/core/` to standalone repository
- [x] Set up basic CI/CD pipeline (security scanning)
- [x] Added license headers to all files (MIT/Apache-2.0)
- [x] Created `CONTRIBUTING.md`
- [x] Created `CHANGELOG.md`
- [x] Verified repository builds from clean checkout

**Remaining CI/CD Work** (moved to Phase 4):
- [ ] Comprehensive CI/CD (Linux, macOS, Windows, multiple Rust versions)
- [ ] Documentation generation in CI
- [ ] Example verification in CI

---

## Post-Soft-Open-Source Phase

**Goal**: Complete features for v0.1.0 announcement

**Dependencies**: Soft open source complete ✅

### Phase 1: CLI and Watch Service ✅ COMPLETE (2025-01-24)

**[Issue 10: Daemon Testing & Library Pattern Extraction](./completed/ISSUE_10_DAEMON_TESTING.md)** ✅ COMPLETE

**Completed Deliverables**:
- [x] Migrated `compiler.rs` → `watch.rs`
- [x] Renamed `LatticeService` → `WatchService`
- [x] Created `bin/noet.rs` with subcommands:
  - `noet parse <path>` - one-shot parsing with diagnostics
  - `noet watch <path>` - continuous foreground parsing
- [x] Tested `FileUpdateSyncer` file watching integration
- [x] Tested database synchronization
- [x] Tutorial documentation with 4 doctests in `watch.rs` module (240+ lines)
- [x] Completed `examples/watch_service.rs` demonstrating full orchestration (432 lines)
- [x] Updated Issue 5 docs with CLI/service examples

**Success Criteria** ✅ ALL MET:
- [x] CLI tool works: `noet parse` and `noet watch`
- [x] Watch service patterns tested and documented
- [x] Tutorial docs with working doctests
- [x] Examples referenced in Issue 5 documentation

---

### Phase 2: HTML Rendering (PARTIALLY COMPLETE, ISSUE 6 IN PROGRESS)

**Roadmap**: [`ROADMAP_HTML_RENDERING.md`](./ROADMAP_HTML_RENDERING.md)

**Goal**: Migrate to clean frontmatter + title-based anchors for universal markdown rendering

**Why Critical**: Documents must render cleanly in GitHub/GitLab/Obsidian for compelling value proposition

#### Parsing & Data Model ✅ COMPLETE (2025-01-28)

**[Issue 1: Schema Registry](./completed/ISSUE_01_SCHEMA_REGISTRY.md)** ✅ COMPLETE
- [x] Refactored to singleton pattern (matches `CodecMap`)
- [x] Enabled downstream schema registration

**[Issue 2: Multi-Node TOML Parsing](./completed/ISSUE_02_MULTINODE_TOML_PARSING.md)** ✅ COMPLETE
- [x] Parse frontmatter with `sections` map
- [x] Match section metadata to heading-generated nodes
- [x] Apply schema-typed payloads to sections

**[Issue 3: Heading Anchors](./completed/ISSUE_03_HEADING_ANCHORS.md)** ✅ COMPLETE
- [x] Parse title-based anchors: `{#introduction}`
- [x] Track BID-to-anchor mappings internally
- [x] Generate title slugs automatically
- [x] Cross-renderer compatibility (GitHub, GitLab, Obsidian)

**[Issue 4: Link Manipulation](./completed/ISSUE_04_LINK_MANIPULATION.md)** ✅ COMPLETE
- [x] Parse NodeKey link attributes: `[text](./path.md){#bid://abc123}`
- [x] Generate relative paths from NodeKey resolution
- [x] Auto-update paths when targets move

**[Issue 21: JSON/TOML Dual-Format Support](./completed/ISSUE_21_JSON_FALLBACK_PARSING.md)** ✅ COMPLETE
- [x] JSON as default format (cross-platform compatibility)
- [x] Support both BeliefNetwork.json and BeliefNetwork.toml
- [x] Network configuration schema for repo-wide format preferences
- [x] Bidirectional JSON/TOML conversion

**[Issue 20: CLI Write-Back Support](./completed/ISSUE_20_CLI_WRITE_BACK.md)** ✅ COMPLETE
- [x] Implemented `noet write` command for updating documents
- [x] Supports frontmatter injection and metadata updates
- [x] Enables migration workflows

#### HTML Generation - **CURRENT WORK, BLOCKING v0.1.0**

**[Issue 6: HTML Generation](./ISSUE_06_HTML_GENERATION.md)** (8-10 days, HIGH) - **IN PROGRESS**
- [ ] Extend `DocCodec` trait with `generate_html()` method
- [ ] Implement HTML generation for `MdCodec`
- [ ] Create JavaScript viewer script for interactive features
- [ ] Create CSS stylesheet for noet documents
- [ ] NodeKey anchor resolution in browser
- [ ] Documentation and examples

**[Issue 13: HTML CLI Integration](./ISSUE_13_HTML_CLI_INTEGRATION.md)** (2-3 days)
- [ ] Add `--html <output_dir>` to `noet parse`
- [ ] Add `--html <output_dir>` to `noet watch`
- [ ] Integrate HTML generation into `FileUpdateSyncer`
- [ ] Preserve directory structure in output
- [ ] Optional live reload server (`--serve`)
- [ ] Requires Issue 6 complete

**Success Criteria**:
- [x] Documents render cleanly in GitHub/GitLab/Obsidian (parsing complete)
- [x] No visible YAML blocks (parsing complete)
- [x] BID system working with clean anchors (parsing complete)
- [ ] HTML generation functional via CLI (**BLOCKED ON ISSUE 6**)
- [x] Migration tool for existing documents (`noet write` complete)

---

### Phase 3: Code Quality & Testing (MOSTLY COMPLETE)

**Dependencies**: Phases 1-2 complete

**Completed Testing Work** ✅:
- **[Issue 22: Duplicate Node Deduplication](./completed/ISSUE_22_DUPLICATE_NODE_DEDUPLICATION.md)** ✅ COMPLETE
  - Fixed duplicate node handling in parser
  - Added deduplication logic
  - All tests passing

- **[Issue 23: Integration Test Convergence](./completed/ISSUE_23_INTEGRATION_TEST_CONVERGENCE.md)** ✅ COMPLETE
  - Comprehensive integration test suite
  - 7 integration tests passing, 1 ignored (documented)
  - Testing across major workflows
  - Database synchronization verified

**Remaining Work**:
- **[Issue 7: Comprehensive Testing](./ISSUE_07_COMPREHENSIVE_TESTING.md)** (5-7 days) - OPTIONAL for v0.1.0
  - Property-based testing for builder
  - Fuzzing for codec implementations
  - Performance benchmarks
  - Memory leak detection
  - Documentation coverage check

**Current Status**:
- [x] Comprehensive unit test suite (via Issues 1-4, 10, 20-23)
- [x] Integration tests for all major workflows (Issue 23)
- [x] All tests passing consistently
- [ ] Property-based testing (deferred, optional)
- [ ] Fuzzing (deferred, optional)
- [ ] Performance benchmarks (deferred, optional)

---

### Phase 4: Repository & Infrastructure ✅ MOSTLY COMPLETE (2025-01-20)

**[Issue 8: Repository Setup](./ISSUE_08_REPOSITORY_SETUP.md)** ✅ COMPLETE

**Completed Deliverables**:
- [x] Extracted repository from monorepo
- [x] Set up basic CI/CD (security scanning)
- [x] License headers added (MIT/Apache-2.0)
- [x] Contributing guidelines created
- [x] Repository is public on GitLab

**Remaining Work** (optional for v0.1.0, can defer):
- [ ] Complete comprehensive CI/CD (Linux, macOS, Windows, multiple Rust versions)
- [ ] Configure crates.io publishing workflow in CI

**Status**: Basic infrastructure complete, comprehensive CI/CD optional

---

### Phase 5: Publication & Announcement - **BLOCKED ON ISSUE 6**

**Timeline**: 2-3 days  
**Dependencies**: Issue 6 complete, Issue 13 complete

**[Issue 9: Crates.io Release](./ISSUE_09_CRATES_IO_RELEASE.md)** (2-3 days)

**Deliverables**:
- [ ] Configure crates.io publishing
- [ ] Add badges to README (CI status, crates.io, docs.rs)
- [ ] Publish v0.1.0 to crates.io
- [ ] Verify crates.io page looks correct
- [ ] Verify docs.rs builds successfully
- [ ] Write announcement blog post
- [ ] Prepare This Week in Rust submission
- [ ] Prepare social media announcements
- [ ] Submit to This Week in Rust
- [ ] Post announcements
- [ ] Monitor feedback and respond to issues

**Success Criteria**:
- [ ] Published to crates.io successfully
- [ ] docs.rs shows documentation correctly
- [ ] This Week in Rust submission accepted
- [ ] Initial community feedback positive
- [ ] No critical bugs reported in first 48 hours

---

## Critical Path

```
✅ Issue 5 (Minimal Docs)
    ↓
✅ SOFT OPEN SOURCE (2025-01-20)
    ↓
✅ Issue 10 (CLI/Daemon)
    ↓
✅ Issue 1 (Schema Registry)
    ↓
✅ Issues 2, 3, 21 (TOML Parsing + Heading Anchors + JSON Support)
    ↓
✅ Issue 4 (Link Manipulation)
    ↓
✅ Issue 20 (CLI Write-Back)
    ↓
✅ Issues 22, 23 (Testing Improvements)
    ↓
→ Issue 6 (HTML Generation) ← **YOU ARE HERE, BLOCKING v0.1.0**
    ↓
→ Issue 13 (HTML CLI Integration)
    ↓
→ Phase 5 (Publication - Issue 9)
    ↓
v0.1.0 ANNOUNCEMENT
```

**Completed** ✅: Issues 1-5, 10, 20-23
**In Progress**: Issue 6 (HTML Generation)
**Blocked**: Issue 13 (requires Issue 6), Issue 9 (requires Issue 6 + 13)

---

## Success Metrics

### Soft Open Source Complete ✅ (2025-01-20):
- [x] Issue 5 complete (minimal docs)
- [x] Cargo.toml clean (no product dependencies)
- [x] README explains library
- [x] Basic examples compile
- [x] `cargo doc` passes

### v0.1.0 Release Complete When:
- [x] Phase 1 complete (Issue 10) ✅
- [x] Phase 2 parsing complete (Issues 1-4, 20-21) ✅
- [ ] Phase 2 HTML generation complete (Issues 6, 13) ← **BLOCKING**
- [x] Phase 3 testing mostly complete (Issues 22-23) ✅
- [x] Phase 4 infrastructure complete (Issue 8) ✅
- [ ] Phase 5 publication complete (Issue 9)
- [x] All tests passing
- [x] Documentation comprehensive and clear
- [ ] HTML generation working correctly ← **BLOCKING**
- [x] CLI tool functional (`noet parse`, `noet watch`)
- [ ] Published to crates.io
- [ ] Announcement posted

### Post-Release Health Indicators:
- Crates.io downloads > 100 in first month
- GitHub/GitLab stars > 50 in first month
- No critical bugs reported
- Positive community sentiment
- Questions answered within 48 hours
- At least one external contribution

---

## Risks & Mitigations

### Risk: Documentation incomplete before soft open source
**Mitigation**: Issue 5 scoped to minimal viable docs; comprehensive docs post-soft-release

### Risk: HTML rendering complexity causes delays
**Mitigation**: Issues 1-4 are well-scoped; can defer Issue 7 features if needed

### Risk: Platform-specific bugs discovered late
**Mitigation**: CI testing on multiple platforms in Phase 4; manual testing earlier

### Risk: API design flaws discovered during documentation
**Mitigation**: Soft open source allows feedback before announcement; pre-1.0 allows breaking changes

### Risk: Negative community reception
**Mitigation**: High-quality documentation; responsive to feedback; clear roadmap for future

### Risk: Critical bug found immediately after publication
**Mitigation**: Comprehensive testing in Phase 3; soft open source period for early detection

---

## Decision Points

### Soft Open Source Timing ✅
**Decision**: After Issue 5 complete, before Issues 10, 1-4  
**Rationale**: Minimal viable documentation enables early feedback; can iterate based on response  
**Outcome**: Successful - repository public since 2025-01-20, no critical issues discovered

### HTML Rendering in v0.1.0 or defer? ✅
**Decision**: Include in v0.1.0 (Issues 1-4, 6, 13)  
**Rationale**: Clean rendering is essential for value proposition  
**Status**: Parsing complete (Issues 1-4), generation in progress (Issue 6)

### CLI Tool in v0.1.0 or defer? ✅
**Decision**: Include in v0.1.0 (Issue 10)  
**Rationale**: `noet parse` and `noet watch` are core workflows; enables HTML generation  
**Status**: Complete (Issue 10)

---

## Post-v0.1.0 Roadmap

See [`ROADMAP.md`](./ROADMAP.md) for future versions:
- **v0.2.0**: IDE Integration (LSP) - Issue 11
- **v0.3.0**: Advanced LSP Features - Issue 12
- **v0.4.0**: Multi-Device Sync & Collaboration - Issue 16
- **v0.5.0+**: Future enhancements

### Technical Debt

**Two-Phase WASM Compilation** (Low Priority, Post-v0.1.0)

**Current State**: Building `noet` with full features (`bin` + `service` + WASM) requires two-phase compilation:
1. Build WASM module: `wasm-pack build --target web --out-dir pkg -- --features wasm --no-default-features`
2. Build CLI binary: `cargo build --features "bin service"`

This is handled by `./scripts/build-full.sh` but is not ergonomic for end users.

**Root Cause**: The `wasm` and `service` features are mutually exclusive (WASM can't have filesystem/SQLite/tokio runtime). When `build.rs` calls `wasm-pack`, it inherits parent build's features, causing conflicts.

**Potential Solutions**:
1. **Split into two crates** (recommended for v0.2.0+):
   - `noet-core-wasm` - WASM-only crate with pre-built artifacts
   - `noet-core` - Main crate, depends on `noet-core-wasm` when `bin` feature enabled
   - Cleanest solution, no build.rs hacks
   - Similar to how `sqlx` separates `sqlx-macros`

2. **Use cargo-make or Just**:
   - Orchestrate builds with external tool
   - Adds tooling dependency

3. **Pre-build WASM and check into git**:
   - Include `pkg/` in repository/crates.io package
   - Simple but increases repo size

**Decision**: Defer to post-v0.1.0. Current workaround (`./scripts/build-full.sh`) is acceptable for development. For crates.io publication, pre-built WASM can be included in package.

**Impact**: Low - only affects developers building from source with full features. End users installing via `cargo install noet` will get pre-built artifacts from crates.io package.

---

## References

- **Main Roadmap**: [`ROADMAP.md`](./ROADMAP.md) - Full backlog and version planning
- **HTML Rendering Details**: [`ROADMAP_HTML_RENDERING.md`](./ROADMAP_HTML_RENDERING.md)
- **Agent Guidelines**: [`../../AGENTS.md`](../../AGENTS.md)
- **Completed Issues**: See [`completed/`](./completed/) directory
- **Active Issues**:
  - [`ISSUE_06_HTML_GENERATION.md`](./ISSUE_06_HTML_GENERATION.md) - **CURRENT WORK, BLOCKING v0.1.0**
  - [`ISSUE_13_HTML_CLI_INTEGRATION.md`](./ISSUE_13_HTML_CLI_INTEGRATION.md) - Requires Issue 6
  - [`ISSUE_09_CRATES_IO_RELEASE.md`](./ISSUE_09_CRATES_IO_RELEASE.md) - Publication

---

## Notes

### Timeline Note
Timelines are relative comparisons between issues, not absolute commitments. AI-assisted development increases timeline uncertainty. Focus on quality and completeness over speed.

### Soft Open Source Philosophy
Making the repository public without announcement allows:
- Early feedback from trusted developers
- Iteration on APIs without public commitments
- Testing assumptions about use cases
- Building confidence before official announcement

### Why Dual MIT/Apache-2.0?
Standard in Rust ecosystem; maximizes compatibility with other projects; users can choose license that fits their needs.

### Repository Extraction Process
1. Copy `rust_core/crates/core/` to new repository
2. Clean git history (optional - keep or squash)
3. Add CI/CD configuration
4. Verify builds independently
5. Set up crates.io publishing
6. Make public (soft open source)
7. Continue development in new repo

---

**Current Status**: Phase 1-3 mostly complete, Issue 6 (HTML Generation) in progress  
**Next Step**: Complete Issue 6 (HTML Generation) - this is blocking v0.1.0 announcement  
**Soft Open Source**: ✅ 2025-01-20  
**Target v0.1.0 Announcement**: After Issue 6 + Issue 13 complete (estimated 2-3 weeks)
