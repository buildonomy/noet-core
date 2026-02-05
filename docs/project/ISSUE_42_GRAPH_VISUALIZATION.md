# Issue 42: Force-Directed Graph Visualization for Interactive Viewer

**Priority**: LOW
**Estimated Effort**: 3-4 days
**Dependencies**: ISSUE_39 (Phase 1 complete), ISSUE_40 (network indices working)
**Version**: 0.1

## Summary

Add an interactive force-directed graph visualization to the HTML viewer that displays belief networks as visual graphs. Users can explore node relationships spatially, with nodes positioned based on their connections and influence (weights).

## Goals

- Visual representation of belief network structure
- Interactive exploration (zoom, pan, drag nodes)
- Filter by node kind, schema, relationship type
- Highlight paths between nodes
- Integration with metadata panel (click node → show metadata)
- Export graph as image (PNG, SVG)

## Architecture

### Technology Stack

**Graph Library Options**:
1. **D3.js** (recommended)
   - Flexible force simulation
   - Full control over rendering
   - Large ecosystem
   - Size: ~300KB (minified)

2. **Cytoscape.js**
   - Purpose-built for network graphs
   - Rich layout algorithms
   - Size: ~500KB (minified)

3. **Vis.js Network**
   - Simple API
   - Good defaults
   - Size: ~200KB (minified)

**Decision**: D3.js (flexibility + ecosystem)

### Graph Data Structure

**From WASM API**:
```rust
pub struct BeliefGraph {
    pub states: BTreeMap<Bid, BeliefNode>,
    pub relations: Relations,  // (nodes, edges with weights)
}
```

**Convert to D3 Format**:
```javascript
{
    nodes: [
        { id: "bid-1", label: "Document Title", kind: "Belief", schema: "Task" },
        { id: "bid-2", label: "Section Title", kind: "Belief", schema: "Section" },
        // ...
    ],
    links: [
        { source: "bid-1", target: "bid-2", weight: 1.0, kind: "Section" },
        // ...
    ]
}
```

### UI Components

**Graph Container**:
- Full-width panel (overlays content when active)
- Toggle button in header: "Graph" (shows/hides graph view)
- Close button (X) in graph panel header
- Controls toolbar: Zoom, Filter, Export, Reset

**Graph Canvas**:
- SVG rendering (scalable, crisp at all zoom levels)
- Force simulation (physics-based layout)
- Node sizing based on connection count (degree centrality)
- Edge thickness based on weight
- Color coding by node kind

**Interaction Patterns**:
- **Click node**: Show metadata panel with node details
- **Drag node**: Reposition node (pin in place)
- **Hover node**: Highlight connected nodes + edges
- **Double-click node**: Navigate to document
- **Scroll**: Zoom in/out
- **Drag background**: Pan viewport

## Implementation Steps

### Phase 1: Basic Graph Rendering (1.5 days)

#### Step 1.1: D3.js Integration (3 hours)
- [ ] Add D3.js to assets (`assets/d3.min.js`)
- [ ] Load D3 in viewer.js
- [ ] Create graph container in template
- [ ] Add graph toggle button to header

#### Step 1.2: Data Conversion (3 hours)
- [ ] Fetch BeliefGraph from WASM (`wasm.get_graph()`)
- [ ] Convert to D3 node/link format
- [ ] Filter system namespaces (buildonomy, href, asset)
- [ ] Handle missing nodes gracefully

#### Step 1.3: Force Simulation Setup (6 hours)
- [ ] Initialize D3 force simulation
- [ ] Configure forces (charge, collision, center, link)
- [ ] Render nodes as circles (radius based on degree)
- [ ] Render links as lines (thickness based on weight)
- [ ] Add labels (node titles)

### Phase 2: Interactivity (1 day)

#### Step 2.1: Zoom & Pan (3 hours)
- [ ] D3 zoom behavior
- [ ] Zoom controls (buttons: +, -, reset)
- [ ] Pan via drag on background
- [ ] Zoom extent limits (min: 0.1x, max: 10x)

#### Step 2.2: Node Interaction (4 hours)
- [ ] Drag nodes (update force simulation)
- [ ] Pin nodes in place (disable forces)
- [ ] Click node → show metadata panel
- [ ] Double-click node → navigate to document
- [ ] Hover node → highlight neighbors

#### Step 2.3: Visual Feedback (3 hours)
- [ ] Highlight connected nodes on hover
- [ ] Dim unconnected nodes
- [ ] Edge highlighting
- [ ] Active node indicator (when metadata panel open)

### Phase 3: Filtering & Styling (half day)

#### Step 3.1: Filter Controls (3 hours)
- [ ] Filter by Kind (dropdown: Belief, Network, Section, etc.)
- [ ] Filter by Schema (dropdown)
- [ ] Filter by relationship type (Edge kind)
- [ ] Apply filters → re-render graph

#### Step 3.2: Color Coding (2 hours)
- [ ] Node color by Kind (Belief: blue, Network: green, etc.)
- [ ] Edge color by WeightKind (Section: gray, Epistemic: orange)
- [ ] Legend (color key)

### Phase 4: Export & Polish (1 day)

#### Step 4.1: Export Functionality (3 hours)
- [ ] Export to PNG (canvas rendering)
- [ ] Export to SVG (download current view)
- [ ] Export to JSON (graph data)

#### Step 4.2: Performance Optimization (2 hours)
- [ ] Limit graph size (max 500 nodes, warn if exceeded)
- [ ] Use quadtree for collision detection
- [ ] Debounce force simulation updates
- [ ] Loading indicator for large graphs

#### Step 4.3: Documentation (2 hours)
- [ ] Update `interactive_viewer.md` design doc
- [ ] In-graph help overlay (keyboard shortcuts, controls)
- [ ] User guide section

#### Step 4.4: Accessibility (1 hour)
- [ ] Keyboard navigation (Tab, Arrow keys)
- [ ] ARIA labels for controls
- [ ] Screen reader support (graph summary)

## Testing Requirements

### Automated Tests

**Browser Tests** (`tests/browser/test_runner.html`):
- [ ] Graph container opens/closes
- [ ] Data conversion from BeliefGraph to D3 format
- [ ] Filter controls work correctly
- [ ] Export functions generate valid files
- [ ] Large graph (>100 nodes) renders without errors

**Visual Regression Tests**:
- [ ] Graph layout is deterministic (same data → same layout)
- [ ] Zoom/pan doesn't break rendering
- [ ] Filters update graph correctly

### Manual Testing

**Desktop**:
- [ ] Graph renders with readable labels
- [ ] Zoom/pan smooth and responsive
- [ ] Drag nodes updates layout
- [ ] Click node shows metadata panel
- [ ] Double-click navigates to document
- [ ] Export to PNG/SVG works
- [ ] Filter controls update graph

**Mobile**:
- [ ] Touch zoom/pan works
- [ ] Tap node shows metadata
- [ ] Graph viewport fits screen
- [ ] Controls accessible

**Performance**:
- [ ] 100-node graph renders smoothly
- [ ] 500-node graph renders with warning
- [ ] Force simulation stabilizes within 5 seconds

## Success Criteria

- [ ] Graph renders BeliefGraph data correctly
- [ ] Interactive zoom/pan/drag works
- [ ] Click node → metadata panel integration
- [ ] Filter by kind/schema/relationship works
- [ ] Export to PNG/SVG functional
- [ ] Performance acceptable for networks <500 nodes
- [ ] All automated tests pass
- [ ] Manual testing confirms UX quality

## Risks

### Risk 1: Performance with Large Graphs
**Problem**: Graphs >500 nodes may freeze browser

**Mitigation**: 
- Warn user if graph exceeds 500 nodes
- Offer filtered views (subgraphs)
- Consider WebGL rendering for large graphs (future)
- Virtualization (only render visible nodes)

### Risk 2: Layout Quality
**Problem**: Force simulation may produce poor layouts (overlapping nodes, crossed edges)

**Mitigation**:
- Tune force parameters (charge, link distance, collision radius)
- Provide layout presets (hierarchical, radial, grid)
- Allow manual node positioning (drag + pin)

### Risk 3: Mobile UX Constraints
**Problem**: Graph may be too complex for small screens

**Mitigation**:
- Simplified mobile view (fewer controls visible)
- Full-screen mode
- Minimap for navigation (optional)

### Risk 4: Library Size
**Problem**: D3.js adds ~300KB to page weight

**Mitigation**:
- Lazy load D3 only when graph view activated
- Use D3 modular imports (reduce size to ~100KB)
- Consider CDN option (reduce local bundle)

## Design Decisions

### Force Simulation Parameters
**Decision**: Use default D3 forces with custom tuning

**Configuration**:
- Charge: `-300` (repulsion between nodes)
- Link distance: `100` (edge length)
- Collision radius: `30` (prevent overlap)
- Center: `[width/2, height/2]` (viewport center)

**Rationale**: Balanced layout for most network structures

### Node Sizing
**Decision**: Size based on degree centrality (connection count)

**Formula**: `radius = 5 + Math.sqrt(degree) * 3`

**Rationale**: Visually emphasizes hub nodes (highly connected)

### Color Scheme
**Decision**: Color by BeliefKind

**Palette**:
- Belief: `#4A90E2` (blue)
- Network: `#7ED321` (green)
- Section: `#F5A623` (orange)
- Unknown: `#9B9B9B` (gray)

**Rationale**: Consistent with metadata panel color coding

### Graph Scope
**Decision**: Show current network only (not cross-network)

**Rationale**:
- Reduces complexity
- Matches user's mental model (exploring one network at a time)
- Can add cross-network view later if needed

### Export Formats
**Decision**: PNG and SVG (not PDF)

**Rationale**:
- PNG: Universal compatibility, easy to embed
- SVG: Scalable, editable in vector tools
- PDF: Adds complexity, similar to SVG use case

## References

- ISSUE_39: Interactive viewer foundation
- `docs/design/interactive_viewer.md`: § Graph Visualization
- D3.js Force Documentation: https://d3js.org/d3-force
- Cytoscape.js: https://js.cytoscape.org/ (alternative)

## Notes

**Lazy Loading Strategy**:
Graph functionality should lazy-load D3.js to avoid bloating initial page load:
```javascript
async function initGraph() {
    if (!window.d3) {
        await loadScript('assets/d3.min.js');
    }
    renderGraph();
}
```

**Future Enhancements** (deferred to later issues):
- 3D graph visualization (Three.js)
- Timeline view (evolution of network over time)
- Path finding (shortest path between two nodes)
- Clustering (group related nodes)
- Community detection (identify sub-networks)
- Diff view (compare two versions of network)

**Alternative Libraries Considered**:
- **Sigma.js**: WebGL rendering, excellent performance, but less flexible
- **GraphViz.js**: Deterministic layouts, but slower and less interactive
- **Plotly.js**: Simple API, but limited customization for network graphs

**Accessibility Note**:
Graph visualization is inherently visual. Provide alternative text summary:
- "Network contains X nodes and Y edges"
- "Most connected node: [title] with N connections"
- "Isolated nodes: [count]"