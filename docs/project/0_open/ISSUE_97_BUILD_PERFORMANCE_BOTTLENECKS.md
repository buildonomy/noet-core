# Issue 97: Corpus Build Performance — Observed Bottlenecks

**Priority**: MEDIUM
**Estimated Effort**: ongoing (RELATIVE COMPARISON ONLY)
**Dependencies**: None. Related to planning Issue 26 (pandoc markdown quality),
which surfaced the original `terminate_stack` symptom.

> [!NOTE]
> **The 300-line issue limit does not apply here.** This issue is a running
> register of observed build-performance bottlenecks and their status, kept in
> one place so that measurements taken across different sessions and code
> versions can be compared rather than re-derived. Individual bottlenecks graduate
> to their own issues once they are understood well enough to be actionable;
> this document records what is known, what has been ruled out, and what is
> still unexplained.
>
> Wall-clock figures belong in application-specific `PERFORMANCE_LOG.md` files in
> order to maintain application <-> tooling IP boundaries. This
> issue records *mechanisms and status*; the log records *runs*.
>
> That boundary also governs *how* findings are written here. Per AGENTS.md
> §"Application-Neutral Content", describe corpora and files by their
> structural properties — "a ~7,600-line document re-exported under 13
> parents", "deeply-included C++ headers" — never by customer, program, or
> repository name. The mechanism is what transfers between corpora; the
> proper noun is what leaks.

## Current status

The original subject of this issue — sequential `terminate_stack` taking ~70%
of wall clock — is resolved, most likely by `6c313d5` (multi-threaded runtime).
The issue was then broadened into a register of build-performance bottlenecks,
because the profile shifted: **parse is no longer the dominant cost.**

On a full-corpus run, time outside the parse path accounted for roughly 4.5 of
4.7 hours. Optimising inside the parse path therefore caps out near 15% of wall
clock, and "outside the parse path" turned out to be at least three distinct
mechanisms with different fixes rather than one.

Progress on the same corpus, in commit order:

| | wall clock | parse phase | seeding (summed) |
|---|---:|---:|---:|
| `6bbf5c3` (baseline) | 28m19s | 20m38s | 2,533s |
| `4452085` (shared epoch base) | 19m22s | 11m32s | 524s |
| + `indexed_path` fallback removal | ~8m | ~5m | 126.9s |

The third row is not a layout-only effect: the `indexed_path` fallback is also
called during parsing (relation resolution, `beliefbase/context.rs`), so
removing it sped up the parse phase independently of `finalize_html`.

**Open**: Bottlenecks 4 (end-of-run insert storm), 5 (warm-cache `--db`
regression), and 9 (per-heading reparse-seed miss). Neither 4 nor 5 touches the
parse or `finalize_html` paths this work exercised.

Bottlenecks 8 and 9 arrived from a correctness investigation rather than a
performance measurement, and are recorded here because a silently duplicated
subtree inflates every later O(graph size) stage.

## Bottleneck register

| # | Bottleneck | Magnitude | Status |
|---|---|---|---|
| 1 | Sequential `terminate_stack` stalls | was ~70% of wall clock | **Likely resolved** — needs confirmation |
| 2 | `compute_layout_metadata` | **290s → 43.0s (6.7x)** | **Resolved** — see entry; stage is now 26.4% of `finalize_html`, no single dominant term |
| 3 | Reparse-budget exhaustion on unresolvable codec-namespace references | full corpus **649 → 73** truncations and **6,170 → 4,980** warnings; **zero** `.h`/`.cpp` remain (was ~460); C++ subtree wall clock −24% at `--jobs 1` | **Resolved** — an unconditional retry that also suppressed the permanently-unresolved latch, so a reference nothing declares re-queued until the budget truncated it. Fixed by the `parse_count <= 1` guard, plus external-reference demotion and four CMake target-recognition gaps. 23 residual truncations are a distinct `Id`-keyed defect |
| 4 | End-of-run insert storm | 962K log lines in 13 min | **Open** — mechanism unclear |
| 5 | Warm-cache (`--db`) regression | 94s → 28m on one subtree | **Open** — `--db` only, not on `render` path |
| 6 | PathMap read-path scan | 422M entries scanned/run | **Resolved** — `PathMap::path_map` index |
| 7 | Pre-spawn epoch seeding collapses to serial | seeding 2,533s → 126.9s (summed, concurrent); wall clock 28m19s → 19m22s | **Resolved** — epoch fragmentation fixed (1,949 → 14 epochs); per-task `PathMapMap::new` rebuild fixed (`4f352e2`); redundant per-task rebuild eliminated via a shared epoch base (`4452085`) |
| 8 | `parse_epoch` ancestor-BID seed miss on reparse | 70 → **0** misses / ~72,600 parses | **Resolved** — an unguarded **directory symlink** re-parented the networks it pointed at, inverting depth-group order so `sync_subnet_stubs` skipped a subtree and left no `PathMap` entry for the owning network. Guarded in `net_dir_partition`; `fallback_queue` added to contain the class |
| 9 | Reparse-seed miss duplicates section BIDs under `--jobs > 1` | 389 `PathMap::new` collisions on a representative subtree (`--jobs 4`, 3 runs); `0` at `--jobs 1` (3 runs) | **Open** — a document's `net_bid`/`doc_bid` resolve correctly but its per-heading `cache_fetch` still misses, minting fresh section BIDs. Disjoint from Bottleneck 8 (measured on a run producing both) |

---

## Bottleneck 1 — Sequential `terminate_stack` (likely resolved)

### Mechanism

Under `jobs=1`, `GraphBuilder::terminate_stack` (Phase 5) consumed ~70% of wall
clock on a large application corpus (~3,700 file records, 92.3 min): summed
Phase-5-to-next-Phase-0 gap of 3,862.0s across 3,479 files, with Phase 0 itself
negligible. The worst single file — a ~7,600-line slide-deck export — took
1,172.73s for one `Diff events (2271): NodeUpdate(455), RelationUpdate(908),
PathsAdded(908)` batch. A cluster of ~13 near-duplicate copies of that document
(the same export re-emitted once per parent node linking it) each showed ~908
`RelationUpdate`s and 20–30s gaps.

The same corpus under `--jobs 4` completed **every** measured `terminate_stack`
in under a second: that same export in **0.12s** with an identical
908-`RelationUpdate` diff, max observed 0.22s, 8.9s summed across 2,556
completions.

### Cause and fix

`6c313d5` changed `noet parse` from `Builder::new_current_thread()` to a
multi-threaded runtime. `parse_content`'s Phase 2 is CPU-bound with no yield
points, so under a single-threaded runtime one task starves every other task —
including tasks that only need to poll an already-resolvable future. The
sequential path was not doing more work; it was waiting on a runtime that could
not schedule the completion. That is what accounts for a gap of four orders of
magnitude, which four workers alone cannot explain.

### Durable findings

**The symptom pointed at the channel; the cause was the runtime underneath it.**
The leading hypothesis was a receiver-side bottleneck, and it was structurally
plausible: `tx` is an `UnboundedSender` to a single `BeliefAccumulator`, and
when `jobs == 1` `parse_one_path` runs inline in the compiler's own async
context using the compiler's own `builder`, with a direct `tx` send and no task
spawn or semaphore (`compiler.rs:1758-1769`). A lock-contended receiver would
therefore serialise straight into `terminate_stack`'s critical path. It was
wrong. When a cost appears only on the inline-dispatch path, check runtime
scheduling before auditing the data structures that path touches.

**The naive phase-gap metric is invalid for parallel runs.**
`parse_log.py --phase-summary` diffs consecutive log records in file order,
which interleaves unrelated concurrent tasks once `jobs > 1`. Use a task-scoped
measure instead: per `task_idx`, the interval from that task's own "Phase 5:
terminating stack" line to its own "Diff events" line.

**The pathology does not reproduce at small scale.** A 30-file subtree at
`jobs=1` completes every Phase 5 in well under a millisecond (51–184 µs
observed). Any confirmation run needs a corpus large enough to have reproduced
the original stall.

### To close

- [ ] Re-run the full application corpus with `jobs=1` at current HEAD
- [ ] Confirm the worst-case slide-deck export completes Phase 5 in well
      under a second (was 1,172.73s)
- [ ] Confirm no regression in event-ordering correctness tests; `codec_test`
      must pass unchanged
- [ ] Add a regression assertion that a single-document `terminate_stack` call
      with a large synthetic diff (hundreds of `RelationUpdate`s) completes
      within a fixed time budget, to catch a reintroduction of this pathology

---

## Bottleneck 2 — `compute_layout_metadata` (single largest serial stage)

`compute_layout_metadata` (`src/layout.rs`) computes the 3D credibility-map
force simulation after all parsing completes. It is single-threaded and nothing
overlaps it, so it is pure critical path. At 290s it was ~4x the next-largest
`finalize_html` stage and 8.6% of build wall clock, stable across two runs.

### Root cause: an exhaustive `PathMapMap::path()` fallback that resolved nothing

The cost was never the `O(200 × n²)` force simulation. It was scope
resolution — `PathMapMap::indexed_path` (BID→path), the same `O(N_networks)`
read-path family as Bottleneck 6. Per-route counters split calls from probes:

| Route | Calls | Probes | Probes/call | Time | Share |
|---|---:|---:|---:|---:|---:|
| indexed | 233,542 | 1,561,651 | 6.7 | 34.7s | 16.7% |
| **fallback** | **58,922** | **66,640,782** | **1,131.0** | **175.3s** | **84.6%** |

The fallback was 20.1% of calls but 97.7% of probes — and it found nothing. The
arithmetic reconciles exactly: 135,590 nodes − 58,922 fallback calls = 76,668
resolvable; minus 3,918 reserved-namespace nodes = 72,750 = the logged
`mapped_nodes`. All 66.6M probes returned `None`. 175.3s of confirming absence.

### The fixes

**1. Delete the fallback** (`src/layout.rs`, `src/paths/pathmap.rs`).
`node_to_nets` holds an entry for every BID in every `PathMap`'s `bid_map`, and
`PathMap::path` resolves only BIDs reachable through some `bid_map` — so an
index miss *is* proof of absence, and the scan only rediscovers that at
`O(N_networks)` cost. `indexed_path` now returns `None` directly on a miss.

That inference is load-bearing, so it is asserted rather than argued:
`test_node_to_nets_miss_implies_no_path` checks, for every BID in the fixture,
that an index miss implies the exhaustive scan also finds nothing. The counter
test asserts a miss records **zero** probes, which is what catches a
reintroduced scan. `scan_indexed_path` is retained `#[cfg(test)]` as the
ground-truth oracle the narrowing tests compare against.

**2. Narrow the indexed route to subnet ancestors.** `node_to_nets` records
*direct* containment, while `PathMap::path` also resolves a BID held by a
*subnet* by recursing into it — so narrowing to direct hits alone silently
regresses those nodes to `None`. Probing subnet ancestors preserves that.
Equivalence against the exhaustive scan is asserted for every BID in the graph
plus an absent one (`test_indexed_path_narrowing_matches_full_scan`),
mutation-checked so that dropping the subnet term makes it fail, with the
fixture asserted to contain a subnet so the recursion case cannot silently stop
being covered.

The subnet-holder set is a property of the map, not of the BID, so it is
memoized; invalidation hangs off `make_pathmap_unique`, the pre-write
chokepoint a new write site cannot bypass. `PathMapMap`'s derived `Clone` was
replaced with a hand-written one so a clone starts cold rather than inheriting
a cache that may not describe it.
`test_indexed_path_narrowing_survives_subnet_mutation` warms the cache, gives a
network a new subnet, and re-asserts scan-equivalence, with vacuity guards
asserting the parent genuinely gained a subnet it lacked when warmed — without
those the test passed even with invalidation disabled.

**3. Exclude reserved namespaces from layout scope.** `compute_layout_metadata`
takes a `LayoutConfig { enabled, max_nodes }` and resolves its network scope up
front via `resolve_scope`:

- Reserved namespaces (`Bid::is_reserved()`, covering all four
  `const_namespaces()`) are excluded. This is a **correctness** fix as much as a
  performance one: those nodes are synthetic `External|Trace` bookkeeping and
  carry none of the N/S/P assumptions the layout scoring is built on (see
  `docs/essays/engineering_model_ontology.md` §3). Computing viewer coordinates
  for them was a category error.
- Networks above `max_nodes` are skipped with a warning naming the flag, so an
  oversized network degrades loudly instead of silently stalling the build.

Exclusion is applied once, in scope resolution, so excluded networks drop out of
*every* downstream step rather than being computed and discarded. The two
exclusions differ in kind: the reserved-namespace rule is permanent and
semantic — synthetic `External|Trace` bookkeeping nodes carry none of the N/S/P
assumptions layout scoring is built on (see
`docs/essays/engineering_model_ontology.md` §3), so computing viewer
coordinates for them was a category error. `max_nodes` is a pragmatic guard
against the `O(n²)` term and can be raised when a large network genuinely needs
layout; networks above it are skipped with a warning naming the flag
(`--layout-max-nodes` / `NOET_LAYOUT_MAX_NODES`, default 5,000), so an oversized
network degrades loudly instead of silently stalling the build. `--no-layout`
skips the stage entirely; layout remains on by default.

**Consumer impact**: `render_position`, `structural_weight` and
`structural_depth` are now legitimately absent for some networks. Issue 85's
viewer must treat all four layout fields as optional and fall back gracefully.

### Measured outcome

**290.0s → 43.0s (6.7x, 247s saved).** Scope resolution 255.5s → 32.3s; fallback
probes 66,640,782 → 0. The stage is no longer dominated by one term — scope
resolution 32.3s (75%), force simulation 10.6s (25%) — and has gone from 62.9%
of `finalize_html` to 26.4%, leaving `create_asset_hardlinks` (72.5s) as the
largest stage in it.

The CoW sentinel is unchanged at 4.17% (baseline ~2.8%), confirming the
`read_arc()` calls added by the subnet-ancestor index did not provoke spurious
copies — worth checking because any read guard held across a write inflates
`Arc::strong_count`.

### Durable findings

**The force simulation was never the problem — and a cost model calibrated on
the wrong corpus said otherwise, confidently.** An `O(200 × n²)` analysis run
against a shard manifest attributed 97.41% of stage cost to the synthetic
href-tracking namespace (`href_namespace()`, `properties.rs::const_namespaces`),
at 78,479 nodes, and predicted a ~39x win from excluding it. On the corpus
actually measured that namespace held **3,676** nodes — a 456x smaller term —
and the force simulation was 3.8% of the stage. Total node counts differed too
(264,757 vs 135,590), which should have been the tell. The arithmetic was
sound; the inputs described a different corpus. Calibrate cost models against
the corpus you will measure on.

**Parallelising the per-network loop is not the fix.** Makespan is bounded
below by the single largest task, so with a dominant network in scope, spawning
cannot beat ~1.03x speedup at *any* worker count. Once the dominant term is
removed the residual force-sim work is ~10s serial, and parallelising it is
worth ~1s. Reserved-namespace exclusion had to come first; the other order buys
3%.

**Scope selection must not add a second `PathMapMap::path()` pass.** Splitting
network selection from home-network mapping made each node pay `path()` twice
(~1.5e8 probes → ~3.1e8 at this corpus size) and regressed the stage 290s →
539s, all of it *before any layout work began*. Selection and mapping are fused
in `resolve_scope` for this reason.

**Cheap attribution beats reasoning about which term dominates.** Three
successive optimisations chosen by inspection moved the stage by ~0, +2%, and
1.25x. Adding one counter that split *calls* from *probes* found the real 84.6%
on its first run and yielded 5.1x. The calls-vs-probes distinction was what
mattered, because the expensive route was the rarer one — a per-call average
would have hidden it. Instrument before optimising.

**`run_intra_bubble_layout`'s per-network full edge scan is a latent
`O(networks × E)` cost** (1,164 × 499,634 ≈ 5.8e8 iterations, two
`BTreeMap<Bid, usize>` lookups each), masked today by other terms. If the force
simulation matters again, bucket edges by home network in one pre-pass.

### Follow-on

Removing the fallback speeds up `indexed_path` for every caller, not just
layout — MCP tools, `beliefbase/context.rs`, and relation resolution all use it,
and 97,952 of the 233,542 indexed calls in this run came from outside layout.
This is also why the fix shortened the parse phase, not just `finalize_html`.

Bottleneck 6 concerns the same read path and is confirmed unaffected: it is
about `PathMap::indexed_get`'s path→BID lookup, not `PathMapMap::path`'s
BID→path lookup this fix touches — different index, same family of defect.

Remaining, if the stage matters again: parallelise the per-network loop
(~10s → ~1s).

## Bottleneck 3 — reparse-budget exhaustion on unresolvable codec-namespace references (resolved)

### Symptom

A full-corpus run truncated 649 files with `ReparseLimitExceeded` (510 at
`--jobs 1`, a perfect subset), ~90% of them C++ headers and sources under one
subtree. Separately, 211 silent parse gaps >20s totalling ~163 min clustered on
the same population. Both were this one defect.

To recognise it elsewhere: a file re-queues on every pass and then truncates,
and **raising `max_reparse_count` does not reduce the count**. Measured 599
truncations on the C++ subtree at limits of 2, 3, 4, 6, and 10, with parses
rising by exactly 599 per increment — a flat plateau means the work is not
converging, so budget is not the constraint.

### Cause and fix

`process_unresolved_reference` returned `true` unconditionally for any
codec-namespace reference. That single value served two purposes: it requested
a re-queue, *and* it withheld the key from `permanently_unresolved`, because the
caller only latches keys for references returning `false`. A reference no
document declares — a third-party header, say — therefore missed identically on
every pass, re-queued every pass, and latched never.

Fixed by returning `parse_count <= 1`, matching the guard the adjacent
synthetic-path case already used. This *adds* a terminating condition rather
than exempting anything from one, so the remainder loop drains strictly sooner.

Two follow-on fixes made the remaining references correct rather than merely
quiet:

- **Demotion to external references.** A codec may now declare that absent keys
  in its namespace denote out-of-corpus targets, so the citation resolves to an
  `href_namespace` node instead of reporting a broken link. Specified in
  [`codec_namespaces.md`](../../design/codecs/codec_namespaces.md) §4.
- **CMake target recognition.** Three parser gaps each silently removed whole
  components from the graph, so their headers registered no include alias and
  every `#include` of them missed permanently: `add_library(foo)` without a type
  keyword, `add_library(foo\n src/...)` with inline sources, and
  `target_sources(... FILE_SET HEADERS BASE_DIRS <dir>)` as the sole declaration
  of an include root. A fourth gap made `include/` inference conditional on the
  file declaring no other include dir, so a sibling *test* target's dir
  suppressed it.

### Measured

| | before | after |
|---|---:|---:|
| truncations, full corpus | 649 | **73** |
| truncations, C++ subtree `--jobs 1` | 483 | **23** |
| `.h`/`.cpp` truncating | ~460 | **0** |
| warnings, full corpus | 6,170 | **4,980** |
| C++ subtree wall clock, `--jobs 1` | 123.5s | **93.5s** |

Rendered output is unchanged: all pages identical after normalising run-to-run
BIDs, and every internal include link resolves to a page that exists.

Of 285 externally-demoted keys, **1** names a file present in the corpus — a
header that exists under that name only in a build directory, created by
`file(COPY_FILE)` during the build. Not resolvable from source; demoting it is
correct.

### Residual and follow-on

The 23 remaining truncations are 21 `.md`, one `.xlsx`, and one directory — no
C++. Identical at `--jobs 1` and `--jobs 8` and flat under a raised budget, so
the same shape but a different cause: their unresolved references are `Id`-keyed
cross-network anchor references, and the `Id`-keyed re-queue branch in
`process_one_parse_result` has the same missing-latch shape.

The demoted set is undifferentiated — a vendored library and a mis-resolved
internal path look alike. Classifying it against a package manifest would answer
a dependency-inventory question and make the set self-auditing. Tracked externally.

- [ ] Latch `Id`-keyed cross-network anchor references that miss on reparse

### Regressions

`test_codec_namespace_ref_stops_requeuing_after_first_parse`,
`codec_namespace_external_demotion_requires_reparse_and_opt_in`,
`test_external_include_does_not_render_an_internal_page_link`, and in the C++
codec four `CmakeInfo::parse` cases covering the target-recognition gaps. All
verified to fail against the unfixed code.


## Bottleneck 4 — End-of-run insert storm

962K log lines in a 13-minute window (minutes 263–276): 373,860 inserts into
the root network and 304,459 into `href_namespace`. `shift_total` is
near-zero (431 on href), so these are tail-appends, not the O(n) shift
pathology — it is raw volume.

The unexplained part is *why these inserts happen at all* this late. Two
readings:

1. Genuinely new registrations during export — meaning something re-registers
   nodes that were already registered during parse.
2. Re-inserts of existing entries into a local map that does not know about
   them.

The evidence leans toward (2), and toward a shared mechanism with Bottleneck
5: during this burst `href_namespace` shows `max_len` 3,465, while the
same namespace reaches ~78k entries over a full build. **A namespace holding
3,465 entries while absorbing 304K inserts is a local map far smaller than
its true membership** — the same signature as the warm-cache regression below,
reached by a different route.

**Update:** the `cache_fetch` census probe (added for Bottleneck 5, see
below) confirms this signature directly on a full-corpus run: at
`source=GlobalCache` outcomes, `session_bb_asset_len`/`session_bb_href_len`
plateau (median 485 / 66,831) while `session_bb_nodes` ranges up to 127,112
— the local const-namespace maps are a small, capped fraction of overall
session growth. This does not yet explain the *insert storm's timing*
(why so many corrections land in one late window), only that the underlying
local-vs-authoritative divergence this bottleneck describes is real and
measurable, not just inferred.

- [ ] Determine whether these inserts are new registrations or re-inserts
- [ ] If re-inserts, check whether the receiving map is a fresh/partial
      instance rather than the accumulated one

## Bottleneck 5 — Warm-cache (`--db`) regression

Parsing the same corpus subtree twice with `--db`, unchanged inputs:

| run | wall |
|-----|-----:|
| cold (empty DB) | 94s |
| warm (populated DB) | **28m 9s** |

In the warm run the asset PathMap holds **70** entries in-session while the
DB holds 17,706, so 63,175 of 63,243 asset lookups (99.9%) miss locally. Log
timestamps show the time spread evenly rather than in stalls, consistent with
per-lookup fallback to `global_bb`/DB. Note the direction: the warm run scans
*fewer* entries (1.5M vs 359M) yet takes 18x longer.

**Scope caveat — this does not affect `just render` today.** That recipe
passes neither `--db` nor `NOET_DB`, so it uses `db_init_memory()` and every
render is cold by construction. The regression reaches watch mode and manual
`--db` profiling runs only. It is recorded here because the mechanism
(local PathMap diverging from the authoritative store) is likely shared with
Bottleneck 4, which *does* affect the render path.

Mechanism inferred from timing distribution and the local/DB size gap;
**now instrumented** via `[cache_fetch] census`/`global_cache_miss_local`
(`noet_core::codec::cache_fetch_census`, debug) — see
`benches/log_analysis/analyze_cache_fetch.py`. On the full-corpus run used
to quantify Bottleneck 7, `GlobalCache` outcomes were only 0.9% of all
`cache_fetch` calls (4,250 / 456,064) with a mean cost of 10.2ms and p95 of
46.2ms — that run used the in-memory `DbConnection` path (not a genuinely
warm on-disk `--db`), so it does not reproduce the 94s→28m regression, but
it does confirm the local/authoritative size gap exists in the same shape
(see Bottleneck 4 update above).

- [x] Add a DB-query-count probe (serves Bottleneck 4 as well)
- [ ] Confirm whether the per-lookup fallback is the actual cost — needs a
      genuinely warm on-disk `--db` run with the new probe enabled, since
      the run captured so far used the in-memory DB path

## Bottleneck 6 — PathMap read-path scan (resolved)

`PathMap::indexed_get` linear-scanned its map, burning 422M string
comparisons per corpus run (85% of it the asset namespace). Fixed by adding
a path index (`PathMap::path_map`, `fe7e8ab`): 422,582,888 → 76,433 entries
scanned, 99.98% reduction.

Recorded here for completeness and as a caution: **wall clock did not move.**
A sequential `Vec` scan is memory-bandwidth-cheap, so 422M comparisons were
not the binding constraint. The change removes an O(namespace size) term that
would grow with the corpus, but it is an example of a headline metric
improving four orders of magnitude with no user-visible effect.

> [!NOTE]
> "Resolved" here means *the scan was indexed*, not *the read path is fast*.
> The sibling lookup on the same structure — `PathMapMap::indexed_path`
> (BID→path, versus this one's path→BID) — was still costing 175s per build
> two months later, via an exhaustive fallback rather than a linear scan. See
> Bottleneck 2. Closing one index is not evidence about the others on the
> same type.

## Bottleneck 7 — Pre-spawn epoch seeding collapses to serial

Found via `parse_log.py --stalls` on a deeply-nested corpus under `--jobs 4`.
Two compounding mechanisms, with three distinct fixes.

### Mechanism 1: epoch batch size collapses with directory depth

`parse_all` grouped files into `parse_epoch` batches by
`dir.components().count()` (OS path components), so batch size was a function
of path shape, not `--jobs`. On a deeply-nested corpus 69.4% of the 1,949
epochs held a single file — rising from 40.2% in the first third of the run to
88.6% in the last — giving `--jobs 4` no parallelism for the bulk of the run.

**Fixed by `ProtoIndex::network_dirs_by_tree_depth()`** (`164a35d`), which
groups by subnet-tree depth (parent hops). `ProtoIndex::children_of` already
flattens plain intervening directories — a subnet at `A/docs/parts/B/` is a
*direct* child of `A`, exactly like one at `A/B2/` — so component-count
grouping was splitting true siblings across epochs and delaying the
deeper-pathed one by the length of its path prefix, serialising work with no
dependency between it. Tree-depth grouping reduces epoch count and grows batch
size without weakening the drain-between-groups invariant: every dir in group D
still has its parent in group D-1, by construction.

| | Before | After |
|---|---:|---:|
| Parallel epochs | 1,949 | **14** |
| Mean batch size | 1.12–3.35 | **265.8** |
| Size-1 epochs | 69.4% | **7.1%** |
| Tasks | 3,721 | 3,721 |

Identical task count with 139× fewer epochs: the corpus was almost entirely
plain-directory indirection.

### Mechanism 2: per-task seeding rebuilt shared state

Every spawned task logged `Initializing GraphBuilder` after semaphore
acquisition and `[parse_epoch] task seeded` after `GraphBuilder::seed_session`
returned — both inside the same task future, so the gap should be near-zero. It
was bimodal: 439 tasks <10ms, 1,171 tasks >5s, max 8.25s, 38.5% of tasks over
3s. Attribution: `BeliefBase::from` rebuild 95.5%, `union_graphs` 2.9%, graph
clone 1.6%. Within `BeliefBase::new_unbalanced`, `PathMapMap::new` is 99.3%,
and within that `PathMap::new` is 92.9%.

**Fix A — `as_subgraph_seeded` scanned the whole graph per network**
(`4f352e2`). `PathMap::new`'s opening `as_subgraph_seeded` call bracketed a
correctly-bounded DFS with two **unbounded** full-graph passes: a `BTreeMap`
over every node in the graph, built only to resolve one seed; and a scan over
every edge, then filtered to the reachable set. With ~1,011 networks that is
O(networks × graph size). Two changes, which only work together (the first
alone leaves the edge scan; the second alone leaves the index build):

1. `as_subgraph_seeded_indexed` takes a caller-supplied `FxHashMap<Bid,
   NodeIndex>`; `PathMapMap::new` builds it once and reuses it across networks.
2. Edge collection walks outward from the reachable set via `edges_directed`
   instead of scanning all edges.

**Fix B — share one prebuilt `BeliefBase` per epoch** instead of rebuilding it
per task (`4452085`, `GraphBuilder::seed_session_from_base`). This did not need
the overlay redesign that made it look invasive: sharing one prebuilt base per
epoch, with copy-on-write in `PathMapMap`, was local to the seeding path.

### Measured outcome

| | wall clock | parse phase | seeding (summed) |
|---|---:|---:|---:|
| baseline | 28m19s | 20m38s | 2,533s |
| + shared epoch base (`4452085`) | **19m22s** | **11m32s** | **524s** |

Fix A alone, measured on the full corpus with identical task count, epoch
structure and `merged_states` (same work, not less): `BeliefBase::from` rebuild
3.97x, seeding total 3.50x, parse phase 48.2 min → 21.2 min. On the slow-path
population (1,432 unioned tasks): mean rebuild 5.85s → 1.48s, 56.3 → 14.2
µs/state, and **tasks >5s: 1,171 → 0**.

`union_graphs` was left untouched as a control and moved 259.5s → 262.5s
(0.99x) — the strongest evidence the gain is real rather than machine-state
drift, since a faster machine would have moved both.

**Residual**: 14.2 µs/state is not flat (a micro-corpus sits near 7.4) and
`PathMapMap::new` remains 84.2% of seeding, since each network still walks its
own reachable set. If this matters again, partition edges by owning network in
one pass rather than per-network — driven by a fresh measurement, not assumed.

### Durable findings

**The `union_graphs` clone-cost theory is refuted.** The const-namespace *is*
the discriminator the bimodality predicted — fast tasks have `unioned=0%` and
`const_ns_states=0`, slow tasks `unioned=100%` and `const_ns_states=112,177` —
but the expensive part is rebuilding indices over those states, not copying
them. Const-namespace nesting by URL segment, once the leading candidate fix,
does not address this: it re-parents nodes rather than reducing their count,
and the cost tracks node count. Backlogged; see `docs/project/BACKLOG.md`.

**Small epoch batches cannot be enlarged by merging the next depth-group
forward.** Under correct depth-grouping every member of group D+1 has its
parent in group D, which has not been drained yet, so every merge candidate
hits the uncommitted-parent path that mints a fresh BID and panics in Phase 4
`get_context`. Fix the grouping metric, not the batch size.

**Tail latency is not the binding metric; aggregate cost is.** After Fix A the
>5s tail was zero, which made Fix B look unwarranted — but seeding was still
~half the parse phase, 97.5% of it redundant reconstruction of shared data, and
Fix B then took seeding 2,533s → 524s. Likewise, removing epoch fragmentation
(Mechanism 1) did not move wall clock on its own: seeding cost is *per task*,
so fewer, larger batches simply meant more tasks running concurrently against a
large namespace. Judge a per-task cost by its sum, not its worst case.

**Edge iteration order was load-bearing and not preserved for free.**
`edge_references()` yields in edge-index order; `edges_directed` yields
per-node in reverse insertion order. Since `BidSubGraph` is a `GraphMap`
(insertion-ordered) that `PathMap::new` then DFSes, the swap would have
silently changed traversal order. Edges are tagged with
`edge_ref.id().index()` and sorted to reproduce the original sequence;
`test_subgraph_seeded_matches_reference_implementation` pins it against an
in-test copy of the old algorithm, and deleting the sort makes it fail.

**A 14-task run is not corpus-scale evidence — but it pointed the right way.**
The small-corpus signal (`BeliefBase::from` 94.4% of seeding, `union_graphs`
1.8%) matched the full-corpus split (95.5% / 2.9%) closely enough to have saved
a session had it been trusted. Treat it as a hypothesis generator, not a
decision input.

**Summing per-task gaps overstates build cost.** Tasks run concurrently, so the
8,772s of summed per-task gaps double-counts overlapping wall clock. The
wall-clock-bounded measure is the sum of `[task-switch]`-tagged gaps between
*consecutive log lines* — moments where nothing in the whole process logged
anything — which gave 1,907s (5.9% of the run) as the portion where every
worker was simultaneously blocked. That is a lower bound: it counts only
all-stall moments, not per-task cost hidden behind other tasks' useful work.

**Tooling**: `[seed_session] session_bb built` splits per-task cost into
`union_us` / `clone_us` / `rebuild_us`; `[epoch_session_snapshot] built` splits
the serial per-epoch cost across its three parts plus the state clone and edge
filter. Both on `noet_core::codec::perf` at `debug`;
`benches/log_analysis/analyze_seed_session.py` aggregates them.

---

## Bottleneck 8 — `parse_epoch` ancestor-BID seed miss on reparse (resolved)

Found while diagnosing a correctness defect (duplicate content nodes; see
`docs/design/codecs/network_authoring.md` §8), not from a performance
measurement — but a silently duplicated subtree inflates every later
O(graph size) stage, so it is recorded here per this issue's cross-cutting
lesson about correctness bugs inflating downstream cost.

### Symptom

Under `--jobs > 1`, `parse_epoch`'s pre-spawn seed computation resolves each
path's owning-network BID and document BID from `session_bb`, falling back to
`global_bb`. On a **reparse** (`processed[path] > 1`) both lookups could miss,
and the path was dispatched into the parallel batch anyway with an **empty
seed**.

An empty seed is not a slower path to the same answer — it is silently wrong.
`GraphBuilder::initialize_stack`'s slow path cannot find the ancestor chain
either (that is *why* the lookup failed), so `push()`'s node-not-found branch
mints a **fresh `Bid::new(parent_bid)`** for the ancestor network, and every
node the reparse produces is keyed under it. Because the duplicate lives in a
structurally distinct subnet it never competes with the original for a slot in
any one network's `PathMap`, so the one-path-one-BID collision warning never
fires — the two copies are invisible to each other by construction, not merely
undetected.

### Root cause

A **directory symlink** pointing from one subtree into another, with two plain
(non-network) directories between the link and its nearest network ancestor.

`net_dir_partition` walks with `follow_links(true)`. Both of its passes drop
symlinked **files**, with an explicit rationale: the target would otherwise be
parsed under two networks and produce duplicate nodes with different BIDs.
There was no equivalent guard for symlinked **directories**, so the walk
descended through one and recorded the target's canonicalized paths while
recursing under the *link's* parent.

The chain:

1. Descending through the link attributes the target network to the link's
   nearest network ancestor, in an unrelated subtree. The plain intervening
   directories are load-bearing: `net_dir_partition` flattens them, so the
   target lands in the child list of a network at a **different tree depth**
   than its true parent.
2. `network_dirs_by_tree_depth`'s `parent_of` map therefore holds the wrong
   parent. That parent's depth is not yet assigned when the child is visited,
   so the child falls through to `.unwrap_or(0)` and is grouped at **depth 0**
   alongside the repo root — ahead of its real parent at depth 1.
3. This violates the invariant that function documents explicitly: *every dir
   in group `k` has its parent network in group `k-1`*.
4. `sync_subnet_stubs` then runs for the subnet before its parent exists in
   `session_bb`, takes its skip branch, and registers no stub.
5. **The skip cascades**: each child subnet finds *its* parent unstubbed and
   skips in turn — one skip per subnet in the subtree.
6. No stub means no `PathMap` entry for the owning network, so `parse_epoch`'s
   seed loop misses it in both `session_bb` and `global_bb`.
7. On a reparse, that is the seed-miss symptom above.

Correlation was exact across every run measured: skips present ⇔ seed misses
present, in fixed proportion; zero skips ⇔ zero seed misses.

**Why a seed miss on reparse is always a corruption signal, never a timing
one.** Epoch staging makes this provable rather than probable. `parse_all`
runs Phase 1 (every network directory, grouped by tree depth, drained between
groups), then Phase 2 (every leaf document), and only then the remainder loop.
So by the time any path is reparsed, every network and document in the corpus
has been parsed once and committed to `global_bb`. The complete Section graph
exists; only epistemic and pragmatic links can still be dangling.

A reparse that cannot resolve its own owning-network BID is therefore not
waiting on anything — the entry it needs was either never created or was
created under the wrong key. That is what made "the sibling task's write hasn't
propagated yet" the wrong hypothesis, and why deferral could never have fixed
this on its own: more epochs cannot supply a `PathMap` entry that nothing
writes. It also gives the `fallback_queue`'s eviction warning its real meaning
— **it is a corruption alarm, not a slow-convergence notice.** Any future
occurrence should be investigated as a structural defect in how the node was
keyed, on the model of this one.

Intermittency came from an id race deciding whether the bad parent edge was
traversed before the real one. The contested ids were title-derived and each
appeared three times — once at the canonical location and again via the link.
The corpus contained no duplicate explicit `id:` fields; the duplication was
the symlink's doing.

**Ruled out, recorded so they are not retried**: `drain_epoch` sequencing;
`session_bb`/`global_bb` accumulator staleness; a `NodeKey::Path` form mismatch
between the two parses; `requeue_reparse_bids` (absent from the failing burst
entirely). Also not an ordering bug *within* `sync_subnet_stubs` — expanding
that pass to pull in missing ancestors and process them shallowest-first does
**not** help, because the parent is genuinely unparsed at that point rather
than merely out of order.

### Fix

Two layers: one prevents the defect, one contains the class.

**Prevention** — `net_dir_partition` no longer descends into a symlinked
directory (`is_dir_symlink`, applied in both `WalkDir` passes). The target is
still discovered and parsed at its canonical location by the same walk.

**Containment** — `fallback_queue` keeps an unresolvable seed loud and bounded
rather than silently corrupting. When pre-spawn seed resolution fails on a
reparse, the path is deferred to a dedicated queue instead of dispatching with
an empty seed. The two queues' entries are never mixed within a batch, so a
retry never races the siblings whose writes it is waiting on.

The remainder loop **alternates** between the queues rather than draining the
fallback queue greedily. Back-to-back fallback epochs are near-pointless: a
fallback epoch parses only paths whose seeds already failed, so if they fail
again the epoch writes almost nothing and the next retry's seed sees a global
state essentially identical to the last. Measured under greedy draining, a
second attempt reached eviction **47ms** after the first — an attempt spent
against unchanged state. With alternation the same gap is **7.5s**, one full
remainder epoch, so each retry is evaluated against a beliefbase that actually
advanced. When one queue is empty the other runs regardless; the alternation is
a preference, not a requirement.

Retries are counted in `fallback_attempts`, separate from `processed` because
the two bound different things: `processed` bounds how many times a document's
*content* is re-examined, `fallback_attempts` how many epochs may be spent
waiting for its *seed infrastructure*. Three rules make the interaction sound:

- **A deferral is not refunded against `processed`.** The deferred path will be
  parsed next epoch, so the attempt it occupies is real. The fallback
  *substitutes* for the reparse the path would otherwise have had rather than
  adding to it, so a successful first fallback costs exactly what a normal
  reparse would. Only the retry after a failed fallback is additional.
- **Eviction is terminal.** Once over budget a path is never deferred or warned
  about again; without the latch the counter climbs on every re-entry and each
  visit re-evicts.
- **The eviction parse is final, and its result is kept.** It is exempt from
  `ReparseLimitExceeded` but has its re-queue suppressed. Both halves are
  load-bearing, and getting either alone wrong is instructive:
  - Exempting *without* suppressing the re-queue is an infinite reparse loop
    (observed: a 900 MB log). The limit is otherwise the only thing that
    terminates the remainder loop.
  - Suppressing *without* exempting discards the best-effort parse as
    over-budget, which measured **54 of 54 deferred paths truncated** — every
    document that hit a seed failure replaced by an empty
    `ReparseLimitExceeded` placeholder. Strictly worse than the duplication
    being prevented.

  With both, zero deferred paths are truncated and the degraded parse is
  retained.

### Measured

Full-corpus runs (`--jobs 8`, ~69,000 files, ~72,600 parses):

| | before | after |
|---|---|---|
| seed misses | 70 | **0** |
| `sync_subnet_stubs` skips | 18 | 0 |
| `PathMap` collisions | 8,619 | 8,610 |

All 70 original misses were `net_bid` misses and **zero** were `doc_bid`
misses — consistent with a missing *network* stub rather than a document-level
problem. The unchanged collision count confirms Bottleneck 9 is a separate
defect this fix does not touch.

Reduced repro: 4-10 failures per 12 runs before, **0 per 14** after, with parse
count stable at the passing value. Wall clock is not a useful signal here —
full-corpus runs vary by more than 4x on the same cache for unrelated reasons;
the per-path instrumentation (`noet_core::codec::fast_path` at `debug`) is what
to measure.

### Reproduction and regression

The defect needs the **triggering structure**, not merely scale. A dozen
earlier attempts capped at ~2,400 files all missed it because none contained a
directory symlink of this shape. Any subtree that does contain one reproduces
it at ~1/30 the cost of a full-corpus run (~2,500 files, ~40s), intermittently,
at `--jobs > 1`; `--jobs 1` never reproduces it.

All 70 full-corpus misses fell in **one subtree** out of ~69,000 files. Nesting
depth is not the discriminator (over a thousand networks sit at depth ≥4
without failing), nor is any particular index-file directive. The subtree was
singled out because exactly one directory symlink in the corpus pointed into
it.

The unit regression is `test_dir_symlink_does_not_reparent_target_network`
(`proto_index.rs`), which reproduces the corpus's *shape* — the link must sit
under plain directories so its nearest network ancestor is in an unrelated
subtree at a different tree depth. Verified to fail without the guard and pass
with it. A flatter synthetic layout does **not** reproduce the misattribution
and silently passes either way.

### Relationship to Bottleneck 9: disjoint defects

Measured on one full-corpus run producing both: **zero** of its 8,619 `PathMap`
collisions fall in the subtree holding **all** 70 seed misses, and the subtree
repro that yields 71 seed misses yields zero collisions. Two defects, disjoint
populations, one shared trigger surface (parallel dispatch plus a reparse
epoch). Bottleneck 9's `net_bid`/`doc_bid` resolve correctly, so a missing
subnet stub cannot explain it.

- [ ] The id race that made this intermittent is no longer reachable through
      this path, but `FIRST-ONE-WINS` firing three times per contested id
      suggests it may surface elsewhere — worth its own investigation

---

## Bottleneck 9 — reparse-seed miss duplicates section BIDs under `--jobs > 1` (open)

A reparsed document's per-heading `cache_fetch` misses even though its
`net_bid`/`doc_bid` resolve correctly, so the reparse task mints fresh BIDs for
every heading. Because those duplicates are ordinary first-one-wins content (not
Bottleneck 8's freshly-minted, structurally-isolated subnet) they do land in the
same `PathMap`, which is why `[PathMap::new] two entries share one path` is the
visible symptom here.

**Reproduction** — a representative requirements subtree, fresh copy per run:
`--jobs 4` produces 389 `PathMap::new` collisions on each of three independent
runs; `--jobs 1` produces **zero** on each of three. Parallel dispatch plus at
least one reparse epoch is the precondition.

Evidence for the mechanism: of the 389 collisions, 387 are plain internal
`path#anchor` keys. Tracing one collision's `previous`/`replacement` BIDs to
their log lines shows one document parsed by two tasks in the same run — once
in the first epoch, once in a reparse epoch triggered by an unrelated
unresolved reference. The reparse logs `[cache_fetch] MISS on re-parse` for all
~45 of its headings while `submap_by_bid(net_bid, Some(doc_bid), 0, true)`
reports a non-empty `seed_states=67`. So the seed resolves the document's
ancestor identity and still fails to make its headings visible — a narrower
failure than Bottleneck 8, and one the `pn > 1` fallback guard does not catch.
The remaining 2 collisions are the pre-existing href-stub absorption pattern
from `5f31d75`.

**Ruled out by measurement, so do not re-investigate:**

- *`MdCodec`'s alias machinery.* No `alias-template`-derived key appears in any
  fresh-parse log, and the per-node alias loop and `AliasScope` play no part.
  Duplicate-BID pairs in a stored snapshot that once suggested otherwise were
  stale build artifacts; a fresh parse resolves the worked example to one BID.
- *The Section-edge-clobbering defect* fixed in `compute_diff` Phase 4 (a
  network's `index.md` citing its own child erased that child's structural edge,
  leaving it unanchorable). Plausible — an unanchored node is unfindable by path
  key on reparse, which is this signature — but the same subtree still produces
  exactly 389 collisions on three runs after that fix, and the corpus contains
  no `index.md` with the trigger shape.
- *Bottleneck 8's missing subnet stub.* Disjoint populations on a single
  full-corpus run producing both: zero of its 8,619 collisions fall in the
  subtree holding all 70 seed misses, and the seed-miss repro yields zero
  collisions. The two share a trigger surface (parallel dispatch plus a reparse
  epoch), not a defect.
- *The corpus's `ReparseLimitExceeded` truncations.* A plausible link, since
  both involve reparses going wrong, but the populations barely intersect: only
  17 of the 139 parallel-only truncations have a matching collision entry, and
  the truncations are 90% C++ while the collisions are markdown sections and
  href stubs. The two metrics also respond differently to `--jobs` — collisions
  scale 6.7x, truncations 1.3x. Bottleneck 3's resolution confirms the
  separation: those truncations were an unlatched codec-namespace re-queue, and
  removing them leaves the collisions untouched.

**Where to look next.** `submap_by_bid`'s `depth: 0` is a *subnet-crossing*
budget, not a Section-tree depth, so headings are included in the returned BID
set — the seed is not truncated at the source. `cache_fetch`'s probe for a
heading is path-index-keyed (`net_get_from_path` → `PathMap`, consulted *before*
`states`), so a node whose state is merged but whose `PathMap` entry is missing
misses. `seed_session_from_base` merges the per-doc seed incrementally via
`process_event_queue` rather than rebuilding the index, and that path has at
least three silent drops worth instrumenting: `to_event_stream_with`'s
`evaluate_query` error return, its `already_present` edge elision, and
`process_relation_update`'s sink-missing early return. Counting each would
distinguish them.

- [ ] Root-cause why `submap_by_bid`'s balanced seed for a reparsed document
      does not make all of its heading BIDs visible to `cache_fetch` on the reparse
      task, despite `doc_bid`/`net_bid` resolving correctly and `seed_states` being
      reported non-empty — likely in `to_event_stream_with`'s tape-scoped
      halo/section-ancestor traversal (`beliefbase/graph.rs`) or in how
      `seed_session_from_base` merges the per-doc seed into the shared epoch base
      (`builder.rs`). Reproduced on two distinct documents, so this is a general
      defect rather than a fixture artifact

- [ ] Extend the seed-failure guard in `parse_epoch` to also catch a
      resolved-but-*incomplete* per-document seed, not just an outright
      ancestor-BID lookup failure. The `fallback_queue` mechanism built for
      Bottleneck 8 is the natural home: the detection differs, the containment
      does not
- [ ] A regression fixture is still needed and is a prerequisite for any fix.
      Synthetic fixtures did **not** reproduce this at `--jobs 4` (tried up to 40
      networks × 60 cross-referencing headings, with reparse-forcing unresolved
      wikilinks); only real corpus subtrees do.

---

## Cross-cutting lessons

General debugging practice earned on this issue's bottlenecks. Mechanism-specific
detail stays in the entry it belongs to; what follows is what transfers.

### Measuring

**Silence is where the time is.** Three bottlenecks were found by measuring
*gaps between log lines*, not by reading instrumented numbers.
`parse_log.py --stalls` is the highest-yield first tool on any unexplained
wall-clock complaint.

**A large metric is not necessarily a binding constraint.** The PathMap scan was
422M operations per run; removing it moved wall clock ~1%. Establish that a cost
is on the critical path before optimising it. The converse also held: a mid-run
projection missed the total by 2.5x because the remaining phases had not started.

**A null result is only as good as the instrument.** `grep -c` returning 0
proves nothing until the log filter is verified — a `RUST_LOG` that excludes the
target cannot emit the line, and the tracing subscriber colourises even when
redirected, so `grep 'parse_task{'` finds none of the 12,046 present. Both fail
silently and in the direction of good news. Confirm the instrument by grepping
for a line you know is there.

**Check which knobs the harness actually sets.** The warm-cache regression is
invisible to the standard render recipe because it never passes `--db`. Two
sessions nearly compared incomparable runs.

### Attributing

**"Same end state" is not "same defect."** Several distinct bugs here converge
on one observable: a node present by BID but with no resolvable path. Matching
symptoms is not attribution — measure whether fixing one moves the other, and
check whether the corpus even *contains* the candidate cause's precondition.
Bottlenecks 8 and 9 share a trigger surface and are disjoint defects.

**A corpus-wide rate is not a per-subtree probability.** Bottleneck 8 measured
0.095% across the corpus and never reproduced in a dozen attempts at ~2,400
files — because all 70 hits sat in *one* subtree, where it fires reliably. Bucket
a failure's own log lines by path before concluding you need scale; a defect
that is 0.1% overall but 100% within one subtree is a localization problem, and
the two call for opposite strategies.

**A correctness bug can look like a performance problem, and compound one.**
Bottleneck 8 was found while investigating data quality, but every silently
duplicated subtree pays full construction cost for content that should not
exist. When node or edge counts look implausible for the file count, check for
silent duplication before assuming the graph is simply big.

### Bounding and retrying

**A budget that truncates reports a symptom; the defect is whatever spent it.**
Sweeping the limit discriminates cheaply: a count that falls as the budget rises
is genuinely budget-bound, while a flat plateau means the work is not converging
and no budget shape will fix it.

**A retry predicate must name who eventually says "stop."** The Bottleneck 3
defect was one unconditional "retry" whose comment explained why a first attempt
deserves one and never addressed what happens when the target never appears.
Worse, the same value doubled as "not yet known bad", so the optimistic branch
could never become pessimistic. State a retry's terminating condition in the
same breath, and check whether a sibling case in the same function already has
one.

**A retry is only worth its budget if something changed between attempts.**
Identify which other work produces the state change the retry depends on, and
ensure it is interleaved — otherwise the retry count measures patience rather
than opportunity. Relatedly, a retry mechanism must be reconciled against *every*
counter and termination guard it touches: individually reasonable choices can
each fail alone, and exempting a parse from a limit without also suppressing its
re-queue is an infinite loop.

**Ask what the pipeline's staging already guarantees before theorising a race.**
Bottleneck 8's miss was attributed to propagation delay for several sessions;
the epoch schedule rules that out for free, because every network and document
is committed before any reparse. A lookup that misses *then* is corrupted, not
early. This converts an open-ended timing hunt into a bounded search for who
wrote the wrong key.

### Changing things safely

**Before reclassifying a population of failures as benign, audit what is in it.**
A change that makes a warning class disappear is only as good as the evidence
that every member deserved to. Partition by a check *independent* of the one
that produced the warning — here "does a file with this name exist?" — because
the alternative converts real defects into plausible-looking successes, which
are harder to find than the warnings they replaced.

**A parser over a permissive grammar fails by omission, and omission is silent.**
Four separate CMake gaps each dropped whole components from the graph: a missing
optional keyword, an inline argument list, an unread modern idiom, and a
condition that was too narrow. Nothing errored in any case. A parser that
*rejects* bad input announces itself; one that *skips* unrecognised-but-valid
input removes content silently. Enumerate the optional syntax the grammar
allows, and be most suspicious where a field is load-bearing for downstream
visibility rather than merely descriptive.

**A guard that names its rationale should be checked against the adjacent case.**
`net_dir_partition` dropped symlinked *files* with a comment explaining why —
a rationale that applied verbatim to symlinked *directories*, which it followed.
When a guard exists for one member of a category, check the siblings.

**Verify a regression test fails without its fix, and reproduce the corpus's
*shape* rather than its scale.** Two tests here passed against unfixed code: one
because a flatter synthetic layout did not reproduce the structural trigger,
another because an unrelated fallback produced the same output. Both would have
shipped as dead tests. Revert the fix, watch the test fail, restore.


## Open Questions


- Do Bottlenecks 4 and 5 share a mechanism? Both show a local PathMap far
  smaller than the authoritative membership. A single DB-query-count probe
  would answer this for both.
- Is `finalize_html` (Bottleneck 2) parallelisable at all, or is it
  inherently a serial tail? Worth knowing before investing in it — a 608s
  serial tail on a 4.7h build caps at ~3.6% even if eliminated entirely.
- Does Bottleneck 7's per-task setup cost share a mechanism with Bottlenecks
  4/5? All three show cost that scales with `session_bb`'s accumulated
  const-namespace size rather than with the individual document being
  parsed. If so, a fix to one may resolve all three.

- The `--jobs` CLI help text (`cli.rs:144`) says "default: available CPUs",
  but `DocumentCompiler::with_html_output` actually defaults to `jobs=1`
  (parallel dispatch requires explicit opt-in via `--jobs` or `NOET_JOBS`).
  This is a doc/behavior mismatch independent of this issue's scope — worth
  a one-line fix to the help text (or, if available-CPU default was the
  intended behavior, a design discussion on making it the default once the
  parallel path is production-validated, per the comment at
  `compiler.rs:313`).
- Does `BeliefAccumulator`'s channel have bounded capacity, and if so, could
  the *receiver* side throttle throughput regardless of sender concurrency?

## References

- `planning/reference/PERFORMANCE_LOG.md` — per-run wall-clock diary; the
  2026-08-21 entry holds the const-namespace profiling detail.
- `noet-core/benches/log_analysis/README.md` — which `RUST_LOG` targets each
  analysis tool needs, and the ANSI-grep trap.
- `noet-core/docs/design/core/beliefbase_architecture.md` §3.1 ("Phase 5 —
  `terminate_stack`") and §3.2 ("Two-Cache Architecture").
- `noet-core/docs/project/BACKLOG.md` — const-namespace nesting (deferred; once
  the leading candidate fix for Bottleneck 7's per-task clone cost, until
  measurement ruled it out), the remaining `PathMap` full-map scans, and the
  warm-cache regression. Bottleneck 6 was resolved by `PathMap::path_map`;
  Bottleneck 7 by `4f352e2` + `GraphBuilder::seed_session_from_base`
  (`4452085`).
- `noet-core/src/codec/builder.rs` — `terminate_stack` (~L1537),
  `parse_content` Phase 5 entry (~L1246); `seed_session_from_base` and
  `epoch_session_snapshot`, the shared-epoch-base path that replaced the
  refuted `union_graphs`-clone-cost theory for Bottleneck 7.
- `noet-core/src/codec/compiler.rs` — `with_html_output` jobs resolution,
  `parse_epoch` sequential/parallel dispatch, `fallback_queue` /
  `fallback_attempts` / `defer_to_fallback_queue` (Bottleneck 8 containment),
  `finalize_html` (Bottleneck 2); `process_unresolved_reference`'s
  codec-namespace and synthetic-path guards plus `permanently_unresolved`
  (Bottleneck 3 fix), with
  `test_codec_namespace_ref_stops_requeuing_after_first_parse` as the
  regression.
- `noet-core/src/codec/proto_index.rs` — `net_dir_partition`'s symlink guards
  and `network_dirs_by_tree_depth`'s parent/depth mapping (Bottleneck 8 fix);
  `test_dir_symlink_does_not_reparent_target_network` is the regression.
- `noet-core/src/layout.rs` — `compute_layout_metadata`, `LayoutConfig`,
  `resolve_scope` (Bottleneck 2 fix: reserved-namespace exclusion,
  `max_nodes` guard, `indexed_path` fallback removal).
- `noet-core/src/paths/pathmap.rs` — `PathMapMap::indexed_path` (BID→path,
  Bottleneck 2's `node_to_nets` fallback fix) and `PathMap::indexed_get`
  (path→BID, Bottleneck 6's `path_map` index) — two distinct indices on
  two distinct types; do not conflate them.
- `noet-core/benches/log_analysis/analyze_finalize_html.py`,
  `analyze_cache_fetch.py`, `analyze_cpp_parse.py`,
  `analyze_seed_session.py` — tooling added for Bottlenecks 2/3/4/5/7
  respectively; see their module docstrings for the `RUST_LOG` targets each
  needs.
- `noet-core/benches/log_analysis/parse_log.py --warnings` — the
  `noet_core::paths::collision`-target classifiers that surfaced Bottlenecks 8
  and 9 ("Stub evicted by content-node claim" and "Duplicate path survived to
  PathMap construction"); see `benches/log_analysis/README.md` §"One-path-one-BID
  enforcement".
- `noet-core/docs/design/codecs/network_authoring.md` §8 ("URL Aliasing") —
  `alias-template`/`alias-scope` mechanism; ruled out as Bottleneck 9's cause,
  kept here as the reference that documents why the theory was plausible.
- `noet-core/src/beliefbase/base.rs` — `compute_diff` Phase 4's weight-union
  clause. A separate one-path-one-BID defect with a symptom that looks like
  Bottleneck 9's (unanchored node → `cache_fetch` miss on reparse) but is
  unrelated to it; see the Cross-cutting lesson on distinguishing the two.
- `planning/project/ISSUE_26_pandoc_markdown_quality.md` — origin of this
  investigation.
- Commits: `6c313d5` (multi-threaded runtime — probable Bottleneck 1 fix),
  `4313c41` (const-namespace seeding), `fe7e8ab` (path index — Bottleneck 6),
  `164a35d` (epoch tree-depth grouping), `4f352e2` (`as_subgraph_seeded` scan
  fix), `2784514` (collision-check index conversion), `85c631a`
  (alias-template scope + path-mangling fix), `d4e0a17` (one path, one BID),
  `4452085` (shared epoch session base — Bottleneck 7 resolved), `ba2f745`
  (preserve edge kinds and unblock queued deps on reparse).
