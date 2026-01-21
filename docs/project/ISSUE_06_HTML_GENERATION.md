# Issue 6: HTML Generation and Interactive Viewer

**Priority**: MEDIUM - Post-open source feature  
**Estimated Effort**: 8-10 days  
**Dependencies**: Phase 1 complete (Issues 1-4), v0.1.0 released

## Summary

Implement optional HTML generation capability for markdown documents with interactive viewer features. Enables static site generation with NodeKey-aware navigation, metadata display, and cross-document linking while maintaining clean, standards-compliant output.

## Goals

1. Extend `DocCodec` trait with `generate_html()` method
2. Implement HTML generation for `MdCodec`
3. Create JavaScript viewer for interactive features
4. Create CSS stylesheet for noet documents
5. CLI command for batch HTML generation
6. Browser-side NodeKey anchor resolution

## Architecture

### DocCodec Extension

```rust
pub trait DocCodec: Sync {
    fn generate_html(
        &self, 
        options: &HtmlGenerationOptions
    ) -> Result<Option<String>, BuildonomyError> {
        Ok(None)  // Default: not supported
    }
}

pub struct HtmlGenerationOptions {
    pub render_metadata: MetadataRenderMode,
    pub include_bid_attributes: bool,
    pub css_class_prefix: String,
    pub inject_viewer_script: bool,
    pub custom_css: Option<String>,
}

pub enum MetadataRenderMode {
    Hidden,         // Strip frontmatter entirely
    Collapsible,    // <details> block (default)
    Visible,        // Always visible
    DataAttributes, // As data-* on root element
}
```

### HTML Output Structure

```html
<div class="noet-document" data-bid="01234567-89ab-cdef">
    <details class="noet-metadata">
        <summary>Document Metadata</summary>
        <pre>bid: 01234567-89ab-cdef
type: procedure
schema: Action</pre>
    </details>
    
    <h1 id="01234567-89ab-cdef" 
        data-nodekey="bid://01234567-89ab-cdef"
        data-bid="01234567-89ab-cdef"
        data-bref="doc-shortname">
        My Document
    </h1>
    
    <p>Content here...</p>
    
    <h2 id="intro" 
        data-nodekey="bref://intro"
        data-bid="98765432-10ab-cdef"
        data-bref="intro">
        Introduction
    </h2>
</div>
```

**Note**: HTML only allows a single `id` attribute per element. We provide multiple resolution paths via `data-*` attributes:
- `id` - Primary anchor (clean value, no prefix)
- `data-nodekey` - Full NodeKey URL for JavaScript parsing
- `data-bid` - Node's BID (always present for all nodes)
- `data-bref` - Node's Bref (if assigned)
- `data-id` - Custom ID (for `id://` NodeKeys)

This enables multiple ways to target the same element:
```javascript
// All resolve to the same heading:
document.querySelector('#01234567-89ab-cdef')
document.querySelector('[data-nodekey="bid://01234567-89ab-cdef"]')
document.querySelector('[data-bid="01234567-89ab-cdef"]')
document.querySelector('[data-bref="doc-shortname"]')
```

### Viewer Features

**Browser-side capabilities:**
- Click heading to copy BID to clipboard
- Hover heading to show metadata tooltip
- Parse NodeKey URL anchors: `#bid://abc123`, `#bref://xyz`, `#id://custom`
- Redirect NodeKey anchors to resolved document locations
- Search DOM for `[data-nodekey]` attributes
- Optional API-based cross-document resolution

## Implementation Steps

### 1. Extend DocCodec Trait (1 day)

**File**: `src/codec/mod.rs`

- [ ] Add `generate_html()` method to trait
- [ ] Define `HtmlGenerationOptions` struct
- [ ] Define `MetadataRenderMode` enum
- [ ] Add default implementation returning `None`
- [ ] Document trait extension

### 2. Implement HTML Generation for MdCodec (3 days)

**File**: `src/codec/md.rs`

- [ ] Implement `generate_html()` for `MdCodec`
- [ ] Render frontmatter based on `MetadataRenderMode`
- [ ] Convert markdown to HTML using `pulldown_cmark`
- [ ] Inject multiple data attributes for resolution paths:
  - `data-nodekey` - Full NodeKey URL
  - `data-bid` - Node's BID (always)
  - `data-bref` - Node's Bref (if present)
  - `data-id` - Custom ID (for `id://` anchors)
- [ ] Strip NodeKey URL prefixes for clean HTML `id` values
- [ ] Add CSS classes for styling (`noet-document`, etc.)
- [ ] Handle metadata rendering modes
- [ ] Wrap output in semantic HTML structure

**Key Function**:
```rust
fn nodekey_to_html_id(nodekey_url: &str) -> String {
    // Strip URL schema prefix for clean ID
    // bid://abc123 -> abc123
    // bref://xyz -> xyz
    // id://custom -> custom
    nodekey_url
        .strip_prefix("bid://")
        .or_else(|| nodekey_url.strip_prefix("bref://"))
        .or_else(|| nodekey_url.strip_prefix("id://"))
        .unwrap_or(nodekey_url)
        .to_string()
}
```

### 3. Create Viewer JavaScript (3 days)

**File**: `viewer/noet-viewer.js` (new)

- [ ] `NoetViewer` class with initialization
- [ ] Attach click handlers to `[data-bid]` elements
- [ ] Copy BID to clipboard on click
- [ ] Parse NodeKey URL anchors from `window.location.hash`
- [ ] Resolve NodeKeys via DOM search (`[data-nodekey]`)
- [ ] Strip URL prefixes for HTML navigation
- [ ] Optional API-based resolution for cross-document references
- [ ] Show unresolved reference warnings
- [ ] Metadata tooltip on hover
- [ ] Auto-initialize on DOMContentLoaded

**Core Methods**:
- `isNodeKeyUrl(url)` - Check if URL uses NodeKey schema
- `nodeKeyToHtmlId(nodekey)` - Strip schema prefix
- `resolveAndNavigate(nodeKeyUrl)` - Find and scroll to target
- `copyBidToClipboard(bid)` - Copy BID on click
- `showUnresolvedWarning(nodekey)` - Display error toast

### 4. Create CSS Stylesheet (2 days)

**File**: `viewer/noet-viewer.css` (new)

- [ ] `.noet-document` container styling
- [ ] `.noet-metadata` collapsible block styling
- [ ] `[data-bid]` hover effects (show copy icon)
- [ ] `[data-nodekey]` scroll offset for fixed headers
- [ ] `[data-nodekey]:target` highlight styling
- [ ] `.noet-unresolved-warning` toast styling
- [ ] Responsive layout
- [ ] Dark mode support via `prefers-color-scheme`

### 5. CLI Command for HTML Generation (1 day)

**File**: `src/bin/noet.rs`

- [ ] Add `html` subcommand
- [ ] Accept input file or directory path
- [ ] Accept output directory path
- [ ] Parse command-line options for `HtmlGenerationOptions`
- [ ] Generate HTML for single file or recursively
- [ ] Copy viewer assets (CSS/JS) to output directory
- [ ] Report generation statistics

**Usage**:
```bash
noet html ./docs ./html-output --inject-viewer --metadata collapsible
```

### 6. Optional: NodeKey Resolution API (Future)

**API Endpoint**: `GET /api/resolve/{nodekey}`

**Response**:
```json
{
  "nodekey": "bid://01234567-89ab-cdef",
  "resolved": true,
  "document": {
    "path": "/docs/my-document.md",
    "html_path": "/docs/my-document.html",
    "title": "My Document"
  },
  "anchor": "01234567-89ab-cdef",
  "position": {
    "heading_level": 1,
    "heading_text": "My Document"
  }
}
```

Defer to Phase 3 - enables cross-document NodeKey navigation in distributed setups.

## Testing Requirements

- Generate HTML with all metadata rendering modes
- Verify all data attributes present (`data-nodekey`, `data-bid`, `data-bref` when applicable)
- Test multiple resolution paths (by ID, BID, Bref, NodeKey)
- Verify clean HTML `id` values (no prefixes)
- Test viewer script: click to copy, anchor resolution
- Test NodeKey URL anchor navigation
- Browser compatibility (Chrome, Firefox, Safari, Edge)
- Dark mode CSS rendering
- CLI generates correct output structure
- Assets copied to output directory
- Round-trip compatibility (markdown → HTML → readable)

## Success Criteria

- [ ] `generate_html()` implemented for `MdCodec`
- [ ] HTML output is standards-compliant
- [ ] Viewer script handles NodeKey URL anchors
- [ ] Clean HTML IDs (no URL prefixes)
- [ ] Metadata rendering modes work correctly
- [ ] Click to copy BID functionality works
- [ ] Browser navigation to NodeKey anchors works
- [ ] CSS provides clean, readable styling
- [ ] Dark mode supported
- [ ] CLI command generates static sites
- [ ] Documentation complete with examples
- [ ] Tests pass with >80% coverage

## Risks

**Risk**: HTML generation complexity  
**Mitigation**: Start simple (basic conversion), iterate based on feedback. Make features optional.

**Risk**: Browser compatibility issues  
**Mitigation**: Test across major browsers, provide fallbacks for unsupported features.

**Risk**: Performance with large documents  
**Mitigation**: Stream HTML generation, optimize DOM queries in viewer script.

**Risk**: NodeKey anchor resolution ambiguity  
**Mitigation**: Prioritize exact matches, log warnings for ambiguous cases.

## Open Questions

1. Should HTML generation be synchronous or async? (Recommend: sync for simplicity)
2. Include syntax highlighting for code blocks? (Yes, use `syntect` or client-side library)
3. Minify HTML output? (Optional flag, default: no)
4. Support custom HTML templates? (Defer to Phase 3)

## References

- Current codec: `src/codec/md.rs`
- NodeKey implementation: `src/properties/nodekey.rs`
- Markdown parser: `pulldown_cmark` crate
- Renderer examples: GitHub Pages, mdBook, Hugo