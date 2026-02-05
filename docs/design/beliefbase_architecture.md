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

noet-core implements **multi-ID triangulation** - the same node can be referenced through multiple identity types, each serving different purposes. This enables robust references that survive structural changes while supporting user-friendly semantic identifiers.

#### Identity Types

Every node in the system can be referenced through five **NodeKey** variants:

```rust
pub enum NodeKey {
    Bid { bid: Bid },                    // Globally unique UUID (primary key)
    Bref { bref: Bref },                 // 12-char hex compact reference
    Id { net: Bid, id: String },         // User-defined semantic ID
    Title { net: Bid, title: String },   // Auto-generated from heading text
    Path { net: Bid, path: String },     // Filesystem location
}
```

**Implementation**: `src/nodekey.rs`, `src/properties.rs:871-923` (BeliefNode::keys())

##### 1. BID (Belief ID) - System-Generated Stable Identity

**Purpose**: Primary key for nodes, globally unique, survives all content changes

**Properties**:
- UUIDv6 format: `01234567-89ab-cdef-0123-456789abcdef`
- Injected automatically during first parse
- Written to source file frontmatter (`bid = "..."`)
- Never changes once assigned
- Includes namespace hierarchy for distributed generation (namespace derived via UUIDv5)

**Generation**: `src/properties.rs:138-141` (Bid::new uses `Uuid::now_v6()`), `src/properties.rs:117-122` (namespace via `Uuid::new_v5()`)

**Example lifecycle**:
```toml
# Before first parse (user-authored)
title = "My Document"

# After first parse (BID injected)
bid = "01234567-89ab-cdef-0123-456789abcdef"
title = "My Document"

# After title change (BID stable)
bid = "01234567-89ab-cdef-0123-456789abcdef"  # Same!
title = "Updated Document Title"
```

**Why UUIDv6**: Time-ordered for efficient database indexing, includes namespace bytes for hierarchical distributed generation without central coordination. Namespace generation uses UUIDv5 for deterministic derivation from parent BIDs.

##### 2. Bref (Belief Reference) - Compact Display Form

**Purpose**: Human-readable compact reference for links and logging

**Properties**:
- 12 hexadecimal characters: `a1b2c3d4e5f6`
- Derived from BID's namespace bytes (last 48 bits)
- Used in markdown links: `[text](doc.md#a1b2c3d4e5f6)`
- Collision probability ~1 in 281 trillion within same namespace
- Maps to BID via `brefs: BTreeMap<Bref, Bid>` in BeliefBase

**Generation**: `src/properties.rs:167-176` (Bid::namespace)

**Usage in links**:
```markdown
# Document A
See [[a1b2c3d4e5f6]] for details.

# Link survives even if target file is renamed or moved!
```

##### 3. ID - User-Defined Semantic Identifier

**Purpose**: Optional user-controlled identifier with semantic meaning

**Properties**:
- Specified in frontmatter: `id = "introduction"`
- For markdown headings: `## Introduction {#introduction}`
- Normalized to HTML-safe anchors (lowercase, hyphens for spaces)
- Scoped to network (namespace) to prevent collisions
- Optional - not all nodes have explicit IDs

**Normalization**: `src/nodekey.rs:to_anchor()` function
- Lowercase: `Section` → `section`
- Spaces to hyphens: `Getting Started` → `getting-started`
- Strip special chars: `API & Reference!` → `api--reference`

**Example**:
```markdown
## Introduction {#intro}
Content here...

## Getting Started
Content here (auto-generates #getting-started)...

## Details
First occurrence...

## Details
Second occurrence - collision! Gets Bref: {#a1b2c3d4e5f6}
```

**Collision Handling**: Two-level detection (see § 2.2.1 below)
1. **Document-level**: During parse, track IDs within single file
2. **Network-level**: During enrichment, check PathMap for cross-file collisions

##### 4. Title - Auto-Generated Anchor

**Purpose**: Automatic anchor generation from heading text

**Properties**:
- Always present: `title = "Introduction"`
- Normalized via `to_anchor()` for HTML anchor compatibility
- Used when no explicit ID provided
- Can change (breaking references unless BID/Bref used)

**Generation**: Automatic from heading text or TOML `title` field

##### 5. Path - Filesystem Location

**Purpose**: File system operations and initial discovery

**Properties**:
- Relative to network root: `docs/design/architecture.md`
- Changes when files move
- Least stable identifier (use BID for permanent references)
- Used by `PathMapMap` for efficient lookups

**Storage**: `src/paths.rs:PathMapMap` maintains bidirectional mappings

#### Identity Resolution Hierarchy

When multiple references could match, resolution priority:

1. **BID** - Most explicit, globally unique, always preferred
2. **Bref** - Compact, collision-resistant, stable
3. **ID** - User-controlled semantic identifier, network-scoped
4. **Title** - Auto-generated, subject to collisions
5. **Path** - Least stable, fallback only

This hierarchy enables **progressive enhancement**: start with simple title references, add explicit IDs where needed, rely on BIDs for permanent stability.

#### 2.2.1. Collision Detection and Resolution

**Problem**: Multiple headings in a document or network may normalize to the same ID.

**Solution**: Two-level collision detection with Bref fallback.

##### Document-Level Collision Detection

**Implementation**: `src/codec/md.rs:1027-1054` (End(Heading) handler)

**Algorithm**:
```rust
fn determine_node_id(
    explicit_id: Option<&str>,      // User-provided {#id}
    title: &str,                     // Heading text
    bref: &str,                      // Node's Bref
    existing_ids: &HashSet<String>,  // Already seen IDs in document
) -> String {
    // Priority: explicit ID > title-derived ID
    let candidate = if let Some(id) = explicit_id {
        to_anchor(id)  // Normalize user ID
    } else {
        to_anchor(title)  // Derive from title
    };
    
    // Fallback to Bref if collision detected
    if existing_ids.contains(&candidate) {
        bref.to_string()
    } else {
        candidate
    }
}
```

**Example**:
```markdown
## Details
<!-- First occurrence: gets ID "details" -->

## Details
<!-- Collision detected: gets Bref {#a1b2c3d4e5f6} -->

## Getting Started {#getting-started}
<!-- Explicit ID: gets "getting-started" -->

## Getting Started
<!-- Collision with explicit ID: gets Bref {#b2c3d4e5f6a1} -->
```

##### Network-Level Collision Detection

**Implementation**: `src/codec/md.rs:700-723` (inject_context function)

**Purpose**: Detect when an ID is already used by a different node in the network

**Algorithm**:
```rust
// After document-level collision detection
if let Some(current_id) = proto.id {
    // Query PathMap for network-level collision
    if let Some((doc_bid, node_bid)) = paths.net_get_from_id(&net, &current_id) {
        if node_bid != ctx.node.bid {
            // Different node already owns this ID - remove it
            tracing::info!("Network collision: '{}' already used", current_id);
            proto.id = None;
        }
    }
}
```

**Why separate levels?**
- Document-level catches `##Details` / `##Details` in same file
- Network-level catches `docs/a.md#intro` and `docs/b.md#intro` collision

##### Selective ID Injection

**Policy**: Only inject anchors when necessary (normalized or collision-resolved)

**Implementation**: `src/codec/md.rs:725-751` (inject_context function)

**Rules**:
1. **Explicit ID matches normalized form**: No injection (keep source clean)
   - User writes `{#intro}` → already normalized → no rewrite
2. **Explicit ID normalized differently**: Inject normalized form
   - User writes `{#Intro!}` → normalized to `{#intro}` → inject `{#intro}`
3. **Collision detected**: Inject Bref
   - Second "Details" → collision → inject `{#a1b2c3d4e5f6}`
4. **Title-derived, no collision**: No injection (implicit anchor)
   - `## Introduction` → generates `#introduction` implicitly → no rewrite

**Write-back**: Uses pulldown_cmark_to_cmark which writes event's `id` field as `{ #id }` syntax

#### 2.2.2. Storage and Indexing

**PathMapMap** (`src/paths.rs:38-362`) maintains bidirectional mappings for O(1) lookups:

```rust
pub struct PathMapMap {
    map: BTreeMap<Bid, Arc<RwLock<PathMap>>>,  // Net → PathMap
    nets: BTreeSet<Bid>,                        // Network BIDs
    docs: BTreeSet<Bid>,                        // Document BIDs
    apis: BTreeSet<Bid>,                        // API node BIDs
    anchors: BTreeMap<Bid, String>,             // BID → normalized title
    ids: BTreeMap<Bid, String>,                 // BID → explicit ID
    // ...
}
```

**Query methods**:
- `net_get_from_id(&net, &id)` → `Option<(doc_bid, node_bid)>`
- `net_get_from_title(&net, &title)` → `Option<(doc_bid, node_bid)>`
- `net_path(&net, &bid)` → `Option<(net, path)>`

**BeliefNode::keys()** (`src/properties.rs:871-923`) generates all valid references:

```rust
fn keys(&self, net: Bid, parent: Option<Bid>, bs: &BeliefBase) -> Vec<NodeKey> {
    vec![
        NodeKey::Bid { bid: self.bid },
        NodeKey::Bref { bref: self.bid.namespace() },
        NodeKey::Id { net, id: self.id.clone() },        // If id.is_some()
        NodeKey::Title { net, title: to_anchor(&self.title) },
        NodeKey::Path { net, path: /* from PathMap */ },
    ]
}
```

#### 2.2.3. Benefits of Multi-ID Triangulation

**For Users**:
- Write natural markdown with simple links
- System maintains stability automatically (BID injection)
- Explicit control when needed (custom IDs)
- Files remain readable as plain text

**For Developers**:
- Query by any identity type
- Graceful degradation (BID → Bref → ID → Title → Path)
- Robust to structural changes (renames, moves)
- Efficient O(1) lookups via PathMapMap indices

**For Distributed Systems**:
- No central ID authority needed (UUIDv6 for BIDs, v5 for namespaces)
- Merge without collisions (BID uniqueness guarantees)
- Namespace hierarchy prevents ID conflicts (v5 ensures deterministic namespace derivation)
- Time-ordered BIDs for efficient database operations

**Example scenario - File refactoring**:
```markdown
# Before: docs/getting-started.md
[[a1b2c3d4e5f6]]  # Link using Bref

# After: tutorials/quickstart.md
# File moved and renamed, but link still works!
# BID unchanged, PathMap updated automatically
```

**Example scenario - Cross-device sync**:
```markdown
# Device A creates: docs/notes.md
bid = "aaaa1111-2222-3333-4444-555566667777"

# Device B creates: drafts/notes.md  
bid = "bbbb8888-9999-aaaa-bbbb-ccccddddeeee"

# No collision - different BIDs despite same filename!
# Merge creates two separate nodes
```

This comprehensive identity system enables robust knowledge management across evolving documents, distributed collaboration, and complex cross-references while maintaining source file readability.

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

### 2.7. The API Node and System Network Namespaces

Every `BeliefBase` contains a special **API node** and uses **three system-defined network namespaces** for tracking special categories of references. Understanding these reserved namespaces is critical for distributed synchronization, schema evolution, and preventing BID collisions.

#### Purpose and Architecture

**1. Version Management (Like Cargo)**

The API node tracks which version of noet-core's data model the BeliefBase uses:

```rust
pub fn api_state() -> BeliefNode {
    BeliefNode {
        bid: buildonomy_api_bid(env!("CARGO_PKG_VERSION")),  // Deterministic per version
        title: format!("Buildonomy API v{}", env!("CARGO_PKG_VERSION")),
        schema: Some("api".to_string()),
        kind: BeliefKindSet(BeliefKind::API | BeliefKind::Trace),
        id: Some("buildonomy_api".to_string()),
        payload: {
            "package": "noet-core",
            "version": "0.0.0",
            "authors": "...",
            // ... metadata fields
        },
    }
}
```

**Why versioning matters:**
- Future noet-core versions may change the graph schema (new WeightKinds, payload formats, etc.)
- Older library versions can detect newer schemas via API node version
- Enables graceful degradation or migration prompts
- Similar to how Cargo handles lockfile version compatibility

**2. Graph Entry Point**

The API node serves as the universal root for graph operations:

```rust
pub struct BeliefBase {
    states: BTreeMap<Bid, BeliefNode>,
    relations: Arc<RwLock<BidGraph>>,
    // ... other fields ...
    api: BeliefNode,  // Immutable reference, set at construction
}
```

**Structural role:**
- All Network nodes create a relation: `Network → API` (Section weight, source-owned)
- PathMapMap uses API node as root for path resolution
- Queries can start from API node to traverse entire graph
- Provides consistent entry point across distributed systems

**3. System Network Namespaces**

Beyond the API node, noet-core defines two additional **system-managed networks** that automatically track references across document collections:

```rust
// properties.rs
pub const UUID_NAMESPACE_BUILDONOMY: Uuid = /* 0x6b3d2154... */;  // API node
pub const UUID_NAMESPACE_HREF: Uuid      = /* 0x5b3d2154... */;  // External links
pub const UUID_NAMESPACE_ASSET: Uuid     = /* 0x4b3d2154... */;  // Images/attachments

pub fn buildonomy_namespace() -> Bid { Bid::from(UUID_NAMESPACE_BUILDONOMY) }
pub fn href_namespace() -> Bid { Bid::from(UUID_NAMESPACE_HREF) }
pub fn asset_namespace() -> Bid { Bid::from(UUID_NAMESPACE_ASSET) }
```

**Href Namespace**: A software-defined network (`BeliefKind::Network`) that collects all external HTTP/HTTPS links:
- When parser encounters `[text](https://example.com)`, creates node in href network
- Enables "find all documents referencing this external URL" queries
- Tracks citation sources and external dependencies
- Title field contains the URL string

**Asset Namespace**: A software-defined network for unparsable embedded resources:
- Images, PDFs, CSS files, fonts referenced in documents
- Enables "which documents use this image?" queries
- Tracks asset dependencies for migration/publishing
- Title field contains relative path to asset

**Why networks?** Networks are **graph entry points** - they enable efficient "find all references to X" queries by maintaining explicit relations rather than scanning all nodes. User-defined networks represent repositories/projects; system networks represent cross-cutting reference tracking.

#### Reserved BID Namespace

To prevent collisions between system nodes and user nodes, all system BIDs fall within a **reserved namespace**.

**Namespace Design:**

```rust
// The root namespace constant (like DNS, URL namespaces in UUID spec)
pub const UUID_NAMESPACE_BUILDONOMY: Uuid = Uuid::from_bytes([
    0x6b, 0x3d, 0x21, 0x54, 0xc0, 0xa9, 0x43, 0x7b, 
    0x93, 0x24, 0x5f, 0x62, 0xad, 0xeb, 0x9a, 0x44,
]);

// Generate versioned API BID (deterministic)
pub fn buildonomy_api_bid(version: &str) -> Bid {
    // 1. Generate UUID v5 from version string (deterministic)
    let mut uuid = Uuid::new_v5(&UUID_NAMESPACE_BUILDONOMY, version.as_bytes());
    
    // 2. Replace octets 10-15 with namespace bytes from UUID_NAMESPACE_BUILDONOMY
    let mut bytes = *uuid.as_bytes();
    bytes[10..16].copy_from_slice(
        &Bid::from(UUID_NAMESPACE_BUILDONOMY).parent_namespace_bytes()
    );
    
    Bid(Uuid::from_bytes(bytes))
}
```

**How namespace checking works:**

Following the same pattern as `Bid::new()` (which uses UUID v7 with namespace in octets 10-15):

```rust
impl Bid {
    pub fn is_reserved(&self) -> bool {
        self.parent_namespace_bytes() 
            == Bid::from(UUID_NAMESPACE_BUILDONOMY).parent_namespace_bytes()
    }
}
```

**Key properties:**
- **Deterministic**: Same version always produces same API BID
- **Checkable**: Namespace bytes in standard location (octets 10-15)
- **Collision-free**: User BIDs cannot accidentally use reserved namespace

**Example:**
```rust
let api_bid = buildonomy_api_bid("0.0.0");
// Result: "5a29441c-37d2-5f41-b61b-5f62adeb9a44"
//         ↑ First 10 bytes from UUID v5 hash
//                                  ↑ Last 6 bytes = reserved namespace

assert!(api_bid.is_reserved());  // true

let user_bid = Bid::new(&some_parent);
assert!(!user_bid.is_reserved());  // false (different namespace)
```

#### Reserved Identifiers Validation

User files **cannot** use reserved identifiers. Parsing fails with clear errors:

**Reserved BIDs:**
- `UUID_NAMESPACE_BUILDONOMY` itself
- `UUID_NAMESPACE_HREF` (for external link tracking)
- Any BID with `parent_namespace_bytes()` matching the Buildonomy namespace

**Reserved IDs:**
- `"buildonomy_api"` - API node identifier
- `"buildonomy_href_network"` - Href tracking network
- Any ID starting with `"buildonomy_"` prefix

**Validation in `ProtoBeliefNode::from_str_with_format()`:**

```rust
// Check reserved BID
if let Some(bid_str) = proto.document.get("bid").and_then(|v| v.as_str()) {
    if let Ok(bid) = Bid::try_from(bid_str) {
        if bid.is_reserved() {
            return Err(BuildonomyError::Codec(
                "BID '{}' is reserved for system use. \
                 Please remove 'bid' field or use different UUID."
            ));
        }
    }
}

// Check reserved ID
if let Some(id_str) = proto.document.get("id").and_then(|v| v.as_str()) {
    if id_str.starts_with("buildonomy_") {
        return Err(BuildonomyError::Codec(
            "ID '{}' uses reserved 'buildonomy_' prefix."
        ));
    }
}
```

**Error example:**

```toml
# user_file.toml - This will FAIL to parse
bid = "6b3d2154-c0a9-437b-9324-5f62adeb9a44"  # This is UUID_NAMESPACE_BUILDONOMY!
title = "My Document"
```

Error: `BID '6b3d2154-c0a9-437b-9324-5f62adeb9a44' is reserved for system use...`

#### API Node Lifecycle

**Creation:**
- API node created in `BeliefBase::empty()` via `BeliefNode::api_state()`
- BID is deterministic per noet-core version
- Stored in immutable `api` field on `BeliefBase`

**Insertion into graph:**
- Added to `doc_bb` during `GraphBuilder::initialize_stack()`
- Also added to `session_bb` and `global_bb` if not present
- Ensures all caches share the same API node

**Relations:**
- Network nodes create `Network → API` edges during `push()` (builder.rs:1003)
- Edge type: `WeightKind::Section`, source-owned (`"owned_by": "source"`)
- PathMapMap registers API node for path resolution

**Immutability:**
- API node BID never changes for a given noet-core version
- `BeliefBase.api` field is read-only (no setter methods)
- If API node gets merged/replaced during parsing (bug), it causes issues
  - This was the root cause of Issue 24 (test file used reserved BID)
  - Now prevented by validation in `ProtoBeliefNode` parsing

#### Implementation Details

**Location:** `src/properties.rs:808-839` (`api_state()` function)

**Reserved namespace checking:** `src/properties.rs:192-208` (`Bid::is_reserved()` method)

**Validation:** `src/codec/belief_ir.rs:1081-1125` (reserved identifier checks)

**Builder integration:** `src/codec/builder.rs:556-559` (API node initialization)

**Tests:** `src/properties.rs:1330-1380` (reserved namespace checking)

#### Future Extensions

**Multi-version graphs:**
When older noet-core versions encounter newer schemas:
1. Check API node version against library version
2. If newer, optionally reject or warn user
3. Enables controlled migration paths

**Schema migrations:**
API node version can trigger migration logic:
```rust
match api_node.payload.get("version") {
    "0.1.0" => migrate_v0_1_to_v0_2(belief_base),
    "0.2.0" => /* current version */,
    unknown => warn!("Unknown schema version: {}", unknown),
}
```

**Distributed sync:**
Multiple devices can detect API version mismatches:
- Device A: noet-core v0.1.0 → creates API v0.1.0 node
- Device B: noet-core v0.2.0 → detects older schema, prompts upgrade

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
   - If resolved: create relation via `RelationChange` event
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
  → create_resolved_relation() → emits RelationChange event
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
    fn generate_source(&self) -> Option<String>;
    
    // HTML Generation API (dual-phase)
    fn should_defer(&self) -> bool { false }
    fn generate_html(&self) -> Result<Vec<(PathBuf, String)>, BuildonomyError> { Ok(vec![]) }
    fn generate_deferred_html(&self, ctx: &BeliefContext) -> Result<Vec<(PathBuf, String)>, BuildonomyError> { Ok(vec![]) }
}
```

#### Factory Pattern Architecture

Codecs are created via **factory functions** (`type CodecFactory = fn() -> Box<dyn DocCodec>`), not singletons:

```rust
pub struct CodecMap(Arc<RwLock<Vec<(String, CodecFactory)>>>);

impl CodecMap {
    pub fn create() -> Self {
        let map = CodecMap(Arc::new(RwLock::new(vec![
            ("md".to_string(), || Box::new(md::MdCodec::new())),
            ("toml".to_string(), || Box::new(ProtoBeliefNode::default())),
            // ... other codecs
        ])));
        map
    }
    
    pub fn get(&self, ext: &str) -> Option<CodecFactory> {
        // Returns factory function, not codec instance
    }
}
```

**Benefits**:
- **Thread-safe**: Each parse operation gets fresh codec instance
- **No state leakage**: Parsing one file doesn't affect another
- **Concurrent parsing**: Multiple threads can parse simultaneously
- **Testability**: Each test gets isolated codec state

#### Dual-Phase HTML Generation

HTML generation happens in two phases to handle different codec needs:

**Phase 1: Immediate Generation** (`generate_html`)
- Called immediately after parsing, before context injection
- Codec has parsed AST but no graph context
- Use for: Static content (Markdown → HTML, syntax highlighting)
- Returns: `Vec<(PathBuf, String)>` of (repo-relative-path, html-body)

**Phase 2: Deferred Generation** (`generate_deferred_html`)
- Called after all documents parsed and context injected
- Codec has full `BeliefContext` with graph relationships
- Use for: Dynamic content (network indices, backlinks, cross-references)
- Returns: Same format as immediate generation

**Deferral Signal**: `should_defer()` tells compiler which phase to use:
- `false` (default): Only immediate generation
- `true`: Skip immediate, use deferred with context

**Example: Network Index Generation**
```rust
impl DocCodec for ProtoBeliefNode {
    fn should_defer(&self) -> bool {
        self.kind.contains(BeliefKind::Network)
    }
    
    fn generate_deferred_html(&self, ctx: &BeliefContext) -> Result<Vec<(PathBuf, String)>, BuildonomyError> {
        // Query child documents via Section (subsection) edges
        let mut children: Vec<_> = ctx.sources()
            .iter()
            .filter_map(|edge| {
                edge.weight.get(&WeightKind::Section).map(|section_weight| {
                    let sort_key: u16 = section_weight.get(WEIGHT_SORT_KEY).unwrap_or(0);
                    (edge, sort_key)
                })
            })
            .collect();
        
        // Sort by sort_key, generate HTML list
        children.sort_by_key(|(_, sort_key)| *sort_key);
        let html = format!("<ul>{}</ul>", 
            children.iter()
                .map(|(edge, _)| format!("<li><a href='/{}'>{}</a></li>", 
                    edge.home_path.replace(".md", ".html"), 
                    edge.other.display_title()))
                .collect::<String>());
        
        Ok(vec![(self.path.with_extension("html"), html)])
    }
}
```

**Current Implementations:**

- **MdCodec** (md.rs): Immediate generation only
  - Parses Markdown with TOML frontmatter
  - Generates HTML from pulldown-cmark AST
  - Rewrites internal links to `.html` extension
  - Extracts headings for structural hierarchy

- **ProtoBeliefNode** (belief_ir.rs): Deferred generation for networks
  - Parses TOML/JSON/YAML files
  - Schema-aware: detects schema from path or frontmatter
  - Networks defer to query child documents from context
  - Generates index pages listing subsections

**Key Responsibility**: Codecs are **syntax-only** for parsing. They produce ProtoBeliefNodes with unresolved references (NodeKey instances). The builder handles semantic analysis and linking. For HTML generation, codecs are **presentation-only** — they return body content, compiler wraps with templates.

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

### 3.7. Event Synchronization and BeliefBase Export

When the compiler finishes parsing, it must ensure all `BeliefEvent`s have been processed before exporting the beliefbase. This is critical for the `parse` command which uses an in-memory `BeliefBase` with asynchronous event processing.

#### The Problem

```
Compiler (tx) → [events in channel] → rx → BeliefBase (processes events)
                                             ↓
                                        export_beliefgraph() ← Called too early!
```

If `export_beliefgraph()` is called before all events are processed, the export will be incomplete.

#### Solution: Event Loop Synchronization (Option G Pattern)

The `parse` command manages the event loop explicitly:

```rust
// In main.rs parse command
runtime.block_on(async {
    // 1. Create event channel
    let (tx, mut rx) = unbounded_channel::<BeliefEvent>();
    
    // 2. Spawn background task to process events
    let mut global_bb = BeliefBase::empty();
    let processor = tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            let _ = global_bb.process_event(&event);
        }
        global_bb  // Return synchronized BeliefBase when channel closes
    });
    
    // 3. Create compiler with event transmitter
    let mut compiler = DocumentCompiler::with_html_output(
        &path, Some(tx), None, write, Some(html_dir), None, cdn
    )?;
    
    // 4. Parse all documents (sends events to processor)
    compiler.parse_all(cache, force).await?;
    
    // 5. Drop compiler to close tx channel
    drop(compiler);
    
    // 6. Wait for event processor to finish (drains all events)
    let final_bb = processor.await?;
    
    // 7. Now safe to export from synchronized BeliefBase
    let graph = final_bb.clone().consume();
    export_beliefbase_json(graph, html_dir).await?;
});
```

**Key Points**:
- Background task processes events asynchronously
- Dropping compiler closes `tx`, signaling processor to finish
- `processor.await` blocks until all events processed
- Export happens from synchronized `final_bb`

**Watch Service vs Parse Command**:
- **Watch service**: Uses `DbConnection` which processes events in its own loop → `finalize()` exports from database
- **Parse command**: Uses in-memory `BeliefBase` with explicit event loop → exports after synchronization

This pattern ensures `beliefbase.json` always contains complete graph data for the interactive viewer.

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
