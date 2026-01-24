# noet-core Development Backlog

**Purpose**: This document tracks all planned work for noet-core, organized by version milestone. It establishes the versioning strategy and explains how issues map to releases.

## Versioning Strategy

### Release Philosophy

**Pre-v0.1.0 - Soft Open Source**
- Repository made public on GitLab
- No announcement or marketing
- May have breaking changes without notice
- Used for early feedback from trusted users
- Not yet on crates.io

**v0.1.0 - Announcement Release**
- First public announcement (This Week in Rust, social media, etc.)
- Published to crates.io
- "Feature complete" for core library functionality
- Production-ready API (semantic versioning starts here)
- Comprehensive documentation
- Working examples

**v0.2+.0+ - Enhancement Releases**
- No breaking changes to public API (semver)
- New features (LSP, advanced tooling, etc.)
- Performance improvements
- Additional documentation

**v1.0.0 - Stability Commitment**
- Full API stability guarantee
- Long-term support (LTS)
- Battle-tested in production
- Complete feature set for intended use cases

## Current Status

**Active Development**: Pre-v0.1.0 (soft open source phase)
**Current Status**: Pre-soft-open-source

**Next Milestone**: Soft open source (make repo public, no announcement)

**Timeline Estimate**: 
- 1 week to soft open source (Issue 5 only)
- 3-4 weeks post-soft-open-source to v0.1.0 announcement

## Version Milestones

### v0.1.0 - Announcement Release (CURRENT TARGET)

**Roadmap**: [`ROADMAP_NOET-CORE_v0.1.md`](./ROADMAP_NOET-CORE_v0.1.md)

**Status**: In Progress (Phases 1-5)

**Goal**: Feature-complete core library ready for public announcement and crates.io publication.

**Blocking Issues** (must complete before announcement):

#### Phase 1: Minimal Documentation for Soft Open Source ✅ COMPLETE (2025-01-18)
- **[Issue 5: Core Library Documentation](./ISSUE_05_DOCUMENTATION.md)** ✅ COMPLETE
  - Migrated `beliefset_architecture.md` (removed product references)
  - Created basic architecture docs
  - Updated READMEs for standalone repository
  - Fixed Cargo.toml dependencies (removed product crate references)
  - **Stage 2 complete**: Issue 10 examples integrated (WatchService tutorial, 2025-01-24)

**SOFT OPEN SOURCE POINT** ✅ ACHIEVED (2025-01-18)
- Repository made public: https://gitlab.com/buildonomy/noet-core
- No announcement yet, no crates.io publication
- Early feedback from trusted users
- Breaking changes acceptable

#### Phase 1b: CLI and Daemon ✅ COMPLETE (2025-01-24)
- **[Issue 10: Daemon Testing & Library Pattern Extraction](./ISSUE_10_DAEMON_TESTING.md)** ✅ COMPLETE
  - Migrated `compiler.rs` → `watch.rs` (renamed `LatticeService` → `WatchService`)
  - Created CLI tool (`noet parse`, `noet watch`)
  - Integration tests (7 passing, 1 ignored - see Issue 19)
  - Comprehensive tutorial docs with 4 doctests (240+ lines)
  - Full orchestration example: `examples/watch_service.rs` (432 lines)
  - Threading model documented

#### Phase 2: HTML Rendering (2 weeks, POST-SOFT-OPEN-SOURCE)
- **[Issue 1: Schema Registry](./ISSUE_01_SCHEMA_REGISTRY.md)** (3-4 days, CRITICAL)
  - Refactor to singleton pattern
  - Enable downstream schema registration

- **[Issue 2: Multi-Node TOML Parsing](./ISSUE_02_MULTINODE_TOML_PARSING.md)** (4-5 days, CRITICAL)
  - Parse frontmatter with `sections` map
  - Apply schema-typed payloads to headings

- **[Issue 3: Heading Anchors](./ISSUE_03_HEADING_ANCHORS.md)** (2-3 days, CRITICAL)
  - Parse title-based anchors: `{#introduction}`
  - Track BID-to-anchor mappings internally

- **[Issue 4: Link Manipulation](./ISSUE_04_LINK_MANIPULATION.md)** (3-4 days, CRITICAL)
  - Parse NodeKey link attributes
  - Auto-update paths when targets move

- **[Issue 13: HTML CLI Integration](./ISSUE_13_HTML_CLI_INTEGRATION.md)** (2-3 days)
  - Add `--html <output_dir>` to `noet parse` and `noet watch`
  - Integrate HTML generation into FileUpdateSyncer
  - Live reload server (optional)
  - Static site generation workflow

#### Phase 3: Code Quality & Testing (1 week, POST-SOFT-OPEN-SOURCE)
- Comprehensive test suite (unit + integration)
- Property-based testing for parser
- Fuzzing for codec implementations
- Performance benchmarks
- Memory leak detection

#### Phase 4: Repository & Infrastructure (3-5 days)
- Extract repository from monorepo
- Set up CI/CD (GitLab CI or GitHub Actions)
- Configure crates.io publishing
- License headers (MIT/Apache-2.0)
- Contributing guidelines

#### Phase 5: Publication & Announcement (2-3 days)
- Publish to crates.io
- Write announcement blog post
- Submit to This Week in Rust
- Social media announcements
- Monitor initial feedback

**Soft Open Source Criteria** (Before making repo public):
- [ ] Issue 5 complete (basic documentation)
- [ ] Cargo.toml dependencies cleaned (no product crate references)
- [ ] README explains library purpose
- [ ] No product-specific code in noet-core
- [ ] Basic examples compile

**v0.1.0 Success Metrics** (Before announcement):
- [ ] All tests passing
- [ ] Documentation complete (including Issue 10 examples)
- [ ] HTML rendering feature complete (Issues 1-4, 6, 13)
- [ ] CLI tool working (`noet parse`, `noet watch`)
- [ ] CI/CD green
- [ ] Published to crates.io
- [ ] Announcement ready

---

### v0.2.0 - IDE Integration (POST-ANNOUNCEMENT)

**Status**: Planning

**Goal**: Language Server Protocol (LSP) support for IDE integration.

**Target Timeline**: 1-2 months after v0.1.0 announcement

**Issues**:

#### Core LSP Implementation
- **[Issue 11: Basic LSP](./ISSUE_11_BASIC_LSP.md)** (3-5 days, HIGH PRIORITY)
  - Add position/range tracking to parser
  - Implement LSP server with `tower-lsp`
  - Document synchronization (didOpen, didChange, didSave, didClose)
  - Real-time diagnostics
  - Hover provider (show node metadata)
  - VSCode/Zed/Neovim configuration

**Deliverables**:
- `noet lsp` command working
- Diagnostics appear in IDEs
- Hover shows BIDs and metadata
- Tested in VSCode, Zed, Neovim
- LSP documentation complete

**Success Metrics**:
- Users can edit noet documents in IDE with real-time feedback
- LSP works in at least 2 different editors
- Documentation enables easy setup

---

### v0.3.0 - Advanced LSP Features (ENHANCEMENT)

**Status**: Backlog

**Goal**: Full-featured IDE experience with navigation, completion, refactoring, and real-time collaboration.

**Target Timeline**: 2-3 months after v0.2.0

**Issues**:

#### Advanced LSP Features
- **[Issue 12: Advanced LSP](./ISSUE_12_ADVANCED_LSP.md)** (5-7 days, MEDIUM PRIORITY)
  - **Navigation**: Go to definition, find references, document outline, symbol search
  - **Editing**: Autocomplete, formatting, code actions, rename
  - **Performance**: Incremental sync, lazy parsing, debounced diagnostics
  - **Quality of Life**: Semantic tokens, inlay hints

#### Real-Time Collaboration
- **[Issue 15: Filtered Event Streaming](./ISSUE_15_FILTERED_EVENT_STREAMING.md)** (5-7 days, MEDIUM PRIORITY)
  - Query-based event subscriptions (reimagined "focus")
  - Bidirectional event streaming (client ↔ server)
  - LSP custom notifications for filtered updates
  - Multiple concurrent subscriptions with different filters
  - Efficient real-time updates for IDE extensions and dashboards

**Deliverables**:
- Click on `[[links]]` to jump to definition
- Autocomplete available references
- Format document (inject BIDs, normalize links)
- Rename symbols with automatic reference updates
- Document outline in sidebar
- Workspace-wide symbol search
- Query-based event subscriptions via LSP
- Real-time filtered updates to IDE extensions
- Bidirectional event flow (client can send updates)

**Success Metrics**:
- IDE experience comparable to programming languages
- Navigation works across documents
- Autocomplete response time < 50ms
- Rename works on 100+ document workspaces
- Filtered event routing latency < 10ms
- Multiple concurrent subscriptions work correctly

---

### v0.4.0 - Multi-Device Sync & Collaboration (BACKLOG)

**Status**: Planning

**Goal**: Distributed state synchronization and collaborative features using Automerge CRDT.

**Target Timeline**: 3-4 months after v0.3.0

**Issues**:

#### Automerge Integration
- **[Issue 16: Automerge Integration for Activity Logs and Distributed State](./ISSUE_16_AUTOMERGE_INTEGRATION.md)** (2-3 weeks, MEDIUM PRIORITY)
  - Activity log sync (user motivations, focus history)
  - Subscription state sync across devices
  - Peer-to-peer sync (mDNS, QR pairing)
  - Collaborative presence indicators
  - Offline-first workflow
  - Optional relay server for internet sync
  - Future: Keyhive authorization integration (v0.5.0+)

**Deliverables**:
- Activity logs synced across user's devices
- Subscription state synced (ISSUE_15 filters)
- Peer discovery on local network (mDNS)
- Automatic conflict resolution (CRDT)
- Collaborative presence (who's viewing/editing what)
- UI state sync (preferences, window layout)
- Offline-first sync on reconnect

**Success Metrics**:
- Sync latency < 100ms on local network
- Activity logs preserved across devices
- Conflicts resolved automatically (no user intervention)
- Works offline → syncs on reconnect

**Architecture**:
- Hybrid model: Files remain canonical (git-versioned), Automerge for ephemeral state
- Feature flag: `automerge` (optional dependency)
- Integration with ISSUE_15 (filtered event streaming)

---

### v0.5.0+ - Future Enhancements (BACKLOG)

**Status**: Ideas / Not Planned Yet

**Potential Features**:

#### Authorization & Security
- Keyhive distributed authorization (when stable)
- Capability-based access control
- Multi-user permissions

#### Language Features

- Syntax extensions (custom block types, attributes)
- Template system for document generation
- Macro system for reusable content
- Query language for complex graph queries

#### Procedural Extensions
- **[Issue 17: Extract noet-procedures Crate](./ISSUE_17_NOET_PROCEDURES_EXTRACTION.md)** (2-3 weeks, MEDIUM PRIORITY)
  - Extract procedural execution functionality to separate crate
  - Implement "as-run" tracking (template + context + execution record)
  - Provide core redline system (deviation recording, not prediction)
  - Depends on Issue 1 (Schema Registry)
  - Note: `procedures.md` will be removed from noet-core - procedures become a runtime-registered schema

- **[Issue 18: Extended Procedure Schemas](./ISSUE_18_EXTENDED_PROCEDURE_SCHEMAS.md)** (1.5-2 weeks, MEDIUM PRIORITY)
  - Observable action schema (`inference_hint` for passive detection)
  - Prompt schema (interactive participant input)
  - Extension points for observation producers and prompt renderers
  - Depends on Issue 1 (Schema Registry) and Issue 17 (noet-procedures)

#### Code Quality
- **[Issue 14: Naming Improvements](./ISSUE_14_NAMING_IMPROVEMENTS.md)** (2-3 days, OPTIONAL)
  - Rename types to match compiler architecture analogy
  - Fix confusing field names (`set` vs `stack_cache`)
  - Improve pedagogical clarity
  - Recommended before v1.0.0, optional for v0.1.0

#### Tooling
- `noet fmt` - standalone formatter
- `noet check` - validation without modification
- `noet migrate` - schema migration tool
- `noet serve` - local documentation server
- `noet export` - export to HTML/PDF

#### Performance
- Parallel parsing for large workspaces
- Incremental compilation cache
- Memory-mapped file support
- Lazy evaluation for queries

#### LSP Advanced
- Refactoring: Extract document, merge documents, split at cursor
- Call hierarchy (document dependencies)
- Type hierarchy (schema relationships)
- Collaborative editing support

#### Integrations
- Git integration (track BID changes)
- Database backends (PostgreSQL, SQLite alternatives)
- Web API / REST server mode
- Plugin system for custom codecs

#### AI/ML Features
- Suggest related documents
- Auto-generate summaries
- Semantic search
- Auto-tagging

---

### Issue Organization

Issues are numbered sequentially and tracked in `docs/project/ISSUE_XX_*.md`:

- **Issue 5**: Documentation - ✅ COMPLETE (Stage 1: 2025-01-18, Stage 2: 2025-01-24)
- **Issue 10**: Daemon Testing - ✅ COMPLETE (2025-01-24)
- **Issue 19**: File Watcher Timing Bug Investigation (HIGH PRIORITY) - Created 2025-01-24
- **Issues 1-4**: HTML Rendering (Phase 2 of v0.1.0)
- **Issue 6**: HTML Generation basics
- **Issue 13**: HTML CLI Integration (integrates Issues 6 + 10)
- **Issue 11**: Basic LSP (v0.2.0)
- **Issue 12**: Advanced LSP (v0.3.0)
- **Issue 14**: Naming Improvements (pedagogical clarity) - Optional for v0.1.0, recommended before v1.0.0
- **Issue 15**: Filtered Event Streaming (v0.3.0)
- **Issue 16**: Automerge Integration (v0.4.0)
- **Issue 17**: Extract noet-procedures Crate (v0.5.0+, depends on Issue 1)
- **Issue 18**: Extended Procedure Schemas (v0.5.0+, depends on Issue 1 and Issue 17)

### Issue States

- **Planning**: Issue written, not started
- **In Progress**: Actively being worked on
- **Blocked**: Waiting on dependency
- **Complete**: Merged to main branch
- **Deferred**: Moved to future version

### Issue Dependencies

```
Soft Open Source:
  Issue 5 (Documentation + Cargo.toml cleanup) → MAKE REPO PUBLIC

v0.1.0 Dependencies:
  Issue 5 (Documentation - basic)
  Issue 10 (Daemon + CLI)
  Issue 1 (Schema) → Issue 2 (TOML) → Issue 3 (Anchors)
  Issue 3 (Anchors) + Issue 2 (TOML) → Issue 4 (Links)
  Issue 6 (HTML Generation) + Issue 10 (CLI) → Issue 13 (HTML CLI Integration)
  All Phase 1-2 → Phase 3 (Testing) → Phase 4 (Infra) → Phase 5 (Publication)

v0.2.0 Dependencies:
  Issue 10 (Daemon) → Issue 11 (Basic LSP)

v0.3.0 Dependencies:
  Issue 11 (Basic LSP) → Issue 12 (Advanced LSP)

v0.5.0+ Dependencies:
  Issue 1 (Schema Registry) → Issue 17 (noet-procedures extraction)
  Issue 17 (noet-procedures) → Issue 18 (extended schemas: observables + prompts)

Bugfixes (no version dependency):
  Issue 19 (File Watcher Bug) - HIGH PRIORITY, may block soft open source if CLI broken
```

## Backlog Management

### Adding New Issues

When creating a new issue:
1. Assign to version milestone (v0.1.0, v0.2.0, etc.)
2. Identify dependencies
3. Estimate effort (days)
4. Add to this BACKLOG.md
5. Create issue file: `docs/project/ISSUE_XX_NAME.md`

### Prioritization Criteria

**CRITICAL**: Blocks v0.1.0 announcement
- Must complete before crates.io publication
- Required for core functionality
- Affects API stability

**HIGH**: Important for version milestone
- Significant user-facing feature
- Impacts documentation or examples
- Enables key use case

**MEDIUM**: Enhancement or optimization
- Nice to have, not blocking
- Performance improvement
- Quality of life feature

**LOW**: Future work
- Experimental feature
- Research needed
- Can be deferred indefinitely

### Version Planning

**Soft Open Source**: Minimal viable documentation
- Basic README explaining library purpose
- Clean Cargo.toml (no product dependencies)
- Repository made public
- No announcement yet

**v0.1.0**: Feature-complete with announcement
- No breaking changes after announcement
- Complete documentation (including CLI/daemon examples)
- Production-ready examples
- HTML rendering working
- Tested on multiple platforms

**v0.2.0+**: Add features without breaking API
- LSP integration (v0.2.0)
- Advanced LSP (v0.3.0)
- Tooling enhancements (v0.4.0+)
- Maintain semantic versioning

## Current Sprint

**Active Phase**: Post-soft-open-source (ready for Phase 2 or v0.1.0 announcement)

**Recently Completed**: 
- Issue 5 - Core Library Documentation ✅ COMPLETE (Stage 1: 2025-01-18, Stage 2: 2025-01-24)
- Issue 10 - Daemon Testing ✅ COMPLETE (2025-01-24)

**Current Priority**: 
- Issue 19 - File Watcher Bug (HIGH PRIORITY - manual CLI testing needed)

**Next Up**: 
- Option A: Resolve Issue 19 (verify `noet watch` works)
- Option B: Proceed to Phase 2 (HTML Rendering - Issues 1-4, 6, 13)
- Option C: Announce v0.1.0 (if Issue 19 not blocking)

**Blocked**: Potentially blocked by Issue 19 if `noet watch` CLI is broken

**Estimated Completion**: 
- Soft open source: ✅ COMPLETE (2025-01-18)
- Issue 19 resolution: 1-2 days
- Phase 2 (HTML): 2 weeks
- v0.1.0 announcement: 2-4 weeks depending on path

## References

- **Main Roadmap**: [`ROADMAP_OPEN_SOURCE_NOET-CORE.md`](./ROADMAP_OPEN_SOURCE_NOET-CORE.md)
- **Agent Guidelines**: [`../../../AGENTS.md`](../../../AGENTS.md)
- **Issues Directory**: `docs/project/ISSUE_*.md`

## Change Log

- **2024-XX-XX**: Created BACKLOG.md with versioning strategy
- **2024-XX-XX**: Added Issue 10 (Daemon Testing)
- **2024-XX-XX**: Added Issue 11 (Basic LSP) for v0.2.0
- **2024-XX-XX**: Added Issue 12 (Advanced LSP) for v0.3.0
- **2024-XX-XX**: Added Issue 13 (HTML CLI Integration)
- **2024-XX-XX**: Clarified soft open source strategy - Issue 5 only, then public repo
- **2024-XX-XX**: Decoupled Issue 5 from Issue 10 - can open source sooner
- **2025-01-18**: Soft open source achieved - repository made public
- **2025-01-24**: Completed Issue 5 Stage 2 (WatchService tutorial from Issue 10)
- **2025-01-24**: Completed Issue 10 (tutorial docs, integration tests, example created)
- **2025-01-24**: Created Issue 19 (File Watcher Timing Bug - HIGH PRIORITY)
