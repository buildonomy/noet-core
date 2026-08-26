# Issue 88: Type-Safe Edge Ownership

**Priority**: MEDIUM
**Estimated Effort**: 2 days
**Dependencies**: None (standalone refactor)

## Summary

Edge ownership (`WEIGHT_OWNED_BY`) is stored as a raw string in
`Weight.payload` and parsed via `weight.get::<String>(WEIGHT_OWNED_BY)`
at ~15 call sites, each repeating the same three-way match on `"source"`,
`"sink"`, or bref-string. Replace with a typed `EdgeOwner` enum on
`Weight`, with getter methods that resolve to `Bref` or `Bid` as needed.

## Goals

- Replace stringly-typed `WEIGHT_OWNED_BY` reads with `weight.owner()`
- Eliminate the repeated match-on-string-variant pattern (~15 sites)
- Keep setter ergonomic for builder sites that know direction but not BID
- Preserve SQLite `owned_by` column semantics (denormalized string for indexing)

## Architecture

### `EdgeOwner` enum (in `properties.rs`)

```rust
/// Ownership model for an edge weight.
///
/// Determines which node "owns" the edge for purposes of GC scoping,
/// diff computation, and `{maps_to}` traceability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EdgeOwner {
    /// The source endpoint owns this edge.
    Source,
    /// The sink endpoint owns this edge (default when absent).
    Sink,
    /// A third-party node (identified by bref) owns this edge.
    /// Typically a section node with a `{maps_to}` directive.
    ThirdParty(Bref),
}
```

Custom `Serialize`/`Deserialize`: serializes as a plain string
(`"source"`, `"sink"`, or the bref hex string). Deserializes the
reverse. This keeps the `Weight` payload backward-compatible (the
`owned_by` key remains a string in TOML/msgpack). Backward
compatibility isn't strictly required since all datasets are
ephemeral, but keeping string serde means existing petgraph
`WeightSet` serialization just works.

### `Weight` changes

Promote `owned_by` from payload bag to a typed field:

```rust
pub struct Weight {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owned_by: Option<EdgeOwner>,

    #[serde(flatten)]
    pub payload: Table,
}
```

With `#[serde(flatten)]` on `payload`, the `owned_by` key is
extracted by serde into the typed field automatically — it won't
appear in `payload` anymore. The `#[serde(default)]` handles the
absent case (→ `None`, treated as sink-owned).

### Getter methods on `Weight`

```rust
impl Weight {
    /// Edge owner as a `Bref`, or `None` for source/sink ownership.
    pub fn owner_bref(&self) -> Option<Bref> {
        match &self.owned_by {
            Some(EdgeOwner::ThirdParty(bref)) => Some(*bref),
            _ => None,
        }
    }

    /// Resolve the owning BID given a bref index.
    pub fn owner_bid(&self, brefs: &BTreeMap<Bref, Bid>) -> Option<Bid> {
        self.owner_bref().and_then(|bref| brefs.get(&bref).copied())
    }

    /// Is this edge owned by its source endpoint?
    pub fn is_source_owned(&self) -> bool {
        matches!(&self.owned_by, Some(EdgeOwner::Source))
    }
}
```

### Setter ergonomics

The builder currently writes ownership via direction:

```rust
let owner = match direction {
    Direction::Incoming => "sink",
    Direction::Outgoing => "source",
};
weight.set(WEIGHT_OWNED_BY, owner).ok();
```

Becomes:

```rust
weight.owned_by = Some(match direction {
    Direction::Incoming => EdgeOwner::Sink,
    Direction::Outgoing => EdgeOwner::Source,
});
```

For third-party (mapping) edges:

```rust
weight.owned_by = Some(EdgeOwner::ThirdParty(owner_bref));
```

### `view::EdgeOwnership` relationship

The existing `view::EdgeOwnership` enum adds a `Missing` variant and
carries `ThirdParty(Bid)` (resolved). After this refactor, it becomes
a thin resolved wrapper:

```rust
pub(crate) enum EdgeOwnership {
    Source,
    Sink,
    ThirdParty(Bid),
    Missing, // data integrity error — edge has no owner
}

impl EdgeOwnership {
    pub(crate) fn from_weight(weight: &Weight, graph: &BeliefGraph) -> Self {
        match &weight.owned_by {
            Some(EdgeOwner::Source) => Self::Source,
            Some(EdgeOwner::Sink) => Self::Sink,
            Some(EdgeOwner::ThirdParty(bref)) => {
                match graph.states.keys().find(|bid| bid.bref() == *bref) {
                    Some(&bid) => Self::ThirdParty(bid),
                    None => Self::Missing,
                }
            }
            None => Self::Sink, // absent defaults to sink-owned
        }
    }
}
```

### SQLite (`db.rs`)

The `relations` table has an `owned_by TEXT` column with an index
used for Owner-role traversal (`WHERE owned_by IN (...)`). The
`update_relation` method currently extracts the string from the
payload. After the refactor, it reads `weight.owned_by` and
serializes to string for the SQL bind:

```rust
let owned_by: Option<String> = weight.weights.values()
    .find_map(|w| w.owned_by.as_ref())
    .map(|eo| match eo {
        EdgeOwner::Source => "source".to_string(),
        EdgeOwner::Sink => "sink".to_string(),
        EdgeOwner::ThirdParty(bref) => bref.to_string(),
    });
```

No schema migration needed — the column value format is unchanged.

## Implementation Steps

1. Define `EdgeOwner` enum with custom serde (0.5 day)
   - [ ] Add `EdgeOwner` to `properties.rs`
   - [ ] Custom `Serialize`/`Deserialize` for string-based format
   - [ ] Add `owned_by: Option<EdgeOwner>` field to `Weight`
   - [ ] Add getter methods (`owner_bref`, `owner_bid`, `is_source_owned`)
   - [ ] Update msgpack round-trip regression test
   - [ ] Deprecate `WEIGHT_OWNED_BY` constant (keep for transition)

2. Update write sites (0.5 day)
   - [ ] `builder.rs` `push()` — set `weight.owned_by` directly
   - [ ] `builder.rs` `push_relation()` — direction-based setter
   - [ ] `builder.rs` `push_mapping()` — `ThirdParty(owner_bref)`
   - [ ] Test fixtures

3. Update read sites (0.5 day)
   - [ ] `base.rs` `compute_diff` (×2) — use `weight.owned_by` match
   - [ ] `base.rs` `third_party_owner_brefs` — use `owner_bref()` getter
   - [ ] `base.rs` `collect_output_bids` — use `owner_bref()` getter
   - [ ] `context.rs` `owned_edges` (×2) — use `owner_bref()` getter
   - [ ] `context.rs` `declared_edges` — compare `owner_bref()`
   - [ ] `graph.rs` `display_contents` — match on `weight.owned_by`
   - [ ] `myst.rs` `build_mapping_table_html` — match on `weight.owned_by`
   - [ ] `db.rs` `update_relation` — serialize from typed field
   - [ ] `shard/export.rs` — use `owner_bref()` getter
   - [ ] `raw_tape.rs` `edges_rows` — use `EdgeOwnership::from_weight`

4. Cleanup (0.5 day)
   - [ ] Remove `WEIGHT_OWNED_BY` constant (or leave deprecated)
   - [ ] Remove inline `use` in `myst.rs` `build_mapping_table_html`
   - [ ] Verify all tests pass

## Testing Requirements

- `Weight` serde round-trip: `EdgeOwner` survives TOML/msgpack/JSON
- `EdgeOwner::ThirdParty(bref)` serializes as the bref hex string
- Absent `owned_by` deserializes as `None` (sink-owned behavior)
- `compute_diff` correctly scopes GC with typed ownership
- Owner-role traversal still works in both in-memory and SQL paths

## Success Criteria

- [ ] Zero call sites use `weight.get::<String>(WEIGHT_OWNED_BY)`
- [ ] All ownership reads go through `weight.owned_by` or getter methods
- [ ] `EdgeOwner` custom serde round-trips through TOML, msgpack, JSON
- [ ] Owner-role traversal tests pass (SQL and in-memory)
- [ ] `view::EdgeOwnership` resolved via `from_weight` helper

## Risks

- **`#[serde(flatten)]` interaction**: promoting `owned_by` from
  the flattened payload to a named field changes deserialization
  order. The `owned_by` key must be consumed by the named field
  before `payload` flattens the remainder. Serde handles this
  correctly for `#[serde(flatten)]`, but the msgpack regression
  test will catch any issues.
  → **Mitigation**: run the existing msgpack round-trip test first.

## Open Questions

- Should `WEIGHT_OWNED_BY` be fully removed or left as a deprecated
  constant? It's only useful if external code reads the raw payload.
  Recommend removing once all internal sites are migrated.
- `WEIGHT_SORT_KEY` is the same payload-bag pattern (`u16` accessed
  via `weight.get::<u16>(WEIGHT_SORT_KEY)`). Less pressing since
  it's a stable simple datatype with no variant matching, but should
  be promoted to a typed field (`pub sort_key: Option<u16>`) in the
  same pass for consistency. Can be done as a follow-on or bundled
  into step 4 cleanup if time permits.
