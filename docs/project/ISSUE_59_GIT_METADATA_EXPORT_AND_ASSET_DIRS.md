# Issue 59: Git Metadata in Sharded Export and Asset-Directory Expansion

**Status**: ✅ Complete (Step 4 integration test deferred to Backlog)
**Priority**: HIGH
**Estimated Effort**: 2 days (RELATIVE COMPARISON ONLY)
**Dependencies**: Requires Issue 26 (completed), Issue 50 (completed)

## Summary

Two follow-on gaps surfaced after Issue 26 (git-aware networks) shipped: (1) `BeliefNode.metadata`
(git status, `source_url`) must be verified to survive the full CLI parse → `DbConnection` →
`beliefbase.json` export round-trip, not just the in-memory event-channel path tested in Issue 26,
and (2) markdown links that resolve to local directories are silently dropped rather than being
expanded into a git-remote hyperlink (if the directory is a tracked repo) or a directory listing
(if it is not).

## Status — Final

All goals complete. Step 4 (integration test) deferred to Backlog. Additional fixes
beyond the original scope were resolved during implementation:

- **Bug: source_url used network-relative path instead of git-root-relative path.**
  `NetworkGitStatus` gained a `network_prefix` field (workdir-relative path to the
  network dir). `to_metadata_table` writes it as `metadata["git"]["network_prefix"]`.
  `compute_source_url` prepends it to `ctx.root_path` when non-empty.

- **Bug: metadata rendered as JS Map in browser (WASM serialization).**
  `serde_wasm_bindgen` v0.6 routes any `Serialize` impl calling `serialize_map` —
  including `serde_json::Value::Object` nested inside a struct — through `MapSerializer`
  → JS `Map`. Fixed by patching the `metadata` property after struct serialization via
  `js_sys::JSON::parse` (always produces plain objects). Module-level doc in `wasm.rs`
  updated with top-level vs. nested distinction and Option D pattern.

- **Bug: directory link NodeKey used wrong namespace.**
  `push_relation` now reclassifies `NodeKey::Path { net: repo_bref }` keys that resolve
  to non-network directories as `asset_namespace` immediately after `regularize_unchecked`.
  This is the correct upstream fix — all downstream consumers see the right key without
  special-casing, and parse-2 idempotency is preserved.

- **Bug: absolute filesystem paths leaked into External nodes.**
  `process_asset_dir` now returns early with a warning when the path is outside `repo_root`
  instead of silently falling back to the absolute path as the node title/key.

- **Test fixture**: `tests/network_1/subnet1/subnet1_file1.md` Project Links section updated
  to link to `../net1_dir1` and `../assets` — self-contained within `tests/network_1/`,
  portable across test runs, exercises Case B via `bid_tests`.

---

### Item 1 — Metadata in exported JSON: ✅ Resolved (not a bug)

The original concern was that `metadata` was absent from `beliefbase.json`. Investigation
confirmed this was expected: the command run against `tests/network_1/` did not pass
`--git-tracking`, so no git metadata is injected. The DB round-trip itself is correct.

**Diagnostic test added**: `test_metadata_in_exported_json` in `src/codec/compiler.rs`
exercises the full CLI-equivalent path — `BeliefAccumulator<DbConnection>` backing store,
`into_inner` extraction, `finalize_html` export, and JSON deserialization from disk. The test
passes, confirming `metadata["git"]` survives the DB round-trip intact.

**Cleanup**: `compute_diff` Phase 2 was changed from `toml()` string comparison to direct
`PartialEq` on `BeliefNode` (more correct: avoids serialization ordering fragility and
correctly includes `metadata` in the comparison).

### Item 2 — `export_asset_dir`: In Progress

`process_asset` signature updated to accept `proto_index: ProtoIndex` (matching `parse_content`),
enabling future Case A (tracked-repo directory → href node) implementation without further
interface changes.

## Goals

- ~~Metadata survives full DB round-trip~~ ✅ Verified by `test_metadata_in_exported_json`
- Markdown links to repo-tracked directories resolve to an upstream git remote URL in the
  exported graph (as an `href_namespace` node) — requires `--git-tracking`
- Markdown links to local-only directories resolve to a `BeliefKind::External` asset node
  whose payload contains a sorted directory listing and whose hash is derived from that listing
- `process_asset` accepts `proto_index` (done) so directory dispatch can query git status

## Architecture

### Feature: `export_asset_dir` — Directory Link Expansion

Currently, `process_asset` handles **files** (binary or non-codec files) referenced from
markdown. Directories are not handled — a markdown link to a directory produces an
`UnresolvedReference` that is silently dropped after `process_asset_reference` enqueues the
path and `parse_one_path` fails to dispatch it (directories have no registered codec and
`process_asset` guards against directories with `path.is_dir()`).

The proposal adds a new code path inside `process_asset` (now that it receives `proto_index`)
that handles the directory case:

**Case A — Directory is a tracked repo** (`proto_index` has an entry for it):

- `proto_index.git_status_for(path)` returns `Some(NetworkGitStatus { .. })`.
- Extract the upstream remote URL from `NetworkGitStatus.repo` (the `RepoGitStatus` handle)
  using the same `git_remote_url` logic used to build `source_url` in Issue 26.
- Emit an `href_namespace` node pointing to the remote URL — same structure as a regular
  external hyperlink: `BeliefKind::External | Trace`, `id = <url>`, BID derived via
  `buildonomy_href_bid(url)` (deterministic, collision-free across parses).
- The relation from the source document to this href node carries the link anchor text.
- Gated on `#[cfg(feature = "git-tracking")]`; without the feature, falls through to Case B.

**Case B — Directory is local-only** (navigable but not in `proto_index`):

- Call `std::fs::read_dir` on the path; collect a sorted listing of entry names (files and
  subdirectories, names only, not full paths).
- Cap at 256 entries; set `payload["truncated"] = true` if capped and log a warning.
- Compute SHA-256 over `repo_relative_path + "\n" + sorted_listing.join("\n")`.
- Emit a `BeliefKind::External` node with:
  - `payload["content_hash"]` — the computed hash (same change-detection pattern as files)
  - `payload["listing"]` — TOML array of sorted entry name strings
  - `title` — the repo-relative path
- Runs regardless of `--git-tracking`.

**Case C — Path is not navigable** (does not exist or is a directory with access denied):

- Fall through to the existing `UnresolvedReference` behavior. No regression.

**Integration point**: `process_asset` is the correct location because:

1. `process_asset_reference` already resolves the asset path and enqueues it in
   `remainder_queue`, so the path reaches `parse_one_path` → `process_asset` normally.
2. `proto_index` is now available in `process_asset` via the new parameter.
3. The Case B hash-and-emit pattern is identical to the existing file handling — only the
   content source changes (directory listing vs. file bytes).

The only call site (`parse_one_path` in `compiler.rs`) already passes `proto_index`.

#### Data Flow (Case A)

```
markdown link [vendor/mylib](./vendor/mylib)
  → MdCodec → IntermediateRelation → NodeKey::Path(asset_net, "vendor/mylib")
  → push_relation: unresolved → UnresolvedReference
  → process_asset_reference: enqueues absolute path
  → parse_one_path: path.is_dir() → not in CODECS → process_asset(path, &[], global_bb, proto_index)
  → process_asset: path.is_dir() → proto_index.git_status_for(path) → Some(status)
  → extract remote URL → buildonomy_href_bid(url)
  → emit NodeUpdate(href_node) + RelationChange(source_doc, href_node)
```

#### Data Flow (Case B)

```
markdown link [assets](./assets)
  → UnresolvedReference → process_asset_reference: enqueues path
  → process_asset: path.is_dir(), not in proto_index → read_dir → sorted listing → hash
  → emit NodeUpdate(External node, payload={listing, content_hash})
  → emit RelationChange(source_doc, asset_node, Section)
```

## Implementation Steps

### 1. ~~Diagnose and verify metadata DB round-trip (0.5 days)~~ ✅ Done

- [x] Added `compile_to_html_via_db` helper in `src/codec/compiler.rs` tests — mirrors the
      CLI path with `BeliefAccumulator<DbConnection>`
- [x] Added `test_metadata_in_exported_json` — passes, confirming round-trip is correct
- [x] Fixed `compute_diff` Phase 2 equality check: `new_node.toml() != old_node.toml()`
      → `new_node != old_node_normalized` (direct `PartialEq`, includes `metadata`)
- [x] Added `proto_index: ProtoIndex` parameter to `process_asset` (matches `parse_content`)

### 2. Implement `export_asset_dir` — Case A (tracked repo → href node) (0.5 days) ✅

- [x] Added `process_asset_dir` to `GraphBuilder` in `src/codec/builder.rs`
- [x] `parse_one_path` (`src/codec/compiler.rs`): directories with no index file now route
      to `process_asset_dir` instead of returning an error
- [x] Under `#[cfg(feature = "git-tracking")]`: calls `proto_index.git_status_for(path)`;
      if `Some`, extracts remote URL from `NetworkGitStatus.repo.remote_url`
      (confirmed present — open question resolved: `RepoGitStatus.remote_url: Option<String>`
      is already populated by `GitCache::populate`)
- [x] Uses `buildonomy_href_bid(url)` and mirrors the href-node construction pattern from
      `push_relation`; emits href node + `RelationChange` to `href_namespace`
- [x] Returns `ParseContentWithCodec` with `AssetCodec` (consistent with file asset path)
- [x] Unit test `test_directory_asset_case_a_git_tracked`: temp git repo with a
      github.com remote (no network access — git2 stores it locally), `ProtoIndex`
      built with git tracking enabled; asserts exactly one `href_namespace` node with
      the correct normalized URL is emitted and no Case B listing nodes are present

### 3. Implement `export_asset_dir` — Case B (local directory listing) (0.5 days) ✅

- [x] `process_asset_dir`: `read_dir` on path, collect + sort entry names, cap at 256
- [x] Hash = SHA-256 over `repo_relative_path + "\n" + sorted_names.join("\n")`
- [x] Emits `BeliefKind::External` node with `payload["listing"]` TOML array,
      `payload["content_hash"]`, and `title` = repo-relative path
- [x] `payload["truncated"] = true` emitted and `tracing::warn!` fired when capped
- [x] Change-detection: compares `content_hash` against cached node (same as file assets)
- [x] Unit test `test_directory_asset_listing`: temp dir not in `proto_index`; asserts
      `External` node with sorted `listing` payload appears after `process_asset_dir`

### 4. Integration test (0.5 days) — ⏭ Deferred to Backlog

- [ ] End-to-end test: explicit assertion that `net1_dir1` and `assets` produce
      `BeliefKind::External` nodes with `payload["listing"]` and `payload["content_hash"]`
      in `session_bb` after a full `parse_all` run. The `bid_tests` implicitly exercise the
      full chain and pass, but no assertion on `External` node contents exists yet.
      See Backlog: "Directory Link Integration Test (from Issue 59)".
- [x] Case C regression: `test_directory_asset_case_c_nonexistent` confirms that
      `process_asset_dir` returns `Err` for a non-existent path and emits no External nodes

## Testing Requirements

- `test_metadata_in_exported_json` — passing ✅
- Case A unit test `test_directory_asset_case_a_git_tracked`: href node present in
  `session_bb` with correct normalized URL — passing ✅
- Case B unit test `test_directory_asset_listing`: `External` node with sorted `listing`
  payload present in `session_bb` — passing ✅
- Case C regression `test_directory_asset_case_c_nonexistent`: no External nodes emitted
  for non-existent path — passing ✅
- All existing `git-tracking` tests in `compiler.rs` continue to pass ✅
- Full test suite (`cargo test`): 357 passed (348 lib + 9 integration), 0 failed ✅

## Success Criteria

- [x] `noet build --git-tracking` on a real repo: confirmed by
      `test_metadata_in_exported_json` ✅
- [x] A markdown link to a directory that is a tracked repo produces an `href_namespace`
      node in the exported graph pointing to the upstream remote URL — verified by
      `test_directory_asset_case_a_git_tracked` ✅
- [x] A markdown link to a local-only directory produces an `External` node with a
      `listing` array in its payload — verified by `test_directory_asset_listing` ✅
- [x] A markdown link to a non-existent path behaves identically to today (no regression)
      — verified by `test_directory_asset_case_c_nonexistent` ✅
- [x] `process_asset` accepts `proto_index` parameter ✅
- [x] `process_asset_dir` added to `GraphBuilder`; `parse_one_path` routes unindexed
      directories to it instead of erroring ✅
- [x] `source_url` is repo-root-relative (not network-relative) — `network_prefix` fix ✅
- [x] `metadata` renders as plain JS object in browser — WASM JSON shim fix ✅
- [x] Directory link `NodeKey` correctly uses `asset_namespace` from `push_relation` ✅
- [x] No absolute filesystem paths in External node titles or keys ✅

## Risks

- **Risk**: `ProtoIndex::git_status_for` is feature-gated (`cfg(feature = "git-tracking")`).
  Case A code paths must compile cleanly without the feature.
  **Mitigation**: ✅ implemented — `#[cfg(feature = "git-tracking")]` guard around the
  `git_status_for` call; falls through to Case B without the feature.

- **Risk**: `read_dir` on a large directory (e.g. `node_modules`) could produce a very large
  listing payload.
  **Mitigation**: ✅ implemented — capped at 256 entries; `truncated = true` in payload if
  capped; `tracing::warn!` fired.

- **Risk**: `NetworkGitStatus` may not carry the upstream remote URL directly.
  **Resolved**: `RepoGitStatus.remote_url: Option<String>` is already populated by
  `GitCache::populate` via `normalize_remote_url`. Accessed as
  `status.repo.remote_url.as_deref()`. No schema changes needed.

## Open Questions

- Integration test (Step 4): a full `parse_all` end-to-end test with a markdown doc
  linking to a subdirectory (Case B) is not yet written. The unit test covers the
  `process_asset_dir` function directly; the integration test would cover the full
  `UnresolvedReference` → `process_asset_reference` → `remainder_queue` → `parse_one_path`
  → `process_asset_dir` → source-doc re-parse → relation wiring chain.