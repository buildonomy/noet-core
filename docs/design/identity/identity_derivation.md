---
title = "Identity Derivation: Minted, Derived, and Reserved Identities"
authors = "Andrew Lyjak, Claude"
last_updated = "2026-09-09"
status = "Draft"
version = "0.1"
dependencies = ["beliefbase_architecture.md", "content_identity.md", "content_versioning.md"]
---

# Identity Derivation

## 1. Purpose

Every durable object in noet needs a name that is the same name tomorrow. This
document states **where identities come from**, which is a different question
from what they mean (`beliefbase_architecture.md` §2.2, the `NodeKey` variants)
or when two of them denote the same content (`content_identity.md`).

There are exactly two ways to obtain an identity, and the choice has one
consequence that dominates every other:

| | **Minted** | **Derived** |
|---|---|---|
| Source | wall clock + entropy | a pure function of inputs |
| Same inputs, second run | a **different** identity | the **same** identity |
| Survives a rebuild | only if persisted | always |
| Example | `Bid::new` (`src/properties.rs:239`) | `buildonomy_api_bid`, `Bid::codec_namespace` |

> **The rule.** An identity that must survive a rebuild must be a **pure
> function of something that survives a rebuild** — or it must be persisted, and
> the persistence path must be part of the design rather than a convention.

The rule is stated here because it has been rediscovered three times in
different subsystems, each time as a separate bug. A minted identity that nobody
persists is not stable; it merely *looks* stable in a single-process test, which
is precisely the condition under which it is always tested.

## 2. Why minting is the default, and where it stops working

`Bid::new` mints: `Uuid::now_v6(parent.bref().bytes())` draws the timestamp from
the system clock, so **two parses of an unchanged file produce two BIDs**. This
is correct for the common case. A node the compiler can resolve against a prior
store keeps its BID because `cache_fetch` returns the stored one; minting is the
fallback for a node nobody has seen before.

The failure appears wherever the store is absent or bypassed:

- **A generated corpus** carries no `bid:` frontmatter, and a regeneration
  discards whatever `--write` injected.
- **A cold parse** — CI with no at rest storage — resolves nothing, so every
  node is new.
- **An unresolvable reference** mints on the spot (`src/codec/builder.rs:2303`,
  `:4820`).

In each case every BID changes on every build, so anything anchored to a BID is
orphaned by the next build. Shard hydration before parse is the durable identity
store that closes this (Issue 66); the point here is that **the mint is not the
bug** — the missing persistence path is.

`Bid::default()` is worth calling out separately: it is `Bid::new(Bid::nil())`
(`src/properties.rs:348`), so a "default" BID is freshly minted rather than a
stable sentinel. Use `Bid::nil()` when a placeholder is wanted.

## 3. Derived identities: the v5 pattern

Where an identity must be reproducible, noet derives it with UUIDv5 over a
stable string and then stamps octets 10-15 with a namespace bref. Four
implementations share the shape (`src/properties.rs`):

| Function | Derived from | Stable across |
|---|---|---|
| `buildonomy_api_bid(version)` | the crate version string | every build of that version |
| `buildonomy_asset_bid(hash)` | the asset's content hash | every corpus holding those bytes |
| `buildonomy_href_bid(url)` | the URL string | every corpus citing that URL |
| `Bid::codec_namespace(term)` | `to_anchor(term)` | every binary, in- or out-of-tree |

Two properties are worth naming because they are easy to lose in a
reimplementation:

- **The input must be normalized.** `codec_namespace` runs `to_anchor` first, so
  casing and separator differences cannot produce two namespaces for one logical
  term. `to_anchor` is required to stay a pure fixed function for exactly this
  reason (`beliefbase_architecture.md` §2.2).
- **`buildonomy_href_bid` hashes the address, not the content.** Fetching remote
  content is expensive and unstable, so URL identity is the hash surface. This
  is a deliberate departure from `buildonomy_asset_bid`, which hashes bytes.

## 4. The packed namespace, and what it is for

A `Bid` is a UUIDv6 whose octets 10-15 hold the v6 `node_id` field, which noet
fills with the **bref of the BID it was generated from** — the low 48 bits of a
v5 hash of that BID (`Bid::bref`). The high bits carry time. So a BID is a packed
composite: *when* in the high bits, *whose* in the low.

> **Call that the derivation parent, and keep it distinct from containment.**
> The word "parent" is accurate here in a way it is not elsewhere in noet: a
> derived BID genuinely descends from the one that generated it, which is why
> `dag_model.md` §2 bans `parent` for graph edges and exempts BID lineage.
>
> **The trap is that for a section node the two coincide, pointing opposite
> ways.** `builder.rs` generates a section's BID from its containing node, so
> that node is simultaneously the section's *derivation parent* and the **sink**
> of its Section edge — containment runs contained → container, while derivation
> runs container → contained. A reader who learns the graph rule and then meets
> `parent_bref` will infer the containing node is a source. It is not.
>
> They are not even always the same node. `Bid::new(asset_namespace())` derives
> an asset's BID from a namespace root that contains it in no graph sense at all.
> Derivation parent answers *where did this identity come from*; containment
> answers *what holds this node*. Use the full phrase whenever both are in
> scope.

The packing exists to serve three comparison-only questions on hot paths:

| Consumer | Question | Implementation |
|---|---|---|
| `Bid::is_parent_filter` | is this node mine? | `src/properties.rs:299` |
| `Bid::is_reserved` | is this a system BID? | `src/properties.rs:255` |
| `partition_graph` | which network owns this node? | `src/shard/export.rs:473` |

**The bref is lossy and one-way.** It is a truncated hash, so membership can be
tested but the parent cannot be recovered. That is sufficient for all three
consumers and is why 48 bits suffice.

> **When to reuse this pattern.** Packing a discriminator into an identifier
> earns its complexity when the discriminator is (a) tested for membership
> rather than read back, (b) tested on a hot path over many objects, (c) not
> already carried as a separate field, and (d) **a function of identity rather
> than of position**. Absent all four, use a struct field: it costs a few bytes
> and cannot be misread. A packed field in an *immutable* artifact is especially
> unforgiving, because a field width chosen wrongly cannot be widened without
> rewriting every stored object.
>
> **(d) is what rules out the tempting variant.** Packing the encapsulating
> *network* would make network lookup a field read rather than a memoized map,
> but `node_to_nets` is a multimap — a node can belong to several networks
> directly, so one packed value cannot represent membership and the map is needed
> regardless. Re-homing a node would also leave the packed value permanently
> stale, because a BID is immutable and position is not.
>
> By the same test, the current value clears (a) through (c) only for
> reserved-namespace nodes; for the general population the derivation parent is a
> *position at generation time* that nothing afterwards may rely on. Whether that
> is worth the bytes is **Issue 113**, in the backlog.

## 5. Reserved namespaces

Four namespace constants are defined (`src/properties.rs:90-113`), and
`const_namespaces()` enumerates them. All system BIDs fall inside one, so user
content cannot collide with system content.

| Constant | First byte | Network | Holds |
|---|---|---|---|
| `UUID_NAMESPACE_BUILDONOMY` | `0x6b` | the API node | the data-model version node |
| `UUID_NAMESPACE_HREF` | `0x5b` | Href | external `http`/`https` targets, keyed by URL |
| `UUID_NAMESPACE_ASSET` | `0x4b` | Asset | unparsable embedded resources, keyed by content hash |
| `UUID_NAMESPACE_CODEC` | `0xff` | — | codec-registered secondary indices (C++ include paths, slug resolution) |

**`UUID_NAMESPACE_CODEC`'s first byte is `0xff` deliberately.** BIDs sort
high-bytes-first, so codec-namespace BIDs sort after every content network,
which makes `partition_graph`'s `or_insert` assign nodes to content networks
first. This is load-bearing ordering encoded in a constant — do not renumber it.

`content_namespaces()` is the subset (Href, Asset) that tracks external content
anchored to the parsed repo; `BeliefNode::keys` uses it to emit `NodeKey::Path`
rather than `NodeKey::Id` for those nodes, because their ids are locations
(`src/properties.rs:1290-1296`).

Reserved identifiers are enforced at parse time: a user-supplied `bid` inside a
reserved namespace is rejected (`src/codec/belief_ir.rs`), as is any `id`
carrying the `buildonomy_` prefix. `beliefbase_architecture.md` §2.4 covers the
API node's lifecycle and the validation errors.

### 5.1 Two proposed namespaces

Two open issues each add a reserved namespace, and each follows the Asset
precedent: a node whose identity is *derived from what it names* rather than
minted from position, so the same subject resolves to the same node in every
corpus with no coordination.

| Proposed | Owner | Key material | Holds |
|---|---|---|---|
| `Record` | Issue 108 | `source_id` + `range` | spans in external evidence stores (a CI log, a telemetry series), carrying `content_hash` and an optional `summary` |
| `Actor` | Issue 105 step 4, Issue 112 | an actor's email address; a role's name | actors and the roles they hold — the agent/authorization boundary |

**`Actor` holds roles as well as actors, deliberately.** A role could have taken
a namespace of its own; putting both inside one draws the boundary where it is
useful — everything in the namespace is either something that can act or
something that says what may be acted. A membership test then answers "is this an
authorization object?" in one comparison, and there is one constant and one
system network rather than two of each.

The two are distinguished *within* the namespace by `BeliefNode.schema`
(`"actor"` / `"role"`), not by BID. That discriminator is load-bearing: a role is
an authorization surface and an actor is an authentication surface, and only the
latter may discharge an obligation. It is declared rather than inferred from
graph position, because a role with no holders is structurally indistinguishable
from an actor and must never become signable by that accident
(`ISSUE_112_CREDENTIALS_AND_PROMOTION.md`).

> **Whichever lands first must widen the table for the other.**
> `const_namespaces()` returns a fixed-size `[Bid; N]`, `is_reserved` iterates
> it, and each namespace needs a first byte preserving §5's sort rule — codec
> BIDs must remain last, so every new content namespace sorts below `0xff`. Two
> issues editing one array in two branches is a merge conflict by construction;
> the first to land should add both constants, even where the second's node
> schema is still open.

**Both belong in `content_namespaces()`, and the reason is lossiness rather
than taxonomy.** That set determines whether `BeliefNode::keys` emits
`NodeKey::Path` or `NodeKey::Id`. `NodeKey::from_str` already routes every
unrecognized scheme to a `Path` under Href, because "URLs are locations (paths),
not identifiers — storing as Path avoids destructive `to_anchor()`
slugification" (`src/nodekey.rs`). A `Record` id is a location in someone else's
store. An `Actor` id is URL-shaped too — an email, a DID — and the `Id` branch
would slugify it: `mailto:` and `@` mangled, case folded, which corrupts a
case-sensitive DID method-specific identifier outright. Take the `Path` branch
for both.

One decision remains per-namespace and must be made deliberately rather than by
copying Asset:
- **Whether a system network exists** beside Href and Asset
  (`beliefbase_architecture.md` §2.4). Issue 108 specifies one for `Record`. The
  `Actor` case is open and interacts with a live question: actors and roles carry
  Section structure between themselves (a role hierarchy), so they have a
  *position*, and which network is home depends on whether a role may also be
  authored in an ordinary corpus document.

> **Two different objects want the `Record` namespace.** Issue 108's record node
> addresses an *external* store. A folded annotation also becomes a node (§6.1),
> and it is internal. Both are reasonably called "record nodes", and code that
> only ever sees one of them cannot tell which it has.
>
> **`BeliefKind::External` is the discriminator**, and it needs no new
> machinery: it already marks "a link to a source we don't have native parsing
> capability for" (`src/properties.rs`), which is precisely what a span in a CI
> log is and precisely what a folded annotation is not. Href and asset nodes
> carry it for the same reason. The two populations can therefore share the
> namespace.
>
> **It does not separate them on its own.** An actor referenced by `mailto:` or
> `did:` is also an unparseable external reference, so `External` partitions
> external-backed from internal across the whole namespace set without
> distinguishing an evidence span from an actor. Within a namespace that is
> `schema`'s job — which is the same field as `record_kind`
> (`attestation_fabric.md` §6.1: `record_kind` *is* the `schema:` filter value),
> and the same discriminator that separates actor from role. Two checks, two
> questions: `External` asks whether the referent is outside the corpus,
> `schema` asks what kind of thing it is. A check for one that assumes the other
> is the failure mode.

## 6. Record nodes: the BID is derived from whatever addresses the record

A record node's BID is never minted. Two kinds of thing land in the `Record`
namespace, and they derive from different strings — but they are not peers, and
reading them as peers is the error this section exists to prevent.

| | Folded annotation | Cited external span |
|---|---|---|
| Owner | Issue 105 | Issue 108 |
| Derives from | `EventId` — `(actor, session, sequence)` | the accessor — `(source_id, range)` |
| `BeliefKind::External` | no | yes |
| Can stand alone | **yes** — it is a claim | **no** — only as cited evidence |

**An external record node exists only as the sink of an Epistemic citation from
an annotation** (Issue 108 Decision 6). It is a *reference inside an
annotation's payload*, not an annotation. The motivating case is a sign-off
citing a data slice that corroborates its conclusion: the claim is the
annotation, and the span is what the claim draws from. Evidence with no citing
claim is a log entry, and a graph accumulating those is a log index.

So the asymmetry runs deeper than the derivation input. A folded annotation has
an actor, a lifecycle, and a position in a `caused_by` chain; it can be
promoted, superseded, and attested. An external record node has none of these —
it is an address with a fingerprint, and everything agential about it belongs to
the annotation citing it. Do not give it an actor, and do not promote it; the
citing annotation is what moves, and the address travels in its payload.

Both are nonetheless the §3 pattern over a canonical string, and the three
properties in §6.1 hold for both.

### 6.1 The internal case: `EventId`

An annotation record's identity is its `EventId` —
`(actor, session, sequence)`, defined in `beliefbase_architecture.md` §4.3. The
record store keys on it, `caused_by` cites it, and Issue 105 derives `run_id`
from it.

Folding an annotation projects the record into the graph as a `NodeUpsert`
(`living_corpus.md` §4, `attestation_fabric.md` §12.3), and `NodeUpsert` applies
"by BID, no collision resolution — BID already canonical"
(`beliefbase_architecture.md` §4.2). So the fold must supply a BID that is
already canonical.

**That BID is derived from the `EventId`, never minted.** Minting it would
re-mint the record node on every fold, so a `caused_by` chain resolved to record
nodes would point at different nodes after each rebuild — §1's rule violated at
the layer whose entire purpose is durable claims. The derivation is the §3
pattern applied to the record's identity string:

```
record_bid = v5(UUID_NAMESPACE_RECORD, canonical(EventId))
             with octets 10-15 stamped to the Record namespace bref
```

Three properties follow, and each matters to a named consumer:

- **Idempotent folding.** Folding the same record set twice produces the same
  nodes, so a re-fold is a no-op rather than a duplicate — which is what lets
  the fold run on every parse without accumulating.
- **Convergent across stores.** Two collectors holding the same record derive
  the same BID, so a G-Set union of records projects to a union of nodes with no
  reconciliation step.
- **Resolvable citations.** An annotation citing another record by `EventId`
  resolves to a graph node without a lookup table, because the edge target is
  computable from the citation itself.

The canonical `EventId` string must be **length-delimited**, for the same reason
the anchor hash input must be (`content_versioning.md` §8): without it, two
distinct field splits can serialize identically.

> **Status.** The derivation is specified here and has no implementation. It is
> *omittable* in the sense of `LESSONS_LEARNED.md` §"A forward-compatibility
> field is a liability" — a pure function over an identity fixed elsewhere,
> addable when the fold is built, at no cost to records written before it.
> Issue 108 owns the namespace; the fold's owner owns the call.

### 6.2 The external case: the record accessor

An external record node has no `EventId` — nothing in noet produced it, and no
actor is accountable for it. What it has is an accessor: a `source_id` naming a
registered `RecordSource` and a `RecordRange` addressing a span within it
(Issue 108 Decisions 3 and 4). That pair is the identity, and it derives the BID
the same way:

```
record_bid = v5(UUID_NAMESPACE_RECORD, canonical(source_id, range))
             with octets 10-15 stamped to the Record namespace bref
```

The three properties carry over with one substitution — two corpora citing the
same span converge on one node, without either knowing of the other. That is
what makes a shared evidence store addressable by independent teams, and it is
strictly stronger than the internal case, where convergence only follows from
having seen the same record.

**Convergence is also what keeps the citation-only rule cheap to enforce.** Two
annotations citing one span resolve to one node with two incoming Epistemic
edges, so "is this evidence still cited?" is an indegree test rather than a
scan — which is how `check_consistency` reports a free-standing record node as a
defect (Issue 108 Decision 6).

> **The accessor must be canonicalized before hashing, and `RecordRange` makes
> this harder than it looks.** `Offset { start: 0, end: 100 }` and
> `Sequence { from: 0, to: 100 }` are different spans that serialize
> near-identically, so the **variant tag is part of the input**, not an
> implementation detail of the encoding. `Selector(String)` is worse: it is
> store-defined and opaque, so two selectors that a store considers equivalent
> — a query with reordered clauses, a run ID with a differing prefix — derive
> different BIDs and produce two nodes for one span. **Canonicalization of a
> `Selector` is the owning `RecordSource`'s responsibility**, and a source that
> cannot canonicalize its own selectors will silently fragment. Issue 108 should
> put this on the trait rather than leaving it to each implementor.

**A `content_hash` must not enter the derivation**, even though a record node
carries one. Content addressing by hash is the Asset pattern
(`buildonomy_asset_bid`), and it is right for an asset because the bytes *are*
the identity — different bytes, different asset. A cited span is the opposite:
`verify` exists precisely to detect that a span's content **changed** while
remaining the same span, and Issue 108 names `Changed` its most alarming
outcome. Deriving the BID from the hash would make a changed span a *new node*,
so the citation would resolve to something that no longer exists and the
alarming case would vanish into a silent orphan. Address by *where*, verify by
*what*.

## 7. Where each identity is specified

| Identity | Minted or derived | Authority |
|---|---|---|
| `Bid` (content node) | minted, persisted via frontmatter and shards | `beliefbase_architecture.md` §2.2 |
| `Bref` | derived from `Bid` | §4 above |
| `NodeId` / anchor | derived from title or explicit `{#id}` via `to_anchor` | `beliefbase_architecture.md` §2.2.1 |
| API node BID | derived from the crate version | §3 above |
| Href / Asset BID | derived from URL / content hash | §3 above |
| Codec namespace BID | derived from a normalized term | §3 above |
| `EventId` | minted (`session` is entropy; `sequence` is a counter) | `beliefbase_architecture.md` §4.3 |
| Record node BID (internal) | derived from `EventId` | §6.1 above |
| Record node BID (external) | derived from `(source_id, range)` | §6.2 above |
| `_content_hash` | derived from node content | `content_versioning.md` §5.1 |
| Identity hash | derived from normalized content | `content_identity.md` |

## 8. References

| Document | Relationship |
|---|---|
| `core/beliefbase_architecture.md` §2.2 | `NodeKey` variants and the resolution hierarchy; what the identities *mean* |
| `core/beliefbase_architecture.md` §2.4 | The API node's lifecycle, reserved-identifier validation |
| `core/beliefbase_architecture.md` §4.3 | `Envelope` and `EventId`; why `session` exists |
| `identity/content_identity.md` | When two nodes are the same node despite moving or reformatting |
| `identity/content_versioning.md` | When a node's *content* has changed; the anchor model |
| `project/LESSONS_LEARNED.md` §Identity and caching | The failure modes this document's rule prevents |
