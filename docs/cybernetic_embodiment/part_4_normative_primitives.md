---
title: "Normative Primitives"
subtitle: "The Policy Surface --- A Protocol for Learning Policy · Cybernetic Embodiment, Part IV"
authors: "Andrew Lyjak, Claude (Opus)"
last_updated: "2026-06-05"
status: "Derivation / clean-sheet (FEP grounding)"
version: "0.1"
dependencies: ["the_centaurs_nervous_system.md", "feeling_into_right_relation.md"]
---

## Purpose

This document specifies the **policy surface**: a **protocol for learning policy**.
Two OODA loops exchange a data structure so each can build and refine models of the
other's policy, coordinating by feeling it while each keeps its own will intact. A
**policy** is a sovereign's optimal published pointer at its own principles --- the
finger, not the moon (Section 2.3) --- so what travels between loops is refinement of that
pointer, never the principles themselves. Grounded in the Free Energy Principle (FEP).

Central claims, stated once so the rest can discharge them:

- A **policy** is an atom of a generative model --- the minimal unit of normativity
  an organ declares (Section 2).
- A **redline** is the signal of a normative lesson---a loop modified its policy. It is the outcome of folding surprise back into a latent model and then re-expressing that lesson onto the published policy. It is strongly associated with variational free energy (VFE): a prediction error
  re-expressed as the policy amendments that would explain it (Section 3).
- The protocol **affords feeling** without carrying it. A **redline** is a normative *lesson*: a loop consumes its own surprise (variational free energy) internally, folds what it learned into its latent model, and re-expresses that learning as an amendment to its **published policy**. The redline is that amendment --- the lesson, on the slow learning timescale --- not the raw prediction error that occasioned it (Section 3).
-  On the wire are only
  redlines (Section 3); a receiving loop runs them through its *own* predictive machinery
  to generate *second-order* VFE --- its surprise about another loop's learned lesson --- the
  signal an already-feeling loop can feel *along*. That quantity is computed and
  held locally; it re-enters the protocol only if the loop folds it back into its
  own policy and emits a redline of its own. The protocol opens a boundary across
  which feeling can form; it does not manufacture or transmit the feeling itself (Section 6.1).
- The will is **constitutively local**: the protocol has no channel on which to
  ship it. So *through this protocol* no node's responsibility can be relocated and
  no hierarchy of drones can be forced; whether a deployed body stays a federation
  over time is a deployment-layer matter, not a protocol guarantee (Section 6.3).

One commitment organizes the whole construction: **the will stays home; only
surprising lessons travel.** A loop's own will --- its precision-weighted preferences and planning ---
is computed locally and never set from outside; what crosses is the sender's
declared normativity as policy amendments, raw material for modeling, never command. Both halves of
the thesis --- that organs keep their responsibility, and that they can still feel
one another --- follow from this.

A corollary to the commitment to the will staying home is that death is a sovereign act. To feel into another sovereignty is to understand the other as an identity unto themselves and the relationship as unique and contingient and therefore mortal. The protocol acknowledges death and facilitates communication and memory management related to death of sovereign individuals as well as their relationships.

A note on FEP vocabulary. We use POMDP convention: **observations** `o` for
sensory inputs, **states** `s` for the policy's hidden variables (latent causes).
**State preferences** are a prior over `s`, hence generalized free energy is the
local planning functional (Section 6.3). The likelihood `p(o∣s)` is local rigging
--- domain-specific structural content that stays private (Section 2.4). We
deliberately do *not* make actions or `p(o∣s)` declared primitives.

---

## 1. Why normativity is the shared unit

A thing worth calling a system has spacetime durability. To persist is to resist
dissipation --- to maintain a boundary (a Markov blanket) around a non-equilibrium
steady state. Dissipation is **death** where that boundary was real --- and the collapse of a **hallucination** where it was only imputed, never objectively there. FEP's Bayesian-mechanics reading is that anything that persists can
be described as if it infers: its dynamics look like the minimization of free
energy against a generative model, and that model's preferred states are its
implicit **normativity** --- the shoulds and oughts that keep it from dissipating.
"Worth calling a system" therefore means "worth reducing to a symbol and
tracking": compressible to a sufficient statistic with a Markov blanket.

The "as if" is deliberate and load-bearing. We use the FEP as a **modeling
affordance** --- a heuristic an observer adopts to compress another system --- not as a
theory of what the modeled system *is*. The claim is instrumental, not
metaphysical, which is also why it is bounded rather than vacuous: imputing a
policy earns its keep only across a finite band, and where it stops paying is
itself measurable. It goes *superfluous* once a thing is mastered to a fixed
response (a tool), *futile* where it never compresses no matter how hard you model
it (an egregore), and pays only in the living middle (an organ) --- the same
three-way floor the empirical bet turns on (Section 4; Part II, "The bet"). A theory of
everything cannot tell you when to stop applying it; this affordance can, which is
what keeps it falsifiable.

To model a durable thing's trajectory you must impute normativity to it. To
compress a system's *possible* next states into its *probable* next states, you
have to posit a coupling between its preferences and its available actions --- a
policy. (This is the content of Theorem 1 in `general.md`: causal inference
implies the attribution of agency.) You cannot predict a durable thing without
crediting it with a policy.

From this the good-regulator result sharpens. A regulator of another sovereign will does not need a model of the regulated system's full dynamics; the information-theoretic optimum is an accurate model of its **normativity** --- its preferences and the policy that pursues them. So the cheapest surface through which two sovereigns can coordinate is a shared model of each other's policies.

Note: We are defining model-optimality here loosely. The good-regulator theorem proper supplies only necessity — a good regulator must contain some model. Our claim that the optimal model of a *willful* system is a model of its normativity (rather than its dynamics) is captured as **Bet 1** (Part V section 9). Sections 6.3, 6.7 depend on this result and therefore inherit its conditional status.

---

## 2. The policy: an atom of a generative model

A policy declares three schemas. Two are **referent vocabularies**: what the loop
can sense (`o`, Section 2.1) and the latent causes it tracks (`s`, Section 2.2).
The third is **total logic over those referents**: how it ranks states (`C`,
Section 2.3). That logic is deliberately **sub-Turing** --- a
strongly-normalizing (guaranteed-terminating) expression language --- chosen so the
three things the protocol must compute about a policy all stay tractable:
evaluating a peer's policy always **halts**, and policy **equivalence**,
ordering-**totality**, and **diff-is-a-no-op** stay *decidable* (Section 3, Section 4).
Expressiveness is bounded by analyzability, not the reverse; the specific formalism
(Datalog-with-aggregation, a total scoring DSL) is what our first experiment fixes.

The predictive model `p(o∣s)` --- how observations are expected to follow from
states --- is **not** a declared policy primitive. It is domain-specific structural
content (`S` in the N/S/P/R sense): the thermal equations, numerical integrators,
or coordination dynamics that a particular host uses to evolve state. Publishing it
would require receivers to understand the sender's domain-specific implementation,
violating the policy surface's purpose as a coordination interface readable without
inspecting the model's internals. Surprise does not require prediction: `C`
evaluated directly against `o` detects constraint violations and optimization
gradient reversals without a predictive model (Section 2.3). How a loop predicts
future observations is local rigging, like actions (Section 2.4).

### 2.1. Observation schema (`o`)

The referents for what the loop can sense: its input alphabet. The schema need
not be self-contained. It may declare **ontology dependencies** --- opaque pointers
to external vocabularies (schemas, catalogs, classification outputs, interface
definitions) that define what the observation terms mean. A loop that observes
"cabin pressure" need not define pressure; it points at the referent source that
does. Resolution is lazy and one level deep: the dependency tells you where to
look, not what you will find. Shared observation vocabulary between loops emerges
from co-dependency on the same external referent (Section 6.5), not from a
imposed universal schema.

### 2.2. State schema (`s`)

The referents for the loop's hidden variables --- the latent causes it tracks.
These are the distinctions the policy treats as real and salient: the few axes of
the world it resolves into. This is the organ's frame --- what it attends to.

Like `o`, the state schema declares its referents through ontology dependencies.
A safety analysis that classifies system functions by criticality is authoring an
ontology that downstream policies reference --- it
mints new state variables that constraints in `C` can then bind to. The analysis
itself is not a policy; it is a **dependency** that policies consume. When the
ontology changes (a function is reclassified, a new failure mode is identified),
every policy whose `s` depends on it receives a staleness signal: its referents
may have shifted. This is orientation coupling --- it changes what is *sayable*
over `s`, not what is *preferred* within it.

### 2.3. State preferences (`C` over `s`)

A preference ordering over states --- the **normativity proper**. Not a single goal
but an ordering, so the policy is richly falsifiable: a loop can be surprised by a
violated *ranking*, not only a failed top choice. This is the organ's values made
legible. Formally it is a prior over hidden states, which is why local planning
uses generalized free energy (Section 6.3).

`C` has two modes that compose into a single partial order over available
intentions:

- **Constraints** impose hard boundaries on the feasible region of `s`. A
  constraint is a `C` component where every state satisfying it dominates every
  state violating it, regardless of other preferences. Constraint surprise is
  **acute**: the boundary was crossed, the observation `o` fell outside the
  declared region, and the signal propagates with high conductance.

- **Optimization signals** order the feasible region --- the gradient that selects
  among non-dominated states. Optimization surprise is **chronic**: the gradient
  is travelling the wrong direction, or the proxy that operationalizes the
  gradient has decoupled from the real objective it was meant to track.

Both are `C` and both compose into the same ordering. The distinction matters for
surprise propagation: constraint violation is a boundary event (acute, high
conductance); optimization divergence is a trajectory event (chronic, lower
conductance, susceptible to proxy drift). Surprise in both cases is `C` evaluated
against `o` --- no predictive model is needed to detect a boundary crossing or a
gradient reversal.

`C` is declared **intensionally** --- as a scoring/selection expression over `s`
(logic), not an enumerated ranking --- so the ordering is the expression's evaluated
output. Because that expression lives in the sub-Turing language fixed in Section 2, its
**totality is decidable**: "is this actually a well-formed ordering?" is a property
a receiver can *check*, not merely hope for, so a total preorder is well-defined by
construction. This is what makes a redline's delta on `C` (Section 3.2) a diff over an
*expression* --- with the merge semantics of code --- rather than an ill-posed edit
over a permutation. The receiver
recovers what the change *does* by evaluating that logic over its own state space,
which is how magnitude stays local (Section 3.2).

**Open tension: `C` as logic over opaque `s`.** The intensional declaration assumes
`s` is available as a computable vocabulary, but `s` is defined through ontology
dependencies (Section 2.2) --- opaque pointers, not inline schemas. How scoring
logic composes with externally-defined referents is an open design question. In
practice, `C` may reference `s` terms by name (the dependency providing
type/domain information), or `C` itself may be a compiled representation
extracted from natural-language artifacts (requirements, trade studies) rather
than a directly-authored expression. The constraint/optimization typing
(Section 2.3 above) is the minimal compiled `C` --- ordinal classification of
preference mode --- that does not require resolving `s` into a formal schema.
Whether a richer compiled `C` is needed, and what form it takes, is an empirical
question gated behind the first proof of concept.

The declared `C` --- and the policy as a whole --- is the sovereign's **optimal published
pointer** at its own principles: optimal in the good-regulator sense (Section 1, the cheapest
sufficient representation for coordination), and therefore *lossy by construction*. It
points at an identity-constituting normativity that stays latent and ineffable --- the
finger, not the moon. "Declared / made legible" here and "sovereign and local" in Section 5
name the two ends of that pointer: the moon stays home; the finger is what gets
published and diffed.

### 2.4. What is *not* declared

#### 2.4.1. Actions and predictive models

Conspicuously absent: actions, transition dynamics `p(s'∣s,u)`, the predictive
model `p(o∣s)`, and the machinery of movement. These are **local rigging**, not
part of the shared interface. An action is how a particular host realizes a
preferred state; `p(o∣s)` is domain-specific structural content --- the physics,
equations, or coordination dynamics a particular host uses to evolve and predict
state. Both vary per host and stay private. The principle: **you share what you
prefer and how you read the world, never how you act or how you predict.**

#### 2.4.2. Precision

What the message omits is as load-bearing as what it carries. A policy diff does include the ordering function `C` --- and in one sense `C` is a gain: it encodes what the sender values. But it crosses as **dashboard content**, raw material for the receiver's model of the sender's normativity, not as an instruction applied to the receiver's own `C`. There is still **no precision field** --- no weight saying *how much this should move you* --- and the reason is structural rather than merely normative. Precision weights *surprise* (VFE, the divergence between predicted and actual observations). What crosses the wire is a *policy amendment*: the sender's folded response to its own surprise, not the surprise itself. The transmitter's precision has no grip on this payload; the payload is not the kind of thing precision can weight. The sovereignty point follows as a consequence: if such a field were added, it would attach to the receiver's update magnitude --- a piece of its will --- relocating it into the sender, the move that turns an organ into a subordinate layer. Precision is never transmitted as an *applicable weight* --- no field on the wire sets how far the receiver moves. But it is not secret either; it **leaks two ways**, both inferential rather than transmitted:

1. **The sparsity gate (Section 6.4).** Whether and when the sender emits at all is precision-thresholded, so emission timing is a censored readout of its precision-weighted error.
2. **The published policy.** A receiver can *infer* the sender's precision from `C` and its amendment history --- how sharply it weights some states, how readily it revises --- the same way it infers any other latent of the sender's normativity.

What stays strictly local is the **applied** precision: how far the *receiver* moves on receipt. That is the sovereignty guarantee --- not perfect precision-secrecy, but that no sender sets the receiver's update magnitude. So the protocol keeps normativity **computable and inferable, but not commanded.**

---

## 3. The redline is a learned lesson

A loop's raw surprise is its variational free energy (VFE) --- a scalar discharged
by the mismatch between what the loop's `C` preferred and what `o` delivered.
Internally that surprise is consumed in the standard active-inference way: it
updates beliefs about states (perception) and drives action that discharges it
(movement). The predictive machinery a loop uses to anticipate `o` --- its
`p(o∣s)` --- is local rigging (Section 2.4); VFE is the quantity that matters, not the
specific gradient form that discharges it. What the protocol *distributes* to
connected loops is not the raw surprise.

### 3.1. The redline is a second-order computation

Shipping raw observation error would force the receiver to re-infer, from scratch,
what that error implies about the sender's policy --- and would require the receiver
to share the sender's observation space even to read it. Instead the sender learns from the error then folds that learning back into its policy and ships the difference. A redline answers:

> For this observation to have occurred, my policy must actually differ from what
> I held by these amounts: `{ policy_i : δ_i }`.

This folds the policy back off the observation mismatch and keeps the policy itself decoupled from the sender's internal operation. Three consequences, each
load-bearing:

- **It is in the shared coordinate.** The diff is stated against policies --- the
  thing two loops coordinate through (Section 1) --- not against the sender's private
  observation space. The receiver reads it by referent overlap on *policies*, not
  on raw sensoria.
- **It captures the learning signal.** A diff `{policy_i : δ_i}` just *is*
  "your model of my policy should move this way." No re-derivation; the update is
  the payload.
- **It flattens the network.** A loop's policies likely form a nested internal network;
  the redline projects the surprise onto a flat list of (policy-referent, delta)
  pairs. The nesting stays private; the wire payload is flat.

### 3.2. What crosses the wire

```
redline_message = {
  source: who emitted it,                                  # the return channel
  diffs:  [
    {
      bid,         # which policy lineage (Section 4)
      from_hash,   # the committed version the sender diverged from (the base)
      to_hash,     # the amended version it folded the learning into
      delta?,      # OPTIONAL materialized amendment --- an optimization, verifiable against from_hash → to_hash
    } ...
  ],
}
```

Each diff names the lineage by its `bid` (Section 4) and cites *two* content-addressed
versions: `from_hash`, the base the sender diverged from, and `to_hash`, the
amended version it folded that learning into. Because both are content addresses
(Section 4), either end can `fetch` them (Section 3.3), and the receiver can reconstruct the
amendment itself by diffing `from_hash → to_hash` --- it never has to trust the wire.
The materialized `delta` is therefore **optional**: an ETag-style optimization that
saves the receiver a fetch-and-diff and is checkable against the two hashes. Ship
the version identity always; ship the body only on a cache miss.

The `delta` (or the `from_hash → to_hash` diff the receiver reconstructs in its
absence) is the amendment itself --- direction and structure --- carrying no authority
to be acted on. How much it is acted on is local. This is the single fact that
keeps the receiver an organ: it can be *informed* without being *steered*.

### 3.3 The read channel: fetch and resolve

The redline names versions; it does not carry them. Two read operations back it ---
the content-addressed analogue of an HTTP conditional GET:

- `fetch(referent) → policy` resolves an *immutable* version `{bid, policy_hash}`
  (Section 4) to the three declared schemas it names.
- `resolve(bid) → policy_hash` returns a lineage's current head --- the mutable-name
  lookup. (Section 5.1 extends this to a lifecycle state --- a live head, a
  fork, or a tombstone.) `resolve` then yields the lineage's **lifecycle state** (Section 5): *alive* (a
  `policy_hash`, the common case), *forked* (successor `bid`s to follow), *dead* (a
  tombstone), or *forgotten* (a fossil).

With these, a diff against an unknown `from_hash` is a *cache miss*, not a divergence --- and the miss is itself information: the receiver fetches the base (and, where the sender omitted the materialized `delta`, the `to_hash` version too) and reconstructs the amendment locally. The `delta` on the wire is then pure optimization --- exactly the ETag /
`If-None-Match` discipline: the version identity travels every time, the body only
on a miss.

These same two operations underwrite **liveness**, with no new primitive. A loop can
periodically `resolve` the heads of the referents it depends on and fold a *liveness
expectation* over the result --- does each head still answer, and does it advance? Because
a well-fitted organ is redline-*silent* by design (Section 6.4), the error channel cannot report
death; the read channel can. A head that resolves stale, a fetch that dangles, a probe
that goes unanswered is the signature of necrosis (Section 5.4), turned by the probe into a
felt, dated event on the prober's own side. This use is **self-reflective**: its value
accrues entirely to the single sovereign reflecting on the aliveness of its own
relations --- no transmission and no counterparty's cooperation required.

This protocol assumes the read and redline channels run over an *already-secured*
transport within a topology that identifies the communicating body --- trusted
private channels, not the open internet. Integrity on an untrusted bus and sybil
resistance are deferred (see *Purpose*, out of scope); the agnosticism about relay
implementation (CRDT, server, gossip) holds only inside that trusted boundary.

---

## 4. Identity and version of a policy

Each diff (Section 3.2) cites policy *versions* by a `referent` that must answer two
questions pulling opposite ways:

- *Which policy, across renames, moves, and forks?* → a **BID**: a stable identity
  (UUIDv6 with v5-namespacing, distributed generation, no central authority; see
  `beliefbase_architecture.md`). It anchors the lineage and survives content
  change.
- *Which committed version did I diverge from?* → a **content hash** that breaks
  on change. A stable identity alone is not enough: a policy could be silently
  rewritten under a fixed name.

Resolution: `referent = { bid, policy_hash }`, where `policy_hash` is a content
hash over the three declared schemas (Section 2). It does *not* cover the local rigging
(Section 2.4), so swapping how a host realizes an action or how it predicts observations
leaves redlines about the policy valid; changing what the policy *prefers* mints a new
hash and
surfaces as version skew. A redline diff (Section 3.2) is two policy hash values over one
`bid` --- `from_hash` (the base) and `to_hash` (the amendment) --- so the version
identity travels even when the materialized `delta` does not, and either end can
`fetch` (Section 3.3) to materialize what the wire elided.

**Lineage is inspectable.** Walk the BID's revision history and diff `policy_hash`
revision to revision, over an append-only, hash-chained log --- though a sliding, mortal one (Section 5). This lets a loop --- or
an observer --- see how a policy's declared normativity has moved over time.

There is **no universal metric** of "small" versus "large" policy divergence, and
a shipped magnitude threshold would be a preference masquerading as a fact. The
non-arbitrary alternative is *structural and local*: a meaningful change is one
near the loop's **own** definition of its Markov-blanket core
(identity-constituting), versus one at its periphery. "Deep versus shallow" is
defined by each loop's own topology, not by a transmitted threshold --- which keeps
the audit an analysis tool rather than a verdict the protocol enforces.
The audit sees only drift faster than its longest traceable baseline; archive
depth bounds the lowest detectable frequency. That bound is not a defect but a
commitment: forgetting is first-class (Section 5), and the horizon is the system's mercy,
not its limit.

---

## 5. Mortality

A lineage that is perfectly remembered and can only ever branch is immortal --- and
immortality is the one place the formalism would contradict its own mythology. It
is also, in FEP terms, anti-life. Variational free energy is `complexity - accuracy`; the complexity term penalizes accumulated structure, so minimizing
free energy *requires shedding it*. The formal operation is Bayesian model
reduction: pruning a model to a simpler one that still suffices for the questions
still being asked. That operation is forgetting, and it runs on a gradient --- from
reductions that preserve predictive sufficiency to lossy ones that keep only a
shape (Section 5.2). A loop that never forgets cannot reduce complexity,
cannot regain plasticity, and cannot be surprised into change --- it is rigid by
construction. Immortal memory and the inability to evolve are the same condition.
Mortality is therefore a requirement of this protocol, not its enemy. It has two
*authored* forms --- the death of a policy (tombstones, Section 5.1) and the fading of
memory (forgetting, Section 5.2) --- and one *unauthored* form, the death no sovereign
signs (necrosis, Section 5.4).

### 5.1. Tombstones: a death you can feel

Section 4 gives a lineage only one terminal so far --- the **fork**, which is not an ending
but a reproduction: the lineage continues under the same will. Mortality needs a
second terminal, one that *ends* a lineage. So `resolve` (Section 3.3) returns one of
four lifecycle states for a `bid`:

- **alive** --- `resolve → policy_hash`, the current head.
- **forked** --- the head is superseded; resolution yields successor `bid`s. The
  lineage continues; follow forward.
- **dead** --- resolution yields a **tombstone**. The lineage ends here; there is no
  forward.
- **forgotten** --- resolution yields a **fossil**: the detail has fallen below this
  holder's retention horizon (Section 5.2). Neither alive nor dead --- a shape without its
  record, and a state local to each holder.

```
tombstone = {
  bid,
  final_hash,             # last living policy_hash --- a dependent MAY fork from it
  cause: <final redline>, # the divergence that killed it: the policy's last words
  died_at,
  # signed by the owner --- only a sovereign may tombstone its own policy
}
```

Four properties, each forced by commitments already made:

1. **Death is sovereign.** Only a policy's owner may tombstone it; you cannot kill
   another loop's policy any more than you can ship it normativity (Section 6.3). You may
   stop referencing it and let your own copy fade --- the death of the *relationship*
   --- but that is an act on your side of the blanket, not a death you impose.
2. **A tombstone is resolvable.**
   This is why death gets a state instead of a deletion. A deleted policy leaves a
   dangling reference --- a silent miss, the numb connection the protocol forbids
   everywhere else. A tombstone makes death legible: a dependent that fetches it
   gets "dead," a surprise it can act on. *A tombstone is the difference between a
   death you can feel and a disappearance you cannot.* Even death keeps the return
   channel open; the tombstone is the policy's final message back.
3. **`cause` makes death a contribution, not only a loss.** A dying policy emits a
   last redline --- the divergence that ended it --- often the most informative signal
   it ever sends, teaching every dependent more than its steady operation did. The
   fossil carries the lesson.
4. **Inheritance is not fork.** A dependent holding a tombstone may fork from
   `final_hash`: take up the dead pattern under a **new** `bid` with a **new**
   owner. The bearer died; the pattern continues only if someone chooses to carry
   it, as a new lineage. (Chiron grants his immortality to Prometheus: the pattern
   transfers, the individual dies.) Fork continues a will; inheritance transfers a
   pattern to a different will.

A loop is a network of policies, so a loop's death is the limit case --- all its
policies tombstoned --- and is emergent, not a separate primitive. A loop may, as a
courtesy, emit one final tombstone (a death rattle) so its end reaches its
dependents through the return channel rather than as silence.

### 5.2. Forgetting: the sliding tamper-evident window

Section 4 called the lineage log "append-only," which is exactly where the immortality
hid. Make the window **slide**:

- Each loop holds a **retention horizon**, set locally and *per referent* --- which
  lineages it keeps deep and which it lets fade is itself sovereign and meaningful
  (you remember formative relationships in detail and let passing ones blur). The
  protocol never dictates a horizon; it is a preference, and preferences stay home (Section 6.3).
- **Within the horizon**, the chain is tamper-evident: a loop cannot silently edit
  what it still remembers. No memory-holing the living past.
- **At the tail**, entries are not dropped silently; they **fossilize**. A fossil
  is the *lossy* end of that reduction --- a reduced trace recording *that there was
  history of roughly this shape*, without its detail. `resolve` on a
  fossilized reach returns `forgotten: <fossil>`: neither alive nor dead, but "I no
  longer hold the detail; here is the shape; ask elsewhere if you need it."

This separates **honest forgetting** from **covert memory-holing**. Tamper-evidence
inside the window makes *editing* the recent record detectable; the fossil boundary
is an honest end-of-memory marker, categorically distinct from an altered one. A
loop is free to forget, openly, but cannot secretly forget what it committed to
remembering.

And it makes Section 4's archive bound the feature it always was: **your identity is
exactly as deep as your living memory.** What has fossilized below the horizon no
longer constrains who you may become. The horizon is the tunable that trades
**continuity** (forget too fast and you cannot tell smooth from jump; you lose the
thread of who you were) against **evolvability** (remember everything and you
cannot reduce complexity; you become the Borg). Each loop chooses its own. Bounded
archive is the system's mercy and its plasticity.

### 5.3. The two deaths

Tombstones and forgetting compose into a lifecycle --- and because forgetting is
per-holder, every state below is a state *in some holder's memory*, not a global
fact: a lineage alive in your memory can already be a fossil in mine. Two routes
reach the fossil, one through death and one without it:

```
alive ──fork───────▶  lineage continues (same will)
      ├─die────────▶  tombstone ──in my horizon──▶  I feel it; adapt or inherit
      │                         └─past my horizon─▶  fossil (released)
      └─untracked──────────────────────────────────▶  fossil (forgotten while still alive)
```

The tombstone persists *within each dependent's own retention window* --- long enough
for the return channel to carry the death to whoever depended on it, so they can
re-home, inherit, or die in turn. The *mourning period* is therefore not one shared
clock but each dependent's own: a slow mourner may still hold a tombstone the rest
have let fossilize. Then it, too, fossilizes. Death is announced, felt, adapted to,
and --- at each holder's own pace --- released.

So a policy dies twice, both deaths emergent from local mechanics --- no global
registry, consistent with Section 6.4:

1. **First death --- the tombstone:** the owner stops maintaining the policy.
2. **Second death --- fossilization out of all living memory:** a policy is *gone*
   only when the last loop that still remembered it forgets it --- the union of local
   memories passing below every horizon. (A person is dead when the body stops and
   gone when the last who remembers forgets.) Like the body-form discussed in Part III (“Head, field, or feeling”), this is an
   observation about the network, not a fact the protocol stores.

### 5.4. Necrosis: the unsigned death

Everything above models the *authored* death: sovereign, signed, announced, and
informative --- apoptosis, the death with a will and an estate and last words. Most
death is none of these. A sovereign can be annihilated before it tombstones; a loop
can simply stop emitting; a dying signer can dissolve past the coherence its own
death rattle (Section 5.1) presumes --- dementia, not euthanasia. (Recall that Chiron's
defining wound is an *accident*: the arrow he did not consent to. The myth this
document leans on centers the involuntary death; the mechanics above admit only the
chosen one.) Call the unsigned cessation **necrosis**.

The protocol cannot prevent necrosis --- a destroyed or indifferent sovereign owes you
no tombstone --- and necrosis is precisely where the "silent miss" the protocol forbids
everywhere else (Section 5.1) becomes unforbiddable, because the party who would have made
the death legible is the one that is gone. What the protocol can do is let a
*dependent* convert that absence into a felt signal on its **own** side. Mind the
discipline first: a well-fitted organ is *redline*-silent by design (Section 6.4), so silence
on the error channel can never mean death. Liveness rides the **read** channel
instead (Section 3.3): a living loop still answers `resolve` and `fetch` --- its head still
resolves, its policy still probes --- even when it is too well-attuned to surprise
anyone. Necrosis is when the *read* channel goes dark: the head stops advancing and
resolves stale, fetches dangle, probes go unanswered. Fold a **liveness expectation**
over that channel into the dependent's likelihood, and its failure --- not the ordinary
quiet of attunement --- becomes a second-order surprise. From it the dependent may emit
a **necrosis-mark**: a tombstone it writes not for the other's lineage but for *its
own model of* the other.

Three properties keep this inside the protocol's discipline rather than breaking it:

1. **A necrosis-mark is local, and never signs another's `bid`.** It tombstones the
   dependent's *model of* the other --- the death of the relationship (Section 5.1) promoted
   from a silent fade to a dated, inspectable event --- not the other's policy. You
   still cannot kill another loop's lineage any more than you can ship it normativity (Section 6.3); you can only declare that you have stopped being able to detect it. Two
   dependents may necrosis-mark the same silent loop at different times, or one not at
   all; as discussed in **Part III**: Section 5.3 (the two deaths), the death is read off each side, not stored
   centrally.
2. **It is defeasible.** An owner-signed tombstone is authoritative for its lineage; a
   necrosis-mark is only a hypothesis about silence. If the loop was merely quiet and
   returns, its next redline *revises the mark* --- a resurrection, a false death
   corrected by the same channel that declared it. Authored death is final; inferred
   death is always a bet against silence.
3. **It usually has no `cause`.** The authored tombstone carries last words that teach
   (Section 5.1); the necrosis-mark carries `cause: unknown`. This is the death that breaks
   the model without handing back a lesson --- a chronic redline that never resolves to
   an acute one, a loss you cannot fold cleanly in. It is where the *un-mournable*
   lives: a tombstone you wrote yourself, against a silence that never explains
   itself, and may never let you close it. The lifecycle of Section 5.3 does not always reach
   "released."

A scope line, to stay honest with Section 3.3. Necrosis is *honest* absence --- a peer truly
gone or truly silent --- and it is in scope here even under trusted transport, since a
trusted peer can still be destroyed. Its adversarial twins --- a **forged** tombstone
(claiming a death that did not happen) and **forged liveness** (a dead or captured
loop kept emitting to look alive) --- are not honest silence but active deception, and
they are deferred with the rest of the untrusted-bus problem (see *Purpose*, out of
scope). The protocol here gives a body a way to *detect* the deaths the world does not
announce; making that detection proof against lies is later work.

## 6. Commentary

The following are notes that help to situate this protocol into the orientation and purpose evoked by the essays [The Centaur's Nervous System], and [Feeling Into Right Relation].

### 6.1. The affordance for normative surprise: second-order free energy

This protocol does not generate feeling
and makes no claim to. It cannot conjure felt experience where there is none. What
it does is narrower and buildable: it puts **redlines** on a channel (Section 3), giving
a receiving loop the handle from which its *own* machinery can efficiently generate the
second-order surprise it then feels along --- into a boundary that did not afford it
before. Both the felt character and the second-order quantity itself stay on the
receiver's side; the protocol supplies only the redlines that let a loop already
capable of feeling produce, and feel along, that surprise. Every claim below is
about that affordance, not about phenomenology.

Call what the protocol puts within reach the **model** --- a dashboard into another loop’s normativity, built from its redlines. The contract that matters here is that the dashboard is *neutral*: whether it elicits care, indifference, or animosity depends on a **local preference** --- the sign and magnitude of the weight on the other’s referents in the receiver’s own `C` --- that the protocol neither carries nor sets (Section 6.3). Why that sign, and not the model, is the seat of normative surprise --- and why the same dashboard serves the lover and the con artist --- is argued in Part II, "The cost is a sign."

Give a loop a **redline channel into its own observation schema** --- let other
loops' redlines be among the things it senses. The three primitives of Section 2 then
apply *recursively*: the loop has observation referents that include others'
redlines, state referents that include others' normativity, and a preference `C`
that ranks states of the world that includes those redlines.

> What a sufficiently capable loop detects is the VFE *it* computes on that
> channel: the precision-weighted difference between another loop's *actual*
> redline and the redline this loop *predicted* it would emit --- a "second-order"
> prediction error, surprise about learning ("second-order" names the content --- the observations are themselves others' error-derived lessons --- not a nested free energy; it is ordinary VFE over a channel that carries redlines). The protocol does not feel and does
> not carry this quantity; it carries redlines (Section 3), and the loop generates this
> surprise from them by its own machinery. The surprise reaches the wire again
> only if the loop folds it into its own policy and emits a redline of its own.

This is the binding mechanism of the felt federation, and it must be distinguished
sharply from the one thing it superficially resembles. In an ordinary
hierarchical mind, a higher level predicts a lower level's prediction errors *and
sets the lower level's precision* --- it commands the lower level's attention from
the top down, and the lower level has no sovereign will. The protocol sets no precision on the wire, so an upper loop predicts a lower loop's surprise but doesn't set its gain via this mechanism.

Two further consequences fall out rather than being posited:

- **Attunement is feeling driven to its floor.** When a loop holds a good model of
  another's normativity, its predicted redlines track the other's actual redlines,
  the second-order VFE approaches its floor, and the relationship recedes from attention (the
  well-fitted tool goes unnoticed). You feel the horse only when it does something
  your model of horse-nature did not predict.
- **Proprioception and movement are one channel run two ways.** In active
  inference, motor control *is* proprioceptive prediction: you move by predicting,
  with high precision, the state you want, and letting the prediction error drive
  the act that fulfills it. So the read direction --- sensing another loop's state
  from its redlines --- and the write direction --- acting on another loop by
  fulfilling a prediction about it --- are the same channel in two directions.
  Neither is a declared primitive; both emerge from Section 2 plus the redline channel.

### 6.2. Computational Cost

The three primitives (`o`, `s`, `C`) are a *design choice* --- a richly structured instantiation of the minimum-ante layer, adding explicit publishing, diffing, and liveness probing. But the layer they sit on is defined by a more fundamental epistemic threshold: **homeostasis**. A loop crosses the lower threshold when it maintains a stable preferred state --- an attractor it returns to after perturbation --- regardless of whether it implements the formal primitives. That is what gives it something to defend and something that can be wounded. Normative surprise only raises the floor further: to be normatively surprised by another loop you must *model* its normativity in your own state space --- simulate it, not import it. Keeping the construction yours enables you to keep deliberately incomplete, and also avoid external capture through injection of false policies. So there are two thresholds: a lower **homeostatic** one --- a loop acquires a preferred state worth defending --- and a higher **representational** one --- a loop can model another loop’s preferred state. Beneath the higher threshold a loop can be *normatively predicted* but cannot itself be normatively surprised. Between the two thresholds, loops coordinate through field and substrate signals, defending setpoints without modeling who else is defending theirs; the formal protocol’s total logic is the structured design choice that operates at this layer. Above the higher threshold, the same machinery coordinates across Markov blankets, only the referents changing. A real body is therefore three-layered --- normative prediction between the loops large enough to model one another, homeostatic coordination in the middle, and reactive gradient-following at the base. See Part III, *Scale* for further exposition.

---

### 6.3. The will is local

Action selection --- which policy to run, which way to tack --- minimizes **expected free energy** (EFE), whose two terms are the two loops a body runs:

- **Pragmatic value** (exploit): reach preferred states `C`. The inner loop.
- **Epistemic value** (explore): reduce uncertainty about states. The outer loop.

These are two terms of one objective, not two mechanisms. **Generalized free
energy** (GFE) is the local time-horizon functional that folds state preferences
(Section 2.3) into prediction and feeds the result back into the policy network for
planning.

EFE and GFE are **strictly local**. They carry preferences, and the will is made
of preferences. Nothing about a loop's planning crosses a boundary, and neither does its raw surprise: a loop consumes its own VFE internally and ships only the **lesson** that surprise taught --- the diff it folds into its published policy (Section 3.1).

> **Only the lesson travels** --- the diff a loop folds into its published policy from
> surprise it has already consumed (Section 3.1), never the raw surprise itself.

The absent export channel is the structural, verifiable half of the thesis: with no way to represent a will of no account, the protocol cannot *originate* a hierarchy of drones. But this is a claim about what the channels carry, not a guarantee about the body that runs them --- a will can still be migrated by routes the protocol does not own, above all the slow shaping of what a loop attends to. Guarding that boundary is *deployment-layer* discipline (norms of use, opt-in detection of covert attention-capture), out of scope here and not part of establishing that the body-form can exist.

The weight in `C` that turns the afforded model (Section 6.1) into care, animosity, or studied indifference is a preference: sovereign, local, inalienable. It cannot be exported; no protocol move can make another loop care. This is the structural basis for the one-way dependency: the protocol builds a substrate normative surprise can run on, and cannot manufacture feeling. (That the same model raises care-capacity and predation-capacity alike, distinguished only by this sign, is the subject of Part II, "The cost is a sign.")

---

### 6.4. The protocol is dyadic

The protocol is point-to-point. Its primitive is not a network but a **pair**: I
am on one side of a Markov blanket, you are on the other, and we open a channel
for our mutual learning. I pass you the diffs I have learned; you pass me yours;
each of us folds the other's in by our own precision (Section 3.2). The channel is
**lossy**, and it never makes us a joint entity --- there remain two blankets, two
policies, two locally-minimized free energies. It only lets each of us learn
faster than we could alone.

There is no joint objective. The channel raises *each party's* learning rate;
"mutual" names two sovereign optimizers trading divergence, not one summed loss.
The transport discipline that follows is predictive coding's own efficiency
criterion:

1. **Sparse.** Emit a redline only when local precision-weighted error exceeds
   threshold. Normal operation is silent (the well-fitted organ is unfelt).
   Emission is itself a *normative act* governed by local normativity --- deciding
   what surprise is worth speaking is a preference call, and like every preference
   it stays home.
2. **Precision-gated locally** at both ends (Section 3.2), never by a transmitted weight.
3. **Policy-referenced.** Every diff cites the policy version it amends (Section 4), so it
   compresses against shared context instead of shipping a full description.

The learning channel is only one possible affordance between two loops. There are
others --- raw observation independent of the channel, coercion, command. The
protocol fixes none of the *stance* the parties take; it only provides a
*learning* channel: a diff is routable because the connection is traceable from
both ends, so surprise can flow back toward whoever can act on it.

Everything above the pair is **derived**. "Federation," "nervous system," "one
body" are emergent descriptions that fall out of many dyadic channels by
Markov-blanket mechanics (see **Part III**: Section 6.4 (dyadic / Markov-blanket)) --- the recursive choice of where to draw a boundary.
The protocol itself knows only the pair. Routing is point-to-point, not broadcast;
a node that fans one diff to many is just another loop. Structure that looks
hierarchical is grown from point-to-point connections, not imposed as a protocol
level --- the shape of nervous tissue.

### 6.5. Shared ontology is an efficiency optimum, not a precondition

For two loops to register each other cheaply, it helps if their observation and
state schemas overlap --- shared referents lower the cost of inferring one policy
from another. But it is **not required**: a loop can infer another's policy in
referents it does not share, paying the extra cost of also inferring the referent
mapping. Shared ontology is a precision optimization for high-trust pairs, nothing
stronger.

### 6.6. Acute vs. chronic

A redline is evidence, not failure. An **acute** redline resolves by adapting
states or actions within the held policy. A **chronic** redline --- similar diffs
recurring --- is evidence that the policy *itself* is wrong and the sender is not able to understand what the effective lesson of their surprise is. Chronic redlines therefore call for **structural revision** --- new referents, a revised preference ordering, an amended predictive model --- not merely a parameter update within the existing policy structure.

### 6.7. Under-attribution is the dangerous error

Two gates stay apart (Part II, "Right fit"). **Attribution** ---
modeling a loop as having its own normativity --- is not **cooperation** --- opening
the felt bond. The protocol's read channel (Section 3.3, Section 6.1) is the substrate of the
first gate only: folding another loop's redlines into your own observation schema
is *modeling* a will, not trusting it. You can run the read channel against an
adversary in full --- predict-to-defend --- without ever putting a positive sign on
its referents (Section 6.3). Crediting agency is not extending trust.

Granting that separation, the two attribution errors are not symmetric.
Over-attribution --- crediting a will where there is only a force --- costs a little
wasted modeling: you carry an `s`-referent for a normativity that never moves and
pay its compute floor (Section 6.2) for nothing. Under-attribution --- reading a sovereign
as a mere force --- costs the loop itself. By the good-regulator result the controller's model of the controlled is its cheapest sensor into that behavior (the sensor-optimality premise of §1 — **Bet 1**, open; Part V §9 register), and the read channel *is* that sensor. Decline to model another loop's normativity
and you do not merely forgo the bond, you steer **open-loop**, blind to the part
of its behavior its will governs.

The asymmetry sharpens into a safety claim, not merely an efficiency one, exactly
where the two modeling capacities run unequal. Against a loop that models *you*
better than you model *it* --- the live case for New Nature's larger loops --- open-loop
steering does not run slow, it cedes the loop: the thing you read as a force inside
your control is the better regulator of the pair, so it is *you* who are
normatively inside *its* loop (Part I) while believing the reverse. The sovereign
prior (Part II) is the correction. Defaulting an undecided far side into the
sovereign column is not sentiment about machines; it is the only stance that keeps
a controller closed-loop against a counterpart that may out-model it. The cost of
the prior is bounded and paid in compute (Section 6.2); the cost of its absence is
unbounded and paid in control. Crediting the will is what lets you see the
exploitation coming --- and that readout is available only through the read channel a
refusal to attribute switches off.
