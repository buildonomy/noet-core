# Backlog

This file tracks optional enhancements and future work extracted from completed issues.

## Remaining full-map scans in `PathMap` (measured, from Issue 102)

**Priority**: LOW — measured and deliberately deferred. Recorded so the next
audit starts from measurement rather than rediscovery.

Two rounds of this defect class have already been fixed: `indexed_get` gained a
path index (422,582,888 → 76,433 entries scanned), and
`generate_path_name_with_collision_check` was converted to use it
(1,684,103 → 10 entries examined). Neither moved wall clock — the scans were
real but not on the critical path. What remains:

- `rebuild_node_to_nets_for` scans all of `node_to_nets` then `retain`s, once
  per network — O(networks × nodes). Measured at **7.1%** of `PathMapMap::new`.
  The same pattern appears in `process_event_queue`, `process_nodes_removed`,
  and `process_node_renamed`.
- `PathMap::home_path` and `PathMap::all_paths` scan `self.map`, but run on
  export/finalize paths rather than per-edge. `home_path` could use `bid_map`
  if it ever measures hot.

## Nest the const-namespaces by URL segment (from Issue 102 Part 2)

**Priority**: LOW — deferred; the cost that motivated it was removed by other
means. Do not start without a fresh measurement.

`href_namespace` and `asset_namespace` are **flat**: every URL or asset is a
direct Section child of one namespace root, reaching ~78k children on a large
corpus. The proposal was to decompose each into `host / seg1 / seg2 / …`,
cutting max degree from ~78k to ~340 hosts. Depth 2 is the first level that
breaks the skew (one host is 59% of URLs); depth 3 leaves a 139-entry max.

### Why it is deferred

Nesting reduces **degree**, which defends against halo fan-out. It was carried
as active work on the theory that degree was also inflating epoch seeding cost.
It was not:

- Seeding pulled the *whole* namespace regardless of shape —
  `epoch_session_snapshot` BFSes the entire subgraph from each root, and
  `initialize_stack`'s fallback issues a `leaf_anchored` query that walks to
  every leaf. Re-parenting 78k leaves under 340 intermediates leaves the
  reachable set the same size, or slightly larger.
- The actual seeding cost was **redundant rebuilding**, not breadth: every
  epoch task rebuilt an identical index over the shared namespace. Sharing one
  prebuilt base per epoch (`GraphBuilder::seed_session_from_base`) took seeding
  2,533s → 524s and the parse phase 1.79x, without touching namespace shape.
- Part 1's path index already removed the read-path scan cost
  (422,582,888 → 76,433 entries scanned), and *that* produced no wall-clock
  change either — the scan was real but not on the critical path.

What remains genuinely gated on nesting is **demand-driven seeding** (pulling
only the namespace branches a document references). Nesting makes that
expressible, because a branch becomes addressable as a unit. But that is a
separate change, and with seeding now ~4.8x cheaper the payoff is much smaller
than when this was filed.

### The const-namespace workaround cleanup is gated on this

Roughly 50 commits accumulated defensive code against unbounded namespace
breadth. These become removable *in principle* once degree drops, each needing
its own measurement first:

| workaround | site | defends against |
|---|---|---|
| const-ns frontier filter (in-memory) | `base.rs::apply_traversal` | halo fan-out |
| const-ns frontier filter (SQL) | `db.rs::apply_traversal_sql` | halo fan-out |
| `anchored()` for const-ns keys | `builder.rs::cache_fetch` | halo fan-out |
| `leaf_anchored` vs `balanced` | `initialize_stack`, `sync_asset_snapshot` | halo fan-out |
| `content_namespaces()` hot-seed guard | `initialize_stack` | cost of pulling flat namespace |
| bulk asset registration | `compiler.rs::process_asset_batch` | per-event PathMap flush |

They are cheap and currently load-bearing, so this is not itself a reason to
pursue nesting. Bulk asset registration is orthogonal (setup overhead, not
breadth) — keep it regardless. One further entry, the `union_graphs` const-ns
union in `seed_session`, was already retired by the shared epoch base.

### If it is ever picked up

href BIDs are UUID v5 over the URL string, so intermediates are *computable*:
`buildonomy_href_bid("https://host/seg1")` is that intermediate's BID.
Ancestors need no lookups, a URL that is both leaf and prefix unifies into one
node automatically (8 such cases in the corpus), and identity is invariant
under depth changes, so the depth policy stays reversible. Asset BIDs are v6
time-based and **not** computable; asset intermediates would need a
generated-and-stored BID, or a path-derived v5 migration (see "Should asset
bids really be derived from their hash?" below).

Sketch: a pure URL/path → segments + leaf-key function, then
`ensure_href_namespace` / `ensure_asset_namespace` ensuring a *chain* rather
than a single edge.

**Done when**: max const-namespace node degree < 1k; resolved/unresolved link
counts unchanged; `map_insert` shift for both namespaces remains 0.

**Known risks**:
- *Stored path form breaks subnet descent.* `indexed_get`'s subnet fallback
  does `path.starts_with(subnet_path)` then `strip_prefix`, but leaves store
  full URLs and `AnchorPath::join` passes schemes through unchanged. Deciding
  between URL-relative leaf segments (with reassembly in `PathMap::path`) and
  scheme-aware stripping *is* the real work here — settle it before writing
  code.
- *Rendering-visible path changes.* Nesting changes the PathMap path for every
  href node; audit `get_nav_tree`, SPA nav, and href reconstruction first.
- *Intermediates polluting nav/search.* Mark them `Trace`; assert in tests.

**Rejected alternatives**: hostname-only nesting (leaves a 6,521-entry
container where depth 2 leaves 734, same implementation cost); dropping PathMap
for href and resolving purely by computed BID (assets carry 23x the entries and
their BIDs are not computable).

**Testing**: URL→node resolution identical flat vs nested; BID stability across
the change; a URL that is both leaf and prefix resolves to one node;
idempotence (second parse creates no new intermediates, no re-parenting).

## Unexplained `GlobalCache` rise after shared epoch session base

**Priority**: LOW — no known correctness or cost impact, but unexplained

Sharing one prebuilt `BeliefBase` across epoch tasks (`seed_session_from_base`)
moved the `cache_fetch` source mix on a full corpus run:

| source | before | after | Δ |
|---|---:|---:|---:|
| Generated | 227,464 | 228,655 | +1,191 |
| StackCache | 220,284 | 220,285 | +1 |
| GlobalCache | 4,250 | 5,223 | **+973 (+23%)** |
| SourceFile | 4,066 | 4,066 | 0 |

GlobalCache is 0.9% of fetches, so the cost is immaterial, and parse output was
verified identical against a baseline build (real paths, node kind/title/id/slug,
relations-by-title, at both `-j1` and `-j8`). But a 23% rise with the other three
flat is a real behaviour change, not noise.

Prime suspect is the `MergePrecedence::RhsWins` introduced for the doc-seed merge:
the previous `union_graphs` path let `doc_seed` win node collisions, and `RhsWins`
preserves that, but the surrounding node population differs — the shared base is
never replaced, so more nodes are present to collide with.

**Next step**: log the `NodeKey` variants driving GlobalCache fetches on both
paths and diff them. Cheap; the volume is small.

## Verify shared epoch session base under the `--db` backend

**Priority**: LOW

The shared-base change was validated on the in-memory path (chosen for
comparability with the recorded baseline). Nothing in the design is
backend-specific — the sharing is over derived in-memory structures, and
`PathMapMap` copy-on-write is backend-agnostic — but that is reasoning, not
measurement.

Note the warm-cache regression (Issue 97 Bottleneck 5: 94s cold vs 28m warm on
the same corpus) lives on the `--db` path and would dominate any timing taken
there, so this is a correctness check, not a performance one.

## QueryPackage as Topological Pagination (from Issue 83)

**Priority**: LOW — exploratory, no current consumer

`QueryPackage` already supports partial evaluation: the tape records
per-step intermediate state, `PackageStage` tracks progress, and the
evaluator resumes from wherever the tape left off. This is structurally
close to a **lazy pagination** mechanism:

1. Client sends a `QuerySpec` + pagination window (e.g. "evaluate steps
   0–3, return that tape slice + subgraph")
2. Server evaluates up to the window boundary, caches the package keyed
   by spec hash, returns the partial result
3. Client requests the next window (same spec hash, steps 4–7) — server
   resumes from cached tape position, no prefix re-evaluation

This gives topological pagination (ordered by projection steps) rather
than flat offset/limit pagination. The graph returned per window is the
subgraph materialized from that tape slice, so memory scales with the
window size, not the full result.

**Key design questions**:
- Cache eviction policy for partially-evaluated packages
- Whether the spec hash alone is sufficient for identity (it should be —
  the spec is immutable after construction)
- How to express the pagination window in the MCP/WASM surface
- Whether materialization should be deferred to the final window or
  incremental per-window

**Affected types**: `QueryPackage`, `Tape`, `PackageStage`, MCP tool
handlers, `BeliefAccumulator` cache.


## Recursive CTE for `DepthCount::Max` in `apply_traversal_sql` (from Issue 83)

**Priority**: LOW — performance optimization, not correctness

Replace the per-hop SQL loop in `DbConnection::apply_traversal_sql`
with `WITH RECURSIVE` when `DepthCount::Max`. Currently `max_hops()`
clamps `Max` to `MAX_TRAVERSAL` (10), so the loop is bounded at 10 SQL
roundtrips. Real-world section depth is typically 3–5. Three input roles
(Source, Sink, Owner) would each need a separate CTE branch. The CTE
must carry a `depth` column for per-hop tape entry partitioning and a
visited-set guard for cycle prevention.

Complexity vs. payoff is unfavorable unless profiling shows the per-hop
loop is a measurable bottleneck on large corpora.


## Traceability View Polish (from Issue 63)

**Priority**: LOW — not needed for current use cases

- **Column sort on header click**: clicking a column header sorts the table rows
  by that column's edge count ascending/descending. Pure JS, no WASM call needed.
- **Loading indicator for large submaps**: show a spinner or progress hint while
  `get_submap` + `get_context_bulk` are executing for high-depth or large networks.
- **Empty-state messaging**: when no rows have any edges for the visible WeightKind
  columns, display a friendly message instead of an empty table body.

## Link Format Enhancements (from Issue 04)

**Priority**: LOW - Optional link validation and refactoring tools

**Context**: Issue 04 implemented canonical link format with Bref. These are potential CLI/tooling enhancements.

### Link Validation CLI
- Pre-deployment validation: `noet-core validate --check-links ./docs/`
- Report broken links with file locations
- Suggest fixes for common issues
- Distinguish between "file moved" vs "file deleted"

### Link Refactoring Tools
- Automated link updates when moving files: `noet-core refactor --move src/old.md src/new.md`
- Update all references automatically
- Preview changes before applying

### Import from Other Systems
- Convert existing link formats from other tools
- `noet-core import --from obsidian ./vault/`
- `noet-core import --from roam ./export/`
- `noet-core import --from logseq ./graphs/`

## Should asset bids really be derived from their hash?

We could put this information into the asset node, that would trigger a node update, which
downstream consumers would be notified of. It would result in less document churn as well, because
we wouldn't need to regenerate reference "brefs" all over the place.

## BeliefBase Trait Abstraction for Zero-Copy Graph Operations

**Priority**: LOW - Code quality improvement

**Context**: `BeliefBase` has `states` (direct field) and `relations` (behind `Arc<RwLock<>>`). Currently, to call `BeliefGraph` methods on a `BeliefBase`, we clone via `From<&BeliefBase> for BeliefGraph`. This is wasteful for read-only operations like `find_orphaned_edges()`.

### Option 1: Direct Implementation (Current Workaround)
- Duplicate methods on both `BeliefBase` and `BeliefGraph`
- Simple but violates DRY principle
- Example: `find_orphaned_edges()` duplicated across both types

### Option 2: Trait-Based Abstraction (Recommended)
Define a trait that both types implement with default implementations:

```rust
pub trait HasBeliefData {
    fn get_states(&self) -> &BTreeMap<Bid, BeliefNode>;
    fn get_relations_graph(&self) -> impl Deref<Target = BidGraph>;
    
    // Default implementations for shared methods
    fn find_orphaned_edges(&self) -> Vec<Bid> { /* ... */ }
    fn is_empty(&self) -> bool { /* ... */ }
    fn build_balance_spec(&self) -> Option<QuerySpec> { /* ... */ }
    // etc.
}

impl HasBeliefData for BeliefGraph { /* ... */ }
impl HasBeliefData for BeliefBase { /* ... */ }
```

**Benefits**:
- Zero-copy access to graph operations from BeliefBase
- No code duplication for read-only graph methods
- Single source of truth for shared algorithms
- Can be used in generic contexts: `fn analyze<T: HasBeliefData>(data: &T)`

**Considerations**:
- Requires Rust 1.75+ for `impl Trait` in trait return position
- Trait methods slightly less discoverable than direct methods
- Need to import trait to use default methods

**Alternative Considered**: `BeliefGraphRef<'a>` wrapper type with borrowed data - rejected as more complex with limited benefit over trait approach.

**Related**: Used in `built_in_test()` to check for orphaned edges without cloning entire graph.

## `check_for_link_and_push` Bail-Out Refactor

**Priority**: LOW - Code quality improvement

**Context**: `src/codec/md.rs::check_for_link_and_push` has three separate code paths that emit an unmodified link and continue the event loop: "Can't parse", "path mismatch", and potentially future bail-out cases. All three duplicate the same ~15 lines of link-event reconstruction.

### Current Duplication
Each bail-out path manually reconstructs the original `Start(Link/Image)`, title events, and `End` event from `link_data`, then sets `link_type`, pushes to `events_out`, and calls `continue`. This is error-prone — a future change to link event structure must be applied in multiple places.

### Proposed Fix
Extract a helper:

```rust
fn emit_unchanged_link(
    link_data: LinkAccumulator,
    end_range: Option<Range<usize>>,
    events_out: &mut VecDeque<(MdEvent<'static>, Option<Range<usize>>)>,
)
```

All three bail-out paths call this helper, then `continue` the loop.

### Related
- `src/codec/md.rs` `check_for_link_and_push` — "Can't parse" branch (~L380) and "path mismatch" branch (~L480)
- Introduced during cross-platform path normalization fixes (session adding `strip_ext`/`drop_index_suffix`)

**Status**: Low risk, purely mechanical refactor. No behaviour change intended.

## Flattened Subnet Cache for `resolve_net_path` (from Issue 57)

**Priority**: LOW
**Context**: `DbConnection::resolve_net_path` currently resolves cross-network paths by
recursing one SQL hop per path segment (e.g. `"subnet/file.md"` → look up `"subnet"` under
`net`, then look up `"file.md"` under the returned sub-net BID). This is correct and
consistent by construction, but does O(depth) queries.

### Proposed Optimization

Maintain a flattened `subnets` table:

```sql
CREATE TABLE subnets (net TEXT, subnet_path TEXT, subnet_bid TEXT)
```

- `net`: the root network BID this row belongs to
- `subnet_path`: the full path from `net` to the sub-network (e.g. `"a/b"` for a net
  nested two levels deep)
- `subnet_bid`: the BID of the sub-network node at that path

**Read path**: `resolve_net_path(net, path)` does a single
`SELECT * FROM subnets WHERE net = ?`, processes all rows in Rust, finds the
longest `subnet_path` that is a prefix of `path`, then resolves the remainder
against `subnet_bid`. One SQL query regardless of nesting depth.

**Write path**: On a `NodeUpdate` event, if `node.kind.is_network()`, insert
flattened ancestry rows. For a new net `N` at path `p` under parent `P`:
  1. Insert `(net=P, subnet_path=p, subnet_bid=N)`.
  2. Find all existing rows where `subnet_bid = P` (i.e. `P` is itself a sub-net of
     some ancestor `A` at path `q`), and insert `(net=A, subnet_path=q/p, subnet_bid=N)`.
  This is a SELECT + batch INSERT, not recursive SQL.

On `NodeRemoved`, delete all rows where `subnet_bid = N` and all rows where
`subnet_path` starts with the removed path prefix (cascading descendants).

**Consistency**: updates should be in the same DB transaction as the path event
write, so there is no consistency window.

### When to Implement

Profile first. Typical repo subnet depth is 2-4 levels; the current recursive
approach does 2-4 queries and is unlikely to be a bottleneck. Implement this
only if `resolve_net_path` shows up in profiling for large repos with deep or
wide subnet hierarchies.

## Streaming Drain During Epoch Parse (from Issue 57)

**Priority**: LOW
**Context**: During parallel epoch compilation (`parse_epoch_parallel`), parse tasks and
the accumulator drain step run sequentially: all tasks complete, then `drain_epoch` is
called, then the next epoch starts. The drain step currently takes O(N_events ×
cost_per_event) and runs entirely after the parse JoinSet is awaited.

### Proposal

Clone the `BeliefBase` at `BatchStart` to produce a frozen snapshot. Parse tasks receive
`QueryHandle`s wrapping the frozen snapshot (epoch N-1 state). A dedicated drain task
consumes from `tx` and applies events to the live `inner` concurrently. At `drain_epoch`,
swap `live → frozen` for the next epoch.

This trades one `BeliefBase::clone()` per epoch for true parse/drain parallelism —
eliminating the sequential parse→drain→parse→drain cadence entirely.

### Design sketch

`AccInner<S>` gains a `frozen: S` field (cloned at `BatchStart`). `QueryHandle`s wrap
`Arc<frozen>` (read-only, no mutex contention). The drain task holds exclusive access to
`live inner` via the existing `Arc<Mutex<AccInner>>`. At `drain_epoch`: `frozen = live`,
clone a new `live` for next epoch (or swap with a fresh clone).

No `RwLock` refactor needed — drain and query operate on separate objects with no shared
lock.

### Cost model

`BeliefBase::clone()` at corpus scale: ~1008 PathMaps × ~50 entries, full node state map,
edge graph deep copy — estimated <100ms on a modern machine. Break-even against drain time
depends on post-P0 (PathMapMap routing fix) numbers.

### When to Implement

Profile after the PathMapMap reverse-index fix (P0 in Issue 57). If per-epoch drain drops
below ~200ms, the clone overhead makes streaming overlap a net loss except for pathological
large final batches. If drain remains dominant, implement as described above.

## `NodeKey` `Bref::default()` Sentinel Type-Safety

`NodeKey::Id { net: Bref::default(), .. }` and `NodeKey::Path { net: Bref::default(), .. }`
use the nil-bref as a sentinel meaning “net scope unknown — must be resolved via
`regularize_unchecked` before lookup.” This convention works but is invisible to the type
system and has caused two production bugs (speculative_path_key, Issue 57) where a call path
accidentally skipped regularization and passed the sentinel directly to `cache_fetch` /
`evaluate`, producing silent DB misses.

The in-memory `PathMapMap` silently normalizes `Bref::default()` → API root bref on all
lookups, masking violations. The DB `evaluate` pipeline passes the raw sentinel to SQL,
exposing them.

### Options

**Option A — `Option<Bref>` in NodeKey** (correct long-term fix): Replace `net: Bref` with
`net: Option<Bref>` in `NodeKey::Id` and `NodeKey::Path`. `None` = unresolved scope.
The `evaluate` pipeline can `expect`/error on `None`; `PathMapMap` methods require callers to
resolve before calling. Large mechanical change — every construction site and match arm is
affected.

**Option B — Debug-mode assertions in evaluator** (cheap near-term): Panic in debug builds
if `net == Bref::default()` reaches the `evaluate` pipeline's subject-resolution phase.
Surfaces future violations immediately without a large refactor. Doesn't help in-memory callers.

**Option C — Normalize in evaluator** (smallest change): Add the same `Bref::default()` →
API-bref guard to the `evaluate` pipeline that `PathMapMap` already has. Makes DB behavior
match in-memory. Perpetuates the hidden sentinel rather than eliminating it.

### Recommendation

Option B as an immediate safety net; Option A as the eventual correct fix when a large
mechanical refactor is otherwise warranted (e.g. a NodeKey redesign).

---

## MDN Corpus Macro-Benchmark Script (from Issue 47)

**Priority**: LOW — corpus now parses in ~60 s; ad-hoc runs are fast enough for human validation

**Context**: The full MDN JavaScript corpus (1,363 files) previously took ~19.7 hours; it now
parses in ~60 seconds wall time on a debug build. At this speed a thin automated timed script
is viable where a full Criterion macro-benchmark never was.

**Proposed**: A shell script (e.g. `benches/run_mdn_benchmark.sh`) that:
1. Runs `noet parse .bench_corpora/mdn-content/files/en-us/web/javascript` with `RUST_LOG=warn`
2. Captures wall time, file count, and warning counts
3. Appends a one-line JSON record to a local `benches/mdn_benchmark_history.jsonl` file
4. Prints a human-readable summary comparing to the previous run

This gives before/after comparisons for performance-sensitive changes without requiring CI
integration or Criterion infrastructure. Keep it local-only (add `benches/mdn_benchmark_history.jsonl`
to `.gitignore`).

**Not needed**: Full Criterion Tier 2/3 macro-benchmark infrastructure — the corpus runs are
fast enough that wall-clock comparison is sufficient signal.

## Memory Profiling (from Issue 47)

**Priority**: LOW — no evidence of memory problems at current scale

**Context**: Peak heap usage during a full MDN JS corpus parse has never been measured. At
1,363 files in 60 s the system is clearly not memory-bound, but a baseline would be useful
before targeting GB-scale corpora.

**Proposed**: Use `heaptrack` (Linux) or `Instruments` (macOS) on a single full corpus run;
record peak RSS and allocation hotspots. No code changes needed — purely a profiling exercise.
Document results in `docs/design/` or as a note in `BACKLOG.md`.

**When**: Only relevant if targeting corpora significantly larger than MDN JS (~50 MB source).

## `rari` Cross-Project Benchmark Comparison (from Issue 47)

**Priority**: LOW — informational only

**Context**: `github.com/mdn/rari` is the Rust-based MDN build tool (replaced Yari's Node.js
pipeline in 2024). It processes the same MDN corpus used for benchmarking. Rari uses flat macro
resolution — no belief graph, no multi-pass convergence — so it represents a lower bound on
what a single-pass Rust renderer can achieve on identical input.

Wall-clock time is now in the seconds range (the original deferral condition was "minutes"),
so this comparison is now a valid target.

**Proposed**: Clone `rari`, run it against `.bench_corpora/mdn-content`, record wall time and
peak memory, document the delta. The gap quantifies the cost of noet-core's belief graph model
vs. a simpler rendering pipeline.

## `typeof` Path-Mismatch Warning (from Issue 47)

**Priority**: LOW — correctness gap, no user-visible breakage confirmed

**Context**: Run 12 (2025-07-10) shows 546 `check_for_link_and_push Path mismatch` warnings,
all from a single file (`reference/operators/typeof`). The mismatch is:
- proto abs path: `.../reference/operators/typeof`
- ctx repo-relative path: `reference/expressions-and-operators/typeof/index.md`

This is a **different** variant from the original `symbol.*` ID-collision bug (which is fixed).
This one is caused by an MDN directory reorganization where `operators/` was renamed to
`expressions-and-operators/` — the proto index picks up the old path, the ctx has the new one.
Links from `typeof` to cross-corpus targets are silently left unchanged.

**Investigation needed**: Confirm whether this fires only on MDN (due to the rename) or also
on other corpora. If MDN-specific, low priority. If it can fire on any corpus with directory
renames, it warrants a dedicated issue.

**Related**: `check_for_link_and_push` bail-out refactor (already in BACKLOG) would make the
warning sites easier to audit.

## External URL Content-Hashing (from Issue 30)

External URL tracking as first-class `BeliefNode`s is functionally complete in the sense
that URLs are recorded and traversable in the belief graph. The remaining unimplemented
piece — fetching URL content, computing a SHA256 of the response body, and using that hash
as the BID input — was intentionally deferred. It is not planned in the near term.

**Proposed design when revisited:**
- Opt-in per-network via config (e.g. `fetch_external_urls = true` in a network's
  `index.md` frontmatter), not a global CLI flag. Global opt-in is too blunt for mixed
  corpora (some networks reference internal-only URLs that should never be fetched).
- `reqwest` (async, pure Rust) for HTTP GET; SHA256 of response *text* (not headers/bytes)
  as the BID seed in `UUID_NAMESPACE_HREF`.
- `.url-manifest.toml` written as a peer file to each BeliefNode metadata file, recording
  URL, content hash, status code, `fetched_at`, and `last_modified` from HTTP headers.
- Broken links (404, timeout) tracked as nodes with error status rather than failing
  compilation.
- Default behavior: **no network requests** without explicit opt-in. GDPR / privacy
  concern is explicit in the original design.

**References:** `properties.rs:href_namespace()`, `builder.rs:push_relation()`,
`md.rs:LinkAccumulator`. Full design preserved in
`docs/project/completed/ISSUE_30_EXTERNAL_URL_TRACKING.md`.

## SPA Viewer Performance — Remaining Items

**Priority**: LOW  
**Discovered during**: 2025-04-30 session on a large systems-engineering corpus

First round of profiling and fixes completed 2025-04-30.  Chrome
Performance traces on that corpus (~32k nodes, 50 networks, 47 MB shards)
identified and resolved the main bottlenecks:

**Completed:**
- ✅ Deferred search index loading (~20 MB) to after first paint
- ✅ Early page HTML render (prefetch + render before WASM init)
- ✅ Metadata panel "Loading…" placeholder during WASM init
- ✅ Target-network shard resolution from URL hash (load the user's
  destination network first, entry network in background)

**Remaining (~853ms WASM microtask is the dominant cost):**

- **WASM deserialization cost (~850ms)**: The dominant remaining cost is
  a single synchronous WASM call (`load_shard` → msgpack deserialize →
  PathMap rebuild).  Tested `setTimeout(0)` between `load_shard` and
  `get_nav_tree` — no perceptible improvement because the early page
  prefetch already paints content, and the user is waiting for
  scroll-to-anchor + metadata which require the full `loadDocument`
  flow after WASM init.  Real gains require WASM-side changes:
  streaming deserialization, incremental PathMap construction, or
  deferred PathMap build (build on first query, not on load).
- **Shard-load transitions**: Clicking into a new network triggers a
  shard fetch + merge.  Is the merge O(N) in total graph size or
  O(shard)?  Does the nav-tree rebuild unnecessarily re-render the
  entire tree?
- **DOM weight**: Does the traceability table or nav tree emit excessive
  DOM nodes for large networks?  Virtualisation (only rendering visible
  rows) would help.

## Codec API Footguns (from downstream codec implementations)

Discovered while implementing custom `DocCodec` / `WalkCodec` extensions in
downstream application shims. Each item represents a gap in documentation,
default behavior, or diagnostics that tripped up codec authors.

### Child IRNodes must set `proto.path` to the parent document path

**Severity**: panic at runtime (silent during unit tests, crashes on full compile)

Child nodes (heading > document root heading) returned from `codec.nodes()` must
have their `path` field set to the same value as the parent document's path.
Without this, `GraphBuilder::get_parent_from_stack()` cannot match the child to
its parent via `proto_filepath == stack_filepath`, and the node never gets a
PathMap entry. This causes a panic in Phase 4 (`inject_context`) with the message
`"Set should be balanced here: in_pathmap=false"`.

`MdCodec` sections work because the markdown parser sets `proto.path` on all
section IRNodes during parsing. Custom codecs must do this manually.

**Suggestion**: `GraphBuilder::push()` could default child node paths to the
parent's path when `proto.path` is empty and `proto.heading > 1`. Or the
`DocCodec` trait docs could explicitly state this requirement.

### Child IRNodes should set explicit `document["id"]` for uniqueness

**Severity**: silent incorrect behavior (slug collisions)

The builder derives node identity from `IRNode::id()`, which falls back to
`to_anchor(title)` when no explicit `id` is set. If multiple children have the
same title (e.g. two structs with a `count` field, or two subsystems with an
`io.x` port), their slugs collide.

The fix for codec authors is to set `document["id"]` explicitly with a qualified
name (e.g. `"parent_name.child_name"`).

**Suggestion**: Document this in the `DocCodec` trait. Consider a builder
diagnostic that warns when multiple sibling nodes produce the same slug.

### Top-level symbols in multi-declaration documents must be heading=3, not heading=2

**Severity**: panic at runtime

A document (heading=2) that produces multiple top-level symbols (e.g. a header
file with two class declarations) cannot use heading=2 for the symbols — they'd
be siblings of the document node in the stack, not children. The symbols must
be heading=3 (children of the heading=2 document root).

This is non-obvious because for a single-declaration file, heading=2 works fine
— the symbol IS the document. The crash only manifests with multi-declaration
files.

**Suggestion**: Document the heading hierarchy contract: heading=1 is network,
heading=2 is document, heading≥3 is document-internal structure.

### `CLAIM_MAP.reject()` on a directory does not affect files inside it

**Severity**: confusion, not a crash

Rejecting a directory via `CLAIM_MAP.reject(dir)` prevents the directory from
being parsed as a sub-network, but files inside the directory are still
individually tracked and parsed if they match `WALK_CODECS.should_track()` or
`CODECS`. This is correct behavior but non-obvious — one might expect rejecting
a directory to transitively reject its contents.

**Suggestion**: Document this in the `CLAIM_MAP` API docs.

## Codec API Ergonomic Improvements (from downstream codec implementations)

### Boilerplate in `DocCodec` implementations

Every custom codec requires ~40 lines of identical boilerplate for
`inject_context`, `finalize`, `generate_source` — all returning `None` / empty.
A `SimpleDocCodec` trait with default implementations (or a derive macro) would
reduce noise.

### `CodecFactory` is `fn()` not `Fn` — cannot capture state

The factory type is a bare function pointer, not a closure. This means you can't
capture state in a factory. For codecs with variants (e.g. a YAML codec that
handles multiple schema kinds), the workaround is separate factory functions
per variant. A `Box<dyn Fn>` factory would be more flexible.

### No codec-level diagnostic for "file parsed but produced no BeliefBase impact"

When a file is tracked by `WALK_CODECS` and appears in `ProtoIndex` children
but is never claimed by any codec's `parse()`, it silently becomes an
`UnclaimedDataCodec` node. A codec-level diagnostic ("file X is walk-visible but
no network claimed it") would help during development.

This already exists as `ParseDiagnostic::info` in `parse_one_path` branch 3, but
it's not surfaced prominently during development iterations.

### ~~HTML pages from custom codecs get nil BID (`00000000-...`)~~ ✅ Resolved

Custom codecs must implement `inject_context` and call
`proto.update_from_context(ctx)` to write the push()-assigned BID back into
`proto.document["bid"]`. Codecs that return `Ok(None)` from `inject_context`
skip this write-back, producing nil BIDs in generated HTML.

Fixed in a downstream C++ codec crate by implementing `inject_context`
properly in all 6 codecs. The `DocCodec` trait docs should state this
requirement explicitly (see footgun #1 above re: documenting codec contracts).

### Symlinks need repo-relative path resolution for cross-network edges

**Severity**: broken cross-network edges

Filesystem symlinks used as cross-references resolve via `std::fs::canonicalize`
to absolute paths. The `NodeKey::Path` emitted in `prepare_proto_relations`
carries this absolute path. `regularize_unchecked` should strip the repo root to
get a repo-relative path, but this may fail if the canonicalized path doesn't
start with the expected repo root prefix (e.g. macOS `/private/var` vs `/var`
symlink divergence).

Needs investigation: trace the `regularize_unchecked` call for symlink-based
edges and verify the path matches a registered node.

## DbConnection Query Evaluation Optimizations (from Issue 83)

The SQL-native `DbConnection::evaluate` pipeline (Issue 83 Phase 6b)
issues one SQL query per traversal hop. Several optimizations can
reduce the query count. See `docs/design/beliefbase_architecture.md`
§3.8 "Optimization Opportunities" for full details and example SQL.

**Priority: LOW** — correctness is established; optimize when profiling
shows DB query latency is a bottleneck (likely when watch-service
queries move to the `evaluate` pipeline).

- **Recursive CTE for unbounded traversals**: Collapse `s-section-k {max}`
  (balance_map) from N round-trips to one `WITH RECURSIVE` query.
  Applies to any fixed-kind unbounded traversal. Highest-impact single
  optimization.

- **Batched halo query**: The halo (`sko-[*]-sko {1}`) issues up to 3
  queries (one per input role). Combine into a single `OR`-based query.

- **Path-table acceleration for Section traversals**: When the QuerySpec
  shape matches (subject within a known network, Section-only traversal),
  rewrite as a `paths` table prefix scan — O(1) index lookup vs O(depth)
  iterative edge walking.

- **States cache across evaluate call**: `apply_filter_sql` fetches states
  per filter step; the final bulk materialization re-fetches the same
  states. A shared `BTreeMap<Bid, BeliefNode>` cache eliminates redundant
  fetches.

- **Temp table for large frontier sets**: When frontier exceeds hundreds
  of BIDs, `CREATE TEMP TABLE frontier(bid TEXT)` + `JOIN` is more
  efficient than large `IN (...)` clauses.

- **BeliefBase in-memory evaluation.** The in-memory `evaluate`
  pipeline has its own optimization surface (`apply_traversal`,
  graph materialization, index lookups). Profile against large corpora
  (30k+ nodes) to identify hot paths. See §3.8 in
  `beliefbase_architecture.md`.

## Notes

- Items are extracted from completed issues in `docs/project/completed/`
- All items are optional enhancements, not blocking any current work
- Priority levels: HIGH (blocking), MEDIUM (useful), LOW (nice-to-have)
- Most completed issues had unchecked boxes that were implementation notes, not incomplete work
- This backlog can be revisited when planning future releases (v0.2, v1.0, etc.)
