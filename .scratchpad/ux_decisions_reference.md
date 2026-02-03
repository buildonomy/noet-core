# SCRATCHPAD - NOT DOCUMENTATION
# UX Decisions Reference

**Date**: 2026-02-03
**Issue**: ISSUE_38_INTERACTIVE_SPA.md
**Purpose**: Quick reference for key UX decisions and clarifications

---

## Responsive Layout Behavior

### Desktop (>1024px)
- Three-column layout: Nav (left) | Content (center) | Metadata (right)
- All panels visible and fixed
- No hamburger menu or drawer icons

### Tablet (768-1024px)
- Two-column layout: Nav (left) | Content (center)
- Metadata becomes **bottom drawer** (slides up from bottom)
- Hamburger menu visible for collapsing nav
- Metadata icon visible in header

### Mobile (<768px)
- Single-column layout: Content only
- Nav becomes **hamburger menu** (slides from left)
- Metadata becomes **bottom drawer** (slides from bottom)
- Both panels can be open simultaneously (different z-index layers)

### Key Insight
**Different access patterns → different responsive behaviors**:
- **Navigation** (structural, select-once): Hamburger menu from left
- **Metadata** (contextual, frequent toggling): Bottom drawer from bottom
- No conflict on limited screen real-estate

---

## Query Builder UI (Step 3)

### Nested Form Pattern

**Approach**: Dynamic form generation based on type selection

1. **Top level**: Expression type dropdown
   ```
   [StateIn ▼] | StateNotIn | RelationIn | RelationNotIn | Dyad
   ```

2. **Second level**: Predicate type based on Expression choice
   - `StateIn` → StatePred dropdown (Any, Bid, Title, Kind, etc.)
   - `RelationIn` → RelationPred dropdown (Any, SinkIn, SourceIn, Kind, etc.)
   - `Dyad` → Two Expression forms + SetOp selector

3. **Third level**: Input fields based on Predicate choice
   - `StatePred::Title` → Network selector + Regex text input
   - `StatePred::Kind` → BeliefKind checkboxes
   - `StatePred::Bid` → BID input (UUID format validation)
   - `RelationPred::Kind` → WeightSet checkboxes (Section, Epistemic, etc.)

### Example User Flow

```
1. Select Expression type: [StateIn ▼]
   ↓
2. Select StatePred: [Title ▼]
   ↓
3. Fill inputs:
   Network: [Select network ▼]
   Regex: [search.*term_____]
   ↓
4. [Add Query] button
   ↓
5. Results displayed in list view
```

### Three Modes

1. **Simple Mode** (default)
   - Pre-built queries: "All Documents", "All Sections", "Search by Title"
   - One-click execution
   
2. **Advanced Mode** (nested form)
   - Full Expression construction
   - Dynamic form fields
   - Visual query building
   
3. **Text Mode** (power users)
   - Direct JSON input/editing
   - Paste Expression objects
   - Copy query as JSON

### Persistence

- **Save queries to localStorage** (better for larger query data)
- Show recent queries dropdown
- "Favorite" queries for quick access
- Clear query history option

**Why localStorage over cookies**:
- Larger storage limit (~5-10MB vs 4KB)
- Not sent with every HTTP request (better performance)
- Standard for client-side SPA data
- Complex Expressions can be large, won't hit cookie limits

---

## Graph Visualization Semantics (Step 4)

### Information Flow Direction

**Key Concept**: noet-core uses **source → sink** consistently
- Information flows FROM source TO sink
- **Counterintuitive result**: Branch nodes are SINKS, leaf nodes are SOURCES
- Think: "Where does information aggregate?" → Sinks (top of graph)

### Layout Strategy

**Upward Flow** (bottom to top):
```
     ┌─────┐
     │Sink │  ← Top of graph (external sinks, no outgoing edges)
     └──┬──┘
   ┌────┴────┐
   │  Sink   │  ← Information aggregation points
   └────┬────┘
     ┌──┴──┐
     │Src→S│  ← Internal nodes (both source and sink)
     └──┬──┘
   ┌────┴────┐
   │ Source  │  ← Bottom of graph (external sources, no incoming edges)
   └─────────┘
```

**Force Parameters**:
- Gravity pulls toward top for sinks (more incoming edges)
- Gravity pulls toward bottom for sources (more outgoing edges)
- Force strength proportional to edge count
- User-selectable edge weight filter (Section, Epistemic, Pragmatic, All)

### Visual Design

**Nodes**:
- **Consistent size** (no variation by importance)
- **Color by BeliefKind**: Document (blue), Section (green), Asset (gray), etc.
- **Selected node**: 
  - Populate right metadata drawer
  - Change background color (highlight)
  - Add border (2-3px accent color)
- **Hover**: 
  - Show title as tooltip
  - Do NOT populate metadata drawer (only on click)

**Edges**:
- Directed arrows (source → sink)
- Thickness by weight (thicker = stronger relation)
- Color by WeightKind when filtered
- Curved paths for readability

### Interaction Patterns

**Two-Click Navigation** (via metadata drawer):
1. **First click on node**:
   - Populate metadata drawer with node info
   - Show backlinks/forward links by WeightKind
   - Highlight node in graph
   
2. **Second click on node** (or click in drawer):
   - Navigate to document view
   - Close graph mode
   - Load node content

**Other Interactions**:
- **Hover**: Tooltip with title only
- **Drag**: Reposition node (pinning)
- **Zoom/Pan**: Explore large graphs
- **Filter**: Show only nodes matching query builder Expression

### Query Integration

- Query builder filters graph nodes
- Show only matching nodes + their immediate neighbors (1-hop)
- Option: "Show full path between results" (all connecting nodes)
- Highlight filtered nodes with accent color

---

## Theme Switcher (Step 1)

### Three Modes

1. **Auto** (default): 🌓
   - Respects system `prefers-color-scheme`
   - Updates automatically when system changes
   
2. **Light**: ☀️
   - Override system preference
   - Force light theme
   
3. **Dark**: 🌙
   - Override system preference
   - Force dark theme

### Cycling Behavior

**Click theme button**:
```
Auto (🌓) → Light (☀️) → Dark (🌙) → Auto (🌓) → ...
```

### Persistence

- Save to `localStorage` (key: `noet-theme`)
- Apply on page load before first paint (avoid flash)
- Listen for system preference changes (only in Auto mode)

### Implementation

**CSS Approach**:
- Two separate theme files: `noet-theme-light.css`, `noet-theme-dark.css`
- Swap `<link>` href on theme change
- Shared layout CSS imported by both

**JavaScript**:
```javascript
// Detect effective theme
if (themeMode === 'auto') {
  effectiveTheme = window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light';
} else {
  effectiveTheme = themeMode;
}

// Swap stylesheet
document.querySelector('link[data-theme]').href = `assets/noet-theme-${effectiveTheme}.css`;
```

---

## Two-Click Link Pattern (Step 2)

### Behavior

**First Click**:
- `preventDefault()` on link click
- Add `.link-activated` class (visual highlight)
- Fetch node metadata from WASM
- Populate metadata drawer:
  - Node info (title, kind, schema, BID)
  - Backlinks by WeightKind (Section, Epistemic, Pragmatic)
  - Forward links by WeightKind
- On mobile/tablet: Slide metadata drawer up from bottom
- Track activated link in viewer state

**Second Click** (on same link):
- Navigate to target (SPA routing)
- Update URL with `pushState`
- Load target content in main area
- Clear `.link-activated` class
- Keep metadata drawer open (now showing new node)

**Click Different Link**:
- Clear previous `.link-activated` class
- Set new link as activated
- Update metadata drawer with new node

**Links in Metadata Drawer**:
- **Direct navigation** (no two-click passthrough)
- Single click navigates immediately
- Updates content and metadata drawer

### Visual Feedback

**Activated Link Styling**:
```css
.link-activated {
  background-color: var(--yellow-2); /* Light yellow highlight */
  border: var(--border-size-2) solid var(--noet-accent);
  border-radius: var(--radius-2);
  padding: 0 var(--size-1);
}
```

---

## Open Props Integration (Step 1)

### Distribution Strategy

**Default**: Vendored (offline-first)
- Download from unpkg.com
- Place in `assets/open-props/`
- Include in static site generation

**Optional**: CDN mode
- CLI flag: `--open-props-cdn`
- Link to `https://unpkg.com/open-props@1.7.4/`
- Requires internet connection

### Files to Vendor

1. `open-props.min.css` (~15KB) - Core design tokens
2. `normalize.min.css` (~3KB) - CSS reset

### Usage Pattern

**Theme files import Open Props**:
```css
/* noet-theme-light.css */
@import url('open-props/open-props.min.css');
@import url('open-props/normalize.min.css');

:root {
  /* Override/extend with noet-specific variables */
  --noet-bg-primary: var(--gray-0);
  --noet-accent: var(--blue-6);
  /* etc. */
}

/* Import shared layout */
@import url('noet-layout.css');
```

**Layout CSS uses Open Props variables**:
```css
/* noet-layout.css */
.noet-header {
  padding: var(--size-3);
  border-bottom: var(--border-size-1) solid var(--noet-border);
  box-shadow: var(--shadow-2);
}
```

### Customization for Users

**Override CSS variables**:
```css
/* user-custom-theme.css */
:root {
  --noet-accent: #ff6b35; /* Custom brand color */
  --noet-sidebar-width: 320px; /* Wider sidebar */
  --font-sans: "Inter", var(--font-sans); /* Custom font */
}
```

No build step required - just edit CSS and reload!

---

## Bundle Size Targets

### Step 1 (Layout Foundation)
- Open Props: ~18KB
- Light theme CSS: ~2KB
- Dark theme CSS: ~2KB
- Layout CSS: ~15-20KB
- Viewer.js (skeleton): ~5-8KB
- **Total**: ~42-50KB

### Step 2 (Two-Click Nav + Metadata)
- Viewer.js grows to ~15-20KB
- Additional CSS: ~5KB
- **Total**: ~60-70KB

### Step 3 (Query Builder)
- Query builder JS: ~15-20KB
- Query builder CSS: ~5-10KB
- **Total**: ~90-100KB

### Step 4 (Graph Visualization)
- D3.js or similar: ~70-100KB
- Graph CSS: ~5KB
- **Total**: ~165-205KB

### Step 5 (Polish)
- Additional features: ~10-20KB
- **Final Total**: ~175-225KB (before gzip)
- **Gzipped**: ~60-80KB (typical compression ratio)

**Target**: Keep under 100KB gzipped for good performance

---

## Decision Log

| Date | Decision | Rationale |
|------|----------|-----------|
| 2026-02-03 | Open Props + Custom CSS | No build step, clean HTML, deep customization |
| 2026-02-03 | Vendor Open Props by default | Offline-first positioning, reliable |
| 2026-02-03 | Light + Dark themes with Auto mode | Respect system preference, manual overrides |
| 2026-02-03 | Dual-panel responsive (hamburger + drawer) | Different access patterns, no conflict |
| 2026-02-03 | Nested form query builder | Dynamic generation, type-safe, intuitive |
| 2026-02-03 | Save queries to localStorage | Larger storage, standard SPA practice, better for complex queries |
| 2026-02-03 | Graph flows upward (source→sink) | Match noet-core semantics, info aggregation |
| 2026-02-03 | Consistent node size in graph | Focus on connections, not importance |
| 2026-02-03 | Two-click via metadata drawer (graph) | Consistent pattern across UI |

---

## Next Steps

1. ✅ Planning complete (ISSUE_38, trade study, scratchpads)
2. ⏳ **Ready to implement Step 1** (Responsive Layout Foundation)
3. ⏸️ Step 2-5 detailed planning deferred until Step 1 complete

**Estimated Timeline**:
- Step 1: 4 days
- Step 2: 4 days
- Step 3: 4-5 days
- Step 4: 3-4 days
- Step 5: 2-3 days
- **Total**: 17-20 days