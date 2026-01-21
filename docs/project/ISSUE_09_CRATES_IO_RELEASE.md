# Issue 9: Crates.io Publication & Release

**Priority**: MEDIUM - Final step before v0.1.0 public release  
**Estimated Effort**: 2-3 days  
**Dependencies**: Issues 5-8 complete (docs, quality, tests, repo ready)  
**Context**: Part of [`ROADMAP_OPEN_SOURCE_NOET-CORE.md`](./ROADMAP_OPEN_SOURCE_NOET-CORE.md) - publishes crate to crates.io and announces

## Summary

Prepare `noet-core` for publication to crates.io, conduct final validation, publish the crate, create GitHub release, and announce to the Rust community. This issue covers the complete release process from dependency verification through community announcements. **Note**: Publication to crates.io is permanent and cannot be undone (only yanked), so thorough validation is critical.

## Goals

1. Verify all dependencies are published or have proper alternatives
2. Validate package contents and metadata
3. Test dry-run publication
4. Publish v0.1.0 to crates.io
5. Create GitHub release with notes
6. Announce to Rust community (TWiR, Reddit, forums)
7. Set up post-release monitoring

## Architecture

### Release Checklist Philosophy

**Quality over speed**: A polished first release builds trust and adoption. Better to delay than publish with known issues.

**Communication matters**: Clear release notes, migration guides, and community engagement drive adoption.

**Versioning commitment**: Pre-1.0 allows breaking changes, but communicate them clearly. Post-1.0 requires semantic versioning discipline.

### Publication Process

```
1. Dependency Audit
   ↓
2. Package Validation (cargo package)
   ↓
3. Final Review (metadata, docs, tests)
   ↓
4. Dry-run Publication
   ↓
5. Actual Publication (cargo publish)
   ↓
6. GitHub Release Creation
   ↓
7. Community Announcements
   ↓
8. Post-release Monitoring
```

## Implementation Steps

1. **Dependency Audit** (0.5 days)
   - [ ] Extract all workspace dependencies to `Cargo.toml`:
     ```bash
     # Review workspace Cargo.toml dependencies
     grep -A 1000 "\[workspace.dependencies\]" ../../Cargo.toml
     ```
   - [ ] For each dependency, verify:
     - Is it published on crates.io?
     - Version constraints appropriate? (avoid `*`, prefer `^1.2`)
     - Optional dependencies properly gated by features?
   - [ ] Remove any internal workspace crates (`noet_config`, `noet_db`, etc.)
   - [ ] Replace git dependencies with crates.io versions (if available)
   - [ ] Update version numbers to latest stable
   - [ ] Run `cargo tree` to verify dependency graph
   - [ ] Check for unused dependencies:
     ```bash
     cargo +nightly udeps --all-targets
     ```
   - [ ] Document any git/path dependencies in README (if necessary)

2. **Package Validation** (1 day)
   - [ ] Run package command:
     ```bash
     cargo package --allow-dirty
     ```
   - [ ] Review packaged contents:
     ```bash
     cargo package --list
     ```
   - [ ] Verify included files:
     - All source files
     - Documentation
     - Examples
     - Tests
     - LICENSE files
     - README.md
     - CHANGELOG.md
   - [ ] Verify excluded files (via `.cargo_vcs_info.json`):
     - No internal docs
     - No workspace artifacts
     - No proprietary references
   - [ ] Extract and inspect package:
     ```bash
     tar -xzf target/package/noet-core-0.1.0.crate
     cd noet-core-0.1.0
     cargo build --all-features
     cargo test --all-features
     ```
   - [ ] Verify metadata in extracted Cargo.toml
   - [ ] Check package size (warn if >10MB)

3. **Final Validation** (0.5 days)
   - [ ] Review Cargo.toml metadata:
     - `name = "noet-core"`
     - `version = "0.1.0"`
     - `description` is compelling (under 200 chars)
     - `keywords` are relevant (max 5)
     - `categories` are appropriate
     - `repository` URL is correct
     - `documentation` points to docs.rs
     - `readme = "README.md"`
     - `license = "MIT OR Apache-2.0"`
   - [ ] Verify documentation builds:
     ```bash
     cargo doc --no-deps --all-features --open
     ```
   - [ ] Check for doc warnings:
     ```bash
     RUSTDOCFLAGS="-D warnings" cargo doc --no-deps
     ```
   - [ ] Run final test suite:
     ```bash
     cargo test --all-features
     cargo test --no-default-features
     ```
   - [ ] Run clippy one last time:
     ```bash
     cargo clippy --all-features --all-targets -- -D warnings
     ```
   - [ ] Verify examples run:
     ```bash
     cargo run --example basic_usage
     cargo run --example file_watching
     cargo run --example querying
     ```
   - [ ] Review README for clarity and accuracy
   - [ ] Check CHANGELOG.md has v0.1.0 entry

4. **Dry-run Publication** (0.5 days)
   - [ ] Test publish without uploading:
     ```bash
     cargo publish --dry-run --allow-dirty
     ```
   - [ ] Review output for warnings or errors
   - [ ] Verify package size is acceptable
   - [ ] Check that all required files are included
   - [ ] Ensure no secrets or tokens in package
   - [ ] Get 2-3 people to review before actual publish
   - [ ] Create git tag (but don't push yet):
     ```bash
     git tag -a v0.1.0 -m "Initial public release"
     ```

5. **Actual Publication** (0.5 days)
   - [ ] **POINT OF NO RETURN** - Double-check everything
   - [ ] Ensure you're on clean commit matching v0.1.0
   - [ ] Publish to crates.io:
     ```bash
     cargo publish
     ```
   - [ ] Verify publication succeeded:
     - Check https://crates.io/crates/noet-core
     - Verify version shows as 0.1.0
     - Check download badge updates
   - [ ] Push git tag:
     ```bash
     git push origin v0.1.0
     ```
   - [ ] Wait for docs.rs to build (~5-10 min)
   - [ ] Verify docs.rs:
     - Check https://docs.rs/noet-core
     - Verify all modules documented
     - Test search functionality

6. **GitHub Release** (0.5 days)
   - [ ] Create GitHub Release for v0.1.0:
     - Title: "v0.1.0 - Initial Release"
     - Tag: v0.1.0
     - Description: See template below
   - [ ] Include in release notes:
     - High-level overview
     - Key features
     - Installation instructions
     - Link to documentation
     - Link to examples
     - Known limitations
     - Acknowledgments
   - [ ] Attach assets (optional):
     - Source tarball
     - SHA256 checksums
   - [ ] Publish release

7. **Community Announcements** (1 day)
   - [ ] Submit to This Week in Rust:
     - URL: https://github.com/rust-lang/this-week-in-rust
     - Use TWiR template (see below)
     - Submit via GitHub PR
   - [ ] Post to /r/rust:
     - Title: "[Release] noet-core 0.1.0 - Hypergraph knowledge management with three-way sync"
     - Include overview, features, use cases
     - Link to docs and examples
     - Engage with comments
   - [ ] Post to Rust Users Forum:
     - Category: Announcements
     - Similar content to Reddit post
   - [ ] Consider posting to:
     - Hacker News (if appropriate)
     - Personal blog/social media
     - LinkedIn (professional network)
     - Twitter/X (tech community)
   - [ ] Update personal website/portfolio with project

8. **Post-Release Monitoring** (ongoing)
   - [ ] Set up notifications:
     - GitHub issues/PRs
     - Crates.io download stats
     - docs.rs build status
   - [ ] Monitor for first 48 hours:
     - GitHub issues
     - Reddit comments
     - Community feedback
   - [ ] Respond to questions promptly
   - [ ] Document common questions for FAQ
   - [ ] Create issues for feature requests
   - [ ] Plan patch releases for critical bugs

## Testing Requirements

- Package builds successfully with `cargo package`
- Extracted package builds and tests pass
- Documentation builds without warnings
- All examples run successfully
- Dry-run publish succeeds
- Git tag created and ready to push
- Release notes reviewed by others

## Success Criteria

- [ ] All dependencies are from crates.io or documented
- [ ] Package validates successfully
- [ ] Published to crates.io as v0.1.0
- [ ] Docs.rs build successful
- [ ] Git tag v0.1.0 pushed
- [ ] GitHub Release created
- [ ] Announced to Rust community (TWiR, Reddit)
- [ ] Initial feedback monitored and addressed
- [ ] Download/usage metrics being tracked

## Risks

**Risk**: Publication fails due to dependency issues  
**Mitigation**: Thorough dependency audit; dry-run catches most issues

**Risk**: Documentation doesn't build on docs.rs  
**Mitigation**: Test locally with docs.rs environment settings; check for platform-specific issues

**Risk**: Critical bug discovered immediately after publication  
**Mitigation**: Publish patch release quickly; yank broken version if necessary

**Risk**: Name already taken on crates.io  
**Mitigation**: Checked in advance - `noet-core` is available

**Risk**: Negative community reception  
**Mitigation**: Clear documentation, manage expectations (v0.1.0), respond professionally

## Templates

### GitHub Release Notes Template

```markdown
# noet-core v0.1.0 - Initial Release

**noet-core** is a hypergraph-based knowledge management library that maintains bidirectional synchronization between markdown documents and a queryable graph.

## Features

- 🔗 **Stable References**: BID injection for permanent cross-document links
- 📝 **Multi-format Support**: Markdown and TOML parsing with extensible codec system
- 🔄 **Three-way Sync**: File system ↔ Cache ↔ Database reconciliation
- 🔁 **Multi-pass Compilation**: Resolves forward references automatically
- 📊 **Graph Queries**: Traverse relationships between documents
- 🎯 **Type-safe**: Leverages Rust's type system for correctness
- ⚡ **Async-first**: Built on Tokio for efficient I/O
- 🔌 **Extensible**: Custom codecs for any document format

## Installation

```toml
[dependencies]
noet-core = "0.1.0"
```

## Documentation

- 📚 [API Documentation](https://docs.rs/noet-core)
- 🎓 [Examples](https://github.com/alyjak/noet-core/tree/main/examples)
- 🏗️ [Architecture Guide](https://github.com/alyjak/noet-core/blob/main/docs/architecture.md)

## Quick Example

```rust
use noet_core::{BeliefSet, codec::BeliefSetParser};

#[tokio::main]
async fn main() -> Result<()> {
    let mut belief_set = BeliefSet::new();
    let mut parser = BeliefSetParser::new(config, &mut belief_set).await?;
    
    parser.parse_all().await?;
    
    // Query the graph
    for node in belief_set.nodes() {
        println!("Node: {}", node.title());
    }
    
    Ok(())
}
```

## Status

This is the initial public release (v0.1.0). The API is subject to change before 1.0. Feedback and contributions welcome!

## Known Limitations

- Pre-1.0: Breaking changes may occur between minor versions
- WASM support is experimental
- HTML rendering in progress (see roadmap)

## Acknowledgments

Thanks to everyone who provided feedback during development!

## What's Next?

See our [roadmap](https://github.com/alyjak/noet-core/blob/main/docs/project/ROADMAP_HTML_RENDERING.md) for upcoming features.
```

### This Week in Rust Submission Template

```markdown
**noet-core** - A hypergraph-based knowledge management library that maintains bidirectional synchronization between markdown documents and a queryable graph. Features automatic BID injection for stable cross-document references, multi-pass compilation for forward references, and three-way reconciliation between file system, cache, and database. Great for building personal knowledge bases, documentation systems, or content management tools.

[Crates.io](https://crates.io/crates/noet-core) | [Docs](https://docs.rs/noet-core) | [GitHub](https://github.com/alyjak/noet-core)
```

### Reddit Post Template

```markdown
**Title**: [Release] noet-core 0.1.0 - Hypergraph knowledge management with three-way sync

Hi r/rust! I'm excited to share the first release of **noet-core**, a library I've been working on for knowledge management systems.

## What is it?

noet-core provides a hypergraph-based system that keeps markdown documents synchronized with a queryable graph database. Think of it as the plumbing for building tools like Obsidian, Roam Research, or custom documentation systems.

## Key Features

- **Stable References**: Automatically injects BIDs (Belief IDs) into documents for permanent cross-document links that survive renames
- **Multi-pass Compilation**: Handles forward references like a compiler, resolving them across multiple passes
- **Three-way Sync**: Reconciles changes between filesystem, in-memory cache, and database
- **Extensible Codecs**: Built-in Markdown and TOML support; easy to add custom formats
- **Async-first**: Built on Tokio for efficient I/O operations

## Example Use Cases

- Personal knowledge bases (Zettelkasten, digital gardens)
- Documentation systems with complex cross-references
- Content management with stable linking
- Graph-based note-taking applications

## Quick Example

[Include minimal code example showing basic usage]

## Links

- **Crates.io**: https://crates.io/crates/noet-core
- **Documentation**: https://docs.rs/noet-core
- **GitHub**: https://github.com/alyjak/noet-core
- **Examples**: [link to examples directory]

## Status

This is v0.1.0 - an early release. The API will evolve before 1.0, but the core concepts are solid. Feedback and contributions are very welcome!

Happy to answer questions about the design, implementation, or use cases!
```

## Open Questions

1. Should we do a soft launch (announce after initial feedback) or full launch immediately?
2. Which communities beyond TWiR and r/rust should we target?
3. Should we prepare a blog post for launch day?
4. Do we want to set up GitHub Sponsors or similar donation mechanism?
5. Should we yank 0.1.0 if critical bugs are found, or just publish 0.1.1 quickly?

## References

- **Cargo Book - Publishing**: https://doc.rust-lang.org/cargo/reference/publishing.html
- **Crates.io Policy**: https://crates.io/policies
- **This Week in Rust**: https://this-week-in-rust.org/
- **Reddit r/rust**: https://reddit.com/r/rust
- **Semantic Versioning**: https://semver.org/
- **Pattern**: How tokio, serde, and other major crates do releases
- **Related**: ISSUE_08 (repository setup) must complete first