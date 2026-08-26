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

## Current status (2026-08-21)

The original subject of this issue — sequential `terminate_stack` taking ~70%
of wall clock — **appears resolved**, most likely by `6c313d5` (multi-threaded
runtime). See "Bottleneck 1" below. The issue has been broadened to track the
bottlenecks that remain, because the profile has shifted substantially: parse
is no longer the dominant cost.

From a full-corpus run at `4313c41` (`just jobs=4 render`, in-memory DB):

| Measure | Value |
|---|---:|
| Wall clock | 4.70h (was 12.45h at the prior version) |
| Parse sequential sum | 0.62h (was 34.81h) |
| In-permit parse work | 0.74h across 7,306 files (mean 0.362s, p95 0.89s) |
| **Time outside the parse path** | **~4.5 of 4.7h** |

**This reframes everything below.** Optimising inside the parse path now
caps out at roughly 15% of wall clock. The open bottlenecks are in
`finalize_html`, the C++/header path, and export.

**Update (product-hierarchy corpus run, 2026-08-21):** two more measurements
narrow this further. Bottleneck 2's `finalize_html` cost is now attributed —
it is `compute_layout_metadata`, a single serial stage, not an unexplained gap
(see below). And a distinct mechanism, Bottleneck 7, shows the pre-spawn
per-epoch task setup collapsing to fully serial for the back ~60% of a
deeply-nested corpus, costing 31.8 min (5.9%) of wall clock in moments where
every worker was simultaneously blocked. Time "outside the parse path" is not
one thing — it's at least three distinct mechanisms with different fixes.

**Update (instrumented re-run, 2026-08-22):** both remaining large costs now
have named root causes rather than symptoms. Bottleneck 2 is
`compute_layout_metadata` at ~290s (8.6% of build wall clock), stable across
two runs. Bottleneck 7's per-task cost is `PathMapMap::new` rebuilding
per-network subgraphs via two unbounded full-graph passes — split into two
fixes, both since implemented: the `as_subgraph_seeded` scan fix (`4f352e2`)
and the shared epoch session base (`GraphBuilder::seed_session_from_base`,
seeding 2,533s → 524s). A recurring lesson across both: **the first framing of
each was an artifact of missing instrumentation**, not a real description of
the defect.

**Update (2026-08-25, both Bottleneck 2 and 7 now resolved):** two more
full-corpus runs closed both remaining register entries. Bottleneck 2's
force-simulation theory was wrong — the actual cost was an exhaustive
`PathMapMap::path()` fallback scan that resolved nothing (84.6% of the
stage, 100% wasted); deleting it took the stage 217.6s → 43.0s, 6.7x below
the original 290s baseline. Bottleneck 7 gained a second fix on top of
`4f352e2`: sharing one prebuilt `BeliefBase` per epoch instead of rebuilding
it per task (`4452085`) took summed seeding time 2,533s → 524s.

Three measurements on the same corpus, in commit order:

| | wall clock | parse phase | seeding (summed) |
|---|---:|---:|---:|
| `6bbf5c3` (baseline) | 28m19s | 20m38s | 2,533s |
| `4452085` (shared epoch base) | 19m22s | 11m32s | 524s |
| + uncommitted `indexed_path` fallback removal | ~8m | ~5m | 126.9s |

The third row's improvement over the second is not a layout-only effect: the
`indexed_path` fallback (Bottleneck 2's fix) is called during parsing too
(relation resolution, `beliefbase/context.rs`), so removing it sped up the
parse phase as a side effect, independent of `finalize_html`. Three of seven
register entries remain open (3, 4, 5); none of the three touch the parse or
`finalize_html` path any of this work exercised.

## Bottleneck register

| # | Bottleneck | Magnitude | Status |
|---|---|---|---|
| 1 | Sequential `terminate_stack` stalls | was ~70% of wall clock | **Likely resolved** — needs confirmation |
| 2 | `compute_layout_metadata` | **290s → 43.0s (6.7x)** | **Resolved** — an exhaustive fallback scan was 84.6% of the stage and resolved nothing; removed. Stage is now 26.4% of `finalize_html`, no single dominant term |
| 3 | C++/header parse gaps | ~163 min across 211 gaps >20s | **Open** — now instrumented, awaiting a full-corpus run to attribute |
| 4 | End-of-run insert storm | 962K log lines in 13 min | **Open** — mechanism unclear |
| 5 | Warm-cache (`--db`) regression | 94s → 28m on one subtree | **Open** — `--db` only, not on `render` path |
| 6 | PathMap read-path scan | 422M entries scanned/run | **Resolved** — `PathMap::path_map` index |
| 7 | Pre-spawn epoch seeding collapses to serial | seeding 2,533s → 126.9s (summed, concurrent); wall clock 28m19s → 19m22s | **Resolved** — epoch fragmentation fixed (1,949 → 14 epochs); per-task `PathMapMap::new` rebuild fixed (`4f352e2`); redundant per-task rebuild eliminated via a shared epoch base (`4452085`) |

---

## Bottleneck 1 — Sequential `terminate_stack` (likely resolved)

### Summary

A sequential (`jobs=1`) large corpus parse showed
`GraphBuilder::terminate_stack` (Phase 5) consuming ~70% of wall clock, with a
worst-case single file taking 20 minutes for one diff. A follow-up run of the
*same corpus* with `--jobs 4` showed every file's `terminate_stack` completing
in **well under one second** — including the file that took 20 minutes under
`jobs=1`, with an identical 908-`RelationUpdate` diff. This four-orders-of-
magnitude gap is too large to be explained by parallelism alone (4 workers
cannot turn a CPU-bound 20-minute task into 0.12s); it strongly suggests the
sequential path was blocked on something external to `compute_diff`/
`process_event`'s actual work — most likely a channel/receiver-side
bottleneck (`tx.send` to `BeliefAccumulator`) or a runtime scheduling
pathology specific to single-threaded inline dispatch.

This issue is scoped to root-causing *why* the sequential path is so much
slower, rather than assuming Phase 5 pipelining (the original framing) is
the right fix — the evidence suggests the fix may be much smaller and
different in kind (e.g. an accidental blocking call, an unbounded/misused
channel, or lock contention specific to single-task dispatch).

### Evidence

#### Run 1 — `jobs=1` (default; no `--jobs`/`NOET_JOBS` set)

```sh
RUST_LOG=noet_core::codec::builder=debug,noet_core::codec::compiler=debug,noet_core::db::query_size=debug \
  noet parse . --html-output /tmp/out 2>&1 | tee /tmp/run.log
```

run from the large application corpus (~3,700 file records, 92.3 minutes
wall clock). Confirmed sequential: zero `parse_task{task_idx=...}` spans
anywhere in the 337,100-line log (per `DocumentCompiler::with_html_output`,
`compiler.rs:311-321`, `jobs` defaults to 1 unless explicitly set — see Open
Questions on the CLI help text mismatch).

Analyzed with `benches/log_analysis/parse_log.py --phase-summary`:

- Naive "Phase 5 start of file N → Phase 0 start of file N+1" gap, summed
  across 3,479 files: 3,862.0s (64.4 minutes) — 69.7% of wall clock. Phase 0
  itself is negligible (mean 0.00s, max 0.18s).
- Worst single file (a large slide-deck export, ~7,600 lines) — 1,172.73s
  (~20 minutes) for one `Diff events (2271): NodeUpdate(455),
  RelationUpdate(908), PathsAdded(908)` batch.
- A cluster of ~13 near-duplicate large slide decks (the same ~7,600-line
  document, re-exported once per parent node it is linked from) each
  independently show ~908 `RelationUpdate`s and 20–30s gaps.
- Zero panics, zero `WARN`/`ERROR`-level lines, zero "unbalanced"
  diagnostics — confirms Issue 26's content-quality fixes are effective for
  this subtree in isolation (does **not** confirm the original full-build
  balanced-set panic is resolved; see Issue 26).

#### Run 2 — `--jobs 4` (same corpus, in progress at time of writing)

Same command with `--jobs 4` added. The naive gap metric from
`parse_log.py --phase-summary` is **not valid for parallel runs** — it
diffs consecutive log records in file order, which interleaves across
unrelated concurrent tasks once `jobs > 1`. A task-scoped measurement was
used instead: for each `task_idx`, the time from that task's own
"Phase 5: terminating stack" line to that same task's own "Diff events"
line (see `.scratchpad` script used for this investigation — not persisted,
reproduce via the method described in Implementation Steps).

Results (2,556 `terminate_stack` completions measured so far, run still in
progress):

- **That same slide-deck export: 0.12s** (was 1,172.73s under
  `jobs=1` — same 908 `RelationUpdate`s, same content).
- **Every measured file completes in under 1 second**; max observed 0.22s.
  Sum across all 2,556 completions: 8.9s total.
- Zero warnings/errors so far, consistent with Run 1.

**This run was still in progress when this data was captured** — the full
warning/panic comparison and final wall-clock total should be re-confirmed
once it completes.

#### Run 3 — `jobs=1` at `6031c95` (2026-08-21)

Spot-check on a 30-file subtree (a small application-corpus subtree) with
`RUST_LOG=debug`, sequential:

```
17:42:50.047692  Phase 5: terminating stack ...
17:42:50.047743  Diff events (12): ...      <-  51 microseconds
17:42:50.051827  Phase 5: terminating stack ...
17:42:50.052011  Diff events (39): ...      <- 184 microseconds
```

Every Phase 5 completes in well under a millisecond under `jobs=1`. The
pathology does not reproduce on this subtree.

#### Probable cause: `6c313d5` (multi-threaded runtime)

That commit changed `noet parse` from `Builder::new_current_thread()` to a
multi-threaded runtime, and its message describes exactly the mechanism this
issue hypothesised: `parse_content`'s Phase 2 is CPU-bound with no yield
points, so under a single-threaded runtime one task starves every other task
— including tasks that only need to poll an already-resolvable future. That
is the "cooperative scheduling starvation" candidate listed under Risks, and
it explains why a 20-minute stall could collapse to 0.12s without the diff
work itself changing.

It also explains the four-orders-of-magnitude gap that seemed too large for
parallelism alone: the sequential path was not doing more work, it was
waiting on a runtime that could not schedule the completion.

**Not yet confirmed**, because the check above used a 30-file subtree rather
than the corpus that originally reproduced it. To close Bottleneck 1:

- [ ] Re-run the full application corpus with `jobs=1` at current HEAD
- [ ] Confirm the worst-case slide-deck export completes Phase 5 in well
      under a second (was 1,172.73s)
- [ ] If confirmed, record the resolution and remove the Goals/Implementation
      sections below, which are written against the unresolved framing

---

## Bottleneck 2 — `compute_layout_metadata` (single largest serial stage)

**This entry was originally "`finalize_html` silent gap" — a hunt for
unaccounted wall time.** That framing was an artifact of low-fidelity logging:
with no stage timers, a 608s block appeared to be a defect in pipeline
plumbing. Stage-level timers (`[finalize_html stage] <name>`,
`noet_core::codec::perf`, debug) closed the gap question completely — all 10
stages are now instrumented and "no silent gap remains uninstrumented" — and
revealed that the time was never mysterious. It was one stage doing real,
expensive, fully serial compute. The entry is now scoped to that cost.

`compute_layout_metadata` (`src/layout.rs`) is the 3D credibility-map force
simulation over the full corpus graph. Measured across two full-corpus runs:

| Stage | Run A | Run B | % of finalize_html (Run B) |
|---|---:|---:|---:|
| **`compute_layout_metadata`** | **292.7s** | **288.6s** | **62.9%** |
| `build_search_indices` | 75.0s | 75.5s | 16.4% |
| `create_asset_hardlinks` | 35.4s | 72.3s | 15.7% |
| `export_beliefbase` | 11.9s | 12.1s | 2.6% |
| `BeliefBase::from(graph)` rebuild | 8.8s | 8.6s | 1.9% |
| `export_beliefgraph` | 1.6s | 1.5s | 0.3% |
| all others | <1s each | <1s each | <0.5% combined |

Stable across runs at ~290s, ~4x the next-largest stage, and **8.6% of total
build wall clock** (288.6s of a 55m48s run). It is single-threaded and runs
after all parsing completes, so it is pure critical path — no other work
overlaps it. That makes it comparable in size to the parse-side bottlenecks in
this register while being considerably more self-contained: one stage, one
module, no concurrency or correctness invariants entangled with it.

The run also carries 137,244 nodes / 219,827 relations into this stage, so
whether cost is linear or superlinear in graph size determines whether this
grows into the dominant build cost as corpora scale.

### Root cause: one synthetic network holds 97.4% of the cost

Cost was attributed analytically against a compiled corpus shard manifest
(1,435 networks, 264,757 nodes) rather than by re-running the build. The
intra-bubble simulation is `O(iterations x n^2)` per network with
`iterations = 200`, so total work is `sum over networks of 200 * n^2 / 2`.
That sum is almost entirely one term:

| Network | Nodes | Share of total force-sim work |
|---|---:|---:|
| **synthetic href-tracking namespace** | **78,479** | **97.41%** |
| largest real content network | 3,095 | 0.15% |
| all 1,430 other networks combined | 183,183 | 2.44% |

The href-tracking namespace is a **reserved, synthetic namespace**
(`href_namespace()`, see `properties.rs::const_namespaces`) holding one
External|Trace node per distinct outbound hyperlink. It is not user-authored
content, it is not browsable in the viewer, and its nodes have no meaningful
N/S/P content profile. Its node count scales with the number of *links* in the
corpus, not the number of documents — so it grows faster than the real corpus
and will increasingly dominate this stage.

Confirmed against the emitted shards: the href-tracking shard carries 78,479
`render_position` entries, i.e. the simulation really is running over all of
them.

### Why aggressive task spawning does not help here

Parallelising the per-network loop is the obvious move and it is nearly
worthless, because makespan is bounded below by the single largest task
(Amdahl). With the href network included:

| Workers | Predicted speedup | Predicted stage time |
|---:|---:|---:|
| 4 | 1.03x | 282s |
| 16 | 1.03x | 282s |
| 64 | 1.03x | 282s |

Spawning cannot beat 282s at *any* worker count. Parallelism only becomes
worthwhile *after* the dominant task is removed:

| Scope | Serial | 4 workers | 8 workers |
|---|---:|---:|---:|
| excluding href-tracking namespace | ~7.5s | ~1.9s | ~0.9s |

So the ordering matters: **skip reserved namespaces first (290s -> ~7.5s, a
~39x win), then parallelise if the residual still matters (~7.5s -> ~1s).**
Doing it in the other order buys 3%.

### Secondary defect: full edge-list rescan per network

`run_intra_bubble_layout` iterates *every* edge in the whole graph to find the
edges local to one network, once per network. That is `O(networks x E)` =
1,164 x 499,634 = **5.8e8 iterations**, each with two `BTreeMap<Bid, usize>`
lookups. It is masked by the `n^2` term today; once the href network is
excluded it becomes the dominant remaining cost. Fix is a single pre-pass
bucketing edges by home network.

- [x] Add spans around the discrete stages of `finalize_html`
- [x] Attribute the silent block to a stage — it is `compute_layout_metadata`,
      confirmed on two independent full-corpus runs
- [x] Profile `compute_layout_metadata` internally — cost is the `O(200 * n^2)`
      intra-bubble repulsion loop, 97.4% of it in the synthetic href-tracking
      namespace. Scaling is **quadratic in the largest network's node count**,
      and that network grows with link count, so this worsens with corpus size.
- [x] Establish whether the simulation needs to run over the *full* graph — it
      does not. Reserved/synthetic namespaces (`Bid::is_reserved()`) are not
      viewer-facing and should be excluded outright.
- [x] Exclude reserved namespaces from layout (expected 290s -> ~7.5s)
- [x] Bucket edges by home network in `run_intra_bubble_layout`, removing the
      `O(networks x E)` rescan
- [x] Add an `O(n^2)` guard: networks above `--layout-max-nodes` /
      `NOET_LAYOUT_MAX_NODES` (default 5,000) are skipped with a warning
- [x] Add `--no-layout` to skip the stage entirely (layout remains on by default)
- [x] Confirm the predicted ~39x on a full-corpus run — **the prediction was
      wrong**; see "Measured result" below
- [x] Narrow `indexed_path` candidates through the `node_to_nets` reverse index
- [x] Re-run with per-step timers — 539s -> 265.8s, and the residual is now
      fully attributed: 96.1% is still `pathmap.path()`
- [x] Remove the per-call `read_arc()` over all networks in `indexed_path`
      (memoized subnet-holder set)
- [ ] Re-measure after the memoization
- [ ] Only then consider parallelising the per-network loop (~7.5s -> ~1s);
      ceiling is 3% while the href network is still in scope

### Resolution (implemented, pending full-corpus confirmation)

`compute_layout_metadata` now takes a `LayoutConfig { enabled, max_nodes }` and
selects its network scope up front via `select_networks`:

- Reserved namespaces (`Bid::is_reserved()`, covering all four
  `const_namespaces()`) are excluded. This is a **correctness** fix as much as a
  performance one: those nodes are synthetic `External|Trace` bookkeeping and
  carry none of the N/S/P assumptions the layout scoring is built on (see
  `docs/essays/engineering_model_ontology.md` §3). Computing viewer coordinates
  for them was a category error.
- Networks above `max_nodes` are skipped with a warning naming the flag, so an
  oversized network degrades loudly instead of silently stalling the build.

Exclusion is applied once, in `build_home_network_map`, so excluded networks
drop out of *every* downstream step rather than being computed and discarded.

Note the two exclusions differ in kind: the reserved-namespace rule is
permanent and semantic, while `max_nodes` is a pragmatic guard against the
`O(n^2)` term and can be raised when a large network genuinely needs layout.

**Consumer impact**: `render_position`, `structural_weight` and
`structural_depth` are now legitimately absent for some networks. Issue 85's
viewer must treat all four layout fields as optional and fall back gracefully.

### Measured result: the prediction was wrong (2026-08-25)

The predicted ~39x did not materialise. The stage went **290s -> 539s** — it
got *worse*. Two independent errors, both instructive:

**1. The cost model was calibrated on a stale corpus.** The `O(n²)` analysis
used a shard manifest from an earlier build in which the href-tracking
namespace held 78,479 nodes. On the actual run it held **3,676** — a 456x
smaller force-simulation term. The whole "97.4% of cost" finding was an
artifact of measuring one corpus and predicting another. Total nodes differed
too (264,757 vs 135,590), which should have been the tell.

**2. The fix introduced a regression.** Timestamps in the run localise it:

| Phase | Elapsed |
|---|---:|
| `BeliefBase::from` -> first `select_networks` log | **265.2s** |
| selection log -> stage end | 273.8s |
| total | 539.0s |

Those 265s are *before any layout work begins*. `select_networks` added a
second full `PathMapMap::path()` pass to count nodes per network, and
`build_home_network_map` then repeated it. `path()` is `O(networks)` — it probes
every network's `PathMap` and takes a minimum — so each pass is ~1.5e8 PathMap
probes at this corpus size, and the change doubled it to ~3.1e8.

**The real bottleneck was never the force simulation.** It is
`PathMapMap::path()`, the same `O(networks)` read-path family as Bottleneck 6.
The force-sim `n²` term is a rounding error on this corpus:
spread over 1,128 selected networks it is ~0.7s.

**Fixed**: selection and mapping are fused into `resolve_scope`, restoring a
single `path()` call per node. Per-step timers were added under
`noet_core::codec::perf` (`[layout step] <name>`) so the residual 273.8s can be
attributed rather than guessed at — `compute_network_aggregates` calls
`pathmap.submap()` once per network and is the leading suspect.

**Lesson**: the exclusion work still stands on its own — scoring synthetic
namespaces against the N/S/P ontology is a category error regardless of cost —
but it was justified with a performance number derived from a different corpus
than the one it ran against. Calibrate cost models against the corpus you will
measure on, and instrument *before* optimising, not after.

### Follow-on: narrowing `indexed_path` via the `node_to_nets` reverse index

With the true bottleneck identified as `PathMapMap::path()`, the fix is the
index that already exists for exactly this fan-out. `process_event_queue`
already routes relation events through `node_to_nets` "rather than broadcasting
to all O(N_networks) PathMaps"; `indexed_path` was still broadcasting.

`indexed_path` now probes only the networks that directly contain the BID, plus
every network holding subnets. That second term is required for correctness,
not caution: `node_to_nets` records **direct** containment, while
`PathMap::path` also resolves a BID held by a *subnet* by recursing into it.
Narrowing to direct hits alone silently regresses those nodes to `None`.
Unknown BIDs fall back to the full scan, so a stale index cannot change results.

Equivalence is asserted against the exhaustive scan for every BID in the graph
plus an absent one (`test_indexed_path_narrowing_matches_full_scan`). The test
was mutation-checked: dropping the subnet term makes it fail, so it is not
passing vacuously. The fixture is also asserted to contain a subnet, so the
recursion case cannot silently stop being covered.

**Expected gain is a ratio, not a constant**: candidate-set width goes from
`N_networks` to `direct + subnet_holding_nets`. At this corpus size the win is
~3x if 30% of networks hold subnets, ~20x if 5% do. `subnet_holding_nets` is now
logged alongside the scope-resolution timer so the next run reports the actual
ratio instead of it being predicted. **No speedup is claimed here until
measured** — that is the error this entry already records once.

Note this also speeds up `indexed_path` for *all* callers, not just layout —
it is the same read path as Bottleneck 6.

#### Measured (2026-08-25, second run)

`compute_layout_metadata`: **539.0s -> 265.8s** (2.03x). Against the original
290s baseline that is only 1.09x — the narrowing repaid the regression I
introduced, and little more. Per-step timers now attribute the whole stage:

| Step | Time | Share |
|---|---:|---:|
| **scope resolution (`pathmap.path`)** | **255.5s** | **96.1%** |
| `compute_render_positions` | 10.2s | 3.8% |
| all other six steps combined | <0.1s | ~0.0% |

Two things this settles:

1. **The force simulation was never the problem.** The `O(n²)` term everything
   was originally built around is 3.8% of the stage. The six remaining steps
   are collectively unmeasurable. Every hypothesis in this entry prior to
   instrumentation was aimed at the wrong 4%.

2. **The first narrowing under-delivered, and the log said why.** The candidate
   set fell 5.5x (1,131 -> 203 subnet-holders + ~2 direct) but the stage only
   improved 1.04x on that step. Cost was not proportional to the candidate
   count because the *filter itself* iterated all 1,131 networks per call,
   taking a `read_arc()` on each just to test `subnets()` — ~1.5e8 lock
   acquisitions, exactly the fan-out the index was meant to remove.

**Fixed**: the subnet-holder set is a property of the map, not of the BID, so
it is now memoized and the per-call loop iterates only `direct + holders`.
Invalidation hangs off `make_pathmap_unique`, which HEAD already established as
the pre-write chokepoint — a new write site cannot bypass it. `PathMapMap`'s
derived `Clone` was replaced with a hand-written one so a clone starts with a
cold cache rather than inheriting one that may not describe it (clones diverge
via `make_pathmap_unique`).

Staleness is the whole risk in a cache like this, so
`test_indexed_path_narrowing_survives_subnet_mutation` warms the cache, gives a
network a *new* subnet, and re-asserts scan-equivalence. It carries vacuity
guards asserting the parent genuinely gained a subnet it lacked when warmed —
without those the test passed even with invalidation disabled, which is how the
first version of it was caught being useless.

**No further speedup is predicted here.** The next run measures it.

#### Measured (2026-08-25, runs 3 and 4)

Probing subnet *ancestors* rather than all subnet-holders: **271.3s -> 217.7s**
(1.25x). Reproduced at 217.6s. Scope resolution 260.2s -> 207.2s.

Smaller than the structure suggested — candidates per call fell ~100x for the
narrowed route, but wall time moved 1.25x — so the per-route counters were
added. They ended the guessing immediately:

| Route | Calls | Probes | Probes/call | Time | Share |
|---|---:|---:|---:|---:|---:|
| indexed | 233,542 | 1,561,651 | 6.7 | 34.7s | 16.7% |
| **fallback** | **58,922** | **66,640,782** | **1,131.0** | **175.3s** | **84.6%** |

**The fallback was 20.1% of calls but 97.7% of probes.** Every narrowing so far
had been tuning the 16.7%.

Worse, the fallback found nothing. The arithmetic reconciles exactly: 135,590
nodes - 58,922 fallback calls = 76,668 resolvable; minus 3,918 reserved-namespace
nodes (3,676 + 241 + 1) = 72,750 = the logged `mapped_nodes`. So all 66.6M probes
returned `None`. 175.3s of confirming absence.

**Fixed by deleting the fallback.** `node_to_nets` holds an entry for every BID
in every `PathMap`'s `bid_map`, and `PathMap::path` resolves only BIDs reachable
through some `bid_map` — so an index miss *is* proof of absence, and the scan
only rediscovers that at `O(N_networks)` cost. `indexed_path` now returns `None`
directly on a miss.

That inference is the load-bearing part, so it is asserted rather than argued:
`test_node_to_nets_miss_implies_no_path` checks, for every BID in the fixture,
that an index miss implies the exhaustive scan also finds nothing. The counter
test now asserts a miss records **zero** probes, which is what would catch a
reintroduced scan. `scan_indexed_path` is retained `#[cfg(test)]` as the
ground-truth oracle the narrowing tests compare against.

**Measured (run 5): 217.6s -> 43.0s** (5.1x), against a predicted ~42.3s — 1.6%
error, the first prediction in this entry that held. Scope resolution 207.2s ->
32.3s; fallback probes 66,640,782 -> 0. Against the original 290s baseline the
stage is **6.7x faster, 247s saved**.

The stage is no longer dominated by one term: scope resolution is 32.3s (75%)
and the force simulation 10.6s (25%). `compute_layout_metadata` has gone from
62.5% of `finalize_html` to 26.4%, and `create_asset_hardlinks` (72.5s) is now
the largest stage in it.

The CoW sentinel is unchanged at 4.17% (baseline ~2.8%), confirming the
`read_arc()` calls added by the subnet-ancestor index did not provoke spurious
copies — worth checking because any read guard held across a write inflates
`Arc::strong_count`.

#### Method note

Three successive "optimisations" of this stage moved wall time by ~0, +2%, and
1.25x, because each targeted a term chosen by inspection. The counter that split
calls from probes found the real 84.6% on its first run. Cheap attribution
beats careful reasoning about which term dominates — and "probes" versus "calls"
was the distinction that mattered, since the expensive route was the rarer one.

The full sequence, for calibration against future estimates:

| Change | Stage | Basis |
|---|---:|---|
| baseline | 290.0s | — |
| exclude reserved namespaces | 539.0s | cost model from a *stale corpus*; also added a second pathmap pass |
| narrow to subnet-holders | 265.8s | reasoned about candidate count |
| memoize holder set | 271.3s | reasoned about lock overhead |
| narrow to subnet-ancestors | 217.6s | reasoned about which candidates could match |
| **delete the fallback** | **43.0s** | **measured attribution** |

Every reasoned step landed within 2x of no-op or made things worse. The one
measured step delivered 5.1x. Note also that the first change *regressed* the
stage by 86% while being justified with a confident quantitative argument — the
argument was arithmetically sound and calibrated on the wrong corpus.

#### Follow-on

Removing the fallback speeds up `indexed_path` for every caller, not just
layout — MCP tools, `beliefbase/context.rs`, and relation resolution all use it,
and 97,952 of the 233,542 indexed calls in this run came from outside layout.
Bottleneck 6 concerns the same read path; confirmed unaffected by this fix
(Bottleneck 6 is about `PathMap::indexed_get`'s path→BID lookup, not
`PathMapMap::path`'s BID→path lookup this fix touches — different index,
same family of defect).

## Bottleneck 3 — C++/header parse gaps (~163 min)

211 process-wide silent gaps >20s, totalling ~163 minutes. The largest
cluster falls in minutes 170–265 during the **C++ source corpus** — gaps of
114–288s each with near-zero log output, at `task_idx` ~1090–1260 on
deeply-included C++ headers.

Larger in aggregate than Bottleneck 2 but spread across 211 events, so more
likely to be genuine CPU-bound tree-sitter parsing (i.e. real work) than a
structural defect. Worth measuring before assuming either.

- [ ] Determine whether these gaps are tree-sitter parse time, include
      resolution, or something else
- [ ] Compare per-file cost against file size / include count to see whether
      the relationship is superlinear

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

Found via `parse_log.py --stalls` on a deeply-nested product-hierarchy corpus
under `--jobs 4`, confirmed by the reporter as consistent with multiple prior
runs, and quantified on the completed run (53m54s wall clock, 63,045 parses).
Two related measurements on the same mechanism:

**1. Epoch batch size collapses as directory depth increases.** `parse_epoch`
groups files into batches by directory-component depth (see `parse_all`'s
depth-grouping comment), so batch size is a function of corpus shape, not
`--jobs`. Measured across 1,949 epoch batches for the full run:

| Segment | Epochs | Mean batch size | Size-1 (fully serial) epochs |
|---|---:|---:|---:|
| First third | 649 | 3.35 | 40.2% |
| Middle third | 650 | 1.26 | 79.4% |
| Last third | 650 | 1.12 | 88.6% |

A deeply-nested corpus (many directories with few siblings at depth) spends
most of its epochs at batch size 1 — `--jobs 4` provides no parallelism for
the bulk of the run once depth grows past the corpus's average branching
factor. 69.4% of all epochs in the full run were size 1.

**2. Each task pays multi-second, fully serial setup cost before its own
parse begins — even at batch size 1.** Every spawned task logs
`Initializing GraphBuilder` immediately after semaphore acquisition, then
`[parse_epoch] task seeded` immediately after `GraphBuilder::seed_session`
returns — both inside the same task future, so the gap between them should
be near-zero. Measured across all 3,721 tasks in the full run:

| Measure | Value |
|---|---:|
| Gaps > 3s | 1,433 (38.5% of all tasks) |
| Sum of per-task gaps | 8,772s (exceeds wall clock — tasks overlap; see caveat below) |
| Mean gap (all tasks) | 2.42s |
| Median gap (all tasks) | 0.15s |
| Max gap | 8.25s |

**Caveat on the 8,772s figure**: this sums each task's individual gap, but
tasks run concurrently, so the sum double-counts overlapping wall-clock time
and cannot be read as "seconds of the build." A wall-clock-bounded measure —
summing only the literal `[task-switch]`-tagged gaps between *consecutive log
lines* (i.e., moments where nothing in the whole process logged anything) —
gives **1,907s (31.8 min, 5.9% of the 53.9-minute run)** as the portion of
wall clock where every worker was simultaneously blocked on this mechanism.
That 5.9% is a lower bound: it only counts moments where *all* concurrent
tasks stalled together, not the (much larger) per-task cost that's hidden
behind other tasks' useful work.

The bimodal distribution (439 tasks <10ms, 1,097 tasks >5s) suggests two
distinct populations rather than one scaling cost — consistent with the
`doc_seed` vs. `network_ancestors` fallback branch in `seed_session`
(`builder.rs`): tasks with a non-empty per-document seed take one path,
fallback tasks clone the full `network_ancestors` snapshot (including the
href/asset const-namespaces) via `union_graphs`. Working theory — **not yet
confirmed** — is that this clone cost is the dominant term, and that it grows
with `session_bb`'s accumulated const-namespace size over the run (the same
growth pattern documented in Bottlenecks 4/5). No log output exists inside
this gap regardless of cause, so the specific line responsible has not been
isolated.

- [x] Add span-level timing inside `seed_session` (the `union_graphs` call
      specifically) and `epoch_session_snapshot` to attribute the gap to a
      specific operation rather than the whole `GraphBuilder::new` →
      `seed_session` span
- [x] Confirm or refute the clone-cost-scales-with-corpus-size theory by
      correlating gap size against `network_ancestors_has_href` /
      `session_bb_nodes` at the time of the gap — **refuted**; see "Measured
      outcome" below
- [x] Determine whether the depth-based epoch batching strategy itself should
      change (e.g. batch across sibling subtrees rather than strictly by
      depth) once the per-task cost is understood, since fixing the per-task
      cost matters most for exactly the size-1 epochs this bottleneck
      describes — **yes; changed to subnet-tree depth**
- [x] Re-run against a full-corpus log (not a partial run) to get total
      wall-clock share rather than an in-flight lower bound

### Agreed next steps (2026-08-22)

Four-session plan, in order:

1. ~~**Rebalance small epoch batches.**~~ **Superseded — done differently.**
   The premise was that small batches should be enlarged by merging the next
   depth-group forward. Reading the code showed that is *never* safe: under
   correct depth-grouping every member of group D+1 has its parent in group D,
   which has not been drained yet, so every merge candidate hits the
   uncommitted-parent path that mints a fresh BID and panics in Phase 4
   `get_context` (the failure the pre-epoch repo-root block documents).

   The real defect was in the grouping metric, not the batch size.
   `parse_all` grouped by `dir.components().count()` (OS path components), but
   `ProtoIndex::children_of` already flattens plain intervening directories —
   a subnet at `A/docs/parts/B/` is a *direct* child of `A`, exactly like one
   at `A/B2/`. Component-count grouping split those true siblings into
   different epochs and delayed the deeper-pathed one by the length of its
   path prefix, serializing work with no dependency between it.

   Fixed by `ProtoIndex::network_dirs_by_tree_depth()`, which groups by
   subnet-tree depth (parent hops) instead. This *reduces* epoch count and
   *grows* batch size without weakening the drain-between-groups invariant:
   every dir in group D still has its parent in group D-1, by construction.
   Note this does not manufacture parallelism where none exists — a genuine
   subnet chain still yields size-1 groups under either metric, so the effect
   on the 69.4% figure depends on how much plain-directory indirection the
   corpus actually has. Step 3 measures that.
2. **Instrument the real `Initializing GraphBuilder` → `task seeded` cost.**
   **Done.** `[seed_session] session_bb built` splits per-task cost into
   `union_us` / `clone_us` / `rebuild_us`; `[epoch_session_snapshot] built`
   splits the serial per-epoch cost across its three parts plus the state
   clone and edge filter. Both on `noet_core::codec::perf` at `debug`.
   `benches/log_analysis/analyze_seed_session.py` aggregates them, tests
   whether the bimodality tracks the `unioned` branch, and reports first- vs.
   last-quarter growth.

   **Early signal, small corpus only — not yet corpus-scale evidence**: on a
   14-task run, `BeliefBase::from` rebuild was 94.4% of seeding cost and
   `union_graphs` only 1.8%. If that holds at corpus scale it points *away*
   from the standing `union_graphs` theory (and therefore away from
   const-namespace nesting as the step-4 fix) and toward PathMap
   reconstruction. Step 3 decides; do not act on this number alone.
3. **Re-run the product-hierarchy corpus** with both changes in place and
   re-assess against this issue's numbers. **Done — see "Measured outcome".**
4. **Tackle the initialization cost directly.** The original plan named
   nesting the const-namespaces by URL segment. **Superseded by measurement**
   — the cost is `BeliefBase::from`'s PathMap rebuild, not `union_graphs`, and
   nesting does not address it (it re-parents nodes rather than reducing their
   count, and the cost tracks node count). Nesting is now backlogged; see
   `docs/project/BACKLOG.md`. Split into the `as_subgraph_seeded` scan fix
   (local, done first — `4f352e2`) and a shared epoch session base (expected to
   be invasive, only if the scan fix left cost on the table — it did).
   **Both are now done.** The shared base turned
   out not to need the overlay redesign that made it look invasive: sharing one
   prebuilt `BeliefBase` per epoch, with copy-on-write in `PathMapMap`, was
   local to the seeding path. See `GraphBuilder::seed_session_from_base`.

### Measured outcome (2026-08-22)

Re-run: same corpus, same `--jobs 4`, 63,045 parses, 55m48s (baseline 53m54s).
A competing benchmark contended for CPU during the first ~10 min; all
attribution figures below are unchanged when that window is excised
(`analyze_seed_session.py --skip-first-min 12`), so the confound does not
affect the conclusions — only absolute wall clock.

**Epoch structure — tree-depth grouping (step 1) worked:**

| | Baseline | This run |
|---|---:|---:|
| Parallel epochs | 1,949 | **14** |
| Mean batch size | 1.12–3.35 | **265.8** |
| Size-1 epochs | 69.4% | **7.1%** (1 of 14) |
| Tasks | 3,721 | 3,721 |

Identical task count with 139× fewer epochs: the corpus was almost entirely
plain-directory indirection, and component-count grouping had been spreading
true siblings across ~1,900 artificial epochs. Batch sizes are now
`[12, 54, 373, 228, 262, 151, 33, 3, 1, 8, 1163, 544, 885, 4]`.

**Per-task seeding — the `union_graphs` theory is refuted:**

| sub-step | total | share |
|---|---:|---:|
| `BeliefBase::from` rebuild | 8,607s | **95.5%** |
| `union_graphs` | 259s | 2.9% |
| graph clone | 145s | 1.6% |

The const-namespace *is* the discriminator, exactly as the bimodality
predicted — fast tasks (439, <10ms) have `unioned=0%` and `const_ns_states=0`;
slow tasks (1,171, >5s) have `unioned=100%` and `const_ns_states=112,177`. But
the expensive part is rebuilding indices over those states, not copying them.

**Root cause, profiled one level deeper:** within `BeliefBase::new_unbalanced`,
`PathMapMap::new` is 99.3%; within that, `PathMap::new` is 92.9%. Its opening
`as_subgraph_seeded` call brackets a bounded DFS with two *unbounded*
full-graph passes, and runs once per network (1,011 of them) — O(networks ×
graph size). Per-state cost rises 5.0 → 20.9 µs/state as graphs grow from 562
to 1,809 states, and a `n_nets × (nodes + edges)` model tracks the measurement
within a small constant. Fixed in `4f352e2` — see below.

### The fix: `as_subgraph_seeded` scanned the whole graph per network

`PathMapMap::new` builds one `PathMap` per network, and each called
`as_subgraph_seeded`, which bracketed a correctly-bounded DFS with two
**unbounded** full-graph passes:

- a `BTreeMap` over *every node* in the graph, built only to resolve one seed
- a scan over *every edge* in the graph, then filtered to the reachable set

With ~1,011 networks this is O(networks × graph size). Two changes, which only
work together (the first alone leaves the edge scan; the second alone leaves the
index build):

1. `as_subgraph_seeded_indexed` takes a caller-supplied `FxHashMap<Bid,
   NodeIndex>`; `PathMapMap::new` builds it once and reuses it across networks.
2. Edge collection walks outward from the reachable set via `edges_directed`
   instead of scanning all edges.

**Measured on the full corpus** (same command, same `--jobs 4`, 3,721 tasks,
identical epoch structure and identical `merged_states` — same work, not less):

| metric | before | after | change |
|---|---:|---:|---:|
| `BeliefBase::from` rebuild | 8,607.8s | 2,166.5s | **3.97x** |
| seeding total | 9,012.5s | 2,574.0s | 3.50x |
| `union_graphs` (control, untouched) | 259.5s | 262.5s | 0.99x |
| **parse phase wall clock** | **48.2 min** | **21.2 min** | **2.3x** |

On the slow-path population specifically (1,432 unioned tasks, mean 104,014
`merged_states` in both runs): mean rebuild 5.85s → 1.48s, max 7.74s → 3.13s,
56.3 → 14.2 µs/state, and **tasks >5s: 1,171 → 0**. Bottleneck 7's headline
finding is eliminated.

The unchanged `union_graphs` total is the strongest evidence the gain is real
rather than machine-state drift — a faster machine would have moved both.

**Ordering was load-bearing and not preserved for free.** `edge_references()`
yields in edge-index order; `edges_directed` yields per-node in reverse
insertion order. Since `BidSubGraph` is a `GraphMap` (insertion-ordered) that
`PathMap::new` then DFSes, this would have silently changed traversal order.
Edges are tagged with `edge_ref.id().index()` and sorted to reproduce the
original sequence;
`test_subgraph_seeded_matches_reference_implementation` pins it against an
in-test copy of the old algorithm, and deleting the sort makes it fail.

**Residual, not yet addressed**: 14.2 µs/state is still not flat (a micro-corpus
sits near 7.4), and `PathMapMap::new` remains 84.2% of seeding. Each network
still walks its own reachable set. If this matters again, the next step is
partitioning edges by owning network in one pass rather than per-network — but
that should be driven by a fresh measurement, not assumed.

**Consequence for the shared-`session_bb` redesign**: it was justified by the
>5s tail, which is now zero, so on this evidence it looked unwarranted.
**That conclusion was wrong** — it judged by *tail latency* when the binding
metric was *aggregate cost*. The tail was gone while seeding was still ~half
the parse phase, 97.5% of it redundant reconstruction of shared data. Sharing
one prebuilt base per epoch subsequently took seeding 2,533s → 524s and wall
clock 28m19s → 19m22s. The fix above was still worth doing and is what made the
remaining cost legible; only the "therefore stop here" inference was mistaken.

**Wall clock did not improve** (55m48s vs 53m54s), and 2,032s of inter-line
gaps still terminate at a seeding line (baseline 1,907s). Step 1 removed epoch
fragmentation, but seeding cost is *per task* — fewer, larger batches mean more
tasks run concurrently against a large namespace, so the aggregate held. The
remaining cost is the `as_subgraph_seeded` full-graph scan's to remove.

**Unrelated finding:** `compute_layout_metadata` took 288.6s in
`finalize_html` — a single serial stage worth 8.6% of the run, not currently in
this register. Worth its own entry.

---

## Bottleneck 1 — working sections

> Retained from the original single-bottleneck framing of this issue.
> Scoped to Bottleneck 1 only; the other bottlenecks track their own
> checkboxes inline above.

### Goals

1. Identify the specific mechanism causing `terminate_stack` to take orders
   of magnitude longer under `jobs=1` than under `jobs=4` for the *identical*
   diff content (908 `RelationUpdate`s, same file).
2. Determine whether this is a bug (e.g. a blocking call, unbounded channel
   growth, or lock contention specific to the inline sequential path) or an
   inherent property of single-threaded dispatch that parallel dispatch
   incidentally avoids.
3. Fix the root cause if it's a bug, or document why `jobs=1` is expected to
   remain slow and recommend `jobs > 1` as the practical mitigation if not.

### Architecture

Per `docs/design/beliefbase_architecture.md` §3.1, Phase 5
(`terminate_stack`) does, per document:

```
compute_diff(session_bb, doc_bb, parsed_nodes)              [O(diff size)]
for event in diff_events: session_bb.process_event(event)   [sequential await loop]
tx.send(event) for all tx_events                             [sequential await loop]
```

Per `compiler.rs:1758-1769`: when `jobs == 1`, `parse_one_path` runs inline
in the compiler's own async context using the compiler's own `builder` — no
task spawn, no semaphore, direct `tx` send. When `jobs > 1`, each path is a
separate `tokio::task::spawn` task with its own `GraphBuilder`, `tx` clone,
and `global_bb` handle.

**Original leading hypothesis** (channel/receiver bottleneck): `tx` is an
`UnboundedSender` to a single `BeliefAccumulator` receiver, so under `jobs=1`
a synchronous or lock-contended receiver would serialise into
`terminate_stack`'s critical path.

**Superseded.** The parenthetical alternative in that hypothesis —
"cooperative yielding starvation" — is what `6c313d5` found and fixed: under
`Builder::new_current_thread()`, a CPU-bound Phase 2 with no yield points
starved every other task on the runtime. Retained here because the reasoning
trail matters: the symptom pointed at the channel, and the cause was the
runtime underneath it.

Other candidate mechanisms to rule out:

- `global_bb` lock contention that behaves pathologically for a single
  writer with no concurrent readers/writers to naturally interleave against
  (unlikely, but should be checked).
- An accidental `O(n)` or worse scan inside `process_event`'s derivative
  computation that scales with total accumulated `session_bb` size in the
  sequential path but is somehow bypassed or amortized differently in the
  parallel path (this would be surprising given both paths call the same
  `process_event` code, but worth explicitly ruling out via profiling
  rather than assumed away).

### Implementation Steps

> Written against the unresolved framing. If the full-corpus `jobs=1`
> confirmation above passes, steps 1–2 are moot and only step 3's validation
> remains.

1. Instrumentation (0.5 day)
   - [ ] Add fine-grained timing (or use `tokio-console` / `tracing` spans)
         inside `terminate_stack` to split `compute_diff` time,
         `process_event` loop time, and `tx.send` loop time separately.
   - [ ] Re-run the `jobs=1` reproduction on a smaller corpus subset
         containing just the worst-case slide-deck export and enough prior
         context to reproduce the 20-minute stall, to get a fast iteration
         loop.
2. Root cause (1 day)
   - [ ] Determine which sub-phase accounts for the multi-order-of-magnitude
         gap between `jobs=1` and `jobs=4` for identical diff content.
   - [ ] If it's `tx.send`/channel-related: inspect `BeliefAccumulator`'s
         receive loop for synchronous or lock-contended per-event work that
         could explain serialized-sender pathology.
   - [ ] If it's `session_bb.process_event`: check for any accumulation
         (e.g. an internal `Vec` or index) whose per-call cost grows with
         total nodes/relations processed so far in the run, and confirm
         whether `jobs>1`'s per-task fresh `GraphBuilder`/`session_bb`
         instances reset that accumulation in a way the sequential path
         does not.
3. Fix + validate (0.5 day)
   - [ ] Apply the identified fix (or, if none is warranted, document the
         finding and recommend `jobs > 1` as the practical mitigation).
   - [ ] Re-run the full application corpus under `jobs=1` post-fix and
         confirm `terminate_stack` durations converge toward the `jobs=4`
         baseline (sub-second per file).

### Testing Requirements

- Existing `codec_test` suite must continue to pass unchanged.
- If a fix is applied: a regression test or benchmark assertion that a
  single-document `terminate_stack` call with a large synthetic diff
  (hundreds of `RelationUpdate`s) completes within a fixed time budget
  (e.g. <1s), to catch future regressions of this specific pathology.
- Benchmark: `jobs=1` vs `jobs=4` on the application corpus (or a
  smaller reproducible subset), before/after the fix, to confirm
  convergence.

### Success Criteria

- [ ] Root cause of the `jobs=1` vs `jobs=4` `terminate_stack` timing gap is
      identified and attributed to a specific code path.
- [ ] Either a fix is implemented and validated (sequential `terminate_stack`
      durations converge to sub-second, matching the parallel path), or a
      documented explanation is provided for why `jobs=1` is inherently slow
      and `jobs > 1` is the recommended mitigation.
- [ ] No regression in event-ordering correctness tests.

### Risks

- Risk: The root cause may be difficult to reproduce outside the full
  corpus context (similar to the original Issue 26 balanced-set panic,
  which only reproduced at full-build scale) → **Mitigation**: start
  instrumentation on the full application corpus where the effect is
  already confirmed, and only attempt to shrink the repro once the
  mechanism is understood well enough to know what state is required to
  trigger it.
- Risk: This may overlap with or be superseded by Issue 66 (Incremental
  Parse via Shard Hydration), which changes the parse pipeline's caching
  model → **Mitigation**: check Issue 66's status before starting
  implementation; coordinate if both are active concurrently.
- Risk: If the mechanism turns out to be inherent to single-threaded
  dispatch (e.g. cooperative scheduling starvation with no other task to
  yield to) rather than a bug, the "fix" may just be recommending
  `jobs > 1` as standard practice, with no code change — acceptable outcome,
  but should be validated rather than assumed given the magnitude of the
  gap (4 orders of magnitude is unusually large for a pure scheduling
  effect).

## Cross-cutting lessons

Recorded because each cost real investigation time and would otherwise be
relearned.

**Silence is where the time is.** Three of the five open bottlenecks were
found by measuring *gaps between log lines*, not by reading instrumented
numbers. `parse_log.py --stalls` is the highest-yield first tool on any
unexplained wall-clock complaint.

**A large metric is not necessarily a binding constraint.** The PathMap scan
was 422M operations per run and removing it changed wall clock by ~1%.
Establish that a cost is on the critical path before optimising it.

**Check which knobs the harness actually sets.** The warm-cache regression is
invisible to `just render` because that recipe never passes `--db`. Two
sessions nearly compared incomparable runs. Confirm the flags before
comparing numbers.

**Strip ANSI before grepping.** The tracing subscriber colourises even when
redirected to a file, wrapping span and field names token by token, so
`grep -c 'parse_task{'` returns 0 on a log containing 12,046 of them. See
`benches/log_analysis/README.md`.

**Extrapolating from a partial run is unreliable.** A mid-run projection of
~2h from a 119-minute checkpoint missed by 2.5x, because the last hours are
dominated by phases that had not started. Wait for completion before
quoting a total.

**Model assumptions before trusting a model.** The original `n²/4`
const-namespace insert-cost model was analytically reasonable and empirically
inapplicable:
sort keys are monotonic, so the random-insert distribution it assumed never
occurs. Measured shift was exactly zero.

## Open Questions

- Was Run 2 (`--jobs 4`) capturing a corpus that had already benefited from
  warm OS filesystem caches from Run 1, and could that (rather than `jobs`)
  explain part of the difference? Largely moot now that `6c313d5` supplies a
  mechanism, but the full-corpus `jobs=1` re-run would settle it.
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
  the *receiver* side have been the actual bottleneck in Run 1 regardless of
  sender concurrency?

## References

- `planning/reference/PERFORMANCE_LOG.md` — per-run wall-clock diary; the
  2026-08-21 entry holds the const-namespace profiling detail.
- `noet-core/benches/log_analysis/README.md` — which `RUST_LOG` targets each
  analysis tool needs, and the ANSI-grep trap.
- `noet-core/docs/design/beliefbase_architecture.md` §3.1 ("Phase 5 —
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
- `noet-core/src/codec/compiler.rs` — `with_html_output` jobs resolution
  (~L311-321), `parse_epoch` sequential/parallel dispatch (~L2150-2444),
  `finalize_html` (Bottleneck 2).
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
- `planning/project/ISSUE_26_pandoc_markdown_quality.md` — origin of this
  investigation.
- Commits: `6c313d5` (multi-threaded runtime — probable Bottleneck 1 fix),
  `4313c41` (const-namespace seeding), `fe7e8ab` (path index — Bottleneck 6),
  `164a35d` (epoch tree-depth grouping), `4f352e2` (`as_subgraph_seeded` scan
  fix), `2784514` (collision-check index conversion), `85c631a`
  (alias-template scope + path-mangling fix), `d4e0a17` (one path, one BID),
  `4452085` (shared epoch session base — Bottleneck 7 resolved).
  `src/layout.rs`'s reserved-namespace exclusion and `indexed_path`
  narrowing (Bottleneck 2 resolved) are uncommitted as of this writing.
