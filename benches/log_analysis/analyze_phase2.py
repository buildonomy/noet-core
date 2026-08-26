#!/usr/bin/env python3
"""Analyze Phase 2 cache_fetch timing in a noet RUST_LOG=debug corpus run log.

Sections produced:
  [1] slow    — [cache_fetch] slow entries: top by elapsed_ms, subnet breakdown
  [2] phase2  — [Phase 2] push_relation loop complete: per-doc grouped stats
  [3] warns   — WARN / ERROR brief summary and re-parse MISS breakdown
  [4] parse_number — overall parse_number=N distribution
  [5] aggregate — aggregate Phase 2 metric totals
  [6] histogram — elapsed_ms distribution for slow entries

Usage:
  python3 benches/log_analysis/analyze_phase2.py <log_file> [--top N]
"""

import argparse
import re
import statistics
from collections import Counter, defaultdict

# ── helpers ──────────────────────────────────────────────────────────────────

ANSI_ESC = re.compile(r"\x1b\[[0-9;]*m")
SEP = "=" * 70
_BUILD_PREFIX_RE = re.compile(r".*/corpus/build/")


def load_lines(log_path: str) -> list[str]:
    raw = open(log_path, encoding="utf-8", errors="replace").readlines()
    return [ANSI_ESC.sub("", l) for l in raw]


def strip_ansi(s: str) -> str:
    return ANSI_ESC.sub("", s)


def short_path(p: str) -> str:
    return _BUILD_PREFIX_RE.sub("", p)


def extract_kv_ints(line: str) -> dict[str, int]:
    return {m.group(1): int(m.group(2)) for m in re.finditer(r"(\w+)=(-?\d+)", line)}


# ── section parsers ───────────────────────────────────────────────────────────


def _parse_slow_entries(lines: list[str]) -> list[dict]:
    slow_lines = [l for l in lines if "[cache_fetch] slow" in l]
    entries = []
    for line in slow_lines:
        task_m = re.search(r"path=(/[^\s}]+)", line)
        task_path = short_path(task_m.group(1)) if task_m else "?"

        ms_m = re.search(r"elapsed_ms=(\d+)", line)
        ms = int(ms_m.group(1)) if ms_m else 0

        src_m = re.search(r"source=(\w+)", line)
        src = src_m.group(1) if src_m else "?"

        sbn_m = re.search(r"session_bb_nodes=(\d+)", line)
        sbn = int(sbn_m.group(1)) if sbn_m else 0

        entries.append(
            {"path": task_path, "ms": ms, "source": src, "sbn": sbn, "_line": line}
        )
    return entries


def _parse_phase2(lines: list[str]) -> list[dict]:
    records = []
    for line in lines:
        if "[Phase 2] push_relation loop complete" not in line:
            continue
        kv = extract_kv_ints(line)
        fp_m = re.search(r"file_path=(/\S+)", line)
        if fp_m:
            kv["file_path"] = short_path(fp_m.group(1))
        records.append(kv)
    return records


# ── report sections ───────────────────────────────────────────────────────────


def section_slow(lines: list[str], entries: list[dict], top: int) -> None:
    slow_lines = [l for l in lines if "[cache_fetch] slow" in l]
    print(SEP)
    print(f"[1] Total [cache_fetch] slow lines: {len(slow_lines)}")

    print()
    print(f"[1a] TOP {top} slow entries by elapsed_ms:")
    for e in sorted(entries, key=lambda x: x["ms"], reverse=True)[:top]:
        print(f"  {e['ms']:>6}ms  src={e['source']}  sbn={e['sbn']}  task={e['path']}")

    print()
    print("[1b] Count by top-level subnet (from task path):")
    subnet_counts: Counter = Counter()
    for e in entries:
        top_key = e["path"].split("/")[0]
        subnet_counts[top_key] += 1
    for k, v in subnet_counts.most_common():
        print(f"  {v:>5}  {k}")

    print()
    print("[1c] Count by source=:")
    for k, v in Counter(e["source"] for e in entries).most_common():
        print(f"  {v:>5}  {k}")

    print()
    has_parse_num = [l for l in slow_lines if "parse_number" in l]
    print(
        f"[1d] [cache_fetch] slow lines with parse_number field: {len(has_parse_num)}"
    )

    node_counts = [e["sbn"] for e in entries]
    if node_counts:
        print(
            f"[1e] session_bb_nodes: min={min(node_counts)}  "
            f"max={max(node_counts)}  median={statistics.median(node_counts)}"
        )
        print(
            f"     sbn==12: {sum(1 for x in node_counts if x == 12)}   "
            f"sbn==4933-4935: {sum(1 for x in node_counts if 4900 <= x <= 5000)}   "
            f"sbn>1000: {sum(1 for x in node_counts if x > 1000)}"
        )


def section_phase2(phase2: list[dict], top: int) -> None:
    print()
    print(SEP)
    print(f"[2] Total [Phase 2] summary lines: {len(phase2)}")

    print()
    print(f"[2a] TOP {top} by cache_fetch_ms:")
    for r in sorted(phase2, key=lambda x: x.get("cache_fetch_ms", 0), reverse=True)[
        :top
    ]:
        fp = r.get("file_path", "?")
        print(
            f"  cache_fetch={r.get('cache_fetch_ms', '?'):>6}ms  "
            f"phase2={r.get('phase2_total_ms', '?'):>6}ms  "
            f"n_cache_arm={r.get('n_cache_arm', '?'):>5}  "
            f"n_push_rel={r.get('n_push_relation', '?'):>6}  {fp}"
        )

    print()
    print(f"[2b] TOP {top} by n_cache_arm (global_bb hits):")
    for r in sorted(phase2, key=lambda x: x.get("n_cache_arm", 0), reverse=True)[:top]:
        fp = r.get("file_path", "?")
        print(
            f"  n_cache_arm={r.get('n_cache_arm', '?'):>5}  "
            f"cache_fetch={r.get('cache_fetch_ms', '?'):>6}ms  "
            f"n_push_rel={r.get('n_push_relation', '?'):>6}  {fp}"
        )

    print()
    print(f"[2c] TOP {top} by neighborhood_total_ms:")
    for r in sorted(
        phase2, key=lambda x: x.get("neighborhood_total_ms", 0), reverse=True
    )[:top]:
        ntms = r.get("neighborhood_total_ms", 0)
        fp = r.get("file_path", "?")
        print(
            f"  neighborhood_total={ntms:>6}ms  "
            f"n_hits={r.get('neighborhood_n_hits', '?'):>4}  "
            f"n_misses={r.get('neighborhood_n_misses', '?'):>4}  {fp}"
        )

    print()
    print(
        f"[2d] Per-document summary grouped by file (top {top} by max cache_fetch_ms):"
    )
    by_fp: dict[str, list] = defaultdict(list)
    for r in phase2:
        by_fp[r.get("file_path", "?")].append(r)

    doc_max = [
        (max(r.get("cache_fetch_ms", 0) for r in passes), fp, passes)
        for fp, passes in by_fp.items()
    ]
    doc_max.sort(reverse=True)

    for max_cfms, fp, passes in doc_max[:top]:
        print(f"\n  {fp}  (passes={len(passes)})")
        for r in sorted(passes, key=lambda x: x.get("cache_fetch_ms", 0), reverse=True):
            print(
                f"    phase2={r.get('phase2_total_ms', '?'):>6}ms  "
                f"cache_fetch={r.get('cache_fetch_ms', '?'):>6}ms  "
                f"push_rel_ms={r.get('push_relation_ms', '?'):>6}ms  "
                f"union_mut={r.get('union_mut_ms', '?'):>5}ms  "
                f"n_cache_arm={r.get('n_cache_arm', '?'):>5}  "
                f"n_push_rel={r.get('n_push_relation', '?'):>6}  "
                f"neighborhood={r.get('neighborhood_total_ms', '?'):>4}ms"
            )


def section_warns(lines: list[str], top: int) -> None:
    print()
    print(SEP)
    warn_lines = [l for l in lines if " WARN " in l]
    error_lines = [l for l in lines if " ERROR " in l]
    print(f"[3] WARN lines: {len(warn_lines)}   ERROR lines: {len(error_lines)}")

    modules: Counter = Counter()
    for line in warn_lines:
        m = re.search(r"noet_core::[\w:]+", line)
        if m:
            modules[m.group(0)] += 1
    print("[3a] WARN by module:")
    for k, v in modules.most_common():
        print(f"  {v:>4}  {k}")

    miss_lines = [l for l in warn_lines if "MISS on re-parse" in l]
    print(f"\n[3b] 'MISS on re-parse' WARN lines: {len(miss_lines)}")
    miss_tasks: Counter = Counter()
    for line in miss_lines:
        m = re.search(r"path=(/[^\s}]+)", line)
        if m:
            p = short_path(m.group(1))
            miss_tasks[p] += 1
    print(f"     Top {top} tasks with re-parse MISSes:")
    for k, v in miss_tasks.most_common(top):
        print(f"       {v:>4}  {k}")

    miss_keys: Counter = Counter()
    for line in miss_lines:
        km = re.search(r"keys=\[(.+?)\]", line)
        if km:
            key_str = km.group(1)
            id_m = re.search(r'id: "([^"]+)"', key_str)
            path_m = re.search(r'path: "([^"]+)"', key_str)
            if id_m:
                miss_keys["id:" + id_m.group(1)] += 1
            elif path_m:
                miss_keys["path:" + path_m.group(1)] += 1
    print(f"\n     Top {top} key IDs/paths that MISS on re-parse:")
    for k, v in miss_keys.most_common(top):
        print(f"       {v:>4}  {k}")

    failed_lines = [l for l in lines if "cache_fetch FAILED" in l]
    print(f"\n[3c] 'cache_fetch FAILED' WARN lines: {len(failed_lines)}")
    for l in failed_lines[:5]:
        print(f"     {l.strip()}")

    m2 = sum(1 for l in miss_lines if "parse_number=2" in l)
    m1 = sum(1 for l in miss_lines if "parse_number=1" in l)
    print(f"\n[3d] MISS at parse_number=1: {m1}   parse_number=2: {m2}")


def section_parse_number(lines: list[str]) -> None:
    print()
    print(SEP)
    p1 = sum(1 for l in lines if "parse_number=1" in l)
    p2 = sum(1 for l in lines if "parse_number=2" in l)
    p3 = sum(1 for l in lines if "parse_number=3" in l)
    print(
        f"[4] Lines with parse_number=1: {p1}   "
        f"parse_number=2: {p2}   parse_number=3: {p3}"
    )


def section_aggregate(phase2: list[dict], slow_count: int) -> None:
    print()
    print(SEP)
    total_push_rel = sum(r.get("n_push_relation", 0) for r in phase2)
    total_cache_arm = sum(r.get("n_cache_arm", 0) for r in phase2)
    total_cfms = sum(r.get("cache_fetch_ms", 0) for r in phase2)
    total_p2ms = sum(r.get("phase2_total_ms", 0) for r in phase2)
    total_union = sum(r.get("union_mut_ms", 0) for r in phase2)
    total_reg = sum(r.get("regularize_ms", 0) for r in phase2)
    total_prms = sum(r.get("push_relation_ms", 0) for r in phase2)
    total_ntms = sum(r.get("neighborhood_total_ms", 0) for r in phase2)
    print(f"[5] Aggregate across all Phase 2 summary lines ({len(phase2)} lines):")
    print(f"    n_push_relation  total: {total_push_rel}")
    pct = (
        f"  ({100 * total_cache_arm / total_push_rel:.1f}% of push_rel)"
        if total_push_rel
        else ""
    )
    print(f"    n_cache_arm      total: {total_cache_arm}{pct}")
    print(f"    push_relation_ms total: {total_prms}ms")
    print(f"    cache_fetch_ms   total: {total_cfms}ms")
    print(f"    union_mut_ms     total: {total_union}ms")
    print(f"    regularize_ms    total: {total_reg}ms")
    print(f"    neighborhood_ms  total: {total_ntms}ms")
    print(f"    phase2_total_ms  total: {total_p2ms}ms  (sum across all docs x passes)")
    if total_p2ms:
        print(f"    cache_fetch share of phase2: {100 * total_cfms / total_p2ms:.1f}%")
    print()
    print(f"    [cache_fetch] slow lines:  {slow_count}")
    if total_cache_arm:
        print(
            f"    Ratio slow/n_cache_arm:    {slow_count}/{total_cache_arm}"
            f" = {100 * slow_count / total_cache_arm:.1f}%"
        )
    else:
        print("    (n_cache_arm total = 0; cannot compute ratio)")


def section_histogram(entries: list[dict]) -> None:
    print()
    print(SEP)
    print("[6] elapsed_ms distribution for [cache_fetch] slow entries:")
    buckets: Counter = Counter()
    for e in entries:
        ms = e["ms"]
        if ms < 15:
            buckets["<15ms"] += 1
        elif ms < 50:
            buckets["15-49ms"] += 1
        elif ms < 100:
            buckets["50-99ms"] += 1
        elif ms < 500:
            buckets["100-499ms"] += 1
        elif ms < 1000:
            buckets["500-999ms"] += 1
        elif ms < 3000:
            buckets["1000-2999ms"] += 1
        else:
            buckets[">=3000ms"] += 1
    order = [
        "<15ms",
        "15-49ms",
        "50-99ms",
        "100-499ms",
        "500-999ms",
        "1000-2999ms",
        ">=3000ms",
    ]
    for b in order:
        print(f"  {b:>12}: {buckets.get(b, 0):>5}")
    ms_vals = [e["ms"] for e in entries if e["ms"] > 0]
    if ms_vals:
        print(
            f"\n  non-zero count: {len(ms_vals)}, max: {max(ms_vals)}, "
            f"median: {statistics.median(ms_vals):.0f}, mean: {sum(ms_vals) / len(ms_vals):.0f}"
        )


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Analyze Phase 2 cache_fetch timing in a noet RUST_LOG=debug corpus run log."
    )
    parser.add_argument("log", help="Path to the log file to analyze")
    parser.add_argument(
        "--top",
        type=int,
        default=15,
        metavar="N",
        help="Number of rows to show in ranked tables (default: 15)",
    )
    args = parser.parse_args()

    lines = load_lines(args.log)
    print(f"Total log lines: {len(lines)}")
    print()

    entries = _parse_slow_entries(lines)
    phase2 = _parse_phase2(lines)

    section_slow(lines, entries, args.top)
    section_phase2(phase2, args.top)
    section_warns(lines, args.top)
    section_parse_number(lines)
    section_aggregate(phase2, len(entries))
    section_histogram(entries)


if __name__ == "__main__":
    main()
