# Issue 8: Repository Setup & Infrastructure

**Priority**: MEDIUM - Required before v0.1.0 publication  
**Estimated Effort**: 2-3 days  
**Dependencies**: Issues 5-7 complete (docs, quality, tests ready)  
**Context**: Part of [`ROADMAP_OPEN_SOURCE_NOET-CORE.md`](./ROADMAP_OPEN_SOURCE_NOET-CORE.md) - creates standalone repository infrastructure

## Summary

Create new standalone git repository for `noet-core`, set up CI/CD infrastructure, create issue/PR templates, configure repository settings, and establish community infrastructure. This issue transforms the workspace crate into a standalone open source project with proper automation and contributor support.

## Goals

1. Create new repository with appropriate structure
2. Set up CI/CD pipelines (tests, clippy, docs)
3. Create issue and PR templates
4. Add status badges to README
5. Configure repository settings and permissions
6. Create initial issues and labels
7. Set up documentation hosting

## Architecture

### Repository Structure

```
noet-core/                    (new repository root)
├── .github/                  (or .gitlab/ depending on platform)
│   ├── workflows/            
│   │   ├── ci.yml           # Run tests on push/PR
│   │   ├── clippy.yml       # Linting
│   │   ├── docs.yml         # Build and deploy docs
│   │   └── release.yml      # Publish to crates.io
│   ├── ISSUE_TEMPLATE/
│   │   ├── bug_report.yml
│   │   ├── feature_request.yml
│   │   └── question.yml
│   └── pull_request_template.md
├── benches/                  (from workspace)
├── docs/                     (from workspace)
│   ├── design/
│   ├── project/
│   ├── architecture.md
│   ├── codecs.md
│   └── ...
├── examples/                 (from workspace)
│   ├── basic_usage.rs
│   ├── file_watching.rs
│   └── querying.rs
├── src/                      (from workspace)
├── tests/                    (from workspace)
├── .gitignore
├── AGENTS.md                 (from workspace)
├── Cargo.toml               (updated from workspace)
├── CHANGELOG.md             (new)
├── CODE_OF_CONDUCT.md       (new)
├── CONTRIBUTING.md          (from workspace)
├── LICENSE-APACHE           (from workspace)
├── LICENSE-MIT              (from workspace)
├── README.md                (from workspace)
└── SECURITY.md              (new)
```

### CI/CD Strategy

**Platform Choice**: GitHub Actions (or GitLab CI)
- Most Rust projects use GitHub
- Excellent tooling and caching
- Free for public repos
- Integrates with crates.io

**Pipeline Goals**:
- Fast feedback (<5 min for basic checks)
- Test on multiple platforms (Linux, macOS, Windows)
- Cache dependencies for speed
- Fail fast on formatting/clippy errors

## Implementation Steps

1. **Repository Creation** (0.5 days)
   - [ ] Choose platform: GitHub or GitLab? (Recommend: GitHub)
   - [ ] Create repository: `github.com/alyjak/noet-core` (or similar)
   - [ ] Set repository description: "Hypergraph-based knowledge management library with three-way sync"
   - [ ] Add topics/tags: `rust`, `knowledge-management`, `hypergraph`, `markdown`, `graph-database`, `sync`
   - [ ] Set up branch protection for `main`:
     - Require PR reviews
     - Require CI to pass
     - No direct pushes to main
   - [ ] Configure default branch settings
   - [ ] Set license in GitHub settings (MIT OR Apache-2.0)

2. **File Migration** (0.5 days)
   - [ ] Copy directory structure from `rust_core/crates/core/`
   - [ ] Update `Cargo.toml`:
     ```toml
     [package]
     name = "noet-core"
     version = "0.1.0"  # No longer workspace inherited
     authors = ["Andrew Lyjak <email@example.com>"]
     edition = "2021"
     license = "MIT OR Apache-2.0"
     repository = "https://github.com/alyjak/noet-core"
     documentation = "https://docs.rs/noet-core"
     # ... rest of metadata
     
     [dependencies]
     # Extract from workspace Cargo.toml, use version numbers
     tokio = { version = "1.40", features = ["sync", "time", "rt", "macros"] }
     # ... etc
     ```
   - [ ] Update all `path` dependencies to version numbers
   - [ ] Remove workspace references
   - [ ] Update documentation URLs in README/CONTRIBUTING
   - [ ] Create `.gitignore` appropriate for Rust project
   - [ ] Initial commit and push

3. **CI/CD Pipeline Setup** (1 day)
   - [ ] Create `.github/workflows/ci.yml`:
     ```yaml
     name: CI
     
     on:
       push:
         branches: [main]
       pull_request:
     
     jobs:
       test:
         strategy:
           matrix:
             os: [ubuntu-latest, macos-latest, windows-latest]
             rust: [stable]
         runs-on: ${{ matrix.os }}
         steps:
           - uses: actions/checkout@v4
           - uses: dtolnay/rust-toolchain@stable
           - uses: Swatinem/rust-cache@v2
           - run: cargo test --all-features
           - run: cargo test --no-default-features
     ```
   - [ ] Create `.github/workflows/clippy.yml`:
     ```yaml
     name: Clippy
     
     on: [push, pull_request]
     
     jobs:
       clippy:
         runs-on: ubuntu-latest
         steps:
           - uses: actions/checkout@v4
           - uses: dtolnay/rust-toolchain@stable
             with:
               components: clippy
           - run: cargo clippy --all-features -- -D warnings
     ```
   - [ ] Create `.github/workflows/docs.yml`:
     ```yaml
     name: Documentation
     
     on:
       push:
         branches: [main]
     
     jobs:
       docs:
         runs-on: ubuntu-latest
         steps:
           - uses: actions/checkout@v4
           - uses: dtolnay/rust-toolchain@stable
           - run: cargo doc --no-deps --all-features
           - name: Deploy to GitHub Pages
             uses: peaceiris/actions-gh-pages@v3
             with:
               github_token: ${{ secrets.GITHUB_TOKEN }}
               publish_dir: ./target/doc
     ```
   - [ ] Test all pipelines run successfully
   - [ ] Set up cargo-deny for dependency auditing (optional)

4. **Issue & PR Templates** (0.5 days)
   - [ ] Create `.github/ISSUE_TEMPLATE/bug_report.yml`:
     - Steps to reproduce
     - Expected vs actual behavior
     - Environment (OS, Rust version)
     - Minimal example
   - [ ] Create `.github/ISSUE_TEMPLATE/feature_request.yml`:
     - Use case description
     - Proposed solution
     - Alternatives considered
   - [ ] Create `.github/ISSUE_TEMPLATE/question.yml`:
     - Quick help for users
     - Links to docs
   - [ ] Create `.github/pull_request_template.md`:
     - Description of changes
     - Motivation and context
     - Testing performed
     - Checklist (tests pass, docs updated, etc.)

5. **Repository Configuration** (0.5 days)
   - [ ] Add status badges to README.md:
     ```markdown
     [![CI](https://github.com/alyjak/noet-core/workflows/CI/badge.svg)](https://github.com/alyjak/noet-core/actions)
     [![Crates.io](https://img.shields.io/crates/v/noet-core.svg)](https://crates.io/crates/noet-core)
     [![Documentation](https://docs.rs/noet-core/badge.svg)](https://docs.rs/noet-core)
     [![License](https://img.shields.io/crates/l/noet-core.svg)](https://github.com/alyjak/noet-core#license)
     ```
   - [ ] Enable GitHub Discussions (or equivalent)
   - [ ] Configure GitHub Pages for docs:
     - Source: gh-pages branch
     - Domain: noet-core.buildonomy.org (optional)
   - [ ] Set up crates.io integration:
     - Add crates.io token to secrets
     - Test release workflow (without publishing)
   - [ ] Enable security advisories
   - [ ] Configure dependabot for dependency updates

6. **Initial Issues & Labels** (0.5 days)
   - [ ] Create label set:
     - `good-first-issue` - For new contributors
     - `help-wanted` - Community help desired
     - `bug` - Something's broken
     - `enhancement` - New feature
     - `documentation` - Docs improvements
     - `performance` - Speed/memory concerns
     - `question` - User questions
     - `wontfix` - Intentional design decision
   - [ ] Create initial issues from roadmap:
     - Issue: Improve parser ergonomics (convenience constructors)
     - Issue: Add more examples
     - Issue: Performance benchmarks
     - Issue: WASM support improvements
   - [ ] Tag issues with appropriate labels and difficulty
   - [ ] Create CHANGELOG.md with v0.1.0 entry

## Testing Requirements

- CI pipelines run successfully on all platforms
- Badge links work and display correct status
- Issue templates are clear and functional
- PR template checklist is actionable
- Documentation builds and deploys
- Test releases don't publish to crates.io

## Success Criteria

- [ ] Repository created with proper structure
- [ ] CI runs on push/PR (tests, clippy, docs)
- [ ] Tests pass on Linux, macOS, Windows
- [ ] Issue and PR templates functional
- [ ] Status badges display in README
- [ ] GitHub Discussions enabled
- [ ] Initial issues created with labels
- [ ] Documentation deployment working
- [ ] Repository settings configured (branch protection, etc.)
- [ ] CHANGELOG.md created

## Risks

**Risk**: CI costs exceed free tier limits  
**Mitigation**: GitHub Actions free tier is generous for public repos; monitor usage

**Risk**: Platform lock-in (GitHub specific)  
**Mitigation**: Keep CI config simple and portable; document alternatives

**Risk**: Too many notifications/issues overwhelm maintainer  
**Mitigation**: Set up notification filters; use issue triage process

**Risk**: CI matrix is too slow (many platform/feature combinations)  
**Mitigation**: Start simple, expand as needed; use caching aggressively

## Open Questions

1. GitHub or GitLab? (Recommend: GitHub for Rust community)
2. Should we set up GitHub Sponsors for donations?
3. Enable GitHub Discussions or direct users to Discord/Zulip?
4. Deploy docs to GitHub Pages or docs.rs only?
5. Use cargo-deny for dependency auditing in CI?

## References

- **GitHub Actions for Rust**: https://github.com/actions-rs
- **Rust CI Template**: https://github.com/dtolnay/rust-toolchain
- **Issue Template Examples**: https://github.com/rust-lang/rust/tree/master/.github/ISSUE_TEMPLATE
- **Pattern**: tokio, serde repository setup
- **Cargo Book - Publishing**: https://doc.rust-lang.org/cargo/reference/publishing.html
- **Related**: ISSUE_09 (crates.io publication) depends on this issue
