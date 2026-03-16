#!/usr/bin/env python3
"""
parse_log.py — Analyse a noet corpus-run debug log.

Extracts timing information from RUST_LOG=debug output produced by:

    cargo run --features service,bin -- --color=always parse \\
        --html-output /tmp/bench-output <corpus_path> 2>&1 | tee run.log

Usage
-----
    python3 benches/log_analysis/parse_log.py run.log
    python3 benches/log_analysis/parse_log.py run.log --phase-summary
    python3 benches/log_analysis/parse_log.py run.log --stalls 2.0
    python3 benches/log_analysis/parse_log.py run.log --warnings
    python3 benches/log_analysis/parse_log.py run.log --file-times
    python3 benches/log_analysis/parse_log.py run.log --concurrency
    python3 benches/log_analysis/parse_log.py run.log --all

Output modes
------------
--phase-summary (default)
    Per-file Phase 0 duration (initialize_stack → [initialize_stack]:)
    sorted descending, with mean/min/max.  Highlights files that are
    statistical outliers (> mean + 2σ).

--stalls SECONDS
    Every gap between consecutive log lines that exceeds SECONDS with
    context lines before and after.  Default threshold: 1.0 s.

    With parallel logs, gaps between lines are expected (other tasks are
    running).  Use --jobs N to suppress false stalls from concurrent tasks.

--warnings
    Count and group WARN/ERROR lines by module path.  Shows the top-N
    warning types and total counts.  Useful for tracking self-connection
    floods, Issue-34 violations, etc.

--phase-detail FILE_FRAGMENT
    Show per-phase timing breakdown for all files whose path contains
    FILE_FRAGMENT.

--file-times
    Total parse time per file (Phase 0 start → Phase 5 end), ranked
    slowest-first with mean/stddev/outlier flagging.  Also shows
    per-attempt breakdown for files parsed more than once.  Includes a
    linear trend fit (OLS over parse order) to detect O(N) parse-time
    growth.

    With parallel logs the "Sum (sequential)" is shown alongside an
    estimated wall-clock time derived from actual task overlaps.

--concurrency
    Concurrency histogram: for each 100 ms bucket show how many
    parse_task spans were active simultaneously (Phase 0 start →
    Phase 5 end).  Also prints peak and mean concurrency.  Useful for
    validating that --jobs N is actually letting N tasks run.

--all
    Run all analyses.
"""

from __future__ import annotations

import argparse
import heapq
import math
import re
import sys
from collections import Counter, defaultdict
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Optional

# ---------------------------------------------------------------------------
# Timestamp / log-line parsing
# ---------------------------------------------------------------------------

_TS_RE = re.compile(r"^(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d+)Z")
_ANSI_RE = re.compile(r"\x1b\[[0-9;]*m")
_LEVEL_RE = re.compile(r"\s+(DEBUG|INFO|WARN|ERROR)\s+")

# Matches the span prefix emitted by .instrument(info_span!("parse_task", ...))
# Example (after ANSI stripping):
#   parse_task{task_idx=3 path=/some/path}: noet_core::codec::builder: Phase 0 ...
_SPAN_RE = re.compile(r"parse_task\{task_idx=(\d+)\s+path=([^}]+)\}:")


def _strip_ansi(s: str) -> str:
    return _ANSI_RE.sub("", s)


def _parse_ts(line: str) -> Optional[datetime]:
    m = _TS_RE.match(line)
    if not m:
        return None
    return datetime.fromisoformat(m.group(1)).replace(tzinfo=timezone.utc)


@dataclass
class LogLine:
    ts: datetime
    level: str  # DEBUG / INFO / WARN / ERROR / ""
    module: str  # e.g. "noet_core::codec::builder"
    body: str  # remainder after module
    raw: str  # original (ANSI-stripped) line
    # Populated when the line came from inside a parse_task span:
    task_idx: Optional[int] = None
    task_path: Optional[str] = None


def _parse_line(raw_line: str) -> Optional[LogLine]:
    line = _strip_ansi(raw_line.rstrip())
    ts = _parse_ts(line)
    if ts is None:
        return None
    m = _LEVEL_RE.search(line)
    if not m:
        return None
    level = m.group(1)
    after_level = line[m.end() :]

    # Extract span context if present (appears before the module path).
    task_idx: Optional[int] = None
    task_path: Optional[str] = None
    span_m = _SPAN_RE.search(after_level)
    if span_m:
        task_idx = int(span_m.group(1))
        task_path = span_m.group(2).strip()
        # Advance past the span prefix so module/body parsing sees the rest.
        after_level = after_level[span_m.end() :].lstrip()

    colon = after_level.find(": ")
    if colon == -1:
        module = after_level.strip()
        body = ""
    else:
        module = after_level[:colon].strip()
        body = after_level[colon + 2 :]

    return LogLine(
        ts=ts,
        level=level,
        module=module,
        body=body,
        raw=line,
        task_idx=task_idx,
        task_path=task_path,
    )


def load_log(path: str) -> list[LogLine]:
    lines = []
    with open(path, errors="replace") as fh:
        for raw in fh:
            ll = _parse_line(raw)
            if ll is not None:
                lines.append(ll)
    return lines


# ---------------------------------------------------------------------------
# Phase-timing extraction
# ---------------------------------------------------------------------------

_PHASE_LABELS = {
    "Phase 0: initialize stack": "phase0_start",
    "[initialize_stack]:": "phase0_end",
    "Phase 1: Create all nodes": "phase1",
    "Phase 2: Balance and process relations": "phase2",
    "Phase 3: inform external sinks": "phase3",
    "Phase 4: context injection": "phase4",
    "Phase 4b: codec finalization": "phase4b",
    "Phase 5: terminating stack": "phase5",
}

_QUEUEING_RE = re.compile(r'Queueing for deferred HTML generation: "(.+)"')
_WRITE_RE = re.compile(r'Write disabled, skipping file write for "(.+)"')
_DIFF_RE = re.compile(r"Diff events \((\d+)\).*RelationUpdate\((\d+)\)")
# Legacy sequential path — still emitted by parse_next for non-parallel runs.
_PARSING_FILE_RE = re.compile(r"\[Compiler\] Parsing file (.+?) \(attempt (\d+)/")


@dataclass
class FileRecord:
    path: str = ""
    phases: dict[str, datetime] = field(default_factory=dict)
    diff_total: int = 0
    diff_relation_updates: int = 0
    parse_start: Optional[datetime] = None
    attempt: int = 1
    # Set for records extracted from parse_task spans:
    task_idx: Optional[int] = None

    def phase0_duration(self) -> Optional[float]:
        p0 = self.phases.get("phase0_start")
        p0e = self.phases.get("phase0_end")
        if p0 and p0e:
            return (p0e - p0).total_seconds()
        return None

    def phase5_to_next(self, next_p0: Optional[datetime]) -> Optional[float]:
        p5 = self.phases.get("phase5")
        if p5 and next_p0:
            return (next_p0 - p5).total_seconds()
        return None

    def phase_span(self, a: str, b: str) -> Optional[float]:
        ta = self.phases.get(a)
        tb = self.phases.get(b)
        if ta and tb:
            return (tb - ta).total_seconds()
        return None

    def total_duration(self) -> Optional[float]:
        """Phase 0 start → Phase 5 start.  Falls back to parse_start."""
        start = self.phases.get("phase0_start") or self.parse_start
        end = self.phases.get("phase5")
        if start and end:
            return (end - start).total_seconds()
        return None

    def interval(self) -> Optional[tuple[datetime, datetime]]:
        """(phase0_start, phase5) pair for concurrency overlap detection."""
        start = self.phases.get("phase0_start")
        end = self.phases.get("phase5")
        if start and end:
            return (start, end)
        return None


# ---------------------------------------------------------------------------
# Record extraction — span-aware
# ---------------------------------------------------------------------------


def extract_file_records(lines: list[LogLine]) -> list[FileRecord]:
    """
    Build one FileRecord per (task_idx, path) span observed in the log.

    Strategy
    --------
    Lines that carry a parse_task span (task_idx + path) are keyed by
    (task_idx, path).  All phase markers within that span accumulate into
    the matching record.  This is robust to interleaved output from
    concurrent tasks.

    Lines without a span (sequential parse_next path, or global compiler
    messages) fall through to the legacy current-pointer approach: a new
    record is opened on each "[Compiler] Parsing file" message and phase
    markers are attributed to the last-opened record.

    The two populations are merged and returned sorted by phase0_start.
    """
    # Key: (task_idx, path) -> FileRecord  (for span-keyed lines)
    span_records: dict[tuple[int, str], FileRecord] = {}
    # Attempt tracking per path across spans
    attempt_counts: dict[str, int] = {}

    # Legacy sequential tracking
    legacy_records: list[FileRecord] = []
    legacy_current: Optional[FileRecord] = None
    last_file_path = ""

    for ll in lines:
        body = ll.body

        if ll.task_idx is not None and ll.task_path is not None:
            # ── Span-keyed line ──────────────────────────────────────────
            key = (ll.task_idx, ll.task_path)
            if key not in span_records:
                attempt_counts[ll.task_path] = attempt_counts.get(ll.task_path, 0) + 1
                rec = FileRecord(
                    path=ll.task_path,
                    attempt=attempt_counts[ll.task_path],
                    task_idx=ll.task_idx,
                    parse_start=ll.ts,
                )
                span_records[key] = rec

            rec = span_records[key]
            for snippet, phase_key in _PHASE_LABELS.items():
                if snippet in body:
                    if phase_key not in rec.phases:
                        rec.phases[phase_key] = ll.ts
                    break

            if "Diff events" in body:
                dm = _DIFF_RE.search(body)
                if dm:
                    rec.diff_total = int(dm.group(1))
                    rec.diff_relation_updates = int(dm.group(2))

        else:
            # ── Legacy / non-span line ───────────────────────────────────
            pm = _PARSING_FILE_RE.search(body)
            if pm:
                file_path = pm.group(1).strip()
                attempt = int(pm.group(2))
                last_file_path = file_path
                attempt_counts[file_path] = attempt
                legacy_current = FileRecord(
                    path=file_path,
                    parse_start=ll.ts,
                    attempt=attempt,
                )
                legacy_records.append(legacy_current)
                continue

            qm = _QUEUEING_RE.search(body)
            wm = _WRITE_RE.search(body)
            if qm:
                last_file_path = qm.group(1)
            elif wm:
                last_file_path = wm.group(1)

            for snippet, phase_key in _PHASE_LABELS.items():
                if snippet in body:
                    if phase_key == "phase0_start" and legacy_current is None:
                        legacy_current = FileRecord(path=last_file_path)
                        legacy_records.append(legacy_current)
                    if legacy_current is not None:
                        legacy_current.phases[phase_key] = ll.ts
                    break

            if legacy_current is not None and "Diff events" in body:
                dm = _DIFF_RE.search(body)
                if dm:
                    legacy_current.diff_total = int(dm.group(1))
                    legacy_current.diff_relation_updates = int(dm.group(2))

    all_records = list(span_records.values()) + legacy_records

    # Sort by phase0_start (records without one go to the end).
    def _sort_key(r: FileRecord):
        p0 = r.phases.get("phase0_start")
        return p0 if p0 is not None else datetime.max.replace(tzinfo=timezone.utc)

    all_records.sort(key=_sort_key)
    return all_records


# ---------------------------------------------------------------------------
# Analysis: helpers
# ---------------------------------------------------------------------------


def _short_path(full: str) -> str:
    """Return the corpus-relative portion of a path."""
    for marker in ("/javascript/", "/mdn-content/files/", "/en-us/"):
        idx = full.find(marker)
        if idx != -1:
            return full[idx + len(marker) :]
    return Path(full).name or full


def _ols(xs: list[float], ys: list[float]) -> tuple[float, float]:
    n = len(xs)
    sx = sum(xs)
    sy = sum(ys)
    sxx = sum(x * x for x in xs)
    sxy = sum(x * y for x, y in zip(xs, ys))
    denom = n * sxx - sx * sx
    if denom == 0:
        return 0.0, sy / n
    slope = (n * sxy - sx * sy) / denom
    intercept = (sy - slope * sx) / n
    return slope, intercept


def _rss(xs: list[float], ys: list[float], slope: float, intercept: float) -> float:
    return sum((y - (slope * x + intercept)) ** 2 for x, y in zip(xs, ys))


def _fit_models(
    xs: list[float], ys: list[float]
) -> list[tuple[str, float, float, float]]:
    models = []

    xs_linear = xs
    s, i = _ols(xs_linear, ys)
    models.append(("O(N)      ", s, i, _rss(xs_linear, ys, s, i)))

    xs_sq = [x * x for x in xs]
    s, i = _ols(xs_sq, ys)
    models.append(("O(N²)     ", s, i, _rss(xs_sq, ys, s, i)))

    xs_log = [math.log(x + 1) for x in xs]
    s, i = _ols(xs_log, ys)
    models.append(("O(log N)  ", s, i, _rss(xs_log, ys, s, i)))

    xs_nlogn = [x * math.log(x + 1) for x in xs]
    s, i = _ols(xs_nlogn, ys)
    models.append(("O(N log N)", s, i, _rss(xs_nlogn, ys, s, i)))

    models.sort(key=lambda m: m[3])
    return models


def _trend_bar(slope_ms: float) -> str:
    abs_s = abs(slope_ms)
    if abs_s < 0.05:
        return "── flat"
    direction = "↑" if slope_ms > 0 else "↓"
    if abs_s < 0.5:
        return f"{direction}  mild ({slope_ms:+.3f} ms/file)"
    if abs_s < 2.0:
        return f"{direction}{direction} moderate ({slope_ms:+.3f} ms/file)"
    return f"{direction}{direction}{direction} STRONG ({slope_ms:+.3f} ms/file)"


def _wall_clock_estimate(records: list[FileRecord]) -> Optional[float]:
    """
    Estimate actual wall-clock elapsed from the union of all parse intervals.

    Merges overlapping (phase0_start, phase5) intervals and sums the gaps.
    Returns None if fewer than 2 records have complete intervals.
    """
    intervals = sorted(
        (iv for r in records for iv in ([r.interval()] if r.interval() else [])),
        key=lambda x: x[0],
    )
    if not intervals:
        return None

    merged: list[tuple[datetime, datetime]] = []
    cur_start, cur_end = intervals[0]
    for start, end in intervals[1:]:
        if start <= cur_end:
            cur_end = max(cur_end, end)
        else:
            merged.append((cur_start, cur_end))
            cur_start, cur_end = start, end
    merged.append((cur_start, cur_end))

    return sum((e - s).total_seconds() for s, e in merged)


# ---------------------------------------------------------------------------
# Analysis: total parse time per file
# ---------------------------------------------------------------------------


def report_file_times(records: list[FileRecord], top_n: int = 30) -> None:
    timed = [(r.total_duration(), r) for r in records if r.total_duration() is not None]
    if not timed:
        print("No total parse timing data found.")
        return

    vals = [d for d, _ in timed]
    mean = sum(vals) / len(vals)
    variance = sum((v - mean) ** 2 for v in vals) / len(vals)
    sigma = math.sqrt(variance)
    threshold = mean + 2 * sigma

    def _epoch_ordered(attempt_pred):
        return sorted(
            [
                (r.total_duration(), r)
                for r in records
                if r.total_duration() is not None
                and r.phases.get("phase0_start") is not None
                and attempt_pred(r.attempt)
            ],
            key=lambda t: t[1].phases["phase0_start"],
        )

    epochs = [
        ("attempt 1  (fresh parse)", _epoch_ordered(lambda a: a == 1)),
        ("attempt 2+ (reparse)    ", _epoch_ordered(lambda a: a > 1)),
    ]

    def _print_fit(epoch_label: str, ordered: list) -> None:
        if len(ordered) < 2:
            if ordered:
                print(
                    f"\n  Complexity fit — {epoch_label}: only {len(ordered)} record(s), skipping fit."
                )
            return
        xs = [float(i) for i in range(len(ordered))]
        ys = [d for d, _ in ordered]
        models = _fit_models(xs, ys)
        lin = next(m for m in models if m[0].startswith("O(N)"))
        slope_ms = lin[1] * 1000.0
        intercept_ms = lin[2] * 1000.0
        pred_last = intercept_ms + slope_ms * (len(ordered) - 1)
        best_label = models[0][0].strip()
        print(f"\n  Complexity fit — {epoch_label} ({len(ordered)} records):")
        print(f"    Best fit   : {best_label}  (lowest residual)")
        print(f"    {'Model':<12}  {'Slope':>12}  {'Intercept':>10}  {'RSS':>14}")
        print(f"    {'-' * 12}  {'-' * 12}  {'-' * 10}  {'-' * 14}")
        for rank, (label, slope, intercept, rss) in enumerate(models):
            marker = "← best" if rank == 0 else ""
            print(
                f"    {label}  {slope * 1000:>+10.4f}ms  {intercept * 1000:>8.1f}ms  {rss:>14.4f}  {marker}"
            )
        print(f"\n    O(N) detail:")
        print(f"      Slope      : {slope_ms:+.3f} ms/file  {_trend_bar(slope_ms)}")
        print(f"      Intercept  : {intercept_ms:.1f} ms  (predicted cost of file #0)")
        print(
            f"      Predicted  : {intercept_ms:.0f} ms → {pred_last:.0f} ms  (first → last file)"
        )

    timed.sort(key=lambda t: t[0], reverse=True)

    total_sequential = sum(vals)
    wall_clock = _wall_clock_estimate(records)

    print(f"\n{'=' * 70}")
    print(f"  Total parse time (Phase 0 start → Phase 5) — top {top_n} slowest")
    print(f"{'=' * 70}")
    print(f"  Records analysed    : {len(vals)}")
    print(f"  Mean                : {mean:.2f}s")
    print(f"  Std-dev             : {sigma:.2f}s")
    print(f"  Min                 : {min(vals):.2f}s")
    print(f"  Max                 : {max(vals):.2f}s")
    print(f"  Outlier cutoff      : {threshold:.2f}s  (mean + 2σ)")
    print(
        f"  Sum (sequential)    : {total_sequential:.0f}s  ({total_sequential / 3600:.2f}h)"
    )
    if wall_clock is not None:
        speedup = total_sequential / wall_clock if wall_clock > 0 else float("inf")
        print(
            f"  Wall-clock (merged) : {wall_clock:.0f}s  ({wall_clock / 3600:.2f}h)  ×{speedup:.1f} speedup"
        )
    for epoch_label, ordered in epochs:
        _print_fit(epoch_label, ordered)
    print()

    print(f"  {'Duration':>9}  {'Att':>3}  {'Tidx':>5}  {'Flag':<5}  File")
    print(f"  {'-' * 9}  {'-' * 3}  {'-' * 5}  {'-' * 5}  {'-' * 50}")
    for dur, rec in timed[:top_n]:
        flag = ">>>" if dur > threshold else "   "
        short = _short_path(rec.path)
        tidx = f"{rec.task_idx}" if rec.task_idx is not None else "seq"
        print(f"  {dur:>8.2f}s  {rec.attempt:>3}  {tidx:>5}  {flag}    {short}")

    outliers = sum(1 for v in vals if v > threshold)
    if outliers:
        print(f"\n  {outliers} outlier(s) above {threshold:.2f}s")

    attempt_dist: Counter = Counter(r.attempt for _, r in timed)
    if max(attempt_dist.keys()) > 1:
        print(f"\n  Parse attempt distribution:")
        for att in sorted(attempt_dist):
            print(f"    attempt {att}: {attempt_dist[att]} records")


# ---------------------------------------------------------------------------
# Analysis: Phase 0 summary
# ---------------------------------------------------------------------------


def report_phase_summary(records: list[FileRecord], top_n: int = 30) -> None:
    durations = [
        (r.phase0_duration(), i, r)
        for i, r in enumerate(records)
        if r.phase0_duration() is not None
    ]
    if not durations:
        print("No Phase 0 timing data found.")
        return

    vals = [d for d, _, _ in durations]
    mean = sum(vals) / len(vals)
    variance = sum((v - mean) ** 2 for v in vals) / len(vals)
    sigma = math.sqrt(variance)
    threshold = mean + 2 * sigma

    durations.sort(reverse=True)

    print(f"\n{'=' * 70}")
    print(f"  Phase 0 (initialize_stack) duration — top {top_n} slowest files")
    print(f"{'=' * 70}")
    print(f"  Files analysed : {len(vals)}")
    print(f"  Mean           : {mean:.2f}s")
    print(f"  Std-dev        : {sigma:.2f}s")
    print(f"  Min            : {min(vals):.2f}s")
    print(f"  Max            : {max(vals):.2f}s")
    print(f"  Outlier cutoff : {threshold:.2f}s  (mean + 2σ)")
    print()
    print(f"  {'Duration':>9}  {'Tidx':>5}  {'Flag':<5}  File")
    print(f"  {'-' * 9}  {'-' * 5}  {'-' * 5}  {'-' * 50}")
    for dur, _i, rec in durations[:top_n]:
        flag = ">>>" if dur > threshold else "   "
        short = _short_path(rec.path)
        tidx = f"{rec.task_idx}" if rec.task_idx is not None else "seq"
        print(f"  {dur:>8.2f}s  {tidx:>5}  {flag}    {short}")

    outliers = sum(1 for v in vals if v > threshold)
    if outliers:
        print(f"\n  {outliers} outlier(s) above {threshold:.2f}s")

    # Phase 5 post-processing gaps (only meaningful for sequential records)
    phase5_gaps = []
    for i, rec in enumerate(records):
        next_p0 = (
            records[i + 1].phases.get("phase0_start") if i + 1 < len(records) else None
        )
        gap = rec.phase5_to_next(next_p0)
        if gap is not None and gap > 0:
            phase5_gaps.append((gap, i, rec))

    if phase5_gaps:
        phase5_gaps.sort(reverse=True)
        big = [(g, r) for g, _i, r in phase5_gaps if g > 5.0]
        if big:
            print(f"\n{'=' * 70}")
            print(
                "  Phase 5 post-processing gaps > 5s (terminate_stack + event fan-out)"
            )
            print(f"{'=' * 70}")
            print(f"  {'Gap':>9}  {'RelUpdates':>10}  File")
            print(f"  {'-' * 9}  {'-' * 10}  {'-' * 50}")
            for gap, rec in big[:20]:
                short = _short_path(rec.path)
                print(f"  {gap:>8.2f}s  {rec.diff_relation_updates:>10}  {short}")


# ---------------------------------------------------------------------------
# Analysis: concurrency histogram
# ---------------------------------------------------------------------------


def report_concurrency(records: list[FileRecord], bucket_ms: int = 100) -> None:
    """
    Show how many parse_task spans were active in each time bucket.

    Each record's interval is (phase0_start → phase5).  We bucket the
    entire run into `bucket_ms`-millisecond windows and count how many
    intervals overlap each window.

    Only records from parse_task spans (task_idx is not None) are used,
    because those are the ones that may truly run concurrently.
    """
    intervals = [
        (r.phases["phase0_start"], r.phases["phase5"])
        for r in records
        if r.task_idx is not None and r.interval() is not None
    ]

    if not intervals:
        print("\nNo parse_task span intervals found (no parallelism data).")
        return

    # Compute run boundaries
    run_start = min(s for s, _ in intervals)
    run_end = max(e for _, e in intervals)
    total_secs = (run_end - run_start).total_seconds()
    bucket_secs = bucket_ms / 1000.0
    n_buckets = math.ceil(total_secs / bucket_secs) + 1

    counts = [0] * n_buckets

    for start, end in intervals:
        b_start = int((start - run_start).total_seconds() / bucket_secs)
        b_end = int((end - run_start).total_seconds() / bucket_secs)
        for b in range(b_start, min(b_end + 1, n_buckets)):
            counts[b] += 1

    # Summary stats
    max_conc = max(counts)
    nonzero = [c for c in counts if c > 0]
    mean_conc = sum(nonzero) / len(nonzero) if nonzero else 0.0
    buckets_gt1 = sum(1 for c in counts if c > 1)
    pct_parallel = 100.0 * buckets_gt1 / len(nonzero) if nonzero else 0.0

    print(f"\n{'=' * 70}")
    print(f"  Concurrency histogram  (bucket size: {bucket_ms} ms)")
    print(f"{'=' * 70}")
    print(f"  Span intervals    : {len(intervals)}")
    print(f"  Run duration      : {total_secs:.1f}s")
    print(f"  Peak concurrency  : {max_conc}")
    print(f"  Mean concurrency  : {mean_conc:.2f}  (over active buckets)")
    print(
        f"  Parallel buckets  : {buckets_gt1} / {len(nonzero)}  ({pct_parallel:.1f}% of active time)"
    )
    print()

    # Distribution table: how many buckets at each concurrency level
    conc_dist: Counter = Counter(counts)
    print(f"  Concurrency distribution (active buckets only):")
    print(f"  {'Level':>7}  {'Buckets':>8}  {'% time':>7}  Bar")
    print(f"  {'-' * 7}  {'-' * 8}  {'-' * 7}  {'-' * 40}")
    for level in sorted(conc_dist):
        if level == 0:
            continue
        n = conc_dist[level]
        pct = 100.0 * n / len(nonzero) if nonzero else 0.0
        bar = "#" * min(int(pct), 40)
        print(f"  {level:>7}  {n:>8}  {pct:>6.1f}%  {bar}")

    # Timeline: print one row per second showing peak concurrency in that second
    print(f"\n  Timeline (peak concurrency per second):")
    print(f"  {'Offset':>8}  {'Peak':>5}  Bar")
    print(f"  {'-' * 8}  {'-' * 5}  {'-' * 40}")
    buckets_per_sec = max(1, int(1.0 / bucket_secs))
    n_seconds = math.ceil(total_secs) + 1
    for sec in range(n_seconds):
        b_lo = sec * buckets_per_sec
        b_hi = min(b_lo + buckets_per_sec, n_buckets)
        if b_lo >= n_buckets:
            break
        peak = max(counts[b_lo:b_hi]) if b_lo < b_hi else 0
        if peak == 0:
            continue
        bar = "#" * min(peak * 5, 40)
        print(f"  {sec:>7}s  {peak:>5}  {bar}")


# ---------------------------------------------------------------------------
# Analysis: stall detection
# ---------------------------------------------------------------------------


def report_stalls(
    lines: list[LogLine],
    threshold: float = 1.0,
    context: int = 3,
    jobs: int = 1,
) -> None:
    print(f"\n{'=' * 70}")
    print(f"  Silent stalls > {threshold:.1f}s between consecutive log lines")
    if jobs > 1:
        print(f"  (--jobs {jobs}: gaps may reflect concurrent tasks, not true stalls)")
    print(f"{'=' * 70}")

    stalls_found = 0
    for i in range(1, len(lines)):
        gap = (lines[i].ts - lines[i - 1].ts).total_seconds()
        if gap < threshold:
            continue
        # With parallel logs: if adjacent lines have different task_idx, the
        # gap is a scheduling artifact, not a real stall.  Flag but don't hide.
        prev_task = lines[i - 1].task_idx
        next_task = lines[i].task_idx
        is_task_switch = (
            jobs > 1
            and prev_task is not None
            and next_task is not None
            and prev_task != next_task
        )
        stalls_found += 1
        tag = " [task-switch]" if is_task_switch else ""
        print(f"\n  --- GAP {gap:.2f}s{tag} ---")
        start = max(0, i - context)
        end = min(len(lines), i + context + 1)
        for j in range(start, end):
            marker = ">>>" if j == i else "   "
            ts_str = lines[j].ts.strftime("%H:%M:%S.%f")[:-3]
            tidx = (
                f"[t{lines[j].task_idx}]" if lines[j].task_idx is not None else "     "
            )
            print(f"  {marker} {ts_str}  {tidx}  {lines[j].body[:110]}")

    if stalls_found == 0:
        print(f"  No stalls found above {threshold:.1f}s threshold.")
    else:
        print(f"\n  Total stalls found: {stalls_found}")


# ---------------------------------------------------------------------------
# Analysis: warnings / errors
# ---------------------------------------------------------------------------

_WARN_CLASSIFIER = [
    ("self-connection", "self-connection flood (BN-2)"),
    ("ISSUE 34 VIOLATION", "Issue-34 nodes-in-relations-not-in-states"),
    ("Unresolved relation", "Unresolved relation (sibling not yet parsed)"),
    ("Setting 2 paths", "Duplicate path for single relation"),
    (
        "Path order depth changed",
        "Gateway-tier depth change [u16::MAX→flat] (node reclassified from index.md plane to doc address space; dependents NOT re-queued)",
    ),
    ("Failed to parse", "File skipped (codec error)"),
    ("cache_fetch FAILED", "cache_fetch returned results but key miss"),
    ("No Codec for extension", "Unknown file extension in codec map"),
    ("BatchStart received with", "BatchStart with non-empty pending (compiler bug)"),
    ("parse task panicked", "Spawned parse task panic"),
]


def report_warnings(lines: list[LogLine], top_n: int = 20) -> None:
    warn_lines = [ll for ll in lines if ll.level in ("WARN", "ERROR")]

    print(f"\n{'=' * 70}")
    print(f"  WARN / ERROR summary  ({len(warn_lines)} total)")
    print(f"{'=' * 70}")

    if not warn_lines:
        print("  No warnings or errors found.")
        return

    bucket_counts: Counter[str] = Counter()
    bucket_examples: dict[str, str] = {}
    uncategorised: list[LogLine] = []

    for ll in warn_lines:
        body = ll.body
        matched = False
        for pattern, label in _WARN_CLASSIFIER:
            if pattern in body:
                bucket_counts[label] += 1
                if label not in bucket_examples:
                    bucket_examples[label] = body[:120]
                matched = True
                break
        if not matched:
            uncategorised.append(ll)

    if bucket_counts:
        print(f"\n  Known warning types:")
        print(f"  {'Count':>7}  Category")
        print(f"  {'-' * 7}  {'-' * 55}")
        for label, count in bucket_counts.most_common():
            print(f"  {count:>7}  {label}")

    if uncategorised:
        module_counts: Counter[str] = Counter(ll.module for ll in uncategorised)
        print(f"\n  Uncategorised warnings/errors by module (top {top_n}):")
        print(f"  {'Count':>7}  Module")
        print(f"  {'-' * 7}  {'-' * 55}")
        for module, count in module_counts.most_common(top_n):
            print(f"  {count:>7}  {module}")

    if warn_lines:
        buckets: dict[str, int] = defaultdict(int)
        for ll in warn_lines:
            minute = ll.ts.strftime("%H:%M")
            buckets[minute] += 1
        print(f"\n  Warnings per minute (non-zero minutes only):")
        for minute in sorted(buckets):
            bar = "#" * min(buckets[minute] // 5, 60)
            print(f"  {minute}  {buckets[minute]:>5}  {bar}")


# ---------------------------------------------------------------------------
# Analysis: per-file phase detail
# ---------------------------------------------------------------------------


def report_phase_detail(records: list[FileRecord], fragment: str) -> None:
    matches = [r for r in records if fragment.lower() in r.path.lower()]
    if not matches:
        print(f"\n  No files matching {fragment!r} found.")
        return

    print(f"\n{'=' * 70}")
    print(f"  Phase timing detail for files matching {fragment!r}")
    print(f"{'=' * 70}")

    phase_pairs = [
        ("phase0_start", "phase0_end", "Phase 0 (init stack)  "),
        ("phase0_end", "phase1", "Phase 0→1 gap         "),
        ("phase1", "phase2", "Phase 1 (create nodes)"),
        ("phase2", "phase3", "Phase 2 (balance)     "),
        ("phase3", "phase4", "Phase 3 (ext sinks)   "),
        ("phase4", "phase4b", "Phase 4 (inject ctx)  "),
        ("phase4b", "phase5", "Phase 4b (finalize)   "),
    ]

    for rec in matches:
        short = _short_path(rec.path)
        tidx = f"task_idx={rec.task_idx}" if rec.task_idx is not None else "sequential"
        print(f"\n  {short}  [{tidx}, attempt {rec.attempt}]")
        total = 0.0
        for a, b, label in phase_pairs:
            dur = rec.phase_span(a, b)
            if dur is not None:
                total += dur
                flag = "  ***" if dur > 5.0 else ""
                print(f"    {label}  {dur:7.3f}s{flag}")
        print(f"    {'Total (phases 0-4b)':23}  {total:7.3f}s")
        if rec.diff_total:
            print(
                f"    Diff events: {rec.diff_total} total, "
                f"{rec.diff_relation_updates} RelationUpdates"
            )


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------


def main() -> None:
    ap = argparse.ArgumentParser(
        description="Analyse a noet corpus-run debug log.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=__doc__,
    )
    ap.add_argument("log", help="Path to the log file (e.g. mdn-javascript.log)")
    ap.add_argument(
        "--phase-summary",
        action="store_true",
        help="Per-file Phase 0 duration table (default if no mode is given)",
    )
    ap.add_argument(
        "--stalls",
        metavar="SECONDS",
        type=float,
        nargs="?",
        const=1.0,
        default=None,
        help="Report gaps between log lines exceeding SECONDS (default 1.0)",
    )
    ap.add_argument(
        "--warnings",
        action="store_true",
        help="Summarise WARN/ERROR lines by category",
    )
    ap.add_argument(
        "--phase-detail",
        metavar="FILE_FRAGMENT",
        help="Per-phase breakdown for files whose path contains FILE_FRAGMENT",
    )
    ap.add_argument(
        "--file-times",
        action="store_true",
        help="Total parse time per file (Phase 0 → Phase 5), ranked slowest-first",
    )
    ap.add_argument(
        "--concurrency",
        action="store_true",
        help="Concurrency histogram: active parse_task spans per time bucket",
    )
    ap.add_argument(
        "--bucket-ms",
        type=int,
        default=100,
        help="Bucket size in milliseconds for --concurrency histogram (default 100)",
    )
    ap.add_argument(
        "--all",
        action="store_true",
        help="Run all analyses (phase-summary + stalls + warnings + file-times + concurrency)",
    )
    ap.add_argument(
        "--top",
        type=int,
        default=30,
        help="Number of rows in ranked tables (default 30)",
    )
    ap.add_argument(
        "--jobs",
        type=int,
        default=1,
        help="Number of parallel jobs used for the run (affects stall interpretation)",
    )
    args = ap.parse_args()

    log_path = args.log
    if not Path(log_path).exists():
        print(f"Error: log file not found: {log_path}", file=sys.stderr)
        sys.exit(1)

    print(f"Loading {log_path} …", end=" ", flush=True)
    lines = load_log(log_path)
    print(f"{len(lines):,} timestamped lines")

    records = extract_file_records(lines)

    span_records = sum(1 for r in records if r.task_idx is not None)
    seq_records = len(records) - span_records
    print(
        f"Extracted {len(records)} file records  "
        f"({span_records} from parse_task spans, {seq_records} sequential)"
    )

    any_mode = (
        args.phase_summary
        or args.stalls is not None
        or args.warnings
        or args.phase_detail
        or args.file_times
        or args.concurrency
        or args.all
    )

    if not any_mode or args.phase_summary or args.all:
        report_phase_summary(records, top_n=args.top)

    if args.file_times or args.all:
        report_file_times(records, top_n=args.top)

    if args.concurrency or args.all:
        report_concurrency(records, bucket_ms=args.bucket_ms)

    if args.stalls is not None or args.all:
        threshold = args.stalls if args.stalls is not None else 1.0
        report_stalls(lines, threshold=threshold, jobs=args.jobs)

    if args.warnings or args.all:
        report_warnings(lines, top_n=args.top)

    if args.phase_detail:
        report_phase_detail(records, args.phase_detail)


if __name__ == "__main__":
    main()
