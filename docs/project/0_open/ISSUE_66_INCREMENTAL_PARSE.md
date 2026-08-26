---
version = "0.1"
title = "Issue 66: Incremental Parse via Shard Mtimes"
---

# Issue 66: Incremental Parse via Shard Hydration

**Priority**: MEDIUM
**Estimated Effort**: 5.5 days (RELATIVE COMPARISON ONLY)
**Dependencies**: Requires Issue 50 (sharding), Requires Issue 64 (MCP — first consumer of static shard loading); Informs Issue 11 (LSP); Supersedes Issue 41 (WebSocket shard invalidation replaces BeliefEvent streaming)

## Summary

`noet parse` currently re-parses every source file on every invocation, and
`noet watch` maintains a file-based SQLite `belief_cache.db` as its cross-invocation
store. Both are unnecessary once shards are treated as the durable, structured
representation of a completed parse pass.

This issue makes shards the authoritative cross-invocation artifact by: (1) embedding
per-network `compiled_at` timestamps and source mtimes into the shard manifest so
clean networks can be skipped on re-parse; (2) hydrating the in-memory DB from shards
at startup so `noet watch` no longer needs a file-based DB; and (3) exposing the
`last_diagnostics` accessor that MCP (`check_consistency`) and LSP
(`publishDiagnostics`) need. A configurable memory budget governs how much shard data
is kept in the in-memory DB at once, with eviction back to shard files when the limit
is approached.

**Lifecycle clarification**: the in-memory DB is *authoritative* while the process is
running. During a `watch` session, multiple consumers (browser viewer, MCP clients,
LSP clients) query the live in-memory DB concurrently. Shards are *checkpoints* for
cold-start hydration and for HTML output, not the primary query surface. The framing
"stateless between restarts" applies to the cold-start path (no persistent DB file
needed), but during a session the DB is the live authority.

The existing `--db` flag is replaced by `--debug-db`, which writes the in-memory DB
state to a file for developer inspection (`sqlite3 /tmp/noet-debug.db`) without
changing the startup sequence. See Architecture § File-based DB for debugging.

## Goals

- `noet parse` skips networks whose shard is newer than all constituent source files,
  reducing re-parse time proportionally to the unchanged fraction of the corpus
- `noet watch` eliminates its file-based `belief_cache.db`: the in-memory DB is
  hydrated from shards at startup and updated incrementally on each dirty-network
  re-parse, making the watch daemon stateless between restarts. During a session,
  the in-memory DB is the authoritative query surface for all consumers (browser
  viewer, MCP, LSP)
- Per-network `compiled_at` timestamp and `source_mtimes` embedded in `NetworkShardMeta`,
  readable by MCP `check_consistency` and the incremental skip logic
- `DocumentCompiler::last_diagnostics()` accessor exposing the diagnostic snapshot
  from the last completed parse pass — consumed by MCP `check_consistency` (live mode)
  and LSP `publishDiagnostics` (Issue 11)
- Configurable in-memory DB memory budget: networks are evicted from the DB (back to
  their shard file) when the budget is approached, and reloaded on demand
- `--force` flag on `noet parse` bypasses incremental skip logic (already exists;
  must remain respected)
- No behavioral change when sharding is disabled (monolithic mode)

## Architecture

### Unified startup model

Both `noet parse` and `noet watch` already use an **ephemeral in-memory SQLite DB**
as `global_bb` during compilation (see `db_init_memory()` in `src/bin/noet/main.rs`).
The only difference is that `noet watch` additionally maintains a persistent
`belief_cache.db` for cross-invocation state. After this issue, the startup sequence
is identical for both commands:

```
Startup:
  1. Read existing shard manifest (if present) → identify clean/dirty networks
  2. Hydrate in-memory DB from clean network shards (within memory budget)
  3. Parse only dirty networks → stream events into in-memory DB
  4. finalize_html → write updated shards from DB state + emit last_diagnostics snapshot

noet watch (continuous loop):
  File change → mark containing network dirty → repeat steps 3-4 for dirty set only
```

`noet watch` becomes stateless between restarts: it always cold-starts from shards,
never needs `belief_cache.db`. The file-based DB is deleted from the watch startup
path once this issue ships.

### Mtime embedding in `NetworkShardMeta`

Add two fields to `NetworkShardMeta` in `src/shard/manifest.rs`:

```
compiled_at:   String   // RFC 3339 UTC timestamp of when this shard was written
source_mtimes: BTreeMap<String, u64>  // relative path → Unix mtime (seconds)
```

`source_mtimes` captures the mtime of every source file that contributed to this
network at the time the shard was written. On the next `noet parse` invocation,
the compiler reads the current mtime of each file and compares: if all current
mtimes are ≤ the stored values, the network is clean and can be skipped.

`compiled_at` is a human-readable ISO 8601 string exposed in Issue 64's
`check_consistency` output and used by the incremental skip logic to log which
networks were reused. `source_mtimes` is not exposed via MCP — it is an internal
implementation detail of the skip logic.

### Skip logic in `DocumentCompiler`

Before dispatching parse work for a network, `DocumentCompiler` (or its caller in
`main.rs`) checks:

1. Does a shard exist for this network? (manifest present, file exists on disk)
2. Is `--force` absent?
3. Are all current source file mtimes ≤ `source_mtimes` stored in the shard meta?

If all three: skip the network. Emit a `tracing::debug!` line noting the skip and
the shard age. If any source file is newer, or the shard is absent: proceed with
normal parse.

**Deleted files**: if a file present in `source_mtimes` no longer exists on disk,
treat the network as dirty (a deletion may have removed a node — must re-parse to
detect orphans).

**New files**: the file walker discovers files not in `source_mtimes` — treat as
dirty.

### Shard hydration into in-memory DB

At startup, after reading the manifest and identifying clean networks, the clean
shards are loaded into the in-memory DB via a new `hydrate_from_shards` function:

```rust
async fn hydrate_from_shards(
    db: &DbConnection,
    output_dir: &Path,
    manifest: &ShardManifest,
    dirty_brefs: &HashSet<String>,
    memory_budget_mb: f64,
) -> Result<(), BuildonomyError>
```

Each clean network's `{bref}.msgpack` is deserialized and its nodes/edges are
inserted into the DB via the existing `Transaction::add_event` path — the same path
used during live parse. Networks are hydrated in ascending `estimated_size_mb` order
until the budget is reached; remaining clean networks are left on disk and loaded on
demand when a query touches them.

The `ShardConfig::memory_budget_mb` field (already present, already used by the
browser viewer to cap client-side shard loading) is reused here as the server-side
in-memory DB budget. The same concept — "how much shard data to keep hot" — applies
to both consumers.

### Shard eviction and on-demand reload

When the in-memory DB approaches the memory budget during a watch session (e.g. after
many incremental re-parses have added data), networks can be evicted by removing their
nodes/edges from the DB (using the existing `Transaction::remove_nodes` path) and
marking them as "on-disk only". A subsequent query that touches an evicted network
triggers a reload from its shard file.

This gives operators a knob — `memory_budget_mb` in `ShardConfig` or a new
`--memory-budget` CLI flag — to tune the watch daemon's footprint without sacrificing
query correctness.

**Multi-consumer awareness**: during a `watch` session, the in-memory DB serves
multiple concurrent consumers (browser viewer via local API, MCP clients, LSP
clients). Eviction must not occur while a query is in flight. The
`compiler_idle_notify` signal in `FileUpdateSyncer` remains the correct eviction
trigger boundary, but the idle detection must account for active queries from all
consumers, not just the compiler.

### File-based DB for debugging

The current `--db` flag creates a persistent `belief_cache.db` that serves as
cross-session state. This issue replaces it with `--debug-db <path>`, which serves
a different purpose: **write-only debugging output**.

- The DB is always initialized from shards (or empty) on startup — never read
  from a prior file-based DB.
- `--debug-db /tmp/noet-debug.db` causes the in-memory DB to be backed by a
  file at the specified path, overwritten each session.
- The file is never read on subsequent startups. It exists solely for developer
  inspection: `sqlite3 /tmp/noet-debug.db` to examine graph state, run ad-hoc
  queries, debug edge resolution, etc.
- Can also be enabled via `NOET_DEBUG_DB=path` environment variable.

This is distinct from the eliminated `belief_cache.db` (cross-session persistence)
and from the in-memory DB (live query surface). The debug DB is a window into the
live session, not a cache or persistence layer.

### `last_diagnostics` accessor on `DocumentCompiler`

Add a `last_diagnostics` field to `DocumentCompiler`:

```rust
last_diagnostics: HashMap<PathBuf, Vec<ParseDiagnostic>>,
```

Populated by taking a snapshot of `latest_results` diagnostics **before** the drain
in `parse_all` / `parse_sequential`. Exposed via:

```rust
pub fn last_diagnostics(&self) -> &HashMap<PathBuf, Vec<ParseDiagnostic>>
```

This is the correct source for:
- MCP `check_consistency` live mode: filter for `ParseDiagnostic::UnresolvedReference`
- LSP `publishDiagnostics` (Issue 11): filter by document path

The change is purely additive — no behavior change to existing callers.

### `BeliefSource` trait

**Note**: `BeliefSource` as a shard-loading abstraction (the original intent here)
conflicts with the existing `BeliefSource` query-execution trait in `src/query.rs`.
The shard-loading abstraction needs a distinct name — `ShardLoader` or
`ShardBeliefSource` are candidates. Decide at implementation time based on import
topology.

The shard-loading abstraction remains useful for MCP static mode as a `TODO(Issue 66)`
hook in `src/mcp/state.rs`, but the primary motivation — eliminating the file DB from
`noet watch` — is better served by the hydration approach above, which reuses the
existing `DbConnection` / `BeliefAccumulator` infrastructure rather than introducing
a new trait.

### Monolithic mode

When the export is below the shard threshold, `beliefbase.msgpack` is written
instead of a `beliefbase/` directory. Incremental skip does not apply in monolithic
mode (the whole graph is one file; there is no per-network manifest to consult).
`ShardBeliefSource` detects monolithic mode by the absence of `beliefbase/manifest.json`
and falls back to loading the single `beliefbase.msgpack`.

## Implementation Steps

1. **Add `compiled_at` + `source_mtimes` to `NetworkShardMeta`** (0.5 days)
   - [ ] Add `compiled_at: String` and `source_mtimes: BTreeMap<String, u64>` to
         `NetworkShardMeta` in `src/shard/manifest.rs`; annotate both with
         `#[serde(default)]` for backward compatibility with old manifests
   - [ ] Populate both fields in `export_sharded` (`src/shard/export.rs`): capture
         `SystemTime::now()` as `compiled_at`; collect source file mtimes from the
         `PathMap` entries for the network
   - [ ] Update `network_shard_meta()` constructor to accept and thread through
         both new fields
   - [ ] Confirm `ShardManifest` JSON roundtrip test still passes

2. **`last_diagnostics` accessor on `DocumentCompiler`** (0.25 days)
   - [ ] Add `last_diagnostics: HashMap<PathBuf, Vec<ParseDiagnostic>>` field to
         `DocumentCompiler` in `src/codec/compiler.rs`
   - [ ] Snapshot diagnostics from `latest_results` immediately before the drain in
         `parse_all` / `parse_sequential`; store in `last_diagnostics`
   - [ ] Expose via `pub fn last_diagnostics(&self) -> &HashMap<PathBuf, Vec<ParseDiagnostic>>`
   - [ ] Wire into MCP `check_consistency` live mode: filter for
         `ParseDiagnostic::UnresolvedReference` via
         `Arc<RwLock<DocumentCompiler>>` from `FileUpdateSyncer`
   - [ ] Note: Issue 11 (LSP `publishDiagnostics`) is the second consumer — add
         the accessor once, coordinate to avoid duplication

3. **Incremental skip + shard hydration in parse/watch startup** (1.5 days)
   - [ ] Read existing `beliefbase/manifest.json` at startup (both `noet parse` and
         `noet watch`); classify each network as clean or dirty by comparing stored
         `source_mtimes` against current `fs::metadata(path).modified()` values
   - [ ] Handle missing files (dirty), new files (dirty), `--force` (all dirty)
   - [ ] Implement `hydrate_from_shards(db, output_dir, manifest, dirty_brefs,
         memory_budget_mb)`: deserialize each clean network's `{bref}.msgpack` and
         insert into the in-memory DB via `Transaction::add_event`; hydrate in
         ascending `estimated_size_mb` order until budget is reached; leave remaining
         clean networks on disk for on-demand reload
   - [ ] Call `hydrate_from_shards` before `parse_all` / the watch loop, passing the
         classified dirty set
   - [ ] Parse only dirty networks; clean networks' data is already in the DB
   - [ ] Add a summary line at `tracing::info!` level:
         `"N/M networks reused from shard cache; K networks re-parsed"`
   - [ ] Remove `belief_cache.db` file creation from `noet watch` startup path;
         replace with in-memory DB + hydration
   - [ ] Replace `--db` CLI flag with `--debug-db <path>` (and `NOET_DEBUG_DB`
         env var): write-only file-backed DB for developer inspection, never
         read on startup

4. **WebSocket shard-invalidation endpoint** (0.5 days)
   - [ ] Add a WebSocket endpoint to `noet watch`/`noet serve` at `/events`
   - [ ] After each incremental re-parse + shard write, broadcast one message per
         regenerated network: `{"type": "shard_updated", "bref": "<bref>",
         "compiled_at": "<ISO 8601>", "kind": "network"}`
   - [ ] Broadcast a separate `{"type": "shard_updated", "kind": "search_index"}`
         message when the TF-IDF search index is regenerated (distinct message type;
         different reload logic in the SPA)
   - [ ] SPA subscribes on startup; on `shard_updated`, fetches and replaces only the
         affected network shard without page reload, preserving scroll position and
         UI state
   - [ ] On WebSocket reconnect, SPA fetches `manifest.json` (small — one entry per
         network), diffs `compiled_at` per network against its in-memory cache, and
         reloads only the stale networks — no full shard reload needed
   - [ ] This replaces the `BeliefEvent` streaming approach from Issue 41; close
         Issue 41 as OBE once this step ships

5. **Shard eviction and on-demand reload** (0.5 days)
   - [ ] Track per-network "hot" flag in the in-memory DB session
   - [ ] When the DB size approaches `memory_budget_mb`, evict the least-recently-used
         network by removing its nodes/edges via `Transaction::remove_nodes` and
         marking it "on-disk"
   - [ ] On a query that touches an evicted network, reload from its shard file
   - [ ] Expose `--memory-budget <MB>` CLI flag on `noet watch` (default: reuse
         `ShardConfig::DEFAULT_MEMORY_BUDGET_MB`)

6. **Tests** (0.75 days)
   - [ ] Unit test: fixture shard with known `source_mtimes`; assert network skipped
         when no source is newer; assert re-parsed when one mtime is bumped
         (use `filetime` crate, already in `dev-dependencies`)
   - [ ] Unit test: `hydrate_from_shards` against a fixture output directory; assert
         the in-memory DB contains the expected node count after hydration
   - [ ] Unit test: eviction + reload cycle; assert query results identical before
         and after eviction
   - [ ] Regression: `noet parse --force` produces identical output to a fresh parse
         on a clean tree
   - [ ] Regression: `noet watch` startup does not create `belief_cache.db`

## Testing Requirements

- `noet parse` on an unchanged `vast_qms` source tree (after an initial full parse)
  skips all networks and completes in < 1 second
- `noet parse --force` re-parses everything; output is byte-identical to a fresh parse
- Modifying one source file causes only the containing network to be re-parsed; others
  are skipped and their data is present in the DB via hydration
- Deleting a source file causes the containing network to be re-parsed
- `noet watch` startup does not create `belief_cache.db`; the in-memory DB is
  hydrated from shards and query results match a fresh parse
- MCP `check_consistency` (live mode) surfaces `ParseDiagnostic::UnresolvedReference`
  entries from `DocumentCompiler::last_diagnostics()` correctly
- MCP static mode returns correct `check_consistency.compiled_at` values per network
- With `--memory-budget 10` on a corpus > 10MB, eviction occurs without query
  correctness regression

## Success Criteria

- [ ] `NetworkShardMeta` has `compiled_at` and `source_mtimes` fields with
      `#[serde(default)]`; existing manifest roundtrip tests pass
- [ ] `noet parse` on an unchanged corpus skips all clean networks, hydrates the
      in-memory DB from shards, and logs a summary line showing reuse count
- [ ] `--force` bypasses skip logic; output is identical to a fresh parse
- [ ] `noet watch` does not create `belief_cache.db`; startup hydrates from shards
- [ ] `DocumentCompiler::last_diagnostics()` exists; MCP `check_consistency` live
      mode uses it to surface `UnresolvedReference` diagnostics
- [ ] Shard eviction + on-demand reload cycle passes correctness tests
- [ ] `--memory-budget` CLI flag on `noet watch` governs in-memory DB footprint
- [ ] Incremental skip unit tests pass, including mtime-bump and file-deletion cases
- [ ] WebSocket `/events` endpoint broadcasts `shard_updated` after each re-parse;
      SPA reflects a single-file edit within 1 second without page reload, UI state
      preserved
- [ ] On WebSocket reconnect, SPA fetches `manifest.json`, diffs `compiled_at`, and
      reloads only stale networks

## Risks

- **Mtime granularity on some filesystems**: FAT32 has 2-second mtime resolution;
  ext3 has 1-second resolution. A file written and immediately re-read within the
  same second may be falsely skipped. → **Mitigation**: Store mtimes with nanosecond
  precision where `fs::Metadata::modified()` provides it; `--force` is the safe
  recovery path. In practice, `noet parse` invocations are human-initiated and the
  race window is negligible.
- **Shard format version skew**: Adding fields to `NetworkShardMeta` must be
  backward-compatible. → **Mitigation**: `#[serde(default)]` on both new fields;
  missing `source_mtimes` treated as empty (dirty — triggers re-parse, safe).
- **Hydration fidelity**: The in-memory DB hydrated from shards must be
  query-equivalent to a DB built by a live parse. Any shard format gap (e.g. missing
  `WEIGHT_OWNED_BY` — see Issue 64 debug notes) will produce query differences.
  → **Mitigation**: Add a round-trip integration test that compares query results
  from a live parse vs. hydration from its own output shards.
- **Eviction correctness**: Evicting a network mid-query could produce inconsistent
  results if the eviction races with an in-progress query. → **Mitigation**: Eviction
  only occurs between parse passes (at the idle boundary), never during a query.
  The `compiler_idle_notify` signal in `FileUpdateSyncer` is the correct eviction
  trigger.
- **Monolithic mode skipped**: Incremental parse and hydration only apply to sharded
  output. Small repos below the 2MB threshold get no benefit. → **Mitigation**:
  Acceptable; large repos that benefit are also large enough to be sharded.
- **`last_diagnostics` snapshot timing**: The snapshot must be taken before the
  `latest_results` drain to capture the full diagnostic set. If taken after, the
  drain discards the data. → **Mitigation**: Code review checkpoint; add an assertion
  in tests that `last_diagnostics` is non-empty after a parse with known errors.

## Open Questions

- Should `source_mtimes` store paths relative to the repo root or relative to the
  network directory? Repo-root-relative is stable across network moves; network-
  relative is shorter. Recommend repo-root-relative for unambiguity.
- Should the incremental skip summary line go to `tracing::info!` or `tracing::debug!`?
  Recommend `info!` — users benefit from seeing that incremental is working.
- Should eviction be triggered by a size threshold (bytes in DB) or by
  `estimated_size_mb` from the manifest? Manifest estimates are coarse but require no
  DB introspection. DB byte-count is accurate but requires a `PRAGMA page_count`
  query. Recommend manifest estimates for simplicity; revisit if they prove inaccurate.
- `--db` → `--debug-db` migration: should `--db` be removed immediately or
  deprecated with a warning for one release? Recommend hard removal with a clear
  error message: "The --db flag has been replaced by --debug-db. The file-based DB
  is now a write-only debugging artifact, not a persistence layer. See Issue 66."
- `ShardBeliefSource` (shard-loading abstraction for MCP static mode) — name
  conflicts with the existing `BeliefSource` query trait in `src/query.rs`. Resolve
  at implementation time; `ShardLoader` or `StaticBeliefSource` are candidates.
- **Command taxonomy: `parse` vs `serve` vs `watch`**: The current `watch` command
  is evolving beyond file-watching into a long-running application server that
  serves the viewer, accepts source edits from the browser, exposes MCP and LSP
  interfaces, and recompiles incrementally. The right verb is `noet serve`, not
  `parse --watch`. The three commands have distinct lifecycles:

  - `noet parse` — one-shot batch compilation. Exits when done.
  - `noet serve` — long-running server. Serves viewer, MCP, LSP. Watches for
    file changes. Accepts browser-initiated edits. The in-memory DB is the
    authoritative live graph for all consumers.
  - `noet watch` — retained as a lighter alias or subset of `serve` (file
    watching + static file serving + live reload, without the full edit/query
    server). May be deprecated in favor of `serve` once the server is complete.

  `parse` and `serve` share the compilation pipeline (shard hydration,
  incremental skip logic, epoch/batch structure). `serve` adds the event loop,
  consumer management, and write-back path. This issue's hydration model
  applies to both `parse` (cold start from shards, one-shot) and `serve`
  (cold start from shards, then live). See `docs/project/UX_AUDIT.md`
  §3.9 for the full watch-as-server evolution.

  **Relationship to the attestation server (Issue 65)**: `noet serve` and
  `noet-collab` are separate processes with different responsibilities.
  `noet serve` owns the compilation pipeline, source files, and the
  compiled graph. `noet-collab` (Issue 65) is a substrate-agnostic
  attestation server that stores comments, sign-offs, and flags keyed on
  `(path, version)` anchors. It has no compiler, no source files, no
  `BeliefBase` — it's a Layer 3 event log.

  They interact via `noet serve` consuming attestation events from
  `noet-collab`'s `/events` endpoint as a `BeliefEvent` stream with
  `EventOrigin::Remote`. The attestation server doesn't need the
  compilation pipeline; `noet serve` doesn't need to store attestations.
  One `noet-collab` instance can serve multiple sites and multiple
  `noet serve` instances. They are co-deployed but architecturally
  independent.

- **Full-text search index synchronization with the DB**: The TF-IDF search
  index (`src/shard/search.rs`, `query_search_index`) is currently built from
  serialized shards at export time and lives entirely separate from the
  `BeliefSource` query path. Issue 79's `QuerySpec` introduces `TextMatch` as
  a `NodeFilter` variant that needs to compose with structural traversals
  (e.g., `->section(*) THEN title:authentication`). This requires the search
  index and the DB/in-memory graph to be queryable through a single
  `BeliefSource` interface.

  In the incremental model, the search index must update incrementally when
  documents are re-parsed — the same mtime/epoch skip logic that gates
  `BeliefBase` updates should gate search index updates. The search index
  should be a peer of the in-memory DB, not a shard-export artifact.

  Consumers:
  - **MCP `search` tool** — currently calls `query_search_index` on shards;
    should call `QuerySpec::evaluate()` with a `TextMatch` step instead
  - **LSP (Issue 11)** — workspace symbol search, go-to-definition fuzzy
    matching, and diagnostics all benefit from a live search index that
    updates as the user edits
  - **`{query}` directive (Issue 81)** — compile-time `TextMatch` evaluation
    needs the search index available during the compiler's deferred pass
  - **Viewer `bb.search()`** — currently shard-based; could eventually use
    the same `QuerySpec` path through WASM

  Design question: should the search index live inside `BeliefSource` (a new
  trait method like `text_search(&str, limit) -> Vec<(Bid, f32)>`) or as a
  separate `SearchIndex` struct that `QuerySpec::evaluate()` accepts
  alongside `BeliefSource`? The former is simpler; the latter avoids
  expanding `BeliefSource` with a concern that not all implementations
  support (e.g., `DbConnection` has no search index today).

## References

- `src/shard/manifest.rs` — `NetworkShardMeta`, `ShardManifest`, `network_shard_meta()`
- `src/shard/export.rs` — `export_sharded`, `export_beliefbase`
- `src/shard/wire.rs` — `NetworkShard`, `GlobalShard` (deserialization types)
- `src/codec/compiler.rs` — `DocumentCompiler`, `latest_results` drain, `parse_all`/`parse_sequential`
- `src/codec/diagnostic.rs` — `ParseDiagnostic::UnresolvedReference`
- `src/db.rs` — `db_init_memory`, `DbConnection`, `Transaction::add_event`, `Transaction::remove_nodes`
- `src/bin/noet/main.rs` — in-memory DB instantiation for `noet parse` (see `db_init_memory()` block)
- `src/watch.rs` — `FileUpdateSyncer`, `compiler_idle_notify` (eviction trigger boundary)
- Issue 41B (Stream BeliefEvents to SPA, `completed/ISSUE_41B_STREAM_EVENTS_TO_SPA.md`) —
  superseded by this issue's WebSocket shard-invalidation step; archived as OBE
- Issue 64 (MCP) — `check_consistency.compiled_at` and `last_diagnostics` consumers;
  `TODO(Issue 66)` hook in `src/mcp/state.rs`
- Issue 65 (Attestation Server) — separate `noet-collab` process; `noet serve`
  consumes its `/events` endpoint as `BeliefEvent` stream
- Issue 11 (LSP) — second consumer of `last_diagnostics` via `publishDiagnostics`
- Issue 50 (Sharding) — shard format foundation; `ShardConfig::memory_budget_mb`
- `docs/project/UX_AUDIT.md` §3.9 — watch-as-server evolution, multi-consumer
  model, LSP integration, attestation feedback loop
- `filetime` crate — already in `dev-dependencies`; use for mtime manipulation in tests