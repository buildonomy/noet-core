# Issue 7: Comprehensive Testing

**Priority**: HIGH - Required for v0.1.0  
**Estimated Effort**: 3-4 days
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
     - `beliefbase/` - Graph operations
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

### 7. Ignored Tests Investigation and Fix (0.5-1 day)

**File Watcher Integration Test (from Issue 19)**

**Context**: Test `test_file_modification_triggers_reparse` is currently ignored due to timing sensitivity.

- [ ] Manual verification: Confirm `noet watch` works correctly (Step 1 from Issue 19)
- [ ] If CLI works: Fix test infrastructure (mock watcher or longer timeouts)
- [ ] If CLI broken: Debug pipeline (add tracing, identify failure point)
- [ ] Make test reliable (>95% pass rate over 20 runs)
- [ ] Remove `#[ignore]` attribute
- [ ] Document platform-specific behavior if needed

**Decision: If manual CLI testing passes**, treat as test infrastructure issue and use mocking or extended timeouts. Don't spend >1 day on this - file watcher tests are inherently flaky in CI.

**Ignored Doctests in `src/codec/md.rs`**

**Context**: Three doctests are marked with `ignore` because they use incomplete examples or placeholders.

- [ ] `parse_title_attribute` (line 199):
  - Currently uses placeholder `Bref::from(...)`
  - Fix: Use proper `Bref::try_from("abc123").unwrap()` syntax
  - Add test for JSON parsing: `{"auto_title":true}`
  - Add test for user words extraction
- [ ] `build_title_attribute` (line 281):
  - Uses string literals instead of actual Bref objects
  - Fix: Import Bref type, use proper construction
  - Test all three formats: bref-only, with auto_title, with user words
- [ ] `make_relative_path` (line 315):
  - Examples look complete, might just need `ignore` removed
  - Verify examples compile and pass
  - Test edge cases: same directory, parent directory, nested paths

**Success Criteria**:
- [ ] All three doctests compile without `ignore` attribute
- [ ] Examples demonstrate actual API usage (not placeholders)
- [ ] `cargo test --doc` passes with 0 ignored tests in `md.rs`


**Critical first step**: Verify if `noet watch` actually works in real usage.

```bash
# Create test directory
mkdir -p /tmp/noet_test/network
cd /tmp/noet_test/network

# Create BeliefNetwork.toml
cat > BeliefNetwork.toml << EOF
id = "test-network"
title = "Test Network"
EOF

# Create initial document
cat > doc1.md << EOF
# Document 1

Initial content.
EOF

# Start watching
cargo run --features service --bin noet -- watch /tmp/noet_test

# In another terminal, modify doc1.md
echo "# Document 1\n\nModified content." > /tmp/noet_test/network/doc1.md

# Observe: Does noet watch output show reparse?
```

**Success criteria**:
- [ ] `noet watch` detects file change within 1-2 seconds
- [ ] Console output shows "Parsing..." or similar
- [ ] Database updated with new content
- [ ] No errors or warnings

**If this fails**: Real bug, proceed to Step 2
**If this succeeds**: Test environment issue, proceed to Step 3

**Open Questions re file watcher**

1. **Does `noet watch` CLI actually work in manual testing?**
   - If yes: Test environment issue only
   - If no: Critical bug blocking soft open source

2. **Which thread/component is the bottleneck?**
   - File watcher thread?
   - Compiler thread?
   - Transaction thread?
   - Event channel?

3. **Is 300ms debounce too aggressive?**
   - Should it be configurable?
   - Does test need longer wait for debounce + parse + transaction?

4. **Is this OS-specific?**
   - Linux inotify vs. macOS FSEvents vs. Windows ReadDirectoryChangesW
   - Test environment (container, VM, CI) affecting notifications?

5. **Are there existing issues in notify-debouncer-full?**
   - Check: https://github.com/notify-rs/notify/issues
   - Version: currently using notify-debouncer-full v0.3.1

### 8. Service Testing Infrastructure (from Issue 10) (1-1.5 days)

**Context**: Core library is well-tested. Service layer (`watch.rs`, feature = "service") has comprehensive rustdoc examples but minimal integration tests. Test skeleton exists at `tests/service_integration.rs`.

**WatchService API Testing**

- [ ] Review `WatchService` API and ensure it implements library operations (not product-specific)
- [ ] Verify rustdoc examples are comprehensive (currently 4 doctests, 240+ lines)
- [ ] Document operation semantics in module-level rustdoc
- [ ] Test `WatchService::new()` initialization with various configurations
- [ ] Test `enable_network_syncer()` / `disable_network_syncer()` lifecycle

**FileUpdateSyncer Integration Tests**

Expand `tests/service_integration.rs` skeleton to cover:

- [ ] Test: Initialize `FileUpdateSyncer` with temp directory
- [ ] Test: Create/modify file, verify compiler thread processes it
- [ ] Test: Verify `BeliefEvent`s flow correctly to transaction thread
- [ ] Test: Verify database sync completes (query DB to confirm)
- [ ] Test: Multiple file changes in quick succession, verify all processed
- [ ] Test: Handle parse errors gracefully (malformed markdown, invalid TOML)
- [ ] Test: Shutdown and cleanup (abort handles, drop resources)
- [ ] Document threading model and synchronization points in module doc

**File Watching Integration Tests**

- [ ] Test: `enable_network_syncer()` sets up file watcher correctly
- [ ] Test: File modification triggers debouncer callback
- [ ] Test: Debouncer filters dot files correctly (`.hidden`, `.git/`)
- [ ] Test: Debouncer filters by codec extensions (only `.md`, `.toml`, `.json`)
- [ ] Test: Compiler queue gets updated when file changes
- [ ] Test: `disable_network_syncer()` tears down watcher cleanly
- [ ] Verify no race conditions between debouncer and compiler thread
- [ ] Test platform-specific behavior (Linux inotify vs macOS FSEvents)

**Database Synchronization Tests**

- [ ] Test: `perform_transaction()` batches multiple events correctly
- [ ] Test: Events update SQLite database with correct data
- [ ] Test: Transaction errors are handled gracefully (DB locked, disk full)
- [ ] Test: Event channel backpressure handling (if applicable)
- [ ] Test: Database state matches `builder.doc_bb()` cache after sync
- [ ] Document transaction boundaries and consistency guarantees
- [ ] Test concurrent read operations during write transactions

**Success Criteria**:
- [ ] Integration test suite at `tests/service_integration.rs` passes
- [ ] Coverage for all major `WatchService` operations
- [ ] File watcher, compiler, and transaction threads tested end-to-end
- [ ] Database sync verified with actual queries
- [ ] Threading model and sync points documented
- [ ] Tests pass with `--features service` flag
- [ ] No race conditions or flaky behavior

## Testing Requirements

- All feature combinations pass tests
- Tests pass on Linux (required), macOS/Windows (strongly recommended)
- Standalone crate compiles and runs outside workspace
- Coverage >70% for core modules
- Benchmarks establish baseline metrics
- No test warnings or ignored tests without justification
  - File watcher test may remain ignored if timing-sensitive (document reason)
  - All doctests in `src/codec/md.rs` must be unskipped and passing
- Examples compile and run successfully

## Success Criteria

- [ ] All feature combinations tested and passing
- [ ] Tests verified on at least 2 platforms (Linux + one other)
- [ ] Standalone crate test successful
- [ ] Test coverage report generated and reviewed
- [ ] Performance baselines established
- [ ] Testing documentation complete
- [ ] CI can reproduce all tests
- [ ] No flaky or timing-dependent tests (or documented with justification)
- [ ] File watcher test (`test_file_modification_triggers_reparse`) either passing or documented as environment-specific
- [ ] All three doctests in `src/codec/md.rs` passing without `ignore` attribute
- [ ] `cargo test --doc` shows 0 ignored doctests
- [ ] Service integration tests at `tests/service_integration.rs` complete and passing
- [ ] End-to-end service layer testing (file watching → compilation → DB sync) verified

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
