# Link Format and Reference System

**Purpose**: Specification of how noet-core handles cross-document links, combining human-readable paths with stable Bref identifiers.

**Status**: Implemented (Issue 04, 2025-01-27)

**Version**: 0.1

## 1. Overview

noet-core transforms markdown links to a **canonical format** that combines:
- Human-readable relative paths (for portability)
- Stable Bref identifiers (for robustness to renames/moves)
- Optional user-provided titles

This enables links that are both **readable** and **resilient**.

## 2. The Problem: Link Fragility

Traditional markdown links break easily:

```markdown
<!-- In docs/guide.md -->
[See tutorial](../tutorials/intro.md)

<!-- If intro.md moves to getting-started/intro.md, link breaks! -->
```

**Why links break**:
1. File renames change the target path
2. Directory restructuring invalidates relative paths
3. No way to distinguish "file not found" from "file moved"

## 3. Solution: Canonical Link Format

noet-core uses the CommonMark **title attribute** to store stable references:

```markdown
[Link Text](relative/path.md#anchor "bref://abc123def456")
```

**Format Components**:
- `Link Text`: User-visible text (can auto-update)
- `relative/path.md`: Current relative path (portable, readable)
- `#anchor`: Optional section anchor
- `"bref://abc123def456"`: Stable 12-character Bref (never changes)

### 3.1. Why Title Attribute?

The CommonMark title attribute is:
- ✅ Part of the markdown spec (not a custom extension)
- ✅ Preserved by all CommonMark parsers
- ✅ Not visually rendered (doesn't clutter the text)
- ✅ Accessible via standard parsing libraries

**Rendering**:
```html
<!-- HTML output -->
<a href="relative/path.md#anchor" 
   data-bref="abc123def456" 
   title="Link Text">
  Link Text
</a>
```

The title attribute becomes `data-bref` in HTML for semantic clarity.

## 4. User Input Formats

Users can write links in multiple ways - all get normalized:

### 4.1. Standard Markdown Links
```markdown
[Tutorial](./docs/tutorial.md)
```

**After transformation**:
```markdown
[Tutorial](docs/tutorial.md "bref://abc123def456")
```

### 4.2. WikiLinks
```markdown
[[Tutorial]]
```

**After transformation**:
```markdown
[Tutorial](docs/tutorial.md "bref://abc123def456")
```

### 4.3. Same-Document Anchors
```markdown
[See below](#details)
```

**After transformation**:
```markdown
[See below](#details "bref://abc123def456")
```

### 4.4. Explicit Bref (Manual Override)
```markdown
[Old Link](old/path.md "bref://xyz789")
```

**Preserved as-is** (path updates if target moves, Bref stays)

### 4.5. Custom Titles
```markdown
[My Custom Title](./docs/tutorial.md)
```

**After transformation**:
```markdown
[My Custom Title](docs/tutorial.md "bref://abc123def456")
```

The custom title is preserved (doesn't auto-update).

## 5. Title Attribute Processing

The title attribute can contain multiple pieces of information:

### 5.1. Format Grammar

```
title_attribute := [words] [config] [bref]

words  := arbitrary text (user-provided title)
config := "{" json "}"  (auto_title, future config)
bref   := "bref://" <12-char-hex>
```

**Examples**:
```markdown
"bref://abc123"                          // Just Bref
"My Title bref://abc123"                 // Title + Bref
"bref://abc123 {\"auto_title\":true}"    // Bref + Config
"My Title {\"auto_title\":false} bref://abc123"  // All three
```

### 5.2. RefConfig Structure

```rust
pub struct RefConfig {
    pub bref: Option<Bref>,
    pub auto_title: bool,  // Default: false
}
```

**auto_title behavior**:
- `false` (default): Keep user's link text even if target title changes
- `true`: Update link text automatically when target's title changes

### 5.3. Processing Algorithm

```rust
fn process_link_title(title: &str) -> LinkTitleParts {
    // 1. Extract Bref (pattern: "bref://[0-9a-f]{12}")
    let bref = extract_bref(title);
    
    // 2. Extract JSON config (pattern: "{...}")
    let config = extract_json_config(title);
    
    // 3. Remaining text is user title
    let words = title - bref - config;
    
    LinkTitleParts { words, config, bref }
}

fn rebuild_title(parts: &LinkTitleParts) -> String {
    format!("{} {} {}", 
        parts.words.unwrap_or(""),
        parts.config.as_json(),
        parts.bref.as_url()
    ).trim()
}
```

### 5.4. Auto-Title Logic

```rust
fn should_enable_auto_title(
    link_text: &str,
    target_title: &str,
    config: &RefConfig
) -> bool {
    // Enable if link text matches target (typical for WikiLinks)
    config.auto_title || link_text == target_title
}
```

**Examples**:

```markdown
<!-- User writes WikiLink -->
[[Tutorial]]

<!-- Transformed with auto_title=true -->
[Tutorial](docs/tutorial.md "bref://abc123 {\"auto_title\":true}")

<!-- Later, target document title changes to "Getting Started" -->
<!-- Link text auto-updates: -->
[Getting Started](docs/tutorial.md "bref://abc123 {\"auto_title\":true}")
```

## 6. Path Generation

Relative paths are calculated using the `pathdiff` crate:

```rust
use pathdiff::diff_paths;

fn generate_relative_path(
    from_doc: &Path,      // e.g., "docs/guide.md"
    to_doc: &Path,        // e.g., "tutorials/intro.md"
) -> String {
    let from_dir = from_doc.parent().unwrap();  // "docs/"
    diff_paths(to_doc, from_dir)
        .unwrap()
        .to_string_lossy()
        .into_owned()
    // Result: "../tutorials/intro.md"
}
```

**Why relative paths?**
- Documents remain portable when moved together
- No dependency on absolute filesystem structure
- Compatible with static site generators

## 7. Link Resolution Process

### 7.1. During Parsing

```rust
// 1. Parse markdown link
let link = "[Tutorial](./docs/tutorial.md)";

// 2. Extract destination and title
let dest = "./docs/tutorial.md";
let title = None;  // No title provided

// 3. Resolve target node via cache_fetch()
let target = cache_fetch(dest, global_bb)?;

// 4. Generate canonical format
let bref = target.bref();
let rel_path = generate_relative_path(current_doc, target_path);
let canonical = format!("[Tutorial]({} \"bref://{}\")", rel_path, bref);

// 5. Store relation in BeliefBase
push_relation(current_node, target, WeightKind::Epistemic);
```

### 7.2. Same-Document Anchors

Fragment-only links stay as fragments:

```markdown
<!-- Input -->
[Details](#section-details)

<!-- Output (NOT converted to full path) -->
[Details](#section-details "bref://abc123")
```

**Detection logic**:
```rust
fn is_same_document_anchor(dest: &str) -> bool {
    dest.starts_with('#')
}
```

## 8. Link Updates on File Operations

### 8.1. File Rename

```markdown
<!-- Before: tutorial.md renamed to intro.md -->
[Tutorial](docs/tutorial.md "bref://abc123")

<!-- After: path updated, Bref unchanged -->
[Tutorial](docs/intro.md "bref://abc123")
```

### 8.2. File Move

```markdown
<!-- Before: intro.md moved from docs/ to getting-started/ -->
[Tutorial](docs/intro.md "bref://abc123")

<!-- After: path updated, Bref unchanged -->
[Tutorial](../getting-started/intro.md "bref://abc123")
```

### 8.3. Title Change (with auto_title)

```markdown
<!-- Before: target title changes from "Tutorial" to "Quick Start" -->
[Tutorial](docs/tutorial.md "bref://abc123 {\"auto_title\":true}")

<!-- After: text updated, path and Bref unchanged -->
[Quick Start](docs/tutorial.md "bref://abc123 {\"auto_title\":true}")
```

## 9. Unresolved References

Links to non-existent targets are preserved with diagnostic tracking:

```markdown
<!-- Input -->
[Missing](./nonexistent.md)

<!-- Output (no Bref available) -->
[Missing](./nonexistent.md)
```

**Diagnostic generated**:
```rust
ParseDiagnostic::UnresolvedReference(UnresolvedReference {
    other_keys: vec![NodeKey::Path { 
        net: current_net,
        path: "nonexistent.md".into()
    }],
    self_bid: current_doc_bid,
    direction: Direction::Outgoing,
    // ... other fields
})
```

**No relation created** - leaves gap in `WEIGHT_SORT_KEY` indices (intentional, tracks missing refs).

## 10. Implementation Details

### 10.1. Code Locations

- **Link parsing**: `src/codec/md.rs` lines 1250-1290
- **Canonical format generation**: `src/codec/md.rs` lines 440-520
- **Title processing**: `src/codec/md.rs` lines 171-270
- **Relation building**: `src/codec/builder.rs` lines 1052-1250
- **Path generation**: `src/paths.rs` lines 774-797

### 10.2. Key Functions

```rust
// Extract title parts
pub fn process_link_title(title: &str) -> LinkTitleParts

// Generate canonical link
pub fn check_for_link_and_push(
    relation: &Relation,
    ctx: &NodeContext,
    events: &mut Vec<MdEvent>
) -> Result<bool>

// Resolve target
async fn push_relation(
    other_key: &NodeKey,
    kind: &WeightKind,
    maybe_weight: &Option<Weight>,
    owner_bid: &Bid,
    direction: Direction,
    index: usize,
    global_bb: BeliefBase,
) -> Result<GetOrCreateResult>
```

### 10.3. Data Structures

```rust
pub struct Relation {
    pub home_bid: Bid,
    pub home_path: String,        // Stripped of anchors
    pub other: Option<NodeContext>,
    pub kind: WeightKind,
    pub payload: Option<Weight>,
}

pub struct NodeContext {
    pub keys: Vec<NodeKey>,
    pub id: Option<String>,
    pub title: Option<String>,
    pub bref: Option<Bref>,
}
```

## 11. Testing

Test coverage (22 tests total):

**Unit tests** (`src/codec/md.rs`):
- Title parsing: Extract Bref, config, user words
- Title rebuilding: Reconstruct from parts
- Config JSON parsing
- auto_title flag handling

**Integration tests** (`tests/codec_test.rs`):
- Canonical format generation
- Same-document anchors
- Path updates on file move
- Title updates with auto_title
- WikiLink conversion

## 12. Design Decisions

### 12.1. Why Not Data Attributes?

We considered `data-bref` in markdown:
```markdown
[Tutorial](docs/tutorial.md){data-bref="abc123"}
```

**Rejected because**:
- Not part of CommonMark spec (custom extension)
- Requires custom parser
- Not compatible with standard tools

### 12.2. Why Not HTML Comments?

We considered embedding Bref in comments:
```markdown
[Tutorial](docs/tutorial.md) <!-- bref:abc123 -->
```

**Rejected because**:
- Visually cluttered
- Comments can be stripped by processors
- Harder to parse reliably

### 12.3. Why Not Separate Metadata File?

We considered external `.noet/links.json`:

**Rejected because**:
- Breaks single-file portability
- Synchronization complexity
- Not self-contained

### 12.4. Path Before Bref

Order in title attribute: `"user words bref://abc123"` not `"bref://abc123 user words"`

**Rationale**:
- User words are primary (human-readable)
- Bref is metadata (secondary)
- Easier to extract Bref from end

## 13. Future Enhancements

### 13.1. Link Validation

Pre-deployment validation:
```bash
noet-core validate --check-links ./docs/
```

Report broken links, suggest fixes.

### 13.2. Link Refactoring

Automated link updates when moving files:
```bash
noet-core refactor --move src/old.md src/new.md
```

### 13.3. Import from Other Systems

Convert existing link formats:
```bash
noet-core import --from obsidian ./vault/
noet-core import --from roam ./export/
```

## 14. References

- CommonMark Spec: https://spec.commonmark.org/0.30/#links
- Issue 04: `docs/project/ISSUE_04_LINK_MANIPULATION.md`
- Bref generation: `src/properties.rs` lines 180-188
- Path resolution: `src/paths.rs::PathMap`
- pulldown_cmark library: https://docs.rs/pulldown-cmark/

## 15. Examples

### 15.1. Basic Link Transformation

```markdown
<!-- Input: user writes -->
[Getting Started](./docs/intro.md)

<!-- After parsing and resolution -->
[Getting Started](docs/intro.md "bref://a1b2c3d4e5f6")
```

### 15.2. Multiple Links to Same Target

```markdown
<!-- Multiple references get same Bref -->
[Intro](docs/intro.md "bref://a1b2c3")
[See introduction](docs/intro.md "bref://a1b2c3")
```

### 15.3. Circular References

```markdown
<!-- doc1.md -->
[Doc 2](doc2.md "bref://abc123")

<!-- doc2.md -->
[Doc 1](doc1.md "bref://def456")
```

Both links work - multi-pass compilation resolves circular dependencies.

### 15.4. External Links (Unchanged)

```markdown
<!-- HTTP links passed through -->
[GitHub](https://github.com/noet/noet-core)

<!-- No Bref added for external URLs -->
```
