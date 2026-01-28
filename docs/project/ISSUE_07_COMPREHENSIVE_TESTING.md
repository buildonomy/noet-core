# Issue 7: Comprehensive Testing

**Priority**: HIGH - Required for v0.1.0  
**Estimated Effort**: 3-4 days
**Dependencies**: Issues 1-6 complete (stable implementation)  
**Context**: Part of [`ROADMAP_NOET-CORE_v0.1.md`](./ROADMAP_NOET-CORE_v0.1.md) - ensures reliability before open source release

**CI/CD Strategy**: Repository is mirrored from GitLab to GitHub. GitHub Actions provides free runners for Linux, macOS, and Windows on public repositories, enabling comprehensive cross-platform testing without cost. See [`.github/workflows/test.yml`](../../.github/workflows/test.yml) for implementation.

## Summary

Establish comprehensive test coverage across all feature combinations and platforms. Verify the library works correctly in isolation (no workspace dependencies), test on multiple operating systems, review test coverage for critical paths, and set up automated testing infrastructure. This issue ensures `noet-core` is robust and reliable for external users.

## Goals

1. Test all feature flag combinations
2. Verify cross-platform compatibility (Linux, macOS, Windows) via GitHub Actions
3. Review and improve test coverage for critical paths
4. Test standalone crate (outside workspace)
5. Establish performance baselines with benchmarks
6. Document testing procedures
7. Verify GitHub Actions CI/CD pipeline is comprehensive and reliable

## Architecture

### CI/CD Infrastructure

**GitHub Actions** (`.github/workflows/test.yml`):
- **Free runners**: Linux, macOS, Windows on public repositories
- **Matrix testing**: OS × Rust version × feature flags
- **Parallel execution**: All combinations run simultaneously
- **Artifacts**: Coverage reports, documentation, benchmarks

**GitLab CI** (`.gitlab-ci.yml`):
- **Security scanning**: SAST, secret detection
- **Mirroring**: Automatic push to GitHub on main/tags
- **Optional**: Redundant testing, can be simplified

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

### 0. GitHub Actions Setup ✅ COMPLETE

**File**: `.github/workflows/test.yml`

- [x] Created comprehensive test matrix workflow
- [x] Matrix dimensions: OS (3) × Rust (2) × Features (3) = 18 combinations
- [x] Parallel execution for fast feedback
- [x] Separate jobs for: MSRV, examples, lint, docs, security, coverage, standalone
- [x] Artifact uploads for documentation and coverage reports
- [x] Summary job for branch protection

**Test Matrix Coverage**:
```yaml
os: [ubuntu-latest, macos-latest, windows-latest]
rust: [stable, beta]
features: [no-default, service, all]
```

**Additional Jobs**:
- MSRV check (Rust 1.85)
- Example verification
- Lint (rustfmt + clippy)
- Documentation generation
- Security audit (cargo-audit)
- Code coverage (tarpaulin + Codecov)
- Performance benchmarks (informational)
- Standalone crate test

**Success Criteria** ✅:
- [x] Workflow file created and committed
- [x] Tests will run on next push to GitHub mirror
- [x] All 18 feature combinations tested across 3 platforms
- [x] Free CI/CD for all platforms (no GitLab runner costs)

### 1. **Feature Combination Testing** (0.5 days - mostly automated)
   - [x] GitHub Actions tests all feature combinations automatically
   - [ ] Verify CI results: Check GitHub Actions run for all green
   - [ ] Document which features enable which functionality
   - [ ] Verify feature gates are correct (#[cfg(feature = "...")])
   - [ ] Check for feature leakage (features accidentally required)
   
   **Local verification** (optional, CI covers this):
   ```bash
   cargo test --no-default-features
   cargo test --features service
   cargo test --all-features
   ```

### 2. **Platform Testing** (0.5 days - automated via GitHub Actions)
   - [x] GitHub Actions tests on: ubuntu-latest, macos-latest, windows-latest
   - [ ] Monitor CI results for platform-specific failures
   - [ ] Identify platform-specific issues if any:
     - Path separator differences (Windows `\` vs Unix `/`)
     - File system case sensitivity (macOS insensitive, Linux sensitive)
     - Line ending handling (CRLF vs LF)
   - [ ] Document any platform-specific behavior
   - [ ] Fix platform-specific bugs if discovered
   
   **GitHub Actions handles**:
   - ✅ Automatic testing on all 3 platforms
   - ✅ Parallel execution (faster than sequential)
   - ✅ No manual setup required
   - ✅ Free for public repositories

### 3. **Standalone Crate Testing** (0.25 days - automated via GitHub Actions)
   - [x] GitHub Actions has `standalone` job that creates test project
   - [x] Tests noet-core as path dependency outside workspace
   - [x] Verifies public API works in isolation
   - [ ] Verify CI standalone test passes
   - [ ] Test with different Rust versions via GitHub Actions (stable, beta, MSRV)
   - [ ] Document minimum supported Rust version (MSRV): **Rust 1.85**
   
   **GitHub Actions workflow**:
   ```yaml
   standalone:
     - Create new cargo project
     - Add noet-core as path dependency
     - Build and run minimal example
     - Verifies no workspace leakage
   ```

### 4. **Test Coverage Review** (0.5 days)
   - [x] GitHub Actions runs `cargo tarpaulin` and uploads to Codecov
   - [ ] Review Codecov report: https://codecov.io/gh/buildonomy/noet-core
   - [ ] Review coverage for critical modules:
     - `codec/` - Document parsing and transformation
     - `beliefbase/` - Graph operations
     - `properties/` - BID generation and resolution
   - [ ] Identify untested code paths
   - [ ] Add tests for critical missing coverage:
     - Error conditions
     - Edge cases (empty files, forward refs, cycles)
     - Concurrent operations
   - [ ] Target: >70% coverage for core modules
   
   **Coverage tracking**:
   - GitHub Actions uploads to Codecov on every push
   - Badge available for README
   - Historical trends tracked automatically

### 5. **Performance Benchmarks** (0.5 days)
   - [ ] Create benchmark suite using Criterion (if not exists):
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
   - [x] GitHub Actions runs benchmarks on main branch (informational only)
   - [ ] Set up regression detection (optional for v0.1.0)
   
   **Note**: GitHub Actions `benchmark` job runs on push to main and stores results as artifacts.

### 6. **Testing Documentation** (0.5 days)
   - [ ] Create `docs/testing.md`:
     - How to run tests locally
     - Feature flag combinations
     - Platform-specific considerations
     - Coverage tools (tarpaulin, Codecov)
     - Benchmark procedures
     - **GitHub Actions CI/CD overview**
     - Link to `.github/workflows/test.yml`
   - [ ] Add testing section to CONTRIBUTING.md:
     - "Tests run automatically via GitHub Actions"
     - "Check CI status before merging PRs"
     - "Local testing: `cargo test --all-features`"
   - [ ] Document test organization:
     - Unit tests (in module files)
     - Integration tests (`tests/` directory)
     - Benchmarks (`benches/` directory)
     - Examples (`examples/` directory)
   - [ ] Document CI/CD infrastructure:
     - GitHub Actions for cross-platform testing
     - GitLab CI for security scanning and mirroring
     - Codecov for coverage tracking

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

- All feature combinations pass tests (automated via GitHub Actions)
- Tests pass on Linux, macOS, and Windows (via GitHub Actions)
- Standalone crate compiles and runs outside workspace (via GitHub Actions)
- Coverage >70% for core modules (tracked via Codecov)
- Benchmarks establish baseline metrics (stored as artifacts)
- No test warnings or ignored tests without justification
  - File watcher test may remain ignored if timing-sensitive (document reason)
  - All doctests in `src/codec/md.rs` must be unskipped and passing
- Examples compile and run successfully (verified via GitHub Actions)
- GitHub Actions workflow passes all jobs (test-summary job green)

## Success Criteria

- [x] GitHub Actions workflow created (`.github/workflows/test.yml`)
- [ ] All feature combinations tested and passing (18 combinations via CI)
- [ ] Tests verified on all 3 platforms: Linux, macOS, Windows (via GitHub Actions)
- [ ] Standalone crate test successful (via GitHub Actions)
- [ ] Test coverage report generated and reviewed (Codecov integrated)
- [ ] Performance baselines established (benchmarks run on main)
- [ ] Testing documentation complete (`docs/testing.md`)
- [ ] CI can reproduce all tests (GitHub Actions provides this)
- [ ] No flaky or timing-dependent tests (or documented with justification)
- [ ] File watcher test (`test_file_modification_triggers_reparse`) either passing or documented as environment-specific
- [ ] All three doctests in `src/codec/md.rs` passing without `ignore` attribute
- [ ] `cargo test --doc` shows 0 ignored doctests
- [ ] Service integration tests at `tests/service_integration.rs` complete and passing
- [ ] End-to-end service layer testing (file watching → compilation → DB sync) verified
- [ ] GitHub Actions test-summary job passes (indicates all required tests green)

## Risks

**Risk**: Platform-specific bugs only caught late  
**Mitigation**: ✅ GitHub Actions tests all platforms on every push; free and automatic

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
5. CI platform: GitHub Actions, GitLab CI, or both? **DECIDED: Both - GitHub for testing, GitLab for security**

## References

- **GitHub Actions Workflow**: `.github/workflows/test.yml` - Comprehensive test matrix
- **GitLab CI**: `.gitlab-ci.yml` - Security scanning and mirroring
- **Cargo Book - Features**: https://doc.rust-lang.org/cargo/reference/features.html
- **Rust Book - Testing**: https://doc.rust-lang.org/book/ch11-00-testing.html
- **Criterion.rs**: https://github.com/bheisler/criterion.rs
- **Tarpaulin**: https://github.com/xd009642/tarpaulin
- **Codecov**: https://codecov.io/gh/buildonomy/noet-core (after first push)
- **Pattern**: tokio testing approach (feature flags, cross-platform)
- **Current tests**: `tests/` directory (review existing test organization)
- **GitHub Actions Free Tier**: Unlimited minutes for public repositories on Linux, macOS, Windows
