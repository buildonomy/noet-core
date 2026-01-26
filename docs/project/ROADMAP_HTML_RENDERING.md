# HTML Rendering Roadmap

**Status**: In Progress  
**Target**: v0.1.0 (Pre-Open Source - Required)  
**Owner**: Andrew  
**Created**: 2025-01-15  
**Updated**: 2025-01-15

**CRITICAL**: This work must be completed BEFORE open sourcing. Clean HTML rendering is essential for the value proposition to be clear.

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

### Phase 0: Pre-Work ✅ COMPLETE

- [x] Document current state and plan migration
- [x] Research anchor syntax support across renderers
- [x] Design frontmatter + anchor approach
- [x] Decide on type inference approach (no prefixes)

### Phase 1: Core Migration (v0.1.0, ~2 weeks) - **REQUIRED FOR OPEN SOURCE**

**Objective**: Migrate to frontmatter + NodeKey URL anchors for clean rendering

**Issues**:
- **[@ISSUE_01_SCHEMA_REGISTRY.md](./ISSUE_01_SCHEMA_REGISTRY.md)** (3-4 days, CRITICAL)
  - Refactor to singleton pattern matching `CodecMap`
  - Enables downstream schema registration
  - Blocks Issues 2, 3, 4

- **[@ISSUE_02_MULTINODE_TOML_PARSING.md](./ISSUE_02_MULTINODE_TOML_PARSING.md)** (4-5 days, CRITICAL)
  - Parse frontmatter with `sections` map for per-heading metadata
  - Match section metadata to heading-generated nodes
  - Apply schema-typed payloads to sections
  - Requires Issue 1

- **[@ISSUE_03_HEADING_ANCHORS.md](./ISSUE_03_HEADING_ANCHORS.md)** (2-3 days, CRITICAL)
  - Parse title-based anchors from headings: `{#introduction}`
  - Track BID-to-anchor mappings internally (not in markdown)
  - Generate title slugs when needed
  - Prioritize cross-renderer compatibility
  - Indirect dependency on Issues 1 & 2

- **[@ISSUE_04_LINK_MANIPULATION.md](./ISSUE_04_LINK_MANIPULATION.md)** (3-4 days, CRITICAL)
  - Parse links with NodeKey attributes: `[text](./path.md){#bid://abc123}`
  - Generate relative paths from NodeKey resolution
  - Auto-update paths when targets move (preserve NodeKey)
  - Requires Issues 1, 2, 3

- **[@ISSUE_21_JSON_FALLBACK_PARSING.md](./ISSUE_21_JSON_FALLBACK_PARSING.md)** (3-4 days, MEDIUM)
  - JSON as default format (cross-platform compatibility)
  - Support both BeliefNetwork.json and BeliefNetwork.toml
  - Network configuration schema for repo-wide format preferences
  - Bidirectional JSON/TOML conversion for uniform handling
  - Requires Issue 1

**Additional Phase 1 Work** (not yet in dedicated issue):
- **Migration Tool** (2 days): Convert existing YAML-block documents to new format
  - CLI: `noet migrate <directory>`
  - Backward compatibility checks
  - Migration report generation

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

### Phase 2: HTML Generation (v0.2.0 → v0.3.0, ~2 weeks)

**Objective**: Add optional HTML generation capability with interactive features

**[@ISSUE_06_HTML_GENERATION.md](./ISSUE_06_HTML_GENERATION.md)** (to be created):
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

**Dependencies**: Phase 1 complete (v0.1.0 released)

### Phase 3: Documentation & Examples (v0.3.0, ~1 week)

**[@ISSUE_05_DOCUMENTATION.md](./ISSUE_05_DOCUMENTATION.md)** (3-4 days, HIGH priority)
- Architecture overview
- Codec implementation tutorial
- BID system deep dive
- Working examples
- FAQ

**Additional Documentation**:
- HTML generation guide (2 days)
- Renderer compatibility matrix (1 day)
- Sample documents showcasing features (1 day)

### Phase 4: Testing & Validation (Ongoing)

- Renderer compatibility testing (GitHub, GitLab, Obsidian, mdBook, Hugo, Jekyll, Pandoc, markdown-it)
- Automated tests for all features
- Performance benchmarks
- Migration validation

## Success Metrics

### Phase 1 Complete (v0.1.0 - REQUIRED):
- [ ] All markdown documents use frontmatter + NodeKey URL anchors
- [ ] No visible YAML blocks in GitHub/GitLab preview
- [ ] Clean HTML IDs (no prefixes, type inferred)
- [ ] Migration tool successfully converts old format
- [ ] Issues 1-4 complete and tested
- [ ] Compatibility verified: GitHub, GitLab, Obsidian

### Phase 2 Complete (v0.2.0):
- [ ] `generate_html()` implemented for `MdCodec`
- [ ] Viewer script provides interactive features
- [ ] CLI command generates static sites
- [ ] Issue 6 complete

### Phase 3 Complete (v0.3.0):
- [ ] Issue 5 complete (core library docs)
- [ ] HTML generation documented
- [ ] Examples demonstrate all features
- [ ] Compatibility tested across 8+ renderers

## Timeline

```
Week 0-1:  Phase 1 - Issues 1-4 (Frontmatter + Anchors) - CRITICAL
Week 1-2:  Phase 1 - Migration Tool + Testing
           ↓
           v0.1.0 RELEASE & OPEN SOURCE
           ↓
Week 2-3:  Phase 2 - Issue 6 (HTML Generation)
Week 3-4:  Phase 2 - Viewer Script + CSS
Week 4-5:  Phase 3 - Issue 5 (Documentation) + HTML docs
Ongoing:   Phase 4 - Testing & validation
```

**Critical Milestone**: v0.1.0 (Phase 1 complete) - Required before open sourcing  
**Target**: v0.1.0 release in 2 weeks, then open source  
**Target**: v0.2.0 (HTML generation) 3-4 weeks after open source

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

**Next Step**: Begin Issue 1 (Schema Registry) immediately - this is blocking open source.

**Critical Path**: Issue 1 → Issues 2,3 → Issue 4 → Migration Tool → v0.1.0 → Open Source → Issue 6 → v0.2.0
