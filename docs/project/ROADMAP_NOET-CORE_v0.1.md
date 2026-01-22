# noet-core v0.1.0 Roadmap

**Status**: In Progress  
**Target**: v0.1.0 - Public Announcement & crates.io Release  
**Created**: 2025-01-17  
**Updated**: 2025-01-20

**Parent Document**: [`ROADMAP.md`](./ROADMAP.md) - Main planning document with full backlog

## Overview

This roadmap covers the path from current state to v0.1.0 public announcement. It includes a **soft open source phase** where the repository is made public without announcement, followed by feature completion and official release.

**Two-Stage Approach**:
1. **Soft Open Source** (1 week): Minimal docs, make repo public, no announcement
2. **v0.1.0 Announcement** (3-4 weeks): Feature complete, crates.io, public announcement

**Philosophy**: Quality over speed. A polished first release builds trust and adoption.

---

## Pre-Soft-Open-Source Phase

**Goal**: Make repository public with minimal viable documentation

**Blocker**: [`ISSUE_05_DOCUMENTATION.md`](./ISSUE_05_DOCUMENTATION.md) (3-4 days)

### Issue 5: Core Library Documentation (Minimal Version)

**Deliverables**:
- [x] Migrate `beliefset_architecture.md` from `docs/design/` to `rust_core/crates/core/docs/design/`
- [x] Remove all product-specific references (LatticeService, Intention Lattice)
- [x] Create basic `docs/architecture.md` (core concepts overview)
- [ ] Create basic `docs/codecs.md` (DocCodec trait explanation)
- [ ] Create basic `docs/ids_and_refs.md` (BID system, NodeKey)
- [x] Update `rust_core/crates/core/README.md`:
  - Explain library purpose
  - Link to documentation
  - Basic usage example
  - Installation instructions
- [x] Clean up `Cargo.toml`:
  - Remove dependencies on product crates
  - Verify library builds standalone
  - Check feature flags are appropriate
- [x] Verify `cargo doc` passes without errors
- [x] Verify basic examples compile (`examples/basic_usage.rs`)

**Success Criteria** (Soft Open Source Ready):
- [x] README clearly explains what noet-core is
- [x] No product-specific code or references
- [x] Cargo.toml has no product dependencies
- [x] Basic documentation exists
- [x] `cargo build` works in isolation
- [x] `cargo test` passes
- [x] `cargo doc` generates docs without errors

**Deferred to Post-Soft-Open-Source**:
- Comprehensive tutorials (can reference Issue 10 examples later)
- CLI tool documentation (Issue 10)
- Extensive FAQ
- Advanced examples

---

## Soft Open Source Checkpoint

**Action**: Make `noet-core` repository public on GitLab

**What This Means**:
- Repository is public and clonable
- No announcement (no blog post, no This Week in Rust, no social media)
- Not published to crates.io
- Breaking changes still acceptable
- Used for early feedback from trusted users

**Communications**:
- Can share repo link privately with interested developers
- Can respond to direct inquiries
- No proactive marketing

**Timeline**: 3-5 days  
**Dependencies**: Issue 5 complete

**Deliverables**:
- [x] Extract `rust_core/crates/core/` to standalone repository
- [ ] Set up CI/CD pipeline (GitLab CI or GitHub Actions)
  - [ ] Test on Linux, macOS, Windows
  - [ ] Multiple Rust versions
  - [ ] Documentation generation
  - [ ] Example verification
  - [x] Security scanning (SAST, secret detection)
- [x] Add license headers to all files (MIT/Apache-2.0)
- [x] Create `CONTRIBUTING.md`
- [x] Create `CHANGELOG.md`
- [x] Verify repository builds from clean checkout

**Success Criteria**:
- [ ] CI/CD green on all platforms (currently only security scanning)
- [x] Repository self-contained (no external dependencies)
- [x] License compliance verified
- [x] Contributing guidelines clear

**Status**: ✅ **SOFT OPEN SOURCE COMPLETE** (2025-01-20)
- Repository is public on GitLab
- Basic CI/CD configured (security only)
- **Next**: Complete comprehensive CI/CD configuration

---

## Post-Soft-Open-Source Phase

**Goal**: Complete features for v0.1.0 announcement

**Dependencies**: Soft open source complete

### Phase 1: CLI and Daemon

**Timeline**: 2-3 days  
**Issue**: [`ISSUE_10_DAEMON_TESTING.md`](./ISSUE_10_DAEMON_TESTING.md)

**Deliverables**:
- [ ] Migrate `compiler.rs` → `daemon.rs`
- [ ] Rename `LatticeService` → `DaemonService`
- [ ] Create `bin/noet.rs` with subcommands:
  - `noet parse <path>` - one-shot parsing with diagnostics
  - `noet watch <path>` - continuous foreground parsing
- [ ] Test `FileUpdateSyncer` file watching integration
- [ ] Test database synchronization
- [ ] Tutorial documentation with doctests in `daemon.rs` module
- [ ] Complete `examples/daemon.rs` demonstrating full orchestration
- [ ] Update Issue 5 docs to include CLI/daemon examples

**Success Criteria**:
- [ ] CLI tool works: `noet parse` and `noet watch`
- [ ] Daemon patterns tested and documented
- [ ] Tutorial docs with working doctests
- [ ] Examples referenced in Issue 5 documentation

---

### Phase 2: HTML Rendering

**Timeline**: 2-3 weeks  
**Issues**: 1-4, 6, 7  
**Roadmap**: [`ROADMAP_HTML_RENDERING.md`](./ROADMAP_HTML_RENDERING.md)

**Goal**: Migrate to clean frontmatter + title-based anchors for universal markdown rendering

**Why Critical**: Documents must render cleanly in GitHub/GitLab/Obsidian for compelling value proposition

#### Issue 1: Schema Registry (3-4 days)
- [ ] Refactor to singleton pattern (matches `CodecMap`)
- [ ] Enable downstream schema registration
- [ ] Blocks Issues 2, 3, 4

#### Issue 2: Multi-Node TOML Parsing (4-5 days)
- [ ] Parse frontmatter with `sections` map
- [ ] Match section metadata to heading-generated nodes
- [ ] Apply schema-typed payloads to sections
- [ ] Requires Issue 1

#### Issue 3: Heading Anchors (2-3 days)
- [ ] Parse title-based anchors: `{#introduction}`
- [ ] Track BID-to-anchor mappings internally
- [ ] Generate title slugs automatically
- [ ] Cross-renderer compatibility (GitHub, GitLab, Obsidian)

#### Issue 4: Link Manipulation (3-4 days)
- [ ] Parse NodeKey link attributes: `[text](./path.md){#bid://abc123}`
- [ ] Generate relative paths from NodeKey resolution
- [ ] Auto-update paths when targets move
- [ ] Requires Issues 1, 2, 3

#### Issue 6: HTML Generation (covered in ROADMAP_HTML_RENDERING.md)
- [ ] Extend `DocCodec` with `generate_html()` method
- [ ] Implement for `MdCodec`
- [ ] Create viewer script and CSS
- [ ] Data attributes for BIDs and NodeKeys

#### Issue 7: HTML CLI Integration (2-3 days)
- [ ] Add `--html <output_dir>` to `noet parse`
- [ ] Add `--html <output_dir>` to `noet watch`
- [ ] Integrate HTML generation into `FileUpdateSyncer`
- [ ] Preserve directory structure in output
- [ ] Optional live reload server (`--serve`)
- [ ] Requires Issues 6 and 10

**Success Criteria**:
- [ ] Documents render cleanly in GitHub/GitLab/Obsidian
- [ ] No visible YAML blocks
- [ ] BID system working with clean anchors
- [ ] HTML generation functional via CLI
- [ ] Migration tool for existing documents

---

### Phase 3: Code Quality & Testing

**Timeline**: 1 week  
**Dependencies**: Phases 1-2 complete

**Deliverables**:
- [ ] Comprehensive unit test suite
- [ ] Integration tests for all major workflows
- [ ] Property-based testing for parser
- [ ] Fuzzing for codec implementations
- [ ] Performance benchmarks established
- [ ] Memory leak detection tests
- [ ] Documentation coverage check
- [ ] Example verification (all examples compile and run)

**Success Criteria**:
- [ ] Test coverage > 80%
- [ ] All tests passing consistently
- [ ] No memory leaks detected
- [ ] Performance baselines established
- [ ] CI runs successfully

---

### Phase 4: Publication & Announcement

**Timeline**: 2-3 days  
**Dependencies**: Phase 4 complete

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
Issue 5 (Minimal Docs)
    ↓
SOFT OPEN SOURCE
    ↓
Issue 10 (CLI/Daemon)
    ↓
Issue 1 (Schema Registry)
    ↓
Issues 2, 3 (parallel: TOML Parsing + Heading Anchors)
    ↓
Issue 4 (Link Manipulation)
    ↓
Issue 6 (HTML Generation)
    ↓
Issue 7 (HTML CLI Integration)
    ↓
Phase 3 (Testing)
    ↓
Phase 4 (Infrastructure)
    ↓
Phase 5 (Publication)
    ↓
v0.1.0 ANNOUNCEMENT
```

**Parallel Work Opportunities**:
- Issues 2 and 3 can proceed in parallel after Issue 1
- Phase 3 testing can start as soon as features stabilize
- Phase 4 infrastructure setup can begin during Phase 3

---

## Success Metrics

### Soft Open Source Complete When:
- [ ] Issue 5 complete (minimal docs)
- [ ] Cargo.toml clean (no product dependencies)
- [ ] README explains library
- [ ] Basic examples compile
- [ ] `cargo doc` passes

### v0.1.0 Release Complete When:
- [ ] All phases 1-5 complete
- [ ] All tests passing on all platforms
- [ ] Documentation comprehensive and clear
- [ ] HTML rendering working correctly
- [ ] CLI tool functional
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

### Soft Open Source Timing
**Decision**: After Issue 5 complete, before Issues 10, 1-4  
**Rationale**: Minimal viable documentation enables early feedback; can iterate based on response  
**Alternative Considered**: Wait until all features complete (rejected - too long without feedback)

### HTML Rendering in v0.1.0 or defer?
**Decision**: Include in v0.1.0 (Issues 1-4, 6, 7)  
**Rationale**: Clean rendering is essential for value proposition  
**Alternative Considered**: Defer to v0.2.0 (rejected - too important for first impression)

### CLI Tool in v0.1.0 or defer?
**Decision**: Include in v0.1.0 (Issue 10)  
**Rationale**: `noet parse` and `noet watch` are core workflows; enables HTML generation  
**Alternative Considered**: Library-only for v0.1.0 (rejected - less useful without tooling)

---

## Post-v0.1.0 Roadmap

See [`ROADMAP.md`](./ROADMAP.md) for:
- **v0.2.0**: LSP Integration (Issue 11)
- **v0.3.0**: Advanced LSP Features (Issue 12)
- **v0.4.0+**: Future enhancements

---

## References

- **Main Roadmap**: [`ROADMAP.md`](./ROADMAP.md) - Full backlog and version planning
- **HTML Rendering Details**: [`ROADMAP_HTML_RENDERING.md`](./ROADMAP_HTML_RENDERING.md)
- **Agent Guidelines**: [`../../AGENTS.md`](../../AGENTS.md)
- **Issues**:
  - [`ISSUE_05_DOCUMENTATION.md`](./ISSUE_05_DOCUMENTATION.md) - BLOCKS soft open source
  - [`ISSUE_10_DAEMON_TESTING.md`](./ISSUE_10_DAEMON_TESTING.md) - CLI and daemon
  - [`ISSUE_01_SCHEMA_REGISTRY.md`](./ISSUE_01_SCHEMA_REGISTRY.md) - Schema refactor
  - [`ISSUE_02_MULTINODE_TOML_PARSING.md`](./ISSUE_02_MULTINODE_TOML_PARSING.md) - Frontmatter
  - [`ISSUE_03_HEADING_ANCHORS.md`](./ISSUE_03_HEADING_ANCHORS.md) - Title anchors
  - [`ISSUE_04_LINK_MANIPULATION.md`](./ISSUE_04_LINK_MANIPULATION.md) - Link rewriting
  - [`ISSUE_07_HTML_CLI_INTEGRATION.md`](./ISSUE_07_HTML_CLI_INTEGRATION.md) - HTML flags

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

**Current Status**: Post-soft-open-source (repository public, basic CI/CD configured)  
**Next Step**: Complete comprehensive CI/CD configuration, then proceed with Issue 10  
**Completed**: 2025-01-20 - Soft open source release  
**Estimated to v0.1.0 Announcement**: 4-5 weeks from soft open source (mid-February 2025)
