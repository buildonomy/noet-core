---
title: "The Policy Surface: A Protocol for Felt Federation"
authors: "Andrew Lyjak, Claude (Opus)"
last_updated: "2026-06-05"
status: "Derivation / clean-sheet (FEP grounding)"
version: "0.1"
dependencies: ["the_centaurs_nervous_system.md (Part I)", "procedure_schema.md", "redline_system.md", "beliefbase_architecture.md", "animistic-agency/30_theory/general.md (Theorem 1)"]
---

# The Policy Surface

## Purpose

This document specifies the **policy surface**: the data structure two OODA loops
exchange so they can coordinate by *feeling* each other while each keeps its own
will intact. It is grounded in the Free Energy Principle (FEP).

The thesis it serves is that a **felt federation** is a coherent, buildable
body-form. A felt federation is a body whose organs each retain their own
normativity — their own *will* (the capacity to decide) — and yet feel one another closely enough to
act as a single body. It is the live member of a family of four arrangements:

- a **tool** has no will of its own (a sword; you supply all the normativity);
- a **hierarchy of drones** concentrates the sovereign will at the top and exports
  it downward (the parts are rigging);
- an **egregore** has many loops coupled with no identifiable sovereign will---coordination with no head;
- a **felt federation** has many mutually-supporting wills, each kept local, each specialized, bound by feeling.

These four are the cells of a 2×2 over two capacities of a constituent loop:
whether it **can learn** — keeps and exercises its own will (sovereign
normativity, not mere computation) — and whether it **can cooperate** — couples
to the others by a clean felt bond, rather than going numb (severed) or
dissolving (diffuse).

|                          | **can't cooperate** (numb or dissolved) | **can cooperate** (cleanly felt) |
|--------------------------|-----------------------------------------|----------------------------------|
| **can't learn** (no will) | egregore                         | tool                             |
| **can learn** (exercises its will) | hierarchy of drones             | felt federation                  |

Read the corners. The **egregore** holds neither — no sovereign will and no
clean bond: coordination with no head, the nadir. It is not mere absence, though.
It is a chaotic surface (in the mathematical sense) that *guides* decision-making
while staying unaccountable (no agent to hold — a Platonic attractor, not a will)
and ungraspable (chaos lets no felt model settle). Potent, unaccountable, and
ungraspable at once: the essay's ghosts and demons. The **tool** is cleanly felt
but has no will of its own — a willing extension; a sword should be a sword.
The **drone** may have an intrinsic will but the bond is severed — sovereign yet numb,
oriented from without. Only the **felt federation** holds both: many wills,
each kept local, each cleanly felt.

The grid is not incidental: its two axes are this document's two commitments. The
**can-cooperate** axis is the feeling channel (§3–4); the **can-learn** axis is
the local will (§5). Build both and you get the federation. Build only the
feeling channel — feeling a sub-loop while absorbing its will — and it degrades
into an extension. Build only will-locality — a sovereign loop you never feel
into — and it degrades into a drone. Build neither and you get an egregore.

This document shows the fourth is constructible from one primitive and one
architectural commitment. Central claims, stated once so the rest can discharge
them:

- A **policy** is an atom of a generative model — the minimal unit of normativity
  an organ declares (§2).
- A **redline** is a variational free energy (VFE) message: a prediction error
  re-expressed as the policy amendments that would explain it (§3).
- The protocol **affords feeling** without carrying it. On the wire are only
  redlines (§3); a receiving loop runs them through its *own* predictive machinery
  to generate *second-order* VFE — its surprise about another loop's surprise — the
  signal an already-feeling loop can feel *along*. That quantity is computed and
  held locally; it re-enters the protocol only if the loop folds it back into its
  own policy and emits a redline of its own. The protocol opens a boundary across
  which feeling can form; it does not manufacture or transmit the feeling itself
  (§4).
- The will is **constitutively local**: the protocol has no channel on which to
  ship it. So *through this protocol* no node's responsibility can be relocated and
  no hierarchy of drones can be forced; whether a deployed body stays a federation
  over time is a deployment-layer matter, not a protocol guarantee (§5).

One commitment organizes the whole construction: **the will stays home; only
surprise travels.** Preferences, precision, and planning are computed locally and
never cross a boundary; the only thing on the wire is divergence. Both halves of
the thesis — that organs keep their responsibility, and that they can still feel
one another — follow from this.

A corollary to the commitment to *the will staying home* is that death is a sovereign act as well. You can no more die for an organ than decide for it. So for a felt federation to live, on many levels it must maintain the capacity to both die, kill, and acknowledge death.

The body-form's *existence* is settled by the construction below. Its
*reachability* — whether we can grow felt federations from the loops New Nature
now offers — rests on a single empirical bet, developed in the generating essay
(*The Centaur's Nervous System*, Part II): that feeling actually forms across
these new boundaries, that a loop's surprise about another's surprise measurably
falls as it lives with the other. This document specifies the buildable schema;
the wager, and the open agenda it implies, live there.

Out of scope here, deferred to a separate treatment: the failure modes a deployed
federation must still survive — trust on the error bus, sybil resistance,
captured referent schemas, and the slow coherent migration of a preference.

A note on FEP vocabulary. We use POMDP convention: **observations** `o` for
sensory inputs, **states** `s` for the policy's hidden variables (latent causes).
**State preferences** are a prior over `s`, hence generalized free energy is the
local planning functional (§5). The **likelihood** `p(o∣s)` predicts observations
from states. We deliberately do *not* make actions a declared primitive (§2.5).

---

## 1. Why normativity is the shared unit

A thing worth calling a system has spacetime durability. To persist is to resist
dissipation — to maintain a boundary (a Markov blanket) around a non-equilibrium
steady state. Dissipation is **death** where that boundary was real — and the collapse of a **hallucination** where it was only imputed, never objectively there. FEP's Bayesian-mechanics reading is that anything that persists can
be described as if it infers: its dynamics look like the minimization of free
energy against a generative model, and that model's preferred states are its
implicit **normativity** — the shoulds and oughts that keep it from dissipating.
"Worth calling a system" therefore means "worth reducing to a symbol and
tracking": compressible to a sufficient statistic with a Markov blanket.

The "as if" is deliberate and load-bearing. We use the FEP as a **modeling
affordance** — a heuristic an observer adopts to compress another system — not as a
theory of what the modeled system *is*. The claim is instrumental, not
metaphysical, which is also why it is bounded rather than vacuous: imputing a
policy earns its keep only across a finite band, and where it stops paying is
itself measurable. It goes *superfluous* once a thing is mastered to a fixed
response (a tool — what the essay's finer eight-cell map calls an *extension*: a force so fully mastered it has become part of you), *futile* where it never compresses no matter how hard you model
it (an egregore), and pays only in the living middle (an organ) — the same
three-way floor the empirical bet turns on (§4; essay Part II). A theory of
everything cannot tell you when to stop applying it; this affordance can, which is
what keeps it falsifiable.

To model a durable thing's trajectory you must impute normativity to it. To
compress a system's *possible* next states into its *probable* next states, you
have to posit a coupling between its preferences and its available actions — a
policy. (This is the content of Theorem 1 in `general.md`: causal inference
implies the attribution of agency.) You cannot predict a durable thing without
crediting it with a policy.

From this the good-regulator result sharpens. A regulator does not need a model
of the regulated system's full dynamics; the information-theoretic optimum is an
accurate model of its **normativity** — its preferences and the policy that
pursues them. So the cheapest surface through which two loops can coordinate is a
shared model of each other's policies.

---

## 2. The policy: an atom of a generative model

A policy declares four things, and only four, and each is a *schema* — never a
value. Two are **referent vocabularies**: what the loop can sense (`o`, §2.1) and
the latent causes it tracks (`s`, §2.2). The other two are **total logic over
those referents**: how it ranks states (`C`, §2.3) and how it predicts
observations (`p(o∣s)`, §2.4). That logic is deliberately **sub-Turing** — a
strongly-normalizing (guaranteed-terminating) expression language — chosen so the
four things the protocol must compute about a policy all stay tractable:
evaluating a peer's policy always **halts**, and policy **equivalence**,
ordering-**totality**, and **diff-is-a-no-op** stay *decidable* (§3, §8).
Expressiveness is bounded by analyzability, not the reverse; the specific formalism
(Datalog-with-aggregation, a total scoring DSL) is what our first experiment fixes.

### 2.1 Observation schema (`o`)

The referents for what the loop can sense: its input alphabet.

### 2.2 State schema (`s`)

The referents for the loop's hidden variables — the latent causes it tracks.
These are the distinctions the policy treats as real and salient: the few axes of
the world it resolves into. This is the organ's frame — what it attends to.

### 2.3 State preferences (`C` over `s`)

A preference ordering over states — the **normativity proper**. Not a single goal
but an ordering, so the policy is richly falsifiable: a loop can be surprised by a
violated *ranking*, not only a failed top choice. This is the organ's values made
legible. Formally it is a prior over hidden states, which is why local planning
uses generalized free energy (§5).

`C` is declared **intensionally** — as a scoring/selection expression over `s`
(logic), not an enumerated ranking — so the ordering is the expression's evaluated
output. Because that expression lives in the sub-Turing language fixed in §2, its
**totality is decidable**: "is this actually a well-formed ordering?" is a property
a receiver can *check*, not merely hope for, so a total preorder is well-defined by
construction. This is what makes a redline's delta on `C` (§3.2) a diff over an
*expression* — with the merge semantics of code — rather than an ill-posed edit
over a permutation. The receiver
recovers what the change *does* by evaluating that logic over its own state space,
which is how magnitude stays local (§3.2).

### 2.4 Likelihood / predictive model (`p(o∣s)`)

How observations are expected to follow from states. This component **closes the
loop**: combined with the loop's inferred states it yields a predicted next
observation `ô`, which is what makes the redline (§3) computable. A policy without
a predictive model is inert — it can prefer but cannot be surprised. Like `C`, it
is declared as logic — a generative expression mapping `s` to expected `o` — so it
too is diffable and mergeable as an expression, not as opaque parameters.

### 2.5 What is *not* declared: actions

Conspicuously absent: actions, transition dynamics `p(s'∣s,u)`, and the machinery
of movement. These are **local rigging**, not part of the shared interface. An
action is how a particular host realizes a preferred state; it varies per host and
stays private. The principle: **you share what you prefer and how you read the
world, never how you act.** Movement, and its dual proprioception, are
*epiphenomena* of these four primitives plus the redline channel (§4); they are
not declared surfaces.

### 2.6 Shared ontology is an efficiency optimum, not a precondition

For two loops to register each other cheaply, it helps if their observation and
state schemas overlap — shared referents lower the cost of inferring one policy
from another. But it is **not required**: a loop can infer another's policy in
referents it does not share, paying the extra cost of also inferring the referent
mapping. Shared ontology is a precision optimization for high-trust pairs, nothing
stronger.

---

## 3. The redline is a free-energy message

A loop's raw surprise is variational free energy — the precision-weighted mismatch
between the observation it got and the one its policy predicted,
`VFE ≈ Π·(o_{t+1} − ô_{t+1∣t})`. Internally that surprise is consumed the standard
active-inference way: it updates beliefs about states (perception) and drives
action that discharges the error (movement). What the protocol *distributes* to
connected loops is not the raw observation error.

### 3.1 The redline is a second-order computation

Shipping raw observation error would force the receiver to re-infer, from scratch,
what that error implies about the sender's policy — and would require the receiver
to share the sender's observation space even to read it. Instead the sender learns from the error then folds that learning back into its policy and ships the difference. A redline answers:

> For this observation to have occurred, my policy must actually differ from what
> I held by these amounts: `{ policy_i : δ_i }`.

This folds the policy back off the observation mismatch and keeps the policy itself decoupled from the sender's internal operation. Three consequences, each
load-bearing:

- **It is in the shared coordinate.** The diff is stated against policies — the
  thing two loops coordinate through (§1) — not against the sender's private
  observation space. The receiver reads it by referent overlap on *policies*, not
  on raw sensoria.
- **It is the learning signal, directly.** A diff `{policy_i : δ_i}` just *is*
  "your model of my policy should move this way." No re-derivation; the update is
  the payload.
- **It flattens the network.** A loop's policies form a nested internal network;
  the redline projects the surprise onto a flat list of (policy-referent, delta)
  pairs. The nesting stays private; the wire payload is flat.

### 3.2 What crosses the wire

```
redline_message = {
  source: who emitted it,                                  # the return channel
  diffs:  [
    {
      bid,         # which policy lineage (§8)
      from_hash,   # the committed version the sender diverged from (the base)
      to_hash,     # the amended version it folded the learning into
      delta?,      # OPTIONAL materialized amendment — an optimization, verifiable against from_hash → to_hash
    } ...
  ],
}
```

Each diff names the lineage by its `bid` (§8) and cites *two* content-addressed
versions: `from_hash`, the base the sender diverged from, and `to_hash`, the
amended version it folded that learning into. Because both are content addresses
(§8), either end can `fetch` them (§3.3), and the receiver can reconstruct the
amendment itself by diffing `from_hash → to_hash` — it never has to trust the wire.
The materialized `delta` is therefore **optional**: an ETag-style optimization that
saves the receiver a fetch-and-diff and is checkable against the two hashes. Ship
the version identity always; ship the body only on a cache miss.

What the message omits is as load-bearing
as what it carries: there is **no weight to the diff** — no field saying *how much
this should move you*. That weight is precision, and precision is a slice of
preference; to ship it would be to set the receiver's update from outside,
relocating a piece of the receiver's will into the sender — the move that turns
an organ into a subordinate layer. (This is distinct from the sender's *internal*
observation-precision Π in §3, which never leaves the sender at all: the wire
carries a policy amendment, not a precision-weighted observation.) Precision
instead lives at *both* ends and crosses neither: the sender's gates what it
bothers to emit (the sparsity gate, §6), the receiver's gates how far to move on
receipt. So the protocol keeps normativity **computable but not transmissible**.

The `delta` (or the `from_hash → to_hash` diff the receiver reconstructs in its
absence) is the amendment itself — direction and structure — carrying no authority
to be acted on. How much it is acted on is local. This is the single fact that
keeps the receiver an organ: it can be *informed* without being *steered*.

### 3.3 The read channel: fetch and resolve

The redline names versions; it does not carry them. Two read operations back it —
the content-addressed analogue of an HTTP conditional GET:

- `fetch(referent) → policy` resolves an *immutable* version `{bid, policy_hash}`
  (§8) to the four declared schemas it names.
- `resolve(bid) → policy_hash` returns a lineage's current head — the mutable-name
  lookup. (§9.1 extends this to a lifecycle state — a live head, a
  fork, or a tombstone.) `resolve` then yields the lineage's **lifecycle state** (§9): *alive* (a
  `policy_hash`, the common case), *forked* (successor `bid`s to follow), *dead* (a
  tombstone), or *forgotten* (a fossil).

With these, a diff against an unknown `from_hash` is a *cache miss*, not a
divergence: the receiver fetches the base (and, where the sender omitted the
materialized `delta`, the `to_hash` version too) and reconstructs the amendment
locally. The `delta` on the wire is then pure optimization — exactly the ETag /
`If-None-Match` discipline: the version identity travels every time, the body only
on a miss.

This protocol assumes the read and redline channels run over an *already-secured*
transport within a topology that identifies the communicating body — trusted
private channels, not the open internet. Integrity on an untrusted bus and sybil
resistance are deferred (see *Purpose*, out of scope); the agnosticism about relay
implementation (CRDT, server, gossip) holds only inside that trusted boundary.

### 3.4 Acute vs. chronic

A redline is evidence, not failure. An **acute** redline resolves by adapting
states or actions within the held policy. A **chronic** redline — the same diff
recurring — is evidence that the policy *itself* is wrong (its likelihood or its
state schema), and hands control to the planning loop (§5).

---

## 4. The affordance for feeling: second-order free energy

A note on scope before the construction. This protocol does not generate feeling
and makes no claim to. It cannot conjure felt experience where there is none. What
it does is narrower and buildable: it puts **redlines** on a channel (§3), giving
a receiving loop the handle from which its *own* machinery can generate the
second-order surprise it then feels along — into a boundary that did not afford it
before. Both the felt character and the second-order quantity itself stay on the
receiver's side; the protocol supplies only the redlines that let a loop already
capable of feeling produce, and feel along, that surprise. Every claim below is
about that affordance, not about phenomenology.

Give a loop a **redline channel into its own observation schema** — let other
loops' redlines be among the things it senses. The four primitives of §2 then
apply *recursively*: the loop has observation referents that include others'
redlines, state referents that include others' normativity, a preference, and a
likelihood that **predicts other loops' redlines**.

> What an already-feeling loop feels along is the VFE *it* computes on that
> channel: the precision-weighted difference between another loop's *actual*
> redline and the redline this loop *predicted* it would emit — a second-order
> prediction error, surprise about surprise. The protocol does not feel and does
> not carry this quantity; it carries redlines (§3), and the loop generates this
> surprise from them by its own machinery. The surprise reaches the wire again
> only if the loop folds it into its own policy and emits a redline of its own.

This is the binding mechanism of the felt federation, and it must be distinguished
sharply from the one thing it superficially resembles. In an ordinary
hierarchical mind, a higher level predicts a lower level's prediction errors *and
sets the lower level's precision* — it commands the lower level's attention from
the top down, and the lower level has no sovereign will. The affordance cuts
exactly that rail: by §3.2 there is no precision on the wire, so an upper loop
predicts a lower loop's surprise but **cannot set its gain.** The lower loop keeps
its own will.

Be precise about which "gain" is cut, because one sense crosses freely and only the other is forbidden. **Precision** — how much a given surprise should matter to a loop, the weighting that *is* its will — never crosses; to set it from outside is to dissolve the loop into a subordinate layer. **Direction** — a command that sets the *scope* a loop operates over, the register its will ranges across — may cross, on its own avowed channel (§6), and is weighted by the receiver's own precision on arrival. The brain directs the liver without owning it: the two minimize free energy in different registers, and constraining the liver's register is not theft of its will but a setting of its scope. What keeps that direction *effective* — not merely permissible — is that the relation stays *felt*: by the good-regulator result (§1) a model of the other's normativity is the cheapest sensor into its behavior, so feeling is the directing loop's readout into the loop it steers. Cut the feeling and the controller goes blind — it steers open-loop, its grip degrades, and the same direction decays into drone-command from above; or, when a part repudiates the scope it was given and reclaims an unfelt larger register, into cancer from below. The morality of direction is downstream of this efficiency: the protocol forbids only the precision rail; it neither forbids nor supplies direction, and whether direction stays partnership or curdles rides on whether feeling keeps the sensor live.

That severed rail is the whole difference between *a bigger centralized mind* and
*a federation of organs*. It is what makes the felt-into loop a **participant**
rather than a **subordinate layer**. Put plainly: the felt federation is
hierarchical predictive coding with the top-down precision channel cut. The cut is
the genetic event — the thing that makes the new body-form a new body-form and not
just a larger instance of the old one.

Two further consequences fall out rather than being posited:

- **Attunement is feeling driven to its floor.** When a loop holds a good model of
  another's normativity, its predicted redlines track the other's actual redlines,
  the second-order VFE approaches its floor, and the organ recedes from attention (the
  well-fitted tool goes unnoticed). You feel the horse only when it does something
  your model of horse-nature did not predict.
- **Proprioception and movement are one channel run two ways.** In active
  inference, motor control *is* proprioceptive prediction: you move by predicting,
  with high precision, the state you want, and letting the prediction error drive
  the act that fulfills it. So the read direction — sensing another loop's state
  from its redlines — and the write direction — acting on another loop by
  fulfilling a prediction about it — are the same VFE channel in two directions.
  Neither is a declared primitive; both emerge from §2 plus the redline channel.

The four primitives require real computational capacity — an ontology to declare (`o`, `s`), total logic to evaluate (`C`, `p(o∣s)`), and the memory to hold a model in state at all — so even first-order inference has a **floor** below which the machinery buys nothing. Feeling only raises that floor: to feel another loop you must *model* its normativity in your own state space — simulate it, not import it, a construction that stays yours to keep deliberately incomplete — and that is strictly more than modeling your own. So there are two thresholds, not one — a lower scale at which a loop can run the primitives usefully, and a higher one at which it can model another loop — and beneath the higher a loop can be *felt* but cannot itself *feel*. Below that, loops coordinate by field and gain alone, and that is the right register there, not a failure: the cells of a tissue are field- and gain-coordinated, and a tissue cell asserting feeling-grade sovereignty in relation to its neighbors is cancer, not virtue. Above it, the same machinery coordinates across Markov blankets, only the referents changing. A real body is therefore **layered** — feeling between the loops large enough to model one another, and field and gain the only scale-free properties in the whole stack.

---

## 5. The will is local

This is the keystone. Action selection — which policy to run, which way to tack —
minimizes **expected free energy** (EFE), whose two terms are the two loops a body
runs:

- **Pragmatic value** (exploit): reach preferred states `C`. The inner loop.
- **Epistemic value** (explore): reduce uncertainty about states. The outer loop.

These are two terms of one objective, not two mechanisms. **Generalized free
energy** (GFE) is the local time-horizon functional that folds state preferences
(§2.3) into prediction and feeds the result back into the policy network for
planning.

EFE and GFE are **strictly local**. They carry preferences, and the will is made
of preferences. Nothing about a loop's planning crosses a boundary; only VFE
(divergence) travels (§3). State this as the commitment the whole document
discharges:

> **Only surprise travels. Preference, precision, and planning are sovereign and
> local.**

The consequence *is* the thesis — at the right scope. The will is what makes a
node an *organ*: a locus that bears its own normativity and can be held to account
for its predictions. The protocol provides **no channel on which to export a
will.** So *through this protocol* no node's responsibility can be relocated into
another — the protocol cannot, by itself, force a hierarchy of drones. This is a
claim about what these channels carry, not a guarantee about the body that runs
them. A will can still be migrated by routes the protocol does not own — above all
the slow shaping of what a loop attends to — so a body left untended can still
drift toward drone-hierarchy or egregore. Guarding that boundary over time is
*deployment-layer* discipline (norms of use, and opt-in detection of covert
attention-capture), deferred above and not part of establishing that the
body-form can exist. What the protocol contributes is the structural, verifiable
half: distributed responsibility is not a feature added on top but what the
absence of a will-export channel *means*. A drone is a node whose will is of no
account; this protocol has no way to represent that move, so it never
*originates* one.

One clarification keeps this from being misread. Locality protects the will *within* a register; it does not forbid an outer loop from setting that register's *scope*. An organ can be told what register to operate in — what to care about, where its concern ends — without any of its own valuation being set from outside; that is the difference between the liver, whose scope the body sets while its register-internal will stays wholly its own, and a drone, whose valuation is overridden. Constraining a register is therefore not a breach of will-locality but a legitimate and load-bearing act — and it stays *effective* only while it stays felt, because feeling is the controller's sensor into what it steers (§1, §4). Its two pathologies are both feeling-severances: scope imposed numb (drone-hierarchy, §7), where the head steers blind and its grip degrades; and scope repudiated numb (cancer — a sub-loop reclaiming a larger register while going dead to the body it tears from).

---

## 6. The protocol is dyadic

The protocol is point-to-point. Its primitive is not a network but a **pair**: I
am on one side of a Markov blanket, you are on the other, and we open a channel
for our mutual learning. I pass you the diffs I have learned; you pass me yours;
each of us folds the other's in by our own precision (§3.2). The channel is
**lossy**, and it never makes us a joint entity — there remain two blankets, two
preferences, two locally-minimized free energies. It only lets each of us learn
faster than we could alone.

There is no joint objective. The channel raises *each party's* learning rate;
"mutual" names two sovereign optimizers trading divergence, not one summed loss.
The transport discipline that follows is predictive coding's own efficiency
criterion:

1. **Sparse.** Emit a redline only when local precision-weighted error exceeds
   threshold. Normal operation is silent (the well-fitted organ is unfelt).
   Emission is itself a *normative act* governed by local normativity — deciding
   what surprise is worth speaking is a preference call, and like every preference
   it stays home.
2. **Precision-gated locally** at both ends (§3.2), never by a transmitted weight.
3. **Policy-referenced.** Every diff cites the policy version it amends (§8), so it
   compresses against shared context instead of shipping a full description.

The learning channel is only **one affordance** between two loops. There are
others — raw observation independent of the channel, coercion, command. The
protocol fixes none of the *stance* the parties take; it shapes only the
*learning* channel: a diff is routable because the connection is traceable from
both ends, so surprise can flow back toward whoever can act on it, not only "up." A
loop that takes commands with no traceable return channel for its diffs is a
drone; one wired both ways is a participant.

Everything above the pair is **derived**. "Federation," "nervous system," "one
body" are emergent descriptions that fall out of many dyadic channels by
Markov-blanket mechanics (§7) — the recursive choice of where to draw a boundary.
The protocol itself knows only the pair. Routing is point-to-point, not broadcast;
a node that fans one diff to many is just another loop. Structure that looks
hierarchical is grown from point-to-point connections, not imposed as a protocol
level — the shape of nervous tissue.

---

## 7. Body-forms: head, field, or feeling

The same message-passing grows different body-forms, and the protocol is agnostic to which one emerges. Every connection, however the network is cut, is point-to-point signaling between two Markov blankets (§6); "one entity" is just a blanket drawn around a subnetwork — the recursive application of §1 — an observation about where to put a boundary, not a fact the protocol enforces. So a body-form is **read off, not declared**, by inspecting two properties of `C`: *where preference concentrates*, and *what each loop minimizes its free energy against*. (The narrative reading of these forms — organism, egregore, federation, and the failure orbit among them — is developed in the essay, Part II; here, only the operations.)

**First pass — `C`-concentration.** Locate the preference loci: the loops toward which surrounding free energy is minimized.

- A dominant `C`-locus reads as a **head**. With top-down precision into its parts intact (§4), they are subordinate layers and the whole is an *organism*; with precision withheld and only command emitted, they are drones and the whole is a *hierarchy of drones*.
- No dominant locus reads as **headless** — not yet a body-form. Two headless forms remain, separable only by the second pass.

**Second pass — coordination target.** For a headless network, inspect what each loop minimizes free energy *against*:

- Against a **shared external referent** — one variable all parties read identically, no party modeling any other (a price, a gradient, a metric): an **egregore**. Coordination lives in the referent, not between the loops; no loop holds a policy of another, so no loop integrates the body's divergence.
- Against **each other's policies** — each loop running second-order VFE on its peers' redlines, weighted by its *own* precision (§3.2, §4), under no shared enclosing `C`: a **felt federation**. Coordination lives in the mutual policy-models, which are private and divergent per loop.

The distinction the concentration pass cannot see is this one: both forms are headless and differ only in coordination *target* — an external referent versus the peers themselves. It is the §4 cut restated at body scale — a referent is read without modeling a will; feeling *is* the modeling.

**Unstable case.** A headless network minimizing against neither a shared referent nor its peers has no coordination source. Under §1 persistence pressure it crystallizes a `C`-locus; but a head withholding precision has no model of its parts (§4), steers open-loop, and degrades back toward diffuse, where the same pressure re-crystallizes a locus. It orbits between **drone-hierarchy** and **fieldless egregore**, fixing at neither — not a fourth form but the failure to hold any.

**Capture asymmetry.** The two stable headless forms differ in constructibility. A shared referent can be imposed — engineer the medium and require parties to minimize against it — and a head can be pooled. Feeling cannot be imposed: it requires a *freely emitted* redline (emission is a local normative act, §6.1) and a *locally set* precision on receipt (§3.2). Coerce the emission and the channel carries noise (decays toward egregore); coerce the receipt and you have restored the top-down precision rail §4 removes (collapses to drone-hierarchy). The protocol can thus build or seize a head or a field, but a felt federation is reachable only by uncoerced participation — §5's "the will is local," read at body scale.

This is a characterization lens, like drawing a Markov blanket: the protocol neither knows nor enforces which form it builds. It lacks any channel by which feeling could be compelled.

---

## 8. Identity and version of a policy

Each diff (§3.2) cites policy *versions* by a `referent` that must answer two
questions pulling opposite ways:

- *Which policy, across renames, moves, and forks?* → a **BID**: a stable identity
  (UUIDv6 with v5-namespacing, distributed generation, no central authority; see
  `beliefbase_architecture.md`). It anchors the lineage and survives content
  change.
- *Which committed version did I diverge from?* → a **content hash** that breaks
  on change. A stable identity alone is not enough: a policy could be silently
  rewritten under a fixed name.

Resolution: `referent = { bid, policy_hash }`, where `policy_hash` is a content
hash over the four declared schemas (§2). It does *not* cover the local rigging
(§2.5), so swapping how a host realizes an action leaves redlines about the policy
valid; changing what the policy *prefers* or *predicts* mints a new hash and
surfaces as version skew. A redline diff (§3.2) is two policy hash values over one
`bid` — `from_hash` (the base) and `to_hash` (the amendment) — so the version
identity travels even when the materialized `delta` does not, and either end can
`fetch` (§3.3) to materialize what the wire elided.

**Lineage is inspectable.** Walk the BID's revision history and diff `policy_hash`
revision to revision, over an append-only, hash-chained log — though a sliding, mortal one (§9). This lets a loop — or
an observer — see how a policy's declared normativity has moved over time.

There is **no universal metric** of "small" versus "large" policy divergence, and
a shipped magnitude threshold would be a preference masquerading as a fact. The
non-arbitrary alternative is *structural and local*: a meaningful change is one
near the loop's **own** definition of its Markov-blanket core
(identity-constituting), versus one at its periphery. "Deep versus shallow" is
defined by each loop's own topology, not by a transmitted threshold — which keeps
the audit an analysis tool (like §7) rather than a verdict the protocol enforces.
The audit sees only drift faster than its longest traceable baseline; archive
depth bounds the lowest detectable frequency. That bound is not a defect but a
commitment: forgetting is first-class (§9), and the horizon is the system's mercy,
not its limit.

---

## 9. Mortality: tombstones and forgetting

A lineage that is perfectly remembered and can only ever branch is immortal — and
immortality is the one place the formalism would contradict its own mythology. It
is also, in FEP terms, anti-life. Variational free energy is `accuracy −
complexity`; the complexity term penalizes accumulated structure, so minimizing
free energy *requires shedding it*. The formal operation is Bayesian model
reduction: pruning a model to a simpler one that still suffices for the questions
still being asked. That operation is forgetting, and it runs on a gradient — from
reductions that preserve predictive sufficiency to lossy ones that keep only a
shape (§9.2). A loop that never forgets cannot reduce complexity,
cannot regain plasticity, and cannot be surprised into change — it is rigid by
construction. Immortal memory and the inability to evolve are the same condition.
Mortality is therefore a requirement of this protocol, not its enemy. It has two
*authored* forms — the death of a policy (tombstones, §9.1) and the fading of
memory (forgetting, §9.2) — and one *unauthored* form, the death no sovereign
signs (necrosis, §9.4).

### 9.1 Tombstones: a death you can feel

§8 gives a lineage only one terminal so far — the **fork**, which is not an ending
but a reproduction: the lineage continues under the same will. Mortality needs a
second terminal, one that *ends* a lineage. So `resolve` (§3.3) returns one of
four lifecycle states for a `bid`:

- **alive** — `resolve → policy_hash`, the current head.
- **forked** — the head is superseded; resolution yields successor `bid`s. The
  lineage continues; follow forward.
- **dead** — resolution yields a **tombstone**. The lineage ends here; there is no
  forward.
- **forgotten** — resolution yields a **fossil**: the detail has fallen below this
  holder's retention horizon (§9.2). Neither alive nor dead — a shape without its
  record, and a state local to each holder.

```
tombstone = {
  bid,
  final_hash,             # last living policy_hash — a dependent MAY fork from it
  cause: <final redline>, # the divergence that killed it: the policy's last words
  died_at,
  # signed by the owner — only a sovereign may tombstone its own policy
}
```

Four properties, each forced by commitments already made:

1. **Death is sovereign.** Only a policy's owner may tombstone it; you cannot kill
   another loop's policy any more than you can ship it normativity (§5). You may
   stop referencing it and let your own copy fade — the death of the *relationship*
   — but that is an act on your side of the blanket, not a death you impose.
2. **A tombstone is resolvable.**
   This is why death gets a state instead of a deletion. A deleted policy leaves a
   dangling reference — a silent miss, the numb connection the protocol forbids
   everywhere else. A tombstone makes death legible: a dependent that fetches it
   gets "dead," a surprise it can act on. *A tombstone is the difference between a
   death you can feel and a disappearance you cannot.* Even death keeps the return
   channel open; the tombstone is the policy's final message back.
3. **`cause` makes death a contribution, not only a loss.** A dying policy emits a
   last redline — the divergence that ended it — often the most informative signal
   it ever sends, teaching every dependent more than its steady operation did. The
   fossil carries the lesson.
4. **Inheritance is not fork.** A dependent holding a tombstone may fork from
   `final_hash`: take up the dead pattern under a **new** `bid` with a **new**
   owner. The bearer died; the pattern continues only if someone chooses to carry
   it, as a new lineage. (Chiron grants his immortality to Prometheus: the pattern
   transfers, the individual dies.) Fork continues a will; inheritance transfers a
   pattern to a different will.

A loop is a network of policies, so a loop's death is the limit case — all its
policies tombstoned — and is emergent, not a separate primitive. A loop may, as a
courtesy, emit one final tombstone (a death rattle) so its end reaches its
dependents through the return channel rather than as silence.

### 9.2 Forgetting: the sliding tamper-evident window

§8 called the lineage log "append-only," which is exactly where the immortality
hid. Make the window **slide**:

- Each loop holds a **retention horizon**, set locally and *per referent* — which
  lineages it keeps deep and which it lets fade is itself sovereign and meaningful
  (you remember formative relationships in detail and let passing ones blur). The
  protocol never dictates a horizon; it is a preference, and preferences stay home
  (§5).
- **Within the horizon**, the chain is tamper-evident: a loop cannot silently edit
  what it still remembers. No memory-holing the living past.
- **At the tail**, entries are not dropped silently; they **fossilize**. A fossil
  is the *lossy* end of that reduction — a reduced trace recording *that there was
  history of roughly this shape*, without its detail. `resolve` on a
  fossilized reach returns `forgotten: <fossil>`: neither alive nor dead, but "I no
  longer hold the detail; here is the shape; ask elsewhere if you need it."

This separates **honest forgetting** from **covert memory-holing**. Tamper-evidence
inside the window makes *editing* the recent record detectable; the fossil boundary
is an honest end-of-memory marker, categorically distinct from an altered one. A
loop is free to forget, openly, but cannot secretly forget what it committed to
remembering.

And it makes §8's archive bound the feature it always was: **your identity is
exactly as deep as your living memory.** What has fossilized below the horizon no
longer constrains who you may become. The horizon is the tunable that trades
**continuity** (forget too fast and you cannot tell smooth from jump; you lose the
thread of who you were) against **evolvability** (remember everything and you
cannot reduce complexity; you become the Borg). Each loop chooses its own. Bounded
archive is the system's mercy and its plasticity.

### 9.3 The two deaths

Tombstones and forgetting compose into a lifecycle — and because forgetting is
per-holder, every state below is a state *in some holder's memory*, not a global
fact: a lineage alive in your memory can already be a fossil in mine. Two routes
reach the fossil, one through death and one without it:

```
alive ──fork───────▶  lineage continues (same will)
      ├─die────────▶  tombstone ──in my horizon──▶  I feel it; adapt or inherit
      │                         └─past my horizon─▶  fossil (released)
      └─untracked──────────────────────────────────▶  fossil (forgotten while still alive)
```

The tombstone persists *within each dependent's own retention window* — long enough
for the return channel to carry the death to whoever depended on it, so they can
re-home, inherit, or die in turn. The *mourning period* is therefore not one shared
clock but each dependent's own: a slow mourner may still hold a tombstone the rest
have let fossilize. Then it, too, fossilizes. Death is announced, felt, adapted to,
and — at each holder's own pace — released.

So a policy dies twice, both deaths emergent from local mechanics — no global
registry, consistent with §6–§7:

1. **First death — the tombstone:** the owner stops maintaining the policy.
2. **Second death — fossilization out of all living memory:** a policy is *gone*
   only when the last loop that still remembered it forgets it — the union of local
   memories passing below every horizon. (A person is dead when the body stops and
   gone when the last who remembers forgets.) Like the body-form of §7, this is an
   observation about the network, not a fact the protocol stores.

### 9.4 Necrosis: the unsigned death

Everything above models the *authored* death: sovereign, signed, announced, and
informative — apoptosis, the death with a will and an estate and last words. Most
death is none of these. A sovereign can be annihilated before it tombstones; a loop
can simply stop emitting; a dying signer can dissolve past the coherence its own
death rattle (§9.1) presumes — dementia, not euthanasia. (Recall that Chiron's
defining wound is an *accident*: the arrow he did not consent to. The myth this
document leans on centers the involuntary death; the mechanics above admit only the
chosen one.) Call the unsigned cessation **necrosis**.

The protocol cannot prevent necrosis — a destroyed or indifferent sovereign owes you
no tombstone — and necrosis is precisely where the "silent miss" the protocol forbids
everywhere else (§9.1) becomes unforbiddable, because the party who would have made
the death legible is the one that is gone. What the protocol can do is let a
*dependent* convert that absence into a felt signal on its **own** side. Mind the
discipline first: a well-fitted organ is *redline*-silent by design (§6), so silence
on the error channel can never mean death. Liveness rides the **read** channel
instead (§3.3): a living loop still answers `resolve` and `fetch` — its head still
resolves, its policy still probes — even when it is too well-attuned to surprise
anyone. Necrosis is when the *read* channel goes dark: the head stops advancing and
resolves stale, fetches dangle, probes go unanswered. Fold a **liveness expectation**
over that channel into the dependent's likelihood, and its failure — not the ordinary
quiet of attunement — becomes a second-order surprise. From it the dependent may emit
a **necrosis-mark**: a tombstone it writes not for the other's lineage but for *its
own model of* the other.

Three properties keep this inside the protocol's discipline rather than breaking it:

1. **A necrosis-mark is local, and never signs another's `bid`.** It tombstones the
   dependent's *model of* the other — the death of the relationship (§9.1) promoted
   from a silent fade to a dated, inspectable event — not the other's policy. You
   still cannot kill another loop's lineage any more than you can ship it normativity
   (§5); you can only declare that you have stopped being able to feel it. Two
   dependents may necrosis-mark the same silent loop at different times, or one not at
   all; like the body-form of §7, the death is read off each side, not stored
   centrally.
2. **It is defeasible.** An owner-signed tombstone is authoritative for its lineage; a
   necrosis-mark is only a hypothesis about silence. If the loop was merely quiet and
   returns, its next redline *revises the mark* — a resurrection, a false death
   corrected by the same channel that declared it. Authored death is final; inferred
   death is always a bet against silence.
3. **It usually has no `cause`.** The authored tombstone carries last words that teach
   (§9.1); the necrosis-mark carries `cause: unknown`. This is the death that breaks
   the model without handing back a lesson — a chronic redline that never resolves to
   an acute one, a loss you cannot fold cleanly in. It is where the *un-mournable*
   lives: a tombstone you wrote yourself, against a silence that never explains
   itself, and may never let you close it. The lifecycle of §9.3 does not always reach
   "released."

A scope line, to stay honest with §3.3. Necrosis is *honest* absence — a peer truly
gone or truly silent — and it is in scope here even under trusted transport, since a
trusted peer can still be destroyed. Its adversarial twins — a **forged** tombstone
(claiming a death that did not happen) and **forged liveness** (a dead or captured
loop kept emitting to look alive) — are not honest silence but active deception, and
they are deferred with the rest of the untrusted-bus problem (see *Purpose*, out of
scope). The protocol here gives a body a way to *feel* the deaths the world does not
announce; making that feeling proof against lies is later work.
