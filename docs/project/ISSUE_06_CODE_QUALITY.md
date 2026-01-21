# Issue 6: Code Quality & API Review

**Priority**: HIGH - Required for v0.1.0  
**Estimated Effort**: 3-4 days  
**Dependencies**: Issues 1-4 complete (stable API surface)  
**Context**: Part of [`ROADMAP_OPEN_SOURCE_NOET-CORE.md`](./ROADMAP_OPEN_SOURCE_NOET-CORE.md) - ensures code quality before open source release

## Summary

Conduct comprehensive code review and quality improvements before open source release. Review public API surface, improve error handling, add module documentation, ensure code follows Rust best practices, and verify no proprietary references remain. This issue ensures the codebase meets community standards and is ready for external contributors.

## Goals

1. Review and refine public API surface (`pub` items)
2. Improve error handling (reduce `unwrap()`, add context)
3. Add comprehensive module-level documentation
4. Clean up TODOs and FIXMEs
5. Remove proprietary/internal references
6. Apply Rust best practices (clippy, fmt)
7. Add license headers (optional)

## Architecture

### Code Quality Standards

**Public API Guidelines**:
- Every `pub` item must have doc comments
- Minimize `pub` exposure (principle of least privilege)
- Stable API: breaking changes post-1.0 require major version bump
- Ergonomic: common use cases should be straightforward

**Error Handling**:
- No `unwrap()` in public-facing code
- Use `?` operator with proper error context
- Custom error types with `thiserror`
- Error messages guide users toward solutions

**Documentation**:
- Module-level docs explain purpose and key concepts
- Function-level docs include examples for non-obvious usage
- Panic conditions documented explicitly
- Unsafe code justified and documented

## Implementation Steps

1. **Public API Audit** (1 day)
   - [ ] List all `pub` items: `rg "^pub " --type rust`
   - [ ] For each `pub` item, verify:
     - Is it necessary for library users?
     - Does it have comprehensive doc comments?
     - Is the API ergonomic?
     - Should it be `pub(crate)` instead?
   - [ ] Document public API surface in architecture.md
   - [ ] Consider API simplification opportunities:
     - `BeliefSetParser::new()` convenience constructor?
     - `BeliefSet` builder pattern?
     - Default trait implementations?

2. **Error Handling Review** (1 day)
   - [ ] Find all `unwrap()` calls: `rg "unwrap\(\)" --type rust`
   - [ ] Categorize by location:
     - Tests: OK to use `unwrap()`
     - Internal helpers: Replace with `expect()` + context
     - Public API: Replace with `?` and proper Result
   - [ ] Find all `expect()` calls: `rg "expect\(" --type rust`
   - [ ] Verify expect messages are descriptive
   - [ ] Review panic paths: `rg "panic!\(" --type rust`
   - [ ] Document intentional panics in doc comments
   - [ ] Add error context with `anyhow` or `thiserror::Context`

3. **Module Documentation** (0.5 days)
   - [ ] Add top-level module docs to `src/lib.rs`
   - [ ] Add module docs to major modules:
     - `codec/` - Parsing and document transformation
     - `beliefset/` - Graph data structure
     - `properties/` - Node and edge properties
     - `error/` - Error types and handling
   - [ ] Document feature flags in `lib.rs`
   - [ ] Add examples to module docs where helpful

4. **Code Cleanup** (0.5 days)
   - [ ] Find TODOs: `rg "TODO|FIXME" --type rust`
   - [ ] For each TODO:
     - Complete if trivial
     - Create GitHub issue if important
     - Remove if obsolete
   - [ ] Search for proprietary references:
     - `rg -i "buildonomy|internal|proprietary" --type rust`
     - Remove or generalize any found
   - [ ] Search for hardcoded paths/credentials:
     - `rg "home|Users|C:\\" --type rust`
   - [ ] Review logging for sensitive info exposure

5. **Rust Best Practices** (0.5 days)
   - [ ] Run clippy with strict settings:
     ```bash
     cargo clippy --all-features --all-targets -- -D warnings
     ```
   - [ ] Fix all clippy warnings
   - [ ] Run rustfmt:
     ```bash
     cargo fmt --all
     ```
   - [ ] Verify no formatting changes needed
   - [ ] Check for unused dependencies:
     ```bash
     cargo +nightly udeps --all-targets
     ```
   - [ ] Review unsafe code blocks (if any):
     - Are they necessary?
     - Are they documented?
     - Are they sound?

6. **License Headers** (0.5 days) - OPTIONAL
   - [ ] Decide: Add license headers to source files?
   - [ ] If yes, create template:
     ```rust
     // Copyright 2025 Andrew Lyjak
     //
     // Licensed under the Apache License, Version 2.0 or the MIT license,
     // at your option. This file may not be copied, modified, or distributed
     // except according to those terms.
     ```
   - [ ] Add to all `.rs` files in `src/`
   - [ ] Note: This is optional; many Rust projects skip headers

## Testing Requirements

- All code compiles without warnings: `cargo build --all-features`
- Clippy passes with `-D warnings`
- Rustfmt shows no changes needed
- Documentation builds without warnings: `cargo doc --no-deps`
- All public items have doc comments
- No TODOs remain in code (converted to issues)

## Success Criteria

- [ ] Public API surface documented and justified
- [ ] No `unwrap()` in public-facing code
- [ ] All modules have descriptive documentation
- [ ] TODOs converted to issues or removed
- [ ] No proprietary references in code
- [ ] Clippy passes with `-D warnings`
- [ ] Code is formatted with rustfmt
- [ ] Documentation builds cleanly
- [ ] Error messages are helpful and actionable

## Risks

**Risk**: Over-aggressive API minimization breaks existing usage  
**Mitigation**: Check examples and tests still work after changes; consider deprecation over removal

**Risk**: Clippy changes introduce bugs  
**Mitigation**: Run full test suite after each clippy fix; review changes carefully

**Risk**: Documentation review reveals API design flaws  
**Mitigation**: Document issues; defer major refactors to post-1.0 if necessary

**Risk**: License headers add noise without value  
**Mitigation**: Make headers optional; cargo.toml metadata is sufficient for most projects

## Open Questions

1. License headers: Add them or skip? (Most Rust projects skip)
2. Should we use `#![deny(missing_docs)]` to enforce documentation? (Too strict for 0.1.0?)
3. API simplification: Which convenience constructors are worth adding?
4. Deprecation policy: How to handle API changes pre-1.0?

## References

- **Rust API Guidelines**: https://rust-lang.github.io/api-guidelines/
- **Error Handling Book**: https://doc.rust-lang.org/book/ch09-00-error-handling.html
- **Clippy Lints**: https://rust-lang.github.io/rust-clippy/master/
- **Example**: `src/codec/parser.rs` - Review `BeliefSetParser::new()` signature for ergonomics
- **Example**: `src/beliefset/mod.rs` - Review public API surface
- **Pattern**: tokio, serde, anyhow for documentation quality benchmarks