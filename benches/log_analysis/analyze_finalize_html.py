#!/usr/bin/env python3
"""Attribute `finalize_html`'s wall-clock time to its constituent stages.

`DocumentCompiler::finalize_html` (noet-core/src/codec/compiler.rs) emits a
`[finalize_html stage] <name>` debug event with `elapsed_ms` around each stage:

    generate_deferred_html
    generate_sitemap
    asset_manifest submap
    create_asset_hardlinks
    export_beliefgraph
    BeliefBase::from(graph) rebuild
    compute_layout_metadata
    build_search_indices
    export_beliefbase
    generate_spa_shell

This tool sums those events (there should be exactly one of each per run;
finalize_html is only called once per parse) and reports each stage's wall
time and share of the finalize_html total, plus whether the wall span between
the "Asset hardlinks created" log line and the last stage event is fully
accounted for by instrumented stages.

Usage:
    RUST_LOG=debug cargo run --features service,bin -- parse \\
        --html-output /tmp/out <corpus> > run.log 2>&1
    python3 benches/log_analysis/analyze_finalize_html.py run.log

Requirements: Python 3.10+, no third-party packages.
"""

import argparse
import re
import sys
from datetime import datetime, timezone

ANSI_RE = re.compile(r"\x1b\[[0-9;]*m")
TS_RE = re.compile(r"^(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d+)Z")
ELAPSED_RE = re.compile(r"elapsed_ms=(\d+)")
NODE_COUNT_RE = re.compile(r"node_count=(\d+)")
ASSET_COUNT_RE = re.compile(r"asset_count=(\d+)")
HARDLINK_MARKER = "Asset hardlinks created"
STAGE_MARKER = "[finalize_html stage]"

# `compute_layout_metadata` is the largest stage on large corpora, so it has its
# own per-step timers and a scope-resolution summary. Both are optional: older
# logs predate them and must still analyse.
LAYOUT_STEP_MARKER = "[layout step]"
LAYOUT_SCOPE_MARKER = "[layout] scope resolution complete"
FIELD_RE = re.compile(r"(\w+)=(\d+)")

STAGE_ORDER = [
    "generate_deferred_html",
    "generate_sitemap",
    "asset_manifest submap",
    "create_asset_hardlinks",
    "export_beliefgraph",
    "BeliefBase::from(graph) rebuild",
    "compute_layout_metadata",
    "build_search_indices",
    "export_beliefbase",
    "generate_spa_shell",
]


def strip_ansi(s: str) -> str:
    return ANSI_RE.sub("", s)


def parse_ts(line: str):
    m = TS_RE.match(line)
    if not m:
        return None
    return datetime.fromisoformat(m.group(1)).replace(tzinfo=timezone.utc)


def main() -> None:
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    ap.add_argument("log", help="Path to the log file to analyze")
    args = ap.parse_args()

    stages: dict[str, int] = {}
    extra: dict[str, dict[str, int]] = {}
    layout_steps: dict[str, int] = {}
    layout_scope: dict[str, int] = {}
    hardlink_ts = None
    last_stage_ts = None

    with open(args.log, errors="replace") as fh:
        for raw in fh:
            line = strip_ansi(raw.rstrip())
            if HARDLINK_MARKER in line and hardlink_ts is None:
                hardlink_ts = parse_ts(line)

            # Optional layout detail — absent from logs predating these timers.
            if LAYOUT_SCOPE_MARKER in line:
                layout_scope = dict(FIELD_RE.findall(line))
                layout_scope = {k: int(v) for k, v in layout_scope.items()}
                continue
            if LAYOUT_STEP_MARKER in line:
                idx = line.find(LAYOUT_STEP_MARKER) + len(LAYOUT_STEP_MARKER)
                rest = line[idx:].strip()
                m = re.search(r"\s\w+=", rest)
                step_name = rest[: m.start()] if m else rest
                em = ELAPSED_RE.search(line)
                if em:
                    layout_steps[step_name] = int(em.group(1))
                continue

            if STAGE_MARKER not in line:
                continue
            ts = parse_ts(line)
            if ts is not None:
                last_stage_ts = ts

            # Identify which known stage this line belongs to by substring
            # match (mirrors the "search for the literal marker" convention
            # used by parse_log.py / analyze_phase2.py rather than a fragile
            # positional regex, since tracing_subscriber's default formatter
            # emits "target: message field1=v1 field2=v2 ...").
            matched_name = None
            for name in STAGE_ORDER:
                if f"{STAGE_MARKER} {name}" in line:
                    matched_name = name
                    break
            if matched_name is None:
                # Unexpected stage name — still record it under the raw
                # suffix so it's visible rather than silently dropped.
                idx = line.find(STAGE_MARKER) + len(STAGE_MARKER)
                rest = line[idx:].strip()
                # Trim at the first "key=" field.
                m = re.search(r"\s\w+=", rest)
                matched_name = rest[: m.start()] if m else rest

            elapsed_m = ELAPSED_RE.search(line)
            if not elapsed_m:
                continue
            stages[matched_name] = int(elapsed_m.group(1))

            fields = {}
            nc = NODE_COUNT_RE.search(line)
            if nc:
                fields["node_count"] = int(nc.group(1))
            ac = ASSET_COUNT_RE.search(line)
            if ac:
                fields["asset_count"] = int(ac.group(1))
            if fields:
                extra[matched_name] = fields

    if not stages:
        print(
            "No '[finalize_html stage]' events found. This probe requires\n"
            "RUST_LOG=debug (target noet_core::codec::perf) and a run that\n"
            "reaches finalize_html (i.e. --html-output was passed)."
        )
        sys.exit(1)

    total_ms = sum(stages.values())
    print("=" * 70)
    print("  finalize_html stage breakdown")
    print("=" * 70)
    print(f"  Stages observed: {len(stages)} / {len(STAGE_ORDER)} expected\n")

    print(f"  {'Stage':<34} {'ms':>10} {'sec':>8} {'% total':>8}")
    print(f"  {'-' * 34} {'-' * 10} {'-' * 8} {'-' * 8}")
    ordered = [s for s in STAGE_ORDER if s in stages]
    unexpected = [s for s in stages if s not in STAGE_ORDER]
    for name in ordered + unexpected:
        ms = stages[name]
        pct = (100.0 * ms / total_ms) if total_ms else 0.0
        tag = "" if name in STAGE_ORDER else "  (unexpected)"
        print(f"  {name:<34} {ms:>10} {ms / 1000:>7.1f}s {pct:>7.1f}%{tag}")
        if name in extra:
            for k, v in extra[name].items():
                print(f"      {k}: {v:,}")

    print(f"\n  Sum of instrumented stages: {total_ms} ms ({total_ms / 1000:.1f}s)")

    report_layout_detail(layout_steps, layout_scope, stages.get("compute_layout_metadata"))

    missing = [s for s in STAGE_ORDER if s not in stages]
    if missing:
        print(f"\n  Stages NOT observed (still silent!): {', '.join(missing)}")
    else:
        print(
            "\n  All expected stages observed — no silent gap remains uninstrumented."
        )

    if hardlink_ts and last_stage_ts:
        span_s = (last_stage_ts - hardlink_ts).total_seconds()
        instrumented_from_hardlink_ms = sum(
            ms
            for name, ms in stages.items()
            if name in STAGE_ORDER
            and STAGE_ORDER.index(name) >= STAGE_ORDER.index("create_asset_hardlinks")
        )
        uncovered_s = span_s - instrumented_from_hardlink_ms / 1000.0
        print(
            f"\n  Wall span 'Asset hardlinks created' → last stage event: {span_s:.1f}s"
        )
        print(
            f"  Instrumented stages in that span   : {instrumented_from_hardlink_ms / 1000:.1f}s"
        )
        verdict = (
            "gap closed"
            if abs(uncovered_s) < 2
            else "STILL UNEXPLAINED"
            if uncovered_s > 2
            else "stage timers overlap wall span (concurrent work?)"
        )
        print(f"  Unaccounted wall time in that span : {uncovered_s:.1f}s  ({verdict})")


def report_layout_detail(steps, scope, layout_total_ms):
    """Break `compute_layout_metadata` into its steps and resolution routes.

    Silently does nothing on logs predating the per-step timers.
    """
    if not steps and not scope:
        return

    print("\n" + "=" * 70)
    print("  compute_layout_metadata detail")
    print("=" * 70)

    scope_ms = int(scope.get("elapsed_ms", 0)) if scope else 0
    rows = list(steps.items())
    if scope:
        rows.insert(0, ("scope resolution (pathmap.path)", scope_ms))

    denom = layout_total_ms or sum(ms for _, ms in rows) or 1
    print(f"\n  {'Step':<34} {'ms':>10} {'sec':>8} {'% stage':>8}")
    print(f"  {'-' * 34} {'-' * 10} {'-' * 8} {'-' * 8}")
    for name, ms in sorted(rows, key=lambda kv: -kv[1]):
        print(f"  {name:<34} {ms:>10} {ms / 1000:>7.1f}s {100.0 * ms / denom:>7.1f}%")

    if not scope:
        return

    # Scope resolution splits across indexed_path's two routes. The narrowed
    # route probes a few candidates; the fallback probes every network. Which
    # one dominates determines where optimisation effort belongs.
    ix_calls = int(scope.get("indexed_calls", 0))
    fb_calls = int(scope.get("fallback_calls", 0))
    ix_probes = int(scope.get("indexed_probes", 0))
    # Older logs carried per-route timings and a fallback probe count. Both were
    # dropped once the fallback was removed: an index miss probes nothing, and
    # timing this hot path cost more than it revealed.
    fb_probes = int(scope.get("fallback_probes", 0))
    ix_ms = int(scope.get("indexed_ms", 0))
    fb_ms = int(scope.get("fallback_ms", 0))
    if not (ix_calls or fb_calls):
        return

    # These counters are process-global and cumulative across the whole run,
    # while `scope_ms` times a single stage. `indexed_path` is called from many
    # sites (MCP tools, relation resolution, context building), so the counter
    # totals legitimately exceed one stage's cost — an earlier version of this
    # table divided the two and printed "109.1% of scope resolution".
    print("\n  indexed_path routes (whole run, cumulative; a 'probe' is one")
    print("  PathMap::path call). Not scoped to compute_layout_metadata:")
    has_ms = bool(ix_ms or fb_ms)
    ms_hdr = f" {'ms':>10}" if has_ms else ""
    ms_sep = f" {'-' * 10}" if has_ms else ""
    print(
        f"\n  {'route':<12} {'calls':>12} {'probes':>14} {'probes/call':>12}{ms_hdr} {'% probes':>9}"
    )
    print(f"  {'-' * 12} {'-' * 12} {'-' * 14} {'-' * 12}{ms_sep} {'-' * 9}")
    all_probes = ix_probes + fb_probes
    for label, calls, probes, ms in (
        ("indexed", ix_calls, ix_probes, ix_ms),
        ("fallback", fb_calls, fb_probes, fb_ms),
    ):
        per = (probes / calls) if calls else 0.0
        pct = (100.0 * probes / all_probes) if all_probes else 0.0
        ms_col = f" {ms:>10,}" if has_ms else ""
        print(
            f"  {label:<12} {calls:>12,} {probes:>14,} {per:>12.1f}{ms_col} {pct:>8.1f}%"
        )

    total_calls = ix_calls + fb_calls
    if total_calls:
        fb_call_share = 100.0 * fb_calls / total_calls
        fb_probe_share = (100.0 * fb_probes / all_probes) if all_probes else 0.0
        print(
            f"\n  Fallback is {fb_call_share:.1f}% of calls and "
            f"{fb_probe_share:.1f}% of probes."
        )
        if fb_probes == 0 and fb_calls:
            print(
                "  -> Index misses short-circuit to None without probing (expected).\n"
                "     A nonzero figure here means the exhaustive scan is back."
            )
        elif fb_probe_share > 50.0:
            print(
                "  -> The exhaustive fallback dominates. Narrowing the indexed\n"
                "     route further is wasted effort; reduce fallback *entries*\n"
                "     (BIDs missing from node_to_nets) instead."
            )
        elif ix_probes:
            print(
                "  -> The narrowed route dominates. Cost is per-probe, inside\n"
                "     PathMap::path itself, not in candidate-set width."
            )


if __name__ == "__main__":
    main()
