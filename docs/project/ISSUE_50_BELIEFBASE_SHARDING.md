# Issue 50: BeliefBase Sharding - Per-Network Export and Loading

**Priority**: MEDIUM
**Estimated Effort**: 4-6 days
**Dependencies**: ISSUE_48 (Full-Text Search MVP) - shares sharding architecture
**Version**: 0.2+

## Summary

Implement per-network BeliefBase sharding for JSON export and WASM loading, mirroring the search index sharding strategy from ISSUE_48. Creates a unified `ShardManager` abstraction to provide consistent API for managing both BeliefBase data and search indices with intelligent memory budgeting.

## Goals

- Export BeliefBase as per-network JSON shards instead of monolithic `beliefbase.json`
- Load BeliefBase shards on-demand in WASM based on user's network selection
- Unified `ShardManager` for consistent sharding API (BeliefBase + search indices)
- Memory budget enforcement: 50% BeliefBase + 50% search indices (200MB total)
- Graceful degradation when corpus exceeds memory budget
- Backward compatibility: Single `beliefbase.json` for small repos (< 10MB)

## Architecture

### Current Problem

**Export side** (compiler.rs:1439-1480):
```rust
pub async fn export_beliefbase_json(
    &self,
    graph: crate::beliefbase::BeliefGraph,
) -> Result<(), BuildonomyError> {
    // WARNING: Exports entire graph to single beliefbase.json
    // Problem: Large repos (1000+ documents) create 10+ MB files
    let json_path = html_dir.join("beliefbase.json");
    tokio::fs::write(&json_path, json_string).await?;
}
```

**Load side** (viewer.js:1201-1279):
```javascript
async function initializeWasm() {
    // Problem: Loads entire beliefbase.json into memory
    // No selective loading by network
    const response = await fetch("/beliefbase.json");
    const beliefbaseJson = await response.text();
    beliefbase = new wasmModule.BeliefBaseWasm(beliefbaseJson, entryBidString);
}
```

### Proposed Sharding Architecture

```
html_output_dir/
├── beliefbase/
│   ├── manifest.json          # Shard metadata
│   ├── networks/
│   │   ├── 01abc.json         # Network A BeliefGraph
│   │   ├── 02def.json         # Network B BeliefGraph
│   │   └── 03ghi.json         # Network C BeliefGraph
│   └── global.json            # API node + cross-network relations
└── search/
    ├── manifest.json          # Search index metadata
    └── networks/
        ├── 01abc/index/       # Network A search index
        ├── 02def/index/       # Network B search index
        └── 03ghi/index/       # Network C search index
```

**Backward compatibility**: If total size < 10MB, export single `beliefbase.json` (no sharding).

### Manifest Format

**`beliefbase/manifest.json`**:
```json
{
  "version": "1.0",
  "sharded": true,
  "totalSizeMB": 15.3,
  "memoryBudgetMB": 200,
  "beliefbaseFraction": 0.5,
  "searchFraction": 0.5,
  "networks": [
    {
      "bref": "01abc",
      "bid": "01234567-89ab-cdef-0123-456789abcdef",
      "title": "Main Documentation",
      "nodeCount": 247,
      "relationCount": 512,
      "estimatedSizeMB": 3.2,
      "path": "networks/01abc.json"
    },
    {
      "bref": "02def",
      "bid": "02345678-9abc-def0-1234-56789abcdef0",
      "title": "API Reference",
      "nodeCount": 456,
      "relationCount": 892,
      "estimatedSizeMB": 5.8,
      "path": "networks/02def.json"
    }
  ],
  "global": {
    "nodeCount": 5,
    "estimatedSizeMB": 0.02,
    "path": "global.json"
  }
}
```

### Per-Network Shard Format

**`beliefbase/networks/01abc.json`**:
```json
{
  "network_bref": "01abc",
  "network_bid": "01234567-89ab-cdef-0123-456789abcdef",
  "states": {
    "01234567-89ab-cdef-0123-456789abcdef": { /* network node */ },
    "11111111-2222-3333-4444-555555555555": { /* document node */ }
  },
  "relations": {
    "edges": [ /* subsection relations within network */ ]
  }
}
```

**`beliefbase/global.json`**:
```json
{
  "states": {
    "buildonomy-api-bid": { /* API node */ }
  },
  "relations": {
    "edges": [ /* cross-network relations */ ]
  }
}
```

### Unified ShardManager API

**Rust side** (`src/shard/manager.rs`):
```rust
pub struct ShardConfig {
    pub total_memory_mb: f64,
    pub beliefbase_fraction: f64,
    pub search_fraction: f64,
    pub shard_threshold_mb: f64,  // Don't shard if < this size
}

impl Default for ShardConfig {
    fn default() -> Self {
        Self {
            total_memory_mb: 200.0,
            beliefbase_fraction: 0.5,
            search_fraction: 0.5,
            shard_threshold_mb: 10.0,
        }
    }
}

pub struct NetworkShard {
    pub bref: Bref,
    pub bid: Bid,
    pub title: String,
    pub node_count: usize,
    pub relation_count: usize,
    pub estimated_size_mb: f64,
}

pub struct ShardManifest {
    pub sharded: bool,
    pub total_size_mb: f64,
    pub networks: Vec<NetworkShard>,
    pub global_size_mb: f64,
}

pub struct ShardManager {
    config: ShardConfig,
}

impl ShardManager {
    pub fn new(config: ShardConfig) -> Self;
    
    /// Analyze BeliefGraph and determine if sharding is needed
    pub fn should_shard(&self, graph: &BeliefGraph) -> bool;
    
    /// Build shard manifest from BeliefGraph
    pub fn build_manifest(
        &self,
        graph: &BeliefGraph,
        pathmap: &PathMapMap,
    ) -> ShardManifest;
    
    /// Export BeliefGraph as sharded JSON files
    pub async fn export_sharded(
        &self,
        graph: BeliefGraph,
        output_dir: &Path,
        pathmap: &PathMapMap,
    ) -> Result<ShardManifest, BuildonomyError>;
    
    /// Export BeliefGraph as single file (backward compat)
    pub async fn export_monolithic(
        &self,
        graph: BeliefGraph,
        output_dir: &Path,
    ) -> Result<(), BuildonomyError>;
}
```

**JavaScript/WASM side** (`assets/shard-manager.js`):
```javascript
class ShardManager {
    constructor(config = {}) {
        this.totalMemoryMB = config.totalMemoryMB || 200;
        this.beliefbaseFraction = config.beliefbaseFraction || 0.5;
        this.searchFraction = config.searchFraction || 0.5;
        
        this.beliefbaseShards = new Map(); // bref -> BeliefGraphShard
        this.searchIndices = new Map();    // bref -> SearchIndex
        
        this.currentBeliefbaseMB = 0;
        this.currentSearchMB = 0;
    }
    
    get maxBeliefbaseMB() {
        return this.totalMemoryMB * this.beliefbaseFraction;
    }
    
    get maxSearchMB() {
        return this.totalMemoryMB * this.searchFraction;
    }
    
    async loadManifest(basePath = '/beliefbase') {
        const response = await fetch(`${basePath}/manifest.json`);
        this.manifest = await response.json();
        return this.manifest;
    }
    
    async loadBeliefbaseShard(networkBref) {
        if (this.beliefbaseShards.has(networkBref)) {
            return this.beliefbaseShards.get(networkBref);
        }
        
        const networkInfo = this.manifest.networks.find(n => n.bref === networkBref);
        if (!networkInfo) {
            throw new Error(`Network ${networkBref} not found in manifest`);
        }
        
        // Check memory budget
        if (this.currentBeliefbaseMB + networkInfo.estimatedSizeMB > this.maxBeliefbaseMB) {
            console.warn(`Cannot load ${networkBref}: would exceed BeliefBase memory budget`);
            return null;
        }
        
        // Load shard
        const response = await fetch(`/beliefbase/${networkInfo.path}`);
        const shardData = await response.json();
        
        this.beliefbaseShards.set(networkBref, shardData);
        this.currentBeliefbaseMB += networkInfo.estimatedSizeMB;
        
        console.log(`Loaded BeliefBase shard ${networkBref} (${networkInfo.estimatedSizeMB} MB)`);
        return shardData;
    }
    
    async loadSearchIndex(networkBref) {
        // Similar to loadBeliefbaseShard but for search indices
        // Coordinates with search index loading from ISSUE_48
    }
    
    unloadBeliefbaseShard(networkBref) {
        const shard = this.beliefbaseShards.get(networkBref);
        if (shard) {
            const networkInfo = this.manifest.networks.find(n => n.bref === networkBref);
            this.currentBeliefbaseMB -= networkInfo.estimatedSizeMB;
            this.beliefbaseShards.delete(networkBref);
            console.log(`Unloaded BeliefBase shard ${networkBref}`);
        }
    }
    
    getLoadedNetworks() {
        return Array.from(this.beliefbaseShards.keys());
    }
    
    getMemoryUsage() {
        return {
            beliefbase: {
                current: this.currentBeliefbaseMB,
                max: this.maxBeliefbaseMB,
                percentage: (this.currentBeliefbaseMB / this.maxBeliefbaseMB) * 100
            },
            search: {
                current: this.currentSearchMB,
                max: this.maxSearchMB,
                percentage: (this.currentSearchMB / this.maxSearchMB) * 100
            },
            total: {
                current: this.currentBeliefbaseMB + this.currentSearchMB,
                max: this.totalMemoryMB,
                percentage: ((this.currentBeliefbaseMB + this.currentSearchMB) / this.totalMemoryMB) * 100
            }
        };
    }
}
```

### WASM BeliefBase Integration

**New WASM API** (`src/wasm/beliefbase.rs`):
```rust
#[wasm_bindgen]
pub struct BeliefBaseWasm {
    // Current: Single BeliefBase with all data
    // New: ShardedBeliefBase with lazy loading
    base: BeliefBase,
    loaded_shards: BTreeSet<Bref>,
}

#[wasm_bindgen]
impl BeliefBaseWasm {
    /// Load from monolithic beliefbase.json (backward compat)
    #[wasm_bindgen(constructor)]
    pub fn new(beliefbase_json: String, entry_bid_string: String) -> Self;
    
    /// Load from sharded manifest
    pub fn from_manifest(manifest_json: String, entry_bid_string: String) -> Self;
    
    /// Load a specific network shard
    pub fn load_shard(&mut self, network_bref: String, shard_json: String);
    
    /// Unload a network shard to free memory
    pub fn unload_shard(&mut self, network_bref: String);
    
    /// Get list of loaded shards
    pub fn loaded_shards(&self) -> JsValue;
    
    /// Check if a BID is in loaded shards (for lazy loading)
    pub fn has_bid(&self, bid: String) -> bool;
}
```

### Updated viewer.js Integration

```javascript
async function initializeWasm() {
    console.log("[Noet] Loading WASM module...");
    wasmModule = await import("/assets/noet_core.js");
    await wasmModule.default();
    
    // Initialize shard manager
    shardManager = new ShardManager();
    await shardManager.loadManifest();
    
    if (shardManager.manifest.sharded) {
        console.log("[Noet] Using sharded BeliefBase");
        
        // Load global shard (API node, cross-network relations)
        const globalResponse = await fetch(`/beliefbase/${shardManager.manifest.global.path}`);
        const globalJson = await globalResponse.text();
        
        beliefbase = wasmModule.BeliefBaseWasm.from_manifest(
            JSON.stringify(shardManager.manifest),
            entryBidString
        );
        beliefbase.load_shard("global", globalJson);
        
        // Load entry point network
        const entryPoint = beliefbase.entryPoint();
        await shardManager.loadBeliefbaseShard(entryPoint.bref);
        const entryShardJson = await fetch(`/beliefbase/networks/${entryPoint.bref}.json`)
            .then(r => r.text());
        beliefbase.load_shard(entryPoint.bref, entryShardJson);
        
        // UI: Show network selector for loading additional shards
        buildNetworkSelector(shardManager);
    } else {
        console.log("[Noet] Using monolithic beliefbase.json");
        const response = await fetch("/beliefbase.json");
        const beliefbaseJson = await response.text();
        beliefbase = new wasmModule.BeliefBaseWasm(beliefbaseJson, entryBidString);
    }
    
    buildNavigation();
}

function buildNetworkSelector(shardManager) {
    const selector = document.getElementById('network-selector');
    const memoryDisplay = document.getElementById('memory-usage');
    
    for (const network of shardManager.manifest.networks) {
        const checkbox = document.createElement('input');
        checkbox.type = 'checkbox';
        checkbox.id = `net-${network.bref}`;
        checkbox.value = network.bref;
        checkbox.checked = shardManager.beliefbaseShards.has(network.bref);
        
        checkbox.addEventListener('change', async (e) => {
            if (e.target.checked) {
                await shardManager.loadBeliefbaseShard(network.bref);
                const shardJson = await fetch(`/beliefbase/networks/${network.bref}.json`)
                    .then(r => r.text());
                beliefbase.load_shard(network.bref, shardJson);
            } else {
                shardManager.unloadBeliefbaseShard(network.bref);
                beliefbase.unload_shard(network.bref);
            }
            updateMemoryDisplay();
        });
        
        const label = document.createElement('label');
        label.htmlFor = checkbox.id;
        label.textContent = `${network.title} (${network.estimatedSizeMB.toFixed(1)} MB)`;
        
        selector.appendChild(checkbox);
        selector.appendChild(label);
    }
    
    updateMemoryDisplay();
}

function updateMemoryDisplay() {
    const usage = shardManager.getMemoryUsage();
    const display = document.getElementById('memory-usage');
    display.textContent = `Memory: ${usage.total.current.toFixed(1)}/${usage.total.max} MB (${usage.total.percentage.toFixed(0)}%)`;
    
    if (usage.total.percentage > 90) {
        display.classList.add('warning');
    }
}
```

## Implementation Steps

### Phase 1: ShardManager Core (1.5 days)

#### Step 1.1: Create Shard Module (0.5 days)
- [ ] Create `src/shard/mod.rs` with ShardManager
- [ ] Create `src/shard/manifest.rs` for manifest types
- [ ] Define ShardConfig, NetworkShard, ShardManifest structs
- [ ] Implement `should_shard()` with 10MB threshold
- [ ] Unit tests for shard decision logic

#### Step 1.2: Manifest Building (1 day)
- [ ] Implement `build_manifest()` from BeliefGraph
- [ ] Extract per-network subgraphs using PathMapMap
- [ ] Estimate shard sizes (JSON serialization dry-run)
- [ ] Separate global nodes (API node, cross-network relations)
- [ ] Generate manifest JSON with network metadata
- [ ] Unit tests with multi-network BeliefGraphs

### Phase 2: Export Implementation (1.5 days)

#### Step 2.1: Sharded Export (1 day)
- [ ] Implement `export_sharded()` in ShardManager
- [ ] Per-network JSON files: `networks/[bref].json`
- [ ] Global JSON file: `global.json`
- [ ] Write manifest: `beliefbase/manifest.json`
- [ ] Create directory structure
- [ ] Integration test: Verify shard files created

#### Step 2.2: Integrate into finalize_html (0.5 days)
- [ ] Replace `export_beliefbase_json()` with ShardManager
- [ ] Backward compat: Use monolithic if < 10MB
- [ ] Log shard statistics (count, sizes)
- [ ] Integration test: Test both monolithic and sharded paths

### Phase 3: WASM Loading (1.5 days)

#### Step 3.1: BeliefBaseWasm Sharding Support (1 day)
- [ ] Add `from_manifest()` constructor
- [ ] Add `load_shard()` method (merge into existing base)
- [ ] Add `unload_shard()` method (remove nodes/relations)
- [ ] Add `loaded_shards()` getter
- [ ] Handle BID queries across shards
- [ ] Unit tests for shard loading/unloading

#### Step 3.2: JavaScript ShardManager (0.5 days)
- [ ] Create `assets/shard-manager.js`
- [ ] Implement ShardManager class
- [ ] Memory budget tracking
- [ ] Shard loading with budget enforcement
- [ ] Integration with BeliefBaseWasm
- [ ] Unit tests (Jest or similar)

### Phase 4: Viewer Integration (1 day)

#### Step 4.1: Update initializeWasm (0.5 days)
- [ ] Detect sharded vs monolithic format
- [ ] Load manifest if sharded
- [ ] Load entry point network shard
- [ ] Load global shard
- [ ] Backward compat: Single beliefbase.json still works
- [ ] Integration test: Load sharded beliefbase in viewer

#### Step 4.2: Network Selector UI (0.5 days)
- [ ] Add network selector component
- [ ] Show network names and sizes
- [ ] Checkboxes for loading/unloading
- [ ] Memory usage display
- [ ] Visual warning when approaching budget
- [ ] CSS styling

### Phase 5: Documentation & Testing (0.5 days)

#### Step 5.1: User Documentation (0.25 days)
- [ ] README: "BeliefBase Sharding"
- [ ] Explain automatic sharding threshold
- [ ] How to select networks in viewer
- [ ] Memory budget explanation
- [ ] Troubleshooting

#### Step 5.2: Integration Tests (0.25 days)
- [ ] Test: Large repo triggers sharding
- [ ] Test: Small repo uses monolithic format
- [ ] Test: Viewer loads sharded beliefbase
- [ ] Test: Network selection works correctly
- [ ] Test: Memory budget enforced

## Testing Requirements

### Unit Tests
- Shard decision logic (size threshold)
- Manifest building from BeliefGraph
- Per-network subgraph extraction
- Size estimation accuracy
- ShardManager memory tracking

### Integration Tests
- Export sharded beliefbase for multi-network repo
- Load sharded beliefbase in WASM
- Network selection and memory management
- Backward compatibility with monolithic format
- Query across loaded shards

### Manual Testing
- Viewer with sharded beliefbase (3+ networks)
- Load/unload networks dynamically
- Memory usage display accuracy
- Performance with large shards (100+ MB)
- Backward compat: Old beliefbase.json still works

## Success Criteria

- [ ] BeliefBase exports as per-network shards when > 10MB
- [ ] Manifest includes accurate size estimates
- [ ] WASM loads shards on-demand
- [ ] Memory budget enforced (50/50 BeliefBase + search)
- [ ] Network selector UI shows loaded networks
- [ ] Backward compat: Single beliefbase.json still works
- [ ] Query performance unchanged (shard lookup overhead < 5ms)
- [ ] Documentation explains sharding strategy
- [ ] Integration with ISSUE_48 search sharding

## Risks

### Risk 1: Query Performance Across Shards
**Impact**: MEDIUM - Queries may need to check multiple shards
**Likelihood**: MEDIUM
**Mitigation**: 
- Cache loaded shards in single BeliefBase
- Most queries stay within single network
- Profile and optimize hot paths

### Risk 2: Shard Loading Race Conditions
**Impact**: HIGH - Concurrent loads may corrupt state
**Likelihood**: LOW
**Mitigation**:
- Async/await ensures sequential loading
- Lock shard map during load/unload
- Test concurrent operations

### Risk 3: Size Estimation Accuracy
**Impact**: LOW - Manifest sizes may not match actual
**Likelihood**: MEDIUM
**Mitigation**:
- Use conservative estimates (10% buffer)
- Warn users if actual size exceeds estimate
- Improve estimation with real-world data

### Risk 4: Backward Compatibility Breakage
**Impact**: HIGH - Old viewers can't load new format
**Likelihood**: LOW (tested explicitly)
**Mitigation**:
- Maintain monolithic export for small repos
- Version manifest format
- Test both code paths

## Design Decisions

### Decision 1: 10MB Threshold for Sharding
**Rationale**: Large enough to avoid unnecessary sharding, small enough to stay under memory budget
**Trade-offs**: May need tuning based on usage patterns
**Configuration**: Exposed in ShardConfig for customization

### Decision 2: Per-Network Sharding (Not Per-Document)
**Rationale**: Aligns with PathMapMap architecture, matches search sharding
**Trade-offs**: Large networks still problematic, but rare
**Future**: Per-document sharding if needed (Phase 2)

### Decision 3: Unified ShardManager for BeliefBase + Search
**Rationale**: Consistent API, shared memory budget, simpler mental model
**Trade-offs**: Tighter coupling, but worth it for consistency
**Benefit**: User sees unified memory management

### Decision 4: Lazy Loading (Not Eager)
**Rationale**: User controls what's loaded, conserves memory
**Trade-offs**: Requires network selector UI, but better UX
**Pattern**: Same as search index loading (ISSUE_48)

### Decision 5: Backward Compatibility
**Rationale**: Smooth migration path, no breaking changes
**Trade-offs**: More code paths to test, but essential
**Threshold**: < 10MB → monolithic, >= 10MB → sharded

## References

- ISSUE_48: Full-Text Search MVP (shared sharding strategy)
- PathMapMap architecture: `docs/design/beliefbase_architecture.md`
- Current export: `src/codec/compiler.rs:1439-1480`
- Current loading: `assets/viewer.js:1201-1279`
- Search sharding discussion: `.scratchpad/search_architecture_review.md`

## Notes

- Shares sharding paradigm with ISSUE_48 (search indices)
- ShardManager provides unified API for both BeliefBase and search
- Memory budget split: 50% BeliefBase + 50% search = 200MB total
- Backward compatibility ensures smooth transition
- Future: Same pattern can extend to other resources (images, videos)
- Per-network sharding is natural boundary (matches PathMapMap)

## Future Enhancements (Backlog)

- Per-document sharding for very large networks (1000+ documents)
- Server-side shard streaming (fetch on-demand, no preload)
- IndexedDB caching for loaded shards (persist across sessions)
- Compression for shard JSON (gzip, brotli)
- Incremental shard updates (daemon mode)
- Network dependency resolution (auto-load referenced networks)
- Shard preloading based on user navigation patterns
- Memory pressure detection (browser API)