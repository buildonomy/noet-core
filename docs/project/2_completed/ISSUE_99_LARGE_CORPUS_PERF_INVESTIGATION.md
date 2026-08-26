# Issue 99: Large-Corpus Full-Build Performance Investigation (production corpus)

**Priority**: MEDIUM
**Estimated Effort**: 1-2 days (RELATIVE COMPARISON ONLY)
**Dependencies**: None. Follow-on from Issues 97/98 (which fixed correctness
bugs surfaced by the same full production-corpus build investigation).
Requires a fresh session — see Handoff Notes.

## Summary

A full production-corpus `--jobs 4 render` build (post Issue-98 fixes)
completed successfully: 64,473 files, 67,378 parses, 6,043 warnings, 0
errors, 0 panics. Log analysis of that run surfaced two categories of signal
worth investigating further: (1) apparent multi-minute-to-hours-long stalls
concentrated in large `repo` documents on their reparse pass, and
(2) very high self-referential-edge-skip counts. **Critically, the specific
timing numbers in this issue are unverified** — the analysis session
discovered partway through that the machine went through repeated
Clamshell/Maintenance sleep cycles during the build window, which explains a
large fraction (measured at ~65% of the top "stall" time investigated, on an
incomplete pass) of the apparent gaps. The investigation was also cut short
because `/tmp/build.log` was overwritten mid-analysis by a subsequent
run, invalidating the working data before the sleep-correlation check could
be completed. **This issue exists to redo that investigation properly, in a
fresh session, against a clean log.**

## Background / What Prompted This

See prior related work for context:
- Planning Issue 26 (pandoc markdown quality) — original diagnosis that
  `repo`'s converted slide decks dominate parse cost.
- noet-core Issue 97 (Phase 5 pipelining investigation) — found that
  sequential (`jobs=1`) `terminate_stack` cost was pathologically high
  compared to `jobs=4`; root cause never fully confirmed, deprioritized once
  `jobs>1` was shown to be a practical mitigation.
- noet-core Issue 98 (asset-sync session_bb gap) — fixed two real bugs
  (traversal direction in `sync_asset_snapshot`/`initialize_stack`, and a
  SQL-backend seed-stripping bug in `apply_traversal_sql`) discovered via a
  full production-corpus `--jobs 4` build. That build's log
  (`/tmp/build.log`, captured ~2026-08-12 15:26:41 UTC through completion)
  is what this issue's preliminary analysis was performed against, before
  the file was overwritten by a later run.

## Preliminary (Unverified) Findings — Re-verify Before Acting On

From a first analysis pass using `benches/log_analysis/parse_log.py` against
the (now-lost) completed log:

- **Wall clock**: reported as ~6h16m for the full build, `--jobs 4`,
  concurrency histogram showing 80.5% of active time at exactly 4 concurrent
  tasks (the semaphore bound) — this part is probably solid, re-confirm.
- **`--corpus-breakdown`**: `repo` region accounted for 89.7% of
  total sequential parse time (61,713s of 68,823s), consistent with prior
  Issue 26/97 findings. A single `release-393/system-4` combination alone
  was 45,452s across 1,183 files.
- **File-times / reparse-pass stalls**: `--file-times` surfaced attempt-2
  (reparse) files with extreme total durations, e.g. a single large
  CDR-package document (`system-1-cdr-part-1.md`) at 6,497.8s (108 min),
  with `--phase-detail` attributing nearly all of it to "Phase 1 (create
  nodes)" rather than Phase 0, 2, 4, or 5.
- **Global stalls**: a custom script found ~34 windows where *zero* log
  lines appeared from *any* of the 4 worker tasks, for durations up to
  ~3,176s. This was initially (and incorrectly) attributed to a global
  `tokio::Mutex` held across the full duration of `BeliefAccumulator::evaluate`
  / `QueryHandle::evaluate` (`beliefbase/accumulator.rs` — confirmed real
  code structure: the lock guard is held across the entire delegated
  `guard.inner.evaluate(package).await` call, not just a quick
  pending-event check).
- **Sleep contamination (found late, incomplete)**: cross-referencing
  `pmset -g log` sleep/wake events (careful with timezone: log timestamps
  are UTC, `pmset` reports local EDT — this session made and caught an error
  here) against the largest stall windows showed the machine was cycling
  through Clamshell Sleep / Maintenance Sleep / DarkWake for large stretches
  overlapping the build. A rough overlap check against ~30 of the largest
  gaps found ~65% of that specific sample's stall time coincided with sleep
  intervals, leaving a smaller but still nonzero "unexplained" residual. A
  second, more careful pass (keying gaps by `(task_idx, file_path)` instead
  of `task_idx` alone, since `task_idx` is reused across epochs) produced
  inconsistent-looking totals that were never resolved before the source log
  was overwritten — **do not trust the specific "65%" or "128 stalls, 767
  minutes" figures from this session; they need to be recomputed from
  scratch.**
- **Self-referential edge skips**: 9,874 occurrences of `[push_relation]
  skipping self-referential Epistemic edge` (DEBUG level, not a warning).
  High count, unclear if expected given corpus content patterns or a sign of
  a systematic issue in how Epistemic edges are being generated for certain
  document types.
- **Content-quality signal** (separate from perf, but observed in the same
  log): 16 "Phase 4: skipping unbalanced node" warnings (confirms the
  Issue-26 hardening fires correctly at full-corpus scale without crashing);
  309 "Asset node not found in session_bb" warnings, which are the expected
  fingerprint of the Issue-98 SQL-backend bug — this log predates that fix,
  so a fresh run should show ~0.

## Goals

1. Capture a **fresh, complete** production-corpus `--jobs N render` log
   with sleep prevented for the entire duration (`caffeinate -i` or
   equivalent), so wall-clock and stall analysis is not confounded by
   system sleep.
2. Re-run `benches/log_analysis/parse_log.py --all` and
   `--corpus-breakdown` against the clean log; confirm or correct the
   `repo` / `system-4` cost concentration finding.
3. Re-identify genuine (non-sleep) Phase-1/reparse-pass stalls, if any exist,
   with correct `(task_idx, file_path)` keying (task_idx is reused across
   epochs — see Known Pitfalls below).
4. If genuine multi-minute stalls remain after removing sleep artifacts,
   determine whether the `BeliefAccumulator`/`QueryHandle` global-mutex
   pattern (lock held across the full `evaluate()` traversal, not just
   pending-event bookkeeping) is implicated, and whether a large `"balanced"`
   query against a big deck's ancestor chain is the trigger.
5. Investigate the self-referential Epistemic edge skip count: is 9,874 a
   sign of a correctness/dedup issue upstream, or an expected byproduct of
   the corpus's `{maps_to}` / cross-reference density?
6. Confirm the 309 "Asset node not found in session_bb" warnings drop to
   ~0 on a fresh run (Issue 98 validation, still outstanding from that
   issue).

## Suggested Approach

1. Kick off the build with sleep prevention:
   ```sh
   caffeinate -i -s -- env RUST_LOG=noet_core::codec::builder=debug,noet_core::codec::compiler=debug,noet_core::db::query_size=debug \
     just cached=true jobs=4 render 2>&1 | tee /tmp/build_clean.log
   ```
   Use a **new filename** (`build_clean.log` or similar) rather than
   reusing `/tmp/build.log`, to avoid collision with any other run in
   flight — this exact collision is what truncated the previous
   investigation.
2. Once complete, immediately copy or archive the log somewhere durable
   (e.g. into this repo's `.scratchpad/` temporarily, or a location outside
   `/tmp`) before doing any analysis, so a subsequent run can't overwrite it
   mid-investigation again.
3. Run the full `benches/log_analysis/parse_log.py --all` plus
   `--corpus-breakdown`, `--concurrency`, and `--file-times` reports; save
   the raw text output alongside the log.
4. For any remaining large stalls, cross-reference against `pmset -g log`
   sleep/wake events **converted to UTC correctly** (pmset is local time;
   log timestamps are UTC — confirm the host's UTC offset explicitly rather
   than assuming EDT/UTC-4, since DST could differ from when this issue was
   written).
5. Only after sleep artifacts are excluded, decide whether the
   `apply_traversal_sql` / accumulator-mutex hypothesis is worth pursuing as
   a follow-on architectural issue (do not create that issue speculatively;
   this one is investigation-only).

## Known Pitfalls (learned the hard way this session)

- **`task_idx` is reused across epochs.** `parse_epoch` restarts task
  numbering from 0 for each depth-group/leaf/remainder batch. Any script
  correlating log lines by `task_idx` alone (without also matching the file
  path from the `parse_task{task_idx=N path=...}` span) will silently
  conflate unrelated files from different epochs. Always key by
  `(task_idx, path)` or scope analysis to a single epoch's line range.
- **`pmset -g log` timestamps are local time; noet log timestamps are UTC.**
  Confirm the conversion offset (`date +"%Z %z"`) rather than hardcoding
  EDT/UTC-4 — DST transitions or a different host would silently break the
  correlation.
- **Log files in `/tmp` can be overwritten mid-analysis** if another build
  is kicked off reusing the same filename. Always copy/archive a completed
  log before starting extended analysis, and use distinct filenames per
  run.
- The existing `benches/log_analysis/README.md` "Phase 5 silent stalls"
  guidance and `parse_log.py --stalls` are useful starting points but do not
  themselves account for system sleep — that cross-reference has to be done
  manually against `pmset -g log` (or equivalent) until/unless the tooling
  is extended to do it automatically.

## Testing Requirements

N/A — this is a log-analysis investigation issue, not a code-change issue.
Any code changes that come out of it (e.g. an architectural fix to
`accumulator.rs`, or a `parse_log.py` enhancement to auto-detect sleep
overlap) should get their own follow-on issue with its own testing
requirements.

## Success Criteria

- [ ] A complete, sleep-artifact-free production-corpus `--jobs N` log
      exists and is archived somewhere durable (not `/tmp`).
- [ ] `repo` cost concentration is confirmed or corrected against
      clean data.
- [ ] A definitive count of genuine (non-sleep) multi-minute stalls, if any,
      with root cause identified or a clear statement that none exist.
- [ ] The self-referential Epistemic edge skip count is explained (expected
      corpus behavior vs. a bug).
- [ ] Issue 98's 309-warning regression is confirmed resolved (~0 on the
      fresh run).
- [ ] If a genuine architectural bottleneck is found (e.g. the
      accumulator-mutex hypothesis), a new, separately-scoped issue is filed
      for the fix — this issue stays investigation-only.

## Risks

- Risk: A "clean" run still shows large stalls, and the accumulator-mutex
  hypothesis turns out to be real → **Mitigation**: this is still valuable
  information; file a properly-scoped architecture issue once confirmed
  rather than guessing now.
- Risk: `caffeinate` alone isn't sufficient to prevent all sleep-adjacent
  interference (e.g. thermal throttling, other background processes) →
  **Mitigation**: cross-check `pmset -g log` after the run regardless, don't
  assume `caffeinate` guarantees a clean sample.

## References

- `noet-core/benches/log_analysis/README.md` — existing tooling and
  guidance, including the "Phase 5 silent stalls" pattern this issue partly
  overlaps with.
- `noet-core/src/beliefbase/accumulator.rs` — `BeliefAccumulator::evaluate`
  and `QueryHandle::evaluate`, both of which hold `acc.lock().await` across
  the full delegated `evaluate()` call. Relevant if the mutex-contention
  hypothesis is confirmed on clean data.
- `noet-core/docs/project/0_open/ISSUE_97_BUILD_PERFORMANCE_BOTTLENECKS.md`
  — related prior investigation into `jobs=1` vs `jobs>1` timing anomalies
  (now the running register of build-performance bottlenecks); may share root
  cause with whatever is found here.
- `noet-core/docs/project/0_open/ISSUE_98_ASSET_SYNC_SESSION_BB_GAP.md` (or
  wherever it landed — it was slated for deletion in favor of commit-message
  documentation; check git history if the file is gone) — the fixes whose
  full-build validation this issue's log was originally captured for.
