# Issue 96: Bulk Asset Registration

**Priority**: HIGH
**Estimated Effort**: 1 day
**Dependencies**: None

## Summary

Asset files (images, PDFs, xlsx without index tabs) are processed one at a
time through the full `parse_task` → `GraphBuilder` → `terminate_stack`
lifecycle. For corpora with tens of thousands of media files (e.g., pandoc
`.media/` directories from converted presentations), this produces ~3
assets/second throughput — 3+ hours for 39K files. The per-asset work is
trivial (SHA-256 hash, BID lookup, 2-3 events); the cost is the heavyweight
document-parsing infrastructure wrapping it.

Replace the per-asset `parse_task` spawn with a bulk registration path that
processes the entire asset `remainder_queue` in one pass, emitting events
directly to the accumulator without GraphBuilder overhead.

## Goals

- Process asset batches at filesystem I/O speed, not GraphBuilder speed
- Eliminate `GraphBuilder::new`, `seed_session`, `initialize_stack`, and
  `terminate_stack` overhead for assets
- Maintain correctness: same BIDs, content hashes, Section edges, and
  PathMap entries as the current per-file path

## Architecture

The compiler's remainder loop (`parse_all` L1515) currently drains
`remainder_queue` into sub-epochs, spawning a `parse_task` per file.
For assets (processed count == 0, no codec claim), replace the spawn
with a bulk path:

```
remainder_queue (assets only)
    │
    ├─ parallel: read bytes + SHA-256 hash (rayon/tokio)
    │
    ├─ sequential: for each asset
    │     ├─ lookup BID in session_bb (by NodeKey::Path in asset_namespace)
    │     ├─ if miss: Bid::new(asset_namespace())
    │     ├─ if hash unchanged: skip
    │     ├─ emit NodeUpsert (External, content_hash in payload)
    │     ├─ emit RelationChange (asset → asset_namespace, Section)
    │     └─ emit PathAdded (asset_namespace bref, repo-relative path)
    │
    └─ single drain_epoch for the batch
```

The compiler already has `self.builder.session_bb()` which can look up
existing asset nodes by path key. Events are sent via `self.builder.tx()`
to the accumulator. No GraphBuilder instantiation needed.

## Steps

- [ ] Extract asset paths from remainder_queue candidates (count == 0,
      not in CLAIM_MAP, not tracked by WALK_CODECS)
- [ ] Implement `process_asset_batch` on `DocumentCompiler`:
  - Read file bytes and compute SHA-256 in parallel
  - Look up each asset BID in `session_bb` via `NodeKey::Path`
  - Compare content_hash; skip unchanged assets
  - Emit `NodeUpsert` + `RelationChange` events via `builder.tx()`
  - Ensure `asset_namespace` network node exists (once per batch)
- [ ] Call `process_asset_batch` in `parse_all` remainder loop before
      the per-file `parse_epoch` dispatch for remaining re-parse items
- [ ] Sync asset state into `session_bb` after drain so
      `epoch_session_snapshot` includes new assets for subsequent epochs
- [ ] Verify `process_asset_dir` (git-tracked asset directories) still
      works — it uses a different code path via `GraphBuilder`
- [ ] Test: existing asset tests pass unchanged
- [ ] Test: corpus with >100 media files processes in bulk

## Done When

- [ ] Assets bypass `parse_task` / `GraphBuilder` entirely
- [ ] Throughput > 100 assets/second (filesystem-bound, not GraphBuilder-bound)
- [ ] All existing tests pass
- [ ] repo media files process in minutes, not hours

## Risks

- Risk: `session_bb` lookup for existing assets may not have the asset
  if it was discovered in a previous epoch but not yet synced →
  **Mitigation**: `sync_asset_snapshot` already runs after each
  drain_epoch; bulk registration runs after epoch 0 drains.
- Risk: `process_asset_dir` (git metadata) shares code with
  `process_asset` → **Mitigation**: `process_asset_dir` is a separate
  code path in `GraphBuilder` that handles directory-level git status;
  it doesn't process individual asset files and is unaffected.
