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
- No way to force re-parse without deleting `.noet/` directory
- Hard to diagnose ("why isn't my change showing up?")

**Status**: Issue identified during Issue 34 manual testing. Cache stability now working, but revealed this blind spot.

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

**Current Workaround**: Delete `.noet/` cache directory to force full re-parse

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

**The Lazy Evaluation Pattern**:
```rust
pub async fn parse_all(&mut self, cache: impl BeliefSource) -> Result<(), BuildonomyError> {
    // Load everything from cache
    let cached_graph = cache.eval_unbalanced(&Expression::StateIn(StatePred::Any)).await?;
    self.builder.session_bb_mut().union_with(&cached_graph);
    
    // Only enqueue files with unresolved relations
    // NO: file modification time checking
    // NO: cache freshness validation
    // NO: forced re-parse option
    
    // Process queues...
}
```

### What's Missing

**File Modification Tracking**:
- Need to store `(path, mtime)` pairs in cache
- Need to check current filesystem mtime against cached mtime
- Need to invalidate cache entries for modified files

**Cache Metadata**:
- Cache has no concept of "when was this parsed?"
- No schema version tracking (for cache format changes)
- No network-level metadata (which files were in last parse?)

**Invalidation Strategy**:
- No API to mark cache entries as stale
- No way to force re-parse of specific files
- No way to do partial cache refresh

## Solution Design

### Option A: Mtime-Based Invalidation (Recommended)

**Store mtimes in cache**:
- Add `file_mtimes` table to SQLite schema
- Track `(path, mtime, last_parse_time)` for each file
- Check mtime on every `parse_all()` call

**Invalidation logic**:
```rust
pub async fn parse_all(&mut self, cache: impl BeliefSource) -> Result<(), BuildonomyError> {
    // 1. Load cache
    let cached_graph = cache.eval_unbalanced(&Expression::StateIn(StatePred::Any)).await?;
    
    // 2. Check file modifications
    let stale_files = self.check_file_modifications(&cache).await?;
    
    // 3. Invalidate stale cache entries
    for path in stale_files {
        self.invalidate_cache_entry(&path, &cached_graph);
        self.enqueue(path); // Force re-parse
    }
    
    // 4. Load remaining fresh cache
    self.builder.session_bb_mut().union_with(&cached_graph);
    
    // 5. Process queues...
}
```

**Pros**:
- Simple and reliable (filesystem is source of truth)
- Works across sessions and machines
- Standard practice (Make, Bazel, etc. all use mtime)

**Cons**:
- Additional I/O to stat all files
- Doesn't catch content-identical re-writes (rare edge case)
- Requires schema change

### Option B: Content Hashing

**Hash file content**:
- Store `(path, content_hash)` in cache
- Compare hash on load
- More robust than mtime

**Pros**:
- Detects actual content changes only
- Immune to mtime manipulation

**Cons**:
- Must read entire file to hash (expensive)
- Much slower than stat
- Overkill for this use case

### Option C: Cache Generation Counter

**Track cache "generation"**:
- Each `parse_all()` increments counter
- Cache entries tagged with generation
- Invalidate entries from old generations

**Pros**:
- Simple to implement
- No filesystem I/O

**Cons**:
- Doesn't detect actual file changes
- Forces re-parse on every new session (defeats cache purpose)
- Not suitable for multi-session workflows

### Recommended: Option A (Mtime-Based)

Most pragmatic solution:
- Standard practice in build systems
- Fast (just stat calls)
- Reliable
- Easy to debug ("why did it re-parse?" → "mtime changed")

## Implementation Plan

### Phase 1: Add Mtime Schema (1 day)

**File**: `src/db.rs`

Add `file_mtimes` table:
```sql
CREATE TABLE IF NOT EXISTS file_mtimes (
    path TEXT PRIMARY KEY,
    mtime INTEGER NOT NULL,        -- Unix timestamp
    last_parse_time INTEGER NOT NULL,
    network_bid TEXT NOT NULL,      -- Which network owns this file
    FOREIGN KEY(network_bid) REFERENCES states(bid)
);
```

Add mtime tracking to `Transaction`:
```rust
impl Transaction {
    pub fn track_file_mtime(&mut self, path: &Path, network_bid: Bid) -> Result<()> {
        let metadata = fs::metadata(path)?;
        let mtime = metadata.modified()?
            .duration_since(SystemTime::UNIX_EPOCH)?
            .as_secs() as i64;
        
        self.mtime_updates.insert(path.to_path_buf(), (mtime, network_bid));
        Ok(())
    }
}
```

**Success**: Schema migration runs, mtime table created

### Phase 2: Track Mtimes During Parse (1 day)

**File**: `src/codec/compiler.rs`

Update `parse_next()` to track mtimes:
```rust
pub async fn parse_next(&mut self, cache: &impl BeliefSource) -> Result<Option<ParseResult>, BuildonomyError> {
    let path = self.primary_queue.pop_front()?;
    
    // Parse file...
    let result = parse_file(&path)?;
    
    // NEW: Track file mtime
    if let Some(tx) = self.current_transaction() {
        tx.track_file_mtime(&path, network_bid)?;
    }
    
    // Emit events...
    Ok(Some(result))
}
```

**Success**: Mtimes stored in cache for all parsed files

### Phase 3: Check Mtimes on Load (1 day)

**File**: `src/codec/compiler.rs`

Add mtime checking to `parse_all()`:
```rust
async fn check_stale_files(&self, cache: &impl BeliefSource) -> Result<Vec<PathBuf>, BuildonomyError> {
    // Query all cached mtimes
    let cached_mtimes = cache.get_file_mtimes().await?;
    
    let mut stale_files = Vec::new();
    
    for (path, cached_mtime) in cached_mtimes {
        // Check current filesystem mtime
        if let Ok(metadata) = fs::metadata(&path) {
            let current_mtime = metadata.modified()?
                .duration_since(SystemTime::UNIX_EPOCH)?
                .as_secs() as i64;
            
            if current_mtime > cached_mtime {
                tracing::info!("File modified: {} (cached: {}, current: {})", 
                    path.display(), cached_mtime, current_mtime);
                stale_files.push(path);
            }
        } else {
            // File deleted since cache - invalidate
            tracing::warn!("Cached file no longer exists: {}", path.display());
            stale_files.push(path);
        }
    }
    
    Ok(stale_files)
}

pub async fn parse_all(&mut self, cache: impl BeliefSource) -> Result<(), BuildonomyError> {
    // Check for stale files BEFORE loading cache
    let stale_files = self.check_stale_files(&cache).await?;
    
    if !stale_files.is_empty() {
        tracing::info!("Found {} modified files, will re-parse", stale_files.len());
        for path in stale_files {
            self.enqueue(path);
        }
    }
    
    // Load cache (excluding stale entries)
    let cached_graph = cache.eval_unbalanced(&Expression::StateIn(StatePred::Any)).await?;
    self.builder.session_bb_mut().union_with(&cached_graph);
    
    // Process queues...
}
```

**Success**: Modified files detected and re-parsed

### Phase 4: Add `--force` Flag (4 hours)

**File**: `src/bin/noet.rs`

Add CLI flag:
```rust
#[arg(long, help = "Force re-parse all files, ignoring cache")]
force: bool,
```

**File**: `src/codec/compiler.rs`

Honor flag:
```rust
pub async fn parse_all(&mut self, cache: impl BeliefSource, force: bool) -> Result<(), BuildonomyError> {
    if force {
        tracing::info!("Force re-parse enabled, ignoring cache");
        // Enqueue all files in network
        let all_files = self.discover_network_files()?;
        for path in all_files {
            self.enqueue(path);
        }
        return self.process_queues().await;
    }
    
    // Normal flow with mtime checking...
}
```

**Success**: `noet watch --force` re-parses everything

### Phase 5: Cache Invalidation API (4 hours)

**File**: `src/db.rs`

Add methods to invalidate cache:
```rust
impl DbConnection {
    /// Remove all cache entries for a specific file
    pub async fn invalidate_file(&self, path: &Path) -> Result<()> {
        // Delete file mtime entry
        sqlx::query("DELETE FROM file_mtimes WHERE path = ?")
            .bind(path.to_str())
            .execute(&self.0)
            .await?;
        
        // Note: Don't delete belief nodes - they may be referenced by other files
        // Instead, they'll be updated when file is re-parsed
        Ok(())
    }
    
    /// Remove all cache entries for a network
    pub async fn invalidate_network(&self, network_bid: Bid) -> Result<()> {
        sqlx::query("DELETE FROM file_mtimes WHERE network_bid = ?")
            .bind(network_bid.to_string())
            .execute(&self.0)
            .await?;
        Ok(())
    }
}
```

**Success**: API available for cache management

### Phase 6: Testing (1 day)

**Unit Tests**:
- `test_mtime_tracking` - Verify mtimes stored correctly
- `test_stale_detection` - Verify modified files detected
- `test_force_reparse` - Verify `--force` flag works

**Integration Tests**:
- `test_cache_invalidation_workflow`:
  1. Parse files, populate cache
  2. Modify file with `touch` (change mtime)
  3. Parse again, verify file re-parsed
  4. Verify unchanged files not re-parsed

**Manual Tests**:
- Edit file between sessions, verify change reflected
- Use `--force`, verify all files re-parsed
- Delete file, verify graceful handling

**Success**: All tests pass

## Testing Requirements

### Unit Tests

**src/db.rs**:
- `test_mtime_schema_migration` - Schema creates successfully
- `test_track_file_mtime` - Mtimes stored in transaction
- `test_invalidate_file` - Cache invalidation works

**src/codec/compiler.rs**:
- `test_check_stale_files` - Modified files detected
- `test_force_reparse` - Force flag bypasses cache

### Integration Tests

**tests/cache_invalidation_test.rs** (new):
- `test_mtime_invalidation_workflow` - Full scenario
- `test_deleted_file_handling` - Graceful deleted file handling
- `test_force_flag` - All files re-parsed with `--force`

## Success Criteria

- [ ] File mtimes stored in SQLite cache during parse
- [ ] Modified files detected on `parse_all()` and re-parsed
- [ ] Unchanged files not re-parsed (lazy evaluation preserved)
- [ ] `--force` flag forces full re-parse
- [ ] Deleted files handled gracefully (no crashes)
- [ ] Clear logging shows which files being re-parsed and why
- [ ] Performance impact minimal (stat calls are fast)
- [ ] Integration tests validate full workflow

## Open Questions

### Q1: What about files discovered during parse?

**Context**: Some files discovered via references (e.g., `![[other.md]]`)

**Options**:
- A) Track all files, discovered or direct
- B) Only track directly parsed files
- C) Track both, distinguish in schema

**Recommendation**: Option A - track all parsed files uniformly

### Q2: How to handle clock skew?

**Context**: Mtime can be in future or past due to clock issues

**Options**:
- A) Treat future mtimes as "modified" (safe, may over-parse)
- B) Ignore mtimes newer than current time (risky, may miss changes)
- C) Log warning, use current time

**Recommendation**: Option A - safe default, log warning

### Q3: Cache format versioning?

**Context**: Schema changes require cache rebuild

**Options**:
- A) Add schema version to DB, auto-invalidate on mismatch
- B) Manual cache deletion when schema changes
- C) Migration scripts for each version

**Recommendation**: Option A for v0.1, Option C for v1.0

### Q4: What about network file discovery?

**Context**: Network config file lists document roots

**Question**: Should network file changes trigger full invalidation?

**Recommendation**: Yes - if `.noet.toml` mtime changes, invalidate entire network

## Implementation Estimate

- Phase 1: Add mtime schema (1 day)
- Phase 2: Track mtimes during parse (1 day)
- Phase 3: Check mtimes on load (1 day)
- Phase 4: Add `--force` flag (4 hours)
- Phase 5: Cache invalidation API (4 hours)
- Phase 6: Testing (1 day)

**Total**: 3-4 days

**Note**: Assumes Issue 34 (cache stability) is complete. Mtime tracking only makes sense when cache is reliable.

## Related Issues

- **Issue 34**: DbConnection vs BeliefBase Equivalence - MUST be complete first
- **Issue 26**: Git-Aware Networks - Could use git timestamps instead of mtimes
- **Issue 31**: Watch Service Asset Integration - Watch service could trigger invalidation

## References

### Implementation Files
- `src/db.rs` - SQLite schema and mtime tracking
- `src/codec/compiler.rs` - Mtime checking and invalidation logic
- `src/bin/noet.rs` - CLI flag handling

### Design Patterns
- Make build system - mtime-based dependency tracking
- Cargo build cache - file hashing and invalidation
- Git index - mtime optimization for dirty file detection