# Issue 92: Compile-Time Layout Pipeline

**Version**: 0.1
**Priority**: MEDIUM
**Estimated Effort**: 3 days (RELATIVE COMPARISON ONLY)
**Dependencies**: Requires Issue 91 (Content-Type Classifier — provides
`metadata.content_profile`, ✅ complete). Blocks Issue 85 (3D Credibility
Map Viewer — requires `metadata.render_position` and
`metadata.assembly_index` for informative layout).

## Summary

Compute layout metadata at compile time and store it on each node for
consumption by the 3D viewer (Issue 85). The pipeline has three stages:

1. **Per-node structural scoring**: compute edge counts per node, call
   Issue 91's `score_structural` and `score_merge` to blend the lexical
   content profile (N/P from text) with the structural channel (S from
   section-edge topology). This fills the S-axis gap that Issue 91's
   lexical-only classifier couldn't address.

2. **Per-network aggregates**: compute mean merged profile across
   constituent nodes + structural depth ratio (max pathmap order depth /
   node count). The depth ratio becomes `metadata.structural_depth` on
   network nodes — the viewer uses this to stretch the bubble ellipsoid
   along the S axis.

3. **Two-level force layout**: first position network bubbles via a force
   simulation on the condensed network graph (seeded from aggregate
   profiles), then position intra-bubble nodes via per-network force
   simulations (seeded from merged profiles offset from bubble center).

All metadata is written into the existing `metadata: Table` on
`BeliefNode` during `finalize_html`, after `export_beliefgraph`
materializes the full graph and before `export_beliefbase` serializes to
msgpack.

## Metadata fields produced

| Field | Scope | Type | Description |
|---|---|---|---|
| `metadata.assembly_index` | all in-scope nodes | integer | Upstream network count via BFS on condensed graph |
| `metadata.render_position` | all in-scope nodes | table `{n, s, p}` | Force-settled 3D coordinates in [0,1]³ |
| `metadata.structural_weight` | in-scope non-network nodes | float | Section edge fraction within home network |
| `metadata.structural_depth` | in-scope network nodes | float | max(order depth) / node count |

> [!IMPORTANT]
> **All four fields are optional.** Layout runs over a restricted scope, so
> nodes in excluded networks carry none of them. Consumers (Issue 85) must
> handle absence rather than assuming presence.

### Layout scope (added under Issue 97)

Two classes of network are excluded, and layout can be disabled outright:

| Exclusion | Rule | Rationale |
|---|---|---|
| Reserved namespaces | `Bid::is_reserved()` | Synthetic `External\|Trace` bookkeeping (href/asset/API/codec). No N/S/P semantics — scoring them is a category error. |
| Oversized networks | node count > `--layout-max-nodes` (default 5,000, or `NOET_LAYOUT_MAX_NODES`) | Intra-bubble sim is `O(iterations · n²)`; one large network can dominate the whole build. |
| Everything | `--no-layout` | Escape hatch; layout is on by default. |

The primary justification is semantic, not performance: those nodes carry no
N/S/P signal, so scoring them is meaningless regardless of what it costs.

> [!WARNING]
> An earlier version of this section claimed the exclusion would cut the stage
> from ~290s to ~7.5s, based on the synthetic href namespace holding 78,479
> nodes and accounting for 97.4% of force-simulation work. **That was measured
> on a stale corpus snapshot and did not reproduce.** On the actual run that
> namespace held 3,676 nodes, and the stage's real cost turned out to be
> `PathMapMap::path()` (`O(networks)` per node), not the force simulation.
> See Issue 97 Bottleneck 2, "Measured result", for the full correction.
>
> The exclusion is retained on ontological grounds. Do not cite a performance
> figure for it until one has been measured on the corpus in question.
>
> **Resolved 2026-08-25**: the stage is now 43.0s (from 290s), but essentially
> none of that came from this exclusion — it came from deleting an exhaustive
> fallback scan in `PathMapMap::indexed_path`. The exclusion's justification
> remains ontological, not performance.

## Architecture

### Per-node structural scoring

Edge counts are computed per node (intra-network edges only). These feed
into `score_structural` (from `content_type.rs`) which maps edge topology
to an N/S/P bias vector:

- High incoming epistemic + pragmatic → S-like (other models reference this)
- High outgoing epistemic → N-like (constrains downstream)
- High outgoing pragmatic + owned edges → P-like (exercises other models)

Then `score_merge` blends lexical (α=0.7) with structural (β=0.3) to
produce a merged profile. The S axis, which was essentially zero in
Issue 91's lexical-only classifier, now gets signal from the graph.

### Structural weight

Per-node structural weight = (section_in + section_out) for this node,
divided by the total section edges in the network. This answers "how much
of the network's skeleton does this node own?" — useful for node sizing
in the viewer.

### Assembly index

Upstream network count via BFS on a condensed network-level directed
graph. Each inter-network edge (source in A, sink in B) becomes A→B.
Walking Incoming from a network finds its upstream providers.

### Network aggregates

- Mean merged profile: centroid of all constituent nodes' merged profiles.
- Structural depth: `max(order_vec.len())` across the network's pathmap,
  divided by node count. High = deeply hierarchical (long document tree
  paths), low = flat. This stretches the bubble's S-axis ellipsoid in the
  viewer.

### Two-level force layout

**Level 1 — Bubble layout**: force simulation on the condensed network
graph. Positions seeded from aggregate merged profiles. Inter-network
edge WeightKinds provide typed gravity (epistemic→N, section→S,
pragmatic→P). Produces network-level positions in [0,1]³.

**Level 2 — Intra-bubble layout**: per-network force simulation on
constituent nodes. Positions seeded at `bubble_center + (merged_profile
- 0.5) * spread`, clustering nodes around their bubble while preserving
relative content-type differentiation. Intra-network edge WeightKinds
provide typed gravity within the bubble.

**Implementation**: pure Rust velocity-Verlet simulation with tunable
parameters (iterations, repulsion, spring, gravity, centering, damping,
cooling). Deterministic — same graph → same positions.

## Implementation Steps

1. Per-node structural scoring (0.5 days)
   - [x] `compute_edge_counts(graph, home_networks)` — intra-network only
   - [x] `compute_merged_profiles(graph, edge_counts)` — calls
         `score_structural` + `score_merge`
   - [x] `compute_structural_weights(edge_counts, home_networks)`
   - [x] Unit test: intra-network edges counted, inter-network excluded
   - [x] Unit test: merged profile S boosted by structural signal
   - [x] Unit test: structural weight proportional to section edge share

2. Assembly index + condensed graph (0.5 days)
   - [x] `build_condensed_network_graph` with typed edges
   - [x] `compute_assembly_indices` — BFS upstream on condensed graph
   - [x] Unit test: linear chain A→B→C → indices 0, 1, 2
   - [x] Unit test: isolated network → index 0

3. Network aggregates (0.5 days)
   - [x] `compute_network_aggregates` — mean profile + structural depth
   - [x] Structural depth from pathmap order vector lengths

4. Two-level force layout (1 day)
   - [x] `run_bubble_layout` — condensed graph, seeded from aggregates
   - [x] `run_intra_bubble_layout` — per-network, seeded from merged
         profiles offset from bubble center
   - [x] `run_force_simulation_core` — shared velocity-Verlet engine
   - [x] Unit test: pragmatic edges → P-axis drift
   - [x] Unit test: epistemic edges → N-axis drift
   - [x] Unit test: deterministic (two runs identical)
   - [x] Unit test: positions bounded to [0,1]³

5. Pipeline integration (0.5 days)
   - [x] `compute_layout_metadata` writes all fields into `graph.states`
   - [x] Called from `finalize_html` after `export_beliefgraph`, before
         `export_beliefbase`
   - [x] Integration test: compile application corpus → verify metadata via MCP
   - [x] Scope guard: exclude reserved namespaces + `--layout-max-nodes` ceiling
         + `--no-layout` (see "Layout scope" above)
   - [ ] Performance test: 30K-node corpus completes in < 10 seconds
         (not met, but no longer dominated by a defect: 43.0s on a 135K-node
         corpus, down from 290s. Remaining cost is 32.3s scope resolution +
         10.6s force simulation. See Issue 97 Bottleneck 2.)

## Testing Requirements

- [x] Edge counts: intra-network only, inter-network excluded
- [x] Structural weight: proportional to section edge share
- [x] Merged profiles: S axis boosted by structural edges
- [x] Assembly index: linear chain → correct counts
- [x] Assembly index: isolated → 0
- [x] Force layout: pragmatic edges → P-axis clustering
- [x] Force layout: epistemic edges → N-axis clustering
- [x] Force layout: deterministic across runs
- [x] Force layout: positions bounded [0,1]³
- [x] No regression in 693 existing tests

## Success Criteria

- [x] `metadata.assembly_index` computed for network nodes
- [x] `metadata.render_position` computed for all nodes
- [x] `metadata.structural_weight` computed for non-network nodes
- [x] `metadata.structural_depth` computed for network nodes
- [x] Merged profiles include S-axis signal from graph structure
- [x] Deterministic: same input → same output
- [x] No regression in existing tests
- [x] Validated on application corpus via MCP (see validation section)
- [ ] Force simulation parameters tuned with visual feedback from
      Issue 85 viewer (further iteration expected)
- [ ] Performance validation on large corpus (production scale, 30K+ nodes)

## Corpus Validation (application, 2475 nodes, 13 networks)

Validated via MCP `get_context` on representative nodes after compiling
application layout pipeline.

### Assembly index

| Network | assembly_index | Assessment |
|---|---|---|
| QMS Root (42 nodes) | 11 | ✅ Root aggregator — most networks feed in |
| NPR 7150 (470 nodes) | 8 | ✅ External standard with upstream deps |
| RFCs (201 nodes) | 8 | ✅ Same dependency level as NPR 7150 |

### Render positions (before → after gravity normalization)

| Node | Before (n, s, p) | After (n, s, p) | Assessment |
|---|---|---|---|
| QMS Root | (0.08, **1.0**, 0.25) | (0.0, **1.0**, 0.50) | s=1.0 correct for root |
| NPR 7150 | (**1.0**, **1.0**, 0.39) | (**1.0**, 0.0, 0.53) | Was double-saturated → now differentiated |
| RFCs | (**1.0**, **1.0**, 0.20) | (0.57, **1.0**, **1.0**) | Was double-saturated → better spread |
| Connectome Compiler | (**1.0**, **1.0**, 0.0) | (**1.0**, **1.0**, 0.86) | N/S hub, P boosted from 0→0.86 |
| Gap Analysis doc | (0.17, 0.86, 0.0) | (0.0, 0.42, 0.0) | Pure S positioning — structural mapping |

### Structural depth

| Network | Before | After | Assessment |
|---|---|---|---|
| QMS Root | **3.0** ⚠️ | **1.0** | Bug fixed: was recursing into subnets |
| NPR 7150 | 0.018 | 0.018 | Unchanged — correct |
| RFCs | 0.022 | 0.022 | Unchanged — correct |

### Key findings

1. **S-axis gap filled**: Connectome Compiler RFC has lexical
   `content_profile.s = 0.0` but `render_position.s = 1.0` after
   structural merging. The graph signal (incoming epistemic edges)
   correctly boosted S — this is the exact gap Issue 91 couldn't close.

2. **Gravity normalization fixed boundary saturation**: v1 applied
   gravity impulse per-edge, so high-degree nodes accumulated
   overwhelming directional force and slammed into [0,1] walls. v2
   pre-computes a normalized gravity bias (mean edge direction per node),
   producing interior-differentiated positions.

3. **Remaining wall-hitting is directionally correct**: the Connectome
   Compiler at n=1.0, s=1.0 is a true N/S hub — saturation there is
   signal, not artifact. Further tuning will need visual feedback from
   the 3D viewer.

## Risks

- **Force layout quality**: current parameters produce directionally
  correct, spatially differentiated results on application corpus. Further tuning
  will use visual feedback from Issue 85's 3D viewer.
  **Mitigation**: parameters are constants, easy to iterate.

- **Performance on large corpora**: brute-force O(n²) repulsion per
  network per iteration. Typical networks have hundreds to low thousands
  of nodes — fast enough. Networks with >5000 nodes may need Barnes-Hut.
  **Mitigation**: implement brute force first; profile if needed.

## Design Decisions

- **Assembly index is network-only**: constituent nodes don't get
  `metadata.assembly_index`. The viewer uses the home network's value
  for bubble sizing — nodes are positioned within their bubble and
  inherit its scale.

- **Gravity normalized per-node**: each node's gravity bias is the mean
  direction across all its edges, not raw per-edge accumulation. This
  prevents high-degree nodes from saturating at boundaries while
  preserving directional signal.

- **structural_depth uses depth=0**: submap call doesn't recurse into
  subnets, measuring only this network's own hierarchy. Subnets have
  their own structural_depth.

## References

- Issue 85 — consumes all four metadata fields
- Issue 91 — provides `content_profile` + `score_structural` +
  `score_merge` (✅ complete)
- `src/layout.rs` — implementation
- `src/codec/compiler.rs` — `finalize_html` integration point (L2650)
- `src/shard/content_type.rs` — `score_structural`, `score_merge`,
  `EdgeCounts`, `ContentProfile`
- `planning/essays/credibility_render_sketch.md` — rendering specification
