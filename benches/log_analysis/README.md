# Log Analysis Tools

Utilities for analysing output from noet corpus runs.

## What to capture

Each tool consumes a different slice of a run's output, so the `RUST_LOG`
setting that makes one work will not necessarily satisfy another. Pick the
narrowest capture that covers the tools you intend to run — `RUST_LOG=debug`
works for everything but produces very large logs (a 90-second corpus run with
`paths::scan` enabled emitted ~130 MB).

| Tool | Needs | Why |
|------|-------|-----|
| `parse_log.py` | `RUST_LOG=debug` | Per-file attribution comes from the `parse_task{…}` span prefix under `--jobs N>1`, or from sequential `[parse_one_path]` lines under `--jobs 1`; the tool handles both. Phase timings are `debug` events. |
| `analyze_phase2.py` | `RUST_LOG=debug` | Reads `[Phase 2]` events from `noet_core::codec::perf` (`debug`). |
| `analyze_finalize_html.py` | `RUST_LOG=debug` | Reads `[finalize_html stage]` events from `noet_core::codec::perf` (`debug`); one event per stage, fires once per run. |
| `analyze_cache_fetch.py` | `noet_core::codec::cache_fetch_census=debug` | Reads `[cache_fetch] census`/`global_cache_miss_local` events. **Not** covered by blanket `RUST_LOG=debug` — fires on every `cache_fetch` call (the hottest call site in Phase 2), so it is opt-in only. |
| `analyze_seed_session.py` | `RUST_LOG=debug` | Reads `[seed_session_from_base] session_bb built` (current) or `[seed_session] session_bb built` (legacy), plus `[epoch_session_snapshot] built`, `[parse_epoch] shared session base built`, and `[parse_epoch] pathmap copy-on-write counters` — all `noet_core::codec::perf` (`debug`). Requires `--jobs N>1`; every probe is on the parallel dispatch path. |
| `analyze_cpp_parse.py` | downstream C++ codec's perf target at `debug` | Reads `[CppCodec::parse]` events from a downstream C++ codec crate, separate from noet-core — only relevant for corpora with `.h`/`.cpp` files. |
| `analyze_warns.py` | any level (default `info` suffices) | Only reads `WARN` lines. |
| `extract_diagnostics.py` | **nothing** | Parses CLI diagnostics on stderr (`file:line:col: warning: …`), printed regardless of log level. |
| `extract_miss_keys.py` | **nothing** | Same CLI diagnostic stream. Also accepts a legacy `MISS on re-parse` tracing format that needed `noet_core::codec::fast_path=debug`. |
| `diff_miss_keys.py` | same as `extract_miss_keys.py` | Compares two such logs. |
| `analyze_pathmap.py` | `noet_core::paths::scan=debug`,<br>`noet_core::paths::perf=debug` | Also covered by a blanket `RUST_LOG=debug`; naming the targets trims the log modestly and keeps it readable. |
| (no tool yet) | `noet_core::paths::collision=debug` | `[indexed_get] multi-candidate path` — fires when one path string has more than one claimant, logging the path, claimant BIDs, stub flags, and ids. **Expected to be silent**: zero hits over 1.29M lookups on a full corpus since the `alias-scope` and bare-anchor fixes. Any hit is either a legitimate stub-vs-content overlap (check the `stub` flag) or a new path-generation defect. Grep it directly. |
| `url_depth_sweep.py` | **no log at all** | Reads a URL list or a `NOET_DUMP_NAMESPACES` dump. |

Targeted captures for the common cases:

```sh
# Phase timings, stalls, and the finalize_html stage breakdown
# (parse_log.py, analyze_phase2.py, analyze_finalize_html.py)
RUST_LOG=debug

# Unresolved links only — small log, no tracing needed
# (extract_diagnostics.py, extract_miss_keys.py, diff_miss_keys.py)
RUST_LOG=info

# PathMap read/write cost (analyze_pathmap.py)
RUST_LOG='info,noet_core::paths::scan=debug,noet_core::paths::perf=debug'

# cache_fetch source-outcome census (analyze_cache_fetch.py) — Issue 97
# Bottlenecks 4/5. Deliberately excluded from blanket debug (see table above).
RUST_LOG='info,noet_core::codec::cache_fetch_census=debug'

# C++ tree-sitter parse cost (analyze_cpp_parse.py, downstream C++ codec only) — Issue 97
# Bottleneck 3. Enable debug-level tracing on the downstream C++ codec's perf target.
RUST_LOG='info,<downstream_crate>::cpp::perf=debug'

# Epoch seeding + PathMap copy-on-write sentinel (analyze_seed_session.py).
# Far smaller than blanket debug: these probes fire once per task and once per
# epoch, not per path lookup.
RUST_LOG='info,noet_core::codec::perf=debug'

# Everything Issue 97 needs in one run (finalize_html + cache_fetch + C++,
# at the cost of also emitting every other debug-level event):
RUST_LOG='debug,noet_core::codec::cache_fetch_census=debug,<downstream_crate>::cpp::perf=debug'
```

Be aware that these logs get large fast, and that the `paths::*` targets are
the bulk of it either way. Measured on a 30-file corpus: blanket
`RUST_LOG=debug` produced 4.4 MB, the targeted `paths::*` filter 3.3 MB. On a
350-file corpus with ~21k assets the same targeted filter produced 130 MB —
the `indexed_get` probe fires on every path lookup. Scope runs to a subtree
when profiling.

### Grepping these logs: strip ANSI first

The tracing subscriber colourises output **even when stderr is redirected to a
file**, and it wraps span names, field names, and separators individually. A
span prefix that reads `parse_task{task_idx=0 path=…}` on screen is actually:

```
^[[1mparse_task^[[0m^[[1m{^[[0m^[[3mtask_idx^[[0m^[[2m=^[[0m0 …
```

So `grep -c 'parse_task{'` returns **0** on a log that contains 12,046 of
them — the escape codes sit between `parse_task` and `{`. Always strip first
when hand-checking:

```sh
sed 's/\x1b\[[0-9;]*m//g' my-run.log | grep -c 'parse_task{'
```

Every tool in this directory strips ANSI internally, so they are unaffected;
this only bites manual inspection.

### Span prefixes appear only under parallel dispatch

`parse_task` spans are created in the parallel epoch dispatcher, so a
`--jobs 1` run has none — it takes the sequential path instead.
`parse_log.py` reports which source it used:

```
Extracted 57 file records  (57 from parse_task spans, 0 sequential)   # --jobs 2
Extracted 57 file records  (0 from parse_task spans, 57 sequential)   # --jobs 1
```

Both yield full phase summaries, so either dispatch mode is fine for
profiling. Note that the 2025-05-27 entry in
`planning/reference/PERFORMANCE_LOG.md` records "File records (parse_log): 0
(no parse_task spans in log)" — that run found neither source, which is a
different (and now unreproducible) condition.

## Quick Start

```sh
# Capture a corpus run log
RUST_LOG=debug cargo run --features service,bin -- --color=always parse \
    --html-output /tmp/bench-output \
    .bench_corpora/mdn-content/files/en-us/web/javascript/ \
    2>&1 | tee my-run.log

# Phase 0 summary (default — slowest files, outliers flagged)
python3 benches/log_analysis/parse_log.py my-run.log

# All analyses at once
python3 benches/log_analysis/parse_log.py my-run.log --all

# Find silent stalls > 2s
python3 benches/log_analysis/parse_log.py my-run.log --stalls 2.0

# Per-phase breakdown for a specific file
python3 benches/log_analysis/parse_log.py my-run.log --phase-detail "temporal/duration"

# PathMap read/write cost (needs its own targets — see "What to capture")
python3 benches/log_analysis/analyze_pathmap.py my-run.log
```

## `parse_log.py`

**Requirements:** Python 3.10+, no third-party packages.

### Modes

| Flag | Default | Description |
|------|---------|-------------|
| `--phase-summary` | ✓ | Per-file Phase 0 (`initialize_stack`) duration, sorted descending. Mean, min, max, std-dev. Outliers flagged at mean + 2σ. Also reports Phase 5 post-processing gaps > 5s. |
| `--stalls SECONDS` | — | Every gap between consecutive log lines exceeding `SECONDS` (default `1.0`), with ±3 lines of context. Catches silent work that emits no log output. |
| `--warnings` | — | Counts WARN/ERROR lines by known category, groups unknowns by module, and shows a per-minute histogram to pinpoint when floods start. |
| `--phase-detail FRAGMENT` | — | Per-phase timing breakdown (phases 0–4b) for every file whose path contains `FRAGMENT`. |
| `--all` | — | Runs phase-summary + stalls (1.0s threshold) + warnings together. |
| `--top N` | 30 | Controls row count in ranked tables. |

### Example output — `--phase-summary`

```
Loading my-run.log … 337,310 timestamped lines
Extracted 1423 file records

======================================================================
  Phase 0 (initialize_stack) duration — top 30 slowest files
======================================================================
  Files analysed :  1422
  Mean           :  2.06s
  Std-dev        :  5.61s
  Min            :  0.03s
  Max            : 49.23s
  Outlier cutoff : 13.29s  (mean + 2σ)

   Duration  Flag   File
  ---------  -----  --------------------------------------------------
     49.23s  >>>    reference/deprecated_and_obsolete_features/index.md
     47.08s  >>>    reference/classes/static_initialization_blocks/index.md
     ...

  86 outlier(s) above 13.29s

======================================================================
  Phase 5 post-processing gaps > 5s (terminate_stack + event fan-out)
======================================================================
        Gap  RelUpdates  File
  ---------  ----------  --------------------------------------------------
    523.84s        1056  reference/trailing_commas/index.md
    498.16s        1019  guide/working_with_objects/index.md
```

### Example output — `--warnings`

```
======================================================================
  WARN / ERROR summary  (8842 total)
======================================================================

  Known warning types:
    Count  Category
  -------  -------------------------------------------------------
     7441  self-connection flood (BN-2)
      278  Issue-34 nodes-in-relations-not-in-states
      256  Duplicate path for single relation
       30  Sort-key sentinel 65535 re-settled

  Warnings per minute (non-zero minutes only):
  22:44    202  ########################################
  22:45      8  #
```

## What to look for

### Phase 0 plateau growing over time

If `--phase-summary` shows Phase 0 durations stepping up in discrete jumps
as the run progresses, the cause is `session_bb` accumulation — each file
re-traverses a larger graph. See `FM1` and `BN-2` in
`.scratchpad/corpus_triage.md`.

Use `--phase-detail <file-just-before-the-step>` to confirm the step boundary.

### Phase 5 silent stalls

Large Phase 5 gaps (visible in the `--phase-summary` table and confirmed with
`--stalls`) indicate that `terminate_stack` is propagating a high number of
`RelationUpdate` events, each triggering expensive downstream work. The
`RelUpdates` column tells you how many were in the diff. See `BN-3` in
`.scratchpad/corpus_triage.md`.

### Self-connection flood

`--warnings` showing thousands of `self-connection flood (BN-2)` hits means a
reflexive Section edge is accumulating in `session_bb`. Each subsequent
`initialize_stack` re-traverses it. The histogram shows which minute the flood
starts, which correlates to the file that created the bad edge.

### Issue-34 violations

`nodes-in-relations-not-in-states` errors mean href/external nodes are in
the relation graph but not in the state map by the time `PathMapMap::new`
runs. These are tracked under ISSUE_34.

### One-path-one-BID enforcement

Two classifiers report on the `PathMap` invariant that a path resolves to
exactly one node. They mean opposite things, so read them separately:

| classifier | meaning | expected count |
|---|---|---|
| `Stub evicted by content-node claim` | working as designed — a content node claimed a URL an `External|Trace` stub held, and the stub was removed | small, or zero |
| `Duplicate path survived to PathMap construction` | **a defect** — two entries reached `PathMap::new` sharing a path, so the write-path guard missed a route | zero |

The first is the enforcement mechanism reporting itself. A steady trickle is
normal on a corpus with unresolved links that are later declared as aliases; a
flood suggests the same URL is being claimed and re-stubbed repeatedly, which
is worth tracing to the document pair involved.

The second should never appear. `path_map` is `FxHashMap<String, usize>` — a
duplicate cannot be represented, so construction keeps the later entry and
discards the earlier one. That is a silent resolution change: a link that used
to reach one node now reaches another. Treat any occurrence as a bug in
whichever write path produced the duplicate, not as a tuning parameter.

Both are on `noet_core::paths::collision` at `warn`, so they appear at any log
level and need no special `RUST_LOG`.

## Adding new warning classifiers

`parse_log.py` contains a `_WARN_CLASSIFIER` list near the top of the file:

```python
_WARN_CLASSIFIER = [
    ("self-connection", "self-connection flood (BN-2)"),
    ("ISSUE 34 VIOLATION", "Issue-34 nodes-in-relations-not-in-states"),
    ...
]
```

Each entry is `(substring_to_match, human_readable_label)`. Add new entries
here as new warning patterns are identified.

## `analyze_phase2.py`

**Requirements:** Python 3.10+, no third-party packages.

Analyzes `[cache_fetch] slow` entries and `[Phase 2] push_relation loop complete`
summary lines from a corpus run log. Useful for diagnosing warm-session slowdowns
where Phase 2 `cache_fetch` dominates wall time.

```sh
python3 benches/log_analysis/analyze_phase2.py my-run.log
python3 benches/log_analysis/analyze_phase2.py my-run.log --top 20
```

### Sections produced

| Section | Description |
|---------|-------------|
| `[1] slow` | `[cache_fetch] slow` entries: total count, top N by `elapsed_ms`, breakdown by subnet and `source=`, `session_bb_nodes` distribution |
| `[2] phase2` | `[Phase 2]` summary lines: top N by `cache_fetch_ms`, by `n_cache_arm`, by `neighborhood_total_ms`, and per-document grouped view |
| `[3] warns` | Brief WARN/ERROR summary, re-parse MISS breakdown by task and key ID, `cache_fetch FAILED` count, MISS by `parse_number` |
| `[4] parse_number` | Overall count of log lines carrying `parse_number=1/2/3` |
| `[5] aggregate` | Summed totals for `n_push_relation`, `n_cache_arm`, `cache_fetch_ms`, `phase2_total_ms`, etc., plus `cache_fetch` share of Phase 2 wall time |
| `[6] histogram` | `elapsed_ms` bucket distribution for all slow entries, with median and mean |

### What to look for

**High `cache_fetch` share of phase2 (>80%)** — most Phase 2 wall time is spent
waiting on `global_bb` mutex locks. The fix is per-document submap seeding before
task spawn (see `.scratchpad/session_bb_submap_seeding.md`).

**`neighborhood_total_ms = 0` everywhere** — `initialize_stack` is not the
bottleneck; Phase 2 `push_relation` is. Confirms submap seeding is the right lever.

**High `n_cache_arm` on pass 2+** — these are warm-session GlobalCache hits for
nodes the task's `session_bb` doesn't have. Each one acquires the `global_bb`
mutex. The count directly predicts the benefit of submap seeding.

---

## `analyze_warns.py`

**Requirements:** Python 3.10+, no third-party packages.

Groups all WARN lines by normalized message pattern, collapsing variable fields
(BIDs, UUIDs, `parse_number`, `keys=[...]`, paths) into placeholders. Shows a
count-sorted list with one representative example per pattern.

```sh
python3 benches/log_analysis/analyze_warns.py my-run.log
```

Use this to identify dominant WARN categories (e.g. repeated re-parse MISSes for
the same unresolvable ID) before diving into raw log searches.

---

## `analyze_pathmap.py`

**Requirements:** Python 3.10+, no third-party packages.

Summarizes `PathMap` read and write cost per network. Unlike the other tools
here, this one needs two specific tracing targets enabled rather than blanket
`RUST_LOG=debug`:

```sh
RUST_LOG='info,noet_core::paths::scan=debug,noet_core::paths::perf=debug' \
    noet parse --db --no-progress --jobs 1 <corpus> > run.log 2>&1
python3 benches/log_analysis/analyze_pathmap.py run.log
python3 benches/log_analysis/analyze_pathmap.py run.log --net 7232a397e404
```

| Flag | Default | Description |
|------|---------|-------------|
| `--net BREF` | all | Restrict the report to a single network |
| `--top N` | 10 | Row count in the ranked tables |

**Scan cost** (`indexed_get`) is the regression sentinel for the path index
(`PathMap::path_map`). Since that index landed, `avg` should be 0–1 for every
network. A climbing average means the index desynchronised or a caller
reintroduced a linear scan — before the index, the asset namespace averaged
2,280 entries scanned per lookup and 359M per corpus run.

**Insert shift cost** (`map_insert`) should be **0** for the href and asset
namespaces: `assign_sort_key` issues monotonic keys per `(sink, kind)`, so
their children tail-append by construction. A non-zero const-namespace total
means sort keys are re-entering an already-occupied range. The `key_regr`
column counts that directly. Content networks legitimately show non-zero shift
from ordinary document churn.

Note the scan counter fires once per subnet-recursion level, so `calls` counts
records rather than logical lookups. Flat networks (including both
const-namespaces today) are 1:1.

---

## `analyze_finalize_html.py`

**Requirements:** Python 3.10+, no third-party packages.

Attributes `finalize_html`'s wall-clock time to its constituent stages.

```sh
RUST_LOG=debug cargo run --features service,bin -- parse \
    --html-output /tmp/out <corpus> > run.log 2>&1
python3 benches/log_analysis/analyze_finalize_html.py run.log
```

Reports each stage's `elapsed_ms` and share of the `finalize_html` total
(`generate_deferred_html`, `generate_sitemap`, asset-manifest submap,
`create_asset_hardlinks`, `export_beliefgraph`, the post-export
`BeliefBase::from(graph)` rebuild, `compute_layout_metadata`,
`build_search_indices`, `export_beliefbase`, `generate_spa_shell`), plus
whether the wall span from "Asset hardlinks created" to the last stage event
is fully accounted for — a nonzero "unaccounted" figure means a stage still
isn't bracketed.

A *negative* unaccounted figure is not a bug: it means instrumented stages sum
to more than the wall span, i.e. some stage overlaps others concurrently. Read
it as "the stage sum overstates the serial critical path".

### `compute_layout_metadata` detail

When the log contains them, the tool also breaks that stage into its eight
internal steps and splits scope resolution across `indexed_path`'s two routes:

- **indexed** — candidates narrowed via `node_to_nets` plus the subnet-ancestor
  index. A few probes per call.
- **fallback** — BID absent from `node_to_nets`, so *every* network is probed.

Both sections are optional and older logs still analyse without them.

The ratio to watch is **probes, not calls**. A fallback rate that looks
negligible by call count can be the majority of actual work, because each
fallback call probes `N_networks` times while an indexed call probes ~1–2. The
tool prints both shares and says which one to attack:

- fallback dominating probes → reduce the number of BIDs missing from
  `node_to_nets`; narrowing the indexed route further buys nothing.
- indexed dominating → cost is per-probe, inside `PathMap::path` itself, not in
  candidate-set width.

This distinction exists because the stage was twice "optimised" against the
wrong term: a narrowing that cut candidates 5.5x left wall time unchanged,
because the candidates removed were the cheap ones.

---

## `analyze_cache_fetch.py`

**Requirements:** Python 3.10+, no third-party packages.

Summarizes `GraphBuilder::cache_fetch` resolution outcomes and, for calls that
fell through to `global_bb`/DB, `session_bb`'s local PathMap size at that
moment (Issue 97 Bottlenecks 4 and 5 — both show local PathMaps far smaller
than the authoritative membership, previously only inferred from timing
distribution).

```sh
RUST_LOG='info,noet_core::codec::cache_fetch_census=debug' \
    noet parse --db --no-progress <corpus> > run.log 2>&1
python3 benches/log_analysis/analyze_cache_fetch.py run.log
```

Reports the `source=` outcome mix (`SourceFile` / `StackCache` / `GlobalCache`
/ `Generated`) with per-source mean/p95 latency, the direct `GlobalCache`
fall-through rate (the number Bottleneck 5's "99.9% miss locally" was
inferred from), and — for `GlobalCache` outcomes only — a time series of
`session_bb_nodes` vs. `session_bb_asset_len`/`session_bb_href_len`. A rising
node count alongside a flat local-map length is the signature both
bottlenecks describe.

This probe is deliberately excluded from blanket `RUST_LOG=debug`: it fires on
every `cache_fetch` call, one of the hottest call sites in Phase 2.

---

## `analyze_seed_session.py`

**Requirements:** Python 3.10+, no third-party packages. Needs `--jobs N>1`.

Attributes the pre-spawn epoch seeding cost (Issue 97 Bottleneck 7 — a bimodal
gap between a task's `Initializing GraphBuilder` line and its `[parse_epoch]
task seeded` line: 439 tasks <10 ms, 1,097 tasks >5 s, with nothing logged in
between to say why).

```sh
RUST_LOG=debug noet parse --jobs 4 --no-progress <corpus> > run.log 2>&1
python3 benches/log_analysis/analyze_seed_session.py run.log
```

### Two seeding paths

The tool prints which one produced the log, and reads both:

- **`seed_session_from_base`** (current) — the epoch builds one `BeliefBase`
  and each task clones it. Cost splits into `base_clone_us` (pointer copies)
  and `merge_us` (merging the per-document seed).
- **`seed_session`** (legacy) — each task rebuilt a private `BeliefBase`. Cost
  splits into `union_us`, `clone_us`, and `rebuild_us`. Retained so older logs
  still analyse, and because epoch-0 still takes this path.

On the legacy path an "(unattributed)" share above ~20% means the cost is *not*
in the three timed sub-steps — the probes are bracketing the wrong window.

If neither section appears on a `--jobs N>1` log, the marker names have drifted
from the code again — check those before concluding seeding was cheap.

### Per-epoch probes

- **`epoch_session_snapshot`** (per epoch, fully serial before any task spawns,
  so its total *is* wall clock) — attributes cost across the network scan, the
  const-namespace BFS, index anchors, the state clone, and the edge filter.
- **`shared session base`** (current path) — the once-per-epoch base build.
  This is what the per-task rebuild was replaced *by*; it should be a small
  multiple of one task's old rebuild, not of all tasks'.

### PathMap copy-on-write — regression sentinel

Sharing the epoch base is only cheap while `PathMapMap::make_pathmap_unique`
rarely has to copy. The tool reports the copy rate overall and **per epoch**
(the cumulative total hides a rate that degrades late in a run), warns above
10%, and flags thrashing above 50%. Healthy baseline is ~2.8%.

A high rate means sharing has become slower than the per-task rebuild it
replaced. The mechanism and its failure mode are documented on
`make_pathmap_unique` in `src/paths/pathmap.rs`; the short version is that any
read guard held across a write inflates `Arc::strong_count` and forces a copy
that was not needed. Such a guard can be introduced anywhere in the parse or
path layers, so check this after work in those areas, not only after work on
sharing itself.

---

## `analyze_cpp_parse.py`

**Requirements:** Python 3.10+, no third-party packages.

Correlates C++ tree-sitter parse cost against file size and include count
(Issue 97 Bottleneck 3 — 211 silent gaps >20s, ~163 min total, clustered in
the C++ corpus; larger in aggregate than Bottleneck 2 but spread across many
files, so more likely genuine parse cost than a defect). This tool exists to
check that hypothesis rather than assume it.

Run the downstream C++ codec's own parse CLI with `info` level plus debug-level
tracing on its `cpp::perf` target and an HTML output path, redirecting combined
stdout/stderr to a log file, then feed that log to this script:

```sh
python3 benches/log_analysis/analyze_cpp_parse.py run.log
```

Reports the top files by tree-sitter parse time, a stage breakdown
(tree-sitter parse vs. symbol/span extraction vs. include collection — to
tell genuine tree-sitter cost apart from the surrounding codec code), a
linear fit of parse time vs. `content_len` and vs. `n_includes` with R², and
a ranked ms-per-KB table to surface outliers relative to their own file size.
A poor linear fit with residuals growing in file size would indicate
superlinear behavior worth investigating further; tree-sitter is expected to
be roughly linear for well-formed C++.

Note this tool reads from a downstream C++ codec crate, not noet-core — it is
only relevant for corpora containing `.h`/`.cpp` files handled by that codec's
`CppCodec`.

---

## `url_depth_sweep.py`

**Requirements:** Python 3.10+, no third-party packages.

Models const-namespace hub degree under different URL-nesting depths. Reads
URLs on stdin or from a file:

```sh
# From markdown sources
grep -rhoE 'https?://[^)"<> ]+' build --include='*.md' | sort -u \
    | python3 benches/log_analysis/url_depth_sweep.py

# From a namespace dump (NOET_DUMP_NAMESPACES=<path> on a parse run)
awk -F'\t' '$1=="href"{print $2}' ns.tsv \
    | python3 benches/log_analysis/url_depth_sweep.py
```

The **max container** column is the one that matters: worst-case Section-child
count of any one namespace node, i.e. the hub degree that makes 1-hop halo
traversal fan out. The relative-cost column ranks groupings but does not
predict wall time — measured insert shift for const-namespaces is 0.

---

## Relationship to benchmarks

These are **log-analysis** tools for diagnosing performance problems observed
during corpus runs. They are distinct from the Criterion benchmarks in
`document_processing.rs` and `macro_benchmarks.rs`, which measure throughput
under controlled conditions.

The typical workflow is:

1. Run `macro_benchmarks.rs` to get a throughput number.
2. If throughput is poor, capture a `RUST_LOG=debug` log with the corpus run
   command above.
3. Use `parse_log.py --all` to identify which phase and which files are slow.
4. Fix the bottleneck, re-run step 1 to confirm improvement.
