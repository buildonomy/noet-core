# noet-core Roadmap

A living document recording where noet-core has been, where it is now, and where it's headed.

**Last Updated**: 2026-04-23

## What noet-core Is

noet-core is a compiler for document networks. It transforms interconnected source
files (Markdown, TOML, XLSX) into a queryable, typed directed acyclic graph (DAG)
called a BeliefBase, with three orthogonal edge dimensions — Section (structure),
Epistemic (provenance), and Pragmatic (actionable claims). It maintains bidirectional
synchronization between human-readable source files and the machine-queryable graph,
automatically managing cross-document references and propagating changes.

The output is an interactive HTML viewer for navigating, searching, and inspecting the
document graph in the browser, plus an MCP server for AI-agent-driven graph queries.

See [Documentation as a Dependency Graph](../design/dag_model.md) for the conceptual
introduction.

## Strategic Direction

noet-core is an application-agnostic open-source tool, but its development is driven
by a specific class of problem: **knowledge management for safety-critical engineered
systems with complex, cross-discipline traceability requirements**.

In these domains — aerospace, automotive, medical devices, nuclear — documentation is
not incidental to the product. It *is* the product's compliance posture. Requirements
trace to hazard analyses. Test results trace to requirements. Design rationale traces
to both. These relationships span organizational boundaries, file formats, and
toolchains. When the traceability is wrong, the system is unsafe — and you cannot tell
whether it is wrong by reading the documents individually.

noet-core treats this traceability the same way a build system treats code
dependencies: as an explicit, queryable, validatable graph. The goal is a tool that
makes structural review — coverage gaps, orphaned claims, missing rationale — a
graph query rather than a manual document audit.

## Project History

### Foundation (2024 — early 2026)

The core compilation model: multi-pass parsing, BID injection, BeliefBase graph
operations, event streaming, and the codec system. Originally developed as part of a
larger workspace, then extracted as a standalone library.

- **Compilation model**: Multi-pass diagnostic-driven resolution of forward references
- **Identity system**: BID (Belief ID) injection for stable cross-document linking
- **Graph operations**: BeliefBase with typed edges (Section, Epistemic, Pragmatic)
- **Event architecture**: Async event streaming for incremental cache updates
- **Codec system**: Extensible parser framework (Markdown, TOML)

**Completed issues**: 1 (Schema Registry), 2 (Multi-Node TOML), 3 (Heading Anchors),
4 (Link Manipulation), 5 (Documentation), 10 (Daemon/CLI), 14 (Naming Improvements),
20 (CLI Write-Back), 21 (JSON/TOML Dual-Format), 22 (Duplicate Node Dedup),
23 (Integration Test Convergence)

### HTML Viewer (January — March 2026)

Full HTML generation with an interactive single-page application viewer. Navigation
tree, metadata panel, theme switching, deferred cross-document content generation.

**Completed issues**: 6 (HTML Generation), 13 (HTML CLI Integration),
24 (API Node Architecture), 29 (Static Asset Tracking), 33 (Weight Doc Paths),
34 (Cache Instability), 35 (Cache Invalidation), 37 (Heading Anchor Bugs),
38 (Interactive SPA Foundation), 39 (Advanced Interactive Features),
40 (Network Index DocCodec), 43 (Codec HTML Refactor), 44 (UI Cleanup),
45 (WASM Threading Fix), 48 (Path Consolidation), 51 (Author Diagnostics),
52 (Network Index Content Merge), 53 (Cache Invalidation Test Sync)

### Search, Sharding, and Performance (March — May 2026)

The viewer scaled to production-sized corpora. Per-network JSON sharding with
on-demand WASM loading. Compile-time TF-IDF search indices. Performance profiling
yielding up to 27× speedup on hotpath operations.

**Completed issues**: 47 (Performance Profiling — 27× hotpath speedup),
50 (BeliefBase Sharding — per-network JSON export, ShardManager, memory budget),
54 (Full-Text Search MVP — compile-time inverted indices, fuzzy matching)

**Superseded issues**: 46 (Full-Text Search — split into 50+54),
49 (Search Production — absorbed into Issue 70)

### Traceability and Relations (May — July 2026)

The system became a traceability tool. MyST directive syntax established `{maps_to}`
and the relation directive framework. The traceability view gave structured tabular
inspection of coverage claims. Parallel compilation achieved 7.8× speedup. The XLSX
codec enabled spreadsheet ingestion. Git-aware networks surfaced source URLs.

**Completed issues**: 26 (Git-Aware Networks), 30 (External URL Tracking — OBE),
41 (Query Builder — superseded by Issue 70), 42 (Graph Visualization — superseded by
Issue 70), 55 (MyST Directive Syntax), 57 (Parallel Epoch Compilation — 7.8× at
`--jobs 8`), 59 (Git Metadata Export), 60 (href/Anchor Cache Fixes),
61 (Mapping Node — `{maps_to}` directive), 62 (Compiler Re-queue Regression),
63 (Traceability View), 69 (XLSX Codec)

## Current State (April 2026)

**What works well**: Multi-pass compilation with parallel epochs, BID stability,
interactive HTML viewer with search and traceability tables, `{maps_to}` cross-document
edge ownership, XLSX ingestion, per-network sharding, git metadata, MCP server for
agent queries, daemon with file watching, event-driven cache updates.

**What's actively being built**: Generalized relation directives (all six
WeightKind × Direction verbs), unified search/query/graph UI, codec architecture for
structured data formats, MCP server hardening.

**What's missing for the systems engineering use case**: Composed multi-axis queries
(the query model is designed but not implemented), source code ingestion, incremental
re-parse across invocations, inline relation syntax, and a collaboration overlay for
human attestation.

## Active Work

### Near-Term (in progress or next)

| Issue | Title | Priority | Status |
|-------|-------|----------|--------|
| 71 | Generalize Relation Block Directives | HIGH | Active |
| 68 | Two-Registry Codec Architecture | HIGH | Active |
| 64 | MCP BeliefBase Server | HIGH | Active (hardening) |
| 60 | Parallel Compilation Follow-On | HIGH | Active (test fixes) |
| 70 | Unified Search, Query, and Graph UI | MEDIUM | Planned (next) |
| 58 | Inline `{implements}` Role | MEDIUM | Planned |
| 66 | Incremental Parse via Shard Hydration | MEDIUM | Planned |

### Medium-Term (enables the systems engineering workflow)

| Issue | Title | Priority | Notes |
|-------|-------|----------|-------|
| 67 | Source Code Codec | MEDIUM | Structured data ingestion from codegen artifacts |
| 56 | PathMap Protocol Invariants | MEDIUM | Formal protocol-level testing for the graph builder |
| 65 | Collaboration Overlay | LOW | Human attestation (comments, sign-offs) on static sites |
| 32 | Schema Registry Production | MEDIUM | Auto-generate edges from structured payload fields |

### Deferred (valuable but not blocking)

| Issue | Title | Notes |
|-------|-------|-------|
| 7 | Comprehensive Testing | Release gate — deferred until product proven in use |
| 8 | Repository Setup | Release gate — CI/CD, issue templates, docs hosting |
| 9 | Crates.io Release | Release gate — final publication step |
| 28 | Code Quality & API Review | Release gate — public API cleanup |
| 11 | Basic LSP | IDE integration — diagnostics, hover, document sync |
| 12 | Advanced LSP | Go-to-definition, autocomplete, find-references |
| 15 | Filtered Event Streaming | Query-filtered subscriptions for real-time UI updates |
| 36 | Section BID Migration | Content-based section identity for stable moves |
| 31 | Watch Service Asset Integration | Auto-reparse on asset changes |
| 25 | Per-Network Theming | Distinct visual styling per network |
| 27 | Rustdoc Integration | Cross-link cargo doc with noet design docs |

### Aspirational (ideas, not plans)

- **Automerge Integration** (Issue 16): Distributed CRDT-based sync, peer-to-peer
  collaboration, offline-first workflow
- **Procedures Extraction** (Issues 17, 18): `noet-procedures` crate for execution
  tracking, redline system, observable actions
- **Authorization**: Capability-based access control (Keyhive)
- **Language features**: Template system, macro system, syntax extensions
- **Tooling**: `noet fmt`, `noet check`, `noet migrate`, `noet serve`
- **Performance**: Memory-mapped files, compilation caching
- **Integrations**: Database backends (PostgreSQL), plugin system

## Design Documents

The query model and DAG model documents define the architectural direction for the
query system and the conceptual framework. Implementation will follow these specs:

- **[DAG Model](../design/dag_model.md)** — Conceptual introduction to the multigraph
  model: nodes, three edge types, the video camera query metaphor
- **[Query Model](../design/query_model.md)** — Formal query algebra: subject,
  projection, instrument, score semiring, textual syntax
- **[BeliefBase Architecture](../design/beliefbase_architecture.md)** — Core data model,
  compilation pipeline, identity management
- **[MCP Server](../mcp.md)** — Agent-facing query interface documentation

## References

- `docs/project/UX_AUDIT.md` — UX depth model audit: depth levels, current violations, subtraction candidates
- `docs/project/README.md` — Issue resolution workflow
- `docs/project/BACKLOG.md` — Optional enhancements
- `docs/project/DOCUMENTATION_STRATEGY.md` — Documentation hierarchy
- `AGENTS.md` — Collaboration guidelines
- `CONTRIBUTING.md` — Development workflow