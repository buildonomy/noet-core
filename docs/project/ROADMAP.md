# noet-core Roadmap

A living document recording where noet-core has been, where it is now, and where it's headed.

**Last Updated**: 2026-03-02

## What noet-core Is

noet-core transforms document networks (Markdown, TOML) into a queryable hypergraph called a BeliefBase. It maintains bidirectional synchronization between human-readable source files and a machine-queryable graph, automatically managing cross-document references and propagating changes. The output is an interactive HTML viewer that lets users navigate, inspect, and (soon) search their document graph in the browser.

## Project History

### Foundation (2024 — early 2025)

The core compilation model: multi-pass parsing, BID injection, BeliefBase graph operations, event streaming, and the codec system. Originally developed as part of a larger workspace, then extracted as a standalone library.

- **Compilation model**: Multi-pass diagnostic-driven resolution of forward references
- **Identity system**: BID (Belief ID) injection for stable cross-document linking
- **Graph operations**: BeliefBase with typed edges (Subsection, Epistemic, Pragmatic)
- **Event architecture**: Async event streaming for incremental cache updates
- **Codec system**: Extensible parser framework (Markdown, TOML)

### Soft Open Source (January 2025)

Repository made public. Core documentation written. CLI tool (`noet parse`, `noet watch`) and daemon created.

**Completed issues**: 1 (Schema Registry), 2 (Multi-Node TOML), 3 (Heading Anchors), 4 (Link Manipulation), 5 (Documentation), 10 (Daemon/CLI), 14 (Naming Improvements), 20 (CLI Write-Back), 21 (JSON/TOML Dual-Format), 22 (Duplicate Node Dedup), 23 (Integration Test Convergence)

**Key decisions**:
- Title-based anchors in markdown, BIDs only in HTML data attributes — universal renderer compatibility
- JSON as default metadata format (TOML fallback) — cross-platform consistency
- Event loop synchronization (Option G pattern) — correct BeliefBase export timing

### HTML Rendering (January — March 2025)

The migration from YAML block BID injection to clean frontmatter + title anchors, followed by full HTML generation with an interactive single-page application viewer.

**Completed issues**: 6 (HTML Generation), 24 (API Node Architecture), 29 (Static Asset Tracking), 33 (Weight Doc Paths), 34 (Cache Instability), 35 (Cache Invalidation), 37 (Heading Anchor Bugs), 38 (Interactive SPA Foundation), 39 (Advanced Interactive Features), 40 (Network Index DocCodec), 43 (Codec HTML Refactor), 44 (UI Cleanup), 45 (WASM Threading Fix), 48 (Path Manipulation Consolidation), 51 (Author Diagnostics), 52 (Network Index Content Merge), 53 (Cache Invalidation Test Sync)

**Key decisions**:
- Single-page application with client-side document fetching — no server required
- WASM-compiled BeliefBase for in-browser graph queries — `BeliefBaseWasm`
- Two-click navigation pattern — preview metadata, then navigate
- PathMapMap-based navigation tree — stack-based construction matching document hierarchy
- Deferred HTML generation — cross-document content (backlinks, related nodes) generated after all documents parsed

### Current State (March 2025)

The interactive HTML viewer is functional: SPA navigation, metadata panel, navigation tree, theme switching, link detection, image modals, header anchors. The compiler produces complete static HTML output via `noet parse` and live-reloading output via `noet watch`.

**What works well**: Compilation model, BID stability, interactive viewer, daemon with file watching, event-driven cache updates, cross-network references.

**What's missing for internal MVP**: Full-text search, BeliefBase sharding for large repositories, performance characterization at scale.

## Current Focus: Internal MVP

The immediate goal is an internal MVP for use at Buildonomy. This means the viewer needs to handle real-world documentation repositories — which means search and scaling.

### Active Work

**Issue 50: BeliefBase Sharding** — Per-network JSON export and on-demand loading in the viewer. Establishes the `ShardManager`, network selector UI, and memory budget infrastructure that search layers onto.

### Planned Sequence

```
Issue 50: BeliefBase Sharding (4–6 days)
    Establishes: finalize_html export hooks, ShardManager,
    network selector UI, memory budget display,
    search/*.idx.json generation (always, both modes)
    ▼
Issue 47: Performance Profiling (3–4 days)
    Establishes: realistic corpus generator, scale-sized
    test fixtures (10KB → 100MB+), macro-benchmarks
    ▼
Issue 54: Full-Text Search MVP (4–5 days)
    Adds: compile-time per-network search indices (.idx.json),
    TF-IDF ranking, fuzzy matching, viewer search UI
    No external dependencies — zero WASM binary increase
    Search covers entire corpus at init (including unloaded shards)
```

**Design document**: `docs/design/search_and_sharding.md` — unified architecture for sharding and search.

### Release Gating

After the internal MVP is validated, the path to public release (v0.1.0):

- **Issue 7**: Comprehensive testing — full test suite, CI matrix, browser compatibility
- **Issue 8**: Repository setup — CI/CD pipelines, issue templates, documentation hosting
- **Issue 9**: Crates.io release — dependency audit, package validation, publication, announcements

These are deliberately deferred until the product is proven internally. Quality pass happens once, not iteratively.

## Vision: Where We're Headed

The following are aspirational directions, roughly ordered by how likely they are to happen. No timelines — they'll be planned when the time comes.

### IDE Integration

Language Server Protocol support for real-time diagnostics, navigation, and editing in IDEs.

- **Issue 11**: Basic LSP — diagnostics, hover, document sync
- **Issue 12**: Advanced LSP — go-to-definition, find references, autocomplete, rename
- **Issue 15**: Filtered event streaming — query-based subscriptions for real-time UI updates

### Graph Visualization and Querying

Interactive graph views embedded in the HTML viewer. Query builder UI for exploring the BeliefBase.

- **Issue 42**: Graph visualization
- **Issue 41**: Query builder UI
- **Issue 41 (Stream)**: Stream events to SPA for live updates

### Multi-Device Sync and Collaboration

Distributed state synchronization using Automerge CRDTs. Peer-to-peer sync, collaborative presence, offline-first workflow.

- **Issue 16**: Automerge integration for activity logs and distributed state

### Per-Network Theming and Git-Aware Networks

Custom themes per network. Git integration for tracking changes and displaying version history.

- **Issue 25**: Per-network theming
- **Issue 26**: Git-aware networks

### Procedural Extensions

Extract procedure execution to a separate crate. "As-run" tracking, redline system, observable actions, interactive prompts.

- **Issue 17**: Extract noet-procedures crate
- **Issue 18**: Extended procedure schemas (observables, prompts)

### Further Out

These are ideas, not plans:

- **Authorization**: Keyhive distributed authorization, capability-based access control
- **Language features**: Syntax extensions, template system, macro system
- **Tooling**: `noet fmt`, `noet check`, `noet migrate`, `noet serve`
- **Performance**: Parallel parsing, incremental compilation cache, memory-mapped files
- **Integrations**: Database backends (PostgreSQL), plugin system for custom codecs
- **AI/ML**: Semantic search, related document suggestions, auto-tagging

## Active Issues

| Issue | Title | Priority | Status |
|-------|-------|----------|--------|
| 50 | BeliefBase Sharding | HIGH | Planned (next) |
| 47 | Performance Profiling | MEDIUM | Planned |
| 54 | Full-Text Search MVP | HIGH | Planned |
| 49 | Full-Text Search Production | MEDIUM | Planned |
| 7 | Comprehensive Testing | MEDIUM | Planned (release gate) |
| 8 | Repository Setup | MEDIUM | Planned (release gate) |
| 9 | Crates.io Release | MEDIUM | Planned (release gate) |
| 11 | Basic LSP | MEDIUM | Planned (post-release) |
| 13 | HTML CLI Integration | MEDIUM | Partially complete |
| 25 | Per-Network Theming | LOW | Backlog |
| 26 | Git-Aware Networks | LOW | Backlog |
| 27 | Rustdoc Integration | LOW | Backlog |
| 28 | Code Quality | LOW | Backlog |
| 30 | External URL Tracking | LOW | Backlog |
| 31 | Watch Service Asset Integration | LOW | Backlog |
| 32 | Schema Registry Production | LOW | Backlog |
| 36 | Section BID Migration | LOW | Backlog |
| 41 | Query Builder / Stream Events | LOW | Backlog |
| 42 | Graph Visualization | LOW | Backlog |
| 46 | Full-Text Search (superseded) | — | Superseded by 50/54/49 |

See `BACKLOG.md` for optional enhancements extracted from completed issues.

## References

- `docs/design/beliefbase_architecture.md` — Core data model and compilation architecture
- `docs/design/interactive_viewer.md` — HTML viewer design
- `docs/design/search_and_sharding.md` — Search and sharding architecture
- `docs/project/README.md` — Issue resolution workflow
- `docs/project/BACKLOG.md` — Optional enhancements
- `AGENTS.md` — Collaboration guidelines