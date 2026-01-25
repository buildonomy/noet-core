# Issue 4: Link Parsing and Manipulation with Bref in Title Attribute

**Priority**: CRITICAL - Required for v0.1.0
**Estimated Effort**: 3-4 days
**Dependencies**: Requires Issues 1, 2, 3 (Schema Registry, Multi-Node TOML, Heading Anchors)

## Summary

Parse and manipulate markdown links to use relative paths (universal renderer compatibility and semantic readability) while storing Bref references in the standard CommonMark title attribute. Transform succinct Bref-only links into full path+Bref format for human navigation. Enable automatic link path updates when targets move, preserving Bref-based identity.

## Goals

1. Parse links in all formats: Bref-only, path+Bref, path-only
2. Transform Bref-only links to path+Bref format (semantic readability)
3. Store Bref in standard title attribute using `noet:` prefix convention
4. Generate relative paths from Bref resolution
5. Auto-update paths when targets move (preserve Bref)
6. Support inline and reference-style links
7. Render to HTML with `data-bref` attributes
8. WikiLink compatibility (convert to standard format)

## Architecture

### Link Format - CommonMark Title Attribute

**Standard CommonMark link with title:**
```markdown
[text](url "title text")
```

**Our convention - structured `noet:key:value` namespace:**
```markdown
[Section](./path.md "noet:bref:abc123")
[Section](./path.md "Click for details noet:bref:abc123")
[Section](./path.md "noet:bref:abc123 noet:auto-title:false")
```

**Why structured namespace:**
- ✅ Standard CommonMark syntax (universal support)
- ✅ No ID conflicts (unlike `{#id}` which sets link element ID)
- ✅ Semantic (title describes link destination)
- ✅ Supports user-provided tooltips + config
- ✅ Extensible (easy to add new parameters: `noet:key:value`)
- ✅ Backward compatible (legacy `noet:abc123` supported)

### User Input Formats

**Format 1: Bref-only (succinct, what user might write):**
```markdown
[Section](bref://abc123)
```

**Format 2: Path+Bref (semantic, what noet generates):**
```markdown
[Section](./docs/guide.md#intro "noet:bref:abc123")
[Section](./docs/guide.md#intro "User tooltip noet:bref:abc123")
```

**Format 3: Path-only (user-provided, noet will inject Bref):**
```markdown
[Details](./docs/details.md#overview)
```

**Format 4: WikiLinks (Obsidian-style):**
```markdown
[[Document Title]]
[[Document Title#Section]]
```

### After Transformation (Canonical Format)

**All links transformed to path+Bref:**
```markdown
[Section](./docs/guide.md#intro "noet:bref:abc123")
[Details](./docs/details.md#overview "noet:bref:def456")
[Document Title](../path/to/doc.md "noet:bref:789abc")
[External](https://example.com/doc "noet:bref:external")
```

**HTML Generation:**
```html
```markdown
<!-- Minimal: just Bref -->
[Link](url "noet:bref:abc123")

<!-- With user title -->
[Link](url "Click here noet:bref:abc123")

<!-- Multiple config params -->
[Link](url "noet:bref:abc123 noet:auto-title:false")

<!-- All features -->
[Link](url "User tooltip noet:bref:abc123 noet:auto-title:true")
```

### Title Attribute Processing

**Structured namespace format: `noet:key:value`**

```rust
#[derive(Debug, Clone)]
struct RefConfig {
    pub bref: Option<String>,
    pub auto_title: Option<bool>,
    // Future extensibility:
    // pub cache_hint: Option<CacheHint>,
    // pub render_mode: Option<RenderMode>,
    // pub relationship: Option<PragmaticKind>,
}

struct LinkTitleParts {
    pub words: Vec<String>,
    pub config: RefConfig,
    pub user_title: Option<String>,
}

fn process_link_title(title: &str) -> LinkTitleParts {
    // Find first "noet:" - everything before is user title, everything after is config
    if let Some(pos) = title.find("noet:") {
        let user_title = title[..pos].trim();
        let config_section = &title[pos..];
        
        LinkTitleParts {
            user_title: if user_title.is_empty() { None } else { Some(user_title.to_string()) },
            config: parse_noet_config(config_section),
        }
    } else {
        LinkTitleParts {
            user_title: Some(title.to_string()),
            config: NoetConfig::default(),
        }
    }
}


fn update_bref_in_config(title: &str, old_bref: &str, new_bref: &str) -> String {
    let parts = process_link_title(title);
    
    if parts.config.bref.as_ref() == Some(&old_bref.to_string()) {
        let mut new_config = parts.config.clone();
        new_config.bref = Some(new_bref.to_string());
        rebuild_title(&parts.user_title, &new_config)
    } else {
        title.to_string()
    }
}

fn rebuild_title(user_title: &Option<String>, config: &RefConfig) -> String {
    let mut words = Vec::new();
    
    // Add user title words
    if let Some(title) = user_title {
        words.extend(title.split_whitespace().map(|s| s.to_string()));
    }
    
    // Add config params
    if let Some(bref) = &config.bref {
        words.push(format!("noet:bref:{}", bref));
    }
    if let Some(auto) = config.auto_title {
        words.push(format!("noet:auto-title:{}", auto));
    }
    
    words.join(" ")
}
```

**Examples:**
```markdown
"noet:bref:abc123"                                  → bref: "abc123", user_title: None, auto_title: None
"Click here noet:bref:abc123"                       → bref: "abc123", user_title: "Click here", auto_title: None
"noet:bref:abc123 noet:auto-title:false"            → bref: "abc123", user_title: None, auto_title: Some(false)
"User text noet:bref:abc123 noet:auto-title:true"  → bref: "abc123", user_title: "User text", auto_title: Some(true)
"noet:abc123"                                       → bref: "abc123" (legacy format), user_title: None, auto_title: None
```

**Examples:**

**Default behavior (text matches old title):**
```markdown
[Getting Started](./guide.md "noet:bref:abc123")
<!-- Target title changes: "Getting Started" → "Quick Start Guide" -->
<!-- link.text == old_node.title → auto-update -->
[Quick Start Guide](./guide.md "noet:bref:abc123")
```

**Default behavior (user customized text):**
```markdown
[Check the Guide](./guide.md "noet:bref:abc123")
<!-- Target title changes, but link.text != old_node.title → preserve -->
[Check the Guide](./guide.md "noet:bref:abc123")
```

**Explicit disable (preserve custom text):**
```markdown
[📖 Read the Guide](./guide.md "noet:bref:abc123 noet:auto-title:false")
<!-- Target title changes, but auto-update disabled → preserve -->
[📖 Read the Guide](./guide.md "noet:bref:abc123 noet:auto-title:false")
```

**Explicit enable (always auto-update):**
```markdown
[My Custom Text](./guide.md "noet:bref:abc123 noet:auto-title:true")
<!-- Target title changes, explicit enable overrides text check → update -->
[Quick Start Guide](./guide.md "noet:bref:abc123 noet:auto-title:true")
```

## Implementation Notes

**Existing Infrastructure in `md.rs`:**
- `LinkAccumulator` (lines 99-162): Already collects link data during parse
- `check_for_link_and_push()` (lines 165-291): Already processes and updates links
- `link_to_relation()` (lines 51-96): Converts links to NodeKey relations

**Integration Points:**
1. Extend `LinkAccumulator` to extract `noet:` config from title
2. Modify `check_for_link_and_push()` to:
   - Parse title for Bref and config using simplified split approach
   - Update paths while preserving Bref
   - Auto-update link text when title changes (use old_context)
3. Update `inject_context()` signature to accept `old_context: Option<&BeliefGraph>`
4. Store old node state from session_bb for comparison

**Title Processing Strategy:**
- Split title at first "noet:" occurrence
- Everything before = user title (clean, no cruft)
- Everything after = config payload (structured namespace)
- Example: "Click here noet:bref:abc123 noet:auto-title:false"
  → user_title: "Click here"
  → config: {bref: "abc123", auto_title: false}

## Implementation Steps

**NOTE** DOn't treat the following function names as canonical. Create an approach using the
implementation notes above and use these implementation steps as inspiration.

### 1. Parse Link Formats (1.5 days)

**File**: `src/codec/md.rs`

- [ ] Detect Bref-only links: `[text](bref://abc123)`
- [ ] Parse path+title: extract href and title from markdown events
- [ ] Implement `process_link_title()` - returns `LinkTitleParts` with bref locations
- [ ] Implement `update_bref_in_title()` - updates all Bref occurrences efficiently
- [ ] Implement `extract_primary_bref()` and `get_user_title()` helper functions
- [ ] Parse WikiLinks: `[[Title]]`, `[[Title#Section]]`
- [ ] Handle inline and reference-style links
- [ ] Store in `LinkInfo` struct with extracted Bref

### 2. Store Links During Parse (1 day)

**File**: `src/codec/md.rs`

- [ ] Add `links: Vec<LinkInfo>` to `MdCodec`
- [ ] Track links during markdown parse (pulldown_cmark events)
- [ ] Capture link text from text events
- [ ] Store position information for error reporting
- [ ] Distinguish between different link formats

### 3. Resolve and Validate Links (1.5 days)

**File**: `src/codec/md.rs::inject_context(old_context, new_context)`

- [ ] Update signature to accept `old_context: Option<&BeliefGraph>` from session_bb
- [ ] For each link, resolve to target node:
  - Bref-only: `new_context.bref_to_bid()` → `new_context.get_node()`
  - Path+Bref: Try Bref first, fallback to path
  - Path-only: `new_context.resolve_path()`
  - WikiLink: `new_context.resolve_title()`
- [ ] Compute relative path from source to target
- [ ] Detect stale paths (path != expected_path)
- [ ] Detect stale Brefs (Bref doesn't resolve or resolves to wrong node)
- [ ] Check if link text should auto-update:
  - Parse title to get `LinkTitleParts` (includes `auto_title` preference)
  - Get old node state from `old_context` via Bref
  - If `auto_title` is Some(false): preserve link text
  - If `auto_title` is Some(true): update if title changed
  - If `auto_title` is None (default): update only if link.text matches old_node.title
- [ ] Handle unresolved references (forward refs) - add to diagnostics

### 4. Transform Links to Canonical Format (1 day)

**File**: `src/codec/md.rs::inject_context()` and `generate_source()`

- [ ] Transform Bref-only → Path+Bref:
  - Compute relative path
  - Add title attribute: `"noet:bref"`
- [ ] Update stale paths, preserve Bref and user title
- [ ] Inject Bref into path-only links (resolve path → get node → get Bref)
- [ ] Auto-update link text when target title changes (if text was auto-generated)
- [ ] Preserve user-customized link text
- [ ] Preserve user title text when present
- [ ] Write format: `[text](./relative/path.md#anchor "user title noet:bref")`
- [ ] Handle reference-style links (transform to inline for simplicity)

### 5. Handle Bref Updates on Node Rename (0.5 days)

**File**: `src/codec/md.rs`

- [ ] When processing `BeliefEvent::NodeRenamed` or BID changes:
  - Find all links with old Bref in title
  - Use `update_bref_in_title()` to update all occurrences efficiently
  - Preserves user title text automatically
- [ ] Handle edge case: multiple different Brefs in same title
- [ ] Only update Brefs that match the old value

### 6. WikiLink Compatibility (0.5 days)

**File**: `src/codec/md.rs`

- [ ] Detect `[[Title]]` and `[[Title#Section]]` syntax
- [ ] Resolve title to node via BeliefBase
- [ ] Extract section anchor if present
- [ ] Generate standard markdown: `[Title](./path.md#section "noet:bref")`
- [ ] Add diagnostic if title doesn't resolve

### 7. HTML Generation (0.5 days)

**File**: `src/codec/md.rs` or Issue 6 HTML generation

- [ ] During HTML generation, process title attributes:
  - [ ] Post-process links to add `data-bref` and `data-auto-title`
    - Extract `noet:bref:*` and `noet:auto-title:*` from title
    - Add `data-bref` attribute for Bref value
    - Add `data-auto-title` attribute if explicitly set
    - Remove `noet:*` prefixed words from user-visible title
  - [ ] Examples:
    - `"Click here noet:bref:abc123"` → `title="Click here" data-bref="abc123"`
    - `"noet:bref:abc123"` → `data-bref="abc123"` (no title attribute)
    - `"Text noet:bref:abc123 noet:auto-title:false"` → `title="Text" data-bref="abc123" data-auto-title="false"`
  - [ ] Escape attribute values properly
</text>

<old_text line=570>
**User writes:**
```markdown
[Getting Started](bref://abc123)
```

**noet transforms to:**
```markdown
[Getting Started](./docs/guide.md#getting-started "noet:abc123")
```

## Testing Requirements

**Link Parsing:**
- Parse Bref-only links: `[text](bref://abc123)`
- Parse path+Bref: `[text](./path.md "noet:abc123")`
- Parse path+Bref with user title: `[text](./path.md "Click here noet:abc123")`
- Parse path-only: `[text](./path.md#anchor)`
- Parse WikiLinks: `[[Title]]`, `[[Title#Section]]`
- Parse reference-style links
- Extract Bref from various title positions (first word, last word, middle)

**Title Processing:**
- `"noet:abc123"` → bref="abc123", title=None
- `"Details noet:abc123"` → bref="abc123", title="Details"
- `"noet:abc123 more text"` → bref="abc123", title="more text"
- `"User text"` → bref=None, title="User text"

**Link Resolution:**
- Resolve Bref to target node
- Resolve path via PathMap
- Resolve WikiLink title
- Handle forward references (unresolved)
- Detect stale paths
- Detect stale Brefs

**Link Transformation:**
- Bref-only → Path+Bref format
- Update stale paths, preserve Bref and user title
- Inject Bref into path-only links
- Preserve user title text

**Bref Update:**
- Update all `noet:old-bref` occurrences to `noet:new-bref` when node renamed
- Efficient batch update via `LinkTitleParts` indices
- Preserve user title: `"Details noet:old"` → `"Details noet:new"`
- Handle multiple occurrences: `"noet:old text noet:old"` → `"noet:new text noet:new"`
- Handle multiple different Brefs: only update matching ones

**Link Text Auto-Update:**
- Parse `LinkTitleParts` to get `auto_title` preference
- If `noet-auto-title:false`: always preserve current link text
- If `noet-auto-title:true`: always update to new title (if changed)
- If unspecified (default): update only if link text matches old target title
- Preserve user-customized link text (default behavior)
- Requires `old_context` from session_bb
</text>

<old_text line=642>
**But if user customized link text:**
```markdown
[Click Here for Guide](./guide.md "noet:abc123")
```

**noet preserves user's custom text (doesn't match old title):**
```markdown
[Click Here for Guide](./guide.md "noet:abc123")
<!-- Link text unchanged - user customized it -->
```

**HTML Generation:**
- Extract Bref from title, add `data-bref` attribute
- Preserve user title text separately
- Handle title-only (no Bref) and Bref-only (no user title)

**Round-trip:**
- Parse → transform → generate → parse should be stable
- Verify semantic readability of generated paths

## Success Criteria

- [ ] Parse all link formats (Bref-only, path+Bref, path-only, WikiLinks)
- [ ] Extract Bref from title attribute using `noet:` prefix
- [ ] Transform Bref-only links to semantic path+Bref format
- [ ] Resolve Brefs and paths to target nodes
- [ ] Auto-update stale paths (preserve Bref and user title)
- [ ] Auto-update link text when target title changes (if text was auto-generated)
- [ ] Inject Brefs into path-only links
- [ ] Update Brefs when target node renamed (all occurrences)
- [ ] Efficient Bref updates via indexed replacement
- [ ] WikiLink compatibility
- [ ] HTML with `data-bref` attributes
- [ ] Generated paths are semantically meaningful (human-readable)
- [ ] User title text preserved alongside Bref
- [ ] Standard CommonMark syntax (no extensions required)
- [ ] Backward compatible
- [ ] Tests pass

## Risks

**Risk**: User confusion about `noet:` prefix in tooltips when viewing raw markdown
**Mitigation**: Document convention clearly; prefix is visible but acceptable; HTML generation strips it

**Risk**: Multiple `noet:` words in title - how to update efficiently?
**Mitigation**: Track all occurrences via `LinkTitleParts`; update all matching old Bref in one pass

**Risk**: Auto-updating link text when user customized it
**Mitigation**: Only update if link text exactly matches old target title; preserve user customizations

**Risk**: Circular link updates (A → B, B → A, both move)
**Mitigation**: Track update generation, limit iterations, detect cycles

**Risk**: Ambiguous title resolution (multiple nodes with same title)
**Mitigation**: Use Bref-based links for disambiguation, warn user via diagnostics

**Risk**: Performance with many links (1000+ links, O(n*m))
**Mitigation**: Build Bref index once, HashMap lookups; batch updates

**Risk**: Title attribute tooltip shows `noet:abc123` on hover in some renderers
**Mitigation**: Acceptable trade-off for standard CommonMark; HTML generation provides clean experience

## Open Questions

1. Should we always strip `noet:` from title in HTML, or keep it? (Recommend: Strip it, add to `data-bref` only)
2. Should we preserve Bref-only format if user explicitly writes it? (Recommend: No, always transform to path+Bref for readability)
3. Auto-update link text when target title changes? (Recommend: Yes, if text matches old title - implement in Phase 1)
4. Support section anchors in WikiLinks: `[[Doc#Section]]`? (Yes, parse and handle)
5. Validate all links at build time? (Yes, return diagnostics via ParseContentResult)
6. How to handle relative path ambiguity when multiple docs have same name? (Recommend: Use Bref to disambiguate, warn user)
7. Should we support user writing multiple `noet:` references in title? (Discourage via docs/linting, but handle gracefully)

## References

- Bref generation: `properties.rs::Bid::namespace()` (lines 180-188)
- CommonMark spec: https://spec.commonmark.org/0.30/#links
- pulldown_cmark events: `Event::Start(Tag::Link { title: Option<CowStr> })`
- Current parser: `codec/md.rs`
- PathMap: `paths.rs`
- Builder: `builder.rs::push()` and `parse_content()`
- Issue 3: Heading anchors and ID management
- Issue 6: HTML generation adds `data-bref` attributes

## Examples

### Example 1: Simple Transformation

**User writes:**
```markdown
[Getting Started](bref://abc123)
```

**noet transforms to:**
```markdown
[Getting Started](./docs/guide.md#getting-started "noet:bref:abc123")
```

**HTML output:**
```html
<a href="./docs/guide.md#getting-started" data-bref="abc123">Getting Started</a>
```

### Example 2: User Title Preserved

**User writes:**
```markdown
[API Reference](./api/reference.md "Click for API docs")
```

**noet injects Bref:**
```markdown
[API Reference](./api/reference.md "Click for API docs noet:bref:def456")
```

**HTML output:**
```html
<a href="./api/reference.md" 
   title="Click for API docs" 
   data-bref="def456">API Reference</a>
```

### Example 3: File Moves, Link Updates

**Original:**
```markdown
[Details](./docs/details.md "See details noet:bref:abc123")
```

**File moves: `docs/details.md` → `reference/details.md`**

**noet updates path, preserves Bref and title:**
```markdown
[Details](./reference/details.md "See details noet:bref:abc123")
```

### Example 4: Node Renamed, Bref Updates (Including Multiple Occurrences)

**Original:**
```markdown
[Section](./doc.md "More info noet:bref:old-bref")
```

**Node's BID changes (merge), Bref changes: `old-bref` → `new-bref`**

**noet updates Bref, preserves path and user title:**
```markdown
[Section](./doc.md "More info noet:bref:new-bref")
```

**Using structured config:**
```rust
// config.bref = Some("old-bref")
// Update to: config.bref = Some("new-bref")
// Rebuild title with new config
```

### Example 5: Auto-Update Link Text When Target Title Changes

**Document A with link:**
```markdown
[Getting Started](./guide.md "noet:bref:abc123")
```

**Target guide.md title changes: "Getting Started" → "Quick Start Guide"**

**noet detects link.text == old_title, updates:**
```markdown
[Quick Start Guide](./guide.md "noet:bref:abc123")
```

**But if user customized link text:**
```markdown
[Click Here for Guide](./guide.md "noet:bref:abc123")
```

**noet preserves user's custom text (doesn't match old title):**
```markdown
[Click Here for Guide](./guide.md "noet:bref:abc123")
<!-- Link text unchanged - user customized it -->
```

### Example 6: WikiLink Conversion

**User writes:**
```markdown
[[Document Title#Section Name]]
```

**noet transforms:**
```markdown
[Document Title](../path/to/doc.md#section-name "noet:bref:789abc")
```

This appendix serves as both implementation guide and reference for the title attribute approach to storing Brefs in standard CommonMark syntax.
