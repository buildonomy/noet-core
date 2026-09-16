---
version = "0.1"
title = "Issue 112: Credentials — Role-Annotated Identity and Promotion Gating"
---

# Issue 112: Credentials — Role-Annotated Identity and Promotion Gating

**Priority**: LOW — needed when a store first gates on *who* rather than *what*
**Estimated Effort**: 3 days (RELATIVE COMPARISON ONLY)
**Dependencies**: **Follows Issue 104** — a credential is a `record_kind` and its
registry entry, so 104's enumeration and transition machinery must exist first.
**Requires Issue 105** for the actor node a credential attaches to, the promotion
path it gates, and the store halo it is read from.
**Requires Issue 103 Part E** (set-valued edge ownership) — without it, the
second attester of any role grant silently overwrites the first, which makes the
web-of-trust model below unimplementable.
**Consumed by Issue 65** (the first store that gates admission) and **Issue 17**
(role conduits: `{exit} n-of` over a queryset resolving a role).

---

## Summary

An actor is a graph node with a derived identity (Issue 105 step 4). A
**credential** annotates that node with a role — "Alice is a structures
engineer" — and a **policy** states which roles a given act requires. Together
they are how a store decides that a promoted record is admissible, and how a
role conduit resolves to a set of actors who may discharge it.

This is identity *annotation*, not authentication. Who someone is comes from the
channel's `ActorId` (`annotation_channel.md`); this issue is about what the
corpus asserts of them, and it asserts it in records like everything else.

## Why it is its own issue

The credential model was drafted inside Issue 65 as part of an attestation
server. It does not belong there: Issue 65 is one store
(`ISSUE_65_ATTESTATION_SERVER.md`), while credentials are consumed by at least
three things that are not that store.

| Consumer | What it needs |
|---|---|
| **Issue 65** — a promotion-accepting store | "may this actor's record cross into this store?" |
| **Issue 17** — role conduits | resolve a role to the actors who may discharge it, so `n-of` has a queryset |
| **Any policy-bearing node** | `sign_off_policy` in frontmatter declares required roles |

**A fourth consumer, and it reopens a design decision.**
`collector_model.md` §4 argues promotion should be automatic and derived, partly
on the grounds that actor-requested push would need each collector to hold a list
of who may push. This issue removes that objection: a credential-based push rule
holds no list and needs no administrator — the credential is peer-attested, lives
in the graph as an edge, and a rule reads it like any other predicate. So
**role-authorized push becomes a viable alternative to automatic promotion once
this lands**, which is worth knowing before either is built. The W1 pilot
(planning Issue 29) is where the choice gets evidence.

A model serving three consumers, one of which is LOW priority and optional,
should not live inside that one.

## Architecture

### A credential is a record, and a role is a Section edge

Nothing new is required at the storage layer. A credential attestation is an
ordinary record with its own `record_kind`, and the fold projects it as an edge:

```
(source: actor, sink: role node, WEIGHT_OWNED_BY: {attesters})
```

Three properties follow from that shape rather than from new machinery:

- **The attesters own the edge.** "Who vouched for this" is on the edge, not in a
  side table. The owner set is **plural** — several peers attesting one grant is
  the normal case, and the single-valued field this was first drafted against
  cannot express it (Issue 103 Part E).
- **Revocation is a new record**, not an edit. The log keeps the sequence; the
  fold decides what the projection shows; whether a promoted summary carries the
  revoked grant is promotion configuration (Issue 105 §Promotion reads the fold).
- **The edge exists while any live attestation supports it.** Its owner set is a
  projection of the record log, so the multiplicity that matters — how many peers
  vouched, and which — is answered from the log and cached on the edge.

### The role structure is a Section-kinded DAG

Roles compose. `safety-reviewer` may sit under `reviewer`, and an actor holding
the specific role holds the general one. That is a partial order over role
nodes, and it is expressed with the edges the corpus already has:

```
actor --Section--> role --Section--> super-role
```

**Section, not Pragmatic, and the reason is empirical.** `PathMapMap::new`
constructs every `PathMap` with `WeightKind::Section` hardcoded, and `PathMap`
early-returns on any edge whose weight set lacks its own kind. There is no
Pragmatic `PathMap` and nothing builds one. Choosing Pragmatic would mean
building a second index to get what Section gives free.

What Section gives:

- **`bid_map` is `Bid → Vec<usize>`** — plural positions per node. A role reachable
  by two super-roles materializes as two paths to one node, which is the diamond
  a role hierarchy needs.
- **Cycles are already handled**: `PathMap::new_indexed` records DFS back-edges in
  `loops` and excludes them from path generation, so a mis-authored role cycle
  degrades rather than hangs.
- **"Everyone holding role R" is `submap(R)`** — existing machinery, no scan.

> **This is a poset, not a lattice.** A DAG gives a partial order; meets and
> joins are not guaranteed to exist. Denning-style lattice models require them,
> and nothing here does — the load-bearing properties are the order and its
> reachability closure. Do not promise lattice completeness.

### Only actors are actionable, and that is declared

A role is an **authorization surface**; an actor is an **authentication surface**.
A policy naming `reviewer` resolves through the structure to actor nodes, and an
attempt to discharge an `{exit}` with a bare role node is a diagnostic.

**Actionability is declared, never derived from position.** A role with no
holders yet is structurally a leaf, so inferring "leaves are actionable" would
make exactly the wrong node signable. The discriminator is
`BeliefNode.schema` — `schema = "actor"` is an authentication surface,
`schema = "role"` never is, whatever its indegree. This deliberately avoids a new
`BeliefKind`: that enum is `repr = "u32"` and load-bearing in shard packing,
where `schema` is an existing free-form field.

### Both live in the `Actor` namespace

Roles and actors share Issue 105's `Actor` namespace rather than taking one of
their own. The namespace is the agent/authorization boundary — everything inside
it is either something that can act or something that says what may be acted —
and keeping the two together means one reserved constant, one system network, and
a membership test that answers "is this an authorization object?" in one
comparison.

Role identity derives from the role string, the same way an actor's derives from
their email, so a policy naming `safety-reviewer` resolves without a lookup table
and two corpora naming the same role agree on the node.

### An actor node's payload must be authentication-bearing, not corpus-authored

An actor node holds content — display name, the addresses that resolve to it,
eventually a public key. **That content cannot be ordinary corpus content**, and
the existing mechanism for the closest analogue makes the reason concrete.

`living_corpus.md` §5 and Issue 105's open questions both route actor aliasing —
one person holding several addresses — to `url_aliases` / `alias-template`
(`codecs/network_authoring.md` §8). Those are **document frontmatter**, editable
by anyone with commit access. Applied to an actor node, that is an identity-merge
primitive: writing

```toml
url_aliases = ["mailto:alice@example.com"]
```

onto a node makes Alice's address resolve to it. Every record attributed through
that address now attaches to a node an attacker controls, and every policy
counting Alice's role counts them. The alias mechanism is correct for tickets and
wrong for identities, and the difference is that a ticket URL is a *name* while
an `ActorId` is a *claim about who acted*.

**`Envelope` carries no signature** (`beliefbase_architecture.md` §4.3: `id`,
`actor`, `observed_at`, `caused_by`, `payload`). So there is currently nothing to
break — the tamper is undetectable rather than merely unauthorized. This is a
gap in the fabric, not an argument against the model: `attestation_fabric.md`
§3.4 already names W3C Verifiable Credentials and DIDs as the intended
substitution, and §10 recommends adopting rather than building the crypto.

The constraint this issue must hold, whatever the eventual mechanism:

- **An actor's identity-bearing fields are derived from, or signed by, the
  authentication mechanism** — never assertable by editing a document. A
  self-asserted display name is harmless; a self-asserted *address* is identity
  forgery.
- **Aliasing an actor is an attested act, not a frontmatter field.** One actor
  holding two addresses is two derived nodes plus a signed record binding them —
  the same shape as a credential, with the subject vouching rather than a peer.
  The `url_aliases` route must be explicitly closed for `schema = "actor"` nodes,
  or it is a standing privilege escalation.
- **Tamper must be detectable offline.** A store that merges by set union cannot
  gate on a central authority, so integrity has to travel with the record. This
  is the same property Issue 108's `verify` gives evidence, applied to identity.

**The likely mechanism is a signature on `RunEnd`** covering an enumeration of
the run's members (Issue 105 §If a record is ever signed). That inverts this
problem rather than patching it: an actor node whose identity-bearing fields are
projected only from signed records has no corpus-authored input, so a
frontmatter edit claiming someone's address changes nothing. The actor node
becomes a derived artifact, and `url_aliases` on it is inert rather than
dangerous.

What this issue then owns is the **binding**: a signature proves a key signed
something, not whose key it is. A credential record attesting "this key is
Alice's" is the same peer-attested shape as a role grant, with the same
revocation path.

> **This is the one place the "credentials are not authentication" boundary
> bends.** Everywhere else this issue annotates an actor whose identity came from
> the channel. Here the corpus holds a payload *about* that identity, so the
> payload inherits the authentication mechanism's integrity requirements or it
> undermines them. Do not resolve this by adding a signature field speculatively
> — settle the credential format first (VC or otherwise); the field follows.

### Peer-derived, not administrator-assigned

Credentials are claims one authenticated actor makes about another. There is no
role administrator: any actor may attest that any other holds a credential, and
the weight of the attestation derives from the web of attesters. This is PGP's
web-of-trust applied to professional roles, and it is the only model consistent
with a store whose merge is set union — an administrator implies a master, and a
G-Set has none.

**Sybil resistance is organizational, not technical.** Nothing prevents one actor
from attesting credentials for all their colleagues. What the model guarantees is
that every grant names its attester, so the web is auditable. A capability layer
that gates *who may attest a given role* is a later addition and needs no schema
change.

### Policy: what an act requires

A policy declares required roles and counts, and names no individuals:

```toml
[sign_off_policy]
required = [
    { credential = "structures-engineer", count = 1 },
    { credential = "safety-reviewer",     count = 1 },
]
```

**This is Issue 17's `n-of` predicate over a role queryset, not a second
evaluator.** `living_corpus.md` §5 establishes that a conduit may name a role and
that "2 of 3 satisfied" is `{exit} n-of :count: 2` over the queryset resolving
it. A `sign_off_policy` is that same construction authored in frontmatter
instead of a directive. Do not build a parallel policy engine; if the two
diverge, the divergence is the finding.

### Where policy evaluation happens

Two distinct moments, and conflating them is the trap:

| Moment | Question | Answer |
|---|---|---|
| **Fold** | is the policy satisfied? | a predicate over projected state — free, a `derive` byproduct |
| **Promotion** | may this record cross? | the same predicate, consulted by a push rule |

Neither is an admission check on *storage*. A record whose credential does not
satisfy a policy is still written and still readable — **gate movement, not
storage** (Issue 105 §Promotion). An unsatisfied policy is a diagnostic, because
it can only be judged after a merge the writer could not see.

## Implementation Steps

1. **Register the credential `record_kind`** (0.5 days)
   - [ ] Payload: `subject` (actor), `role`, optional note. The attester is the
         record's `actor`; `observed_at` is the grant time — do not duplicate
         either into the payload
   - [ ] Revocation as a `caused_by`-citing record; transition table per Issue 104
   - [ ] Roles are free-form strings resolved to nodes in the **`Actor`**
         namespace alongside actors — no separate role namespace
         (`identity_derivation.md` §5.1). Coordinate the constant with Issue 105
         and Issue 108's `Record`

2. **Project credential → Section edge** (0.5 days)
   - [ ] `(source: actor, sink: role, WEIGHT_OWNED_BY: {attesters})`, **Section**
         kinded so `PathMap` indexes it
   - [ ] Mark role nodes `schema = "role"` and actor nodes `schema = "actor"`;
         a bare role is never an authentication surface
   - [ ] Test: revoking a grant changes the projection without mutating a record
   - [ ] Test: "everyone holding role R" is one `submap` call
   - [ ] Test: a role under a super-role resolves holders of the specific role as
         holders of the general one, and **not** the reverse
   - [ ] Test: an `{exit}` naming a bare role node is a diagnostic, not a
         discharge

3. **Policy as a predicate** (1 day)
   - [ ] Parse `sign_off_policy` from frontmatter into the same form Issue 17's
         `{exit} n-of` produces — one representation, two authoring surfaces
   - [ ] Evaluate at fold time; expose satisfaction as derived state
   - [ ] Test: a policy authored in frontmatter and the equivalent `{exit}`
         directive yield identical results on the same records

4. **Promotion gating** (1 day)
   - [ ] A push rule may name a policy predicate (Issue 105 §Promotion)
   - [ ] Test: a record failing the policy is stored, readable, and **not**
         promoted — all three
   - [ ] Test: the failure surfaces via `check_consistency`, not as a refused
         write

## Testing Requirements

- A credential grant, a revocation, and a re-grant fold to the correct final
  state with no record mutated
- Two attesters granting the same role to one actor produce **one edge with two
  owners** (Issue 103 Part E), and the policy counts the actor once
- A role cycle authored by mistake is reported, not hung on — `PathMap`'s
  back-edge detection already covers this, so the test guards the integration
- A policy naming a role nobody holds evaluates cleanly to unsatisfied, not to
  an error

## Success Criteria

- [ ] A credential is an ordinary record; no bespoke credential store exists
- [ ] "Everyone holding role R" is a graph traversal, including holders of roles
      beneath R
- [ ] Every attester of a grant is recoverable from the edge; none is overwritten
- [ ] `sign_off_policy` and `{exit} n-of` evaluate through one code path
- [ ] A failing policy blocks promotion and never blocks a write

## Risks

- **A parallel policy engine grows inside a store implementation** →
  **Mitigation**: step 3's equivalence test is the guard; it fails loudly if the
  two surfaces diverge.
- **Credentials drift toward authentication** → **Mitigation**: identity comes
  from the channel's `ActorId`. This issue annotates an actor node; it never
  establishes who someone is.
- **An actor node is aliased by a document edit**, merging identities without a
  broken signature → **Mitigation**: close `url_aliases` for `schema = "actor"`
  nodes before any store gates on actor identity. This is a correctness gate on
  Issue 65, not a hardening task.

## Open Questions

- **Two authoring surfaces write the same Section edge.** A role hierarchy
  authored as a corpus document (a static org chart) and a credential attested at
  runtime both project into one structure, with different owners. An authored
  role edge is corpus content that a reparse may withdraw; an attested one is a
  fold projection. The union semantics are Issue 103 Part E's, but which surface
  wins when they disagree is unsettled — and it decides whether an actor's home
  network is the `Actor` system network or the document that declared them.
- **What signs an actor's identity payload?** The constraint above says it must
  be signed or derived; it does not say by what. `attestation_fabric.md` §3.4
  points at W3C VC and DIDs, and §10 recommends adopting a library rather than
  building one. Settle the format before adding any field to `Envelope` — a
  speculative `signature: Option<Vec<u8>>` is the liability
  `LESSONS_LEARNED.md` warns about. The *scope* is settled ahead of the format:
  `RunEnd`, over an enumeration (Issue 105).
- **Does key rotation invalidate history?** A rotated or revoked key leaves
  previously signed runs verifiable only against the old key, so the graph must
  retain superseded keys as revoked-but-readable rather than removing them —
  the same shape as credential revocation, which is a new record rather than an
  edit. Confirm the two use one mechanism.
- **Can a credential be scoped to a corpus?** "Safety reviewer for this program"
  differs from "safety reviewer". Probably payload, possibly a distinct role
  node; needs a real policy to decide against.
- **Does the attester need a credential to attest?** The `attester_credential`
  meta-policy is expressible and unbuilt. Defer until an organization asks.

## References

- `ISSUE_103_LIVING_CORPUS_INFRASTRUCTURE.md` Part E — set-valued edge
  ownership; without it every attester after the first is overwritten
- `ISSUE_104_ANNOTATION_VOCABULARY.md` — the `record_kind` registry this extends
- `ISSUE_105_RECORD_STORE_AND_FOLD.md` — actor nodes (step 4), the fold,
  §Promotion reads the fold
- `ISSUE_17_NOET_PROCEDURES_EXTRACTION.md` — `n-of` over a queryset; the agential
  tense of declared-set-versus-discharged-set
- `ISSUE_65_ATTESTATION_SERVER.md` — the first store to gate on a policy
- `docs/design/annotation/living_corpus.md` §5 — role conduits, the actor edge
- `docs/design/annotation/attestation_fabric.md` §7 — identity and credentials;
  §4.3a policy-required provenance
- `docs/design/annotation/collector_model.md` §4 — push rules and admission
- `src/paths/pathmap.rs` — `PathMapMap::new` (Section hardcoded), `PathMap`'s
  `bid_map` and `loops`, `submap`
- `src/properties.rs` — `BeliefNode.schema`, the actor/role discriminator
