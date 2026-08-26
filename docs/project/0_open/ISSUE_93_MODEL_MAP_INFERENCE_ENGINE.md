# Issue 93: Model Map — Inference Engine, Metadata Card, and Credibility Surface

**Priority**: HIGH
**Estimated Effort**: 2 days (split across 2 sessions)
**Dependencies**: Informed by [wp-model-ontology] §5-9 (projections,
geometry, E/F/M, credibility assessment), Issue 91 (content classifier),
Issue 85 (graph mode / 3D viewer), and the collaboration overlay design
(`docs/design/collaboration_overlay.md` — provides the identity/role
layer that parameterizes inference per user).

**Origin**: migrated from planning Issue 17. The inference algorithm,
metadata card, and credibility surface are application-agnostic noet-core
features that work on any compiled graph.

## Summary

Build the inference engine described in Issue 14 §8 as two concrete
rendering surfaces: a **metadata card** (per-node dashboard in the
viewer) and a **credibility surface** (rendered onto the 3D graph).
Both express the same underlying computation: given a focus node,
traverse its couplings, classify them using EMO primitives, and report
what the node has, what it's missing, and how well-grounded its claims
are.

The engine works on any node — product, QMS document, standard, test
fixture. The procedure is the same; the E/F/M labels rotate relative
to focus. The deliverable is not a document but working code and a
pilot demonstration that validates universality.

## Architecture

### The inference algorithm

Given a focus node (any BID in the compiled graph):

1. **Identify the model boundary.** Consume boundary results from
   Issue 94. For this issue, start with **Tier 1: explicit boundaries**
   — network-kinded nodes define model scope via structural containment.
   This already exists in noet-core (`get_submap` with section edges).
   The inference engine works with any boundary source; Issue 94 adds
   paradigmatic heuristics (Tier 2) and coupling-density inference
   (Tier 3) as the boundary detection matures. A single node may
   belong to multiple models — the engine runs separately for each
   model context.

2. **Classify N/S/P content.** For each node in M's submap, read the
   content profile from the compiler (Issue 91). This tells you what
   the model carries on each axis.

3. **Classify E/F/M interactions.** For each coupling edge crossing
   M's boundary: if the source supplies N that governs M but is not
   in M's submap, it is M(F)→M' — factory norms shaping this model.
   If M's content is exercised against an external entity to produce
   R, it is M(E)→M' — verification/validation. F is relative: when
   M is a software component, F is Vast. When M is Vast, F is
   {standards bodies, regulators, physics}. The focus defines the
   boundary; the labels follow.

4. **Assess projection completeness.** The six projections (§5) predict
   what couplings SHOULD exist for a fully-formed model. Check which
   exist:
   - N→S coupling? (requirements traced to design)
   - N→P coupling? (requirements traced to verification)
   - S→P coupling? (design exercised by procedures)
   - R exists at couplings? (projections have been executed)
   - What was the surrogate E for each R? (fidelity)
   Missing projections are inferrable gaps — the engine reports them
   without needing a configured checklist.

5. **Assess credibility texture.** For each coupling with R:
   - **Opacity**: R exists? How recent in model-time?
   - **Fidelity**: what surrogate E produced the R? Validated fixture
     (high) vs. latent reviewer model (low)?
   - **Coverage**: how much of the operating domain was exercised?

6. **Report surprise state.** For each coupling:
   - Unresolved surprise? (open bugs, waivers, anomalies)
   - Predicted surprise? (open risks, planned tests)
   - Clean resolved history? (no outstanding signals)

7. **Assess P-internalization.** Where on the discovery→production
   spectrum is this model? This is not a discrete event but a
   gradient observable from R provenance:
   - R produced by factory-aggregate P (QMS review, program-level
     gate) → model is in discovery, factory-governed
   - R produced by model-dedicated P (assigned RE, dedicated test
     infrastructure, project CI/CD) → model is in production,
     self-governing
   - Mix of both → model is in synthesis (Phase 2), vesicle
     partially installed

   The key: each R event records which P produced it (§3.4 —
   production context includes P-identity). The inference engine
   reads the P-provenance across the model's R events to determine
   how internalized the model's execution capacity is. This
   requires that R observation streams identify which P-lens
   (procedure/role) they are projecting through — the same
   P-identity metadata that the collaboration overlay's attestation
   records provide.

### Two rendering surfaces

**Metadata card** (`noet-core/assets/viewer/metadata.js`): Per-node
dashboard. Today this shows flat lists of sources and sinks. The
enhancement adds a **model view** section that organizes relationships
to show:

- Which model(s) this node belongs to (explicit or inferred boundary)
- Projection completeness: which of the six cross products have
  couplings (✓/✗ or coverage fraction)
- E/F/M classification of boundary-crossing edges
- Credibility summary per axis:
  - N: how complete? (TBD/TBR count, traceability coverage)
  - S: how traced? (N→S coupling density)
  - P: what executor committed? (RE assigned? test infrastructure? CI/CD?)
  - R: how recent, what fidelity surrogate?
- Inferred missing relations: "this node has N coupled to S but no
  N(S)→P — verification planning is absent"
- Surprise state summary: unresolved count, predicted count

**Credibility surface** (3D viewer, Issue 85): The same data rendered
as texture on the graph. Opacity from R evidence. Color from fidelity.
Gaps visible as transparent regions on the wireframe. Unresolved
surprise visible as highlighted edges. This is the §7.5 wireframe +
texture rendering made concrete.

Both surfaces consume the same underlying computation. The metadata
card is the detail view (one node at a time). The credibility surface
is the overview (all nodes simultaneously, pattern-visible).

### Role-parameterized inference

P is categorically external to the legible model (§7.4): a model
cannot turn itself on. The observer — the person looking at the
metadata card — IS P. Their role determines which inferences from
the declarative graph state are relevant to them:

Each PII (Personal Inference Interface) surface presents the same
inference output parameterized by the user's role:

| Role | Primary inference | What the card highlights |
|---|---|---|
| Epistemic assessor | Epistemic grounding | Which couplings lack R? Which surrogates are unvalidated? Where is epistemic mass concentrated? |
| Resource planner | Resource commitment | Which models have no P committed? Where is the vesicle incomplete? What’s the gap between current state and gate requirements? |
| Verification engineer | Surrogate fidelity | Which simulation environments are available? What fidelity? What coverage of the operating domain? |
| Safety assessor | Safety coupling | Which models have E(M)→E’ effects on critical systems? Are controls verified? |
| Release authority | Deployment readiness | Is the controlled transition path from surrogate-validated to operational complete? |

The graph state is the shared declarative object. The inference
algorithm is parameterized by role. The collaboration overlay
(`collaboration_overlay.md`) provides the identity layer: when a
user authenticates with a role, the metadata card installs that
role’s inference lens. Same data, different derivations — because
different P asks different questions of the same (N, S).

This means the inference algorithm needs a **role specification
format**: a declarative description of which projections, which
E/F/M cells, which surprise states, and which gap types a given
role cares about. The collaboration overlay’s credential model
(§4a) already defines peer-derived credentials; the inference
algorithm can consume these as role selectors.

The existing procedure system (`docs/design/procedure_schema.md`,
`procedure_execution.md`, `redline_system.md`) is the early
incarnation of this. Its three-layer architecture maps directly
to EMO primitives: Intention = N (what should happen), Execution
= P (the runtime exercising structure), Reality = R (as-run
record). The redline system is the surprise lifecycle: template =
predicted surprise, as-run deviation = unresolved surprise,
template promotion = resolved surprise (N updated from R). The
`inference_hint` / observation channel mechanism is the wiring
that connects different P to the same S. The procedure schema is
the role specification format — it defines what an executor
should attend to in a given context. The inference engine
generalizes this from domain-specific procedure execution to
universal graph inference: different roles install different
procedure-like lenses on the declarative belief network.

### Delta as focus

The inference algorithm's focus need not be a single node. A pull
request, ticket, or release is a **delta** against the compiled
graph — a set of added, modified, or removed nodes and edges. That
delta defines a query scope that the inference engine evaluates
exactly as it would a single-node focus: what projections are
complete within this scope? What couplings cross the boundary? What
R exists?

This enables a **rule map** pattern: a declarative specification of
which inference procedures fire based on what the delta contains.
"If the delta touches N→S couplings in a safety-classified model,
activate the SA review procedure. If the delta adds S content with
no N→S coupling, flag a traceability gap." Rule maps are the
operational form of project-specific guidelines and standards —
they define what "sufficient" means for each content type without
prescribing the process. The rules are structural consequences of
the EMO's projection algebra, not arbitrary if-then patterns.

A **role** is a list of active rule maps. The process standard
defines which rule maps must be active for a given role. The
collaboration overlay provides the identity; the rule maps provide
the inference; the compiled graph provides the shared object.

### R and surprise as halo connections

The inference engine assesses credibility texture and surprise state,
but the compiled belief network does not currently represent R or
surprise. These are a separate graph layer — federated attestation
shards that connect into the belief network as "halo" connections
via the attestation fabric (`docs/design/attestation_fabric.md`).

The belief network is the observed model (compiled from sources of
truth, not directly manipulated). The R/surprise layer is the dynamic
surface where P acts. Users control which R/surprise sources are
active based on their role configuration — different attestation
servers provide different halo connections (defect tracking R,
automated test R, human review R).

The inference engine consumes these halo connections when assessing
credibility texture and surprise state. The execution loop closes
when the engine's gap findings trigger procedures that emit
BeliefEvents into the attestation service, which creates R records,
which connect back as halo edges, which the engine reads on its
next evaluation.

This architecture is specified in planning Issue 16 (surprise
lifecycle protocol). The inference engine defined here is the
consumer; the attestation service is the producer.

### Universality validation

The pilot must demonstrate that the same algorithm produces meaningful
output on at least two structurally different focal entities:

- A **product node** (a model with milestones, requirements,
  verification events)
- A **process document node** (a factory N document with compliance
  mappings, gap analysis couplings, audit findings as R)

If the inference algorithm and metadata card produce meaningful
completeness/maturity assessments on both, the universality claim is
validated.

## Steps

1. Design the inference algorithm and card layout (0.5 day)
   - [ ] Consume model boundary from Issue 94 (start with Tier 1:
         explicit network containment via `get_submap`)
   - [ ] Specify the projection completeness checks (which coupling
         patterns map to which projections)
   - [ ] Specify the E/F/M classification rule (focus-relative:
         F = N not in M's submap)
   - [ ] Specify the credibility texture computation (R→opacity,
         surrogate provenance→fidelity, domain coverage→coverage)
   - [ ] Design the metadata card layout — what sections, what
         visual indicators, what interaction (click to navigate to
         the gap)
   - [ ] Draft the MCP query patterns for each inference step

2. Implement metadata card enhancements (0.5 day)
   - [ ] Consume model boundary from Issue 94's Tier 1 output
   - [ ] Add projection completeness section to card
   - [ ] Add E/F/M classification section to card
   - [ ] Add credibility summary section to card
   - [ ] Add inferred-gaps section to card

3. Pilot on two focal entities (0.5 day)
   - [ ] Select pilot product node (criteria: mid-complexity, active
         milestones, verification events, requirements traceability)
   - [ ] Select pilot process document node (a factory N document
         with compliance mappings and gap analysis couplings)
   - [ ] Run inference algorithm on both, capture metadata card output
   - [ ] Validate: does the same algorithm produce meaningful output
         on both?

4. Connect to credibility surface (0.5 day)
   - [ ] Define how metadata card data maps to 3D viewer texture
         (opacity, color, highlight)
   - [ ] Implement credibility overlay as a viewer mode (toggle on/off)
   - [ ] Verify that transparent regions on the 3D graph correspond
         to gaps identified on the metadata card

## Done When

- [ ] Metadata card shows model view with projection completeness,
      E/F/M classification, credibility summary, and inferred gaps
      for any node in the viewer
- [ ] Credibility surface renders on the 3D graph as a toggleable
      overlay
- [ ] Pilot product node produces meaningful completeness/maturity
      assessment
- [ ] Pilot process document node produces meaningful assessment
      using the same algorithm (universality validated)

## Risks

- **Model boundary is a separate hard problem**: see Issue 94.
  This issue starts with Tier 1 (explicit network containment)
  which requires no inference. Implicit boundary detection layers
  on via Issue 94 without changing the inference engine's API.
- **Content profile (Issue 91) may need recalibration**: the P
  refinement (P is executor, not procedure; classifier detects S(P))
  changes how the content profile is interpreted. → **Mitigation**:
  the classifier's detection mechanism is unchanged — only the
  label interpretation shifts. Recalibration is Issue 91 follow-on
  work, not a blocker.
- **Pilot nodes may lack sufficient corpus data**: coverage varies
  across any compiled graph. → **Mitigation**: select pilot based
  on corpus density (use edge count and submap depth to find
  well-connected nodes).
- **Process document pilot may lack R**: factory N documents may have
  few verification events or audit findings in the corpus. →
  **Mitigation**: sparse R IS the finding — the metadata card
  should show "no R at these couplings" as a credibility gap,
  which is meaningful output.

## Relationship to Other Work

- **Issue 91** (content classifier): provides the N/S/P content
  profile consumed by the inference algorithm. Interpretation of
  the P score shifts (detects S(P), not P directly) but the
  classifier code is unchanged.
- **Issue 85** (3D credibility viewer): the credibility surface is
  the rendering mode this issue adds to the viewer
- **Collaboration overlay** (`docs/design/collaboration_overlay.md`):
  provides the identity/role layer that parameterizes inference per
  user. Attestation records are R with P-identity (who produced this
  observation). Credentials are role selectors for the inference
  algorithm. Sign-off policies are gate specifications tied to
  specific P-credentials.
- **Issue 94** (model boundary inference): provides the boundary
  detection that this issue's engine operates within. Tier 1
  (explicit network containment) is sufficient to start. Tiers 2-3
  add paradigmatic heuristics and coupling-density inference.
- **[wp-model-ontology]** (`docs/essays/engineering_model_ontology.md`):
  §5 (projections), §7 (E/F/M + vesicles), §7.5 (wireframe + texture),
  §8 (coupling + registration), §9 (credibility assessment) — the
  ontological foundations the inference engine implements.
- **Planning Issue 17** (`planning/project/ISSUE_17_*`): the
  application-specific pilot (a systems-engineering product corpus, a QMS
  process document corpus, 7009B crosswalk) that validates this engine
  against a real corpus.
