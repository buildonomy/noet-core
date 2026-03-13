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
- Store network config in `IRNode` for network nodes
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


## BeliefBase Sharding and Built-In Search (from Issues 50/54)

**Priority**: MEDIUM - Performance improvement for large repositories

**Context**: Issue 50 implements per-network BeliefBase sharding (JSON export/loading with memory budget) and always generates compile-time `search/*.idx.json` files alongside the data export. Issue 54 adds full-text search by deserializing those pre-built indices in `BeliefBaseWasm` — no Tantivy, no runtime index construction, no WASM binary size increase. Search covers the entire corpus from init, including networks whose data shards haven't been loaded. See `docs/design/search_and_sharding.md` for the architecture. See `docs/project/ISSUE_49_FULL_TEXT_SEARCH_PRODUCTION.md` for post-MVP search enhancement ideas (stemming, boolean queries, phrase search, ranking boosts).

### Implementation Status
- Issue 50 (BeliefBase Sharding): Per-network export, `ShardManager`, memory budget, `search/*.idx.json` generation
- Issue 54 (Search MVP): Compile-time per-network search indices, TF-IDF ranking, fuzzy matching, viewer search UI

### Future Enhancements
- Per-document sharding for very large networks (1000+ documents)
- IndexedDB caching for loaded shards
- Compression for shard JSON (gzip, brotli)
- Network dependency resolution (auto-load referenced networks)
- Shard preloading based on navigation patterns
- Federated shard access: remote `BeliefSource` for data not loaded locally (see `federated_belief_network.md` §3.6)

**Related Files**:
- Export: `src/codec/compiler.rs::export_beliefbase_json()` and `finalize_html` (search index generation)
- Search index output: `search/manifest.json`, `search/{bref}.idx.json` (generated at compile time)
- Loading: `assets/viewer/wasm.js::initializeWasm()`, `assets/viewer/shard-manager.js`
- Search query: `src/wasm.rs::BeliefBaseWasm` (deserializes `.idx.json`, runs TF-IDF queries)
- Sharding: `src/shard/` (to be implemented in Issue 50)

## Windows WatchService mtime Tracking Failure

**Priority**: MEDIUM - CI reliability on Windows

**Context**: `cache_invalidation_test.rs` tests (`test_mtime_tracking`, `test_stale_file_detection_and_reparse`, `test_multiple_files_mtime_tracking`, `test_unchanged_files_keep_same_mtime`, `test_deleted_file_handling`) consistently fail on `windows-latest` CI with the `service` feature. The symptom is that `test.md` is never tracked — only `index.md` appears in the DB mtime table after initial parse.

### Observed Symptom
```
test.md should have mtime tracked.
Found mtimes: {
  "C:\\\\Users\\RUNNER~1\\AppData\\Local\\Temp\\.tmpXXX\\test_network\\index.md": ...,
  "C:\\\\Users\\runneradmin\\AppData\\Local\\Temp\\.tmpXXX\\test_network\\index.md": ...
}
```
Two issues visible: (1) `test.md` never emits `FileParsed`, (2) `index.md` is stored twice under both the 8.3 short name (`RUNNER~1`) and the full name (`runneradmin`).

### Suspected Root Causes
1. **`WatchService` initial parse ordering**: On Windows, the filesystem watcher may not deliver events for files added before the watcher starts, or event delivery is racy. `test.md` is created before `WatchService::enable_network_syncer` is called, so the initial scan may not reliably pick it up.
2. **Windows 8.3 short names**: `os_path_to_string` is called with short-name paths in some cases (e.g. from the watcher callback), producing duplicate DB entries that fail lookup. Fix: canonicalize in `Transaction::track_file_mtime` via `fs::canonicalize(path).unwrap_or(path)` before `os_path_to_string`.

### Investigation Steps
1. Add tracing to `WatchService::enable_network_syncer` initial scan to confirm whether `test.md` is being enqueued for parse on Windows.
2. Check `FileParsed` event emission path — does the compiler emit it for all files or only modified ones on restart?
3. Apply `fs::canonicalize` in `track_file_mtime` (`src/db.rs`) and verify it resolves the duplicate-entry symptom.
4. Consider adding a `std::thread::sleep` or explicit flush barrier in the test to rule out timing issues.

### Related
- Prior fix: commit `1a7f3fb` ("Fix mtime resolution on windows") addressed separator handling in `os_path_to_string` but did not resolve the underlying parse-ordering issue.
- `src/db.rs` `Transaction::track_file_mtime`
- `src/codec/compiler.rs` `DocumentCompiler::parse_next` (emits `FileParsed`)
- `tests/cache_invalidation_test.rs`

**Status**: Pre-existing, non-blocking for Linux/macOS. Needs Windows-native debugging.

## `check_for_link_and_push` Bail-Out Refactor

**Priority**: LOW - Code quality improvement

**Context**: `src/codec/md.rs::check_for_link_and_push` has three separate code paths that emit an unmodified link and continue the event loop: "Can't parse", "path mismatch", and potentially future bail-out cases. All three duplicate the same ~15 lines of link-event reconstruction.

### Current Duplication
Each bail-out path manually reconstructs the original `Start(Link/Image)`, title events, and `End` event from `link_data`, then sets `link_type`, pushes to `events_out`, and calls `continue`. This is error-prone — a future change to link event structure must be applied in multiple places.

### Proposed Fix
Extract a helper:

```rust
fn emit_unchanged_link(
    link_data: LinkAccumulator,
    end_range: Option<Range<usize>>,
    events_out: &mut VecDeque<(MdEvent<'static>, Option<Range<usize>>)>,
)
```

All three bail-out paths call this helper, then `continue` the loop.

### Related
- `src/codec/md.rs` `check_for_link_and_push` — "Can't parse" branch (~L380) and "path mismatch" branch (~L480)
- Introduced during cross-platform path normalization fixes (session adding `strip_ext`/`drop_index_suffix`)

**Status**: Low risk, purely mechanical refactor. No behaviour change intended.

## Site-Root-Relative Slug Resolution via Slug Namespace

**Priority**: MEDIUM - Improves cross-reference fidelity for common corpus types

**Context**: Many static site generators (MDN, Jekyll, Hugo, Docusaurus, Sphinx) use
absolute-path cross-references relative to a site root rather than the filesystem root
(e.g. MDN's `/en-US/docs/Web/JavaScript/Reference/Global_Objects/Iterator`). These are
distinguishable from external `http://` hrefs by the signature `is_absolute() &&
!has_schema()` on `AnchorPath`. Currently `regularize_unchecked` correctly classifies
these as `href_namespace` externals (fix landed in Issue 34 session), but they could be
resolved to real in-graph nodes if the document registers its slug in a dedicated
`slug_namespace` PathMap.

### Design

**Slug registration**: When a document's frontmatter contains a `slug:` field (or
equivalent site-root-relative path declaration), the codec emits an additional
`RelationUpdate` placing the document's BID into the `slug_namespace` PathMap under the
slug string. Same BID as the file-derived node — the slug is just an alternate path alias.

**Slug lookup**: In `regularize_unchecked`, paths matching `is_absolute() &&
!has_schema()` are returned as `NodeKey::Path { net: slug_namespace().bref(), path }` (as
they are today for `href_namespace`, but using `slug_namespace` instead). `cache_fetch`
checks the `slug_namespace` PathMap after the normal filesystem PathMap miss. On a hit,
returns `GetOrCreateResult::Resolved` with the real node — full graph edge established, no
`External` node created. On a miss, falls back to `href_namespace` external as today.

**Parse ordering**: If document B's slug is not yet registered when document A references
it, `cache_fetch` misses and returns `UnresolvedReference` as normal. B is parsed later,
registers its slug. A is re-queued via the standard reparse loop and resolves on the
second pass. No special handling needed — the existing multi-pass convergence loop covers
this.

**Corpus scoping**: If the referenced slug is outside the parsed corpus subtree (e.g.
running against `javascript/` when the slug points to `web/css/`), the slug PathMap will
never contain it and the reference correctly degrades to `href_namespace` external. No
prefix-stripping or site-root configuration required — the slug PathMap either has the
entry or it doesn't.

### Implementation Sketch

1. Add `slug_namespace()` Bid in `properties.rs` (parallel to `href_namespace()`).
2. In `regularize_unchecked`: `is_absolute() && !has_schema()` → return
   `NodeKey::Path { net: slug_namespace().bref(), path }` instead of `href_namespace`.
3. In `cache_fetch`: after filesystem PathMap miss, check slug PathMap; on hit return
   resolved node.
4. In `md.rs` (or a generic frontmatter hook): when `slug:` key present in frontmatter,
   emit `RelationUpdate` adding the slug path to `slug_namespace` PathMap for this node's
   BID.
5. `push_relation` fallback: if slug lookup misses, re-classify as `href_namespace`
   external (existing behaviour).

### Notes
- No "site root URL prefix" configuration needed — the PathMap match is purely by slug
  string against registered slugs.
- Works across any corpus that registers slugs, not just MDN.
- Running against a full corpus root (e.g. `files/en-us/`) vs a subtree
  (`files/en-us/web/javascript/`) naturally determines how many slugs resolve vs degrade
  to externals — no code change required for either mode.

**Status**: Design complete. Blocked on nothing. Low implementation risk.

## Flattened Subnet Cache for `resolve_net_path` (from Issue 57)

**Priority**: LOW
**Context**: `DbConnection::resolve_net_path` currently resolves cross-network paths by
recursing one SQL hop per path segment (e.g. `"subnet/file.md"` → look up `"subnet"` under
`net`, then look up `"file.md"` under the returned sub-net BID). This is correct and
consistent by construction, but does O(depth) queries.

### Proposed Optimization

Maintain a flattened `subnets` table:

```sql
CREATE TABLE subnets (net TEXT, subnet_path TEXT, subnet_bid TEXT)
```

- `net`: the root network BID this row belongs to
- `subnet_path`: the full path from `net` to the sub-network (e.g. `"a/b"` for a net
  nested two levels deep)
- `subnet_bid`: the BID of the sub-network node at that path

**Read path**: `resolve_net_path(net, path)` does a single
`SELECT * FROM subnets WHERE net = ?`, processes all rows in Rust, finds the
longest `subnet_path` that is a prefix of `path`, then resolves the remainder
against `subnet_bid`. One SQL query regardless of nesting depth.

**Write path**: On a `NodeUpdate` event, if `node.kind.is_network()`, insert
flattened ancestry rows. For a new net `N` at path `p` under parent `P`:
  1. Insert `(net=P, subnet_path=p, subnet_bid=N)`.
  2. Find all existing rows where `subnet_bid = P` (i.e. `P` is itself a sub-net of
     some ancestor `A` at path `q`), and insert `(net=A, subnet_path=q/p, subnet_bid=N)`.
  This is a SELECT + batch INSERT, not recursive SQL.

On `NodeRemoved`, delete all rows where `subnet_bid = N` and all rows where
`subnet_path` starts with the removed path prefix (cascading descendants).

**Consistency**: updates should be in the same DB transaction as the path event
write, so there is no consistency window.

### When to Implement

Profile first. Typical repo subnet depth is 2-4 levels; the current recursive
approach does 2-4 queries and is unlikely to be a bottleneck. Implement this
only if `resolve_net_path` shows up in profiling for large repos with deep or
wide subnet hierarchies.

## Notes

- Items are extracted from completed issues in `docs/project/completed/`
- All items are optional enhancements, not blocking any current work
- Priority levels: HIGH (blocking), MEDIUM (useful), LOW (nice-to-have)
- Most completed issues had unchecked boxes that were implementation notes, not incomplete work
- This backlog can be revisited when planning future releases (v0.2, v1.0, etc.)
