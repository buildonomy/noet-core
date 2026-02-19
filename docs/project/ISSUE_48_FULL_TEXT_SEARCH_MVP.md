# Issue 48: Full-Text Search MVP - Embedded WASM

**Priority**: HIGH
**Estimated Effort**: 5-7 days
**Dependencies**: None
**Version**: 0.1

## Summary

Implement full-text search for static HTML output using Tantivy embedded in WASM. Establishes per-network indexing architecture from the start, enabling users to search content within browser with no external dependencies. Scales to 100s of documents per network with intelligent memory management.

## Goals

- Full-text search works in static HTML output (`noet parse`, `noet watch`)
- Per-network indexing architecture (one Tantivy index per network)
- Keyword search with fuzzy matching for typo tolerance
- Scales to 100s of documents per network (in-memory WASM)
- Intelligent memory budget: 50% BeliefBase cache + 50% search indices
- User selects which networks to search (controls memory footprint)
- No external dependencies (fully embedded in browser)

## Architecture

### Per-Network Indexing Model

```
BeliefBase
├── Network A (50 documents)  → search/[bref_a]/index/
├── Network B (120 documents) → search/[bref_b]/index/
└── Network C (30 documents)  → search/[bref_c]/index/

User searches → Selects [A, B] → Load indices A+B → Search across both
```

**Key insight**: Networks are already isolated in PathMapMap architecture. This is the natural sharding boundary.

### Tantivy Schema

**Per-document indexed fields**:
- `bid` (STRING, STORED): Stable identifier for results
- `network` (STRING, STORED): Network bref this document belongs to
- `title` (TEXT, STORED): Document/section title for display
- `content` (TEXT): Extracted from `payload["text"]` (markdown format)
- `kind` (FACET): Document/Section/Procedure for filtering
- `schema` (STRING, STORED): Optional schema type
- `id` (STRING, STORED): Semantic identifier (e.g., "intro")
- `path` (STRING, STORED): Display path from PathMap (for URLs)

**Schema mirrors BeliefNode**: Only difference is extracting text from payload for full-text indexing.

### Index Building in finalize_html

```rust
// In compiler.rs::finalize_html()
pub async fn finalize_html<B: BeliefSource + Clone>(
    &self,
    global_bb: B,
) -> Result<(), BuildonomyError> {
    // ... existing code ...
    
    // Export beliefbase.json
    let graph = global_bb.export_beliefgraph().await?;
    self.export_beliefbase_json(graph).await?;
    
    // NEW: Build per-network search indices
    self.build_search_indices(global_bb.clone()).await?;
    
    Ok(())
}

async fn build_search_indices<B: BeliefSource>(
    &self,
    global_bb: B,
) -> Result<(), BuildonomyError> {
    let html_dir = self.html_output_dir.as_ref().unwrap();
    let pathmap = /* get from global_bb */;
    
    for network_bref in pathmap.nets() {
        // Query global_bb for this network's complete data
        // (session_bb is incomplete due to lazy compilation)
        let network_expr = Expression::StateIn(
            StatePred::InNamespace(network_bref.clone())
        );
        let network_bb = global_bb.eval_balanced(&network_expr).await?;
        
        let index_dir = html_dir.join("search").join(network_bref.to_string());
        let mut index_builder = NetworkIndexBuilder::new(index_dir)?;
        
        for (bid, node) in network_bb.states() {
            let path = pathmap.net_path(&network_bref, bid);
            let text = node.payload.get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            
            index_builder.add_document(
                node.bid,
                network_bref,
                &node.title,
                text, // markdown format - Tantivy handles it well
                node.kind,
                node.schema.as_deref(),
                node.id.as_deref(),
                path,
            )?;
        }
        
        index_builder.commit()?;
    }
    
    Ok(())
}
```

### Memory Budget Strategy

**Total budget**: 200MB (conservative for modern devices including smartphones)
- **100MB**: BeliefBase cache (beliefbase.json)
- **100MB**: Search indices (per-network Tantivy indices)

**Network size manifest** (`search/manifest.json`):
```json
{
  "networks": [
    {
      "bref": "01abc",
      "title": "Main Docs",
      "documentCount": 45,
      "estimatedSizeMB": 0.2
    },
    {
      "bref": "02def",
      "title": "API Reference",
      "documentCount": 120,
      "estimatedSizeMB": 0.5
    }
  ],
  "totalEstimatedSizeMB": 0.7
}
```

**User experience**:
- UI shows network selector with size estimates
- "Load All Networks" (with warning if exceeds budget)
- "Smart Selection" (auto-select networks that fit in budget)
- Progress indicator during index loading

### WASM API

```rust
// src/wasm/search.rs
#[wasm_bindgen]
pub struct SearchEngineWasm {
    loaded_indices: BTreeMap<Bref, TantivyIndex>,
    current_size_mb: f64,
    max_size_mb: f64,
}

#[wasm_bindgen]
impl SearchEngineWasm {
    /// Load search manifest with network metadata
    pub fn from_manifest(manifest_json: String) -> Self;
    
    /// Load a specific network's index
    /// Returns true if loaded, false if would exceed memory budget
    pub async fn load_network(&mut self, network_bref: String) -> bool;
    
    /// Unload a network's index to free memory
    pub fn unload_network(&mut self, network_bref: String);
    
    /// Search across loaded networks
    /// Returns: [{bid, network, title, snippet, score, path}]
    pub fn search(&self, query: String, limit: usize) -> JsValue;
    
    /// Get currently loaded networks
    pub fn loaded_networks(&self) -> JsValue;
    
    /// Get current memory usage estimate
    pub fn current_size_mb(&self) -> f64;
}
```

**JavaScript usage**:
```javascript
import init, { BeliefBaseWasm, SearchEngineWasm } from './noet_wasm.js';

async function main() {
    await init();
    
    // Load BeliefBase
    const bbResponse = await fetch('beliefbase.json');
    const bb = BeliefBaseWasm.from_json(await bbResponse.text());
    
    // Load search engine
    const manifestResponse = await fetch('search/manifest.json');
    const search = SearchEngineWasm.from_manifest(await manifestResponse.text());
    
    // User selects networks to search
    await search.load_network('01abc'); // Main Docs
    await search.load_network('02def'); // API Reference
    
    // Search
    const results = search.search('authentication', 10);
    
    // Display results with full node data
    for (const result of results) {
        const node = bb.get_by_bid(result.bid);
        console.log(`${result.path} - ${result.title}`);
        console.log(`  Snippet: ${result.snippet}`);
    }
}
```

### Viewer.js Integration

**New search UI components**:
1. Search input with fuzzy matching
2. Network selector (checkboxes with size estimates)
3. Results list with snippets and paths
4. "Load More Networks" button
5. Memory usage indicator

**Search result format**:
```typescript
interface SearchResult {
    bid: string;          // For fetching full node from BeliefBase
    network: string;      // Which network this result is from
    title: string;        // Display title
    snippet: string;      // Highlighted snippet from content
    score: number;        // Relevance score
    path: string;         // Display path for URL (join with entry_point)
}
```

## Implementation Steps

### Phase 1: Core Search Module (2 days)

#### Step 1.1: Add Dependencies (0.5 days)
- [ ] Add `tantivy = "0.22"` to `Cargo.toml`
- [ ] Add `#[cfg(feature = "service")]` gate for search module
- [ ] Verify Tantivy compiles for WASM target (`wasm32-unknown-unknown`)

#### Step 1.2: Create Search Module Structure (0.5 days)
- [ ] Create `src/search/mod.rs` with public API
- [ ] Create `src/search/schema.rs` for Tantivy schema definition
- [ ] Create `src/search/indexer.rs` for index building
- [ ] Create `src/search/query.rs` for query execution
- [ ] Add module declaration in `src/lib.rs`

#### Step 1.3: Implement Tantivy Schema (1 day)
- [ ] Define `create_schema()` with fields: bid, network, title, content, kind, schema, id, path
- [ ] Configure tokenizers: English stemmer, lowercase filter
- [ ] Add fuzzy matching support (Levenshtein distance)
- [ ] Add schema validation tests

### Phase 2: Index Building (2 days)

#### Step 2.1: NetworkIndexBuilder (1.5 days)
- [ ] `NetworkIndexBuilder::new(index_path)` - create/open index
- [ ] `add_document()` - index single BeliefNode
- [ ] `commit()` - finalize index to disk
- [ ] Extract text from `payload["text"]` field
- [ ] Handle markdown format (no conversion needed)
- [ ] Unit tests with small BeliefGraph samples

#### Step 2.2: Integrate into finalize_html (0.5 days)
- [ ] Add `build_search_indices()` method to DocumentCompiler
- [ ] Query global_bb per network (not session_bb - incomplete)
- [ ] Use PathMap for path resolution
- [ ] Write indices to `html_output_dir/search/[network_bref]/`
- [ ] Generate `search/manifest.json` with size estimates
- [ ] Integration test: Verify indices created for test documents

### Phase 3: Query API (1.5 days)

#### Step 3.1: Search Engine (1 day)
- [ ] `SearchEngine::open(index_path)` - open existing index
- [ ] `search(query, limit)` - execute search with fuzzy matching
- [ ] `SearchResult` struct: bid, network, title, snippet, score, path
- [ ] Generate snippets with context (3 lines before/after match)
- [ ] Rank by TF-IDF (Tantivy default BM25)
- [ ] Query parsing tests (keyword, fuzzy, filtering)

#### Step 3.2: Multi-Network Search (0.5 days)
- [ ] `MultiNetworkSearchEngine` - manages multiple indices
- [ ] Load/unload networks dynamically
- [ ] Merge results from multiple indices
- [ ] Re-rank merged results by score
- [ ] Track memory usage estimates

### Phase 4: WASM Bindings (1.5 days)

#### Step 4.1: SearchEngineWasm (1 day)
- [ ] Create `src/wasm/search.rs`
- [ ] Implement `from_manifest()` - load network metadata
- [ ] Implement `load_network()` - load index with memory budget check
- [ ] Implement `unload_network()` - free memory
- [ ] Implement `search()` - returns JSON results (plain object, not Map)
- [ ] Handle async index loading (use wasm-bindgen-futures)
- [ ] WASM serialization tests (verify plain objects, not Maps)

#### Step 4.2: Build Configuration (0.5 days)
- [ ] Add WASM build target to CI/CD
- [ ] Update `wasm-pack` configuration
- [ ] Verify Tantivy WASM compatibility
- [ ] Document WASM bundle size impact

### Phase 5: Frontend Integration (1.5 days)

#### Step 5.1: Search UI (1 day)
- [ ] Add search input to viewer.js
- [ ] Add network selector with checkboxes
- [ ] Display size estimates and memory usage
- [ ] Results list with snippets and paths
- [ ] Click result → navigate to document
- [ ] Loading indicators during index load

#### Step 5.2: Search State Management (0.5 days)
- [ ] Store loaded networks in localStorage
- [ ] Restore search state on page load
- [ ] Handle network selection changes
- [ ] Display warnings when exceeding memory budget

### Phase 6: Documentation & Testing (1 day)

#### Step 6.1: User Documentation (0.5 days)
- [ ] README section: "Full-Text Search"
- [ ] How to use search in viewer
- [ ] Network selection strategy
- [ ] Memory budget explanation
- [ ] Troubleshooting (index not loading, out of memory)

#### Step 6.2: Integration Tests (0.5 days)
- [ ] Test: Index building for multi-network repo
- [ ] Test: Search across multiple networks
- [ ] Test: Fuzzy search finds typos
- [ ] Test: Memory budget enforcement
- [ ] Test: Path resolution via PathMap

## Testing Requirements

### Unit Tests
- Tantivy schema creation and validation
- Text extraction from BeliefNode payload
- Query parsing (keyword, fuzzy)
- Result snippet generation
- Memory budget calculations

### Integration Tests
- Full index building from BeliefGraph
- Multi-network search with result merging
- WASM module loading and search execution
- Path resolution via PathMap
- Network size estimation accuracy

### Manual Testing
- Search UI responsiveness
- Network selector usability
- Memory usage in browser DevTools
- Search quality (relevance, fuzzy matching)
- Large repository (100+ documents per network)

## Success Criteria

- [ ] Search works in static HTML output from `noet parse`
- [ ] Per-network indices generated in `html_output_dir/search/`
- [ ] WASM search module loads and executes queries
- [ ] Fuzzy search finds results with typos (1-2 character difference)
- [ ] Memory budget enforced (user can't load more than 100MB indices)
- [ ] Search across 3+ networks with 100+ documents each
- [ ] Search latency < 200ms for typical queries
- [ ] Results include highlighted snippets and correct paths
- [ ] Network selector shows accurate size estimates
- [ ] User documentation explains search usage

## Risks

### Risk 1: Tantivy WASM Compatibility
**Impact**: HIGH - Tantivy may not fully support WASM target
**Likelihood**: MEDIUM
**Mitigation**: 
- Verify WASM compatibility early (Step 1.1)
- Tantivy 0.22+ has WASM support via community efforts
- Fallback: Use simpler search library (elasticlunr.js) for Phase 1

### Risk 2: Index Size Exceeds Memory Budget
**Impact**: MEDIUM - Large networks may not fit in 100MB budget
**Likelihood**: LOW (for typical repos)
**Mitigation**:
- Generate size estimates during index building
- Warn users before loading large networks
- Phase 2 will add server-side search for large deployments

### Risk 3: Fuzzy Search Performance
**Impact**: LOW - Fuzzy matching may be slow for large indices
**Likelihood**: MEDIUM
**Mitigation**:
- Limit fuzzy search to 2-edit distance
- Only apply fuzzy matching to queries < 20 characters
- Add configuration to disable fuzzy if needed

### Risk 4: Path Resolution Complexity
**Impact**: MEDIUM - Sub-network paths need special handling
**Likelihood**: LOW
**Mitigation**:
- PathMap already handles sub-network path resolution
- Join with entry_point path for correct URLs
- Test with nested network structures

## Design Decisions

### Decision 1: Per-Network Indexing from Start
**Rationale**: Aligns with PathMapMap architecture, enables modular scaling
**Trade-offs**: More indices to manage, but better memory control
**Alternatives Considered**: Single monolithic index - rejected (doesn't scale)

### Decision 2: Index from global_bb, Not session_bb
**Rationale**: Compiler is lazy, session_bb is incomplete
**Trade-offs**: Requires per-network queries, but ensures comprehensive indexing
**Pattern**: Same as asset manifest in compiler.rs:889-897

### Decision 3: Markdown Format for Content
**Rationale**: Tantivy handles markdown well, no conversion needed
**Trade-offs**: Some markdown syntax in search results, but acceptable
**Alternatives Considered**: Convert to plain text - rejected (unnecessary complexity)

### Decision 4: 50/50 Memory Split
**Rationale**: Both BeliefBase and search need significant memory
**Trade-offs**: May need adjustment based on usage patterns
**Future**: Both will need sharding strategy for large repos (Phase 2+)

### Decision 5: Fuzzy Over Boolean
**Rationale**: Better user experience for typos and misspellings
**Trade-offs**: Slightly slower queries, but worth it for UX
**Future**: Add boolean operators in Phase 2/Backlog

## References

- Tantivy documentation: https://docs.rs/tantivy
- WASM integration: wasm-bindgen guide
- PathMap architecture: `docs/design/beliefbase_architecture.md`
- Memory budget discussion: `.scratchpad/search_architecture_review.md`

## Notes

- Phase 1 establishes architecture for Phase 2 (ISSUE_49) scaling
- Per-network indexing is critical architectural decision - not optional
- Memory budget applies to BOTH beliefbase.json and search indices
- Future work: Server-side search, event-based updates, GB-scale testing
- Backlog: Boolean operators, advanced ranking signals, query.rs integration