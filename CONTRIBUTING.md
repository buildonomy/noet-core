# Contributing to noet-core

Thank you for your interest in contributing to noet-core! This document provides guidelines and information for contributors.

## Code of Conduct

Be respectful, inclusive, and considerate in all interactions. We're here to build something useful together.

## Getting Started

### Prerequisites

- Rust 1.70 or later
- Git
- Familiarity with async Rust (tokio)

### Setting Up Your Development Environment

1. Fork the repository on GitHub
2. Clone your fork:
   ```bash
   git clone https://github.com/yourusername/noet-core
   cd noet-core
   ```
3. Add the upstream repository:
   ```bash
   git remote add upstream https://gitlab.com/buildonomy/noet-core
   ```
4. Install dependencies:
   ```bash
   cargo build
   ```
5. Run tests to ensure everything works:
   ```bash
   cargo test
   ```

## Development Workflow

### Branch Strategy

- `main`: Stable, released code
- `develop`: Integration branch for features
- `feature/*`: New features
- `fix/*`: Bug fixes
- `docs/*`: Documentation improvements

### Making Changes

1. Create a new branch from `develop`:
   ```bash
   git checkout develop
   git pull upstream develop
   git checkout -b feature/your-feature-name
   ```

2. Make your changes, following the coding standards below

3. Write tests for your changes

4. Run the test suite:
   ```bash
   cargo test
   ```

5. Check formatting and lints:
   ```bash
   cargo fmt --check
   cargo clippy -- -D warnings
   ```

6. Commit your changes with clear, descriptive messages:
   ```bash
   git commit -m "feat: add support for custom codecs"
   ```

### Commit Message Convention

We follow conventional commits format:

- `feat:` New feature
- `fix:` Bug fix
- `docs:` Documentation changes
- `test:` Test additions or modifications
- `refactor:` Code refactoring
- `perf:` Performance improvements
- `chore:` Maintenance tasks

Example:
```
feat: add YAML codec support

- Implement YamlCodec struct
- Add yaml parsing tests
- Update documentation

Closes #123
```

### Pull Request Process

1. Push your branch to your fork:
   ```bash
   git push origin feature/your-feature-name
   ```

2. Open a Pull Request against the `develop` branch

3. Fill out the PR template with:
   - Clear description of changes
   - Related issue numbers
   - Testing performed
   - Breaking changes (if any)

4. Wait for review and address feedback

5. Once approved, a maintainer will merge your PR

## Coding Standards

### Rust Style

- Follow the [Rust API Guidelines](https://rust-lang.github.io/api-guidelines/)
- Use `cargo fmt` for formatting (enforced in CI)
- Fix all `cargo clippy` warnings
- Prefer explicit error types over `.unwrap()`
- If an explicit error type is not possible, use `.expect("Including your reasoning why this cannot fail")`
- Document public APIs with doc comments (`///`)

### Documentation

All public items must have documentation:

```rust
/// Parses a document and returns a BeliefSet.
///
/// # Arguments
///
/// * `path` - Path to the document to parse
/// * `belief_set` - Mutable reference to the target BeliefSet
///
/// # Examples
///
/// ```
/// use noet::codec::parse_document;
///
/// let result = parse_document("doc.md", &mut belief_set)?;
/// ```
///
/// # Errors
///
/// Returns an error if the file cannot be read or parsed.
pub fn parse_document(path: &Path, belief_set: &mut BeliefSet) -> Result<()> {
    // implementation
}
```

### Testing

- Write unit tests for new functionality
- Add integration tests for major features
- Use descriptive test names:
  ```rust
  #[test]
  fn test_parser_handles_forward_references() {
      // test implementation
  }
  ```
- Test error conditions, not just happy paths
- Use `test-log` for tests that need logging:
  ```rust
  #[test_log::test]
  fn test_with_logging() {
      // test implementation
  }
  ```

### Error Handling

- Use `thiserror` for error types
- Provide context with error messages
- Don't panic in library code (except for `unreachable!()` in truly unreachable code)

Example:
```rust
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ParseError {
    #[error("failed to read file {path}: {source}")]
    FileRead {
        path: String,
        source: std::io::Error,
    },
    
    #[error("invalid syntax at line {line}: {message}")]
    Syntax {
        line: usize,
        message: String,
    },
}
```

## Project Structure

```
noet/
├── src/
│   ├── beliefset.rs      # Core hypergraph data structures
│   ├── codec/            # Document parsing and codecs
│   ├── properties.rs     # Node/edge types and BIDs
│   ├── event.rs          # Event streaming
│   ├── query.rs          # Query language
│   ├── paths.rs          # Path resolution
│   ├── compiler.rs       # Multi-pass compilation
│   └── lib.rs            # Library entry point
├── tests/                # Integration tests
├── examples/             # Example usage
└── benches/              # Benchmarks
```

## Areas for Contribution

### High Priority

- [ ] Additional codec implementations (YAML, JSON, etc.)
- [ ] Query language improvements
- [ ] Performance optimizations
- [ ] Documentation and examples
- [ ] Integration with popular tools

### Good First Issues

Look for issues labeled `good-first-issue` in the issue tracker. These are typically:
- Documentation improvements
- Simple bug fixes
- Test additions
- Example code

### Feature Requests

Before implementing a major feature:
1. Open an issue to discuss the feature
2. Wait for maintainer feedback
3. Proceed with implementation once approved

This prevents wasted effort on features that may not align with project goals.

## Testing

### Running Tests

```bash
# Run all tests
cargo test

# Run specific test
cargo test test_name

# Run with logging output
RUST_LOG=debug cargo test

# Run integration tests only
cargo test --test integration_tests
```

### Writing Tests

Place unit tests in the same file as the code:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_belief_set_creation() {
        let bs = BeliefSet::new();
        assert!(bs.is_empty());
    }
}
```

Integration tests go in `tests/`:
```rust
// tests/integration_tests.rs
use noet::*;

#[test]
fn test_full_parse_cycle() {
    // test implementation
}
```

## Documentation

### Building Documentation

```bash
# Build and open docs
cargo doc --open

# Build docs with private items
cargo doc --document-private-items --open
```

### Documentation Guidelines

- Document all public APIs
- Include examples in doc comments
- Use `#[doc(hidden)]` for internal-but-public items
- Add module-level documentation explaining the module's purpose

## Performance

### Benchmarking

```bash
# Run benchmarks
cargo bench

# Run specific benchmark
cargo bench bench_name
```

### Profiling

For performance-critical changes:
1. Add benchmarks
2. Profile before and after
3. Document performance impact in PR

## Release Process

(For maintainers)

1. Update version in `Cargo.toml`
2. Update CHANGELOG.md
3. Create release tag: `git tag v0.x.0`
4. Push tag: `git push --tags`
5. Publish to crates.io: `cargo publish`

## Getting Help

- **Questions**: Open a discussion on GitHub
- **Bugs**: Open an issue with reproduction steps
- **Security**: Email security@example.com (do not open public issues)

## Recognition

Contributors are recognized in:
- CHANGELOG.md for significant contributions
- GitHub contributors page
- Release notes

Thank you for contributing to noet! 🎉
