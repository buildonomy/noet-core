# Issue 29: Static Asset Tracking and Management (Core)

**Priority**: HIGH
**Estimated Effort**: 2-3 days
**Dependencies**: Issue 6 (HTML Generation - Phase 1.5), Issue 33 (WEIGHT_DOC_PATHS refactor)
**Blocks**: Issue 30 (External URL Tracking), Issue 31 (Watch Service Asset Integration)

## Summary

Track static assets (images, PDFs, media) as first-class BeliefNodes with stable BIDs and content hash in payload, enabling automatic hardlink creation during HTML export with content-addressed paths, usage tracking, and bidirectional queryability. Assets remain at their source locations in the repository while relations in the belief graph track their usage.

## Goals

1. **Generate stable BIDs** for static assets using `Bid::now_v6()` (time-based UUID)
2. **Store content hash in payload** enabling content change detection without BID changes
3. **Track assets as BeliefNodes** in the belief graph (same pattern as external hrefs)
4. **Create Section relations** from asset nodes to `asset_namespace()` with repo-relative paths
5. **Query asset manifests** from BeliefBase (derived state, not stored in files)
6. **Create content-addressed hardlinks** in HTML output directory (`static/{hash}.{ext}`)
7. **Enable bidirectional queries**: "What uses this asset?" and "What assets does this document use?"
8. **Achieve deduplication** at HTML output layer (same hash → one physical file)
9. **Stable document references** - asset content changes emit NodeUpdate, not NodeRenamed

## Architecture

### Overview: Assets as External BeliefNodes

Static assets follow the same architectural pattern as external hrefs (see `properties.rs:href_namespace()`):

```
Repository (Source of Truth)
├── docs/
│   ├── design/
│   │   ├── architecture.md          (references ./images/arch.png)
│   │   ├── architecture.toml        (BeliefNode metadata)
│   │   └── images/
│   │       └── arch.png             (stays at original location)
│   ├── research/
│   │   ├── paper.md                 (references ../docs/whitepaper.pdf)
│   │   └── whitepaper.pdf
│   └── BeliefNetwork.toml           (the doc folder is registered as a BeliefNetwork)

HTML Output (Hardlinks Preserve Paths)
├── docs/                            (asset manifest isn't relevant)
│   ├── design/
│   │   ├── architecture.html
│   │   └── images/
│   │       └── arch.png             (hardlink → source or static/)
│   ├── research/
│   │   ├── paper.html
│   │   └── whitepaper.pdf           (hardlink → source or static/)
│   └── index.html                   (the BeliefNetwork is transformed to an index.html)
└── static/
    ├── sha256-abc123.png            (canonical copy if deduped)
    └── sha256-def456.pdf
```

**Key Design Principles:**

1. **Repository**: Assets stay at original locations, tracked by BeliefBase asset_namespace network via the `paths.rs` module.
2. **Stable BIDs**: Assets get stable v6 UUIDs (time-based), not content-hash. Documents reference stable BIDs that never change.
3. **Content Hash in Payload**: Asset nodes store SHA256 hash in payload. Content changes → NodeUpdate event (same BID, new payload).
4. **HTML Output**: Content-addressed hardlinks (`static/{hash}.{ext}`) enable automatic deduplication at output layer.
5. **No Document Rewrites**: Asset content changes don't require markdown source updates (BIDs stable).

### Asset BID Generation

**Stable BIDs with Hash in Payload** (NEW ARCHITECTURE):

```rust
/// Generate stable BID for static asset (time-based v6 UUID)
/// Content hash stored in node payload, not BID
/// This enables stable document references while tracking content changes
pub fn buildonomy_asset_bid() -> Bid {
    Bid::now_v6() // Time-based UUID in UUID_NAMESPACE_ASSET
}
```

**Why Stable BIDs?**
- Documents reference assets by stable BID (via bref) that never changes
- Asset content changes emit NodeUpdate (same BID, updated payload)
- No document rewrites needed when assets change (stable references)
- Deduplication happens at HTML output layer (content-addressed paths)

**Content Hash Storage**: SHA256 hash stored in asset node payload field (e.g., `content_hash`).

### BeliefNode Structure for Assets

Assets represented as `External` or `Trace` nodes with hash in payload:

```rust
impl BeliefNode {
    /// Create BeliefNode for a static asset
    /// BID is stable (v6 UUID), content hash stored in payload
    pub fn static_asset(hash: &str) -> Self {
        let bid = buildonomy_asset_bid(); // Stable BID
        let mut node = BeliefNode {
            bid,
            kind: BeliefKind::External,
            ..Default::default()
        };
        // Store content hash in payload
        node.payload.insert("content_hash".to_string(), toml::Value::String(hash.to_string()));
        node
    }
}
```

**Content Change Detection**:
```rust
// When asset file changes:
let new_hash = compute_sha256(&file_bytes);
let existing_node = session_bb.get(&asset_bid)?;
let old_hash = existing_node.payload.get("content_hash");
if old_hash != Some(&new_hash) {
    // Emit NodeUpdate event (same BID, new payload)
    // Documents never need rewriting
}
```

**Epistemic Relation**: Document → Asset with original path in relation weight:

Via `md.rs` LinkAccumulator, Markdown parsing identifies asset references, and links to them as follows (pseudocode)

```rust
proto.upstream.push((NodeKey::Path(net: asset_namespace(), path: make_relative_path(repo_anchored_doc_path, link_defined_relation_path), RelationType::Epistemic, TomlTable::default()))
```

## Implementation Steps

### 1. Add Static Asset Namespace ✅ COMPLETE

Already implemented in `properties.rs`:
- `UUID_NAMESPACE_ASSET` constant
- `static_namespace()` function
- `buildonomy_asset_bid(hash_str)` function

### 2. Extend LinkAccumulator for Static Assets ✅ COMPLETE

Already implemented in `md.rs`:
- `is_image: bool` field tracks image vs link
- `MdTag::Image` handling in `LinkAccumulator::new()`
- Distinguishes images from document links

### 3. Compiler Asset Resolution - 2 days

**Goal**: Detect asset references, compute content hashes, generate BIDs, create asset lookup list that `watch.rs` file watch can use to inform of asset content updates.

#### 3a. Add Asset Manifest Types (REMOVED)

YAGNI, we don't need any of these additional properties.

#### 3b. Extend DocumentCompiler ✅ COMPLETE

Added to `compiler.rs`:

```rust
pub struct DocumentCompiler {
    // ... existing fields
    
    /// Asset tracking for file watcher integration.
    /// Maps repo-relative asset paths to content-addressed BIDs.
    /// Wrapped in Arc<RwLock<_>> for cross-thread access.
    asset_manifest: Arc<RwLock<BTreeMap<String, Bid>>>,
}
```

**Implementation notes:**
- Uses `parking_lot::RwLock` for efficiency
- Public accessor `asset_manifest()` returns cloned Arc for file watcher
- Initialized in both `new()` and `simple()` constructors
- Added `session_bb_mut()` and `tx()` accessors to GraphBuilder

#### 3c. Asset Detection During Parse ✅ COMPLETE (NEEDS PIVOT)

**Entry Point 1: Asset file enters queue directly** ✅

In `parse_next()` when filepath extension is NOT in CODECs (this is an asset file):

1. Read file bytes and compute SHA256 hash
2. Generate `asset_bid = buildonomy_asset_bid(hash)`
3. Resolve network-relative path
4. Check if asset already tracked at this path:
   - Query `session_bb().paths().net_get_from_path(&asset_namespace(), &repo_relative_path)` 
   - Get `old_bid` if path exists (returns `Option<(Bid, Bid)>` - home net bid, path bid)
   - **Use `net_get_from_path()` to avoid lock contention and ensure BeliefBase is source of truth**
   
5. Determine what changed:
   - **Path exists with SAME bid** → Skip (no change, can happen via file watcher poke)
   - **Path exists with DIFFERENT bid** → Content changed (issue NodeUpdate + RelationChange)
   - **Path doesn't exist** → New asset (issue NodeUpdate + RelationChange)

6. If change detected, create events:
     ```rust
     let asset_node = BeliefNode {
         bid: asset_bid,
         kind: BeliefKind::External.into(),
         ..Default::default()
     };
     
     // NodeKey array depends on whether old_bid exists
     let node_keys = if let Some(old_bid) = old_bid {
         vec![
             NodeKey::Bid { bid: old_bid },  // OLD BID (content changed at same path)
             NodeKey::Bid { bid: asset_bid }, // NEW BID
         ]
     } else {
         vec![NodeKey::Bid { bid: asset_bid }] // NEW asset, only new BID
     };
     
     let mut update_queue = Vec::default();
     update_queue.push(BeliefEvent::NodeUpdate(
         node_keys,
         asset_node.toml(),
         EventOrigin::Remote,
     ));
     
     // Create Section relation to asset_namespace with repo-relative path
     // This creates a NEW relation or ADDS this path to existing relation (via generate_edge_update)
     let mut edge_payload = toml::Table::new();
     edge_payload.insert(
         WEIGHT_DOC_PATHS.to_string(), 
         toml::Value::Array(vec![toml::Value::String(repo_relative_path.display().to_string())])
     );
     let weight = Weight { payload: edge_payload };
     
     update_queue.push(BeliefEvent::RelationChange(
         asset_bid,
         asset_namespace(),
         WeightKind::Section,
         Some(weight),
         EventOrigin::Remote,
     ));
     
     // Process into session_bb
     let mut derivatives = Vec::new();
     for event in update_queue.iter() {
         derivatives.append(&mut self.builder.session_bb_mut().process_event(event)?);
     }
     update_queue.append(&mut derivatives);
     
     // Send to global cache via tx
     for event in update_queue.into_iter() {
         self.builder.tx().send(event)?;
     }
     
     // Update asset_manifest for file watcher
     {
         let mut manifest = self.asset_manifest.write();
         manifest.insert(repo_relative_path.clone(), asset_bid);
     }
     ```

**Implementation location:** `src/codec/compiler.rs` lines 328-562 in `parse_next()`

**Critical Fix - Asset Network Node Creation:**
Before adding asset relations, we now ensure `asset_namespace()` exists as a BeliefNode (not just a Bid). Added `BeliefNode::asset_network()` in `src/properties.rs` (similar to `href_network()`). The compiler checks if `asset_namespace()` exists in `session_bb().states()` and creates it before adding asset relations (lines 484-509). Without this, relations fail with "sink is missing" warnings.

**Note on Multi-Path Strategy**: Each path discovery creates a separate `RelationChange` event (Strategy B). The `generate_edge_update()` function in `beliefbase.rs` automatically merges paths into the `WEIGHT_DOC_PATHS` array when multiple RelationChange events reference the same asset BID. This enables assets with multiple paths (symlinks, copies, multiple references) to accumulate all their paths in one relation.

**Entry Point 2: Document references asset (UnresolvedReference)** ✅

In `parse_next()` after normal document parse:

1. In unresolved_references loop, detect asset references via `NodeKey::Path { net, .. }` where `net == asset_namespace()`
2. For each asset reference:
   - Extract asset path from NodeKey and resolve relative to document directory
   - Canonicalize to get absolute filesystem path
   - Compute repo-relative path with Windows normalization
   - Check if already tracked: `session_bb().paths().net_get_from_path(&asset_namespace(), &repo_relative_path)`
   - If NOT tracked:
     - Add asset absolute path to `primary_queue` (triggers Entry Point 1)
     - Add document to `reparse_queue` if not already there (will reparse after asset processed)
     - Set `reparse_stable = false`
   - Skip normal dependency handling with `continue` for asset references

**Implementation notes:**
- Uses `net_get_from_path()` to query BeliefBase directly (single source of truth)
- Avoids lock contention by NOT using nested PathMapMap locks
- File watcher resets `reparse_count`, enabling document reparse after assets tracked
- Implementation location: `src/codec/compiler.rs` lines 658-755 in `parse_next()`

**Queue Empty Detection** ✅

At end of `parse_next()`, when both queues empty (lines 277-327):

1. Regenerate `self.asset_manifest` from BeliefBase:
   ```rust
   let asset_map = self.builder.session_bb().paths().asset_map();
   let assets: Vec<(String, Bid)> = asset_map
       .map()
       .iter()
       .filter_map(|(path, bid, _order)| {
           // Verify this is actually an External node (asset)
           self.builder
               .session_bb()
               .states()
               .get(bid)
               .filter(|n| n.kind.is_external())
               .map(|_| (path.clone(), *bid))
       })
       .collect();
   ```
 
2. Clear and update `asset_manifest` with current assets
3. Check for duplicate BIDs (same content at multiple paths) → log at **debug** level (not warning, as this is informational)
4. Log info message with count of unique assets

**Why regenerate?** The manifest is derived state from BeliefBase. Regenerating ensures it's synchronized with the final state after all parsing and relation merging is complete.


#### 3d. Deduplication Warnings (REMOVED - STEP NO LONGER NEEDED)

**Previously**: This step updated Network BeliefNode files with asset manifests.

**Now**: Asset manifests are **derived state** from BeliefBase relations. No file writing needed.

**To get assets for a network**:

**Deduplication warnings** are logged in Step 3c when parsing completes (queue empty check).

### 4. Pivot Step 3c to Stable BIDs ✅ COMPLETE - 0.5 days

**Status**: COMPLETE ✅ - Architectural pivot implemented

**Why**: Current implementation uses content-addressed BIDs (hash-based), causing document rewrites on asset changes. Stable BIDs with hash in payload enable cleaner architecture.

**Changes Needed**:

1. **BID Generation** (`src/codec/compiler.rs`)
   - Replace `buildonomy_asset_bid(hash)` with `Bid::now_v6()`
   - Store hash in node payload: `asset_node.payload.insert("content_hash", hash)`

2. **Content Change Detection** (`src/codec/compiler.rs`)
   - Compare `existing_node.payload["content_hash"]` vs `new_hash`
   - If different: emit NodeUpdate (same BID, new payload)
   - If same: skip (no change)

3. **Test Updates** (`tests/codec_test.rs`)
   - Update expectations: NodeUpdate instead of NodeRenamed
   - Verify payload contains content_hash
   - Verify documents don't get rewritten on asset changes

**Success Criteria**: ✅ ALL COMPLETE
- ✅ Assets get stable v6 UUID BIDs (via `Bid::new(&asset_namespace())`)
- ✅ Content hash stored in node payload (`payload["content_hash"]`)
- ✅ Content changes emit NodeUpdate events (single BID in node_keys)
- ✅ Documents never rewritten on asset content changes (stable brefs)
- ✅ All 10 asset tests passing (updated expectations)

**Implementation Details**:
- BID generation: `Bid::new(&asset_namespace())` creates stable v6 UUID per path
- Hash storage: `payload.insert("content_hash", hash)` in asset BeliefNode
- Change detection: Compare `existing_node.payload["content_hash"]` vs new file hash
- Event emission: Single BID in node_keys → NodeUpdate (not NodeRenamed)
- Test updates: Fixed namespace filtering, updated expectations for stable BIDs
- Files modified: `src/codec/compiler.rs`, `tests/codec_test.rs`

### 5. HTML Output Hardlink Creation ✅ COMPLETE - 1 day

**Status**: COMPLETE ✅ - Asset hardlinks with content-addressed deduplication

**Goal**: Create hardlinks in `html_output_dir` preserving semantic paths.

#### 5a. Content-Addressed Hardlink Strategy

During HTML generation, inside or near `generate_html_for_path()` (called from `compiler.rs` ~line 368-407 when content changes or first parse):

```rust
// Pseudocode - implementation details TBD
let net_paths = self.builder.session_bb.paths()
    .get_map(self.builder.repo).unwrap()
    .all_net_paths(self.builder.session_bb.paths(), &mut BTreeSet::default());

let mut copied_canonical = BTreeSet::<PathBuf>::default();

for (asset_path, asset_bid) in asset_manifests.iter() {
    // Get asset node to extract content hash from payload
    let asset_node = self.builder.session_bb.states().get(&asset_bid)?;
    let content_hash = asset_node.payload
        .get("content_hash")
        .and_then(|v| v.as_str())
        .ok_or("Asset missing content_hash in payload")?;
    
    // Get extension, default to empty string if none (no trailing dot)
    let ext = asset_path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    
    // Content-addressed location: static/{hash}.{ext} or static/{hash} if no extension
    let canonical_name = if ext.is_empty() {
        format!("{}", content_hash)
    } else {
        format!("{}.{}", content_hash, ext)
    };
    let canonical = output_base.join("static").join(canonical_name);
        
        // Copy to canonical location (once per content hash)
        if !copied_canonical.contains(&canonical) {
            let repo_full_path = repo_path.join(asset_path);
            fs::create_dir_all(canonical.parent().unwrap()).await?;
            fs::copy(&repo_full_path, &canonical).await?;
            copied_canonical.insert(canonical.clone());
        }
        
        // Create hardlink at semantic path
        let html_full_path = output_base.join(asset_path);
        fs::create_dir_all(html_full_path.parent().unwrap()).await?;
        
        // hard_link(src, dst) - canonical is src, semantic path is dst
        fs::hard_link(&canonical, &html_full_path).await?;
    }
}
```

**Result**: 
- `static/{hash}.png` - canonical physical file (one per unique content hash)
- `assets/test_image.png` - hardlink → canonical (preserves semantic path)
- Both paths share same inode (automatic deduplication via hardlinks)

**Success Criteria**: ✅ ALL COMPLETE
- ✅ Assets copied to `static/{hash}.{ext}` (content-addressed canonical location)
- ✅ Semantic paths hardlinked to canonical files
- ✅ Deduplication: same hash → one physical file, multiple hardlinks
- ✅ Fallback to copy if hardlinks not supported by filesystem
- ✅ Integration test verifies end-to-end asset pipeline
- ✅ All 10 asset tests passing (added new hardlink test)

**Implementation Details**:
- Method: `DocumentCompiler::create_asset_hardlinks()` in `src/codec/compiler.rs`
- Called from: `parse_all()` after all documents parsed
- Hash extraction: Read `node.payload["content_hash"]` from asset BeliefNodes
- Canonical naming: `static/{hash}.{ext}` or `static/{hash}` if no extension
- Deduplication: Track copied canonical files in HashSet, copy once per hash
- Hardlink creation: `tokio::fs::hard_link(canonical, semantic_path)` with copy fallback
- Error handling: Warns on failure, doesn't fail entire parse
- Files modified: `src/codec/compiler.rs`, `tests/codec_test.rs`


## Testing Requirements

### Unit Tests

- `test_asset_bid_generation()` - Verify deterministic BIDs from hashes
- `test_duplicate_detection()` - Same hash at multiple paths logs warning
- `test_missing_asset_handling()` - Missing file creates unresolved relation
- `test_asset_no_extension()` - Verify assets without extensions get canonical names without trailing dots

### Integration Tests

- `test_asset_belief_nodes()` - Verify assets appear in belief graph
- `test_asset_queries()` - Query assets by document and vice versa
- `test_hardlink_creation()` - Verify hardlinks created in HTML output
- `test_multi_path_asset_tracking()` - Same asset content at multiple paths (symlinks, copies)
- `test_multi_document_asset_refs()` - Multiple documents referencing same asset
- `test_asset_all_paths_query()` - BeliefBase queries returning all paths for a given asset BID
- `test_asset_path_accumulation()` - WEIGHT_DOC_PATHS array accumulates paths via multiple RelationChange events

### Manual Testing

1. Create test document with images at various relative paths
2. Run HTML export → verify hardlinks created preserving paths
3. Modify asset content → verify BID changes (when watch integration complete)

## Success Criteria ✅ ALL COMPLETE

- [x] Assets tracked as BeliefNodes in belief graph with External kind
- [x] Hardlinks created in HTML output preserving semantic paths
- [x] Duplicate content detection logs warnings
- [x] Missing assets create diagnostic warnings (no hard failure)
- [x] Document → Asset relations created during reparse
- [x] All tests passing (11 asset tests, 180 total tests)

## Risks

### Risk 1: Hardlink Filesystem Support
**Impact**: Medium  
**Likelihood**: Low  
**Mitigation**: Hardlinks supported on Windows (NTFS), macOS (APFS/HFS+), Linux (ext4/btrfs). Fallback to copy if hardlink fails.

### Risk 2: Large Asset Performance
**Impact**: Medium  
**Likelihood**: Medium  
**Mitigation**: SHA256 computation is I/O bound. For >1000 assets, consider parallel hashing with rayon. Defer to Issue 31 if needed.

### Risk 3: Path Resolution Edge Cases
**Impact**: Low  
**Likelihood**: Medium  
**Mitigation**: Use `dunce::canonicalize()` for Windows UNC paths, handle symlinks in source. Document known limitations.

## Open Questions

### Q1: What to do when asset file missing?
**Decision**: Emit diagnostic warning in ParseResult, continue compilation. Don't fail hard.

### Q2: Handle git-ignored binary files?
**Decision**: Track all referenced assets regardless of .gitignore. User controls what gets committed. Reactive discovery (only track referenced assets).

## References

### Related Issues
- **Issue 32**: Schema Registry Productionization (will automate manifest → edge translation)
- **Issue 30**: External URL tracking (parallel architecture)
- **Issue 31**: Watch service integration for asset changes
- **Issue 6**: HTML generation (depends on this for asset copying)
- **Issue 25**: Per-network theming (will use asset tracking)

### Architecture References
- `properties.rs:buildonomy_asset_bid()` - BID generation pattern
- `properties.rs:href_namespace()` - Parallel external resource pattern
- `md.rs:LinkAccumulator` - Image tag detection

### Future Enhancements
- **Broken Link Detection**: Automated scanning for missing assets (separate issue)
- **Asset Optimization**: Image compression, format conversion (separate issue)
- **Space-Efficient Deduplication**: Canonical `static/` directory for all hardlinks (optimize after profiling)

## Implementation Progress

### Session 7 (2026-01-31): Steps 4 & 5 COMPLETE ✅

**Achievement**: Completed stable BID architecture AND HTML asset hardlinks

#### Step 4: Stable BIDs ✅

**Achievement**: Pivoted from content-addressed to stable BID architecture

**Changes**:
- BID generation: `buildonomy_asset_bid(hash)` → `Bid::new(&asset_namespace())`
- Hash storage: Added `payload["content_hash"]` to asset nodes
- Change detection: Compare payload hash vs file hash (not BID comparison)
- Event emission: NodeUpdate with single BID (not NodeRenamed with two BIDs)
- Test updates: Fixed namespace filtering, updated expectations
- Result: All 10 asset tests passing, all 166 total tests passing

#### Step 5: HTML Asset Hardlinks ✅

**Achievement**: Content-addressed hardlink creation with automatic deduplication

**Implementation**:
- Added `DocumentCompiler::create_asset_hardlinks()` method (L1436-1572)
- Extracts `content_hash` from asset node payloads
- Copies unique assets to `static/{hash}.{ext}` (canonical location)
- Creates hardlinks from semantic paths to canonical files
- Falls back to copy if hardlinks unsupported
- Called from `parse_all()` after document parsing completes

**Test Coverage**:
- Added `test_asset_html_hardlinks`: Verifies canonical files exist in static/
- Validates semantic paths are created (hardlinks or copies)
- Confirms file contents match between original and output
- Result: All 10 asset tests passing, 179 total tests passing

**Files Modified**:
- `src/codec/compiler.rs`: Added hardlink creation method, integrated into parse_all
- `tests/codec_test.rs`: Added comprehensive integration test

### Session 6 (2026-01-31): Step 3c Content Change Detection ✅
- ✅ Fixed asset content change detection across compiler sessions
- ✅ Added `initialize_stack()` cache fetch: Loads asset_namespace from global_bb into session_bb
- ✅ Modified queue-empty detection: Enqueues unparsed assets from PathMap for content verification
- ✅ Always enqueue assets when referenced: Enables content change detection even for tracked assets
- ✅ Fixed `test_asset_content_changed`: Now properly detects BID changes and emits NodeRenamed events
- ✅ Fixed `test_multi_path_asset_tracking`: Added document reference for duplicate asset (no file walking)
- **All 178 tests passing** (129 lib + 25 codec + 4 schema + 8 service + 12 server) ✅
- **All 10/10 asset-specific tests passing** ✅
- Documented PathMap multi-path query issue in BACKLOG.md (non-blocking for Issue 29)

**Key Architectural Insight**: Asset discovery through document references (no unconstrained file walking) is correct design - constrains generation to referenced content only.

**Implementation Files**:
- `src/codec/builder.rs` (lines 576-605): Asset namespace cache fetch in initialize_stack()
- `src/codec/compiler.rs` (lines 277-335): Unparsed asset enqueueing during queue-empty detection
- `src/codec/compiler.rs` (lines 728-742): Always enqueue assets for content verification
- `tests/codec_test.rs` (lines 1378-1385): Fixed test_multi_path_asset_tracking with document reference

### Session 5 (2026-01-30): Step 3c COMPLETE ✅
- ✅ Step 3b: Added `asset_manifest` field to DocumentCompiler with `parking_lot::RwLock`
- ✅ Step 3c Entry Point 1: Asset file processing (SHA256 hashing, BID generation, BeliefNode creation)
- ✅ Step 3c Entry Point 2: Document asset reference detection and queueing
- ✅ Step 3c Queue Empty Detection: Regenerate asset_manifest from BeliefBase when queues empty
- Fixed lock contention by using `net_get_from_path()` to query BeliefBase directly
- Fixed synchronization by making BeliefBase the single source of truth
- **Critical fix**: Added `BeliefNode::asset_network()` to create asset namespace node before relations
- Added GraphBuilder accessors: `session_bb_mut()`, `tx()`
- Added dependency: `sha2 = "0.10"` for SHA256 hashing
- Updated test `test_belief_set_builder_bid_generation_and_caching` to account for asset BIDs
- **9/10 asset-specific tests passing** (2 content-change tests deferred to Session 6)

### Session 4 (2026-01-29): Issue Rewrite ✅
- Clarified architecture based on Q&A
- Focused scope on core tracking (removed watch service to Issue 31)
- Corrected BID generation to content-hash only
- Documented hardlink strategy
- Split external URL tracking to Issue 30

### Sessions 1-3 (2026-01-29): Initial Implementation
- ✅ Step 1: Asset namespace (`properties.rs`)
- ✅ Step 2: LinkAccumulator image tracking (`md.rs`)
- ✅ Step 3b: DocumentCompiler extension
- ✅ Step 3c: Asset detection and processing (all three components)
- 🚧 Step 5: TODO (HTML hardlink creation)

## Notes

**Why Hardlinks Over Symlinks**: Cross-platform compatibility. Windows requires admin privileges for symlinks but not hardlinks. Git handles hardlinks correctly.

**Why Stable BIDs + Hash in Payload**: (NEW ARCHITECTURE)
- Documents reference stable BIDs that never change (no rewrites on asset updates)
- Content hash in payload enables change detection via NodeUpdate events
- Deduplication happens at HTML output layer (content-addressed paths)
- Watch service (Issue 31) emits NodeUpdate when hash changes - simple and clean

**Content-Addressed Output Paths**: 
- HTML references use `/static/{hash}.{ext}` (immutable, cache-friendly)
- Multiple source paths with same content → one output file (automatic deduplication)
- BeliefBase preserves semantic distinction (different source BIDs)
- Best of both worlds: stable source references, efficient output

**Deduplication Philosophy**: 
- Source layer: Preserve semantic paths (different BIDs for different purposes)
- Output layer: Automatic deduplication via content-addressed hardlinks
- Users control source organization, system handles output efficiency
