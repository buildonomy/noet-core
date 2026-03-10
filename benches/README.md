# Performance Benchmarks

This directory contains Criterion-based performance benchmarks for noet-core.

## Benchmark Tiers

### Micro-benchmarks (`document_processing.rs`)

Function-level benchmarks using the small `tests/network_1` corpus (~10 KB).
Purpose: regression detection on specific operations.

- **`parse_all_documents`** — full document parsing with multi-pass compilation
- **`bid_generation_and_caching`** — BID generation with event accumulation
- **`multi_pass_reference_resolution`** — cold-cache vs warm-cache parse
- **`graph_queries`** — PathMap lookups and edge traversal on a compiled graph

### Macro-benchmarks (`macro_benchmarks.rs`)

Corpus-scale benchmarks using the [MDN content](https://github.com/mdn/content)
repository (~14 000 `index.md` files, ~55 MB of markdown). Large enough to
trigger noet's sharding strategy and reveal scaling behaviour invisible in the
micro-benchmark corpus.

- **`parse_throughput/mdn_en_us`** — single-pass parse + HTML generation across
  the full `files/en-us/` tree; measures end-to-end throughput (bytes/sec)
- **`cache_warmup/mdn_en_us`** — two consecutive passes (cold then warm);
  comparing against `parse_throughput` isolates the BID cache benefit
- **`graph_queries/traverse_all_edges`** — traverses every edge in the compiled
  graph; corpus is pre-compiled outside the timed loop

## Quick Start

```sh
# 1. Fetch the MDN corpus (sparse checkout, only *.md — ~30 s first time)
bash benches/fetch_corpora.sh

# 2. Run micro-benchmarks (fast, no corpus needed)
cargo bench --features service

# 3. Run macro-benchmarks (slow, corpus required)
cargo bench --bench macro_benchmarks --features bin,service

# 4. Browse HTML output
xdg-open target/bench-output/mdn/index.html   # Linux
open target/bench-output/mdn/index.html        # macOS
```

## Baseline Workflow

```sh
# Save a named baseline on main
cargo bench --bench macro_benchmarks --features bin,service -- --save-baseline main

# Compare a branch against the saved baseline
cargo bench --bench macro_benchmarks --features bin,service -- --baseline main
```

Criterion generates HTML reports under `target/criterion/`:

```sh
xdg-open target/criterion/report/index.html
```

## Corpus Details

| Property | Value |
|----------|-------|
| Source | `mdn/content` @ `6c53947` |
| Root | `.bench_corpora/mdn-content/files/en-us/` |
| Files | ~14 000 `index.md` files |
| Size | ~55 MB markdown |
| Structure | Every directory has an `index.md` — no staging needed |

The MDN layout matches what `NetworkCodec` expects: each directory is a subnet
identified by the presence of `index.md`. No copying or pre-processing is
performed; the benchmark reads directly from `.bench_corpora/`.

To advance the corpus baseline, update `MDN_SHA` in `fetch_corpora.sh` to a
newer commit on `mdn/content:main` and re-run `fetch_corpora.sh --force`.

## Configuration

| Setting | Micro | Macro |
|---------|-------|-------|
| Sample size | 50 | 10 |
| Measurement time | 10 s | 120 s |
| Corpus | `tests/network_1` (~10 KB) | `mdn/content` (~55 MB) |
| Features | `service` | `bin,service` |

Sample counts are intentionally low for macro-benchmarks: each iteration
involves real file I/O across thousands of files. At 10 samples the relative
comparisons (e.g. "warm cache is 2× faster") are still statistically sound
even if absolute numbers vary between machines.

## CI Integration

Micro-benchmarks run automatically in GitHub Actions on push to `main`
(see `.github/workflows/test.yml` → `benchmark` job). Results are stored as
artifacts but not automatically compared — use `--baseline` locally for
regression detection.

Macro-benchmarks are **not** run in CI: the MDN corpus fetch would add ~30 s
and the measurement time is too long for shared runners. Run them locally
before performance-sensitive merges.

## Relationship to ISSUE_47

- **ISSUE_07** established the micro-benchmarks for regression detection.
- **ISSUE_47** added the macro-benchmarks and memory profiling infrastructure.
  The MDN corpus replaces the earlier rust-book / rust-reference corpora, which
  were too small to trigger sharding and contained non-CommonMark syntax
  (`r[rule.id]` anchor labels in rust-reference) that `MdCodec` does not parse.