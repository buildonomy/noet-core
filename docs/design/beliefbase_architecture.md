---
title = "BeliefBase Architecture: The Compiler IR for Document Graphs"
authors = "Andrew Lyjak, Claude Code"
last_updated = "2025-01-17"
status = "Draft"
version = "0.2"
---

# BeliefBase Architecture

## 1. Purpose

This document specifies the architecture of the **beliefbase** and **GraphBuilder**, the core data structures that transform source files into an executable graph representation. These components serve as the **compiler infrastructure** for document graph systems, bridging the gap between human-authored markdown/TOML files and runtime applications that query and manipulate the graph.

**Core Responsibilities:**

1. **Parse heterogeneous source formats** (Markdown, TOML) into a unified intermediate representation
2. **Resolve references** between nodes across file boundaries, creating a coherent graph
3. **Maintain structural invariants** ensuring the graph is valid and balanced
4. **Track identity mappings** between file paths, node IDs, and internal identifiers (BIDs)
5. **Support incremental updates** allowing source files to change while preserving graph consistency
6. **Enable bidirectional synchronization** between the in-memory graph and source files

The BeliefBase is not merely a data container—it is a compiled program representation where documents are connected through a rich, typed relationship graph.

## 2. Core Concepts

### 2.1. The Compilation Model

The BeliefBase architecture follows a multi-stage compilation pipeline analogous to traditional language compilers:

```
Source Files (*.md, *.toml)
    ↓
[Multi-Pass Orchestration] ← DocumentCompiler (work queue, file watching)
    ↓
[Lexing & Parsing] ← DocCodec implementations (TomlCodec, MdCodec)
    ↓
ProtoBeliefNode (Intermediate Representation)
    ↓
[Reference Resolution & Linking] ← GraphBuilder
    ↓
BeliefBase (Compiled Graph IR)
    ↓
[Runtime Execution] ← Application-specific query and traversal logic
    ↓
Event Stream
```

Each stage has distinct responsibilities:

- **DocumentCompiler**: Build system/compiler driver - orchestrates multi-pass compilation, manages work queue
- **DocCodec**: Lexer/parser - syntax analysis, producing unlinked ProtoBeliefNodes
- **GraphBuilder**: Semantic analyzer + linker - parsing context and reference resolution
- **BeliefBase**: Compiled IR - optimized graph representation with fast lookup indices
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
- Maps to BID via `brefs: BTreeMap<Bref, Bid>` in BeliefBase

**NodeKey:**
- A polymorphic reference type used during parsing and linking
- Can represent:
  - `Bid { bid: Bid }` - Direct reference to a node
  - `Id { id: String, net: Bid }` - Human-readable ID within a network
  - `Path { ... }` - File system path reference
  - `UnresolvedRef { href: String }` - Deferred resolution (future enhancement)

The `IdMap` (beliefbase.rs:617-657) and `PathMapMap` structures maintain bidirectional mappings between these identity schemes, enabling fast lookups in either direction.

### 2.3. Schema vs Kind: Semantic Distinction

BeliefNode has two fields that might appear similar but serve fundamentally different purposes:

**`schema: Option<String>` - Domain Classification:**
- Defines what kind of entity this represents in the application domain
- Examples: `"Action"`, `"Document"`, `"Section"`, `"CustomType"`
- Used by schema parsers to determine which fields are valid in `payload`
- Queryable by domain logic
- Schema-agnostic to BeliefBase core infrastructure

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

The BeliefBase maintains a **typed, weighted, directed acyclic graph (DAG)** where:

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

**Static Invariants (verified by `BeliefBase::built_in_test()`):**

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

### 2.5. Multi-Component Architecture: Compiler → Builder → Set

**DocumentCompiler** (codec/compiler.rs):
- Orchestrates multi-pass compilation across multiple files
- Manages work queue with priority ordering
- Handles file watching and incremental updates
- Coordinates which files get parsed when
- Drives the compilation process to convergence

**GraphBuilder** (codec/mod.rs):
- Stateful builder for constructing a BeliefBase
- Parses files via DocCodec implementations
- Maintains parsing state across multiple files
- Implements a **document stack** for tracking nested structure during parsing
- Resolves relative references to absolute BIDs (linking)
- Publishes `BeliefEvent` updates via an async channel

**BeliefBase** (beliefbase.rs):
- Immutable (logically) snapshot of the graph
- Optimized for queries and graph traversals
- Thread-safe via `Arc<RwLock<BidGraph>>` for concurrent reads
- Provides graph operations (union, intersection, difference, filtering)

The architecture maps to traditional compilers as:
- **DocCodec** → Lexer/Parser (syntax-level)
- **GraphBuilder** → Semantic analyzer + Linker (parsing + reference resolution)
- **DocumentCompiler** → Build system/Compiler driver (orchestration, multi-pass)
- **BeliefBase** → Compiled IR/Executable (queryable result)

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
┌────────────────────────────────────────────────────────────┐
│              GraphBuilder                                  │
│  (Parser/Linker - Converts files → graph)                  │
└──────────┬─────────────────────────────────────────────────┘
           │
           ├── Uses ────────────────────────────────┐
           │                                        │
           ▼                                        ▼
┌────────────────────┐              ┌────────────────────────────────┐
│   DocCodec         │              │      BeliefBase                │
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
3. Builder resolves references → emits BeliefEvents
4. Events update database and application state
5. Applications query BeliefBase for graph traversal

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
   - Compiler tracks unresolved refs for later resolution checking
   - Continue parsing all files

2. **Propagation**: Compiler-driven resolution checking
   - After parsing each file, check if it resolves any tracked unresolved refs
   - If resolved: create relation via `RelationInsert` event
   - If NodeKey type requires rewrite (Path, Title): enqueue source file for reparse
   - Queue managed by `DocumentCompiler` with priority ordering

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
  → Node added to self.doc_bb, transmitted to global cache

Compiler checks unresolved refs → finds B now resolvable
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

This enables parsing files in any order while maintaining referential integrity. The `UnresolvedReference` diagnostic tracks missing targets, and the compiler's resolution checking ensures convergence.

### 3.1. GraphBuilder: Parsing and Linking

The accumulator is responsible for parsing individual files and linking references across the document network. It is driven by `DocumentCompiler`, which orchestrates the multi-pass compilation process.

**Key Data Structures:**

```rust
pub struct GraphBuilder {
    pub parsed_content: BTreeSet<Bid>,    // Nodes parsed from content
    pub parsed_structure: BTreeSet<Bid>,  // Nodes generated from structure (headings)
    pub set: BeliefBase,                  // The compiled graph
    repo: Bid,                            // Root network BID
    repo_root: PathBuf,                   // File system anchor
    pub stack: Vec<(Bid, String, usize)>, // Document parsing stack (bid, heading, level)
    pub session_bb: BeliefBase,          // Temporary cache during parsing
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

### 3.2. The Codec System: Three Sources of Truth

The `GraphBuilder` mediates between three sources of truth during parsing:

1. **The Parsed Document** (source of truth for text and ordering)
   - Absolute authority for its own content
   - Defines the sequence of subsections
   - The builder must trust this order implicitly
   - Changes here trigger cache updates

2. **The Local Cache (`self.doc_bb`)** (source of truth for current parse state)
   - In-memory representation of the filesystem tree being parsed
   - Resolves cross-document links within the same filesystem
   - Represents the **NEW state** being built from parsing
   - Source of truth for what documents currently contain

3. **The Global Cache (Database)** (source of truth for identity)
   - Persistent canonical store of all `BeliefNode`s
   - Ultimate authority for BIDs (Belief IDs)
   - Canonicalizes references across different filesystems/networks
   - Queried to resolve node identities

**The Core Challenge**: The builder generates:
1. `BeliefEvent`s that update the global cache to reflect source documents
2. Context to inject BIDs back into source documents for absolute references

This synchronization enables cross-document and cross-project coordination. For example, if a subsection title changes within a document, external documents can be updated to reflect the new title in their link text.

#### Two-Cache Architecture: `self.doc_bb` vs `session_bb`

The `GraphBuilder` maintains two separate `BeliefBase` instances during parsing:

- **`self.doc_bb`**: The NEW state (what documents currently contain after parsing)
- **`session_bb`**: The OLD state (what existed in the global cache before this parse)

**Parsing Lifecycle**:

1. **`initialize_stack`**: Clears `session_bb` to start fresh for this parse operation

2. **During parsing (`push`)**:
   - `cache_fetch` queries the global cache and populates `session_bb` via `merge()`
   - This includes both nodes and their relationships, building a snapshot of the old state
   - Remote events are processed into `self.doc_bb` only
   - `self.doc_bb` and `session_bb` intentionally diverge during this phase

3. **`terminate_stack`**: Reconciles the two caches:
   - Compares `self.doc_bb` (new parsed state) against `session_bb` (old cached state)
   - Identifies nodes that existed before but are no longer referenced
   - Generates `NodesRemoved` events for the differences
   - Sends reconciliation events to both `session_bb` and the transmitter (for global cache)

**Key Insight**: This two-cache architecture enables the builder to detect what was removed from a document by comparing old and new manifolds, then propagating those removals to other caches.

#### Link Rewriting and Bi-Directional References

Links are critical to the Buildonomy system. All links in source material are treated as bi-directional references. Links are one of the only places Buildonomy will edit a source document directly (the other being metadata blocks).

**Link Design Constraints** (simultaneously satisfied):

- **Preserve legibility**: Practitioners should be able to manually navigate to referenced documents without complicated tools. Link text should indicate what the link contains.

- **Auto-update descriptions**: When a reference title changes, link descriptions update automatically, unless explicitly specified separately from the link reference.

- **Track all references**: Track references-to (sinks) for everything important enough to document, even external sources.

- **External reference navigation**: Be able to fetch a node that navigates to an external reference simply by failing resolution of the reference's NodeKey (preserving schema, host, etc.).

- **Anchor uniqueness**: Treat URL anchors as unique nodes, not just the anchored document.

**Link Types**:
- **Epistemic links**: Appear within the text of a node
- **Pragmatic/Subsection references**: Appear in metadata

Implementation is handled via the interaction between `GraphBuilder::cache_fetch` and `crate::nodekey::href_to_nodekey`.

#### Relative Path Resolution Protocol

Links must be interpretable by both practitioners reading raw source documents and the software parsing them. Source documents constantly evolve, and links must remain interpretable as both source and reference material change.

**Relative Path Philosophy**:

Within source documents, relative links should be prioritized for readability:
- **Titles as anchors**: Preferred when unique
- **Path-based anchors**: When titles are non-unique, use `/source/network/relative/doc_path#node_bref` (abbreviated bid)

Within the instantiated network cache:
- Nodes are referenced by `Bid` (Belief ID)
- If a BID is not available in source, one is generated and injected back into the source
- `GraphBuilder::{push,push_relation}` generate appropriate `BeliefNode`s when necessary.

**Path Tracking** (`crate::beliefbase::BeliefBase::paths`):

The path system tracks:
- **Relative paths**: Anchored with respect to each network sink
- **External URLs**: Treated as absolute paths; if not resolvable, returned as `UnresolvedReference`
- **Resolved references**: BID is synchronized with source document and cache
- **Path relativity**: Paths are not intrinsic to nodes but are properties relative to network spatial structure

**Complexity: Path Stability**:

Relative paths change when documents are restructured or renamed:
- Section reordering breaks document index anchors
- Title changes break slug-based anchors
- Must rely on BIDs for stability, but BIDs are human-illegible
- After querying by BID, must translate back to relative link format

**Reference Resolution Protocol**:

1. **BID Generation**: If a parsed node (proto node) lacks a BID in source material, one is generated and written back to the source

2. **Unresolved References**: When parsing a link, if the path is not resolvable, an `UnresolvedReference` diagnostic is returned. The compiler uses this to:
   - Queue the referenced file for parsing (if available)
   - Track which files need reparsing once the reference is resolved

3. **Network Context**: When mapping a reference to an ID, the nearest network must be specified so only paths relative to that network are considered

4. **Path Change Propagation**: When a subsection reference path changes between versions, the builder must:
   - Find all sink relationships containing the old relative path
   - Propagate events back to source documents to rewrite them with updated relative links

**Unresolved References as Promises**:

We cannot assume all relations are immediately accessible during parsing. Unresolved references represent *promises* that something useful exists and will be resolved in subsequent passes. The `DocumentCompiler` maintains a two-queue architecture:
- **Primary queue**: Never-parsed files
- **Reparse queue**: Files with unresolved dependencies

This handles multi-pass resolution efficiently without polluting the cache with incomplete nodes.

### 3.4. BeliefBase vs BeliefGraph: Full API vs Transport Layer

The codebase maintains two distinct but related structures for representing compiled graphs:

**BeliefGraph: Lightweight Transport Structure**

```rust
pub struct BeliefGraph {
    pub states: BTreeMap<Bid, BeliefNode>,
    pub relations: BidGraph,
}
```

`BeliefGraph` is a minimal structure optimized for:
- **Query results**: Database queries return `BeliefGraph` (see query.rs:735-739, `ResultsPage<BeliefGraph>`)
- **Network transport**: Serialization between services
- **Set operations**: Union, intersection, difference operations (beliefbase.rs:313-455)
- **Pagination**: Breaking large graphs into pages (beliefbase.rs:546-601)

It contains only the essential graph data (states + relations) without the indexing overhead.

**BeliefBase: Full-Featured API**

```rust
pub struct BeliefBase {
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

`BeliefBase` is the full-featured structure providing:
- **Identity resolution**: Multiple lookup indices (BID, Bref, ID, Path)
- **Graph operations**: Context queries, traversals, filtering
- **Validation**: Invariant checking via `built_in_test()`
- **Incremental updates**: Event processing and diff computation
- **Thread-safe access**: Arc/RwLock for concurrent reads

**Conversion Pattern:**

```rust
impl From<BeliefGraph> for BeliefBase {
    fn from(beliefs: BeliefGraph) -> Self {
        BeliefBase::new_unbalanced(beliefs.states, beliefs.relations, true)
    }
}
```

Query results come back as `BeliefGraph`, which can be converted to `BeliefBase` when full API access is needed. This separation enables:
- Efficient pagination without building full indices for every page
- Lightweight serialization over network boundaries
- Fast set operations on query results before materializing as BeliefBase

**Usage Pattern:**

```rust
// Query returns lightweight BeliefGraph
let page: ResultsPage<BeliefGraph> = service.get_states(paginated_query).await?;

// Convert to BeliefBase for full API access
let belief_set: BeliefBase = page.results.into();

// Now can use full API
let context = belief_set.get_context(some_bid)?;
```

**Graph Operations:**

1. **Set Operations** (union, intersection, difference):
   - Combine multiple BeliefBases (e.g., merging branches)
   - Used for computing deltas between versions

2. **Filtering** (`filter_states`, `filter_paths`):
   - Extract subgraphs by node properties or path patterns
   - Enable scoped queries (e.g., "all documents under /docs")

3. **Graph Traversal** (`get_context`, `evaluate_expression`):
   - Compute sources/sinks for a node
   - Walk parent/child relationships by WeightKind

4. **Incremental Updates** (`process_event`):
   - Handle add/remove/update events from builder
   - Maintain invariants during mutations

**Lazy Indexing:**
The `bid_to_index` mapping is rebuilt only when `index_dirty` is set, enabling batched updates without per-operation overhead. This is analogous to incremental compilation in modern compilers.

### 3.5. DocCodec: The Frontend Interface

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

**Key Responsibility**: Codecs are **syntax-only**. They produce ProtoBeliefNodes with unresolved references (NodeKey instances). The builder handles semantic analysis and linking.

### 3.6. The Document Stack: Nested Structure Parsing

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
- **BeliefEvent Stream**: Builder → Applications
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
3. Builder resolves references and creates structural relationships
4. BeliefBase stores nodes with Subsection edges representing hierarchy

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

Introduce a **layered abstraction** where BeliefBase remains schema-agnostic, and applications can register custom schema handlers:

**Architecture:**

```
Application Schemas (domain-specific)
    ↓ Registered via SchemaRegistry
Schema-Aware Layer (application code)
  - Knows about domain-specific types
  - Implements schema-specific parsing
    ↓ Produces
BeliefBase Infrastructure (beliefbase.rs)
  - Generic graph operations
  - schema: Option<String> (opaque)
  - payload: toml::Table (opaque)
  - NO knowledge of application schemas
```

**Benefits:**

1. **BeliefBase remains schema-agnostic** - Can be used for any graph domain
2. **Extensible** - Applications can add schema types without modifying library
3. **No manual changes required** - Schema logic stays in application layer
4. **Query by type** - Can filter `schema` without BeliefBase knowing domain semantics

### 6.2. Reference Resolution Timing

**Status**: ✅ **Already Resolved**

The system already implements multi-pass reference resolution via the DocumentCompiler and UnresolvedReference System. See Section 3.0 for details on the multi-pass algorithm.

**Key mechanism**: `ParseDiagnostic::UnresolvedReference` diagnostics track unresolved references, and the `parse_content` return signature (`ParseContentResult` with `diagnostics: Vec<ParseDiagnostic>`) drives automatic convergence through iterative reparsing.

### 6.3. Error Recovery and Partial Compilation

**Current State**: The compiler continues processing files even when individual files fail to parse, logging errors and continuing with other files.

**Assessment**: Partial error recovery already exists at the file level. Within-file error recovery (continuing after syntax errors within a single document) is not currently implemented.

**Decision**: **Defer** - Current approach provides valuable architectural feedback during development. File-level recovery is sufficient for most use cases.

When needed, fine-grained error recovery within documents could be implemented by:
- Extending `ProtoBeliefNode` with an `errors: Vec<ParseError>` field
- Allowing partial node construction (e.g., node created but some relationships failed)
- Marking invalid nodes with `BeliefKind::Invalid` flag for UI feedback

### 6.4. Intermediate Representation Optimization

**Current State**: BeliefBase directly represents parsed structure without optimization passes.

**Assessment**: Current architecture is already quite efficient:
- Lazy indexing (`index_dirty` flag) - Rebuilds only when needed
- Arc-based structural sharing - Clone is cheap
- Multi-pass compilation - Natural convergence without explicit optimization

**Decision**: **Defer to Database Layer**

Optimization is better suited for the **DbConnection** persistent cache (db.rs) rather than the in-memory BeliefBase. The database serves as the "global cache" and can maintain optimized views:

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

1. **Separation of concerns** - BeliefBase stays simple, database handles optimization
2. **Persistent optimization** - Computed once, cached across sessions
3. **User-driven** - Suggestions reviewed by human, not auto-applied
4. **Analytics-friendly** - Database can track usage patterns for better suggestions

### 6.5. Concurrent Parsing

**Current State**: Files are parsed sequentially in the compiler thread work queue.

**Decision**: **Defer**

While concurrent parsing could improve throughput for large document sets (100+ files), it introduces complexity:

1. **Cache consistency challenges**: Multiple threads updating `GraphBuilder` simultaneously requires careful locking
2. **Multi-pass coordination**: The diagnostic-based unresolved reference resolution algorithm depends on parse ordering for convergence
3. **Limited bottleneck**: Parsing is already fast; transaction batching and DB writes are typically the bottleneck
4. **Complexity vs. gain**: Tokio async already provides concurrency for I/O; CPU-bound parsing parallelism adds minimal benefit

**Future approach** (when needed):
- Parse independent files concurrently in Phase 1 (no shared state)
- Synchronize before Phase 2 (reference resolution with shared builder)
- Use work-stealing queue for dynamic load balancing
- Benchmark to confirm bottleneck before implementing

### 6.6. Formal Grammar Specification

**Status**: For future consideration

**Current State**: Parsing logic is embedded in Rust code without formal grammar definition.

**Future Direction**: A schema registry system could provide declarative parsing rules that serve as a formal specification. Applications could define schemas declaratively, and the library could generate or validate parsing logic based on these specifications.

This would provide benefits similar to parser generators while maintaining flexibility for domain-specific parsing needs.

---

**Document Status**: Draft - This document captures the core architecture for the noet-core library, focusing on the graph compilation infrastructure that can be used by various applications.
