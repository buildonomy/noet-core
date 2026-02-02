# Issue 35: Cache Invalidation and File Modification Tracking

**Priority**: HIGH
**Estimated Effort**: 3-4 days
**Dependencies**: Issue 34 (cache stability must work first)
**Blocks**: Multi-session workflows, production use without `--watch`

## Summary

The compiler lacks file modification time tracking, causing it to serve stale content when files are modified between `noet` invocations. When loading from SQLite cache, the compiler assumes cached data is authoritative and only re-parses when it detects unresolved relations. This creates a blind spot where file changes go undetected.

**Core Issue**: No mechanism to detect when source files have been modified since cache was populated.

**Impact**: 
- Modified files not re-parsed in subsequent sessions
- Stale content served to users
- `--write` flag has no effect if cache says "everything resolved"
- No way to force re-parse without deleting database (`belief_cache.db` in CWD by default)
- Hard to diagnose ("why isn't my change showing up?")

**Status**: ✅ **COMPLETE** - All phases implemented and integration tests passing.

## Evidence

**Scenario**:
1. Session 1: `noet watch` (no `--write`) → parses files, populates cache
2. User edits `README.md` between sessions
3. Session 2: `noet watch` → loads from cache, sees everything resolved → **doesn't re-parse `README.md`**
4. Result: Stale `README.md` content served, user's edits not reflected

**Why This Happens**:
- `DocumentCompiler::parse_all()` loads from cache into `session_bb`
- Compiler logic "runs" off tracking unresolved relations (`pending_dependencies`)
- If cache has all relations resolved, `primary_queue` stays empty
- No file modification checks trigger re-parsing
- `--write` flag irrelevant because nothing gets queued for parsing

**Current Workaround**: Delete `belief_cache.db*` sqlite file to force full re-parse

## Goals

1. **Detect file modifications**: Track file mtimes and compare against cache
2. **Invalidate stale cache entries**: Re-parse files modified since last cache
3. **Preserve lazy evaluation**: Don't re-parse unchanged files
4. **Support `--force` flag**: Allow explicit full re-parse when needed
5. **Clear diagnostics**: Warn users when serving potentially stale content

## Root Cause Analysis

### Compiler Assumptions

**Current behavior** (`src/codec/compiler.rs`):
1. `parse_all(cache)` loads entire cache into `session_bb`
2. `enqueue()` only called for files with unresolved relations
3. No file modification time checking anywhere
4. Assumes cache is always fresh


### What's Missing

**File Modification Tracking**:
- Need to store `(path, mtime)` pairs in cache
- Need to check current filesystem mtime against cached mtime
- Need to re-run a file parse if the filesystem mtime is greater than the cached mtime

**Cache Metadata**:
- Cache has no concept of "when was this parsed?"
- schema version tracking can be inferred from the version information in the cached API node.

**Invalidation Strategy**:
- No need to explicitly invalidate cache entries
- Query `file_mtimes` table to get previously parsed files
- Compare filesystem mtimes with cached mtimes
- Enqueue stale files for re-parsing
- Builder's `terminate_stack()` → `BeliefBase::compute_diff()` automatically reconciles changes

## Implemented Solution

### Mtime-Based Invalidation (IMPLEMENTED)

**Implementation approach**:
- Added `file_mtimes` table to SQLite schema: `(path TEXT PRIMARY KEY, mtime INTEGER NOT NULL)`
- Track repo-relative paths only (no network bid needed)
- Use `BeliefEvent::FileParsed(PathBuf)` to trigger mtime tracking after successful parse
- Query `file_mtimes` table directly to get cached mtimes (no need to load entire belief graph)
- Compare filesystem mtimes with cached mtimes to detect stale files
- Enqueue stale files to `primary_queue` for re-parsing
- Builder's `terminate_stack()` → `BeliefBase::compute_diff()` automatically reconciles changes

**Key design decisions**:
- **No cache loading**: Don't load entire belief graph into memory - just query mtime table
- **Event-driven tracking**: `FileParsed` event flows through Transaction for batch insertion
- **Simple comparison**: Query cached mtimes, compare with `fs::metadata()`, enqueue if stale
- **Efficient**: Only stat files that have been parsed before (from mtime table)

**Benefits**:
- ✅ Simple and reliable (filesystem is source of truth)
- ✅ Works across sessions and machines
- ✅ Standard practice (Make, Bazel, etc. all use mtime)
- ✅ Fast (just stat calls on known files)
- ✅ No unnecessary memory usage (doesn't load entire cache)

## Implementation Plan

### Phase 1: Add Mtime Schema ✅ COMPLETE

**File**: `src/db.rs`

**Implemented**:
- Added `file_mtimes` table to initial migration: `CREATE TABLE file_mtimes (path TEXT PRIMARY KEY, mtime INTEGER NOT NULL)`
- Added `mtime_updates: BTreeMap<PathBuf, i64>` field to `Transaction` struct
- Implemented `Transaction::track_file_mtime(&mut self, path: &Path)` to capture mtime
- Modified `Transaction::execute()` to batch insert mtimes: `INSERT OR REPLACE INTO file_mtimes (path, mtime)`
- Added `DbConnection::get_file_mtimes()` to query: `SELECT path, mtime FROM file_mtimes`
- Added `BeliefSource::get_file_mtimes()` trait method with default empty implementation

**Rationale**: Simplified schema with just `(path, mtime)`:
- Path is repo-relative (consistent with existing path handling)
- No network_bid needed (path lookup determines network membership)
- No last_parse_time needed (mtime comparison is sufficient)

### Phase 2: Add Network Path Querying ✅ COMPLETE

**Files**: `src/query.rs`, `src/beliefbase.rs`, `src/db.rs`

**Implemented**:
- Added `StatePred::NetPathIn(Bid)` variant for querying all paths in a network
- BeliefBase implementation uses `pm.all_net_paths()` for sub-network traversal
- DbConnection implementation: `SELECT path, target FROM paths WHERE net = ?`
- Added `BeliefSource::get_network_paths(network_bid)` trait method
  - Returns `Vec<(String, Bid)>` path-bid pairs
  - BeliefBase impl traverses sub-networks using `all_net_paths()`
  - DbConnection impl queries paths table directly
  - Default stub returns empty Vec
- Used for asset manifest querying (replaces incorrect session_bb assumption)

### Phase 3: Track Mtimes During Parse ✅ COMPLETE

**Files**: `src/event.rs`, `src/codec/compiler.rs`, `src/codec/builder.rs`, `src/db.rs`, `src/beliefbase.rs`

**Implemented**:
- Added `BeliefEvent::FileParsed(PathBuf)` variant to event enum
- Compiler emits `FileParsed(file_path)` after successful parse (line ~676 in parse_next)
- `Transaction::add_event()` handles `FileParsed` by calling `track_file_mtime(path)`
- Added `FileParsed` to all event match statements (Display, PartialEq, origin, with_origin)
- Builder and BeliefBase treat `FileParsed` as metadata-only (no graph changes)
- Watch service's `perform_transaction` processes events and batch inserts mtimes

**Flow**:
1. Compiler parses file successfully
2. Emits `FileParsed(path)` event to channel
3. Watch service receives event in transaction loop
4. Calls `transaction.add_event(&FileParsed(path))`
5. Transaction captures mtime in `mtime_updates` map
6. `transaction.execute()` batch inserts all mtimes to database

### Phase 4: Check Mtimes on Load ✅ COMPLETE

**File**: `src/codec/compiler.rs`

**Implemented**:
- Added `check_stale_files(&self, cache: &B, force: bool)` method
- **Key design**: Query `file_mtimes` table directly - don't load entire belief graph
- Gets cached mtimes: `cache.get_file_mtimes().await?`
- Filters to document paths (no anchors): `!path.to_string_lossy().contains('#')`
- For each cached path:
  - `fs::metadata(path)` to get current mtime
  - Compare: `current_mtime > cached_mtime` → stale
  - If file deleted → find parent network with `ProtoBeliefNode::from_file()`
  - Clock skew detection: warn on suspicious mtimes
- If `force=true`: all cached paths treated as stale
- Updated `parse_all(cache, force)` signature to accept force parameter
- Enqueues stale files before normal parsing flow
- Sorts and deduplicates stale file list

**Important**: Does NOT load cache into session_bb - only queries mtime table for efficiency

### Phase 5: Add `--force` Flag ✅ COMPLETE

**File**: `src/bin/noet.rs`

**Implemented**:
- Added `--force` flag to Parse command: `#[arg(long, help = "Force re-parse all files, ignoring cache")]`
- Added to command destructuring pattern
- Threaded through to compiler: `compiler.parse_all(cache, force).await?`
- When `force=true`, `check_stale_files` treats all cached files as stale
- Logs: "Force re-parse enabled, will re-parse N files"

**Usage**: `noet parse <path> --force`

### Phase 6: Testing ✅ COMPLETE

**File**: `tests/cache_invalidation_test.rs` (new)

**Implemented Integration Tests** (all passing):
1. `test_mtime_tracking` - Verifies mtimes tracked in database after WatchService parse
2. `test_stale_file_detection_and_reparse` - Modifies file, verifies reparse and mtime update
3. `test_multiple_files_mtime_tracking` - Verifies multiple files tracked correctly
4. `test_deleted_file_handling` - Verifies deleted files handled gracefully
5. `test_unchanged_files_keep_same_mtime` - Verifies unchanged files keep same mtime

**Test approach**:
- Uses full WatchService setup (not just DocumentCompiler)
- Creates temp directory with BeliefNetwork.toml
- Enables network syncer, waits for parsing
- Queries database directly to verify mtimes
- Tests file modification, deletion, and force scenarios
- All tests use `--test-threads=1` to avoid database conflicts

**Status**: All 5 integration tests passing (35s runtime)

### Bonus: Asset Manifest Architecture Refactor ✅ COMPLETE

**Issue**: Asset manifest was duplicated in compiler memory, creating maintenance burden and timing issues

**Solution - Removed asset_manifest from DocumentCompiler**:
- Removed `asset_manifest: Arc<RwLock<BTreeMap>>` field from DocumentCompiler
- Removed `asset_manifest()` public accessor method
- Asset verification now queries `session_bb` for assets discovered in current parse session
- Cache invalidation via `check_stale_files()` handles cached assets through mtime tracking

**DbConnection BeliefSource Implementation**:
- Implemented `get_file_mtimes()` in `BeliefSource` trait for `DbConnection`
- Added warning to default `get_file_mtimes()` implementation for debugging

**Asset FileParsed Events**:
- Assets now emit `FileParsed` events (in addition to documents)
- Enables mtime tracking in database for asset cache invalidation
- Event emitted after successful asset processing in compiler

**WatchService Integration**:
- Queries cache for asset manifest after parsing completes
- Creates asset hardlinks explicitly using `create_asset_hardlinks()`
- Generates network indices before hardlinks

**CLI Parse Command**:
- Now does complete HTML generation workflow
- Generates network indices
- Queries `session_bb` for asset manifest
- Creates asset hardlinks
- Reports asset count in output

**Test Improvements**:
- Tests build asset manifests from `PathAdded` events (not querying BeliefBase)
- Event-based approach avoids timing issues with path indexing
- Filter out empty paths (network node itself) from manifests
- Filter out `FileParsed` events when checking for graph modifications

**Architectural Benefits**:
- Cache is single source of truth for assets
- No duplicated state in compiler
- Clear separation: compiler parses, caller generates HTML
- `parse_all()` is pure parsing operation (no side effects)
- HTML generation explicit in calling code

## Testing Status

### Integration Tests (PASSING)

**tests/cache_invalidation_test.rs**:
- ✅ `test_mtime_tracking` - Mtimes tracked after parse
- ✅ `test_stale_file_detection_and_reparse` - Stale files detected and re-parsed
- ✅ `test_multiple_files_mtime_tracking` - Multiple files tracked
- ✅ `test_deleted_file_handling` - Deleted files handled gracefully
- ✅ `test_unchanged_files_keep_same_mtime` - Unchanged files not re-parsed

### Unit Tests (PASSING)

**All codec tests passing**: 28/28
- Tests updated to build asset manifests from events
- Tests using DbConnection for mtime-based invalidation
- FileParsed events filtered from graph modification checks

**All lib tests passing**: 133/133

## Success Criteria

- ✅ File mtimes stored in SQLite cache during parse (via FileParsed event for documents AND assets)
- ✅ Modified files detected on `parse_all()` and re-parsed (check_stale_files)
- ✅ Unchanged files not re-parsed (only cached files checked, mtime comparison)
- ✅ `--force` flag forces full re-parse (treats all cached files as stale)
- ✅ Deleted files handled gracefully (parent network re-parsed)
- ✅ Clear logging shows which files being re-parsed and why
- ✅ Performance impact minimal (only stat cached files, no full cache load)
- ✅ Integration tests validate full workflow (5 cache invalidation tests + 28 codec tests passing)
- ✅ Asset manifest removed from compiler - cache is single source of truth
- ✅ DbConnection implements get_file_mtimes() in BeliefSource trait
- ✅ HTML generation workflow complete in both Parse command and WatchService
- ✅ Tests use event-based manifest building (robust, no timing issues)

## Design Decisions

### File Discovery Scope
**Decision**: Track all parsed files uniformly (both direct and discovered via references like `![[other.md]]`)
- All files go through `parse_next()` → all get mtime tracking
- Simplifies implementation and reasoning

### Clock Skew Handling
**Decision**: Treat future mtimes as "modified" (safe, may over-parse)
- Log warning for suspicious mtimes
- Better to over-parse than miss changes
- Edge case in practice

### Cache Format Versioning
**Decision**: Manual cache deletion for schema changes in v0.1
- Delete test caches and rebuild (we're only users)
- Add schema version checking in v1.0+ if needed
- Schema version can be inferred from cached API node version

### Network Configuration Changes
**Decision**: Yes - if `.noet.toml` mtime changes, entire network should be re-scanned
- Network config file is a document in the network
- Will naturally trigger re-parse via mtime tracking
- Network re-scan discovers any added/removed document roots

## Implementation Summary

**Actual Time**: ~1 session with AI assistance

All phases completed:
- ✅ Phase 1: Mtime schema and Transaction tracking
- ✅ Phase 2: Network path querying (NetPathIn + get_network_paths)
- ✅ Phase 3: FileParsed event emission
- ✅ Phase 4: Efficient stale file detection (query mtimes, don't load cache)
- ✅ Phase 5: --force CLI flag
- ✅ Phase 6: Integration tests with WatchService

**Key architectural decisions**:
1. Event-driven: `FileParsed` event triggers mtime tracking for both documents and assets
2. No cache loading: Query mtime table directly for efficiency
3. Simple comparison: filesystem mtime vs cached mtime
4. Asset manifest removal: Cache is single source of truth, no duplication in compiler
5. Explicit HTML generation: Caller controls complete workflow (indices + assets)
6. Test robustness: Event-based manifest building avoids BeliefBase timing issues

**Complete**: All tests passing (33 integration + 133 lib), full functionality implemented

## Related Issues

- **Issue 34**: DbConnection vs BeliefBase Equivalence - MUST be complete first
- **Issue 26**: Git-Aware Networks - Could use git timestamps instead of mtimes
- **Issue 31**: Watch Service Asset Integration - Watch service could trigger invalidation

## References

### Implementation Files
- `src/db.rs` - SQLite schema, mtime tracking, BeliefSource::get_file_mtimes() implementation
- `src/codec/compiler.rs` - Mtime checking, invalidation logic, asset_manifest removed, FileParsed for assets
- `src/query.rs` - BeliefSource trait with get_file_mtimes() default + warning
- `src/watch.rs` - Asset manifest query from cache, explicit HTML generation workflow
- `src/bin/noet/main.rs` - Parse command with complete HTML generation (indices + assets)
- `tests/codec_test.rs` - Event-based asset manifest building, DbConnection for mtime tests

### Design Patterns
- Make build system - mtime-based dependency tracking
- Cargo build cache - file hashing and invalidation
- Git index - mtime optimization for dirty file detection
