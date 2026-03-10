# Issue 47: Performance Profiling Infrastructure

**Priority**: HIGH - O(N²) bottleneck confirmed on realistic corpus
**Estimated Effort**: 3-4 days
**Dependencies**: ISSUE_07 (basic benchmarks established)
**Context**: Preparation for processing GB-scale documentation corpora

## Summary

Establish performance profiling infrastructure to characterize noet-core's behavior at scale. Currently we have micro-benchmarks (Criterion) for regression detection, but need macro-benchmarks with realistic workloads, memory profiling, and performance characterization for GB-scale document processing. This issue creates the foundation for identifying bottlenecks before they become critical.

**Update**: An O(N²) bottleneck has been confirmed empirically on the MDN `web/javascript` sub-corpus (~1 300 files). The bottleneck is in `BeliefGraph::add_relations` (called from `session_bb.merge` at `builder.rs:459-474`). Root cause and candidate fixes are documented in the new **Confirmed Bottlenecks** section below. Profiling infrastructure is now needed primarily to measure the fix, not just find the problem.

## Goals

1. Create realistic test corpus generator for benchmarking
2. Establish macro-benchmarks (10KB → 100MB+ document sets)
3. Add memory profiling infrastructure
4. Characterize current performance baselines
5. Document performance characteristics and expected scaling behavior
6. Identify potential bottlenecks for GB-scale processing

## Confirmed Bottlenecks

### BN-1: O(N²) Phase 2 merge in `parse_content` — **confirmed**

**Location**: `src/codec/builder.rs:459-474`, specifically the call to
`self.session_bb.merge(&missing_structure)`.

**Observed symptom**: When parsing the MDN `web/javascript` sub-corpus
(~1 300 files), each file in the latter half of the lexicographic parse order
stalls for >10 seconds at the log line:

```
Phase 2: merging missing structure onto session_bb and set
```

**Root cause**: `BeliefBase::merge` → `BeliefGraph::union_mut` →
`add_relations`. The `add_relations` function seeds a DFS from all nodes in
`self.states` that also appear in the rhs graph. `session_bb` grows
monotonically across the entire parse session — every file permanently adds
its nodes. By file 1 000 of 1 300, `session_bb` holds thousands of nodes,
making every `add_relations` call proportionally more expensive:

```
cost(file N) ≈ O(session_bb_size(N) × missing_structure_edges)
             ≈ O(N × K)   where K = avg edges per GlobalCache hit
total cost   ≈ O(N² × K)
```

**Secondary contributor**: `missing_structure` is accumulated across all
`push_relation` calls within a single file (each `GlobalCache` hit appends a
subgraph via `union_mut`). For densely-linked files (e.g. MDN `String`
reference with 40+ method links), `missing_structure` can reach hundreds of
nodes before the Phase 2 merge runs.

**Why it gets worse late in the parse**: the corpus parses roughly
lexicographically. The `global_objects/string/` subtree — where the stalls are
observed — is parsed after `array/`, `boolean/`, `error/`, etc., so
`session_bb` is already large when those files are reached.

**Candidate fixes** (in order of invasiveness):

1. **Restrict the DFS seed set** (`add_relations`): instead of seeding from
   all nodes in `self.states` that appear in rhs, accept an optional
   `seed_bids: &BTreeSet<Bid>` parameter and only seed from those. For the
   Phase 2 merge, the caller already knows the relevant seeds are the current
   file's `parsed_bids`. This reduces the DFS scope from O(session_bb_size) to
   O(file_nodes). New signature: `add_relations_from(rhs, seed_bids)`.

2. **Skip DFS for the Phase 2 case entirely**: `missing_structure` is produced
   by `cache_fetch` which already knows exactly which nodes are needed. The DFS
   is redundant for this call site — all required nodes are already in the
   graph.

3. **Lazy `session_bb` population**: accumulate a per-file index of
   BID → graph fragment during `push_relation` and merge into `session_bb`
   only in `terminate_stack`, paying the cost once per file instead of once
   per relation.

**Interaction with Issue 57 (parallel epochs)**: parallelism does not fix
this. The post-epoch merge (merging N task `session_bb`s into the compiler's
`session_bb`) still calls `add_relations` sequentially per task, and the
merged seed for epoch 1 would be as large as the current sequential case. Fix
BN-1 first; then Issue 57 compounds the win.

**Note on `BalanceCheck`**: the two `process_event(&BeliefEvent::BalanceCheck)`
calls in the Phase 2 block add constant overhead per file but are not the
quadratic term. They can be deferred to `terminate_stack` as a secondary
optimization after BN-1 is addressed.

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

**Gap**: No macro-benchmarks for realistic workloads or memory profiling. The MDN corpus run described above was ad-hoc (`noet parse` against `.bench_corpora/mdn-content/files/en-us/web/javascript`); we have no automated way to measure the fix or detect regressions at this scale.

## Architecture

### Three-Tier Benchmark Strategy

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
   - [x] O(N²) bottleneck in `add_relations` DFS confirmed on MDN `web/javascript`
         (see **Confirmed Bottlenecks** above)
   - [ ] Profile `add_relations` with `perf` or `flamegraph` to measure DFS share
         vs. edge-insertion share of wall time
   - [ ] Implement and benchmark candidate fix: `add_relations_from(rhs, seed_bids)`
         — seed DFS only from current file's `parsed_bids` in Phase 2
   - [ ] Measure fix: run MDN `web/javascript` parse before and after; target <1 s/file
         on latter half of corpus (currently >10 s/file)
   - [ ] Check remaining bottlenecks after BN-1 fix:
     - PathMap collision detection?
     - `BalanceCheck` frequency?
     - Global cache query latency?
   - [ ] Document confirmed findings; update this issue or create ISSUE_48 for fix work

## Testing Requirements

- [ ] Corpus generator produces valid, parseable markdown
- [ ] Generated corpora are deterministic (reproducible benchmarks)
- [ ] Benchmarks run successfully in CI (optional: store as artifacts)
- [ ] Memory profiling identifies no obvious leaks
- [ ] Baseline metrics documented and reviewable

## Success Criteria

- [ ] Realistic corpus generator implemented and validated
- [ ] Macro-benchmarks characterize 10KB → 100MB+ scaling
- [ ] Memory profiling infrastructure operational
- [ ] Baseline performance metrics documented
- [x] At least 1 confirmed O(N²) bottleneck identified (BN-1, `add_relations` DFS)
- [ ] BN-1 candidate fix implemented and benchmarked against MDN `web/javascript`
- [ ] At least 2 additional bottlenecks characterized after BN-1 fix
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
- ISSUE_47 adds macro-benchmarks and memory profiling for scaling analysis
- Both are needed: ISSUE_07 prevents regressions, ISSUE_47 prevents surprises at scale

**Future work** (not in this issue):
- Performance optimization based on profiling results
- Streaming/incremental processing for truly massive corpora
- Parallel document processing (if bottlenecks are per-document)
- Cache tuning and optimization