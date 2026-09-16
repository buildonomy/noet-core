---
version = "0.1"
title = "Issue 113: Revisit the Packed BID Discriminator"
---

# Issue 113: Revisit the Packed BID Discriminator

**Priority**: LOW — backlog. Nothing is broken; this is a simplification with a
storage dividend, and the migration cost is the reason it is not urgent.
**Estimated Effort**: unscoped — step 1 is a measurement that decides whether
there is an issue at all
**Dependencies**: Would touch every stored artifact, so it wants **Issue 66**
(shard hydration) and **Issue 74**'s archive format settled first — a format
change is cheapest before generations accumulate, and impossible to do quietly
after.
**Design context**: `docs/design/identity/identity_derivation.md` §4

---

## Summary

A `Bid` packs a 6-byte discriminator into octets 10–15: the **bref of its
derivation parent**, the BID it was generated from. That field earns its place
for one population — reserved-namespace nodes, where `is_reserved()` tests it on
a hot path — and appears to earn nothing for the rest, where the generator is
merely the encapsulating section and containment is already in the graph.

This issue asks whether the general-population case can be dropped or simplified,
and what that would return in storage.

## Why it is worth asking

**The discriminator is tested, not read, and only for one thing.**
`is_reserved()` checks membership in the const namespaces on every BID it sees.
That is exactly the case §4's criteria endorse: tested for membership, on a hot
path, not otherwise carried.

For the other several thousand nodes in a corpus, the packed value answers *which
node generated this BID*. Nothing queries that. Containment — the question it
resembles — is a Section edge, answered by the graph, and the two are not even
the same relation (`identity_derivation.md` §4: a section's derivation parent is
its containment *sink*).

## The storage argument, and its real size

BIDs serialize as **36-character hyphenated UUIDs** (`Display for Bid`), so the
6-byte field is **12 hex characters of every BID string**, not 6 bytes. It
appears in:

| Site | Occurrences per unit |
|---|---|
| `NetworkShard.states` keys | 1 per node |
| `BeliefNode.bid` | 1 per node |
| `SerializableEdge.source` / `.sink` | 2 per edge |
| `WEIGHT_OWNED_BY` | 1 per owned edge — but a **bref**, 12 chars total, so unaffected |
| `GlobalShard.bref_index` | 2 per entry (bref → bref; already compact) |

So the exposure is roughly **12 chars × (2 per node + 2 per edge)**. On the
measured corpus in `generational_archive.md` §3.3 — 2,445 shards, 95.5 MB, with
the largest at 9.3 MB / 5,664 nodes — relations are 18–26% of a shard and states
74–82%, so edges dominate the count. **Step 1 is to measure this rather than
estimate it**; msgpack string encoding and any downstream compression both blunt
the gain, and a number nobody has taken is not an argument.

## The variant that does not work, and why

Recording this because it is the first idea anyone has, including the author of
this issue.

**Pack the encapsulating *network* instead of the generator.** Network lookup —
`PathMapMap::node_to_nets`, shard routing, `bref_index` — would become a field
read rather than a memoized map. Tempting, and wrong for a structural reason:

- **`node_to_nets` is a multimap.** A node can belong directly to several
  networks. A packed field holds one value, so the map is needed regardless, and
  a packed copy would be a second account of a fact the map already holds.
- **A BID is immutable; network membership is not.** Re-homing a node would
  leave a permanently stale discriminator.

That generalizes into the criterion §4's three-part test does not quite state:

> **A packed discriminator must be a function of identity, not of position.**

Namespace membership is fixed at generation and qualifies. Network membership is
positional and does not. Containment is positional too, which is the deeper
reason the current general-population value is inert: it records a position at
generation time that nothing may rely on afterwards.

## Candidate directions

Unranked; step 1 decides whether any is worth pursuing.

1. **Keep the field, narrow the claim.** Change nothing in the format; state in
   `identity_derivation.md` that the discriminator is meaningful only for
   reserved-namespace testing and that the general-population value must not be
   read. Zero migration, zero dividend, removes a foot-gun.
2. **Reserved flag instead of a bref.** `is_reserved()` needs one bit, not 48.
   A single reserved marker frees the remaining bits for entropy, shrinking
   collision risk rather than storage.
3. **Drop the packing; carry reserved-ness as a field.** `BeliefKindSet` already
   travels with every node and already distinguishes `API` and `External`. If it
   can answer the reserved question, the packed field has no consumer.
4. **Shorter serialization.** Orthogonal to the above and possibly the larger
   win: BIDs are stored as 36-char hyphenated strings where a 22-char base64 or
   a 16-byte binary encoding would do. That is a wire-format change with the same
   migration cost and a bigger dividend.

Note that (4) subsumes much of the benefit of (1)–(3) without touching identity
semantics at all, which may make it the better first move.

## Steps

1. **Measure** (0.5 days)
   - [ ] Bytes attributable to the discriminator across a real shard set, before
         and after msgpack encoding and after gzip
   - [ ] Same for the DB
   - [ ] If the answer is under a few percent, close this issue with direction
         (1) and stop

2. **Confirm the consumer set** (0.5 days)
   - [ ] Every caller of `parent_bref`, `is_parent_filter`, `adopt_into`,
         `is_reserved` — which of them need the packed value rather than a flag?
   - [ ] Does anything outside `is_reserved` depend on the general-population
         value? If so, that is the finding

3. **Decide and migrate** (unscoped)
   - [ ] Any format change rewrites every stored BID; sequence it with the
         archive format so generations are not written twice

## Risks

- **A format change after generations accumulate is expensive and possibly
  irreversible** → **Mitigation**: this is why the issue is sequenced behind
  Issues 66 and 74 rather than picked up opportunistically.
- **The measurement shows a real gain and tempts a rushed change** → the
  discriminator is load-bearing for `is_reserved`, which gates system-namespace
  protection. Direction (2) or (3) must keep that test correct.
- **BID ephemerality confounds the measurement** — unpersisted BIDs embed a
  timestamp and differ between runs. Measure against a corpus with persisted
  BIDs, or compare sizes rather than values.

## References

- `docs/design/identity/identity_derivation.md` §4 — the packed namespace, the
  reuse criteria, and the derivation-parent-versus-containment distinction
- `docs/design/identity/generational_archive.md` §3.3 — the shard size
  measurements this issue's step 1 extends
- `src/properties.rs` — `Bid`, `Bref`, `adopt_into`, `parent_bref`,
  `is_reserved`, `is_parent_filter`
- `src/shard/wire.rs` — where BIDs become strings
- `ISSUE_66_INCREMENTAL_PARSE.md`, `ISSUE_74_CROSS_VERSION_DIFF.md` — the format
  consumers this must sequence behind
