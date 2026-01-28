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

### 4. Node Identity: BID and Multi-ID Triangulation

Every node can be referenced through **multiple identity types** working together:

```rust
pub enum NodeKey {
    Bid { bid: Bid },                    // Globally unique UUIDv6
    Bref { bref: Bref },                 // 12-char compact reference
    Id { net: Bid, id: String },         // User-defined semantic ID
    Title { net: Bid, title: String },   // Auto-generated from title
    Path { net: Bid, path: String },     // Filesystem location
}
```

**Why multiple IDs?**

Each identity type serves different needs:

- **BID**: Globally unique UUIDv6 that survives all changes (renames, moves, title edits)
- **Bref**: Compact 12-char reference for links (derived from BID)
- **ID**: Optional user-defined identifier (e.g., `{#intro}` in markdown headings)
- **Title**: Auto-generated anchor from heading text
- **Path**: Filesystem location (changes on move)

**Example: BID injection lifecycle**
```toml
# Before: user creates file
title = "My Document"

# After: BID injected by system
bid = "01234567-89ab-cdef-0123-456789abcdef"
title = "My Document"

# Later: title changes, BID stays same
bid = "01234567-89ab-cdef-0123-456789abcdef"  # Stable!
title = "Updated Document"
```

**Benefits**:
- **Stable references**: Links survive file renames/moves (BID-based)
- **User control**: Optional explicit IDs for semantic anchors
- **Collision handling**: Automatic Bref fallback for duplicate titles
- **Distributed sync**: No ID collisions across devices

**For detailed specifications** including collision detection, normalization rules, and implementation details, see [`design/beliefbase_architecture.md` § 2.2](./design/beliefbase_architecture.md#22-identity-management-bid-bref-and-nodekey).

### 5. Link Format: Readable + Resilient

noet-core transforms markdown links to a **canonical format** that combines human-readable paths with stable Bref identifiers:

```markdown
[Link Text](relative/path.md#anchor "bref://abc123def456")
```

**Why this format?**

Traditional markdown links break when files are renamed or moved. noet-core solves this by:
- **Path**: Human-readable, portable relative path (updates automatically on moves)
- **Bref**: Stable 12-character identifier (never changes, even if file renamed)
- **Title attribute**: CommonMark-compliant storage (not a custom extension)

**Example transformation**:
```markdown
<!-- User writes -->
[Tutorial](./docs/intro.md)

<!-- After compilation -->
[Tutorial](docs/intro.md "bref://a1b2c3d4e5f6")
```

**Benefits**:
- Links survive file renames and moves (Bref-based resolution)
- Documents remain portable (relative paths)
- Compatible with standard markdown tools (uses CommonMark title attribute)
- Auto-updating link text (optional, via `auto_title` config)

**Supported input formats**:
- Standard markdown: `[text](path.md)`
- WikiLinks: `[[Document Name]]`
- Same-document anchors: `[text](#anchor)`
- Explicit Bref: `[text](path.md "bref://abc123")`

When files move or are renamed, noet-core automatically updates the paths while preserving the Bref, ensuring links never break.

**For detailed specification** including title attribute processing, path generation, and link resolution algorithm, see [`design/link_format.md`](./link_format.md).

### 6. Relationship Types (WeightKind)

Edges are classified by infrastructure type:

- **Subsection**: Document structure (heading hierarchy)
- **Epistemic**: Knowledge dependencies (citations, prerequisites)
- **Pragmatic**: Domain-specific relationships (application-defined)

Each relationship can carry a `payload` with custom metadata.

### 7. Schema Extensibility

Nodes have an optional `schema` field for domain classification:

```rust
BeliefNode {
    bid: Bid("abc123..."),
    schema: Some("Action"),      // Application-specific
    payload: { /* custom fields */ },
}
```

**noet-core is schema-agnostic** - you define what schemas mean in your application.

### 8. Metadata Format Flexibility

Document metadata (frontmatter) supports **three formats with automatic fallback**:

```yaml
# YAML (default, markdown ecosystem standard)
bid: "01234567-89ab-cdef-0123-456789abcdef"
schema: "intention_lattice.intention"
title: "My Document"
```

```json
// JSON (web/programmatic use)
{
  "bid": "01234567-89ab-cdef-0123-456789abcdef",
  "schema": "intention_lattice.intention",
  "title": "My Document"
}
```

```toml
# TOML (Hugo compatibility)
bid = "01234567-89ab-cdef-0123-456789abcdef"
schema = "intention_lattice.intention"
title = "My Document"
```

**How it works**:
- **Priority order**: YAML → JSON → TOML (tries formats in sequence)
- **Extension synonyms**: `.yaml`/`.yml`, `.json`/`.jsn`, `.toml`/`.tml`
- **Automatic fallback**: If parsing fails in one format, tries the next
- **Network files**: `BeliefNetwork.yaml`, `.json`, or `.toml` all supported
- **Full compatibility**: Existing JSON/TOML documents continue to work

This enables smooth adoption: start with familiar formats (JSON for web developers, TOML for Hugo users) and optionally migrate to YAML for markdown ecosystem alignment.

**Implementation**: See `src/codec/belief_ir.rs` for three-way parsing logic.

### 9. The API Node: Version Management and Entry Point

Every BeliefBase has a special **API node** that serves two critical purposes:

1. **Version Management**: Like Cargo's version system, the API node tracks which version of noet-core's schema/format the graph uses. This enables:
   - Older noet-core versions to parse newer document trees (forward compatibility)
   - Schema evolution tracking across library updates
   - Migration paths when the data model changes

2. **Graph Entry Point**: The API node acts as the root for graph traversal:
   - All network nodes relate to the API node (Network → API)
   - PathMapMap uses it as the starting point for path resolution
   - Provides a universal anchor for distributed graphs

**Reserved Namespace**: The API node uses a reserved BID namespace to prevent collision with user nodes:

```rust
// API node BID is deterministic per version
let api_bid = buildonomy_api_bid("0.0.0");  // e.g., "5a29441c-37d2-5f41-b61b-5f62adeb9a44"

// Check if a BID is reserved
if bid.is_reserved() {
    // This BID is in the system namespace (API, href tracking, etc.)
}
```

**How it works**: All system BIDs have reserved namespace bytes (octets 10-15) that match `UUID_NAMESPACE_BUILDONOMY`. User files cannot use BIDs in this namespace - parsing will fail with a clear error.

**For detailed specification** including namespace checking algorithm and reserved identifier validation, see [`beliefbase_architecture.md` § 2.7](./beliefbase_architecture.md#27-the-api-node-versioning-and-reserved-namespace).

### 10. Extensible Document Parsing (DocCodec)

noet-core supports **multiple document formats** through a pluggable codec system:

```rust
pub trait DocCodec {
    fn parse(&mut self, content: String, current: ProtoBeliefNode) -> Result<(), BuildonomyError>;
    fn nodes(&self) -> Vec<ProtoBeliefNode>;
    fn inject_context(&mut self, node: &ProtoBeliefNode, ctx: &BeliefContext) -> Result<Option<BeliefNode>, BuildonomyError>;
    fn generate_source(&self) -> Option<String>;
}
```

**Built-in codecs**:
- **MdCodec** (`.md`) - Markdown with frontmatter, extracts heading hierarchy
- **TomlCodec** (`.toml`) - Standalone TOML files, schema-aware

**Register custom codecs** via the global `CODECS` registry:

```rust
use noet_core::codec::{CODECS, DocCodec};

#[derive(Default, Clone)]
struct OrgModeCodec;

impl DocCodec for OrgModeCodec {
    // Implement trait methods...
}

// Register for .org files
CODECS.insert::<OrgModeCodec>("org".to_string());
```

**Key principle**: Codecs handle **syntax only** (parsing documents into `ProtoBeliefNode` structures). The `GraphBuilder` handles **semantics** (resolving references, creating relations, managing identity).

**Example**: MdCodec parses headings into a stack-based hierarchy, but doesn't resolve cross-document links. The builder later matches `[[Document Name]]` references to actual BIDs during multi-pass compilation.

**For detailed specification** including the document stack algorithm and codec implementation details, see [`beliefbase_architecture.md` § 3.5-3.6](./beliefbase_architecture.md#35-doccodec-the-frontend-interface).

## Architecture Overview

### Components

**[`beliefbase`](../src/beliefbase.rs)**: Core hypergraph data structures
- `BeliefBase`: Full-featured graph with indices, query operations, and API node
- `BeliefGraph`: Lightweight transport structure (states + relations only)
- `BidGraph`: Underlying petgraph representation
- API node: Version management and graph entry point (automatically managed)

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
