#!/usr/bin/env python3
"""Summarize PathMap read/write cost from a noet corpus run log.

Reports two things per network:

  - **Scan cost** (`PathMap::indexed_get`) — how many candidate entries each
    path lookup had to consider. Since the `PathMap::path_map` index landed,
    this should stay at 0–1 per call. A climbing `avg` means the
    index desynchronised or a caller reintroduced a linear scan; before the
    index it averaged 2,280 on the asset namespace.

  - **Insert shift cost** (`PathMap::map_insert`) — how many entries each
    insert displaced. Expected 0 for const-namespaces (`assign_sort_key`
    issues monotonic keys per `(sink, kind)`, so their children tail-append
    by construction) and small for content networks. A non-zero
    const-namespace total means sort keys are re-entering an occupied range.

Usage:
    RUST_LOG='info,noet_core::paths::scan=debug,noet_core::paths::perf=debug' \\
        noet parse --db --no-progress --jobs 1 <corpus> > run.log 2>&1
    python3 benches/log_analysis/analyze_pathmap.py run.log
    python3 benches/log_analysis/analyze_pathmap.py run.log --net 7232a397e404

Requirements: Python 3.10+, no third-party packages.

CAVEAT on scan accounting: the counter is emitted *before* `indexed_get`'s
subnet-recursion fallback, so one logical lookup that descends N subnets emits
N+1 records, each attributed to the network whose map it scanned. Per-network
totals are correct as "work done by this map", but `calls` counts records, not
logical lookups, and summing across networks mixes levels of one lookup. This
is currently moot for const-namespaces — they are flat (`n_subnets=0` on 100%
of records), so records map 1:1 there — but it stops being moot if those
namespaces are ever nested.
"""

import collections
import re
import sys

# The tracing subscriber emits ANSI styling even when stderr is redirected to
# a file, wrapping every field name in escape codes. Strip before matching.
ANSI_RE = re.compile(r"\x1b\[[0-9;]*m")

SCAN_RE = re.compile(
    r"indexed_get net=(\S+) scanned=(\d+) map_len=(\d+) "
    r'outcome="(\w+)" n_subnets=(\d+)'
)
INSERT_RE = re.compile(
    r"map_insert net=(\S+) idx=(\d+) map_len=(\d+) shift=(\d+) "
    r"sort_key=(\d+) order_depth=(\d+)"
)


def pct(n, d):
    return f"{100.0 * n / d:.1f}%" if d else "n/a"


def new_scan():
    return {
        "calls": 0,
        "scanned": 0,
        "max_len": 0,
        "outcome": collections.Counter(),
    }


def new_insert():
    return {
        "calls": 0,
        "shifted": 0,
        "max_shift": 0,
        "max_len": 0,
        "zero": 0,
        "gt1000": 0,
        "regressions": 0,
        "last_key": {},
    }


def parse(path, want_net):
    scans = collections.defaultdict(new_scan)
    inserts = collections.defaultdict(new_insert)

    with open(path, errors="replace") as fh:
        for raw in fh:
            line = ANSI_RE.sub("", raw)

            match = SCAN_RE.search(line)
            if match:
                net, scanned, map_len, outcome, _subnets = match.groups()
                if want_net and net != want_net:
                    continue
                entry = scans[net]
                entry["calls"] += 1
                entry["scanned"] += int(scanned)
                entry["max_len"] = max(entry["max_len"], int(map_len))
                entry["outcome"][outcome] += 1
                continue

            match = INSERT_RE.search(line)
            if match:
                net, _idx, map_len, shift, sort_key, depth = match.groups()
                if want_net and net != want_net:
                    continue
                entry = inserts[net]
                entry["calls"] += 1
                entry["shifted"] += int(shift)
                entry["max_shift"] = max(entry["max_shift"], int(shift))
                entry["max_len"] = max(entry["max_len"], int(map_len))
                if int(shift) == 0:
                    entry["zero"] += 1
                if int(shift) > 1000:
                    entry["gt1000"] += 1
                # A sort key at or below the previous one at the same order
                # depth means the counter re-entered an occupied range, which
                # is what produces mid-map inserts instead of tail appends.
                key, order_depth = int(sort_key), int(depth)
                if (
                    order_depth in entry["last_key"]
                    and key <= entry["last_key"][order_depth]
                ):
                    entry["regressions"] += 1
                entry["last_key"][order_depth] = key

    return scans, inserts


def report_scans(scans, top):
    print("=== indexed_get scan cost (by entries scanned) ===")
    print(
        "note: one logical lookup emits one record per subnet-recursion level;\n"
        "      'calls' counts records, not logical lookups (flat nets: 1:1)."
    )
    print(f"{'net':14} {'calls':>8} {'scanned':>12} {'avg':>8} {'max_len':>8}  outcomes")
    ranked = sorted(scans.items(), key=lambda kv: -kv[1]["scanned"])
    for net, entry in ranked[:top]:
        avg = entry["scanned"] / entry["calls"] if entry["calls"] else 0
        mix = " ".join(f"{k}={v}" for k, v in entry["outcome"].most_common())
        print(
            f"{net:14} {entry['calls']:8} {entry['scanned']:12} {avg:8.1f} "
            f"{entry['max_len']:8}  {mix}"
        )
    total = sum(e["scanned"] for e in scans.values())
    print(f"\ntotal entries scanned across all nets: {total:,}")


def report_inserts(inserts, top):
    print("\n=== map_insert shift cost (by entries shifted) ===")
    print(
        f"{'net':14} {'calls':>8} {'shifted':>12} {'max_shift':>10} "
        f"{'max_len':>8} {'shift=0':>9} {'>1000':>7} {'key_regr':>9}"
    )
    ranked = sorted(inserts.items(), key=lambda kv: -kv[1]["shifted"])
    for net, entry in ranked[:top]:
        print(
            f"{net:14} {entry['calls']:8} {entry['shifted']:12} "
            f"{entry['max_shift']:10} {entry['max_len']:8} "
            f"{pct(entry['zero'], entry['calls']):>9} "
            f"{pct(entry['gt1000'], entry['calls']):>7} "
            f"{pct(entry['regressions'], entry['calls']):>9}"
        )
    total = sum(e["shifted"] for e in inserts.values())
    print(f"\ntotal entries shifted across all nets: {total:,}")


def main():
    if len(sys.argv) < 2:
        print(__doc__)
        sys.exit(1)

    log_path = sys.argv[1]
    want_net = None
    if "--net" in sys.argv:
        want_net = sys.argv[sys.argv.index("--net") + 1]
    top = 10
    if "--top" in sys.argv:
        top = int(sys.argv[sys.argv.index("--top") + 1])

    scans, inserts = parse(log_path, want_net)

    if not scans and not inserts:
        print(
            "No PathMap telemetry found. Enable the probes with:\n"
            "  RUST_LOG='info,noet_core::paths::scan=debug,"
            "noet_core::paths::perf=debug'"
        )
        sys.exit(1)

    report_scans(scans, top)
    report_inserts(inserts, top)


if __name__ == "__main__":
    main()
