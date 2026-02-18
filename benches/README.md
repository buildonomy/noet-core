# Performance Benchmarks

This directory contains Criterion-based performance benchmarks for noet-core.

## Purpose

These benchmarks measure regression in key operations:
- Document parsing and compilation
- BID generation and caching
- Multi-pass reference resolution
- Graph queries (PathMap lookups, edge traversal)

**Note**: These are micro-benchmarks using the small `tests/network_1` corpus (~10KB). For GB-scale performance characterization, see **ISSUE_47**.

## Running Benchmarks

```bash
# Run all benchmarks
cargo bench --features service

# Run specific benchmark
cargo bench --features service -- parse_all_documents

# Save baseline for comparison
cargo bench --features service -- --save-baseline main

# Compare against baseline
cargo bench --features service -- --baseline main
```

## Viewing Results

Criterion generates HTML reports in `target/criterion/`:

```bash
# Open in browser (macOS)
open target/criterion/report/index.html

# Open in browser (Linux)
xdg-open target/criterion/report/index.html
```

## Benchmark Details

### `parse_all_documents`
- **What**: Full document parsing with multi-pass compilation
- **Mirrors**: `examples/basic_usage.rs`
- **Measures**: End-to-end parse time for entire corpus

### `bid_generation_and_caching`
- **What**: BID generation with event accumulation
- **Mirrors**: `tests/codec_test/bid_tests.rs::test_belief_set_builder_bid_generation_and_caching`
- **Measures**: Parse time with BeliefBase event tracking

### `multi_pass_reference_resolution`
- **What**: Two consecutive parses (cold cache, then warm cache)
- **Measures**: Cache warming effects on parse performance

### `graph_queries`
- **What**: Document node queries and edge traversal
- **Measures**: Query performance on compiled graph (no I/O)

## Configuration

- **Sample size**: 50 iterations (reduced from default 100 due to file I/O)
- **Measurement time**: 10 seconds per benchmark
- **Corpus**: `tests/network_1` (~10KB, 9 markdown files, 31 cross-references)

## CI Integration

Benchmarks run automatically in GitHub Actions on push to `main`:
- See `.github/workflows/test.yml` → `benchmark` job
- Results stored as artifacts (not compared automatically)
- For local regression detection, use `--baseline` flag

## Future Work

See **ISSUE_47** for:
- Macro-benchmarks with realistic corpora (10KB → 100MB+)
- Memory profiling infrastructure
- GB-scale performance characterization