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

**Status**: MOVED TO ISSUE 07 (Section 8)

**Context**: Service layer testing was backlogged but is now integrated into comprehensive testing for v0.1.0 release.

**See**: `docs/project/ISSUE_07_COMPREHENSIVE_TESTING.md` Section 8 for:
- WatchService API Testing
- FileUpdateSyncer Integration Tests
- File Watching Integration Tests
- Database Synchronization Tests
- Integration test expansion at `tests/service_integration.rs`

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

## PathMap Multi-Path Query Issue (from Issue 29)

**Priority**: MEDIUM - PathMap queries should work for all asset paths

**Context**: Issue 29 implemented static asset tracking with multi-path support (same content at multiple file paths gets same BID). The WEIGHT_DOC_PATHS relation correctly stores multiple paths, but PathMap queries via `asset_map().get(path)` fail to find the paths.

### Current Behavior
- Assets with same content correctly get same BID (content-addressed) ✓
- Multiple paths accumulate in WEIGHT_DOC_PATHS relation ✓
- Warning: "Setting 2 paths for single relation (expected 1)" appears
- PathMap construction creates separate entries for each path (lines 854-859 in `paths.rs`) ✓
- But `asset_map().get("assets/test.png")` returns `None` even when path exists ✗

### Investigation Needed
1. Verify PathMapMap is being rebuilt after asset events processed into global_bb
2. Check if asset_namespace node itself is in states (required for PathMap construction)
3. Verify relations with WEIGHT_DOC_PATHS are correctly indexed into PathMap
4. Test if issue is specific to asset_namespace or affects all multi-path relations
5. Consider if PathMap construction needs special handling for multi-path weights

### Workaround
`asset_manifest` (populated during compilation) provides reliable path→BID queries and is sufficient for Issue 29's requirements (HTML output hardlinking).

### Test Case
`tests/codec_test.rs::test_multi_path_asset_tracking` currently uses asset_manifest instead of PathMap queries. Update test to use PathMap queries once fixed.

**Status**: Discovered during Issue 29 implementation, deferred as non-blocking for asset tracking functionality.

## Should asset bids really be derived from their hash?

We could put this information into the asset node, that would trigger a node update, which
downstream consumers would be notified of. It would result in less document churn as well, because
we wouldn't need to regenerate reference "brefs" all over the place.

## BeliefBase Trait Abstraction for Zero-Copy Graph Operations

**Priority**: LOW - Code quality improvement

**Context**: `BeliefBase` has `states` (direct field) and `relations` (behind `Arc<RwLock<>>`). Currently, to call `BeliefGraph` methods on a `BeliefBase`, we clone via `From<&BeliefBase> for BeliefGraph`. This is wasteful for read-only operations like `find_orphaned_edges()`.

### Option 1: Direct Implementation (Current Workaround)
- Duplicate methods on both `BeliefBase` and `BeliefGraph`
- Simple but violates DRY principle
- Example: `find_orphaned_edges()` duplicated across both types

### Option 2: Trait-Based Abstraction (Recommended)
Define a trait that both types implement with default implementations:

```rust
pub trait HasBeliefData {
    fn get_states(&self) -> &BTreeMap<Bid, BeliefNode>;
    fn get_relations_graph(&self) -> impl Deref<Target = BidGraph>;
    
    // Default implementations for shared methods
    fn find_orphaned_edges(&self) -> Vec<Bid> { /* ... */ }
    fn is_empty(&self) -> bool { /* ... */ }
    fn build_balance_expr(&self) -> Option<Expression> { /* ... */ }
    // etc.
}

impl HasBeliefData for BeliefGraph { /* ... */ }
impl HasBeliefData for BeliefBase { /* ... */ }
```

**Benefits**:
- Zero-copy access to graph operations from BeliefBase
- No code duplication for read-only graph methods
- Single source of truth for shared algorithms
- Can be used in generic contexts: `fn analyze<T: HasBeliefData>(data: &T)`

**Considerations**:
- Requires Rust 1.75+ for `impl Trait` in trait return position
- Trait methods slightly less discoverable than direct methods
- Need to import trait to use default methods

**Alternative Considered**: `BeliefGraphRef<'a>` wrapper type with borrowed data - rejected as more complex with limited benefit over trait approach.

**Related**: Used in `built_in_test()` to check for orphaned edges without cloning entire graph.


## BeliefBase and Search Index Sharding (from Issue 48/50)

**Priority**: MEDIUM - Performance improvement for large repositories

**Context**: ISSUE_48 implements per-network search index sharding. ISSUE_50 extends this paradigm to BeliefBase JSON export/loading with unified ShardManager.

### Unified Sharding Architecture
- Per-network sharding for both BeliefBase JSON and search indices
- Unified ShardManager API for consistent memory management
- 200MB total budget: 50% BeliefBase + 50% search indices
- User selects which networks to load (controls memory footprint)
- Automatic sharding when total size exceeds 10MB threshold

### Implementation Status
- ISSUE_48 (Search MVP): Establishes per-network indexing architecture
- ISSUE_50 (BeliefBase Sharding): Extends pattern to beliefbase.json
- Both share ShardManager abstraction for consistency

### Future Enhancements
- Per-document sharding for very large networks (1000+ documents)
- Server-side shard streaming (fetch on-demand)
- IndexedDB caching for loaded shards
- Compression for shard JSON (gzip, brotli)
- Network dependency resolution (auto-load referenced networks)
- Shard preloading based on navigation patterns

**Related Files**:
- Export: `src/codec/compiler.rs::export_beliefbase_json()`
- Loading: `assets/viewer.js::initializeWasm()`
- Sharding: `src/shard/manager.rs` (to be implemented)

## Notes

- Items are extracted from completed issues in `docs/project/completed/`
- All items are optional enhancements, not blocking any current work
- Priority levels: HIGH (blocking), MEDIUM (useful), LOW (nice-to-have)
- Most completed issues had unchecked boxes that were implementation notes, not incomplete work
- This backlog can be revisited when planning future releases (v0.2, v1.0, etc.)
