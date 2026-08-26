#!/usr/bin/env python3
"""Extract and summarize noet CLI diagnostic warnings from a corpus run log.

The noet CLI emits a block of diagnostics at the end of the run in the format:

    /abs/path/to/file.md:LINE:COL: warning: MESSAGE

These are distinct from the RUST_LOG structured lines parsed by extract_miss_keys.py
and analyze_phase2.py. This tool parses that block and produces a grouped summary.

Usage:
    python3 benches/log_analysis/extract_diagnostics.py /tmp/corpus_11.log
    python3 benches/log_analysis/extract_diagnostics.py /tmp/corpus_11.log --diff /tmp/corpus_10.log
    python3 benches/log_analysis/extract_diagnostics.py /tmp/corpus_11.log --top 20

Output sections:
    [1] Total counts by category
    [2] Unresolved links — unique keys, grouped and counted (mirrors extract_miss_keys.py)
    [3] Anchor collisions — by file
    [4] Other warnings — by normalized message pattern
    [5] Source-file breakdown — which files have the most warnings
    [6] If --diff provided: resolved vs new vs unchanged keys vs prior log
"""

import argparse
import re
import sys
from collections import Counter, defaultdict

# ---------------------------------------------------------------------------
# Regex patterns
# ---------------------------------------------------------------------------

ANSI_RE = re.compile(r"\x1b\[[0-9;]*m")

# Matches the CLI diagnostic line format:
#   /abs/path/to/file:LINE:COL: warning: MESSAGE
#   /abs/path/to/file:LINE: warning: MESSAGE   (no column)
DIAG_RE = re.compile(r"^(/[^:]+):(\d+)(?::(\d+))?: (warning|error): (.+)$")

# Matches the "tried [...]" key inside an unresolved link message
TRIED_KEY_RE = re.compile(r"tried \[(.+)\]$")

# Build dir prefix to strip for short paths
_BUILD_RE = re.compile(r".*/build/")


def strip_ansi(s: str) -> str:
    return ANSI_RE.sub("", s)


def short_path(p: str) -> str:
    """Strip the absolute build prefix, leaving the repo-relative path."""
    return _BUILD_RE.sub("", p)


# ---------------------------------------------------------------------------
# Diagnostic categorisation
# ---------------------------------------------------------------------------


def categorise(message: str) -> str:
    """Return a short category label for a diagnostic message."""
    if "unresolved link" in message:
        return "unresolved_link"
    if (
        "anchor collision" in message.lower()
        or "Intra-document heading anchor" in message
    ):
        return "anchor_collision"
    if "Could not rewrite link" in message:
        return "link_rewrite_failed"
    if "Path mismatch" in message:
        return "path_mismatch"
    if "balance loop hit BALANCE_CUTOFF" in message:
        return "balance_cutoff"
    return "other"


# ---------------------------------------------------------------------------
# Parsing
# ---------------------------------------------------------------------------


class Diagnostic:
    __slots__ = ("file", "line", "col", "level", "message", "category", "short_file")

    def __init__(self, file: str, line: int, col: int | None, level: str, message: str):
        self.file = file
        self.line = line
        self.col = col
        self.level = level
        self.message = message
        self.category = categorise(message)
        self.short_file = short_path(file)


def parse_diagnostics(log_path: str) -> list[Diagnostic]:
    """Parse all CLI diagnostic lines from the log file."""
    diags: list[Diagnostic] = []
    with open(log_path, encoding="utf-8", errors="replace") as fh:
        for raw in fh:
            line = strip_ansi(raw.rstrip())
            m = DIAG_RE.match(line)
            if not m:
                continue
            file_path, lineno, colno, level, message = m.groups()
            diags.append(
                Diagnostic(
                    file=file_path,
                    line=int(lineno),
                    col=int(colno) if colno is not None else None,
                    level=level,
                    message=message,
                )
            )
    return diags


# ---------------------------------------------------------------------------
# Unresolved-link key extraction (mirrors extract_miss_keys.py style)
# ---------------------------------------------------------------------------


def extract_unresolved_keys(diags: list[Diagnostic]) -> tuple[list[str], Counter]:
    """Return (unique_sorted_keys, key_counts) for unresolved-link diagnostics."""
    keys = []
    for d in diags:
        if d.category != "unresolved_link":
            continue
        m = TRIED_KEY_RE.search(d.message)
        if m:
            keys.append(m.group(1).strip())
    counts: Counter = Counter(keys)
    unique = sorted(set(keys))
    return unique, counts


def net_bref(key: str) -> str:
    m = re.search(r"net: Bref\(([0-9a-f]+)\)", key)
    return m.group(1) if m else "?"


def key_path(key: str) -> str | None:
    m = re.search(r'path: "([^"]+)"', key)
    return m.group(1) if m else None


def first_segment(path: str) -> str:
    return path.split("/")[0].split("#")[0]


# ---------------------------------------------------------------------------
# Diff support — BRef-normalised so churn doesn't create false positives
# ---------------------------------------------------------------------------


def normalise_key(key: str) -> str:
    """Strip BRef values so keys compare correctly across runs."""
    return re.sub(r"Bref\([0-9a-f]+\)", "Bref(?)", key)


def diff_keys(
    before_counts: Counter, after_counts: Counter
) -> tuple[dict[str, int], dict[str, int], dict[str, tuple[int, int]]]:
    """
    Returns (resolved, new, unchanged).

    resolved:  {raw_key: before_count}  — present before, gone after
    new:       {raw_key: after_count}   — absent before, present after
    unchanged: {raw_key: (before, after)} — present in both (counts may differ)

    BRef values are ignored when comparing keys.
    """
    before_norm: dict[str, tuple[str, int]] = {}
    for k, c in before_counts.items():
        nk = normalise_key(k)
        if nk not in before_norm or before_norm[nk][1] < c:
            before_norm[nk] = (k, c)

    after_norm: dict[str, tuple[str, int]] = {}
    for k, c in after_counts.items():
        nk = normalise_key(k)
        if nk not in after_norm or after_norm[nk][1] < c:
            after_norm[nk] = (k, c)

    before_set = set(before_norm)
    after_set = set(after_norm)

    resolved: dict[str, int] = {}
    for nk in before_set - after_set:
        raw, cnt = before_norm[nk]
        resolved[raw] = cnt

    new: dict[str, int] = {}
    for nk in after_set - before_set:
        raw, cnt = after_norm[nk]
        new[raw] = cnt

    unchanged: dict[str, tuple[int, int]] = {}
    for nk in before_set & after_set:
        _, cnt_b = before_norm[nk]
        raw_a, cnt_a = after_norm[nk]
        unchanged[raw_a] = (cnt_b, cnt_a)

    return resolved, new, unchanged


# ---------------------------------------------------------------------------
# Report sections
# ---------------------------------------------------------------------------

SEP = "=" * 70


def report_counts(diags: list[Diagnostic]) -> None:
    print(f"\n{SEP}")
    print(f"  [1] Diagnostic totals  ({len(diags)} total)")
    print(SEP)

    cat_counts: Counter = Counter(d.category for d in diags)
    level_counts: Counter = Counter(d.level for d in diags)

    print(f"\n  By level:")
    for level, cnt in sorted(level_counts.items()):
        print(f"    {cnt:>6}  {level}")

    print(f"\n  By category:")
    for cat, cnt in cat_counts.most_common():
        print(f"    {cnt:>6}  {cat}")


def report_unresolved(unique_keys: list[str], key_counts: Counter, top: int) -> None:
    total_occ = sum(key_counts.values())
    print(f"\n{SEP}")
    print(
        f"  [2] Unresolved links — {total_occ} occurrences, {len(unique_keys)} unique keys"
    )
    print(SEP)

    path_keys = [k for k in unique_keys if k.startswith("Path {")]
    id_keys = [k for k in unique_keys if k.startswith("Id {")]
    other_keys = [
        k
        for k in unique_keys
        if not k.startswith("Path {") and not k.startswith("Id {")
    ]

    def print_keys(label: str, keys: list[str]) -> None:
        if not keys:
            return
        shown = sorted(keys, key=lambda k: -key_counts[k])[:top]
        print(f"\n  {label} ({len(keys)}):")
        for k in shown:
            print(f"    {key_counts[k]:>4}x  {k}")
        if len(keys) > top:
            print(f"    … {len(keys) - top} more (use --top N to show more)")

    print_keys("Path keys", path_keys)
    print_keys("Id keys", id_keys)
    if other_keys:
        print_keys("Other keys", other_keys)

    # Net breakdown
    net_ctr: Counter = Counter()
    for k, cnt in key_counts.items():
        net_ctr[net_bref(k)] += cnt
    print(f"\n  Net breakdown:")
    for bref, cnt in net_ctr.most_common():
        print(f"    {cnt:>4}x  Bref({bref})")

    # Path prefix breakdown
    prefix_ctr: Counter = Counter()
    for k, cnt in key_counts.items():
        p = key_path(k)
        if p:
            prefix_ctr[first_segment(p)] += cnt
    if prefix_ctr:
        print(f"\n  Path prefix breakdown (first segment):")
        for seg, cnt in prefix_ctr.most_common(10):
            print(f"    {cnt:>4}x  {seg}/...")


def report_anchor_collisions(diags: list[Diagnostic], top: int) -> None:
    collisions = [d for d in diags if d.category == "anchor_collision"]
    print(f"\n{SEP}")
    print(f"  [3] Anchor collisions — {len(collisions)} total")
    print(SEP)

    if not collisions:
        print("  None.")
        return

    by_file: dict[str, list[Diagnostic]] = defaultdict(list)
    for d in collisions:
        by_file[d.short_file].append(d)

    shown = sorted(by_file.items(), key=lambda x: -len(x[1]))[:top]
    for f, ds in shown:
        print(f"\n  {f}  ({len(ds)} collision{'s' if len(ds) > 1 else ''})")
        for d in sorted(ds, key=lambda x: x.line):
            slug_m = re.search(r"'([^']+)' appears more than once", d.message)
            new_m = re.search(r"assigned the anchor '([^']+)'", d.message)
            slug = slug_m.group(1) if slug_m else "?"
            new = new_m.group(1) if new_m else "?"
            print(f"    line {d.line:>5}: #{slug}  ->  #{new}")

    if len(by_file) > top:
        print(f"\n  … {len(by_file) - top} more files (use --top N to show more)")


def report_other(diags: list[Diagnostic], top: int) -> None:
    other = [
        d for d in diags if d.category not in ("unresolved_link", "anchor_collision")
    ]
    if not other:
        return

    print(f"\n{SEP}")
    print(f"  [4] Other warnings/errors — {len(other)} total")
    print(SEP)

    def normalise_msg(msg: str) -> str:
        msg = re.sub(r"Bref\([0-9a-f]+\)", "Bref(?)", msg)
        msg = re.sub(r"Bid\([0-9a-f-]+\)", "Bid(?)", msg)
        # Collapse long single-quoted strings (file paths, etc.)
        msg = re.sub(r"'[^']{30,}'", "'<value>'", msg)
        return msg

    norm_ctr: Counter = Counter()
    norm_example: dict[str, str] = {}
    for d in other:
        nk = normalise_msg(d.message)
        norm_ctr[nk] += 1
        if nk not in norm_example:
            norm_example[nk] = d.message

    for nk, cnt in norm_ctr.most_common(top):
        print(f"\n  {cnt:>4}x  {norm_example[nk][:120]}")


def report_by_source(diags: list[Diagnostic], top: int) -> None:
    print(f"\n{SEP}")
    print(f"  [5] Warnings by source file (top {top})")
    print(SEP)

    file_ctr: Counter = Counter(d.short_file for d in diags)
    for f, cnt in file_ctr.most_common(top):
        print(f"  {cnt:>4}  {f}")


def report_diff(
    before_log: str,
    after_unique: list[str],
    after_counts: Counter,
) -> None:
    print(f"\n{SEP}")
    print(f"  [6] Diff vs {before_log}")
    print(SEP)

    before_diags = parse_diagnostics(before_log)
    before_unique, before_counts = extract_unresolved_keys(before_diags)

    resolved, new, unchanged = diff_keys(before_counts, after_counts)

    total_before = sum(before_counts.values())
    total_after = sum(after_counts.values())
    delta = total_after - total_before
    sign = "+" if delta >= 0 else ""

    print(f"\n  Before: {total_before} occurrences, {len(before_unique)} unique keys")
    print(f"  After:  {total_after} occurrences, {len(after_unique)} unique keys")
    print(
        f"  Delta:  {sign}{delta} occurrences  ({sign}{len(after_unique) - len(before_unique)} unique keys)"
    )

    def _print_group(label: str, items: dict, value_fn) -> None:
        if not items:
            print(f"\n  {label}: none")
            return
        sorted_items = sorted(items.items(), key=lambda x: -value_fn(x[1]))
        print(f"\n  {label} ({len(items)} keys):")
        for k, v in sorted_items:
            print(f"    {value_fn(v):>4}x  {k}")

    _print_group("RESOLVED", resolved, lambda v: v)
    _print_group("NEW", new, lambda v: v)
    _print_group("UNCHANGED", unchanged, lambda v: v[1])


# ---------------------------------------------------------------------------
# Entry point
# ---------------------------------------------------------------------------


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Extract and summarize noet CLI diagnostics from a corpus run log."
    )
    parser.add_argument("log", help="Path to the log file to analyse")
    parser.add_argument(
        "--diff",
        metavar="PRIOR_LOG",
        help="Compare unresolved-link keys against a prior log file",
    )
    parser.add_argument(
        "--top",
        type=int,
        default=30,
        help="Number of items to show in ranked lists (default: 30)",
    )
    args = parser.parse_args()

    diags = parse_diagnostics(args.log)

    if not diags:
        print(f"No CLI diagnostics found in {args.log}.")
        print("(Expected lines like: /abs/path/file.md:LINE:COL: warning: MESSAGE)")
        sys.exit(0)

    unique_keys, key_counts = extract_unresolved_keys(diags)

    report_counts(diags)
    report_unresolved(unique_keys, key_counts, args.top)
    report_anchor_collisions(diags, args.top)
    report_other(diags, args.top)
    report_by_source(diags, args.top)

    if args.diff:
        report_diff(args.diff, unique_keys, key_counts)

    print(f"\n{SEP}\n")


if __name__ == "__main__":
    main()
