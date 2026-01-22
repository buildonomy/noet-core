# CI/CD Setup Guide

This document explains the GitLab CI/CD pipeline configuration for noet-core and how to optimize it for your environment.

## Pipeline Overview

The pipeline is organized into 5 stages:

1. **lint**: Code formatting and style checks
2. **test**: Multi-platform and multi-version testing
3. **security**: Security scanning and dependency auditing
4. **docs**: Documentation generation and verification
5. **deploy**: Publishing to crates.io and GitLab Pages

## Current Configuration

### Lint Stage

- **fmt**: Verifies code formatting with `rustfmt`
- **clippy**: Lints code with `clippy` (all warnings treated as errors)

### Test Stage

#### Linux Testing (Docker-based)
✅ **Fully Configured** - No additional setup required

- `test:linux:stable`: Latest stable Rust (required)
- `test:linux:beta`: Beta channel (allowed to fail)
- `test:linux:nightly`: Nightly channel (allowed to fail)
- `test:linux:msrv`: Minimum Supported Rust Version 1.70 (required)

Each job tests:
- Build with all features
- Tests with all features
- Tests with no features
- Tests with `service` feature only

#### macOS Testing
⚠️ **Requires macOS Runner** - Currently set to `allow_failure: true`

- `test:macos:stable`: Tests on macOS with latest stable Rust

#### Windows Testing
⚠️ **Requires Windows Runner** - Currently set to `allow_failure: true`

- `test:windows:stable`: Tests on Windows with latest stable Rust

#### Example Verification
✅ **Fully Configured**

- `test:examples`: Builds and runs all examples

### Security Stage

✅ **Fully Configured**

- **sast**: Static Application Security Testing (GitLab template)
- **secret_detection**: Scans for exposed secrets (GitLab template)
- **audit**: Checks dependencies for known vulnerabilities with `cargo-audit`

### Documentation Stage

✅ **Fully Configured**

- **docs**: Generates documentation (artifact saved)
- **docs:check**: Ensures documentation builds without warnings

### Deploy Stage

🔐 **Requires Configuration**

- **publish:crates-io**: Publishes to crates.io (manual trigger, tags only)
  - Requires `CRATES_IO_TOKEN` CI/CD variable
- **pages**: Deploys documentation to GitLab Pages (automatic on main branch)

## Runner Requirements

### Shared Runners (Fully Supported)

GitLab's shared Docker runners work out-of-the-box for:
- All Linux test jobs
- Lint jobs
- Security jobs
- Documentation jobs
- Example verification

**No additional configuration needed** for these jobs.

### Platform-Specific Runners (Optional)

For macOS and Windows testing, you have three options:

#### Option 1: Use GitLab Shared Runners (If Available)

GitLab.com provides shared runners for macOS and Windows on some tiers. Check your GitLab plan.

#### Option 2: Register Your Own Runners

Register platform-specific runners with appropriate tags:

**macOS Runner:**
```bash
gitlab-runner register \
  --url https://gitlab.com/ \
  --registration-token YOUR_TOKEN \
  --tag-list macos \
  --executor shell \
  --description "macOS Runner"
```

**Windows Runner:**
```powershell
gitlab-runner.exe register `
  --url https://gitlab.com/ `
  --registration-token YOUR_TOKEN `
  --tag-list windows `
  --executor shell `
  --description "Windows Runner"
```

#### Option 3: Disable Platform-Specific Jobs

If you don't have access to macOS/Windows runners, you can:

1. Leave as-is: Jobs are set to `allow_failure: true`, so they won't block CI
2. Remove the jobs: Delete the macOS/Windows job definitions
3. Use GitHub Actions: Run platform-specific tests on GitHub (see below)

## Configuring Secrets

### For crates.io Publishing

1. Get your crates.io API token:
   ```bash
   cargo login
   # Token is stored in ~/.cargo/credentials.toml
   ```

2. Add to GitLab CI/CD variables:
   - Go to: **Settings → CI/CD → Variables**
   - Add variable:
     - Key: `CRATES_IO_TOKEN`
     - Value: Your token from crates.io
     - Flags: ✅ Protected, ✅ Masked

3. Publishing workflow:
   ```bash
   # Create and push a tag
   git tag v0.1.0
   git push origin v0.1.0
   
   # Manually trigger publish job in GitLab UI
   # (Or configure automatic publishing on tags)
   ```

## Optimization Tips

### Cache Configuration

The pipeline uses Cargo caching to speed up builds:
- Cache key based on `Cargo.lock` and job name
- Caches both `.cargo/` (dependencies) and `target/` (build artifacts)

To clear cache if needed:
```bash
# In GitLab UI: CI/CD → Pipelines → Clear Runner Caches
```

### Parallel Execution

Jobs in the same stage run in parallel. With sufficient runners:
- All Linux versions test simultaneously
- Lint, test, security, and docs stages proceed in order
- Total pipeline time ≈ slowest job + stage overhead

### Reducing CI Time

If CI is too slow, consider:

1. **Reduce test matrix**: Remove beta/nightly tests
2. **Selective testing**: Only run full tests on `main` branch
3. **Split test jobs**: Separate feature combinations into parallel jobs
4. **Use cargo-nextest**: Faster test execution (requires update to jobs)

Example for selective testing:
```yaml
test:linux:stable:
  only:
    - main
    - merge_requests
    changes:
      - "**/*.rs"
      - Cargo.toml
      - Cargo.lock
```

## GitLab Pages

Documentation is automatically deployed to GitLab Pages on pushes to `main`:

- URL: `https://buildonomy.gitlab.io/noet-core/noet_core/`
- Auto-redirects from root to `noet_core` module
- Updated on every merge to main

To disable: Remove the `pages:` job from `.gitlab-ci.yml`

## Monitoring Pipeline Health

### Required Jobs (Must Pass)

- ✅ fmt
- ✅ clippy
- ✅ test:linux:stable
- ✅ test:linux:msrv
- ✅ test:examples
- ✅ sast
- ✅ secret_detection
- ✅ docs:check

### Optional Jobs (Can Fail)

- ⚠️ test:linux:beta
- ⚠️ test:linux:nightly
- ⚠️ test:macos:stable (if no runner)
- ⚠️ test:windows:stable (if no runner)
- ⚠️ audit (advisory warnings)
- ⚠️ coverage (optional metric)

### Pipeline Status

Check pipeline status at:
```
https://gitlab.com/buildonomy/noet-core/-/pipelines
```

Add badge to README:
```markdown
[![pipeline status](https://gitlab.com/buildonomy/noet-core/badges/main/pipeline.svg)](https://gitlab.com/buildonomy/noet-core/-/commits/main)
```

## Alternative: GitHub Actions

If you prefer GitHub Actions for cross-platform testing:

1. Keep GitLab CI for security and Linux tests
2. Add `.github/workflows/cross-platform.yml` for macOS/Windows
3. GitHub provides free macOS and Windows runners

Example workflow structure:
```yaml
name: Cross-Platform Tests
on: [push, pull_request]
jobs:
  test:
    strategy:
      matrix:
        os: [macos-latest, windows-latest]
    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@v3
      - uses: dtolnay/rust-toolchain@stable
      - run: cargo test --all-features
```

## Troubleshooting

### "No runner available for job"

**Problem**: Job stuck in "pending" state  
**Solution**: 
- Check if runners with required tags exist
- Use shared runners for Linux jobs
- Add `allow_failure: true` for optional jobs

### "Cargo cache corruption"

**Problem**: Build fails with dependency errors  
**Solution**:
```bash
# Clear CI/CD cache in GitLab UI
# Or add to job:
before_script:
  - rm -rf $CARGO_HOME
```

### "Documentation warnings fail build"

**Problem**: `docs:check` job fails  
**Solution**: Fix doc warnings locally:
```bash
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features
```

### "SAST job fails"

**Problem**: Security scanner reports issues  
**Solution**: Review SAST report in Security Dashboard, address issues or mark as false positive

## Next Steps

After CI/CD is configured:

1. ✅ Verify all required jobs pass
2. ✅ Configure `CRATES_IO_TOKEN` for publishing
3. ✅ Add CI badge to README
4. ✅ Enable GitLab Pages (automatic)
5. ⏭️ Proceed with Issue 10 (CLI and Daemon)

## References

- [GitLab CI/CD Documentation](https://docs.gitlab.com/ee/ci/)
- [GitLab Runner Documentation](https://docs.gitlab.com/runner/)
- [cargo-audit](https://github.com/RustSec/rustsec/tree/main/cargo-audit)
- [Rust CI Best Practices](https://doc.rust-lang.org/cargo/guide/continuous-integration.html)