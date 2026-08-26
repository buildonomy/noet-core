# Issue 94: Model Boundary Inference

**Priority**: HIGH
**Estimated Effort**: 1-2 days
**Dependencies**: Informed by [wp-model-ontology] §8.1 (model identity),
§8.3 (registration event), Issue 93 (inference engine — consumer of
boundary results).

## Summary

Define and implement the algorithm that determines which nodes constitute
a model, given a focus node. This is the foundation that Issue 93's
inference engine operates on — the engine assesses projection completeness
and credibility, but it needs to know what scope to assess.

The problem is harder than it appears because:

1. **Explicit boundaries exist but are insufficient.** Network-kinded
   nodes in noet-core already define structural containment — a network
   IS a model boundary by authoring convention. But not all models align
   with network boundaries, and not all networks correspond to coherent
   models.

2. **Implicit boundaries require mature data.** The registration
   criterion (§8.3 — cross-axis coupling density exceeds same-axis
   density) requires sufficient coupling data to compute. Sparse,
   newly-born models won't have enough density. The algorithm must
   degrade gracefully from inference to heuristic to explicit.

3. **Models overlap.** A single node may participate in multiple models
   — a requirement node might be part of a product model, a compliance
   model, and a safety model simultaneously. Each viewpoint produces
   a different model boundary around the same node, with different
   E/F/M classifications and different projection completeness
   assessments. Model membership is not exclusive.

4. **Heterogeneous maturity.** A compiled graph will contain mature
   models with dense couplings alongside evolving models with sparse
   couplings. The boundary algorithm must handle both without the
   dense regions dominating the sparse ones.

## Architecture

### Three tiers of boundary detection

**Tier 1: Explicit boundaries (always available)**

Network-kinded nodes define model boundaries by authoring convention.
This is already how noet-core works — a network IS a model. The
inference engine (Issue 93) can start here with zero new code. The
metadata card shows: "this node belongs to network X" and runs
projection completeness within that scope.

Explicit model boundaries can also be declared via frontmatter or
schema metadata — an author can state "this subtree is a model" by
marking it as such. This tier requires no inference.

**Tier 2: Paradigmatic boundaries (cross-check)**

The EMO ontology predicts that certain structural patterns correspond
to model boundaries:

- N→N couplings (same-axis, cross-abstraction) are strong model
  boundary signals — they mark where one model's normative surface
  becomes input to a different model at a different scale.
- A cluster of nodes that share a common parent N source but have
  independent S and P content is a candidate model — the shared N
  defines the model's normative surface.
- A set of nodes with a common responsible owner (if ownership
  metadata exists) is a candidate model — the owner is the vesicle
  provider.

These heuristics can cross-check Tier 1 boundaries: does the
network boundary align with the paradigmatic signals? Where they
diverge, either the network boundary is wrong (too broad or too
narrow) or the model spans multiple networks.

**Tier 3: Implicit boundaries (requires coupling density)**

The registration criterion from §8.3: a model boundary exists where
cross-axis coupling density (N→S, N→P) exceeds same-axis density
(N→N, S→S). This is computable but requires sufficient data.

For sparse graphs, this tier produces no results — which is correct.
The absence of implicit boundaries in a sparse graph means the graph
doesn't yet have enough structure to infer model boundaries, and the
algorithm should say so rather than guess.

### Multi-model membership

A node can belong to multiple models simultaneously. The metadata
card should show ALL model memberships, not just one. Each model
membership produces a different context for the inference engine:

- Node X as part of product model A → E/F/M labels relative to A
- Node X as part of compliance model B → E/F/M labels relative to B
- Node X as part of safety model C → E/F/M labels relative to C

The card shows: "this node participates in 3 models" with each model's
inference results separately surfaced. The credibility surface on the
3D graph would need to select which model's texture to render (or
composite them).

### Graceful degradation

| Graph maturity | Available tiers | Behavior |
|---|---|---|
| Mature (dense couplings) | Tier 1 + 2 + 3 | Full inference, cross-checked |
| Developing (moderate couplings) | Tier 1 + 2 | Heuristic boundaries, paradigmatic cross-checks |
| Sparse (few couplings) | Tier 1 only | Explicit boundaries only, gaps reported honestly |

The inference engine (Issue 93) works at any tier — it doesn't care
how the boundary was determined. It just needs a boundary.

## Steps

- [ ] Implement Tier 1: explicit boundary detection from network
      containment (may already exist as `get_submap` with section
      edges — verify)
- [ ] Specify Tier 2 paradigmatic heuristics: what structural
      patterns predict model boundaries?
- [ ] Implement Tier 2 cross-check: given a Tier 1 boundary, do
      paradigmatic signals agree?
- [ ] Specify Tier 3 coupling density algorithm: what threshold
      of cross-axis vs same-axis density constitutes a boundary?
- [ ] Implement Tier 3 with graceful degradation (no result for
      sparse graphs, not a wrong result)
- [ ] Implement multi-model membership: a node can return multiple
      model contexts
- [ ] Define the boundary result data structure consumed by Issue 93

## Done When

- [ ] Tier 1 boundaries work on any compiled graph
- [ ] Tier 2 heuristics specified and implemented for at least
      N→N-based boundary detection
- [ ] Multi-model membership supported: metadata card can show
      multiple model contexts for a single node
- [ ] Graceful degradation demonstrated: sparse graph produces
      Tier 1 only, no false positives

## Risks

- **Tier 3 may be intractable without ground truth**: coupling
  density thresholds need calibration against known model
  boundaries. → **Mitigation**: use Tier 1 boundaries as ground
  truth for calibrating Tier 3 thresholds.
- **Multi-model rendering may be visually noisy**: compositing
  multiple model textures on the 3D graph could be confusing. →
  **Mitigation**: start with model-selection (user picks which
  model context to render), add compositing later.

## Relationship to Other Work

- **Issue 93** (inference engine): direct consumer — needs
  boundary results to run projection completeness assessment
- **[wp-model-ontology] §8.3** (registration event): defines
  the theoretical criterion for model boundaries
- **Network authoring** (`docs/design/network_authoring.md`):
  Tier 1 boundaries derive from network structure
- **Application-specific validation cases** exist in the planning
  repo that exercise Tier 2 boundary detection against real
  corpora (e.g. safety-critical function identification from
  hazard/control/architecture graph intersections). These are
  kept application-side per noet-core's application-neutral
  content rule.
