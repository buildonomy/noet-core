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
- Open Props design tokens
- Standard browser APIs (no dependencies)

**Interactive Layer**:
- WASM module (`noet_core.wasm`) compiled from Rust
- JavaScript viewer (`viewer.js`) for DOM manipulation
- BeliefGraph JSON (`beliefbase.json`) for full network data

**Key Design Decision**: No JavaScript frameworks (React/Vue/etc). Pure DOM manipulation keeps bundle size minimal and avoids framework lock-in.

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

**Note**: See `tests/browser/test-output/beliefbase.json` for complete real-world example.

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

**First Click** (any link):
- **Internal doc**: Fetch HTML, inject `<article>` into current page
- **Anchor**: Scroll to section, highlight
- **External link**: Show metadata panel (analyze link frequency/context)
- Track `selectedNodeBid = link.bid`

**Second Click** (same link):
- **Internal doc/Anchor**: Show metadata panel with full `NodeContext` from WASM
- **External link**: Open in new tab (after reviewing metadata)
- **Asset link** (`<a href="image.png">`): Download or open asset
- Metadata panel includes pass-through link to clicked element (single-click navigation)

**Not Intercepted**:
- `<img src>` - Images load normally on page load
- `<script src>` - Scripts load normally
- `<link href>` - Stylesheets load normally
- Only `<a>` element clicks are intercepted for two-click pattern

**Different Link Click**:
- Navigate/scroll to new target
- If metadata panel open: Update content (sticky behavior)
- If metadata panel closed: Stay closed

**Rationale**: Two-click pattern reduces modal spam while keeping metadata accessible. Sticky panel shows user intent to explore metadata.

**Link Interception Scope**:
- **Main content area** (`<main class="noet-content">`): Two-click pattern applies
- **Nav panel links**: Single-click navigation (bypass two-click)
- **Metadata panel links**: Single-click navigation (bypass two-click)
- **Header/footer links**: Single-click navigation (bypass two-click)

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

**Metadata Sections**:
1. **Node Info**: BID, title, schema, kind
   - **Pass-through link**: Direct link to node (single click navigation from metadata drawer)
2. **Network**: Home network, path
3. **Relations**: 
   - Sources (backlinks): Nodes linking here
   - Sinks (forward links): Nodes this links to
   - Grouped by WeightKind (Section, Epistemic, etc.)
4. **Payload**: Key-value display of custom fields

**Example Layout**:
```
┌─────────────────────────┐
│ Metadata            [×] │
├─────────────────────────┤
│ ■ Node                  │
│   BID: 1f100f54-...     │
│   Title: Section Title  │
│   [→ Navigate to Node]  │ ← Pass-through link
│   Schema: Section       │
│   Kind: Belief          │
│                         │
│ ■ Network               │
│   Home: my-network      │
│   Path: /docs/guide     │
│                         │
│ ■ Relations (3)         │
│   ▸ Section (2)         │
│     → Parent Document   │
│     → Sibling Section   │
│   ▸ Epistemic (1)       │
│     → Referenced By     │
│                         │
│ ■ Payload               │
│   complexity: 3         │
│   priority: HIGH        │
└─────────────────────────┘
```

**Note**: All links in metadata drawer and nav panel bypass two-click pattern - single click navigates directly to target. Two-click pattern only applies to links in main content area (`<main class="noet-content">`).

## Navigation Tree Generation

### Algorithm

**Data Source**: PathMaps from `BeliefBase::paths()` via WASM
- PathMaps provide network structure (documents, sections, hierarchy)
- Each PathMap represents a network's document tree
- Current document determines which PathMap to render and which branch to expand

**WASM Bindings Required**:
```rust
#[wasm_bindgen]
impl BeliefBaseWasm {
    /// Get all PathMaps (network structures)
    pub fn get_paths() -> JsValue;
    
    /// Get specific PathMap for a network
    pub fn get_path_for_network(net_bid: String) -> JsValue;
}
```

**Process**:
1. Load PathMaps from WASM on page initialization
2. Identify current document's network BID
3. Render PathMap for that network as nav tree
4. Expand/collapse branches based on current document focus
5. Highlight current document/section in tree

**PathMap Structure** (from `src/paths.rs`):
- Hierarchical document structure for a network
- Contains: Documents, sections, relationships
- Ordered by relation weight (sort_key)
- Already structured as tree (no need to parse headings)

**Output**: Hierarchical nav structure from PathMap
```javascript
// PathMap serialized to JavaScript
{
  network_bid: "network-bid-here",
  documents: [
    {
      bid: "doc-bid",
      title: "Document Title",
      path: "/path/to/doc",
      sections: [
        {
          bid: "section-bid",
          title: "Section Title",
          id: "section-id",
          subsections: [...]
        }
      ]
    }
  ]
}
```

**Rendering**:
- Nested `<ul>` structure from PathMap hierarchy
- Collapse/expand buttons for document branches
- Expand current document's branch automatically
- Click handlers for two-click pattern (single-click in nav panel)
- Active document/section highlighting (`.active` class)

**Rationale**: PathMaps provide authoritative network structure. Building nav from DOM headings would be document-centric (misses cross-document hierarchy) and wouldn't reflect network relationships.

### Collapsible Branches

**Behavior**: Nav tree branches (documents with sections) are collapsible
- Expand/collapse buttons for document branches
- Current document's branch auto-expands on page load
- Other documents collapsed by default (cleaner view)
- No state persistence needed (ephemeral per-session preference)

**Implementation**:
```html
<ul class="noet-nav-tree">
  <li class="nav-document">
    <button class="nav-toggle" aria-expanded="true">▼</button>
    <a href="document.html">Document Title</a>
    <ul class="nav-sections">
      <li><a href="document.html#section-1">Section 1</a></li>
      <li><a href="document.html#section-2">Section 2</a></li>
    </ul>
  </li>
  <li class="nav-document">
    <button class="nav-toggle" aria-expanded="false">▶</button>
    <a href="other.html">Other Document</a>
    <ul class="nav-sections" hidden>
      <!-- Collapsed -->
    </ul>
  </li>
</ul>
```

**Focus Behavior**:
- On page load: Expand current document's branch, collapse others
- On navigation: Expand new document's branch, collapse previous
- User can manually expand/collapse any branch

**Rationale**: Collapsible branches improve navigation of large networks without cluttering screen. Auto-expanding current document provides context. State persistence not critical since collapse state is per-session preference, unlike theme which is cross-session preference.

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