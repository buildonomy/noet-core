---
title = "Section Metadata Manifest Design"
version = "0.1"
status = "draft"
---

# Section Metadata Manifest Design

## Purpose

Enable document authors to attach metadata (BID, schema, custom fields) to section headings via a centralized `sections` table in document frontmatter, without polluting section headings with individual TOML blocks.

## Problem Statement

**User Need**: Authors want to:
1. Reference specific sections by stable ID across sessions (BID stability)
2. Attach metadata to sections (schema, status, complexity, etc.)
3. Keep section headings clean and readable (no TOML blocks under every heading)

**Anti-Pattern** (what we DON'T want):
```markdown
## Introduction

---
bid = "1234-5678"
schema = "TechnicalSection"
status = "draft"
---

Content here...
```

**Desired Pattern**:
```markdown
---
title = "My Document"
bid = "doc-bid"

[sections.introduction]
bid = "section-bid"
schema = "TechnicalSection"
status = "draft"
---

## Introduction

Content here (no TOML block!)
```

## Design Principles

1. **Single Source of Truth**: Document frontmatter contains ALL section metadata
2. **Lookup Table**: `sections` is a map from section ID → metadata
3. **Stable Keys**: Section IDs are normalized anchors (e.g., "introduction", "getting-started")
4. **Bidirectional Sync**:
   - **Read**: Parse `sections` table, enrich heading nodes in-memory
   - **Write**: Collect heading nodes, build/update `sections` table
5. **Garbage Collection**: Sections removed from markdown → removed from `sections` table

## Data Model

### Document Frontmatter Schema

```toml
title = "Document Title"
bid = "document-bid"
schema = "Document"

[sections.<section-id>]
bid = "section-bid"           # Required: stable identity
id = "section-id"             # Optional: explicit ID (defaults to anchor)
schema = "SectionSchema"      # Optional: section-specific schema
# ... custom metadata fields
```

### Section Key Types (Priority Order)

1. **Explicit ID** (`{#custom-id}`): `sections.custom-id`
2. **Normalized Title**: `sections.getting-started` (from "## Getting Started")
3. **BID** (fallback): `sections["bid:1234-5678"]` (when title collisions exist)

### Matching Algorithm

**Reading (Parse → Enrich)**:
```
For each heading node (heading > 2):
  1. Extract section key (ID or normalized title)
  2. Look up sections[key] in document frontmatter
  3. If found: merge metadata into heading node (in-memory only)
  4. Track matched key
```

**Writing (Finalize → Update Frontmatter)**:
```
After all inject_context() calls:
  1. Collect all section nodes (heading > 2)
  2. Build sections table: key → {bid, id, schema, ...}
  3. Compare with existing sections in frontmatter
  4. Add missing sections, remove orphaned sections
  5. Update document frontmatter if changed
```

## Implementation Architecture

### Phase 1: Reading (Issue 02 - COMPLETE ✅)

**File**: `src/codec/md.rs`

**parse()**: Extract `sections` from frontmatter, store in `ProtoBeliefNode.document`

**inject_context()** (for section nodes):
```rust
// Extract sections table from document node
let sections_metadata = if node.heading > 2 {
    self.current_events
        .first()
        .and_then(|doc| doc.0.document.get("sections"))
        .map(parse_sections_metadata)
} else {
    None
};

// Match section to metadata
if let Some(sections_map) = sections_metadata {
    if let Some((matched_key, metadata)) = find_metadata_match(&node, &sections_map) {
        self.matched_sections.insert(matched_key);
        merge_metadata_into_node(&mut node, metadata);
    }
}
```

### Phase 2: Writing (NEW - TO IMPLEMENT)

**File**: `src/codec/md.rs`

**finalize()**: Build `sections` table from all section nodes

```rust
fn finalize(&mut self) -> Result<Vec<(ProtoBeliefNode, BeliefNode)>, BuildonomyError> {
    let mut modified_nodes = Vec::new();
    
    // 1. Collect all section nodes (heading > 2)
    let mut sections_table = toml_edit::Table::new();
    
    for (section_proto, _) in self.current_events.iter().skip(1) {
        if section_proto.heading > 2 {
            if let Some(section_id) = section_proto.id.as_ref() {
                let mut metadata = toml_edit::Table::new();
                
                // Required: BID
                if let Some(bid) = section_proto.document.get("bid") {
                    metadata.insert("bid", bid.clone());
                }
                
                // Optional: id, schema, custom fields
                if let Some(id) = section_proto.document.get("id") {
                    metadata.insert("id", id.clone());
                }
                // ... other fields
                
                sections_table.insert(section_id, metadata);
            }
        }
    }
    
    // 2. Update document's sections field
    if let Some((doc_proto, doc_events)) = self.current_events.first_mut() {
        let existing_sections = doc_proto.document.get("sections");
        
        if should_update(existing_sections, &sections_table) {
            doc_proto.document.insert("sections", sections_table);
            
            // 3. Regenerate document frontmatter
            let metadata_string = doc_proto.as_frontmatter();
            update_or_insert_frontmatter(doc_events, &metadata_string)?;
            
            // 4. Create updated BeliefNode
            let updated_node = BeliefNode::try_from(doc_proto.as_ref())?;
            modified_nodes.push((doc_proto.clone(), updated_node));
        }
        
        // 5. Garbage collect unmatched sections (existing logic)
        // Remove sections from table that have no matching headings
    }
    
    Ok(modified_nodes)
}
```

## Execution Flow

### Parsing (Read Path)

```
1. parse() extracts frontmatter → ProtoBeliefNode.document["sections"]
2. parse() creates heading nodes → ProtoBeliefNode(heading > 2)
3. inject_context(document) → BID assigned
4. inject_context(section1) → match to sections[key], merge metadata
5. inject_context(section2) → match to sections[key], merge metadata
6. finalize() → garbage collect unmatched sections
```

### Writing (Write Path)

```
1. inject_context() calls complete (all nodes enriched)
2. finalize() called:
   - Collect all section nodes (now have BIDs!)
   - Build sections table from section metadata
   - Compare with existing sections in document
   - Update document frontmatter if changed
   - Generate updated BeliefNode for document
```

## Key Insight: Timing

**Critical**: `finalize()` runs **AFTER** all `inject_context()` calls

- `inject_context(document)`: Sections don't have BIDs yet ❌
- `inject_context(section1)`: Section gets BID assigned ✅
- `inject_context(section2)`: Section gets BID assigned ✅
- `finalize()`: All sections have BIDs, build table ✅

This is why we **cannot** build the sections table in `inject_context(document)` - section nodes haven't been processed yet!

## Metadata Fields

### Required Fields

- **bid**: Stable identity (UUID v7)
- **id**: Section anchor (normalized from title or explicit ID)

### Optional Fields

- **schema**: Section-specific schema type
- **status**: draft | review | complete
- **complexity**: low | medium | high
- **author**: User-defined
- **tags**: Array of strings
- **custom fields**: Schema-specific attributes

### Internal Fields (Never Written)

- **title**: Extracted from heading, not stored in sections table
- **text**: Section content, not metadata
- **heading**: Internal structure marker

## Examples

### Example 1: Basic Document with Sections

**Input Markdown**:
```markdown
---
title = "User Guide"
---

## Introduction

Welcome to the guide!

## Installation

Follow these steps...
```

**After First Parse** (BIDs assigned):
```markdown
---
title = "User Guide"
bid = "doc-1234"

[sections.introduction]
bid = "section-5678"
id = "introduction"

[sections.installation]
bid = "section-9abc"
id = "installation"
---

## Introduction

Welcome to the guide!

## Installation

Follow these steps...
```

### Example 2: Custom Metadata

**Author adds metadata**:
```markdown
---
title = "API Reference"
bid = "doc-1234"

[sections.authentication]
bid = "section-5678"
id = "authentication"
schema = "TechnicalSection"
difficulty = "advanced"
required_knowledge = ["oauth2", "jwt"]
---

## Authentication

OAuth2 flow documentation...
```

**On next parse**:
- Metadata enriches heading node in-memory
- Section node has `schema`, `difficulty`, `required_knowledge` available
- Schemas can validate custom fields
- No changes written (metadata already correct)

### Example 3: Section Removal (Garbage Collection)

**User deletes "Installation" section from markdown**:

**Before**:
```markdown
[sections.introduction]
bid = "section-5678"

[sections.installation]
bid = "section-9abc"  # ← This section will be removed

[sections.usage]
bid = "section-def0"
```

**After finalize()**:
```markdown
[sections.introduction]
bid = "section-5678"

[sections.usage]
bid = "section-def0"
```

The orphaned `sections.installation` entry is garbage collected.

## Benefits

1. **Clean Markdown**: Sections have no TOML blocks, just clean headings
2. **Centralized Metadata**: All section metadata in one place (document frontmatter)
3. **Stable References**: BIDs enable cross-document links that survive refactoring
4. **Schema Extensibility**: Custom metadata fields for domain-specific workflows
5. **Garbage Collection**: Automatic cleanup when sections removed
6. **Diff-Friendly**: Changes to section metadata are localized to document frontmatter

## Alternatives Considered

### Alternative 1: TOML Blocks Under Each Heading

```markdown
## Introduction

---
bid = "section-bid"
schema = "Section"
---

Content...
```

**Rejected**: Pollutes markdown, hard to read, poor author UX

### Alternative 2: Sidecar Files

```
docs/guide.md           # Markdown content
docs/guide.metadata.toml  # Section metadata
```

**Rejected**: File proliferation, harder to maintain consistency

### Alternative 3: Inline Attributes

```markdown
## Introduction {bid="section-bid" schema="Section"}
```

**Rejected**: Limited to simple key-value pairs, not extensible for nested metadata

## Migration Path

**For existing documents without `sections` table**:

1. First parse: Generate `sections` table from discovered headings
2. Assign BIDs to all sections
3. Write updated frontmatter
4. Subsequent parses: Stable BIDs maintained

**For documents with old-style section TOML blocks** (if any exist):

1. Parse detects TOML blocks under headings
2. Extract metadata, consolidate into document `sections` table
3. Remove individual TOML blocks
4. Write clean markdown with centralized `sections`

## Future Enhancements

1. **Schema Validation**: Validate section metadata against declared schemas
2. **Section Templates**: Auto-populate sections based on document schema
3. **Hierarchical Sections**: Nested section metadata for deep document structures
4. **Section Reordering**: Track section order independently of markdown sequence
5. **Cross-Document Sections**: Reference sections across document boundaries

## References

- Issue 02: Section Metadata Enrichment (Implementation)
- `src/codec/md.rs`: MdCodec implementation
- `docs/design/beliefbase_architecture.md` § 2.2: Identity Management
- `docs/design/beliefbase_architecture.md` § 3.5: DocCodec Interface