# Interactive HTML Viewer Design

**Version**: 0.1  
**Status**: Draft  
**Last Updated**: 2025-02-03

---

> **Note**: This is the **authoritative architecture reference** for the Interactive HTML Viewer. All design decisions are documented here. For implementation tasks and progress tracking, see [ISSUE_38: Interactive SPA Implementation](../project/ISSUE_38_INTERACTIVE_SPA.md).

---

## Summary

The Interactive HTML Viewer is a progressive-enhancement Single-Page Application (SPA) that provides rich navigation, metadata exploration, and graph visualization for Noet-generated HTML documents. It combines static HTML (for accessibility and no-JS environments) with WASM-powered interactivity for enhanced user experience.

**Core Principle**: The same HTML file works with or without JavaScript:
- **No JS**: Clean, readable article with standard links
- **With JS**: Interactive SPA with navigation panels, metadata display, and client-side routing

## Goals

1. **Progressive Enhancement**: Static HTML degrades gracefully without JavaScript
2. **True SPA**: Client-side navigation between documents without page reloads
3. **Rich Metadata**: Display node context, relations, and network information
4. **Graph Visualization**: Force-directed graph for exploring document relationships
5. **Query Interface**: Visual query builder for complex belief graph queries
6. **Mobile-First**: Responsive design with touch-friendly interactions

## Architecture Overview

### Technology Stack

**Static Foundation**:
- HTML5 semantic markup
- CSS Grid layout (responsive, no framework)
- Open Props design tokens (CDN or vendored)
- Standard browser APIs (no dependencies)

**Interactive Layer**:
- WASM module (`noet_core.wasm`) compiled from Rust
- JavaScript viewer (`viewer.js`) for DOM manipulation
- BeliefGraph JSON (`beliefbase.json`) for full network data

**Key Design Decision**: No JavaScript frameworks (React/Vue/etc). Pure DOM manipulation keeps bundle size minimal and avoids framework lock-in.

### Output Structure

HTML generation produces a complete SPA architecture:

```
html_output/
  index.html              ← SPA shell (Layout::Responsive, repo metadata)
  sitemap.xml             ← SEO sitemap with all document URLs
  beliefbase.json         ← Full graph data (synchronized export)
  assets/
    noet-layout.css       ← Custom layout styles
    noet-theme-light.css  ← Light theme
    noet-theme-dark.css   ← Dark theme
    viewer.js             ← Interactive viewer (future)
    open-props/           ← Design tokens (if not using CDN)
      open-props.min.css
      normalize.min.css
  pages/
    docs/
      guide.html          ← Document fragment (Layout::Simple)
      tutorials/
        intro.html        ← Nested document
      index.html          ← Network index (deferred generation)
```

**Two Template Modes**:
- **Layout::Simple**: Minimal wrapper for document fragments (in `pages/`)
- **Layout::Responsive**: Full SPA interface for root `index.html`

**Asset Management**:
- `--cdn` flag: Use unpkg.com for Open Props (smaller output)
- Default: Vendor all assets locally (offline-first)

### Data Flow

```
┌─────────────────┐
│  beliefbase.json│  ← Full network data (root of HTML output)
│  (Network-wide) │
└────────┬────────┘
         │
         ↓ Loaded by viewer.js
┌─────────────────┐
│ BeliefBaseWasm  │  ← WASM module (query engine)
│  (In Browser)   │
└────────┬────────┘
         │
         ↓ Queries by BID
┌─────────────────┐
│  NodeContext    │  ← Rich metadata (node + relations + network)
│ (Per Interaction)│
└─────────────────┘
```

**Document Metadata** (embedded per-HTML file):
```json
{
  "document": {
    "bid": "doc-bid-here"
  },
  "sections": {
    "section-id": "section-bid",
    "another-section": "another-bid"
  }
}
```

**BeliefGraph JSON** (network-wide, at output root):
```json
{
  "states": {
    "1f100f54-2a03-6efc-b251-b6aac611f7f2": {
      "bid": "1f100f54-2a03-6efc-b251-b6aac611f7f2",
      "kind": [
        "Network"
      ],
      "title": "network_1",
      "schema": null,
      "payload": {
        "text": "Small Test directory for BeliefBase codec"
      },
      "id": "belief-network-test-1"
    }
  },
  "relations": {
    "nodes": [...],
    "node_holes": [],
    "edge_property": "directed",
    "edges": [...]
  }
}
```

**Export Synchronization**: The `beliefbase.json` file is guaranteed to contain complete graph data through event loop synchronization (see § Data Export Timing below).

## Standard Paths

All paths are relative to HTML output directory root:

| Asset | Path | Notes |
|-------|------|-------|
| BeliefGraph JSON | `beliefbase.json` | Network-wide data |
| Viewer Script | `assets/viewer.js` | Main interactive logic |
| Stylesheets | `assets/*.css` | Themes and layout |
| Open Props | `assets/open-props/*` | Design tokens (optional CDN) |

**WASM Integration**: WASM artifacts are embedded in the binary (like CSS/JS) and extracted to `assets/` during HTML generation. Build process must run `wasm-pack build` before embedding.

**Rationale**: Fixed paths simplify deployment and eliminate configuration. Embedding WASM maintains offline-first architecture and consistent UX (no manual copy steps).

## Data Export Timing

The `beliefbase.json` file must contain complete graph data with all nodes and relations from the parsing session. This requires **event synchronization** to ensure all `BeliefEvent`s have been processed before export.

### The Challenge

Parsing emits events asynchronously:

```
Compiler → [events in channel] → BeliefBase.process_event()
           ↓ (parse completes)
      Export too early! ← Missing in-flight events
```

If export happens before event processing completes, the JSON will be incomplete.

### Solution: Event Loop Synchronization

The `parse` command uses explicit event loop management (Option G pattern):

```rust
// 1. Create event channel
let (tx, mut rx) = unbounded_channel::<BeliefEvent>();

// 2. Spawn background task to process events
let mut global_bb = BeliefBase::empty();
let processor = tokio::spawn(async move {
    while let Some(event) = rx.recv().await {
        let _ = global_bb.process_event(&event);
    }
    global_bb  // Return synchronized BeliefBase
});

// 3. Parse all documents (sends events via tx)
let mut compiler = DocumentCompiler::with_html_output(path, Some(tx), ...)?;
compiler.parse_all(cache, force).await?;

// 4. Close channel to signal completion
drop(compiler);  // Drops tx, closing channel

// 5. Wait for all events to drain
let final_bb = processor.await?;

// 6. Export from synchronized state
let graph = final_bb.clone().consume();
export_beliefbase_json(graph, html_dir).await?;
```

**Key Points**:
- Background task processes events asynchronously
- Dropping compiler closes transmitter, signaling no more events
- `processor.await` blocks until event queue is empty
- Export happens from fully-synchronized `final_bb`

### Watch Service vs Parse Command

| Mode | BeliefSource | Event Processing | Export Location |
|------|-------------|------------------|----------------|
| **Watch Service** | `DbConnection` (SQLite) | Database transaction loop | `finalize()` exports from DB |
| **Parse Command** | `BeliefBase` (in-memory) | Explicit background task | After synchronization in main |

Watch service has persistent database that processes events in its own loop, so `finalize()` can safely export. Parse command requires explicit synchronization pattern.

**Result**: `beliefbase.json` always contains complete graph (typically 30-50KB for medium networks, 57+ nodes in test fixtures).

## BID Injection for Reliable Metadata Loading

To enable reliable metadata loading on page load, document BIDs are injected directly into HTML templates during compilation, eliminating path/extension mismatch issues.

### The Problem

Path-based BID lookup was unreliable:
- URL: `net1_dir1/spatial_web_standards.html`
- PathMap key: `net1_dir1/spatial_web_standards.md`
- Error: "No BID found for path" (extension mismatch)

Extension normalization at runtime is fragile and fails for edge cases.

### Solution: Compile-Time BID Injection

**Template Modification** (`template-simple.html`):
```html
<body data-document-bid="{{BID}}">
    <!-- content -->
</body>
```

**Compiler Modification** (`compiler.rs`):
```rust
async fn write_fragment(
    &self,
    html_output_dir: &Path,
    rel_path: &Path,
    html_body: String,
    title: &str,
    bid: &Bid,  // ← New parameter
) -> Result<(), BuildonomyError> {
    // ... template loading ...
    
    let html = template
        .replace("{{BODY}}", &html_body)
        .replace("{{CANONICAL}}", &canonical_url)
        .replace("{{SPA_ROUTE}}", &spa_route)
        .replace("{{TITLE}}", title)
        .replace("{{BID}}", &bid.to_string());  // ← Inject BID
    
    // ... write file ...
}
```

**JavaScript Extraction** (`viewer.js`):
```javascript
async function loadDocument(path, sectionAnchor, targetBid) {
    const response = await fetch(fetchPath);
    const html = await response.text();
    const doc = new DOMParser().parseFromString(html, "text/html");
    
    // Extract BID from data attribute
    let documentBid = null;
    const bodyElement = doc.querySelector("body[data-document-bid]");
    if (bodyElement) {
        documentBid = bodyElement.getAttribute("data-document-bid");
        console.log(`[Noet] Extracted document BID: ${documentBid}`);
    }
    
    // Use extracted BID for metadata (fallback chain)
    const bidToShow = targetBid || documentBid || getBidFromPath(path);
    if (bidToShow) {
        showMetadataPanel(bidToShow);
    }
}
```

### BID Extraction During Compilation

**Immediate HTML Generation (Phase 1)**:
```rust
// ProtoBeliefNode stores parsed TOML with "bid" field
let bid = proto
    .document
    .get("bid")
    .and_then(|b_val| b_val.as_str().map(|b| Bid::try_from(b).ok()).flatten())
    .unwrap_or(Bid::nil());
```

**Deferred HTML Generation (Phase 2)**:
```rust
// BeliefBase has full node context with actual BID
let node = bb.get(&nodekey)?;
self.write_fragment(html_output_dir, &rel_path, html_body, &title, &node.bid)
```

### Benefits

- **Extension-agnostic**: Works regardless of `.md`, `.html`, or other formats
- **Single source of truth**: BID injected at compile time, not runtime lookup
- **Reliable**: No path normalization or extension guessing needed
- **Graceful degradation**: Falls back to path-based lookup if attribute missing

## Multi-Network Context Queries

The WASM `get_context()` method must handle nodes from different network namespaces, not just the entry point network.

### The Problem

Special namespace nodes (href_namespace, asset_namespace) have different home networks than the entry point:
```
Entry point: 1f10cfd9-19ab-6ef2-86d2-6ace77cb4a7d (user's network)
Asset node:  1f10cfd9-1cc3-6a93-86f9-0e90d9cb2fdb (asset_namespace)
```

Querying with entry point network fails: "⚠️ Node not found in context"

### Solution: Multi-Network Fallback Strategy

**WASM Implementation** (`wasm.rs`):
```rust
pub fn get_context(&self, bid: String) -> JsValue {
    let bid = Bid::try_from(bid.as_str())?;
    
    let mut inner = self.inner.borrow_mut();
    
    // Try entry point first (fast path for regular content)
    let ctx = match inner.get_context(&self.entry_point_bid, &bid) {
        Some(c) => c,
        None => {
            // Fallback to special namespaces
            let href_ns = href_namespace();
            let asset_ns = asset_namespace();
            let buildonomy_ns = buildonomy_namespace();
            
            inner
                .get_context(&href_ns, &bid)
                .or_else(|| inner.get_context(&asset_ns, &bid))
                .or_else(|| inner.get_context(&buildonomy_ns, &bid))
                .or_else(|| {
                    console::warn_1(&format!(
                        "⚠️ Node not found in any context: {}", bid
                    ).into());
                    None
                })?
        }
    };
    
    // ... serialize context to JsValue ...
}
```

### Why Multiple Networks Are Needed

`BeliefBase::get_context(root_net: &Bid, bid: &Bid)` requires the correct root network:
- Uses `root_net` to lookup path map
- Path map contains nodes belonging to that network
- Cross-network nodes (href/asset references) live in their own namespaces

**Network Ownership**:
- Regular content nodes → Entry point network
- External references → `href_namespace()`
- Images/assets → `asset_namespace()`
- System nodes → `buildonomy_namespace()`

### Performance Considerations

- Entry point network checked first (most common case)
- Only 3 additional lookups needed on cache miss
- Each `get_context()` call has index synchronization overhead regardless
- Fallback pattern adds negligible latency (~microseconds)

## Link Detection and Navigation

### Link BID Attribution

During HTML generation, **all links** (internal and external) are marked with BID metadata:

```html
<a href="other-doc.html" title="bref://285efc055ac2">Link Text</a>
<a href="#section-id" title="bref://1f100f54-2a96">Section Link</a>
<a href="https://example.com" title="bref://external-bid">External</a>
```

**Key**: The `title` attribute contains `bref://[bref]` for NodeKey extraction.

**Conversion**: Use `NodeKey::from_str("bref://[bref]")` to parse, then `BeliefBase::get(&node_key)` to retrieve node.

### Internal vs External Detection

**Algorithm**:
1. Extract NodeKey from `title="bref://[bref]"` attribute via `NodeKey::from_str()`
2. Resolve to BID: Call `beliefbase.get(&node_key)` to get node
3. Call `beliefbase.get_context(bid)` to fetch node context
4. Check `context.home_net` to determine link type:
   - **`home_net == href_namespace()`**: External web link (open in new tab or navigate away)
   - **`home_net == asset_namespace()`**: Static asset (display inline or download)
   - **`home_net == buildonomy_namespace()`**: Network node (navigate to network index)
   - **Any other network BID**: Document network link
     - If network is in current beliefbase: Internal link (fetch HTML, inject into DOM)
     - If network not loaded: External link (open in new tab)

**Special Namespaces** (from `src/properties.rs`):
- `href_namespace()`: External URLs (http://, https://)
- `asset_namespace()`: Static assets (images, CSS, fonts)
- `buildonomy_namespace()`: Network metadata nodes

**Multiple Networks**: A beliefbase may contain multiple document networks (root + subnetworks). All are considered "internal" for navigation purposes.

**External Link Handling**:
- **First click**: Show metadata panel with link context (analyze frequency/cross-references)
- **Second click**: Open external link in new tab (after reviewing metadata)
- Same two-click pattern as internal links (consistency)

**Use Case**: Link frequency analysis
- See how often external links are cross-referenced within docs
- Identify critical external dependencies
- Track citation patterns across documentation
- Context before leaving site (review metadata first)

**Special Cases**:
- Anchor links (`#section-id`): Scroll to section AND update metadata panel (same two-click pattern)
- Asset links in `<a>` tags (`asset_namespace()`): Show metadata first, download on second click
- Images/scripts (`<img src>`, `<script src>`): Load normally (not intercepted)
- Missing BID: Treat as external (fail gracefully)

### Two-Click Navigation Pattern

The two-click pattern provides contextual metadata access without interrupting reading flow. Links within the main content area require two clicks: first to show metadata, second to navigate.

#### Scope

**Pattern Applies To**:
- All `<a>` elements within `<article>` tag (main content area only)
- Both internal links (documents, sections) and external links
- **Images with bref:// in title attribute** (wrapped in clickable divs)
- **Header anchor links** (automatically generated for all headers)

**Pattern Does NOT Apply To**:
- Navigation panel links (single-click navigation)
- Metadata panel links (single-click navigation)
- Header/footer links (single-click navigation)
- Images without bref:// in title (single-click opens modal)

#### State Management

```javascript
// Global state variable
let selectedBid = null;

// Click handler on <article> links
article.addEventListener('click', (e) => {
    if (e.target.tagName !== 'A') return;
    
    const linkBid = getLinkBid(e.target); // from data-bid or href resolution
    
    if (selectedBid === linkBid) {
        // Second click: navigate
        navigateToTarget(e.target);
        selectedBid = null; // Reset for next interaction
    } else {
        // First click: show metadata
        showMetadataPanel(linkBid);
        selectedBid = linkBid; // Track for potential second click
    }
    
    e.preventDefault();
});
```

#### First Click Behavior

**Any Link Type** (internal, anchor, external):
1. Call `wasm.get_context(selectedBid)` to fetch full `NodeContext`
2. Populate metadata panel with:
   - Node properties (kind, schema, title, payload)
   - Backlinks (who references this node)
   - Forward links (what this node references)
   - Related nodes from graph
3. Show metadata panel (slide in from right on desktop, drawer on mobile)
4. Highlight link to indicate "click again to navigate"
5. Store `selectedBid = linkBid`

#### Second Click Behavior

**Internal Document Link**:
1. Fetch full HTML document from server
2. Extract `<article>` content via DOM parsing
3. Replace current `<article>` with fetched content
4. Update URL via hash routing (`window.location.hash`)
5. Reset `selectedBid = null`

**Section/Anchor Link**:
1. Scroll to target section smoothly
2. Highlight section temporarily (CSS animation)
3. Update URL hash (`#section-id`)
4. Reset `selectedBid = null`

**External Link**:
1. Open link in new tab/window
2. Reset `selectedBid = null`

#### Click Reset Scenarios

**Different Link Clicked**:
- If `selectedBid !== linkBid`: Perform first-click behavior for new link
- If metadata panel open: Update content (sticky panel behavior)
- If metadata panel closed: Stay closed

**Panel Closed Manually**:
- Reset `selectedBid = null`
- Next click is always "first click"

**Navigation Event** (browser back/forward):
- Reset `selectedBid = null`
- Close metadata panel

#### Document Fetching Strategy

**Full HTML Fetch** (not fragments):
- Fetch complete HTML document (191-line template overhead is acceptable)
- Use DOM parser: `new DOMParser().parseFromString(html, 'text/html')`
- Extract via `fetchedDoc.querySelector('article').innerHTML`
- Replace current article: `document.querySelector('article').innerHTML = extractedContent`

**Why Full HTML**:
- Template overhead (~191 lines) is minimal
- DOM extraction is efficient
- Avoids creating separate fragment endpoint
- Consistent with static site serving

#### Visual Feedback

**First Click**:
- Link gets `.clicked-once` class (subtle visual indicator)
- Optional tooltip: "Click again for metadata"

**Second Click**:
- Remove `.clicked-once` class
- Metadata panel animates in
- Link highlighted in metadata panel header

#### Rationale

Two-click pattern reduces metadata panel spam while keeping information accessible:
- First click: Fast navigation (reading flow preserved)
- Second click: Deep dive into node relationships (exploration mode)
- Sticky panel: User intent to explore metadata maintained across links

### Image Modal Integration

Images in content are post-processed after document load to support modal viewing and two-click metadata preview.

#### Image Wrapping

All `<img>` elements are wrapped in `.noet-image-wrapper` divs:

```javascript
// In processLoadedContent()
const images = article.querySelectorAll("img");
images.forEach((img) => {
    const wrapper = document.createElement("div");
    wrapper.className = "noet-image-wrapper";
    
    // Check for bref:// in title attribute
    const imgTitle = img.getAttribute("title");
    if (imgTitle && imgTitle.includes("bref://")) {
        wrapper.setAttribute("data-two-click", "true");
        wrapper.setAttribute("data-image-title", imgTitle);
    }
    
    img.parentNode.insertBefore(wrapper, img);
    wrapper.appendChild(img);
});
```

#### Two-Click Pattern for Images

**Images with `bref://` in title**:
1. First click: Show metadata panel (extract BID from title)
2. Second click: Open full-screen modal

**Images without `bref://`**:
- Single click: Open full-screen modal directly

#### Modal Behavior

Full-screen image modal with:
- Dark overlay (rgba(0,0,0,0.8))
- Padded content container with border radius
- Small close button (top-right, inside padding)
- Close on: button click, overlay click, Escape key
- Image constrained to 90vh with padding

```css
.noet-image-modal__content {
    padding: var(--size-4);
    background: var(--noet-bg-primary);
    border-radius: var(--radius-3);
}

.noet-image-modal__close {
    width: var(--size-6);
    height: var(--size-6);
    font-size: var(--font-size-3);
}
```

### Header Anchor Links

All headers (`h1-h6`) automatically receive anchor links for section navigation.

#### Anchor Generation

```javascript
// In processLoadedContent()
const headers = article.querySelectorAll("h1, h2, h3, h4, h5, h6");
headers.forEach((header) => {
    const headerId = header.getAttribute("id");
    if (!headerId) return;
    
    const anchor = document.createElement("a");
    anchor.className = "noet-header-anchor";
    anchor.href = `#${headerId}`;
    anchor.textContent = "🔗";
    anchor.setAttribute("title", `bref://${headerId}`);
    
    header.appendChild(anchor);
});
```

#### Visual Design

- Font size: 60% of header text (subtle, not prominent)
- Opacity: 0 by default, 1 on header hover
- Position: Appended to end of header with margin-left
- Vertical align: middle

#### Two-Click Pattern

Anchor links include `title="bref://[section-id]"`:
1. First click: Show metadata for section node
2. Second click: Navigate to section (scroll + highlight)

### Section Navigation Highlighting

When navigating to a section (via `navigateToSection()`), the target element receives visual highlight:

```javascript
function navigateToSection(anchor, targetBid) {
    const sectionId = anchor.substring(1);
    const targetElement = document.getElementById(sectionId);
    
    if (targetElement) {
        targetElement.scrollIntoView({ behavior: "smooth", block: "start" });
        highlightElementById(sectionId); // Apply .noet-link-selected class
        // ... update hash and show metadata
    }
}
```

Uses same `.noet-link-selected` styling as two-click link highlights.

## WASM Integration

### NodeContext Structure

**Challenge**: `BeliefContext<'a>` in Rust has lifetime bounds (can't cross FFI boundary).

**Solution**: Serialize immediately into owned structure:

```rust
#[derive(Serialize)]
pub struct NodeContext {
    /// The node itself
    pub node: BeliefNode,
    
    /// Relative path within home network (e.g., "docs/guide.md#section")
    pub home_path: String,
    
    /// Home network BID (which Network node owns this document)
    pub home_net: Bid,
    
    /// All nodes related to this one (other end of all edges)
    /// Map from BID to BeliefNode for O(1) lookup when displaying graph relations.
    /// For each relation where this node is a source, includes the sink.
    /// For each relation where this node is a sink, includes the source.
    /// Provides BeliefNode data for display_title(), keys(), etc. in metadata panel.
    pub related_nodes: BTreeMap<Bid, BeliefNode>,
    
    /// Relations by weight kind: Map<WeightKind, (sources, sinks)>
    /// Sources: BIDs of nodes linking TO this one
    /// Sinks: BIDs of nodes this one links TO
    /// Both vectors are sorted by WEIGHT_SORT_KEY edge payload value
    pub graph: HashMap<WeightKind, (Vec<Bid>, Vec<Bid>)>,
}
```

**Rationale**: 
- `related_nodes` provides O(1) BID→BeliefNode lookup for graph navigation
- `graph` groups relations by type with sorted BID lists (for navigation structure)
- JavaScript can lookup node details: `ctx.related_nodes[bid]` for any BID in graph
- Matches Rust's `BeliefContext` structure but with owned data (no lifetimes)
- Serializable to JSON for JavaScript consumption (BTreeMap → object)

### WASM API

```rust
#[wasm_bindgen]
impl BeliefBaseWasm {
    /// Get full context for a node
    pub fn get_context(&self, bid: String) -> JsValue;
    
    /// Get node by BID (convenience)
    pub fn get_by_bid(&self, bid: String) -> JsValue;
    
    /// Search by title substring
    pub fn search(&self, query: String) -> JsValue;
    
    /// Get backlinks (nodes linking TO this one)
    pub fn get_backlinks(&self, bid: String) -> JsValue;
    
    /// Get forward links (nodes this one links TO)
    pub fn get_forward_links(&self, bid: String) -> JsValue;
    
    /// Query with Expression syntax
    pub async fn query(&self, expr: JsValue) -> Result<JsValue, JsValue>;
    
    /// Get PathMaps (network document structures for navigation)
    pub fn get_paths(&self) -> JsValue;
    
    /// Get PathMap for specific network
    pub fn get_path_for_network(&self, net_bid: String) -> JsValue;
    
    /// Get system network namespace BIDs
    /// These identify special tracking networks for external links, assets, and API versioning.
    /// See `docs/design/architecture.md` § 10 for conceptual overview.
    /// See `docs/design/beliefbase_architecture.md` § 2.7 for technical specification.
    pub fn href_namespace() -> String;        // External HTTP/HTTPS links network
    pub fn asset_namespace() -> String;       // Images/PDFs/attachments network
    pub fn buildonomy_namespace() -> String;  // API node (version management)
}
```



## HTML Template Structure

**Single Template** (`template-responsive.html`):
```html
<!doctype html>
<html>
<head>
  <!-- Stylesheets -->
  <script type="application/json" id="noet-metadata">
    {{METADATA}}
  </script>
</head>
<body>
  <div class="noet-container">
    <header>
      <h1>{{TITLE}}</h1>
    </header>
    
    <nav class="noet-nav">
      <button id="nav-toggle" class="mobile-only">☰</button>
      <div id="nav-content">
        <!-- Generated by viewer.js -->
      </div>
      <div class="noet-nav-footer">
        <label for="theme-select">Theme:</label>
        <select id="theme-select">
          <option value="system">System</option>
          <option value="light">Light</option>
          <option value="dark">Dark</option>
        </select>
      </div>
    </nav>
    
    <main class="noet-content">
      <article>{{CONTENT}}</article>
    </main>
    
    <aside class="noet-metadata" id="metadata-panel" hidden>
      <button id="metadata-toggle" class="mobile-only">ℹ️</button>
      <div id="metadata-content">
        <!-- Populated by viewer.js -->
      </div>
    </aside>
    
    <footer>
      <!-- ... -->
    </footer>
  </div>
  
  <script type="module" src="assets/viewer.js"></script>
</body>
</html>
```

**Progressive Enhancement**:
- Without JS: `<nav>` and `<aside>` hidden via CSS, only `<article>` visible
- With JS: Panels populated and made visible, SPA navigation enabled

**Reading-Mode Layout**:
- Body content (`<main>`) stays centered on screen regardless of panel state
- When nav/metadata panels collapse: Content doesn't expand to fill space
- Maintains optimal reading width (prevents excessively long line lengths)
- Similar to reader modes in browsers (focus on content, not UI)

**Theme System**:
- Three-way dropdown in nav panel footer: System, Light, Dark
- **System**: Respects `prefers-color-scheme` media query (default)
- **Light**: Force light theme regardless of system preference
- **Dark**: Force dark theme regardless of system preference
- Preference saved in localStorage: `noet-theme` = `"system"` | `"light"` | `"dark"`

**Theme Implementation**:
```javascript
function applyTheme(preference) {
    if (preference === 'system') {
        const isDark = window.matchMedia('(prefers-color-scheme: dark)').matches;
        setTheme(isDark ? 'dark' : 'light');
    } else {
        setTheme(preference);
    }
    localStorage.setItem('noet-theme', preference);
}

function setTheme(theme) {
    if (theme === 'dark') {
        document.querySelector('#theme-light').disabled = true;
        document.querySelector('#theme-dark').disabled = false;
        document.documentElement.setAttribute('data-theme', 'dark');
    } else {
        document.querySelector('#theme-light').disabled = false;
        document.querySelector('#theme-dark').disabled = true;
        document.documentElement.setAttribute('data-theme', 'light');
    }
}

// Listen for system preference changes when in system mode
window.matchMedia('(prefers-color-scheme: dark)').addEventListener('change', (e) => {
    if (localStorage.getItem('noet-theme') === 'system') {
        setTheme(e.matches ? 'dark' : 'light');
    }
});
```

**Rationale**: Three-way dropdown provides explicit control while defaulting to system preference. Users can override system setting when needed (e.g., prefer dark docs even with light system theme).

## Metadata Panel Display

### Panel Behavior

**Desktop**:
- Fixed right sidebar
- Toggle button embedded in metadata panel
- Nav panel stays open during navigation

**Mobile**:
- Slide-up drawer from bottom
- Toggle button embedded in metadata panel
- **Nav panel auto-closes after navigation** (maximizes screen real estate)
- Metadata panel behavior follows user preference

**Default State**:
- **First visit**: Open on desktop (discoverability), closed on mobile (space constraints)
- **Return visits**: Remember last state in localStorage (power user efficiency)
- Small, unobtrusive toggle button for open/close

**Rationale**: 
- Discoverability: New users see metadata panel immediately, learn about feature
- Power users: Panel remembers their preference (no need to open every time)
- Mobile: Starts closed to maximize content area, but remembers if user opens it

**State Management**:
```javascript
// On page load
const savedState = localStorage.getItem('noet-metadata-panel-open');
if (savedState !== null) {
    metadataPanel.hidden = (savedState === 'false');
} else {
    // First visit - open on desktop, closed on mobile
    metadataPanel.hidden = isMobileViewport();
}

// On toggle
function toggleMetadataPanel() {
    metadataPanel.hidden = !metadataPanel.hidden;
    localStorage.setItem('noet-metadata-panel-open', !metadataPanel.hidden);
}
```

**Sticky Behavior**:
- If user opens panel: Stays open, content updates on navigation
- If user closes panel: Stays closed until explicit reopen
- State persists across page reloads (localStorage)

### Content Structure

**Data Source**: `wasm.get_context(bid)` returns `NodeContext`:
```rust
pub struct NodeContext {
    pub node: BeliefNode,           // The node itself
    pub home_path: Option<String>,  // Path in home network
    pub home_net: Option<Bid>,      // Home network BID
    pub related_nodes: Vec<BeliefNode>, // Related nodes from graph
    pub graph: BeliefGraph,         // Full graph with states and relations
}
```

**Metadata Sections** (Phase 1.1 Initial Scope):

1. **Node Properties** (from `context.node`):
   - BID (truncated with copy button: `1f100f54...`)
   - Title
   - Kind (Belief, Network, etc.)
   - Schema (if present)
   - Payload (key-value pairs from `node.document`)

2. **Location** (from `context.home_path`, `context.home_net`):
   - Home network name (resolve from `home_net` BID)
   - Path within network

3. **Backlinks** (from `context.graph.relations`):
   - Filter relations where current node is **sink** (target)
   - Group by WeightKind (Section, Epistemic, etc.)
   - Show source node title + link

4. **Forward Links** (from `context.graph.relations`):
   - Filter relations where current node is **source**
   - Group by WeightKind
   - Show sink node title + link

5. **Related Nodes** (from `context.related_nodes`):
   - Display nodes with strong relational connections
   - Useful for discovery ("nodes you might be interested in")

**Pass-Through Navigation Link**:
- Prominent button at top: "→ Navigate to [Title]"
- Single-click navigation from metadata panel (bypasses two-click pattern)
- Closes metadata panel after navigation

**Example Layout**:
```
┌──────────────────────────────┐
│ Metadata                 [×] │
├──────────────────────────────┤
│ [→ Navigate to Section Title]│ ← Pass-through link
│                              │
│ ■ Node Properties            │
│   BID: 1f100f54... [copy]    │
│   Title: Section Title       │
│   Kind: Belief               │
│   Schema: Section            │
│                              │
│ ■ Location                   │
│   Network: my-network        │
│   Path: /docs/guide.md       │
│                              │
│ ■ Backlinks (3)              │
│   Section (2)                │
│     • Parent Document        │
│     • Sibling Section        │
│   Epistemic (1)              │
│     • Referenced By          │
│                              │
│ ■ Forward Links (2)          │
│   Section (1)                │
│     • Child Section          │
│   Asset (1)                  │
│     • diagram.png            │
│                              │
│ ■ Related Nodes (5)          │
│   • Similar Topic A          │
│   • Similar Topic B          │
│   • ...                      │
│                              │
│ ■ Payload                    │
│   custom_field: "value"      │
│   tags: ["tag1", "tag2"]     │
│ ■ Payload               │
│   complexity: 3         │
│   priority: HIGH        │
└─────────────────────────┘
```

**Note**: All links in metadata drawer and nav panel bypass two-click pattern - single click navigates directly to target. Two-click pattern only applies to links in main content area (`<main class="noet-content">`).

### Special Namespace Rendering

Nodes in special namespaces require different rendering logic in the metadata panel.

#### Namespace Detection

Before rendering related nodes, check `home_net` against special namespace BIDs:

```javascript
const hrefNamespace = wasmModule.BeliefBaseWasm.href_namespace();
const assetNamespace = wasmModule.BeliefBaseWasm.asset_namespace();

// For each related node:
if (relatedNode.home_net === hrefNamespace) {
    // Render as external reference
} else if (relatedNode.home_net === assetNamespace) {
    // Render as asset with special handling
} else {
    // Render as normal document link
}
```

#### href_namespace Nodes (External References)

Rendered as clickable external links:
- Icon: 🔗
- Opens in new tab (`target="_blank" rel="noopener noreferrer"`)
- **No slash prefix** (paths are full URLs, not internal routes)
- On click: Shows metadata for the href node before opening

```html
<a href="https://example.com" class="noet-href-link" 
   data-bid="..." target="_blank">🔗 Example Site</a>
```

#### asset_namespace Nodes (Images/Assets)

Rendered as metadata-aware asset links:
- Icon: 📎
- Class: `.noet-asset-metadata-link`
- On click from metadata panel:
  1. Find matching image in content by src path
  2. Highlight the image wrapper (`.noet-link-selected`)
  3. Scroll to center the image
  4. Update metadata panel to show asset's metadata

```javascript
// In attachMetadataLinkHandlers()
assetLinks.forEach((link) => {
    link.addEventListener("click", (e) => {
        e.preventDefault();
        const targetBid = link.getAttribute("data-bid");
        const assetPath = link.getAttribute("data-asset-path");
        
        highlightAssetInContent(assetPath);
        showMetadataPanel(targetBid);
    });
});
```

#### Normal Document Nodes

Rendered as internal links with slash-prefixed paths:
- Navigates via hash routing (`/#/path/to/doc.html`)
- Single-click navigation from metadata panel

## Navigation Tree Generation

### Flat Map Data Structure

**Design Decision**: Use flat map with BID references instead of nested tree structure.

**Data Structure**:
```rust
pub struct NavTree {
    pub nodes: BTreeMap<String, NavNode>,  // Flat map: BID → NavNode
    pub roots: Vec<String>,                // Root BIDs (networks)
}

pub struct NavNode {
    pub bid: String,
    pub title: String,
    pub path: String,              // Normalized to .html extension
    pub parent: Option<String>,    // Parent BID for chain traversal
    pub children: Vec<String>,     // Child BIDs (not nested objects)
}
```

**Benefits of Flat Map**:
- O(1) lookup by BID for active node highlighting
- Trivial parent chain traversal (follow `parent` field)
- No recursive tree walking needed in JavaScript
- Simpler rendering logic (map children BIDs to nodes)
- Enables intelligent expand/collapse (expand ancestors, collapse siblings)

**Why Not Nested Tree?**
```javascript
// Nested approach (rejected): Deep traversal for active node
tree.nodes[0].children[0].children[0]  // O(n) search

// Flat map approach (implemented): Direct lookup
const node = tree.nodes[activeBid];    // O(1) lookup
const parent = tree.nodes[node.parent]; // O(1) parent access
```

### WASM API

```rust
#[wasm_bindgen]
impl BeliefBaseWasm {
    /// Get pre-structured navigation tree (flat map)
    pub fn get_nav_tree() -> JsValue;
}
```

### Algorithm: Stack-Based Tree Construction

**Data Source**: `PathMapMap` from `BeliefBase::paths()`
- Already ordered by `WEIGHT_SORT_KEY`
- Each path has depth information in `order_indices`

**Process**:
1. Iterate through `PathMapMap` entries (path, bid, order_indices)
2. For each entry:
   - `depth = order_indices.len()`
   - Pop stack until `stack.last().depth < depth` (find parent level)
   - Parent is `stack.last().bid` (or None for networks)
   - Extract title from `BeliefBase.states().get(bid)`
   - Normalize path to `.html` extension using `CODECS.extensions()`
   - Create `NavNode` with parent reference
   - Add node to flat map
   - Add node BID to parent's children list
   - Push `(bid, depth)` to stack
3. Result: Flat map where each node knows its parent and children (by BID)

**Title Extraction**:
- Network nodes: Use network title from state
- Document nodes: Use document title from state (not filename)
- Section nodes: Use heading text from state

**Path Normalization**:
- Convert `.md` paths to `.html` using codec extensions
- Preserve anchor fragments for sections (`doc.html#section-id`)
- Networks have empty path (no direct link)

**Complexity**: O(n) single pass through paths

### JavaScript Integration: Intelligent Expand/Collapse

**On Page Load**:
```javascript
// Get current document/section from URL or embedded data
const activeBid = document.body.dataset.bid;
const tree = beliefbase.get_nav_tree();

// Build parent chain (O(depth) where depth << n)
const chain = [];
let bid = activeBid;
while (bid) {
    chain.push(bid);
    bid = tree.nodes[bid].parent;
}

// Expand ancestors, collapse everything else
expandedSet.clear();
chain.forEach(bid => expandedSet.add(bid));

// Render tree with expand/collapse state
renderNavTree(tree, expandedSet);
```

**Active Node Highlighting**:
```javascript
// O(1) lookup for active node styling
const activeNode = tree.nodes[activeBid];
document.querySelector(`[data-bid="${activeBid}"]`).classList.add('active');
```

**Section-to-BID Mapping**:
- Each HTML document embeds `data-section-bids` attribute
- Maps section IDs to BIDs: `{"intro": "bid-123", "setup": "bid-456"}`
- On scroll or anchor navigation, lookup section BID and update active state

**Rationale**: Flat map structure optimized for interactive navigation patterns. Active node lookup and parent chain traversal are O(1) and O(depth) respectively, enabling responsive UI. PathMaps provide authoritative network structure; building nav from DOM would be document-centric and miss cross-document relationships.

### Collapsible Branches & Rendering

**Behavior**: Intelligent expand/collapse based on active document/section
- Active node's parent chain auto-expands
- Siblings (not in parent chain) collapsed by default
- User can manually toggle any branch
- Expand/collapse state stored in `Set` (ephemeral, per-session)

**Rendering Strategy**:
```javascript
function renderNavTree(tree, expandedSet) {
    const ul = document.createElement('ul');
    ul.className = 'noet-nav-tree';
    
    tree.roots.forEach(rootBid => {
        renderNode(tree.nodes[rootBid], tree, expandedSet, ul);
    });
    
    return ul;
}

function renderNode(node, tree, expandedSet, parentUl) {
    const li = document.createElement('li');
    li.dataset.bid = node.bid;
    
    // Toggle button if node has children
    if (node.children.length > 0) {
        const toggle = document.createElement('button');
        toggle.className = 'nav-toggle';
        toggle.textContent = expandedSet.has(node.bid) ? '▼' : '▶';
        toggle.onclick = () => toggleNode(node.bid, expandedSet);
        li.appendChild(toggle);
    }
    
    // Link to node (if it has a path)
    if (node.path) {
        const link = document.createElement('a');
        link.href = node.path;
        link.textContent = node.title;
        li.appendChild(link);
    } else {
        // Network node (no direct link)
        const span = document.createElement('span');
        span.textContent = node.title;
        li.appendChild(span);
    }
    
    // Render children if expanded
    if (expandedSet.has(node.bid) && node.children.length > 0) {
        const childUl = document.createElement('ul');
        node.children.forEach(childBid => {
            renderNode(tree.nodes[childBid], tree, expandedSet, childUl);
        });
        li.appendChild(childUl);
    }
    
    parentUl.appendChild(li);
}
```

**Unified Node Rendering**:
- Networks, documents, sections all use same `NavNode` structure
- Leaf nodes (sections) just have empty `children` array
- No artificial type distinctions in rendering logic

**Rationale**: Flat map enables efficient parent chain traversal for intelligent expand/collapse. Auto-expanding active branch provides context while keeping UI clean. State persistence not critical (ephemeral per-session preference, unlike theme which persists cross-session).

## Client-Side Document Fetching

### Fetch Strategy

**Document Loading**: Load on second click only (no preloading)
- First click shows metadata panel
- Second click fetches document HTML
- Show loading indicator during fetch (100-500ms typical)
- Simple implementation, no wasted bandwidth

**When user clicks internal document link** (second click):
1. Show loading indicator
2. Fetch HTML: `fetch(href)`
3. Parse response into DOM: `new DOMParser().parseFromString(html, 'text/html')`
4. Extract content: `doc.querySelector('.noet-content article')`
5. Extract metadata: `doc.querySelector('#noet-metadata')`
6. Replace current content:
   - Swap `<article>` content
   - Update `documentMetadata` global
   - Rebuild navigation tree
   - Update URL with `history.pushState()` (see URL Routing)
   - If anchor present, scroll to section
7. Hide loading indicator

**Rationale for no preloading**:
- Simplicity: No cache management, no background fetches
- Efficiency: Only fetch documents user actually navigates to
- Acceptable latency: 100-500ms load time reasonable for deliberate navigation
- Can optimize later if measurements show need

**Error Handling**:
- Network errors: Show error message, don't navigate
- Parse errors: Log to console, fallback to full page load
- Missing content: Warn user, offer "open in new tab"

**Future Optimization**:
- Add preloading if users complain about speed
- Cache fetched documents in memory
- Service worker for offline support

### URL Routing

**Strategy**: History API with pushState
- Use `history.pushState()` to update full URL path when navigating between documents
- Use URL hash for within-document section navigation: `doc.html#section-id`
- Browser back/forward works via `popstate` event
- Bookmarkable and shareable

**Document Navigation** (SPA):
```javascript
// Navigate to different document
function navigateToDocument(href, anchor = null) {
    // Update URL
    const newUrl = anchor ? `${href}#${anchor}` : href;
    history.pushState({ href, anchor }, '', newUrl);
    
    // Fetch and inject content
    fetchAndInjectDocument(href);
    
    // Scroll to anchor if present
    if (anchor) {
        setTimeout(() => {
            document.getElementById(anchor)?.scrollIntoView({ behavior: 'smooth' });
        }, 100); // Wait for DOM update
    }
}

// Handle browser back/forward
window.addEventListener('popstate', (e) => {
    if (e.state && e.state.href) {
        fetchAndInjectDocument(e.state.href);
        if (e.state.anchor) {
            document.getElementById(e.state.anchor)?.scrollIntoView({ behavior: 'smooth' });
        }
    }
});
```

**Section Navigation** (within same document):
```javascript
// Navigate to section in current document
function navigateToSection(sectionId) {
    const currentPath = window.location.pathname + window.location.search;
    history.pushState({ anchor: sectionId }, '', `${currentPath}#${sectionId}`);
    document.getElementById(sectionId)?.scrollIntoView({ behavior: 'smooth' });
    highlightNavItem(sectionId);
}

// Handle hash changes (for direct hash links)
window.addEventListener('hashchange', () => {
    const sectionId = window.location.hash.slice(1);
    if (sectionId) {
        document.getElementById(sectionId)?.scrollIntoView({ behavior: 'smooth' });
        highlightNavItem(sectionId);
    }
});
```

**Benefits**:
- Clean URLs: `https://example.com/docs/guide.html` and `https://example.com/docs/guide.html#installation`
- Full path updates on document navigation (SPA routing)
- Hash updates on section navigation (within document)
- Browser back/forward works automatically (popstate)
- Bookmarkable and shareable
- No backend needed: Works with static hosting (paths are actual files)

**Navigation Context**: Full nav tree with highlighted current section provides better context than breadcrumbs. Nav tree shows full hierarchy, siblings, and children - always visible without needing to scroll.

## Query Builder UI (Step 3)

**Deferred to separate session** - See ISSUE_38 Step 3 for details.

Brief overview:
- Visual expression builder (drag-and-drop predicates)
- Live preview of matching nodes
- Export to shareable query URL
- Integration with graph visualization

## Graph Visualization (Step 4)

**Deferred to separate session** - See ISSUE_38 Step 4 for details.

Brief overview:
- Force-directed graph using D3.js or Cytoscape.js
- Nodes colored by schema/kind
- Edges labeled by relation type
- Click node → Show metadata
- Zoom, pan, filter controls

## Accessibility

**Keyboard Navigation**:
- Tab through nav links, metadata panel, controls
- Enter/Space to activate links
- Escape to close panels
- Arrow keys for nav tree traversal

**Screen Readers**:
- ARIA labels on all interactive elements
- `aria-expanded` on collapsible sections
- `aria-current="page"` on active nav item
- Announce panel state changes

**Note**: Accessibility implementation relies on web development best practices and automated validation tools (see Testing section). Manual screen reader testing recommended but not required for initial implementation.

**Contrast**:
- WCAG AA compliance for light and dark themes
- High-contrast mode detection (future)

## Accessibility Testing

**Automated Tools** (Required):
- **Lighthouse**: Chrome DevTools accessibility audit (target score: 90+)
- **axe DevTools**: Browser extension for ARIA validation
- **WAVE**: Web accessibility evaluation tool

**Manual Testing** (Recommended):
- Keyboard-only navigation: Tab through all interactive elements
- Screen reader spot check (if available): VoiceOver (macOS), NVDA (Windows)
- Color contrast verification: Use Lighthouse or WebAIM contrast checker

**Reference Resources**:
- [MDN ARIA Guide](https://developer.mozilla.org/en-US/docs/Web/Accessibility/ARIA)
- [WCAG 2.1 AA Guidelines](https://www.w3.org/WAI/WCAG21/quickref/?versions=2.1&levels=aa)
- [WAI-ARIA Authoring Practices](https://www.w3.org/WAI/ARIA/apg/)

**Implementation Approach**: Follow established patterns from MDN and ARIA Authoring Practices. Validate with automated tools. Manual screen reader testing is valuable but not blocking for initial release.

## Performance Considerations

**Initial Load**:
- WASM module: ~200KB (compressed)
- BeliefGraph JSON: Varies by network size (10KB - 10MB+)
- viewer.js: ~15KB
- Total overhead: ~230KB + network data

**Optimization Strategies**:
1. **Lazy WASM loading**: Only load when metadata/query features used
2. **JSON streaming**: Parse beliefbase.json incrementally
3. **Virtual scrolling**: For large nav trees (>1000 items)
4. **Web Workers**: Offload WASM queries to background thread

**Target**:
- < 500ms to interactive (on 3G connection)
- < 100ms for navigation clicks
- < 50ms for metadata panel updates

## Security

**XSS Prevention**:
- All user-generated content (titles, payloads) escaped before rendering
- CSP headers restrict inline scripts (future)
- WASM sandbox isolates query execution

**Data Privacy**:
- All processing client-side (no analytics by default)
- LocalStorage only for theme preference (no PII)
- External links clearly marked

## Browser Compatibility

**Minimum Supported**:
- Chrome/Edge 90+
- Firefox 88+
- Safari 14+
- Mobile browsers: iOS Safari 14+, Chrome Android 90+

**Required Features**:
- ES6 modules
- CSS Grid
- Fetch API
- WASM (WebAssembly)
- History API (pushState)

**Polyfills**: None planned (target modern browsers only)

## Future Enhancements

1. **Collaborative Features**:
   - Share annotations via URL fragments
   - Real-time presence indicators (with backend)
   
2. **Advanced Query**:
   - Save query presets
   - Query history
   - Export results to CSV/JSON

3. **Offline Support**:
   - Service worker for offline viewing
   - IndexedDB cache for large networks

4. **Customization**:
   - User CSS overrides
   - Plugin system for custom visualizations



## References

- [ISSUE_06: HTML Generation and Interactive Viewer](../project/completed/ISSUE_06_HTML_GENERATION.md) - WASM infrastructure
- [ISSUE_38: Interactive SPA Implementation](../project/ISSUE_38_INTERACTIVE_SPA.md) - Implementation plan
- [BeliefBase Architecture](./beliefbase_architecture.md) - Data model
- [Link Format Design](./link_format.md) - BID attribution system
- [Open Props](https://open-props.style/) - Design token system
- [WASM Bindgen](https://rustwasm.github.io/docs/wasm-bindgen/) - Rust ↔ JS bridge

## Change Log

### Version 0.3 (2025-02-04)
- Added Image Modal Integration
  - All images wrapped in `.noet-image-wrapper` divs during post-processing
  - Two-click pattern for images with `bref://` in title attribute
  - Full-screen modal with dark overlay, padded content, small close button
  - Close on button, overlay, or Escape key
  - Single-click modal for images without `bref://`
- Added Header Anchor Links with Section ID Resolution
  - Auto-generated anchor links (🔗) appended to all headers (`h1-h6`)
  - Font size: 60% of header text, hidden by default, visible on hover
  - Section IDs resolved to BIDs at document load time via PathMap lookup
  - WASM method `get_bid_from_id(net_bref, id)` returns `BidBrefResult` with both bid and bref
  - Anchor links set `title="bref://[bref]"` for two-click pattern support
  - First click shows section metadata, second click navigates
  - Section anchor hrefs use relative paths (template rewrites to hash routes)
- Added Section Navigation Highlighting
  - `navigateToSection()` applies `.noet-link-selected` class to target element
  - Uses same visual highlighting as two-click link selection
- Added Special Namespace Rendering
  - Detect `home_net` against `href_namespace()` and `asset_namespace()`
  - **href_namespace nodes**: Rendered as clickable external links (🔗)
    - Opens in new tab via `window.open()` with `noopener,noreferrer`
    - No slash prefix on paths (full URLs preserved)
    - First click shows metadata, second click opens in new tab
    - Detection in `handleContentClick()` prevents internal routing
  - **asset_namespace nodes**: Rendered as metadata-aware asset links (📎)
    - On click from metadata panel: highlights image in content + scrolls to center
    - Updates metadata panel to show asset's metadata
  - Normal document nodes continue with slash-prefixed internal routing
- Added BID Injection for Reliable Metadata Loading
  - Template modified: `<body data-document-bid="{{BID}}">`
  - Compiler injects BID during HTML generation (both immediate and deferred phases)
  - JavaScript extracts BID from `data-document-bid` attribute on page load
  - Eliminates path/extension mismatch issues (`.html` vs `.md`)
  - Fallback chain: `targetBid || documentBid || getBidFromPath(path)`
- Added Multi-Network Context Queries
  - WASM `get_context()` tries multiple network namespaces with fallback strategy
  - Helper function `extract_node_context()` immediately consumes `BeliefContext` to avoid borrow conflicts
  - First tries entry point network (fast path for regular content)
  - Falls back to `href_namespace()`, `asset_namespace()`, `buildonomy_namespace()`
  - Enables metadata display for special namespace nodes
  - Returns null only after trying all known namespaces
- Added BidBrefResult Struct
  - WASM-exposed struct stores single `Bid`, computes both string representations on demand
  - Getters: `bid()` returns full BID string, `bref()` returns compact bref
  - Static method `from_bid_string()` parses BID strings with validation
  - Used for section ID resolution and entry point representation
- Refactored Entry Point Handling
  - Templates store only BID string (not full BeliefNode JSON)
  - `template-responsive.html`: `<script id="noet-entry-bid">"{{BID}}"</script>`
  - BeliefBaseWasm stores entry_point_bid internally
  - Access via `beliefbase.entryPoint()` getter (returns BidBrefResult)
  - Removed separate JavaScript entryPoint variable
  - Fallback: Finds first Network node if script tag missing (static pages)

### Version 0.2 (2025-02-04)
- Expanded Two-Click Navigation Pattern section with full implementation details
  - Scope: Links within `<article>` only (not nav/metadata/header)
  - State management: Single `selectedBid` variable
  - First click: Navigate/scroll behavior for internal/anchor/external links
  - Second click: Show metadata panel with full `NodeContext`
  - Click reset scenarios documented
  - Visual feedback patterns specified
- Expanded Metadata Panel Display section with NodeContext integration
  - Data source: `wasm.get_context(bid)` returns complete `NodeContext`
  - Five display sections: Node Properties, Location, Backlinks, Forward Links, Related Nodes
  - Pass-through navigation link for single-click access from metadata panel
  - Example layout with all sections detailed
- Document Fetching Strategy clarified
  - Fetch full HTML documents (191-line template overhead acceptable)
  - DOM parser extraction of `<article>` content
  - Rationale: Simplicity over fragment endpoint complexity
- Scoped to Phase 1 features only
  - Query Builder extracted to ISSUE_41
  - Graph Visualization extracted to ISSUE_42

### Version 0.1 (2025-02-03)
- Initial design document
- Defined progressive enhancement architecture
- Specified NodeContext structure for WASM (HashMap-based, no lifetimes)
- Established standard paths for assets (WASM embedded in binary)
- Documented two-click navigation pattern (all link types)
- Removed layout system (single template with JS enhancement)
- Integrated all design decisions into main sections (removed "Open Questions" appendix)
- Decisions documented:
  - Navigation tree: Collapsible, no persistence
  - URL routing: Hash-based (#section-id)
  - Mobile nav: Auto-close after navigation
  - Metadata panel: Open by default (desktop), remember state (localStorage)
  - External links: Show metadata first, open in new tab on second click
  - Document preloading: Load on second click only (no preloading)
  - Reading-mode layout: Content stays centered regardless of panel state
  - Toggle buttons: Embedded in panels (not header)
  - Pass-through links: Metadata drawer includes direct navigation link
  - Namespace functions: Exposed via WASM for link classification
  - Accessibility: Automated tools + established patterns (Lighthouse, axe, WAVE)