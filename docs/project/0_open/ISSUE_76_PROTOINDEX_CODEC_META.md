# Issue 76: ProtoIndex Codec Metadata Cache

**Priority**: MEDIUM
**Estimated Effort**: 2 days (RELATIVE COMPARISON ONLY)
**Dependencies**: None (standalone infrastructure). Unblocks Backlog item "Site-Root-Relative Slug Resolution via Slug Namespace"

## Summary

`ProtoIndex` currently stores network-level codec metadata via a hardcoded, feature-gated `git_cache: Arc<GitCache>` field. This is a specific instance of a general pattern: network codecs need to stash metadata during the build scan (or during Phase 1 network parsing) that child codecs read during their own `parse()`. Replace `git_cache` with a generic, serde-based metadata map that any codec can populate and any other codec can read, keyed by canonical network directory path and namespaced by string key.

## Goals

1. Add a generic `codec_meta` field to `ProtoIndex` — a serde-based map keyed by `(PathBuf, String)` (network dir × namespace).
2. Migrate `GitCache` to populate this map under namespace `"git"` during `ProtoIndex::build`.
3. Migrate all consumers of `git_status_for()` and `proto_for()` to deserialize from the generic map.
4. Remove the `git_cache` struct field from `ProtoIndex` (the `GitCache` type remains as the computation engine; it just writes to `codec_meta` instead of a dedicated field).
5. Maintain feature-gate semantics: when `git-tracking` is disabled, no git metadata is computed or stored.

## Architecture

### Current state

ProtoIndex has:
- `inner: Arc<RwLock<HashMap<PathBuf, Vec<PathBuf>>>>` — children map
- `git_cache: Arc<GitCache>` — feature-gated, hardcoded type with `HashMap<PathBuf, NetworkGitStatus>`

Consumers access git metadata via:
- `proto_for(dir)` → returns `(IRNode, Option<NetworkGitStatus>)` — used by `initialize_stack`
- `git_status_for(dir)` → returns `Option<&NetworkGitStatus>` — used by `parse_content` and `process_asset_dir`

### Target state

```
pub struct ProtoIndex {
    inner: Arc<RwLock<HashMap<PathBuf, Vec<PathBuf>>>>,
    codec_meta: Arc<RwLock<HashMap<PathBuf, HashMap<String, serde_json::Value>>>>,
}
```

Accessors:
- `set_meta(dir: &Path, namespace: &str, value: serde_json::Value)` — canonicalizes path, inserts into nested map
- `get_meta(dir: &Path, namespace: &str) -> Option<serde_json::Value>` — canonicalizes path, looks up and clones
- `get_meta_as<T: DeserializeOwned>(dir: &Path, namespace: &str) -> Option<T>` — convenience wrapper, deserializes in place

`GitCache::populate()` continues to do the efficient `Arc<RepoGitStatus>` shared computation internally, but writes flattened `NetworkGitStatus` entries into `codec_meta` under namespace `"git"` via `serde_json::to_value()`. This requires `NetworkGitStatus` (and the repo-level fields it contains) to derive `Serialize`.

### Serialization of NetworkGitStatus

`NetworkGitStatus` currently holds `repo: Arc<RepoGitStatus>`. For serialization:
- Derive `Serialize` on both `RepoGitStatus` and `NetworkGitStatus`
- Use `#[serde(flatten)]` on the `repo` field, or serialize the `Arc` transparently (serde handles `Arc<T>` when `T: Serialize`)
- The `PathBuf` fields serialize as strings naturally

For deserialization, consumers use `get_meta_as::<NetworkGitStatus>()` which deserializes from the JSON value. The `Arc<RepoGitStatus>` is reconstructed as a fresh `Arc` per deserialization — the sharing optimization is internal to `GitCache::populate()` and not preserved across the serde boundary. This is acceptable: the metadata is read infrequently (once per network during `initialize_stack` and `parse_content`).

### Feature gate migration

The `git-tracking` feature gate moves from the struct field level to the population call site:
- `codec_meta` field is always present (no feature gate)
- `GitCache::populate()` call in `ProtoIndex::build` remains gated on `#[cfg(feature = "git-tracking")]`
- When the feature is off, namespace `"git"` simply has no entries
- `git_status_for()` becomes `get_meta_as::<NetworkGitStatus>(dir, "git")` — returns `None` when no entries exist, regardless of feature gate

This eliminates the placeholder `#[cfg(not(feature = "git-tracking"))] pub struct NetworkGitStatus;` in `proto_index.rs`.

## Implementation Steps

1. Add `codec_meta` field and accessors (0.5 days)
   - [x] Add `codec_meta: Arc<RwLock<HashMap<PathBuf, HashMap<String, serde_json::Value>>>>` to `ProtoIndex`
   - [x] Implement `set_meta()`, `get_meta()`, `get_meta_as::<T>()`
   - [x] Canonical path normalization in both accessors (same pattern as `GitCache::get`)

2. Derive `Serialize` on git types (0.25 days)
   - [x] Add `#[derive(Serialize)]` to `RepoGitStatus` and `NetworkGitStatus`
   - [x] Handle `Arc<RepoGitStatus>` serialization (serde supports `Arc<T>` natively when T: Serialize)
   - [x] Add `#[derive(Deserialize)]` for the deserialization path
   - [x] Verify round-trip: `to_value(status)` → `from_value::<NetworkGitStatus>()` preserves all fields

3. Migrate GitCache to populate codec_meta (0.5 days)
   - [x] In `ProtoIndex::build`, after `GitCache::populate()`, iterate `git_cache.by_network` and call `set_meta(dir, "git", to_value(&status))` for each entry
   - [x] Remove `git_cache` field from `ProtoIndex`
   - [x] Remove placeholder `NetworkGitStatus` for non-git-tracking builds

4. Migrate consumers (0.5 days)
   - [x] `git_status_for(dir)` → `get_meta_as::<NetworkGitStatus>(dir, "git")`
   - [x] `proto_for(dir)` return type changes: `Option<serde_json::Value>` read from `codec_meta` instead of `git_cache`
   - [x] `parse_content` Phase 1 git override: read from `proto_index.get_meta_as::<NetworkGitStatus>(...)`
   - [x] `process_asset_dir` git status reads: same migration

5. Tests (0.25 days)
   - [x] Existing `test_directory_asset_case_a_git_tracked` passes with migrated accessors
   - [x] New test: `set_meta` / `get_meta_as` round-trip with a custom type
   - [x] New test: `get_meta` returns `None` for unknown namespace / unknown dir

## Testing Requirements

- All existing git-tracking tests pass without modification (behavior identical)
- Round-trip test: arbitrary `Serialize + Deserialize` type survives `set_meta` → `get_meta_as`
- Feature gate test: `git-tracking` disabled → `get_meta_as::<NetworkGitStatus>(_, "git")` returns `None`
- Concurrent access test: `set_meta` from one thread, `get_meta` from another (validates `RwLock` safety)

## Success Criteria

- [x] `git_cache` field removed from `ProtoIndex` struct
- [x] Placeholder `#[cfg(not(feature = "git-tracking"))] pub struct NetworkGitStatus` removed
- [x] All codec tests pass (451 unit + 23 codec + 8 integration + 38 doc-tests, both feature configs)
- [x] `test_directory_asset_case_a_git_tracked` passes with migrated accessors

## Risks

- **Serde overhead on hot path**: `serde_json::from_value` on every `get_meta_as` call vs. the current direct struct access. → **Mitigation**: git metadata is read once per network during `initialize_stack` + `parse_content`, not per-file. The overhead is negligible.
- **`Arc<RepoGitStatus>` sharing loss**: Each deserialization creates a fresh `Arc` instead of sharing across networks in the same repo. → **Mitigation**: The `Arc` sharing was a memory optimization inside `GitCache::populate()`. The deserialized copies are small (a few strings) and short-lived. No measurable impact.

## Open Questions

1. ~~Should `get_meta_as` return `Result<Option<T>, serde_json::Error>` or silently return `None` on deserialization failure?~~ **RESOLVED**: Returns `Option<T>` with a `tracing::warn` on deserialization failure. Matches the graceful degradation pattern used throughout the codebase.
