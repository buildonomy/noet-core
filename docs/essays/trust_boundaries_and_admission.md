---
title: "Trust Boundaries and Admission: Why Integrity Lives at the Edge"
id: wp-trust-boundaries
version: 0.1
---

## Abstract

A store that accepts everything cannot make claims about what it holds. A store
that validates on the way in can. This essay argues that **integrity is a
property of admission, not of storage**, and that the mechanism has been
independently rediscovered often enough — in commercial security models, in
information-flow control, in supply-chain attestation, and in decentralized
publishing — that it should be treated as a known shape rather than invented
again.

The practical consequence for a knowledge system: **replication and validation
are different problems and want different architectures.** Conflating them
produces systems that are simultaneously too strict (rejecting merges that
should converge) and too permissive (admitting claims nobody checked).

## 1. The problem

Consider any system where records accumulate from multiple sources and later
support consequential decisions — a review log, an audit trail, a body of
evidence backing a release.

Two requirements pull in opposite directions:

**Records must merge without conflict.** Two people annotating the same document
must not lose each other's work. The standard answer is to make records immutable
and uniquely identified, so merge is set union and convergence is guaranteed by
construction rather than by policy.

**Records must be trustworthy.** Not every claim belongs in every context. A
release corpus should not silently absorb an unreviewed assertion just because
someone wrote one.

Stated together they look contradictory. Union semantics say *never reject*;
integrity says *sometimes reject*. Systems that notice only the first become
sophisticated garbage collectors. Systems that notice only the second lose the
convergence that made distribution tractable.

## 2. The resolution: a store is not a boundary

The contradiction dissolves once two things are distinguished:

- A **store** holds records. It never rejects a record it holds, and merging two
  stores is union. This is where convergence lives.
- A **boundary** is where a record moves from one store to another. It may
  decline. This is where integrity lives.

Declining is not deletion. A record refused at a boundary remains authoritative
in its origin store; what failed was a *copy*, not the record. So the origin
loses nothing, and the destination gains a property it could not otherwise have:
**everything inside it passed a check.**

The check must be **locally computable** — decidable from the arriving record
plus the receiving store's own state. A boundary that must consult an outside
authority to decide has not moved the trust problem, only relocated it.

## 3. This is Clark-Wilson

The strongest antecedent is the **Clark-Wilson integrity model** (1987), proposed
as the commercial counterpart to military confidentiality models. Its structure
maps almost directly:

| Clark-Wilson | Here |
|---|---|
| Constrained Data Item (CDI) | a record inside a validated store |
| Unconstrained Data Item (UDI) | a record arriving at a boundary |
| Integrity Verification Procedure (IVP) | the check that a store is in a valid state |
| Transformation Procedure (TP) | the admission step — the *only* way a UDI becomes a CDI |
| Certification vs. enforcement rules | policy definition vs. runtime admission |

Clark-Wilson's central rule is that **a UDI may enter the constrained set only by
passing through a transformation procedure**, and that all TPs are certified in
advance. Uncontrolled data does not become trusted data by being copied; it
becomes trusted by being *checked at a defined point*.

Its second contribution is **separation of duty** — the agent who certifies a
procedure may not be the agent who executes it. In a records system that becomes:
the party configuring an admission policy is not the party pushing records
through it. Concentrating both is the failure mode the model exists to prevent.

Clark-Wilson was written for transaction processing. What has changed is not the
model but the substrate: the "constrained data items" are now claims about a
corpus rather than rows in a ledger.

## 4. Integrity levels form a lattice

**Biba's integrity model** (1977) supplies the second piece: integrity levels,
ordered, with information permitted to flow *up* only through validation.

The practical read is that boundaries compose into tiers. A personal working set
is low-integrity — its owner may rewrite it freely. A shared collection is
higher — entry requires passing a check. A release set is higher still. Each tier
admits from the tier below, and never the reverse.

This is not primarily an access-control statement. It is a **claim about what a
tier means**: "everything here passed the checks required to get here" is only
true if there is exactly one way in.

Two properties worth stating because they are easy to lose:

- **The tiers form a DAG, not a mesh.** Each boundary knows only the sources it
  is configured to accept from. Fan-out is bounded by configuration rather than
  by network size — which is what keeps a validating topology from degenerating
  into every node talking to every other.
- **Promotion is one-way.** Demotion would mean removing a record from a
  higher tier, which the append-only property forbids. A record that should not
  have been promoted is corrected by a *later record*, not by deletion.

## 5. Relays: the same shape, arrived at from distribution

The Nostr protocol reaches a structurally similar place from an unrelated
starting point. Its argument, roughly: peer-to-peer networks develop
high-availability superpeers, federated networks consolidate into oligopolies,
and both outcomes look like centralization arrived at expensively. So begin
where the evolution ends — with **dumb relays that store and serve but do not
interpret**, and clients that own their keys and publish to several.

Three properties transfer:

- **Relays do not talk to relays.** This is what avoids the N² connection growth
  that pushes other topologies toward consolidation.
- **The client's copy is authoritative.** A relay refusing a post is not data
  loss; publish elsewhere.
- **Relays refuse routinely** — allowlists, payment, proof-of-work — without
  breaking the protocol, precisely because refusal happens at ingress rather
  than inside the store.

That last point is the one usually missed. A protocol built on append-only union
semantics can *still* have selective admission, and Nostr demonstrates it at
scale. Refusal-at-ingress and convergence-in-storage are not in tension; they
operate on different objects.

## 6. Supply chain: the same shape, arrived at from provenance

**in-toto** defines a supply chain as a layout of steps with expected inputs,
outputs, and authorized functionaries; each step emits signed link metadata, and
verification checks the chain against the layout. **SLSA** grades build systems
by the strength of such guarantees.

The transferable insight is not the cryptography — it is that **verification is
compositional**. If each boundary is checked locally, the trustworthiness of a
long chain follows from the trustworthiness of each link, and no global authority
need see the whole thing. A gap anywhere is a *locatable* gap.

in-toto also supplies a cautionary note it acknowledges itself: adoption
struggled because it required every step to participate at once. A boundary model
that can be adopted **one boundary at a time**, delivering value at the first
one, has a materially better chance than one that pays off only when complete.

## 7. Append-only plus monitors: Certificate Transparency

**Certificate Transparency** contributes the operational pattern for a log nobody
fully trusts: the log is append-only and cryptographically verifiable, and
separate *monitors* and *auditors* watch it for entries that should not be there.

The design lesson is that **detection can substitute for prevention** where
prevention is too expensive. Not every boundary needs to block; some can admit
and flag. A system that only blocks tends to be configured permissively so work
can proceed, whereas a system that admits-and-reports keeps the record and makes
the anomaly visible.

## 8. What a boundary check can actually check

Independent of substrate, the useful checks fall into a small number of kinds:

| Kind | Question |
|---|---|
| **Structural** | Is it well-formed? Schema, required fields, version compatibility. |
| **Invariant** | Does it satisfy declared bounds? Ranges, monotonicity, units. |
| **Consistency** | Is it compatible with the receiving store's current state? |
| **Correlation** | Do independent sources agree, within tolerance? |
| **Authenticity** | Is it from who it claims, unmodified? Signatures, hashes. |
| **Reproducibility** | Does re-running the claimed derivation reproduce the result? |

Two observations about this list.

First, **only authenticity requires cryptography.** A boundary between two stores
under one operator's control can do structural, invariant, consistency, and
correlation checks with no keys at all. This matters for adoption: the
local-only case is genuinely simple, and signatures become necessary only when
the boundary spans a trust domain.

Second, **the required kinds follow from what the information is *for*.** A
configuration value wants structural and invariant checks. A command wants
authority and timing checks. An observation wants bounds and correlation. A
verification claim wants provenance traversal — does the cited chain resolve to
checked roots? Deriving required checks from declared intent is what keeps a
policy registry from becoming an unbounded configuration surface.

## 9. Where this leaves a knowledge system

For a system that compiles a corpus and accumulates claims about it:

- **Records converge; boundaries decide.** Keep union semantics inside a store so
  concurrent work cannot be lost, and put every integrity decision at ingress.
- **The lowest tier is single-writer.** An actor's own working set needs no
  admission control, because there is only one writer. This is also what makes
  wholesale replacement legitimate there — a compiler regenerating its
  observations is rewriting its own store, not violating anyone's append-only
  guarantee.
- **Promotion is derived, not commanded.** If a record crosses a boundary because
  it has reached a state, rather than because someone issued a request, then
  propagation is a consequence of annotation rather than an operation requiring
  its own authorization model.
- **Adopt one boundary at a time.** The first boundary that catches something is
  the argument for the second.

## 10. What is genuinely unresolved

Honesty about the edges of the borrowed material:

- **What crosses with a summary.** If a boundary admits a folded conclusion but
  not the records it summarizes, the receiving tier cannot re-verify — it is
  trusting the sender's fold, which is precisely what local computability was
  supposed to avoid. Sending the evidence restores verifiability at the cost of
  volume and of whatever privacy the summarization provided. This is a real
  trade, and it is likely to be *per relation* rather than global.
- **Unreachable dependencies.** A check requiring data from a store that is
  currently unavailable must fail closed, fail open, or defer. Clark-Wilson
  assumes availability; distributed systems cannot.
- **Configuration is the new attack surface.** Moving policy out of code and into
  configuration makes it inspectable and changeable — and makes a misconfigured
  predicate a silent integrity failure. Whatever governs code changes should
  govern these.

## References

- Clark, D. and Wilson, D. *A Comparison of Commercial and Military Computer
  Security Policies*, IEEE Symposium on Security and Privacy, 1987 — CDI/UDI,
  IVPs, TPs, separation of duty.
- Biba, K. *Integrity Considerations for Secure Computer Systems*, 1977 —
  integrity levels and the lattice of permitted flows.
- Myers, A. and Liskov, B. *Decentralized Information Flow Control*, 1997 —
  flow policy without a central authority.
- Laurie, B. et al. *Certificate Transparency* (RFC 6962) — append-only logs with
  independent monitors; detection rather than prevention.
- in-toto: *Providing farm-to-table guarantees for bits and bytes*, USENIX
  Security 2019 — layouts, link metadata, compositional verification.
- SLSA (Supply-chain Levels for Software Artifacts) — graded provenance
  guarantees.
- Brander, G. *Nature's many attempts to evolve a Nostr*, 2024 — the argument
  that relay topology is where distributed architectures converge.
