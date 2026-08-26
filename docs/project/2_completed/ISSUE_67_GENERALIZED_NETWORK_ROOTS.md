# Issue 67: Generalized Network Root Detection

**Priority**: HIGH
**Estimated Effort**: 4 days (RELATIVE COMPARISON ONLY)
**Dependencies**: Requires Issues 68 (Two-Registry Codec Architecture) and 72
(Network Child Filtering) — both complete.

## Summary

`ProtoIndex` currently hardcodes `NETWORK_NAME` (`"index.md"`) as the sole subnet
boundary marker. Application codecs need the ability to declare that other filenames
also define network roots, so that directories containing those files are treated as
sub-networks with their own child lists, PathMaps, and graph identity.

The motivating pattern is build-system manifest files (e.g. a project where each
component directory contains a manifest that declares its identity, children,
dependencies, and generated artifacts). Such manifests are structurally analogous to
`index.md` but use a different filename. Crucially, the manifest filename may appear
in many directories (hundreds in a large project), but only a subset — identifiable by
content — should define network boundaries. A simple filename list is insufficient;
a path-predicate with optional content inspection is required.

## Goals

1. Add `WalkCodec::network_filenames()` to declare candidate network root filenames.
2. Generalize `net_dir_partition` to detect candidate subnet boundaries via both
   `NETWORK_NAME` and `WALK_CODECS.is_network_file()`, then cull false positives
   in `ProtoIndex::build` via `DocCodec::proto()`.
3. Generalize `detect_network_file` to find network files beyond `index.md`.
4. Replace all `== NETWORK_NAME` path-normalization checks in `builder.rs`,
   `compiler.rs`, and `proto_index.rs` with a unified `is_network_index_file()` helper.
5. Thread the network index filename through `PathMap` so path construction uses the
   actual filename (e.g. `"manifest.txt#slug"`) rather than hardcoded
   `"index.md#slug"`.
6. Establish `payload["codec"]` as a schema-level contract of
   `BeliefKind::Network`, following the precedent set by `BeliefKind::API` payload
   contracts (`payload["package"]`, `payload["version"]`, etc. in `api_state()`).
   The value must be a valid `CODECS` lookup key (filename that round-trips through
   `AnchorPath` → `CODECS.get()`).
7. Maintain full backward compatibility: behavior for `index.md`-only corpora is
   unchanged.

## Architecture

### `WalkCodec::network_filenames()`

A new optional method on the `WalkCodec` trait:

```rust
fn network_filenames(&self) -> Vec<&'static str> {
    vec![]
}
```

This is a superset declaration: "these filenames MIGHT define network roots." A file
matching one of these names causes `net_dir_partition` to tentatively treat its
directory as a subnet boundary. The definitive check happens later in
`ProtoIndex::build`, which calls `DocCodec::proto()` on each candidate — `proto()`
already performs I/O (reads frontmatter/content) and returns `Some(IRNode)` with
`BeliefKind::Network` for real network roots, or `None` for files that happen to
share the filename but aren’t network roots.

This two-phase design avoids adding I/O to the walk pass. `net_dir_partition` stays
pure (filename checks only). Content-based discrimination (e.g. checking for
`add_library(` in a build manifest) lives in the codec’s `proto()` implementation,
which is the natural place for it.

### `WalkCodecMap` extensions

```rust
impl WalkCodecMap {
    /// Filename-only check (no I/O). True if `filename` matches NETWORK_NAME
    /// or any registered walk codec's `network_filenames()`.
    pub fn is_network_file(&self, filename: &str) -> bool {
        if filename == NETWORK_NAME {
            return true;
        }
        self.0.read().iter().any(|c| c.network_filenames().contains(&filename))
    }

    /// Returns all registered network filenames (including NETWORK_NAME),
    /// deduplicated.
    pub fn network_filenames(&self) -> Vec<String> {
        let mut names = vec![NETWORK_NAME.to_string()];
        for codec in self.0.read().iter() {
            for name in codec.network_filenames() {
                if !names.contains(&name.to_string()) {
                    names.push(name.to_string());
                }
            }
        }
        names
    }
}
```

`is_network_file` is a filename-only check (no I/O) used in two places:
- **Subnet detection** (Category A): `net_dir_partition` uses it to tentatively
  classify directories as subnet boundaries.
- **Path normalization** (Category B): builder/compiler/db sites use it to decide
  whether to strip a filename to get the network directory.

### Generalized `detect_network_file`

The current implementation checks only `NETWORK_NAME`. The generalized version checks
`NETWORK_NAME` first (preserving priority for `index.md`), then iterates registered
network filenames:

```rust
pub fn detect_network_file(dir: &Path) -> Option<PathBuf> {
    // Fast path: already pointing at a known network file
    if let Some(name) = dir.file_name().and_then(|n| n.to_str()) {
        if WALK_CODECS.is_network_file(name) {
            return Some(dir.to_path_buf());
        }
    }
    let mut base = dir.to_path_buf();
    if !base.is_dir() { base.pop(); }
    // Check NETWORK_NAME first, then registered network filenames
    let candidate = base.join(NETWORK_NAME);
    if candidate.exists() { return Some(candidate); }
    for filename in WALK_CODECS.network_filenames_excluding(NETWORK_NAME) {
        let candidate = base.join(filename);
        if candidate.exists() { return Some(candidate); }
    }
    None
}
```

WASM note: `WALK_CODECS` is `cfg(not(wasm32))`. The `CodecManifest` pattern
(commit 40b7d79, `codecs.json`) already bridges native-only registries to the WASM
viewer for extensions. Apply the same approach here: extend `CodecManifest` with a
`network_filenames: Vec<String>` field, populated from `WALK_CODECS.network_filenames()`
at export time. The WASM viewer loads these alongside `document_extensions` at startup
so that link normalization and path resolution handle custom network files correctly.

### Generalized `net_dir_partition` + `ProtoIndex::build` culling

Replace `NETWORK_NAME == p_ap.filename()` with
`WALK_CODECS.is_network_file(p_ap.filename())` in both passes of
`net_dir_partition`. This tentatively over-discovers subnet boundaries — every
directory containing a file matching any registered network filename becomes a
candidate.

`ProtoIndex::build` then culls false positives: for each candidate subnet dir
(other than `NETWORK_NAME` dirs, which are always valid), it calls
`CODECS.path_get(network_file)` → `codec.proto(path)`. If `proto()` returns `None`
or returns a node without `BeliefKind::Network`, the directory is demoted — removed
from the partition keys, its children merged back into the parent group.

This keeps `net_dir_partition` pure (no I/O, no content sniffing) and consolidates
all content-based discrimination into `DocCodec::proto()`, where it naturally belongs.

### `is_network_index_file` helper

A unified helper for the path-normalization sites (Category B):

```rust
pub fn is_network_index_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|n| WALK_CODECS.is_network_file(n))
        .unwrap_or(false)
}
```

Replaces 6+ inline `== NETWORK_NAME` checks in `builder.rs`, `compiler.rs`,
`proto_index.rs`, and `db.rs`.

### PathMap: `payload["codec"]` contract

PathMap currently hardcodes `NETWORK_NAME` in three places:
- Initial entry: `(NETWORK_NAME.to_string(), net, [NETWORK_SECTION_SORT_KEY])`
- Anchor prefix: `format!("{NETWORK_NAME}{p}")` for headings
- Bare-anchor alias: `format!("{NETWORK_NAME}{path}")` in `indexed_get`

The fix: Network-kinded nodes carry their index filename in
`payload["codec"]`. This is a schema-level contract of `BeliefKind::Network`,
following the precedent set by `BeliefKind::API` nodes which carry
`payload["package"]`, `payload["version"]`, `payload["authors"]`, etc. in
`api_state()` (see `src/properties.rs`). The kind defines the contract; the payload
carries the data.

**Invariant**: `node.kind.is_network()` → `payload["codec"]` is present.

The value MUST be a filename that, when passed through `AnchorPath` to `CODECS.get()`,
returns the correct codec factory. This ensures the round-trip: given a network node,
the system can always recover the codec that produced it. `NetworkCodec` sets it to
`"index.md"` (matching its `(Some("index"), Some("md"))` CODECS registration);
application codecs set it to their own registered filename.

`PathMapMap::new` already has the full `BeliefNode` for each network (from the
`states` map). It reads `payload["codec"]` and passes it to `PathMap::new`
via a new `codec: String` parameter. `PathMap` stores it in a new field
and uses it in place of `NETWORK_NAME` for the three hardcoded sites. If the key is
missing (backward compat with existing nodes), default to `NETWORK_NAME`.

### Build-system `add_subdirectory` exclusivity note

Some build systems (e.g. CMake) enforce at configure time that each source directory
may be included by at most one parent. When an application codec relies on such an
invariant, the `CLAIM_MAP` first-one-wins property is safe: no two parents will
compete to claim the same child directory. Cross-component dependencies produce
Pragmatic edges (many-to-many), not Section edges (exclusive ownership).

Application codecs that rely on a build-system exclusivity guarantee should document
the assumption. If a future application codec needs multi-parent ownership, the
CLAIM_MAP model would need revisiting — but that is not in scope here.

### `db.rs`

The 4 `NETWORK_NAME` occurrences in `db.rs` serve the same path-normalization and
path-construction roles as the builder/compiler sites. They use the same
`is_network_index_file()` helper (Category B) or read `payload["codec"]` from the
network node when constructing paths (Category C). No separate treatment needed.

## NETWORK_NAME Audit

39 occurrences across 5 files, classified by required action:

| Category | Files | Count | Action |
|---|---|---|---|
| A: Subnet detection | `proto_index.rs` | 3 | Generalize with `is_network_root()` |
| B: Path normalization | `builder.rs`, `proto_index.rs`, `compiler.rs` | 10 | Replace with `is_network_index_file()` |
| C: Path construction | `pathmap.rs` | 3 | Thread `network_filename` field |
| D: DB path resolution | `db.rs` | 4 | `is_network_index_file()` + `payload["codec"]` |
| E: Definitions | `mod.rs`, `network.rs` | 3 | Unchanged |
| F: Network construction | `network.rs`, `compiler.rs` | 5 | Unchanged (specific to `index.md` networks) |
| G: Tests | `proto_index.rs`, `network.rs` | 11 | Add new tests; existing tests unchanged |

## Implementation Steps

### Phase 1: WalkCodec trait extension (0.5 day)

1. Add `network_filenames` method to `WalkCodec` trait
   - [x] Default implementation returns `vec![]`
   - [x] Add `is_network_file` and `network_filenames` to `WalkCodecMap`
   - [x] Unit tests: defaults return empty; registered codec is consulted;
     `is_network_file` returns true for NETWORK_NAME without consulting codecs

### Phase 2: Subnet detection generalization (1 day)

2. Generalize `net_dir_partition` — `proto_index.rs`
   - [x] Pass 1: replace with `WALK_CODECS.is_network_file(filename)`
   - [x] Pass 2: same
   - [x] `discover_network_dirs`: same
   - [x] Unit test: mock `WalkCodec` with `network_filenames` returning a custom
     filename; verify `net_dir_partition` treats its directory as a candidate subnet

3. Cull false-positive subnets in `ProtoIndex::build`
   - [x] After `net_dir_partition`, iterate candidate subnet dirs whose network file
     is not `NETWORK_NAME`
   - [x] Call `CODECS.path_get(network_file)` → `codec.proto(path)`; if `None` or
     missing `BeliefKind::Network`, demote: remove from partition, merge children
     into parent group
   - [x] Unit test: directory with candidate filename but `proto()` returning `None`
     is demoted back to a regular child

4. Generalize `detect_network_file` — `network.rs`
   - [x] Check `NETWORK_NAME` first, then registered network filenames
   - [x] Extend `CodecManifest` with `network_filenames` for WASM bridge
   - [x] Unit test: with a registered network filename, `detect_network_file` finds it

### Phase 3: Path normalization (1 day)

5. Add `is_network_index_file` helper — `codec/mod.rs`
   - [x] Implement using `WALK_CODECS.is_network_file()`
   - [x] Replace `== NETWORK_NAME` in `builder.rs` (3 sites)
   - [x] Replace `== NETWORK_NAME` in `compiler.rs` (2 sites)
   - [x] Replace `== NETWORK_NAME` in `proto_index.rs` (2 sites: `owning_net_dir_for`,
     `sort_key_for`)
   - [x] Replace `== NETWORK_NAME` in `db.rs` (2 comparison sites; 2 path-construction
     sites deferred with TODO comments pending `payload["codec"]` availability)

### Phase 4: PathMap generalization (1.5 days)

6. Establish `payload["codec"]` contract for Network nodes
   - [x] `NetworkCodec::proto()`: set `document["codec"] = "index.md"`
   - [x] `ProtoIndex::proto_for()`: set if not already present (custom codec may
     have set it); fallback to detected network filename
   - [x] Unit test: `NetworkCodec::proto()` produces `document["codec"] == "index.md"`
   - [x] Document the invariant in `beliefbase_architecture.md`

7. Thread network filename through PathMap
   - [x] Add `network_filename: String` field to `PathMap`
   - [x] `PathMap::new` receives the filename as a parameter; uses it instead of
     `NETWORK_NAME` for initial entry, anchor prefix, and bare-anchor alias in
     `indexed_get`
   - [x] `PathMapMap::new`: read `payload["codec"]` from each network
     `BeliefNode`; pass to `PathMap::new`; default to `NETWORK_NAME` if absent
   - [x] `PathMapMap::process_event_queue`: same pattern for runtime PathMap rebuilds
   - [x] All other `PathMap::new` call sites (Default, api_map, asset_map, href_map):
     pass `NETWORK_NAME.to_string()`
   - [x] Integration test: network with non-`index.md` filename produces correct
     PathMap entries (covered by integration tests)

## Testing Requirements

### Unit Tests

- `WalkCodec::network_filenames` default returns empty
- `WalkCodecMap::is_network_file` checks NETWORK_NAME first, then codecs
- `net_dir_partition` with a mock walk codec: directory with candidate filename
  becomes a candidate subnet
- `ProtoIndex::build` culling: candidate subnet with `proto()` returning `None`
  is demoted; candidate with `proto()` returning `BeliefKind::Network` is kept
- `detect_network_file` with a registered network filename: finds non-index.md files
- `is_network_index_file` helper: true for index.md, true for registered filenames,
  false for unregistered filenames
- `NetworkCodec::proto()` sets `payload["codec"] = "index.md"`

### Integration Tests

- PathMap with a custom network filename: initial entry uses the custom filename,
  anchor prefix is `"custom.txt#slug"`, bare `#slug` resolves
- Full compile with a mock walk codec: directory with custom network file becomes a
  sub-network with correct child lists and PathMap

### Regression Guard

- All existing `ProtoIndex`, `NetworkCodec`, and `PathMap` tests pass unchanged
- `index.md`-only corpora produce identical output

## Success Criteria

- [x] A registered `WalkCodec` can declare `network_filenames`, causing
  `ProtoIndex` to treat directories containing those files as candidate sub-networks,
  confirmed by `DocCodec::proto()`. Verified via vast-noet integration tests.
- [x] `detect_network_file` finds non-`index.md` network files.
- [x] Builder and compiler path normalization handles non-`index.md` network files.
- [x] Network nodes carry `payload["codec"]` as a kind-level contract;
  value round-trips through `CODECS.get()`.
- [x] PathMap entries for non-`index.md` networks use the correct filename.
- [x] `CodecManifest` exports network filenames for WASM bridge.
- [x] All existing tests pass without modification (451+ noet-core tests).
- [x] No behavior change for corpora without custom walk codecs.
- [x] Document `payload["codec"]` invariant in `beliefbase_architecture.md`.
- [x] Integration test: network with non-`index.md` filename produces correct
  PathMap entries (covered by integration tests).

## Risks

- **Risk: Over-discovery in `net_dir_partition`** — A non-unique network filename
  (e.g. present in hundreds of directories) produces many candidate subnet dirs,
  most of which are culled by `proto()`. → **Mitigation**: Culling is cheap —
  `proto()` reads a small file and returns `None` quickly for non-network files.
  The walk pass itself is unchanged in cost; only `ProtoIndex::build` does extra
  work proportional to the number of false positives.

- **Risk: `net_dir_partition` ordering with mixed network types** — A directory
  containing both `index.md` and a custom network file. → **Mitigation**: `index.md`
  is checked first in `is_network_file`. A directory with `index.md` is always an
  `index.md` network; the custom file becomes a regular child.

## Open Questions

1. **Over-discovery culling heuristic**: `proto()` already populates `upstream`
   relations via `prepare_proto_relations`. A candidate subnet whose `proto()`
   returns an `IRNode` with empty `upstream` (no children) is a strong signal
   that it’s not a real network root. This could serve as an additional
   pre-emptive culling signal beyond `BeliefKind::Network` presence, reducing
   the number of false-positive partitions that persist into later phases.

## References

- Issue 68: Two-Registry Codec Architecture — `WALK_CODECS`, `CLAIM_MAP`,
  `DocCodec::parse` proto_index parameter
- Issue 72: Network Child Filtering — whitelist/blacklist, subnet suppression via
  `CLAIM_MAP.reject()`
- `src/properties.rs` — `BeliefNode::api_state()` payload contract precedent
- `src/codec/mod.rs` — `WalkCodec` trait, `WalkCodecMap`, `WALK_CODECS`
- `src/codec/network.rs` — `detect_network_file`, `NETWORK_NAME`, `NetworkCodec`
- `src/codec/proto_index.rs` — `net_dir_partition`, `ProtoIndex::build`,
  `discover_network_dirs`, `owning_net_dir_for`, `sort_key_for`
- `src/codec/builder.rs` — `initialize_stack`, `parse_content`, `build_path_key`
- `src/codec/compiler.rs` — `parse_one_path`, `process_one_parse_result`,
  `parse_epoch`, `process_unresolved_reference`
- `src/shard/manifest.rs` — `CodecManifest`, `codecs.json` WASM bridge
- `src/paths/pathmap.rs` — `PathMap::new`, `PathMap::indexed_get`,
  `PathMapMap`
- `src/db.rs` — `resolve_net_path`, `eval_unbalanced`
