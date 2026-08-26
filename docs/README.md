# noet-core Documentation

This directory contains all documentation for the noet-core library.

## Start Here

**[Documentation as a Dependency Graph](design/dag_model.md)** — Why noet models
documents as a directed acyclic graph, what the three edge types mean, and how
queries work. Read this first if you are new to noet.

## Quick Navigation

### For Users

- [Main README](../README.md) — Installation, quick start, and overview
- [DAG Model](design/dag_model.md) — Conceptual introduction: nodes, edges, the video camera query model
- [Architecture Overview](design/architecture.md) — Core concepts and how the library works
- [MCP Server](mcp.md) — Agent-facing tool documentation for querying via Model Context Protocol
- [API Reference](https://docs.rs/noet-core) — Generated from rustdoc (run `cargo doc --open`)

### For Contributors

- [Query Model](design/query_model.md) — Formal query algebra: traversal, composition, scoring, instruments
- [BeliefBase Architecture](design/beliefbase_architecture.md) — Detailed technical specification
- [Roadmap](project/ROADMAP.md) — Project history, current focus, and vision
- [Documentation Strategy](project/DOCUMENTATION_STRATEGY.md) — How documentation is organized
- [AGENTS.md](../AGENTS.md) — Guidelines for AI-assisted development
- [CONTRIBUTING.md](../CONTRIBUTING.md) — Development environment and pull request workflow

## Directory Structure

```
docs/
├── README.md                      # This file
├── mcp.md                         # MCP server setup and tool reference
├── design/                        # Architecture and design specifications
│   ├── dag_model.md               # Conceptual intro: the DAG model (start here)
│   ├── architecture.md            # High-level architecture guide
│   ├── beliefbase_architecture.md # Detailed technical specification
│   └── query_model.md             # Formal query algebra specification
└── project/                       # Project management documents
    ├── ROADMAP*.md                # Version roadmaps and planning
    ├── ISSUE_*.md                 # Issue tracking and specifications
    └── DOCUMENTATION_STRATEGY.md  # Documentation organization guide
```

## Documentation Levels

noet-core follows a **hierarchical documentation strategy**:

1. **Quick Start** → [`../README.md`](../README.md) — "Should I use this library?"
2. **Conceptual** → [`design/dag_model.md`](design/dag_model.md) — "Why a DAG? What are the three edge types?"
3. **Architectural** → [`design/architecture.md`](design/architecture.md) — "How does the compiler work?"
4. **Formal** → [`design/query_model.md`](design/query_model.md) — "How does the query algebra work?"
5. **Technical** → [`design/beliefbase_architecture.md`](design/beliefbase_architecture.md) — "How is it implemented?"
6. **API Reference** → [Rustdoc](https://docs.rs/noet-core) — "How do I call this function?"

See [DOCUMENTATION_STRATEGY.md](project/DOCUMENTATION_STRATEGY.md) for details on the single-source-of-truth approach.

## Design Documents (`design/`)

### [dag_model.md](design/dag_model.md)
Conceptual introduction to noet's multigraph model for engineers and agents
encountering noet for the first time.

**Contents**: Why flat documentation fails, the three edge types
(Section/Epistemic/Pragmatic), source/sink/owner directionality, the video camera
query model, composed queries as stereoscopic vision.

**Audience**: Anyone new to noet — engineers, evaluators, AI agents.

### [architecture.md](design/architecture.md)
High-level overview of the software architecture for developers getting started
with the library.

**Contents**: Core concepts (BID, BeliefBase, multi-pass compilation), architecture
components and data flow, relationship to prior art, getting started examples.

**Audience**: Developers learning the library.

### [query_model.md](design/query_model.md)
Formal specification of the query algebra.

**Contents**: Subject/projection/instrument decomposition, Score semiring,
NodeFilter and Traversal primitives, And/Or/Difference compositions, the Tape,
sort functions, render modes, textual query syntax, prior art grounding.

**Audience**: Contributors implementing or extending the query system.

### [beliefbase_architecture.md](design/beliefbase_architecture.md)
Complete technical specification for understanding internals and contributing.

**Contents**: Compilation model, identity management (BID, Bref, NodeKey), graph
structure and invariants, multi-pass reference resolution, codec system,
integration points, future enhancements.

**Audience**: Contributors, maintainers, advanced users.

**Note**: This is the **source of truth** for implementation details.

## Project Documents (`project/`)

**Purpose**: Project planning, issue tracking, and development guidelines.

- **[ROADMAP.md](project/ROADMAP.md)** — Project history, current focus, and vision
- **Issues** — Numbered sequentially as individual markdown files (`ISSUE_*.md`)

## Contributing

See [CONTRIBUTING.md](../CONTRIBUTING.md) for contribution guidelines.

For AI-assisted development, see [AGENTS.md](../AGENTS.md).