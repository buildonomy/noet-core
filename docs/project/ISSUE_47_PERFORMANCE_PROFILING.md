# Issue 47: Performance Profiling Infrastructure

**Priority**: MEDIUM - Foundation for scaling to GB-scale documentation
**Estimated Effort**: 3-4 days
**Dependencies**: ISSUE_07 (basic benchmarks established)
**Context**: Preparation for processing GB-scale documentation corpora

## Summary

Establish performance profiling infrastructure to characterize noet-core's behavior at scale. Currently we have micro-benchmarks (Criterion) for regression detection, but need macro-benchmarks with realistic workloads, memory profiling, and performance characterization for GB-scale document processing. This issue creates the foundation for identifying bottlenecks before they become critical.

## Goals

1. Create realistic test corpus generator for benchmarking
2. Establish macro-benchmarks (10KB → 100MB+ document sets)
3. Add memory profiling infrastructure
4. Characterize current performance baselines
5. Document performance characteristics and expected scaling behavior
6. Identify potential bottlenecks for GB-scale processing

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

**Gap**: No macro-benchmarks for realistic workloads or memory profiling.

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
   - [ ] Profile actual GB-scale workload (if available)
   - [ ] Identify top 5 performance bottlenecks:
     - PathMap collision detection?
     - Cache lookup misses?
     - Repeated parsing/allocations?
     - Graph query performance?
     - BID generation overhead?
   - [ ] Document findings in trade study or design doc
   - [ ] Prioritize optimization opportunities

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
- [ ] At least 3 potential bottlenecks identified for future optimization
- [ ] Performance characteristics documented in design docs
- [ ] Clear answer to: "Can we process GB-scale corpora with current architecture?"

## Risks

**Risk 1: Generated corpus not representative**
- Synthetic data may miss real-world patterns
- **Mitigation**: Validate against actual documentation sets (Rust docs, Linux kernel docs, etc.)

**Risk 2: Performance bottlenecks require architectural changes**
- May discover O(n²) operations that can't be fixed easily
- **Mitigation**: Characterize early, before committing to GB-scale features

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