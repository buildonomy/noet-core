#!/usr/bin/env python3
"""Model const-namespace hub degree under different URL-nesting depths.

Reads a newline-delimited list of URLs on stdin (or from a file argument) and
reports, for each nesting depth, how many namespace containers result and how
large the biggest one is.

The "max container" column is the number that matters: it is the worst-case
Section-child count of any one namespace node, i.e. the hub degree that makes
1-hop halo traversal fan out across a corpus. Use this to pick a nesting depth
if const-namespace nesting is ever picked up (see `docs/project/BACKLOG.md`).

The relative-cost column models per-container insert cost as `n^2/4`. Treat it
as a *breadth* comparison only — measured `map_insert` shift for both
const-namespaces is 0, because `assign_sort_key` issues monotonic keys and
their children tail-append. It is retained because it ranks groupings
consistently, not because it predicts wall time.

Requirements: Python 3.10+, no third-party packages.

Usage:
    grep -rhoE 'https?://[^)"<> ]+' build --include='*.md' | sort -u \
        | python3 benches/log_analysis/url_depth_sweep.py

    # Or against a namespace dump captured with NOET_DUMP_NAMESPACES:
    awk -F'\t' '$1=="href"{print $2}' ns.tsv \
        | python3 benches/log_analysis/url_depth_sweep.py
"""

import collections
import sys


def split_url(u):
    """Return (host, [path segments]) without urllib's IPv6 strictness."""
    rest = u.split("://", 1)[1] if "://" in u else u
    rest = rest.split("#", 1)[0].split("?", 1)[0]
    host, _, path = rest.partition("/")
    return host, [s for s in path.split("/") if s]


def cost(groups):
    return sum((n * n) / 4 for n in groups.values())


def group_at(urls, depth):
    g = collections.Counter()
    for u in urls:
        host, segs = split_url(u)
        g[(host,) + tuple(segs[:depth])] += 1
    return g


def main():
    src = open(sys.argv[1]) if len(sys.argv) > 1 else sys.stdin
    urls = [line.strip() for line in src if line.strip()]
    base = cost({"*": len(urls)})

    print(f"{'grouping':18} {'containers':>10} {'max':>7} {'rel cost':>9}")
    print(f"{'flat':18} {1:10} {len(urls):7} {100.0:8.2f}%")
    for depth in range(0, 5):
        g = group_at(urls, depth)
        label = "host" if depth == 0 else f"host/seg1..{depth}"
        print(
            f"{label:18} {len(g):10} {max(g.values()):7} "
            f"{100 * cost(g) / base:8.2f}%"
        )

    print("\ntop containers at depth 2:")
    for key, n in group_at(urls, 2).most_common(8):
        print(f"  {n:6}  {'/'.join(key)}")


if __name__ == "__main__":
    main()
