#!/usr/bin/env python3
"""Analyze WARN lines in a noet log file, grouped by normalized message pattern.

Usage:
    python3 benches/log_analysis/analyze_warns.py <log_file>

Example:
    python3 benches/log_analysis/analyze_warns.py /tmp/my-run.log
"""

import argparse
import collections
import re

ANSI_RE = re.compile(r"\x1b\[[0-9;]*m")
WARN_RE = re.compile(r"WARN\s+[^:]+: (.+)")


def normalize(msg: str) -> str:
    msg = re.sub(r"Bref\([0-9a-f]+\)", "Bref(X)", msg)
    msg = re.sub(
        r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}", "UUID", msg
    )
    msg = re.sub(r"parse_number=\d+", "parse_number=N", msg)
    msg = re.sub(r"nodes=\d+", "nodes=N", msg)
    msg = re.sub(r"edges=\d+", "edges=N", msg)
    msg = re.sub(r"keys=\[[^\]]*\]", "keys=[...]", msg)
    msg = re.sub(r'path="[^"]+"', 'path="P"', msg)
    msg = re.sub(r"path=[^ ,}\]]+", "path=P", msg)
    msg = re.sub(r'"[^"]{50,}"', '"..."', msg)
    return msg[:150]


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Analyze WARN lines in a noet log file, grouped by normalized message pattern."
    )
    parser.add_argument("log", help="Path to the log file to analyze")
    args = parser.parse_args()

    counts: collections.Counter = collections.Counter()
    examples: dict[str, str] = {}

    with open(args.log, errors="replace") as f:
        for line in f:
            if "WARN" not in line:
                continue
            line = ANSI_RE.sub("", line)
            m = WARN_RE.search(line)
            if not m:
                continue
            raw = m.group(1)
            key = normalize(raw)
            counts[key] += 1
            if key not in examples:
                examples[key] = raw[:200]

    print(f"{'COUNT':>6}  MESSAGE")
    print("-" * 80)
    for msg, count in counts.most_common():
        print(f"{count:6d}  {msg}")
        print(f"         ex: {examples[msg]}")
        print()


if __name__ == "__main__":
    main()
