---
version = "0.1"
title = "Issue 66: Incremental Parse via Shard Hydration"
---

# Issue 66: Incremental Parse via Shard Hydration

**Priority**: HIGH — first in the living-corpus sequence (planning Issue 31, Wave B)
**Estimated Effort**: 5 days (RELATIVE COMPARISON ONLY)
**Dependencies**: Requires Issue 50 (sharding), Requires Issue 64 (MCP — first consumer of static shard loading); Informs Issue 11 (LSP); Feeds Issue 102 (`noet serve` — owns the `/events` shard-invalidation endpoint that consumes this issue's per-network `compiled_at` values, and with it the supersession of Issue 41's BeliefEvent streaming). **Blocks Issue 105 and every annotation anchored to `(bid, version)`** — see § Why this issue is first.

## Summary

`noet parse` currently re-parses every source file on every invocation, and
`noet watch` maintains a file-based SQLite `belief_cache.db` as its cross-invocation
store. Both are unnecessary once shards are treated as the durable, structured
representation of a completed parse pass.

This issue makes shards the authoritative cross-invocation artifact by: (1) introducing
**`ShardStore`**, a `BeliefSource` + `BeliefSink` backend over the shard directory that
**replaces `DbConnection`** as the store backing the parse, so unchanged nodes resolve
to their existing BIDs instead of minting new ones; (2) embedding per-network
`compiled_at` timestamps and source content hashes into the shard manifest so clean
networks can be skipped on re-parse; (3) making `noet watch` stateless between restarts
by removing its file-based DB; and (4) exposing the `last_diagnostics` accessor that
MCP (`check_consistency`) and LSP (`publishDiagnostics`) need.

Because `GraphBuilder::cache_fetch` is already generic over `BeliefSource`
(`builder.rs:4026`), shard awareness lives entirely behind the trait and the compiler
is unchanged. Memory budgeting and eviction are deferred to § Performance — fetch-on-
miss makes a partially-loaded store correct, so the budget is a footprint knob rather
than a correctness concern.

## Why this issue is first

**BID stability is the primary deliverable. Skip logic is the secondary one.** An
earlier framing of this issue led with incremental skip and treated hydration as the
mechanism that makes skip possible. That ordering is backwards for the program this
issue now gates.

`Bid::new` is time-based (`Uuid::now_v6`, `src/properties.rs:239-241`). A node that
`cache_fetch` cannot resolve against `global_bb` gets a fresh BID on every parse
(`src/codec/builder.rs:2306-2308`, `NodeSource::Generated`). Today the only way a BID
survives across invocations is either `--write` (persist it into source frontmatter)
or `noet watch`'s `belief_cache.db`. **Neither is available on the corpora this
program targets:**

- **Generated corpora cannot be written back to.** Where source files are produced by
  an upstream generator on each build, `--write` is pointless — the next regeneration
  discards the injected BIDs. The generator emits stable `id:` frontmatter but no
  `bid:`; measured on one such corpus, zero of ~120 generated documents carried a BID.
- **CI builds are cold.** The production render path is a one-shot `parse` with no
  `--db` and no `--write`, so every build mints every non-persisted BID afresh.

The consequence: **every annotation anchored to `(bid, version)` is orphaned on the
next build**, regardless of how good the version hash is. `content_versioning.md` §6
states the determinism requirement for the *hash*; this issue is what makes the *BID*
half of the anchor deterministic across builds. Without it, Issue 105's store cannot
survive a rebuild on the pilot corpus, and the living corpus has no anchor to stand on.

Backing `global_bb` with the prior shards gives `cache_fetch` a `GlobalCache` hit for
every unchanged heading, so only genuinely new content reaches `Generated`. **This must
hold for dirty networks too** — a network with one edited file still has hundreds of
unchanged headings whose BIDs must be preserved, so its prior shard backs the re-parse
and is overwritten at export, never skipped over.

Two further reasons this precedes the annotation wave rather than following it:

- **Corpus currency.** A compute-limited CI runner cannot afford a full re-parse of a
  large corpus daily; an out-of-date corpus is not annotatable in any useful sense.
  Skip logic is what makes a daily build affordable.
- **Iteration speed** on the served corpus, for ordinary corpus work. This is real but
  is the weakest of the three — the annotation layer's own development loop should run
  against a small fixture, not the production corpus.

> **Operational consequence, outside this repo.** Shard-based BID stability holds only
> across an unbroken shard chain: the previous run's `beliefbase/*.msgpack` and
> manifest must be **restored as an input** before `parse` runs in CI, not merely
> published as an output. Losing the chain once re-mints every BID. Consumers of the
> anchor should therefore carry a secondary re-attachment key (source path plus
> frontmatter `id:` or heading slug) — that is Issue 105's concern, noted here so
> the chain's fragility is not mistaken for an Issue 66 defect. The CI restore step is
> a planning-repo item, not a noet-core one.

**Lifecycle clarification**: `ShardStore`'s in-memory graph is *authoritative* while
the process is running. During a `watch` session, multiple consumers (browser viewer,
MCP clients, LSP clients) query it concurrently. Shard files are *checkpoints* for
cold start and for HTML output, not the live query surface. "Stateless between
restarts" means the cold-start path needs no persistent DB file — during a session the
store is the live authority.

The existing `--db` flag is replaced by `--debug-db`, which mirrors session state to a
file for developer inspection (`sqlite3 /tmp/noet-debug.db`). See Architecture
§ File-based DB for debugging.

## Goals

- **A node whose source is unchanged between two `noet parse` runs keeps its BID**,
  with no `--write` and no `belief_cache.db`, provided the prior run's shards are
  present. This holds for nodes in dirty networks as well as clean ones
- **`ShardStore` replaces `DbConnection`** as the store backing the parse, so shards
  are the durable representation and SQLite leaves the identity path entirely
- `noet parse` skips networks whose constituent source files all hash identically to
  the values recorded in their shard, reducing re-parse time proportionally to the
  unchanged fraction of the corpus
- `noet watch` eliminates its file-based `belief_cache.db`: `ShardStore` loads from
  shards at startup and is updated in memory on each dirty-network re-parse, making
  the watch daemon stateless between restarts. During a session the store is the
  authoritative query surface for all consumers (browser viewer, MCP, LSP)
- Per-network `compiled_at` timestamp and `source_hashes` embedded in
  `NetworkShardMeta`, readable by MCP `check_consistency` and the incremental skip
  logic
- `DocumentCompiler::last_diagnostics()` accessor exposing the diagnostic snapshot
  from the last completed parse pass — consumed by MCP `check_consistency` (live mode)
  and LSP `publishDiagnostics` (Issue 11)
- A key whose home shard is not loaded resolves by **fetch-on-miss**, so a
  partially-loaded store is correct rather than merely fast
- **Exactly one writer per output directory**, enforced by an exclusive advisory lock
  held for the writing process's lifetime; readers take no lock and observe whole
  generations via atomic rename
- `--force` flag on `noet parse` bypasses incremental skip logic (already exists;
  must remain respected)
- Monolithic mode is the one-shard case: identity preservation works there too, and
  only *skip* requires sharding

## Architecture

### Unified startup model

Both `noet parse` and `noet watch` build an **ephemeral store** as `global_bb` during
compilation — today an in-memory SQLite DB (`db_init_memory()`, wired at
`src/cli.rs:583-602`), with `noet watch` additionally maintaining a persistent
`belief_cache.db`. After this issue both use `ShardStore`, and the startup sequence is
identical for the two commands:

```
Startup:
  1. Open ShardStore over the output directory — read the manifest, load the
     global shard (the bref → home-network routing table)
  2. Classify networks clean/dirty by comparing source_hashes
  3. Load prior network shards into the store. Dirty networks matter most:
     their unchanged nodes resolve to existing BIDs via cache_fetch during
     the re-parse. A key whose shard is not loaded is fetched on miss.
  4. Parse only dirty networks → events land in the store, superseding the
     loaded state for those networks
  5. finalize_html → re-export dirty shards + emit last_diagnostics snapshot

noet watch (continuous loop):
  File change → mark containing network dirty → repeat steps 4-5 for dirty set only
```

Step 3 loading *dirty* networks is what distinguishes this from a pure skip cache, and
it is the load-bearing part (§ Why this issue is first). On a re-parse,
`GraphBuilder::push` calls `cache_fetch`, which checks `doc_bb` → `session_bb` →
`global_bb`; a hit returns the existing node and BID (`NodeSource::GlobalCache`), a
miss mints `Bid::new(parent_bid)` (`src/codec/builder.rs:2306-2308`). The prior shard
behind `global_bb` is what turns the second case into the first for every heading whose
path key has not changed.

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

> **Do not drop the prior node before the new hash is written.** A follow-on
> capability (Issue 74 §Archive) writes a content-addressed store of prior node
> states alongside the shards, and export is the one place where the hydrated
> prior and the freshly parsed node are both in hand. Nothing here needs to
> *build* that store — only to avoid a structure that discards the prior state at
> the moment the new hash is computed. This is a don't-foreclose constraint.

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
`src/cli.rs`) checks:

1. Does a shard exist for this network? (manifest present, file exists on disk)
2. Is `--force` absent?
3. Is the current file set for this network identical to the key set of
   `source_hashes`?
4. Does every current file's content hash equal its stored value?

If all four hold: skip the network. Emit a `tracing::debug!` line noting the skip and
the shard age. If any hash differs, the file set differs, or the shard is absent:
proceed with normal parse — **with the network's prior shard backing `global_bb`** via
`ShardStore` (if one exists), so the re-parse preserves BIDs for its unchanged nodes.

Skip and identity are independent: skipping is an optimisation over the *parse*, while
the store preserves BIDs whether or not anything is skipped.

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

### `ShardStore`: shards as a `BeliefSource` backend

Shard awareness lives **behind the `BeliefSource` trait**, not in the compiler.
`GraphBuilder::cache_fetch` is already generic over `B: BeliefSource + Clone`
(`builder.rs:4026`) and reaches its backing store through exactly one call —
`global_bb.evaluate(&mut package)`. A backend that knows how to find, load, and
write shards therefore needs **no compiler changes at all**.

`ShardStore` implements `BeliefSource` (`src/query/mod.rs:40`) and `BeliefSink`
(`src/beliefbase/sink.rs:41`), and **replaces `DbConnection`** as the store backing
the accumulator:

```rust
// cli.rs — was: BeliefAccumulator::new(DbConnection(db_pool), rx)
BeliefAccumulator::new(ShardStore::open(output_dir)?, rx)
```

`BeliefAccumulator<S>` is already generic over its store, so batching, query caching,
and `resolve_merge_keys` are unchanged. SQLite leaves the parse path entirely.

**Read path.** `ShardStore` holds a `BeliefBase`. Loading a shard is
`BeliefBase::merge` (`base.rs:2946`), whose pass 3 drives `PathMapMap` via
`process_event_queue` (`base.rs:2996`) and builds the path index. `NodeKey::Path`
then resolves through `BeliefBase::get` (`base.rs:616`) — in memory, no SQL, no
`paths` table. This is the mechanism the browser viewer already uses
(`wasm.rs:902`).

**Routing.** The manifest plus the always-resident global shard is a complete
routing table; no scan is needed to find a key's home shard:

| Key | Resolves via |
|---|---|
| `Path { net, .. }`, `Id { net, .. }` | `net` **is** the network bref — names its shard directly |
| `Bid`, `Bref` | `GlobalShard.bref_index` (node bref → home network bref) |

A miss against the loaded set is therefore a **fetch**, not a failure: resolve the
key to a home shard, load it, retry. This is what makes a partially-loaded store
correct rather than merely fast.

**The Trace halo is the fetch trigger.** Each network shard embeds more than its own
members: every edge endpoint outside the network (`export.rs:279-299`), every
third-party `{maps_to}` owner (`:301-324`), and each extern's Section edge to its
namespace parent together with that parent node (`:326-353`). So a shard is
self-sufficient for upward traversal, and an extern copy carries the real node with
its real BID — enough to identify the home shard and pull it. A partially-loaded
store has no broken parent chains.

**Write path — deferred.** `apply_batch` marks the touched network dirty; whole
shards are re-exported at `finalize_html` through the existing `export_sharded`.
No per-event shard mutation. The store is live and authoritative in memory
throughout, so deferring the write costs no correctness. This is also the shape
`generational_archive.md` §9.1 assumes: a single `STAGED` generation overwritten
every parse.

**Monolithic mode is the one-shard case.** Absence of `beliefbase/manifest.json`
means load `beliefbase.msgpack` as a single unit. Identity preservation works
everywhere; only *skip* is sharded-only, because only sharding provides the
per-network granularity a skip decision needs.

**`get_file_mtimes` is retired.** It exists on `BeliefSource` with a default-empty
implementation (`query/mod.rs:68`), and `DbConnection` satisfied it from the
`file_mtimes` table for `check_stale_files`. That table was a workaround for having
no durable prior generation. `ShardStore` has one: "did this file change?" is
answered by comparing `source_hashes` in the manifest (§ Skip logic), which also
catches additions and deletions by key-set comparison. `ShardStore` takes the
default impl and `check_stale_files` goes with it. Record this rationale in a doc
comment on the impl, so the default does not read as an unimplemented stub.

**Fidelity is the thing that can silently fail.** If a loaded shard does not yield
the same network-relative keys `speculative_path_key` produces, every lookup misses,
every node falls through to `Generated`, and the parse still succeeds. Issue 75 found
this path fragile once already (`beliefbase_architecture.md` §2.2.1, the `cache_fetch`
miss when `--write` was off). Two tests gate it, and the second is the more useful
diagnostic: the BID-stability round-trip reports that the aggregate went wrong, while
the path-key fidelity test names *which* key shape broke. Both are in § Testing
Requirements.

**Three consumers, one loader.** MCP static mode (`mcp/state.rs:272-380`) and the
browser viewer (`wasm.rs:819`) each hand-roll shard deserialization today. MCP static
mode adopts `ShardStore` once the parse path is verified, retiring the
`TODO(Issue 66)` hook in `src/mcp/state.rs`. The viewer keeps its own loader for now —
it is `wasm32` and carries its own `loaded_shards` eviction bookkeeping — but the two
should converge, and `ShardStore` is the shape they converge on.

> **Non-foreclosure for Issue 74.** `generational_archive.md` §3.1 requires archived
> stub shards to "hydrate through the existing shard path" so that `compute_diff`
> receives a real `BeliefBase` with relations and indices. Because `ShardStore` is a
> backend rather than a startup step, the archive's old side is simply **a second
> instance over a different generation**. A startup-step design would have required a
> parallel loader; this one does not. Do not reintroduce an assumption that only one
> store exists per process.

### One writer per output directory

Once shards are the identity store, two processes exporting to one output directory
can interleave their writes and produce a mixed generation. Before this issue that
cost a re-render; after it, it costs every BID in the corpus. The store therefore
enforces what `living_corpus.md` §2 already asserts about Layer 2 —
**single-owner-per-node, one writer** — as opposed to Layer 3, which is many-writer
and uncoordinated by design.

The two hazards are different and need different mechanisms. Conflating them produces
a design where readers block on every export.

| Hazard | Mechanism |
|---|---|
| Two writers interleaving a generation | exclusive advisory lock on the output directory |
| A reader observing a half-written shard | temp-write + atomic rename; manifest renamed last |
| A reader observing a consistent but stale generation | nothing — that is correct behaviour |

**The writer lock.** A `ShardStore` opened for writing acquires an exclusive advisory
lock on the output directory at open, holds it for the process lifetime, and releases
it on exit. `noet serve` takes it the moment it comes online against a given output
directory — not per export — so the "there is exactly one writer" invariant holds for
the whole session rather than only during the write burst.

**Contention fails fast**, with a message naming the holder:

```
another noet process is writing to `_site/` (pid 4821, since 14:02:11)
run against a different --html-output, or stop that process
```

Failing loses no work, and this is worth stating because the instinct is to block:
**Layer 2 is a pure function of Layer 1** (`living_corpus.md` §2). A refused parse has
written nothing and discarded nothing; re-running it reproduces the identical graph.
The artifact that genuinely cannot be re-derived is an annotation, and annotations
never pass through this writer — they are Layer 3, written to the halo of stores
(`annotation_channel.md` §6) alongside the shard corpus rather than into it. A
blocking acquire would instead turn a hand-run `noet parse` into a silent hang behind
a watch daemon, and give CI a queue where it wants an error.

**Readers take no lock.** MCP static mode and the browser viewer open the store
read-only, and must never block behind an export. Safety comes from atomic publication
instead: `export_sharded` currently writes each shard in place with `tokio::fs::write`
(`export.rs:388`, manifest at `:425`), which truncates — a reader mid-write sees a
truncated msgpack. Writing each file to a temp name and `rename`-ing it into place
makes every observation atomic, and renaming `manifest.json` **last** makes it the
commit point: a reader either sees the whole prior generation or the whole new one.
The manifest-last ordering is already what the code does; only the atomicity is
missing. This also makes MCP's existing mtime-polling reload (`mcp/mod.rs:96-130`)
sound, which today races the export it is watching for.

> **Scope.** This guards *local* concurrency. Advisory locks are unreliable on NFS,
> and the CI shard-restore path crosses machines entirely, so an unbroken chain across
> runners remains an operational concern — see § Why this issue is first.

### File-based DB for debugging

With `ShardStore` backing the parse path, SQLite has no role in identity or
resolution. The `--db` flag and its persistent `belief_cache.db` are replaced by
`--debug-db <path>`, which is unambiguously **write-only debugging output**:

- `--debug-db /tmp/noet-debug.db` mirrors the session's graph state into a SQLite
  file for inspection — `sqlite3 /tmp/noet-debug.db` to examine nodes, run ad-hoc
  queries, debug edge resolution.
- The file is overwritten each session and never read back. Nothing in the startup
  path consults it, so there is no "must not read this" discipline to enforce — the
  read path no longer exists.
- Also settable via `NOET_DEBUG_DB=path`.

The debug DB is a window into the live session, not a cache and not a persistence
layer.

### CLI consequences: `--write` retires, `--html-output` becomes required

Retiring `--write` moves identity persistence **from N per-codec source caches to
one durable store.** That is the whole of the argument, and it is worth stating in
its general form because the special cases are otherwise easy to mistake for
exceptions.

Every mechanism below writes a value into source so that the *next* parse can read
it back and recover identity. Each exists because there was no durable store to
recover it from. `ShardStore` is that store:

| Cache | Written into | Recovered from the store by |
|---|---|---|
| `bid:` frontmatter | markdown | the hydrated node's BID |
| `{#anchor}` heading injection | markdown | the hydrated node's `id` |
| `[sections."id://x"]` table | markdown | the hydrated section nodes |
| `__noet_bid__`, `tabs_meta.<tab>.bid` | xlsx cells | the hydrated node's BID |
| hidden `RelationBref` columns | xlsx | the hydrated relation (`xlsx/codec.rs:669-692`) |

The xlsx entries are the least visible — a hidden column is not something you notice
reading the file — but they are instances of the rule, not exceptions to it. The
comment at `xlsx/codec.rs:671` says as much: the bref "was written by a prior
`--write` pass" to give "stable resolution even if the human-readable cell text
changes." That is a hand-rolled identity store.

Keeping both means two identity stores that can disagree — a frontmatter BID and a
shard BID for the same heading — which is worse than either alone.

- **Remove `--write` from `Parse` and `Watch`** (`src/cli.rs:192-194`, `:259-262`;
  note `#[arg(short, long)]` also binds `-w`). `DocumentCompiler::new` /
  `with_html_output` lose the `write: bool` parameter, as do `WatchService::new` /
  `with_html_output` and `FileUpdateSyncer::new`, which carry a parallel chain
  (`watch.rs:293`, `:305`, `:324`, `:354`, `:517`, `:730`). `parse_one_path`'s
  write-back block (`compiler.rs:2159-2190`) goes with it — **both arms**, text and
  binary; the binary arm is `generate_source_bytes` for xlsx.
  The `generate_source() != content` check (`builder.rs:1488-1496`) stays — it
  still answers "did normalization change anything?" for diagnostics, and Issue
  107 (codec write-back) will need it — but nothing acts on the answer here.
- **Make `--html-output` required on `Parse`** (`cli.rs:200-202`). The shard
  directory lives under it, and a parse that writes no shards preserves no
  identity; a parse with no output directory is now a parse whose BIDs are
  discarded, which is not a mode worth supporting. `Watch` already requires an
  output directory when `--serve` is set; make it unconditional there too.
  **`DocumentCompiler::html_output_dir` stays `Option<PathBuf>`** — required at the
  CLI is not the same as non-optional in the struct, and `DocumentCompiler::simple`,
  `WatchService::new`, and ~20 tests still construct with `None`.
- **What `--write` also did**: link normalization and the `bref://` title
  annotation ride the same `rewritten_content` path. Those are source *edits*, not
  identity persistence, and they belong to Issue 107's write-back — which runs
  through `BeliefEvent` → codec, not through a parse-time flag. Record in Issue 107
  that it inherits normalization-on-request; do not preserve `--write` as a stopgap.

**Consequence to state plainly**: shard presence becomes load-bearing for identity
in cases where a source-embedded fallback previously existed. A cold parse with no
shard chain recovers nothing. This is the same chain fragility § Why this issue is
first already documents for BIDs, now applying uniformly.

**Test migration.** Tests that observe write-back by reading the source file back
from disk migrate to observing `payload["text"]` — the pattern two alias tests
already use (`tests/codec_test/alias_tests.rs:223`, `:272`). Three groups need more
than a parameter drop, and the CLI teardown should not start until they are
scoped:

- `tests/codec_test/xlsx_tests.rs:486`, `:549` assert BIDs land *inside the workbook
  bytes*. They test the mechanism being removed; delete or re-point at the store.
  `:397` derives BID continuity from the file rather than a store and needs a
  shard-backed harness.
- `tests/cache_invalidation_test.rs` — all 5 tests open `belief_cache.db`
  out-of-band to observe mtimes. Both the file and the mtime table are going away.
- `tests/codec_test/bid_tests.rs` `_db` variants and `link_tests.rs:106` use a file-DB
  cold start as the harness. The shard equivalent is the BID-stability round-trip
  below. Note the two `in_memory` variants already pass `write = false` and
  self-manage persistence, and `collect_bids` (`compiler.rs:6711-6745`) has a single
  caller passing `false` — both are arity drops only.

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

### Naming

There is no new trait. `ShardStore` is a new *implementation* of the existing
`BeliefSource` (`src/query/mod.rs:40`) and `BeliefSink` (`src/beliefbase/sink.rs:41`)
traits, so the name only has to describe a store — not disambiguate itself from the
query-execution trait.

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

3. **`ShardStore` + incremental skip in parse/watch startup** (1.5 days)

   **Spike first.** Before estimating the rest of this step: load a fixture's shards
   through `BeliefBase::merge`, recompute `speculative_path_key` for a sample of
   headings, and assert the keys resolve to the expected BIDs. This converts the
   design's central claim into evidence and is the cheapest place to discover a
   path-key mismatch.

   - [ ] Implement `ShardStore` with `BeliefSource` (`src/query/mod.rs:40`) and
         `BeliefSink` (`src/beliefbase/sink.rs:41`). It owns a `BeliefBase`, the
         manifest, and the set of currently-loaded network brefs
   - [ ] Load a shard by deserializing `{bref}.msgpack` and calling
         `BeliefBase::merge` — pass 3 builds the `PathMapMap` (`base.rs:2996`).
         Model on `wasm.rs:819-918`; lift the shared deserialization out of
         `mcp/state.rs:330` rather than writing a third copy
   - [ ] Route a key to its home shard: `Path`/`Id` carry `net` directly;
         `Bid`/`Bref` resolve via `GlobalShard.bref_index`. Load the global shard
         eagerly at open
   - [ ] `evaluate` loads the home shard on miss, then retries. A key that resolves
         to no known shard is a genuine miss
   - [ ] `apply_batch` marks the touched network dirty; no per-event shard write
   - [ ] Re-export dirty shards at `finalize_html` via the existing `export_sharded`
   - [ ] **Exclusive writer lock** on the output directory, acquired at open for a
         writable store and held for the process lifetime. Fail fast on contention
         with a message naming the holding pid and its start time. Readers
         (MCP static, viewer) open read-only and take no lock
   - [ ] **Atomic publication** in `export_sharded`: write each shard and the manifest
         to a temp name and `rename` into place, manifest last. Removes the torn-read
         window that `tokio::fs::write` (`export.rs:388`, `:425`) leaves open
   - [ ] Monolithic: absence of `beliefbase/manifest.json` → load
         `beliefbase.msgpack` as a single unit
   - [ ] `get_file_mtimes` takes the default-empty impl; document why in a doc
         comment on the impl. Retire `check_stale_files` and its call sites
         (`watch.rs:815`, `compiler.rs:1214`, `:1476`)
   - [ ] Swap `BeliefAccumulator::new(DbConnection(db_pool), rx)` →
         `BeliefAccumulator::new(ShardStore::open(..)?, rx)` in `cli.rs:583-602`.
         Decide what the `#[cfg(not(feature = "service"))]` arm (`cli.rs:603-607`)
         does — it currently has no DB at all
   - [ ] Remove `belief_cache.db` creation from `WatchService::with_html_output`
         (`watch.rs:336-338`). Remove the dead `BELIEF_CACHE_DB` const (`db.rs:44`)
   - [ ] Replace `--db` with `--debug-db <path>` (and `NOET_DEBUG_DB`)
   - [ ] Read `beliefbase/manifest.json` at startup; classify each network clean or
         dirty by comparing stored `source_hashes` against step 1b's hashes
   - [ ] Handle missing files (dirty), new files (dirty), `--force` (all dirty)
   - [ ] Assert the invariant in review: no code path may conclude *clean* from an
         mtime comparison. A `stat` may short-circuit toward *dirty* only
   - [ ] Parse only dirty networks. Confirm a dirty network's re-parse resolves
         unchanged headings through `cache_fetch` → `GlobalCache` rather than
         `Generated`
   - [ ] **`--force` re-parses everything but still loads prior shards.** Force means
         "do not trust the skip decision", not "discard identity". A separate
         `--fresh-bids` (or equivalent) is the only way to intentionally re-mint
   - [ ] Log the `GlobalCache`/`Generated` ratio per re-parsed network at `info!` —
         a high `Generated` count on a lightly edited network is the symptom of a
         fidelity failure that otherwise passes silently
   - [ ] Add a summary line at `tracing::info!` level:
         `"N/M networks reused from shard cache; K networks re-parsed"`
   - [ ] Assert **at rest, `Trace` implies `External`** on both crossings (export and
         load), logging a violation that names the node. `generational_archive.md`
         §3.2 drops the archive stub's kind set on the strength of this invariant

4. **WebSocket shard-invalidation endpoint** — MOVED to Issue 102 (`noet serve`). Issue 66 provides the per-network `compiled_at` values that the broadcast carries; Issue 102 owns the endpoint, the consumer registry, and the SPA reload logic.

5. **MCP static mode adopts `ShardStore`** (0.5 days) — *after step 3 is verified*
   - [ ] Replace `mcp/state.rs:272-380`'s hand-rolled loader with `ShardStore`;
         retire the `TODO(Issue 66)` hook
   - [ ] `check_consistency` reports per-network `compiled_at` from the manifest

6. **CLI teardown** (1 day) — *after step 3 is verified*
   - [ ] Drop `write` from `DocumentCompiler` (`compiler.rs:146`, `:281`, `:302`,
         `:357`, `:437`, `:1887`) and the `WatchService`/`FileUpdateSyncer` chain
         (`watch.rs:293`, `:305`, `:324`, `:354`, `:517`, `:730`)
   - [ ] Remove the write-back block (`compiler.rs:2159-2190`), **both arms**
   - [ ] Remove `--write` from `Parse` and `Watch`; make `--html-output` required on
         `Parse` and unconditional on `Watch`, collapsing the two-arm constructor
         splits (`cli.rs:615`, `:690`, `:955`, `:990`)
   - [ ] Migrate disk-observing tests to `payload["text"]`; re-point or delete the
         xlsx byte-assertion tests and the `cache_invalidation_test.rs` suite
         (see § CLI consequences)
   - [ ] Fix stale docs: `src/lib.rs:118` doctest, `src/watch.rs:177-194` and `:202`,
         `src/mcp/mod.rs:431-432`, `src/mcp/state.rs:12-13`
   - [ ] Reconcile `docs/project/UX_AUDIT.md` §3.5, which argues for zero-config
         `noet parse <dir>` — the opposite of required `--html-output`. A decision,
         not a silent edit

7. **Tests** (0.75 days)
   - [ ] Unit test: fixture shard with known `source_hashes`; assert network skipped
         when no file's content changed; assert re-parsed when one file's bytes change
   - [ ] Unit test: a file whose mtime is bumped but whose bytes are unchanged is
         classified **CLEAN** (use `filetime` crate, already in `dev-dependencies`)
   - [ ] Unit test — **false-clean regression**: a file whose bytes change while its
         mtime is set *backwards* (the `rsync -t` / archive-extraction case) is
         classified **DIRTY**. Under the rejected mtime-first ordering this test fails
   - [ ] Unit test — **false-clean regression**: two distinct writes to one file
         within the same whole second are both detected. `as_secs()` truncation
         (`src/codec/compiler.rs:567`) makes this invisible to mtime
   - [ ] Unit test: `ShardStore::open` against a fixture output directory; assert the
         expected node count after loading
   - [ ] Unit test: fetch-on-miss — query a key whose home shard is not loaded;
         assert it resolves and the shard is now loaded
   - [ ] Unit test: a second writable `ShardStore` against a locked output directory
         fails with the holder named; a read-only open against the same directory
         succeeds
   - [ ] Unit test: a reader opening concurrently with an export observes either the
         whole prior generation or the whole new one — never a truncated shard and
         never a manifest referencing a shard that is not yet in place
   - [ ] Regression: `noet parse --force` produces identical output to a fresh parse
         on a clean tree
   - [ ] Regression: `noet watch` startup does not create `belief_cache.db`

8. **Closeout: reconcile the archive design** (0.25 days)

   `identity/generational_archive.md` was drafted against the `DbConnection` parse
   path. Three of its claims are affected, and all three are load-bearing for Issue 74
   rather than cosmetic.

   - [ ] §3.4 names `DbConnection` as "the untested half" of the bare-`Trace`
         invariant and asks for assertions on two dissimilar crossings. That half no
         longer exists; both crossings are shard crossings through `merge`
   - [ ] §9.1's `STAGED` generation and this issue's deferred whole-shard re-export are
         the same operation — confirm they are described as such
   - [ ] §3.1's "hydrate through the **existing** shard path" now names `ShardStore`;
         confirm the archive's old side is described as a second instance rather than
         a parallel loader
   - [ ] Either correct the three sections directly — **rewriting the claims, not
         annotating them** (AGENTS.md § No Historical Narrative) — or, if the scope is
         larger than an edit, file the cleanup against Issue 74, which owns the archive

## Testing Requirements

- **Path-key fidelity (the sharpest diagnostic)**: load a fixture's shards into a
  `ShardStore`; for a sample of headings spanning a document root, a nested section,
  a subnet child, and a node reached only through the Trace halo, recompute
  `speculative_path_key` and assert each key resolves to the BID the prior parse
  recorded. The BID-stability round-trip below reports that the aggregate went
  wrong; this names *which key shape* broke. Run it first — it is the spike in
  step 3 promoted to a permanent test.
- **BID stability round-trip (the primary test)**: parse a multi-network fixture;
  record every node's BID from the shards; edit one file in one network; parse again
  with the prior shards present. Every node whose source is unchanged — including
  nodes in the *edited* network — has the same BID as before. Only nodes whose heading
  text or position changed may differ. **This test is expected to fail before this
  issue lands**, because every non-persisted BID is currently re-minted per parse.

  **Build this by adapting `tests/codec_test/bid_tests.rs`, not from scratch.** Its
  `test_sequential_db` / `test_parallel_db` already do the right shape: parse 1
  populates a persistent store, parse 2 cold-starts from it with a fresh compiler and
  asserts zero `rewritten_content` and zero graph-modifying events. Three changes
  turn it into this test: (1) the store is a `ShardStore` over the shard directory,
  not `belief_cache.db`; (2) parse 1 no longer writes
  `rewritten_content` to disk (`apply_rewrite`, L210-225) — with `--write` retired
  the *only* thing carrying BIDs across the two parses is the shards, which is what
  the test must prove; (3) add the edit-one-file step between the parses, and assert
  BID equality on unchanged nodes by set comparison rather than only asserting "no
  events" — the zero-event check is necessary but does not distinguish "same BIDs"
  from "nothing was compared". The `in_memory` variants stay as-is; they test the
  parse pipeline, not persistence.
- BID stability across a shard-only cold start: delete the source tree's `.git` and
  any `belief_cache.db`, keep the shards, parse again — BIDs are unchanged. Nothing
  other than the shards may be load-bearing for identity. (The `db` variants above
  currently prove this for `belief_cache.db`; once that file is removed from the
  startup path they must prove it for shards, or they test a store that no longer
  exists.)
- `noet parse` on an unchanged multi-network source tree of a few thousand documents
  (after an initial full parse) skips all networks and completes in < 1 second,
  *including* the full-tree hash pass
- `noet parse --force` re-parses everything; output is byte-identical to a fresh
  parse **and BIDs are preserved** — force bypasses skip, not identity
- Modifying one source file causes only the containing network to be re-parsed; others
  are skipped and their data is present in the store
- A content change is detected regardless of what the mtime does — unchanged, moved
  backwards, or within the same whole second as the previous write
- Deleting a source file causes the containing network to be re-parsed
- The `ProtoIndex::build()` hash pass adds no more than a small fraction of a full
  parse's wall time on a few-thousand-file corpus
- `noet watch` startup does not create `belief_cache.db`; the store loads from shards
  and query results match a fresh parse
- MCP `check_consistency` (live mode) surfaces `ParseDiagnostic::UnresolvedReference`
  entries from `DocumentCompiler::last_diagnostics()` correctly
- MCP static mode returns correct `check_consistency.compiled_at` values per network
- A query for a key whose home shard is not yet loaded resolves correctly, and the
  home shard is loaded as a result (fetch-on-miss)
- Query results from a `ShardStore` loaded from a run's own output shards match the
  results from the live `BeliefBase` that produced them (hydration equivalence)

## Success Criteria

- [ ] **An unchanged node keeps its BID across two cold `noet parse` runs with no
      `--write`**, given the prior shards, whether or not its network was re-parsed.
      The BID-stability round-trip test passes
- [ ] `NetworkShardMeta` has `compiled_at` and `source_hashes` (SHA-256 hex) fields
      with `#[serde(default)]`; existing manifest roundtrip tests pass
- [ ] `ProtoIndex::build()` produces a content hash per in-scope source file, grouped
      by `net_dir_partition`'s existing `network_dir → children` partition, hashed in
      pass 2 only
- [ ] `ShardStore` implements `BeliefSource` + `BeliefSink` and backs the
      accumulator in place of `DbConnection`; no SQLite is on the parse path
- [ ] The path-key fidelity test passes for document roots, nested sections, subnet
      children, and Trace-halo nodes
- [ ] A key whose home shard is unloaded resolves by fetch-on-miss
- [ ] Exactly one writable `ShardStore` may hold an output directory; a second fails
      fast naming the holder, and readers are never blocked by a writer
- [ ] Shards and the manifest are published by atomic rename, manifest last, so no
      reader can observe a torn generation
- [ ] `noet parse` on an unchanged corpus skips all clean networks and logs a summary
      line showing reuse count
- [ ] `--force` bypasses skip logic but still loads prior shards; output is
      identical to a fresh parse and BIDs are preserved
- [ ] `--write` is removed from `Parse` and `Watch`; `--html-output` is required on
      `Parse`; `DocumentCompiler` constructors no longer take `write`. No test passes
      `write = true`
- [ ] `noet watch` does not create `belief_cache.db`; startup loads from shards
- [ ] `DocumentCompiler::last_diagnostics()` exists; MCP `check_consistency` live
      mode uses it to surface `UnresolvedReference` diagnostics
- [ ] At rest, `Trace` implies `External`, asserted on both the export and load
      crossings, with violations logged by node
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

## Performance: deferred until fidelity is proven

This issue's deliverable is identity, not speed. Everything below is deferred so that
the first pass optimises nothing and hides nothing — a `ShardStore` that loads every
shard eagerly is correct, and correctness is what the annotation wave is waiting on.
Promote these once the path-key fidelity and BID-stability tests pass.

- **Memory budget and eviction.** `ShardConfig::memory_budget_mb` already exists and
  already caps client-side loading in the viewer. Server-side it becomes an eviction
  policy over `ShardStore`'s loaded set: drop a network's nodes and reload on the next
  touch. Because fetch-on-miss makes a partially-loaded store *correct*, this is purely
  a footprint knob — which is why it is not in the first pass. Add
  `--memory-budget <MB>` when it is.
- **Eviction boundary.** During a `serve` session the store backs several concurrent
  consumers (viewer, MCP, LSP). Eviction must not race an in-flight query;
  `compiler_idle_notify` in `FileUpdateSyncer` is the trigger boundary, extended to
  account for all consumers rather than only the compiler.
- **Partial-load behaviour under a budget.** With eviction active, measure how often a
  parse touches an evicted network and what the reload costs. The Trace halo means the
  worst case is a re-read, not a wrong answer, but the frequency is unmeasured.
- **`ProtoIndex::build()` hash pass.** Benchmark the full-tree read added in step 1b on
  a few-thousand-file corpus and record the delta. Estimated at roughly one `cat` of
  the tree (~50–150 ms warm for ~3,000 files / ~30 MB); if the measurement contradicts
  the estimate, revisit before shipping. Do **not** add a `(path, mtime, size)` hash
  cache to avoid it without reading § On hash caching first.
- **Incremental shard export.** `apply_batch` defers to a whole-shard re-export at
  finalize. If re-exporting a large unchanged-but-dirty shard proves costly, the
  question becomes worth asking; nothing needs it yet.

Measurements belong in the application-specific `PERFORMANCE_LOG.md`, with mechanisms
and status recorded in `ISSUE_97_BUILD_PERFORMANCE_BOTTLENECKS.md`.

## Risks

- **Generated corpora defeat content hashing too**: Upstream document generators
  commonly stamp a render timestamp into each generated document's frontmatter — a
  `generated:` or `generated_date:` field interpolated at render time. When such a
  generator re-runs, every output file gets a new content hash even when the upstream
  source data is unchanged. In one measured corpus, ~1,260 of the generated Markdown
  documents carried such a timestamp; an incremental parse would correctly classify
  all of them as dirty and skip nothing. The skip rate on that corpus would be zero.
  **The BID-stability goal survives this** — prior shards back the store regardless of
  what is skipped, so unchanged headings keep their BIDs even when nothing is skipped —
  but the corpus-currency goal does not, since every build is still a full parse.
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
- **Load fidelity**: a `ShardStore` loaded from shards must be query-equivalent to the
  live `BeliefBase` that produced them. Any shard format gap (e.g. missing
  `WEIGHT_OWNED_BY` — see Issue 64 debug notes) will produce query differences.
  → **Mitigation**: the hydration-equivalence test in § Testing Requirements.
- **A store that resolves nothing looks like success.** If loaded shards do not yield
  the keys `speculative_path_key` produces, every `cache_fetch` misses, every node is
  `Generated`, the parse completes, the output renders, and the skip rate is fine — but
  every BID has changed and every downstream annotation is orphaned. Nothing fails
  loudly. → **Mitigation**: the path-key fidelity test is the gate, because it names
  the broken key shape rather than reporting an aggregate; additionally log the
  `GlobalCache`/`Generated` ratio per re-parsed network at `info!`, since a high
  `Generated` count on a lightly edited network is the symptom.
- **The shard chain is the identity store, and it can break.** A CI cache miss, a
  deleted `_site/`, or a shard format change re-mints every BID. → **Mitigation**:
  this issue makes stability *possible*; keeping the chain unbroken is an operational
  concern for the deploying pipeline (restore shards before parse), and anchor
  consumers should carry a secondary re-attachment key. Both are noted in § Why this
  issue is first; neither is fixed here.
- **Replacing the store is a larger step than hydrating into one.** `ShardStore`
  displaces `DbConnection` on the parse path rather than sitting beside it, so a defect
  in it has no fallback. → **Mitigation**: the constituent parts all ship today —
  `merge` (`base.rs:2946`), shard deserialization (`wasm.rs:819`, `mcp/state.rs:330`),
  `BeliefSink for DbConnection` as the write template (`sink.rs:78`) — and the
  accumulator is already generic over its store, so the swap is one line at
  `cli.rs:601`. The spike in step 3 runs before any of it is wired.
- **A stale lock file strands an output directory.** A process killed with `SIGKILL`
  cannot run its release path. → **Mitigation**: use OS advisory locking
  (`flock`/`LockFileEx`) rather than a hand-rolled lock file — the kernel releases the
  lock when the fd closes, including on abnormal termination, so there is no stale
  state to reap. A pid file alongside it carries the diagnostic message only and is
  never the lock itself.
- **Monolithic mode gets no skip**: below the 2MB threshold there is no per-network
  manifest to consult. → **Mitigation**: identity still works — monolithic is the
  one-shard case for `ShardStore`, so only *skip* is unavailable. Large repos that
  benefit from skip are also large enough to be sharded.
- **`last_diagnostics` snapshot timing**: The snapshot must be taken before the
  `latest_results` drain to capture the full diagnostic set. If taken after, the
  drain discards the data. → **Mitigation**: Code review checkpoint; add an assertion
  in tests that `last_diagnostics` is non-empty after a parse with known errors.

## Open Questions

- **Is a bare-`Trace` node at rest an integrity defect?** Trace marks partially
  loaded relations — an in-transit condition — and it is stripped on merge. It
  should therefore never survive into a shard or the DB. The exception is
  `External | Trace`, which marks a *permanently* incomplete node (href, asset,
  API, namespace root) where no deeper fetch will ever yield a complete version;
  those are at rest legitimately and in volume.

  **The invariant to test: at rest, `Trace` implies `External`.** This issue owns
  it because it owns both crossings — shard export and shard load — and either is a
  natural place to assert it. With `ShardStore` replacing `DbConnection` on the parse
  path, both crossings run through `merge`, so one assertion site covers both rather
  than two dissimilar ones. Expect violations on a real corpus: each one is a
  machinery defect to chase, not a case to accommodate.

  **Indirect evidence says it already holds for shards.** The SPA loads shards on
  demand across a ~2,445-shard corpus via `bref_index` lookups and `get_context`
  misses. A node reporting `is_complete() == false` while in fact complete
  produces spurious fetches and unresolvable metadata panels — a failure class
  that surfaces immediately in a browser. `export.rs` also states the intent
  directly: "Trace nodes introduced by balanced traversal (cross-network
  references) are excluded."

  Two consumers depend on this. `identity/generational_archive.md` §3.2 drops the
  kind set from its archive stub on the strength of the invariant, accepting a
  frivolous blob fetch as the cost of a violation — which is why §3.4 asks for a
  **loud warning naming the node** rather than silent tolerance.
  `content_versioning.md` §5.2 excludes Trace from hashing, which becomes vacuous
  at rest if the invariant holds and still guards the in-memory case if it does
  not.

- Should `source_hashes` store paths relative to the repo root or relative to the
  network directory? Repo-root-relative is stable across network moves; network-
  relative is shorter. Recommend repo-root-relative for unambiguity.
- **What preserves a BID when a heading is renamed?** The store resolves by path key,
  so a heading whose slug changes misses `cache_fetch` and re-mints even though it is
  "the same" section. That is Issue 36's content-identity problem and is explicitly
  out of scope here — this issue preserves identity for *unchanged* nodes only. State
  the boundary so a rename-induced re-mint is not filed as an Issue 66 regression.
- **Should the shards record their own lineage?** A `parent_compiled_at` or manifest
  hash in `NetworkShardMeta` would let a consumer detect a broken chain ("these BIDs
  descend from no prior run") rather than inferring it from mass orphaning. Cheap;
  not required for the primary goal; recommend deferring until an anchor consumer
  asks for it.
- Should the incremental skip summary line go to `tracing::info!` or `tracing::debug!`?
  Recommend `info!` — users benefit from seeing that incremental is working.
- **What does the `#[cfg(not(feature = "service"))]` path use?** `cli.rs:603-607`
  currently builds the accumulator over a bare `BeliefBase` with no DB at all. Since
  `ShardStore` needs no `service` feature — it is filesystem plus `BeliefBase` — it
  could become the single store for both arms, removing the branch. Confirm at
  implementation time.
- **`--write` removal: hard or deprecated?** Recommend hard removal in the same
  release as `--debug-db`, with an error pointing at this issue — a deprecated
  `--write` that still stamps frontmatter BIDs would create the two-identity-store
  problem § CLI consequences describes. Anyone relying on it for link normalization
  is waiting on Issue 107.
- `--db` → `--debug-db` migration: should `--db` be removed immediately or
  deprecated with a warning for one release? Recommend hard removal with a clear
  error message: "The --db flag has been replaced by --debug-db. The file-based DB
  is now a write-only debugging artifact, not a persistence layer. See Issue 66."
- **Which advisory-locking crate?** No dependency provides this today and no locking
  precedent exists in the crate — every `lock()` in `src/` is an in-process mutex.
  `fs4` (maintained successor to `fs2`) and `fd-lock` are the candidates; both wrap
  `flock`/`LockFileEx`. Pick at implementation time on maintenance and platform
  coverage, not features.
- **Does `noet parse` need the lock, or only `noet serve`?** A one-shot parse writes a
  generation and exits, so it must hold the lock across its export. The asymmetry is
  duration, not applicability: `serve` holds for its session, `parse` for its run.
  Confirm there is no read-only `parse` mode that should be exempt once
  `--html-output` is required.
- **When does the browser viewer adopt `ShardStore`?** MCP static mode adopts it in
  step 5. The viewer (`wasm.rs:819`) is the third hand-rolled loader, but it is
  `wasm32` and carries its own `loaded_shards` eviction bookkeeping, so it stays put
  for now. Converging it is worth an issue once the server-side eviction policy exists
  to compare against.
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

- [`docs/design/identity/generational_archive.md`](../../design/identity/generational_archive.md) —
  §3.1 requires archived stubs to hydrate through the existing shard path, which
  `ShardStore` provides as a second instance over another generation; §3.4 assigns this
  issue the bare-`Trace` invariant on both crossings; §9.1's `STAGED`-overwritten-every-
  parse is this issue's deferred write. **Closeout task**: §3.4 and §9.1 describe a
  `DbConnection` crossing that no longer exists — assess and either correct them
  directly or hand the cleanup to Issue 74
- [`docs/design/identity/content_versioning.md`](../../design/identity/content_versioning.md) — owns
  per-*node* content hashing, which is **out of scope for this issue** (Issue 105
  owns it). Its §6 determinism requirement covers the *hash* half of a
  `(bid, version)` anchor; this issue supplies the *BID* half. This issue's
  `source_hashes` is a per-*file* hash and is unrelated to the node hash.
- `src/properties.rs:239-241` — `Bid::new` is `Uuid::now_v6`; a BID not resolved from
  cache is time-based and differs on every parse
- `src/codec/builder.rs:2306-2308` — the `Generated` fall-through in `push` that mints
  a fresh BID on a `cache_fetch` miss; hydration exists to make this branch rare
- `docs/design/core/beliefbase_architecture.md` §2.2.1 (Issue 75 finding) — the prior
  `cache_fetch` miss when `--write` was off; the same path key fidelity governs whether
  hydration preserves BIDs
- Issue 36 (content-based section identity) — owns BID preservation across *renames*;
  this issue covers unchanged nodes only
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
- `src/query/mod.rs:40` — the `BeliefSource` trait `ShardStore` implements
- `src/beliefbase/sink.rs:41`, `:78` — `BeliefSink`, and `DbConnection`'s impl as the
  write-side template
- `src/beliefbase/base.rs:2946`, `:2996` — `merge_graph_mut`; pass 3 drives
  `PathMapMap` and builds the path index that makes `NodeKey::Path` resolvable
- `src/beliefbase/accumulator.rs` — `BeliefAccumulator<S>`, generic over its store;
  the swap point is its type parameter
- `src/wasm.rs:819-918` — `BeliefBaseWasm::load_shard`, the shipped precedent for
  deserialize-and-merge; `:855` for `bref_index` routing
- `src/mcp/state.rs:272-380` — the second hand-rolled shard loader; adopts
  `ShardStore` in step 5
- `src/shard/export.rs:279-360` — the Trace halo: extern endpoints, `{maps_to}`
  owners, and namespace parents embedded per shard, which is what makes a
  partially-loaded store traversable
- `src/shard/manifest.rs` — `NetworkShardMeta`, `ShardManifest`, `network_shard_meta()`
- `src/shard/export.rs` — `export_sharded`, `export_beliefbase`; `:388` and `:425` are
  the in-place writes that become temp-write + atomic rename
- `src/mcp/mod.rs:96-130` — `maybe_reload`, the reader that polls `manifest.json` mtime
  and today races the export it watches for
- [`docs/design/annotation/living_corpus.md`](../../design/annotation/living_corpus.md) §2 —
  Layer 2 is single-owner-per-node and a pure function of Layer 1; Layer 3 is
  many-writer and append-only. The writer lock enforces the first, and the second is
  why refusing a parse loses no work
- `src/shard/wire.rs` — `NetworkShard`, `GlobalShard` (deserialization types)
- `src/codec/xlsx/codec.rs:669-692` — the hidden `RelationBref` column: a per-codec
  identity cache that `ShardStore` supersedes
- `src/codec/compiler.rs` — `DocumentCompiler`, `latest_results` drain, `parse_all`/`parse_sequential`
- `src/codec/diagnostic.rs` — `ParseDiagnostic::UnresolvedReference`
- `src/db.rs` — `db_init_memory`, `DbConnection`; the store `ShardStore` replaces on
  the parse path
- `src/cli.rs:583-602` — where the accumulator's store is constructed today; the swap
  point. (`src/bin/noet/main.rs` is three lines and holds no DB code)
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