---
title = "BeliefSet Architecture: The Compiler IR for Document Graphs"
authors = "Andrew Lyjak, Claude Code"
last_updated = "2025-01-17"
status = "Draft"
version = "0.2"
---

# BeliefSet Architecture

## 1. Purpose

This document specifies the architecture of the **BeliefSet** and **BeliefSetAccumulator**, the core data structures that transform source files into an executable graph representation. These components serve as the **compiler infrastructure** for document graph systems, bridging the gap between human-authored markdown/TOML files and runtime applications that query and manipulate the graph.

**Core Responsibilities:**

1. **Parse heterogeneous source formats** (Markdown, TOML) into a unified intermediate representation
2. **Resolve references** between nodes across file boundaries, creating a coherent graph
3. **Maintain structural invariants** ensuring the graph is valid and balanced
4. **Track identity mappings** between file paths, node IDs, and internal identifiers (BIDs)
5. **Support incremental updates** allowing source files to change while preserving graph consistency
6. **Enable bidirectional synchronization** between the in-memory graph and source files

The BeliefSet is not merely a data container—it is a compiled program representation where documents are connected through a rich, typed relationship graph.

## 2. Core Concepts

### 2.1. The Compilation Model

The BeliefSet architecture follows a multi-stage compilation pipeline analogous to traditional language compilers:

```
Source Files (*.md, *.toml)
    ↓
[Multi-Pass Orchestration] ← BeliefSetParser (work queue, file watching)
    ↓
[Lexing & Parsing] ← DocCodec implementations (TomlCodec, MdCodec)
    ↓
ProtoBeliefNode (Intermediate Representation)
    ↓
[Reference Resolution & Linking] ← BeliefSetAccumulator
    ↓
BeliefSet (Compiled Graph IR)
    ↓
[Runtime Execution] ← Application-specific query and traversal logic
    ↓
Event Stream
```

Each stage has distinct responsibilities:

- **BeliefSetParser**: Build system/compiler driver - orchestrates multi-pass compilation, manages work queue
- **DocCodec**: Lexer/parser - syntax analysis, producing unlinked ProtoBeliefNodes
- **BeliefSetAccumulator**: Semantic analyzer + linker - parsing context and reference resolution
- **BeliefSet**: Compiled IR - optimized graph representation with fast lookup indices
- **Runtime Applications**: Execution layer - query, traversal, and domain-specific logic

### 2.2. Identity Management: BID, Bref, and NodeKey

The system maintains three parallel identity schemes to handle the complexity of distributed, evolving documents:

**Bid (Belief ID):**
- A UUID-like globally unique identifier
- Persisted in source files (TOML frontmatter `bid` field)
- Primary key for graph nodes
- Stable across file renames, moves, and content changes

**Bref (Belief Reference):**
- A human-readable namespace derived from the BID (e.g., first 8 characters)
- Used for compact display and logging
- Maps to BID via `brefs: BTreeMap<Bref, Bid>` in BeliefSet

**NodeKey:**
- A polymorphic reference type used during parsing and linking
- Can represent:
  - `Bid { bid: Bid }` - Direct reference to a node
  - `Id { id: String, net: Bid }` - Human-readable ID within a network
  - `Path { ... }` - File system path reference
  - `UnresolvedRef { href: String }` - Deferred resolution (future enhancement)

The `IdMap` (beliefset.rs:617-657) and `PathMapMap` structures maintain bidirectional mappings between these identity schemes, enabling fast lookups in either direction.

### 2.3. Schema vs Kind: Semantic Distinction

BeliefNode has two fields that might appear similar but serve fundamentally different purposes:

**`schema: Option<String>` - Domain Classification:**
- Defines what kind of entity this represents in the application domain
- Examples: `"Action"`, `"Document"`, `"Section"`, `"CustomType"`
- Used by schema parsers to determine which fields are valid in `payload`
- Queryable by domain logic
- Schema-agnostic to BeliefSet core infrastructure

**`kind: EnumSet<BeliefKind>` - Infrastructure Metadata:**
- Tracks provenance and compiler handling requirements
- Examples: `Http` (external web reference), `Anchored` (has source file), `Document` (file root)
- Used by compilation system for multi-pass resolution and BID injection
- Multiple flags can be active simultaneously via `EnumSet`
- Core infrastructure concern, not domain-specific

**Example:**
```rust
BeliefNode {
    bid: Bid("abc123..."),
    kind: BeliefKindSet::from(EnumSet::from(BeliefKind::Document)),  // Infrastructure: real file
    schema: Some("CustomSchema".to_string()),   // Domain: application-specific
    title: "My Document",
    payload: { /* schema-specific fields */ },
}
```

Infrastructure asks: "Is this external? Does it have a file? Can I access its contents? Do I have a comprehensive map of its relationships?"
Domain asks: "What schema defines this node's structure?"

### 2.4. Graph Structure and Invariants

The BeliefSet maintains a **typed, weighted, directed acyclic graph (DAG)** where:

- **Nodes** are `BeliefNode` instances (states in the graph)
- **Edges** are typed relationships with `WeightKind` infrastructure classification
- **Sub-graphs** exist per `WeightKind`, each forming its own DAG

**WeightKind Architecture**:

`WeightKind` is a simple enum classifying edge infrastructure types:

```rust
pub enum WeightKind {
    Subsection,   // Document structure edges
    Epistemic,    // Knowledge dependency edges
    Pragmatic,    // Domain-specific relationship edges
}
```

**Crucially, WeightKind variants carry NO semantic payload.** All semantic information is stored in the `Relation.payload` field:

- For **Pragmatic edges** (domain relationships), the payload contains application-specific metadata
- For **Epistemic edges** (knowledge dependencies), the payload contains dependency metadata
- For **Subsection edges** (document structure), the payload contains structural metadata

This design separates **graph infrastructure concerns** (WeightKind) from **domain semantics** (payload), enabling clean separation of graph algorithms from domain-specific relationship logic.

**Static Invariants (verified by `BeliefSet::built_in_test()`):**

1. **No cycles within any WeightKind sub-graph** - Each relationship type forms a DAG
2. **Sink nodes have corresponding states** - Every node referenced in a relationship exists
3. **API paths are consistent** - All structural subsections have valid path mappings
4. **Deterministic ordering** - Edges are ordered by weight, enabling deterministic traversal

**Operational Rules:**

1. **Link directionality**: Parent (sink) → Child (source)
   - The parent "consumes" or "references" the child
   - Counterintuitive for subsections, but consistent: parent indexes child content

2. **Network vs. Document nodes**:
   - `Network` nodes represent repository roots (BeliefNetwork.toml files)
   - `Document` nodes represent individual source files

### 2.5. Multi-Component Architecture: Parser → Accumulator → Set

**BeliefSetParser** (codec/parser.rs):
- Orchestrates multi-pass compilation across multiple files
- Manages work queue with priority ordering
- Handles file watching and incremental updates
- Coordinates which files get parsed when
- Drives the compilation process to convergence

**BeliefSetAccumulator** (codec/mod.rs):
- Stateful builder for constructing a BeliefSet
- Parses files via DocCodec implementations
- Maintains parsing state across multiple files
- Implements a **document stack** for tracking nested structure during parsing
- Resolves relative references to absolute BIDs (linking)
- Publishes `BeliefEvent` updates via an async channel

**BeliefSet** (beliefset.rs):
- Immutable (logically) snapshot of the graph
- Optimized for queries and graph traversals
- Thread-safe via `Arc<RwLock<BidGraph>>` for concurrent reads
- Provides graph operations (union, intersection, difference, filtering)

The architecture maps to traditional compilers as:
- **DocCodec** → Lexer/Parser (syntax-level)
- **BeliefSetAccumulator** → Semantic analyzer + Linker (parsing + reference resolution)
- **BeliefSetParser** → Build system/Compiler driver (orchestration, multi-pass)
- **BeliefSet** → Compiled IR/Executable (queryable result)

## 3. Architecture

### 3.0. System Overview

The complete compilation system consists of multiple cooperating layers:

```
┌─────────────────────────────────────────────────────────────┐
│                  Application Layer                          │
│  (Domain-specific services, UI, query interfaces)           │
└────────────┬────────────────────────────────────────────────┘
             │
             ├── Uses ──────────────────────────────────┐
             │                                          │
             ▼                                          ▼
┌────────────────────────┐              ┌────────────────────────────┐
│   File Watcher         │              │   DbConnection             │
│   (notify-debouncer)   │◄───── ──────►│   (Persistent Cache)       │
└───────┬────────────────┘              └────────────────────────────┘
        │
        │ File system events
        ▼
┌────────────────────────────────────────────────────────────────────┐
│              BeliefSetAccumulator                                  │
│  (Parser/Linker - Converts files → graph)                          │
└──────────┬─────────────────────────────────────────────────────────┘
           │
           ├── Uses ────────────────────────────────┐
           │                                        │
           ▼                                        ▼
┌────────────────────┐              ┌────────────────────────────────┐
│   DocCodec         │              │      BeliefSet                 │
│   (TomlCodec,      │              │   (Compiled Graph IR)          │
│    MdCodec)        │              │                                │
└────────────────────┘              └────────────────────────────────┘
           │                                        │
           │ Parses                                 │ Queries
           ▼                                        ▼
┌────────────────────┐              ┌────────────────────────────────┐
│  Source Files      │              │   Application Logic            │
│  (*.md, *.toml)    │              │   (Domain-specific processing) │
└────────────────────┘              └────────────────────────────────┘
```

**Data Flow:**
1. File watcher detects changes → triggers parsing
2. Parser uses DocCodec → produces ProtoBeliefNodes
3. Accumulator resolves references → emits BeliefEvents
4. Events update database and application state
5. Applications query BeliefSet for graph traversal

**Multi-Pass Reference Resolution:**

The system implements automatic multi-pass compilation to handle forward references, circular dependencies, and incremental updates using diagnostic-based tracking. The `parse_content` method returns:

```rust
pub struct ParseContentResult {
    pub rewritten_content: Option<String>,  // Rewritten content (BID injection)
    pub diagnostics: Vec<ParseDiagnostic>,  // Unresolved refs, warnings, info
}

pub enum ParseDiagnostic {
    UnresolvedReference(UnresolvedReference),  // Missing target during parse
    SinkDependency {
        /// Path to the sink document (relative to repo root)
        path: String,
        /// BID of the sink document
        bid: crate::properties::Bid,
    },
    Warning(String),
    Info(String),
}

pub async fn parse_content(...) -> Result<ParseContentResult, BuildonomyError>
```

**Key Concept - Source/Sink Semantics:**
- **Source** = content provider (information origin)
- **Sink** = content consumer (information accessor)
- Example: `Document (sink) ← contains ← Section (source)`
- Example: `Text with link (sink) ← links to ← Referenced doc (source)`

**Resolution Algorithm:**

1. **First Pass - Virgin Repository**: Parse files with no prior context
   - Target not yet parsed → `cache_fetch()` returns `GetOrCreateResult::Unresolved(...)`
   - Collect `UnresolvedReference` diagnostic (no relation created yet)
   - Parser tracks unresolved refs for later resolution checking
   - Continue parsing all files

2. **Propagation**: Parser-driven resolution checking
   - After parsing each file, check if it resolves any tracked unresolved refs
   - If resolved: create relation via `RelationInsert` event
   - If NodeKey type requires rewrite (Path, Title): enqueue source file for reparse
   - Queue managed by `BeliefSetParser` with priority ordering

3. **Subsequent Passes**: Reparse files after dependencies resolve
   - Previously missing targets now exist in cache
   - Links resolve to concrete BIDs
   - Inject BIDs into source files (rewritten content)

4. **Convergence**: Iterate until all resolvable refs are resolved
   - Internal refs resolve after full tree parse
   - External refs or typos remain in unresolved list

**Example - Initial Parse with Auto-Title (WikiLinks):**
```
Parse File A → contains WikiLink: [[ file_b ]]
  → cache_fetch(NodeKey::Path("B")) → GetOrCreateResult::Unresolved
  → Diagnostic collected: UnresolvedReference { 
      self_path: "A", 
      other_key: Path("B"),
      weight_data: { auto_title: true }  // WikiLinks always have this
    }
  → No relation created
  → Continue parsing

Parse File B → creates node with BID and title "File B Title"
  → Node added to self.set, transmitted to global cache

Parser checks unresolved refs → finds B now resolvable
  → can_resolve_key(Path("B")) → true
  → create_resolved_relation() → emits RelationInsert event
  → should_rewrite_for_key(unresolved) → checks auto_title=true → YES
  → Enqueue A for reparse

Reparse File A → link now resolves to BID, auto-populates title
  → cache_fetch(NodeKey::Path("B")) → GetOrCreateResult::Resolved(bid, ...)
  → Relation created
  → Content rewritten: [[ file_b ]] with BID reference and auto-title
  → No unresolved ref diagnostic
```

**Example - Incremental Update (File Watcher with Auto-Title):**
```
File sub_2.md subsection title changes from "Old Title" to "New Title"
  → File watcher triggers reparse of sub_2.md
  → Phase 3.2 sink detection: check relations for auto_title=true
  → Cache returns: README.md has WikiLink [[ sub_2 ]] with auto_title=true
  → Emit SinkDependency diagnostic for README.md (WikiLinks auto-update)
  → Enqueue README.md for reparse with reset_processed()
  → README.md reparsed → WikiLink auto-updated with new title

Note: If README had [Custom Text](sub_2) (regular MD link), auto_title not set,
      no SinkDependency emitted, link text stays "Custom Text"
```

This enables parsing files in any order while maintaining referential integrity. The `UnresolvedReference` diagnostic tracks missing targets, and the parser's resolution checking ensures convergence.

### 3.1. BeliefSetAccumulator: Parsing and Linking

The accumulator is responsible for parsing individual files and linking references across the document network. It is driven by `BeliefSetParser`, which orchestrates the multi-pass compilation process.

**Key Data Structures:**

```rust
pub struct BeliefSetAccumulator {
    pub parsed_content: BTreeSet<Bid>,    // Nodes parsed from content
    pub parsed_structure: BTreeSet<Bid>,  // Nodes generated from structure (headings)
    pub set: BeliefSet,                   // The compiled graph
    repo: Bid,                            // Root network BID
    repo_root: PathBuf,                   // File system anchor
    pub stack: Vec<(Bid, String, usize)>, // Document parsing stack (bid, heading, level)
    pub stack_cache: BeliefSet,           // Temporary cache during parsing
    tx: UnboundedSender<BeliefEvent>,     // Event publication channel
}
```

**Parsing Algorithm (codec/mod.rs:733-1057):**

1. **Initialization** (`initialize_network`): 
   - Parse BeliefNetwork.toml to establish repository root node
   - Set up identity mappings for the network

2. **Document Iteration** (`parse_content`):
   - Discover all matching files via `iter_docs()`
   - For each file:
     - Detect schema type from file path
     - Select appropriate `DocCodec` (TomlCodec, MdCodec)
     - Parse into `ProtoBeliefNode` instances

3. **Stack-Based Structural Parsing**:
   - Markdown headings create a nested structure
   - `initialize_stack()`: Push nodes onto stack as headings are encountered
   - `terminate_stack()`: Pop nodes and create subsection relationships
   - Stack depth corresponds to heading level (H1, H2, H3, etc.)

4. **Reference Resolution** (`push_relation`):
   - Convert `NodeKey` references to `Bid` using identity maps
   - Handle cross-file references via path resolution
   - Create typed edges in the graph

5. **Node Insertion** (`cache_fetch`):
   - Check if node already exists (by BID, ID, or path)
   - Merge new content with existing node
   - Update indices (`IdMap`, `PathMapMap`, `brefs`)

**Key Insight**: The stack-based approach enables **streaming parsing** of large document trees without loading entire files into memory first.

### 3.2. BeliefSet vs Beliefs: Full API vs Transport Layer

The codebase maintains two distinct but related structures for representing compiled graphs:

**Beliefs: Lightweight Transport Structure**

```rust
pub struct Beliefs {
    pub states: BTreeMap<Bid, BeliefNode>,
    pub relations: BidGraph,
}
```

`Beliefs` is a minimal structure optimized for:
- **Query results**: Database queries return `Beliefs` (see query.rs:735-739, `ResultsPage<Beliefs>`)
- **Network transport**: Serialization between services
- **Set operations**: Union, intersection, difference operations (beliefset.rs:313-455)
- **Pagination**: Breaking large graphs into pages (beliefset.rs:546-601)

It contains only the essential graph data (states + relations) without the indexing overhead.

**BeliefSet: Full-Featured API**

```rust
pub struct BeliefSet {
    states: BTreeMap<Bid, BeliefNode>,              // Node storage
    relations: Arc<RwLock<BidGraph>>,               // Edge storage (petgraph)
    bid_to_index: RwLock<BTreeMap<Bid, NodeIndex>>, // BID → graph index
    index_dirty: AtomicBool,                        // Lazy reindexing flag
    brefs: BTreeMap<Bref, Bid>,                     // Short ref → BID
    ids: IdMap,                                      // ID ↔ BID mapping
    paths: PathMapMap,                               // Path ↔ BID mapping
    errors: Option<Vec<String>>,                     // Validation errors
    api: BeliefNode,                                 // Special API root node
}
```

`BeliefSet` is the full-featured structure providing:
- **Identity resolution**: Multiple lookup indices (BID, Bref, ID, Path)
- **Graph operations**: Context queries, traversals, filtering
- **Validation**: Invariant checking via `built_in_test()`
- **Incremental updates**: Event processing and diff computation
- **Thread-safe access**: Arc/RwLock for concurrent reads

**Conversion Pattern:**

```rust
impl From<Beliefs> for BeliefSet {
    fn from(beliefs: Beliefs) -> Self {
        BeliefSet::new_unbalanced(beliefs.states, beliefs.relations, true)
    }
}
```

Query results come back as `Beliefs`, which can be converted to `BeliefSet` when full API access is needed. This separation enables:
- Efficient pagination without building full indices for every page
- Lightweight serialization over network boundaries
- Fast set operations on query results before materializing as BeliefSet

**Usage Pattern:**

```rust
// Query returns lightweight Beliefs
let page: ResultsPage<Beliefs> = service.get_states(paginated_query).await?;

// Convert to BeliefSet for full API access
let belief_set: BeliefSet = page.results.into();

// Now can use full API
let context = belief_set.get_context(some_bid)?;
```

**Graph Operations:**

1. **Set Operations** (union, intersection, difference):
   - Combine multiple BeliefSets (e.g., merging branches)
   - Used for computing deltas between versions

2. **Filtering** (`filter_states`, `filter_paths`):
   - Extract subgraphs by node properties or path patterns
   - Enable scoped queries (e.g., "all documents under /docs")

3. **Graph Traversal** (`get_context`, `evaluate_expression`):
   - Compute sources/sinks for a node
   - Walk parent/child relationships by WeightKind

4. **Incremental Updates** (`process_event`):
   - Handle add/remove/update events from accumulator
   - Maintain invariants during mutations

**Lazy Indexing:**
The `bid_to_index` mapping is rebuilt only when `index_dirty` is set, enabling batched updates without per-operation overhead. This is analogous to incremental compilation in modern compilers.

### 3.3. DocCodec: The Frontend Interface

The `DocCodec` trait defines the contract for file format parsers:

```rust
pub trait DocCodec {
    fn parse(&mut self, content: String, current: ProtoBeliefNode) -> Result<(), BuildonomyError>;
    fn nodes(&self) -> Vec<ProtoBeliefNode>;
    fn inject_context(&mut self, node: &ProtoBeliefNode, ctx: &BeliefContext) -> Result<Option<BeliefNode>, BuildonomyError>;
    fn is_changed(&self) -> bool;
    fn generate_source(&self) -> Option<String>;
}
```

**Current Implementations:**

- **TomlCodec** (lattice_toml.rs): Parses standalone TOML files and TOML frontmatter
  - Schema-aware: Can detect schema type from file path or frontmatter
  - Preserves formatting via `toml_edit::DocumentMut`
  - Extensible for custom relationship field handling

- **MdCodec** (md.rs): Parses Markdown with TOML frontmatter
  - Extracts frontmatter as ProtoBeliefNode payload
  - Parses headings to create structural hierarchy
  - Extracts code blocks and other structural elements

**Key Responsibility**: Codecs are **syntax-only**. They produce ProtoBeliefNodes with unresolved references (NodeKey instances). The accumulator handles semantic analysis and linking.

### 3.4. The Document Stack: Nested Structure Parsing

The stack mechanism (codec/mod.rs:1160-1402) is a critical innovation enabling hierarchical document parsing:

**Stack Entry**: `(Bid, String, usize)` = (node BID, heading text, heading level)

**Algorithm**:
```
On encountering heading H at level N:
1. Pop all stack entries with level >= N
2. For each popped entry, create Subsection edge to current node
3. Push H onto stack at level N
4. Set current node as child of stack top (if exists)
```

**Example**:
```markdown
# Top Level (L1)
## Section A (L2)
### Subsection A1 (L3)
## Section B (L2)
```

**Stack evolution**:
```
After "Top Level":   [(top_bid, "Top Level", 1)]
After "Section A":   [(top_bid, "Top Level", 1), (a_bid, "Section A", 2)]
After "Subsection":  [(top_bid, "Top Level", 1), (a_bid, "Section A", 2), (a1_bid, "Subsection A1", 3)]
After "Section B":   [(top_bid, "Top Level", 1), (b_bid, "Section B", 2)]
                     ^ a1 and a are popped, edges created
```

This creates the **Structural Hierarchy** of Subsection relationships, enabling table-of-contents generation and scoped queries.

## 4. Integration Points

### 4.1. Upstream: Source Files
- Reads: `*.md`, `*.toml` files via `iter_docs()`
- Writes: Updates BIDs and titles via `inject_context()` and `generate_source()`
- Watches: File system monitoring via `notify-debouncer`

### 4.2. Downstream: Runtime Applications
- Applications query the compiled graph for domain-specific logic
- Graph operations enable complex traversals and filtering
- Event stream provides reactive updates to graph changes

### 4.3. Event System
- **BeliefEvent Stream**: Accumulator → Applications
- Event types: NodeAdded, NodeRemoved, NodeUpdated, RelationChanged
- Async channels enable non-blocking updates
- Event batching prevents UI thrashing during bulk changes

### 4.4. Persistence Layer
- **Database**: SQLite-based persistent cache (accessed via `DbConnection`)
- **Config Files**: Configuration storage for network registry
- **Network Files**: `BeliefNetwork.toml` per repository root
- **Query Cache**: In-memory pagination cache with automatic invalidation

### 4.5. UI Layer
- Query interfaces for filtered graph views
- Content access for file editing
- Event subscription for reactive rendering

## 5. Examples

### 5.1. Parsing a Simple Document

**Source File** (`/docs/example.md`):
```markdown
---
id = "doc_example"
title = "Example Document"
schema = "Document"
---

# Example Document

This is an example document with content.

## Section 1

Content for section 1.

## Section 2

Content for section 2.
```

**Parsing Steps**:
1. MdCodec extracts frontmatter → ProtoBeliefNode with `id`, `title`, `schema`
2. Codec parses headings → Creates hierarchy nodes for each section
3. Accumulator resolves references and creates structural relationships
4. BeliefSet stores nodes with Subsection edges representing hierarchy

**Resulting Graph**:
```
doc_example (Document root)
   ↓ (Subsection)
Section 1
   ↓ (Subsection)
Section 2
```

### 5.2. Stack-Based Heading Resolution

**Source** (`/docs/guide.md`):
```markdown
# User Guide

## Getting Started
Content about getting started...

### Installation
Details about installation...

## Advanced Topics
Content about advanced topics...
```

**Stack Evolution**:
```
After "User Guide":      [(guide_bid, "User Guide", 1)]
After "Getting Started": [(guide_bid, "User Guide", 1), (start_bid, "Getting Started", 2)]
After "Installation":    [(guide_bid, "User Guide", 1), (start_bid, "Getting Started", 2), (install_bid, "Installation", 3)]
After "Advanced Topics": [(guide_bid, "User Guide", 1), (adv_bid, "Advanced Topics", 2)]
```

**Resulting Subsection Edges**:
- User Guide → Getting Started (Subsection)
- Getting Started → Installation (Subsection)
- User Guide → Advanced Topics (Subsection)

## 6. Architectural Concerns and Future Enhancements

Based on the architectural analysis, the following concerns require attention:

### 6.1. Schema Awareness Coupling

**Current State**: Codec implementations may contain schema-specific parsing logic that switches on file path patterns or frontmatter fields.

**Concern**: This tightly couples syntax parsing to semantic knowledge, violating separation of concerns. As more schema types are added, codecs can grow with conditional logic.

**Proposed Solution: Schema Registry and Extension System**

Introduce a **layered abstraction** where BeliefSet remains schema-agnostic, and applications can register custom schema handlers:

**Architecture:**

```
Application Schemas (domain-specific)
    ↓ Registered via SchemaRegistry
Schema-Aware Layer (application code)
  - Knows about domain-specific types
  - Implements schema-specific parsing
    ↓ Produces
BeliefSet Infrastructure (beliefset.rs)
  - Generic graph operations
  - schema: Option<String> (opaque)
  - payload: toml::Table (opaque)
  - NO knowledge of application schemas
```

**Benefits:**

1. **BeliefSet remains schema-agnostic** - Can be used for any graph domain
2. **Extensible** - Applications can add schema types without modifying library
3. **No manual changes required** - Schema logic stays in application layer
4. **Query by type** - Can filter `schema` without BeliefSet knowing domain semantics

### 6.2. Reference Resolution Timing

**Status**: ✅ **Already Resolved**

The system already implements multi-pass reference resolution via the BeliefSetParser and UnresolvedReference System. See Section 3.0 for details on the multi-pass algorithm.

**Key mechanism**: `ParseDiagnostic::UnresolvedReference` diagnostics track unresolved references, and the `parse_content` return signature (`ParseContentResult` with `diagnostics: Vec<ParseDiagnostic>`) drives automatic convergence through iterative reparsing.

### 6.3. Error Recovery and Partial Compilation

**Current State**: The parser continues processing files even when individual files fail to parse, logging errors and continuing with other files.

**Assessment**: Partial error recovery already exists at the file level. Within-file error recovery (continuing after syntax errors within a single document) is not currently implemented.

**Decision**: **Defer** - Current approach provides valuable architectural feedback during development. File-level recovery is sufficient for most use cases.

When needed, fine-grained error recovery within documents could be implemented by:
- Extending `ProtoBeliefNode` with an `errors: Vec<ParseError>` field
- Allowing partial node construction (e.g., node created but some relationships failed)
- Marking invalid nodes with `BeliefKind::Invalid` flag for UI feedback

### 6.4. Intermediate Representation Optimization

**Current State**: BeliefSet directly represents parsed structure without optimization passes.

**Assessment**: Current architecture is already quite efficient:
- Lazy indexing (`index_dirty` flag) - Rebuilds only when needed
- Arc-based structural sharing - Clone is cheap
- Multi-pass compilation - Natural convergence without explicit optimization

**Decision**: **Defer to Database Layer**

Optimization is better suited for the **DbConnection** persistent cache (db.rs) rather than the in-memory BeliefSet. The database serves as the "global cache" and can maintain optimized views:

**Proposed Approach:**

1. **Database maintains optimized projections**:
   - Pre-computed traversals
   - Materialized views for common queries
   - Deduplicated data with references

2. **DbConnection suggests optimizations back to source**:
   - Detect unreachable nodes → suggest pruning
   - Find duplicate content → suggest refactoring
   - Identify unused references → suggest cleanup

**Benefits:**

1. **Separation of concerns** - BeliefSet stays simple, database handles optimization
2. **Persistent optimization** - Computed once, cached across sessions
3. **User-driven** - Suggestions reviewed by human, not auto-applied
4. **Analytics-friendly** - Database can track usage patterns for better suggestions

### 6.5. Concurrent Parsing

**Current State**: Files are parsed sequentially in the parser thread work queue.

**Decision**: **Defer**

While concurrent parsing could improve throughput for large document sets (100+ files), it introduces complexity:

1. **Cache consistency challenges**: Multiple threads updating `BeliefSetAccumulator` simultaneously requires careful locking
2. **Multi-pass coordination**: The diagnostic-based unresolved reference resolution algorithm depends on parse ordering for convergence
3. **Limited bottleneck**: Parsing is already fast; transaction batching and DB writes are typically the bottleneck
4. **Complexity vs. gain**: Tokio async already provides concurrency for I/O; CPU-bound parsing parallelism adds minimal benefit

**Future approach** (when needed):
- Parse independent files concurrently in Phase 1 (no shared state)
- Synchronize before Phase 2 (reference resolution with shared accumulator)
- Use work-stealing queue for dynamic load balancing
- Benchmark to confirm bottleneck before implementing

### 6.6. Formal Grammar Specification

**Status**: For future consideration

**Current State**: Parsing logic is embedded in Rust code without formal grammar definition.

**Future Direction**: A schema registry system could provide declarative parsing rules that serve as a formal specification. Applications could define schemas declaratively, and the library could generate or validate parsing logic based on these specifications.

This would provide benefits similar to parser generators while maintaining flexibility for domain-specific parsing needs.

---

**Document Status**: Draft - This document captures the core architecture for the noet-core library, focusing on the graph compilation infrastructure that can be used by various applications.
