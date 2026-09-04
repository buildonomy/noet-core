---
version = "0.1"
title = "Issue 66: Incremental Parse via Shard Hydration"
---

# Issue 66: Incremental Parse via Shard Hydration

**Priority**: MEDIUM
**Estimated Effort**: 5 days (RELATIVE COMPARISON ONLY)
**Dependencies**: Requires Issue 50 (sharding), Requires Issue 64 (MCP — first consumer of static shard loading); Informs Issue 11 (LSP); Feeds Issue 102 (`noet serve` — owns the `/events` shard-invalidation endpoint that consumes this issue's per-network `compiled_at` values, and with it the supersession of Issue 41's BeliefEvent streaming)

## Summary

`noet parse` currently re-parses every source file on every invocation, and
`noet watch` maintains a file-based SQLite `belief_cache.db` as its cross-invocation
store. Both are unnecessary once shards are treated as the durable, structured
representation of a completed parse pass.

This issue makes shards the authoritative cross-invocation artifact by: (1) embedding
per-network `compiled_at` timestamps and source content hashes into the shard manifest
so clean networks can be skipped on re-parse; (2) hydrating the in-memory DB from shards
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

- `noet parse` skips networks whose constituent source files all hash identically to
  the values recorded in their shard, reducing re-parse time proportionally to the
  unchanged fraction of the corpus
- `noet watch` eliminates its file-based `belief_cache.db`: the in-memory DB is
  hydrated from shards at startup and updated incrementally on each dirty-network
  re-parse, making the watch daemon stateless between restarts. During a session,
  the in-memory DB is the authoritative query surface for all consumers (browser
  viewer, MCP, LSP)
- Per-network `compiled_at` timestamp and `source_hashes` embedded in
  `NetworkShardMeta`, readable by MCP `check_consistency` and the incremental skip
  logic
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

### Content hashing in `NetworkShardMeta`

Add two fields to `NetworkShardMeta` in `src/shard/manifest.rs`:

```
compiled_at:   String   // RFC 3339 UTC timestamp of when this shard was written
source_hashes: BTreeMap<String, String>  // relative path → SHA-256 of file bytes
```

Both carry `#[serde(default)]` for backward compatibility with existing manifests.
`source_hashes` uses SHA-256 hex strings, matching the asset content hash already
produced at `src/codec/builder.rs:4271` — one hash function across the codebase, not
two.

`source_hashes` captures the content hash of every source file that contributed to
this network at the time the shard was written. On the next `noet parse` invocation,
the compiler hashes each file currently in scope and compares: if every hash matches
the stored value, and the file set is unchanged, the network is clean and can be
skipped.

`compiled_at` is a human-readable ISO 8601 string exposed in Issue 64's
`check_consistency` output and used by the incremental skip logic to log which
networks were reused. `source_hashes` is not exposed via MCP — it is an internal
implementation detail of the skip logic.

#### Why hashing is primary and mtime is not a skip decision

An earlier draft of this issue specified the opposite ordering: a cheap mtime check
first, with hashing as a fallback discriminator that ran only when the mtime check had
already failed. **That ordering is unsound and is replaced.** The defect is structural,
not statistical — mtime is *usually* right, and that is beside the point.

The two checks have asymmetric failure modes:

| Check | Can produce FALSE-DIRTY | Can produce FALSE-CLEAN |
|-------|-------------------------|-------------------------|
| mtime comparison | yes | **yes** |
| content hash | yes (hash collision, negligible) | no |

False-dirty costs a wasted re-parse. False-clean silently skips a changed file, so the
compiled graph no longer reflects the source — a corruption the user cannot detect and
cannot recover from without knowing to pass `--force`.

In the mtime-first composite, a check capable of false-clean *gates* a check that can
only produce false-dirty. When mtime says "clean", the file is skipped and the hash
never runs. The hash therefore rescues only the false-dirty direction — the direction
that was already safe. The composite inherits mtime's false-clean rate in full; the
hash contributes exactly zero safety in the only direction that matters.

> **Rule**: *no check whose failure mode is false-clean may terminate the evaluation.*
> mtime may short-circuit toward **dirty** only, never toward **clean**.

The false-clean cases are reachable, not theoretical:

- **Second-granularity truncation.** Mtimes are recorded in whole seconds — `src/db.rs:186`
  and `src/codec/compiler.rs:567` both use `as_secs()`. A file written twice within the
  same second is misclassified clean. `noet watch` reacting at machine speed hits this
  window routinely.
- **Backwards mtimes.** A `≤`/`>` comparison (`src/codec/compiler.rs:570` uses
  `current_mtime > cached`) treats an *older* mtime as clean. Timestamp-preserving
  restores — `rsync -t`, `cp -p`, `tar -p`, archive extraction — write older mtimes with
  different content, and are classified clean.

Because hashing is now primary, mtime precision no longer gates correctness. The
seconds-vs-nanoseconds question is moot: mtime is not consulted for a clean decision at
any precision.

**Where mtime still lives**: indirectly, in `WatchService` (`src/watch.rs:287`). The OS
file-change notification path tells the watcher that *something happened*; that signal
then triggers a hash to determine what actually changed. Mtime becomes an interrupt
source and stops being a skip decision anywhere.

### Where the hashing happens

`src/codec/proto_index.rs` treats the whole file tree the way the compiler already
treats assets: one content hash per file in scope, produced during the index build and
handed to the skip logic.

The precedent already ships. `DocumentCompiler::process_asset_batch`
(`src/codec/compiler.rs:1016-1026`) does exactly this for every asset — `tokio::fs::read`
plus SHA-256, concurrency-bounded by a semaphore. This step generalizes it to the source
tree.

`net_dir_partition` (`src/codec/proto_index.rs:133-307`) already returns
`network_dir → children`, which is precisely the per-network granularity the skip logic
needs. Nothing needs re-deriving; the hashes attach to a partition that already exists.

**Cost.** This is a genuine extra full read — the content is *not* already in hand,
because `MdCodec::proto` (`src/codec/md.rs:2147-2148`) opens the file and reads only the
frontmatter. For a corpus of ~3,000 files / ~30 MB the additional I/O is roughly one
`cat` of the tree: on the order of 50–150 ms warm, low seconds cold. That is negligible
against a parse, and `ProtoIndex::build()` runs once at compiler startup, not once per
re-parse.

Two caveats to carry into implementation:

- `ProtoIndex::build()` (`src/codec/proto_index.rs:348`) is currently synchronous and
  metadata-only. Adding reads makes it I/O-bound; it wants the same async +
  semaphore treatment `process_asset_batch` already has.
- **Do not merge the two `WalkDir` passes in `net_dir_partition`.** The two-pass
  structure exists for a documented correctness reason — pass 1 pre-scans subnet
  directories so pass 2 is `readdir` order-independent (`proto_index.rs:150-161`).
  Hash in pass 2 only.

### Skip logic in `DocumentCompiler`

Before dispatching parse work for a network, `DocumentCompiler` (or its caller in
`main.rs`) checks:

1. Does a shard exist for this network? (manifest present, file exists on disk)
2. Is `--force` absent?
3. Is the current file set for this network identical to the key set of
   `source_hashes`?
4. Does every current file's content hash equal its stored value?

If all four hold: skip the network. Emit a `tracing::debug!` line noting the skip and
the shard age. If any hash differs, the file set differs, or the shard is absent:
proceed with normal parse.

The evaluation never terminates on an mtime comparison. A cheap `stat` may be used to
short-circuit toward *dirty* (a newer mtime is a sufficient reason to re-parse without
hashing), but never toward *clean*.

**Deleted files**: a file present in `source_hashes` that no longer exists on disk
makes the network dirty (a deletion may have removed a node — must re-parse to detect
orphans). This is covered by check (3).

**New files**: the file walker discovers files not in `source_hashes` — also check (3),
also dirty.

**On hash caching**: an implementation may be tempted to cache hashes keyed on
`(path, mtime, size)` to avoid re-reading. Be explicit that this reintroduces a
narrower false-clean — same-second, same-size rewrites — by the same mechanism this
section rejects. If such a cache ships, it ships as a knowingly-taken risk with a
documented off switch, not as an accident. The measured read cost above is the argument
for not needing one.

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

1. **Add `compiled_at` + `source_hashes` to `NetworkShardMeta`** (0.5 days)
   - [ ] Add `compiled_at: String` and `source_hashes: BTreeMap<String, String>` to
         `NetworkShardMeta` in `src/shard/manifest.rs`; annotate both with
         `#[serde(default)]` for backward compatibility with old manifests
   - [ ] Populate both in `export_sharded` (`src/shard/export.rs`): capture
         `SystemTime::now()` as `compiled_at`; take each source file's SHA-256 from
         the `ProtoIndex` hashes (step 1b) rather than re-reading at export
   - [ ] Update `network_shard_meta()` constructor to accept and thread through both
         new fields
   - [ ] Confirm `ShardManifest` JSON roundtrip test still passes

1b. **Hash the source tree in `ProtoIndex`** (0.5 days)
   - [ ] Compute a SHA-256 per in-scope source file during `ProtoIndex::build()`
         (`src/codec/proto_index.rs:348`), keyed repo-root-relative, grouped by the
         `network_dir → children` partition `net_dir_partition` already returns
   - [ ] Model the read on `DocumentCompiler::process_asset_batch`
         (`src/codec/compiler.rs:1016-1026`): `tokio::fs::read` + SHA-256 with a
         semaphore bounding concurrency to `jobs`. `build()` is synchronous today and
         becomes I/O-bound; convert it accordingly
   - [ ] **Hash in pass 2 of `net_dir_partition` only.** Do not merge the two
         `WalkDir` passes — pass 1 exists so pass 2 is `readdir` order-independent
         (`proto_index.rs:150-161`)
   - [ ] Expose the hashes to both consumers: the skip logic (step 3) and shard export
         (step 1)
   - [ ] Benchmark on a few-thousand-file corpus; record the delta to
         `ProtoIndex::build()` wall time. Expectation is one `cat` of the tree

1c. **Make `payload["text"]` unconditionally derived from source** (0.5 days)

   Text regeneration is currently gated at `src/codec/md.rs:2380-2384` on
   `frontmatter_changed || sections_metadata_merged || link_changed || id_changed`.
   On a steady-state re-parse of a document whose BIDs are all persisted and
   whose links all resolve, none of those fire, so `payload["text"]` is never
   regenerated — the stored value is a fossil of whichever earlier parse last
   ran `inject_context` (`md.rs:2405-2427`).

   **This is a correctness defect in its own right.** `payload["text"]` purports to
   be the document's text; it should be a pure function of the file's content. Today
   it is a function of parse history — two runs over identical source can leave
   different values in the DB depending on what happened in earlier parses. That makes
   the field unreliable for every consumer that reads it, independent of whether
   anything hashes it.

   This issue owns it because it owns source-change propagation: a file whose content
   changed must have that change reach every derived field, and this one is exempt.

   - [ ] Decouple text derivation from the mutation flags: `payload["text"]`
         must be a pure function of the parsed source, computed on every parse
   - [ ] Preserve the existing write-avoidance behaviour separately — the
         `generate_source() != content` check at `src/codec/builder.rs:1488-1496`
         is what prevents file rewrites, and must keep working unchanged
   - [ ] Test: parse a document with all BIDs persisted and all links resolving
         (so `inject_context` fires nothing); assert `payload["text"]` is present
         and matches the source. **This is expected to fail before the fix.**
   - [ ] Test: two parses of identical source produce byte-identical
         `payload["text"]` regardless of parse order or prior state

> **Out of scope — per-node `_content_hash` is Issue 105's.** An earlier draft of
> this issue carried a step adding a per-*node* content hash
> (`metadata["_content_hash"]`, radius 0) for cross-version diff and annotation
> anchoring, per `docs/design/identity/content_versioning.md` §5.1–5.2. Skip logic needs
> only per-*file* hashes, so it moved to **Issue 105 step 3a** — the first issue
> in the build sequence that actually requires a per-node hash. Issue 105 also
> owns the closures (step 3b) and inherits §8's two open specification gaps (hash
> input encoding; whether `BeliefKind::External` is content-bearing).
>
> **Step 1c below remains this issue's**, and Issue 105 depends on it: nothing
> can soundly hash `payload` until `payload["text"]` is a pure function of file
> content.

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
         `source_hashes` against the hashes computed in step 1b
   - [ ] Handle missing files (dirty), new files (dirty), `--force` (all dirty)
   - [ ] Assert the invariant in review: no code path may conclude *clean* from an
         mtime comparison. A `stat` may short-circuit toward *dirty* only
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

4. **WebSocket shard-invalidation endpoint** — MOVED to Issue 102 (`noet serve`). Issue 66 provides the per-network `compiled_at` values that the broadcast carries; Issue 102 owns the endpoint, the consumer registry, and the SPA reload logic.

5. **Shard eviction and on-demand reload** (0.5 days)
   - [ ] Track per-network "hot" flag in the in-memory DB session
   - [ ] When the DB size approaches `memory_budget_mb`, evict the least-recently-used
         network by removing its nodes/edges via `Transaction::remove_nodes` and
         marking it "on-disk"
   - [ ] On a query that touches an evicted network, reload from its shard file
   - [ ] Expose `--memory-budget <MB>` CLI flag on `noet watch` (default: reuse
         `ShardConfig::DEFAULT_MEMORY_BUDGET_MB`)

6. **Tests** (0.75 days)
   - [ ] Unit test: fixture shard with known `source_hashes`; assert network skipped
         when no file's content changed; assert re-parsed when one file's bytes change
   - [ ] Unit test: a file whose mtime is bumped but whose bytes are unchanged is
         classified **CLEAN** (use `filetime` crate, already in `dev-dependencies`)
   - [ ] Unit test — **false-clean regression**: a file whose bytes change while its
         mtime is set *backwards* (the `rsync -t` / archive-extraction case) is
         classified **DIRTY**. Under the rejected mtime-first ordering this test fails
   - [ ] Unit test — **false-clean regression**: two distinct writes to one file
         within the same whole second are both detected. `as_secs()` truncation
         (`src/db.rs:186`, `src/codec/compiler.rs:567`) makes this invisible to mtime
   - [ ] Unit test: `hydrate_from_shards` against a fixture output directory; assert
         the in-memory DB contains the expected node count after hydration
   - [ ] Unit test: eviction + reload cycle; assert query results identical before
         and after eviction
   - [ ] Regression: `noet parse --force` produces identical output to a fresh parse
         on a clean tree
   - [ ] Regression: `noet watch` startup does not create `belief_cache.db`

## Testing Requirements

- `noet parse` on an unchanged multi-network source tree of a few thousand documents
  (after an initial full parse) skips all networks and completes in < 1 second,
  *including* the full-tree hash pass
- `noet parse --force` re-parses everything; output is byte-identical to a fresh parse
- Modifying one source file causes only the containing network to be re-parsed; others
  are skipped and their data is present in the DB via hydration
- A content change is detected regardless of what the mtime does — unchanged, moved
  backwards, or within the same whole second as the previous write
- Deleting a source file causes the containing network to be re-parsed
- The `ProtoIndex::build()` hash pass adds no more than a small fraction of a full
  parse's wall time on a few-thousand-file corpus
- `noet watch` startup does not create `belief_cache.db`; the in-memory DB is
  hydrated from shards and query results match a fresh parse
- MCP `check_consistency` (live mode) surfaces `ParseDiagnostic::UnresolvedReference`
  entries from `DocumentCompiler::last_diagnostics()` correctly
- MCP static mode returns correct `check_consistency.compiled_at` values per network
- With `--memory-budget 10` on a corpus > 10MB, eviction occurs without query
  correctness regression

## Success Criteria

- [ ] `NetworkShardMeta` has `compiled_at` and `source_hashes` (SHA-256 hex) fields
      with `#[serde(default)]`; existing manifest roundtrip tests pass
- [ ] `ProtoIndex::build()` produces a content hash per in-scope source file, grouped
      by `net_dir_partition`'s existing `network_dir → children` partition, hashed in
      pass 2 only
- [ ] `noet parse` on an unchanged corpus skips all clean networks, hydrates the
      in-memory DB from shards, and logs a summary line showing reuse count
- [ ] `--force` bypasses skip logic; output is identical to a fresh parse
- [ ] `noet watch` does not create `belief_cache.db`; startup hydrates from shards
- [ ] `DocumentCompiler::last_diagnostics()` exists; MCP `check_consistency` live
      mode uses it to surface `UnresolvedReference` diagnostics
- [ ] Shard eviction + on-demand reload cycle passes correctness tests
- [ ] `--memory-budget` CLI flag on `noet watch` governs in-memory DB footprint
- [ ] Incremental skip unit tests pass, including mtime-bump and file-deletion cases
- [ ] A file whose mtime is bumped but whose content is unchanged is classified CLEAN
- [ ] Both false-clean regression tests pass: backwards-mtime content change and
      same-second double write are both classified DIRTY
- [ ] No code path concludes *clean* from an mtime comparison; mtime is consulted only
      by `WatchService` as a change-notification trigger
- [ ] `payload["text"]` is a pure function of file content: two parses of identical
      source produce byte-identical values regardless of prior state

The two WebSocket criteria previously listed here (`/events` broadcasting
`shard_updated` after each re-parse, and reconnect-time `manifest.json` diffing) are
now **Issue 102's success criteria**. This issue's obligation is only to make the
per-network `compiled_at` values available for Issue 102 to broadcast.

## Risks

- **Generated corpora defeat content hashing too**: Upstream document generators
  commonly stamp a render timestamp into each generated document's frontmatter — a
  `generated:` or `generated_date:` field interpolated at render time. When such a
  generator re-runs, every output file gets a new content hash even when the upstream
  source data is unchanged. In one measured corpus, ~1,260 of the generated Markdown
  documents carried such a timestamp; an incremental parse would correctly classify
  all of them as dirty and skip nothing. The skip rate on that corpus would be zero,
  and this issue would deliver no value there while still paying its complexity cost.
  → **Mitigation**: this **cannot be fixed inside noet-core**. A document whose bytes
  genuinely changed is genuinely dirty, and noet-core cannot know that one field is
  noise. The fix belongs in the generator: move the render timestamp out of
  per-document frontmatter into a single build-manifest sidecar, so that a no-op
  regeneration produces byte-identical documents. Document this as a **required
  precondition** for incremental parse to deliver value on generated corpora. Note
  that content hashing does recover the *other* generator case for free — a generator
  that rewrites files byte-identically with fresh mtimes is now classified clean,
  which the rejected mtime-first ordering would have missed.
- **Full-tree hashing adds a read pass**: `ProtoIndex::build()` gains a full read of
  every in-scope source file where it previously read only frontmatter
  (`src/codec/md.rs:2147-2148`). → **Mitigation**: the pass runs once at compiler
  startup, not per re-parse, and costs roughly one `cat` of the corpus (~50–150 ms
  warm for ~3,000 files / ~30 MB) against a parse measured in tens of seconds. Bound
  concurrency with the same semaphore pattern `process_asset_batch` uses. Benchmark
  in step 1b; if the measurement contradicts the estimate, revisit before shipping.
- **A hash cache would reintroduce false-clean**: caching hashes keyed on
  `(path, mtime, size)` avoids re-reading but restores exactly the failure mode this
  design rejects, narrowed to same-second same-size rewrites. → **Mitigation**:
  ship no cache unless the step 1b benchmark demands one. If one ships, it is a
  knowingly-taken risk with a documented flag to disable it — not an implementation
  detail.
- **Shard format version skew**: Adding fields to `NetworkShardMeta` must be
  backward-compatible. → **Mitigation**: `#[serde(default)]` on both new fields;
  a missing or empty `source_hashes` is treated as dirty, which triggers a re-parse —
  the safe direction. Old manifests degrade to full re-parse, never to a false skip.
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

- Should `source_hashes` store paths relative to the repo root or relative to the
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
- **Command taxonomy: `parse` vs `serve` vs `watch`** — **RESOLVED, owned by Issue
  102.** `watch` is renamed to `serve`: `noet parse` is one-shot batch compilation
  that exits when done, while `noet serve` is the long-running application server
  that watches for changes, serves the viewer, and hosts the MCP, LSP, and
  annotation consumers. Both share this issue's compilation pipeline and cold-start
  hydration model; Issue 102 owns the event loop, the consumer registry, and the
  idle boundary. See `docs/project/UX_AUDIT.md` §3.9 for the original
  watch-as-server evolution.

  **Relationship to the attestation server (Issue 65)**: `noet serve` and
  `noet-collab` are separate processes with different responsibilities.
  `noet serve` owns the compilation pipeline, source files, and the
  compiled graph. `noet-collab` (Issue 65) is a substrate-agnostic
  attestation server that stores comments, sign-offs, and flags keyed on
  version anchors. It has no compiler and no source files. It does *hold out* a
  BeliefBase — the overlay its records fold into
  (`docs/design/annotation/living_corpus.md` §2) — but it compiles nothing.

  The attestation server is **no longer the primary system of record**. The local
  sidecar annotation store (Issue 105) — a gitignored file tree in the corpus, keyed
  on `(bid, version)` — is the primary store for a given working copy, and the
  attestation server is the **same mechanism at shared scope**, a sync peer that
  annotations replicate to and from rather than a second system. This
  makes the system local-first: annotation works with no server running, and the
  server's role is convergence across working copies rather than custody of the
  data. `noet serve` still consumes remote attestation events as a `BeliefEvent`
  stream with `EventOrigin::Remote`; the difference is that the local store is now
  the write target and the sync is bidirectional.

- **Full-text search index synchronization with the DB**: The TF-IDF search
  index (`src/shard/search.rs`, `query_search_index`) is currently built from
  serialized shards at export time and lives entirely separate from the
  `BeliefSource` query path. Issue 79's `QuerySpec` introduces `TextMatch` as
  a `NodeFilter` variant that needs to compose with structural traversals
  (e.g., `->section(*) THEN title:authentication`). This requires the search
  index and the DB/in-memory graph to be queryable through a single
  `BeliefSource` interface.

  In the incremental model, the search index must update incrementally when
  documents are re-parsed — the same content-hash skip logic that gates
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

- [`docs/design/identity/content_versioning.md`](../../design/identity/content_versioning.md) — owns
  per-*node* content hashing, which is **out of scope for this issue and unowned**.
  Its §8 open questions (hash input encoding; `External` as content-bearing) remain
  open. This issue's `source_hashes` is a per-*file* hash and is unrelated.
- `src/codec/proto_index.rs:348` — `ProtoIndex::build()`, where source-tree hashing
  lands; currently synchronous and metadata-only
- `src/codec/proto_index.rs:133-307` — `net_dir_partition`, the existing
  `network_dir → children` partition; §150-161 documents why the two `WalkDir` passes
  must not be merged
- `src/codec/compiler.rs:1016-1026` — `process_asset_batch`, the shipped precedent for
  concurrency-bounded `tokio::fs::read` + SHA-256 per file
- `src/codec/builder.rs:4271` — asset content hash; SHA-256 is the codebase convention
- `src/codec/md.rs:2147-2148` — `MdCodec::proto` reads frontmatter only, which is why
  hashing is a genuine extra read rather than free
- `src/db.rs:186`, `src/codec/compiler.rs:567` — mtime captured via `as_secs()`; the
  truncation behind the same-second false-clean case
- `src/codec/compiler.rs:570` — `current_mtime > cached`, the comparison that treats a
  backwards mtime as clean
- `src/shard/manifest.rs` — `NetworkShardMeta`, `ShardManifest`, `network_shard_meta()`
- `src/shard/export.rs` — `export_sharded`, `export_beliefbase`
- `src/shard/wire.rs` — `NetworkShard`, `GlobalShard` (deserialization types)
- `src/codec/compiler.rs` — `DocumentCompiler`, `latest_results` drain, `parse_all`/`parse_sequential`
- `src/codec/diagnostic.rs` — `ParseDiagnostic::UnresolvedReference`
- `src/db.rs` — `db_init_memory`, `DbConnection`, `Transaction::add_event`, `Transaction::remove_nodes`
- `src/bin/noet/main.rs` — in-memory DB instantiation for `noet parse` (see `db_init_memory()` block)
- `src/watch.rs` — `WatchService` (the only remaining mtime consumer: OS change
  notification triggers a hash, never a skip), `FileUpdateSyncer`,
  `compiler_idle_notify` (eviction trigger boundary)
- Issue 41B (Stream BeliefEvents to SPA, `completed/ISSUE_41B_STREAM_EVENTS_TO_SPA.md`) —
  superseded by the WebSocket shard-invalidation endpoint, now owned by Issue 102;
  archived as OBE
- Issue 102 (`noet serve`) — owns the `/events` endpoint, consumer registry, and SPA
  reload logic moved out of this issue's step 4
- Issue 105 (annotation sidecar store) — local-first store for which the attestation
  server is a sync peer
- Issue 64 (MCP) — `check_consistency.compiled_at` and `last_diagnostics` consumers;
  `TODO(Issue 66)` hook in `src/mcp/state.rs`
- Issue 65 (Attestation Server) — separate `noet-collab` process; `noet serve`
  consumes its `/events` endpoint as `BeliefEvent` stream
- Issue 11 (LSP) — second consumer of `last_diagnostics` via `publishDiagnostics`
- Issue 50 (Sharding) — shard format foundation; `ShardConfig::memory_budget_mb`
- `docs/project/UX_AUDIT.md` §3.9 — watch-as-server evolution, multi-consumer
  model, LSP integration, attestation feedback loop
- `filetime` crate — already in `dev-dependencies`; use for mtime manipulation in tests