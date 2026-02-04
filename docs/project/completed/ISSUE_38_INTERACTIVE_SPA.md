# Issue 38: Interactive SPA for HTML Viewer - Foundation Phase

**Priority**: HIGH  
**Estimated Effort**: 8 days (actual)  
**Dependencies**: ISSUE_06 (✅ Complete - WASM infrastructure)  
**Status**: ✅ COMPLETE (2026-02-04)  
**Continuation**: ISSUE_39 (Two-Click Navigation + Advanced Features)

---

> **Note**: This is an **implementation planning document**. For architecture and design decisions, see [Interactive Viewer Design](../design/interactive_viewer.md). This issue tracks tasks, progress, and success criteria only.

---

## Completion Summary

**Deliverables**: Functional interactive HTML viewer with responsive layout, navigation tree, theme system, and WASM integration.

### Phase 0: Planning & Architecture ✅
- Created comprehensive design document ([interactive_viewer.md](../design/interactive_viewer.md))
- Removed layout system complexity (single responsive template)
- Defined standard asset paths and progressive enhancement strategy
- Established NodeContext structure for WASM bindings

### Phase 1: Responsive Foundation ✅
- CSS Grid three-column layout (nav | content | metadata)
- Mobile-first responsive breakpoints (< 1024px: drawers, ≥ 1024px: sidebars)
- Toggle buttons for mobile navigation/metadata panels
- Backdrop overlay for modal behavior
- Theme selector dropdown (System/Light/Dark) in navigation footer
- Footer positioning in grid layout

### Phase 2: WASM Integration & Navigation ✅
**Phase 2.1**: WASM Data Bindings
- `get_paths()` method returns navigation tree data (BTreeMap<network_bid, Vec<(path, bid, order_indices)>>)
- `get_context(bid)` returns NodeContext (related_nodes, graph structure, paths)
- Namespace getters (href/asset/buildonomy)
- Browser tests added (Test 9: Namespaces, Test 10: NodeContext, Test 11: get_paths)

**Phase 2.2**: Build System Automation
- build.rs compiles WASM automatically via wasm-pack
- WASM artifacts embedded in binary (~2.8MB)
- Feature-gated on 'bin' feature for conditional compilation
- Cross-platform tested (lib-only vs full binary builds)

**Phase 2.3**: Interactive Template
- Hamburger menu (☰) and Info buttons for mobile toggles
- Theme dropdown with localStorage persistence
- Real-time OS theme detection (MediaQuery listeners)
- Collapsible navigation tree with expand/collapse buttons (› / ▾)

**Phase 2.4**: Link Normalization
- All codec extensions (.md/.toml/.org) rewritten to .html
- Event-based processing in MdCodec (pulldown-cmark)
- Handles both resolved (bref://) and unresolved links
- Documented as codec responsibility in trait

**Phase 2.5**: Client-Side Tree Generation
- JavaScript builds hierarchical tree from flat paths
- Sorts by WEIGHT_SORT_KEY order indices
- Recursive rendering with proper nesting
- Delegated event handling for toggles/navigation
- MetaMask/SES compatibility verified

### Files Modified (8 total)
1. `src/wasm.rs` - Added get_paths() method
2. `src/codec/mod.rs` - Removed layout parameter, documented link normalization
3. `src/codec/md.rs` - Implemented link rewriting for all codecs
4. `src/codec/belief_ir.rs` - Updated signature
5. `src/codec/compiler.rs` - Removed layout infrastructure
6. `src/watch.rs` - Removed layout parameter
7. `src/bin/noet/main.rs` - Removed CLI flags
8. `tests/codec_test.rs` - Fixed test calls

### Assets Modified (3 total)
1. `assets/template-responsive.html` - Toggle buttons, theme selector, footer
2. `assets/noet-layout.css` - Grid layout, theme styles, collapsible tree
3. `assets/viewer.js` - WASM loading, tree generation, theme system (~300 lines)

### Testing Completed
- ✅ All unit tests passing
- ✅ WASM compilation successful
- ✅ Browser tests 9-11 added and passing
- ✅ Manual testing: theme switching, navigation tree, mobile behavior
- ✅ MetaMask/browser extension compatibility verified

### Current State
**What Works**:
- Responsive layout (mobile drawers, desktop sidebars)
- Navigation tree with collapsible sections
- Theme switching with persistence (System/Light/Dark)
- WASM loading with error handling
- Link normalization across all codecs
- Progressive enhancement (works without JS)

**Known Limitations** (addressed in ISSUE_39):
- No two-click navigation pattern yet (single-click navigates immediately)
- No metadata panel display (panel exists but empty)
- No client-side routing (full page reloads)
- No query builder UI
- No graph visualization

---

## Original Goals (Foundation Phase Complete)

This issue established the **foundation** for an interactive SPA viewer. All core infrastructure is now in place:

1. ✅ **Responsive Layout**: CSS Grid with mobile/desktop breakpoints
2. ✅ **Navigation Tree**: WASM-powered hierarchical document navigation
3. ✅ **Theme System**: System/Light/Dark with persistence
4. ✅ **Progressive Enhancement**: Works without JavaScript
5. ✅ **WASM Integration**: BeliefBase queries, path mapping, context retrieval
6. ✅ **Build Automation**: WASM compilation embedded in binary

**Remaining Goals** moved to ISSUE_39:
- Two-click navigation pattern
- Metadata panel display (backlinks/forward links)
- Client-side routing with History API
- Query builder UI
- Force-directed graph visualization

## Architecture Reference

**Complete documentation**: [Interactive Viewer Design](../design/interactive_viewer.md)

**Key Architectural Decisions Implemented**:
- Single responsive template (no layout switching)
- Standard asset paths (`./assets/`, `./beliefbase.json`)
- WASM bindings for navigation (`get_paths()`) and metadata (`get_context()`)
- Progressive enhancement (static HTML → interactive with JS)
- Event-based link normalization (codec-specific, safe from false positives)
- Footer grid positioning for theme selector accessibility

## Implementation Summary

### Step 0: Planning & Cleanup ✅

**Deliverables**:
- [interactive_viewer.md](../design/interactive_viewer.md) design document (859 lines)
- Removed layout system complexity (Single vs Responsive templates)
- Standard paths defined (`assets/`, `beliefbase.json`)
- NodeContext structure specified

---

### Step 1: Responsive Layout Foundation ✅

**Deliverables**:
- CSS Grid: `header / (nav | main | aside) / footer`
- Mobile breakpoint (< 1024px): Off-canvas nav, bottom drawer metadata
- Desktop (≥ 1024px): Visible sidebars (280px nav, 320px metadata)
- Toggle buttons (☰ hamburger, Info button)
- Backdrop overlay for modal behavior
- Theme selector dropdown in navigation footer

---

### Step 2: WASM Integration & Interactive Navigation ✅

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

**Implementation**:
- `build.rs` runs `wasm-pack build --target web` when `bin` feature enabled
- WASM artifacts embedded via `include_bytes!` in `src/codec/assets.rs`:
  - `pkg/noet_core.js` (~32KB)
  - `pkg/noet_core_bg.wasm` (~2.2MB)
- `extract_assets()` writes WASM files to output directory
- Feature-gated on `bin` feature (not `service` or `wasm`)
- Cross-platform tested (lib-only vs full binary builds)

**Deliverables**:
- Automated WASM compilation in build pipeline ✅
- WASM artifacts embedded in binary (~2.8MB total) ✅
- Offline-first deployment (no CDN required) ✅
- Feature gating prevents circular dependencies ✅

**Time Spent**: ~1 hour (Session 4, 2026-02-04)

---

## Success Criteria

**Foundation Phase Complete** ✅

All success criteria achieved:

### Infrastructure ✅
- [x] WASM module compiles and loads in browser
- [x] BeliefBase JSON loads and initializes
- [x] Navigation tree generates from PathMaps
- [x] Theme system works with persistence (System/Light/Dark)
- [x] Responsive layout works on mobile and desktop
- [x] Progressive enhancement verified (works without JS)

### Navigation ✅
- [x] Navigation tree displays hierarchical document structure
- [x] Collapsible sections with expand/collapse buttons
- [x] Links navigate to documents (single-click, full page reload)
- [x] Network structure preserved (multiple networks supported)
- [x] WEIGHT_SORT_KEY ordering maintained

### UX ✅
- [x] Mobile toggle buttons work (hamburger, info button)
- [x] Desktop sidebars visible by default
- [x] Backdrop overlay on mobile (modal behavior)
- [x] Theme selector in navigation footer
- [x] Real-time OS theme detection
- [x] No console errors in supported browsers

### Code Quality ✅
- [x] All unit tests passing
- [x] WASM compilation successful
- [x] Browser tests added (Tests 9-11)
- [x] Manual testing completed
- [x] MetaMask/browser extension compatibility verified
- [x] Documentation updated (design docs, architecture)

### Known Limitations (Deferred to ISSUE_39)
- [ ] Two-click navigation pattern not implemented
- [ ] Metadata panel empty (no backlinks/forward links display)
- [ ] No client-side routing (full page reloads)
- [ ] No query builder UI
- [ ] No graph visualization
- [ ] No resizable panels
- [ ] Error states need visible UI (not just console.error)

---

## Testing Summary

### Automated Tests ✅
- **Unit tests**: All passing (codec, WASM bindings, asset embedding)
- **WASM compilation**: `wasm-pack build` successful
- **Browser Test 9**: Namespace getters (href/asset/buildonomy)
- **Browser Test 10**: NodeContext structure and get_context()
- **Browser Test 11**: get_paths() navigation data

### Manual Testing ✅
**Platform**: Linux desktop, Chrome browser

**Test Scenarios**:
1. ✅ Page loads without JS → article readable, links work
2. ✅ Page loads with JS → nav tree builds, theme switcher works
3. ✅ Click nav link → navigates to document (full page reload)
4. ✅ Expand/collapse sections → arrows rotate (› / ▾), children show/hide
5. ✅ Theme dropdown → switches System/Light/Dark
6. ✅ Theme persistence → survives page reload (localStorage)
7. ✅ OS theme change → updates when System selected
8. ✅ Mobile: Hamburger menu → nav slides in from left
9. ✅ Mobile: Info button → metadata drawer slides up from bottom
10. ✅ Mobile: Backdrop → closes panels on click
11. ✅ MetaMask installed → no conflicts, viewer works

**Browser Compatibility**:
- Chrome/Edge: ✅ Tested and working
- Firefox: ⏳ Not tested (ISSUE_39)
- Safari: ⏳ Not tested (ISSUE_39)
- iOS Safari: ⏳ Not tested (ISSUE_39)

### Performance (Baseline Measurements)
- Initial load: ~200ms to interactive (local server)
- WASM initialization: ~50ms
- Navigation tree render: ~20ms (12 documents, 45 sections in network_1)
- Theme switch: <10ms (instant)

---

## Design Decisions Finalized

### Layout System
**Decision**: Single responsive template with progressive enhancement
- Removed layout enum (Simple vs Responsive)
- All documents use same interactive viewer structure
- Network index pages use simple template (from ProtoBeliefNode)

### Link Normalization
**Decision**: Each codec's responsibility (not trait-level abstraction)
- Event-based processing prevents false positives (code blocks)
- MdCodec uses pulldown-cmark events
- Documented as requirement in DocCodec trait
- All registered codec extensions rewritten to .html

### Theme System
**Decision**: Dropdown with System/Light/Dark options
- System follows OS preference (default)
- Real-time MediaQuery listener for OS changes
- Persists to localStorage
- Located in navigation footer (accessible on mobile and desktop)

### Mobile UX
**Decision**: Off-canvas navigation, bottom drawer metadata
- Hamburger menu (☰) for navigation toggle
- Info button for metadata toggle
- Mutually exclusive panels (only one open at a time)
- Backdrop overlay for modal behavior
- 60vh drawer height (will reduce to 40-50vh in ISSUE_39)

### Footer Positioning
**Decision**: CSS Grid with explicit footer row
- 3-row layout: header | content | footer
- Footer at bottom of viewport on all breakpoints
- Theme selector moved from header to nav footer

---

## Lessons Learned

### Successes
1. **Progressive enhancement works**: Static HTML is fully functional without JS
2. **WASM integration smooth**: build.rs automation prevents manual steps
3. **Event-based link processing**: Safer than string manipulation
4. **Graceful degradation**: Theme works even if WASM fails
5. **Feature gating**: Prevents circular dependencies in build system

### Challenges Overcome
1. **Stray XML tags**: Editor artifacts broke entire viewer.js (syntax error)
2. **WASM constructor naming**: `from_json` with `#[wasm_bindgen(constructor)]` requires `new` syntax
3. **Map iteration**: JavaScript Map vs Object (Array.from vs Object.entries)
4. **MetaMask/SES**: Browser extensions can block dynamic imports (try-catch essential)
5. **Layout infrastructure**: Removing enum reduced complexity significantly

### Next Time
1. **Check data types carefully**: Map vs Object methods differ
2. **Watch for editor artifacts**: Stray tags from tool use
3. **Test early, test often**: Syntax errors caught faster with manual testing
4. **Document codec requirements**: Trait contracts need clear prose explanation
5. **Start with functionality**: Get features working before perfect UI

---

## References

- **[Interactive Viewer Design](../design/interactive_viewer.md)** - Complete architecture (859 lines)
- **[ISSUE_39: Advanced Interactive Features](ISSUE_39_ADVANCED_INTERACTIVE.md)** - Continuation work
- [ISSUE_06: HTML Generation](completed/ISSUE_06_HTML_GENERATION.md) - WASM infrastructure (complete)
- [BeliefBase Architecture](../design/beliefbase_architecture.md) - Data model
- [Link Format Design](../design/link_format.md) - BID attribution
- [Architecture Overview](../design/architecture.md) - System namespaces

---

## Completion Notes

**Actual Effort**: 8 days across 5 sessions (Sessions 1-5, 2026-02-02 to 2026-02-04)
- Session 1-2: Planning, layout foundation
- Session 3: Cleanup, design doc
- Session 4: WASM bindings, build automation (~3 hours)
- Session 5: Interactive navigation, tree generation (~5 hours)

**Estimated vs Actual**: Originally estimated 15-20 days for full SPA. Foundation phase took 8 days, remaining work moved to ISSUE_39.

**Why Split**: Clear natural boundary after navigation tree foundation. Two-click pattern, metadata display, and advanced features are distinct phase requiring different design validation.

**Current State**: Fully functional viewer with navigation tree, theme system, and responsive layout. Ready for ISSUE_39 (two-click navigation, metadata panel, resizable panels, error states).

**No Orphaned Actions**: All remaining work tracked in ISSUE_39.

**Next Steps**: See ISSUE_39 for:
- Phase 2.5b: Pre-structured navigation tree API (Rust-generated hierarchy)
- Phase 2.3b: Resizable nav panel (drag handles)
- Phase 2.3c: Mobile drawer height fix (60vh → 40vh)
---

**All remaining work moved to [ISSUE_39: Advanced Interactive Features](ISSUE_39_ADVANCED_INTERACTIVE.md)**