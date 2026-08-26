# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).


## Upcoming

See [ROADMAP.md](docs/project/ROADMAP.md) for planned features.


## Unreleased - 2026-08-26

Five months of feature work: cross-document traceability, a unified query
system, a new spreadsheet codec, an MCP server for agent access to the
BeliefBase, and substantial correctness and performance work for large
(30K–70K node) multi-network corpora.

### Added

- **`{maps_to}` directive**: any section or document node can now own directed
  edges between two *other* nodes (source/sink) without being an endpoint
  itself — the foundation for compliance/traceability mapping tables that
  live alongside narrative content instead of in a separate matrix.
- **Traceability view**: a sortable, filterable matrix (viewer modal + MCP
  tools `get_traceability`, `get_maps_to`, `get_maps_to_traceability`) keyed
  on canonical network order, with CSV/XLSX export.
- **Query system**: a unified `QuerySpec` model (Subject/Projection/
  Instrument) with a textual grammar shared across the viewer's `?q=` URLs,
  a new `{query}` MyST directive for compile-time query rendering embedded
  in documents, and the MCP `query` tool. Supports composition grouping,
  precedence, and inverted traversals.
- **xlsx/ods codec**: parses Excel/OpenDocument workbooks into first-class
  graph nodes (via `calamine`), renders them as tab-switched tables in the
  viewer, and supports write-back of relation columns to the source file.
- **MCP BeliefBase server**: exposes the compiled graph to AI agents over
  MCP — `get_networks`, `search`, `get_context`, `get_submap`, `query`,
  `check_consistency`, plus bref/traceability helpers — so agents can query
  a live semantic graph instead of reading source files ad hoc.
- **Two-registry codec architecture** (`WALK_CODECS` + `CLAIM_MAP`): separates
  "should this file be visible in a network's child list" from "which codec
  owns and parses it," enabling codecs that claim structured data files
  (YAML, CSV, etc.) after inspecting an orchestrating document.
- **Network child filtering**: per-network `whitelist`/`blacklist` glob
  patterns in `index.md` frontmatter to exclude generated/vendored files
  from a network's node graph.
- **Generalized network roots**: application codecs can declare additional
  filenames (beyond `index.md`) as network-root markers, with content-based
  disambiguation for cases where the filename alone is ambiguous.
- **msgpack shard format**: WASM shards and search indices moved from JSON
  to MessagePack, reducing payload size and parse cost for sharded corpora.
- **`noet distribute` command**: packages a rendered site with a vendored
  static file server and launcher script into a self-contained,
  double-click-runnable directory for non-technical stakeholders.
- **URL alias resolution**: nodes can declare `url_aliases` or a network-level
  `alias-template`, registered in the existing href namespace, so external
  URLs/paths that match internal nodes resolve to them instead of becoming
  disconnected stub nodes.
- **Secondary index namespaces**: codecs can register named, cross-network
  PathMaps for lookup paths that don't match a node's canonical path (e.g.
  resolving `#include` edges across build-system prefix conventions).
- **`RawTapeView`**: a general per-entry tape renderer replacing hard-coded
  JS rendering logic for `maps_to` display, plus a JSON view output for the
  WASM/JS viewer.
- **N/S/P/R content-type classifier**: scores document nodes on normative/
  structural/procedural/as-run-record axes using voice/tense heuristics,
  domain-independent by construction.
- **Inline anchor nodes**: `{#id}` in paragraphs and list items (not just
  headings) now creates a first-class, individually addressable graph node.
- **Compile-time layout pipeline**: computes force-settled 3D positions and
  structural-depth/weight metadata per node, blending content-type and
  edge-topology signal (feeds a future graph-visualization viewer).
- **Version selector UI**: viewer dropdown for switching between versioned
  deployments, driven by a `versions.json` manifest assembled by CI.
- Server-side KaTeX pre-rendering, symlink-following during directory walks
  (with cycle handling), `{#__continue}` heading ID for section continuation,
  and a bulk asset-registration fast path for corpora with tens of thousands
  of media files.

### Fixed

- Anchor identity separated from network-scoped node ID (new `NodeId` enum),
  eliminating duplicate section nodes previously caused by re-parse and ID
  collision interaction.
- Numerous href/asset path-resolution bugs (bare `#anchor` lookups, base-URL
  handling in the SPA shell, path aliasing, alias-template scoping).
- Cache invalidation and `cache_fetch` hit logic tightened to stop spurious
  re-parses and pathmap warnings.
- Search ranking: exact ID matches always rank first for single-token queries.
- `noet distribute` no longer recurses infinitely when the target directory
  is inside the source directory.
- DB/in-memory parity fixes: `NetId`/`NetPath` resolution, `submap_by_bid`
  returning an empty subtree for non-network leaf documents, and a Phase 4
  panic from stale parsed-BID state after a cross-sheet ID collision.
- Path collisions (multiple paths claiming one BID, or vice versa) now
  resolved consistently through the path index; enforced one-path-per-BID
  invariant in `PathMap`.
- Symlinked files skipped correctly during network directory partitioning.
- "Halo explosion" and SQLite bind-variable overflow that blocked full
  builds of large, densely cross-referenced corpora.
- Node absorption/rename semantics unified between the in-memory and SQL
  accumulator backends, fixing content nodes that failed to retire external
  stub nodes claiming the same URL.

### Performance

- Replaced multiple O(N²)/O(n²) hot paths (`bid_to_index` rebuild, PathMap
  relation-update indexing, hub-sink scans) with incremental or indexed
  structures.
- Migrated core lookup tables (`states`, `bid_to_index`) from `BTreeMap` to
  `FxHashMap`.
- Split the accumulator's global lock so concurrent SQL reads no longer
  serialize behind a single in-flight traversal.
- Multi-threaded runtime for the `parse` CLI command; epoch/session seeding
  batched and shared across parse tasks instead of per-document.
- Bulk asset registration bypasses per-file `GraphBuilder` overhead for
  media-heavy corpora.
- SPA viewer first-paint optimizations and lazy, on-demand shard loading.

### Changed

- `BeliefSource` refactored so its public API takes `QuerySpec` directly;
  the intermediate `Expression`/`eval_query` layer was deleted.
- Relation directives (`{implements}` and friends) generalized to a unified
  codespan-toggle syntax.
- Documentation overhaul: DAG model explainer, revised query-model spec,
  network-authoring reference, and new lessons-learned / performance-findings
  registers for maintainers.

## Unreleased - 2025-01-20

**Soft Open Source Release** - Repository made public without announcement.

This is a pre-release version for early feedback. The API is not yet stable and breaking changes are expected before v0.1.0.

### Added

- Multi-pass compilation system for document networks
- Bidirectional synchronization between documents and multigraph
- BID (Belief ID) system for stable cross-document references
- Diagnostic-driven reference resolution
- Markdown codec with full parsing support
- TOML codec for metadata and structured data
- Event streaming for incremental cache updates
- SQLite database integration for persistent storage
- File watching with `FileUpdateSyncer`
- BeliefBase multigraph data structures
- Query system for graph traversal
- Nested network support (similar to git submodules)
- Extensible codec system via `DocCodec` trait
- Feature flags: `service` (daemon/database), `wasm` (WebAssembly support)

### Documentation
- Comprehensive README with usage examples
- Architecture overview in `docs/architecture.md`
- Detailed specification in `docs/design/beliefbase_architecture.md`
- API documentation with examples
- Contributing guidelines
- Basic usage example

### Infrastructure
- GitLab CI/CD pipeline with multi-platform testing
- Dual MIT/Apache-2.0 licensing
- Security scanning (SAST, secret detection)
- Code coverage reporting
- Documentation generation

### Notes
- Not published to crates.io
- Pre-1.0 development version
- Breaking changes allowed
- Used for gathering early feedback from trusted developers
