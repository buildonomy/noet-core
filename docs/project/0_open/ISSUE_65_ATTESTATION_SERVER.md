---
version = "0.1"
title = "Issue 65: Attestation Server — General Infrastructure and noet Collaboration Overlay MVP"
---

# Issue 65: Attestation Server — General Infrastructure and noet Collaboration Overlay MVP

**Priority**: LOW
**Estimated Effort**: 7–9 days (RELATIVE COMPARISON ONLY)
**Dependencies**: None blocking — zero required changes to noet-core for Phase 1
**Related design doc**: `docs/design/collaboration_overlay.md` (sketch, v0.1)

---

## Summary

Static noet sites have no native surface for human attestation — comments,
review sign-offs, or flags tied to specific nodes. This issue tracks the work
to resolve open design questions in `collaboration_overlay.md` and deliver a
Phase 1 MVP: a self-hosted attestation server plus a thin viewer client script
that provides progressive enhancement on top of any static noet site.

The server is a **general attestation server** — substrate-agnostic by design —
whose Phase 1 anchor happens to be the noet document node. The anchor schema is
`(path, version)` from `docs/design/attestation_fabric.md` §4.1, where for
noet nodes `path = site_url + "/" + bid` and `version = asset_version`. This
mapping is backward-compatible and costs nothing upfront; it means the server
can generalize to non-noet substrates in later phases without schema migration.

The collaboration overlay is a Layer 3 peer in the federated belief network
(no compiler, no source files) that stores attestations keyed on
`(path, version)` — both values already embedded in every rendered noet page,
requiring zero compiler changes for Phase 1.

---

## Goals

1. Resolve the four blocking open questions in `collaboration_overlay.md`
   (identity model, approver policy, storage substrate, viewer hook strategy)
   and upgrade the design doc from Sketch → Draft status.
2. Deliver a working `noet-collab` server: Axum + SQLite, JWT auth, REST API
   for attestation read/write and sign-off summary. The server uses the
   generalized `(path, version)` anchor schema from `attestation_fabric.md`
   §4.1 so it can serve non-noet substrates in later phases without migration.
3. Deliver a `noet-collab.js` client module that decorates a live static noet
   site with comment counts, sign-off badges, and flag markers fetched from
   the attestation server.
4. Add `{{COLLAB_ENDPOINT}}` opt-in placeholder to `template-responsive.html`
   (one-line noet-core change).
5. Wire the `DocumentCompiler::on_belief_event` hook so the noet beliefbase
   can consume attestation records as a `BeliefEvent` stream with
   `EventOrigin::Remote`, enabling graph queries over attestation data.
6. Validate end-to-end: static site + local attestation server + browser overlay.

---

## Architecture

See `docs/design/collaboration_overlay.md` for the full design. Summary:

**Attestation anchor**: `(path, version)` — the generalized two-component anchor
from `docs/design/attestation_fabric.md` §4.1. For noet document nodes:
- `path = site_url + "/" + bid` — stable identity of the node across versions;
  `site_url` from `<script id="noet-base-url">`, `bid` from `data-bid` DOM attributes
- `version = asset_version` — FNV-1a hash of the compiled beliefbase,
  from `<script id="noet-asset-version">`, already in every page

This is a strict generalization of the original `(site_url, asset_version, bid)`
triple. All existing Phase 1 API query parameters remain unchanged; the server
stores `path` and `version` internally rather than the three-field decomposition.

**Attestation kinds** (Phase 1): `Comment`, `SignOff`, `Flag`

**Attestation server**: separate repo (`noet-collab`), Axum + SQLite,
self-hosted, substrate-agnostic (one instance serves multiple sites and, in
later phases, multiple substrate types beyond noet document nodes).

**Client script**: `noet-collab.js` loaded from the attestation server endpoint,
injected via `{{COLLAB_ENDPOINT}}` template placeholder (no-op when empty).
This script is noet-specific — it reads DOM attributes and decorates rendered
pages. The server it talks to is not noet-specific.

**Identity**: Phase 1 uses OIDC/JWT from operator-configured provider.
Phase 2 migrates to Keyhive (no schema changes needed).

**Credentials**: Sign-offs carry a credential claim. Credentials are peer-attested
("I attest that Alice is a Structures Engineer") rather than administrator-assigned.
A node's sign-off policy (embedded in its noet frontmatter) declares what credential
types are required and in what quantity. The signer presents a peer-issued credential
alongside their sign-off; the collab server validates the credential against the policy
and records both. No central role administrator is required.

**Key API endpoints**:
```
GET  /attestations?path=&version=&path_prefix=  → Array<AttestationEvent>
POST /attestations                              → { event_id }  (auth required)
GET  /sign-offs/summary?path=&version=
     → Map<path, { approved, approver_ids, prior_version, policy_status }>
GET  /credentials?subject=<author_id>           → Array<CredentialAttestation>
POST /credentials                               → { credential_id }  (auth required)
GET  /events?since=<cursor>                     → Array<BeliefEvent> (for noet integration)
```

The `path_prefix` parameter on `GET /attestations` allows the noet client to
fetch attestations for all BIDs under a given `site_url` in a single request,
replacing the previous `bids[]` multi-value parameter. The `/events` endpoint
emits attestation records as `BeliefEvent` stream chunks consumable by the
`DocumentCompiler::on_belief_event` hook.

---

## Implementation Steps

### 0. Design resolution (1 day)
- [ ] ~~Decide ❓1~~: **Resolved** — JWT Phase 1 is sufficient. `author_id` field
      remains opaque; Keyhive replaces JWT validation in Phase 2 without schema change.
- [ ] ~~Decide ❓3~~: **Resolved** — sign-off policy is embedded in the document's
      noet frontmatter (not server-side config, not purely client-side). The server
      validates presented credentials against the policy at POST time and records
      `policy_satisfied: bool` in the attestation. The summary endpoint returns
      `policy_status` derived from stored records, not from a server-side config table.
      This keeps the server stateless with respect to policy and makes policy changes
      visible as source edits (reviewable, diffable, version-controlled).
- [ ] ~~Decide ❓2~~: **Resolved** — `hashchange` + DOM observation is sufficient
      for Phase 1; no noet-core hook required.
- [ ] Design peer-attested credential model (see §Credential Model below) and record
      in `collaboration_overlay.md`.
- [ ] Upgrade `collaboration_overlay.md` from Sketch → Draft with all decisions recorded.

### 1. `noet-collab` server (3 days)
- [ ] New repo: `noet-collab` (Rust, Axum, SQLite via sqlx)
- [ ] SQLite schema using generalized `(path, version)` anchor:
  - `attestations(id, path, version, attester_id, credential_type, kind, protocol_id, result, evidence_hash, timestamp, policy_satisfied, credential_id FK)`
  - `provenance(attestation_id FK, cited_path, cited_version, role)` — optional per-attestation cited records
  - `versions(path, version, predecessor, registered, registrant)` — predecessor chain for path+version history
  - `credential_attestations` table (see §Credential Model)
  - `peer_watermarks`, `site_config`
- [ ] JWT validation middleware (configurable JWKS endpoint or static secret)
- [ ] `GET /attestations` — query by `path` prefix (covers all BIDs under a site_url) and optional `version`
- [ ] `POST /attestations` — authenticated write; server fills `attester_id`,
      `timestamp`; for `SignOff` kind, validate presented credential against the
      sign-off policy embedded in the request body; validate any `provenance` citations
      exist in the `versions` table; set `policy_satisfied`
- [ ] `GET /sign-offs/summary` — per-path approval state at requested version,
      with `prior_version` field when stale sign-offs exist, and `policy_status`
      (count of policy-satisfying sign-offs vs. required)
- [ ] `GET /credentials?subject=<author_id>` — return all credential attestations
      for a subject (public; credentials are public claims)
- [ ] `POST /credentials` — authenticated write; attester asserts that subject
      holds a named credential; records attester's `author_id`, subject's `author_id`,
      credential type, and timestamp
- [ ] `GET /events?since=<cursor>` — emit attestation records as a `BeliefEvent`
      stream (newline-delimited JSON) consumable by `DocumentCompiler::on_belief_event`
- [ ] CORS headers (the static site and attestation server are different origins)
- [ ] Basic config: `COLLAB_ENDPOINT`, `JWKS_URL` / `JWT_SECRET`, `DATABASE_URL`

### 2. `noet-collab.js` client module (2 days)
- [ ] Reads `asset_version` and `site_url` from existing script tags on load
- [ ] On navigation (`hashchange`): collects `data-bid` attributes from visible
      DOM elements, fetches attestations from the configured endpoint
- [ ] Renders comment count badges, sign-off badges (current/stale/policy-satisfied),
      flag markers on BID-anchored elements
- [ ] Sign-off badge shows policy status: "2/3 required credentials signed off" when
      `policy_status` from the server indicates partial approval
- [ ] Right-click / toolbar UI: add comment, sign off, add flag (auth-gated)
- [ ] Sign-off flow: before submitting, client fetches signer's own credentials from
      `/credentials?subject=<my_author_id>`, displays which credentials they hold,
      and lets them select which credential satisfies this node's sign-off policy.
      The selected credential is included in the `SignOff` attestation payload.
- [ ] Credential attestation UI (separate panel): "Vouch for a colleague" — enter
      their `author_id`, select a credential type, submit to `POST /credentials`.
      No admin required; any authenticated user can attest credentials for any other user.
- [ ] JWT login flow: redirect to configured OIDC provider, store token in
      `sessionStorage`, attach as `Authorization: Bearer` header on writes
- [ ] Graceful degradation: if collab server is unreachable, no overlay rendered,
      no errors surfaced to user

### 3. noet-core template placeholder (0.25 days)
- [ ] Add `{{COLLAB_ENDPOINT}}` to `assets/template-responsive.html` after the
      viewer script tag — renders an empty string when not configured (no-op)
- [ ] Add `collab_endpoint: Option<String>` to the compiler's template
      substitution map; default `None`
- [ ] CLI flag: `--collab-endpoint <url>` on `noet parse` / `noet serve`

### 4a. BeliefEvent integration (0.5 days)
- [ ] Implement `DocumentCompiler::on_belief_event` to accept `BeliefEvent` chunks
      from the `/events` endpoint with `EventOrigin::Remote`
- [ ] Map attestation records to noet graph nodes: attestation → `NodeUpsert`;
      provenance link → `RelationUpdate` (Epistemic); boundary coverage →
      `RelationUpdate` (Pragmatic); use `BatchStart`/`BatchEnd` to frame each
      fetched page of events as a coherent atomic commit
- [ ] Verify that `get_traceability` and `check_consistency` return meaningful
      results over a beliefbase that includes attestation nodes

### 4b. Integration test (0.5 days)
- [ ] Script that: builds a small noet site, starts a local attestation server,
      verifies `GET /attestations` returns empty for fresh build, posts a
      comment and sign-off, verifies summary reflects them
- [ ] Verify stale sign-off detection: rebuild site (new `asset_version`),
      confirm summary returns `prior_version` for the prior sign-off
- [ ] Verify BeliefEvent stream: fetch `/events`, apply to a fresh beliefbase
      via `on_belief_event`, confirm attestation nodes appear in query results

---

## Credential Model

### Design Principle: Peer-Derived, Not Administrator-Assigned

Credentials are claims that one authenticated user makes about another. There is no
administrator who configures roles. Any authenticated user can attest that any other
user holds a credential; the weight of that attestation derives from the web of attesters,
not from a privileged admin account.

This is the same model as PGP's web-of-trust applied to professional credentials: "I,
Alice (Structures Lead), attest that Bob holds the credential `structures-engineer`."

### Credential Attestation Schema

```
CredentialAttestation {
    credential_id:   Uuid,         // stable ID for this attestation record
    attester_id:     AuthorId,     // who is making the claim (JWT subject)
    subject_id:      AuthorId,     // who the credential is being claimed for
    credential_type: String,       // e.g. "structures-engineer", "safety-reviewer"
                                   // free-form string; consuming app defines valid types
    issued_at:       SystemTime,
    revoked_at:      Option<SystemTime>,  // null = active; set to revoke
    note:            Option<String>,      // optional justification
}
```

Credential types are free-form strings. The collaboration server does not define
a fixed set — it stores whatever strings are used. The sign-off policy in the
document frontmatter defines which types are required; the server checks that the
presented credential's type matches what the policy requires.

### Sign-Off Policy in Document Frontmatter

The sign-off policy lives in the noet source document's frontmatter (TOML, YAML, or
JSON block). It is compiled into the rendered page's `data-signoff-policy` attribute
on the node's DOM element, where `noet-collab.js` reads it.

Example frontmatter policy:

```toml
[sign_off_policy]
required = [
    { credential = "structures-engineer", count = 1 },
    { credential = "safety-reviewer",     count = 1 },
]
# Optional: require attesters themselves to hold a meta-credential
# attester_credential = "team-lead"
```

The policy says: this node requires at least one sign-off from someone holding
`structures-engineer` AND at least one from someone holding `safety-reviewer`.
Neither requirement names a specific individual — only a credential type.

### Validation Flow at Sign-Off Time

```
1. Signer POSTs a SignOff attestation including:
   - their JWT (identifies author_id)
   - presented_credential_id: the CredentialAttestation UUID they are signing with

2. Server validates:
   a. JWT is valid → author_id extracted
   b. CredentialAttestation[presented_credential_id] exists, is not revoked,
      and subject_id == author_id  (signer can only present their own credentials)
   c. policy (from request body) requires a credential of type
      CredentialAttestation[presented_credential_id].credential_type
   d. If all pass: record attestation with policy_satisfied = true
      If (c) fails: record attestation with policy_satisfied = false
      (the sign-off is still recorded; it just doesn't count toward policy)

3. GET /sign-offs/summary computes policy_status by counting distinct
   credential_type buckets that have at least `count` policy_satisfied sign-offs
   at the current asset_version.
```

### Trust and Sybil Resistance

The system does not prevent a single user from attesting credentials for all of their
colleagues. Trust in the credential graph is a social and organizational concern, not
a technical one. For QMS contexts, the expectation is that credential attestations are
traceable — every credential record shows who vouched for whom — so organizational
review processes can audit the web of trust.

For stronger Sybil resistance in later phases, Keyhive's capability model can gate
who is permitted to issue credentials of a given type (e.g. only existing
`team-lead`-credentialed users can attest `team-lead` credentials). This is a Phase 2
concern; the schema supports it without changes.

---

## Backend Analytics (Internal Operator Tooling)

The attestation ledger is append-only and queryable. A set of backend analytics
queries — not exposed in the public API, available only to operators and
designated leadership — can be derived from the ledger data without any
additional instrumentation. These are framed as internal tooling deliberately:
widely advertising their existence would invite Goodhart's Law degradation of
the signals they surface.

**Attestation latency by boundary** — for each `path`, the median and p95 time
between a `BatchStart` event and the first `policy_satisfied = true` attestation
at the current version. High latency indicates a queue, a capacity constraint,
or unclear ownership at that boundary. In a well-organized team, latency is low
and spikes are traceable to specific external events.

**Yield rate by protocol** — for each `protocol_id`, the fraction of
`IndependenceCheck` attestations that return `result = fail` over a rolling
window. Near-zero yield over an extended period is evidence the protocol is not
calibrated to catch real problems. High yield is evidence of a real quality
signal. Comparison of yield rates for the same protocol across different teams
receiving artifacts from the same upstream source surfaces calibration
discrepancies worth investigating.

**Credential footprint** — which `credential_type` values appear in the most
boundary policies, and which `attester_id` values hold those credentials. This
is the informal authority map of the organization made legible: concentration
in a small number of credential types or attesters indicates de facto gatekeepers
operating without explicit organizational sanction.

**Provenance chain depth distribution** — for `SignOff` attestations, the
distribution of `provenance` citation counts. A team whose sign-offs
consistently cite zero provenance records is either working from deep expertise
(legitimate) or signing off on faith (not). The distribution across teams, and
its change over time, is the signal worth monitoring.

**Goodhart's Law containment.** The yield rate is the hardest of these metrics
to game sustainably. Inflating sign-off volume is easy; engineering a plausible
false-positive rate on independence checks over time requires the checks to
actually run and sometimes fail. Yield rate is therefore the metric most worth
watching and the one least worth advertising. The others are useful leading
indicators but are more susceptible to surface-level gaming once known to be
measured.

These analytics are not success criteria for Phase 1. They are latent
capabilities of the ledger that become meaningful once a sufficient number of
boundaries have been instrumented. Implementation is a single read-only query
layer over the existing `attestations` table — no schema changes required.

---

## Testing Requirements

- Attestation round-trip: POST a `Comment`, `SignOff`, and `Flag`; GET returns
  all three keyed correctly on `(site_url, asset_version, bid)`.
- Sign-off summary: node with current-version sign-off from a policy-satisfying
  credential returns `policy_status` showing that bucket satisfied; node with
  prior-version sign-off returns `prior_version: <old>`.
- Credential round-trip: Alice POSTs a CredentialAttestation asserting Bob holds
  `structures-engineer`; `GET /credentials?subject=bob` returns that attestation.
- Policy validation at sign-off: Bob presents his `structures-engineer` credential
  when signing off on a node whose policy requires `structures-engineer`; server
  records `policy_satisfied = true`. Bob presents a `safety-reviewer` credential on
  the same node; server records `policy_satisfied = false` (wrong type for the slot).
- Revocation: Alice revokes Bob's credential attestation; Bob's subsequent sign-offs
  with that credential record `policy_satisfied = false`; his prior sign-offs are
  unaffected (append-only log; revocation is prospective not retroactive).
- Auth enforcement: unauthenticated POST returns 401; authenticated GET returns
  200 without auth header (reads are public).
- CORS: browser can fetch from a different origin than the static site.
- Graceful degradation: `noet-collab.js` loads without error when collab server
  is absent; no console errors, no UI breakage.
- Template placeholder: a site built without `--collab-endpoint` loads and
  functions identically to today; no extra script tag emitted.

---

## Success Criteria

- [ ] `collaboration_overlay.md` upgraded to Draft with ❓1 and ❓3 resolved and
      credential model documented
- [ ] `noet-collab` server starts, accepts JWT-authenticated attestation writes,
      validates presented credentials against sign-off policy, and serves sign-off
      summary for a known `(site_url, asset_version, bid)`
- [ ] `GET /credentials` and `POST /credentials` endpoints operational; credential
      attestation round-trip works end-to-end
- [ ] `noet-collab.js` renders comment/sign-off/flag overlays on a live static
      noet site without modifying the site's served files
- [ ] Sign-off badge correctly distinguishes: current-version + policy-satisfied (green),
      current-version + policy-not-satisfied (yellow), prior-version/stale (amber)
- [ ] Sign-off flow shows signer their available credentials and which policy slot
      each satisfies before they submit
- [ ] `{{COLLAB_ENDPOINT}}` placeholder is a no-op when empty — existing sites
      unaffected
- [ ] Integration test passes end-to-end (build → vouch → sign → verify policy_status)

---

## Risks

- **Identity model complexity**: JWT is sufficient for Phase 1. `author_id` is
  opaque; Keyhive replaces JWT validation in Phase 2 without schema migration. ✅ Resolved.
- **Credential Sybil attack**: a single user can vouch for all their colleagues,
  inflating the web of trust. → **Mitigation**: The credential graph is fully auditable
  (every attestation records who vouched for whom); organizational review processes
  can inspect it. Phase 2 can gate credential issuance on holding a meta-credential
  (e.g. only `team-lead`s can issue `team-lead` credentials) using Keyhive capabilities.
- **Policy drift**: a document's sign-off policy may change after sign-offs are
  recorded. Old sign-offs were valid against the old policy. → **Mitigation**: policy
  is embedded in the attestation POST body (snapshot at sign-off time) and stored
  alongside the record. The summary endpoint re-evaluates against the current policy
  from the request; divergence is surfaced as `policy_status: stale_policy`. Accept
  as correct behavior — policy changes should prompt re-approval.
- **Cross-origin script trust**: Loading `noet-collab.js` from a different
  origin requires the operator to trust that origin. A compromised collab
  server could inject arbitrary JS. → **Mitigation**: Document clearly that
  the collab endpoint must be operator-controlled; never use a third-party
  hosted instance.
- **`asset_version` coarseness**: Any content change anywhere invalidates all
  sign-offs, even for unchanged nodes. → **Mitigation**: The stale sign-off
  UI surface ("approved at prior version") makes this visible and actionable
  rather than silent; accept as correct behavior for Phase 1.
- **Scope creep toward real-time / redlines**: Both are large scopes that
  should stay out of Phase 1. → **Mitigation**: Explicitly out of scope in
  the design doc; resist adding WebSocket or diff payload support until
  Phase 1 is validated.

---

## Open Questions

- **❓1** ✅ **Resolved**: JWT Phase 1 is sufficient. `author_id` is opaque; Keyhive
  replaces validation in Phase 2 without schema changes.
- **❓2** ✅ **Resolved**: `hashchange` + DOM observation is sufficient for Phase 1.
- **❓3** ✅ **Resolved**: policy lives in document frontmatter (not server config, not
  client-only). Server validates presented credentials against policy at POST time and
  records `policy_satisfied`. Summary endpoint derives `policy_status` from stored
  records. Policy is version-controlled alongside the document.

- **❓4** (new) **Credential type namespace**: credential type strings are free-form.
  Should the collab server enforce a per-site whitelist of valid credential types
  (configured at server startup), or remain fully open? Open whitelist is simpler and
  avoids a config-management burden; typos produce harmless uncounted sign-offs.
  **Recommendation**: open for Phase 1; add optional whitelist config in Phase 2.

- **❓5** (new) **Attester trust threshold**: should the sign-off summary count a
  credential as valid only if it has been attested by N ≥ 2 distinct peers (quorum),
  or is a single peer attestation sufficient? Single-peer is simpler and consistent
  with how most web-of-trust systems work for professional vouching.
  **Recommendation**: single-peer attestation sufficient for Phase 1; quorum threshold
  as an optional per-credential-type config in Phase 2.

- **❓6** ✅ **Resolved**: Policy inheritance for auto-generated section nodes follows
  this chain: node's own frontmatter → parent document frontmatter → network
  `index.md` frontmatter → no policy (`policy_status: no_policy`; sign-offs accepted
  but not counted toward any requirement). This must be implemented in
  `noet-collab.js` before the sign-off UI ships — without it, section nodes silently
  surface `no_policy` when users expect inherited policies.

- **❓7** (new) **Quorum design for Phase 2 credential validation**: single-peer
  attestation is sufficient for Phase 1. For Phase 2, if a quorum threshold is added
  (credential valid only if attested by N ≥ 2 distinct peers), the following must be
  specified: what counts as "distinct" (same org? same device? same SSO provider?),
  whether the threshold is global or per-credential-type, and how existing single-peer
  credentials are treated at migration time.

- **❓8** (new) **`on_belief_event` wiring design**: the integration between the
  attestation server's `/events` endpoint and `DocumentCompiler::on_belief_event` is
  described in §4a of Implementation Steps, but the following must be specified before
  implementation: cursor/`since` parameter semantics (wall-clock? sequence number?
  per-server opaque token?), error handling when the event stream is interrupted
  mid-batch, and whether the wiring is pull (compiler polls `/events`) or push
  (server sends SSE/WebSocket). Recommendation: pull with sequence-number cursor,
  consistent with `peer_watermarks` existing sync model.

- **❓9** (new) **Assertion/Verification dual-role case**: when a CI/CD pipeline both
  produces and verifies its own output (e.g. a build system that compiles and then
  runs its own tests), it occupies both the Assertion producer role and the
  Verification consumer role at the same boundary. A worked example in the IIC entry
  template is needed before practitioners can classify real boundaries correctly.
  **Recommendation**: treat producer and consumer as distinct IIC entries sharing the
  same artifact anchor; the pipeline posts two attestation records (one Assertion,
  one Verification) rather than one combined record.

---

## Extension Path

The Phase 1 anchor (`path`, `version`), credential model, provenance chain, and
federation sync primitives are designed from day one to generalize beyond noet
document nodes to arbitrary fingerprintable artifacts — firmware images,
configuration files, telemetry schemas, requirements documents — while supporting
mixed human and machine attesters and a shared independence protocol registry.

The generalized design, prior art analysis (in-toto, SLSA, Sigstore, W3C VC,
noet `BeliefEvent` stream), build-vs-adopt recommendations, and the relationship
between the attestation fabric and the noet DAG model are specified in
`docs/design/attestation_fabric.md`.

**What remains noet-specific in Phase 1:** the `noet-collab.js` client, the
`{{COLLAB_ENDPOINT}}` template placeholder, and the DOM overlay. The server
is substrate-agnostic from its first commit.