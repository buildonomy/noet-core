# Issue 2: Section Metadata Enrichment for Markdown Headings

**Priority**: CRITICAL - Blocks Issue 4  
**Estimated Effort**: 3 days  
**Dependencies**: Requires Issue 1 (Schema Registry)

## Summary

Enable TOML frontmatter `sections` field to provide metadata for markdown heading nodes. The `sections` field is a flat lookup map that enriches heading-generated nodes with schema types, custom fields, and validation rules. Maintains clean separation: markdown defines structure (which nodes exist), frontmatter defines metadata (what fields they have).

## Goals

1. Parse frontmatter `sections` as flat metadata lookup map
2. Match section metadata to heading-generated nodes (by BID/anchor/title)
3. Enrich matched nodes with schema, custom fields (complexity, etc.)
4. Ensure all markdown headings create nodes (enables cross-reference tracking)
5. Maintain 1:1 correspondence: all headings get sections entries (auto-generate ID if not pre-defined)
6. Maintain clean authority model: markdown = structure, sections = metadata enrichment + synchronization

## Author Workflow

This section documents the typical content creation and metadata enrichment process. This describes the **raw workflow** at the file format level. Future application software will provide helper methods and GUI tools to abstract these operations, but understanding the underlying mechanism is important for library users and integration developers.

### Step 1: Create Markdown Content

Authors write standard markdown with headings defining document structure:

```markdown
# My Document

## Introduction

This section introduces the topic.

## Background

Historical context goes here.

## Implementation

Technical details here.
```

**Result**: Parser creates nodes for document + 3 heading sections with default metadata.

### Step 2: First Parse - Metadata Injection

When first parsed, the system generates:
- Document node with auto-generated BID
- Heading nodes for each section with auto-generated BIDs
- Default schema based on file extension (`Document` for `.md`)
- No custom fields

**Frontmatter injected** (if missing):
```yaml
---
bid: 01234567-89ab-cdef-0123-456789abcdef
schema: Document
---
```

### Step 3: Metadata Enrichment (Optional)

Authors can now add the `sections` field to customize heading nodes:

```yaml
---
bid: 01234567-89ab-cdef-0123-456789abcdef
schema: Document

sections:
  "introduction":
    schema: Section
    complexity: high
    status: draft
  
  "bid://98765432-10ab-cdef-0123-456789abcdef":
    schema: TechnicalSection
    difficulty: advanced
    required_knowledge: ["databases", "networking"]
---
```

**Result on next parse**:
- `Introduction` heading node enriched with `complexity: high`, `status: draft`
- `Implementation` heading (matched by BID) enriched with custom schema + fields
- `Background` heading remains with default metadata (not in sections)

### Step 4: Refinement and Schema Application

Authors can iteratively refine metadata:

1. **Add custom schemas**: Define section types with validation rules
2. **Add cross-references**: Use schema fields to create graph edges
3. **Add domain-specific fields**: Complexity, priority, tags, etc.
4. **Preserve structure**: Markdown headings remain authoritative for which nodes exist

### Workflow Properties

**Idempotent**: Parsing the same document multiple times produces consistent results (BIDs cached in frontmatter).

**Gradual enhancement**: Documents work immediately with default metadata, authors add richness over time.

**Separation of concerns**: 
- Markdown = human-readable content structure
- Frontmatter sections = machine-processable metadata
- Schema = validation and relationship rules

**Future Tooling**: Application software will wrap these raw file operations with convenience methods:
- CLI: `noet enrich <section> --schema TechnicalSection --field complexity=high`
- CLI: `noet list-sections` (show all sections with current metadata)
- CLI: `noet validate` (check schema compliance)
- API: `Document.enrich_section(name, schema, fields)` (programmatic access)
- GUI: Visual section metadata editor with schema-aware forms

**Why document the raw workflow?**
- Library users need to understand the file format for integration
- Enables custom tooling development
- Clarifies the authority model (markdown structure vs. frontmatter metadata)
- Supports debugging and migration scenarios

## Architectural Decisions

### Key Principle: Codecs Generate Nodes, Schemas Validate Fields

After extensive design exploration, we've established:

**Schemas do NOT generate nodes** - only validate fields and map relationships:
- `SchemaOperation::CreateEdges` - Maps fields to graph edges (e.g., `parent_connections`)
- `SchemaOperation::StoreAsPayload` - Validates field structure and types
- `SchemaOperation::UseAsIdentity` - Marks field as node BID
- **NO `GenerateChildren` operation** - Node generation is codec responsibility

**Codecs generate nodes** from content/structure:
- `MdCodec` - Generates nodes from markdown headings (content-driven)
- Future `ProcedureCodec` - Generates nodes from `steps` field structure (schema-driven)

**Why**: Attempting to merge three sources of node definitions (metadata, schema, content) creates "merge hell" with ambiguous authority. Clean separation prevents complexity explosion.

### Authority Model

1. **Markdown headings** = PRIMARY STRUCTURE AUTHORITY
   - All headings create nodes (ensures cross-reference tracking)
   - Heading levels (H2, H3) define parent-child via GraphBuilder stack
   - Unmatched headings (not in `sections`) still create nodes with default metadata

2. **`sections` field** = METADATA ENRICHMENT ONLY
   - Flat map: `NodeKey → metadata table`
   - Does NOT define which nodes exist
   - Does NOT define parent-child hierarchy
   - Enriches matched heading nodes with schema, custom fields

3. **Schema** = VALIDATION AND RELATIONSHIP MAPPING
   - Validates `sections` field structure (map format, value types)
   - Maps relationship fields to graph edges (e.g., `parent_connections`)
   - Does NOT generate nodes (codec responsibility)

### Frontmatter Structure

```yaml
---
bid: doc-abc123
schema: Document

# Flat metadata map - NO nesting
sections:
  "bid://intro-abc123":    # Explicit BID (highest priority match)
    schema: Section
    complexity: high
    custom_field: value
  
  "introduction":          # Anchor or title match
    schema: Section
    complexity: medium
---

# My Document

## Introduction {#bid://intro-abc123}
Content here.

## Background {#background}
<!-- Not in sections map - still creates node with default metadata -->

## Goals
<!-- No anchor, matches by title slug - still creates node -->
```

### Schema Definition

```rust
// Document schema - sections is metadata storage only
SchemaDefinition {
    fields: vec![
        SchemaField {
            field_name: "sections".to_string(),
            required: false,
            operation: SchemaOperation::StoreAsPayload {
                validation: Some(FieldValidation::Map {
                    key_format: KeyFormat::NodeKey,  // BID, anchor, or title
                    value_type: ValueType::Table,     // Arbitrary metadata
                })
            },
        },
    ],
}
```

### Matching Strategy

**Priority**: BID URL > Anchor ID > Title slug

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum NodeKey {
    Bid(Bid),           // "bid://doc-123/section-456"
    Anchor(String),     // "introduction" from {#introduction}
    Title(String),      // "introduction" from slugified "## Introduction"
}

// MdCodec struct with fields for sections tracking
pub struct MdCodec {
    current_events: Vec<ProtoNodeWithEvents>,
    content: String,
    matched_sections: HashSet<NodeKey>,  // Track which sections were matched by headings
}

// Parse sections field into flat map
fn parse_sections_metadata(sections: &TomlValue) -> HashMap<NodeKey, TomlTable> {
    // Simple flat extraction - NO recursive nesting
}

// Match heading node to sections metadata
fn find_metadata_match(node: &ProtoBeliefNode, metadata: &HashMap<NodeKey, TomlTable>) 
    -> Option<&TomlTable> 
{
    // Try BID first (most explicit)
    if let Some(bid) = &node.accumulator.id {
        if let Some(meta) = metadata.get(&NodeKey::Bid(bid.clone())) {
            return Some(meta);
        }
    }
    
    // Try anchor from {#anchor} syntax
    if let Some(anchor) = extract_anchor(&node) {
        if let Some(meta) = metadata.get(&NodeKey::Anchor(anchor)) {
            return Some(meta);
        }
    }
    
    // Try title slug (least specific)
    if let Some(title) = node.content.get("title").and_then(|v| v.as_str()) {
        if let Some(meta) = metadata.get(&NodeKey::Title(slugify(title))) {
            return Some(meta);
        }
    }
    
    None
}
```

## Implementation Steps

### Overview: "Look Up" Architecture

Sections metadata enrichment happens in `MdCodec::inject_context()` using a **"look up"** pattern:
- Each heading node looks up to its parent document for sections metadata
- Document node is always processed first (index 0 in `current_events`)
- Heading nodes processed in order (indices 1+ in `current_events`)
- Each heading enriches itself by matching against document's sections map

**Key insights**:
- `current_events` vector structure is `[doc_node, heading1, heading2, ...]`
- Index correspondence: `current_events[0]` = document, `current_events[i]` (i > 0) = i-th heading in document order
- We use **NodeKey matching** (not index-based) because:
  - Authors may add/remove sections entries without changing markdown
  - Not all headings have sections metadata (sparse map)
  - Explicit matching by BID/anchor/title is more robust and maintainable
  - Supports future features (external section references, etc.)

### 1. Add Matched Sections Tracking to MdCodec (0.5 days)

- [ ] Add private field to `MdCodec`: `matched_sections: HashSet<NodeKey>`
- [ ] Initialize to empty `HashSet::new()` in constructor
- [ ] Clear at start of `parse()` method

**Rationale**: 
- Track which section keys were matched by headings during inject_context
- Document node uses this during finalize() to identify unmatched sections
- No caching needed - direct mutable access to `document.get_mut("sections")` table
- `toml_edit::Table` maintains insertion order and supports iteration

### 2. Implement Helper Functions (1 day)

These are **already tested** with comprehensive unit tests in `src/codec/md.rs::tests`:

- [ ] `get_sections_table_mut() -> Option<&mut toml_edit::Table>`
  - Helper to access `current_events[0].document.get_mut("sections")`
  - Returns mutable reference to ordered sections table
  - Called by headings during inject_context for direct access

- [ ] `find_metadata_in_sections(node: &ProtoBeliefNode, sections_table: &toml_edit::Table, ctx: &BeliefContext) -> Option<(NodeKey, &toml_edit::Table)>`
  - Priority matching: BID (from ctx) > Anchor > Title
  - Use `to_anchor()` for title slugification
  - Return matched (key, metadata) tuple or None
  - Key returned for tracking in matched_sections

- [ ] `merge_metadata(proto: &mut ProtoBeliefNode, metadata: &TomlTable)`
  - Merge metadata into proto.document
  - Preserve existing fields (don't overwrite title, text)
  - Handle schema field specially (validates against registry)

- [ ] `merge_metadata_from_table(proto: &mut ProtoBeliefNode, metadata: &toml_edit::Table)`
  - Merge metadata table entries into proto.document
  - Preserve existing fields (don't overwrite title, text, bid)
  - Handle schema field specially (validates against registry)

### 3. Implement "Look Up" in inject_context() (1 day)

Modify `MdCodec::inject_context()` to enrich heading nodes using direct table access:

```rust
fn inject_context(
    &mut self,
    node: &ProtoBeliefNode,
    ctx: &BeliefContext<'_>,
) -> Result<Option<BeliefNode>, BuildonomyError> {
    // NEW: Heading nodes look up to document for sections metadata
    if node.heading > 2 {  // Heading nodes (doc is level 2)
        // Get direct mutable access to document's sections table
        // Note: We access first() immutably for reading, then get mutable access for writing
        let sections_match = if let Some((doc_proto, _)) = self.current_events.first() {
            if let Some(sections_item) = doc_proto.document.get("sections") {
                if let Some(sections_table) = sections_item.as_table() {
                    // Find this heading's metadata in the sections table
                    find_metadata_in_sections(node, sections_table, ctx)
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };
        
        // If we found a match, track it and merge metadata
        if let Some((matched_key, metadata)) = sections_match {
            // Track this section key as matched
            self.matched_sections.insert(matched_key.clone());
            
            // Find this node in current_events for mutation
            let proto_events = self.current_events
                .iter_mut()
                .find(|(proto, _)| proto == node)
                .ok_or(BuildonomyError::Codec("Node not found".to_string()))?;
            
            merge_metadata_from_table(&mut proto_events.0, metadata);
            // Will trigger frontmatter update below
            
            tracing::debug!("Enriched heading '{}' with section metadata (key: {:?})", 
                node.document.get("title").and_then(|v| v.as_str()).unwrap_or("unknown"),
                matched_key);
        } else {
            // No match - heading keeps default metadata (normal for headings without custom metadata)
            tracing::debug!("Heading '{}' has no sections metadata, using defaults", 
                node.document.get("title").and_then(|v| v.as_str()).unwrap_or("unknown"));
        }
    }
    
    // EXISTING: frontmatter update, link resolution, text generation...
}
```

**Implementation notes**:
- [ ] Add matched_sections tracking field to MdCodec struct
- [ ] Clear matched_sections in `parse()` method (start of document)
- [ ] Access document's sections table directly via `current_events[0].document.get("sections")`
- [ ] No caching needed - toml_edit::Table maintains order and supports iteration
- [ ] Match and merge for each heading, tracking matched keys in HashSet
- [ ] Finalize (see next step) processes unmatched sections

### 4. Implement finalize() for Unmatched Sections (0.5 days)

Add `finalize()` implementation to MdCodec (called after all inject_context operations):

```rust
impl DocCodec for MdCodec {
    fn finalize(&mut self) -> Result<Vec<(ProtoBeliefNode, BeliefNode)>, BuildonomyError> {
        let mut modified_nodes = Vec::new();
        
        // Access document's sections table directly
        if let Some((doc_proto, _)) = self.current_events.first_mut() {
            if let Some(sections_item) = doc_proto.document.get_mut("sections") {
                if let Some(sections_table) = sections_item.as_table_mut() {
                    // Collect all keys from sections table
                    let all_keys: Vec<NodeKey> = sections_table.iter()
                        .filter_map(|(key_str, _)| NodeKey::from_str(key_str).ok())
                        .collect();
                    
                    // Find unmatched section keys
                    let unmatched: Vec<_> = all_keys.iter()
                        .filter(|key| !self.matched_sections.contains(key))
                        .collect();
                    
                    // Log info for unmatched sections (heading was removed from markdown)
                    for key in &unmatched {
                        tracing::info!("Section '{}' has no matching heading (will be garbage collected)", key);
                    }
                    
                    // Remove unmatched sections from document (garbage collection)
                    // Unmatched sections mean the heading was removed from markdown
                    if !unmatched.is_empty() {
                        let mut sections_modified = false;
                        
                        for key in &unmatched {
                            let key_str = key.to_string();
                            if sections_table.remove(&key_str).is_some() {
                                sections_modified = true;
                                tracing::info!("Garbage collecting section '{}' (heading removed from markdown)", key_str);
                            }
                        }
                        
                        // If we modified the document, create updated BeliefNode
                        if sections_modified {
                            let updated_node = BeliefNode::try_from(doc_proto.as_ref())?;
                            modified_nodes.push((doc_proto.clone(), updated_node));
                        }
                    }
                }
            }
        }
        
        Ok(modified_nodes)
    }
}
```

**Implementation notes**:
- [ ] Implement finalize() method on MdCodec
- [ ] Calculate unmatched sections (all keys - matched keys)
- [ ] Log info for each unmatched section key (heading was removed from markdown)
- [ ] **Remove unmatched sections** from document (garbage collection)
  - Rationale: Unmatched sections mean heading was deleted, metadata is stale
  - Maintains 1:1 correspondence between sections and markdown headings
  - Ensures clean round-trip with no orphaned metadata
- [ ] Return (proto, node) pair for NodeUpdate event when document modified
- [ ] Builder will emit BeliefEvent::NodeUpdate in Phase 4b

### 5. Testing and Edge Cases (0.5 days)

**Unit tests already written and passing** (see `src/codec/md.rs::tests`):
- [ ] Verify integration tests still pass with new inject_context logic
- [ ] Test matched_sections cleared between documents
- [ ] Test finalize() logs info for unmatched sections
- [ ] Test finalize() returns modified document node if sections changed
- [ ] Test unmatched headings create nodes with defaults
- [ ] Test duplicate keys (first match wins)
- [ ] Test missing `sections` field (graceful handling)

### 6. Logging and Diagnostics (incorporated into previous steps)

- [x] Log info for sections entries without matching headings (in finalize())
- [x] Log debug for successful matches (in inject_context())
- [x] Track unmatched section keys via matched_sections field
- [ ] Add diagnostic for invalid NodeKey formats during parse_sections_metadata()

## Testing Requirements

- Parse frontmatter with/without `sections` field
- Match by BID, anchor, title (priority order)
- Enrich matched nodes with schema and custom fields
- Unmatched sections logged as info (not errors)
- Unmatched headings create nodes with default metadata
- Duplicate keys use first match, log ambiguity
- Round-trip: matched sections preserved, unmatched sections removed (garbage collected)

## Success Criteria

- [ ] All markdown headings create nodes (cross-reference tracking works)
- [ ] Sections metadata enriches matched nodes
- [ ] Priority matching: BID > Anchor > Title
- [ ] Unmatched sections: garbage collected (heading was removed from markdown)
- [ ] Unmatched headings: nodes created with defaults
- [ ] Schema validates sections field structure
- [ ] Clean round-trip: sections maintains 1:1 mapping with headings
- [ ] Backward compatible with existing documents
- [ ] Tests pass

## Edge Cases

**Case 1: Section without matching heading (heading was removed)**
```yaml
sections:
  "conclusion": { complexity: high }
```
```markdown
## Introduction
## Goals
<!-- "Conclusion" heading was deleted -->
```
**Behavior**: During `finalize()`, unmatched section is detected, logged as info, and removed from sections table (garbage collected). Document node emits NodeUpdate event. Clean round-trip maintains 1:1 correspondence between sections and headings.

**Case 2: Heading without section metadata**
```yaml
sections:
  "intro": {}
```
```markdown
## Introduction {#intro}
## Background {#background}
<!-- Not in sections -->
```
**Behavior**: Both headings create nodes. `background` gets default metadata. No warning.

**Case 3: Duplicate section keys**
```yaml
sections:
  "intro": { complexity: high }
  "introduction": { complexity: low }
```
**Behavior**: Matches by priority. If heading has `{#intro}`, uses first. If matches by title, uses first encountered. Log info about ambiguity.

## Risks

**Risk**: Matching ambiguity (duplicate titles without anchors)  
**Mitigation**: Encourage BID URLs or anchors for explicit matching. Log info on ambiguous matches.

**Risk**: Authors expect sections to create nodes  
**Mitigation**: Documentation clearly states markdown defines structure. Section without heading = info log.

**Risk**: Performance with many sections  
**Mitigation**: HashMap lookup is O(1). Acceptable for <1000 sections per document.

## Out of Scope (Future Issues)

**Deferred: Procedural node generation**
- Generating nodes from `steps` field (hierarchical procedures)
- Requires specialized codec (`.procedure` extension)
- Complex: nesting, execution order, type system
- Separate issue to avoid "merge hell"

**Deferred: Nested sections**
- `sections.intro.sections.background` hierarchy
- Conflicts with markdown heading structure
- Use flat metadata for Issue 02, revisit if needed

**Deferred: Schema-driven node generation**
- `SchemaOperation::GenerateChildren` concept rejected
- Node generation is codec responsibility, not schema
- Schemas validate and map relationships only

## References

- `codec/md.rs` - MdCodec implementation
- `codec/schema_registry.rs` - Schema validation
- `codec/builder.rs` - GraphBuilder with heading-based stack
- `.scratchpad/schema_operations_design.md` - Design exploration

## Design Rationale

This narrowed scope avoids "merge hell" by establishing clear authority:
- **Markdown** defines which nodes exist (structure)
- **Sections** enriches nodes with metadata (fields)
- **Schema** validates and maps relationships (edges)

No conflicts, no ambiguity, predictable behavior. Complex node generation (procedures) deferred to specialized codecs that don't conflict with markdown structure.
