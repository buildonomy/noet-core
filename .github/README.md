# GitHub Actions CI/CD

Automated testing runs on push/PR. No setup required for contributors.

## Workflows

- **[`test.yml`](workflows/test.yml)** - Cross-platform testing (Linux, macOS, Windows)
  - 18 test combinations (OS × Rust × features)
  - Lint, docs, coverage, benchmarks
  - Mirrors to GitLab after tests pass

**View results**: https://github.com/buildonomy/noet-core/actions

## For Contributors

```bash
# CI runs automatically on your PR
cargo test --all-features  # Verify locally (optional)
```

See [CONTRIBUTING.md](../CONTRIBUTING.md) for details.

## For Maintainers

**Mirror setup**: Set `GITLAB_MIRROR_TOKEN` secret in GitHub repository settings

---

**Why GitHub?** Free runners for all platforms. See workflow file for complete details.