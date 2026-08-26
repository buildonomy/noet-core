#!/usr/bin/env python3
"""Correlate C++ tree-sitter parse cost against file size and include count.

Issue 97 Bottleneck 3: 211 process-wide silent gaps >20s (~163 min total), the
largest cluster falling in the C++ source corpus with near-zero log output.
Larger in aggregate than Bottleneck 2 but spread across many files, so more
likely genuine tree-sitter parse cost than a structural defect -- this tool
exists to check that hypothesis rather than assume it.

A downstream C++ codec crate's `CppCodec::parse` now emits two debug events on
its `cpp::perf` tracing target per file:

  `[CppCodec::parse] tree_sitter parse`       — path, content_len, elapsed_ms
  `[CppCodec::parse] post-parse extraction`   — path, n_symbols,
                                                 syntax_spans_ms,
                                                 extract_symbols_ms
  `[CppCodec::parse] collect_include_paths`   — path, content_len, n_includes,
                                                 elapsed_ms

This tool joins all three per file and reports:
  - top files by tree-sitter parse time
  - a linear fit of parse time vs. content_len and vs. n_includes, to check
    for superlinearity (the open question in Bottleneck 3)
  - the breakdown of tree_sitter parse vs. symbol/span extraction vs. include
    collection, so a genuine tree-sitter bottleneck can be told apart from
    the noet-side extraction code around it

Usage:
    Run the downstream C++ codec's own parse CLI with `info` level plus
    debug-level tracing enabled on its `cpp::perf` target, pointing at an
    HTML output path, and redirect combined stdout/stderr to a log file:
        <downstream-cli> parse --html-output /tmp/out <corpus> > run.log 2>&1
    Then feed that log to this script:
        python3 benches/log_analysis/analyze_cpp_parse.py run.log

Requirements: Python 3.10+, no third-party packages.
"""

import argparse
import re
import statistics

ANSI_RE = re.compile(r"\x1b\[[0-9;]*m")

PARSE_MARKER = "[CppCodec::parse] tree_sitter parse"
EXTRACT_MARKER = "[CppCodec::parse] post-parse extraction"
INCLUDES_MARKER = "[CppCodec::parse] collect_include_paths"

PATH_RE = re.compile(r"path=(\S+)")
CONTENT_LEN_RE = re.compile(r"content_len=(\d+)")
ELAPSED_MS_RE = re.compile(r"elapsed_ms=(\d+)")
N_SYMBOLS_RE = re.compile(r"n_symbols=(\d+)")
SYNTAX_SPANS_MS_RE = re.compile(r"syntax_spans_ms=(\d+)")
EXTRACT_SYMBOLS_MS_RE = re.compile(r"extract_symbols_ms=(\d+)")
N_INCLUDES_RE = re.compile(r"n_includes=(\d+)")


def strip_ansi(s: str) -> str:
    return ANSI_RE.sub("", s)


def _ols(xs: list[float], ys: list[float]) -> tuple[float, float]:
    n = len(xs)
    sx, sy = sum(xs), sum(ys)
    sxx = sum(x * x for x in xs)
    sxy = sum(x * y for x, y in zip(xs, ys))
    denom = n * sxx - sx * sx
    if denom == 0:
        return 0.0, sy / n if n else 0.0
    slope = (n * sxy - sx * sy) / denom
    intercept = (sy - slope * sx) / n
    return slope, intercept


def _r_squared(xs: list[float], ys: list[float], slope: float, intercept: float) -> float:
    if not ys:
        return 0.0
    mean_y = sum(ys) / len(ys)
    ss_tot = sum((y - mean_y) ** 2 for y in ys)
    if ss_tot == 0:
        return 1.0
    ss_res = sum((y - (slope * x + intercept)) ** 2 for x, y in zip(xs, ys))
    return 1 - ss_res / ss_tot


def main() -> None:
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    ap.add_argument("log", help="Path to the log file to analyze")
    ap.add_argument("--top", type=int, default=20, help="Rows in ranked tables")
    args = ap.parse_args()

    files: dict[str, dict] = {}

    with open(args.log, errors="replace") as fh:
        for raw in fh:
            line = strip_ansi(raw.rstrip())
            # Under --jobs N>1, lines carry a parse_task{task_idx=N path=DIR}:
            # span prefix that itself contains a `path=` field (the *directory*
            # passed to parse_epoch, not the file). Searching the whole line for
            # PATH_RE would match that span-prefix field first (picking up its
            # trailing "}:" since that isn't whitespace). Restrict every field
            # search to the substring starting at the marker itself, which is
            # always emitted after the span prefix.
            if PARSE_MARKER in line:
                body = line[line.index(PARSE_MARKER) :]
                path_m = PATH_RE.search(body)
                if not path_m:
                    continue
                path = path_m.group(1)
                rec = files.setdefault(path, {})
                cl_m = CONTENT_LEN_RE.search(body)
                el_m = ELAPSED_MS_RE.search(body)
                if cl_m:
                    rec["content_len"] = int(cl_m.group(1))
                if el_m:
                    rec["parse_ms"] = int(el_m.group(1))
            elif EXTRACT_MARKER in line:
                body = line[line.index(EXTRACT_MARKER) :]
                path_m = PATH_RE.search(body)
                if not path_m:
                    continue
                rec = files.setdefault(path_m.group(1), {})
                ns_m = N_SYMBOLS_RE.search(body)
                ss_m = SYNTAX_SPANS_MS_RE.search(body)
                es_m = EXTRACT_SYMBOLS_MS_RE.search(body)
                if ns_m:
                    rec["n_symbols"] = int(ns_m.group(1))
                if ss_m:
                    rec["syntax_spans_ms"] = int(ss_m.group(1))
                if es_m:
                    rec["extract_symbols_ms"] = int(es_m.group(1))
            elif INCLUDES_MARKER in line:
                body = line[line.index(INCLUDES_MARKER) :]
                path_m = PATH_RE.search(body)
                if not path_m:
                    continue
                rec = files.setdefault(path_m.group(1), {})
                ni_m = N_INCLUDES_RE.search(body)
                iem_m = ELAPSED_MS_RE.search(body)
                if ni_m:
                    rec["n_includes"] = int(ni_m.group(1))
                if iem_m:
                    rec["includes_ms"] = int(iem_m.group(1))

    if not files:
        print(
            "No '[CppCodec::parse]' events found. This probe requires debug-level\n"
            "tracing on the downstream C++ codec's 'cpp::perf' target\n"
            "and a corpus containing .h/.cpp files handled by that codec's CppCodec."
        )
        return

    complete = [
        (path, rec)
        for path, rec in files.items()
        if "parse_ms" in rec and "content_len" in rec
    ]
    print("=" * 70)
    print("  C++ tree-sitter parse cost")
    print("=" * 70)
    print(f"  Files with timing     : {len(complete)} / {len(files)} total records")
    if not complete:
        return

    parse_times = [rec["parse_ms"] for _, rec in complete]
    total_parse_ms = sum(parse_times)
    print(f"  Total tree_sitter parse time : {total_parse_ms} ms ({total_parse_ms / 1000:.1f}s)")
    print(f"  Mean   : {statistics.mean(parse_times):.1f} ms")
    print(f"  Median : {statistics.median(parse_times):.1f} ms")
    print(f"  Max    : {max(parse_times)} ms")

    extract_total = sum(rec.get("syntax_spans_ms", 0) + rec.get("extract_symbols_ms", 0) for _, rec in complete)
    includes_total = sum(rec.get("includes_ms", 0) for _, rec in complete)
    grand_total = total_parse_ms + extract_total + includes_total
    print(f"\n  Stage breakdown (of {grand_total} ms grand total across all files):")
    print(f"    tree_sitter parse      : {total_parse_ms:>10} ms  ({100 * total_parse_ms / grand_total:5.1f}%)")
    print(f"    symbol/span extraction : {extract_total:>10} ms  ({100 * extract_total / grand_total:5.1f}%)")
    print(f"    collect_include_paths  : {includes_total:>10} ms  ({100 * includes_total / grand_total:5.1f}%)")

    print(f"\n  Top {args.top} files by tree_sitter parse time:")
    print(f"  {'ms':>8} {'content_len':>12} {'n_includes':>11} {'n_symbols':>10}  path")
    print(f"  {'-' * 8} {'-' * 12} {'-' * 11} {'-' * 10}  {'-' * 40}")
    ranked = sorted(complete, key=lambda kv: -kv[1]["parse_ms"])
    for path, rec in ranked[: args.top]:
        print(
            f"  {rec['parse_ms']:>8} {rec['content_len']:>12} "
            f"{rec.get('n_includes', '?'):>11} {rec.get('n_symbols', '?'):>10}  {path}"
        )

    # Superlinearity check: parse_ms vs. content_len.
    print("\n" + "=" * 70)
    print("  Superlinearity check: parse_ms vs. content_len")
    print("=" * 70)
    xs = [float(rec["content_len"]) for _, rec in complete]
    ys = [float(rec["parse_ms"]) for _, rec in complete]
    slope, intercept = _ols(xs, ys)
    r2 = _r_squared(xs, ys, slope, intercept)
    print(f"  Linear fit: parse_ms = {slope:.6f} * content_len + {intercept:.3f}")
    print(f"  R\u00b2 = {r2:.3f}  (closer to 1.0 = better linear fit)")
    print(
        "  If R\u00b2 is poor and residuals grow with file size, check the same fit\n"
        "  against content_len\u00b2 or n_includes below before assuming a defect --\n"
        "  tree-sitter incremental parsing is expected to be roughly linear in\n"
        "  content length for well-formed C++."
    )

    with_includes = [(path, rec) for path, rec in complete if "n_includes" in rec]
    if with_includes:
        print("\n  Superlinearity check: parse_ms vs. n_includes")
        xs2 = [float(rec["n_includes"]) for _, rec in with_includes]
        ys2 = [float(rec["parse_ms"]) for _, rec in with_includes]
        slope2, intercept2 = _ols(xs2, ys2)
        r2_2 = _r_squared(xs2, ys2, slope2, intercept2)
        print(f"  Linear fit: parse_ms = {slope2:.4f} * n_includes + {intercept2:.3f}")
        print(f"  R\u00b2 = {r2_2:.3f}")

    # content_len per ms, to catch outliers where a small file took disproportionately long.
    print(f"\n  Top {args.top} files by ms-per-KB (outliers relative to their own size):")
    per_kb = [
        (rec["parse_ms"] / max(rec["content_len"] / 1024.0, 0.01), path, rec)
        for path, rec in complete
    ]
    per_kb.sort(reverse=True)
    print(f"  {'ms/KB':>8} {'ms':>8} {'content_len':>12}  path")
    print(f"  {'-' * 8} {'-' * 8} {'-' * 12}  {'-' * 40}")
    for rate, path, rec in per_kb[: args.top]:
        print(f"  {rate:>8.2f} {rec['parse_ms']:>8} {rec['content_len']:>12}  {path}")


if __name__ == "__main__":
    main()
