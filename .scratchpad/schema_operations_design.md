# SCRATCHPAD - Schema Operations Design for Recursive Sections

**Status**: ✅ RESOLVED - Narrow scope to metadata enrichment only
**Date**: 2025-01-27
**Context**: Issue 02 - Multi-node TOML parsing with recursive sections

---

## TL;DR - Final Architecture Decision

**Problem**: Attempting to merge node definitions from metadata (sections field), schema (GenerateChildren), and content (markdown headings) creates "merge hell" with ambiguous authority.

**Solution**: **Codecs generate nodes. Schemas validate and map relationships.**

### Schema Operations (Final)
1. `CreateEdges` - Maps fields to graph edges (Builder enforces)
2. `StoreAsPayload` - Validates field structure/types
3. `UseAsIdentity` - Marks field as node BID
4. ❌ **NO GenerateChildren** - This is codec responsibility

### Issue 02 Implementation
- `sections` field = **metadata only** (StoreAsPayload)
- MdCodec generates **all nodes from

## Executive Summary

### Key Architectural Decisions

1. **Schema Operations Model**: Expand `SchemaDefinition` from graph-edges-only to full builder operations
   - `GenerateChildren`: Field generates child ProtoBeliefNodes (e.g., `sections`)
   - `CreateEdges`: Field creates graph edges (existing `parent_connections` behavior)
   - `StoreAsPayload`: Field stored as data (most scalar fields)
   - `UseAsIdentity`: Field used as node BID

2. **Recursive Sections**: The `sections` field applies to Document AND Section schemas
   - Enables nested hierarchies: Document → Section → SubSection → ...
   - Used by procedures for nested procedure/step definitions
   - Tree structure preserved via `heading` field levels

3. **Authority Model**: `sections` field has PRIMARY WRITE AUTHORITY
   - Frontmatter `sections` tree defines which nodes exist and their metadata
   - Markdown headings provide CONTENT INJECTION ONLY (`title`, `text` fields)
   - Unmatched markdown headings are IGNORED (not in sections tree = don't exist)
   - Sections without markdown are VALID (procedure references, external nodes)

4. **Parent-Child via Heading Levels**: No explicit `parent_id` field needed
   - MdCodec sets `heading` field when generating nodes from sections tree
   - GraphBuilder uses heading levels + stack for parent-child relationships
   - Children: `heading = parent.heading + 1`
   - Peers: same heading level

5. **Synchronization Direction**: One-way (sections → markdown) for Issue 02
   - Parse: Generate nodes from sections, inject markdown content
   - Future: MdCodec::generate_source() must reverse (rebuild sections from nodes)

### Implementation Flow

```
Frontmatter sections tree
    ↓ generate_nodes_from_sections()
ProtoBeliefNode stream (with heading levels set)
    ↓ parse_markdown_headings_for_content()
Markdown content map (NodeKey → title/text)
    ↓ inject_markdown_content()
Enriched ProtoBeliefNode stream
    ↓ GraphBuilder::push()
Belief graph (with WeightKind::Section edges via heading-based stack)
```

## Problem Statement

Current `SchemaDefinition` only models graph edges via `GraphField`. Need to expand to:
1. Support recursive `sections` field that generates child ProtoBeliefNodes
2. Enable MdCodec to inject markdown content into section-defined nodes
3. Make schema → builder operations explicit

**Key Insights**: 
- `sections` field applies to BeliefKind::Document AND to section nodes themselves (recursive)
- `sections` field has PRIMARY WRITE AUTHORITY - defines node structure
- Markdown has PRIMARY CONTENT AUTHORITY - provides text/title for matched nodes
- Parent-child relationships encoded via `heading` field (heading levels determine nesting)

## Core Architecture

### Schema Operations Enum

**FINAL DESIGN**: Schemas do NOT generate nodes, only validate and map relationships.

```rust
#[derive(Debug, Clone, PartialEq)]
pub enum SchemaOperation {
    /// Field creates graph edges to existing nodes
    /// Used for: "parent_connections" in intention_lattice.intention
    /// Builder reads this during push to create edges
    CreateEdges {
        direction: EdgeDirection,
        weight_kind: WeightKind,
        payload_fields: Vec<&'static str>,
    },
    
    /// Field stored as payload (no special processing)
    /// Used for: most fields like "title", "text", "sections", "complexity"
    /// Can include validation rules
    StoreAsPayload {
        /// Optional: validation rules for the field
        validation: Option<FieldValidation>,
    },
    
    /// Field used as node identity
    /// Used for: "bid" field
    UseAsIdentity,
}

// NOTE: NO GenerateChildren operation!
// Node generation is codec responsibility, not schema responsibility.
// Codecs (MdCodec, ProcedureCodec) generate nodes from content/structure.
// Schemas validate fields and map relationships only.

#[derive(Debug, Clone, PartialEq)]
pub enum FieldValidation {
    String { max_length: Option<usize> },
    Number { min: Option<f64>, max: Option<f64> },
    Enum { allowed_values: Vec<String> },
}

#[derive(Debug, Clone)]
pub struct SchemaField {
    pub field_name: String,  // Changed from &'static str for flexibility
    pub required: bool,
    pub operation: SchemaOperation,
}

#[derive(Debug, Clone)]
pub struct SchemaDefinition {
    pub fields: Vec<SchemaField>,
}
```

### Built-in Schema Definitions

```rust
// Document schema (applies to BeliefKind::Document)
// Used by MdCodec - nodes generated from markdown headings
SchemaDefinition {
    fields: vec![
        SchemaField {
            field_name: "bid".to_string(),
            required: false,
            operation: SchemaOperation::UseAsIdentity,
        },
        SchemaField {
            field_name: "schema".to_string(),
            required: false,
            operation: SchemaOperation::StoreAsPayload { validation: None },
        },
        SchemaField {
            field_name: "sections".to_string(),
            required: false,
            // METADATA ONLY - does not generate nodes
            operation: SchemaOperation::StoreAsPayload { 
                validation: Some(FieldValidation::Nested {
                    // Map of heading identifiers to metadata
                    key_format: KeyFormat::NodeKey,  // BID, anchor, or title
                    value_type: ValueType::Table,
                })
            },
        },
    ],
}

// intention_lattice.intention schema (existing, working correctly)
SchemaDefinition {
    fields: vec![
        SchemaField {
            field_name: "parent_connections".to_string(),
            required: false,
            operation: SchemaOperation::CreateEdges {
                direction: EdgeDirection::Downstream,
                weight_kind: WeightKind::Pragmatic,
                payload_fields: vec!["relationship_semantics", "motivation_kinds", "notes"],
            },
        },
    ],
}

// Future: Procedure schema for .procedure files
// Would be used by ProcedureCodec (not MdCodec)
// ProcedureCodec generates nodes from `steps` field structure
// Schema validates step structure, maps relationships
```

## Recursive Sections: The Matching Problem

### Example Document

```yaml
---
bid: doc-abc
schema: Document
sections:
  "intro":
    schema: Section
    complexity: high
    sections:
      "background":
        schema: Section
        complexity: medium
      "motivation":
        schema: Section
        complexity: low
  "goals":
    schema: Section
    complexity: medium
---

# My Document

## Introduction {#intro}
Some intro text.

### Background {#background}
Background details.

### Motivation {#motivation}
Why we're doing this.

## Goals {#goals}
What we want to achieve.
```

### Tree Structures

**Frontmatter Tree** (metadata):
```
Document (doc-abc)
└─ sections:
   ├─ "intro" → { complexity: high, sections: {...} }
   │  └─ sections:
   │     ├─ "background" → { complexity: medium }
   │     └─ "motivation" → { complexity: low }
   └─ "goals" → { complexity: medium }
```

**Markdown Tree** (structure):
```
Document
├─ H2: Introduction (#intro)
│  ├─ H3: Background (#background)
│  └─ H3: Motivation (#motivation)
└─ H2: Goals (#goals)
```

**Belief Graph** (should mirror markdown structure):
```
doc-abc (Document)
├─ [Section edge] → intro (Section, complexity: high)
│  ├─ [Section edge] → background (Section, complexity: medium)
│  └─ [Section edge] → motivation (Section, complexity: low)
└─ [Section edge] → goals (Section, complexity: medium)
```

## MdCodec Implementation Strategy

### Step 1: Flatten Frontmatter Metadata Tree

```rust
/// NodeKey can be BID, anchor, or title slug
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum NodeKey {
    Bid(Bid),
    Anchor(String),   // From {#anchor}
    Title(String),    // Slugified title
}

#[derive(Debug, Clone)]
pub struct SectionMetadata {
    pub bid: Option<Bid>,
    pub schema: Option<String>,
    pub content: TomlTable,  // All other fields
}

fn flatten_sections_tree(
    sections_value: &TomlValue
) -> Result<HashMap<NodeKey, SectionMetadata>> {
    let mut flat = HashMap::new();
    
    fn recurse(
        key: &str,
        value: &TomlValue,
        flat: &mut HashMap<NodeKey, SectionMetadata>
    ) -> Result<()> {
        let mut table = value.as_table()
            .ok_or("Section value must be table")?
            .clone();
        
        // Extract nested sections before storing
        let nested_sections = table.remove("sections");
        
        // Parse metadata
        let metadata = SectionMetadata {
            bid: table.get("bid").and_then(|v| parse_bid(v).ok()),
            schema: table.get("schema")
                .and_then(|v| v.as_str())
                .map(String::from),
            content: table,
        };
        
        // Determine NodeKey from key string
        let node_key = if key.starts_with("bid://") {
            NodeKey::Bid(Bid::from_str(key)?)
        } else if is_anchor_format(key) {
            NodeKey::Anchor(key.to_string())
        } else {
            NodeKey::Title(key.to_string())
        };
        
        flat.insert(node_key, metadata);
        
        // Recurse into nested sections
        if let Some(TomlValue::Table(nested_table)) = nested_sections {
            for (nested_key, nested_value) in nested_table {
                recurse(&nested_key, &nested_value, flat)?;
            }
        }
        
        Ok(())
    }
    
    let sections_table = sections_value.as_table()
        .ok_or("sections must be table")?;
    
    for (key, value) in sections_table {
        recurse(key, value, &mut flat)?;
    }
    
    Ok(flat)
}
```

### Step 2: Build Heading Tree from Markdown

```rust
#[derive(Debug)]
struct HeadingNode {
    proto: ProtoBeliefNode,
    level: u32,  // 1 for H1, 2 for H2, etc.
    children: Vec<HeadingNode>,
}

fn build_heading_tree(
    flat_headings: Vec<(ProtoBeliefNode, u32)>  // (node, heading_level)
) -> Vec<HeadingNode> {
    let mut root_nodes = Vec::new();
    let mut stack: Vec<HeadingNode> = Vec::new();
    
    for (proto, level) in flat_headings {
        let node = HeadingNode {
            proto,
            level,
            children: Vec::new(),
        };
        
        // Pop stack until we find the parent level
        while let Some(top) = stack.last() {
            if top.level < level {
                break;  // Found parent
            }
            // Current level <= top level, pop and add to its parent
            let finished = stack.pop().unwrap();
            if let Some(parent) = stack.last_mut() {
                parent.children.push(finished);
            } else {
                root_nodes.push(finished);
            }
        }
        
        // Push current node
        stack.push(node);
    }
    
    // Drain remaining stack
    while let Some(node) = stack.pop() {
        if let Some(parent) = stack.last_mut() {
            parent.children.push(node);
        } else {
            root_nodes.push(node);
        }
    }
    
    root_nodes
}
```

### Step 3: Match and Merge

```rust
fn match_sections(
    heading_tree: Vec<HeadingNode>,
    flat_metadata: HashMap<NodeKey, SectionMetadata>
) -> Vec<ProtoBeliefNode> {
    let mut result = Vec::new();
    
    fn find_metadata<'a>(
        proto: &ProtoBeliefNode,
        metadata_map: &'a HashMap<NodeKey, SectionMetadata>
    ) -> Option<&'a SectionMetadata> {
        // Try BID first
        if let Some(bid) = &proto.accumulator.id {
            if let Some(meta) = metadata_map.get(&NodeKey::Bid(bid.clone())) {
                return Some(meta);
            }
        }
        
        // Try anchor from proto.path (e.g., "file.md#intro")
        if let Some(anchor) = extract_anchor(&proto.path) {
            if let Some(meta) = metadata_map.get(&NodeKey::Anchor(anchor)) {
                return Some(meta);
            }
        }
        
        // Try title slug
        if let Some(title) = proto.content.get("title").and_then(|v| v.as_str()) {
            let slug = slugify(title);
            if let Some(meta) = metadata_map.get(&NodeKey::Title(slug)) {
                return Some(meta);
            }
        }
        
        None
    }
    
    fn recurse(
        node: HeadingNode,
        metadata_map: &HashMap<NodeKey, SectionMetadata>,
        result: &mut Vec<ProtoBeliefNode>
    ) {
        let mut proto = node.proto;
        
        // Match and merge metadata
        if let Some(metadata) = find_metadata(&proto, metadata_map) {
            // Merge schema if present
            if let Some(schema) = &metadata.schema {
                proto.document.schema = schema.clone();
            }
            
            // Merge content fields (but don't overwrite text/title from markdown)
            for (key, value) in &metadata.content {
                if key != "text" && key != "title" {
                    proto.content.insert(key.clone(), value.clone());
                }
            }
            
            tracing::debug!(
                "Matched section: {} → schema={:?}",
                proto.accumulator.id.as_ref().map(|b| b.to_string()).unwrap_or_default(),
                metadata.schema
            );
        } else {
            tracing::warn!(
                "Unmatched section in markdown: {:?}",
                proto.accumulator.id
            );
        }
        
        result.push(proto);
        
        // Recurse into children
        for child in node.children {
            recurse(child, metadata_map, result);
        }
    }
    
    for root in heading_tree {
        recurse(root, &flat_metadata, &mut result);
    }
    
    result
}
```

### Step 4: Full MdCodec::parse Implementation

```rust
impl DocCodec for MdCodec {
    fn parse(&mut self, content: &str) -> Result<Vec<ProtoBeliefNode>> {
        // 1. Parse frontmatter
        let frontmatter_end = find_frontmatter_end(content)?;
        let (frontmatter_str, markdown_content) = if let Some(end) = frontmatter_end {
            content.split_at(end)
        } else {
            ("", content)
        };
        
        let mut doc_node = if !frontmatter_str.is_empty() {
            ProtoBeliefNode::from_str(frontmatter_str)?
        } else {
            ProtoBeliefNode::new(BeliefKind::Document)
        };
        
        // 2. Get schema definition
        let schema = SCHEMAS.get(&doc_node.document.schema)
            .ok_or("Unknown schema")?;
        
        // 3. Check if schema has sections field with GenerateChildren
        let sections_field = schema.fields.iter()
            .find(|f| f.field_name == "sections" 
                && matches!(f.operation, SchemaOperation::GenerateChildren {..}));
        
        let section_nodes = if let Some(_sections_field) = sections_field {
            // 4. Extract and flatten sections from frontmatter
            let flat_metadata = if let Some(sections_value) = doc_node.content.get("sections") {
                flatten_sections_tree(sections_value)?
            } else {
                HashMap::new()
            };
            
            // 5. Parse markdown headings
            let parser = Parser::new_ext(markdown_content, buildonomy_md_options());
            let mut flat_headings = Vec::new();
            let mut current_heading: Option<(ProtoBeliefNode, u32)> = None;
            
            for event in parser {
                match event {
                    Event::Start(Tag::Heading { level, .. }) => {
                        let proto = ProtoBeliefNode::new(BeliefKind::Document);
                        current_heading = Some((proto, level as u32));
                    }
                    Event::Text(text) if current_heading.is_some() => {
                        let (proto, level) = current_heading.as_mut().unwrap();
                        proto.content.insert("title".to_string(), TomlValue::String(text.to_string()));
                    }
                    Event::End(TagEnd::Heading(_)) if current_heading.is_some() => {
                        flat_headings.push(current_heading.take().unwrap());
                    }
                    _ => {}
                }
            }
            
            // 6. Build heading tree from flat list
            let heading_tree = build_heading_tree(flat_headings);
            
            // 7. Match and merge
            match_sections(heading_tree, flat_metadata)
        } else {
            Vec::new()
        };
        
        // 8. Return document + section nodes
        Ok(vec![doc_node].into_iter().chain(section_nodes).collect())
    }
}
```

## Synchronization Strategy: Sections Have Write Authority

**CRITICAL PRINCIPLE**: The `sections` field defines the authoritative node structure. Markdown provides content injection into those nodes.

### Authority Model

1. **`sections` field** = PRIMARY WRITE AUTHORITY
   - Defines which nodes exist and their structure
   - Defines node metadata (schema, custom fields like `complexity`)
   - Defines parent-child hierarchy (nesting in sections tree)
   - Can reference external nodes (BID URLs to other docs)
   - Can define internal nodes WITHOUT markdown headings (e.g., procedure steps)

2. **Markdown headings** = CONTENT INJECTION ONLY
   - Provides `title` and `text` fields for matched nodes
   - Does NOT define node structure or hierarchy
   - Heading levels (H2, H3) used by GraphBuilder to set `heading` field for stack-based parenting

3. **Synchronization Direction**: `sections` → markdown (one-way)
   - MdCodec generates ProtoBeliefNodes from `sections` tree
   - If markdown heading matches (by BID/anchor/title), inject its `title` and `text`
   - If no match, section node still exists (just missing content fields)
   - Unmatched markdown headings are IGNORED (not part of sections tree)

### Implementation Strategy

```rust
impl DocCodec for MdCodec {
    fn parse(&mut self, content: &str) -> Result<Vec<ProtoBeliefNode>> {
        // 1. Parse frontmatter
        let doc_node = parse_frontmatter(content)?;
        
        // 2. Generate ProtoBeliefNode tree from sections field
        let section_nodes = if let Some(sections_value) = doc_node.content.get("sections") {
            generate_nodes_from_sections(sections_value, &doc_node)?
        } else {
            Vec::new()
        };
        
        // 3. Parse markdown headings to extract content
        let markdown_content_map = parse_markdown_headings_for_content(content)?;
        
        // 4. Inject markdown content into matching section nodes
        let enriched_sections = inject_markdown_content(section_nodes, markdown_content_map)?;
        
        // 5. Return document + section nodes
        Ok(vec![doc_node].into_iter().chain(enriched_sections).collect())
    }
}
```

### Case 1: Section with Matching Markdown Heading

```yaml
sections:
  "intro":
    complexity: high
    sections:
      "background":
        complexity: medium
```

```markdown
## Introduction {#intro}
This is intro text.

### Background {#background}
Background details.
```

**Result**: 
- Two ProtoBeliefNodes generated from `sections` tree
- `intro` node gets: `complexity: high` (from sections) + `title: "Introduction"` + `text: "This is intro text."` (from markdown)
- `background` node gets: `complexity: medium` + `title: "Background"` + `text: "Background details."`
- Parent-child relationship: `background.heading = intro.heading + 1` (set by GraphBuilder based on nesting in sections tree)

### Case 2: Section WITHOUT Matching Markdown Heading

```yaml
sections:
  "intro": {}
  "step_mix_batter":
    reference: "bid://procedures/baking#mix_batter"
```

```markdown
## Introduction {#intro}
Intro text here.
```

**Result**:
- Two ProtoBeliefNodes generated from `sections` tree
- `intro` node gets markdown content
- `step_mix_batter` node exists with reference, NO `title` or `text` fields (that's fine - it's a reference)
- No warning - this is valid for procedures that reference external steps

### Case 3: Markdown Heading WITHOUT Section Entry

```yaml
sections:
  "intro": {}
```

```markdown
## Introduction {#intro}
## Goals {#goals}  <!-- Not in sections! -->
```

**Result**:
- ONE ProtoBeliefNode generated (only `intro`)
- `goals` heading is IGNORED - not in authoritative `sections` tree
- No node created for unmatched markdown

**Rationale**: 
- Prevents markdown from accidentally creating nodes
- Authors must explicitly add to `sections` to create nodes
- Clear separation: structure (sections) vs. documentation (markdown)

### Case 4: Duplicate Section Keys

```yaml
sections:
  "intro": { complexity: high }
  "introduction": { complexity: low }
```

```markdown
## Introduction {#intro}
```

**Result**:
- Two ProtoBeliefNodes generated (both `intro` and `introduction`)
- Markdown matches `intro` by anchor (priority: BID > anchor > title)
- `introduction` node has no markdown content
- Log info message (not warning): "Section 'introduction' has no matching markdown heading"

### Case 5: Nested Sections in Frontmatter

```yaml
sections:
  "intro":
    sections:
      "background": {}
      "motivation": {}
```

**Result**: 
- THREE ProtoBeliefNodes generated:
  1. `intro` (parent)
  2. `background` (child of intro)
  3. `motivation` (child of intro)
- Tree structure from sections nesting determines parent relationships
- GraphBuilder uses `heading` field (set during generation) for stack-based edge creation

## Generating Nodes from Sections Tree

Key implementation: `generate_nodes_from_sections()` recursively walks the sections tree.

```rust
fn generate_nodes_from_sections(
    sections_value: &TomlValue,
    parent: &ProtoBeliefNode,
) -> Result<Vec<ProtoBeliefNode>> {
    let mut nodes = Vec::new();
    
    fn recurse(
        key: &str,
        value: &TomlValue,
        parent_heading: u32,
        nodes: &mut Vec<ProtoBeliefNode>,
    ) -> Result<()> {
        let mut table = value.as_table()
            .ok_or("Section value must be table")?
            .clone();
        
        // Extract nested sections before creating node
        let nested_sections = table.remove("sections");
        
        // Create ProtoBeliefNode for this section
        let mut proto = ProtoBeliefNode::new(BeliefKind::Document);
        
        // Set heading level (parent + 1)
        proto.heading = Some(parent_heading + 1);
        
        // Set BID if key is BID format
        if key.starts_with("bid://") {
            proto.accumulator.id = Some(Bid::from_str(key)?);
        }
        
        // Extract schema if present
        if let Some(schema_value) = table.remove("schema") {
            if let Some(schema_str) = schema_value.as_str() {
                proto.document.schema = schema_str.to_string();
            }
        }
        
        // Store remaining fields in content
        proto.content = table;
        
        nodes.push(proto);
        
        // Recurse into nested sections
        if let Some(TomlValue::Table(nested_table)) = nested_sections {
            for (nested_key, nested_value) in nested_table {
                recurse(&nested_key, &nested_value, parent_heading + 1, nodes)?;
            }
        }
        
        Ok(())
    }
    
    let sections_table = sections_value.as_table()
        .ok_or("sections must be table")?;
    
    // Parent heading level (document = 1, first section = 2)
    let parent_heading = parent.heading.unwrap_or(1);
    
    for (key, value) in sections_table {
        recurse(key, value, parent_heading, &mut nodes)?;
    }
    
    Ok(nodes)
}
```

**Key Points**:
- Sets `heading` field based on nesting depth in sections tree
- Preserves sections tree hierarchy via heading levels
- GraphBuilder uses heading levels to maintain stack and create parent edges
- Does NOT set `title` or `text` (that comes from markdown injection)

## Injecting Markdown Content

```rust
#[derive(Debug)]
struct MarkdownHeading {
    anchor: Option<String>,  // From {#anchor}
    title: String,
    text: String,  // Content until next heading
    line_range: (usize, usize),
}

fn parse_markdown_headings_for_content(
    content: &str
) -> Result<HashMap<NodeKey, MarkdownHeading>> {
    let mut map = HashMap::new();
    let parser = Parser::new_ext(content, buildonomy_md_options());
    
    let mut current_heading: Option<MarkdownHeading> = None;
    let mut current_text = String::new();
    
    for event in parser {
        match event {
            Event::Start(Tag::Heading { id, .. }) => {
                // Save previous heading
                if let Some(heading) = current_heading.take() {
                    let key = if let Some(ref anchor) = heading.anchor {
                        NodeKey::Anchor(anchor.clone())
                    } else {
                        NodeKey::Title(slugify(&heading.title))
                    };
                    map.insert(key, heading);
                }
                
                // Start new heading
                current_heading = Some(MarkdownHeading {
                    anchor: id.map(|s| s.to_string()),
                    title: String::new(),
                    text: String::new(),
                    line_range: (0, 0),
                });
                current_text.clear();
            }
            Event::Text(text) if current_heading.is_some() => {
                let heading = current_heading.as_mut().unwrap();
                if heading.title.is_empty() {
                    heading.title = text.to_string();
                } else {
                    current_text.push_str(&text);
                }
            }
            Event::End(TagEnd::Heading(_)) => {
                if let Some(ref mut heading) = current_heading {
                    heading.text = current_text.trim().to_string();
                    current_text.clear();
                }
            }
            _ => {}
        }
    }
    
    // Save last heading
    if let Some(heading) = current_heading {
        let key = if let Some(ref anchor) = heading.anchor {
            NodeKey::Anchor(anchor.clone())
        } else {
            NodeKey::Title(slugify(&heading.title))
        };
        map.insert(key, heading);
    }
    
    Ok(map)
}

fn inject_markdown_content(
    section_nodes: Vec<ProtoBeliefNode>,
    markdown_map: HashMap<NodeKey, MarkdownHeading>
) -> Result<Vec<ProtoBeliefNode>> {
    section_nodes.into_iter().map(|mut node| {
        // Try to find matching markdown content
        let markdown = find_markdown_match(&node, &markdown_map);
        
        if let Some(heading) = markdown {
            // Inject title and text (don't overwrite existing fields)
            if !node.content.contains_key("title") {
                node.content.insert(
                    "title".to_string(), 
                    TomlValue::String(heading.title.clone())
                );
            }
            if !node.content.contains_key("text") {
                node.content.insert(
                    "text".to_string(), 
                    TomlValue::String(heading.text.clone())
                );
            }
            
            tracing::debug!(
                "Injected markdown content into section: {:?}",
                node.accumulator.id
            );
        } else {
            tracing::info!(
                "Section has no matching markdown heading: {:?}",
                node.accumulator.id
            );
        }
        
        Ok(node)
    }).collect()
}

fn find_markdown_match<'a>(
    node: &ProtoBeliefNode,
    markdown_map: &'a HashMap<NodeKey, MarkdownHeading>
) -> Option<&'a MarkdownHeading> {
    // Priority: BID > Anchor > Title
    
    // Try BID
    if let Some(bid) = &node.accumulator.id {
        if let Some(heading) = markdown_map.get(&NodeKey::Bid(bid.clone())) {
            return Some(heading);
        }
    }
    
    // Try anchor (extract from node.path or node.content)
    if let Some(anchor) = extract_anchor_from_node(node) {
        if let Some(heading) = markdown_map.get(&NodeKey::Anchor(anchor)) {
            return Some(heading);
        }
    }
    
    // Try title slug (if node already has title from sections metadata)
    if let Some(title_value) = node.content.get("title") {
        if let Some(title) = title_value.as_str() {
            let slug = slugify(title);
            if let Some(heading) = markdown_map.get(&NodeKey::Title(slug)) {
                return Some(heading);
            }
        }
    }
    
    None
}
```

## GraphBuilder Integration

GraphBuilder already handles heading-based parenting via its stack:

```rust
// In GraphBuilder::push (existing code)
async fn push(&mut self, proto: ProtoBeliefNode) -> Result<NodeSource> {
    // ...
    
    // Stack management based on heading levels
    if let Some(heading) = proto.heading {
        // Pop stack until we find parent with lower heading level
        while let Some(top) = self.stack.last() {
            if top.heading < heading {
                break;  // Found parent
            }
            self.stack.pop();
        }
        
        // Parent is now top of stack (or document root if stack empty)
        let parent = self.get_parent_from_stack()?;
        
        // Create edge with WeightKind::Section
        // (GraphBuilder already knows to use Section weight for heading-based hierarchy)
    }
    
    // ...
}
```

**No changes needed to GraphBuilder** - it already uses heading levels to maintain parent-child relationships via stack. MdCodec just needs to set the `heading` field correctly when generating nodes from sections tree.

## Schema Registry Updates

```rust
impl SchemaRegistry {
    pub fn create() -> Self {
        let registry = SchemaRegistry(Arc::new(RwLock::new(HashMap::new())));

        // Register Document schema
        registry.register(
            "Document".to_string(),
            SchemaDefinition {
                fields: vec![
                    SchemaField {
                        field_name: "bid".to_string(),
                        required: false,
                        operation: SchemaOperation::UseAsIdentity,
                    },
                    SchemaField {
                        field_name: "sections".to_string(),
                        required: false,
                        operation: SchemaOperation::GenerateChildren {
                            child_schema: "Section".to_string(),
                            recursive: true,
                            edge_weight: WeightKind::Section,
                        },
                    },
                ],
            },
        );

        // Register Section schema
        registry.register(
            "Section".to_string(),
            SchemaDefinition {
                fields: vec![
                    SchemaField {
                        field_name: "bid".to_string(),
                        required: false,
                        operation: SchemaOperation::UseAsIdentity,
                    },
                    SchemaField {
                        field_name: "title".to_string(),
                        required: false,
                        operation: SchemaOperation::StoreAsPayload { validation: None },
                    },
                    SchemaField {
                        field_name: "text".to_string(),
                        required: false,
                        operation: SchemaOperation::StoreAsPayload { validation: None },
                    },
                    SchemaField {
                        field_name: "sections".to_string(),  // RECURSIVE
                        required: false,
                        operation: SchemaOperation::GenerateChildren {
                            child_schema: "Section".to_string(),
                            recursive: true,
                            edge_weight: WeightKind::Section,
                        },
                    },
                ],
            },
        );

        // Existing intention schema
        registry.register(
            "intention_lattice.intention".to_string(),
            SchemaDefinition {
                fields: vec![
                    SchemaField {
                        field_name: "parent_connections".to_string(),
                        required: false,
                        operation: SchemaOperation::CreateEdges {
                            direction: EdgeDirection::Downstream,
                            weight_kind: WeightKind::Pragmatic,
                            payload_fields: vec!["relationship_semantics", "motivation_kinds", "notes"],
                        },
                    },
                ],
            },
        );

        registry
    }
}
```

## Open Questions

1. **Should ProtoBeliefNode have explicit `parent_relationship` field?**
   - **RESOLVED: No** - GraphBuilder uses `heading` field for stack-based parenting
   - ProtoBeliefNode.heading controls parent-child relationships
   - Children have heading = parent.heading + 1
   - GraphBuilder pops stack until finding parent with lower heading level

2. **How to handle schema validation of nested sections?**
   - Option A: Validate during generate_nodes_from_sections()
   - Option B: Validate during GraphBuilder::push()
   - **Recommendation**: Option B (defer to builder where all context is available)

3. **Should we support non-Section schemas for child nodes?**
   - Example: Document with "chapters" field generating Chapter schema nodes
   - **Answer**: Yes, `GenerateChildren.child_schema` is configurable
   - "Section" is just the default for markdown documents

4. **Performance with deeply nested sections?**
   - Generate operation is O(n) where n = total sections (recursive tree walk)
   - Match operation is O(m * log n) where m = section nodes, n = markdown headings (HashMap lookup)
   - Should be fine for documents with <1000 sections

5. **How to synchronize sections back to frontmatter when markdown changes?**
   - Out of scope for Issue 02 (read-only parsing)
   - Future: MdCodec::generate_source() must rebuild sections tree from ProtoBeliefNode stream
   - Challenge: Preserve section metadata while updating from markdown content

## CRITICAL UNRESOLVED DESIGN QUESTIONS

### Authority Conflict: Sections vs. Markdown Headings

**THE PICKLE**: We need to merge node trees from TWO sources with competing authority:

1. **`sections` field** (schema-driven, structured metadata)
   - Builder has primary write authority over this field content
   - Contains node metadata: schema, complexity, references
   - Can be nested (recursive sections)
   - Authoritative for procedural nodes (steps, sub-procedures)

2. **Markdown headings** (document structure, human-authored)
   - Defines document narrative structure
   - Must create nodes for cross-reference tracking (bi-directional traceability)
   - Cannot be ignored (low-fidelity tracking is unacceptable)

**CONFLICT**: "Unmatched markdown headings are IGNORED" is WRONG.

**Why**: Cross-references require addressable nodes.
- If heading has no node, cross-reference tracking breaks
- Tracking to document instead = low fidelity → useless
- Creating tiny documents for every heading = unmanageable

**Example of the problem**:
```yaml
sections:
  "intro": { complexity: high }
```

```markdown
## Introduction {#intro}
### Background {#background}  <!-- NOT in sections, but should be referenceable! -->
See [[#background]] for details.  <!-- This link must resolve to a node -->
```

**Current design fails**: Background heading ignored → link broken → traceability lost.

### Possible Solutions

#### Option A: Two-Pass MdCodec with Merge

**Pass 1**: Parse markdown → generate ALL heading nodes (complete structure)
**Pass 2**: Inject sections metadata into matched nodes via `inject_context()`

```rust
impl DocCodec for MdCodec {
    fn parse(&mut self, content: &str) -> Result<Vec<ProtoBeliefNode>> {
        // Pass 1: Generate nodes from markdown headings (all headings become nodes)
        let heading_nodes = parse_markdown_to_nodes(content)?;
        
        // Pass 2: Inject sections metadata into matched nodes
        let enriched_nodes = inject_sections_metadata(heading_nodes, frontmatter)?;
        
        Ok(enriched_nodes)
    }
    
    fn inject_context(&mut self, context: &ProtoBeliefNode) -> Result<()> {
        // Called when builder is "in" a section node
        // Use context.content["sections"] to inject metadata into current_events
        // ???
    }
}
```

**Problems**:
- `inject_context()` happens AFTER parse returns
- `sections` field contains non-relational AND relational content (schema, payload fields)
- Unclear how to inject nested sections metadata post-parse

#### Option B: Markdown Headings Define Structure, Sections Provide Metadata

**All headings become nodes** (markdown has structural authority)
**Sections field is metadata lookup** (flat map, no nesting)

```yaml
sections:
  "intro": { complexity: high, schema: "Section" }
  "background": { complexity: medium, schema: "Section" }
  "goals": { complexity: low, schema: "Section" }
```

**Problems**:
- Loses nested sections capability (needed for procedures!)
- Procedures with `sections.step_1.sections.substep_1` can't be represented
- Breaks recursive GenerateChildren model

#### Option C: Hybrid - Markdown Creates Nodes, Sections Creates Additional Nodes

**Markdown headings** → Always create nodes (for cross-reference tracking)
**Sections entries** → Create ADDITIONAL nodes if no heading match

```rust
fn parse(&mut self, content: &str) -> Result<Vec<ProtoBeliefNode>> {
    // 1. Parse markdown → base node tree (all headings)
    let mut nodes = parse_markdown_to_nodes(content)?;
    
    // 2. Parse sections → metadata + additional nodes
    let (metadata_map, additional_nodes) = parse_sections_field(frontmatter)?;
    
    // 3. Inject metadata into matching heading nodes
    for node in &mut nodes {
        if let Some(meta) = find_match(node, &metadata_map) {
            merge_metadata(node, meta)?;
        }
    }
    
    // 4. Add nodes from sections that didn't match any heading
    //    (procedure references, external nodes)
    nodes.extend(additional_nodes);
    
    Ok(nodes)
}
```

**Problems**:
- How to preserve heading-based parent hierarchy when adding unmatched section nodes?
- Where in tree do unmatched sections attach?
- How to represent nested sections if parent doesn't have markdown heading?

#### Option D: Sections Field Stores Non-Nested Metadata Only

**Radical simplification**: `sections` is NOT recursive, just metadata lookup

```yaml
# Frontmatter can't define tree structure, only metadata
sections:
  "intro": { complexity: high }
  "background": { complexity: medium }

# For procedures, use different field that IS hierarchical
procedure:
  steps:  # This is GenerateChildren, but NOT via sections field
    - id: "step_1"
      substeps:
        - id: "substep_1"
```

**Problems**:
- Breaks unified model (different fields for different schemas)
- Procedures can't be documented in markdown with matching structure
- Still need to solve procedure nesting + markdown integration

#### Option E: Sections as Post-Parse Metadata Enrichment ONLY

**Accept the constraints**: `sections` field is NOT generative, only metadata

**Parse flow**:
1. Markdown headings → ALL nodes created (ensures cross-reference tracking)
2. `sections` field → metadata lookup map (flat or nested, doesn't matter)
3. Match and enrich nodes with sections metadata

**For procedures**: Use DIFFERENT schema field that IS generative

```yaml
# Document schema - sections is metadata only
sections:
  "intro": { complexity: high }
  "background": { complexity: medium }

# Procedure schema - steps field generates nodes
[procedure]
steps = [
  { id = "step_1", type = "action" },
  { id = "step_2", type = "action", substeps = [...] }
]
```

**SchemaOperation for procedures**:
```rust
// Procedure schema has GenerateChildren on "steps" field
SchemaField {
    field_name: "steps".to_string(),
    operation: SchemaOperation::GenerateChildren {
        child_schema: "Step".to_string(),
        recursive: true,  // substeps
        edge_weight: WeightKind::Section,
    },
}

// Document schema has StoreAsPayload on "sections" field
SchemaField {
    field_name: "sections".to_string(),
    operation: SchemaOperation::StoreAsPayload { validation: None },
}
```

**Advantages**:
- ✅ Markdown headings always create nodes (cross-reference tracking works)
- ✅ Sections field enriches metadata (simple, predictable)
- ✅ Procedures use appropriate field for hierarchical steps
- ✅ No authority conflict (markdown = structure, sections = metadata)
- ✅ Builder can update sections field with discovered headings

**Disadvantages**:
- ❌ Procedures can't be documented as markdown (no heading → step mapping)
- ❌ Two different fields for similar concepts (sections vs steps)
- ❌ Procedures in markdown docs need different approach

**Viability**: HIGH for Issue 02 scope (markdown documents only)

### Use Case Analysis: What Are We Actually Trying to Solve?

**Use Case 1: Markdown Documentation with Metadata**
```yaml
# docs/architecture.md frontmatter
sections:
  "overview": { complexity: low }
  "core_concepts": { complexity: high }
  "implementation": { complexity: high }
```

**Requirements**:
- All headings in markdown must be addressable (cross-reference tracking)
- Section metadata enriches matched headings
- Unmatched headings still create nodes (for cross-references)
- Unmatched sections metadata is orphaned (warning, but acceptable)

**Solution**: Option E works perfectly (sections = metadata only)

---

**Use Case 2: Procedures with Inline Steps**
```yaml
# procedures/baking.md frontmatter
[procedure]
steps = [
  { id = "preheat", type = "action" },
  { id = "mix", type = "action", substeps = [
      { id = "add_flour", type = "action" },
      { id = "add_eggs", type = "action" }
  ]},
]
```

**Requirements**:
- Steps define hierarchical structure (nested substeps)
- Steps may NOT have corresponding markdown headings (data, not prose)
- Steps that DO have headings get title/text injected
- Markdown is optional documentation, not structure

**Problem**: If markdown creates nodes, we get duplicate nodes (step nodes + heading nodes)

**Solution Options**:
- A) Procedure codec (not MdCodec) generates step nodes from `steps` field
- B) MdCodec detects procedure schema, switches to steps-driven mode
- C) Steps are separate schema, procedures can't be documented in markdown

---

**Use Case 3: Procedures Documented in Markdown**
```yaml
# procedures/deployment.md
[procedure]
# Want sections to map to steps somehow?
```

```markdown
## Step 1: Build Docker Image {#build}
Run `docker build`...

## Step 2: Deploy to Production {#deploy}
### Substep 2.1: Backup Database {#backup}
Run backup script...
```

**Requirements**:
- Headings define step structure (for readable docs)
- Headings also define procedure steps (for execution)
- Need to extract step metadata from headings + frontmatter

**Problem**: Fundamental conflict
- Headings are prose structure (human-readable docs)
- Steps are procedural structure (machine-executable)
- These are NOT always the same hierarchy

**Critical Question**: Should we support this at all?
- Option A: No - procedures are data, use separate files (procedure.toml + docs.md)
- Option B: Yes - but only for simple linear procedures (no nesting mismatch)
- Option C: Yes - complex mapping rules between headings and steps

---

**Use Case 4: Mixed Document (Prose + Embedded Procedure)**
```markdown
# Deployment Guide

## Overview {#overview}
This guide explains our deployment process.

## Procedure {#procedure}
<!-- Embedded procedure here, defined in frontmatter sections? -->

## Troubleshooting {#troubleshooting}
Common issues and solutions.
```

**Requirements**:
- Most headings are prose (overview, troubleshooting)
- One section contains embedded procedure
- Procedure may have nested structure not reflected in markdown

**Problem**: How to distinguish prose sections from procedural sections?

---

**CRITICAL REALIZATION**: We're conflating TWO different problems:

1. **Metadata enrichment**: sections field adds metadata to markdown headings
   - Simple: flat lookup, heading → metadata
   - No authority conflict: markdown creates nodes, sections enriches

2. **Procedural node generation**: steps field generates nested execution nodes
   - Complex: hierarchical generation, may not align with headings
   - Authority conflict: steps vs headings define structure

**PROPOSED RESOLUTION**:
- Issue 02 scope: ONLY solve problem #1 (metadata enrichment)
- Defer problem #2 to future issue (procedural execution)
- `sections` field = metadata lookup ONLY (no GenerateChildren)
- Future: `procedure.steps` field uses different codec/schema

### The Core Question

**How do we reconcile**:
1. Schema-driven node generation (sections field with nesting)
2. Markdown-driven node generation (headings for cross-references)
3. Metadata injection (sections → nodes)
4. Bi-directional traceability (all headings must be addressable)

**When these conflict?**

### Investigation Results: inject_context() Behavior

**Timing**: Called AFTER parse completes, during GraphBuilder "Phase 4"
```rust
// In GraphBuilder::parse_content
codec.parse(content, initial)?;  // Phase 1-3
// ... push all nodes, build graph ...
// Phase 4: context injection
for (proto, bid) in codec.nodes().iter().zip(parsed_bids.iter()) {
    let ctx = self.doc_bb.get_context(bid)?;
    if let Some(updated_node) = codec.inject_context(proto, &ctx)? {
        // Node updated with injected context
    }
}
```

**Purpose**: 
- Node is already in graph with BID and relationships
- Context contains full graph neighborhood (upstream/downstream edges)
- Codec can update node content based on graph context
- Used by MdCodec to update cross-reference links

**Current MdCodec usage**:
- Updates frontmatter if context changed
- Resolves and updates cross-reference links
- Generates updated markdown text with resolved links
- Returns updated BeliefNode if changes made

**Capabilities**:
- ✅ Can update individual node content
- ✅ Has access to full graph context (edges, related nodes)
- ✅ Can modify markdown text (regenerate with link updates)
- ❌ Cannot add NEW nodes to stream (parse already complete)
- ❌ Cannot modify parent-child relationships (graph already built)
- ❌ Cannot access sections field from other nodes easily

**For sections injection**:
- ❌ Too late - nodes already created from markdown
- ❌ Can't add nodes from sections that didn't match headings
- ❌ Can't restructure tree to match sections nesting
- ✅ COULD update node metadata if sections is in ctx.node.payload

**Conclusion**: inject_context() is for POST-PARSE refinement, not structure generation.

### Investigation Needed

1. **Should sections field be flattened (non-recursive)?**
   - Simplifies matching
   - Breaks procedural nesting use case
   - Need alternative for procedures

2. **Should markdown headings ALWAYS create nodes?**
   - Enables cross-reference tracking
   - Creates nodes not in sections field
   - Builder must sync sections field with discovered headings

3. **Two-way synchronization strategy?**
   - Parse: markdown → nodes, sections → metadata
   - Generate: nodes → markdown + sections (preserve both)
   - Who has authority on conflicts?

4. **Where does sections field content live?**
   - Document node payload? (accessible via ctx.node.payload in inject_context)
   - Separate schema registry lookup? (not tied to specific node)
   - Distributed across nodes? (each node knows its section metadata)

**RECOMMENDATION**: Halt implementation until authority model is resolved.

**Critical Insight**: inject_context() happens AFTER graph is built. It cannot be used to generate nodes from sections field. We need a different approach.

**RECOMMENDATION FOR ISSUE 02**:
1. **Narrow scope**: `sections` field is metadata enrichment ONLY
2. **Defer procedures**: Procedural node generation is separate issue
3. **Simple model**: Markdown headings create ALL nodes, sections adds metadata
4. **No authority conflict**: Clear separation (structure vs metadata)

**Next Steps**:
1. Update Issue 02 to clarify: metadata enrichment only (no GenerateChildren)
2. Create separate issue for procedural execution (steps field, different codec?)
3. Implement simple matching: heading BID/anchor/title → sections metadata
4. Define round-trip: generate_source() preserves sections + discovered headings

## CRITICAL UNRESOLVED: Schema-Driven Node Generation vs Codec Responsibility

### The Fundamental Question

If `SchemaOperation::GenerateChildren` exists in the schema registry, **who is responsible for generating those nodes?**

#### Option A: Codec-Specific Interpretation

Each codec interprets GenerateChildren based on its content type:

**MdCodec**:
```rust
// Sees GenerateChildren on "sections" field
// Interprets: "sections is metadata only, markdown headings create nodes"
// Ignores GenerateChildren directive
```

**TomlCodec** (for procedures):
```rust
// Sees GenerateChildren on "steps" field  
// Interprets: "steps field generates child nodes"
// Recursively creates ProtoBeliefNodes from steps tree
```

**Problems**:
- Schema says GenerateChildren, but MdCodec doesn't generate
- Inconsistent behavior across codecs
- Schema operation loses meaning

#### Option B: Schema-Driven, Codec-Agnostic

GraphBuilder reads schema, enforces GenerateChildren:

```rust
// In GraphBuilder::push
if let Some(field) = schema.find_generate_children_field() {
    // Extract field value from node.content
    let children_data = node.content.get(field.field_name)?;
    
    // Generate child ProtoBeliefNodes from data
    let children = generate_children_from_data(children_data, field)?;
    
    // Push children after parent
    for child in children {
        self.push(child, ...)?;
    }
}
```

**Problems**:
- GraphBuilder doesn't know how to parse TOML/YAML/JSON structures
- Different schemas have different data formats
- Codec already parsed content, GraphBuilder doing it again?

#### Option C: Two-Phase Codec Parse

Codec returns nodes in two categories:

```rust
struct CodecParseResult {
    content_nodes: Vec<ProtoBeliefNode>,     // From content (markdown headings)
    schema_nodes: Vec<ProtoBeliefNode>,      // From schema GenerateChildren fields
}
```

**MdCodec**:
- `content_nodes`: All markdown headings
- `schema_nodes`: Empty (doesn't interpret GenerateChildren)

**TomlCodec**:
- `content_nodes`: Document node only
- `schema_nodes`: Nodes from GenerateChildren fields

**Problems**:
- Why have GenerateChildren if MdCodec ignores it?
- Inconsistent schema interpretation
- GraphBuilder doesn't know how to merge the two sets

#### Option D: Schema Operations Are Codec-Specific

Different schemas use different operations:

```rust
// Document schema (for MdCodec) - NO GenerateChildren
SchemaDefinition {
    fields: vec![
        SchemaField {
            field_name: "sections".to_string(),
            operation: SchemaOperation::StoreAsPayload,  // Metadata only
        }
    ]
}

// Procedure schema (for TomlCodec/ProcedureCodec) - YES GenerateChildren
SchemaDefinition {
    fields: vec![
        SchemaField {
            field_name: "steps".to_string(),
            operation: SchemaOperation::GenerateChildren {
                child_schema: "Step".to_string(),
                recursive: true,
                edge_weight: WeightKind::Section,
            }
        }
    ]
}
```

**Advantages**:
- ✅ Clear separation: metadata vs generation
- ✅ Each codec handles appropriate schemas
- ✅ No authority conflicts
- ✅ Schema operations match actual behavior

**Disadvantages**:
- Schema and codec are coupled (can't use Document schema with ProcedureCodec)
- Multiple schemas for similar concepts (sections vs steps)

#### Option E: Codec Registry with Schema Capabilities

Codecs register which SchemaOperations they support:

```rust
trait DocCodec {
    fn supports_operation(&self, op: &SchemaOperation) -> bool;
    fn parse_with_schema(&mut self, content: &str, schema: &SchemaDefinition) -> Result<Vec<ProtoBeliefNode>>;
}

impl DocCodec for MdCodec {
    fn supports_operation(&self, op: &SchemaOperation) -> bool {
        match op {
            SchemaOperation::StoreAsPayload => true,
            SchemaOperation::CreateEdges => true,
            SchemaOperation::GenerateChildren => false,  // Don't support
            _ => false,
        }
    }
}

impl DocCodec for TomlCodec {
    fn supports_operation(&self, op: &SchemaOperation) -> bool {
        match op {
            SchemaOperation::GenerateChildren => true,  // DO support
            _ => true,
        }
    }
}
```

**Problems**:
- Schema validity depends on codec
- Same document, different codec → different nodes?
- Complicates schema registry

### The Core Issue: Who Generates Nodes?

**Current architecture assumption**: Codecs generate ALL nodes during parse()

**Schema-driven architecture**: Schemas tell codecs what to generate

**Conflict**: 
- MdCodec generates nodes from markdown structure (content-driven)
- Schema says generate nodes from `sections` field (schema-driven)
- These can conflict (different hierarchies)

### Proposed Resolution

**For Issue 02**: Schema operations are **descriptive**, not **prescriptive**

```rust
// Schema describes what the codec WILL DO, not what it MUST DO
SchemaOperation::StoreAsPayload { .. }  // "This field will be stored as payload"
SchemaOperation::GenerateChildren { .. } // "This field generates children" (informational)
```

Codecs have full authority over node generation. Schema registry is for:
1. **Edge creation** (CreateEdges) - GraphBuilder reads this
2. **Validation** (field types, required fields)
3. **Documentation** (what fields mean)

**NOT for**:
- Prescribing node generation strategy
- Forcing codecs to generate specific structures

**Future procedural execution**: Separate codec (ProcedureCodec) that DOES use GenerateChildren

### Answer to "How do you suggest we resolve schema parsing?"

**For Issue 02 (MdCodec + Document schema)**:
- Schema has `sections` field with `StoreAsPayload` operation
- MdCodec generates nodes from markdown headings (ignores schema generation)
- Sections enriches metadata (uses schema for validation)

**For Future (ProcedureCodec + Procedure schema)**:
- Schema has `steps` field with `GenerateChildren` operation
- ProcedureCodec reads schema, generates nodes per GenerateChildren
- Markdown is optional documentation (separate concern)

**Different codecs, different schemas, different behaviors - all consistent**

## Next Steps

## Final Conclusions

### Issue 02 Should Be Narrowed to Metadata Enrichment Only

**Original scope (too broad)**:
- Multi-node TOML parsing with recursive sections
- Merge frontmatter tree with markdown tree
- Generate nodes from both sources

**Recommended scope (achievable)**:
- Parse `sections` field as FLAT metadata lookup
- Match metadata to markdown headings by BID/anchor/title
- Enrich heading nodes with section metadata
- All markdown headings create nodes (ensures cross-reference tracking)

### SchemaOperation for Issue 02

```rust
// Document schema - sections is metadata storage, NOT GenerateChildren
SchemaField {
    field_name: "sections".to_string(),
    required: false,
    operation: SchemaOperation::StoreAsPayload { 
        validation: Some(FieldValidation::Nested {
            // Each section can have arbitrary metadata fields
            allowed_fields: None,  // Open-ended
        })
    },
}
```

**NOT**:
```rust
// This was the original design - deferred to future procedural execution issue
SchemaOperation::GenerateChildren {
    child_schema: "Section".to_string(),
    recursive: true,
    edge_weight: WeightKind::Section,
}
```

### Implementation Strategy for Issue 02

```rust
impl DocCodec for MdCodec {
    fn parse(&mut self, content: &str) -> Result<Vec<ProtoBeliefNode>> {
        // 1. Parse frontmatter (document node)
        let doc_node = parse_frontmatter(content)?;
        
        // 2. Extract sections as flat metadata map
        let sections_metadata: HashMap<NodeKey, TomlTable> = 
            if let Some(sections_value) = doc_node.content.get("sections") {
                flatten_sections_to_metadata(sections_value)?
            } else {
                HashMap::new()
            };
        
        // 3. Parse ALL markdown headings → nodes
        let mut heading_nodes = parse_markdown_headings_to_nodes(content)?;
        
        // 4. Enrich matched nodes with sections metadata
        for node in &mut heading_nodes {
            if let Some(metadata) = find_match(node, &sections_metadata) {
                // Merge metadata fields into node.content
                for (key, value) in metadata {
                    if !node.content.contains_key(key) {
                        node.content.insert(key.clone(), value.clone());
                    }
                }
            }
        }
        
        // 5. Return document + heading nodes
        Ok(vec![doc_node].into_iter().chain(heading_nodes).collect())
    }
}

fn flatten_sections_to_metadata(sections: &TomlValue) -> HashMap<NodeKey, TomlTable> {
    // Simple flat extraction - NO recursive nesting
    let table = sections.as_table()?;
    let mut map = HashMap::new();
    
    for (key, value) in table {
        let node_key = parse_node_key(key)?;
        let metadata = value.as_table()?.clone();
        map.insert(node_key, metadata);
    }
    
    Ok(map)
}
```

### What This Solves

✅ **Markdown documentation with metadata** (Use Case 1)
- Headings create nodes (cross-reference tracking)
- Sections enriches with complexity, schema, custom fields
- Round-trip preserves both markdown + sections

✅ **Simple authority model**
- Markdown = structure (what nodes exist)
- Sections = metadata (what fields they have)
- No conflicts, clear separation

✅ **Builder synchronization**
- Builder can update sections field with discovered headings
- generate_source() rebuilds sections from node metadata

### What This Defers (Future Issues)

❌ **Procedural node generation** (Use Case 2)
- Needs different schema field (`procedure.steps`)
- May need different codec (not MdCodec)
- Complex: nesting, execution order, substeps
- Out of scope for Issue 02

❌ **Mixed prose + procedure documents** (Use Case 4)
- Needs schema to distinguish prose vs procedural sections
- Complex mapping rules
- Unclear requirements

❌ **Recursive sections nesting**
- Not needed for markdown metadata enrichment
- Needed for procedures (deferred)
- Keep schema model simple for Issue 02

### Updated Issue 02 Title

**Old**: "Multi-Node TOML Parsing and Markdown Integration"

**New**: "Section Metadata Enrichment for Markdown Headings"

### Key Design Principles Validated

1. ✅ **Simplicity** - Flat metadata lookup vs recursive tree merge
2. ✅ **Clarity** - Markdown = structure, sections = metadata (no ambiguity)
3. ✅ **YAGNI** - Don't build procedural execution until we need it
4. ✅ **Refactoring** - Can extend to GenerateChildren later if needed

## References

- `schema_registry.rs` - Current GraphField model (expand to SchemaOperation)
- `md.rs` - Current MdCodec implementation (add sections metadata matching)
- `builder.rs` - GraphBuilder that processes ProtoBeliefNode streams (no changes needed)
- `procedure_schema.md` - Future: steps field needs separate implementation
- Issue 02 - Multi-node TOML parsing → **Narrow to metadata enrichment**

---

## FINAL RESOLUTION: Schemas for Validation/Relationships, Codecs for Generation

### The Merge Hell Problem

**Attempting to unify THREE sources of node definitions creates merge hell**:
1. Metadata-defined nodes (sections field)
2. Schema-defined nodes (GenerateChildren operations)
3. Content-defined nodes (markdown headings)

**When these conflict**: Who wins? How to reconcile? Authority becomes ambiguous.

### The Solution: Specialized File Extensions

**For simple metadata enrichment** (Issue 02):
- `.md` files → MdCodec
- Markdown headings define nodes (content authority)
- Frontmatter `sections` provides metadata only
- Schema validates, doesn't generate

**For complex procedural generation** (Future):
- `.procedure` files → ProcedureCodec
- Schema `steps` field defines nodes (schema authority)
- Markdown is optional documentation (separate concern)
- Codec and schema are tightly coupled

### Advantages of Codec-Schema Pairing

✅ **No merge conflicts** - One authority per file type
✅ **Clear semantics** - File extension tells you what to expect
✅ **Specialized behavior** - Each codec optimized for its use case
✅ **Extensible** - New domains get new extensions (`.recipe`, `.workflow`, etc.)

### Trade-off: Cross-Platform Compatibility

❌ `.procedure` files won't render nicely in GitHub/editors
❌ Can't embed procedures in markdown documentation seamlessly
❌ Separate files for structure vs documentation

**But this is acceptable** because:
- Procedures are data, not prose (machine-executable)
- Clean separation prevents complexity explosion
- Can still cross-reference between files
- Avoid "merge hell" of conflicting node definitions

## FINAL RECOMMENDATION

**Schema Operations (Final)**:
1. `CreateEdges` - Maps fields to graph edges (GraphBuilder enforces)
2. `StoreAsPayload` - Validates field structure and types
3. `UseAsIdentity` - Marks field as node BID
4. **NO GenerateChildren** - Node generation is codec responsibility

**Issue 02 Scope**: 
1. `sections` field = metadata only (StoreAsPayload operation)
2. MdCodec generates ALL nodes from markdown headings
3. Schema validates `sections` structure (map of NodeKey → metadata)
4. Match and enrich: heading nodes get metadata from `sections` field

**Future Procedural Execution**:
1. Create `.procedure` extension with ProcedureCodec
2. ProcedureCodec generates nodes from `steps` field structure
3. Procedure schema validates `steps` field, doesn't generate nodes
4. Tight coupling between codec and schema (by design)

**Architectural Principle**: 
**Codecs generate nodes from content/structure. Schemas validate fields and map relationships.**

This separation avoids "merge hell" where metadata, schema, and content definitions conflict.
