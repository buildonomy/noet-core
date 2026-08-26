#!/usr/bin/env python3
"""Compare MISS-on-re-parse keys between two noet run logs.

Shows which keys were resolved, which counts changed, and which are new.

Usage:
    python3 benches/log_analysis/diff_miss_keys.py <log_before> <log_after>

Example:
    python3 benches/log_analysis/diff_miss_keys.py /tmp/corpus_7.log /tmp/corpus_8.log
"""

import re
import sys
from collections import Counter

ANSI_RE = re.compile(r"\x1b\[[0-9;]*m")


def load_keys(path: str) -> Counter:
    keys: Counter = Counter()
    with open(path, encoding="utf-8", errors="replace") as f:
        for raw_line in f:
            line = ANSI_RE.sub("", raw_line)
            if "MISS on re-parse" not in line:
                continue
            m = re.search(r"keys=\[(.+)\] repo=", line)
            if m:
                keys[m.group(1).strip()] += 1
    return keys


def main() -> None:
    if len(sys.argv) != 3:
        print("Usage: diff_miss_keys.py <log_before> <log_after>", file=sys.stderr)
        sys.exit(1)

    before_path, after_path = sys.argv[1], sys.argv[2]
    before = load_keys(before_path)
    after = load_keys(after_path)

    total_before = sum(before.values())
    total_after = sum(after.values())
    unique_before = len(before)
    unique_after = len(after)

    resolved = {k: v for k, v in before.items() if k not in after}
    reduced = {
        k: (before[k], after[k]) for k in before if k in after and after[k] < before[k]
    }
    new = {k: v for k, v in after.items() if k not in before}
    unchanged = {k: v for k, v in after.items() if before.get(k) == v}

    resolved_lines = sum(resolved.values())
    reduced_lines_saved = sum(b - a for b, a in reduced.values())
    new_lines = sum(new.values())

    print(f"Before: {total_before} lines, {unique_before} unique keys  ({before_path})")
    print(f"After:  {total_after} lines, {unique_after} unique keys  ({after_path})")
    net = total_after - total_before
    print(
        f"Delta:  {net:+d} lines  ({-resolved_lines} resolved, {-reduced_lines_saved} reduced, {+new_lines} new)"
    )

    print()
    print(f"=== RESOLVED — {len(resolved)} keys, {resolved_lines} lines eliminated ===")
    for k, v in sorted(resolved.items(), key=lambda x: -x[1]):
        print(f"  {v:4}x  {k}")

    if reduced:
        print()
        print(
            f"=== COUNT REDUCED — {len(reduced)} keys, {reduced_lines_saved} lines saved ==="
        )
        for k, (b, a) in sorted(reduced.items(), key=lambda x: -(x[1][0] - x[1][1])):
            print(f"  {b}→{a}  {k}")

    if new:
        print()
        print(f"=== NEW (regression) — {len(new)} keys, {new_lines} lines ===")
        for k, v in sorted(new.items(), key=lambda x: -x[1]):
            print(f"  {v:4}x  {k}")

    print()
    print(f"=== UNCHANGED — {len(unchanged)} keys, {sum(unchanged.values())} lines ===")
    for k, v in sorted(unchanged.items(), key=lambda x: -x[1]):
        print(f"  {v:4}x  {k}")


if __name__ == "__main__":
    main()
