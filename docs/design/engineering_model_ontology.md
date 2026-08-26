---
title: "Engineering Artifacts as Model Content: A Compositional Ontology"
id: wp-model-ontology
version: 0.1
---

## Abstract

Engineering artifacts — requirements, designs, test plans, execution records —
are managed as documents, but they are content that participates in models.
This paper proposes a compositional ontology based on three content types
(**Normative**, **Structural**, **Procedural**) and an observation stream
(**As-run records**). Six generative projections — cross products on the
`N×S×P` axes — describe how content types produce each other; inner products
compare predictions against `R` to evaluate surprise. `N`, `S`, and `P` are
genuinely orthogonal spatial dimensions of model-space; `R` is categorically
different — it is the time dimension, consisting of immutable observation
events pinned to specific model-spacetime coordinates. The cross-product
structure, the informational mass of each model (its dependency chain depth),
and the `R`-as-opacity mapping define a **credibility rendering pipeline**:
the coupled model network can be projected into a navigable 3D map where
position encodes content type, size encodes mass, and opacity encodes
proximity to observation events in model-time. This rendering compresses
the information currently in hundreds of pages of traceability matrices and
credibility assessments into a single visual object where epistemic risk
concentrations, coverage gaps, and coupling health are spatially apparent.

---

## 1. Introduction

Engineering organizations produce artifacts — requirements documents, design
specifications, source code, test plans, procedures, and execution records. These
artifacts are typically managed as documents: versioned, reviewed, approved, and
stored. But managing artifacts as documents obscures what they actually are: content
that participates in models.

A **model** is a systemization of how some observable entity is expected to behave.
This definition has three load-bearing components. *Systemization* means the model
is structured — it does not merely record past observations but organizes them into
a generative account that produces predictions about unobserved states and future
behavior. *Observable entity* means the model has a referent that exists
independently of the model itself — the model is about something, and that
something can in principle be measured. *Expected to behave* means the model makes
commitments: it asserts that under specified conditions, the referent will produce
specified outputs. A model that makes no commitments is not a model — it is a
description.

The most important consequence of this definition is that **a model can be wrong**.
Not just incomplete or approximate — genuinely wrong, in ways that are detectable
by confronting the model's expectations against the referent's actual behavior.
This capacity to be wrong is what makes a model a model rather than a tautology.
And the mechanism by which wrongness is detected is **surprise**: an observation
that the model did not predict, arriving at a magnitude the model did not expect.
A model that cannot be surprised is either perfectly calibrated (practically
impossible) or has been constructed to be unfalsifiable (useless). The degree
to which a model can be surprised — and learns from that surprise — is the measure
of its epistemic health.

This framing unifies several observations that are treated as separate in the
literature. The distinction between verification and validation (Banks, 1998;
ASME V&V 10) is derivable from first principles within this framework: both
follow the universal projection structure. The generative model `P(S)` produces
predicted observations `R'`; actual observations `R` supply the comparison stream;
the normative scoring function `N` evaluates the divergence: `N(R, R') → surprise`.
The difference is the provenance of `R`: validation evaluates `R` produced by the
world actually running (`P(s_t) → s_{t+1}` where `s_t` is a Real-world system
(RWS) state), while verification evaluates `R` produced by a model-internal process
(a test harness, formal checker, or controlled environment). Verification is
internal surprise — model versus its own specification. Validation is external
surprise — model versus world. The operation is the same; the anchor of the
evidence differs.

The aleatory/epistemic uncertainty distinction (Der Kiureghian and Ditlevsen, 2009)
is a classification of surprise sources: aleatory surprise comes from genuine
randomness in the referent that accessible observations cannot eliminate; epistemic surprise comes
from gaps in the model's systemization that could, in principle, be reduced by
knowledge acquisition. And the Goodhart failure mode — when a measure becomes a
target and ceases to be a good measure — is the specific pathology where a model's
internal optimization pressure decouples the model's predictions from its referent,
producing a model that cannot be surprised externally because it has stopped
engaging with the world it was supposed to represent.

A requirements document is not a model; it carries the
normative content that partially constitutes one or more models. A design
specification is not a model; it carries structural content about
those models. A test campaign is not a model; it generates the observational record
that validates them.

The cybernetic embodiment framework (CE) provides a foundational ontology for how
learning systems coordinate through normative surfaces. In that framework, each
system maintains a generative model with a predictive component `p(o|s)` — the
domain-specific structural content mapping hidden states to expected observations.
CE establishes that `p(o|s)` is **local rigging**: it stays private across the
coordination boundary because publishing it would require receivers to internalize
the sender's domain-specific dynamics. What crosses the boundary instead is
normativity — preferences and constraints that other systems can read and
coordinate against without inspecting the model's internals (CE Part IV §2.4).

This paper asks the question CE's boundary implies but does not answer: **what
does it take to communicate `p(o|s)` in a manner that enables principled
capability assessment?** When an engineering organization must evaluate whether a
model's predictions are trustworthy — whether its generative model faithfully
represents the real-world system it claims to represent — the `p(o|s)` cannot
remain opaque. But it also cannot be shipped raw: a thermal model's equations, a
flight software binary, and a team's coordination dynamics are incommensurable.
The answer proposed here is a decomposition of `p(o|s)` into independently
assessable facets: the norms it should satisfy, the structure it acts on, the
procedures that exercise it, and the observations produced when it is confronted
with reality.

This paper proposes a compositional ontology for engineering artifacts based on
three content types and an observation stream: **Normative** (`N`), **Structural**
(`S`), **Procedural** (`P`), and **As-run records** (`R`). `N`, `S`, and `P` are the
spatial dimensions of model-space — the coordinate axes along which a model's
content is distributed. `R` is categorically different: it is the time dimension.
Each `R` is an observation event pinned to specific model-space coordinates at a
specific moment in model-time, immutable once recorded. A model's `N`, `S`, and `P`
content evolves; its `R` events are fixed points in the causal structure. Six
generative projections — cross products on the `N×S×P` axes — describe how content
types produce each other; inner products compare predictions against `R` to evaluate
surprise. The cross-product structure is what makes `N`, `S`, and `P` genuinely
orthogonal: each is the predicted output of the other two, and the prediction can
be wrong.

This framework has four consequences. First, it grounds the organizational belief
network ([wp-connectome]) in a precise theory of what "models" are and how they
couple. Second, it provides a principled basis for model credibility assessment:
assessment factors map onto the content types and their projection relationships.
Third, it resolves the boundary question — a model needs to be formally registered
when its `N` content becomes an ontology dependency for another model's `S` content.
Fourth, the 3+1 dimensional structure defines a **credibility rendering pipeline**:
the coupled model network can be rendered as a navigable map where position encodes
content type, size encodes informational mass (dependency chain depth), and opacity
encodes proximity to `R` events in model-time. This rendering compresses information
currently spread across hundreds of pages of traceability matrices, credibility
assessments, and gap analyses into a single visual object where epistemic risk
concentrations, coverage gaps, and coupling health are spatially apparent —
transferring the chief engineer's mental model into a shareable, queryable,
version-controlled artifact.

---

## 2. The Problem with Artifact-Centric Engineering

Engineering organizations manage artifacts. Every mature engineering process
specifies which artifacts must be produced, reviewed, and approved: requirements
documents, design documents, interface control documents, test plans, test reports,
analysis reports, and so on. This artifact-centric view is not wrong — artifacts
are the observable outputs of engineering work, and managing them provides
traceability, auditability, and organizational memory.

But the artifact-centric view systematically obscures the relationships between
artifacts that give them meaning. A requirements document disconnected from any
design or test is a collection of assertions with unknown consequences. A design
document disconnected from requirements or test results is an undocumented
implementation whose correctness cannot be assessed. A test plan disconnected from
the design it is testing is steps without a referent. The artifacts are not
independently meaningful — they are meaningful in relation to each other, and the
relations are structured.

The artifact-centric view also removes the principled basis for forgetting. When
an artifact is managed as a document, its continued existence is governed by
approval status and document control processes, not by whether it is still doing
epistemic work. Requirements that no longer trace to any active design, procedures
that exercise systems no longer in the design, analyses whose conclusions have been
superseded by newer `R` — none of these can be identified as candidates for
retirement by a document management system. They are alive in the sense that they
exist, are under configuration management, and appear in compliance matrices. They
are dead in the sense that they are no longer participating in any active model
relationship — no coupling depends on them, no `S` implements their `N`, no `P`
exercises their claimed domain. Just as finite element topology optimization
removes material where stress is low — where matter is not doing structural
work — the model-centric view enables removal of content where coupling is
absent: informational topology optimization that reduces the compliance surface
without reducing epistemic or pragmatic capability.

Without this basis, the result is **organizational calcification**: the proliferation of policy,
procedures, and structural artifacts that are functionally dead code. The compliance
surface grows — more requirements to satisfy, more procedures to follow, more
analyses to maintain — while the live epistemic content of the engineering program
does not. Each iteration of a program inherits the accumulated artifacts of prior
iterations, with no systematic method for distinguishing the living content from
the necrotic content. Reviews cite requirements that nobody knows why they were
written. Tests exercise conditions that no longer correspond to any failure mode
in the current design. Standards are invoked whose applicability to the current
program was established a decade ago under different assumptions. The organization
cannot shed the dead weight because it cannot see it.

A model-centric view provides the basis for principled forgetting. An artifact
can be retired when the coupling it maintains — the normative or surrogate
relationship it participates in — is no longer active. `N` content can be retired
when no `S` implements it and no model's design decisions are conditioned on it.
`S` content can be retired when no downstream `P` consumes its outputs in a surrogate
coupling. `P` content can be retired when the `S` it exercises has been superseded and
no new `R` from it contributes to any credibility assessment. Retirement is not
deletion without evidence; it is the explicit recording of which coupling ended,
when, and why — a death that can be felt rather than a silent disappearance. The
archive of retired couplings is itself epistemic evidence: it documents what the
organization has learned, what it has outgrown, and what it chose to stop tracking.

The organizational belief network framework ([wp-connectome]) identifies this relational
structure as the engineering program's primary epistemic object: the graph of
models, assumptions, and inter-disciplinary agreements that the organization holds
about the system it is building. But that paper treats "model" informally — the
meaning of "model" is taken as understood, and the compilation of artifacts into
a belief network is described at the level of document types and cross-references.

This paper makes the model concept precise. The central claim: **artifacts are not
models; they are carriers of content on which models are built.** A model is a
coherent assembly of three content types, and a set of six generative projections
describes how those content types produce each other.

---

## 3. Three Content Types

Every durable engineering model contains three types of content, each with a
distinct structural character. These content types are not artifact categories —
a single artifact may carry multiple content types — but they can be identified by
their structural properties and their role in model relationships.

The three types are deliberately sub-Turing in isolation. `N` is a constraint set:
declarative, with no execution model. `S` is a read-only tape: structure without
agency, specifying what something is but incapable of producing state transitions
on its own. `P` is logic without persistent state: it reads `S` and drives
`s_{t+1}`, but it does not own the tape. None of the three can compute alone.
Turing-complete behavior emerges from their interaction: `P(S) → s_{t+1}`, where
`s_{t+1}` becomes the next `S` in the dynamic system `Env + Model`, is a
read-eval loop that is Turing complete. The r/w behavior of the system is not a
property of any single type but of their composition. `N` then acts as the halting
oracle — the condition under which the loop's trajectory is evaluated as acceptable
or not. This decomposition is principled: separating the constraint surface (`N`),
the structural description (`S`), and the operational logic (`P`) makes each
component independently auditable and the interactions between them explicitly
typed.

The decomposition also has a physical correspondence[^4] that explains why these
three axes and not some other partition. `S` corresponds to **matter**: discrete,
compositional, persistent — things that *are*. `P` corresponds to **fields**:
continuous operators that mediate interactions and carry dynamics — things that
*act*. `N` corresponds to **information**: relational, perspectival, encoding —
things that *mean*. This correspondence is not a metaphor; it reflects the fact
that engineering models are models of physical systems, and the content types
inherit the structure of the phenomena they represent.

[^4]: The physical correspondence has a consequence for the measurement geometry of the framework. Information (N) is both one of the three axes and the encoding medium for all three — you cannot externalize S or P content without encoding it as information. This means the three axes are orthogonal in the latent phenomenon but inherently non-orthogonal in the expressed model: externalization introduces a systematic skew toward the N plane. This encoding parallax is one reason the expressed N, S, and P are lossy projections of the latent model (§5), beyond the dimensional loss from finite fidelity. Surprise evaluation on the N axis (comparing normative predictions against actuals) has minimal parallax because the measurement apparatus is in its own medium. Surprise evaluation on the S and P axes has greater aberration because the comparison is between two N-encoded representations of non-informational phenomena. In a complex model system (a lens system with many coupled sub-models), these aberrations cascade: each coupling introduces its own encoding parallax, and the accumulated distortion across a chain of surrogate couplings is not easily decomposed into its per-element contributions. This is why structural and procedural errors typically require testing (applying P to S to produce R that bypasses the N-encoded chain) rather than document review alone — document review propagates through the aberrated channel, while testing produces R that contacts the latent phenomenon directly.

### 3.1 Normative Content (`N`)

Normative content consists of individually-standing assertions about what should
be true. Each element of `N` is independently meaningful: it can be read, evaluated,
and traced without requiring the other elements to be present. Requirements
statements, design constraints, interface specifications, safety criteria, and
standards are all `N` content. `N` is the array structure of a model — a set whose
elements are individually assertable.

`N` has two sub-modes that compose into a single preference ordering:

**Constraints** are hard boundaries on the feasible design space. A constraint is
violated when the observable state of the system falls outside its declared
boundary. Constraint violation produces acute surprise — the boundary was crossed,
the signal is sharp, and the propagation through the model network is
high-conductance. Examples: "Maximum structural deflection shall not exceed 5mm
under design load," "Software response latency shall be less than 10ms."

**Optimization signals** are gradients that select within the feasible region —
they order the acceptable states rather than excluding the unacceptable ones.
Optimization surprise is chronic: the gradient is travelling the wrong direction,
or the proxy used to operationalize the gradient has decoupled from the real
objective. Examples: "Minimize dry mass," "Maximize reuse of qualified heritage
components," "Minimize time-to-diagnosis for operator-facing fault conditions."

The distinction matters for credibility assessment and for surprise propagation
dynamics. Constraint violations are detectable by direct comparison of observation
to threshold. Optimization signal failures require tracking the proxy relationship
over time and detecting drift — the Goodhart failure mode described in [wp-connectome].

`N` content has a special role in inter-model coordination: it is the **exposed
preference surface** of a model, published at an abstraction level that other
models can read and coordinate against without inspecting the model's full internals.
A safety analysis model publishes `N` content (safety requirements, hazard controls)
that the design model's `S` content must satisfy. The `N` content is the cross-model
coordination interface — the finger pointing at the moon, not the moon itself.

### 3.2 Structural Content (`S`)

Structural content describes how something is composed. `S` has the property that
the whole is semantically prior to its parts: sub-elements can be named and
referenced, but they cannot be extracted without losing the context that gives them
meaning. Design documents, architecture specifications, mathematical models,
implemented source code, and physical hardware configurations all carry `S` content.
`S` is the nested, self-referential structure of a model — a graph whose nodes
derive meaning from their position within the whole.

`S` is a **passive structural description** — it specifies what something is, not
what it does. A CAD assembly specifies geometry and material; flight software
specifies a transistor configuration (the binary); a simulation specifies model
equations and parameters; an org chart specifies roles and reporting lines. `S`
does not itself produce observations or transitions. What produces states is `P`
acting on `S`: the thermal equations act on the geometry, the physics of
transistor switching acts on the binary, the numerical integrator acts on the
model equations, the coordination dynamics act on the org structure. In the active
inference framework (Friston, 2010), `p(o|s)` — the generative model mapping
hidden states to expected observations — corresponds to the `P(S)` pair taken
together, not to `S` alone. `S` is the argument; `P` is the operator; the
generation requires both.

This is why structural content is the target of credibility assessment (§8).
`S` is what is being trusted when a downstream model treats a `P(S)` output as
an RWS substitute. Errors in `S` — wrong geometry, wrong binary, wrong equations
— propagate through every `P` that acts on it, producing systematically wrong
`s_{t+1}` that no amount of procedural rigor in `P` can correct. The credibility
question is always: does `P(S)` produce states consistent with `P(RWS)` within
the claimed operating domain?

`S` content is hierarchically decomposable: a system-level structural model is
composed of subsystem structural models, each of which may be further decomposed.
This recursion is not unlimited — at some level of granularity, the sub-elements
are primitive enough to be treated as atomic. The boundary between "this model's
`S`" and "its sub-models' `S`" is a design decision about model granularity (§10).

### 3.3 Procedural Content (`P`)

Procedural content is a chronologically ordered sequence with a beginning, middle,
and end. `P` operates in time: it has preconditions, steps, and postconditions. It
may contain loops, branches, and dependencies, but it is fundamentally narrative —
it describes something that happens, not something that is. Verification plans,
manufacturing processes, test campaigns, integration sequences, and operational
procedures all carry `P` content.

`P` is the operator that acts on `S` to produce states — the logic in the
read-eval loop. When `P` is applied to `S`, it drives the dynamic system
`Env + Model` forward: `P(S) → s_{t+1}`. `R` is then the measurement of `s_{t+1}`
by some observing instrument — itself a `P` operation on the resulting state.
`S` is inert without `P`; `R` is unreachable without both. Without `P`, a model
with `N` and `S` content has a structural description and normative claims but no
mechanism for producing the states that generate evidence. The read-write behavior
of the full system emerges from `P(S) → s_{t+1}` iterated — not from any property
of `P` or `S` in isolation.

`P` is also the mechanism of generation: every model must have been created somehow,
and the creation process is procedural. The development process that produced `S`
(the design process, the implementation process, the analysis methodology) is `P`
content. The model's `N` content constrains that development `P`; the quality of the
`P` and the fidelity with which `N` constrained it are factors in the model's
credibility.

### 3.4 As-Run Records (`R`)

As-run records are measurements of world-state at a point in time: test results,
telemetry data, review minutes, build logs, anomaly records, operational data.
`R` is not authored content — it is a read of `s_t`, a snapshot of the state of
the dynamic system `Env + Model` at the moment of observation. `R` has no model
owner and cannot be wrong in the way `N` or `S` can be wrong; it simply records
what was.

This categorical difference between `R` and the three content types has a
geometric interpretation developed in §6: `N`, `S`, and `P` are the spatial
dimensions of model-space; `R` is the time dimension. Each `R` packet is an
event pinned to specific coordinates in model-spacetime, immutable once
recorded. The model evolves through `N×S×P`; its `R` events are fixed points
in the causal structure.

`R` becomes epistemically useful when consumed as the comparison stream in the
universal projection structure: the generative model `P(S)` produces predicted
observations `R'`; `R` supplies the actual observations; the normative scoring
function `N` evaluates the divergence — `N(R, R') → surprise`. This is the unified
form of both verification and validation — the difference is the provenance of the
`R` being evaluated:

- **Validation `R`**: produced by the world actually running — `P(s_t) → s_{t+1}`
  where `s_t` is a RWS state. The resulting surprise is RWS-grounded: the
  divergence was measured against real system behavior.
- **Verification `R`**: produced by a model-internal process — a test harness,
  formal checker, or controlled environment standing in for the RWS. The resulting
  surprise is model-internal: the divergence was measured against the model's own
  behavior under controlled conditions.

In both cases the operation is identical; the credibility weight of the resulting
surprise signal depends on how closely the process that produced `R` approximates
RWS conditions. Statistics on `R` across the operating domain — how much of the
constraint surface has been checked, against what provenance of `R`, with what
margin — constitute the model’s credibility evidence. A region where `R` is rich
and RWS-grounded is well-anchored. A region where `R` is absent is epistemically
unanchored. A region where `R` was once present but `S` has since changed without
new `R` is necrotic — a stale model trusted as live, the most dangerous case.

### 3.5 Grammatical Signatures of Content Types

The `N/S/P/R` decomposition has a natural correspondence in grammatical voice and
tense. Each content type is characterised by how it speaks about its subject,
independently of what the subject is:

| Content type | Voice / tense | Examples | Signal |
|---|---|---|---|
| `N` | **Passive voice, modal future** | “shall be maintained”, “is required”, “must not exceed” | Entity boundary specification — declaring what something is or must be |
| `S` | **Present indicative, compositional** | “consists of”, “contains”, “routes from X to Y” | Structural description — but primarily identified by graph topology, not lexicon |
| `P` | **Dynamic: imperative, future, causal, transitional** | “begin the sequence”, “will initialize”, “if X then Y”, “triggers a shutdown” | What happens, what causes what, and in what order |
| `R` | **Past tense, data-like** | “measured 5.03 volts”, “observed at 14:32”, “passed” | Temporal anchoring — reporting what already happened |

The `P` signature has several sub-categories that all express the same
underlying property — *dynamism* (temporal extent and causation):

- **Imperative verbs**: commands and actions — “execute”, “verify”, “connect”
- **Future tense**: prediction of what happens next — “will initialize”
- **Logic conjunctions**: branching — “if”, “else”, “unless”, “while”
- **Sequential markers**: ordering — “then”, “next”, “first”, “step”
- **Causal verbs**: dynamics — “causes”, “triggers”, “produces”, “yields”
- **State transition verbs**: movement between states — “transition”, “switch”,
  “enter”, “exit”, “become”

By contrast, `N` is fundamentally *static* — it declares timeless boundaries.
The grammatical difference between `N` and `P` is the difference between
describing *what is* (or what must be) and describing *what happens* (or what
will happen). Passive voice naturally expresses `N`: the entity is the
grammatical subject being acted upon by the specification. `N` includes
descriptive-definitional text (“is configured with”, “are specified as”) that
establishes the referent’s identity — description is specification, because it
defines the boundary of the referent as an entity of attention.

`R` content reports what *already happened* — past tense is the natural
expression of an observation pinned to a moment that has already passed and
cannot be un-observed. `S` is the weakest lexical signal because structural
content is primarily identified by its position in a reference graph (what
refers to what) rather than by distinctive vocabulary.

The grammatical mapping provides a domain-independent lexical classifier: the
signal is in the voice and tense, not in the nouns. “The interface shall maintain”
and “the gasket shall withstand” are both `N` regardless of domain; “a voltage
spike triggers a protective shutdown” is `P` regardless of the system;
“measured 5.03 volts” and “recorded 23.1 degrees” are both `R` regardless of
what was measured. This makes the classifier robust across engineering domains
without recalibration.

---

## 4. A Model Implies All Three

Any artifact that performs a genuine modeling role implies all three content types,
even when one or more are not yet externalized. The missing content is latent —
held implicitly in an engineer's head, embedded in organizational convention, or
distributed across informal practice — rather than absent. Recognizing this changes
the diagnostic question from "is this model broken?" to "where is the missing
content, and at what cost is it being held there?"

| Apparent content | Latent content | Epistemic cost of leaving it latent |
|-----------------|----------------|-------------------------------------|
| `N` only | `S` lives in the implementer's judgment; `P` lives in informal test practice | Wrongness is undetectable; internal consistency cannot rule out consistent error |
| `S` only | `N` is implicit in the designer's intent; `P` lives in informal use | No basis for evaluating outputs; no mechanism for generating validation evidence |
| `P` only | `S` is the tacit model the procedure was written against; `N` is the unstated pass/fail criterion | `R` is produced but uninterpretable; no model to evaluate against |
| `N` + `S`, no `P` | `P` exists as informal or assumed practice | Validity cannot be empirically confirmed; the verified-on-paper failure mode |
| `N` + `P`, no `S` | `S` is whatever the executor improvises | Procedure exercises nothing stable; `R` cannot be attributed to a persistent structure |
| `S` + `P`, no `N` | `N` is the executor's implicit pass/fail judgment | `R` is produced but the evaluation standard is private and irreproducible |

Models grow toward explicitness rather than appearing complete. The latent content
becomes load-bearing — and must be externalized — when coupling is established:
when another model's `S` is designed against this model's `N`, or when this model's
`S` enters a surrogate relationship. At that point, content held implicitly can no
longer be governed. This is the principled basis for the registration event (§7.3).

The six generative projections (Section 4) describe the structured operations by
which latent content is elicited and externalized.

---

## 5. Six Generative Projections

The three content types combine in six ways to generate or constrain each other.
Every `N`, `S`, or `P` artifact has a dual nature: as **data** (the outside view —
readable, diffable, queryable) and as **execution machinery** (the inside view —
the lens that generates predictions when exercised). A projection[^1] puts one
content type in execution mode (the function) and another in data mode (the
argument), producing a *predicted observation* on the output axis.

[^1]: The latent model is the operational reality. The expressed `N`, `S`, and `P` are lossy projections of it along three orthogonal axes — no externalized artifact fully reproduces the latent phenomenon.

The notation `F(arg) → R'` should be read as: the model exercises its internal
content of type `F` as a lens on an argument `arg` (in data mode) to produce `R'`
— a predicted observation of what content on the output axis should look like.
The prediction does not directly update the model. The model update is a separate
operation: the prediction `R'` is compared against the actual content `R` on that
axis, producing a **surprise** signal — `axis(R', R) → surprise`. Content changes
only when surprise is non-zero.

The **argument** (parenthesized term) is content in data mode — sourced from the
model itself or from an external model or artifact. The **function** (term before
the parenthesis) is content in execution mode — the internal lens. The **output**
(term after `→`) is the predicted observation on the third axis.

The six projections exhaust the combinations[^3]: three content types, each occupying
the function role once per pair of argument types. All six are regularized
inversions.

[^3]: The projections have cross-product character: each takes content on two axes and produces a prediction on the third — the missing orthogonal dimension. The six operations correspond to the six ordered pairs of three axes (the cross product is anti-commutative; swapping function and argument roles produces a different prediction, matching `P(N) → R'_S` ≠ `N(P) → R'_S`). The surprise evaluation `axis(R', R) → surprise` has dot-product (inner product) character: it compares two values on the *same* axis and produces a scalar. Cross products for inter-axis prediction, inner products for same-axis evaluation — together these form a complete algebra on the three-axis model space.

The expressed `N`, `S`, and `P` are not the latent model — they are
low-dimensional projections of it: lossy, finite-fidelity representations captured
along three orthogonal axes. `N` captures behavioral commitments independent of
mechanism. `S` captures compositional structure independent of the procedures that
exercise it. `P` captures operational sequence independent of the structure it acts
on. Each projection constrains the others — the internal content on one axis serves
as the regularizer when reconstructing content on another — but none is a lossless
encoding of the underlying phenomenon.

The function/argument assignment gives each projection a **signed direction**, not
just an axis. For any output axis, two projections predict onto it from opposite
orientations of the input plane. The sign carries physical meaning: it determines
whether the projection is **exploratory** or **exploitative**.

A model is analogous to an adaptive optical element — a lens in `N × S × P` space. When the
lens acts as a **source** at a coupling, it projects its internal geometry onto the
output — shaping the interference pattern that the world receives. This is
**exploitation**: the model imposes its structure on the environment. When the lens
acts as a **sink**, the interference pattern flows inward and reshapes the lens's
bulk — the aberration between the incoming wavefront and the lens surface tells
the model where it needs grinding. This is **exploration**: the environment
reshapes the model.

The directionality of the coupling determines which mode is active. The **sink**
(the more-interconnected, depended-upon end) defines the inner surface — it is the
referrer. The **source** (the more-discrete end) is the referent. Traversing from
sinks toward sources ablates the environment — the model looks outward through
its couplings to see what the world provides. Traversing from sources toward sinks
ablates the model — the world's content flows inward to reshape the model's
surface. This duality operates on all three projection planes:

| Output axis | Explore (sink: absorb, grind) | Exploit (source: project, shape) |
|---|---|---|
| `N` (norms) | `P(S) → R'_N`: world's matter reshapes self's norms | `S(P) → R'_N`: self's matter shapes the world's norms |
| `S` (structure) | `P(N) → R'_S`: world's information reshapes self's structure | `N(P) → R'_S`: self's information shapes the world's structure |
| `P` (field) | `S(N) → R'_P`: world's information reshapes self's processes | `N(S) → R'_P`: self's information shapes the world's processes |

This corresponds directly to the perceptual inference / active inference duality
in the free energy literature (Friston, 2010): explore updates the model; exploit
acts on the world. The two orientations on each plane are irreducible — collapsing
them loses the sign information that distinguishes learning from acting.

Although a model can be conceptualized as a single lens, it can also be
decomposed into a **system of lenses** — sub-models with their own `N`, `S`, and
`P` content, coupled to each other through the same projection structure. This is
what sub-modeling and dependency relationships do: each sub-lens has its own
projection properties, and the composed system's behavior emerges from their
interaction. The structural containment hierarchy (§7.1) ensures that every
epistemic and procedural coupling has a structural home.

This is why iterated projection is the convergence mechanism. Each round-trip
(e.g. `P(N) → R'_S` followed by `N(S) → R'_P`) folds the model's own state
through a different axis — an adaptive optics cycle that grinds the lens surface
toward less aberration on each pass. When the argument is sourced from a different
model, the projection is an inter-model operation — the normal case for coupled
systems. Whether iterated predict-evaluate-update cycles converge to a fixed
point, oscillate, or diverge depends on the initial configuration and the
structure of the model-space (§10).

![The Model Cube: Six Projections in N × S × P Space](figures/model_cube_projections.svg)

![The Universal Projection Cycle: Predict → Evaluate → Update](figures/projection_cycle.svg)

### 5.1 The Six Projections

Each projection below describes what the operation *predicts* on its output axis.
The prediction is subject to surprise evaluation against actual content — the model
update is downstream of the comparison, not a direct consequence of the projection.

**`P(N) → S`** — exploit (Top-down implementation): The model's internal development process
(`P`) is applied to normative requirements (`N`) to produce a structural
implementation (`S`). `N` constrains the solution space;  `P`
selects within it.

*Example*: A set of interface latency requirements from a customer specification
(external `N`) is applied through a software architecture process (internal `P`) to
produce a service decomposition and data-flow design (`S`). The requirements bound
which architectures are admissible; the process selects among them.

*PM register*: A budget ceiling and delivery schedule from a program directive
(external `N`) is applied through a work-breakdown and staffing process (internal `P`)
to produce a project plan (`S`) — the structural implementation of the commitment.

**`P(S) → N`** — explore (Interface definition): The model's internal analysis methodology
(`P`) is applied to an external structural implementation (`S`) to derive normative
constraints (`N`) governing its interfaces. The procedure observes how the external
`S` behaves at its boundaries and codifies what it finds.

*Example*: An integration test campaign (internal `P`) run against a supplier's
delivered subsystem (external `S`) produces the interface control document (`N`)
specifying the timing, data format, and error-handling constraints that all
downstream consumers of that subsystem must satisfy.

*PM register*: A capacity assessment process (internal `P`) applied to an existing
team's demonstrated velocity (external `S`) produces the throughput constraints
(`N`) — rate limits, queue depths, dependency windows — that bound what the
program plan can commit to.

**`S(N) → P`** — explore (Operational procedure derivation): The model's internal structural
implementation (`S`) is evaluated against external normative constraints (`N`) to
derive the procedures by which the implementation correctly and safely performs its
intended function. The procedures are elicited by the combination of what the
structure can do and what the norms require of it — they are not written from
scratch.

*Example*: A database system's replication architecture (internal `S`) is evaluated
against external data-durability and recovery-time requirements (`N`) to derive the
backup schedule, failover procedure, and operator runbook (`P`) that correctly
operationalize the architecture within its normative constraints.

*PM register*: A team's organizational structure and toolchain (internal `S`) is
evaluated against external delivery commitments and reporting requirements (`N`) to
derive the sprint cadence, review gates, and escalation procedure (`P`) that
correctly operationalizes the team's capacity against its commitments.

**`S(P) → N`** — exploit (Procedure-bounded constraint derivation): The model's internal
structural implementation (`S`) is evaluated against external execution procedures
(`P`) to derive the normative boundaries accessible to the model within those
procedural constraints. This projection produces the boundary conditions under
which the model's claims are epistemically grounded — the entire observable space
that the model can accommodate. This is equivalent to model-checking, such as
implemented in LTL or CTL.

*Example*: A control system's implemented state machine (internal `S`) is
exhaustively explored against a qualification test suite from a certification body
(external `P`) to derive the full set of reachable states and transition
properties (`N`) — the complete behavioral envelope that the state machine can
exhibit under the conditions the test suite exercises.

*PM register*: A project's current tracking and reporting infrastructure (internal
`S`) is evaluated against the audit and review procedures required by a program
oversight body (external `P`) to derive the complete set of observable metrics and
status categories (`N`) — everything the infrastructure can actually measure and
report under those procedures.

**`N(P) → S`** — explore (Procedure-feasibility-filtered design): The model's internal
normative content (`N`) is applied to external development or qualification
procedures (`P`) to derive the structural implementation that the available
procedures can actually build, exercise, and verify. This projection acts as a
feasibility filter: it eliminates designs that satisfy `N` on paper but cannot
be realized or confirmed given the `P` actually available.

*Example*: A system's safety requirements (internal `N`) are evaluated against the
qualification test procedures available at the intended test facility (external
`P`) to derive the design configuration (`S`) that satisfies the requirements and
is fully exercisable by those procedures — excluding configurations that would
require test capabilities the facility does not have.

*PM register*: A program's contractual deliverable requirements (internal `N`) are
evaluated against the review and acceptance procedures specified by the customer
(external `P`) to derive the work product structure (`S`) — document formats,
traceability matrices, artifact naming — that satisfies the requirements and
passes through the customer's acceptance process.

**`N(S) → P`** — exploit (Verification and validation planning): The model's internal
normative content (`N`) is applied to an external structural implementation (`S`) to
derive the procedures by which the implementation can be shown to satisfy the
requirements. This is the canonical V&V planning operation, and the primary
mechanism by which normative couplings are made empirically checkable.

*Example*: A set of reliability and fault-tolerance requirements (internal `N`) is
evaluated against an implemented service architecture (external `S`) to produce the
failure-injection test plan and coverage matrix (`P`) that demonstrates the
architecture satisfies the requirements under the specified fault conditions.

*PM register*: A program's schedule and budget commitments (internal `N`) are
evaluated against the current project plan and burn rate (external `S`) to produce
the earned-value measurement procedure and variance-reporting cadence (`P`) that
keeps the commitments empirically checkable throughout execution.

### 5.2 Artifacts and Projection Roles

The central consequence of the projection structure is that **artifact type
classification is a category error**. An artifact does not have a fixed type —
it carries content that plays different roles in different projection instances.

A requirements document carries `N` content. In a `P(N) → S` projection instance,
it is the external argument: the stimulus a team's development process acts
against. In an `N(S) → P` projection instance, it is the internal lens: the model's
own normative content applied to an external design. In a `P(S) → N` projection
instance, it may be the *output* — the normative content that an analysis
procedure is generating for the first time.

The same artifact, three distinct roles — external argument, internal function,
or generated output — depending entirely on which projection instance it participates
in. A single artifact may participate in multiple projection instances simultaneously,
playing a different role in each.

This is why "what type is this artifact?" is the wrong question. The right
questions are: "What content type does this artifact primarily carry?" and
"Which projection instances is it currently participating in, and in what role?"

### 5.3 Model Completeness and Growth

Section 3 established that incomplete models carry latent content; the projections
describe the operations by which that content is elicited. Model growth is the
sequential application of projection instances to externalize what was implicit:

- A model with only `N` grows by applying `P(N) → S` (elicit the implementation),
  then `N(S) → P` (elicit the verification procedure), then executing `P` to produce `R`.
- A model with only `S` grows by applying `P(S) → N` (elicit the interface
  requirements), then `N(S) → P` (elicit the verification procedure).
- A model with only `R` (informal test data) grows by articulating the `S` that
  produced it, then the `N` against which it should be evaluated.

The six projections are not one-time operations. They recur throughout the model's
life as design matures, the operating environment changes, and new `R` arrives —
each new external signal potentially triggering a projection instance that updates
another content type.

---

## 6. The Geometry of Model-Space

The projection structure of §5 implies a geometry. The cross products define
orthogonal axes; the inner products define a metric. This section shows that
the `N/S/P/R` framework is a **3+1 dimensional model-spacetime** with
computable properties, and that this geometry enables a rendering pipeline
that makes credibility state visually inspectable.

### 6.1 Three Spatial Dimensions, One Time Dimension

`N`, `S`, and `P` are the three spatial dimensions of model-space. They are
genuinely orthogonal: each is the cross-product output of the other two (§5),
and the prediction can be wrong — non-zero surprise on the output axis is what
makes the axes independent. If `N` were derivable from `S` and `P`, the
projection `P(S) → R'_N` would always produce zero surprise, and the `N` axis
would be redundant. It is not. A node can carry strong content on all three axes
simultaneously — a safety analysis document that states requirements (high `N`),
maps architecture (high `S`), and defines verification procedures (high `P`) lives
in the interior of the `[0,1]³` cube, not on a simplex.

`R` is not a fourth spatial axis. `R` is the **time dimension** of
model-spacetime. Each `R` is an observation event — a packet with a specific
space-time context, analogous to a log message or a git commit. It records what
was observed (coordinates in `N×S×P` space — what content was being evaluated),
when (model-time — which version of the model, what upstream state existed), by
what process (the provenance of the observation — RWS test, surrogate analysis,
expert review), and what was found (the surprise value).

Once produced, `R` is **immutable**. A model can revise its `N` (change
requirements), redesign its `S` (update architecture), rewrite its `P` (modify
procedures). It cannot un-observe. A test result happened. A review was
conducted. The model evolves past its `R` events, but the events are fixed
points in the causal structure of model-spacetime.

**Model-time** is not clock-time. It is measured in surprise-generating events
at each coupling point: upstream model changes, environment changes, new `R`
arrivals at connected couplings. A perfectly calibrated model in a perfectly
understood environment has zero surprise rate — model-time has stopped — and its
`R` events remain current indefinitely. A model in a rapidly evolving environment
has high surprise rate — model-time moves fast — and its `R` events become stale
quickly.

The analogy to physical spacetime is structural:

| Physics | Model-spacetime |
|---|---|
| 3 spatial dimensions | `N`, `S`, `P` — axes of model content |
| Time dimension | Model-time — measured in surprise-generating events |
| Event | `R` packet — observation at specific `(N, S, P, t)`, immutable |
| World line | Model's trajectory through `N×S×P` over model-time |
| Light cone | Causal reach of an `R` event — which downstream couplings it can anchor |
| Metric | Surprise — the distance measure between prediction and observation |
| Proper time | Local model-time at a coupling (depends on local surprise rate) |

This resolves the asymmetry noted in §3: why are there three content types plus
"as-run records"? Because there are three spatial dimensions plus time. `R` is
categorically different from `N`, `S`, and `P` — it is not content the model
carries but evidence of where the model has been.

### 6.2 Informational Mass

The six projections produce predictions; the inner products evaluate them
against `R`. But the raw surprise `σ = N(R', R)` is scale-dependent. A 1mm
alignment error is catastrophic for a docking mechanism and negligible for a
test fixture. The same physical divergence produces different credibility
impact depending on how much structure depends on the entity being within its
normative boundary.

**Informational mass** is the depth of an entity's intra-model dependency chain
— the minimum number of intermediate assemblies needed to reconstruct the
entity's content from primitives, with credit for shared subassembly reuse. This
is the model-space analog of assembly index (Cronin et al., 2023): the
complexity of the entity measured by its construction depth, not by its size or
age.

Informational mass is computable from the belief network graph: trace incoming
epistemic and pragmatic edges backward to primitive nodes (those with no incoming
edges of those types), counting minimum path depth. A shared dependency consumed
by multiple downstream nodes is assembled once, not once per consumer.

A top-level requirement ("system mass < 500 kg") has low mass — it is a
primitive, reconstructible without intermediate assemblies. A derived timing
requirement ("VCU responds within 50 ms") has medium mass — its construction
requires system architecture, timing budget analysis, and control loop
characterization as intermediate assemblies. A probabilistic timing closure
analysis has high mass — its construction requires the full chain from hazard
analysis through FDIR architecture through analytical bounds through measured
distributions, with shared subassemblies (the analytical bounds model is reused
across all per-function analyses).

Mass is what makes criticality principled rather than judgment-based. The
informal classification of a model as "catastrophic consequence" is a proxy
for high informational mass — a deep dependency chain where boundary violation
reverberates widely through the coupled network.

### 6.3 Normalized Surprise

The normative constraint `N` defines a boundary on the entity — what the entity
claims about its behavior. The **boundary divergence** `a` measures how far the
observation landed from that boundary, in units of the constraint's own
tolerance. The informational mass `m` is the assembly index. Their product is
the **normalized surprise** `F` — the credibility impact on the belief network:

```
F = a × m = boundary_divergence(N, R', R) × assembly_index(entity)
```

This is `F = ma` for credibility. The boundary divergence is the
acceleration — how hard the observation pushes against the model's claims. The
mass determines how much of the belief network that push affects. A small
divergence on a high-mass entity (a tight tolerance nearly violated on a
deep-chain model) produces moderate `F` — worth monitoring. A large divergence
on a high-mass entity produces very high `F` — a credibility crisis. A large
divergence on a low-mass entity produces moderate `F` — locally concerning but
contained.

Policy thresholds at each lifecycle gate are applied to `F`, not to raw
divergence. A threshold on `F` automatically demands tighter raw tolerance on
high-mass entities and permits looser raw tolerance on low-mass entities.
Criticality classification — the manual assignment of consequence levels — is
subsumed by the mass computation.

### 6.4 The Credibility Rendering Pipeline

The 3+1 dimensional model-spacetime structure defines a rendering pipeline:
the coupled model network can be projected into a navigable visual where the
geometric properties carry diagnostic meaning.

**Position** (where in the cube): Each model or network occupies a region of
`[0,1]³` determined by the `N/S/P` content profile of its constituent nodes.
A requirements-heavy network sits near the `N` axis. A design-heavy network
sits near `S`. A verification-heavy network near `P`. Mixed-content networks
occupy the interior. Force-directed layout with typed gravity — section edges
pull toward `S`, epistemic toward `N`, pragmatic toward `P` — refines positions
so that coupling topology and content classification reinforce each other.

**Size** (how large the region): The assembly index (informational mass)
determines a model's visual extent. High-mass models — those with deep
dependency chains — are large. Low-mass models are small. The visual hierarchy
immediately communicates where the network's complexity concentrates.

**Opacity** (how solid the region): Each `R` event at a coupling point
contributes opacity. Well-grounded couplings with recent, high-provenance `R`
are opaque — the region is confirmed and current. Couplings with stale `R`
(many model-time ticks since the last observation) are fading — the region
was once confirmed but may not hold. Couplings with no `R` at all are
transparent — the region is asserted in the traceability structure but has
never been evaluated. The rendering naturally decays in model-time: `R` ages
not by the clock but by the rate of surprise-generating events at each
coupling. A coupling whose upstream models haven't changed stays opaque
indefinitely. A coupling whose upstream has evolved fades until new `R`
restores it.

**Edge properties**: Edges between models carry visual encodings for coupling
type (`WeightKind` as color: section = grey, epistemic = blue, pragmatic =
orange), for normalized surprise (`F` as thickness), and for `R` provenance
and recency (as opacity, matching the node rendering).

**Query-driven overlays**: The rendering supports composable diagnostic
layers, each driven by a query against the compiled belief network. Each query
highlights matching nodes and edges with a specified color or pattern. Multiple
overlays compose via blending — where a "coverage gap" overlay (red) intersects
a high-mass region (large bubble), the visual combination is the diagnostic:
a credibility crisis on a critical entity. The composability of the query
language becomes a composable visual analysis language.

The rendering pipeline compresses information that currently exists only in
hundreds of pages of traceability matrices, credibility assessments, and gap
analyses — or in a chief engineer's non-transferable mental model — into a
single navigable object. The same information, but spatially organized so that
risk concentrations, coverage gaps, necrotic couplings, and mass hierarchies
are apparent at a glance.

### 6.5 Lifecycle Gates as Projection Specifications

The engineering lifecycle follows a characteristic pattern of projection
operations with increasing self-reference. The factory and the engineered
system exchange `N/S/P` content through cross products, generating
progressively richer manifolds of the system's credibility state.

**Phase 1 — Outer manifold** (concept through preliminary design): The
factory's `N/S/P` is the active lens. It takes the normative kernel of the
system (mission requirements, customer constraints, regulatory framework)
and projects through factory processes to generate the space of what the
system *could* look like:

- `P_factory(N_kernel) → R'_S`: factory process generates candidate designs
- `N_factory(S_concept) → R'_P`: factory norms generate candidate
  verification approaches

The output is the outer manifold — the envelope of possible configurations.
The system exists on paper and in prototypes. The factory does all the
projection work; the product model cannot yet project onto itself.

Critically, the factory also **buds off vesicles** of its own projection
machinery into the product space. Assigning a responsible engineer installs
a clone of the factory's design process. Establishing a review board
installs a clone of the factory's `N(S) → R'_P` projection capacity.
Setting up a CI pipeline installs a clone of the factory's verification
process. These organizational acts are the factory seeding the product model
with the generative capacity it needs for Phase 2 — the ability to project
onto itself. The responsibility at this phase is ensuring the *right* pieces
of the factory's `N/S/P` get cloned: miss a vesicle (no one owns the safety
analysis) and the product has a blind spot where it cannot self-project;
install the wrong vesicle (apply a hardware review process to software) and
the product's self-projections produce systematically wrong predictions.

**Phase 2 — Inner manifold** (preliminary through critical design): The
product's own `N/S/P` becomes the active lens. It projects through itself
to generate the specific configuration that satisfies its own constraints:

- `N_product(S_product) → R'_P`: requirements applied to design generate
  verification procedures
- `P_product(S_product) → R'_N`: analysis of design generates derived
  requirements
- `S_product(N_product) → R'_P`: design evaluated against requirements
  generates operational procedures

The output is the inner manifold — a self-consistent design that can
surprise itself. The factory's role shifts from generator to auditor.

**Phase 3 — Cross-manifold testing** (critical design through verification):
Permutations of factory `N/S/P` and product `N/S/P` exercise each other,
extending `R` nodes in both meshes simultaneously:

- `P_factory(S_product) → R`: factory test infrastructure exercises product
  design, producing test data
- `N_product(R, R') → σ`: product evaluates test results against its own
  norms, producing surprise
- `P_product(S_factory) → R'_N`: product projects onto factory, revealing
  factory process gaps

Each test is an `R` event that anchors *both* the product mesh (did the
design work?) and the factory mesh (did the test infrastructure work?).
Factory credibility grows alongside product credibility.

This pattern is universal. Every standard in the quality management system
space — lifecycle process standards, model credibility standards, hardware
test requirements — specifies what the **factory manifold** looks like:
what projections the factory must be capable of executing, what `N/S/P`
content the factory must carry, and what surprise thresholds the factory's
outputs must satisfy.

**A lifecycle gate is therefore a projection specification:**

> **Gate = (projection set, input specification, surprise threshold)**
> - Which cross products must have been executed (from factory, product,
>   or both)
> - With what input `N/S/P` content (at what maturity and resolution)
> - Producing what output `R` with what maximum normalized surprise `F`

The **(query, threshold)** formulation from §6.4 is the rendered version of
this underlying structure: queries highlight couplings where the specified
projections have not been executed or where `F` exceeds the gate's
threshold. The projection specification is the principled basis; the
rendered overlay is the visual diagnostic.

The gate progression on the credibility map is visible as a wave of opacity
solidifying across the render:

- **Early gates** (outer manifold): mostly transparent — factory
  projections have generated the product's initial `N/S/P` content but
  little `R` exists. The factory cluster is the opaque substrate.
- **Middle gates** (inner manifold): substantial opacity in the product
  region — the product model's internal projections have generated `R`
  from analysis and surrogate evaluation.
- **Late gates** (cross-manifold testing): the flight-critical mesh is
  fully opaque — `P_factory(S_product)` has produced high-provenance `R`
  at all critical couplings.

This makes gate compliance auditable against the compiled corpus: the
compiler evaluates the specified projections, measures surprise at each
coupling, and reports which thresholds are met. The diagnostic stream is
the compiled verification output — the same rendering pipeline, in text
form.

---

## 7. Model Identity, Coupling, and the Registration Event

### 7.1 Model Identity

A model is identifiable when it has a stable set of `N`, `S`, and `P` content that can
be tracked as a unit across time.[^5]

[^5]: In the language of Bayesian mechanics (Ramstead et al., 2022), a model is a *particular* — a system that maintains statistical independence from its environment through a Markov blanket. The model's externalized N, S, and P content constitutes the blanket: the typed boundary separating the model's internal dynamics (the owner's internalized understanding) from the external states (the RWS). The source/sink directionality at each coupling (§5) corresponds to the sensory/active decomposition of the blanket — sensory states flow inward (explore), active states flow outward (exploit). The registration event (§7.3) is the formation of the blanket: the moment cross-axis coupling establishes a statistical boundary. Model identity is not artifact identity — a
model may be distributed across multiple artifacts (a requirements database, a
design document, a test plan). The model is the coherent assembly; the artifacts
are the carriers.

Model identity requires a responsible owner: the person or team accountable for the
model's completeness and credibility. The process of generatively projecting a model or
executing a model along one of its primary axis often involves the owner's internalization
of the model. The owner is responsible for ensuring that the published `N`, `S`, and `P` content
remain consistent with each other and with `R` as the model evolves.

### 7.2 Model Coupling

Models are coupled when one model's content participates in another model's
projection relationships. The diagnostic for coupling is **cross-axis information
density**: when the most efficient encoding of the relationship between two bodies
of content requires a cross-dimensional projection (A's `N` constraining B's `S`,
or A's `S` substituting for `R` in B's projection), the relationship is an
inter-model coupling. When the most efficient encoding is same-axis (A's `S`
refining A's own `S`, or A's `N` elaborating A's own `N`), the relationship is an
intra-model refinement. The coupling boundary is where cross-axis compression
becomes cheaper than same-axis patching — the point at which treating the content
as a single model no longer reduces the description length.

Two cross-axis coupling patterns are particularly important:

**Normative coupling** (`N → S`): Model A's `N` content becomes an ontology
dependency for Model B's `S` content. B's structural implementation references A's
constraints or optimization signals as the basis for its design decisions. Changes
to A's `N` propagate as staleness signals to B: B's structural implementation may
no longer satisfy the requirements it was designed against. Every engineering
traceability relationship is an instance of normative coupling.

**Surrogate coupling** (`P(A.S) → R`): Model B exercises Model A's structural
content by running `P(A.S)` against some input stream to produce an observation
record `R` — then consumes that `R` as if it were RWS-grounded. A simulation's
outputs are consumed by an analysis procedure as if they were telemetry. A timing
model's bounds are consumed by a safety argument as if they were empirically
confirmed flight data. The surrogate is the execution `P(A.S) → R`: `A.S` is
authored and owned; the resulting `R` is treated as observed and grounded. When
`N(R_surrogate, R')` is evaluated as if `R_surrogate` were `R_RWS`, the resulting
surprise signal inherits A's model error invisibly — every downstream model that
acts on B's surprise-driven updates is implicitly trusting A's `S` as if
exercising it produced RWS-equivalent observations.

The organizational belief network ([wp-connectome]) is the compiled graph of these
coupling relationships: normative couplings on the epistemic axis, surrogate
couplings on the validation axis. Both verification and validation follow the
universal projection structure `P(S) → R'`, `N(R, R') → surprise`; what the belief
network tracks is the provenance of `R` at each coupling — whether it is
RWS-grounded or model-internal — which determines the credibility weight of the
surprise signal it produces.

### 7.3 The Registration Event

The boundary question — when does a model need to be formally tracked as a unit,
with explicit `N, S, P` content and a credibility record — has a principled answer
in the coupling structure.

**A model should be formally registered when cross-axis coupling appears**: when
its `N` content becomes an ontology dependency for another model's `S` content, or
when its `S` content enters a surrogate coupling. These are the events where
cross-dimensional information density exceeds same-axis density — where it becomes
cheaper to establish the inter-model relationship than to keep the content folded
into the originating model.

Before either coupling exists, the model's content may be informal and
incompletely assembled. The content is still potentially valuable — informal
analyses, exploratory designs, and undocumented implementations carry real
epistemic content — but the credibility stakes are local. The engineer who
produced the content holds its context and can evaluate its applicability.
Same-axis refinements (elaborating one's own `S`, tightening one's own `N`) do
not trigger registration — they are intra-model convergence, not inter-model
coupling.

Once normative coupling is established, A's `N` is constraining B's S. Changes to
A's `N` now have downstream consequences that extend beyond A's owner. The coupling
makes A's `N` content a shared dependency — a coordination interface that other
models rely on. Formal tracking of A as a model (with explicit `N`, responsible
owner, and staleness management) is what makes the coupling epistemically
governable.

Once surrogate coupling is established, A's `S` is standing in for the RWS in B's
P. The consumers of B's analysis or decision are implicitly trusting A's `S` as if
it were an RWS measurement. The credibility stakes extend to everyone who acts on
B's outputs. Formal credibility assessment of A is what makes the surrogate
coupling explicitly characterized rather than implicitly trusted.

The registration event is therefore not a bureaucratic threshold but a structural
event in the model network: the moment cross-axis coupling makes a model's content
a shared dependency whose description length is minimized by treating it as a
distinct, governed unit.

---

## 8. Credibility Assessment in `N/S/P/R` Terms

The standard model credibility assessment frameworks (NASA-STD-7009B; ASME V&V 10;
DoD VV&A) specify assessment factors that are typically presented as checklists.
The `N/S/P/R` framework gives each factor a precise grounding.

### 8.1 Capability Assessment Factors (Development-Phase Evidence)

These factors assess the quality of the model's `N`, `S`, and `P` content as assembled
during development. They answer: how well was the model built?

**Data pedigree**: the `R` chain tracing `S`'s input data back to reality coupling
points. For each input to `S`, how many intermediate models does the chain pass
through before reaching a direct measurement of the RWS? Long chains accumulate
epistemic uncertainty at each coupling. Short chains with strong measurement
coupling give high pedigree.

**Verification, validation, and review** are all instances of the same
projection: `P(S)(R) → R'`, `N(R, R') → surprise`. The operation is identical;
the provenance of `R` differs:

- *Verification `R`*: produced by a model-internal process — a test harness,
  formal checker, or controlled environment. The projection checks whether `S`
  behaves consistently with its own `N` specification under controlled
  conditions. High verification means `P` was executed rigorously, `R` covers
  the claimed domain, and the resulting surprise signal is explicitly
  characterized.
- *Validation `R`*: produced by the world actually running — `P(s_t) → s_{t+1}`
  where `s_t` is a RWS state. The projection checks whether `S`'s `p(o|s)`
  preserves its `N` constraints across actual operating trajectories. Every
  `S` — a CAD assembly, flight software, a simulation, an organizational
  structure — defines `p(o|s)`, differing only in what its state space
  represents. A static load case is a trajectory of length one. What varies
  across domains is the form `N` constraints take: fidelity bounds
  (`Δ = o - ô ≤ ε`) for simulation models, operational bounds (stability
  margins, deflection limits, latency ceilings) for physical and control
  systems.
- *Review `R`*: produced by a domain expert's internalized model — the
  reference is a human-epistemic lens ground through experience. The expert
  configures the environment to produce an interference pattern between the
  explicit model and their internal understanding. The surprise signal is the
  expert's discomfort: places where the documented `N`, `S`, or `P` diverges
  from what they believe to be true. Review quality is bounded by the
  expert's internal lens quality — an expert who has not ground their lens on
  the relevant domain produces an empty coupling regardless of the signature
  on the document. The independence protocol is signal quality control on this
  channel: it ensures the expert actually confronted their internal model
  against the artifact rather than rubber-stamping.

The **validation domain** is the region of the operating space over which
RWS-grounded `R` has been evaluated and the surprise signal confirms `N`
constraints hold. This is why `S` is the target of credibility assessment
regardless of domain: the question is always whether the surprise signal was
grounded in RWS `R`, and whether that `R` covers the domain in which `S` is
being trusted as an RWS substitute.

**Development process/product management**: is `S` under configuration management,
versioned, and change-controlled such that `R` can be associated with the specific
`S` version that produced it? Without version control, `R` cannot be reliably
attributed to the `S` that generated it, making validation evidence untrustworthy.

### 8.2 Results Assessment Factors (Use-Phase Evidence)

These factors assess the quality of a specific `R` instance produced by a specific
use of the model. They answer: how well was this particular use of the model
executed?

**Use assessment**: is the proposed surrogate coupling within the model's
permissible use — i.e., does the consuming model's `P` propose to use A's `S` in a
domain and role consistent with A's stated `N` (intended use, permissible use) and
bounded by A's established validation domain? A use assessment that finds the
proposed consumption outside the validation domain is a surrogate coupling warning:
the consuming model's `P` is being asked to treat A's `S` as an RWS substitute in
a region where A's `S` has not been compared to the RWS.

**Input pedigree**: what is the `R` chain for the inputs to this specific S
execution? Even a well-validated model can produce unreliable outputs if its
inputs for a specific use have low pedigree — derived from informal sources,
modified without tracking, or drawn from a domain different from the one the
model was validated against.

**Uncertainty characterization**: what is the distribution of constraint preservation
margin in `R` for this domain, and how does input uncertainty propagate through `S`
to that margin? The margin is the distance between the observed `s_{t+1}` and the
nearest `N` constraint boundary — `ε - Δ` for fidelity constraints, headroom to
stability or yield limits for physical and control systems. The aleatory/epistemic
classification is frame-relative (Part V §1 of the CE framework): what is aleatory
for one consuming model may be epistemic for another. The characterization must
specify which `N` constraints are being evaluated and the frame it applies from.

**Results robustness**: how sensitive is `S`'s output to perturbations in its key
inputs? This is the sensitivity of the surrogate coupling to its inputs — if small
changes in input produce large changes in output in the domain where the surrogate
is being used, the consuming model's decisions are highly sensitive to the accuracy
of those inputs. High robustness means the outputs are stable across the plausible
input range; low robustness means the surrogate relationship is fragile.

**Use/analysis technical review**: has an independent agent executed an `N(S) → P`
projection for this specific consumption context — evaluated the model's `N` against
its `S` to generate a specific verification of this use, and executed that
verification? Independent review of the use assessment, input pedigree, and
uncertainty characterization closes the human-epistemic coupling for this
specific application.

**Use process/product management**: is this specific `R` instance traceable to the
`S` version and input configuration that produced it? This is the audit trail for
the surrogate coupling: if a downstream model's decision is later questioned, the
`R` that supported it must be traceable to the specific model version and input
state that generated it.

### 8.3 Credibility as a Typed Floor Map

The standard credibility assessment frameworks assign factor scores on an ordinal
scale (e.g., 0–4 per NASA-STD-7009B). These scores are useful for triage and
comparison, but the underlying structure they approximate is a typed classification
of the residual uncertainty floor in each factor's domain.

![Precision Map: Anchoring State as Mesh Resolution](figures/precision_map.svg)

Following the propagation model of the organizational belief network ([wp-connectome]),
four floor types can be distinguished:

| Floor | Description | Engineering signature |
|-------|-------------|----------------------|
| **Through-zero** | Mastered; model transparent | Validation domain fully covered; divergence from RWS consistently small; no surprises |
| **Living floor** | Structured residual that is learnable | Divergence shows systematic patterns traceable to model assumptions; reducing with more `R` |
| **Dead noise floor** | Irreducible aleatory | Divergence is stationary, shapeless, non-decreasing with more `R` |
| **Necrosis / phantom** | Stale model trusted as live | No `R`; or `R` exists but `S` has changed since `R` was produced; confabulation |

A credibility assessment that produces a typed floor map — assigning each factor
to one of these floors — is more actionable than a scalar score. The through-zero
factors need no investment. The living-floor factors need targeted additional `R`.
The dead-noise-floor factors need margin and redundancy, not more validation. The
necrotic factors need urgent attention: the model is being trusted as valid in
a region where it has not been recently evaluated against the RWS.

The typed floor map is also the input to the propagation model that characterizes
the model network's epistemic health: which sub-networks are super-critical (small
surprises amplify) and which are sub-critical (surprises dampen to distributed
necrosis, the silent failure mode).

The credibility rendering pipeline (§6.4) makes this map concrete: the floor
types project as opacity in a navigable 3D model-space visualization, where
through-zero regions are fully opaque, living-floor regions are translucent,
and necrotic regions are transparent. The rendering compresses the typed floor
map from a per-factor assessment table into a spatially organized visual where
risk concentrations are apparent at a glance.

---

## 9. Relationship to Model-Based Systems Engineering

Model-Based Systems Engineering (MBSE) is the established field most closely
related to the framework proposed here. The INCOSE definition (Estefan, 2008)
describes MBSE as "the formalized application of modeling to support system
requirements, design, analysis, verification and validation activities beginning
in the conceptual design phase and continuing throughout development and later life
cycle phases." In practice, MBSE is implemented through formal modeling languages
(principally SysML) and associated toolchains that centralize system information
in a structured model rather than in documents. The promise: linked views on a
single authoritative model, so design changes propagate automatically rather than
requiring manual synchronization across documents.

The N/S/P/R framework shares MBSE's foundational premise: document-centricity is
the wrong organizing principle for engineering knowledge, and the relationships
between artifacts carry meaning that document management systems cannot see. Both
approaches recognize that traceability is a first-class concern, not an audit
activity. Both recognize that models can be wrong and that verification and
validation are the mechanisms for detecting wrongness.

The differences are substantive, and they are not primarily about tooling.

**MBE defines model as artifact; this framework defines model as epistemic unit.**
In MBSE practice, "the model" is the SysML file — a specific artifact in a specific
format managed by a specific tool. This paper's framework defines a model as a
coherent assembly of N, S, and P content that is identifiable by its coupling
relationships, regardless of which tools or formats carry that content. A SysML
model may carry the S content of multiple engineering models, or S content without
the N and P that would make those models epistemically complete. The tool artifact
and the epistemic unit are not the same thing, and conflating them is one source
of MBSE's implementation difficulties: organizations build large SysML models that
are internally consistent but not epistemically complete, because the N content
(natural-language requirements) lives in a separate requirements management tool
and the P content (test plans, procedures) lives in yet another system.

**MBE has no definition of what makes a model wrong.** The MBSE literature defines
a model as a representation of a system — a definition that emphasizes structure
without foregrounding falsifiability. The N/S/P/R framework grounds model identity
in the capacity to be surprised: a model makes commitments about how an observable
referent will behave, and those commitments can be confronted by measurement. This
difference is not semantic — it has direct consequences for what MBSE toolchains
are designed to check. SysML tools enforce structural consistency within the model
(do the interface definitions match? do the requirements have downstream
allocations?) but they do not enforce or even represent the model's fidelity to
the real-world system. The credibility assessment framework of §8, which maps
directly onto the N/S/P/R structure, has no analog in standard MBSE practice.

**MBE handles S content well and N/P content poorly.** SysML's block definition
diagrams, internal block diagrams, and parametric diagrams are well-suited to
representing `S` content — nested structural descriptions of how a system is
composed. But SysML captures the *composition* of `S` without capturing its
generative character: a CAD assembly represented as SysML blocks does not encode
the `p(o|s)` projections — structural, thermal, dynamics — that make the assembly
predictively useful. SysML's parametric diagrams gesture toward this but cannot
represent the full projection structure or the `P`-dependency of which projection
is being read. This is why MBSE toolchains cannot perform credibility assessment:
the question "does `S`'s `p(o|s)`, read through `P`, match the RWS?" is not
representable in the toolchain's data model. SysML's requirements diagrams can
represent `N` content, but in practice requirements remain in natural language and
their relationship to the formal model is maintained by manual link management
rather than semantic coupling. Procedural content (`P`) — the test plans,
verification procedures, manufacturing processes, and operational guides that
generate the `R` that validates `S` against `N` — is generally outside the scope of
MBSE toolchains entirely. The N/S/P/R framework is explicitly designed to cover
all four content categories and to characterize their couplings as typed projection
relationships, not tool-specific links.

**MBE has no principled forgetting mechanism.** For the same reason as
document-centric engineering (§2), MBSE toolchains have no basis for retiring
model elements when the couplings they maintain are no longer active. A SysML
block whose downstream systems have been removed from the architecture continues
to exist in the model, consuming review attention and appearing in consistency
checks. The coupling-based retirement criterion of §7.3 applies equally to MBSE
artifacts: a model element can be retired when the normative or surrogate coupling
it participates in is no longer active — and the basis for that determination is
the coupling structure, not the artifact's presence in the tool.

**MBE does not address inter-model coupling across organizational and tool
boundaries.** A SysML model is typically owned by one team and managed in one
tool instance. Cross-subsystem normative couplings — where subsystem B's design
depends on subsystem A's interface requirements — are represented within the model
as allocations and satisfies relationships. But cross-organizational couplings,
where an external standard's N content becomes an ontology dependency for the
program's S content, or where a supplier's model's S content is consumed in a
surrogate coupling by the integrating organization's P, are outside the scope
of any MBSE toolchain. These inter-organizational surrogate and normative couplings
are precisely where model error has the highest consequence and the least
visibility. The N/S/P/R framework makes them first-class objects in the model
network, subject to the same coupling characterization and credibility assessment
as intra-organizational couplings.

The synthesis: MBSE correctly identifies document-centricity as the problem and
modeling as the organizing principle, but substitutes one artifact (the SysML
model) for another (the document) rather than making the epistemic relationships
themselves the primary object. The N/S/P/R framework is a candidate theoretical
foundation for MBSE — it explains *why* modeling is the right organizing principle
(models are falsifiable; documents are not), *what* a model is (a coherent N/S/P
assembly with a defined coupling structure), and *what* the toolchain should be
managing (coupling relationships and their credibility evidence, not artifact
containment). An MBSE toolchain built on this foundation would track projection
instances rather than document links, classify content by epistemic type rather
than diagram type, and surface retired couplings rather than accumulating dead
model elements.


## 10. Open Problems

**Model-space stability and iterated projections**: the expressed `N`, `S`, and
`P` are lossy projections of the latent operational reality along three orthogonal
axes (§5). Each projection produces a predicted observation `R'` on its output
axis; the model updates only when the prediction diverges from actual content —
when surprise is non-zero. Iterated projection is the mechanism by which the model
captures increasing fidelity: each predict-evaluate-update cycle refines the
expressed content toward the latent phenomenon.

Each cycle is a state transition in the model-space `M = (N, S, P)`. A model's
trajectory through `M` under iterated projections — driven by external inputs and
internal folds — may converge, oscillate, or diverge. The convergence question can
also be restated as a path-integral question (Friston et al., 2023): rather than
asking whether each step reduces surprise, ask whether the trajectory's total
action (accumulated free energy) converges — which naturally handles the strange
attractor case where instantaneous surprise is non-zero but the trajectory is
bounded. The alternating projections framing remains useful: convex constraint
sets converge to their intersection (von Neumann, 1950); for non-convex sets,
convergence is initial-condition-dependent.[^2] The bet is that certain initial
configurations of observable model-space converge — where the regularizers
(internal content) are well-calibrated to the RWS — and others do not. A second
bet: the cross-product structure of the projections and the inner-product structure
of surprise evaluation (see footnote 3) suggest that the model-space carries a
geometric algebra in which convergence conditions can be stated in terms of the
relationship between the projection basis and the latent phenomenon — analogous to
how the Babuška-Brezzi condition states convergence in terms of the relationship
between the element basis and the solution space.

[^2]: Finite element mesh refinement is a well-characterized engineering instance of this convergence question. The mesh is a projection of the continuous RWS onto a finite-dimensional subspace (the element basis functions); refinement is iterated re-projection onto progressively richer subspaces. Convergence depends on whether the projection basis can represent the solution — the Babuška-Brezzi (inf-sup) condition. When it is not satisfied, the discrete projection *cannot* converge regardless of refinement: the element formulation is structurally incapable of representing certain field components (e.g., pressure locking in equal-order incompressible elasticity). This is the projection-basis failure mode: the regularizer (element formulation) is wrong for the physics, and no amount of iteration fixes it. The false fixed point also appears: a mesh that passes a global convergence study (strain energy stabilizes) while the local prediction (stress at a notch tip) diverges has converged against a proxy rather than the quantity of interest — the Goodhart attractor in FEM form.

Convergent trajectories reach a fixed point: an `(N, S, P)` state where every
projection's predicted observation matches actual content — surprise is zero on all
axes, and no external signal produces divergence large enough to trigger an update. The most important case is the **false fixed point**:
a model that converges not because its `S` faithfully represents the RWS, but
because its `N` has been narrowed to match whatever `S` produces — the Goodhart
attractor. False fixed points are stable in the dynamical sense but epistemically
dead; they are the formal counterpart of the necrotic floor type (§8.3). In the
inverse-problem framing, the Goodhart attractor is a regularization artifact: the
internal content that serves as the reconstruction operator has been shaped by
optimization pressure to produce solutions that satisfy `N` without matching the
RWS. Distinguishing live fixed points from false ones requires external
anchoring — `R` produced by confronting `S` with the RWS rather than with the
model's own `P`.

A third attractor type corresponds to the **living floor** (§8.3): a bounded
non-convergent trajectory — the chaotic or strange attractor. The model's
projection sequence neither converges to a point nor diverges; it orbits a
bounded region of model-space where surprise is structured and non-zero but
reducible. Each iteration captures more of the attractor's structure, shrinking
the orbit radius, without ever collapsing it to a fixed point — because the
latent phenomenon has structure that the three axes cannot fully resolve at any
finite fidelity. This is the dynamical counterpart of "learnable residual": the
model is not wrong in the way a false fixed point is wrong (it is genuinely
tracking the RWS), but it has not — and may never — reach zero surprise. The
dead noise floor is the limiting case where the orbit radius stabilizes at the
aleatory bound: the residual is irreducible, and further iteration does not
shrink it.

Coupling two models (§7.2) couples two dynamical systems. The joint stability of
the coupled system is a distinct question from the stability of either model alone,
and is where inter-model surprise propagation lives. This is the model-level
counterpart of the quantitative surprise propagation problem identified in
[wp-connectome] §10: whether message-passing inference on a factor graph
representation of the compiled belief network can tractably propagate surprise
signals across coupled models.

**Model granularity**: the framework does not specify the right level of
granularity at which to identify a model. A system can be treated as one model
(the system-level `N`, `S`, and `P`) or as a hierarchy of sub-models (each subsystem
is a model, with the system model's `S` referencing them). The granularity decision
affects the structure of the belief network: finer granularity gives denser edges
and more tractable sub-model credibility assessments; coarser granularity gives
sparser edges but requires monolithic credibility assessments.

**`N` content formalization**: the framework identifies `N` content as a preference
ordering with constraint and optimization sub-modes. The cybernetic embodiment
framework (CE Part IV) gives a formal treatment of this ordering in terms of a
sub-Turing scoring language. Connecting the informal natural-language requirements
in engineering practice to this formal treatment is an open problem — it is the
question of whether requirements can be compiled into evaluable C functions.

**`R` coverage and sampling**: `R` is produced by applying `P` to `S` and confronting
the result with the RWS — but each `P` reads only one projection of `S`'s `p(o|s)`,
and `R` from that `P` covers only the portion of the operating domain actually
exercised. Coverage gaps are unsampled projections or unexercised regions of the
domain: conditions not tested, operating modes not observed, failure modes not
injected. The question of how to characterize coverage across the full projection
structure of `p(o|s)` — and how to bound credibility in the unsampled regions —
is unresolved. It is complicated by the fact that different projections (predictive,
reactive, coherence) require different `P`s and produce incommensurable `R`, making
a unified coverage metric non-obvious.

**Implicit projection instances**: many projection instances in engineering
organizations are executed informally — a design is built without explicit
requirements (an implicit `N`), or a test is run without an explicit test plan (an
implicit `P`). The framework assumes that projection instances can be identified and
that the content types can be distinguished. In practice, implicit instances may be
numerous and their content types may be ambiguous. Methods for surfacing implicit
projection instances from engineering artifacts are needed.

**Human-epistemic lens calibration**: §8.1 establishes that review quality is
bounded by the expert's internal lens quality — an expert who has not ground
their lens on the relevant domain produces an empty coupling. The open question
is whether lens quality can be *measured* empirically by presenting experts with
historical `R, R'` pairs from the compiled graph (Cooke, 1991) and scoring their
predictions against revealed outcomes. If calibration improves with repeated
exercises, the measurement is simultaneously credentialing and development — a
feedback loop that grinds the lens.

**Quantitative surprise propagation across coupled models**: the compiler can
detect surprise at individual couplings — the divergence between expected and
actual informational mass at each node (§8 of [wp-connectome]). Propagating these
signals across chains of surrogate couplings is harder: each coupling introduces
its own encoding parallax (footnote 4), and the accumulated aberration across a
chain is not easily decomposed into per-element contributions. The factor graph
remains the most promising inference structure — couplings carry the epistemic
state as variables, models are the local constraint functions — but tractability
at organizational scale (tens of thousands of nodes) is undemonstrated.

**Encoding parallax in mixed variable types**: epistemic uncertainty involves
both low-parallax N-axis measurements (review status, constraint satisfaction —
information evaluating information) and high-parallax S and P axis measurements
(divergence at coupling points, test coverage — information-encoded representations
of non-informational phenomena). Combining these into a coherent precision map
requires accounting for the axis-dependent aberration structure: N-axis surprise
has minimal parallax, while S and P axis surprise has greater aberration that
cascades through coupled models (footnote 4).

**Anchoring degradation as precision-map staleness**: anchoring state is the
precision map of the model's documented surface — high confidence near reality
coupling points, degrading confidence between them, unknown confidence in regions
with no coupling points at all (§8.3). A model anchored by data from one
operational period becomes stale as the RWS evolves: the coupling point (mesh
node) existed but the phenomenon it was validated against has moved. The open
question is the rate function: how fast does precision degrade as the RWS drifts,
and how should the precision map represent partial staleness when some axes of
the validation domain remain current while others have drifted?

**Epistemic and pragmatic surprise as projection orientations**: the epistemic
and pragmatic axes of the organizational belief network ([wp-connectome] §3)
correspond to the explore and exploit orientations of the S×P projection plane
(§5). The epistemic axis is the explore orientation — self's processes as lens,
world's structure as data, asking how the model's norms must change to accommodate
reality. The pragmatic axis is the exploit orientation — self's structure as lens,
world's processes as data, asking how the world must change to match the model's
commitments. The open question is whether surprise on the explore projection
correlates predictably with surprise on the exploit projection — whether epistemic
surprise (a model was wrong about reality) predicts pragmatic surprise (a plan was
wrong about schedule or cost).

---

## References

Estefan, J.A. (2008). *Survey of Model-Based Systems Engineering (MBSE) Methodologies*.
INCOSE MBSE Initiative.

Friston, K. (2010). "The free-energy principle: a unified brain theory?"
Reviews Neuroscience*, 11(2), 127–138.

Lyjak, A. [wp-connectome]. "The Organizational Connectome: Toward a Paradigm for
Systems Engineering and Project Management." Unpublished companion paper.

NASA (2024). NASA-STD-7009B: Standard for Models and Simulations. National
Aeronautics and Space Administration.

ASME (2006). V&V 10: Guide for Verification and Validation in Computational
Solid Mechanics. American Society of Mechanical Engineers.

Der Kiureghian, A. and Ditlevsen, O. (2009). "Aleatory or epistemic? Does it
matter?" *Structural Safety*, 31(2), 105–112.

Friston, K., Da Costa, L., Sakthivadivel, D.A.R., Heins, C., Pavliotis, G.A.,
Ramstead, M., and Parr, T. (2023). "Path integrals, particular kinds, and strange
things." *Physics of Life Reviews*, 47, 257–284.

Knight, J.C. and Leveson, N.G. (1986). "An Experimental Evaluation of the
Assumption of Independence in Multi-Version Programming." *IEEE Transactions on
Software Engineering*, SE-12(1), 96–109.

Ramstead, M.J.D., Sakthivadivel, D.A.R., Heins, C., Koudahl, M., Millidge, B.,
Da Costa, L., Klein, B., and Friston, K.J. (2022). "On Bayesian mechanics: a
physics of and by beliefs." *Interface Focus*, 13(3), 20220029.
