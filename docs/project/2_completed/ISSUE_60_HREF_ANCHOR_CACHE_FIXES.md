# Issue 60: href/mailto Cache MISS and Bare Anchor Path Form Fixes

**Priority**: HIGH (was causing unresolved-link warnings on every parse)
**Status**: COMPLETE
**Estimated Effort**: 2 days (RELATIVE COMPARISON ONLY)
**Dependencies**: None

## Summary

Two independent sets of bugs caused persistent `UnresolvedReference` warnings
on valid links. First: href and mailto nodes were cache-missing on every re-parse
due to a Trace-filter over-exclusion and session_bb seeding gap. Second: bare
`#anchor` links in network index files always missed the PathMap because the
query key form (`"#quick-links"`) never matched the stored key form
(`"index.md#quick-links"`). Both were diagnosed via targeted instrumentation and
fixed at the correct abstraction layer.

## Goals

- Eliminate spurious `UnresolvedReference` warnings for href/mailto/asset links
- Eliminate spurious `UnresolvedReference` warnings for same-document `#anchor` links
- Establish `BeliefKind::External` as the canonical marker for permanently-Trace nodes

## Root Causes and Fixes

### Fix A: PathMap bare anchor alias (`src/paths/pathmap.rs`)

**Root cause**: `[foo](#quick-links)` inside a network's `index.md` is regularized
by `regularize_unchecked` into `Path { net: repo_bref, path: "#quick-links" }`.
`path_to` short-circuits on bare `#` inputs and returns the fragment unchanged.
`build_path_key` stores these as `"index.md#quick-links"` (NETWORK_NAME-prefixed).
The query key `"#quick-links"` never matched the stored key → permanent MISS on
every pass → promoted to warning.

**Fix**: In `PathMap::indexed_get`, transparently qualify bare anchor paths
(`#anchor`) to `index.md#anchor` before lookup. The PathMap is the single
canonical lookup authority used by all callers (`BeliefBase::filter_states`,
`DbConnection::eval_unbalanced`, `cache_fetch` via `eval_query`), so all call
sites get the fix without special-casing.

```rust
let qualified_path;
let path = if path.starts_with('#') {
    qualified_path = format!("{NETWORK_NAME}{path}");
    qualified_path.as_str()
} else {
    path
};
```

### Fix B: href node session_bb seeding gap (`src/codec/builder.rs`)

**Root cause**: `push_relation` created href nodes and emitted events to
`update_queue` and via `tx` to `global_bb`, but did NOT apply them directly to
`session_bb`. On the first parse, `href_namespace` was absent from `session_bb`,
so `process_event(RelationChange)` dropped the PathMap registration. On re-parse,
`cache_fetch` missed because the PathMap entry was never established.

**Fix**: Apply href events directly to `session_bb` in `push_relation`, mirroring
how `process_asset` / `process_asset_dir` handle their events. Order: namespace
node first, leaf node second, relation third.

### Fix C: Trace filter over-exclusion in cache_fetch (`src/codec/builder.rs`)

**Root cause**: `cache_fetch` used a bespoke `is_content_ns_key` predicate
(checking key net bref against href/asset namespace brefs) to decide whether to
accept a Trace node as a valid cache hit. href nodes are always `External | Trace`
by design — a correct PathMap hit was filtered out before being returned. The API
node (also permanently Trace) used a different bref and was not covered.

**Fix**: Replace the key-based predicate with `n.kind.contains(BeliefKind::External)`.
External is now the canonical marker for "permanently Trace by design — no deeper
fetch will return a non-Trace version." Applied to both the `doc_bb` check and the
`session_bb` check in `cache_fetch`.

### Fix D: BeliefKind::External on const namespace nodes (`src/properties.rs`)

**Root cause**: `api_state()`, `href_network()`, and `asset_network()` did not
include `BeliefKind::External` in their kind sets, making the permanently-Trace
nature of these nodes implicit and uncheckable.

**Fix**: Add `BeliefKind::External` to all three const namespace node constructors.
Enables Fix C to work uniformly across API, href, and asset namespace nodes.

### Fix E: AnchorPath::join schema stripping (`src/paths/path.rs`)

**Root cause**: `generate_path_name_with_collision_check` calls `sink_ap.join(terminal_path)`
where `terminal_path` is a full URL (e.g. `"mailto:user@example.com"`). The
`join()` guard short-circuited for hierarchical URLs (`https://` — has hostname)
but fell through for non-hierarchical URLs (`mailto:`, `javascript:`, `data:` —
no hostname). `filepath()` then stripped the schema, storing
`"user@example.com"` while queries used `"mailto:user@example.com"` →
permanent MISS.

**Fix**: Replace `has_hostname()` with `has_schema()` in `join()`. Any URL with
a schema is an absolute external reference; joining relative to a base path is
never correct.

```rust
// Before:
if end.is_absolute() || end.has_hostname() && end.hostname() != self.hostname() {
// After:
if end.is_absolute() || end.has_schema() {
```

**Secondary fix** (`src/codec/diagnostic.rs` `as_unresolved_source`): Was calling
`AnchorPath::from(&path).filepath()` on the raw `NodeKey::Path` string — same
schema-stripping hazard. Changed to `path.to_string()` directly.

### Fix F: initialize_stack WARN→DEBUG for expected root network slow path

**Root cause**: The root network's `index.md` legitimately has no parent network,
so falling through to the slow path is always expected for it. The WARN was noise.

**Fix**: Downgrade to DEBUG for the root network case; keep WARN only for child
networks whose parent was unexpectedly absent from `session_bb` / `global_bb`.

## Files Changed

| File | Change |
|------|--------|
| `src/paths/pathmap.rs` | Fix A: bare anchor alias in `indexed_get` |
| `src/paths/path.rs` | Fix E: `has_schema()` guard in `join()`; regression tests |
| `src/codec/builder.rs` | Fix B: href seeding into session_bb; Fix C: Trace exemption; Fix F: WARN→DEBUG |
| `src/codec/diagnostic.rs` | Fix E secondary: `filepath()` → raw path string |
| `src/properties.rs` | Fix D: `BeliefKind::External` on const namespace nodes |

## Testing Requirements

- All href/mailto links resolve without `UnresolvedReference` warnings on first and second parse
- Same-document `#anchor` links in network index files resolve correctly
- `cargo test` — all codec tests pass
- Idempotency: second parse produces zero graph-modifying events (`bid_tests` suite)

## Success Criteria

- [x] href/mailto `UnresolvedReference` warnings eliminated
- [x] Bare `#anchor` link warnings eliminated for network index files
- [x] `BeliefKind::External` established as canonical permanently-Trace marker
- [x] `join()` correct for non-hierarchical URL schemas (mailto, javascript, data)
- [x] All codec tests pass (409 passed, 0 failed, 3 ignored)
- [x] `bid_tests` idempotency suite green

## Notes

Diagnosed via targeted `tracing::warn!` instrumentation in `push()` and
`push_relation()`, which confirmed the bref and key forms produced by both passes
were consistent — the miss was a stored-vs-queried path form mismatch, not a BID
instability issue. Fix A was applied at the PathMap lookup layer (single authority)
rather than at individual call sites to avoid fragmented special-casing.

Fix E was identified separately by tracing the PathMap key mismatch for mailto
links through `generate_path_name_with_collision_check` → `AnchorPath::join` →
`filepath()`. The `has_hostname()` condition was too narrow by design; `has_schema()`
is the correct predicate.