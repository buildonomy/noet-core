# Issue 85: 3D Credibility Map Viewer

**Version**: 0.1
**Priority**: MEDIUM
**Estimated Effort**: 6 days (RELATIVE COMPARISON ONLY)
**Dependencies**: Requires Issue 82 (Viewer Query UI, ✅ complete),
Requires Issue 91 (N/S/P/R Content-Type Classifier, ✅ complete),
Requires Issue 92 (Compile-Time Layout Pipeline, ✅ complete)

## Summary

Add a 3D credibility map viewer to the noet viewer using Three.js. The
viewer renders the corpus as a recursive bubble structure: the entry
point network is the initial view, its depth-0 section children (which
are sub-networks) appear as bubbles. Clicking a bubble focuses it,
revealing its own depth-0 children inside, with intra-network edges
drawn between siblings and inter-network edges drawn to the most
general external network bubble. This recursive focus model unifies
the "overview" and "detail" views — the overview IS the root network
focused, and drilling down is the same operation applied to a child.

Edges are colored by WeightKind (Section=grey, Epistemic=blue,
Pragmatic=orange). Node and bubble positions come from Issue 92's
pre-computed `render_position` in N/S/P content-type space. Bubble
radius is proportional to `assembly_index`.

This issue covers the JavaScript/Three.js viewer side. The Rust
compile-time layout pipeline is Issue 92.

## Goals

- Recursive focus model: initial view = entry point focused, click to
  drill into any sub-network, click again or Escape to return
- Render networks as transparent 3D bubbles with assembly_index radius
- Position nodes/bubbles by `metadata.render_position` (fallback:
  `metadata.content_profile`, then center)
- On focus: show depth-0 section children, intra-network edges, and
  inter-network edges to external bubbles
- Edge coloring encodes WeightKind
- Click-to-inspect: clicking a node/bubble opens the metadata drawer
- Smooth animated orbit transitions on focus/defocus
- Bubble highlight: focused bubble brightens, others dim
- Camera orbit, zoom, pan via OrbitControls
- PNG export from the 3D scene

## Architecture

### Rendering engine: Three.js

Three.js (~750 KB total: core + OrbitControls + internal dependency)
provides the 3D WebGL canvas. Vendored into `assets/threejs/` via
npm + `copy:threejs` script. Lazy-loaded on first Graph activation.

Files: `three.module.min.js` (357 KB), `three.core.min.js` (385 KB),
`OrbitControls.js` (40 KB, patched to import from relative path).

### Recursive focus model

Every view is a "focus" on some network. The initial view focuses the
corpus entry point (`state.beliefbase.entryPoint().bid`). Its depth-0
section children ARE the network bubbles the user sees. Clicking a
bubble focuses that sub-network, revealing its own children. Defocus
(double-click, Escape, or click the focused bubble again) returns to
the entry point view.

This unifies overview and detail — there is no separate "unfocused"
code path.

### Network containment tree

Built during scene initialization from all networks' `home_net`:

```
networkParents: Map<networkBid, parentNetworkBid>
networkChildren: Map<parentBid, Set<childBid>>
```

Used to compute the "inside set" for a focused network (the network
itself + all descendant sub-networks). This determines whether an
edge target is intra-network (both ends inside) or inter-network
(one end outside).

### Inter-network edge resolution

For an edge from a focused network's child to a node outside the
focused network: resolve the target's `home_net` upward through the
containment tree to find the most general ancestor network that is
still outside the focused network's "inside set". Draw the edge to
that ancestor's bubble. This prevents edges from targeting deeply
nested sub-networks of sibling networks — the user sees "this child
connects to NPR 7150" not "this child connects to section 3.1.4a of
NPR 7150."

### Data access pattern

- `bb.get_networks()` → all Network-kinded nodes (for bubble rendering
  and containment tree)
- `bb.get_context(bid)` → per-network metadata (render_position,
  assembly_index, home_net) for bubble placement
- `bb.get_submap(networkBid, "", 0, true)` → depth-0 section children
  of the focused network
- `bb.get_context_bulk(childBids)` → batch fetch contexts for all
  children in a single WASM call (metadata for positioning + graph
  edges for edge detection + related_nodes for home_net lookup)

### Scene structure

```
Scene (persistent)
├── Origin axes: N (blue), S (green), P (orange) arrows at (0,0,0)
├── Network bubbles (transparent spheres, always visible)
│   ├── Radius ∝ sqrt(assembly_index)
│   ├── Position = render_position of network node
│   ├── Color = N/S/P content profile blend
│   └── Wireframe overlay for visibility
└── Focus detail group (cleared + rebuilt on each focus change)
    ├── Child nodes (small spheres inside focused bubble)
    │   ├── Position = render_position offset from bubble center
    │   └── Color = individual content profile blend
    ├── Intra-network edges (lines between sibling children)
    │   └── Color by WeightKind
    └── Inter-network edges (lines from child → external bubble)
        └── Color by WeightKind, multi-kind edges offset
```

### Performance

Three.js with instanced rendering handles 10K+ nodes. The focus model
keeps visible node count low — only the focused network's depth-0
children are rendered as individual nodes (typically 5–20 per network).
All other networks remain as opaque bubbles. This makes the renderer
fast regardless of corpus size.

## Implementation Steps

1. Vendor Three.js (✅ complete)
   - [x] `assets/package.json`: `three@^0.185.1` + `copy:threejs` script
   - [x] Vendored: `three.module.min.js`, `three.core.min.js`,
         `OrbitControls.js` (patched for relative import)
   - [x] Lazy-load wrapper: `loadThree()` in `assets/viewer/graph3d.js`

2. Scene scaffolding (✅ complete)
   - [x] `renderGraph3D(container)`: Three.js scene, camera, lighting
   - [x] OrbitControls with damping
   - [x] Raycasting for click-to-inspect → `showMetadataPanel(bid)`
   - [x] Responsive canvas sizing (ResizeObserver)
   - [x] "Graph" display mode in traceability panel selector
   - [x] Removed standalone `#graph-container` in favor of panel mode
   - [x] PNG export button; CSV/XLSX hidden in graph mode
   - [x] `disposeGraph3D()` cleanup on mode switch and panel close

3. Bubble rendering + recursive focus (✅ complete)
   - [x] All networks rendered as transparent spheres
   - [x] Radius from `metadata.assembly_index`
   - [x] Position from `metadata.render_position`
   - [x] Color from N/S/P content profile blend
   - [x] Network containment tree from `home_net`
   - [x] Initial view = entry point network focused
   - [x] Click bubble → focus (orbit transition + detail render)
   - [x] Click focused bubble / double-click / Escape → defocus
   - [x] Bubble highlight: focused brightens, others dim
   - [x] Origin axes with labeled arrows (N/S/P)

4. Focus detail rendering (✅ complete)
   - [x] Depth-0 section children from `get_submap`
   - [x] Batch context fetch via `get_context_bulk`
   - [x] Intra-network edges between sibling children
   - [x] Inter-network edges resolved to external bubble targets
   - [x] Edge coloring by WeightKind (grey/blue/orange)
   - [x] Multi-kind edges offset for visual distinction
   - [x] Focus detail group cleared on defocus

5. Layout tuning (in progress)
   - [ ] Address boundary saturation in `src/layout.rs` — nodes with
         similar content profiles slam into [0,1] walls instead of
         distributing through the interior
   - [ ] Visual feedback loop: viewer ↔ layout parameter tuning
   - [ ] Performance validation on large corpus (production scale, 30K+ nodes)

6. Query overlay palette (future)
   - [ ] Overlay data structure: `{ query, color, opacity, label }`
   - [ ] Evaluate query against loaded data (reuse Issue 82's
         `queryView` infrastructure)
   - [ ] Apply material overrides to matching scene objects
   - [ ] Multiple overlays compose via additive blending
   - [ ] Built-in palette entries for common diagnostics (gaps,
         unexercised)

## Testing Requirements

- Switching to Graph mode produces a Three.js canvas
- Network bubbles render with correct radius ordering (larger
  assembly_index = larger bubbles)
- Clicking a bubble focuses it, showing depth-0 children
- Clicking a child node opens metadata drawer with correct BID
- Inter-network edges draw to correct external bubble targets
- Defocus (Escape / double-click / click focused) returns to root view
- Switching back to Table mode does not re-fetch data
- All existing traceability tests pass (no regression)
- Performance: renders at 30+ fps with all bubbles visible

## Success Criteria

- [x] Recursive focus model: overview = root focused, drill-in = same
- [x] Network bubbles rendered with assembly_index-proportional radius
- [x] Nodes positioned by render_position
- [x] Edge styling encodes WeightKind via color
- [x] Click-to-inspect works for nodes and bubbles
- [x] Camera orbit, zoom, pan functional
- [x] PNG export produces valid output
- [x] No regression in existing traceability or search functionality
- [ ] Layout positions spread through [0,1]³ interior (Issue 92 tuning)
- [ ] At least 2 built-in query overlays functional

## Risks

- **Boundary saturation in layout**: nodes with similar content
  profiles cluster at [0,1] walls. Confirmed on vast_qms corpus —
  QMS Documents children all have `content_profile.s=0, p≈0` and
  render_position hits 0.0 or 1.0 on multiple axes.
  **Mitigation**: tune force layout parameters in `src/layout.rs`
  with visual feedback from this viewer. Parameters are constants,
  easy to iterate.

- **Three.js vendoring complexity**: Three.js r185 split
  `three.module.min.js` into a re-export shim + `three.core.min.js`.
  Both must be vendored.
  **Mitigation**: Resolved — `copy:threejs` script copies both files.

- **WASM data access for edge detection**: `get_context_bulk` returns
  JS Maps (from `serde_wasm_bindgen`), requiring `.entries()` iteration.
  `home_net` comparison needs `String()` coercion.
  **Mitigation**: Working — validated on vast_qms corpus.

- **Performance on large corpora**: brute-force edge iteration in
  `renderFocusDetail` scales with total edges per network. Typical
  networks have hundreds of depth-0 children — fast enough.
  **Mitigation**: Focus model keeps visible node count low.

## Open Questions

- **Layout interior distribution**: how to tune force layout to avoid
  boundary saturation? Candidates: stronger centering force, clamped
  repulsion, per-axis normalization after simulation.
- **Credibility texture layer (future)**: the current implementation
  is wireframe — bubbles, nodes, edges with color and position. The
  missing layer is credibility texture: R-derived properties (opacity,
  fidelity, coverage) painted onto the wireframe at each coupling
  point. See `planning/essays/credibility_render_sketch.md` §R as
  opacity for the specification. A follow-on issue should add texture
  rendering driven by R event metadata and model-time decay.

## References

- Issue 82 — query UI and data flow (✅ complete)
- Issue 91 — N/S/P content profiles (✅ complete)
- Issue 92 — compile-time layout pipeline (✅ complete)
- `assets/viewer/graph3d.js` — primary implementation
- `assets/viewer/traceability.js` — display mode integration
- `src/layout.rs` — compile-time force layout (tuning target)
- `planning/essays/credibility_render_sketch.md` — rendering spec
