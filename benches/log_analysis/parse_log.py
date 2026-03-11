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

--warnings
    Count and group WARN/ERROR lines by module path.  Shows the top-N
    warning types and total counts.  Useful for tracking self-connection
    floods, Issue-34 violations, etc.

--phase-detail FILE_FRAGMENT
    Show per-phase timing breakdown for all files whose path contains
    FILE_FRAGMENT.

--all
    Run all analyses.
"""

from __future__ import annotations

import argparse
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


# Strip ANSI colour codes that cargo/tracing inject when --color=always is used.
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
    colon = after_level.find(": ")
    if colon == -1:
        module = after_level.strip()
        body = ""
    else:
        module = after_level[:colon].strip()
        body = after_level[colon + 2 :]
    return LogLine(ts=ts, level=level, module=module, body=body, raw=line)


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


@dataclass
class FileRecord:
    path: str = ""
    phases: dict[str, datetime] = field(default_factory=dict)
    diff_total: int = 0
    diff_relation_updates: int = 0

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


def extract_file_records(lines: list[LogLine]) -> list[FileRecord]:
    """
    Walk the log and group phase markers + file-path lines into FileRecord
    objects, one per parsed file.
    """
    records: list[FileRecord] = []
    current: Optional[FileRecord] = None
    last_file_path = ""

    for ll in lines:
        # Track most-recently-seen file path from compiler messages
        qm = _QUEUEING_RE.search(ll.body)
        wm = _WRITE_RE.search(ll.body)
        if qm:
            last_file_path = qm.group(1)
        elif wm:
            last_file_path = wm.group(1)

        # Match phase markers
        for snippet, key in _PHASE_LABELS.items():
            if snippet in ll.body:
                if key == "phase0_start":
                    # Start of a new file record
                    current = FileRecord(path=last_file_path)
                    records.append(current)
                if current is not None:
                    current.phases[key] = ll.ts
                break

        # Diff events line (lives between phase5 and the next file's phase0)
        if current is not None and "Diff events" in ll.body:
            dm = _DIFF_RE.search(ll.body)
            if dm:
                current.diff_total = int(dm.group(1))
                current.diff_relation_updates = int(dm.group(2))

    return records


# ---------------------------------------------------------------------------
# Analysis: Phase 0 summary
# ---------------------------------------------------------------------------


def _short_path(full: str) -> str:
    """Return the corpus-relative portion of a path (after /javascript/ etc.)"""
    for marker in ("/javascript/", "/mdn-content/files/", "/en-us/"):
        idx = full.find(marker)
        if idx != -1:
            return full[idx + len(marker) :]
    return Path(full).name or full


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
    print(f"  {'Duration':>9}  {'Flag':<5}  File")
    print(f"  {'-' * 9}  {'-' * 5}  {'-' * 50}")
    for dur, _i, rec in durations[:top_n]:
        flag = ">>>" if dur > threshold else "   "
        short = _short_path(rec.path)
        print(f"  {dur:>8.2f}s  {flag}    {short}")

    outliers = sum(1 for v in vals if v > threshold)
    if outliers:
        print(f"\n  {outliers} outlier(s) above {threshold:.2f}s")

    # Phase 5 post-processing time (time from Phase 5 log to next Phase 0)
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
# Analysis: stall detection
# ---------------------------------------------------------------------------


def report_stalls(
    lines: list[LogLine],
    threshold: float = 1.0,
    context: int = 3,
) -> None:
    print(f"\n{'=' * 70}")
    print(f"  Silent stalls > {threshold:.1f}s between consecutive log lines")
    print(f"{'=' * 70}")

    stalls_found = 0
    for i in range(1, len(lines)):
        gap = (lines[i].ts - lines[i - 1].ts).total_seconds()
        if gap < threshold:
            continue
        stalls_found += 1
        print(f"\n  --- GAP {gap:.2f}s ---")
        start = max(0, i - context)
        end = min(len(lines), i + context + 1)
        for j in range(start, end):
            marker = ">>>" if j == i else "   "
            ts_str = lines[j].ts.strftime("%H:%M:%S.%f")[:-3]
            print(f"  {marker} {ts_str}  {lines[j].body[:120]}")

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
    ("Path order depth changed", "Sort-key sentinel 65535 re-settled"),
    ("Failed to parse", "File skipped (codec error)"),
    ("cache_fetch FAILED", "cache_fetch returned results but key miss"),
    ("No Codec for extension", "Unknown file extension in codec map"),
]


def report_warnings(lines: list[LogLine], top_n: int = 20) -> None:
    warn_lines = [ll for ll in lines if ll.level in ("WARN", "ERROR")]

    print(f"\n{'=' * 70}")
    print(f"  WARN / ERROR summary  ({len(warn_lines)} total)")
    print(f"{'=' * 70}")

    if not warn_lines:
        print("  No warnings or errors found.")
        return

    # Classify into known buckets
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

    # Group uncategorised by module
    if uncategorised:
        module_counts: Counter[str] = Counter(ll.module for ll in uncategorised)
        print(f"\n  Uncategorised warnings/errors by module (top {top_n}):")
        print(f"  {'Count':>7}  Module")
        print(f"  {'-' * 7}  {'-' * 55}")
        for module, count in module_counts.most_common(top_n):
            print(f"  {count:>7}  {module}")

    # Timeline: warn rate per minute
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
        print(f"\n  {short}")
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
        "--all",
        action="store_true",
        help="Run all analyses (phase-summary + stalls + warnings)",
    )
    ap.add_argument(
        "--top",
        type=int,
        default=30,
        help="Number of rows in ranked tables (default 30)",
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
    print(f"Extracted {len(records)} file records")

    any_mode = (
        args.phase_summary
        or args.stalls is not None
        or args.warnings
        or args.phase_detail
        or args.all
    )

    if not any_mode or args.phase_summary or args.all:
        report_phase_summary(records, top_n=args.top)

    if args.stalls is not None or args.all:
        threshold = args.stalls if args.stalls is not None else 1.0
        report_stalls(lines, threshold=threshold)

    if args.warnings or args.all:
        report_warnings(lines, top_n=args.top)

    if args.phase_detail:
        report_phase_detail(records, args.phase_detail)


if __name__ == "__main__":
    main()
