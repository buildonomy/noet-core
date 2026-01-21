# noet-core Documentation

This directory contains all documentation for the noet-core library.

## Quick Navigation

### For Users

**Getting Started**:
- [Main README](../README.md) - Installation, quick start, and overview
- [Architecture Overview](design/architecture.md) - Core concepts and how the library works
- [API Reference](https://docs.rs/noet-core) - Generated from rustdoc (run `cargo doc --open`)

### For Contributors

**Project Planning**:
- [Roadmap](project/ROADMAP.md) - Version milestones and feature backlog
- [v0.1.0 Roadmap](project/ROADMAP_NOET-CORE_v0.1.md) - Detailed plan for first release
- [Issue 5: Documentation](project/ISSUE_05_DOCUMENTATION.md) - Current documentation work (Stage 1 ✅ complete)

**Development Guidelines**:
- [Documentation Strategy](project/DOCUMENTATION_STRATEGY.md) - How documentation is organized
- [AGENTS.md](../AGENTS.md) - Guidelines for AI-assisted development
- [CONTRIBUTING.md](../CONTRIBUTING.md) - Setting up your development environment and getting pull requests merged

## Directory Structure

```
docs/
├── README.md                    # This file
├── design/                      # Architecture and design specifications
│   ├── architecture.md          # High-level architecture guide
│   └── beliefset_architecture.md # Detailed technical specification
└── project/                     # Project management documents
    ├── ROADMAP*.md              # Version roadmaps and planning
    ├── ISSUE_*.md               # Issue tracking and specifications
    └── DOCUMENTATION_STRATEGY.md # Documentation organization guide
```

## Documentation Levels

noet-core follows a **hierarchical documentation strategy**:

1. **Quick Start** → `../README.md` - "Should I use this library?"
2. **Conceptual** → `design/architecture.md` - "How does it work?"
3. **Technical** → `design/beliefset_architecture.md` - "How is it implemented?"
4. **API Reference** → Rustdoc - "How do I use this API?"

See [DOCUMENTATION_STRATEGY.md](project/DOCUMENTATION_STRATEGY.md) for details on our single-source-of-truth approach.

## Design Documents (`design/`)

**Purpose**: Architecture specifications and design decisions

### [architecture.md](design/architecture.md)
High-level overview of noet-core's architecture for developers getting started with the library.

**Contents**:
- Core concepts (BID, BeliefSet, multi-pass compilation)
- Architecture components and data flow
- Relationship to prior art (Obsidian, Neo4j, rust-analyzer)
- Getting started examples

**Audience**: Developers learning the library

### [beliefset_architecture.md](design/beliefset_architecture.md)
Complete technical specification for understanding internals and contributing.

**Contents**:
- Detailed compilation model and algorithms
- Identity management (BID, Bref, NodeKey)
- Graph structure and invariants
- Multi-pass reference resolution
- Integration points
- Future enhancements

**Audience**: Contributors, maintainers, advanced users

**Note**: This is the **source of truth** for implementation details.

## Project Documents (`project/`)

**Purpose**: Project planning, issue tracking, and development guidelines

### Roadmaps

- **[ROADMAP.md](project/ROADMAP.md)** - Main roadmap with version milestones (v0.1.0, v0.2.0, v0.3.0+)
- **[ROADMAP_NOET-CORE_v0.1.md](project/ROADMAP_NOET-CORE_v0.1.md)** - Detailed v0.1.0 plan (soft open source → announcement)
- **[ROADMAP_HTML_RENDERING.md](project/ROADMAP_HTML_RENDERING.md)** - HTML rendering feature plan

### Issues

Issues are numbered sequentially and tracked as individual markdown files:


## Contributing

See [CONTRIBUTING.md](../CONTRIBUTING.md) for contribution guidelines.

For AI-assisted development, see [AGENTS.md](../AGENTS.md).
