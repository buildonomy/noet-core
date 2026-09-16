---
title = "Content Identity: Stable Node Identity Across Moves and Reformatting"
authors = "Andrew Lyjak, Claude"
last_updated = "2026-09-03"
status = "Draft"
version = "0.1"
dependencies = ["content_versioning.md", "link_format.md", "beliefbase_architecture.md"]
---

# Content Identity

> [!NOTE]
> **Target architecture. Nothing specified here is implemented.** Issue 36
> (`../project/0_open/ISSUE_36_SECTION_BID_MIGRATION.md`) is the implementation
> vehicle and owns the consumers — move detection and shared-section
> unification. This document owns the hash they consume.

## 1. Purpose

A compiled corpus needs to answer two questions about a node, and they are not
the same question.

| Question | Answered by | Specified in |
|---|---|---|
| **"Did this change?"** | `metadata["_content_hash"]` | `content_versioning.md` §5.1 |
| **"Is this the same thing?"** | `metadata["_identity_hash"]` | this document |

`content_versioning.md` is this document's sibling. Versioning detects change so
that a claim about a node — a review, an attestation, a cached derivation — can
be told it is stale. Identity recognizes *sameness across change of location*, so
that a section cut from one document and pasted into another keeps its BID and
its inbound links.

The two requirements pull in **opposite directions**, and that is the reason
there are two hashes rather than one:

- **Identity must survive incidental change.** Surviving reformatting is the
  entire point — a section that acquires a different line-wrapping on the way
  into its new home is still the same section.
- **Staleness must notice change.** Anything the versioning hash normalizes away
  is an edit that an annotation will silently fail to notice, so its
  normalization budget stays minimal.

Collapsing them into one key would force a single normalization policy onto two
requirements that pull apart, and whichever requirement lost would fail silently.

**Scope.** This document owns the identity hash: where it is stored, what field
set it covers, and how content is normalized before hashing. It does not own
version computation (`content_versioning.md`), the link syntax it normalizes
(`link_format.md`), or the BID/bref identity scheme it depends on
(`beliefbase_architecture.md` §2.2).

**The model.** A section's identity is a hash of its normalized content — title
plus body text. The hash is computed during `IRNode` construction and stored in
`node.metadata["_identity_hash"]`. **The hash is the identity; location is merely
where a node is anchored.** That single premise generalizes move detection and
copy-unification into one mechanism.

## 2. Placement: `metadata["_identity_hash"]`

The obvious placement — `node.payload["content_hash"]` — is wrong on two counts,
and the name was already taken twice over.

### 2.1 Three claimants on "content hash"

| | Key | Hashes | Serves | Status |
|---|---|---|---|---|
| **A** | `payload["content_hash"]` on asset/directory nodes | raw external bytes | asset dedup | shipped, but **misplaced** — relocating to `metadata` (Issue 105 step 3a-pre); see `content_versioning.md` §5.1 |
| **B** | `metadata["_content_hash"]` | the node's content-bearing fields | **staleness** | specified in `content_versioning.md` §5.1 |
| **C** | `metadata["_identity_hash"]` | normalized title + body | **identity** | this document |

**C is the one that breaks things.** Two independent faults rule out the naive
placement.

### 2.2 Fault 1: C in `payload` is circular — twice over

`content_versioning.md` §5.1 hashes `payload`, so putting C there means **B
hashes C**.

Worse, C is circular on its own terms. C derives the BID; a section's `id`,
absent an explicit anchor, derives from the title; and on anchor collision it
derives from the **bref**, which derives from the **BID**
(`src/codec/builder.rs:2410-2432`; `BeliefNode::collision_aware_id`,
`src/properties.rs:1262`). That is BID → `id` → hash → BID.

Moving the hash to `metadata` breaks the first loop by construction — §5.1
excludes `metadata` from the hash — and narrowing the field set (§2.4) breaks the
second.

### 2.3 Fault 2: C cannot simply reuse B

B includes `id` in its hash; `content_versioning.md` §5.1 states explicitly that
a rename is meaningful. But a section moved into a document where its anchor
collides receives `NodeId::Collision` and therefore a different `id` and a
different B.

**Move detection built on B fails precisely when the destination document already
contains a section with that anchor** — not a rare case, and exactly the scenario
identity hashing exists to handle.

### 2.4 The field set

`_identity_hash` lives in `metadata`, alongside `_content_hash` under the same
underscore convention for compiler-internal keys, but as a **distinct key** with
a **deliberately narrower field set**:

| Field | In `_content_hash` (B) | In `_identity_hash` (C) | Why C differs |
|---|---|---|---|
| `title` | yes | yes (normalized) | — |
| `payload` body text | yes | yes (normalized) | — |
| `id` | yes | **no** | collision-derived; see §2.3 and §4.5 |
| `schema` | yes | **no** | a section's identity is its content, not its validation contract |
| `sections` table | via `payload` | **no** | child anchoring is location, not identity |

Both keys coexist; neither is derived from the other. `_content_hash` is not a
substitute for `_identity_hash`, for the reason in §2.3.

**Algorithm**: SHA256, already a project dependency via `sha2` — no reason to add
Blake3 for this. The hash is computed in the codec during `parse()`, before BID
assignment, and stored in both `IRNode` and the persisted `BeliefNode`.

**Round-tripped form.** The `sections` frontmatter table carries the hash back
into the source file so identity survives a session boundary:

```toml
[sections.installation]
bid = "section-1234"
id = "installation"
identity_hash = "sha256:abc123..."
```

That is authoring state in the sense of `content_versioning.md` §5.1a. It is not
`_content_hash`, and it is not the asset-node `payload["content_hash"]`.

## 3. Normalization

Identity survives reformatting. That requires a normalization pass, and it is
this hash's alone — `_content_hash` deliberately does not normalize beyond the
minimum (§1).

### 3.1 Hash an intermediate, not a string

Every node is `(properties, text)`, and **all node text passes through
`pulldown_cmark`**. The most stable identity is therefore a hash of an
intermediate form — the content-bearing properties plus the **cmark token
stream** — rather than a hash of normalized source text.

Why this beats string normalization:

- **Formatting differences that carry no semantics disappear for free.** `*em*`
  vs `_em_`, setext vs ATX headings, and reference vs inline links are different
  bytes but identical token streams. A string normalizer would need a bespoke
  rule per case; the token stream needs none.
- **It cannot silently erase a real distinction.** A naive `trim_end` collapses
  the two-space hard break, making two visually-different documents hash equal.
  The token stream keeps `SoftBreak` and `HardBreak` distinct because the parser
  already had to distinguish them.
- **No second grammar.** String-level markdown normalization means reimplementing
  the parser's judgments in a regex-shaped way, and the two will drift.

### 3.2 Residual string normalization, inside tokens

A string pass is still needed for the *text inside* tokens: line endings
(CRLF→LF), trailing whitespace, and Unicode NFC.

**Do not reuse `to_anchor`** (`src/paths/path.rs:169`). It uses **NFKC**, which is
correct for slugs and wrong here — it folds `①`→1 and full-width forms, erasing
characters an author may have chosen.

## 4. References: the injected `bref://` link title

The hardest part of the design, and the reason the normalizer cannot be a text
filter.

When `MdCodec` resolves a link it **rewrites the source**, injecting a title
attribute of the form `[text](path.md#anchor "bref://abc123 {config} words")`
(`src/codec/md.rs:894`, `:924`). The bref is derived from the *target node's BID*
(`format!("bref://{}", relation.other.bid.bref())`).

### 4.1 The title has a genuinely mixed character

| Component | Character |
|---|---|
| The link text and target path | **Authored** — unambiguously content |
| Trailing user words in the title | **Authored** |
| `bref://…` | **Compiler-injected**, derived from the target's BID |
| `{config}` / `rel` pairs | **Derived** from the matched relation's weight kinds |

It round-trips into the source file, so it is textually indistinguishable from
authored content by the time anything reads the file back.

### 4.2 The bref must be included, and the path must not be the fallback

The bref exists *precisely so the reference is more stable than the path* — it is
what lets a link survive the target moving. Substituting "path + anchor" as a
BID-independent stand-in would invert the design: it makes identity depend on the
least stable component available, and it would break on exactly the moves
identity hashing exists to handle. **A hash that changes whenever a link target
relocates is not an identity hash.**

`{config}` and the `rel` pairs are excluded regardless. They are derived from the
matched relation's weight kinds (`src/codec/md.rs:940-965`), so they restate graph
structure the hash should not carry — and unlike the bref, nothing is lost by
dropping them.

### 4.3 The dependency chain terminates

Including the bref is not circular in the sense §2.2 rejects. That circularity is
*intra-node*: `payload` would contain the node's own hash, so hashing `payload`
hashes the hash. Here the dependency is **inter-node** — `_identity_hash(A)`
depends on `bref(B)`, which is `BID(B)`, not `BID(A)`. `content_versioning.md`
§5.1 also excludes `bid` from the hash, so a node never hashes its own identifier.

So the question is not "is this self-referential" but **"does the dependency chain
terminate?"** It does, and there is **no ordering constraint**:

```
_identity_hash(A)  →  BID(B)  →  _identity_hash(B)  →  B's own content  ── terminates
```

The chain bottoms out because **`_identity_hash(B)` does not depend on
`BID(B)`** — §5.1 excludes `bid`, and the collapse rule (§5) hashes B's *own*
content, never its identifier. Even for two new, mutually-linked nodes: A needs
B's BID, B's BID needs B's content, and B's content is on disk. Both resolve.
**There is no cycle to break and no tiebreaker to invent.**


**And the precondition is self-evidencing.** If a bref can be injected, the
target's BID *already exists* — that is what injection means
(`format!("bref://{}", relation.other.bid.bref())` runs only on a resolved
`relation`). The bref's presence is the evidence that resolution completed, so
there is nothing to sequence.

### 4.4 Identity is a function of `(content, corpus resolution state)`

A section's identity hash changes when one of its references resolves for the
first time. That is a **version change**, exactly like a content edit — not an
ill-definedness. Since `_identity_hash` lives in `metadata`, a version change is
the ordinary case the system already handles.

This convergence is already load-bearing elsewhere: the compiler re-queues files
with unresolved references when corpus state may now resolve them, with
`permanently_unresolved` (`src/codec/compiler.rs:194-209`) as the fixpoint guard
against re-parse storms. "Identity sharpens as resolution improves" is that same
behaviour, and the fixpoint is already bounded.

### 4.5 Two consequences

Neither blocks the design; both are easy to get wrong later.

> **1. An identity hash is only comparable against one taken at equal-or-better
> resolution.** Two parses that reached different resolution states — one where a
> reference resolved, one where it did not — produce different hashes for
> identical content. That is correct behaviour, but it means move detection must
> not diff hashes across a partial or interrupted parse and conclude a section
> moved. **Compare only at a settled fixpoint.**

> **2. Excluding `id` from `_identity_hash` is load-bearing for a second
> reason.** `builder.rs:2410-2432` resolves anchor collisions first-one-wins, and
> `collision_aware_id()` then returns the **bref** — so a *losing* node's `id`
> derives from its own BID. Including `id` would therefore reintroduce genuine
> *intra-node* circularity (BID → id → hash → BID) for exactly those nodes. §2.4
> excludes `id` because identity and staleness want different field sets; it
> stays excluded for this reason too. **Do not "helpfully" add it back for
> disambiguation.**

## 5. The link-collapse rule

Masking the `title` attribute alone is not enough, because `auto_title` also
rewrites the link *text* (§6). Both problems have one fix.

**Normalize a link to a single token carrying its strongest cross-repo
identifier.** The normalizer walks the token stream, and on each
`Start(Tag::Link)` it consumes through the matching `End(TagEnd::Link)` and
replaces the entire slice with one `Text(node_key.to_string())`.

### 5.1 Which identifier — the two reference populations

The identifier is chosen by availability, which is exactly the distinction
between the two populations of reference:

| Reference population | Identifier used |
|---|---|
| **Resolved** — the compiler located the target and injected `bref://…` into the title | `NodeKey::Bref` — the strongest available, and stable across moves |
| **Unresolved** — no bref present (never resolved, external, or broken) | The `NodeKey` the link's own syntax denotes |

So a link normalizes to the best identity the corpus currently supports, and
improves automatically when a previously-unresolved reference later resolves.

### 5.2 The infrastructure exists; this is composition, not new machinery

- **Bref extraction** — `parse_title_attribute` (`src/codec/md.rs:469`) already
  scans the title for `bref://` and `bid://` and returns a `Bref`
  (`md.rs:494-503`). It also parses out the `{config}` blob, so `auto_title` and
  `rel` are identified in the same pass.
- **Fallback key derivation** — `link_to_relation` (`src/codec/md.rs:257-309`) is
  already a pure function from `(link_type, dest_url, title, id)` to
  `Vec<NodeKey>`, handling every `LinkType`: autolink, email, inline, wikilink
  (with its id-then-path fallback pair), reference, collapsed, and shortcut.
  Nothing needs reimplementing.
- **Serialization** — `NodeKey`'s `Display` (`src/nodekey.rs:515-545`) emits the
  canonical forms (`bref:…`, `bid:…`, `id:…`, `path://net/…`), and it already
  normalizes content-namespace paths to a bare form for clean round-tripping.
- **Slice-walking** — `check_for_link_and_push` (`src/codec/md.rs:558`) already
  walks a link's `Start`-to-`End` span with a `stop_event`. Its subroutines are
  the model for the traversal.

### 5.3 Determinism: first key wins

Where `link_to_relation` returns multiple candidate keys (wikilinks return an `Id`
and a `Path`), the normalizer must pick deterministically — **first key wins**,
matching the key order `link_to_relation` already returns. Do not hash the whole
vector; a later change to the fallback list would silently move every hash.

### 5.4 Why this resolves both problems at once

- **The title attribute disappears** — bref, `{config}`, `rel`, and user words are
  all inside the replaced slice. No separate masking rule, and the `{config}`/`rel`
  exclusion argued in §4.2 happens for free.
- **`auto_title` stops propagating** — the link *text* is inside the slice too, so
  substituting the target's title no longer changes the hash. Renaming a node no
  longer churns the identity of every node that links to it.
- **The dependency becomes explicit and minimal.** Identity depends on the
  target's *identifier*, not on a rendered string that happens to contain it.

### 5.5 What it costs

**Authored link text stops contributing to identity.** "See [the spec](a.md)" and
"See [the overview](a.md)" become identical. That is probably right — both cite
the same target, and the wording is presentation — but it is a deliberate
narrowing, not a free win.

It also means a section whose *only* difference from another is its link wording
will unify. Combined with the boilerplate hazard Issue 36 records, that is an
argument for shipping move detection before unification.

**The bref dependency survives, and should.** The bref is what lets a reference
outlive a move. Collapsing to `NodeKey::Bref` makes the identity hash depend on
the target's BID — which is §4.4's resolution-state property, unchanged. The
collapse removes the *string* coupling, not the *identity* coupling.

## 6. Why link *text* forced this — the `auto_title` coupling

Recorded because it is the constraint that rules out the simpler fixes, and
because anyone tempted to "just mask the title attribute" will hit it.

When `auto_title` is set, the codec replaces the **link text itself** with the
target's current title (`src/codec/md.rs:975-982` — `new_link_text =
relation.other.title.clone()`). Link text is ordinary `Text`, indistinguishable
from prose. So without the collapse in §5:

> **Renaming node B would change the body text of every node linking to B with
> `auto_title` — and therefore change their `_identity_hash`.**

A rename would propagate identity churn across every referrer: sections that did
not move, whose authors changed nothing, would stop matching their persisted
hashes. For move detection that is a false negative, and a common one.

**And `auto_title` is *inferred*, not merely declared.** `md.rs:928-938` sets it
true when the user wrote it explicitly **or** when the link text happens to equal
the target's title. A hand-typed link that coincidentally matches the target's
wording is silently enrolled into auto-updating. The coupling is opt-out by
accident rather than opt-in by intent, and its extent is not visible from the
source text — which is why a rule targeting only *explicitly* auto-titled links
would have missed most of it.

Collapsing the whole `Start`-to-`End` slice handles this without a special case:
the substituted text is inside the slice, so it never reaches the hash. **This is
the main reason the collapse is the right shape** rather than a narrower mask.

### The general form of the problem

Both the title attribute and the link text are instances of one thing: **the
codec rewrites source, so "the authored content" and "the content on disk" are
not the same string.** Any identity hash must decide which one it means. This
design answers: neither — it hashes the *resolved identity* the rewriting was
trying to encode.

That question belongs to identity alone. No other consumer of node content has to
care, which is why `_content_hash` stays simple while `_identity_hash` cannot.

## 7. Consumers

Identity hashing exists to serve two capabilities, both specified in Issue 36:

| Capability | What it does with hash equality |
|---|---|
| **Move detection** | A deleted node's hash reappears on a created node → migrate the BID rather than deleting and re-minting |
| **Shared-section unification** | Two *live* nodes share a hash → emit one node with a `Section` edge from each parent |

The second is **deferred pending design** — hash equality does not distinguish a
genuine shared section from replicated boilerplate, and merging two editable
nodes carries a destructive-propagation hazard. Nothing in this document depends
on that resolution; the hash is the same either way.

Prior art for the mechanism ships today over a different substrate:
`src/codec/compiler.rs:5084-5096` deduplicates **assets** on content-hash
collision, reusing the canonical node and emitting a `ParseDiagnostic::info`. The
salient difference is that an asset is immutable content addressed by its bytes,
while a section is editable — which is why unification is safe there and not here.

## 8. Open Questions

- **Is the `pulldown_cmark` token stream reachable and stable?** §3.1 assumes the
  token stream is available at the point identity is computed, and that it is
  stable across the `pulldown_cmark` version range noet supports. A token-stream
  hash that changes on a dependency bump would migrate every BID in the corpus —
  far worse than the problem identity hashing solves. If stability cannot be
  guaranteed, string normalization is the fallback, and the reason should be
  recorded here.

- **Href-aliased links.** `src/codec/md.rs:894-906` keeps the original URL rather
  than rewriting to a document-relative path. Whether these carry a bref at all
  is unconfirmed, and if they do not, what their identity contribution should be
  is unspecified.

- **What "settled fixpoint" means operationally.** §4.5 requires that hashes only
  be compared at a settled resolution fixpoint. `permanently_unresolved`
  (`compiler.rs:194-209`) bounds the fixpoint, but the predicate a consumer should
  test before trusting a hash comparison is not defined.

- **Node deletion and orphaned anchors.** An annotation anchored directly to a
  node whose BID migrates — or to a deleted node — has no specified behaviour.
  The anchor is intact; its target is not, which is a different condition from
  staleness and wants a different presentation. It arises only for stores that
  accept promotion, since a regenerated-scope record does not outlive the parse
  that wrote it — so it is a UX question owned by a follow-on to Issue 105.
  `content_versioning.md` §8 raises the structurally identical case from the
  staleness side; answer them together.

## 9. References

- `docs/design/identity/content_versioning.md` §5.1, §5.1a — `_content_hash`, and the rule
  that hashes live in `metadata` rather than `payload`
- `docs/design/identity/link_format.md` — link syntax and the title-attribute convention
- `docs/design/identity/section_metadata_manifest.md` — the `sections` table this hash
  round-trips through
- `src/codec/md.rs:257-309` — `link_to_relation`, the `NodeKey` fallback derivation
- `src/codec/md.rs:469`, `:494-503` — `parse_title_attribute`, bref extraction
- `src/codec/md.rs:558` — `check_for_link_and_push`, the slice-walking model
- `src/codec/md.rs:894`, `:924`, `:940-965`, `:975-982` — title injection,
  `{config}`/`rel` derivation, `auto_title` text substitution
- `src/codec/md.rs:894-906` — href-aliased links
- `src/codec/md.rs:928-938` — `auto_title` inference
- `src/nodekey.rs:515-545` — `NodeKey::Display` canonical forms
- `src/codec/builder.rs:2410-2432`, `src/properties.rs:1262` — anchor collision and
  `collision_aware_id`; the BID → `id` → hash → BID circularity
- `src/codec/builder.rs:4344`, `:4493`, `:4799` — asset/directory
  `payload["content_hash"]` (claimant A)
- `src/codec/compiler.rs:194-209` — `permanently_unresolved`, the resolution fixpoint
- `src/codec/compiler.rs:5084-5096` — shipped asset hash-dedup
- `src/paths/path.rs:169` — `to_anchor`, NFKC, and why not to reuse it
- `docs/project/0_open/ISSUE_36_SECTION_BID_MIGRATION.md` — the implementation
  vehicle and the consumers
