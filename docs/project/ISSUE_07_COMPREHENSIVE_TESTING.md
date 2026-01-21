# Issue 7: Comprehensive Testing

**Priority**: HIGH - Required for v0.1.0  
**Estimated Effort**: 2-3 days  
**Dependencies**: Issues 1-6 complete (stable implementation)  
**Context**: Part of [`ROADMAP_OPEN_SOURCE_NOET-CORE.md`](./ROADMAP_OPEN_SOURCE_NOET-CORE.md) - ensures reliability before open source release

## Summary

Establish comprehensive test coverage across all feature combinations and platforms. Verify the library works correctly in isolation (no workspace dependencies), test on multiple operating systems, review test coverage for critical paths, and set up automated testing infrastructure. This issue ensures `noet-core` is robust and reliable for external users.

## Goals

1. Test all feature flag combinations
2. Verify cross-platform compatibility (Linux, macOS, Windows)
3. Review and improve test coverage for critical paths
4. Test standalone crate (outside workspace)
5. Establish performance baselines with benchmarks
6. Document testing procedures

## Architecture

### Feature Combinations

Current features in `Cargo.toml`:
- `default = []` - Minimal, no optional dependencies
- `service` - File watching, database, full sync capabilities
- `wasm` - WebAssembly target support

**Test Matrix**:
```
1. No features (default)
2. --features service
3. --features wasm
4. --all-features
5. --no-default-features
```

### Platform Matrix

**Primary Targets**:
- Linux (x86_64-unknown-linux-gnu) - CI primary
- macOS (x86_64-apple-darwin, aarch64-apple-darwin)
- Windows (x86_64-pc-windows-msvc)

**Secondary Targets**:
- WASM (wasm32-unknown-unknown) - with `wasm` feature
- Linux ARM (aarch64-unknown-linux-gnu) - future

## Implementation Steps

1. **Feature Combination Testing** (1 day)
   - [ ] Test with no features:
     ```bash
     cargo test --no-default-features
     ```
   - [ ] Test with `service` feature:
     ```bash
     cargo test --features service
     ```
   - [ ] Test with `wasm` feature:
     ```bash
     cargo test --features wasm
     ```
   - [ ] Test with all features:
     ```bash
     cargo test --all-features
     ```
   - [ ] Document which features enable which functionality
   - [ ] Verify feature gates are correct (#[cfg(feature = "...")])
   - [ ] Check for feature leakage (features accidentally required)

2. **Platform Testing** (1 day)
   - [ ] Test on Linux (primary development platform):
     ```bash
     cargo test --all-features
     ```
   - [ ] Test on macOS (if available):
     ```bash
     cargo test --all-features
     ```
   - [ ] Test on Windows (if available or via CI):
     ```bash
     cargo test --all-features
     ```
   - [ ] Identify platform-specific issues:
     - Path separator differences
     - File system case sensitivity
     - Line ending handling
   - [ ] Document any platform-specific behavior

3. **Standalone Crate Testing** (0.5 days)
   - [ ] Create test directory outside workspace
   - [ ] Add `noet-core` as dependency (path or git):
     ```toml
     [dependencies]
     noet-core = { path = "../../rust_core/crates/core" }
     ```
   - [ ] Write minimal example using public API
   - [ ] Verify no workspace dependencies leak through
   - [ ] Test with different Rust versions (stable, beta, nightly)
   - [ ] Document minimum supported Rust version (MSRV)

4. **Test Coverage Review** (0.5 days)
   - [ ] Run coverage tool (e.g., `cargo tarpaulin`):
     ```bash
     cargo tarpaulin --all-features --out Html
     ```
   - [ ] Review coverage report for critical modules:
     - `codec/` - Document parsing and transformation
     - `beliefset/` - Graph operations
     - `properties/` - BID generation and resolution
   - [ ] Identify untested code paths
   - [ ] Add tests for critical missing coverage:
     - Error conditions
     - Edge cases (empty files, forward refs, cycles)
     - Concurrent operations
   - [ ] Target: >70% coverage for core modules

5. **Performance Benchmarks** (0.5 days)
   - [ ] Create benchmark suite using Criterion:
     ```rust
     // benches/parsing.rs
     use criterion::{criterion_group, criterion_main, Criterion};
     
     fn benchmark_parse(c: &mut Criterion) {
         c.bench_function("parse_simple_doc", |b| {
             b.iter(|| {
                 // Benchmark parsing logic
             });
         });
     }
     
     criterion_group!(benches, benchmark_parse);
     criterion_main!(benches);
     ```
   - [ ] Benchmark key operations:
     - Document parsing (small, medium, large)
     - BID injection
     - Graph querying
     - Multi-pass compilation
   - [ ] Establish baseline metrics
   - [ ] Document expected performance characteristics
   - [ ] Set up regression detection (optional for v0.1.0)

6. **Testing Documentation** (0.5 days)
   - [ ] Create `docs/testing.md`:
     - How to run tests
     - Feature flag combinations
     - Platform-specific considerations
     - Coverage tools
     - Benchmark procedures
   - [ ] Add testing section to CONTRIBUTING.md
   - [ ] Document test organization:
     - Unit tests (in module files)
     - Integration tests (`tests/` directory)
     - Benchmarks (`benches/` directory)
     - Examples (`examples/` directory)

## Testing Requirements

- All feature combinations pass tests
- Tests pass on Linux (required), macOS/Windows (strongly recommended)
- Standalone crate compiles and runs outside workspace
- Coverage >70% for core modules
- Benchmarks establish baseline metrics
- No test warnings or ignored tests without justification
- Examples compile and run successfully

## Success Criteria

- [ ] All feature combinations tested and passing
- [ ] Tests verified on at least 2 platforms (Linux + one other)
- [ ] Standalone crate test successful
- [ ] Test coverage report generated and reviewed
- [ ] Performance baselines established
- [ ] Testing documentation complete
- [ ] CI can reproduce all tests
- [ ] No flaky or timing-dependent tests

## Risks

**Risk**: Platform-specific bugs only caught late  
**Mitigation**: Set up CI for multiple platforms early; document platform differences

**Risk**: Feature combination explosions (2^n combinations)  
**Mitigation**: Focus on common combinations; document which are tested

**Risk**: Tests pass in workspace but fail standalone  
**Mitigation**: Test extraction early; verify no workspace leakage

**Risk**: Low coverage in critical paths  
**Mitigation**: Prioritize high-value tests; defer exhaustive coverage to post-1.0

**Risk**: Benchmarks are noisy or unreliable  
**Mitigation**: Run multiple iterations; document variance; focus on trends not absolutes

## Open Questions

1. What's our minimum supported Rust version (MSRV)? (Suggest: 1.70+)
2. Should we test on Rust beta/nightly, or just stable?
3. Coverage target: 70%, 80%, or best-effort?
4. Which benchmark results should we publish?
5. CI platform: GitHub Actions, GitLab CI, or both?

## References

- **Cargo Book - Features**: https://doc.rust-lang.org/cargo/reference/features.html
- **Rust Book - Testing**: https://doc.rust-lang.org/book/ch11-00-testing.html
- **Criterion.rs**: https://github.com/bheisler/criterion.rs
- **Tarpaulin**: https://github.com/xd009642/tarpaulin
- **Pattern**: tokio testing approach (feature flags, cross-platform)
- **Current tests**: `tests/` directory (review existing test organization)