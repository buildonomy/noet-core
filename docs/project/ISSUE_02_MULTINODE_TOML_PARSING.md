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
5. Maintain clean authority model: markdown = structure, sections = metadata

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

1. **Parse Sections as Flat Metadata Map** (1 day)
   - [ ] Parse `sections` field from frontmatter
   - [ ] Extract as `HashMap<NodeKey, TomlTable>`
   - [ ] Validate keys are valid NodeKey formats
   - [ ] Handle missing `sections` field gracefully

2. **Generate Nodes from Markdown Headings** (0.5 days)
   - [ ] Parse ALL markdown headings to nodes (existing behavior)
   - [ ] Set `heading` field for parent-child stack management
   - [ ] Extract anchors from `{#anchor}` syntax
   - [ ] Generate title slugs for matching

3. **Match and Enrich Nodes** (1 day)
   - [ ] Implement `find_metadata_match()` with priority matching
   - [ ] Merge metadata into matched nodes
   - [ ] Preserve existing node fields (don't overwrite `title`, `text`)
   - [ ] Log info for unmatched sections (not warnings)

4. **Testing and Edge Cases** (0.5 days)
   - [ ] Test BID, anchor, title matching
   - [ ] Test unmatched sections (info log)
   - [ ] Test unmatched headings (creates node anyway)
   - [ ] Test duplicate keys (first match wins)
   - [ ] Test missing `sections` field

## Testing Requirements

- Parse frontmatter with/without `sections` field
- Match by BID, anchor, title (priority order)
- Enrich matched nodes with schema and custom fields
- Unmatched sections logged as info (not errors)
- Unmatched headings create nodes with default metadata
- Duplicate keys use first match, log ambiguity
- Round-trip preserves sections + discovered headings

## Success Criteria

- [ ] All markdown headings create nodes (cross-reference tracking works)
- [ ] Sections metadata enriches matched nodes
- [ ] Priority matching: BID > Anchor > Title
- [ ] Unmatched sections: info log (expected for references)
- [ ] Unmatched headings: nodes created with defaults
- [ ] Schema validates sections field structure
- [ ] Backward compatible with existing documents
- [ ] Tests pass

## Edge Cases

**Case 1: Section without matching heading**
```yaml
sections:
  "conclusion": { complexity: high }
```
```markdown
## Introduction
## Goals
<!-- No "conclusion" heading -->
```
**Behavior**: Info log "Section 'conclusion' has no matching heading". Metadata ignored (acceptable for external references).

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