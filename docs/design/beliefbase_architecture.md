---
title = "BeliefBase Architecture: The Compiler IR for Document Graphs"
authors = "Andrew Lyjak, Claude Code"
last_updated = "2025-04-28"
status = "Draft"
version = "0.3"
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
IRNode (Intermediate Representation)
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
- **DocCodec**: Lexer/parser - syntax analysis, producing unlinked IRNodes
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

##### 5. Path - Filesystem Location

**Purpose**: File system operations and initial discovery

**Properties**:
- Relative to network root: `docs/design/architecture.md`
- Changes when files move
- Least stable identifier (use BID for permanent references)
- Used by `PathMapMap` for efficient lookups

**Storage**: `src/paths.rs:PathMapMap` maintains bidirectional mappings

**Network Node Dual-Path Representation**:

Network nodes are special: they exist simultaneously at two filesystem levels,
and this duality propagates through the system with different invariants at each
layer.

| Form | Example (absolute) | Example (repo-relative) | Where used |
|---|---|---|---|
| **Directory path** | `/repo/subnet1` | `"subnet1"` | `proto.path`, `GraphBuilder` stack, `ProtoIndex` keys |
| **Index file path** | `/repo/subnet1/index.md` | `"subnet1/index.md"` | Filesystem codec dispatch, `detect_network_file` output |

`detect_network_file` (`src/codec/network.rs`) is the authoritative converter
between the two forms — given either a directory path or a direct `index.md`
path it returns the `index.md` file path. It requires a filesystem existence
check so it cannot be used for in-memory path key construction.

**PathMap dual-entry**: `PathMap::new` registers the network root node under
*two* path strings:
- `""` (empty string, directory form) — the parent anchor for document children
- `"index.md"` (file form) — the parent anchor for heading/section children
  inside the network's own index file (stored at order key
  `[NETWORK_SECTION_SORT_KEY]` to keep sections non-colliding with documents)

A subnet as seen from its parent's `PathMap` appears as a single directory-form
entry (e.g. `"subnet1"`), delegating to the subnet's own `PathMap` for
sub-lookups via `indexed_get`'s subnet traversal.

**`GraphBuilder` stack always holds the directory form**: `push()` stores
`proto.path.clone()` for network nodes (heading level 1). `proto.path` is set
by `NetworkCodec::proto` / `ProtoIndex::proto_for` as
`os_path_to_string(network_dir)` — the absolute directory path with no trailing
slash and no `index.md` suffix.

**`NodeKey::Path` for documents inside a subnet**: `build_path_key` uses the
stack's `net_path` (directory form) as the strip prefix, yielding a
subnet-relative path such as `"doc.md"`. The `net` field is the subnet BID's
bref. The subnet's `PathMap` stores documents under the same subnet-relative
form. These must match; any normalization divergence produces a cache miss and a
fresh time-based BID for the document.

**`AnchorPath` hazard with directory-form paths**: `AnchorPath::new` classifies
extension-less paths as directories, and its `filepath()` method returns
`dir()` for such paths. `dir()` returns everything up to the *last* `/`
separator — silently stripping the final path component. Callers that construct
an `AnchorPath` from a directory-form `net_path` and then call `filepath()` or
`strip_prefix` on it must use `AnchorPath::new_dir(net_path)` (or append a
trailing slash) so the full directory path is preserved. Passing a bare
directory path without this correction causes `strip_prefix` to strip the
grandparent directory instead of the network directory, making the resulting
child path repo-root-relative while the `net` key is the subnet's bref — an
internally inconsistent `NodeKey::Path` that never resolves in the `PathMap`.

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

**Solution**: Two-level collision detection using the `NodeId` enum to distinguish
anchor identity (document-scoped) from network-scoped identity.

##### The `NodeId` Enum

**Implementation**: `src/properties.rs`

`BeliefNode.id` is a `NodeId` enum (not `Option<String>`) that captures the
identity state of a node's anchor:

```rust
pub enum NodeId {
    Slug,              // Title-derived slug; no explicit ID
    Explicit(String),  // User-authored {#id}, intra-doc collision suffix, or bref fallback
    Collision,         // Inter-doc network-level collision (ephemeral, not persisted)
}
```

**Key semantic split**:
- `anchor()` → the value for the HTML `id` attribute and PathMap path fragment.
  Returns the explicit string for `Explicit`, empty for `Slug`/`Collision` (callers
  fall through to `to_anchor(title)`).
- `id()` → backward-compatible effective ID. Falls through: `Explicit` string →
  `to_anchor(title)` → bref string.

**Serialization**: `Slug` and `Collision` serialize as absent (`None`);
`Explicit(s)` serializes as `Some(s)`. Backward-compatible with existing msgpack
shards.

##### Document-Level Collision Detection

**Implementation**: `src/codec/md.rs` (End(Heading) handler)

**Algorithm**:
```rust
fn determine_node_id(
    explicit_id: Option<&str>,      // User-provided {#id}
    title: &str,                     // Heading text
    bref: &str,                      // Node's Bref
    existing_ids: &HashSet<String>,  // Already seen IDs in document
) -> String {
    let candidate = if let Some(id) = explicit_id {
        to_anchor(id)
    } else {
        to_anchor(title)
    };
    if existing_ids.contains(&candidate) {
        bref.to_string()  // Fallback to Bref
    } else {
        candidate
    }
}
```

Intra-document collisions produce `NodeId::Explicit` with a bref or slug-N suffix.
The anchor and network ID are the same value — no ambiguity.

**Example**:
```markdown
## Details
<!-- First occurrence: NodeId::Slug, anchor="details" -->

## Details
<!-- Collision: NodeId::Explicit("a1b2c3d4e5f6"), anchor="a1b2c3d4e5f6" -->
```

##### Network-Level Collision Detection

**Implementation**: `src/codec/builder.rs` (push, FIRST-ONE-WINS) and
`src/beliefbase/base.rs` (insert_state)

**Purpose**: Detect when an ID is already used by a different node in the network.
Two documents with `## Data Sharing` produce the same slug `data-sharing`; the
network-scoped `NodeKey::Id` collides even though the PathMap paths are distinct
(`a.md#data-sharing` vs `b.md#data-sharing`).

**Algorithm**: FIRST-ONE-WINS. The first node to claim an ID keeps it; the
loser is marked `NodeId::Collision`:

```rust
// In push() when network-level ID collision is detected:
node.id = NodeId::Collision;
// NodeKey::Id key is still updated to use bref for collision avoidance:
keys[id_key_idx] = NodeKey::Id { net, id: node.bid.bref().to_string() };
```

**Why `Collision` instead of setting `id = bref`**: The anchor and the network-scoped
ID serve different purposes. The anchor (`doc.md#data-sharing`) is document-unique
and should remain the title slug for PathMap paths and HTML rendering. Only the
network-scoped `NodeKey::Id` needs disambiguation (via bref). Setting `node.id` to
the bref conflated these roles, corrupting PathMap paths and causing `cache_fetch`
misses on re-parse when `--write` is off (the source heading text doesn't change,
so `speculative_path_key` generates the slug-based path, but the PathMap stored
the bref-based path). See Issue 75 for the full investigation.

**`Collision` is ephemeral**: re-derived each parse from the dynamic collision check.
Not persisted to shards (serializes as absent, same as `Slug`). If the conflicting
document is removed, the next parse no longer detects the collision and the node
returns to `Slug` state automatically.

##### Selective ID Injection

**Policy**: Only inject anchors when necessary (normalized or collision-resolved)

**Implementation**: `src/codec/md.rs` (inject_context function)

**Rules**:
1. **Explicit ID matches normalized form**: No injection (keep source clean)
   - User writes `{#intro}` → already normalized → no rewrite
2. **Explicit ID normalized differently**: Inject normalized form
   - User writes `{#Intro!}` → normalized to `{#intro}` → inject `{#intro}`
3. **Intra-doc collision detected**: Inject Bref
   - Second "Details" → collision → inject `{#a1b2c3d4e5f6}`
4. **Inter-doc collision (`NodeId::Collision`)**: No injection
   - Treated same as `Slug` — title slug used for HTML anchor
5. **Title-derived, no collision**: No injection (implicit anchor)
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
        NodeKey::Bref { bref: self.bid.bref() },
        NodeKey::Id { net, id: self.id.clone() },        // If id.is_some()
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

### 2.4. The API Node and System Network Namespaces

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

**Validation in `IRNode::from_str_with_format()`:**

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
  - Now prevented by validation in `IRNode` parsing

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

### 2.5. Graph Structure and Invariants

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

3. **Network `payload["codec"]` contract**: Every `BeliefKind::Network` node carries
   `payload["codec"]` — a string identifying the filename that defines the network
   (e.g. `"index.md"`, `"CMakeLists.txt"`). This is set by `DocCodec::proto()` (e.g.
   `NetworkCodec` sets it to `NETWORK_NAME`); `ProtoIndex::build` fills it in from the
   detected network filepath when the codec omits it. `PathMap` reads this value at
   construction time (stored as `network_filename`) to correctly prefix anchor paths
   and resolve bare-anchor references (e.g. `"#slug"` → `"CMakeLists.txt#slug"`) for
   networks whose index file is not `index.md`. Custom codecs that declare
   `WalkCodec::network_filenames()` must ensure their `DocCodec::proto()` sets
   `document["codec"]` to the network filename so downstream path resolution is correct.

### 2.6. Multi-Component Architecture: Compiler → Builder → Set

**DocumentCompiler** (`codec/compiler.rs`):
- Orchestrates multi-pass compilation across multiple files
- Owns the `ProtoIndex` (filesystem index built once at startup), the work queue
  (`remainder_queue`), and the main `GraphBuilder`
- Exposes `parse_sequential` (single-threaded) and `parse_all` (parallel, epoch-based)
- Drives compilation to convergence via depth-grouped network epochs, a leaf-document
  epoch, and a remainder loop for assets and re-parses

**GraphBuilder** (`codec/builder.rs`):
- Stateful parser-linker for a single session; one instance per `DocumentCompiler`
- In parallel mode, each epoch task gets a **fresh task-local `GraphBuilder`** seeded
  from `epoch_session_snapshot`; events flow via isolated per-task channels
- Maintains `doc_bb` (current-parse scratch space) and `session_bb` (accumulated
  session state / proxy for `global_bb`)
- Implements a **document stack** for tracking the ancestor-network/heading hierarchy
- Resolves references via `cache_fetch` (`doc_bb` → `session_bb` → `global_bb`)
- Publishes `BeliefEvent` updates via an async channel to `BeliefAccumulator`

**BeliefAccumulator** (`beliefbase/accumulator.rs`):
- Receives `BeliefEvent`s from the shared channel between epochs
- Applies events to its backing `BeliefBase` in `BatchStart`/`BatchEnd`-delimited
  batches; `drain_epoch()` commits one epoch and returns query access via `QueryHandle`
- `QueryHandle` is the `global_bb` clone each task builder queries during parsing

**BeliefBase** (`beliefbase/base.rs`):
- The compiled, indexed graph — nodes + typed edges + multiple lookup indices
- Thread-safe via `Arc<RwLock<BidGraph>>`; identity lookups via `PathMapMap`, `brefs`
- Mutated by `process_event`; queried by `evaluate` / `get_context`

The architecture maps to traditional compilers as:
- **DocCodec** → Lexer/Parser (syntax analysis, produces IRNodes)
- **GraphBuilder** → Semantic analyser + Linker (reference resolution, BID assignment)
- **DocumentCompiler** → Build-system driver (orchestration, multi-pass)
- **BeliefAccumulator** → Assembler/Linker output buffer (event serialisation, epoch commit)
- **BeliefBase** → Compiled IR (queryable, indexed result)

## 3. Architecture

### 3.0. System Overview and Data Flow

The complete compilation system consists of multiple cooperating layers:

```
Source Files (*.md, *.yaml, *.xlsx, …)
        │ read
        ▼
┌───────────────────────────────────────────────────────────┐
│  DocumentCompiler                                         │
│  - owns ProtoIndex, remainder_queue, main GraphBuilder    │
│  - drives epoch batches: Phase1 nets → Phase2 leaves      │
│  - remainder loop for assets and re-parses                │
└────┬──────────────────────────────────────────────────────┘
     │ per-file / per-epoch
     ▼
┌───────────────────────────────────────────────────────────┐
│  GraphBuilder  (one per task in parallel mode)            │
│  - doc_bb: current-parse scratch (rebuilt per file)       │
│  - session_bb: accumulated session state                  │
│  - stack: ancestor network + heading hierarchy            │
│  - DocCodec: parse → IRNode → push/push_relation          │
│  - cache_fetch: doc_bb → session_bb → global_bb           │
│  - terminate_stack: compute_diff → BeliefEvents → tx      │
└────┬──────────────────────────────────────────────────────┘
     │ BeliefEvent stream (BatchStart/BatchEnd bracketed)
     ▼
┌───────────────────────────────────────────────────────────┐
│  BeliefAccumulator                                        │
│  - collects events inside batch boundaries                │
│  - drain_epoch() commits one epoch's events               │
│  - QueryHandle cloned by each parallel task as global_bb  │
└────┬──────────────────────────────────────────────────────┘
     │ committed state
     ▼
┌───────────────────────────────────────────────────────────┐
│  BeliefBase  (global_bb)                                  │
│  - indexed graph: states, relations, PathMapMap, brefs    │
│  - queried by task builders via QueryHandle               │
│  - final output for application queries                   │
└───────────────────────────────────────────────────────────┘
```

**Walk-time visibility and parse-time dispatch** are handled by two complementary registries
(implemented in Issue 68):

- **`WALK_CODECS`**: a global registry of [`WalkCodec`] implementations that determine walk-time
  file visibility. Pre-populated with `MdWalkCodec` (`.md`) and `YamlWalkCodec` (`.yaml`/`.yml`).
  Application shims register additional codecs before `DocumentCompiler::new`. The
  `net_dir_partition` filter includes any file matched by `CODECS ∪ WALK_CODECS`.
  Walk codecs that define network boundaries implement `network_filenames()` — returning the
  filenames (e.g. `"CMakeLists.txt"`) that mark a directory as a network root. These filenames
  feed into `WalkCodecMap::is_network_file()` (subnet detection in `net_dir_partition`),
  `detect_network_file()` (index-file resolution), and `CodecManifest` (WASM viewer setup).

- **`CLAIM_MAP`**: a global path→codec registry populated during Phase 1 by network codecs
  calling `CLAIM_MAP.claim(path, factory)` inside `DocCodec::parse`. `CLAIM_MAP.get(path)` is
  the single dispatch entry point — it checks the claim registry first, then falls back to
  `CODECS.path_get()` internally. This gives per-file dispatch precision that extension matching
  alone cannot provide. Files explicitly excluded by a network's whitelist/blacklist are stored as
  `None` sentinels (`CLAIM_MAP.reject(path)`) so they route to `UnclaimedDataCodec` rather than
  being re-dispatched via `CODECS`.

**Data Flow (parse_all parallel path):**
1. `DocumentCompiler` builds `ProtoIndex` (one O(files) WalkDir pass at startup)
2. Phase 1: network dirs processed depth-group by depth-group; each group is one
   `parse_epoch` batch, drained before the next depth level begins
3. Phase 2: all leaf documents in one `parse_epoch` batch
4. Each `parse_epoch` task gets a fresh `GraphBuilder` seeded from
   `epoch_session_snapshot` and a clone of the `QueryHandle` (`global_bb`)
5. Tasks emit `BeliefEvent`s to per-task isolated channels; after all tasks finish,
   events are forwarded to the shared channel in deterministic path-index order
6. `drain_epoch()` commits the batch to `global_bb`; the next epoch sees the result
7. Remainder loop handles assets and re-parses, split into asset sub-epoch then
   reparse sub-epoch so asset BIDs are visible when referencing documents re-run

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

1. **Initial Pass**: `DocumentCompiler` iterates `ProtoIndex::ordered_paths()` — a
   depth-first, lexicographic traversal of the repo tree that guarantees every
   network's `index.md` is committed to `global_bb` before its sibling files are
   dispatched. For each path:
   - Parse via `GraphBuilder::parse_content`
   - Unresolvable cross-file refs produce `UnresolvedReference` diagnostics
   - Resolved deps that need a source rewrite (BID injection, title update) are
     pushed to `remainder_queue`

2. **Remainder Loop**: While `remainder_queue` is non-empty:
   - Pop the next path (ordered by `processed` count ascending — assets at count=0
     sort before doc rereparses at count=1+)
   - Reparse; new unresolved refs or rewrite triggers extend `remainder_queue`
   - Stop when a full drain produces no new entries, or `max_reparse_count` is reached

3. **Convergence**: Internal refs resolve after full tree parse; external refs or
   typos remain as `UnresolvedReference` diagnostics in the final result.

`DocumentCompiler` exposes two public drivers built on this model — see §3.3.

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

`GraphBuilder` is the parser-linker. `DocumentCompiler` drives it by calling
`parse_content` for each file. In `parse_all`'s parallel path, each epoch task
constructs a **fresh task-local `GraphBuilder`** with `epoch_session_snapshot` as
its initial `session_bb` state and a `QueryHandle` clone as its `global_bb`.

**Key State:**

```rust
pub struct GraphBuilder {
    // Per-parse scratch: rebuilt from scratch by initialize_stack for every file.
    doc_bb: BeliefBase,

    // Accumulated session state. In the main compiler builder this grows across
    // all files. In parallel task builders it starts from epoch_session_snapshot
    // (networks + const-namespace assets + index.md anchor nodes).
    // Acts as the task's local proxy for global_bb: cache_fetch checks here before
    // querying global_bb, avoiding a mutex round-trip on re-parses.
    session_bb: BeliefBase,

    // Root network BID (Bid::nil() until the repo-root index.md is parsed).
    repo: Bid,
    repo_root: PathBuf,

    // Ancestor-network + in-document heading stack.
    // Each entry: (bid, absolute_path, heading_level)
    // heading_level: 1=Network, 2=Document, 3=H1-anchor, 4=H2-anchor, …
    stack: Vec<(Bid, String, usize)>,

    // Event publication channel. In parallel tasks this is an isolated per-task
    // channel; events are forwarded to the shared channel in path-index order
    // after all tasks complete.
    tx: UnboundedSender<BeliefEvent>,
}
```

**Five-Phase `parse_content` Algorithm:**

1. **Phase 0 — `initialize_stack`**:
   - Clears `doc_bb` and rebuilds it from the ancestor-network subgraph
   - Fast path (`try_initialize_stack_from_session_cache`): looks up the parent
     network via StackCache in `session_bb`, avoiding a `global_bb` mutex call
   - Slow path: queries `global_bb` for the ancestor chain and merges into
     `session_bb` for subsequent sibling file fast-paths

2. **Phase 1 — `push` loop (node creation)**:
   - `DocCodec::parse` produces `IRNode`s for the document and each heading section
   - For each `IRNode`, `push()` calls `cache_fetch` to resolve or create the node:
     - **StackCache** hit (`session_bb`): node exists, no missing_structure populated
     - **GlobalCache** hit (`global_bb`): node fetched, neighborhood merged into
       `session_bb` via `missing_structure`
     - **Generated**: brand-new node — fresh time-based BID assigned, inserted into
       `doc_bb` via `doc_bb.process_event(NodeUpdate)`; `session_bb` does NOT get it
       (it will arrive via `compute_diff` in Phase 5)
   - Stack is maintained: network/doc/heading entries pushed for nested resolution

3. **Phase 2 — `push_relation` loop (edge creation)**:
   - For each relation in `proto.upstream` / `proto.downstream`, calls `push_relation`
   - `push_relation` resolves the other endpoint via `cache_fetch`, populates
     `missing_structure`, emits a `RelationChange` into `relation_event_queue`
   - After all push_relation calls: `session_bb.merge_from(&missing_structure,
     &relation_seeds)` and `doc_bb.merge_from(&missing_structure, &relation_seeds)`
     seeded by `relation_seeds` (the resolved endpoint BIDs), bounding the DFS to
     nodes reachable from those endpoints
   - `doc_bb.apply_events_batch(&relation_event_queue)` applies all relation events
     in a single three-pass flush (nodes → edges → PathMapMap)

4. **Phase 4 — `inject_context` (BID + metadata back-injection)**:
   - For each parsed node, calls `codec.inject_context(proto, &ctx)` where `ctx`
     comes from `doc_bb.get_context()`
   - `inject_context` calls `update_from_context` which compares the node's current
     field values (bid, title, id, kind) directly against `ctx.node` and updates the
     proto's TOML document where they differ — no TOML string round-trip
   - If `frontmatter_changed`, `sections_metadata_merged`, `id_changed`, or
     `link_changed` is true, `events_to_text` regenerates the markdown text for that
     node; `generate_source()` is called if `is_changed || has_new_bids`
   - `rewritten_content` is set when `generate_source()` produces content that
     differs from the on-disk file (or when new BIDs were assigned)

5. **Phase 5 — `terminate_stack`**:
   - Emits deferred collision removals (`NodesRemoved` for stale BIDs)
   - Calls `compute_diff(old=session_bb, new=doc_bb, parsed_nodes)` to produce
     `NodeUpdate`, `RelationUpdate`, and `NodesRemoved` events for the delta
   - Applies diff events to `session_bb` (so subsequent files in this task see them)
   - Sends all events via `tx` to the `BeliefAccumulator`

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

- **`doc_bb`**: Per-file scratch space. Rebuilt from scratch by `initialize_stack` for
  every file. Receives the fully-merged node state from `push()` via
  `doc_bb.process_event(NodeUpdate)`. Represents "what this file contains after parsing."
- **`session_bb`**: Accumulated session state. NOT cleared per file. Grows across all
  files parsed in the session. In parallel task builders it starts from
  `epoch_session_snapshot` (networks + const-namespace assets + index.md anchor nodes).
  Acts as the task's local proxy for `global_bb`: `cache_fetch` checks `session_bb`
  before querying `global_bb`, avoiding a mutex round-trip on cache hits.

**`cache_fetch` resolution chain** (checked in order):

1. **`doc_bb`** (when `check_local = true`): node already parsed in this file — fastest
2. **`session_bb`** (StackCache): node in accumulated session state — no `missing_structure`
   populated; does not trigger `missing_structure` merge to avoid corrupting `doc_bb`
3. **`global_bb`** (GlobalCache): node fetched from accumulator; full neighborhood
   returned in `missing_structure`, merged into `session_bb` for future StackCache hits
4. **Generated**: no cache hit — fresh time-based BID assigned. Inserted into `doc_bb`
   only; `session_bb` does NOT receive it at push-time (arrives later via `compute_diff`)

`cache_fetch` returns a `NodeSource` enum (`StackCache`, `GlobalCache`, `SourceFile`,
`Generated`, `Merged`) that governs downstream behavior in `push()`.

**`terminate_stack` and `compute_diff`:**

After all `push` and `push_relation` calls complete, `terminate_stack` calls
`compute_diff(old_set=session_bb, new_set=doc_bb, parsed_nodes)`:

- **`NodeUpdate`**: node is in `doc_bb` (new) but absent from `session_bb` (old), or its
  state differs. Generated nodes (not in `session_bb`) are correctly picked up here.
- **`RelationUpdate`**: edge is in `doc_bb` scoped to `parsed_nodes` but not in `session_bb`
- **`NodesRemoved`**: node was reachable from `parsed_nodes` in `session_bb` (old) but is
  absent from `doc_bb` (new) — i.e., a node was deleted from this document

`compute_diff` events are sent via `tx` to the `BeliefAccumulator`, then applied to
`session_bb` in `terminate_stack` so subsequent files in the same task see the result.

**Key Insight**: `doc_bb` and `session_bb` intentionally diverge during Phase 1 and 2.
The delta between them is precisely the set of changes this file's parse introduces,
which `compute_diff` turns into `BeliefEvent`s for `global_bb`.

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

#### Two-Registry Codec Dispatch (`parse_one_path`)

`parse_one_path` uses a three-branch dispatch to route each file to the correct codec.
`CLAIM_MAP.get(path)` is the single entry point — it checks the claim registry first,
then falls back to `CODECS.path_get()` internally, so callers do not need to consult
`CODECS` directly for dispatch.

1. **`CLAIM_MAP.get(path)` returns `Some`** — a codec factory was found, either from an
   explicit claim registered by a network codec during Phase 1, or from the `CODECS`
   extension/stem registry (e.g. `index.md` → `NetworkCodec`, `.xlsx` → `XlsxCodec`).
   Plain `.md` files are claimed by `NetworkCodec::parse()` during Phase 1; the bare `.md`
   extension entry was removed from `CODECS` so they always go through the claim path.

2. **`CLAIM_MAP.is_rejected(path)`** — path was explicitly rejected by a network's
   whitelist/blacklist filter (`CLAIM_MAP.reject()` stores a `None` sentinel).
   `CLAIM_MAP.get()` returns `None` for rejected paths; `is_rejected()` distinguishes
   "rejected" from "never seen". Routes to `UnclaimedDataCodec` + `ParseDiagnostic::info`.

3. **`WALK_CODECS.should_track(path)` but no claim** — file is walk-visible but no codec
   claimed it and `CODECS` has no entry for it (e.g. a stray `.yaml` in a corpus without
   a YAML-owning network codec). Routes to `UnclaimedDataCodec` + `ParseDiagnostic::info`.
   Does NOT reach `process_asset`.

4. **Neither** — genuine binary asset (image, PDF, etc.). Routes to `process_asset`
   (existing behavior, unchanged).

**Claiming pattern for structured data codecs**: A codec that owns structured data files
calls `CLAIM_MAP.claim(path, factory)` inside its `DocCodec::parse()` implementation,
using `proto_index.children_of()` to discover candidate files. `DocCodec::parse()` receives
a `proto_index: &ProtoIndex` parameter for this purpose (added in Issue 68).

**WASM note**: `WALK_CODECS` and `CLAIM_MAP` are native-only (`#[cfg(not(target_arch = "wasm32"))]`).
The WASM viewer uses a runtime extension set initialized from `BUILTIN_EXTENSIONS` and
updated at startup from `codecs.json` — a codec manifest written by `export_beliefbase`
that lists all extensions known at build time (`CODECS` + `WALK_CODECS`). The viewer
fetches `codecs.json` and calls `BeliefBaseWasm.setKnownExtensions()` before any link
resolution, ensuring custom extensions from application shims (e.g. `.yaml`, `.h`) are
correctly rewritten to `.html`. See `CodecManifest` in `shard/manifest.rs`.

`codecs.json` is a **required asset**, and the viewer treats a missing, unreachable
or malformed manifest as a fatal init error rather than falling back to
`BUILTIN_EXTENSIONS`. The fallback is worse than the failure: with only the
built-ins, a link to any walk-codec or shim-extension document normalises to a
directory URL that 404s, so the site appears healthy until a reader clicks the
wrong link. Failing at startup converts a broad, silent, per-link fault into one
loud one. Both export paths write the manifest unconditionally, and the viewer
has already fetched the shell and the WASM binary from the same origin by this
point — so absence indicates an incomplete deployment, not a transient condition,
and is not retried.

### 3.3. DocumentCompiler: Orchestration and Work Queue

`DocumentCompiler` is the build-system driver. It owns the `ProtoIndex`, the work
queue, per-path parse counts, and the `GraphBuilder`. It exposes two public parse
drivers:

| Method | Description |
|--------|-------------|
| `parse_sequential` | Single-threaded. Three-phase: (1) network dirs depth-grouped shallowest-first, (2) all leaf documents, (3) remainder loop for assets and re-parses. Uses `BeliefSink::apply_batch` to drain the event channel between files. No epoch/batch machinery. |
| `parse_all` | Parallel. Drives epoch batches via `BeliefAccumulator`; epoch-0 uses depth-grouped network dirs then a single leaf batch; remainder loop handles assets and re-parses. Each batch is bounded by `BatchStart`/`BatchEnd`/`drain_epoch`. |

**State:**

```rust
pub struct DocumentCompiler {
    /// Whether to write rewritten content back to disk.
    write: bool,

    /// Maximum number of parallel parse tasks. 1 = sequential path.
    jobs: usize,

    /// Optional HTML output directory. `None` means source-only mode.
    html_output_dir: Option<PathBuf>,

    /// Injected `<script>` tag for the HTML template.
    html_script: Option<String>,

    /// Whether to use CDN-hosted assets instead of local copies.
    use_cdn: bool,

    /// Optional base URL prefix for generated HTML links.
    base_url: Option<String>,

    /// The GraphBuilder that owns the session belief state and the event tx channel.
    builder: GraphBuilder,

    /// Pre-built filesystem index: network dirs → ordered child lists.
    /// Built once at startup via a single O(files) WalkDir pass.
    proto_index: ProtoIndex,

    /// Ordered work queue for remainder (reparse) passes.
    /// Entries are sorted by `processed[path]` ascending: assets (count=0)
    /// sort before doc rereparses (count=1+), which sort before repeated rereparses.
    remainder_queue: VecDeque<PathBuf>,

    /// Number of times each path has been parsed. Used for reparse-limit enforcement
    /// and remainder_queue ordering.
    processed: HashMap<PathBuf, usize>,

    /// Maximum number of times any path may be reparsed before emitting
    /// ReparseLimitExceeded and dropping the path.
    max_reparse_count: usize,

    /// Latest ParseResult per path — final output after the last parse of each file.
    latest_results: HashMap<PathBuf, ParseResult>,

    /// Paths whose HTML generation was deferred (contain MyST directives that need
    /// a graph query pass). Processed by `generate_deferred_html` after parse_all.
    deferred_html: Vec<PathBuf>,

    /// Paths being processed in the current epoch batch. Used to detect same-batch
    /// siblings: a file with an unresolved Id-keyed ref to a sibling re-queues itself
    /// so the link gets another chance once the sibling's output is in session_bb.
    /// Cleared after each batch's results are processed.
    current_batch: HashSet<PathBuf>,

    /// NodeKeys confirmed permanently unresolvable — failed on a prior pass with no
    /// possibility of resolution (broken wikilinks, dead asset paths, etc.).
    /// When every unresolved ref in a file's diagnostic list has its primary key here,
    /// the file is not re-queued. Prevents re-parse storms from broken links.
    permanently_unresolved: BTreeSet<NodeKey>,
}
```

**`BeliefSink`**: the trait used by `parse_sequential` (and `BeliefAccumulator` internally)
to apply a batch of `BeliefEvent`s to a backing store in one call:

```rust
pub trait BeliefSink: Send {
    fn apply_batch(&mut self, events: &[BeliefEvent])
        -> impl Future<Output = Result<(), BuildonomyError>> + Send;
}
```

`BeliefBase` applies events one-by-one via `process_event`. `DbConnection` (feature
`service`) wraps the batch into a single database `Transaction` for atomic commit.
`parse_sequential`'s `drain_rx!` macro calls `apply_batch` after each file or
depth-group to keep the caller's `global_bb` incrementally warm between files.

**Three-phase structure of `parse_sequential` and `parse_all`:**

Both drivers share the same logical phases; `parse_sequential` uses sequential
`run_one!` + `drain_rx!` macros while `parse_all` uses `parse_epoch` batches
bounded by `BatchStart`/`BatchEnd`/`drain_epoch`:

```
Phase 1 — network dirs, one depth-level at a time:
  net_dirs = proto_index.network_dirs()   // shallowest-first
  for each depth group in net_dirs:
    [parse_sequential]: run each dir individually, drain rx after each
    [parse_all]:        BatchStart → parse_epoch(group) → BatchEnd → drain_epoch()

Phase 2 — all leaf documents (non-dir children of every network dir):
  leaf_batch = all non-dir children not yet processed
  [parse_sequential]: run each leaf individually, drain rx after each
  [parse_all]:        BatchStart → parse_epoch(leaf_batch) → BatchEnd → drain_epoch()

Phase 3 — remainder loop (assets + re-parses):
  while remainder_queue is non-empty:
    candidates = remainder_queue.drain(), sorted by (processed_count, DFS_order)
    increment processed counts for all candidates before dispatch

    Sub-epoch A — assets only (processed_count == 1, i.e. first-time items):
    [parse_sequential]: run each individually, drain rx after each
    [parse_all]:        BatchStart → parse_epoch(assets) → BatchEnd → drain_epoch()
                        sync_asset_snapshot()  // pull asset nodes into session_bb

    Sub-epoch B — document re-parses (processed_count >= 2):
    [parse_sequential]: run each individually, drain rx after each
    [parse_all]:        BatchStart → parse_epoch(reparsed) → BatchEnd → drain_epoch()

  // Splitting into two sub-epochs is required for parallel correctness: asset BIDs
  // must be committed to global_bb (drain_epoch) before document tasks that reference
  // them run, otherwise those tasks see the asset as unresolved and re-queue again.
```

**Why depth-grouping matters**: `try_initialize_stack_from_session_cache` needs the
parent network node already in `global_bb` to fire the `GlobalCache` fast path.
Committing all depth-D dirs before any depth-D+1 dir is dispatched guarantees this
invariant. Within a depth group all dirs are mutually independent (no parent/child
relationship), so they may run in parallel safely.

**`ProtoIndex::network_dirs()` — canonical network directory order:**

Returns every directory that owns an `index.md`, sorted shallowest-first (primary
key: component count ascending, secondary key: lexicographic). Subnet directories
are returned at their natural depth; plain directories (no `index.md`) are not
included.

**Watch service integration**: `on_file_modified(path)` resets `processed[path] = 0`
and pushes to the front of `remainder_queue`. `on_file_deleted(path)` removes from
`remainder_queue` and `processed`. The watch service calls `parse_all` with a live
`DbConnection` as `global_bb`; `BatchStart`/`BatchEnd` are no-ops in
`Transaction::add_event`, so the epoch machinery is inert and the watch path is
unchanged by the parallel compilation work.

### 3.4. BeliefAccumulator: Epoch Commit and Query Access

`BeliefAccumulator` sits between the `GraphBuilder` event stream and `global_bb`. It
is the mechanism that makes parallel epoch correctness possible.

**Responsibilities:**

- Receives `BeliefEvent`s from the shared channel in `BatchStart`/`BatchEnd`-delimited
  batches
- Buffers events inside a batch; applies them atomically on `BatchEnd`
- `drain_epoch()` is called by `DocumentCompiler` after each epoch to commit the batch
  and invalidate the query cache
- Provides a `QueryHandle` — an `Arc`-backed, cheaply cloneable handle that task
  builders use as their `global_bb` during `parse_epoch`

**`QueryHandle` as `global_bb`:**

Each parallel task's `GraphBuilder` receives a `QueryHandle` clone at task creation.
When `cache_fetch` reaches the GlobalCache branch, it queries this handle. Because
`QueryHandle` is a snapshot of `global_bb` at the start of the epoch, all tasks in
the same epoch see the same consistent baseline. Tasks cannot see each other's
outputs until `drain_epoch()` runs.

**Inter-epoch correctness invariant:**

```
epoch N tasks all query global_bb snapshot S(N)
    ↓ all tasks finish
drain_epoch() commits all epoch-N events to global_bb → S(N+1)
    ↓
epoch N+1 tasks all query global_bb snapshot S(N+1)
```

This guarantees that every depth-D network node is committed to `global_bb` before
any depth-(D+1) parallel task starts, and that asset BIDs committed in remainder
sub-epoch A are visible to document re-parses in sub-epoch B.

**`BatchStart` / `BatchEnd` semantics:**

- Events arriving outside a batch (before `BatchStart` or after `BatchEnd`) are
  applied immediately — used by the pre-epoch sequential root parse.
- Events inside a batch are buffered and applied together on `BatchEnd`. The
  accumulator's query cache is invalidated at that point.
- `drain_epoch()` blocks until the channel drains; it is always called after
  `BatchEnd` has been sent.

**`EpochDrain` trait:**

`parse_all` requires its `global_bb` parameter to implement `EpochDrain`
(providing `drain_epoch()`). `BeliefAccumulator` implements this trait.
`BeliefBase`'s implementation is a no-op, correct only in unit-test contexts
where no inter-epoch state sharing is expected.

### 3.5. BeliefBase vs BeliefGraph: Full API vs Transport Layer

The codebase maintains two distinct but related structures for representing compiled graphs:

**BeliefGraph: Lightweight Transport Structure**

```rust
pub struct BeliefGraph {
    pub states: BTreeMap<Bid, BeliefNode>,
    pub relations: BidGraph,
}
```

`BeliefGraph` is a minimal structure optimized for:
- **Query results**: `QueryPackage` evaluation produces a `BeliefGraph` as its output graph
- **Network transport**: Serialization between services (shard export, WASM boundary)
- **Set operations**: Union, intersection, difference operations
- **Conversion**: `From<BeliefGraph> for BeliefBase` enables full API access on query results

It contains only the essential graph data (states + relations) without the indexing overhead.

**BeliefBase: Full-Featured API**

```rust
pub struct BeliefBase {
    states: BTreeMap<Bid, BeliefNode>,              // Node storage
    relations: Arc<RwLock<BidGraph>>,               // Edge storage (petgraph StableGraph)
    bid_to_index: RwLock<BTreeMap<Bid, NodeIndex>>, // BID → graph NodeIndex (kept current
                                                    // incrementally — no lazy rebuild)
    brefs: BTreeMap<Bref, Bid>,                     // Short ref → BID
    paths: PathMapMap,                               // Path/ID/Title ↔ BID (per-network)
    label: &'static str,                            // Diagnostic label ("doc_bb", "session_bb", …)
}
```

`BeliefBase` is the full-featured structure providing:
- **Identity resolution**: Multiple lookup indices (BID, Bref, ID, Path, Title) via
  `PathMapMap` and `brefs`
- **Graph operations**: Context queries, traversals, `evaluate` (via `QueryPackage`), `get_context`
- **Validation**: Invariant checking via `is_balanced()` / `diagnostics()`
- **Incremental updates**: `process_event`, `merge_from`, `compute_diff`
- **Thread-safe access**: `Arc<RwLock<BidGraph>>` for concurrent reads; `RwLock` on
  `bid_to_index` updated incrementally on every node insert/remove

**Conversion Pattern:**

```rust
impl From<BeliefGraph> for BeliefBase {
    fn from(beliefs: BeliefGraph) -> Self {
        BeliefBase::new_unbalanced(beliefs.states, beliefs.relations, false)
            .with_label("bg_bb")
    }
}
```

`QueryPackage` evaluation produces a `BeliefGraph` as output, which can be converted to `BeliefBase` when full API access is needed. This separation enables:
- Lightweight serialization over network boundaries (shard export, WASM)
- Fast set operations on query results before materializing as BeliefBase
- Clean `From<BeliefGraph>` conversion when identity resolution is needed

**Usage Pattern:**

```rust
// Build a query and evaluate via BeliefSource
let spec = QuerySpec { subject, projection };
let mut package = QueryPackage::balanced(spec);
source.evaluate(&mut package).await?;

// Extract the output BeliefGraph
let graph: BeliefGraph = package.take_graph().unwrap();

// Convert to BeliefBase for full API access (identity resolution, get_context)
let belief_set: BeliefBase = graph.into();
let context = belief_set.get_context(some_bid)?;
```

**Graph Operations:**

1. **Set Operations** (union, intersection, difference):
   - Combine multiple BeliefBases (e.g., merging branches)
   - Used for computing deltas between versions

2. **Filtering** (`filter_states`, `filter_paths`):
   - Extract subgraphs by node properties or path patterns
   - Enable scoped queries (e.g., "all documents under /docs")

3. **Graph Traversal** (`get_context`, `evaluate` via `QueryPackage`):
   - Compute sources/sinks for a node
   - Walk parent/child relationships via `TraversalSpec`

4. **Incremental Updates** (`process_event`):
   - Handle add/remove/update events from builder
   - Maintain invariants during mutations

**Incremental Indexing:**
`bid_to_index` is maintained incrementally via `graph_insert_node` / `graph_remove_node`,
which update both the `StableGraph` and the `bid_to_index` map atomically. There is no
lazy rebuild — the index is always current, enabling O(log N) BID → NodeIndex lookups
for `update_relation` in `merge_graph_mut` without a full graph scan.

### 3.6. DocCodec: The Frontend Interface

> **Note**: The HTML generation API shown in this section reflects an earlier design.
> The `generate_deferred_html` trait method has been removed. The deferred phase is now
> owned entirely by `DocumentCompiler::generate_html_for_path`, which runs an async query
> pipeline defined in `src/codec/myst.rs`. See
> [`myst_directive_architecture.md`](./myst_directive_architecture.md) for the current
> specification of the deferred pipeline, sentinel splicing, and the `DirectiveDef`
> registry. The conceptual two-phase model (immediate → deferred) remains accurate;
> only the implementation boundary has moved from the codec to the compiler.

The `DocCodec` trait defines the contract for file format parsers:

```rust
pub trait DocCodec {
    fn proto(&self, path: &Path) -> Result<Option<IRNode>, BuildonomyError>;
    fn parse(&mut self, content: &str, current: IRNode, diagnostics: &mut Vec<ParseDiagnostic>)
        -> Result<(), BuildonomyError>;
    fn nodes(&self) -> Vec<IRNode>;
    fn inject_context(&mut self, node: &IRNode, ctx: &BeliefContext,
        diagnostics: &mut Vec<ParseDiagnostic>) -> Result<Option<BeliefNode>, BuildonomyError>;
    fn finalize(&mut self, diagnostics: &mut Vec<ParseDiagnostic>)
        -> Result<HashMap<Bid, IRNode>, BuildonomyError>;
    fn generate_source(&self) -> Option<String>;

    // Content mode: Text (default) or Binary. Binary codecs re-open the file from
    // current.path and use generate_source_bytes() for write-back instead of generate_source().
    fn content_mode(&self) -> CodecContentMode { CodecContentMode::Text }
    fn generate_source_bytes(&self) -> Option<Vec<u8>> { None }

    // HTML Generation API (two-phase)
    fn should_defer(&self) -> bool { false }
    fn generate_html(&self) -> Result<Vec<(String, String)>, BuildonomyError> { Ok(vec![]) }
    // Note: generate_deferred_html has been removed from this trait.
    // The deferred phase is now handled by DocumentCompiler::generate_html_for_path
    // via the myst::DIRECTIVES query pipeline. See myst_directive_architecture.md.
}
```

#### Factory Pattern Architecture

Codecs are created via **factory functions** (`type CodecFactory = fn() -> Box<dyn DocCodec>`), not singletons:

```rust
// CodecMap stores entries as (Option<stem>, Option<extension>, CodecFactory).
// Lookup priority: stem+extension match → extension-only match → (None,None) wildcard.
pub struct CodecMap(Arc<RwLock<Vec<(Option<String>, Option<String>, CodecFactory)>>>);

impl CodecMap {
    pub fn create() -> Self {
        // Built-in registrations (non-WASM):
        //   (Some("index"), Some("md")) → NetworkCodec  (index.md by name)
        //   (None, None)                → NetworkCodec  (bare directory paths)
        //   (None, Some("xlsx"))        → XlsxCodec     (xlsx feature only)
        //
        // Note: plain .md files are NOT registered here by bare extension.
        // They are claimed per-network by NetworkCodec::parse() via CLAIM_MAP.
        // CLAIM_MAP.get() falls back to CODECS.path_get() internally, so callers
        // use CLAIM_MAP.get() as the single dispatch entry point.
    }

    pub fn path_get(&self, path: &Path) -> Option<CodecFactory> { ... }
    pub fn get(&self, ap: &AnchorPath) -> Option<CodecFactory> { ... }
    pub fn insert_codec(&self, stem: Option<String>, ext: Option<String>, factory: CodecFactory) { ... }
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
- For documents containing MyST directives with deferred content, `generate_html` emits
  a sentinel placeholder string (e.g. `<!--@@noet-network-children@@-->`) at the
  directive's position. `should_defer()` returns `true` to signal the deferred pass.
- Returns: `Vec<(String, String)>` of (filename, html-body)

**Phase 2: Deferred Generation** (compiler-owned, not a codec method)
- After all documents are parsed, `DocumentCompiler::generate_html_for_path` runs an
  async query pipeline for each document in the deferred queue.
- Each `DirectiveDef` in `myst::DIRECTIVES` that has a non-empty sentinel declares a
  `queries` slice of refiner functions. The compiler runs these against the live
  `BeliefSource`, accumulating results in a `Vec<BeliefGraph>`. The sync `builder`
  function receives this slice and produces HTML. `myst::splice_sentinels` replaces
  the placeholder in the on-disk HTML file.
- Use for: Dynamic content (network child listings, requirements traceability tables,
  cross-references that need full graph context).

**Deferral Signal**: `should_defer()` tells the compiler to enqueue this document:
- `false` (default): Only immediate generation needed
- `true`: Document contains at least one sentinel-bearing directive; deferred pass required

**See** [`myst_directive_architecture.md`](./myst_directive_architecture.md) for the
complete specification of the directive registry, query pipeline, sentinel protocol, and
extension point.

**Current Implementations:**

- **MdCodec** (`md.rs`): `CodecContentMode::Text` (default)
  - Parses Markdown with TOML frontmatter
  - Generates HTML from pulldown-cmark AST
  - Rewrites internal links to `.html` extension
  - Extracts headings for structural hierarchy

- **NetworkCodec** (`network.rs`): wraps `MdCodec` for network index files
  - Always outputs `index.html`
  - Handles `{network_children}` sentinel injection

- **XlsxCodec** (`xlsx/codec.rs`, `xlsx` feature, non-wasm): `CodecContentMode::Binary`
  - Reads `.xlsx` and `.ods` files via `calamine`
  - Reserved `index` tab carries YAML/TOML/JSON schema declaration
  - Emits workbook (Document, h=2) → tab (Symbol, h=3) → row (Symbol, h=4) hierarchy
  - BID write-back via `__noet_bid__` annotation column using `rust_xlsxwriter`
  - `parse()` ignores `content: &str`; re-opens file from `current.path`

**Key Responsibility**: Codecs are **syntax-only** for parsing. They produce IRNodes with unresolved references (NodeKey instances). The builder handles semantic analysis and linking. For HTML generation, codecs are **presentation-only** — they return body content, compiler wraps with templates.

For binary codecs, `generate_source_bytes() -> Option<Vec<u8>>` is called instead.
The compiler checks `content_mode()` (via a cheap probe instantiation) and routes
write-back accordingly.

### 3.7. The Document Stack: Nested Structure Parsing

The `GraphBuilder` maintains a single stack (`self.stack: Vec<(Bid, String, usize)>`)
that serves as both the ancestor-network context and the in-document heading
hierarchy. Each entry is `(bid, absolute_path, heading_level)` where heading
levels encode node kind:

| `heading` value | Node kind | Notes |
|---|---|---|
| `1` | Network | Absolute directory path stored as path component |
| `2` | Document | Absolute file path |
| `3` | H1 heading anchor | Absolute section path (net_path joined with anchor) |
| `4` | H2 heading anchor | |
| … | … | heading N+2 = HTML heading level N |

`build_path_key` walks the stack in reverse to find the innermost `heading == 1`
entry (the owning network). It uses that entry's absolute directory path as the
strip prefix to compute the subnet-relative path for the `NodeKey::Path`. See
the "Network Node Dual-Path Representation" note in section 2.2 for the
`AnchorPath::new_dir` requirement at this call site.

The stack mechanism (`codec/builder.rs`) enables hierarchical document parsing:

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

### 3.8. Query Evaluation: BeliefSource and the QueryPackage Pipeline

`BeliefSource` is the trait that all query backends implement. The primary
entry point is `evaluate(&mut QueryPackage)`, which drives a `QuerySpec`
through a three-stage lifecycle:

```
Constructed → Anchored → Projected
```

1. **Anchor**: Resolve `Subject` to `Vec<Bid>` (the seed set).
2. **Graph context**: When the caller uses `QueryPackage::balanced(spec)`,
   halo and section-roots projection steps are appended to the effective
   spec before evaluation. These use `StepInput::Cumulative` so the
   traversal operates on `seed ∪ all prior tape BIDs`.
3. **Project**: Walk each `ProjectionStep` (Filter, Traverse, Compose),
   building a `Tape` of intermediate BID sets. The package graph is
   populated incrementally during projection — discovered edges and
   endpoint nodes are added at each hop. After evaluation, the package
   graph + tape together ARE the evaluation output.

#### Backend Implementations

**BeliefBase (in-memory):** All four stages operate on the in-memory graph.
`eval_subject` resolves via `PathMapMap` lookups. `apply_traversal` walks
`petgraph` edges using `bid_to_index` for O(log N) BID → NodeIndex
lookup. `apply_filter` evaluates `PropertyPredicate` against node states.
`materialize_graph` extracts the subgraph for all accumulated BIDs with
Trace coloring: BIDs in the primary set (pre-halo/section-roots) are full
nodes; BIDs discovered by halo/section-roots steps are marked `BeliefKind::Trace`.

**DbConnection (SQL):** Subject resolution issues SQL against the `beliefs`
and `paths` tables (reusing `resolve_net_path` / `resolve_net_id` for
`NodeKey::Path` and `NodeKey::Id` variants). Each traversal hop issues one
SQL query per active input role against the `relations` table:

- **Source input:** `WHERE source IN (frontier) AND {kind_col} IS NOT NULL`
- **Sink input:** `WHERE sink IN (frontier) AND {kind_col} IS NOT NULL`
- **Owner input:** `WHERE owned_by IN (frontier_brefs) AND {kind_col} IS NOT NULL`

The frontier advances by collecting output-role endpoints from matched
edges. Filters fetch states via SQL, then apply `NodeFilter` predicates
in memory. After all projection steps complete, a single bulk fetch
retrieves states and relations for all accumulated BIDs and populates
the package graph. Seed nodes are marked as complete; discovered
endpoints from halo/section-roots steps are marked `BeliefKind::Trace`.

#### Query Cost Model

For a typical balanced `QueryPackage` evaluation on a single node, the DB backend issues:

| Phase | Queries | Description |
|-------|---------|-------------|
| Subject | 1 | Bref/BID/path/id resolution |
| Halo (1 hop) | 1–3 | Source + Sink + Owner input roles |
| Section roots | 3–8 | One per ancestor level to root |
| Bulk states | 1 | `SELECT * FROM beliefs WHERE bid IN (...)` |
| Bulk relations | 1 | `SELECT * FROM relations WHERE sink/source IN (...)` |
| Orphaned endpoints | 0–1 | Missing edge-endpoint states |
| **Total** | **~7–15** | |

#### Optimization Opportunities

The current implementation issues one SQL query per traversal hop. Several
optimizations can reduce this:

**Recursive CTE for unbounded traversals.** SQLite’s `WITH RECURSIVE`
can collapse an unbounded section roots walk (`k-section-s {max}`)
into a single query:

```sql
WITH RECURSIVE ancestors(bid) AS (
    SELECT sink FROM relations
    WHERE source IN (seed_bids) AND section IS NOT NULL
  UNION
    SELECT r.sink FROM relations r
    JOIN ancestors a ON r.source = a.bid
    WHERE r.section IS NOT NULL
) SELECT bid FROM ancestors;
```

This eliminates the per-level round-trips for the most common traversal
pattern (Section roots walk). The same pattern applies to any fixed-kind
unbounded traversal.

**Batched halo query.** The halo (`sko-[*]-sko {1}`) currently issues
separate queries per input role. Since it queries all three roles, a single
query with `OR` conditions on `source IN`, `sink IN`, and `owned_by IN`
would reduce three round-trips to one.

**Path-table acceleration for Section traversals.** A Section traversal
with an `EdgePredicate` on `doc_paths` is structurally equivalent to a
`paths` table walk. The `paths` table already stores the
network → document → section hierarchy with ordering. When the
`QuerySpec` shape matches (subject within a known network, Section-only
traversal, deterministic path prefix), the traversal can be rewritten as
a `paths` table prefix scan:

```sql
SELECT target FROM paths WHERE net = ? AND path LIKE ? || '%'
```

This is O(1) index lookup vs O(depth) iterative edge walking.

**States cache across the evaluate call.** `apply_filter_sql` fetches
states into a temporary `BeliefBase` per filter step. A shared
`BTreeMap<Bid, BeliefNode>` cache across the entire `evaluate` call
would avoid redundant fetches when multiple filter steps or the final
bulk materialization re-request the same states.

**Temp table for large frontier sets.** When frontier sets exceed
hundreds of BIDs, `IN (...)` clause parsing becomes expensive. A
SQLite temp table (`CREATE TEMP TABLE frontier(bid TEXT)`) with a
`JOIN` is more efficient for large sets.

**BeliefBase in-memory evaluation.** The in-memory backend has its own
optimization surface — `apply_traversal` walks `petgraph` edges
per-hop, `materialize_graph` clones nodes into a new `BeliefGraph`,
and `QueryPackage::balanced()` / `append_graph_context` appends halo/section-roots
steps unconditionally. Profiling
against large corpora (30k+ nodes) will identify whether traversal
hot paths, graph materialization copies, or index lookups dominate.
See BACKLOG for the placeholder.

## 4. Integration Points

### 4.1. Upstream: Source Files
- Reads: `*.md`, `*.toml` files discovered via `ProtoIndex` (built at startup)
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

### 4.5. Shard Export and Compile-Time Search Indices

`finalize_html` (called after `parse_all`) writes two categories of output to the HTML directory: compile-time search indices (always) and BeliefBase data (monolithic or sharded depending on size). The implementation lives in `src/shard/`.

#### ShardConfig

```rust
pub struct ShardConfig {
    /// Byte threshold above which the export is split into per-network shards.
    /// Default: 10 MB.
    pub shard_threshold: usize,
    /// Browser memory budget for loaded data shards (MB). Default: 200 MB.
    pub memory_budget_mb: f64,
}
```

`ShardConfig::should_shard(serialized_bytes) -> bool` is the single decision point. It is called once after the full `BeliefGraph` is serialized to measure its size.

#### Search Index Format

`build_search_indices` runs unconditionally before the sharding decision. It writes:

- `search/manifest.json` — `SearchManifest { version, networks: Vec<NetworkSearchMeta> }`
- `search/{bref}.idx.json` — one `SearchIndex` per network

```rust
pub struct SearchIndex {
    pub network_bref: String,
    pub doc_count: usize,
    /// Whether terms are stemmed: "English" | "None"
    pub stemmed: StemMode,
    /// Document metadata list (title, path, term_count)
    pub docs: Vec<IndexedDoc>,
    /// Inverted index: term → Vec<(doc_index, tf_idf_score)> sorted descending
    pub index: BTreeMap<String, Vec<(usize, f32)>>,
}
```

Tokenization pipeline (in `src/shard/search.rs::tokenize`):
1. Lowercase and split on whitespace/punctuation
2. Drop tokens shorter than 3 chars or purely numeric
3. Remove English stop words (~150-word `ENGLISH_STOP_WORDS` set)
4. Apply Snowball English stemming via `rust-stemmers` (feature-gated; no-op shim when disabled)

The `stemmed` field records the mode used at index-build time so the WASM query side (Issue 54) applies the same transformation to query terms before index lookup.

#### Shard Wire Formats

In sharded mode, `export_sharded` writes:

```rust
// beliefbase/global.json
pub struct GlobalShard {
    pub states: BTreeMap<String, BeliefNode>,   // API node + namespace nodes + unowned nodes
    pub relations: SerializableBidGraph,         // Cross-network edges
}

// beliefbase/networks/{bref}.json
pub struct NetworkShard {
    pub network_bref: String,
    pub network_bid: String,
    pub states: BTreeMap<String, BeliefNode>,   // Nodes owned by this network
    pub relations: SerializableBidGraph,         // Intra-network edges only
}
```

`SerializableBidGraph` uses BID strings as node identifiers instead of petgraph internal indices, which are not stable across independent deserialization.

The `ShardManifest` written to `beliefbase/manifest.json` lists per-network metadata (bref, bid, title, node count, estimated size, shard path) and global shard metadata. It is the first file the viewer fetches in sharded mode.

#### BeliefBaseWasm Shard-Aware API

`BeliefBaseWasm` (in `src/wasm.rs`) was extended in Issue 50 with:

| Method | Description |
|--------|-------------|
| `from_manifest(manifest_json, entry_bid)` | Creates an empty `BeliefBase`; caller must then call `load_shard` |
| `load_shard(bref_key, shard_json)` | Deserializes a `NetworkShard` or `GlobalShard`, calls `BeliefBase::merge` |
| `unload_shard(bref_key)` | Removes tracked BIDs via `process_event(NodesRemoved, Remote)` |
| `loaded_shards()` | Returns JSON array of currently-loaded bref keys |
| `has_bid(bid_str)` | Returns `true` if a BID is present in the loaded graph |
| `memory_usage_mb()` | Heuristic node-count estimate for UI display |

Internally, `BeliefBaseWasm` maintains a `loaded_shards: RefCell<HashMap<String, BTreeSet<Bid>>>` that tracks which BIDs belong to which shard. `unload_shard` filters out BIDs still referenced by other loaded shards before removing them, preventing incorrect removal of nodes shared between the global shard and a network shard.

The `"global"` key is reserved for the global shard. Network shards use their 5-hex-char bref as the key.

See `docs/design/search_and_sharding.md` for the complete specification including manifest JSON schemas, memory budget model (§6), and WASM integration (§8).

### 4.6. UI Layer
- Query interfaces for filtered graph views
- Content access for file editing
- Event subscription for reactive rendering
- **Network Selector panel** (sharded mode): collapsible panel in the left nav showing per-network checkboxes, node counts, estimated shard sizes, and a memory usage bar (yellow ≥ 80%, red ≥ 90% of the 200 MB budget). Implemented in `assets/viewer/network-selector.js` and driven by `assets/viewer/shard-manager.js`. Hidden in monolithic mode.

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
1. MdCodec extracts frontmatter → IRNode with `id`, `title`, `schema`
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

### 5.3. Event Synchronization and BeliefBase Export

When the compiler finishes parsing, all `BeliefEvent`s must be committed to `global_bb`
before the result is exported. `BeliefAccumulator` handles this via its `into_inner()`
method, which drains the channel and returns the fully-populated `BeliefBase`.

#### Pattern: BeliefAccumulator

```rust
// 1. Create accumulator wrapping an empty (or pre-populated) BeliefBase
let (accum_tx, accum_rx) = unbounded_channel::<BeliefEvent>();
let accum = BeliefAccumulator::new(BeliefBase::empty(), accum_rx);
let global_handle = accum.query_handle(); // QueryHandle cloned by each parse task

// 2. Create compiler with the shared tx channel
let mut compiler = DocumentCompiler::new(&repo_root, Some(accum_tx), None, write)?;

// 3. Parse all documents; each epoch is drained via drain_epoch()
compiler.parse_all(global_handle, force).await?;

// 4. Drain any remaining channel events and recover the BeliefBase
let global_bb = accum.into_inner().await?;

// 5. Now safe to export — all events committed
let graph = global_bb.clone().consume();
export_beliefbase_json(graph, html_dir).await?;
```

**Key points:**
- `accum.query_handle()` returns the `QueryHandle` that tasks query as `global_bb`
- `drain_epoch()` inside `parse_all` commits each epoch batch incrementally
- `into_inner()` performs a final drain and returns ownership of the `BeliefBase`
- Export always happens from a fully-committed `BeliefBase`

**Watch Service vs Parse Command:**
- **Watch service**: passes a live `DbConnection` as `global_bb`; `BatchStart`/`BatchEnd`
  are no-ops in `Transaction::add_event`; export is from the database after commit
- **Parse command**: uses `BeliefAccumulator` as above; export is from the in-memory
  `BeliefBase` returned by `into_inner()`

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

### 6.2. Error Recovery and Partial Compilation

**Current State**: The compiler continues processing files even when individual files fail to parse, logging errors and continuing with other files.

**Assessment**: Partial error recovery already exists at the file level. Within-file error recovery (continuing after syntax errors within a single document) is not currently implemented.

**Decision**: **Defer** - Current approach provides valuable architectural feedback during development. File-level recovery is sufficient for most use cases.

When needed, fine-grained error recovery within documents could be implemented by:
- Extending `IRNode` with an `errors: Vec<ParseError>` field
- Allowing partial node construction (e.g., node created but some relationships failed)
- Marking invalid nodes with `BeliefKind::Invalid` flag for UI feedback

### 6.3. Intermediate Representation Optimization

**Current State**: BeliefBase directly represents parsed structure without optimization passes.

**Assessment**: Current architecture is already quite efficient:
- `bid_to_index` is maintained incrementally (no lazy rebuild) — O(log N) BID → NodeIndex
- Arc-based structural sharing — `BeliefGraph` clone is cheap
- Multi-pass compilation — natural convergence without explicit optimization passes

**Decision**: **Defer to Database Layer**

Optimization is better suited for the **DbConnection** persistent cache rather than
the in-memory BeliefBase. The database can maintain pre-computed traversals,
materialized views, and deduplicated data. It can also surface suggestions back to
authors (unreachable nodes, duplicate content, unused references) without
auto-applying changes.

### 6.4. Concurrent Parsing

**Current State**: Fully implemented. See §3.4 (BeliefAccumulator) and §3.3
(DocumentCompiler) for the complete parallel architecture. Key points repeated here
for the concerns log:

`parse_epoch` spawns one `tokio::task` per path, gated by `Arc<Semaphore>` of size
`jobs`. `--jobs 1` uses a sequential inline loop as a deterministic baseline.

**Epoch invariant**: within a single epoch no file's parse output is an input to
any other file's parse in that epoch. Cross-file dependencies only flow across epoch
boundaries. This is what makes intra-epoch parallelism safe without locking
`GraphBuilder`.

**`parse_epoch` — parallel implementation (`jobs > 1`)**:

Each spawned task owns:
- A fresh `GraphBuilder` seeded with `repo_bid` and the full network-ancestor
  snapshot (`epoch_session_snapshot`) so `try_initialize_stack_from_session_cache`
  can walk upward through Section edges.
- A per-task `UnboundedSender<BeliefEvent>` (isolated channel). Events from each
  document accumulate in the task's local buffer.
- A clone of `global_bb` (cheap — `QueryHandle` is `Clone` + `Arc`-backed).

After `JoinSet::join_next` drains all tasks, per-task event buffers are forwarded
to the shared `tx` in original path index order (task 0's events before task 1's,
etc.). This enforces deterministic first-one-wins collision resolution regardless
of OS task scheduling order. The surrounding `BatchStart`/`BatchEnd` pair in
`parse_all` brackets the entire epoch so the accumulator sees one coherent batch.

**`parse_all` epoch structure (actual):**

```
Phase 1 — network dirs, depth-grouped:
  for each depth group in network_dirs() [shallowest-first]:
    BatchStart → parse_epoch(group) → BatchEnd → drain_epoch()
    // all depth-D dirs committed before any depth-D+1 dir is dispatched

Phase 2 — leaf documents (single parallel batch):
  leaf_batch = all non-dir children of all network dirs not yet processed
  BatchStart → parse_epoch(leaf_batch) → BatchEnd → drain_epoch()

Remainder loop (epoch ≥ 1):
  while remainder_queue is non-empty:
    batch = remainder_queue.drain(), sorted by (processed_count, DFS_order)
    BatchStart → parse_epoch(batch) → BatchEnd → drain_epoch()
```

**`parse_sequential` (no epochs, single-threaded):**

Structurally mirrors `parse_all` with three identical phases but uses sequential
`run_one!` + `drain_rx!` macros instead of epoch batches. No `BatchStart`/`BatchEnd`,
no `drain_epoch`. Takes `BeliefSink` (not `EpochDrain`) as its `global_bb` bound.
Used as a stable sequential baseline and in contexts where no inter-epoch
accumulator is available (e.g. watch service via `DbConnection`).

**`BeliefAccumulator`**: the `BatchStart`/`BatchEnd` sentinel pair drives
accumulator commit and cache invalidation. `drain_epoch()` must not be a no-op
for the inter-epoch cache invariant to hold — `BeliefBase`'s no-op `EpochDrain`
implementation is correct only in contexts where no inter-epoch state sharing is
expected (e.g. single-epoch unit tests that pass a bare `BeliefBase` as `global_bb`).

**`node_to_nets` reverse index**: `PathMapMap` maintains
`node_to_nets: BTreeMap<Bid, BTreeSet<Bref>>` to route `RelationUpdate` events
only to the PathMaps that contain the source or sink node (O(1) vs prior
O(N_networks) broadcast). Maintained by `rebuild_node_to_nets_for` after each
PathMap construction, and incrementally from `PathAdded` derivatives in the sort
pass of `process_event_queue`. **Critical invariant**: the `node_to_nets`
incremental update must run regardless of whether the PathMap required a sort —
skipping it when `already_sorted=true` causes section nodes at depth ≥ 2 to be
silently dropped from the PathMap.

### 6.5. Formal Grammar Specification

**Status**: For future consideration

**Current State**: Parsing logic is embedded in Rust code without formal grammar definition.

**Future Direction**: A schema registry system could provide declarative parsing rules that serve as a formal specification. Applications could define schemas declaratively, and the library could generate or validate parsing logic based on these specifications.

This would provide benefits similar to parser generators while maintaining flexibility for domain-specific parsing needs.

---

**Document Status**: Draft - This document captures the core architecture for the noet-core library, focusing on the graph compilation infrastructure that can be used by various applications.
