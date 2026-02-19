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
- Test output: Use `tests/*/test-output/` (gitignored, see below)
- See [Rust Book - Testing](https://doc.rust-lang.org/book/ch11-00-testing.html)

**Test Output Convention**:
```bash
# Standardized test output locations (all gitignored)
tests/browser/test-output/    # Browser test artifacts
tests/integration/test-output/ # Integration test output
test-output/                   # Ad-hoc testing at project root

# Generate test HTML/artifacts
./target/debug/noet parse tests/network_1 --html-output tests/browser/test-output

# Browser tests (automated)
./tests/browser/run.sh  # Uses tests/browser/test-output/
```

**Why standardized locations?**
- Single `.gitignore` rule: `test-output/`
- Predictable cleanup: `find tests -name test-output -type d -exec rm -rf {} +`
- CI can cache test artifacts by known paths
- Documentation examples use consistent paths

**Documentation**: Document public items
- See [DOCUMENTATION_STRATEGY.md](docs/project/DOCUMENTATION_STRATEGY.md) for organization
- Brief in rustdoc, link to detailed docs when needed

## CI/CD

GitHub Actions runs automatically:
- Cross-platform tests (Linux, macOS, Windows)
- Multiple Rust versions (stable, beta, MSRV 1.88)
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

assets/                 # UI assets (CSS, JS, fonts) - vendored in binary
├── package.json        # UI dependencies (npm)
├── node_modules/       # Downloaded dependencies (gitignored)
├── open-props/         # Vendored CSS framework (~38KB)
├── *.css               # Theme and layout styles (~24KB)
└── *.js                # Browser scripts

tests/                  # Integration tests
├── browser/            # WASM browser tests
│   └── README.md       # Browser testing guide
examples/               # Usage examples
benches/                # Performance benchmarks
docs/
├── architecture.md     # Conceptual overview
└── design/             # Technical specs
```

## UI Asset Workflow

The `assets/` directory contains CSS, JavaScript, and other UI resources for the HTML viewer. These assets are **always embedded in the binary** using `include_dir`, making the binary self-contained and offline-first. Assets are managed via npm but vendored at compile time.

### Working with UI Assets

```bash
# Install/update UI dependencies
cd assets
npm install

# Vendor Open Props CSS framework
npm run copy:open-props

# Verify vendored assets
ls -lh open-props/
# Should see: normalize.min.css (~9KB), open-props.min.css (~29KB)
```

**Key Points:**
- Assets are embedded in binary at compile time (~40KB overhead)
- `node_modules/` is gitignored (only needed during development)
- `open-props/` is committed (vendored and embedded)
- Theme CSS files (`noet-theme-*.css`, `noet-layout.css`) are committed and embedded
- Users can optionally use `--cdn` flag to reference Open Props from CDN
- See [`tests/browser/README.md`](tests/browser/README.md) for browser testing

**Binary Size Impact:**
- Base binary: ~2MB
- With embedded assets: ~2.04MB (+40KB, 2% increase)
- Negligible overhead for offline-first capability

### Adding New UI Dependencies

1. Update `assets/package.json` with new dependency
2. Run `npm install` in `assets/`
3. Add vendor script to `package.json` to copy files from `node_modules/`
4. Commit vendored files to `assets/` (they will be embedded in binary)
5. Do not commit `node_modules/` (gitignored)

**Note:** All committed files in `assets/` are embedded in the binary at compile time via `include_dir!`. Keep vendored assets minimal to avoid binary bloat.

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