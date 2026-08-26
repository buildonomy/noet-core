---
title = "Attestation Fabric: A General Infrastructure for Cross-Domain Provenance and Annotation"
authors = "Andrew Lyjak, Claude"
last_updated = "2025-07-14"
status = "Draft"
version = "0.1"
dependencies = ["collaboration_overlay.md (v0.1)", "federated_belief_network.md (v0.1)"]
---

# Attestation Fabric: A General Infrastructure for Cross-Domain Provenance and Annotation

> **Draft** — This document generalizes the `noet-collab` collaboration overlay
> into a domain-neutral attestation fabric. Phase 1 of `noet-collab`
> (see `collaboration_overlay.md` and `ISSUE_65`) remains unchanged. This design
> describes the extension path that Phase 1 is architected to support.

---

## 1. Purpose

The `noet-collab` collaboration overlay (Phase 1) provides human attestations —
comments, sign-offs, flags — keyed on noet document nodes. Its anchor is
`(site_url, asset_version, bid)`.

This document generalizes that model into an **attestation fabric**: an
infrastructure layer that can attach structured, identity-attributed attestation
records to any artifact in any system, as long as that artifact can be given a
stable path and a content fingerprint.

The generalization is additive. Phase 1 schemas, APIs, and records require no
migration. The extensions described here slot into the same data model.

---

## 2. Scope

**In scope:**
- Generalized anchor schema (`path` + `version`)
- Path+version history and predecessor chains
- Machine-issued attestation kinds alongside human attestations
- Independence protocol field in boundary policy
- Protocol registry design and governance
- Federation model across heterogeneous substrate servers
- Prior art mapping and build-vs-adopt recommendations

**Out of scope:**
- Domain-specific substrate definitions (those belong in domain documentation)
- The `noet-collab.js` client and DOM overlay (see `collaboration_overlay.md`)
- Independence protocol content (those belong in a protocol registry, not here)
- Specific deployment configurations

---

## 3. Prior Art

Several mature systems address overlapping problems. Understanding them prevents
reinvention and identifies what is genuinely novel.

### 3.1 in-toto

[in-toto](https://in-toto.io) (CNCF graduated) is the closest prior art.
It provides:

- **Links** — attestation records for each step in a software supply chain:
  who ran the step, what inputs went in, what outputs came out.
- **Layouts** — policy documents declaring which steps are authorized, who may
  perform them, and what the expected artifact flow is. This is the in-toto
  equivalent of an independence protocol definition.
- **Compositional verification** — traversing the link chain and verifying the
  whole pipeline matches the layout.
- **Predicate types** — the `in-toto/attestation` spec supports arbitrary typed
  JSON payloads, directly analogous to machine-issued attestation kinds.

Tooling: Go (most mature), Python, Rust, Java language bindings.

**Key lesson:** in-toto struggled with adoption because it required buy-in from
every pipeline step simultaneously. The attestation fabric avoids this by
supporting single-boundary pilots (the "hancock" pattern) without requiring
end-to-end adoption.

**Key difference:** in-toto is scoped to software supply chains and artifact
provenance. The attestation fabric extends the model to non-software substrates
and integrates human sign-off credentials in the same schema.

### 3.2 SLSA

[SLSA](https://slsa.dev) (Supply chain Levels for Software Artifacts, OpenSSF)
provides a four-level maturity model built on top of in-toto attestations:

- **L1** — Provenance exists (any record of how an artifact was built)
- **L2** — Signed provenance from a hosted build platform
- **L3** — Hardened build platform (tamper-resistant during the build)

**Key lesson:** the level model gives organizations a clear incremental adoption
path. A single-boundary pilot is effectively SLSA L1 for configuration artifacts.

**Key lesson:** SLSA deliberately split Source and Build tracks because trying to
cover everything at once killed adoption.

### 3.3 Sigstore

[Sigstore](https://sigstore.dev) (OpenSSF) solves the key management problem:

- **Cosign** — artifact signing tool (containers, blobs, SBOMs)
- **Fulcio** — certificate authority that binds ephemeral signing keys to OIDC
  identities (GitHub Actions workflows, service accounts, user emails)
- **Rekor** — immutable, append-only transparency log of signing events

**Key lesson:** ephemeral keys + OIDC identity binding eliminates long-term key
management for machine attesters. The Sigstore identity model maps directly onto
the attestation server's `attester_id` + `credential_type` model.

**Key lesson:** Rekor's transparency log is the right model for the attestation
server's storage layer — append-only, tamper-evident, publicly auditable.

**Recommendation:** for software artifact attestation (the hancock pilot scoped
to CI/CD configuration files), consider using Cosign + in-toto links + Rekor
directly rather than building a custom server. The custom server is needed for
the human sign-off overlay, the path+version history registry, and the
cross-substrate federation — not for basic software provenance.

### 3.4 W3C Verifiable Credentials

[W3C Verifiable Credentials](https://www.w3.org/TR/vc-data-model/) is the
standardized model for the peer-attested credential system in `noet-collab`:

- Credentials are claims made by an **issuer** about a **subject**
- Credentials are cryptographically signed by the issuer
- **DIDs** (Decentralized Identifiers) provide the identity anchor

**Key lesson:** the `noet-collab` `CredentialAttestation` schema re-invents a
subset of VC. Phase 2 should evaluate whether `CredentialAttestation` should be
shaped as a VC for interoperability. The `author_id` opaque field in Phase 1 is
explicitly designed to accommodate this migration without schema changes.

### 3.5 noet BeliefEvent Stream

The noet-core beliefbase (`src/event.rs`, `src/codec/compiler.rs`,
`src/codec/builder.rs`) provides a concrete prior art reference for how a
knowledge graph integrates external event streams:

- **`BeliefEvent`** — a typed event enum covering node upserts, relation
  updates, path changes, and batch framing (`BatchStart`/`BatchEnd`). Every
  event carries an `EventOrigin` discriminant: `Local` for events generated by
  this beliefbase, `Remote` for events arriving from external sources.
- **`on_belief_event` hook** — `DocumentCompiler` exposes this method as the
  designed integration point for external event streams. Currently a stub; wired
  by consumers to drive incremental graph updates from remote sources.
- **`EventOrigin::Remote`** — the compiler applies remote events through the
  same pipeline as locally-generated ones, with no special-casing. This makes
  the beliefbase a natural consumer of any append-only event log, including an
  attestation server's ledger.
- **Federated belief network** — `federated_belief_network.md` describes
  multiple peer nodes each emitting `BeliefEvent` streams that are merged into
  a unified graph. The attestation fabric's federation model is structurally
  identical: each domain's attestation server is a peer emitting events.

**Key lesson:** the operational model is not batch-pull from source files — it
is stream integration via typed events with origin tracking. An attestation
server emitting `NodeUpdate` and `RelationUpdate` events with
`EventOrigin::Remote`, framed by `BatchStart`/`BatchEnd`, integrates into the
beliefbase without any new infrastructure. The `on_belief_event` hook is the
natural wiring point.

**Key difference:** noet's event stream is currently designed for belief graph
nodes (documents, sections, edges). The attestation fabric extends this to
arbitrary substrate types by assigning attestation records and provenance links
as first-class nodes and typed edges in the graph.

### 3.6 Trusted Platform Module / Remote Attestation

TPM remote attestation is prior art for hardware substrate attestation:

- A TPM generates a hardware-bound attestation that specific software (measured
  by hash) is running on specific hardware, signed by a key that cannot leave
  the chip
- Platform Configuration Registers (PCRs) accumulate a hash chain of everything
  loaded at boot — the foundation of secure boot independence checks

**Key lesson:** attestation infrastructure that requires hardware changes has
slow adoption. The software-layer attestation fabric should be designed to
**consume** TPM attestations as one input (a machine attester whose
`credential_type` is `tpm-remote-attestation`) rather than depending on them.

### 3.7 What Is Genuinely Novel

| Component | Prior Art | Novel |
| --------- | --------- | ----- |
| Artifact fingerprinting | Sigstore/Rekor, in-toto | Extension to non-software substrates |
| Pipeline attestation records | in-toto links | Mixed human + machine attesters in one schema |
| Provenance maturity levels | SLSA | Applied to configuration/parameter artifacts |
| Hardware attestation | TPM/TrustZone | Integration into a cross-domain federated fabric |
| Peer-issued credentials | W3C VC, PGP web-of-trust | Engineering role credentials with policy evaluation |
| Threshold governance | FROST, Shamir | Applied to protocol registry governance |
| Path+version history registry | git (source code only) | Generalized across substrate types |
| Independence protocol registry | None known | Genuinely novel |
| Cross-substrate composability | None known | Genuinely novel |
| Event-stream graph integration | noet `BeliefEvent` / `EventOrigin` | Already implemented — attestation server wires to existing `on_belief_event` hook; not novel, cost is near zero |
| Universal comment layer | noet-collab `Comment` kind (noet nodes only) | Generalized to any fingerprintable substrate via `(path, version)` anchor; content-addressed, identity-weighted, cross-substrate |

---

## 4. Core Model

### 4.1 The Anchor

Every attestation is keyed on a two-component anchor:

```toml
[anchor]
path    = "<substrate-scheme>://<authority>/<resource-path>"
version = "sha256:<hex>"
```

**`path`** is the stable, location-identifying URI for the artifact. It names
the thing across time, independent of its current content. The path scheme is
substrate-specific:

```
noet://site.example.com/network/node-bid        # noet document node
git://github.com/org/repo/src/module/file.rs    # source file
fw://device-family/component-name               # firmware image
cfg://product/mission/parameter-set-name        # configuration artifact
part://part-number/serial/SN-XXXX               # physical hardware instance
req://tracker/PROJECT/TICKET-42                 # requirements record
schema://system/message-type-name               # message schema
```

Path scheme definitions are maintained in the protocol registry (§6).

**`version`** is the cryptographic fingerprint of the artifact at this path at
a specific point in time. For a noet site, `version = asset_version` (FNV-1a
hash of the compiled beliefbase). For a file, `version = sha256(file_content)`.
For a physical part, `version = sha256(inspection_record_content)`.

This two-component anchor generalizes the Phase 1 `(site_url, asset_version, bid)`
without invalidating any existing records:

| Phase 1 field | General field |
| ------------- | ------------- |
| `site_url` + `bid` | `path` |
| `asset_version` | `version` |

### 4.2 The Attestation Record

```toml
[[attestation]]
# Anchor — what is being attested
path             = "<path>"
version          = "sha256:<hex>"

# Who attested
attester_id      = "<opaque identity — JWT subject, DID, OIDC claim>"
credential_type  = "<string — matches policy requirement>"

# What was attested
kind             = "Comment | SignOff | Flag | IndependenceCheck | SchemaValidation | ..."
protocol_id      = "<string — from registry, or 'local:<team>:<name>:<ver>'>"
result           = "pass | fail | n/a"
evidence_hash    = "sha256:<hex>"   # hash of structured evidence payload; null for Comment/Flag

# Provenance — prior records this attestation derives from (optional)
[[attestation.provenance]]
path             = "<path of cited record>"
version          = "sha256:<hex>"
role             = "<string — how this prior record supports the attestation>"

# Temporal
timestamp        = "<RFC 3339>"
```

Attestation records are **append-only**. A posted record is never modified or
deleted. Revocation of a credential affects future attestations; it does not
erase historical ones (which were made in good faith and whose historical
validity matters for audit).

### 4.2a Provenance-Chained Attestations

The `provenance` field is **optional**. An attestation with no `provenance`
entries is fully valid — the chain terminates at any record whose attester
attests from direct inspection or authority rather than from prior computational
work. There is no requirement to chain back to a ground truth and no risk of
infinite regress: the chain is a DAG that naturally bottoms out at
root attestations with empty provenance.

The `provenance` field allows an attestation to *optionally* cite prior records
— other attestations, analysis outputs, test evidence, or simulation results —
as the basis for its conclusion. When present, it makes the reasoning chain
machine-traversable rather than implicit in free-text comments.

**When to use provenance:** when the attester's confidence in the attestation
derives not from direct inspection of the artifact but from prior computational
or analytical work that has itself been attested. The classic case is a sign-off
on a parameter set that is justified by simulation results:

```toml
[[attestation]]
path             = "cfg://mission-1/parameter-set/guidance"
version          = "sha256:b7c9d..."
attester_id      = "engineer@example.com"
credential_type  = "guidance-engineer"
kind             = "SignOff"
protocol_id      = "iie:peer-signoff:v1"
result           = "pass"
evidence_hash    = "sha256:e4f2a..."   # structured rationale document

[[attestation.provenance]]
path             = "sim://monte-carlo/guidance-controllability/mission-1"
version          = "sha256:9c3b1..."
role             = "monte-carlo simulation demonstrating positive controllability
                    across all X distribution perturbed environments applicable
                    to this mission scope"

[[attestation.provenance]]
path             = "cfg://mission-1/parameter-set/guidance"
version          = "sha256:a3f81..."   # predecessor version
role             = "prior approved baseline; this version changes only gains
                    within the bounds demonstrated by the simulation"
```

The provenance chain is partially verified by the attestation server. It does
not re-run the cited simulation or re-execute any independence protocol. It
does, however, validate:

1. **Existence** — each cited `(path, version)` must resolve to a known record
   in the federation. A citation to a non-existent or unregistered artifact is
   rejected. This prevents an attester from citing a simulation that was never
   run or a record that was never posted.
2. **Result** — if `required_provenance` in the boundary policy specifies
   `result = "pass"`, the server checks that the cited record has at least one
   attestation satisfying that result. A citation to a failing or unattempted
   record does not satisfy a `pass`-required provenance constraint.

What the server does not do is re-evaluate the quality or correctness of the
cited record's reasoning. That is the attester's responsibility and the
auditor's concern. The server records the stated basis so that:

1. **Auditors** can traverse the chain: "this sign-off cites this simulation;
   was the simulation itself attested? By whom? Under which protocol?"
2. **Downstream consumers** can evaluate the quality of the reasoning: a
   sign-off backed by a cited monte-carlo record with a machine-issued
   `DryRunSimulation` attestation is stronger evidence than one with no
   provenance.
3. **Change propagation** can be automated: if the cited simulation record is
   superseded, its attestation result changes, or the record is flagged, any
   sign-offs that cite it can be surfaced as potentially requiring
   re-evaluation — because the server already holds the citation links.

**`role`** is a free-form string describing how the cited record supports this
attestation. It is human-readable rationale, not a machine-executable check.
The machine-executable check is the `protocol_id` on the cited record itself.

**Provenance is not policy** — with one exception. The sign-off policy for a
boundary declares what credential types are required; provenance records are
supplementary evidence the attester chose to cite and do not substitute for
credential satisfaction. However, when `required_provenance` is present in the
boundary policy, cited provenance *is* policy: the server enforces both that
the right credential is present and that the sign-off cites a passing record
of the declared type. An attestation with the right credential but missing or
failing required provenance does not satisfy that policy. An attestation with
the right credential and no `required_provenance` constraint is policy-valid
(though an auditor may have questions about the basis).

**Provenance anchors use the same `(path, version)` schema as the primary
anchor.** Any fingerprintable artifact in the federation — a simulation output,
a test report, a requirements record, another attestation's evidence payload —
can be cited as provenance, as long as it has a stable path and a content hash.

### 4.3 Attestation Kinds

Human-authored kinds (Phase 1):
- `Comment` — free-form text; no protocol, no result, no evidence
- `SignOff` — policy-gated approval; credential must satisfy boundary policy
- `Flag` — attention marker; no policy requirement

Machine-issued kinds (Phase 2+):
- `IndependenceCheck` — execution of a named independence protocol
- `SchemaValidation` — artifact conforms to declared schema
- `SignatureVerification` — cryptographic signature verified
- `BoundsCheck` — values within declared limits
- `CrossCorrelation` — consistency across related fields or sensors
- `DryRunSimulation` — artifact executed in sandbox without side effects

A CI/CD pipeline, a schema validator, a test harness, or an autonomous agent is
simply another `attester_id` presenting a `credential_type` of
`ci-pipeline`, `schema-validator`, etc. The server evaluates policy without
distinguishing human from machine — that distinction lives in the credential type
string and the policy that requires it.

### 4.3a Policy-Required Provenance

In some cases a boundary policy may require that a sign-off cite specific
categories of provenance — not just that the attester holds the right
credential, but that their attestation is backed by particular kinds of prior
records. This is expressed as an optional `required_provenance` block in the
boundary policy:

```toml
[sign_off_policy]
required = [
    { credential = "guidance-engineer", count = 1 },
]

[[sign_off_policy.required_provenance]]
path_scheme  = "sim://monte-carlo"   # cited record must use this path scheme
role_pattern = "controllability"     # role string must contain this substring
result       = "pass"               # cited record's own attestation must be pass
```

This is a stronger constraint than credential alone: the server checks not only
that the right person signed off, but that their sign-off cites a passing
record of the declared type. This pattern is appropriate for high-consequence
boundaries where the reasoning chain is itself part of the compliance record —
for example, a flight parameter release that must be backed by a demonstrated
simulation result, not just an engineer's judgment.

`required_provenance` is an optional policy field. Most boundaries do not need
it. Use it when the organizational or regulatory context requires demonstrable
computational backing, not just qualified human approval.

**Combinatorial semantics:** multiple `required_provenance` entries are
**AND**-evaluated — every entry must be satisfied independently by a distinct
provenance citation. This is the same semantics as `sign_off_policy.required`:
each entry is a separate requirement, not an alternative. An attestation that
cites one simulation result satisfying two different `required_provenance`
entries satisfies only the first matching entry; the second entry remains
unsatisfied until a distinct citation covers it.

### 4.4 Boundary Policy

The sign-off policy (currently in noet frontmatter) extends to include an
optional independence protocol specification:

```toml
[sign_off_policy]
required = [
    { credential = "schema-validator", count = 1 },   # machine
    { credential = "bounds-checker",   count = 1 },   # machine
    { credential = "domain-reviewer",  count = 1 },   # human
]

[independence_protocol]
protocol_id = "iie:schema-validate:v1"
checks = [
    "payload conforms to declared schema",
    "all required fields present",
    "version field satisfies semver constraint",
]
inputs   = ["wrapper.metadata.schema_ref", "substrate.local_state"]
evidence = "machine-readable JSON report, hash-bound to input"
```

The `independence_protocol` block tells any machine attester what it must verify
before posting an `IndependenceCheck` attestation for this boundary. The server
does not re-execute checks; it validates that the posted attestation's
`credential_type` matches what the policy requires.

---

## 5. Path+Version History

### 5.1 The Problem with Pure Content-Addressing

A pure `content_hash` anchor is ahistorical — it identifies *what* something is,
not *where* it lives or *when* it existed. When an artifact at a stable location
evolves over time, each new version gets a new hash and historical attestations
become orphaned. You cannot ask "show me all comments on this artifact across
all its versions."

### 5.2 Version Records and Predecessor Chains

The server maintains a version registry — an explicit DAG of versions per path:

```toml
[[version]]
path        = "cfg://mission-1/parameter-set/guidance"
version     = "sha256:b7c9d..."
predecessor = "sha256:a3f81..."   # null for initial version
registered  = "<RFC 3339>"
registrant  = "<attester_id>"
```

This is structurally identical to git's commit DAG: content-addressed nodes
linked by explicit predecessor references. It handles branching (two versions
with the same predecessor) and is immune to clock skew because the chain is
structural, not temporal.

### 5.3 History Query Model

With path + version + predecessor chain, three query modes become available:

```
# All attestations for all versions of this artifact
GET /attestations?path=cfg://mission-1/parameter-set/guidance

# Attestations for one specific version
GET /attestations?path=cfg://mission-1/parameter-set/guidance&version=sha256:b7c9d...

# Ordered version timeline with attestation counts per version
GET /history?path=cfg://mission-1/parameter-set/guidance
```

The history query traverses the predecessor chain from the most recent version
backward, collecting all attestations at each version. This reconstructs the
full annotation timeline of any named artifact — comments, sign-offs, machine
checks — across its entire evolution.

### 5.4 Relationship to Existing Fields

For noet document nodes, path+version history maps cleanly onto existing fields:

| History field | noet-collab field | Semantics |
| ------------- | ----------------- | --------- |
| `path` | `site_url` + `bid` | Stable identity of the node |
| `version` | `asset_version` | Content fingerprint of the compiled site |
| `predecessor` | (implicit, via `prior_version`) | Previous `asset_version` |

The `prior_version` field in the Phase 1 sign-off summary is a degenerate form
of predecessor tracking. The version registry formalizes and generalizes it.

---

## 6. Protocol Registry

### 6.1 Role

The protocol registry is a shared, version-controlled store that resolves
`protocol_id` strings to their check specifications. It plays the same role as
DNS: a shared vocabulary, not a shared gatekeeper.

A `protocol_id` like `iie:bounds-check:v1` resolves to a registry entry that
is simultaneously a **check specification**, a **node schema definition**, and
a **graph traversal role declaration**. The `protocol_id` is the `schema:`
value used in noet query filters — the same identifier viewed from three angles.

```toml
[[protocol]]
id          = "iie:bounds-check:v1"
name        = "Bounds and Sanity Check"
version     = "1"
intent      = ["Configuration", "Observation"]
identity    = ["Semantic", "Cryptographic"]

# Check specification — what an attesting agent must verify
checks      = [
    "value within declared min/max",
    "unit dimension consistent with schema",
    "no NaN or Inf",
]
inputs      = ["wrapper.metadata.bounds", "substrate.local_state.schema"]
outputs     = ["pass | fail", "violated_check_id"]
evidence    = "machine-readable JSON report, hash-bound to input"
attester_credential = "bounds-checker"

# Node schema definition — fields a node tagged with this protocol_id must carry
[protocol.schema]
required = ["path", "version", "attester_id", "result", "evidence_hash", "timestamp"]
optional = ["provenance"]
result_values = ["pass", "fail"]

# Graph traversal roles — how nodes of this schema participate in the belief graph
# These declarations determine which edges the attestation server emits as
# BeliefEvents, and which query traversal expressions are meaningful over this schema.
[[protocol.graph_roles]]
edge_kind   = "Epistemic"
source_role = "self"           # this attestation node is the source
sink_role   = "provenance"     # each cited provenance record is the sink
description = "draws from — this attestation's conclusion derives from cited records"

[[protocol.graph_roles]]
edge_kind   = "Pragmatic"
source_role = "self"           # this attestation node is the source
sink_role   = "boundary"       # the trust boundary node this attestation covers
description = "maps_to — this attestation covers this boundary"
```

The `protocol.graph_roles` block is the bridge between the check specification
and the noet DAG model. It declares which `BeliefEvent` variants the attestation
server emits when a node of this schema is posted:

- Each `Epistemic` role entry → `RelationUpdate(WeightKind::Epistemic)` from the
  attestation node to each cited provenance record
- Each `Pragmatic` role entry → `RelationUpdate(WeightKind::Pragmatic)` from the
  attestation node to the boundary it covers

Query expressions over attestation nodes use `schema:iie:bounds-check:v1` as
the `NodeFilter` predicate, then traverse the declared edges:

```
# All passing bounds checks on this artifact
schema:iie:bounds-check:v1 AND result:pass

# Provenance chain: what did this bounds check draw from?
schema:iie:bounds-check:v1 s-epistemic-k(*)

# What boundaries does this bounds check cover?
schema:iie:bounds-check:v1 s-pragmatic-k
```

No new query primitives are required. The registry entry defines the vocabulary;
the existing traversal syntax (`KIND_SET`, `INPUT_ROLES`, `OUTPUT_ROLES`) handles
the relational structure.

The `attester_credential` field names the credential type that a passing
attestation must carry. The attestation server resolves the `protocol_id` at
policy evaluation time — a single read-only lookup, with no write coupling.

The five seed protocol entries for the initial registry are:

| `protocol_id` | Schema tag | Primary graph roles |
| ------------- | ---------- | ------------------- |
| `iie:peer-signoff:v1` | Human approval with credential | Epistemic → provenance; Pragmatic → boundary |
| `iie:schema-validate:v1` | Machine schema conformance check | Pragmatic → boundary |
| `iie:bounds-check:v1` | Machine range/sanity check | Pragmatic → boundary |
| `iie:sig-verify:v1` | Cryptographic signature verification | Pragmatic → boundary |
| `iie:mode-gate:v1` | State/mode compatibility check | Pragmatic → boundary |

`iie:peer-signoff:v1` is the only seed protocol with an Epistemic role — human
sign-offs are the primary carriers of provenance chains, because human judgment
is typically backed by prior computational records. Machine check protocols cover
boundaries directly without citing prior records (their evidence payload is
self-contained in the `evidence_hash` field).

### 6.2 Namespacing

- **`iie:<name>:<ver>`** — registered protocols, governed by the registry
- **`local:<team>:<name>:<ver>`** — experimental protocols; valid but not
  portable across organizations. Teams can use these without registry approval,
  accepting that downstream consumers may not recognize them. When an attestation
  server receives an attestation referencing a `local:` `protocol_id` it has
  never seen, it accepts the attestation but records `policy_status:
  unrecognized_protocol` for that entry. The attestation is never silently
  rejected — unknown local protocols are logged as candidates for registry
  nomination. An attestation with an unrecognized `local:` protocol does not
  satisfy any `required_provenance` or `sign_off_policy` entry that names that
  protocol.

### 6.3 Governance

Protocol registry updates require a **threshold signature** — a `k-of-n`
construction where any `k` of `n` designated key holders must co-sign an update
for it to be accepted. No single holder can unilaterally add or modify a
protocol definition.

Threshold signature schemes (e.g., FROST — Flexible Round-Optimized Schnorr
Threshold signatures) produce a single aggregate signature verifiable against the
group's public key. The registry server refuses any update without a valid
threshold signature. Clients can independently verify that the registry they are
reading was legitimately updated.

Key holder composition should span organizational functions so that no single
team controls registry evolution. Changes are also visible as a PR history on
the underlying git repository, providing a human-readable governance audit trail
alongside the cryptographic one.

The critical design constraint: **the registry defines what protocols mean;
it never defines which boundaries must use them.** Boundary policy assignment
stays local, version-controlled alongside domain source files, and owned by the
domain team. This is what prevents the registry from becoming a centralized
gatekeeper.

---

## 7. Identity and Credentials

### 7.1 JWT as Enterprise SSO Wrapper

Phase 1 uses JWT from an operator-configured OIDC provider. JWT provides:

- Revocation via issuer (the SSO provider stops issuing tokens on offboarding)
- Centralized audit (every token issuance logged by the SSO provider)
- No key management burden on individuals
- Integration with existing HR provisioning and deprovisioning

JWT does not provide durable cryptographic proof of identity — a JWT expires and
becomes meaningless. The append-only ledger compensates: a JWT says "Alice was
authenticated when this attestation was posted"; the ledger says "this is what
was posted and it cannot be altered." The combination is enterprise-grade:

```
JWT (who posted it, when, revocable)
+
Append-only ledger (what was posted, immutable)
=
Auditable attestation record
```

This is the same model Sigstore uses: Fulcio issues a short-lived cert bound to
an OIDC/JWT identity; Rekor records the signing event permanently. The cert
expires; the Rekor entry does not.

### 7.2 Migration Path

The `author_id` / `attester_id` field is deliberately opaque in Phase 1. In
Phase 2, it can hold a DID, a W3C Verifiable Credential reference, or a
Keyhive capability token without schema migration. The identity layer is a
pluggable field; the ledger and policy evaluation logic are unchanged.

For inter-organization or public federation, JWT's dependence on a private SSO
provider becomes a liability — external parties cannot verify the issuer's
integrity without trusting that organization's infrastructure. DIDs and W3C VCs
provide identity claims verifiable without trusting the issuing organization.
This is a Phase 3 concern for intra-enterprise deployments.

### 7.3 Peer-Attested Credentials

The `CredentialAttestation` model in `noet-collab` (one authenticated user
asserting that another holds a named credential) is a simplified subset of the
W3C Verifiable Credentials model. Its current shape:

```
CredentialAttestation {
    credential_id:   Uuid,
    attester_id:     AuthorId,
    subject_id:      AuthorId,
    credential_type: String,
    issued_at:       SystemTime,
    revoked_at:      Option<SystemTime>,
    note:            Option<String>,
}
```

Phase 2 evaluation: determine whether `CredentialAttestation` should be shaped
as a W3C VC. Benefits: interoperability with any VC-compatible tool; VC
credential status lists provide more scalable revocation than `revoked_at`.
Cost: additional schema complexity and dependency on VC tooling.

---

## 8. Federation

### 8.1 Model

Each domain, team, or system operates its own attestation server instance. The
existing `peer_watermarks` sync mechanism in `noet-collab` (used for federation
between noet-collab peers) generalizes to cross-domain federation: each server
knows its configured peers and syncs attestation records from them.

Cross-domain traceability emerges from the anchor: the same `(path, version)`
pair appears in attestation records across servers. A firmware image attested by
the CI/CD server as cryptographically signed carries the same `version` hash as
the record on the avionics server attesting it passed bounds checks. Any client
that queries both servers for the same anchor sees a unified attestation picture.

### 8.2 Discovery

The protocol registry's DNS role extends to path scheme authority resolution:
the registry maps path scheme prefixes (e.g., `fw://`, `cfg://`) to the
federation member that owns that substrate namespace. This allows a client that
holds a `(path, version)` anchor to discover which server(s) to query without
pre-configuring every possible peer.

For intra-enterprise deployment, the set of servers is small and known; explicit
peer configuration is sufficient. The discovery layer is a Phase 3 concern for
larger or open federations.

### 8.3 Scope Boundaries

`noet-collab` is infrastructure, not policy. It does not:

- Define which protocol IDs apply to which boundaries (that is domain frontmatter)
- Execute independence checks (that is the attesting agent)
- Maintain the protocol registry (that is a separately governed service)
- Enforce organizational compliance (that is a reporting layer built over the ledger)

These constraints keep the server application-domain-agnostic and prevent it from
becoming the centralized gatekeeper the architecture is designed to avoid.

---

## 9. Universal Comment Layer

A consequence of the generalized anchor is that the attestation fabric becomes a
**universal margin** — a structured, identity-attributed comment layer that can
attach to any fingerprintable artifact in any system in the federation.

A `Comment` attestation anchored to `(path, version)` is semantically equivalent
to a margin note on a specific version of a specific artifact. Because the anchor
is content-addressed, the comment follows the artifact's identity across systems —
not a URL that becomes stale, but a hash that is stable.

This is qualitatively different from existing comment systems (issue tracker
comments, pull request reviews, document suggestions) because:

- **Content-addressed, not location-addressed** — comments survive link rot,
  repository moves, and system migrations as long as the artifact hash is preserved
- **Identity-weighted** — a comment from a `safety-engineer` credentialed attester
  is queryably distinguishable from a comment from an unauthenticated observer
- **Composable with machine attestations** — a machine check result and a human
  explanatory comment share the same anchor and appear in the same query response
- **Cross-substrate** — comments from different domains on the same artifact are
  unified by the shared hash, regardless of which federation server holds them

The history model (§5) extends this: comments are threaded across versions via
the predecessor chain, providing a persistent conversation across the evolution
of any named artifact.

---

## 10. Build vs. Adopt Recommendations

| Capability | Recommendation |
| ---------- | -------------- |
| Software artifact signing | Use **Cosign** (Sigstore) directly |
| Software provenance records | Use **in-toto** link format directly |
| Append-only transparency log | Use **Rekor** as storage backend or reference design |
| Human sign-off overlay on noet nodes | Build `noet-collab` Phase 1 (novel, no OTS equivalent) |
| Path+version history registry | Build (novel, git is source-code-only) |
| Cross-substrate federation | Build (novel) |
| Independence protocol registry | Build (novel) |
| Credential issuance (Phase 2) | Evaluate **W3C VC** tooling |
| Threshold signature governance | Use **FROST** or equivalent TSS library |

The minimum viable path for a first domain-specific pilot (e.g., configuration
artifact attestation in a CI/CD pipeline):

1. Sign the artifact with Cosign using workload identity
2. Record an in-toto link for the boundary crossing
3. Post the link hash to a Rekor instance as the append-only receipt
4. Define the boundary policy in domain frontmatter referencing `iie:sig-verify:v1`

This requires zero custom server infrastructure and provides immediate
interoperability with the broader supply chain security ecosystem. The custom
attestation server is needed when human sign-off credentials, cross-substrate
federation, or the path+version history model are required.

---

## 11. Open Questions

- **❓1 Rekor as backend** — Should the attestation server use Rekor as its
  append-only storage backend rather than SQLite? Benefit: tamper-evidence and
  public auditability for free. Cost: operational dependency on Rekor or a
  self-hosted Rekor instance.

- **❓2 in-toto predicate compatibility** — Should machine-issued attestation
  records use the `in-toto/attestation` predicate format as their evidence
  payload schema? Benefit: interoperability with any in-toto-aware tool. Cost:
  additional schema constraints on evidence payloads.

- **❓3 VC migration trigger** — What organizational event or capability
  requirement should trigger the Phase 2 migration from JWT `CredentialAttestation`
  to W3C VC shape? Candidates: first inter-organization federation, first
  external audit requirement, Keyhive capability model reaching production.

- **❓4 Version registration authorization** — Who is authorized to register a
  new `version` for a given `path`? Open: any authenticated user (permissive,
  enables drive-by attestation); only the path's declared owner (controlled, but
  requires owner registry); any user who can produce a valid attestation for that
  path (self-certifying). Recommendation: open for Phase 2, with owner-registry
  as a per-path-scheme config option in Phase 3.

- **❓5 Predecessor declaration timing** — Must a `predecessor` be declared at
  version registration time, or can it be asserted later by a separate
  `PredecessorClaim` attestation? Later assertion is more flexible but opens the
  chain to retroactive rewriting. Recommendation: declare at registration time;
  allow additive `PredecessorClaim` only for linking pre-existing unregistered
  versions into the chain.

- **❓6 `on_belief_event` wiring design** — §3.5 asserts that wiring the
  attestation server to the noet beliefbase via the `on_belief_event` hook is
  "near-zero cost," but the concrete wiring design is unspecified: who initiates
  the connection, how the `/events?since=<cursor>` pagination is managed, how
  `BatchStart`/`BatchEnd` framing maps to the server's ledger pages, and how the
  compiler handles a stalled or unavailable attestation server. This must be
  designed before Step 4a of ISSUE_65 can be implemented.

- **❓7 Version registration authorization** — Who may register a new `version`
  for a given `path`? Three candidates: (a) any authenticated user — permissive,
  enables drive-by attestation but low spoofing risk since versions are
  content-addressed; (b) only the path's declared substrate owner — controlled
  but requires an owner registry; (c) self-certifying — any user who can produce
  a valid attestation for that path. Recommendation: open for Phase 2 (any
  authenticated user); owner-registry as an opt-in per-path-scheme config in
  Phase 3.

- **❓8 Hancock pilot OTS→custom server transition trigger** — Next Steps §2
  specifies qualitative conditions for moving from Cosign/in-toto/Rekor to the
  custom attestation server ("when human sign-off credentials, cross-substrate
  federation, or path+version history are needed"), but no concrete trigger is
  defined. Recommendation: the trigger is the addition of the first human
  `SignOff` requirement to a hancock IIC entry — at that point the custom server
  is strictly required and the OTS path is exhausted.

- **❓9 Protocol registry governance parameters** — §6.3 specifies threshold
  signature governance but does not name the initial `n` key holders, the
  threshold `k`, or which TSS scheme (FROST or other) to use. These three
  parameters must be decided before the first five seed protocols can be
  published under governance. This is a human/organizational decision, not a
  design question; it is recorded here so it is not silently skipped when the
  registry is first created.

- **❓10 Assertion/Verification dual-role case** — The intent class taxonomy
  (§ of IIE process) distinguishes Verification (demonstrating a prior claim is
  true) from Assertion (a substrate reporting on itself). The common CI/CD case
  — where the same agent both produces an artifact and verifies its own output
  — spans both classes simultaneously. Guidance is needed on how to classify
  IIC entries for dual-role boundaries: two separate IIC entries (one per
  intent), a single entry with both intents listed, or a new `Assertion+
  Verification` compound class.

- **❓11 Attester trust quorum design** — Phase 1 uses single-peer credential
  attestation. Phase 2 may require quorum (N ≥ 2 distinct peers). The quorum
  design is non-trivial: what counts as "distinct" (different user? different
  organization? different device?), whether quorum is per-credential-type or
  global policy, and how partial quorum is surfaced in `policy_status`. Should
  be scoped before Phase 2 credential work begins.

- **❓12 VC migration trigger (coupled with ❓3)** — The trigger for migrating
  `CredentialAttestation` from JWT shape to W3C VC shape is unresolved. The
  three candidate triggers (first inter-organization federation, first external
  audit requirement, Keyhive reaching production) should be evaluated together
  with ❓3, since both concern the same identity layer migration.

---

## 12. Relationship to the noet DAG Model

The noet beliefbase graph (`docs/design/dag_model.md`) uses three orthogonal edge
types — **Section**, **Epistemic**, and **Pragmatic** — to encode containment,
provenance, and normative coverage respectively. These three dimensions map onto the
attestation fabric's structure with striking fidelity:

| noet edge type | Attestation fabric analog | Encodes |
| -------------- | ------------------------- | ------- |
| **Section** | `path` substrate namespace; `(path, version)` anchor | Containment — which artifact, in which substrate boundary, at which version |
| **Epistemic** | `provenance` DAG on attestation records | Reasoning chain — "this attestation draws from this prior simulation/analysis result" |
| **Pragmatic** | The attestation record itself keyed to a boundary | Normative coverage — "this check covers this artifact crossing this boundary" |

The `{maps_to}` directive in noet — where a review section declares third-party
coverage of a requirement without modifying either document — is the proto-attestation:
an owned edge asserting a normative relationship between two nodes the owner does not
structurally contain. The attestation record generalizes this to arbitrary substrate
types and adds cryptographic identity, append-only persistence, and policy evaluation.

### 12.1 Attestation Records as noet Nodes

If attestation records are compiled into a noet beliefbase, the full noet query model
becomes available over attestation data with no new query infrastructure required:

- **Gap analysis** — "which trust boundaries in the IIC have no passing attestation
  for the current artifact version?" is the noet complement operation: nodes reachable
  via Section traversal from the substrate registry but not reachable via Pragmatic
  traversal from the attestation ledger.
- **Provenance traceability** — `get_maps_to_traceability` traverses the Epistemic
  (provenance) DAG from any attestation record back to its root attestations,
  answering "what is the full chain of evidence backing this sign-off?"
- **Coverage reports** — `get_traceability` produces the edge-count matrix across all
  three dimensions simultaneously: which boundaries have Section identity, which have
  Epistemic provenance chains, which have Pragmatic attestation coverage.
- **Consistency checks** — `check_consistency` surfaces unresolved cross-references:
  provenance citations to non-existent records, attestations referencing unregistered
  protocol IDs, or boundaries declared in the IIC with no corresponding attestation
  ledger entry.

The MCP tools already deployed against knowledge corpora (the `search`, `get_submap`,
`get_context` primitives) would apply directly to attestation data once compiled,
giving compliance and audit workflows the same graph-aware query surface used for
requirements traceability and hazard analysis.

### 12.2 Operational Model

The attestation server and the noet beliefbase serve different but directly
composable operational roles:

- **Attestation server** — the write path. Accepts append-only POST requests from
  arbitrary attesters (humans, CI/CD pipelines, agents, TPM hardware) in real time.
  The authoritative store of attestation records. Optimized for low-latency write and
  policy evaluation at POST time.
- **noet beliefbase** — the query path. Consumes attestation records as a
  `BeliefEvent` stream and integrates them into the graph, assigning BIDs to
  attestation records and typed edges to provenance links. The beliefbase is the
  live or snapshot query layer over the attestation ledger.

This is not a batch-pull model. The `BeliefEvent` enum in `src/event.rs` carries an
`EventOrigin` discriminant — `Local` for events generated by this beliefbase,
`Remote` for events arriving from external sources. The `DocumentCompiler` exposes an
`on_belief_event` hook (`src/codec/compiler.rs`) that is the designed integration
point for external event streams. An attestation server integration would emit
`NodeUpdate`, `RelationUpdate`, and `RelationChange` events with
`EventOrigin::Remote`; the compiler applies them through the same pipeline as any
other remote belief stream, with `BatchStart`/`BatchEnd` framing coherent groups of
attestation records for atomic commit.

This is the same pattern used in production deployments, where external records
(issue trackers, hazard reports) are ingested as event streams and compiled into a
noet network queryable via the MCP tools — the external system is the authoritative
write store; the noet beliefbase is the query and traversal layer.

### 12.3 Edge Type Assignments

When attestation records are compiled into a noet network, edge types are assigned as
follows:

- **Section edges** — substrate namespace containment: a `cfg://` substrate node
  *consists of* its registered boundary nodes, which *consist of* their versioned
  attestation records.
- **Epistemic edges** — provenance links: an attestation record *draws from* each
  record in its `provenance` field. The epistemic graph is the evidence reasoning
  chain, traversable upstream to root attestations and downstream to all sign-offs
  that cited a given simulation or analysis result.
- **Pragmatic edges** — coverage assertions: an attestation record *maps to* the
  boundary it attests, and *maps to* each protocol ID it satisfies. These are the
  normative claims that gap analysis and coverage reports operate on.

This assignment means the existing `{maps_to}` traceability machinery in noet —
designed for requirements coverage — applies without modification to attestation
coverage, producing compliance reports as a natural byproduct of the query model
rather than a bespoke audit tool.

## 13. PII Surface Architecture

The attestation service is a multi-tenant running service, not a batch ETL
tool. It receives `BeliefEvent`s from multiple users simultaneously, stores
R and surprise records, and translates them to/from external APIs. Multiple
**PII surfaces** (Personal Inference Interfaces) connect different kinds of
executors (P) to the same attestation service and inference engine.

The acronym is deliberately dual: PII is also Personally Identifiable
Information. The PII surface is where the user's identity (credentials,
role, P-provenance) meets the inference engine's output — where "who you
are" determines "what you see." Every action through the PII surface
produces R that carries the user's identity as provenance. The infosec
meaning reinforces the design constraint: this surface handles
identity-sensitive data and must be governed accordingly.

| PII surface | Executor type | What it does |
|---|---|---|
| **LSP** (Issues 11/12) | Human in editor | Presents inference gaps as diagnostics, captures edits as deltas, emits BeliefEvents for review/sign-off actions |
| **Viewer** (metadata card) | Human in browser | Presents inference results as dashboard, captures annotations/comments |
| **MCP server** | AI agent | Presents inference results as structured queries, captures agent actions |
| **CLI** | Automated pipeline | Emits BeliefEvents from CI/CD, test runners, vulnerability scanners |

All PII surfaces share:
- **Read path**: consume the compiled belief network + inference engine
  output (projection completeness, credibility texture, gaps)
- **Write path**: emit `BeliefEvent`s into the attestation service
- **Role awareness**: the user's role (from the collaboration overlay's
  credential model) parameterizes which inference results are surfaced
  and which procedures are active

The LSP is not a compilation feature — it is a PII surface.
Editor diagnostics are the inference engine's gap findings surfaced
in-editor. Code actions are procedures that fire based on rule maps.
Hover is the metadata card content rendered inline. The engineer's
edits are P acting on S, producing deltas that traverse the write-truth
process (save → commit → PR → merge → recompile). The LSP is the
interface between the inference engine and the user's execution context.

The `api2doc` tool is a complementary but distinct component: it is a
batch ETL library that reads external APIs and produces noet-compilable
documents. It shares entity mapping infrastructure with the attestation
service (same APIs, same credential, same field mappings) but operates
in the opposite direction (external → compilable documents, not
BeliefEvents → external). The attestation service may embed `api2doc`
as a library for the read direction, but the service itself provides
the multi-tenant, role-aware, real-time surface that `api2doc` does not.