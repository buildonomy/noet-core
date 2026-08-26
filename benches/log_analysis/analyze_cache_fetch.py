#!/usr/bin/env python3
"""Summarize `GraphBuilder::cache_fetch` resolution sources and local-cache size.

Issue 97 Bottlenecks 4 and 5 both show the same suspected signature: a local
PathMap far smaller than the authoritative membership (70 entries vs. 17,706
in the DB for the warm-cache regression; 3,465 entries absorbing 304K inserts
for the end-of-run insert storm). Neither was directly instrumented — both
were inferred from timing distribution and namespace size snapshots taken by
hand.

`GraphBuilder::cache_fetch` (noet-core/src/codec/builder.rs) is the single
call site every resolution path (doc_bb / session_bb / global_bb, DB-backed
or in-memory) routes through. It now emits two debug events on the
`noet_core::codec::cache_fetch_census` target:

  `[cache_fetch] census`                  — every call: source, n_keys,
                                             parse_number, elapsed_us
  `[cache_fetch] global_cache_miss_local` — only when source=GlobalCache:
                                             session_bb_nodes,
                                             session_bb_asset_len,
                                             session_bb_href_len

This tool aggregates both: the source-outcome mix over the run (how often did
resolution fall through to global_bb/DB?) and, for GlobalCache outcomes only,
a time series of session_bb's local PathMap sizes so the local-vs-authoritative
divergence in Bottlenecks 4/5 can be seen directly rather than inferred.

Usage:
    RUST_LOG='info,noet_core::codec::cache_fetch_census=debug' \\
        noet parse --db --no-progress <corpus> > run.log 2>&1
    python3 benches/log_analysis/analyze_cache_fetch.py run.log

Requirements: Python 3.10+, no third-party packages.
"""

import argparse
import re
import statistics
from collections import Counter

ANSI_RE = re.compile(r"\x1b\[[0-9;]*m")
CENSUS_MARKER = "[cache_fetch] census"
MISS_LOCAL_MARKER = "[cache_fetch] global_cache_miss_local"

SOURCE_RE = re.compile(r"source=(\w+)")
N_KEYS_RE = re.compile(r"n_keys=(\d+)")
PARSE_NUMBER_RE = re.compile(r"parse_number=(\d+)")
ELAPSED_US_RE = re.compile(r"elapsed_us=(\d+)")
SESSION_BB_NODES_RE = re.compile(r"session_bb_nodes=(\d+)")
SESSION_BB_ASSET_RE = re.compile(r"session_bb_asset_len=(\d+)")
SESSION_BB_HREF_RE = re.compile(r"session_bb_href_len=(\d+)")


def strip_ansi(s: str) -> str:
    return ANSI_RE.sub("", s)


def main() -> None:
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    ap.add_argument("log", help="Path to the log file to analyze")
    ap.add_argument("--top", type=int, default=10, help="Rows in ranked tables")
    args = ap.parse_args()

    source_counts: Counter[str] = Counter()
    parse_number_by_source: dict[str, Counter[int]] = {}
    elapsed_by_source: dict[str, list[int]] = {}
    n_keys_total = 0
    n_calls = 0

    # Time series (in call order) of session_bb PathMap sizes, sampled only on
    # GlobalCache outcomes -- the arm that actually reached global_bb/DB.
    miss_local_samples: list[dict[str, int]] = []

    with open(args.log, errors="replace") as fh:
        for raw in fh:
            line = strip_ansi(raw.rstrip())
            if CENSUS_MARKER in line:
                n_calls += 1
                src_m = SOURCE_RE.search(line)
                src = src_m.group(1) if src_m else "?"
                source_counts[src] += 1

                pn_m = PARSE_NUMBER_RE.search(line)
                if pn_m:
                    parse_number_by_source.setdefault(src, Counter())[
                        int(pn_m.group(1))
                    ] += 1

                el_m = ELAPSED_US_RE.search(line)
                if el_m:
                    elapsed_by_source.setdefault(src, []).append(int(el_m.group(1)))

                nk_m = N_KEYS_RE.search(line)
                if nk_m:
                    n_keys_total += int(nk_m.group(1))
                continue

            if MISS_LOCAL_MARKER in line:
                sbn_m = SESSION_BB_NODES_RE.search(line)
                asset_m = SESSION_BB_ASSET_RE.search(line)
                href_m = SESSION_BB_HREF_RE.search(line)
                miss_local_samples.append(
                    {
                        "session_bb_nodes": int(sbn_m.group(1)) if sbn_m else 0,
                        "session_bb_asset_len": int(asset_m.group(1)) if asset_m else 0,
                        "session_bb_href_len": int(href_m.group(1)) if href_m else 0,
                    }
                )

    if n_calls == 0:
        print(
            "No '[cache_fetch] census' events found. This probe requires:\n"
            "  RUST_LOG='info,noet_core::codec::cache_fetch_census=debug'\n"
            "It is deliberately excluded from blanket RUST_LOG=debug to avoid\n"
            "flooding the log on every push_relation call — see\n"
            "benches/log_analysis/README.md."
        )
        return

    print("=" * 70)
    print("  cache_fetch source-outcome mix")
    print("=" * 70)
    print(f"  Total cache_fetch calls : {n_calls:,}")
    print(f"  Total keys examined     : {n_keys_total:,}")
    print()
    print(f"  {'Source':<14} {'Count':>10} {'% calls':>8} {'mean us':>10} {'p95 us':>10}")
    print(f"  {'-' * 14} {'-' * 10} {'-' * 8} {'-' * 10} {'-' * 10}")
    for src, count in source_counts.most_common():
        pct = 100.0 * count / n_calls
        elapsed = elapsed_by_source.get(src, [])
        mean_us = statistics.mean(elapsed) if elapsed else 0
        p95_us = (
            statistics.quantiles(elapsed, n=20)[18]
            if len(elapsed) >= 20
            else (max(elapsed) if elapsed else 0)
        )
        print(f"  {src:<14} {count:>10,} {pct:>7.1f}% {mean_us:>9.0f} {p95_us:>9.0f}")

    global_cache_count = source_counts.get("GlobalCache", 0)
    if global_cache_count:
        print(
            f"\n  GlobalCache outcomes (fell through to global_bb/DB): "
            f"{global_cache_count:,} / {n_calls:,} ({100.0 * global_cache_count / n_calls:.1f}%)"
        )
        print(
            "  This is the direct count corresponding to Bottleneck 5's inferred\n"
            "  '99.9% miss locally' — previously read off timing distribution."
        )

    print()
    print("  parse_number distribution by source:")
    for src in source_counts:
        pn_counts = parse_number_by_source.get(src, Counter())
        if pn_counts:
            dist = ", ".join(f"pn={pn}: {c}" for pn, c in sorted(pn_counts.items()))
            print(f"    {src:<14} {dist}")

    if miss_local_samples:
        print()
        print("=" * 70)
        print("  session_bb PathMap size at GlobalCache outcomes")
        print("  (Bottlenecks 4/5: local map size vs. authoritative membership)")
        print("=" * 70)
        nodes = [s["session_bb_nodes"] for s in miss_local_samples]
        asset_lens = [s["session_bb_asset_len"] for s in miss_local_samples]
        href_lens = [s["session_bb_href_len"] for s in miss_local_samples]

        def _stats(label: str, vals: list[int]) -> None:
            print(
                f"  {label:<24} min={min(vals):<8} max={max(vals):<8} "
                f"median={statistics.median(vals):<8.0f} mean={statistics.mean(vals):.0f}"
            )

        _stats("session_bb_nodes", nodes)
        _stats("session_bb_asset_len", asset_lens)
        _stats("session_bb_href_len", href_lens)

        print(f"\n  Samples: {len(miss_local_samples)}")
        print(
            "\n  A small, non-growing asset/href_len across many samples while\n"
            "  session_bb_nodes keeps climbing indicates the local PathMap is\n"
            "  not accumulating const-namespace registrations the way session_bb's\n"
            "  overall node count is -- the signature this probe was built to confirm."
        )

        # First vs. last few samples, to see whether the local map ever grows.
        n_show = min(args.top, len(miss_local_samples))
        print(f"\n  First {n_show} samples (call order):")
        print(f"  {'nodes':>10} {'asset_len':>10} {'href_len':>10}")
        for s in miss_local_samples[:n_show]:
            print(
                f"  {s['session_bb_nodes']:>10} {s['session_bb_asset_len']:>10} "
                f"{s['session_bb_href_len']:>10}"
            )
        if len(miss_local_samples) > n_show:
            print(f"\n  Last {n_show} samples (call order):")
            print(f"  {'nodes':>10} {'asset_len':>10} {'href_len':>10}")
            for s in miss_local_samples[-n_show:]:
                print(
                    f"  {s['session_bb_nodes']:>10} {s['session_bb_asset_len']:>10} "
                    f"{s['session_bb_href_len']:>10}"
                )


if __name__ == "__main__":
    main()
