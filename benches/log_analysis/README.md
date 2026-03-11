# Log Analysis Tools

Utilities for analysing `RUST_LOG=debug` output from noet corpus runs.

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