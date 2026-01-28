# Contributing to noet-core

Thank you for your interest in contributing! This guide follows the principle of [smooth iterative deepening](docs/design/smooth_iterative_deepening.md) - start simple, deepen as needed.

## Quick Start

```bash
# Fork and clone
git clone https://github.com/yourusername/noet-core
cd noet-core

# Check your changes
cargo fmt --check
cargo clippy --all-features --all-targets -- -D warnings
cargo test --all-features

# Open PR on GitHub
```

**CI will verify** everything automatically. See [`.github/workflows/test.yml`](.github/workflows/test.yml) for what's checked.

## Development Workflow

**Current (pre-v0.1.0)**: Direct commits to `main`
**After v0.1.0**: Feature branches → PRs to `main`

### Commit Messages

Use [conventional commits](https://www.conventionalcommits.org/):
- `feat:` New feature
- `fix:` Bug fix
- `docs:` Documentation
- `test:` Tests
- `refactor:` Code changes without behavior change

Example: `feat: add YAML codec support`

## Code Standards

**Rust Style**: Follow [Rust API Guidelines](https://rust-lang.github.io/api-guidelines/)
- `cargo fmt` for formatting (enforced in CI)
- Fix all `cargo clippy` warnings
- Document public APIs with `///` doc comments
- Prefer `Result` over `.unwrap()`, use `.expect("reason")` when safe

**Testing**: Write tests for new code
- Unit tests: Same file as code in `#[cfg(test)]` module
- Integration tests: `tests/` directory
- See [Rust Book - Testing](https://doc.rust-lang.org/book/ch11-00-testing.html)

**Documentation**: Document public items
- See [DOCUMENTATION_STRATEGY.md](docs/project/DOCUMENTATION_STRATEGY.md) for organization
- Brief in rustdoc, link to detailed docs when needed

## CI/CD

GitHub Actions runs automatically:
- Cross-platform tests (Linux, macOS, Windows)
- Multiple Rust versions (stable, beta, MSRV 1.85)
- Feature combinations
- Formatting, linting, docs, examples, coverage

**View results**: https://github.com/buildonomy/noet-core/actions

**Details**: See [`.github/workflows/test.yml`](.github/workflows/test.yml)

## Project Structure

```
src/
├── lib.rs              # Library entry, overview docs
├── beliefbase.rs       # Core graph structures
├── codec/              # Document parsers
├── properties.rs       # Node/edge types, BIDs
└── ...

tests/                  # Integration tests
examples/               # Usage examples
benches/                # Performance benchmarks
docs/
├── architecture.md     # Conceptual overview
└── design/             # Technical specs
```

## Getting Help

- **Questions**: [GitHub Discussions](https://github.com/buildonomy/noet-core/discussions)
- **Bugs**: [GitHub Issues](https://github.com/buildonomy/noet-core/issues)
- **Design Docs**: See [`docs/design/`](docs/design/)

## Good First Issues

Look for [`good-first-issue`](https://github.com/buildonomy/noet-core/labels/good-first-issue) label:
- Documentation improvements
- Test additions
- Simple bug fixes
- Example code

## Before Major Features

Open an issue first to discuss:
1. Describe the feature
2. Wait for maintainer feedback
3. Proceed when approved

This prevents wasted effort on misaligned features.

## Recognition

Contributors are recognized in:
- CHANGELOG.md
- GitHub contributors page
- Release notes

---

**Philosophy**: This project follows [smooth iterative deepening](docs/design/smooth_iterative_deepening.md). Documentation should be brief with links to deeper material. Start simple, deepen iteratively.

Thank you for contributing! 🎉