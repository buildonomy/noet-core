# Issue 78: Codec-Registered Secondary Index Namespaces

**Version**: 0.1
**Priority**: HIGH
**Estimated Effort**: 5 days
**Dependencies**: Issue 76 (ProtoIndex Codec Meta — complete). Unblocks: Backlog "Site-Root-Relative Slug
Resolution" (MDN codec)

## Summary

`#include <target/Header.h>` edges cannot resolve because the target header's
PathMap entry is network-relative (`target/Header.h` inside the `target`
network) while the edge key is repo-relative (`src/core/target/Header.h`).
These live in different resolution namespaces — `cache_fetch` can never match
one against the other.  The fix is to let codecs register **secondary index
namespaces** — named, cross-network PathMaps that map synthetic lookup paths
to node BIDs, analogous to a SQL secondary index on a non-primary-key column.

## Problem

The PathMap stores each node under a single canonical path, scoped to its home
network.  When a codec rewrites a header's path for include-convention
resolution (stripping `include/` prefixes), the rewritten path becomes the
canonical PathMap entry.  But the `#include` edge from another file computes a
*different* path — either repo-relative or using a different prefix convention.

The existing `path_aliases` mechanism (IRNode field, appended to `doc_paths`
weight) adds additional entries to the *same network's* PathMap.  This works
for intra-network resolution but not for cross-network `#include` edges, where
the includer doesn't know (and shouldn't need to know) the target's home
network bref.

The `href_namespace` and `asset_namespace` const namespaces solve an analogous
problem for external URLs and static assets — they provide a global,
network-independent PathMap.  But they use hardcoded UUIDs in `properties.rs`,
which is wrong for codec-specific dynamic content.

## Goals

- Codecs can declare named secondary index namespaces (e.g. `"include"`)
- Nodes can be registered in a namespace under synthetic lookup paths
- Edge keys can target a namespace for cross-network resolution
- Namespace BIDs are deterministic and stable across runs for a given name
- Shard export includes only the referenced subset of each namespace (same
  strategy as href/asset)
- The mechanism is generic — not C++-specific — so future codecs (MDN slug
  resolution, Rust `use` paths, Python import paths) can reuse it

## Architecture

### Mental Model

A network's PathMap is the **primary index** — it maps canonical filesystem
paths to BIDs within that network's scope.  A secondary index namespace is an
**additional index** that maps synthetic lookup paths to the same BIDs, scoped
globally (cross-network).  The codec defines what paths go into the index; the
engine maintains the PathMap and resolves lookups.

### Namespace Identity

A new `UUID_NAMESPACE_CODEC` constant in `properties.rs` (parallel to
`UUID_NAMESPACE_BUILDONOMY`, `_HREF`, `_ASSET`) serves as the root for all
codec-registered namespaces.

The first byte is `0xff` (vs `0x6b`/`0x5b`/`0x4b` for the existing
namespaces).  This ensures codec namespace BIDs sort **after** all content
network BIDs in `BTreeSet<Bid>` iteration order.  `partition_graph` in shard
export uses `or_insert` semantics when assigning nodes to networks — by
sorting codec namespaces last, content network assignments always win without
requiring any logic changes to `partition_graph`.

Individual namespace BIDs are derived via:

```
pub const UUID_NAMESPACE_CODEC: Uuid = Uuid::from_bytes([
    0xff, 0x3d, 0x21, 0x54, 0xc0, 0xa9, 0x43, 0x7b,
    0x93, 0x24, 0x5f, 0x62, 0xad, 0xeb, 0x9a, 0x44,
]);

impl Bid {
    pub fn codec_namespace(term: &str) -> Bid {
        Bid(Uuid::new_v5(&UUID_NAMESPACE_CODEC, term.as_bytes()))
    }
}
```

This is a **pure function** on `Bid` — callable anywhere, no state needed.
`DocCodec` instances are created fresh by the factory for each file parse
(they are not durable), so they cannot stash state from a prior registration
call.  Instead, a codec uses a hardcoded string constant (e.g. `"include"`)
and calls `Bid::codec_namespace("include")` to derive the bref whenever it
needs to annotate an `IRNode` or edge key.  The derivation is pure and
deterministic, so this is safe to call from any codec instance at any time.

The codec namespace family is kept distinct from system namespaces:

- `UUID_NAMESPACE_BUILDONOMY` → system infrastructure (API nodes)
- `UUID_NAMESPACE_HREF` → external URLs
- `UUID_NAMESPACE_ASSET` → static assets
- `UUID_NAMESPACE_CODEC` → codec-registered secondary indices

`is_reserved()` is updated to include `UUID_NAMESPACE_CODEC` in
`const_namespaces()`, so codec namespace BIDs are recognized as reserved
without risk of collision with system or user-generated BIDs.

### No Registration Step

href and asset namespaces have no registration step — they are lazily created
inside `push_relation()` and `process_asset()` when the builder first
encounters a key targeting them.  Codec namespaces follow the same pattern.

`push()` can identify codec namespace BIDs via `is_reserved()` — the
octets 10–15 stamping (see Namespace Identity above) makes codec namespace
BIDs self-identifying.  No ProtoIndex registration is needed to distinguish
them from regular network brefs.

### Population (via `push()`)

A new field on `IRNode`:

```
pub namespace_paths: Vec<(Bref, String)>,  // (namespace_bref, alias_path)
```

The codec is responsible for annotating this field with the bref it received
from `register_namespace`.  Each entry declares: "register this node in the
namespace identified by `bref` under path `alias_path`."

During `push()`, noet-core:

1. Lazily creates the namespace's `BeliefNode` (kind: `Network | External |
   Trace`, same as `href_network()` / `asset_network()`) if not already
   present in `doc_bb`
2. Ensures the namespace has a `PathMap` entry in `PathMapMap`
3. Emits a `RelationChange` (Section edge) from the node to the namespace
   root, with `alias_path` as the `doc_paths` weight
4. The PathMap processes this event and creates the lookup entry

For the C++ include case, the C++ codec (during `parse()`) would emit:

```
const INCLUDE_NS: &str = "include";  // codec-level constant

// In parse():
let ns_bid = Bid::codec_namespace(INCLUDE_NS);  // pure derivation
root.namespace_paths.push((ns_bid, "target/Header.h".into()));
```

### Edge Emission (via `push_relation()`)

The consuming codec annotates `IntermediateRelation` keys with the namespace
bref.  The codec is responsible for computing the correct `NodeKey::Path`:

```
NodeKey::Path {
    net: include_ns_bref,  // from register_namespace
    path: "target/Header.h".into(),
}
```

Since the bref is derived from the term alone, any codec can compute it
independently via `Bid::codec_namespace("include").bref()`.  The consuming
codec uses the same hardcoded string constant as the producing codec:

```
let ns_bref = Bid::codec_namespace(INCLUDE_NS).bref();
root.upstream.push(IntermediateRelation::new(
    NodeKey::Path { net: ns_bref, path: "target/Header.h".into() },
    WeightKind::Pragmatic,
    None,
));
```

### Resolution (via `push_relation()` → `cache_fetch`)

`push_relation()` passes the `NodeKey::Path { net: ns_bref, path }` through
`regularize_unchecked` and into `cache_fetch`.  `cache_fetch` already resolves
`NodeKey::Path { net, path }` against the PathMap for network `net`.  No
changes to `cache_fetch` are needed — the namespace PathMap is just another
PathMap in `PathMapMap`, keyed by the namespace bref.  The resolution path is
identical to href/asset namespace resolution.

`regularize_unchecked` must treat namespace brefs as non-default (no owner
path joining) and non-href (no external classification).  Since namespace
brefs are neither `Bref::default()` nor `href_namespace().bref()` nor
`asset_namespace().bref()`, they already fall into the "non-default net, just
normalize" branch — no changes needed.

### Shard Scoping

Same strategy as href/asset (see `export_sharded` in `shard/export.rs`):

1. `partition_graph` assigns namespace nodes to their namespace network.
   Because `UUID_NAMESPACE_CODEC` sorts after all content network BIDs
   (`0xff` first byte), content network assignments win via `or_insert`
   semantics — no logic changes to `partition_graph` needed.
2. Per-network shard export collects `referenced_extern_states` — namespace
   nodes referenced by edges in the shard but not in the network's own state
   set
3. Embeds those stubs (plus their Section edge to the namespace root) in the
   network shard

No new shard logic required — the existing extern-state embedding handles
any network, including dynamic namespaces.

### Differences from Const Namespaces

| Aspect | Const (href/asset) | Codec-registered |
|--------|-------------------|---------------------------|
| BID source | Hardcoded UUID bytes | `Uuid::new_v5(UUID_NAMESPACE_CODEC, term)` |
| Parent namespace | `UUID_NAMESPACE_HREF/ASSET` | `UUID_NAMESPACE_CODEC` |
| Lifetime | Always present | Created on demand during parse |
| `BeliefNode` factory | `BeliefNode::href_network()` | Generic factory taking term |
| `PathMapMap::new` | Pre-inserted | Inserted on first `push()` |
| `is_reserved()` | Yes (parent bref check) | Yes (parent = CODEC namespace) |
| Node BID derivation | `buildonomy_href_bid(url)` | Standard `Bid::new(namespace_bid)` |

## Implementation Steps

1. `UUID_NAMESPACE_CODEC` and `Bid::codec_namespace` (0.5 days)
   - [x] Add `pub const UUID_NAMESPACE_CODEC: Uuid` to `properties.rs`
   - [x] Add `pub fn codec_namespace(term: &str) -> Bid` method on `Bid`
   - [x] Add `pub fn codec_network(term: &str) -> BeliefNode` factory
   - [x] Update `const_namespaces()` to include `codec_namespace` root
   - [x] Verify `is_reserved()` returns true for codec namespace BIDs
   - [x] Verify determinism: same term → same BID across calls and processes
   - [x] Unit tests for determinism and non-collision with existing
         const namespaces

2. `IRNode::namespace_paths` field and `push()` integration (1.5 days)
   - [x] Add `namespace_paths: Vec<(Bid, String)>` to `IRNode`
   - [x] In `push()`: for each `(ns_bid, alias)`, lazily create namespace
         network node in `doc_bb` + `session_bb` + `tx`, emit
         `RelationChange` with alias as `doc_paths` weight
   - [x] `CODEC_NAMESPACES` singleton registry in `codec/mod.rs`
   - [x] `process_unresolved_reference` returns `true` for codec NS brefs
   - [x] `find_externals` skips `External+Trace` nodes (Issue 34 balance fix)
   - [x] `BALANCE_CUTOFF` raised from 10 to 15
   - [x] Verified PathMap creates lookup entries (large systems-engineering corpus build)

3. Edge resolution validation (0.5 days)
   - [x] Verified `regularize_unchecked` passes namespace-bref keys through
         the "non-default net, just normalize" branch unchanged
   - [x] Verified `cache_fetch` resolves `NodeKey::Path { net: ns_bref, path }`
         against the namespace PathMap
   - [x] Large systems-engineering corpus build: `#include` edges resolve via namespace

4. Shard export validation (0.5 days)
   - [x] `partition_graph` assigns namespace nodes correctly (0xff sort)
   - [x] Extern-state embedding works for dynamic namespaces
   - [x] Large systems-engineering corpus build produces correct shard output

5. Downstream C++ codec migration (1 day)
   - [x] Define `const INCLUDE_NS: &str = "include"` in the downstream C++ codec's module
   - [x] C++ codec: `compute_include_convention_path` replaces
         `rewrite_path_for_include_dirs` — canonical path stays real,
         include-convention path goes into `namespace_paths`
   - [x] C++ codec: `#include` edges target
         `Bid::codec_namespace(INCLUDE_NS).bref()`
   - [x] `CmakeInfo::parse` infers `include_dirs` from
         `add_subdirectory(include)` when `target_include_directories` absent
   - [x] `compute_include_convention_path` walks to deepest cmake ancestor
   - [x] Verified include edge resolution in large systems-engineering corpus build

6. Backlog cleanup (0.5 days)
   - [ ] Update Backlog "Site-Root-Relative Slug Resolution" to reference
         this mechanism
   - [ ] Create backlog item for MDN codec using namespace_paths for slug
         resolution

## Testing Requirements

- Deterministic BID: `dynamic_namespace("include")` returns the same BID
  across separate calls and separate process runs
- Round-trip: node registered in namespace → `cache_fetch` with namespace key
  resolves to same BID
- Cross-network: header in network A registered in `"include"` namespace,
  includer in network B resolves via namespace key
- Shard: namespace stub embedded in consumer network's shard
- No regression: large systems-engineering corpus parse time, FIRST-ONE-WINS
  count, cache_fetch MISS count unchanged or improved

## Success Criteria

- [x] C++ `#include` edges resolve correctly in large systems-engineering corpus build
- [x] `cache_fetch` MISS count on that corpus does not increase
- [x] Shard sizes do not grow meaningfully (namespace stubs are small)
- [x] Mechanism is codec-agnostic — no C++ specific code in noet-core

## Risks

- **PathMap entry collision**: Two nodes in different networks register the same
  namespace alias path. → **Mitigation**: This is semantically valid (e.g. two
  headers with the same include path in different components would be a real
  C++ build error).  First registration wins; emit a diagnostic warning.
- **`partition_graph` BID→net assignment**: A node in both a content network
  PathMap and a codec namespace PathMap could be mis-assigned. →
  **Mitigation**: `UUID_NAMESPACE_CODEC` uses first byte `0xff` so codec
  namespace BIDs sort after all content network BIDs.  `or_insert` semantics
  in `partition_graph` means content networks always claim the node first.
- **Ordering dependency**: Namespace network node must exist before child nodes
  emit `RelationChange` to it. → **Mitigation**: `push()` lazily creates the
  namespace node on first encounter (same pattern as `process_asset` creating
  `asset_namespace` on first use — no registration step needed).
- **Performance**: Additional PathMap per namespace adds memory and lookup time.
  → **Mitigation**: Each namespace PathMap is small (only registered aliases,
  not full filesystem trees).  Lookup is O(log n) in the PathMap size.

## Resolved Questions

- **`rewrite_path_for_include_dirs`: removed.**  The canonical PathMap path
  is the real filesystem path.  Include-convention lookups go through the
  secondary index namespace.  Keeping both the rewrite and the namespace
  alias would give nodes three paths (real, rewritten-canonical,
  namespace-alias) — one canonical path + one namespace alias is cleaner.
- **`codec_namespace` term normalization**: terms are run through
  `to_anchor()` before BID derivation.  This is consistent with how all
  other path-like strings are normalized in the system and prevents
  whitespace, casing, or separator differences from producing silently
  different BIDs for the same logical namespace.
