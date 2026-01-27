# Backlog

This file tracks optional enhancements and future work extracted from completed issues.

## Documentation Enhancements (from Issue 05)

**Priority**: LOW - Optional improvements to existing documentation

### Architecture Deep Dive
- Extract and expand multi-pass compilation explanation from `lib.rs`
- Extract compilation model details from `beliefbase_architecture.md`
- Add beginner-friendly examples beyond existing doctests
- **Current Status**: Basic architecture documented, comprehensive details available in rustdoc

### Codec Implementation Tutorial
- Step-by-step guide: Build custom codec (JSON example)
- Document DocCodec trait integration
- Best practices for error handling
- **Current Status**: DocCodec trait documented in rustdoc, basic usage shown in examples

### BID Deep Dive Guide
- BID lifecycle (generation, injection, resolution)
- Why BIDs matter (forward refs, cross-doc links)
- Usage patterns and best practices
- **Current Status**: BIDs explained in architecture.md and rustdoc

### Additional Tutorial Content
- `querying.md` - Graph query patterns
- `custom_codec.md` - Full codec implementation walkthrough
- **Current Status**: Query examples in rustdoc, WatchService tutorial covers file watching and DB sync

### FAQ and Troubleshooting
- Common questions about multi-pass compilation
- Performance characteristics
- Troubleshooting guide for common issues
- **Current Status**: Basic usage covered in README and rustdoc

### Documentation Navigation Improvements
- Cross-link tutorial docs with headers/footers
- Comprehensive "Documentation" section in README
- **Current Status**: Rustdoc provides good navigation, manual docs reference rustdoc

## Service Testing Infrastructure (from Issue 10)

**Priority**: MEDIUM - Testing gaps in service layer (feature = "service")

**Context**: Core library is well-tested. Service layer (`watch.rs`) has comprehensive rustdoc examples but minimal integration tests.

### WatchService API Testing
- Update `WatchService` to implement library operations (vs product-specific ops)
- Document operation semantics in rustdoc
- **Current Status**: WatchService documented with 4 comprehensive doctests (240+ lines)

### FileUpdateSyncer Integration Tests
- Test: Initialize `FileUpdateSyncer` with temp directory
- Test: Modify file, verify compiler thread processes it
- Test: Verify `BeliefEvent`s flow to transaction thread
- Test: Verify database sync completes
- Test: Multiple file changes, verify all processed
- Test: Handle parse errors gracefully
- Test: Shutdown and cleanup (abort handles)
- Document threading model and synchronization points in module doc
- **Note**: Integration test skeleton created at `tests/service_integration.rs`

### File Watching Integration Tests
- Test: `enable_network_syncer()` sets up watcher
- Test: File modification triggers debouncer callback
- Test: Debouncer filters dot files correctly
- Test: Debouncer filters by codec extensions
- Test: Compiler queue gets updated on file change
- Test: `disable_network_syncer()` tears down cleanly
- Verify no race conditions between debouncer and compiler thread

### Database Synchronization Tests
- Test: `perform_transaction()` batches multiple events
- Test: Events update database correctly
- Test: Transaction errors are handled gracefully
- Test: Event channel backpressure (if applicable)
- Verify database state matches builder cache after sync
- Document transaction boundaries and consistency guarantees

## Anchor Injection Enhancement (from Issue 03)

**Priority**: LOW - Optional UX improvement

**Context**: Currently, IDs are only injected into headings when collisions occur (Bref-based IDs). Consider always injecting IDs for explicit anchors.

### Always-Inject-IDs Mode
- Inject calculated IDs into all headings (even without collisions)
- Makes it easier for users to reference sections explicitly
- Format: `# Title {#calculated-id}` or `# Title {#bref-value}` for collisions
- Use `update_or_insert_frontmatter()` pattern to inject anchor into heading events
- pulldown_cmark will serialize it correctly when generating source
- **Current Status**: IDs are only injected for collision resolution

## Network Configuration Features (from Issue 21)

**Priority**: LOW - Optional configuration system extensions

**Context**: Issue 21 implemented JSON/TOML dual-format support. Network configuration schema exists but these features are not yet implemented.

### Network-Level Format Preferences
- Parse network file and extract `config` object
- Store network config in `ProtoBeliefNode` for network nodes
- Pass network config down to child document parsing
- Respect `default_metadata_format` preference
- Implement `strict_format` validation (if enabled, reject non-default format)
- **Current Status**: JSON-first parsing works, TOML fallback works, but network config not yet propagated

### Format Preference API
- Add `from_str_with_format()` method for explicit format preference
- Update call sites to pass network config when available
- Support NetworkConfig fields: `default_metadata_format`, `strict_format`, `validate_on_parse`, `auto_normalize`

## Link Format Enhancements (from Issue 04)

**Priority**: LOW - Optional link validation and refactoring tools

**Context**: Issue 04 implemented canonical link format with Bref. These are potential CLI/tooling enhancements.

### Link Validation CLI
- Pre-deployment validation: `noet-core validate --check-links ./docs/`
- Report broken links with file locations
- Suggest fixes for common issues
- Distinguish between "file moved" vs "file deleted"

### Link Refactoring Tools
- Automated link updates when moving files: `noet-core refactor --move src/old.md src/new.md`
- Update all references automatically
- Preview changes before applying

### Import from Other Systems
- Convert existing link formats from other tools
- `noet-core import --from obsidian ./vault/`
- `noet-core import --from roam ./export/`
- `noet-core import --from logseq ./graphs/`

## Migration Guides (from Issue 14)

**Priority**: LOW - Documentation for users migrating from pre-1.0 versions

**Context**: Issue 14 renamed core types (`BeliefSet` → `BeliefBase`, etc.). Migration guide would help users update.

### Type Rename Migration Guide
- Document all renamed types and rationale
- Provide search-and-replace patterns
- Note breaking changes vs backward compatibility
- Consider type aliases for gradual migration
- **Current Status**: Renames complete, but no migration guide written

## Notes

- Items are extracted from completed issues in `docs/project/completed/`
- All items are optional enhancements, not blocking any current work
- Priority levels: HIGH (blocking), MEDIUM (useful), LOW (nice-to-have)
- Most completed issues had unchecked boxes that were implementation notes, not incomplete work
- This backlog can be revisited when planning future releases (v0.2, v1.0, etc.)