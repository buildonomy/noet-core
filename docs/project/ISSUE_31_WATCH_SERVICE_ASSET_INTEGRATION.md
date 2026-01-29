# Issue 31: Watch Service Integration for Assets and URLs

**Priority**: MEDIUM
**Estimated Effort**: 2-3 days
**Dependencies**: Issue 29 (Static Asset Tracking), Issue 30 (External URL Tracking)
**Blocks**: None

## Summary

Integrate static asset and external URL tracking with the file watcher service, enabling automatic document reparse when assets change or URLs need refetching. This completes the live development workflow for non-document resources.

## Goals

1. **Watch asset files** referenced by documents for changes
2. **Detect asset modifications** and trigger document reparse (BID changes due to content hash)
3. **Propagate asset changes** through the belief graph (update all referencing documents)
4. **Watch asset manifests** for changes (discover new assets, remove deleted ones)
5. **Periodic URL refetching** (optional) to detect external content changes
6. **Integrate with dev server** for live reload on asset/URL changes

## Architecture

### Overview: Manifest-Aware File Watching

The watch service must understand the compiler's `asset_manifests` and `url_manifests` data structures to properly track non-document resources:

```
┌─────────────────────────────────────────────────────────────┐
│ DocumentCompiler                                            │
│   - asset_manifests: HashMap<Bid, AssetManifest>           │
│   - url_manifests: HashMap<Bid, UrlManifest>               │
│   - Writes manifests to disk as peer files                 │
└──────────────────┬──────────────────────────────────────────┘
                   │
                   ↓ manifest files
┌─────────────────────────────────────────────────────────────┐
│ FileUpdateSyncer (watch.rs)                                │
│   - Watches source files (already implemented)             │
│   - NEW: Watches .asset-manifest.toml files                │
│   - NEW: Watches asset files listed in manifests           │
│   - NEW: Periodic URL refetch task (optional)              │
└──────────────────┬──────────────────────────────────────────┘
                   │
                   ↓ on change
┌─────────────────────────────────────────────────────────────┐
│ Change Detection & Reparse Triggering                      │
│                                                             │
│ Asset Modified:                                             │
│   1. Detect file change via watcher                        │
│   2. Load relevant asset manifests                         │
│   3. Find documents that reference this asset              │
│   4. Recompute content hash (BID will change)              │
│   5. Enqueue referencing documents for reparse             │
│   6. Reparse → markdown source rewritten with new bref     │
│                                                             │
│ URL Refetch:                                                │
│   1. Periodic task fetches URL content                     │
│   2. Compare new hash with manifest entry                  │
│   3. If changed, enqueue referencing documents             │
│   4. Reparse → BID updated in markdown source              │
└─────────────────────────────────────────────────────────────┘
```

### Manifest-Driven Asset Discovery

Instead of watching all files recursively, watch service reads manifests:

```rust
pub struct AssetWatcher {
    /// Maps asset absolute paths → set of document BIDs that reference them
    asset_to_docs: HashMap<PathBuf, HashSet<Bid>>,
    
    /// Maps document BIDs → their asset manifests (cached)
    doc_manifests: HashMap<Bid, AssetManifest>,
    
    /// Debouncer for filesystem events
    debouncer: Debouncer<RecommendedWatcher>,
}

impl AssetWatcher {
    /// Load manifests from disk and build asset → document mapping
    pub async fn load_manifests(&mut self, network_paths: &[PathBuf]) -> Result<()> {
        for network_path in network_paths {
            // Find all .asset-manifest.* files in network
            let manifests = find_manifests_in_network(network_path)?;
            
            for manifest_path in manifests {
                let manifest: AssetManifest = read_manifest(&manifest_path)?;
                let doc_bid = manifest.network_bid;
                
                // Build reverse mapping: asset path → documents
                for asset in &manifest.assets {
                    self.asset_to_docs
                        .entry(asset.absolute_path.clone())
                        .or_default()
                        .insert(doc_bid);
                }
                
                // Add asset paths to watcher
                for asset in &manifest.assets {
                    self.debouncer.watcher().watch(
                        &asset.absolute_path,
                        RecursiveMode::NonRecursive
                    )?;
                }
                
                self.doc_manifests.insert(doc_bid, manifest);
            }
        }
        Ok(())
    }
}
```

### Integration with Existing FileUpdateSyncer

Extend `watch.rs::FileUpdateSyncer`:

```rust
pub struct FileUpdateSyncer {
    // ... existing fields ...
    
    /// Asset watcher (new)
    asset_watcher: Option<AssetWatcher>,
    
    /// URL refetch interval (None = disabled)
    url_refetch_interval: Option<Duration>,
}

impl FileUpdateSyncer {
    pub fn with_asset_watching(mut self, enabled: bool) -> Self {
        if enabled {
            self.asset_watcher = Some(AssetWatcher::new());
        }
        self
    }
    
    pub fn with_url_refetch(mut self, interval: Duration) -> Self {
        self.url_refetch_interval = Some(interval);
        self
    }
}
```

### Asset Change Flow

When asset file modified:

1. **Watcher fires event**: `path = /repo/docs/images/arch.png`
2. **Look up referencing docs**: `asset_to_docs.get(path)` → `{bid:doc1, bid:doc2}`
3. **For each referencing doc**:
   - Recompute asset content hash
   - New hash → new BID (content-addressed)
   - Enqueue doc for reparse: `compiler.on_file_modified(doc_path)`
4. **Reparse document**:
   - Parser encounters `![img](./images/arch.png)`
   - Resolves to new asset BID (hash changed)
   - Writes new `bref` attribute to markdown source
   - Updates `.asset-manifest.toml` with new BID
5. **Update watcher mappings**:
   - Old asset BID → removed from belief graph
   - New asset BID → inserted
   - `asset_to_docs` updated with new mappings

### URL Refetch Strategy

Optional periodic task:

```rust
pub async fn run_url_refetch_task(
    url_manifests: Vec<UrlManifest>,
    compiler: &mut DocumentCompiler,
    interval: Duration,
) {
    loop {
        tokio::time::sleep(interval).await;
        
        for manifest in &url_manifests {
            for url_entry in &manifest.urls {
                // Skip broken URLs or recently fetched
                if url_entry.status_code != 200 {
                    continue;
                }
                
                // Refetch
                let new_result = compiler.url_fetcher
                    .fetch_and_hash(&url_entry.url)
                    .await?;
                
                // Compare hashes
                if let Some(new_hash) = new_result.content_hash {
                    if new_hash != url_entry.content_hash {
                        // Content changed! Enqueue doc for reparse
                        let doc_path = compiler.cache()
                            .get_path_for_bid(manifest.network_bid)?;
                        compiler.on_file_modified(doc_path);
                    }
                }
            }
        }
    }
}
```

## Implementation Steps

### 1. Create AssetWatcher Module (1 day)

Create `src/codec/asset_watcher.rs`:
- `AssetWatcher` struct with manifest loading
- `load_manifests()` to discover assets and build mappings
- `on_asset_changed()` handler to find referencing docs
- `update_mappings()` when manifests change

### 2. Extend FileUpdateSyncer (0.5 days)

Modify `src/watch.rs`:
- Add `asset_watcher` field
- Initialize with network paths on startup
- Call `asset_watcher.on_asset_changed()` when asset events fire
- Watch `.asset-manifest.toml` files for additions/removals

### 3. Integrate with DocumentCompiler (0.5 days)

Add hook in `compiler.rs`:
- `pub fn on_asset_modified(&mut self, asset_path: PathBuf)` method
- Query manifests to find referencing docs
- Enqueue docs for reparse
- Return list of affected document paths

### 4. Manifest Change Detection (0.5 days)

Watch manifest files themselves:
- Detect when `.asset-manifest.toml` modified
- Reload manifest and update watcher
- Add newly referenced assets to watch list
- Remove deleted assets from watch list

### 5. URL Refetch Task (0.5 days)

Create optional periodic task:
- Spawn background tokio task
- Load all `.url-manifest.toml` files
- Refetch URLs on interval
- Compare content hashes, trigger reparse if changed

### 6. Dev Server Integration (0.5 days)

Extend dev server (Issue 6) to trigger live reload:
- Asset changed → HTML regenerated → browser reload
- URL refetched (content changed) → HTML regenerated → reload
- Show notification in browser: "Asset updated" or "External link changed"

## Testing Requirements

### Unit Tests
- `test_asset_to_docs_mapping()` - Verify reverse index built correctly
- `test_manifest_loading()` - Parse manifests and extract asset paths
- `test_asset_change_detection()` - Mock file event, verify docs enqueued
- `test_manifest_reload_on_change()` - Manifest modified → watcher updated

### Integration Tests
- `test_asset_modification_triggers_reparse()` - Modify image → doc reparsed
- `test_url_refetch_detects_change()` - Mock HTTP server, change content
- `test_watch_limit_handling()` - Many assets, verify graceful degradation
- `test_dev_server_live_reload()` - Asset change → browser reload

### Manual Testing
1. Start dev server with asset watching enabled
2. Modify image file → verify document HTML regenerated
3. Check browser live reload works
4. Enable URL refetch → modify mock server response → verify detection
5. Test with >100 assets (watch limit stress test)

## Success Criteria

- [ ] Asset file changes trigger document reparse
- [ ] Manifests loaded and watched for changes
- [ ] URL refetch detects external content changes (if enabled)
- [ ] Dev server live reload works for asset changes
- [ ] Watch limits handled gracefully (prioritize docs over assets)
- [ ] Performance acceptable with 100+ assets
- [ ] All tests passing

## Risks

### Risk 1: Watch Limit Exhaustion
**Impact**: HIGH  
**Likelihood**: MEDIUM  
**Mitigation**: 
- Prioritize document watching over asset watching
- Log warning when approaching limit (use `sysinfo` crate to check)
- Document workarounds: increase `fs.inotify.max_user_watches` on Linux
- Consider polling fallback for assets if watch limit hit

### Risk 2: Reparse Cascades
**Impact**: MEDIUM  
**Likelihood**: MEDIUM  
**Mitigation**:
- Asset used by 10 docs → all 10 reparsed (potentially slow)
- Use debouncing to batch asset changes (already in `notify-debouncer-full`)
- Add rate limiting to prevent reparse storms
- Log warnings for heavily-referenced assets

### Risk 3: Stale Manifest Cache
**Impact**: LOW  
**Likelihood**: LOW  
**Mitigation**:
- Reload manifest after document reparse
- Watch manifest files themselves for changes
- Clear cache on compiler reset

### Risk 4: URL Refetch Network Load
**Impact**: MEDIUM  
**Likelihood**: LOW (user-controlled)  
**Mitigation**:
- Default: disabled (no network requests)
- User must explicitly enable with interval choice
- Respect rate limits (add backoff for 429 responses)
- Skip broken URLs (don't retry 404s repeatedly)

## Open Questions

### Q1: Watch priority when limits hit?
**Options**:
- A) Drop asset watching, keep document watching
- B) Hybrid: watch heavily-referenced assets only
- C) Polling fallback for assets

**Recommendation**: A for simplicity, B for future enhancement.

### Q2: URL refetch interval?
**Options**:
- Hourly (aggressive, detects changes quickly)
- Daily (reasonable for documentation links)
- On-demand only (`noet refetch-urls` command)

**Recommendation**: Start with on-demand only, add interval in future.

### Q3: Handle asset deletion?
**Scenario**: Asset file deleted but still referenced in markdown.

**Options**:
- A) Warning diagnostic, continue compilation
- B) Reparse doc → unresolved reference
- C) Remove asset from manifest, keep node as "missing"

**Recommendation**: B - same as missing asset handling in Issue 29.

## References

### Related Issues
- **Issue 29**: Static Asset Tracking (provides manifests to watch)
- **Issue 30**: External URL Tracking (URL refetch integration)
- **Issue 6**: Dev Server (live reload integration point)

### Architecture References
- `watch.rs:FileUpdateSyncer` - Existing file watching infrastructure
- `compiler.rs:on_file_modified()` - Document reparse trigger
- Asset manifests (Issue 29) - Source of asset paths to watch
- URL manifests (Issue 30) - Source of URLs to refetch

### Future Enhancements
- **Selective Asset Watching**: Only watch frequently-changing assets
- **Smart URL Refetch**: Use HTTP `ETag` / `Last-Modified` headers for efficiency
- **Asset Change Notifications**: Desktop notifications for large repos
- **Watch Limit Auto-Increase**: Automatically adjust system limits (requires sudo)

## Notes

**Manifest-Driven Discovery**: Don't watch all files recursively. Only watch assets discovered via manifests. This avoids watching unrelated files (node_modules, .git, etc.).

**BID Changes Trigger Rewrites**: When asset content changes, its BID changes (content-addressed). This causes markdown source to be rewritten with new `bref` attribute. Watch service doesn't need to understand belief graph - compiler handles BID updates.

**Graceful Degradation**: If watch limit hit, continue compilation without watching. Log warning, provide documentation on increasing limits. Don't fail hard.

**Network Privacy**: URL refetch is opt-in, disabled by default. Respects privacy principles from Issue 30.

**Integration Pattern**: This issue completes the live development workflow:
1. User edits markdown → document reparsed
2. User modifies asset → referencing docs reparsed
3. External URL content changes → referencing docs reparsed (if enabled)
4. Dev server detects HTML changes → browser reload

All changes flow through the same reparse mechanism, maintaining consistency.