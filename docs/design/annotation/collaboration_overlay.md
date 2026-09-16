---
title = "Collaboration Overlay: Attested Annotation Layer for Static Noet Sites"
authors = "Andrew Lyjak, Claude"
last_updated = "2025-07-11"
status = "Draft"
version = "0.1"
dependencies = ["federated_belief_network.md (v0.1)", "search_and_sharding.md"]
---

# Collaboration Overlay: Attested Annotation Layer for Static Noet Sites

> [!IMPORTANT]
> **Design sketch, owned by Issue 65. Read §1–§3 only.**
>
> **§3.4, §5, §6, and §§9–10 are stale** — they describe `asset_version` as the
> anchor and the server as the system of record. Neither holds: the anchor is a
> per-node `sha256` content hash (`identity/content_versioning.md`), and the
> server is a **sync peer** for Issue 105's local sidecar store, not the system
> of record. Where they conflict with §3.2, §3.2 governs.
>
> **Where this is headed**: the general sidecar-propagation design — how an
> overlay is maintained, scoped, and propagated between stores and peers — with
> `federated_belief_network.md` as its use case rather than its sibling. The
> propagation mechanism lives in that document's §1.2 (scoped queues and
> percolation) and the composition rule in `overlay_model.md`; cross-reference
> both rather than restating them. Nothing below has been restructured for this.

---

## 1. Purpose

Static noet sites are read-only by construction: the compiler produces a
frozen artifact (HTML + shards + search indices) served from a CDN or file
host. This is a feature — static sites are fast, cheap, and trivially
deployable. But it means there is no native surface for human attestation:
comments, review sign-offs, redline proposals tied to specific nodes.

This document describes a **collaboration overlay**: a thin identity-bearing
event stream that provides progressive enhancement on top of any static noet
site, without modifying the site's served files or the noet-core compiler.

**Scope of this document.** This is the **noet-specific integration layer**.
It specifies how a general attestation server attaches to a static noet site:
the DOM anchor (`data-bid`), the content fingerprint (`asset_version`), the
client script injection (`{{COLLAB_ENDPOINT}}`), and the noet-core integration
points. The substrate-agnostic infrastructure design — attestation record
schema, provenance model, protocol registry, federation, path+version history,
and prior art analysis — is specified in `attestation_fabric.md`. This document
treats that as a dependency and specifies only the noet-specific surface on top
of it.

The overlay is a concrete first application of the **Layer 3 Activity Events**
concept from `federated_belief_network.md` §3.7. It introduces a new peer
type — the **collaboration peer** — that owns no source files and emits no
`BeliefEvent`s (Layer 2), but accumulates and serves human attestations keyed
on stable Layer 2 identifiers.

---

## 2. Layered Model

The federated belief network doc defines three layers. This feature adds a
fourth that sits at the boundary between Layer 2 and Layer 3:

```
Layer 3: Human Attestations  ← THIS DOCUMENT
  Comments, sign-offs, redline proposals
  Identity-bearing, multi-producer, CRDT-merged
  "What have humans said about the content?"

Layer 2: Belief Graph
  Parsed nodes, edges, paths, compiled shards
  Single-owner-per-node, pull-replicated
  "How does the content relate?"

Layer 1: Source Files
  Filesystem, WatchService, DocumentCompiler
  "What content is in the data?"
```

The collaboration peer lives at Layer 3 and reads Layer 2 identifiers
(`Bid`, `asset_version`) as opaque keys. It has no compiler, no WatchService,
no filesystem root. It is a pure authenticated event log server.

---

## 3. Core Concepts

### 3.1. The Attestation

An **attestation** is a statement about a specific version of a specific node.
`attestation_fabric.md` §4.1 defines the general two-component anchor
`(path, version)`. For noet document nodes, the mapping is:

| General field | noet-specific value |
| ------------- | ------------------- |
| `path` | `site_url + "/" + bid` |
| `version` | `sha256` content hash of the node's non-metadata content |

The noet-specific fields that compose the anchor are:

- **`bid`** — the `Bid` of the target node (stable across renames, moves,
  and re-renders; embedded in the DOM by the existing viewer)
- **`version`** — a scoped content hash of the target node. **Defined by
  [`content_versioning.md`](../identity/content_versioning.md)**, which is authoritative;
  it is not restated here. What matters to this document: `version` changes when
  the annotated scope changes and not otherwise, and which scope a record anchors
  to is selected by its `record_kind`.
- **`site_url`** — the canonical base URL of the deployed site (from
  `<script id="noet-base-url">`), disambiguating between multiple deployments
  of the same source

Together, `(site_url, bid, version)` is the noet-specific instantiation
of the general `(path, version)` anchor. The collaboration server stores `path`
and `version` internally; the decomposition is a convenience for
the client and for human readability. See `attestation_fabric.md` §4.1 for the
full anchor schema and §5 for the path+version history and predecessor chain
model.

### 3.2. Consequences for This Overlay

> Earlier revisions argued at length that `asset_version` — an FNV-1a hash over
> the *entire* compiled beliefbase — was the right fingerprint. That is rejected;
> `content_versioning.md` §2 records why. Only the consequences for the overlay
> are kept here.

Three properties follow from the anchor being a per-node scoped hash:

- **The client needs per-node hashes in the DOM.** With a global
  `asset_version` the value was already on every page. Now the viewer must emit
  the target node's hash alongside `data-bid`; Issue 65 carries the task.
- **`asset_version` is retained as an informational field** — which build the
  attester was viewing — but is never the anchor.
- **The server still treats `version` as an opaque string.** It does not need to
  understand noet's shard format, or which scope a hash covers.

### 3.3. Attestation Kinds

Three kinds are in scope for Phase 1:

| Kind | Description |
|------|-------------|
| `Comment` | Free-text annotation on a node; may thread (reply-to chain) |
| `SignOff` | Explicit approval of a node at a specific `asset_version`; identity-bearing |
| `Flag` | Lightweight marker: "needs review", "outdated", "question" |

Out of scope for Phase 1 (may be Phase 2):
- **Redline / proposed diff** — proposes a source edit; requires the
  federation write-back path and source-level diff semantics. Much larger scope.

### 3.4. Attestation Record Schema

The canonical attestation record schema is defined in `attestation_fabric.md`
§4.2. The noet-specific wire format exposes the three-field anchor
decomposition as a convenience for the client script and API consumers:

```
AttestationEvent {
    // Identity
    peer_id:       PeerId,        // collaboration peer's stable UUID
    author_id:     AuthorId,      // identity of the human (see §4)
    timestamp:     SystemTime,

    // Anchor (noet decomposition of attestation_fabric.md (path, version))
    site_url:      String,        // e.g. "https://docs.example.com"
    asset_version: String,        // e.g. "3a9f1b2c" (FNV-1a hex)
    bid:           String,        // target node BID (UUID string)
    // Stored internally as: path = site_url + "/" + bid, version = node content hash

    // Content
    kind:          AttestationKind,  // Comment | SignOff | Flag
    payload:       AttestationPayload,

    // Threading (comments only)
    reply_to:      Option<EventId>,  // parent comment event ID
}

AttestationPayload = one of:
    Comment  { text: String }
    SignOff  { note: Option<String> }
    Flag     { label: String, note: Option<String> }
```

This is a **Layer 3 event** in the sense of `federated_belief_network.md`
§3.7: multi-producer, identity-bearing, suitable for CRDT-style merge. The
natural storage substrate is Automerge (as the federation doc recommends for
Layer 3), or a simpler append-only SQLite log for the initial implementation.
The Phase 2+ attestation kinds (`IndependenceCheck`, `SchemaValidation`, etc.)
are defined in `attestation_fabric.md` §4.3 and are not repeated here.

---

## 4. Identity Layer

noet-core deliberately has no identity model. The collaboration overlay
requires one. Two options, in order of complexity:

### 4.1. Phase 1: Signed JWTs from a self-hosted auth server

The collaboration server validates JWTs issued by a configured identity
provider (any OIDC-compatible provider: Authentik, Keycloak, GitHub OAuth,
Google, etc.). The `author_id` in the attestation record is the subject claim
from the JWT.

This is the minimal viable approach. It requires no new cryptographic
infrastructure and is deployable today.

**✅ ❓1 Resolved**: JWT Phase 1 is sufficient. The `author_id` field is
intentionally opaque — it carries only the JWT subject claim. Phase 2 replaces
JWT validation with Keyhive capability verification without any schema changes
to attestation records.

### 4.2. Phase 2: Keyhive capability-based identity

`federated_belief_network.md` §5.3 already names Keyhive as the target
authorization layer for the federated network. Keyhive is a capability-based
access control system designed for signed, attributed event streams across
untrusted peers — exactly what attestations require.

Migration path: the `author_id` field is already present in the attestation
schema. Phase 2 replaces the JWT validation in the collaboration server with
Keyhive capability verification; the attestation records themselves don't
change.

---

## 4a. Credential Model

### Design Principle: Peer-Derived, Not Administrator-Assigned

Sign-offs are only meaningful when the signer holds credentials relevant to
the content being signed. The collaboration overlay uses a **peer-derived
credential model**: credentials are claims that one authenticated user makes
about another. There is no administrator who configures roles or assigns
permissions.

Any authenticated user can attest that any other user holds a named
credential. The weight of that attestation derives from the web of attesters
and is auditable — every credential record shows who vouched for whom. This
is the same model as PGP's web-of-trust applied to professional credentials:
"I, Alice (Structures Lead), attest that Bob holds the credential
`structures-engineer`."

This contrasts with top-down role systems (an administrator assigns roles via
a config panel) which create a single point of failure and require privileged
access to manage.

### Credential Attestation Schema

```
CredentialAttestation {
    credential_id:   Uuid,               // stable ID for this attestation record
    attester_id:     AuthorId,           // who is making the claim (JWT subject)
    subject_id:      AuthorId,           // who the credential is being claimed for
    credential_type: String,             // e.g. "structures-engineer", "safety-reviewer"
                                         // free-form; consuming app defines valid types
    issued_at:       SystemTime,
    revoked_at:      Option<SystemTime>, // null = active; non-null = revoked (prospective)
    note:            Option<String>,     // optional justification
}
```

Credential types are free-form strings. The collaboration server does not
maintain a fixed registry — it stores whatever strings are used. The sign-off
policy in the document frontmatter defines which types are required; the
server checks that the presented credential's type matches.

### Sign-Off Policy in Document Frontmatter

The sign-off policy lives in the noet source document's frontmatter. It is
compiled into the rendered page as a `data-signoff-policy` attribute on the
node's DOM element, where `noet-collab.js` reads it at sign-off time and
includes it in the `POST /attestations` body.

Example frontmatter policy:

```toml
[sign_off_policy]
required = [
    { credential = "structures-engineer", count = 1 },
    { credential = "safety-reviewer",     count = 1 },
]
```

The policy says: this node requires at least one sign-off from someone holding
`structures-engineer` AND at least one from someone holding `safety-reviewer`.
Neither requirement names a specific individual — only a credential type and
a minimum count. The same person may not satisfy two distinct credential slots
unless they hold both credential types.

For nodes without their own source file (e.g. section nodes generated from
headings), the policy is inherited from the parent document's frontmatter. If
absent there, no policy applies — sign-offs are accepted and recorded but
`policy_status` returns `no_policy`.

### Validation Flow at Sign-Off Time

```
1. Signer POSTs a SignOff attestation including:
   - their JWT (identifies author_id)
   - presented_credential_id: the CredentialAttestation UUID they sign with
   - policy: snapshot of the node's sign_off_policy at sign-off time

2. Server validates:
   a. JWT is valid → author_id extracted
   b. CredentialAttestation[presented_credential_id] exists, is not revoked,
      and subject_id == author_id  (signers may only present their own credentials)
   c. The presented credential's type appears in the policy's required list
   d. If (a)+(b)+(c) pass: record attestation with policy_satisfied = true
      If (c) fails (wrong type): record with policy_satisfied = false
      (the sign-off is still recorded; it just does not count toward policy)

3. GET /sign-offs/summary computes policy_status by:
   - grouping policy_satisfied sign-offs at the current asset_version
     by credential_type
   - checking whether each required bucket has >= count distinct signers
   - returning policy_status: { satisfied_buckets, required_buckets, complete: bool }
```

### Revocation Semantics

Credential revocation is prospective, not retroactive. When Alice revokes
Bob's `structures-engineer` credential attestation, she sets `revoked_at` to
the current timestamp. All of Bob's prior sign-offs that presented that
credential remain recorded with their original `policy_satisfied` value —
the append-only attestation log is never modified. Future sign-offs by Bob
presenting the revoked credential will be rejected at step 2b above.

This means a node approved before revocation retains its approval state at
that `asset_version`. The natural re-approval trigger is a content change
(new `asset_version`), not retroactive invalidation.

### Trust and Sybil Resistance

The system does not technically prevent a single user from vouching for all
their colleagues. Trust is a social and organizational concern — every
credential record is auditable (who vouched for whom, when). For QMS contexts,
organizational review processes can inspect the credential graph.

For stronger Sybil resistance in later phases, Keyhive's capability model
can gate who is permitted to issue credentials of a given type (e.g. only
existing `team-lead`-credentialed users can attest the `team-lead` credential).
This is a Phase 2 concern; the schema supports it without changes.

### New API Endpoints (Credential Layer)

```
GET  /credentials
     ?subject=<author_id>
     → Array<CredentialAttestation>   (public; credentials are public claims)

POST /credentials
     Authorization: Bearer <jwt>
     Body: { subject_id, credential_type, note? }
     → { credential_id: Uuid }

DELETE /credentials/<credential_id>
     Authorization: Bearer <jwt>
     (only the original attester may revoke; sets revoked_at, does not delete)
     → { revoked_at: SystemTime }
```

---

## 5. Architecture

### 5.1. Components

```
┌─────────────────────────────────────────────────────────────────┐
│  Static noet site (CDN / file host)                             │
│  HTML + shards + search indices                                 │
│  <script id="noet-asset-version">"3a9f1b2c"</script>           │
│  <script id="noet-base-url">"https://docs.example.com"</script> │
└──────────────────────────┬──────────────────────────────────────┘
                           │  page load
                           ▼
┌─────────────────────────────────────────────────────────────────┐
│  noet viewer (WASM SPA, runs in browser)                        │
│  Knows: BIDs of all visible nodes                               │
│  Knows: asset_version, site_url (from script tags)             │
└──────────────────────────┬──────────────────────────────────────┘
                           │  GET /attestations?...
                           │  POST /attestations  (authenticated)
                           ▼
┌─────────────────────────────────────────────────────────────────┐
│  Collaboration server  (separate service / repo)                │
│  - Authenticates requests (JWT / Keyhive)                       │
│  - Stores AttestationEvents (SQLite or Automerge)               │
│  - Serves overlay data keyed on (site_url, bid, version)        │
│  - No knowledge of noet shard format                            │
└─────────────────────────────────────────────────────────────────┘
```

### 5.2. Viewer Client Script Injection

The collaboration overlay is **opt-in per site**. A new template placeholder
`{{COLLAB_ENDPOINT}}` is added to `template-responsive.html`. When empty
(default), no overlay script is loaded — the static site behaves identically
to today. When set, the viewer loads a thin JS module from the collaboration
server's endpoint:

```html
<!-- In template-responsive.html, after the viewer script tag -->
{{#if COLLAB_ENDPOINT}}
<script type="module"
  src="{{COLLAB_ENDPOINT}}/assets/noet-collab.js">
</script>
{{/if}}
```

The `noet-collab.js` module:
1. Reads `asset_version` and `site_url` from the existing script tags
2. Observes the viewer's BID-to-DOM mapping (via a small hook in `viewer.js`)
3. On page navigation: fetches attestations for visible BIDs
4. Decorates DOM elements with comment counts, sign-off badges, flag markers
5. Provides highlight-and-comment and sign-off UI (right-click / toolbar)

### 5.3. API Surface (Collaboration Server)

Minimal REST API for Phase 1:

```
GET  /attestations
     ?site=<site_url>
     &version=<asset_version>
     &bids=<bid1,bid2,...>
     → Array<AttestationEvent>  (public, no auth required for reads)

POST /attestations
     Authorization: Bearer <jwt>
     Body: AttestationEvent (without peer_id, author_id — server fills these)
     → { event_id: String }

GET  /attestations/<event_id>
     → AttestationEvent

GET  /sign-offs/summary
     ?site=<site_url>
     &version=<asset_version>
     &bids=<bid1,bid2,...>
     → Map<bid, { approved: bool, approver_ids: String[], prior_version: String|null }>
```

The sign-offs summary endpoint is the key one for QMS use: it answers "is
this node approved at the current version, and if not, was it approved at a
prior version?" in a single call.

### 5.4. Viewer DOM Integration

The viewer already emits BID information into the DOM (via `data-bid`
attributes on node elements, used by the metadata panel and traceability
view). The collaboration overlay reads these attributes rather than requiring
any new viewer API.

The `noet-collab.js` module registers a `hashchange` listener (mirroring
`viewer.js`) to re-fetch attestations when the user navigates to a new
document, passing the new page's visible BIDs.

**❓ Open question**: Does `viewer.js` need an explicit hook for the collab
script to subscribe to navigation events, or is `hashchange` + DOM observation
sufficient? A formal hook would be cleaner but requires a small noet-core
change. The `hashchange` approach requires zero noet-core changes.

---

## 6. Sign-Off Semantics

A sign-off is meaningful only if its `asset_version` matches the currently
loaded site's `asset_version`. The collaboration server and client both enforce
this:

- **Server**: The summary endpoint returns `prior_version` when a node has a
  sign-off at a different `asset_version` than requested, allowing the client
  to surface "approved at a prior version — re-approval may be needed."
- **Client**: The overlay renders three distinct visual states:
  - **Green**: current-version sign-off, `policy_status.complete = true`
  - **Yellow**: current-version sign-off present but `policy_status.complete = false`
    (partial — some credential buckets still unsatisfied)
  - **Amber**: no current-version sign-off; `prior_version` is set (stale)

**✅ ❓3 Resolved**: The required approver set is defined by the **document's
sign-off policy** (in frontmatter), not by server-side config and not purely
by the consuming application. The policy is included as a snapshot in each
`POST /attestations` body and stored alongside the record. The server
validates the presented credential against the policy at write time
(`policy_satisfied` field) and computes `policy_status` from stored records
at read time. This keeps the server stateless with respect to policy (no
per-site policy tables to manage) while still enabling a definitive
`policy_status.complete` answer in the summary endpoint.

Policy changes are version-controlled alongside the document source. If a
policy changes after sign-offs are recorded, the summary endpoint re-evaluates
against the current policy; sign-offs recorded under the old policy retain
their stored `policy_satisfied` value but may no longer satisfy the new policy.
This surfaces as `policy_status: { complete: false, note: "policy_updated" }`
and prompts re-approval — correct behavior in a QMS context.

---

## 7. Relation to Existing Design Docs

### 7.1. `federated_belief_network.md`

The collaboration server is a **Layer 3 peer** in the federation. It:
- Has a `PeerId` (stable UUID, persisted in server config)
- Owns no Layer 2 networks (no compiler, no WatchService)
- Emits only Layer 3 events (attestations, not `BeliefEvent`s)
- Uses the same pull-based replication envelope (`LogEntry` with `peer_id`,
  `sequence`, `timestamp`) for any future federation integration

The federation doc's open question §7.1 (log storage format) applies here:
SQLite is the simplest start; Automerge unifies with Issue 16 but adds
complexity. For Phase 1, SQLite is recommended.

### 7.2. `redline_model.md`

[`redline_model.md`](./redline_model.md) defines the general proposal kind: an
annotation whose payload says what content should say instead. The overlay's
`Flag` and `Comment` kinds are complementary to it — a flag ("this is
outdated") observes, a redline proposes — and the two differ in what they commit
their author to rather than in how they are stored.

**A redline is a `record_kind`, not an `AttestationKind` variant.** It is a
registered kind with a payload schema (Issue 104), and promotion of one
into a source edit is Issue 106's. So the overlay needs **no
`AttestationKind::Redline` variant** — adding one would be the enum-variant
approach the record model exists to avoid.

That makes the overlay's forward compatibility stronger than a Phase 2 schema
change: **a new kind is a registry entry, not a code change**
(`attestation_fabric.md` §6; `beliefbase_architecture.md` §4.3). Carrying
redline payloads costs this document nothing, which is why §3.3 can defer
redlines to a later phase on scope grounds alone — the constraint is the
write-back path, not the schema.

### 7.3. noet-core impact

**Zero required changes to noet-core** for Phase 1. The overlay relies
entirely on:
- `asset_version` — already in every page
- `site_url` — already in every page
- `data-bid` DOM attributes — already emitted by the viewer

The `{{COLLAB_ENDPOINT}}` template placeholder is a one-line addition to
`template-responsive.html` and a one-field addition to the compiler's template
substitution map.

---

## 8. Deployment Model

The collaboration server is a **separate service**, not part of noet-core.
Likely a separate repository (e.g. `noet-collab`). It is:

- **Self-hosted**: The operator runs their own instance. There is no
  hosted/SaaS version in scope. This keeps the trust boundary clear — the
  collaboration server is within the operator's control, appropriate for QMS
  and internal review use cases.
- **Lightweight**: A single Axum binary backed by SQLite. No external
  dependencies beyond the auth provider.
- **Site-agnostic**: One collaboration server instance can serve multiple
  static noet sites, disambiguated by `site_url`.

---

## 9. Out of Scope

- **Proposed diffs / redline write-back**: Requires the federation write-back
  path (compiler accepting external edits). Phase 2 at earliest.
- **Real-time collaboration** (live cursors, presence): Not needed for the
  QMS/review use case.
- **Public/anonymous comments**: The auth requirement is intentional. Unauthenticated
  comments are a moderation problem outside this scope.
- **Per-network versioning**: `asset_version` covers the full beliefbase.
  Fine-grained per-network hashes are a future enhancement if needed.
- **Hosted SaaS collaboration service**: Self-hosted only for now.
- **noet-core changes beyond template placeholder**: The overlay is
  progressive enhancement; noet-core stays identity-free.

---

## 10. Open Questions Summary

| # | Question | Status |
|---|----------|--------|
| ❓1 | JWT (Phase 1) vs. Keyhive (Phase 2) for sign-off legal weight | ✅ **Resolved** — JWT Phase 1 sufficient; `author_id` opaque for Phase 2 migration |
| ❓2 | `hashchange` observation vs. explicit `viewer.js` hook | ✅ **Resolved** — `hashchange` + DOM observation sufficient for Phase 1 |
| ❓3 | Required approver set: server-side policy vs. client/application delegation | ✅ **Resolved** — policy in document frontmatter; server validates at write time |
| ❓4 | SQLite vs. Automerge for attestation storage | ✅ **Resolved** — SQLite for Phase 1; no blocking issue |
| ❓5 | Separate `noet-collab` repo vs. mode of `noet serve` | ✅ **Resolved** — separate repo |
| ❓6 | Credential type namespace: open strings vs. per-site whitelist | 🔵 Non-blocking — open for Phase 1; optional whitelist config in Phase 2 |
| ❓7 | Attester trust threshold: single-peer vs. quorum attestation | 🔵 Non-blocking — single-peer sufficient for Phase 1; quorum as optional Phase 2 config |
| ❓8 | Policy inheritance for section nodes without own source files | 🔵 Non-blocking — inherit from parent document; `no_policy` if absent |

---

## 11. Implementation Plan (Phase 1)

Open questions are resolved. Implementation is tracked in
`docs/project/ISSUE_65_ATTESTATION_SERVER.md`. High-level sequence:

1. **`noet-collab` repo**: Axum server, SQLite schema (attestations +
   credential_attestations tables), JWT validation middleware, REST API (§5.3
   + §4a credential endpoints)
2. **`noet-collab.js` client module**: fetch attestations, decorate DOM,
   comment/sign-off/flag UI; sign-off flow shows signer their available
   credentials and which policy slot each satisfies; "Vouch for a colleague"
   panel for credential attestation
3. **noet-core**: add `{{COLLAB_ENDPOINT}}` placeholder to
   `template-responsive.html` and compiler template substitution (one-line
   change); add `data-signoff-policy` attribute emission for nodes with a
   `sign_off_policy` frontmatter key
4. **Integration test**: deploy a static noet site, start a local collab
   server, vouch for a test user, submit a sign-off with a credential, verify
   `policy_status.complete` in the summary response

See `docs/project/ISSUE_65_ATTESTATION_SERVER.md` for the tracked work.
