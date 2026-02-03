# Issue 38: Interactive SPA for HTML Viewer

**Priority**: HIGH  
**Estimated Effort**: 15-20 days (multi-session)  
**Dependencies**: ISSUE_06 (✅ Complete - WASM infrastructure)  
**Status**: IN PROGRESS

---

> **Note**: This is an **implementation planning document**. For architecture and design decisions, see [Interactive Viewer Design](../design/interactive_viewer.md). This issue tracks tasks, progress, and success criteria only.

---

## Progress Tracker

### Completed (Session 5 - 2026-02-04)
- ✅ **Step 0**: Planning & Cleanup (Session 1-2)
- ✅ **Step 1**: Responsive Layout Foundation (Session 2-3)
- ✅ **Step 2 Phase 1**: WASM NodeContext Binding (Session 4, ~2 hours)
  - NodeContext struct with BTreeMap for O(1) BID lookups
  - get_context() method with related_nodes collection (all connected nodes)
  - Namespace getter functions (href/asset/buildonomy)
  - Documentation updates (architecture.md, beliefbase_architecture.md, interactive_viewer.md)
  - Browser tests (Test 9: Namespaces, Test 10: NodeContext)
- ✅ **Step 2 Phase 1.5**: WASM Build Automation & Embedding (Session 4, ~1 hour)
  - build.rs automated wasm-pack compilation
  - WASM artifacts embedded in binary (~2.8MB total)
  - Extraction during HTML generation
  - Feature gating on 'bin' feature (correct anchor, not 'service' or 'wasm')
  - Cross-platform build tested (lib-only skips WASM, bin includes it)
- ✅ **Step 2 Phase 2.1-2.5**: Full Interactive Navigation (Session 5, ~3 hours)
  - get_paths() WASM method for navigation tree data
  - Removed layout infrastructure (always use responsive template)
  - Template updates: toggle buttons, footer positioning, theme selector (System/Light/Dark)
  - Link normalization for all codec extensions (.md/.toml/.org → .html)
  - WASM loading with error handling (theme works even if WASM fails)
  - Navigation tree generation with collapsible sections
  - Fixed syntax errors, Map iteration, WASM constructor usage
  - MetaMask/SES compatibility verified

### In Progress
- 🚧 **Step 2 Phase 2.6-2.7**: Two-Click Navigation + Metadata Panel (next session)

### Remaining
- ⏳ **Step 3**: Query Builder UI
- ⏳ **Step 4**: Force-Directed Graph Visualization
- ⏳ **Step 5**: Polish + Testing

**Estimated Progress**: ~40% complete (7.5 of ~18 days spent)
**Viewer Status**: ✅ FUNCTIONAL - Navigation tree, theme switching, responsive layout all working

---

## Summary

Build a progressive-enhancement Single-Page Application (SPA) for Noet-generated HTML documents. The viewer provides rich navigation, metadata exploration, graph visualization, and query interfaces - all client-side using WASM.

**Core Principle**: Same HTML works with or without JavaScript:
- **No JS**: Clean, readable article with standard links
- **With JS**: Interactive SPA with panels, metadata, client-side routing

## Goals

1. ✅ **Responsive Layout**: CSS Grid-based three-column design (complete)
2. 🚧 **Two-Click Navigation**: Link activation → metadata, second click → navigate
3. ⏳ **Metadata Display**: Node context, relations, network info via WASM
4. ⏳ **Query Builder**: Visual Expression constructor
5. ⏳ **Graph Visualization**: Force-directed graph with WeightKind gravity
6. ⏳ **Progressive Enhancement**: Static HTML degrades gracefully

## Architecture

**See**: [Interactive Viewer Design](../design/interactive_viewer.md) for complete architecture documentation.

**Key Decisions**:
- Single HTML template (progressive enhancement, no layout switching)
- Standard asset paths (`assets/`, `beliefbase.json` at root)
- WASM-powered metadata via `BeliefBaseWasm.get_context()`
- Two-click navigation pattern (activate → show metadata, click again → navigate)
- Client-side document fetching for SPA routing

## Implementation Steps

### Step 0: Planning & Cleanup ✅ COMPLETE

**Tasks**:
- [x] Create design doc (`docs/design/interactive_viewer.md`)
- [x] Remove layout system (Simple vs Responsive templates)
- [x] Define standard paths for assets
- [x] Document NodeContext structure for WASM
- [x] Update ISSUE_38 to reference design doc

**Deliverables**:
- Design doc with architecture decisions
- Cleaned up codebase (no layout enum)
- Clear scope for remaining steps

**Session**: Session 3 (2025-02-03)

---

### Step 1: Responsive Layout Foundation ✅ COMPLETE

**Tasks**:
- [x] Create CSS Grid layout (three-column responsive)
- [x] Add Open Props design tokens
- [x] Create light/dark themes with CSS custom properties
- [x] Build HTML template structure
- [x] Embed assets in binary with optional CDN mode
- [x] Add theme switcher to viewer.js skeleton
- [x] CLI flags: `--cdn` for CDN mode

**Deliverables**:
- `assets/noet-layout.css` (~16KB)
- `assets/noet-theme-light.css`, `assets/noet-theme-dark.css` (~8KB)
- `assets/template-responsive.html` (full SPA structure)
- `assets/viewer.js` (skeleton with theme switcher working)
- All assets embedded in binary (~65KB total)

**Sessions**: Session 1-2 (2025-02-02/03)

---

### Step 2: Two-Click Navigation + Metadata Panel (6-8 days)

**Goal**: Interactive navigation with metadata display powered by WASM.

#### Phase 2.1: get_paths() WASM Method ✅ COMPLETE (2026-02-04)

Added WASM binding to dump `PathMapMap` for navigation tree generation.

**Implementation**:
- Added `BeliefBaseWasm::get_paths()` method in `src/wasm.rs`
- Returns nested map: `network_bid → Vec<(path, bid, order_indices)>`
- Order indices from WEIGHT_SORT_KEY for document structure
- Added Test 11 in browser test runner
- Verified serialization and structure

**Example output**:
```javascript
const paths = beliefbase.get_paths();
// {
//   "network_bid": [
//     ["path/to/doc.md", "doc_bid", [0]],
//     ["path/to/doc.md#section", "section_bid", [0, 1]],
//     ...
//   ]
// }
```

#### Phase 2.2: Remove Layout Infrastructure ✅ COMPLETE (2026-02-04)

Simplified codebase to always use responsive template for interactive SPA.

**Changes**:
- Removed `--default-layout` CLI flag from `parse` and `watch` commands
- Removed `default_layout` parameter from `DocumentCompiler` and `WatchService`
- Removed `layout` field parsing from markdown frontmatter
- Always use `Layout::Responsive` template for document HTML generation
- Updated `DocCodec::generate_html()` trait signature (removed layout parameter)
- Updated all callers and tests

**Rationale**:
- Interactive SPA requires consistent structure across all documents
- `layout` field is obsolete - all documents use same interactive viewer
- Simplifies codebase and reduces CLI complexity
- Network index pages still use simple template (from `ProtoBeliefNode`)

**Files modified**:
- `src/codec/mod.rs` - Updated trait definition
- `src/codec/md.rs` - Removed layout parsing, always use responsive
- `src/codec/belief_ir.rs` - Updated signature
- `src/codec/compiler.rs` - Removed default_layout parameter
- `src/watch.rs` - Removed default_layout parameter
- `src/bin/noet/main.rs` - Removed CLI flags
- `tests/codec_test.rs` - Updated test calls

#### Phase 2.3: Template Updates + Toggle Buttons ✅ COMPLETE (2026-02-04)

Added mobile-first responsive behavior with toggle buttons for navigation and metadata panels.

**Template Changes** (`assets/template-responsive.html`):
- Added hamburger menu button (☰) to header for navigation toggle (mobile/tablet only)
- Added "Info" button to header for metadata toggle (mobile/tablet only)
- Added backdrop overlay for modal behavior on mobile
- Updated navigation structure to use `.noet-nav__content` wrapper
- Updated nav content to use `.noet-nav-tree` class
- Added inline JavaScript for panel toggle behavior:
  - Click hamburger → show/hide navigation panel
  - Click Info → show/hide metadata panel
  - Click backdrop → close all panels
  - Only one panel open at a time (mutually exclusive)

**CSS Updates** (`assets/noet-layout.css`):
- Added `.noet-header-left` and `.noet-header-right` layout containers
- Added `.noet-site-title` styling (responsive font size)
- Added `.noet-theme-toggle` styling for theme switcher button
- Added `.theme-icon` styling for moon/sun emoji
- Fixed indentation (spaces → consistent formatting)

**Responsive Behavior**:
- **Mobile/Tablet** (< 1024px):
  - Navigation: Off-canvas slide-in from left (280px wide)
  - Metadata: Bottom drawer slide-up (60vh height, max 600px)
  - Toggle buttons visible in header
  - Backdrop overlay when panels open
  - Smooth transitions with `ease-out-3` timing
- **Desktop** (≥ 1024px):
  - Navigation: Always visible sidebar (280px)
  - Metadata: Always visible sidebar (320px)
  - Toggle buttons hidden
  - Three-column grid layout: nav | content | metadata

**Files Modified**:
- `assets/template-responsive.html` - Toggle buttons, panel structure, JavaScript
- `assets/noet-layout.css` - Header styling, theme toggle, formatting

**Testing**:
- ✅ HTML generation works (no template errors)
- ✅ Toggle buttons present in generated HTML
- ✅ Panel toggle JavaScript embedded correctly
- ✅ CSS classes match template structure
- Ready for manual browser testing in next phase

#### Phase 2 Fix: Rewrite All Codec Extensions to .html ✅ COMPLETE (2026-02-04)

Fixed link rewriting to handle all registered codec extensions, not just `.md`.

**Problem**: 
- HTML generation only rewrote `.md` → `.html`
- Other codec extensions (`.toml`, `.org`, etc.) were left unchanged
- Unresolved links (missing `bref://`) weren't rewritten at all

**Solution** (`src/codec/md.rs`):
- Updated `rewrite_md_links_to_html()` to dynamically check all registered codec extensions
- Uses `CODECS.extensions()` to get list of registered extensions
- Rewrites both resolved links (with `bref://`) AND unresolved links (graceful degradation)
- Handles both `.ext` at end and `.ext#anchor` with fragment

**Design Decision**:
- Link normalization is the **responsibility of each DocCodec implementation**
- Cannot be abstracted to trait level because:
  - String-based post-processing risks matching `.md` in code blocks (unsafe)
  - Event-based processing is codec-specific (pulldown-cmark for markdown)
- Documented as **requirement** in `DocCodec::generate_html()` trait docs
- Future codecs (Org, RST, etc.) must implement their own link normalization

**Example**:
```html
<!-- Before -->
<a href="/net1_dir1/hsml.md#definition">Link</a>
<a href="document.org#section">Link</a>

<!-- After -->
<a href="/net1_dir1/hsml.html#definition">Link</a>
<a href="document.html#section">Link</a>
```

**Testing**:
- ✅ `.md` → `.html` conversion works
- ✅ Unresolved links converted (e.g., reference-style links)
- ✅ Anchor fragments preserved (`.md#anchor` → `.html#anchor`)
- ✅ All codec extensions supported dynamically

**Files Modified**:
- `src/codec/mod.rs` - Documented link normalization requirement in trait
- `src/codec/md.rs` - Updated `rewrite_md_links_to_html()` function with requirement notes

**Note**: Absolute paths (e.g., `/net1_dir1/...`) are preserved as-is. These may need relative path conversion in future work if they cause navigation issues.

#### Phase 2.4: Load WASM + BeliefGraph ✅ COMPLETE (2026-02-04)

Added WASM module loading and BeliefBase initialization to viewer.js.

**Implementation** (`assets/viewer.js`):
- Added `initializeWasm()` async function:
  - Dynamically imports WASM module (`./noet_core.js`)
  - Fetches and parses `beliefbase.json`
  - Initializes `BeliefBaseWasm` from JSON
  - Calls `get_paths()` to retrieve navigation data
  - Builds navigation tree on success
- Added global state variables:
  - `wasmModule` - WASM module instance
  - `beliefbase` - BeliefBaseWasm instance
  - `pathsData` - Navigation tree data (network_bid → paths)
- Error handling with user-friendly messages in nav panel
- Integration with DOMContentLoaded lifecycle

**Testing**:
- ✅ WASM module loads successfully
- ✅ beliefbase.json fetched and parsed
- ✅ `get_paths()` returns navigation data
- ✅ No console errors on initialization
- Ready for navigation tree rendering

#### Phase 2.5: Build Navigation Tree ✅ COMPLETE (2026-02-04)

Implemented hierarchical navigation tree generation from flat paths data.

**Implementation** (`assets/viewer.js`):
- `buildNavigation()` - Main entry point:
  - Filters out system namespaces (href, asset, buildonomy)
  - Selects primary network for display
  - Builds tree structure and renders to HTML
- `buildTreeStructure(paths)` - Hierarchical tree builder:
  - Sorts paths by order_indices (WEIGHT_SORT_KEY)
  - Identifies documents vs sections (based on `#` in path)
  - Nests sections under parent documents
  - Converts `.md` paths to `.html` hrefs
- `extractTitle(path)` - Display name extraction:
  - Sections: anchor text with cleanup (dashes → spaces)
  - Documents: filename without extension
- `renderTree(nodes, level)` - Recursive HTML renderer:
  - Collapsible toggle buttons for nodes with children
  - Links with `data-bid` and `data-path` attributes
  - Nested `<ul>` structure for hierarchical display
- `handleNavClick(event)` - Delegated event handler:
  - Toggle button clicks: expand/collapse children (▾/›)
  - Navigation link clicks: logged for now (Phase 2.6 will add client-side fetching)

**CSS Updates** (`assets/noet-layout.css`):
- `.noet-nav-tree__toggle` - Collapsible toggle button styling
- `.noet-nav-tree__children` - Hidden by default, shown when `.is-expanded`
- `.noet-nav-tree__item.is-expanded` - Rotates toggle arrow (›  → ▾)
- Proper flexbox layout for toggle + link alignment
- Indentation with `padding-left` for nested levels

**Features**:
- ✅ Hierarchical document tree with sections nested under documents
- ✅ Collapsible sections with smooth expand/collapse
- ✅ Clean title extraction from file paths
- ✅ Links ready for navigation (`.md` → `.html`)
- ✅ Data attributes for future two-click pattern
- ✅ Responsive design (works in nav panel and drawer)

**Files Modified**:
- `assets/viewer.js` - WASM loading + tree generation (~200 lines added)
- `assets/noet-layout.css` - Collapsible tree styling (~40 lines updated)

**Testing**:
- ✅ HTML generation successful
- ✅ Navigation tree JavaScript embedded correctly
- ✅ WASM files and beliefbase.json extracted to output
- Ready for manual browser testing with HTTP server

#### Phase 1: WASM NodeContext Binding ✅ COMPLETE (2026-02-04)

**Implementation Notes**:
- Most WASM methods already implemented in `src/wasm.rs` (from ISSUE_06)
- Need to add: `get_context()` and namespace helper functions
- Existing methods: `get_by_bid()`, `get_backlinks()`, `get_forward_links()`, `search()`, `query()`

**Tasks**:
- [x] Add `NodeContext` struct to `src/wasm.rs`:
  ```rust
  pub struct NodeContext {
      pub node: BeliefNode,
      pub home_path: String,
      pub home_net: Bid,
      pub related_nodes: BTreeMap<Bid, BeliefNode>,  // All connected nodes (O(1) lookup)
      pub graph: HashMap<WeightKind, (Vec<Bid>, Vec<Bid>)>,  // Sorted by WEIGHT_SORT_KEY
  }
  ```
- [x] Add namespace helper functions (static, not instance methods):
  ```rust
  pub fn href_namespace() -> String;
  pub fn asset_namespace() -> String;
  pub fn buildonomy_namespace() -> String;
  ```
- [x] Implement `BeliefBaseWasm::get_context(bid: String) -> JsValue`
- [x] Handle lifetime bounds (serialize immediately)
- [x] Compile WASM module with `wasm-pack build`
- [x] Test in browser console (tests added to test_runner.html)

**Deliverables**:
- `src/wasm.rs` updated with `get_context()` + namespace functions ✅
- `pkg/noet_core.js` + `pkg/noet_core_bg.wasm` regenerated ✅
- Browser tests added (Test 9: Namespaces, Test 10: NodeContext) ✅
- Documentation updated:
  - `docs/design/architecture.md` - Added § 10 (System Network Namespaces) ✅
  - `docs/design/beliefbase_architecture.md` - Expanded § 2.7 ✅
  - `docs/design/interactive_viewer.md` - Added cross-references ✅

**Time Spent**: ~2 hours (Session 4, 2026-02-04)

**Reference**: Design doc § WASM Integration, `.scratchpad/SESSION_4_COMPLETE.md`

---

#### Phase 1.5: WASM Build Automation & Embedding ✅ COMPLETE (2026-02-04)

**Goal**: Automate WASM compilation and embed artifacts in binary for offline-first deployment.

**Context**: WASM must be compiled with different features than the main build:
- Main build: `--features bin` (includes service, CLI deps)
- WASM build: `--features wasm --no-default-features` (minimal for browser)

**Tasks**:
- [x] Create `build.rs` to run `wasm-pack build` before compilation
  - Triggers when `bin` feature is enabled (not `service` or `wasm`)
  - Checks if `wasm-pack` is available
  - Runs: `wasm-pack build --target web -- --features wasm --no-default-features`
  - Skips rebuild if `pkg/` artifacts already exist (incremental)
  - Clear error message if `wasm-pack` not installed
- [x] Update `src/codec/assets.rs` to embed WASM artifacts
  - Add `pkg/noet_core.js` via `include_bytes!` (32KB)
  - Add `pkg/noet_core_bg.wasm` via `include_bytes!` (2.2MB)
  - Feature-gated on `#[cfg(feature = "bin")]`
- [x] Update `extract_assets()` to write WASM files to output
  - Extracts to `{output_dir}/assets/noet_core.js`
  - Extracts to `{output_dir}/assets/noet_core_bg.wasm`
  - Binary integrity preserved (raw bytes)
  - Always extracts when `bin` feature enabled

**Deliverables**:
- `build.rs` - Automated WASM compilation ✅
- `src/codec/assets.rs` - WASM embedding with `include_bytes!` ✅
- `pkg/` generated automatically on `cargo build` (default/bin) ✅
- Binary includes WASM (~2.8MB total: 2.2MB WASM + 32KB JS + base)
- Library-only build (`--no-default-features --lib`) skips WASM ✅

**Binary Size Impact**:
- Before: ~500KB (core + CSS/JS)
- After: ~2.8MB (core + CSS/JS + WASM)
- Acceptable for offline-first deployment model

**Feature Resolution**:
- `bin` feature triggers WASM build (correct anchor point)
- `service` alone does NOT trigger WASM (library use case)
- `wasm` feature only for compiling WASM target itself
- Build script compiles WASM with `--features wasm --no-default-features`

**Testing**:
- [x] Default build: `cargo build` includes WASM ✅
- [x] Library build: `cargo build --no-default-features --lib` skips WASM ✅
- [x] HTML generation: `noet parse tests/network_1 --html-output test-output/` extracts WASM ✅
- [x] Incremental: WASM only rebuilds when sources change ✅
- [x] Browser tests: `./tests/browser/run.sh` uses standardized `tests/browser/test-output/` ✅

**Time Spent**: ~1 hour (Session 4, 2026-02-04)

**Reference**: `.scratchpad/SESSION_4_COMPLETE.md` for implementation details

---

#### Phase 2.6: Metadata Panel Display (Next - 1-2 days estimate)

**Goal**: Show backlinks, forward links, metadata for current node

**Tasks**:
- [ ] Call `beliefbase.get_context(bid)` for current document
- [ ] Display backlinks (nodes linking TO this document)
- [ ] Display forward links (nodes this document links TO)
- [ ] Display node metadata (BID, title, home_path, home_net)
- [ ] Group relations by WeightKind
- [ ] Clickable links to navigate to related nodes
- [ ] Sticky panel behavior (remembers open/closed state)
- [ ] Format cleanly (sections, headers, readable layout)
- [ ] Responsive (works in drawer and sidebar)

**Reference**: Design doc § Metadata Panel Display

**Testing approach**: Test standalone by calling `showMetadataPanel(bid)` from browser console before wiring up two-click navigation

---

#### Phase 2.7: Two-Click Navigation Pattern (After 2.6 - 2-3 days estimate)

**Goal**: First click = preview/navigate, second click = action (metadata/open)

**Tasks**:
- [ ] Intercept `<a>` clicks in main content ONLY (not nav/metadata panels)
- [ ] Extract BID from `title="bref://[bref]"` attribute
- [ ] Detect link type (internal document, external URL, asset, anchor)
- [ ] Implement first click: Show metadata preview in panel (NO navigation/scroll)
- [ ] Implement second click: Navigate to document OR open in new tab (external)
- [ ] Client-side document fetching (AJAX, replace `<article>` content)
- [ ] URL routing with `history.pushState()` (full path + hash)
- [ ] Handle browser back/forward with `popstate` event
- [ ] Highlight active link with CSS class

**Reference**: Design doc § Two-Click Navigation Pattern, § URL Routing

**Note**: Nav and metadata panel links are single-click (bypass two-click pattern)

**Why after 2.6**: Metadata display must work before we can wire up clicks to show it. Build destination before route.

---

#### Step 2 Success Criteria

**Phase 2.1-2.5 Complete ✅**:
- [x] WASM module loads and queries BeliefGraph
- [x] Navigation tree generated from PathMaps (network structure)
- [x] Collapsible document branches with expand/collapse
- [x] Single-click navigation from nav tree
- [x] Theme dropdown in nav footer: System (default), Light, Dark
- [x] Mobile toggle buttons (nav/metadata panels)
- [x] Desktop sidebars visible by default

**Phase 2.6-2.7 Remaining** (in order):
- [ ] Metadata panel displays NodeContext (backlinks, forward links, relations) - **DO FIRST**
- [ ] Sticky panel behavior with localStorage
- [ ] Two-click pattern: First click = metadata preview (no scroll/navigation) - **DO SECOND**
- [ ] Two-click pattern: Second click = navigate to document
- [ ] Client-side routing with History API (second click only)
- [ ] External links: First click = metadata, second click = open in new tab
- [ ] Images/scripts load normally (not intercepted)

**Estimated Total**: 6-8 days across 3-4 sessions

---

### Step 3: Query Builder UI (4-5 days)

**Goal**: Visual interface for constructing Expression queries.

**Tasks**:
- [ ] Simple mode: Dropdown for common queries (Kind, Title, Backlinks)
- [ ] Advanced mode: Nested Expression builder
  - Dropdown for Expression type (StateIn, Dyad, etc.)
  - Dynamic form inputs based on selection
  - Validation for regex, BID format
- [ ] Text mode: JSON input for power users
- [ ] Execute query via `beliefbase.query(expr)`
- [ ] Display results in list view
- [ ] Option to visualize results in graph mode
- [ ] Save queries to LocalStorage

**Deliverables**:
- Query builder UI in header or collapsible panel
- Results display with node cards
- Integration with graph view (filter graph by query)

**Reference**: Design doc § Query Builder UI (Step 3)

---

### Step 4: Force-Directed Graph Visualization (3-4 days)

**Goal**: Full-page graph view with WeightKind-based layout.

**Tasks**:
- [ ] Choose library (D3.js or Cytoscape.js)
- [ ] Toggle button in header switches to graph mode
- [ ] Render nodes (colored by BeliefKind)
- [ ] Render edges (weighted by selected WeightKind)
- [ ] Force simulation with gravity:
  - Sources flow to sinks (bottom to top)
  - External sources at bottom
  - External sinks at top
- [ ] Interactions:
  - Two-click pattern (metadata on second click)
  - Hover shows title
  - Drag to reposition
  - Zoom and pan
- [ ] Filter graph by query results

**Deliverables**:
- Graph view toggle working
- Force-directed layout renders full network
- Node styling by kind/schema
- Two-click pattern integrated with graph

**Reference**: Design doc § Graph Visualization (Step 4)

---

### Step 5: Polish + Testing (2-3 days)

**Goal**: Refinements, accessibility, performance, edge cases.

**Tasks**:
- [ ] Keyboard navigation (Tab, Enter, Escape, Arrows)
- [ ] ARIA labels and screen reader support
  - Follow MDN ARIA Guide and WAI-ARIA Authoring Practices
  - `aria-label` on all interactive elements
  - `aria-expanded` on collapsible sections
  - `aria-current="page"` on active nav item
  - `role` attributes where semantic HTML insufficient
- [ ] Accessibility validation (automated tools - required):
  - [ ] Lighthouse accessibility audit (target score: 90+)
  - [ ] axe DevTools browser extension scan (0 violations)
  - [ ] WAVE accessibility checker (no errors)
  - [ ] Color contrast verification (WCAG AA compliance)
- [ ] Keyboard-only navigation test (manual - required)
- [ ] Screen reader spot check (manual - recommended but not blocking)
- [ ] Loading states (spinners for WASM/JSON fetch)
- [ ] Error messages (network errors, parse failures)
- [ ] Performance optimization:
  - Lazy WASM loading
  - Virtual scrolling for large nav trees
  - Throttle scroll events
- [ ] Browser testing (Chrome, Firefox, Safari, Mobile)
- [ ] Documentation updates (README, usage guide)

**Deliverables**:
- All browsers tested and working
- Accessibility validation complete:
  - Lighthouse score 90+ (automated)
  - axe DevTools 0 violations (automated)
  - Keyboard navigation verified (manual)
  - Screen reader spot check recommended (manual, not blocking)
- Performance benchmarks pass (< 500ms interactive, < 100ms navigation)
- User documentation with examples

**Note on Accessibility**: Implementation follows established patterns from MDN and WAI-ARIA Authoring Practices. Automated tools provide primary validation. Manual screen reader testing valuable but not required for initial release.

---

## Testing Requirements

### Manual Testing

**Browser Matrix**:
- Chrome/Edge 90+ (desktop + mobile)
- Firefox 88+ (desktop + mobile)
- Safari 14+ (desktop + iOS)

**Test Scenarios**:
1. Page loads without JS → article readable, links work
2. Page loads with JS → nav tree builds, theme switcher works
3. Click nav link once → scrolls to section, highlights
4. Click same link twice → metadata panel appears
5. Click different link → navigates, metadata updates if open
6. Metadata panel close button → hides panel
7. Mobile: Hamburger menu → nav slides in
8. Mobile: Metadata toggle → drawer slides up
9. Theme toggle → switches light/dark, persists in LocalStorage
10. External link → shows metadata, opens in new tab on third click (?)

### Performance

**Targets**:
- Initial load: < 500ms to interactive (3G connection)
- Navigation click: < 100ms response time
- Metadata fetch: < 50ms (WASM query)
- Graph render: < 1s for 1000 nodes

**Monitoring**:
- Use Chrome DevTools Performance tab
- Lighthouse audit for accessibility/performance
- Manual testing on throttled connection

---

## Success Criteria

**Step 2 Complete When**:
- [ ] Navigation tree generated and functional
- [ ] Two-click pattern working for all links
- [ ] Metadata panel displays NodeContext from WASM
- [ ] Client-side routing with URL updates
- [ ] Mobile responsive (nav/metadata toggle buttons)
- [ ] No console errors in supported browsers

**Full Issue Complete When**:
- [ ] All 5 implementation steps done
- [ ] Query builder functional
- [ ] Graph visualization working
- [ ] Accessibility audit passed
- [ ] Browser compatibility verified
- [ ] Documentation complete

---

## Risks

### Risk 1: Performance with Large Graphs
**Impact**: Slow rendering, poor UX for networks with >10K nodes  
**Mitigation**: 
- Virtual scrolling for nav tree
- Lazy load graph visualization
- Pagination for query results
- Web Workers for WASM queries

### Risk 2: Mobile UX Complexity
**Impact**: Cramped UI, difficult navigation on small screens  
**Mitigation**:
- Mobile-first design approach
- Touch-friendly tap targets (min 44px)
- Slide-in panels with backdrop
- Extensive mobile testing

### Risk 3: WASM Load Time
**Impact**: Slow initial page load, especially on slow connections  
**Mitigation**:
- Lazy load WASM only when interactive features needed
- Show loading indicators
- Fallback to static mode if WASM fails

### Risk 4: Browser Compatibility
**Impact**: Features broken in older browsers  
**Mitigation**:
- Target modern browsers only (documented minimum versions)
- Feature detection (check for WASM support)
- Graceful degradation to static HTML

---

## Resolved Design Decisions

All design decisions have been integrated into the [Interactive Viewer Design](../design/interactive_viewer.md) document. Key decisions:

- **Navigation Tree**: Built from PathMaps (network structure), collapsible branches, current document auto-expanded
- **URL Routing**: History API for document navigation (full path), hash for section navigation
- **Mobile Nav**: Auto-closes after navigation (maximizes screen real estate)
- **Metadata Panel**: Open by default (desktop), closed (mobile), remembers state with localStorage
- **External Links**: Show metadata first (link frequency analysis), open in new tab on second click
- **Theme System**: Three-way dropdown in nav footer (System/Light/Dark), System default
- **Link Interception**: Two-click pattern for main content only, single-click in nav/metadata panels
- **Document Preloading**: Load on second click only (no preloading)
- **Toggle Buttons**: Embedded in panels (not header)

See design doc for complete rationale and implementation details.

---

## References

- **[Interactive Viewer Design](../design/interactive_viewer.md)** - Complete architecture documentation
- [ISSUE_06: HTML Generation](completed/ISSUE_06_HTML_GENERATION.md) - WASM infrastructure (complete)
- [BeliefBase Architecture](../design/beliefbase_architecture.md) - Data model
- [Link Format Design](../design/link_format.md) - BID attribution
- [Open Props](https://open-props.style/) - Design tokens
- [WASM Bindgen](https://rustwasm.github.io/docs/wasm-bindgen/) - Rust ↔ JS bridge

---

## Notes

**Multi-Session Scope**: This issue spans 15-20 days across 6-8 sessions. Each step is a natural stopping point for review and testing.

**Progressive Enhancement**: Static HTML must work without JS. All interactive features are enhancements, not requirements.

**Standard Paths**: Assets at fixed locations eliminate configuration complexity. See design doc for details.

**Layout System Removed**: Earlier plan had Simple vs Responsive templates. Now single template with progressive JS enhancement (simpler, more maintainable).

**Design Doc as Authority**: [Interactive Viewer Design](../design/interactive_viewer.md) is the authoritative reference for architecture. This issue tracks implementation tasks and progress. When in doubt, consult design doc.

**Key Architectural Points**:
- Navigation from PathMaps (not DOM headings)
- Two-click pattern scope: main content only
- URL routing: History API (full path) + hash (sections)
- External links: Metadata first for frequency analysis
- Theme: Three-way dropdown (System/Light/Dark)