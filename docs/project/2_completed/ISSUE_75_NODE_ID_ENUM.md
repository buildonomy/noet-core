# Issue 75: Separate Anchor Identity from Network-Scoped Identity

**Priority**: HIGH
**Estimated Effort**: 3 days
**Dependencies**: None
**Blocks**: production corpus duplicate section nodes (~850 affected)

## Summary

`BeliefNode.id: Option<String>` conflates the HTML anchor (document-scoped) with
the network-scoped ID used for `NodeKey::Id` resolution. Inter-document
FIRST-ONE-WINS sets `node.id` to a bref to resolve the network collision, but
this corrupts PathMap paths and causes `cache_fetch` misses on re-parse when
`--write` is off. Each miss creates a duplicate section node. Replace
`Option<String>` with a `NodeId` enum that separates the collision signal from
the anchor value.

## Goals

- Eliminate duplicate section nodes caused by FIRST-ONE-WINS + re-parse interaction
- Reduce MISS-on-re-parse warnings from ~850 to 0 in the production corpus
- Preserve backward compatibility with existing msgpack shards
- Keep `--write` off as a first-class mode (no source modifications required)

## Architecture

Replace `BeliefNode.id: Option<String>` with `BeliefNode.id: NodeId`:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum NodeId {
    /// No explicit ID. Title-derived slug used for anchor and path fragment.
    #[default]
    Slug,
    /// User-provided explicit ID (e.g., `{#intro}`). Already normalized.
    Explicit(String),
    /// Inter-doc network-level ID collision occurred. Title-derived slug is
    /// still used for anchor and path fragment (document-unique). Network-
    /// scoped disambiguation uses `node.bid.bref()` (already on the node;
    /// no inner value needed). Ephemeral — re-derived each parse from the
    /// dynamic FIRST-ONE-WINS check. Not persisted to shards.
    Collision,
}
```

### Why this works

`speculative_path_key` already generates `NodeKey::Path { path: "doc.md#slug" }`
for section nodes — it never generates `NodeKey::Id`. The PathMap path just needs
to store the slug. The fix chain:

1. `push()` sets `NodeId::Collision` (no bref injection into `node.id`)
2. `push()` still fixes up the `NodeKey::Id` key in `keys` to use the bref
   (collision avoidance in `insert_state` unchanged)
3. `generate_terminal_path` sees `Collision` → uses `to_anchor(title)` instead
   of `ids.get()` → PathMap stores `doc.md#data-sharing`
4. `inject_context` treats `Collision` same as `Slug` → no explicit ID injected
   into source or heading event → HTML anchor derived from title slug
5. Re-parse: `speculative_path_key` → `"doc.md#data-sharing"` → `cache_fetch`
   hits PathMap → existing BID reused → no duplicate

### Ephemeral, not persisted

`Collision` is re-derived each parse from the dynamic FIRST-ONE-WINS check. It
doesn't need to survive in shards. Serialization: `Collision` → `None` (same as
`Slug`). On shard load, the node appears as `Slug`; the next parse re-detects
the collision and re-marks it. No durable state to manage or invalidate when
collisions resolve (e.g., conflicting document removed).

## Implementation Steps

1. Define `NodeId` enum in `properties.rs` (~0.5 day)
   - [x] Add enum with `Slug`, `Explicit(String)`, `Collision`
   - [x] Implement `anchor() -> &str`, `is_collision() -> bool`
   - [x] Backward-compat `id() -> String` for migration
   - [x] Serde: `Slug`/`Collision` ↔ absent, `Explicit` ↔ plain string

2. Update collision sites (~0.5 day)
   - [x] `builder.rs` `push()`: set `NodeId::Collision` instead of `node.id = Some(bref)`
   - [x] `base.rs` `insert_state()`: same change
   - [x] Keep `NodeKey::Id` key fixup to bref (unchanged)

3. Update path generation (~0.5 day)
   - [x] `pathmap.rs` `generate_terminal_path`: `Collision` nodes use title slug
         via `id()` fallback (returns `to_anchor(title)` for `Collision`)
   - [x] `ids` index populated with title slug for `Collision` nodes (correct for
         PathMap paths); `NodeKey::Id` resolution still uses bref via `keys` fixup

4. Update HTML anchor injection (~0.5 day)
   - [x] `md.rs` `inject_context`: `Collision` treated same as `Slug` — `id()` returns
         title slug, no bref injected into heading event or source
   - [x] Intra-doc collision handling (`-N` suffixes) unaffected

5. Audit `node.id()` call sites (~1 day)
   - [x] Grep all `.id()` calls across codebase (~50 sites)
   - [x] Classify: anchor vs network-id vs backward-compat
   - [x] Updated raw field access sites (`node.id` → `NodeId` pattern matches)

6. Update design docs
   - [x] Update `beliefbase_architecture.md` §2.2.1 (Collision Detection and Resolution)
         to document `NodeId` enum and the anchor vs network-ID distinction

## Testing Requirements

- Existing test suite passes (regression gate)
- production corpus: 0 MISS-on-re-parse warnings, 0 duplicate section nodes
- FIRST-ONE-WINS at parse_number=1 count unchanged (collision detection works)
- HTML heading anchors use slugs, not brefs, for inter-doc collision losers
- `NodeKey::Id` resolution still uses bref for collision losers (network uniqueness)
- Shard round-trip: `Collision` serializes as absent, deserializes as `Slug`

## Success Criteria

- [x] `make render` on the production corpus with `--write` off: 0 Path-based `MISS on re-parse`
      (down from 857 to 31; remaining 31 are cross-network forward refs, not slug collisions;
      218 ID-based MISSes are pre-existing unresolvable links)
- [x] Site verified: no duplicate metadata links on hazard report pages
- [x] Existing tests pass (38 doctests, full unit suite)
- [x] Shard backward compat: `NodeId` serializes as `Option<String>`
- [x] `beliefbase_architecture.md` §2.2.1 updated with `NodeId` enum semantics

## Risks

- **Call-site audit scope**: ~50-80 `node.id()` sites need classification.
  **Mitigation**: keep backward-compat `id()` method; deprecate later.
- **`generate_terminal_path` needs title access**: currently uses `nets.ids`
  only. Must also check `nets.titles` for `Collision` nodes.
  **Mitigation**: `nets.titles` already exists and is populated.

## Open Questions

- Should `NodeId` support `Aliases(Vec<String>)` now, or defer to a future issue?
