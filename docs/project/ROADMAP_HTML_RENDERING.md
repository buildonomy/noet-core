# HTML Rendering Roadmap

**Status**: Phase 1 Complete, Phase 2 In Progress  
**Target**: v0.1.0 (Required for Announcement)  
**Owner**: Andrew  
**Created**: 2025-01-15  
**Updated**: 2025-01-28

**CRITICAL**: Issue 6 (HTML Generation) must be completed BEFORE v0.1.0 announcement. Clean HTML rendering is essential for the value proposition to be clear.

## Executive Summary

This roadmap outlines the migration from YAML block BID injection to a cleaner, renderer-friendly approach using YAML frontmatter and NodeKey URL anchors. The goal is to ensure noet-generated markdown renders cleanly in any standard renderer (GitHub, GitLab, Obsidian, mdBook, etc.) while maintaining stable references and enabling future interactive HTML generation.

### Key Architectural Decision

**Use NodeKey URL schemas in markdown anchors, infer type in HTML:**
**Key Architectural Decision:**

Use **title-based anchors** in markdown for cross-renderer compatibility. Store BID mappings internally and add as data attributes during HTML generation only.

- **Markdown source**: `# Introduction {#introduction}` (title-based anchor)
- **Internal tracking**: BID-to-anchor mapping in node structure
- **HTML output**: `<h1 id="introduction" data-bid="01234567-89ab-cdef" data-nodekey="bid://01234567">`
- **Links**: `[Section](./doc.md#introduction)` - works everywhere
- **NodeKey tracking**: In link attributes, not heading anchors

**Benefits:**
- ✅ Universal compatibility (GitHub, GitLab, Obsidian auto-generate title anchors)
- ✅ Relative path + title anchor links work everywhere
- ✅ Clean markdown source (no BID clutter)
- ✅ BID tracking via HTML data attributes (HTML generation phase)
- ✅ NodeKey stability via link attributes (Issue 4)
- ✅ Future-proof for distributed/federated document networks

### Two-Level Metadata Injection

1. **Document-Level**: YAML frontmatter (industry standard)
2. **Heading-Level**: Title-based anchors + internal BID tracking
3. **Link-Level**: NodeKey attributes for stable references (Issue 4)

This provides clean separation of metadata and content while ensuring universal renderer compatibility. BID tracking happens internally and in HTML data attributes, not in markdown anchors.

## Problem Statement

Current BID injection uses YAML blocks under headings, which:
- Render as visible code blocks in standard renderers
- Create poor UX when viewing raw markdown on GitHub/GitLab
- Are incompatible with static site generators
- Clutter documents visually

## Critical Path

### Phase 0: Pre-Work ✅ COMPLETE (2025-01-15)

- [x] Document current state and plan migration
- [x] Research anchor syntax support across renderers
- [x] Design frontmatter + anchor approach
- [x] Decide on type inference approach (no prefixes)

### Phase 1: Core Migration ✅ COMPLETE (2025-01-28)

**Objective**: Migrate to frontmatter + NodeKey URL anchors for clean rendering

**Completed Issues**:
- **[Issue 1: Schema Registry](./completed/ISSUE_01_SCHEMA_REGISTRY.md)** ✅ COMPLETE
  - Refactored to singleton pattern matching `CodecMap`
  - Enabled downstream schema registration

- **[Issue 2: Multi-Node TOML Parsing](./completed/ISSUE_02_MULTINODE_TOML_PARSING.md)** ✅ COMPLETE
  - Parse frontmatter with `sections` map for per-heading metadata
  - Match section metadata to heading-generated nodes
  - Apply schema-typed payloads to sections

- **[Issue 3: Heading Anchors](./completed/ISSUE_03_HEADING_ANCHORS.md)** ✅ COMPLETE
  - Parse title-based anchors from headings: `{#introduction}`
  - Track BID-to-anchor mappings internally (not in markdown)
  - Generate title slugs when needed
  - Cross-renderer compatibility achieved

- **[Issue 4: Link Manipulation](./completed/ISSUE_04_LINK_MANIPULATION.md)** ✅ COMPLETE
  - Parse links with NodeKey attributes: `[text](./path.md){#bid://abc123}`
  - Generate relative paths from NodeKey resolution
  - Auto-update paths when targets move (preserve NodeKey)

- **[Issue 21: JSON/TOML Dual-Format Support](./completed/ISSUE_21_JSON_FALLBACK_PARSING.md)** ✅ COMPLETE
  - JSON as default format (cross-platform compatibility)
  - Support both BeliefNetwork.json and BeliefNetwork.toml
  - Network configuration schema for repo-wide format preferences
  - Bidirectional JSON/TOML conversion for uniform handling

**Migration Tool**:
- **[Issue 20: CLI Write-Back Support](./completed/ISSUE_20_CLI_WRITE_BACK.md)** ✅ COMPLETE
  - Implemented `noet write` command for updating documents
  - Supports frontmatter injection and metadata updates
  - Enables migration workflows

**Dependencies**:
```
Issue 1 (Schema Registry)
    ↓
    ├──→ Issue 21 (JSON/TOML Dual-Format)
    ↓
Issue 2 (Multi-Node TOML) ← Issue 3 (Heading Anchors)
    ↓                              ↓
    └──────→ Issue 4 (Link Manipulation)
                      ↓
              Migration Tool
                      ↓
                  v0.1.0 RELEASE
```

### Phase 2: HTML Generation (v0.1.0, ~2 weeks) - **IN PROGRESS**

**Objective**: Add optional HTML generation capability with interactive features

**[Issue 6: HTML Generation](./ISSUE_06_HTML_GENERATION.md)** (8-10 days, HIGH) - **CURRENT WORK**:
- Extend `DocCodec` trait with `generate_html()` method
- Implement HTML generation for `MdCodec`
- Create JavaScript viewer script for interactive features
- Create CSS stylesheet for noet documents
- CLI command for batch HTML generation
- NodeKey anchor resolution in browser

**Features**:
- Metadata rendering modes (hidden, collapsible, visible)
- `data-nodekey` attributes on headings and links
- Interactive viewer script (copy BID, navigate, tooltips)
- Browser-side NodeKey resolution
- Optional API-based cross-document resolution

**Status**: Phase 1 complete ✅, Issue 6 in progress

**[Issue 13: HTML CLI Integration](./ISSUE_13_HTML_CLI_INTEGRATION.md)** (2-3 days):
- Add `--html <output_dir>` to `noet parse` and `noet watch`
- Integrate HTML generation into FileUpdateSyncer
- Live reload server (optional)
- Static site generation workflow
- Requires Issue 6 complete

### Phase 3: Documentation & Examples ✅ COMPLETE (2025-01-24)

**[Issue 5: Core Library Documentation](./completed/ISSUE_05_DOCUMENTATION.md)** ✅ COMPLETE
- Architecture overview
- Codec implementation tutorial
- BID system deep dive
- Working examples
- FAQ

**[Issue 10: Daemon Testing & Library Pattern Extraction](./completed/ISSUE_10_DAEMON_TESTING.md)** ✅ COMPLETE
- Comprehensive tutorial docs with 4 doctests
- Full orchestration example: `examples/watch_service.rs`
- Threading model documented

**Remaining Documentation**:
- [ ] HTML generation guide (part of Issue 6)
- [ ] Renderer compatibility matrix (part of Issue 6)
- [ ] Sample HTML documents showcasing features (part of Issue 6)

### Phase 4: Testing & Validation (Ongoing)

- Renderer compatibility testing (GitHub, GitLab, Obsidian, mdBook, Hugo, Jekyll, Pandoc, markdown-it)
- Automated tests for all features
- Performance benchmarks
- Migration validation

## Success Metrics

### Phase 1 Complete ✅ (2025-01-28):
- [x] All markdown documents use frontmatter + NodeKey URL anchors
- [x] No visible YAML blocks in GitHub/GitLab preview
- [x] Clean HTML IDs (no prefixes, type inferred)
- [x] Migration tool available (`noet write` command)
- [x] Issues 1-4, 21 complete and tested
- [x] Compatibility verified: GitHub, GitLab, Obsidian

### Phase 2 Complete (BLOCKING v0.1.0):
- [ ] `generate_html()` implemented for `MdCodec` (Issue 6)
- [ ] Viewer script provides interactive features (Issue 6)
- [ ] CLI command generates static sites (Issue 13)
- [ ] HTML generation documented (Issue 6)

### Phase 3 Complete ✅ (2025-01-24):
- [x] Issue 5 complete (core library docs)
- [x] Issue 10 complete (comprehensive examples)
- [x] Compatibility tested across major renderers
- [ ] HTML generation examples (pending Issue 6)

## Timeline

```
✅ COMPLETE: Week 0-1:  Phase 1 - Issues 1-4, 21 (Frontmatter + Anchors)
✅ COMPLETE: Week 1-2:  Phase 1 - Migration Tool + Testing
✅ COMPLETE: Week 2:    Phase 3 - Issue 5, 10 (Documentation)
✅ COMPLETE: Week 3:    SOFT OPEN SOURCE (repository public, no announcement)
           ↓
→ CURRENT:   Week 4-5:  Phase 2 - Issue 6 (HTML Generation) - BLOCKING v0.1.0
→ NEXT:      Week 5:    Phase 2 - Issue 13 (HTML CLI Integration)
           ↓
           v0.1.0 ANNOUNCEMENT & CRATES.IO PUBLICATION
```

**Critical Milestone**: Issue 6 (HTML Generation) - **BLOCKS v0.1.0 announcement**  
**Current Status**: Phase 1 complete ✅, Phase 3 complete ✅, Phase 2 in progress
**Target**: v0.1.0 announcement after Issue 6 + Issue 13 complete

## Future Enhancements (Post-v0.3.0)

### Dynamic Content Nodes
Schema-based HTML-only generation for:
- Table of contents (`schema: toc`)
- Graph queries (`schema: query`)
- Backlinks display (`schema: backlinks`)
- Embedded visualizations

### Interactive Features
- Graph visualization embedded in HTML
- Live sync between markdown and HTML views
- Collaborative editing with CRDTs
- NodeKey resolution API service
- Browser extension for cross-site NodeKey navigation

### Advanced Rendering
- LaTeX math rendering
- Mermaid diagram support
- Code execution (Jupyter-style)
- Transclusion of content from other documents

### Tooling
- LSP server for markdown with noet metadata
- VS Code extension
- Web-based editor
- Mobile viewer app

## Risks & Mitigations

**Risk 1: Anchor Syntax Not Supported Everywhere**  
Mitigation: Anchors are additive—documents still work without them. Gracefully degrade.

**Risk 2: Frontmatter Conflicts**  
Mitigation: Use distinct key names (`bid`, `type`) unlikely to conflict. Document reserved keys.

**Risk 3: HTML Generation Complexity**  
Mitigation: Start simple (basic HTML), iterate based on feedback. Make it optional.

**Risk 4: Migration Breaks Existing Documents**  
Mitigation: Thorough testing, backup before migration, provide rollback tool.

## Communication

- Weekly updates in team sync
- Document progress in this file
- Phase 1 (v0.1.0) must complete before open source announcement
- Announce v0.1.0 in This Week in Rust
- Announce v0.2.0 (HTML generation) separately

## References

- **Issues**: See `ISSUE_01` through `ISSUE_06` in this directory
- **Design Docs**: `docs/design/beliefbase_architecture.md`
- **NodeKey Implementation**: `src/properties/nodekey.rs`
- **Current Codec**: `src/codec/md.rs`

---

**Current Step**: Complete Issue 6 (HTML Generation) - this is blocking v0.1.0 announcement.

**Critical Path Completed** ✅: Issue 1 → Issues 2,3 → Issue 4 → Issue 21 → Migration Tool (Issue 20)

**Critical Path Remaining**: Issue 6 (HTML Generation) → Issue 13 (HTML CLI) → v0.1.0 ANNOUNCEMENT
