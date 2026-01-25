# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).


## Upcoming

See [ROADMAP.md](docs/project/ROADMAP.md) for planned features.


## Unreleased - 2025-01-20

**Soft Open Source Release** - Repository made public without announcement.

This is a pre-release version for early feedback. The API is not yet stable and breaking changes are expected before v0.1.0.

### Added

- Multi-pass compilation system for document networks
- Bidirectional synchronization between documents and hypergraph
- BID (Belief ID) system for stable cross-document references
- Diagnostic-driven reference resolution
- Markdown codec with full parsing support
- TOML codec for metadata and structured data
- Event streaming for incremental cache updates
- SQLite database integration for persistent storage
- File watching with `FileUpdateSyncer`
- BeliefBase hypergraph data structures
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
