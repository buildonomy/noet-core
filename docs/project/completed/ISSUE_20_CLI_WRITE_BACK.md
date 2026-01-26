# Issue 20: CLI Write-Back Support

**Priority**: HIGH - Blocks CLI utility for actual document management  
**Estimated Effort**: 1-2 days  
**Dependencies**: Issue 10 (completed), Issue 19 (file watcher bug)  
**Context**: Part of v0.1.0 Phase 2 - enhancing CLI tool functionality

## Summary

Add `--write` flag to `noet parse` and `noet watch` subcommands to enable writing changes back to source files. Currently, both commands operate in read-only mode. Additionally, ensure graceful error handling when `watch` subcommand is used without the `service` feature flag (edge case protection).

## Goals

1. Add `--write/-w` flag to both `parse` and `watch` subcommands
2. Implement write-back logic for one-shot parsing (`parse` command)
3. Implement write-back logic for continuous watching (`watch` command)
4. Add runtime validation for `watch` command dependencies
5. Document write-back behavior in CLI help text and module docs
6. Default to safe read-only mode (opt-in for writes)

## Architecture

### Current Behavior

**`noet parse <path>`**:
- Parses documents once using `DocumentCompiler::simple()`
- Displays statistics to stdout
- Never writes back to disk
- Safe for inspection/validation workflows

**`noet watch <path>`**:
- Continuously watches for file changes
- Parses updated documents
- Displays events to stdout
- Never writes back to disk
- Requires `service` feature (currently enforced via `required-features = ["bin"]` in Cargo.toml)

### Target Behavior

**`noet parse <path> --write`**:
- Parse documents once
- Write normalized/updated content back to source files
- Use cases:
  - Normalize formatting across documents
  - Apply link updates after file moves
  - Update anchor references
  - Batch processing of document changes

**`noet watch <path> --write`**:
- Continuously watch for changes
- Parse and write back on each update
- Use cases:
  - Live formatting/normalization
  - Auto-update cross-references
  - Development workflow with auto-formatting

### Write-Back Implementation

**For `parse` command**:
1. After `compiler.parse_all(cache).await`, iterate over parsed documents
2. For each document with changes, use codec's `to_string()` method
3. Write serialized content back to original file path
4. Track and report write statistics (files written, errors)

**For `watch` command**:
1. In event handler thread, detect `BeliefEvent::NodeUpdated` events
2. Extract NodeKey and determine source file path
3. Serialize updated document using appropriate codec
4. Write atomically (temp file + rename pattern)
5. Avoid triggering recursive watch events (debouncer handles this)

### Feature Flag Protection

**Current situation**:
- Binary requires `bin` feature via `required-features = ["bin"]`
- `bin` feature depends on `service` feature
- Therefore, `watch` subcommand always has `WatchService` available
- Edge case: Manual build with `--no-default-features --features clap,ctrlc,tracing-subscriber`

**Protection strategy**:
1. Add compile-time check in `src/bin/noet.rs`:
   ```rust
   #[cfg(not(feature = "service"))]
   compile_error!("The 'watch' subcommand requires the 'service' feature");
   ```
2. Add runtime validation in `watch` subcommand handler:
   - Check that `WatchService` type is available
   - Provide helpful error message if somehow unavailable
   - Suggest rebuild with proper features

## Implementation Steps

1. **Add `--write` flag to CLI arguments** (0.5 days) ✅ COMPLETE
   - [x] Add `write: bool` field to both `Parse` and `Watch` commands
   - [x] Add `#[arg(short = 'w', long)]` attribute
   - [x] Update CLI help text describing write-back behavior
   - [x] Document safety implications (destructive operation)

2. **Implement write-back for `parse` command** (0.5 days) ✅ COMPLETE
   - [x] After `parse_all()`, check if `--write` flag is set
   - [x] Iterate over compiled documents in `DocumentCompiler`
   - [x] For each document, check if content changed (compare hash or dirty flag)
   - [x] Serialize changed documents using codec's `to_string()`
   - [x] Write atomically to disk (temp file + rename)
   - [x] Track and report statistics (N files written, M errors)
   - [x] Handle write errors gracefully (don't abort on single file failure)

3. **Implement write-back for `watch` command** (0.5 days) ✅ COMPLETE
   - [x] Modify event handler thread to check `--write` flag
   - [x] On `BeliefEvent::NodeUpdated`, extract source file path
   - [x] Serialize updated document content
   - [x] Write atomically to disk
   - [x] Log write operations (use tracing for verbosity control)
   - [x] Ensure debouncer doesn't trigger recursive watch events
   - [x] Handle write errors without crashing watch loop

4. **Add feature flag protection** (0.25 days) ✅ COMPLETE
   - [x] Add `compile_error!` for missing `service` feature
   - [x] Add runtime validation in `watch` command handler
   - [x] Test manual build without service feature (verify error message)
   - [x] Document feature requirements in binary's rustdoc

5. **Testing and validation** (0.25 days) ✅ COMPLETE
   - [x] Test `noet parse` with and without `--write`
   - [x] Test `noet watch` with and without `--write`
   - [x] Verify write-back updates files correctly
   - [x] Verify read-only mode doesn't write
   - [x] Test error handling (read-only files, permission errors)
   - [x] Test atomic write behavior (no partial writes on failure)

## Testing Requirements

### Unit Tests
- CLI argument parsing with `--write` flag
- Write-back logic in isolation (mock filesystem)
- Feature flag validation

### Integration Tests
- `noet parse <path> --write` updates files
- `noet parse <path>` (no flag) doesn't write
- `noet watch <path> --write` updates on file change
- `noet watch <path>` (no flag) doesn't write
- Error handling for write failures
- Atomic write behavior (crash during write)

### Manual Testing
- Parse multiple documents with `--write`
- Watch directory with live changes and `--write`
- Verify no recursive watch loops
- Test permission errors (read-only files)

## Success Criteria

1. ✅ Both `parse` and `watch` accept `--write/-w` flag
2. ✅ Default behavior is read-only (safe)
3. ✅ `--write` flag enables file updates
4. ✅ Write operations are atomic (no partial writes)
5. ✅ Write errors don't crash commands
6. ✅ Statistics reported for write operations
7. ✅ Feature flag protection prevents confusing errors
8. ✅ Documentation updated (CLI help, module docs)
9. ✅ All tests passing

## Risks

**Risk 1: Recursive watch events**
- Mitigation: Debouncer should filter self-triggered events
- Validation: Test with `--write` enabled, verify no infinite loops

**Risk 2: Data loss on write failure**
- Mitigation: Atomic writes (temp file + rename)
- Validation: Test crash scenarios, verify no corruption

**Risk 3: Performance with `watch --write`**
- Mitigation: Only write files that actually changed
- Validation: Monitor event volume, write frequency

**Risk 4: Race conditions in watch mode**
- Mitigation: File watcher debouncer, atomic writes
- Validation: Test rapid file changes, verify consistency

## Open Questions

1. **Should write-back include formatting normalization?**
   - Current behavior: Codecs may reformat during serialization
   - Decision needed: Preserve original formatting vs. normalize
   - Recommendation: Start with normalization, add `--preserve-format` later if needed

2. **What should happen with parse errors during write-back?**
   - Option A: Skip writing files with errors
   - Option B: Write partial results (dangerous)
   - Recommendation: Skip errored files, log warning

3. **Should write-back be behind additional confirmation?**
   - Consider: `--write --force` for safety
   - Or: Interactive confirmation for first write
   - Recommendation: Start simple (just `--write`), add safety later if needed

## Decision Log

**Decision 1: Leverage Existing Write Infrastructure**
- Date: 2025-01-26
- Context: `DocumentCompiler` already had a `write` field controlling write-back
- Decision: Modified CLI to pass write flag through to DocumentCompiler and WatchService
- Rationale: Reuses well-tested write logic in compiler, atomic writes already implemented
- Impact: Minimal code changes needed, consistent behavior across parse/watch commands

**Decision 2: WatchService Write Flag as Constructor Parameter**
- Date: 2025-01-26
- Decision: Added `write: bool` parameter to `WatchService::new()` and `FileUpdateSyncer::new()`
- Rationale: Write behavior should be determined at service creation, not per-operation
- Impact: All WatchService instantiations updated (tests, examples, doctests)
- Breaking change: Yes, but pre-v0.1.0 so acceptable

**Decision 3: Feature Flag Protection via Cargo.toml**
- Date: 2025-01-26
- Decision: Rely on `required-features = ["bin"]` in Cargo.toml instead of complex compile-time checks
- Rationale: Cargo's feature system already prevents building without required features
- Implementation: Added compile_error! and runtime checks as defense-in-depth
- Impact: Clean error messages at both compile-time and runtime

**Decision 4: Write Statistics in Parse Command**
- Date: 2025-01-26
- Decision: Show "Write Results" section only when --write is enabled
- Rationale: Reduces noise in read-only mode, makes write mode explicit
- Implementation: Conditional printing based on write flag
- Impact: Improved UX clarity about when files are modified

**Decision 5: Examples Use Write Mode**
- Date: 2025-01-26
- Decision: Set `write: true` in examples, `write: false` in tests
- Rationale: Examples demonstrate full functionality, tests need predictable read-only behavior
- Impact: Examples show realistic usage, tests are safer and more isolated

## References

- **Extends**: [Issue 10: Daemon Testing](./completed/ISSUE_10_DAEMON_TESTING.md) - CLI tool foundation
- **Blocks**: CLI utility for production use
- **Related**: ~~[Issue 19: File Watcher Bug](./ISSUE_19_FILE_WATCHER_TIMING_BUG.md)~~ - must be fixed (Deleted and consolidated into [Issue 07: Comprehensive Testing](./ISSUE_07_COMPREHENSIVE_TESTING.md)) for reliable watch testing
- **Code locations**:
  - `src/bin/noet.rs` - CLI entry point
  - `src/codec/compiler.rs` - `DocumentCompiler` interface
  - `src/codec/markdown.rs` - Markdown serialization
  - `src/watch.rs` - `WatchService` and event handling
- **Dependencies**:
  - `clap` v4.5 - CLI argument parsing (already present)
  - Standard library `fs` - atomic write operations
