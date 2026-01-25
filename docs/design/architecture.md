# noet-core Architecture

**Purpose**: High-level overview of noet-core's architecture for developers getting started with the library.

**For detailed specifications**, see [`design/beliefbase_architecture.md`](./design/beliefbase_architecture.md).

## What is noet-core?

noet-core is a **hypergraph-based knowledge management library** that transforms interconnected documents (Markdown, TOML, etc.) into a queryable graph structure. It maintains **bidirectional synchronization** between human-readable source files and a machine-queryable hypergraph, automatically managing cross-document references and propagating changes.

The name "noet" comes from "noetic" - relating to knowledge and the intellect.

## Core Concepts

### 1. Documents → Graph Compilation

noet-core acts as a **compiler** for document networks:

```
Source Files (*.md, *.toml)
    ↓
[Parse] → Syntax analysis via DocCodec
    ↓
ProtoBeliefNode (Intermediate Representation)
    ↓
[Link] → Reference resolution via GraphBuilder
    ↓
BeliefBase (Compiled Hypergraph)
    ↓
[Query/Traverse] → Application logic
```

### 2. The BeliefBase: Your Document Graph

A **BeliefBase** is the compiled representation of your document network:

- **Nodes** (`BeliefNode`): Represent documents, sections, or custom entities
- **Edges** (typed relationships): Connect nodes with semantic meaning
- **Identifiers**: Each node has a stable UUID-based `Bid` (Belief ID)

**Key Feature**: The graph is always synchronized with source files - changes flow bidirectionally.

### 3. Multi-Pass Compilation

Like a traditional compiler, noet-core handles forward references and circular dependencies through multiple parse passes:

**Pass 1 - Discovery**: Parse all files, collect unresolved references as diagnostics  
**Pass 2+ - Resolution**: Reparse files with resolved dependencies, inject BIDs, create relations  
**Convergence**: Iterate until all resolvable references are linked

This means you can reference documents before they're parsed - the system figures it out automatically.

### 4. BID: Stable Identity

Every node gets a **Bid** (Belief ID) - a UUID injected into the source document:

```toml
# Before first parse (user-authored)
id = "my_document"
title = "My Document"

# After compilation (BID injected by system)
bid = "01234567-89ab-cdef-0123-456789abcdef"
id = "my_document"
title = "My Document"
```

**Why BIDs matter**:
- **Stable references**: Links survive file renames/moves
- **Cross-network merging**: Combine graphs without ID collisions
- **Distributed collaboration**: Unique IDs across all users

### 5. Relationship Types (WeightKind)

Edges are classified by infrastructure type:

- **Subsection**: Document structure (heading hierarchy)
- **Epistemic**: Knowledge dependencies (citations, prerequisites)
- **Pragmatic**: Domain-specific relationships (application-defined)

Each relationship can carry a `payload` with custom metadata.

### 6. Schema Extensibility

Nodes have an optional `schema` field for domain classification:

```rust
BeliefNode {
    bid: Bid("abc123..."),
    schema: Some("Action"),      // Application-specific
    payload: { /* custom fields */ },
}
```

**noet-core is schema-agnostic** - you define what schemas mean in your application.

## Architecture Overview

### Components

**[`beliefbase`](../src/beliefbase.rs)**: Core hypergraph data structures
- `BeliefBase`: Full-featured graph with indices and query operations
- `BeliefGraph`: Lightweight transport structure (states + relations only)
- `BidGraph`: Underlying petgraph representation

**[`codec`](../src/codec/mod.rs)**: Document parsing and synchronization
- `DocCodec` trait: Pluggable parsers for different formats
- `DocumentCompiler`: Queue-based multi-pass compilation orchestrator
- `GraphBuilder`: Stateful builder for constructing BeliefBases
- Built-in codecs: `MdCodec` (Markdown), `TomlCodec` (TOML)

**[`properties`](../src/properties.rs)**: Node and edge types
- `BeliefNode`: Node structure with BID, schema, payload
- `Bid`, `Bref`: Identity types
- `NodeKey`: Polymorphic reference (Bid, ID, Path)
- `WeightKind`: Edge classification

**[`event`](../src/event.rs)**: Event streaming for synchronization
- `BeliefEvent`: Node/relation add/update/remove events
- Enables reactive updates to graph changes

**[`query`](../src/query.rs)**: Query language for graph traversal
- Expression-based filtering
- Context queries (sources/sinks)
- Pagination support

**[`paths`](../src/paths.rs)**: Relative path resolution across nested networks

### Data Flow

```
1. File system changes detected
   ↓
2. DocumentCompiler enqueues modified files
   ↓
3. DocCodec parses file → ProtoBeliefNodes
   ↓
4. GraphBuilder resolves references → BeliefEvents
   ↓
5. BeliefBase updated, events published
   ↓
6. Application reacts to events, queries graph
```

## Multi-Pass Reference Resolution

The diagnostic-driven compilation model:

```rust
pub struct ParseContentResult {
    pub rewritten_content: Option<String>,  // BID injection, link updates
    pub diagnostics: Vec<ParseDiagnostic>,  // Errors, warnings, unresolved refs
}

pub enum ParseDiagnostic {
    UnresolvedReference(UnresolvedReference),  // Forward ref (will resolve later)
    SinkDependency { path, bid },               // This file references changed content
    Warning(String),
    Info(String),
}
```

**Key concept**: Unresolved references are **diagnostics, not errors**. The compiler tracks them and resolves automatically in subsequent passes.

## Relationship to Prior Art

### Knowledge Management Tools (Obsidian, Roam, Logseq)
✅ Bidirectional linking  
✅ Graph visualization  
✅ Markdown-based  
**+** Automatic BID injection for stable references  
**+** Multi-format support  
**+** Nested network hierarchies  
**+** Multi-pass forward reference resolution  

### Hypergraph Libraries (HGX, HIF)
✅ Directed, weighted hypergraph  
✅ Multiple relationship types  
**+** Document management focus  
**+** Bidirectional doc-graph sync  
**+** Diagnostic-driven unresolved reference tracking  

### Knowledge Graph Systems (Neo4j, Docs2KG)
✅ Document → graph construction  
✅ Multi-format parsing  
✅ Rich querying  
**+** Writes BIDs/links back to source  
**+** Three-way reconciliation (docs/cache/DB)  
**+** Auto-updating WikiLink titles  
**+** Source documents are authoritative  

### Language Servers (rust-analyzer, tree-sitter)
✅ Incremental, error-tolerant parsing  
✅ Diagnostic tracking  
✅ File watcher integration  
**+** Knowledge management domain  
**+** Writes back to source (BID injection)  
**+** Multi-pass cross-file resolution  

## Unique Features

1. **Bidirectional doc-graph sync**: Changes flow both ways via dynamic source blocks
2. **Diagnostic-driven compilation**: Multi-pass resolution guided by diagnostics
3. **Nested network paths**: Hierarchical dependencies with stable BID references
4. **Three-source reconciliation**: Parsed docs + local cache + global DB
5. **Continuous error-tolerant compilation**: Parsing never fails catastrophically
6. **Dynamic source blocks**: BID injection, auto-title references, path updates

## Features

Default: Core parsing and graph construction

**Optional features**:
- `service`: File watching, database integration (`notify`, `sqlx`, `futures-core`)
- `wasm`: WebAssembly support (`serde-wasm-bindgen`, `uuid/js`)

Enable features in `Cargo.toml`:
```toml
noet-core = { version = "0.0.0", features = ["service"] }
```

## Status

**Pre-1.0**: API may change. Feedback welcome!

This library is under active development. Expect breaking changes before v1.0.0.
