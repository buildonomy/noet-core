# Issue 47: Performance Profiling Infrastructure

**Priority**: MEDIUM - FM1b + ProtoIndex fix committed; Run 3 queued; BN-1 is next bottleneck
**Estimated Effort**: 3-4 days
**Dependencies**: ISSUE_07 (basic benchmarks established)
**Context**: Preparation for processing GB-scale documentation corpora

## Summary

Establish performance profiling infrastructure to characterize noet-core's behavior at scale. Currently we have micro-benchmarks (Criterion) for regression detection, but need macro-benchmarks with realistic workloads, memory profiling, and performance characterization for GB-scale document processing. This issue creates the foundation for identifying bottlenecks before they become critical.

**Update 3 (Run 2 analysed; ProtoIndex landed)**: Run 2 on the MDN `web/javascript` corpus (1,329 files) ran for ~19.7 hours and confirmed FM1b as the dominant bottleneck: Phase 0 mean was 10.38 s (target <0.5 s), 106 outlier files exceeded 34 s, and 47 Phase 5 stalls totalling ~6.3 h of wall time were driven by RelationUpdate fan-out on `trailing_commas`, `working_with_objects`, and `functions/set`. The FM1b fix (ProtoIndex + three correctness bugs) is now committed and all 7 codec tests pass. Run 3 will measure the improvement. A secondary bottleneck — BN-1 (`add_relations` DFS in `session_bb.merge`) — was visible from ~05:33 onward in Run 2 as silent 0-RelUpdate stalls; this will become the dominant term once FM1b is gone.

**Update 2 (FM1b fixed)**: The dominant O(siblings) bottleneck in `initialize_stack` has been eliminated. The `push_relation` sibling fan-out loop is gone; `initialize_stack` now returns `(IRNode, Option<u16>)` carrying the entry doc's sort key directly. The fast path queries the parent network (not the entry doc), hitting `StackCache` on the first parse of every child.

**Earlier update**: An O(N²) bottleneck was confirmed empirically on the MDN `web/javascript` sub-corpus (~1 300 files). The bottleneck was in `initialize_stack`'s `push_relation` sibling fan-out (FM1b), not `BeliefGraph::add_relations` as originally suspected. Profiling infrastructure is now needed primarily to measure the fix, not just find the problem.

## Goals

1. Create realistic test corpus generator for benchmarking
2. Establish macro-benchmarks (10KB → 100MB+ document sets)
3. Add memory profiling infrastructure
4. Characterize current performance baselines
5. Document performance characteristics and expected scaling behavior
6. Identify potential bottlenecks for GB-scale processing

## Confirmed Bottlenecks

### ✅ FM1b: O(siblings) fan-out in `initialize_stack` — **FIXED**

**Location**: `src/codec/builder.rs`, `initialize_stack` slow path.

**Observed symptom**: Every file in a large flat network spent ~4 ms per
sibling during `initialize_stack` re-processing the parent network's sibling
list. The 645 s stall for `trailing_commas` (1 193 RelationUpdates) and 618 s
stall for `working_with_objects` (1 156 RelationUpdates) in Run 2 confirmed
`session_bb` was O(all-prior-files) in size.

**Root cause**: The slow-path `push_relation` loop over
`maybe_content_parent_proto.upstream` (all sibling docs) was pre-seeding
`session_bb` and `doc_bb` with sibling edges on every file parse, causing
O(siblings) work per file → O(N × siblings) total.

**Fix** (landed — ProtoIndex commit):
- Replaced per-session `network_proto_cache` on `GraphBuilder` with
  `ProtoIndex` — a pre-built filesystem index (one WalkDir pass at compiler
  startup, shared via `Arc<RwLock<...>>` clone).
- Removed `push_relation` sibling fan-out entirely from `initialize_stack`.
- `initialize_stack` now returns `(IRNode, Option<u16>)` — sort key from
  `proto_index.sort_key_for()`, single source of truth for both fast and slow paths.
- Fast path (`try_initialize_stack_from_session_cache`) redesigned to query
  the **parent network** in `session_bb` instead of the entry doc.
- Three correctness bugs introduced by the FM1b draft were also fixed:
  sort_key_for index.md handling; StackCache branch polluting missing_structure;
  stale doc_bb carried forward via consume()+union_mut.
- `PathMap::order_map` index added for O(log N) ancestor prefix lookup.

**Test result**: 7/7 codec tests pass (all three bugs fixed).

**Run 2 corpus baseline** (pre-fix, mdn-javascript.log, ~19.7 h wall time):

| Metric | Value |
|--------|-------|
| Phase 0 mean | **10.38 s** |
| Phase 0 max | 56 s |
| Outlier files (>34 s) | 106 |
| Phase 5 stalls >30s | 47 (6.3 h total) |
| Worst stall | 705 s (`trailing_commas`, 624 RelUpdates) |
| Parse attempts: 1st/2nd/3rd | 1,552 / 1,155 / 195 |

**Run 3 target**: Phase 0 mean <0.5 s; FM1b Phase 5 stalls gone; BN-1
silent stalls will remain and become the new dominant term.

---

### ❌ BN-DB: `with_db_cache` section anchor not in PathMap — **PRE-EXISTING, OPEN**

**Location**: `src/codec/builder.rs`, Phase 1 `push(section)` during reparse
with `DbConnection` as `global_bb`.

**Symptom**: `test_belief_set_builder_with_db_cache` panics:
```
Set should be balanced here: bid=X in_states=true in_pathmap=false
proto.heading=4 proto.path=".../asset_tracking_test.md"
```
A section anchor (`heading=4`) is in `doc_bb.states` but not `doc_bb.paths()`
after Phase 1.

**Root cause** (partially confirmed): On reparse, `doc_bb` already contains a
`Section(section, doc, {sk:N})` edge before Phase 1 `push(section)` fires its
`RelationChange`. `generate_edge_update` compares incoming weight (no
`sort_key`) against present weight (`sort_key: N`) — sees no meaningful change
— returns `None` — PathMap update skipped — section not in PathMap. The
seeding path that puts the edge in `doc_bb` has not been fully traced; the
`downstream_query` in `try_initialize_stack_from_session_cache` and
`RelationPred::NodeIn` semantics in `cache_fetch` are candidates.

**Candidate fix**: In `generate_edge_update`, when the incoming weight has no
`sort_key` but the present weight does, treat the existing `sort_key` as
authoritative and still mark `changed = true` so the PathMap entry is
(re)created. This preserves idempotency without requiring a fresh auto-assign.

**Blocked by**: needs one targeted trace log to confirm exactly which code path
seeds the section→doc edge into `doc_bb` before `push(section)` fires.

---

### BN-1: O(N²) Phase 2 merge in `parse_content` — **superseded by FM1b**

Originally suspected as the dominant cost driver. Run 2 confirmed FM1b
(O(siblings) fan-out) was the actual dominant term. BN-1 (`add_relations` DFS
in `session_bb.merge`) is a secondary cost; address after Run 3 confirms
whether it remains significant post-FM1b fix.

**Candidate fixes** (deferred):

1. **Restrict the DFS seed set** (`add_relations`): accept optional
   `seed_bids: &BTreeSet<Bid>` and only seed from those.
2. **Skip DFS for Phase 2**: `missing_structure` from `cache_fetch` already
   contains exactly the needed nodes; the DFS is redundant.
3. **Lazy `session_bb` population**: merge into `session_bb` only in
   `terminate_stack`.

---

## Current State

**Existing test corpus**: `tests/network_1/`
- **Size**: ~10KB total across 9 markdown files (344 lines)
- **References**: 31 links total, 5 wikilinks
- **Structure**: Mix of sections, lists, definition lists, quotes
- **Sufficient for**: Unit tests, correctness verification, micro-benchmarks
- **Insufficient for**: Performance characterization, memory profiling, scaling analysis

**Existing benchmarks** (from ISSUE_07):
- Criterion-based micro-benchmarks in GitHub Actions
- Run on push to main branch (informational only)
- Focus: Function-level performance regression detection

**MDN corpus runs** (ad-hoc, `.bench_corpora/mdn-content/files/en-us/web/javascript`, 1,329 files):
- **Run 2** (pre-FM1b fix): ~19.7 h, Phase 0 mean 10.38 s, 106 outliers, 47 Phase 5 stalls
- **Run 3** (post-ProtoIndex): in progress — expected to confirm Phase 0 collapse and reveal BN-1 as next bottleneck

**Gap**: No macro-benchmarks for realistic workloads or memory profiling. Corpus runs are ad-hoc; we have no automated way to measure fixes or detect regressions at this scale.

## Architecture

### Three-Tier Benchmark Strategy

**Tier 0: Log analysis tools** (implemented, `benches/log_analysis/`)
- `parse_log.py`: analyses `RUST_LOG=debug` output from real corpus runs
- Extracts per-file, per-phase timing from timestamped log lines
- Modes: `--phase-summary` (slowest files, outlier flagging), `--stalls`
  (silent gaps between log lines), `--warnings` (WARN/ERROR classification
  and histogram), `--phase-detail` (per-phase breakdown for a named file)
- Warning classifier maps known patterns (self-connection flood, Issue-34
  violations, sort-key sentinel resets) to human-readable labels
- No dependencies beyond Python 3.10 stdlib
- Purpose: diagnose *which phase* and *which files* are slow in a real run,
  before and after a candidate fix

**Tier 1: Micro-benchmarks** (existing, via Criterion)
- Function-level: parsing, BID injection, graph queries
- Purpose: Regression detection on specific operations
- Already implemented in ISSUE_07

**Tier 2: Macro-benchmarks** (this issue)
- Document-level: 10KB, 100KB, 1MB, 10MB, 100MB documents
- Multi-document: 10, 100, 1000 file sets
- Purpose: Characterize O(n) scaling, identify bottlenecks
- **New infrastructure needed**

**Tier 3: Memory profiling** (this issue)
- Peak heap usage per document size
- Allocation hotspots
- Memory growth patterns (linear? exponential?)
- Purpose: Ensure GB-scale is feasible
- **New infrastructure needed**

### Realistic Corpus Generator

Generate markdown that resembles real documentation:

**Content mix** (based on typical technical docs):
- 60% prose paragraphs (low reference density)
- 20% lists with cross-references (medium density)
- 10% code blocks (zero density)
- 10% tables and headings (varied density)

**Reference density** (critical for graph performance):
- Real docs: 5-20 references per 1KB content
- Mix of: wikilinks, path references, section anchors
- Both internal (within corpus) and external references

**Structural patterns**:
- Nested headings (6 levels deep)
- Multi-file cross-references
- Repeated reference targets (collision testing)
- Long reference chains (A→B→C→D)

### Key Metrics to Track

**Performance**:
- Parse time vs. document size (expect linear O(n))
- Multi-pass compilation overhead
- Graph query time (PathMap lookups, reference resolution)
- BID injection and cache operations

**Memory**:
- Peak heap usage vs. corpus size
- BeliefBase growth (session_bb vs. doc_bb)
- PathMap size with 10K, 100K, 1M nodes
- Allocation count and hotspots

**Scaling characteristics**:
- Per-document processing (parallelizable?)
- Cross-document references (synchronization cost?)
- Cache hit rates at different scales

## Implementation Steps

### 0. **Log Analysis Tools** (complete)
   - [x] Create `benches/log_analysis/parse_log.py`
   - [x] Parse timestamped `RUST_LOG=debug` lines; extract per-file `FileRecord`
         with phase timestamps, diff-event counts
   - [x] `--phase-summary`: ranked Phase 0 table with mean/σ outlier flagging
         and Phase 5 post-processing gap table
   - [x] `--stalls SECONDS`: silent-gap detector with ±3-line context
   - [x] `--warnings`: WARN/ERROR classifier (BN-2 floods, Issue-34 violations,
         sentinel resets, …) with per-minute histogram
   - [x] `--phase-detail FRAGMENT`: per-phase breakdown for named files
   - [x] `benches/log_analysis/README.md` with quick-start, example output,
         and diagnostic decision tree

### 1. **Corpus Generator** (1 day)
   - [ ] Create `benches/corpus_generator.rs`
   - [ ] Implement realistic markdown structure generation:
     - Prose paragraphs with internal references
     - Nested lists and code blocks
     - Multi-file document sets with cross-links
   - [ ] Parameterize by size (bytes) and reference density
   - [ ] Generate deterministic output (seeded RNG for reproducibility)
   - [ ] Validate generated corpus parses correctly

### 2. **Macro-Benchmarks** (1 day)
   - [ ] Create `benches/macro_benchmarks.rs`
   - [ ] Benchmark single-document processing:
     - 10KB (baseline, similar to network_1)
     - 100KB (typical reference manual)
     - 1MB (large specification)
     - 10MB (comprehensive documentation set)
     - 100MB (stress test)
   - [ ] Benchmark multi-document sets:
     - 10 files × 50KB each (small project)
     - 100 files × 50KB each (medium project)
     - 1000 files × 50KB each (large monorepo)
   - [ ] Track throughput (bytes/sec) and latency

### 3. **Memory Profiling** (0.5 days)
   - [ ] Add `dhat` or `heaptrack` integration
   - [ ] Create `benches/memory_profile.rs` or separate profile script
   - [ ] Measure peak heap usage for each corpus size
   - [ ] Identify allocation hotspots
   - [ ] Document memory budget expectations

### 4. **Baseline Characterization** (0.5 days)
   - [ ] Run benchmarks on current codebase
   - [ ] Document current performance characteristics
   - [ ] Identify O(n), O(n²), O(n log n) operations
   - [ ] Note any unexpected scaling behavior
   - [ ] Establish acceptable performance targets:
     - Example: "Process 1GB corpus in < 60 seconds"
     - Example: "Peak memory < 2× corpus size"

### 5. **Bottleneck Analysis** (1 day)
   - [x] FM1b O(siblings) fan-out in `initialize_stack` confirmed as dominant cost driver (Run 2)
   - [x] `parse_log.py --phase-summary` and `--stalls` used to isolate FM1b and
         FM1a symptoms; `--warnings` used to quantify BN-2 self-connection flood
         and Issue-34 violations across the full run
   - [x] FM1b fix landed: `push_relation` fan-out removed, `doc_sort_key` sentinel,
         parent-network fast path
   - [x] ProtoIndex landed: replaces `network_proto_cache`; three correctness bugs fixed;
         all 7/7 codec tests pass; Windows normalization applied
   - [x] Run 2 analysed: Phase 0 mean 10.38 s, 106 outliers, 47 Phase 5 stalls (~6.3 h),
         BN-1 silent stalls confirmed as next bottleneck from ~05:33 onward
   - [ ] Run 3: MDN corpus benchmark post-ProtoIndex; target Phase 0 mean <0.5 s,
         FM1b Phase 5 stalls gone; characterise residual BN-1 stalls
   - [ ] Check remaining bottlenecks after Run 3:
     - BN-1 (`add_relations` DFS) still significant?
     - PathMap collision detection?
     - `BalanceCheck` frequency?
   - [ ] Document confirmed findings; update this issue or create ISSUE_48 for remaining fix work

## Testing Requirements

- [ ] Corpus generator produces valid, parseable markdown
- [ ] Generated corpora are deterministic (reproducible benchmarks)
- [ ] Benchmarks run successfully in CI (optional: store as artifacts)
- [ ] Memory profiling identifies no obvious leaks
- [ ] Baseline metrics documented and reviewable
- [x] FM1b fix: `initialize_stack` sibling fan-out eliminated
- [x] ProtoIndex: replaces network_proto_cache; 7/7 codec tests pass; Windows normalization applied
- [x] Run 2 baseline documented: Phase 0 mean 10.38 s, 47 Phase 5 stalls, BN-1 confirmed secondary
- [ ] Run 3 corpus benchmark confirms Phase 0 mean <0.5 s and FM1b stalls gone
- [ ] BN-1 (`add_relations` DFS) quantified post-Run 3; fix if dominant

## Success Criteria

- [ ] Realistic corpus generator implemented and validated
- [ ] Macro-benchmarks characterize 10KB → 100MB+ scaling
- [ ] Memory profiling infrastructure operational
- [ ] Baseline performance metrics documented
- [x] At least 1 confirmed O(N²) bottleneck identified and fixed (FM1b, `initialize_stack` fan-out)
- [x] Run 2 baseline: Phase 0 mean 10.38 s, 106 outliers, 47 stalls (6.3 h), BN-1 visible
- [ ] Run 3 confirms FM1b fix effective on MDN corpus (target Phase 0 mean <0.5 s)
- [ ] BN-1 (`add_relations` DFS) quantified post-Run 3; fix if dominant
- [ ] BN-DB (`with_db_cache`) investigated; fix or track in separate issue
- [ ] At least 2 additional bottlenecks characterized after FM1b fix
- [ ] Performance characteristics documented in design docs
- [ ] Clear answer to: "Can we process GB-scale corpora with current architecture?"

## Risks

**Risk 1: Generated corpus not representative**
- Synthetic data may miss real-world patterns
- **Mitigation**: Validate against actual documentation sets (Rust docs, Linux kernel docs, etc.)

**Risk 2: BN-1 fix changes `add_relations` semantics**
- `add_relations_from` with a restricted seed set may fail to pull in nodes that
  the current unbounded DFS would have found, causing missing edges in edge
  cases.
- **Mitigation**: The `--jobs 1` sequential path must remain byte-identical to
  the current output (enforced by Issue 57 step 7). Run the full `tests/network_1`
  suite and the MDN warm-cache idempotency check before merging any fix.

**Risk 3: Memory profiling adds complexity**
- Tools like `dhat` require specific build configurations
- **Mitigation**: Keep profiling separate from main benchmarks, optional for CI

**Risk 4: Benchmark noise in CI**
- GitHub Actions runners have variable performance
- **Mitigation**: Focus on relative comparisons (10× corpus = ~10× time), not absolute numbers

## Open Questions

- Should macro-benchmarks run in CI or only locally? (Tradeoff: coverage vs. runtime)
- What's the target performance for 1GB corpus? (Need product requirements)
- Should we benchmark streaming/incremental processing? (If we add that capability)
- Do we need distributed processing for multi-GB corpora? (Out of scope for v0.1)
- Should BN-1 fix work be tracked in this issue or a new ISSUE_48? Given the fix
  touches `add_relations` (a core merge primitive), it may warrant its own issue
  with its own correctness gate and rollback plan.

## Notes

**Why not use `tests/network_1` for performance testing?**
- At ~10KB, it's too small to reveal scaling issues
- Not representative of reference density in real docs
- Good for correctness, insufficient for performance characterization

**Relationship to ISSUE_07**:
- ISSUE_07 established Criterion micro-benchmarks for regression detection
- ISSUE_47 adds macro-benchmarks, memory profiling, and log-analysis tools for
  scaling analysis
- Both are needed: ISSUE_07 prevents regressions, ISSUE_47 prevents surprises at scale

**`benches/log_analysis/` workflow**:
The typical use of the log-analysis tools is:
1. Capture a run: `RUST_LOG=debug cargo run … parse <corpus> 2>&1 | tee run.log`
2. `parse_log.py run.log --all` to locate slow files and dominant warning types
3. `parse_log.py run.log --phase-detail <slow-file>` to pinpoint the bottleneck phase
4. Apply fix, re-run step 1, compare Phase 0 distributions to confirm improvement
These tools complement (not replace) the Criterion benchmarks: Criterion measures
throughput under controlled synthetic conditions; `parse_log.py` diagnoses real
corpus behaviour where the bottleneck may be structural (e.g. `session_bb` growth).

**Future work** (not in this issue):
- Performance optimization based on profiling results
- Streaming/incremental processing for truly massive corpora
- Parallel document processing (if bottlenecks are per-document)
- Cache tuning and optimization