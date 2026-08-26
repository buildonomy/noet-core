#!/usr/bin/env python3
"""Extract unique unresolved-link keys from noet CLI diagnostic output.

Parses CLI warning diagnostics of the form:

    path/to/file.md:15:3: warning: unresolved link — tried [Id { ... }, ...]
    path/to/file.md: warning: unresolved link — tried [Path { ... }]

Also supports the legacy tracing format (MISS on re-parse) for backwards
compatibility with older log files.

Usage:
    python3 benches/log_analysis/extract_miss_keys.py /tmp/build.log

Output:
    Unique key strings, sorted, one per line — then count breakdowns
    split by key type (Path vs Id), by source file, by net bref,
    and by path-prefix.
"""

import re
import sys
from collections import Counter

ANSI_RE = re.compile(r"\x1b\[[0-9;]*m")

# New diagnostic format:
#   path/to/file.md:15:3: warning: unresolved link — tried [...]
#   path/to/file.md: warning: unresolved link — tried [...]
# The separator between "link" and "tried" is U+2014 em dash, but we match
# flexibly with .{1,4} to handle encoding variations and plain hyphens.
DIAG_RE = re.compile(
    r"(?P<path>[^:]+?)(?::(?P<line>\d+):(?P<col>\d+))?:\s*"
    r"(?:\x1b\[[0-9;]*m)*warning(?:\x1b\[[0-9;]*m)*:\s*"
    r"unresolved link\s*.{1,4}\s*tried\s*\[(?P<keys>.+)\]"
)

# Legacy tracing format:
#   ... MISS on re-parse ... keys=[...] repo=...
TRACE_RE = re.compile(r"keys=\[(.+)\] repo=")


def split_top_level_keys(raw: str) -> list[str]:
    """Split a comma-separated key list respecting nested braces.

    Keys look like:  Id { net: Bref("a1b2c"), id: "foo" }
    A naive split on ", " would break inside the braces.
    """
    keys = []
    depth = 0
    start = 0
    for i, ch in enumerate(raw):
        if ch == "{":
            depth += 1
        elif ch == "}":
            depth -= 1
            if depth == 0:
                keys.append(raw[start : i + 1].strip())
                # skip past the ", " separator
                start = i + 1
        elif ch == "," and depth == 0:
            start = i + 1
    # capture any trailing content (shouldn't happen with well-formed input)
    tail = raw[start:].strip()
    if tail:
        keys.append(tail)
    return [k for k in keys if k]


def main():
    if len(sys.argv) < 2:
        print("Usage: extract_miss_keys.py <log_file>", file=sys.stderr)
        sys.exit(1)

    log_path = sys.argv[1]
    keys: list[str] = []
    source_files: list[str] = []

    with open(log_path, encoding="utf-8", errors="replace") as f:
        for raw_line in f:
            line = ANSI_RE.sub("", raw_line)

            # Try new diagnostic format first
            dm = DIAG_RE.search(raw_line)  # search raw to preserve em dash bytes
            if dm:
                raw_keys = dm.group("keys")
                src = dm.group("path")
                for k in split_top_level_keys(raw_keys):
                    keys.append(k)
                    source_files.append(src)
                continue

            # Fall back to legacy tracing format
            if "MISS on re-parse" not in line:
                continue
            tm = TRACE_RE.search(line)
            if not tm:
                continue
            keys.append(tm.group(1).strip())
            source_files.append("<tracing>")

    unique_keys = sorted(set(keys))
    key_counts = Counter(keys)

    print(f"Total warning lines: {len(keys)}")
    print(f"Unique keys:         {len(unique_keys)}")
    print()

    # Split into Path and Id
    path_keys = [k for k in unique_keys if k.startswith("Path {")]
    id_keys = [k for k in unique_keys if k.startswith("Id {")]
    other_keys = [
        k
        for k in unique_keys
        if not k.startswith("Path {") and not k.startswith("Id {")
    ]

    print(f"=== Path keys ({len(path_keys)}) ===")
    for k in path_keys:
        print(f"  {key_counts[k]:>4}x  {k}")

    print()
    print(f"=== Id keys ({len(id_keys)}) ===")
    for k in id_keys:
        print(f"  {key_counts[k]:>4}x  {k}")

    if other_keys:
        print()
        print(f"=== Other keys ({len(other_keys)}) ===")
        for k in other_keys:
            print(f"  {key_counts[k]:>4}x  {k}")

    # Source file breakdown
    file_counter = Counter(source_files)
    if file_counter and not (
        len(file_counter) == 1 and "<tracing>" in file_counter
    ):
        print()
        print(f"=== Source file breakdown ({len(file_counter)} files) ===")
        for src, count in file_counter.most_common():
            print(f"  {count:>4}x  {src}")

    print()
    print("=== Net breakdown ===")
    net_counter: Counter[str] = Counter()
    for k in keys:
        # Handle both Bref("a1b2c") (diagnostic) and Bref(a1b2c) (tracing)
        m = re.search(r'net: Bref\("?([0-9a-f]+)"?\)', k)
        if m:
            net_counter[m.group(1)] += 1
    for bref, count in net_counter.most_common():
        print(f"  {count:>4}x  Bref({bref})")

    print()
    print("=== Path-key path-prefix breakdown (first segment) ===")
    prefix_counter: Counter[str] = Counter()
    for k in keys:
        if "path:" not in k and 'path: "' not in k:
            continue
        m = re.search(r'path: "([^"]+)"', k)
        if not m:
            continue
        p = m.group(1)
        seg = p.split("/")[0].split("#")[0]
        prefix_counter[seg] += 1
    for seg, count in prefix_counter.most_common():
        print(f"  {count:>4}x  {seg}/...")


if __name__ == "__main__":
    main()
