#!/usr/bin/env python3
"""
Performance analysis script for req12.log
Analyzes Phase 2 timing data across parse epochs.
"""

import re
from datetime import datetime

ANSI_ESCAPE = re.compile(r"\x1b\[[0-9;]*[mK]")


def strip_ansi(text):
    return ANSI_ESCAPE.sub("", text)


def parse_ts(ts_str):
    return datetime.fromisoformat(ts_str.rstrip("Z"))


def ts_diff_ms(t1, t2):
    """t2 - t1 in ms"""
    return (t2 - t1).total_seconds() * 1000


def short_name(full_path):
    parts = full_path.rstrip("/").split("/")
    # Return last 2 components, e.g. "derived_requirements/index.md"
    return "/".join(parts[-2:])


def load_log(path):
    with open(path) as f:
        return f.read()


def parse_push_rel(log):
    pattern = re.compile(
        r"(\S+)  INFO.*\[Phase 2\] push_relation loop complete "
        r"file_path=(\S+) push_relation_ms=(\d+) phase2_total_ms=(\d+)"
    )
    rows = []
    for m in pattern.finditer(log):
        rows.append(
            {
                "ts": parse_ts(m.group(1)),
                "short": short_name(m.group(2)),
                "full_path": m.group(2),
                "push_relation_ms": int(m.group(3)),
                "phase2_total_ms": int(m.group(4)),
            }
        )
    return rows


def parse_aeb(log):
    """Parse [apply_events_batch] timing breakdown lines with label=doc_bb"""
    pattern = re.compile(
        r"(\S+)  INFO.*\[apply_events_batch\] timing breakdown "
        r'label="doc_bb" '
        r"pass1_n_node_update=(\d+) pass1_n_node_upsert=(\d+) "
        r"pass1_insert_ms=(\d+) pass1_total_ms=(\d+) "
        r"n_relation_change=(\d+) n_relation_update=(\d+) "
        r"n_skipped=(\d+) n_no_change=(\d+) "
        r"sort_key_ms=(\d+) gen_edge_ms=(\d+) update_rel_ms=(\d+)"
    )
    rows = []
    for m in pattern.finditer(log):
        rows.append(
            {
                "ts": parse_ts(m.group(1)),
                "pass1_n_node_upsert": int(m.group(3)),
                "pass1_insert_ms": int(m.group(4)),
                "pass1_total_ms": int(m.group(5)),
                "n_relation_change": int(m.group(6)),
                "n_no_change": int(m.group(9)),
                "sort_key_ms": int(m.group(10)),
                "gen_edge_ms": int(m.group(11)),
                "update_rel_ms": int(m.group(12)),
            }
        )
    return rows


def parse_flush(log):
    pattern = re.compile(
        r"(\S+)  INFO.*\[Phase 2\] doc_bb\.apply_events_batch \+ flush_paths "
        r"file_path=(\S+) relation_event_count=(\d+) elapsed_ms=(\d+)"
    )
    rows = []
    for m in pattern.finditer(log):
        rows.append(
            {
                "ts": parse_ts(m.group(1)),
                "short": short_name(m.group(2)),
                "full_path": m.group(2),
                "relation_event_count": int(m.group(3)),
                "elapsed_ms": int(m.group(4)),
            }
        )
    return rows


def align_triplets(push_rel_rows, aeb_rows, flush_rows):
    """
    For each flush row, find the nearest preceding push_rel row for the same file,
    then find the nearest aeb (doc_bb) row between them.
    Returns a list of aligned dicts.
    """
    results = []
    for f_row in flush_rows:
        fp = f_row["full_path"]
        f_ts = f_row["ts"]

        # Latest push_rel for same file before this flush
        candidates_pr = [
            r for r in push_rel_rows if r["full_path"] == fp and r["ts"] < f_ts
        ]
        if not candidates_pr:
            continue
        pr_row = max(candidates_pr, key=lambda r: r["ts"])

        # Latest aeb row between push_rel and flush
        candidates_aeb = [r for r in aeb_rows if pr_row["ts"] <= r["ts"] <= f_ts]
        if not candidates_aeb:
            continue
        aeb_row = max(candidates_aeb, key=lambda r: r["ts"])

        gap_ms = round(ts_diff_ms(pr_row["ts"], aeb_row["ts"]))
        elapsed_ms = f_row["elapsed_ms"]
        pass1_total_ms = aeb_row["pass1_total_ms"]
        flush_dominant_ms = elapsed_ms - pass1_total_ms

        results.append(
            {
                "file": f_row["short"],
                "full_path": fp,
                "epoch_ts": pr_row["ts"],
                "push_relation_ms": pr_row["push_relation_ms"],
                "phase2_total_ms": pr_row["phase2_total_ms"],
                "gap_after_push_ms": gap_ms,
                "pass1_insert_ms": aeb_row["pass1_insert_ms"],
                "pass1_total_ms": pass1_total_ms,
                "gen_edge_ms": aeb_row["gen_edge_ms"],
                "update_rel_ms": aeb_row["update_rel_ms"],
                "sort_key_ms": aeb_row["sort_key_ms"],
                "n_relation_change": aeb_row["n_relation_change"],
                "n_no_change": aeb_row["n_no_change"],
                "pass1_n_node_upsert": aeb_row["pass1_n_node_upsert"],
                "elapsed_ms": elapsed_ms,
                "flush_dominant_ms": flush_dominant_ms,
                "relation_event_count": f_row["relation_event_count"],
            }
        )
    return results


def wall_clock(log):
    ts_pattern = re.compile(
        r"^(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d+Z)", re.MULTILINE
    )
    all_ts = [parse_ts(m.group(1)) for m in ts_pattern.finditer(log)]
    if not all_ts:
        return None, None, None
    first = min(all_ts)
    last = max(all_ts)
    total_s = (last - first).total_seconds()
    return first, last, total_s


def print_all_triplets(results):
    print("\n" + "=" * 90)
    print("ALL ALIGNED TRIPLETS (push_relation -> apply_events_batch -> flush_paths)")
    print("=" * 90)
    for i, r in enumerate(results):
        print(f"\n[{i + 1}] {r['epoch_ts'].strftime('%H:%M:%S')}  {r['file']}")
        print(
            f"     push_relation_ms : {r['push_relation_ms']:>8}  phase2_total_ms : {r['phase2_total_ms']:>8}"
        )
        print(
            f"     gap_after_push   : {r['gap_after_push_ms']:>8} ms  (merge_from + interim work)"
        )
        print(
            f"     pass1_total_ms   : {r['pass1_total_ms']:>8} ms  (insert={r['pass1_insert_ms']}ms  "
            f"gen_edge={r['gen_edge_ms']}ms  update_rel={r['update_rel_ms']}ms  sort_key={r['sort_key_ms']}ms)"
        )
        print(f"     elapsed_ms       : {r['elapsed_ms']:>8} ms  (aeb + flush total)")
        print(
            f"     flush_dominant   : {r['flush_dominant_ms']:>8} ms  (elapsed - pass1_total; flush_paths_for_events cost)"
        )
        print(
            f"     n_upsert={r['pass1_n_node_upsert']}  n_relation_change={r['n_relation_change']}  "
            f"n_no_change={r['n_no_change']}  relation_event_count={r['relation_event_count']}"
        )


def print_four_files_table(results):
    targets = {
        "derived_requirements/index.md": "derived_req",
        "program_requirements/index.md": "program_req",
        "subsystem_requirements/index.md": "subsystem_req",
        "system_requirements/index.md": "system_req",
    }
    req5_baseline = {
        "derived_req": {"phase2_total_ms": 1612},
        "program_req": {"phase2_total_ms": 147},
        "subsystem_req": {"phase2_total_ms": 14884},
        "system_req": {"phase2_total_ms": 22415},
    }

    first_epoch = {}
    for r in results:
        key = targets.get(r["file"])
        if key and key not in first_epoch:
            first_epoch[key] = r

    print("\n" + "=" * 110)
    print("FOUR MAIN FILES — FIRST PARSE EPOCH vs. req5 BASELINE")
    print("=" * 110)
    hdr = (
        f"{'File':<18} {'push_rel_ms':>12} {'gap_ms':>8} "
        f"{'pass1_ms':>9} {'elapsed_ms':>11} {'flush_dom_ms':>13} "
        f"{'phase2_ms':>10} {'req5_ms':>9} {'delta_ms':>9} {'speedup':>8}"
    )
    print(hdr)
    print("-" * 110)
    for key, label in [
        ("derived_req", "derived_req"),
        ("program_req", "program_req"),
        ("subsystem_req", "subsystem_req"),
        ("system_req", "system_req"),
    ]:
        r = first_epoch.get(key)
        if r is None:
            print(f"  {label:<16}  *** NOT FOUND ***")
            continue
        baseline = req5_baseline[key]["phase2_total_ms"]
        delta = r["phase2_total_ms"] - baseline
        speedup = (
            baseline / r["phase2_total_ms"] if r["phase2_total_ms"] else float("inf")
        )
        sign = "+" if delta >= 0 else ""
        print(
            f"{label:<18} {r['push_relation_ms']:>12} {r['gap_after_push_ms']:>8} "
            f"{r['pass1_total_ms']:>9} {r['elapsed_ms']:>11} {r['flush_dominant_ms']:>13} "
            f"{r['phase2_total_ms']:>10} {baseline:>9} {sign}{delta:>8} {speedup:>7.2f}x"
        )
    print()
    print(
        "Columns: push_rel_ms=push_relation loop, gap_ms=time between push_rel done and aeb start,"
    )
    print("         pass1_ms=pass1_total inside aeb, elapsed_ms=aeb+flush total,")
    print("         flush_dom_ms=elapsed-pass1 (flush_paths_for_events cost),")
    print(
        "         phase2_ms=full phase2_total_ms, req5_ms=baseline, speedup positive=faster"
    )


def print_bottleneck_analysis(results):
    print("\n" + "=" * 90)
    print("BOTTLENECK ANALYSIS — ALL EPOCHS, SORTED BY phase2_total_ms DESC")
    print("=" * 90)
    sorted_r = sorted(results, key=lambda r: r["phase2_total_ms"], reverse=True)
    print(
        f"\n{'Time':>8}  {'File':<42} {'push_rel':>9} {'gap':>7} {'elapsed':>8} {'flush_dom':>10} {'phase2':>8}"
    )
    print("-" * 100)
    for r in sorted_r:
        print(
            f"{r['epoch_ts'].strftime('%H:%M:%S')}  {r['file']:<42} "
            f"{r['push_relation_ms']:>9} {r['gap_after_push_ms']:>7} "
            f"{r['elapsed_ms']:>8} {r['flush_dominant_ms']:>10} {r['phase2_total_ms']:>8}"
        )


def print_large_push_relation(results, threshold=1000):
    print("\n" + "=" * 90)
    print(f"FILES WITH push_relation_ms > {threshold}ms (push dwarfs aeb+flush)")
    print("=" * 90)
    flagged = [r for r in results if r["push_relation_ms"] > threshold]
    flagged.sort(key=lambda r: r["push_relation_ms"], reverse=True)
    if not flagged:
        print("  None found.")
        return
    print(
        f"\n{'Time':>8}  {'File':<42} {'push_rel_ms':>12} {'elapsed_ms':>11} {'ratio push/elapsed':>20}"
    )
    print("-" * 100)
    for r in flagged:
        ratio = (
            r["push_relation_ms"] / r["elapsed_ms"]
            if r["elapsed_ms"] > 0
            else float("inf")
        )
        print(
            f"{r['epoch_ts'].strftime('%H:%M:%S')}  {r['file']:<42} "
            f"{r['push_relation_ms']:>12} {r['elapsed_ms']:>11} {ratio:>20.1f}x"
        )


def main():
    log_path = "/tmp/req12.log"
    print(f"Loading {log_path} ...")
    log = strip_ansi(load_log(log_path))

    push_rel_rows = parse_push_rel(log)
    aeb_rows = parse_aeb(log)
    flush_rows = parse_flush(log)

    print(f"  push_relation rows : {len(push_rel_rows)}")
    print(f"  aeb rows (doc_bb)  : {len(aeb_rows)}")
    print(f"  flush rows         : {len(flush_rows)}")

    first_ts, last_ts, total_s = wall_clock(log)
    print(
        f"\nWall-clock span: {first_ts.strftime('%H:%M:%S')} -> {last_ts.strftime('%H:%M:%S')} "
        f"= {total_s:.1f}s ({total_s / 60:.2f} min)"
    )

    results = align_triplets(push_rel_rows, aeb_rows, flush_rows)
    print(f"\nAligned triplets: {len(results)}")

    print_four_files_table(results)
    print_bottleneck_analysis(results)
    print_large_push_relation(results, threshold=1000)
    print_all_triplets(results)


if __name__ == "__main__":
    main()
