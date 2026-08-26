#!/usr/bin/env python3
"""Attribute the pre-spawn epoch seeding cost in Issue 97 Bottleneck 7.

Bottleneck 7 measured a bimodal gap between a spawned task's
`Initializing GraphBuilder` line and its `[parse_epoch] task seeded` line:
439 tasks under 10 ms, 1,097 tasks over 5 s, mean 2.42 s, median 0.15 s, max
8.25 s. Both lines are emitted inside the same task future, so the gap should
be near-zero. Nothing logged inside it, so the responsible operation could
only be guessed at -- the standing theory being that `union_graphs` clones the
flat const-namespace snapshot (~78k children) on every task taking the
fallback branch.

Probes bracket that window, all on `noet_core::codec::perf` (`debug`).

THE SEEDING PATH CHANGED. Two mutually exclusive shapes appear in logs:

  LEGACY -- `[seed_session] session_bb built`, once per spawned task:
        seed_states_in, const_ns_states, merged_states, merged_edges,
        unioned, union_us, clone_us, rebuild_us, total_us

  CURRENT -- `[seed_session_from_base] session_bb built`, once per task:
        shared_states, doc_seed_states, base_clone_us, merge_us, total_us

The legacy path rebuilt a private `BeliefBase` per task, of which ~99.99% was
a const-namespace snapshot identical across every task in the epoch. The
current path builds that base once per epoch and clones it; `PathMapMap`'s
clone shares `PathMap`s by `Arc`, with copy-on-write on write. So `union_us`
and `rebuild_us` no longer exist -- the cost is now `base_clone_us` (cheap,
pointer copies) plus `merge_us` (the per-document seed).

This tool reads both and says which it found. A log with neither, under
`--jobs N>1`, means the probes did not fire -- not that seeding was free.

Supporting probes, both per-epoch:

  `[epoch_session_snapshot] built`   -- serial, before any task spawns:
        session_bb_nodes, session_bb_edges, network_bids, ns_bids,
        index_anchor_bids, included_bids, out_states, out_edges,
        part1_network_us, part2_const_ns_bfs_us, part3_index_anchors_us,
        state_clone_us, edge_filter_us, total_us

  `[parse_epoch] shared session base built`  -- CURRENT path only:
        states, build_us

  `[parse_epoch] pathmap copy-on-write counters`  -- CURRENT path only,
        process-wide and CUMULATIVE (this tool diffs consecutive epochs):
        cow_calls, cow_copies, cow_entries_copied, cow_us

The copy-on-write counters are a regression sentinel: a copies/calls ratio near
1.0 means sharing has become slower than the per-task rebuild it replaced.
Healthy baseline ~2.8%. See `make_pathmap_unique` in `src/paths/pathmap.rs` for
the mechanism and how it degrades.

Usage:
    RUST_LOG=debug noet parse --jobs 4 <corpus> > run.log 2>&1
    python3 benches/log_analysis/analyze_seed_session.py run.log

Requirements: Python 3.10+, no third-party packages.
"""

import argparse
import datetime as dt
import re
import statistics

ANSI_RE = re.compile(r"\x1b\[[0-9;]*m")
SEED_MARKER = "[seed_session] session_bb built"
BASE_SEED_MARKER = "[seed_session_from_base] session_bb built"
SHARED_BASE_MARKER = "[parse_epoch] shared session base built"
COW_MARKER = "[parse_epoch] pathmap copy-on-write counters"
SNAPSHOT_MARKER = "[epoch_session_snapshot] built"
# Leading tracing timestamp, e.g. `2026-08-22T12:01:51.229656Z`.
TIMESTAMP_RE = re.compile(r"^(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d+)Z")

# Field values are emitted unquoted by the tracing subscriber; `unioned` is a bool.
INT_FIELD_RE = {
    name: re.compile(rf"\b{name}=(\d+)")
    for name in (
        "seed_states_in",
        "const_ns_states",
        "merged_states",
        "merged_edges",
        "union_us",
        "clone_us",
        "rebuild_us",
        "session_bb_nodes",
        "session_bb_edges",
        "network_bids",
        "ns_bids",
        "index_anchor_bids",
        "included_bids",
        "out_states",
        "out_edges",
        "part1_network_us",
        "part2_const_ns_bfs_us",
        "part3_index_anchors_us",
        "state_clone_us",
        "edge_filter_us",
        "total_us",
        # Current (shared-base) seeding path.
        "shared_states",
        "doc_seed_states",
        "base_clone_us",
        "merge_us",
        # Shared epoch base construction.
        "states",
        "build_us",
        # PathMap copy-on-write counters (cumulative).
        "cow_calls",
        "cow_copies",
        "cow_entries_copied",
        "cow_us",
    )
}
UNIONED_RE = re.compile(r"\bunioned=(true|false)")


def strip_ansi(s: str) -> str:
    return ANSI_RE.sub("", s)


def parse_timestamp(line: str) -> dt.datetime | None:
    m = TIMESTAMP_RE.match(line)
    if not m:
        return None
    return dt.datetime.fromisoformat(m.group(1))


def parse_fields(line: str, names: tuple[str, ...]) -> dict[str, int]:
    out: dict[str, int] = {}
    for name in names:
        m = INT_FIELD_RE[name].search(line)
        out[name] = int(m.group(1)) if m else 0
    return out


def us(v: float) -> str:
    """Render microseconds in the largest sensible unit."""
    if v >= 1_000_000:
        return f"{v / 1_000_000:.2f}s"
    if v >= 1_000:
        return f"{v / 1_000:.1f}ms"
    return f"{v:.0f}us"


def stat_row(label: str, vals: list[int]) -> None:
    if not vals:
        print(f"  {label:<26} (no samples)")
        return
    p95 = (
        statistics.quantiles(vals, n=20)[18] if len(vals) >= 20 else max(vals)
    )
    print(
        f"  {label:<26} n={len(vals):<6} mean={us(statistics.mean(vals)):>9} "
        f"median={us(statistics.median(vals)):>9} p95={us(p95):>9} "
        f"max={us(max(vals)):>9} sum={us(sum(vals)):>9}"
    )


def main() -> None:
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    ap.add_argument("log", help="Path to the log file to analyze")
    ap.add_argument("--top", type=int, default=10, help="Rows in ranked tables")
    ap.add_argument(
        "--slow-us",
        type=int,
        default=5_000_000,
        help="Threshold for the slow-task tail (default 5s, matching Bottleneck 7)",
    )
    ap.add_argument(
        "--fast-us",
        type=int,
        default=10_000,
        help="Threshold for the fast-task mode (default 10ms, matching Bottleneck 7)",
    )
    ap.add_argument(
        "--skip-first-min",
        type=float,
        default=0.0,
        help=(
            "Drop events in the first N minutes of the run. Use when the start of "
            "a run competed with other load for CPU: contention inflates absolute "
            "timings and, because the first quarter is what growth-over-run is "
            "measured against, can mask or invert real growth."
        ),
    )
    args = ap.parse_args()

    seed_fields = (
        "seed_states_in",
        "const_ns_states",
        "merged_states",
        "merged_edges",
        "union_us",
        "clone_us",
        "rebuild_us",
        "total_us",
    )
    snapshot_fields = (
        "session_bb_nodes",
        "session_bb_edges",
        "network_bids",
        "ns_bids",
        "index_anchor_bids",
        "included_bids",
        "out_states",
        "out_edges",
        "part1_network_us",
        "part2_const_ns_bfs_us",
        "part3_index_anchors_us",
        "state_clone_us",
        "edge_filter_us",
        "total_us",
    )

    base_seed_fields = (
        "shared_states",
        "doc_seed_states",
        "base_clone_us",
        "merge_us",
        "total_us",
    )
    shared_base_fields = ("states", "build_us")
    cow_fields = ("cow_calls", "cow_copies", "cow_entries_copied", "cow_us")

    seeds: list[dict[str, int | bool]] = []
    base_seeds: list[dict[str, int]] = []
    shared_bases: list[dict[str, int]] = []
    cows: list[dict[str, int]] = []
    snapshots: list[dict[str, int]] = []
    run_start: dt.datetime | None = None
    n_skipped = 0

    with open(args.log, errors="replace") as fh:
        for raw in fh:
            line = strip_ansi(raw.rstrip())
            # Order matters: BASE_SEED_MARKER is not a substring of SEED_MARKER
            # ("[seed_session]" vs "[seed_session_from_base]"), but check the more
            # specific marker first so a future rename cannot silently alias them.
            is_base_seed = BASE_SEED_MARKER in line
            is_seed = (not is_base_seed) and SEED_MARKER in line
            is_shared_base = SHARED_BASE_MARKER in line
            is_cow = COW_MARKER in line
            is_snapshot = SNAPSHOT_MARKER in line
            if not (is_seed or is_base_seed or is_shared_base or is_cow or is_snapshot):
                # Still track the first timestamped line so the skip window is
                # measured from the true start of the run, not the first probe.
                if run_start is None:
                    run_start = parse_timestamp(line)
                continue

            ts = parse_timestamp(line)
            if run_start is None:
                run_start = ts
            if args.skip_first_min and ts and run_start:
                if (ts - run_start).total_seconds() < args.skip_first_min * 60.0:
                    n_skipped += 1
                    continue

            if is_seed:
                rec: dict[str, int | bool] = dict(parse_fields(line, seed_fields))
                m = UNIONED_RE.search(line)
                rec["unioned"] = bool(m and m.group(1) == "true")
                seeds.append(rec)
            elif is_base_seed:
                base_seeds.append(parse_fields(line, base_seed_fields))
            elif is_shared_base:
                shared_bases.append(parse_fields(line, shared_base_fields))
            elif is_cow:
                cows.append(parse_fields(line, cow_fields))
            else:
                snapshots.append(parse_fields(line, snapshot_fields))

    if args.skip_first_min:
        print(
            f"NOTE: skipped {n_skipped:,} events in the first "
            f"{args.skip_first_min:g} min of the run.\n"
            "      Absolute totals below therefore cover only the retained window.\n"
        )

    if not seeds and not base_seeds and not snapshots:
        print(
            "No seeding events found.\n"
            "Expected one of:\n"
            "  '[seed_session_from_base] session_bb built'  (current path)\n"
            "  '[seed_session] session_bb built'            (legacy path)\n"
            "  '[epoch_session_snapshot] built'\n"
            "These probes are on the noet_core::codec::perf target at debug level:\n"
            "  RUST_LOG=debug\n"
            "They only fire under parallel dispatch (--jobs N>1); a --jobs 1 run\n"
            "takes the sequential path and spawns no tasks.\n"
            "See benches/log_analysis/README.md."
        )
        return

    # Say which seeding path produced this log. Silence here previously meant a
    # log from the current path rendered as "no tasks" with no explanation.
    if seeds and base_seeds:
        print(
            f"NOTE: log contains BOTH seeding paths — {len(seeds):,} legacy and "
            f"{len(base_seeds):,} shared-base tasks.\n"
            "      Expected only during epoch-0 fallback; both are reported below.\n"
        )
    elif base_seeds:
        print(f"Seeding path: shared epoch base ({len(base_seeds):,} tasks).\n")
    elif seeds:
        print(
            f"Seeding path: legacy per-task rebuild ({len(seeds):,} tasks).\n"
            "      This log predates the shared epoch base.\n"
        )

    # ── seed_session: per-task cost, the inner half of the Bottleneck 7 gap ──
    if seeds:
        totals = [int(s["total_us"]) for s in seeds]
        print("=" * 78)
        print("  seed_session -- per-task, inside the spawned task future")
        print("=" * 78)
        print(f"  Tasks seeded: {len(seeds):,}")
        print(f"  Total seeding time (summed across tasks): {us(sum(totals))}")
        print(
            "  NOTE: tasks run concurrently, so this sum double-counts overlapping\n"
            "  wall clock -- same caveat as Bottleneck 7's 8,772s figure.\n"
        )
        stat_row("total", totals)
        stat_row("  union_us", [int(s["union_us"]) for s in seeds])
        stat_row("  clone_us", [int(s["clone_us"]) for s in seeds])
        stat_row("  rebuild_us", [int(s["rebuild_us"]) for s in seeds])

        # Attribution: which sub-step dominates the summed cost?
        sub_sums = {
            "union_graphs": sum(int(s["union_us"]) for s in seeds),
            "graph clone": sum(int(s["clone_us"]) for s in seeds),
            "BeliefBase::from rebuild": sum(int(s["rebuild_us"]) for s in seeds),
        }
        accounted = sum(sub_sums.values())
        total_sum = sum(totals)
        print(f"\n  Attribution of {us(total_sum)} total:")
        for label, val in sorted(sub_sums.items(), key=lambda kv: -kv[1]):
            pct = 100.0 * val / total_sum if total_sum else 0.0
            print(f"    {label:<26} {us(val):>10}  {pct:>5.1f}%")
        unattributed = total_sum - accounted
        pct = 100.0 * unattributed / total_sum if total_sum else 0.0
        print(f"    {'(unattributed)':<26} {us(unattributed):>10}  {pct:>5.1f}%")
        if pct > 20:
            print(
                "    ^ a large unattributed share means the cost is NOT in the three\n"
                "      timed sub-steps; look outside seed_session for the gap."
            )

        # Bimodality: does the fast/slow split track the `unioned` branch?
        fast = [s for s in seeds if int(s["total_us"]) < args.fast_us]
        slow = [s for s in seeds if int(s["total_us"]) > args.slow_us]
        print(
            f"\n  Bimodality check (Bottleneck 7 saw 439 <{us(args.fast_us)}, "
            f"1,097 >{us(args.slow_us)}):"
        )
        print(f"    fast (<{us(args.fast_us)}): {len(fast):,}")
        print(f"    slow (>{us(args.slow_us)}): {len(slow):,}")
        print(f"    middle              : {len(seeds) - len(fast) - len(slow):,}")

        for label, group in (("fast", fast), ("slow", slow)):
            if not group:
                continue
            n_union = sum(1 for s in group if s["unioned"])
            pct_union = 100.0 * n_union / len(group)
            mean_in = statistics.mean(int(s["seed_states_in"]) for s in group)
            mean_ns = statistics.mean(int(s["const_ns_states"]) for s in group)
            mean_out = statistics.mean(int(s["merged_states"]) for s in group)
            print(
                f"    {label:<5} unioned={pct_union:.0f}%  "
                f"mean seed_states_in={mean_in:,.0f}  "
                f"const_ns_states={mean_ns:,.0f}  merged_states={mean_out:,.0f}"
            )
        print(
            "\n  If slow tasks are ~100% unioned and fast ones ~0%, the union branch\n"
            "  is the discriminator. If both branches are slow, the cost is the clone\n"
            "  or rebuild of a large graph regardless of how it was assembled."
        )

        # Does per-task cost grow over the run?
        if len(seeds) >= 4:
            q = len(seeds) // 4
            first_q = [int(s["total_us"]) for s in seeds[:q]]
            last_q = [int(s["total_us"]) for s in seeds[-q:]]
            print("\n  Growth over the run (does cost scale with accumulated corpus?):")
            print(
                f"    first quarter mean={us(statistics.mean(first_q))}  "
                f"merged_states={statistics.mean(int(s['merged_states']) for s in seeds[:q]):,.0f}"
            )
            print(
                f"    last  quarter mean={us(statistics.mean(last_q))}  "
                f"merged_states={statistics.mean(int(s['merged_states']) for s in seeds[-q:]):,.0f}"
            )

        print(f"\n  Slowest {min(args.top, len(seeds))} tasks:")
        print(
            f"    {'total':>9} {'union':>9} {'clone':>9} {'rebuild':>9} "
            f"{'in':>8} {'const_ns':>9} {'merged':>8}  unioned"
        )
        for s in sorted(seeds, key=lambda r: -int(r["total_us"]))[: args.top]:
            print(
                f"    {us(int(s['total_us'])):>9} {us(int(s['union_us'])):>9} "
                f"{us(int(s['clone_us'])):>9} {us(int(s['rebuild_us'])):>9} "
                f"{int(s['seed_states_in']):>8,} {int(s['const_ns_states']):>9,} "
                f"{int(s['merged_states']):>8,}  {s['unioned']}"
            )

    # ── seed_session_from_base: current path, per-task clone + merge ─────────
    if base_seeds:
        totals = [s["total_us"] for s in base_seeds]
        print("=" * 78)
        print("  seed_session_from_base -- per-task (clone shared base, merge doc seed)")
        print("=" * 78)
        print(f"  Tasks seeded: {len(base_seeds):,}")
        print(f"  Total seeding time (summed across tasks): {us(sum(totals))}")
        print(
            "  NOTE: tasks run concurrently, so this sum double-counts overlapping\n"
            "  wall clock. Compare against the legacy path's summed total, not against\n"
            "  wall clock.\n"
        )
        stat_row("total", totals)
        stat_row("  base_clone_us", [s["base_clone_us"] for s in base_seeds])
        stat_row("  merge_us", [s["merge_us"] for s in base_seeds])

        sub_sums = {
            "shared base clone": sum(s["base_clone_us"] for s in base_seeds),
            "doc-seed merge": sum(s["merge_us"] for s in base_seeds),
        }
        total_sum = sum(totals)
        print(f"\n  Attribution of {us(total_sum)} total:")
        for label, val in sorted(sub_sums.items(), key=lambda kv: -kv[1]):
            pct = 100.0 * val / total_sum if total_sum else 0.0
            print(f"    {label:<26} {us(val):>10}  {pct:>5.1f}%")
        unattributed = total_sum - sum(sub_sums.values())
        pct = 100.0 * unattributed / total_sum if total_sum else 0.0
        print(f"    {'(unattributed)':<26} {us(unattributed):>10}  {pct:>5.1f}%")

        mean_shared = statistics.mean(s["shared_states"] for s in base_seeds)
        mean_doc = statistics.mean(s["doc_seed_states"] for s in base_seeds)
        print(
            f"\n  Mean shared_states={mean_shared:,.0f}  doc_seed_states={mean_doc:,.0f}"
        )
        if mean_shared > 0:
            ratio = 100.0 * mean_doc / (mean_shared + mean_doc)
            print(
                f"  A task's own seed is {ratio:.2f}% of what it holds. The rest is shared\n"
                "  and no longer rebuilt per task -- but it IS still carried per task.\n"
                "  Only demand-driven seeding reduces that."
            )

        if len(base_seeds) >= 4:
            q = len(base_seeds) // 4
            first_q = [s["total_us"] for s in base_seeds[:q]]
            last_q = [s["total_us"] for s in base_seeds[-q:]]
            print("\n  Growth over the run:")
            print(
                f"    first quarter mean={us(statistics.mean(first_q))}  "
                f"shared_states={statistics.mean(s['shared_states'] for s in base_seeds[:q]):,.0f}"
            )
            print(
                f"    last  quarter mean={us(statistics.mean(last_q))}  "
                f"shared_states={statistics.mean(s['shared_states'] for s in base_seeds[-q:]):,.0f}"
            )

        print(f"\n  Slowest {min(args.top, len(base_seeds))} tasks:")
        print(f"    {'total':>9} {'clone':>9} {'merge':>9} {'shared':>9} {'doc_seed':>9}")
        for s in sorted(base_seeds, key=lambda r: -r["total_us"])[: args.top]:
            print(
                f"    {us(s['total_us']):>9} {us(s['base_clone_us']):>9} "
                f"{us(s['merge_us']):>9} {s['shared_states']:>9,} "
                f"{s['doc_seed_states']:>9,}"
            )
        print()

    # ── shared epoch base construction: once per epoch, serial ───────────────
    if shared_bases:
        builds = [s["build_us"] for s in shared_bases]
        print("=" * 78)
        print("  shared session base -- built once per epoch, serial")
        print("=" * 78)
        print(f"  Epochs: {len(shared_bases):,}   Total: {us(sum(builds))}")
        stat_row("build", builds)
        print(
            f"  Final base size: {shared_bases[-1]['states']:,} states\n"
            "  This is the cost the per-task rebuild was replaced BY. It should be\n"
            "  a small multiple of one task's old rebuild, not of all tasks'.\n"
        )

    # ── PathMap copy-on-write: the sentinel for whether sharing stays cheap ──
    if cows:
        # Counters are process-wide and cumulative; the last line is the run total.
        last = cows[-1]
        calls, copies = last["cow_calls"], last["cow_copies"]
        entries, cow_us_total = last["cow_entries_copied"], last["cow_us"]
        ratio = 100.0 * copies / calls if calls else 0.0
        print("=" * 78)
        print("  PathMap copy-on-write -- sentinel: is sharing still cheap?")
        print("=" * 78)
        print(f"  Uniqueness checks : {calls:,}")
        print(f"  Actual copies     : {copies:,}  ({ratio:.2f}% of checks)")
        print(f"  Entries copied    : {entries:,}")
        print(f"  Time in copies    : {us(cow_us_total)}")
        if calls:
            print(f"  Mean entries/copy : {entries / copies:,.0f}" if copies else "")

        if ratio >= 50.0:
            print(
                "\n  *** THRASHING. Over half of all writes are paying for a copy. ***\n"
                "  Sharing is likely now SLOWER than the per-task rebuild it replaced.\n"
                "  Most likely cause: a read guard (`get_map`/`href_map`/`api_map`, all\n"
                "  of which return `read_arc()`) is being held across a write, inflating\n"
                "  `Arc::strong_count` so the map looks shared when it is not."
            )
        elif ratio >= 10.0:
            print(
                f"\n  WARNING: {ratio:.1f}% copy rate, against a ~2.8% healthy baseline.\n"
                "  Look for a newly-introduced read guard held across a write."
            )
        else:
            print(f"\n  Healthy (baseline ~2.8%). Copy cost is {us(cow_us_total)}.")

        # Per-epoch deltas: a ratio that degrades late in the run is the shape to
        # catch, and the cumulative total hides it.
        if len(cows) >= 2:
            print("\n  Per-epoch (deltas of the cumulative counters):")
            print(f"    {'epoch':>5} {'checks':>10} {'copies':>9} {'rate':>7} {'entries':>11} {'time':>9}")
            prev = {k: 0 for k in cow_fields}
            for i, c in enumerate(cows):
                d_calls = c["cow_calls"] - prev["cow_calls"]
                d_copies = c["cow_copies"] - prev["cow_copies"]
                d_entries = c["cow_entries_copied"] - prev["cow_entries_copied"]
                d_us = c["cow_us"] - prev["cow_us"]
                d_ratio = 100.0 * d_copies / d_calls if d_calls else 0.0
                print(
                    f"    {i:>5} {d_calls:>10,} {d_copies:>9,} {d_ratio:>6.1f}% "
                    f"{d_entries:>11,} {us(d_us):>9}"
                )
                prev = c
        print()

    # ── epoch_session_snapshot: per-epoch, fully serial before any spawn ─────
    if snapshots:
        totals = [s["total_us"] for s in snapshots]
        print()
        print("=" * 78)
        print("  epoch_session_snapshot -- per-epoch, serial (blocks all spawning)")
        print("=" * 78)
        print(f"  Epochs: {len(snapshots):,}")
        print(
            f"  Total: {us(sum(totals))}  "
            "(this one IS wall clock -- it runs before any task spawns)\n"
        )
        stat_row("total", totals)
        stat_row("  part1 network scan", [s["part1_network_us"] for s in snapshots])
        stat_row("  part2 const-ns BFS", [s["part2_const_ns_bfs_us"] for s in snapshots])
        stat_row("  part3 index anchors", [s["part3_index_anchors_us"] for s in snapshots])
        stat_row("  state clone", [s["state_clone_us"] for s in snapshots])
        stat_row("  edge filter", [s["edge_filter_us"] for s in snapshots])

        sub_sums = {
            "part1 network scan": sum(s["part1_network_us"] for s in snapshots),
            "part2 const-ns BFS": sum(s["part2_const_ns_bfs_us"] for s in snapshots),
            "part3 index anchors": sum(s["part3_index_anchors_us"] for s in snapshots),
            "state clone": sum(s["state_clone_us"] for s in snapshots),
            "edge filter": sum(s["edge_filter_us"] for s in snapshots),
        }
        total_sum = sum(totals)
        print(f"\n  Attribution of {us(total_sum)} total:")
        for label, val in sorted(sub_sums.items(), key=lambda kv: -kv[1]):
            pct = 100.0 * val / total_sum if total_sum else 0.0
            print(f"    {label:<26} {us(val):>10}  {pct:>5.1f}%")

        if len(snapshots) >= 4:
            q = len(snapshots) // 4
            print("\n  Growth over the run:")
            for label, chunk in (
                ("first quarter", snapshots[:q]),
                ("last  quarter", snapshots[-q:]),
            ):
                print(
                    f"    {label} mean total={us(statistics.mean(s['total_us'] for s in chunk))}  "
                    f"ns_bids={statistics.mean(s['ns_bids'] for s in chunk):,.0f}  "
                    f"out_states={statistics.mean(s['out_states'] for s in chunk):,.0f}"
                )
            print(
                "\n  ns_bids growing while total climbs is the const-namespace signature:\n"
                "  the BFS walks a hub whose breadth grows with the corpus."
            )


if __name__ == "__main__":
    main()
