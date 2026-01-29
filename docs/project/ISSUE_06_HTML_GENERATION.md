# Issue 6: HTML Generation and Interactive Viewer

**Priority**: MEDIUM - Post-open source feature  
**Estimated Effort**: 12-15 days (includes WASM SPA architecture)  
**Dependencies**: Phase 1 complete (Issues 1-4), v0.1.0 released

## Summary

Implement static site generation with progressive enhancement via WASM-powered BeliefBase. Generate Jekyll-style static HTML that works without JavaScript, then layer on rich interactivity by loading the entire belief network in the browser via WebAssembly. This enables offline-capable, client-side query/navigation while maintaining SEO-friendly static content.

**Architecture**: Static HTML (fast initial load, SEO) + WASM BeliefBase (rich interactivity) = Progressive Enhancement SPA

## Goals

### Phase 1: Static HTML Generation (Like Jekyll)
1. Extend `DocCodec` trait with `generate_html()` method
2. Implement HTML generation for `MdCodec`
3. Create CSS stylesheet for noet documents
4. CLI command for batch HTML generation
5. Export BeliefBase to portable format (JSON/MessagePack)

### Phase 2: WASM SPA Enhancement (Progressive)
6. Compile `noet-core` to WASM (browser-compatible subset)
7. Create JavaScript viewer that loads WASM BeliefBase
8. Implement client-side NodeKey resolution and navigation
9. Add interactive features powered by local belief queries
10. Enable offline-capable operation after initial load

## Architecture

### Overview: Static + WASM Hybrid

```
┌─────────────────────────────────────────────────────────────┐
│  Build Time (Server/CI)                                     │
├─────────────────────────────────────────────────────────────┤
│  1. Parse markdown files → BeliefBase                       │
│  2. Generate static HTML (SEO-friendly, works w/o JS)       │
│  3. Export BeliefBase → belief-network.json + .wasm runtime │
│  4. Copy viewer assets (CSS/JS) to output                   │
└─────────────────────────────────────────────────────────────┘
                          │
                          ↓ Deploy to static host
┌─────────────────────────────────────────────────────────────┐
│  Runtime (Browser)                                          │
├─────────────────────────────────────────────────────────────┤
│  1. Load HTML (fast, works immediately)                     │
│  2. Load WASM + belief-network.json (progressive)           │
│  3. Instantiate BeliefBase in browser memory                │
│  4. Install SPA navigation handler (CRITICAL)               │
│     → Intercept internal link clicks                        │
│     → Fetch HTML fragments, swap content                    │
│     → Keep WASM in memory (avoid reload cost)               │
│  5. Enable rich interactive features:                       │
│     - Real-time NodeKey validation                          │
│     - Client-side search across all documents               │
│     - Relationship graph visualization                      │
│     - Dynamic filtering/navigation                          │
│     - Offline operation                                     │
└─────────────────────────────────────────────────────────────┘
```

**Key Benefits**:
- **Fast Initial Load**: Static HTML renders immediately (SEO, no JS required)
- **Rich Interactivity**: WASM provides full query power client-side
- **No WASM Reload**: SPA navigation keeps BeliefBase in memory across page transitions
- **Offline Capable**: Once loaded, works without server connection
- **Progressive Enhancement**: Core content accessible, features enhance experience

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

**Static HTML (No JavaScript Required)**:
- Clean, readable document rendering
- Standard anchor navigation (`<a href="#section">`)
- Collapsible metadata blocks
- Mobile-responsive layout
- Print-friendly styling

**Progressive Enhancement (With WASM)**:
- **Client-Side BeliefBase**: Entire belief network loaded in browser memory
- **Real-Time NodeKey Resolution**: All links validated locally, no server round-trips
- **Interactive Navigation**: 
  - Click heading to copy BID to clipboard
  - Hover to show full node metadata (schema, kind, relationships)
  - Navigate via `#bid://`, `#bref://`, `#id://` anchors
  - Cross-document navigation without page reloads
- **Advanced Features**:
  - Full-text search across all documents (client-side)
  - Relationship graph visualization (D3.js/vis.js)
  - "What links here?" backlinks panel
  - Filter documents by schema, kind, metadata
  - Dependency tree visualization
  - Offline operation after initial load

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

### 3. Export BeliefBase to Portable Format (2 days)

**File**: `src/export.rs` (new)

- [ ] Implement `export_to_json()` - Full belief network serialization
- [ ] Implement `export_to_messagepack()` - Binary format for smaller size
- [ ] Include all nodes, relationships, paths, metadata
- [ ] Optimize for browser deserialization (flat structures)
- [ ] Add compression option (gzip)
- [ ] CLI integration: `noet export ./docs belief-network.json`

**Export Format**:
```json
{
  "version": "0.1.0",
  "nodes": [
    {
      "bid": "01234567-89ab-cdef",
      "kind": ["Document"],
      "title": "My Document",
      "id": "my-document",
      "payload": { ... }
    }
  ],
  "paths": [
    {
      "net": "network-bid",
      "path": "docs/readme.md",
      "node": "node-bid",
      "order": [0, 1]
    }
  ],
  "relations": [
    {
      "source": "bid-1",
      "sink": "bid-2",
      "weight": { "Section": { "parent": "bid-1" } }
    }
  ]
}
```

### 4. Compile noet-core to WASM (3 days)

**Dependencies**: `wasm-bindgen`, `wasm-pack`

- [ ] Create `noet-wasm` crate (subset of noet-core for browser)
- [ ] Exclude server-only dependencies (tokio, sqlx, file watcher)
- [ ] Implement `BeliefBaseWasm` wrapper with `wasm-bindgen`
- [ ] Expose query methods to JavaScript:
  - `query_by_nodekey(nodekey: String) -> Option<Node>`
  - `query_relations(bid: String) -> Vec<Relation>`
  - `search(query: String) -> Vec<Node>`
  - `get_backlinks(bid: String) -> Vec<Node>`
- [ ] Load from JSON: `BeliefBaseWasm::from_json(data: String)`
- [ ] Build with `wasm-pack build --target web`
- [ ] Test in browser environment

**Example WASM API**:
```rust
#[wasm_bindgen]
pub struct BeliefBaseWasm {
    inner: BeliefBase,
}

#[wasm_bindgen]
impl BeliefBaseWasm {
    #[wasm_bindgen(constructor)]
    pub fn from_json(data: String) -> Result<BeliefBaseWasm, JsValue>;
    
    pub fn query_by_nodekey(&self, nodekey: String) -> JsValue;
    pub fn search(&self, query: String) -> JsValue;
    pub fn get_backlinks(&self, bid: String) -> JsValue;
}
```

### 5. Create Viewer JavaScript with WASM Integration (4 days)

**File**: `viewer/noet-viewer.js` (new)

- [ ] `NoetViewer` class with WASM initialization
- [ ] Load WASM module and belief network data:
  - `await init('noet-wasm.wasm')`
  - `beliefBase = BeliefBaseWasm.from_json(networkData)`
- [ ] **Install SPA navigation to prevent WASM reload**:
  - Intercept all internal link clicks (`<a href="/...">`)
  - Fetch HTML via `fetch()`, parse with `DOMParser`
  - Swap content in place, keep WASM in memory
  - Update browser history with `pushState`
  - Handle browser back/forward with `popstate` listener
- [ ] Graceful fallback if WASM unavailable (DOM-only mode)
- [ ] Attach click handlers to `[data-bid]` elements
- [ ] Copy BID to clipboard on click
- [ ] Parse NodeKey URL anchors from `window.location.hash`
- [ ] Resolve NodeKeys via WASM (fast, local queries)
- [ ] Show unresolved reference warnings
- [ ] Metadata tooltip on hover (query WASM for full node data)
- [ ] Client-side search across all documents
- [ ] Backlinks panel ("What links here?")
- [ ] Relationship graph visualization
- [ ] Auto-initialize on DOMContentLoaded

**Core Methods**:
- `async init()` - Initialize WASM + data, install SPA navigation
- `installSPANavigation()` - Intercept links, handle history (CRITICAL for performance)
- `async navigateTo(url)` - Fetch and swap content without WASM reload
- `isInternalLink(url)` - Check if link should use SPA navigation
- `isNodeKeyUrl(url)` - Check if URL uses NodeKey schema
- `resolveNodeKey(nodekey)` - Query WASM for node location
- `copyBidToClipboard(bid)` - Copy BID on click
- `showMetadataTooltip(bid)` - Query WASM, display rich metadata
- `searchDocuments(query)` - Client-side full-text search
- `renderBacklinks(bid)` - Show "What links here?" panel
- `renderRelationshipGraph(bid)` - Visualize node relationships

**WASM Integration with SPA Navigation**:
```javascript
class NoetViewer {
  async init() {
    // Load WASM module
    await init('./noet-wasm.wasm');
    
    // Load belief network data
    const response = await fetch('./belief-network.json');
    const data = await response.text();
    
    // Instantiate BeliefBase in browser (stays in memory)
    this.beliefBase = BeliefBaseWasm.from_json(data);
    
    // CRITICAL: Install SPA navigation to avoid WASM reload
    this.installSPANavigation();
    
    // Enable interactive features
    this.attachEventHandlers();
  }
  
  installSPANavigation() {
    // Intercept all internal link clicks
    document.body.addEventListener('click', async (e) => {
      const link = e.target.closest('a');
      if (!link || !this.isInternalLink(link.href)) return;
      
      e.preventDefault();
      await this.navigateTo(link.href);
    });
    
    // Handle browser back/forward buttons
    window.addEventListener('popstate', (e) => {
      if (e.state?.path) {
        this.loadContent(e.state.path);
      }
    });
  }
  
  async navigateTo(url) {
    // Fetch HTML, extract content, swap in-place
    const html = await fetch(url).then(r => r.text());
    const doc = new DOMParser().parseFromString(html, 'text/html');
    
    // Swap content (WASM stays in memory)
    const newContent = doc.querySelector('.document');
    document.querySelector('.document').replaceWith(newContent);
    
    // Re-attach handlers to new content
    this.attachEventHandlers();
    
    // Update browser history
    history.pushState({path: url}, '', url);
  }
  
  isInternalLink(url) {
    return url.startsWith('/') || url.startsWith(window.location.origin);
  }
  
  resolveNodeKey(nodekey) {
    // Query WASM instead of DOM search
    const node = this.beliefBase.query_by_nodekey(nodekey);
    if (node) {
      return {
        documentPath: node.path,
        anchor: node.id || node.bid
      };
    }
    return null;
  }
  
  async searchDocuments(query) {
    // Client-side search via WASM
    const results = this.beliefBase.search(query);
    return results.map(node => ({
      title: node.title,
      path: node.path,
      snippet: this.getSnippet(node, query)
    }));
  }
}
```

### 6. Create CSS Stylesheet (2 days)

**File**: `viewer/noet-viewer.css` (new)

- [ ] `.noet-document` container styling
- [ ] `.noet-metadata` collapsible block styling
- [ ] `[data-bid]` hover effects (show copy icon)
- [ ] `[data-nodekey]` scroll offset for fixed headers
- [ ] `[data-nodekey]:target` highlight styling
- [ ] `.noet-unresolved-warning` toast styling
- [ ] Responsive layout
- [ ] Dark mode support via `prefers-color-scheme`

### 7. CLI Command for Static Site Generation (2 days)

**File**: `src/bin/noet.rs`

- [ ] Add `generate` subcommand (replaces `html`)
- [ ] Accept input file or directory path
- [ ] Accept output directory path
- [ ] Parse command-line options for `HtmlGenerationOptions`
- [ ] Generate HTML for single file or recursively
- [ ] Export BeliefBase to JSON/MessagePack
- [ ] Copy WASM module to output directory
- [ ] Copy viewer assets (CSS/JS) to output directory
- [ ] Generate index.html with proper asset references
- [ ] Report generation statistics

**Usage**:
```bash
# Static HTML only (no WASM)
noet generate ./docs ./html-output --metadata collapsible

# Full SPA with WASM
noet generate ./docs ./html-output --wasm --search --graph

# Custom configuration
noet generate ./docs ./html-output \
  --wasm \
  --format messagepack \
  --compress \
  --inject-viewer
```

**Output Structure**:
```
html-output/
├── index.html
├── docs/
│   ├── readme.html
│   └── guide.html
├── assets/
│   ├── noet-viewer.css
│   ├── noet-viewer.js
│   ├── noet-wasm.wasm          # WASM runtime
│   └── belief-network.json      # Exported belief data
└── search-index.json            # Optional: pre-built search index
```

### 8. Advanced Interactive Features (3 days)

**File**: `viewer/noet-features.js` (new)

- [ ] **Full-Text Search**: Client-side search across all documents
  - Highlight matching terms
  - Ranking by relevance
  - Filter by schema/kind
- [ ] **Relationship Graph**: Visualize document relationships
  - D3.js or vis.js integration
  - Show Section/Procedure/Reference relationships
  - Interactive node exploration
- [ ] **Backlinks Panel**: "What links here?" for any node
  - Show all incoming references
  - Group by relationship type
  - Click to navigate
- [ ] **Document Outline**: Dynamic TOC based on belief structure
  - Collapsible sections
  - Highlight current position
  - Show metadata badges
- [ ] **Filter Panel**: Filter documents by attributes
  - Schema type (Action, Symbol, etc.)
  - Kind (Document, Section, etc.)
  - Custom metadata fields
- [ ] **Offline Support**: Service Worker for offline operation
  - Cache all HTML + assets
  - WASM + belief data cached
  - Works without network after initial load

### 9. Optional: Server-Side API (Future - Phase 3)

**For distributed/multi-user scenarios, not needed for static SPA**

**API Endpoint**: `GET /api/resolve/{nodekey}`

Defer to Phase 3 - WASM approach handles most use cases client-side.

## Testing Requirements

### Static HTML Generation
- Generate HTML with all metadata rendering modes
- Verify all data attributes present (`data-nodekey`, `data-bid`, `data-bref` when applicable)
- Test multiple resolution paths (by ID, BID, Bref, NodeKey)
- Verify clean HTML `id` values (no prefixes)
- Browser compatibility without JavaScript
- Dark mode CSS rendering
- Mobile responsiveness
- Print-friendly styling

### WASM Integration
- WASM module loads successfully in all browsers
- BeliefBase deserializes from JSON correctly
- Query methods return accurate results
- Memory usage acceptable (< 50MB for typical sites)
- Load time reasonable (< 3s for WASM + data on 3G)
- Graceful fallback if WASM unavailable

### Interactive Features
- Click to copy BID functionality
- NodeKey URL anchor navigation
- Cross-document navigation without reloads
- Client-side search returns relevant results
- Backlinks panel shows correct incoming references
- Relationship graph renders correctly
- Offline mode works after initial load

### CLI Integration
- CLI generates correct output structure
- Assets copied to output directory correctly
- WASM + data files included when `--wasm` flag used
- File paths resolved correctly in HTML
- Round-trip compatibility (markdown → HTML → readable)

## Success Criteria

### Phase 0: Dogfooding Setup (Immediate After MVP)

- [ ] Add GitHub Actions workflow to export `docs/design/` to HTML on every push to main
- [ ] Deploy exported HTML to GitHub Pages or upload as artifact
- [ ] Validate browsable documentation at `https://buildonomy.github.io/noet-core/`
- [ ] Ensure CI fails if HTML export fails (validates feature keeps working)

**Rationale**: Dogfooding validates the feature works in production, provides browsable docs, and ensures regressions are caught immediately.

### Phase 1: Static HTML (MVP)
- [ ] `generate_html()` implemented for `MdCodec`
- [ ] HTML output is standards-compliant and works without JavaScript
- [ ] Clean HTML IDs (no URL prefixes)
- [ ] Metadata rendering modes work correctly
- [ ] CSS provides clean, readable styling
- [ ] Dark mode supported
- [ ] CLI command generates static sites
- [ ] Deployable to GitHub Pages, Netlify, etc.

### Phase 2: WASM SPA (Enhanced)
- [ ] `noet-wasm` crate compiles successfully
- [ ] BeliefBase loads in browser from exported JSON
- [ ] WASM query API works correctly from JavaScript
- [ ] Viewer script handles NodeKey URL anchors via WASM
- [ ] Click to copy BID functionality works
- [ ] Cross-document navigation without page reloads
- [ ] Client-side search functional
- [ ] Backlinks panel shows correct data
- [ ] Relationship graph visualization works
- [ ] Offline mode functional after initial load
- [ ] Performance acceptable (< 3s initial load on 3G)
- [ ] Memory usage reasonable (< 50MB typical)

### General
- [ ] Documentation complete with examples
- [ ] Tests pass with >80% coverage
- [ ] Browser compatibility (Chrome, Firefox, Safari, Edge)
- [ ] Works on mobile devices

## Risks

**Risk**: WASM bundle size too large  
**Mitigation**: Use MessagePack for smaller data format, implement lazy loading, compress assets. Target < 2MB total (WASM + data).

**Risk**: WASM load time unacceptable  
**Mitigation**: Progressive enhancement - static HTML works immediately, WASM enhances. Show loading indicator. Cache aggressively.

**Risk**: Browser compatibility issues with WASM  
**Mitigation**: Feature detection, graceful fallback to DOM-only mode. Test across browsers. WASM supported by 95%+ of browsers (2023+).

**Risk**: Memory usage with large belief networks  
**Mitigation**: Implement pagination, lazy loading of node details, prune unnecessary data from export. Monitor memory usage in tests.

**Risk**: HTML generation complexity  
**Mitigation**: Start simple (basic conversion), iterate based on feedback. Make features optional.

**Risk**: Performance with large documents  
**Mitigation**: Stream HTML generation, optimize WASM queries, use Web Workers for heavy computation.

**Risk**: NodeKey anchor resolution ambiguity  
**Mitigation**: WASM provides deterministic resolution via full belief graph. Fall back to DOM search if WASM unavailable.

## Open Questions

1. **WASM vs JavaScript BeliefBase?** WASM provides better performance and code reuse. Stick with WASM.
2. **JSON vs MessagePack for export?** Support both - JSON for debugging, MessagePack for production (smaller).
3. **Include syntax highlighting?** Yes, use client-side library (Prism.js or highlight.js) to avoid bloating WASM.
4. **Minify HTML output?** Optional flag, default: no (keep readable).
5. **Service Worker for offline?** Yes, but defer to Phase 2.5 - enable with `--offline` flag.
6. **Custom HTML templates?** Defer to Phase 3 - use default template initially.
7. **Search index: pre-built or runtime?** Runtime via WASM for simplicity, consider pre-built index if performance issues.
8. **Graph visualization library?** D3.js (flexible) or vis.js (easier), decide based on use case complexity.

## References

- Current codec: `src/codec/md.rs`
- NodeKey implementation: `src/properties/nodekey.rs`
- Markdown parser: `pulldown_cmark` crate
- WASM tools: `wasm-bindgen`, `wasm-pack`
- Similar projects:
  - **mdBook**: Rust-based static site generator
  - **Docusaurus**: React-based docs with client-side search
  - **Observable**: Notebooks with embedded data/compute
  - **Datasette**: SQLite in browser via WASM
- Graph visualization: D3.js, vis.js, cytoscape.js
- Static site examples: GitHub Pages, mdBook, Hugo, Jekyll