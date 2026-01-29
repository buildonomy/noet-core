# Issue 6: HTML Generation and Interactive Viewer

**Priority**: MEDIUM - Post-open source feature  
**Estimated Effort**: 12-15 days (includes WASM SPA architecture)  
**Dependencies**: Phase 1 complete (Issues 1-4), v0.1.0 released  
**Status**: Phase 1 ✅ Complete | Phase 1.5 🚧 In Progress (Steps 1-5/6 Complete, 2026-01-29)
**Architecture Update**: HTML generation is a parse/watch option, not a separate command

## Summary

Implement static site generation with progressive enhancement via WASM-powered BeliefBase. Generate Jekyll-style static HTML that works without JavaScript, then layer on rich interactivity by loading the entire belief network in the browser via WebAssembly. This enables offline-capable, client-side query/navigation while maintaining SEO-friendly static content.

**Architecture**: Static HTML (fast initial load, SEO) + WASM BeliefBase (rich interactivity) = Progressive Enhancement SPA

## Goals

### Phase 1: Static HTML Generation (Like Jekyll) ✅ COMPLETE
1. ✅ Extend `DocCodec` trait with `generate_html()` method
2. ✅ Implement HTML generation for `MdCodec` with minimal metadata
3. ✅ HTML generation integrated as `--html-output` option for `parse` and `watch` commands
4. ✅ Create CSS stylesheet for noet documents (just-the-docs inspired)
5. 🚧 Export BeliefBase to portable format (deferred to Phase 2)

### Phase 1.5: Static Site Polish + Dev Server (Critical Path) 🚧 IN PROGRESS
1. ✅ Copy static assets (CSS embedded in binary, auto-copied to output)
2. ✅ Rewrite .md links to .html (with bref:// filtering)
3. ✅ Generate network index pages (index.html for each network)
4. ✅ Document titles in HTML body
5. ✅ Dev server with live reload (completed 2026-01-29)
6. 🚧 Dogfooding CI/CD integration (~2 hours)
7. Rewrite `.md` → `.html` in links during HTML generation
8. Generate `index.html` for each BeliefNetwork (document listing)
9. Implement static file server with SSE-based live reload
10. Add `--serve` flag to `watch` command: `noet watch --html-output ./html --serve`
11. Auto-refresh browser when HTML regenerates
12. Hide behind `service` feature flag (requires axum/tower-http)

**Rationale**: Speeds up testing and development feedback loop for Phase 1 and Phase 2. Essential for efficient iteration on CSS/WASM features.

### Phase 2: WASM SPA Enhancement (Progressive)
10. Compile `noet-core` to WASM (browser-compatible subset)
11. Create JavaScript viewer that loads WASM BeliefBase
12. Implement client-side NodeKey resolution and navigation
13. Add interactive features powered by local belief queries
14. Enable offline-capable operation after initial load

## Architecture

### Overview: Static + WASM Hybrid

```
┌─────────────────────────────────────────────────────────────┐
│  Build Time (Server/CI)                                     │
├─────────────────────────────────────────────────────────────┤
│  1. Parse markdown files → BeliefBase                       │
│  2. Generate static HTML (via --html-output flag)           │
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

### 1. Extend DocCodec Trait ✅ COMPLETE (0.5 days)

**File**: `src/codec/mod.rs`

- [ ] Add `generate_html()` method to trait
- [ ] Define `HtmlGenerationOptions` struct
- [ ] Define `MetadataRenderMode` enum
- [ ] Add default implementation returning `None`
- [ ] Document trait extension

### 2. Implement HTML Generation for MdCodec ✅ COMPLETE (2 days)

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

### 3. Bundle Default CSS Theme ✅ COMPLETE (0.5 days)

- [ ] Select default theme (just-the-docs or minima)
- [ ] Copy CSS to `assets/default-theme.css` (embed in binary or install to system)
- [ ] Update `generate_html()` to include `<link rel="stylesheet" href="...">` tag
- [ ] Add `--css` CLI flag for custom CSS override
- [ ] Test rendered HTML in browser with default theme

**CSS Structure:**
```html
<head>
  <link rel="stylesheet" href="/assets/default-theme.css">
  <!-- If --css custom.css provided: -->
  <link rel="stylesheet" href="/assets/custom.css">
</head>
```

### 4. Static Assets and Link Handling ✅ COMPLETE (0.5 days)

**4a. Copy Static Assets to Output Directory**

- [ ] Create `assets/` directory in HTML output on first generation
- [ ] Embed default CSS in binary: `const DEFAULT_CSS: &str = include_str!("../assets/default-theme.css");`
- [ ] Copy CSS to `{html_output_dir}/assets/default-theme.css`
- [ ] Copy custom CSS if `--css` flag provided
- [ ] Update HTML generation to reference: `<link rel="stylesheet" href="/assets/default-theme.css">`

**4b. Rewrite `.md` → `.html` in Links**

Rewrite link extensions during HTML generation in `MdCodec::generate_html()`:

```rust
// In generate_html(), modify events before push_html()
let events = self
    .current_events
    .iter()
    .flat_map(|(_p, events)| {
        events.iter().map(|(e, _)| match e {
            MdEvent::Start(MdTag::Link { link_type, dest_url, title, id }) => {
                // Rewrite .md extensions to .html
                let new_url = if dest_url.ends_with(".md") {
                    CowStr::from(dest_url.replace(".md", ".html"))
                } else if dest_url.contains(".md#") {
                    CowStr::from(dest_url.replace(".md#", ".html#"))
                } else {
                    dest_url.clone()
                };
                MdEvent::Start(MdTag::Link {
                    link_type: *link_type,
                    dest_url: new_url,
                    title: title.clone(),
                    id: id.clone(),
                })
            }
            _ => e.clone(),
        })
    });

let mut html_body = String::new();
pulldown_cmark::html::push_html(&mut html_body, events);
```

**Why this approach:**
- Codec already has events loaded (no duplicate parsing)
- Clean transformation during generation (not post-processing)
- Preserves anchors: `./doc.md#section` → `./doc.html#section`
- Doesn't affect source markdown (pure output transformation)

**4c. Generate Network Index Pages**

Generate `index.html` for each BeliefNetwork after all documents parsed:

```rust
// In DocumentCompiler, after parse_all() completes
async fn generate_network_indices(&self, html_output_dir: &Path) -> Result<()> {
    // Find all network nodes in the belief base
    let networks = self.builder()
        .doc_bb()
        .query(Query::of_kind(BeliefKind::Network))
        .collect::<Vec<_>>();
    
    for network in networks {
        // Query all documents in this network
        let docs = self.builder()
            .doc_bb()
            .paths()
            .get_docs_in_network(network.bid)
            .collect::<Vec<_>>();
        
        // Generate index.html
        let index_html = self.generate_index_page(&network, &docs)?;
        
        // Determine output path (network root or subdir)
        let network_path = self.builder().doc_bb().paths().get_network_path(network.bid)?;
        let index_path = html_output_dir.join(network_path).join("index.html");
        
        tokio::fs::write(index_path, index_html).await?;
    }
    
    Ok(())
}

fn generate_index_page(&self, network: &BeliefNode, docs: &[PathBuf]) -> Result<String> {
    let doc_links = docs.iter()
        .filter_map(|path| {
            let file_stem = path.file_stem()?.to_str()?;
            let title = self.builder().doc_bb()
                .get_node_by_path(path)
                .and_then(|n| n.title())
                .unwrap_or(file_stem);
            Some(format!(
                r#"    <li><a href="{}.html">{}</a></li>"#,
                file_stem, title
            ))
        })
        .collect::<Vec<_>>()
        .join("\n");
    
    Ok(format!(
        r#"<!DOCTYPE html>
<html>
<head>
  <meta charset="utf-8">
  <title>{}</title>
  <link rel="stylesheet" href="/assets/default-theme.css">
</head>
<body>
  <h1>{}</h1>
  <ul>
{}
  </ul>
</body>
</html>"#,
        network.title().unwrap_or("Documents"),
        network.title().unwrap_or("Documents"),
        doc_links
    ))
}
```

- [ ] Call `generate_network_indices()` after `parse_all()` in `parse` command
- [ ] Call `generate_network_indices()` after each compile round in `watch` command
- [ ] Test: verify `index.html` exists at network root with document links

### 5. Dev Server with Live Reload ✅ COMPLETE (2026-01-29)

**Goal**: Seamless development workflow with auto-refresh

```bash
# Developer workflow
noet watch docs/ --html-output ./html --serve --port 3000
# → Serves HTML at http://localhost:3000
# → Edit markdown → HTML regenerates → browser auto-refreshes
```

**Implementation:**

- [x] Add dependencies to `Cargo.toml` (behind `service` feature):
  ```toml
  axum = { version = "0.7", optional = true, features = ["http1"] }
  tokio-stream = { version = "0.1", optional = true, features = ["sync"] }
  tower-http = { version = "0.5", optional = true, features = ["fs"] }
  ```

- [x] Create `src/bin/noet/dev_server.rs` module:
  - Static file server using `tower_http::services::ServeDir`
  - SSE endpoint `/events` that broadcasts reload notifications
  - Receives `broadcast::Receiver<String>` with changed file paths
  - Listens on configurable port (default: 3000)

- [x] Integrate with `watch` command in `src/bin/noet/main.rs`:
  - Add `--serve` flag
  - Add `--port <PORT>` flag (default: 9037 - mnemonic for "noet")
  - ~~Add `--open` flag to auto-launch browser~~ (deferred)
  - Spawn dev server if `--serve` enabled
  - Connect compiler events to dev server reload channel

- [x] Inject live reload script in `generate_html()`:
  - Use `NOET_DEV_MODE` environment variable to detect dev mode
  - Inject SSE client script only when dev mode enabled:
    ```javascript
    const events = new EventSource('/events');
    events.onmessage = (e) => {
      const data = JSON.parse(e.data);
      if (data.type === 'reload') location.reload();
    };
    ```

- [x] Test workflow:
  - Start watch with serve: `noet watch docs/ --html-output ./html --serve`
  - Open browser to http://localhost:3000
  - Edit markdown file
  - Verify browser auto-refreshes

**Architecture:**
```
┌──────────────┐        ┌──────────────────┐        ┌──────────────┐
│  File Watch  │───────>│  DocumentCompiler│───────>│  Dev Server  │
│  (notify)    │ change │  (regenerate HTML)│ event  │  (SSE push)  │
└──────────────┘        └──────────────────┘        └──────────────┘
                                                            │
                                                            ↓ /events
                                                     ┌──────────────┐
                                                     │   Browser    │
                                                     │ (auto-reload)│
                                                     └──────────────┘
```

**Why SSE (Server-Sent Events)?**
- Native browser API (no WebSocket complexity)
- One-way push (perfect for reload notifications)
- Auto-reconnects on disconnect
- Minimal JS footprint

**Why behind `service` feature flag?**
- Adds axum/tower-http dependencies (~2MB compiled)
- Not needed for basic export-html usage
- Optional convenience for active development

### 6. Export BeliefBase to Portable Format (2 days) - DEFERRED TO PHASE 2

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

### 7. Compile noet-core to WASM (3 days) - PHASE 2

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

### 8. Create Viewer JavaScript with WASM Integration (4 days) - PHASE 2

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

### 9. Create CSS Stylesheet (2 days) - MOVED TO STEP 3

**File**: `viewer/noet-viewer.css` (new)

- [ ] `.noet-document` container styling
- [ ] `.noet-metadata` collapsible block styling
- [ ] `[data-bid]` hover effects (show copy icon)
- [ ] `[data-nodekey]` scroll offset for fixed headers
- [ ] `[data-nodekey]:target` highlight styling
- [ ] `.noet-unresolved-warning` toast styling
- [ ] Responsive layout
- [ ] Dark mode support via `prefers-color-scheme`

### 10. CLI Command for Static Site Generation ✅ COMPLETE

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

### 11. Advanced Interactive Features (3 days) - PHASE 2

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

### 12. Optional: Server-Side API (Future - Phase 3)

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
- [x] `generate_html()` implemented for `MdCodec`
- [x] HTML output is standards-compliant and works without JavaScript
- [x] Clean HTML IDs (inherited from Issue 03 - heading anchor management)
- [x] Minimal metadata structure (document BID + section mappings)
- [x] HTML generation integrated with `parse` and `watch` commands (`--html-output` flag)
- [x] Smart regeneration logic (first parse always, reparses only on content change)
- [x] Integrated with DocumentCompiler infrastructure (no BID mocking needed)
- [ ] CSS provides clean, readable styling (next session)
- [ ] Dark mode supported (next session)
- [ ] Deployable to GitHub Pages, Netlify, etc. (needs CSS)

### Phase 1.5: Static Site Polish + Dev Server
- [ ] Copy static assets (CSS) to HTML output directory
- [ ] Embed default CSS in binary, copy to `assets/` on first generation
- [ ] Rewrite `.md` → `.html` in links during HTML generation (in event stream)
- [ ] Generate `index.html` for each BeliefNetwork with document listing
- [ ] Static file server with axum/tower-http (behind `service` feature)
- [ ] SSE endpoint for live reload notifications
- [ ] Add `--serve` flag to `watch` command (requires `--html-output`)
- [ ] Add `--port` and `--open` flags for server configuration
- [ ] Live reload script injection in dev mode
- [ ] Auto-refresh browser on markdown changes
- [ ] Works with both static HTML and future WASM SPA

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

## Implementation Progress (2026-01-29 - Sessions 2-3)

### ✅ Completed (2026-01-29 - Sessions 2-3)

**1. Extended `DocCodec` trait** (`src/codec/mod.rs`)
- Added `generate_html()` method returning `Result<Option<String>, BuildonomyError>`
- Default implementation returns `None` (codec doesn't support HTML)
- Non-breaking change for existing codecs

**2. Implemented `MdCodec::generate_html()`** (`src/codec/md.rs`)
- Generates minimal metadata JSON structure:
  ```json
  {
    "document": { "bid": "..." },
    "sections": { "anchor_id": "section_bid", ... }
  }
  ```
- Uses `pulldown_cmark::html::push_html()` for markdown → HTML conversion
- Wraps in HTML5 boilerplate with embedded `<script type="application/json" id="noet-metadata">`
- Clean semantic HTML (no `data-*` attributes in body)
- Leverages codec singleton architecture (document already parsed, no duplication)

**3. Integrated with `DocumentCompiler`** (`src/codec/compiler.rs`)
- Added `html_output_dir: Option<PathBuf>` field
- New constructor: `DocumentCompiler::with_html_output()`
- Smart regeneration logic in `parse_next()`:
  - **First parse** (parse_count == 0): Always generate HTML (handles empty output dir)
  - **Reparse** (parse_count > 0): Only generate if `rewritten_content.is_some()` (content changed)
- Generates HTML after successful parse, writes to output directory with proper path structure
- Handles errors gracefully (adds diagnostic warnings, continues processing)

**4. Integrated with CLI** (`src/bin/noet.rs`)
- Added `--html-output <dir>` flag to `parse` and `watch` commands
- HTML generation is now a parse side effect, not a separate workflow
- Uses existing DocumentCompiler infrastructure (no BID mocking!)
- Parses documents properly with BeliefBase context
- Works with `--write` flag: can normalize markdown AND generate HTML simultaneously
- Verbose mode shows parse statistics and HTML output location

**5. Updated WatchService** (`src/watch.rs`)
- Added `html_output_dir` field and `with_html_output()` constructor
- Passes HTML output configuration to `FileUpdateSyncer` and `DocumentCompiler`
- Enables continuous HTML generation during file watching

**6. Unit Tests** (`src/codec/md.rs`)
- `test_generate_html_basic` - Verifies HTML structure and metadata
- `test_generate_html_minimal_metadata` - Tests simple document
- Both tests passing ✅

### Test Results

Successfully exported `link_format.md` (design document) to HTML:
- ✅ Valid HTML5 structure with proper DOCTYPE, meta charset, title
- ✅ Valid JSON metadata with document BID + 46 section BID mappings
- ✅ Clean semantic HTML body (no `data-*` pollution)
- ✅ All heading IDs preserved from Issue 03 (heading anchor management)
- ✅ 19KB output file with complete markdown content rendered as HTML
- ✅ Metadata enables SPA to map: document → BID, section anchor → BID

**Example metadata structure:**
```json
{
  "document": {
    "bid": "1f0fd234-f252-6086-9b03-58407829e822"
  },
  "sections": {
    "1-overview": "1f0fd234-f26f-6225-9b07-a5cc3685ac57",
    "3-solution-canonical-link-format": "1f0fd234-f273-63de-9b0b-a5cc3685ac57",
    "link-format-and-reference-system": "1f0fd234-f26c-6fee-9b05-ec31da388413"
  }
}
```

### Architecture Decisions

**Why minimal metadata in HTML?**
- Document BID + section anchor→BID mappings provide complete linkage to BeliefBase
- WASM SPA can query BeliefBase for any additional metadata (title, backlinks, etc.)
- Keeps HTML clean and readable (no visual clutter)
- Extensible: can add per-anchor metadata later without changing HTML structure

**Why HTML as parse option vs separate command?**
- HTML generation happens during parse (already integrated in `DocumentCompiler`)
- Consistent with `--write` flag (both are output modes for parse results)
- Simpler mental model: parse produces markdown and/or HTML
- Avoids redundant parsing: one pass produces both outputs

**Why generate in Compiler vs GraphBuilder?**
- Keeps separation of concerns: GraphBuilder parses, Compiler orchestrates I/O
- HTML generation is "optional export feature" not "core parsing"
- File I/O stays in compiler layer (consistent with `write` flag)
- Codec singleton means document is still loaded (no duplicate parsing)

**Why smart regeneration logic?**
- First parse always generates: handles empty output dir or new files
- Reparse only on content change: avoids unnecessary file writes
- Based on `rewritten_content.is_some()` which indicates actual markdown changes

### ✅ Session 3 Progress (2026-01-29) - Steps 1-4 Complete

**Step 1: Static Assets & Link Rewriting - COMPLETE**
- Implemented `rewrite_md_links_to_html()` in `md.rs`
- Only rewrites links with `bref://` in title attribute (BeliefBase links)
- Preserves anchors: `doc.md#section` → `doc.html#section`
- External links without bref:// left unchanged
- Created `assets/default-theme.css` (~230 lines, just-the-docs inspired)
- Implemented `copy_static_assets()` to embed CSS in binary and copy to output
- Added unit test `test_generate_html_link_rewriting` - all tests passing

**Step 2: Network Index Generation - COMPLETE**
- Implemented `generate_network_indices()` using `PathMap::all_net_paths()`
- Uses `Builder.repo()` to get repository root network (not `api_map()`)
- Generates `index.html` for each network at correct filesystem location
- Filters to documents only via `paths.docs()` (excludes sections/anchors)
- Groups documents by directory with proper hierarchy
- Root index: `{html_output}/index.html`
- Subnet indices: `{html_output}/subnet1/index.html`, etc.
- Document titles from BeliefBase with filename fallback

**Step 3: Bundle Default CSS Theme - COMPLETE**
- Included in Step 1 implementation
- CSS embedded in binary via `include_str!("../../assets/default-theme.css")`
- Automatically copied to `{html_output}/assets/default-theme.css`
- HTML references: `<link rel="stylesheet" href="assets/default-theme.css">`

**Step 4: Static Assets & Link Handling - COMPLETE**
- Included in Steps 1-2 implementation
- Asset copying integrated with compiler initialization
- Link rewriting at event stream level (not post-processing)

**Bonus: Document Titles in HTML Body - COMPLETE**
- Added `<h1 class="document-title">{title}</h1>` to HTML template
- Distinct CSS styling (2.5em font, 2px border, #5c5962 color)
- Preserves markdown h1 structure (both titles present)
- Better standalone HTML experience

**Test Results with `tests/network_1`:**
- ✅ Root index at root with 12 documents
- ✅ Subnet index at `subnet1/` with 2 documents  
- ✅ All HTML files with rewritten links
- ✅ CSS copied to `assets/default-theme.css`
- ✅ Document titles visible with proper styling
- ✅ 3/3 unit tests passing

**Files Modified:**
- `src/codec/md.rs` - Link rewriting, document title injection
- `src/codec/compiler.rs` - Static asset copying, index generation
- `src/codec/belief_ir.rs` - Placeholder index for Network nodes
- `src/bin/noet.rs` - Call `generate_network_indices()` after parse_all
- `assets/default-theme.css` - New CSS theme with document-title styling

### ✅ Step 5 Complete (2026-01-29 - Session 4)

**Dev Server with Live Reload - COMPLETE**

Implementation details:
- Created `src/bin/noet/dev_server.rs` with axum-based server
- SSE endpoint at `/events` for browser notifications
- File watcher monitors HTML output directory (using existing notify infrastructure)
- Debounced changes (500ms) for efficient reload
- Port 9037 default (9=n, 0=o, 3=e, 7=t)
- CLI: `noet watch <path> --html-output <dir> --serve [--port 9037]`
- Live reload script injected via `NOET_DEV_MODE` env var
- Dependencies added to `service` feature: axum, tokio-stream, tower, tower-http

**Architecture decision**: Dev server watches HTML directory directly (using notify) rather than callback mechanism through WatchService - cleaner separation of concerns.

**Testing**: 
```bash
./target/release/noet watch tests/network_1/subnet1 --html-output docs-html --serve
# Access at http://127.0.0.1:9037 ✅
# Edit markdown → HTML regenerates → Browser auto-reloads ✅
```

**Issue 29 Created**: During implementation, identified gap in static asset tracking (images, PDFs not copied during export). Created comprehensive issue for static asset management using External BeliefNode pattern.

### 🚧 Remaining for Phase 1.5

**Phase 1.5 Remaining:**

1. **Dev Server with Live Reload** (~8 hours) - **NEXT SESSION - CRITICAL PATH**
   - Add axum/tokio-stream/tower-http dependencies (behind `service` feature)
   - Create `src/dev_server.rs` with static file serving + SSE
   - Integrate with `watch` command (`--serve`, `--port`, `--open` flags)
   - Inject live reload script in `generate_html()` (dev mode only)
   - Test: edit markdown → auto-refresh in browser
   - **Why critical**: Speeds up testing for CSS styling and future WASM work

2. **Dogfooding: CI/CD integration** (~2 hours)
   - Add GitHub Actions workflow to export `docs/design/` on push to main
   - Deploy to GitHub Pages or upload as artifact
   - Validates feature works in production
   - Validates feature works in production, provides browsable docs

**Phase 2: WASM SPA** (defer until Phase 1.5 complete)
   - Export BeliefBase to JSON
   - Compile noet-core to WASM
   - Create JavaScript viewer with SPA navigation
   - Implement interactive features (will reuse dev server infrastructure)

### Files Modified

- `src/codec/mod.rs` - Added `generate_html()` to `DocCodec` trait
- `src/codec/md.rs` - Implemented HTML generation with metadata, added tests
- `src/codec/compiler.rs` - Added HTML generation integration with smart regeneration
- `src/bin/noet.rs` - Added `--html-output` flag to `parse` and `watch` commands
- `src/watch.rs` - Added `html_output_dir` support to `WatchService` and `FileUpdateSyncer`

### Estimated Remaining Effort (Phase 1.5)

- ~~Static assets & link rewriting~~ ✅ **COMPLETE**
- ~~Network index generation~~ ✅ **COMPLETE**
- ~~CSS theming~~ ✅ **COMPLETE**
- ~~Document titles in body~~ ✅ **COMPLETE**
- Dev server with live reload: ~8 hours **(NEXT SESSION)**
- Dogfooding CI/CD: ~2 hours
- **Total remaining**: ~10 hours to complete Phase 1.5

**Why dev server is the critical path:**
- Eliminates manual refresh during CSS development
- Essential for testing WASM SPA in Phase 2
- Provides professional development experience
- Minimal complexity (SSE is ~100 lines total)
- Hidden behind feature flag (no bloat for basic usage)

## CLI Usage Examples

```bash
# One-shot HTML generation
noet parse docs/design --html-output ./html

# Continuous watching with HTML generation
noet watch docs/design --html-output ./html

# Normalize markdown AND generate HTML
noet parse docs/ --write --html-output ./html

# Watch with live reload (Phase 1.5)
noet watch docs/ --html-output ./html --serve --port 3000

# Combined workflow: normalize, export HTML, serve with live reload
noet watch docs/ --write --html-output ./html --serve --open
```
