# Trade Study: Text Search Indexer for noet-core

**Version**: 0.1
**Date**: 2025-01-24
**Status**: Investigation
**Updated**: 2025-01-24 (GB-scale requirements)

## Summary

Evaluate open-source text search indexers for integration into noet-core's HTML compilation workflow. The indexer must support stemming/lemmatization, return BID-based results for link generation, and integrate cleanly with the existing `finalize_html()` pipeline.

**Critical Constraint**: System must scale to **GB-scale document collections** (1000s of documents, 100MB+ JSON exports). This eliminates client-side-only solutions and requires server-side search architecture.

## Context

**Integration Point**: `src/codec/compiler.rs::finalize_html()` after `export_beliefbase_json()`
- Has access to synchronized `BeliefBase` (global_bb)
- Runs after all document parsing and event processing
- Already exports `beliefbase.json` for client-side use

**Data Structure** (`BeliefNode`):
```rust
pub struct BeliefNode {
    pub bid: Bid,              // Stable identifier for search results
    pub title: String,         // Primary searchable text
    pub kind: BeliefKindSet,   // Document, Section, etc.
    pub schema: Option<String>, // Optional categorization
    pub payload: Table,        // TOML table with additional text
    pub id: Option<String>,    // Optional semantic identifier
}
```

**Search Requirements**:
- Full-text search across `title` and `payload` fields
- Stemming/lemmatization (e.g., "running" matches "run")
- Return results as BIDs for link generation
- Support filtering by `kind` and `schema`
- Fast indexing (< 1 second for typical knowledge base)
- Fast search (< 100ms for interactive queries)

**Use Cases**:
1. **Phase 0**: Static index generated at compile time, searched client-side (WASM)
2. **Phase 1**: Real-time search via daemon/LSP (future enhancement)
3. **Phase 2**: Incremental updates on file change (future enhancement)

**Existing Implementation**: 
noet-core already has a basic `search()` method in the WASM API (`src/wasm.rs`):
- Case-insensitive substring matching on `title` and `payload.id` fields
- Returns matching BeliefNodes as JSON
- No stemming, ranking, or advanced query features
- Simple linear scan through all nodes (O(n) per search)

## Evaluation Criteria

1. **Integration Complexity**: How hard to integrate into existing pipeline?
2. **Performance**: Indexing speed, search speed, index size
3. **Stemming Support**: Quality of language processing
4. **License**: Compatible with MIT/Apache-2.0
5. **Maturity**: Production readiness, maintenance status
6. **Query Features**: Boolean operators, fuzzy search, ranking
7. **Size**: Binary size, dependency footprint
8. **Improvement over existing**: Does it meaningfully enhance current substring search?
9. **Scalability**: Can it handle GB-scale document collections efficiently?

## Options

### Option A: Tantivy (Rust Native)

**Description**: Full-text search engine library in Rust (similar to Lucene)

**Repository**: https://github.com/quickwit-oss/tantivy
**License**: MIT
**Language**: Rust
**Latest Release**: v0.22.0 (2024-11)

**Architecture**:
- In-process library (no subprocess)
- Disk-based index (can be embedded in HTML output dir)
- WASM-compatible (with `wasm32-unknown-unknown` target)

**Integration Pattern**:
```rust
// In finalize_html()
let index_path = html_dir.join("search_index");
let mut indexer = TantivyIndexer::new(&index_path)?;

for (bid, node) in graph.states.iter() {
    indexer.add_document(
        bid,
        &node.title,
        node.payload.get("content").and_then(|v| v.as_str()),
        &node.kind,
        node.schema.as_deref(),
    )?;
}

indexer.commit()?;
```

**Pros**:
- Native Rust integration (no subprocess overhead)
- Excellent performance (benchmarks competitive with Lucene)
- Rich query syntax (boolean, phrase, fuzzy, ranking)
- Active development (used by Quickwit for PB-scale log search)
- Strong stemming support via `tantivy-stempel` or custom tokenizers
- **Designed for GB-TB scale** (disk-based index, memory-mapped I/O)
- **Incremental updates** (add/remove documents without full rebuild)
- **Pagination support** (return top-N results efficiently)
- Can integrate with daemon/service architecture

**Cons**:
- Large dependency (~50 crates transitively)
- Learning curve for query DSL
- Binary size increase (~2-3 MB compiled)
- Requires daemon/service for real-time search (can't be purely static HTML)

**Stemming**:
- Built-in stemmers via tokenizer pipeline
- Supports English (Porter, Snowball), multi-language via `tantivy-stempel`
- Custom tokenizers possible

**Performance** (estimated):
- **Small (1000 docs)**: Index ~500-1000ms, search ~10-50ms, index size ~1-5 MB
- **Medium (10k docs)**: Index ~5-10s, search ~20-100ms, index size ~10-50 MB
- **Large (100k docs, GB-scale)**: Index ~30-60s, search ~50-200ms, index size ~100-500 MB

**GB-Scale Suitability**: ✅ **EXCELLENT**
- Designed for this use case
- Memory-mapped I/O avoids loading entire index
- Incremental updates essential at this scale
- Production-proven at PB-scale (Quickwit)

**Deployment**:
- Requires daemon/service architecture (not static HTML-only)
- Index stored on disk, served via HTTP/LSP API
- Can't embed in WASM at GB-scale (index too large)

---

### Option B: Sonic (Server Process)

**Description**: Fast, lightweight search backend with simple text protocol

**Repository**: https://github.com/valeriansaliou/sonic
**License**: MPL-2.0
**Language**: Rust
**Latest Release**: v1.4.9 (2024-08)

**Architecture**:
- Standalone server process (TCP connection)
- In-memory index with disk persistence
- Text-based protocol (not HTTP)

**Integration Pattern**:
```rust
// In finalize_html()
let sonic_client = SonicClient::connect("127.0.0.1:1491")?;
sonic_client.flushb("noet")?; // Clear previous index

for (bid, node) in graph.states.iter() {
    sonic_client.push(
        "noet",
        "documents",
        &bid.to_string(),
        &format!("{} {}", node.title, extract_text(&node.payload)),
    )?;
}
```

**Pros**:
- Very lightweight (< 10 MB binary)
- Simple protocol (easy to implement client)
- Low memory footprint (~20 MB for small datasets)
- Fast indexing and search
- Active maintenance

**Cons**:
- Requires separate server process (complicates deployment)
- MPL-2.0 license (compatible but copyleft)
- Limited query features (no boolean operators, basic ranking)
- No WASM support (server-based only)
- Text-only protocol (no structured queries)
- **Not designed for GB-scale** (in-memory index, would need massive RAM)
- No incremental updates (full reindex on change)

**Stemming**:
- Basic normalization (lowercase, stop words)
- No built-in stemming (would need preprocessing)

**Performance** (estimated):
- Indexing: ~200-500ms
- Search: ~5-20ms
- Memory: ~20-50 MB

**GB-Scale Suitability**: ❌ **POOR**
- In-memory design doesn't scale to GB
- Would need 10+ GB RAM for large datasets
- No incremental updates

**Deployment Issues**:
- Not suitable for GB-scale search
- Better alternatives exist (Tantivy)

---

### Option C: MiniSearch (JavaScript)

**Description**: Tiny in-memory full-text search engine for JavaScript

**Repository**: https://github.com/lucaong/minisearch
**License**: MIT
**Language**: JavaScript/TypeScript
**Latest Release**: v7.1.0 (2024-10)

**Architecture**:
- Client-side library (no server required)
- In-memory index from JSON data
- Runs in browser via WASM-loaded BeliefGraph

**Integration Pattern**:
```rust
// No Rust code changes needed!
// Use existing beliefbase.json export

// In HTML/JavaScript:
const beliefbase = await fetch('beliefbase.json').then(r => r.json());
const searchIndex = MiniSearch.loadJSON(indexFromBeliefBase(beliefbase));
const results = searchIndex.search(query);
```

**Pros**:
- Zero Rust integration (uses existing JSON export)
- Tiny footprint (~10 KB minified)
- Runs entirely in browser (perfect for Phase 0)
- No compilation overhead (index built at page load)
- Excellent docs and examples
- Active development

**Cons**:
- Index built at runtime (latency on page load)
- Limited to JavaScript environment (no LSP/daemon support)
- Memory-only (no persistence)
- Stemming limited (basic normalization)
- **Not suitable for large datasets (> 10k documents)**
- **FATAL at GB-scale**: Would freeze browser, crash on memory exhaustion

**Stemming**:
- Basic normalization (lowercase, diacritics)
- Prefix search (good for completion)
- No true stemming (could add custom processor)

**Performance**:
- Indexing: 500-2000ms (client-side on page load)
- Search: 10-50ms
- Index size: Similar to JSON source (~1-2x beliefbase.json)

**GB-Scale Suitability**: ❌ **FATAL**
- In-memory only (browser crashes with 100MB+ JSON)
- Can't paginate or lazy-load
- Indexing would take minutes and freeze page

**Best for**: Small knowledge bases (< 1000 documents, < 10MB JSON) ONLY

---

### Option D: Stork (Static Search)

**Description**: Fast static search generator for JAMstack sites

**Repository**: https://github.com/jameslittle230/stork
**License**: Apache-2.0
**Language**: Rust
**Latest Release**: v1.6.0 (2024-03)

**Architecture**:
- CLI tool generates static `.st` index file
- WASM library loads index in browser
- Designed for static site deployment

**Integration Pattern**:
```rust
// Generate stork config from BeliefGraph
let stork_config = generate_stork_config(&graph);
fs::write("stork.toml", stork_config)?;

// Build index via subprocess
Command::new("stork")
    .args(&["build", "--input", "stork.toml", "--output", "index.st"])
    .spawn()?
    .wait()?;

// Copy index.st to HTML output dir
```

**Pros**:
- Purpose-built for static sites (matches Phase 0 use case)
- WASM library for client-side search
- Pre-built index (no runtime indexing overhead)
- Good stemming support
- Apache-2.0 license

**Cons**:
- Requires separate CLI tool (subprocess overhead)
- Less flexible than Tantivy (opinionated config format)
- Smaller community than Tantivy
- Index format is opaque (hard to customize)
- Moderate binary size (~5 MB compiled)

**Stemming**:
- English stemming via Snowball
- Configurable stop words
- Diacritic normalization

**Performance**:
- Indexing: ~500-1000ms (CLI build)
- Search: ~20-50ms (WASM)
- Index size: ~500 KB - 2 MB compressed

**GB-Scale Suitability**: ⚠️ **MARGINAL**
- Static index generation would take many minutes
- Index file size would be 100s of MB (slow to download)
- No incremental updates (full rebuild on any change)
- Browser WASM would struggle with large index

**Best for**: Small-to-medium static sites (< 10k documents), not GB-scale

---

### Option E: Elasticlunr.rs (Rust Port)

**Description**: Rust port of Elasticlunr.js (itself a Lunr.js fork)

**Repository**: https://github.com/mattico/elasticlunr-rs
**License**: MIT
**Language**: Rust
**Status**: **Unmaintained** (last release 2019)

**Not recommended**: Abandoned project, better alternatives exist (Tantivy, Stork)

---

### Option F: Custom Inverted Index

**Description**: Build minimal inverted index from scratch

**Architecture**:
- Simple HashMap<String, Vec<Bid>>
- Serialize to JSON for WASM consumption
- Custom stemmer using `rust-stemmers` crate

**Integration Pattern**:
```rust
use rust_stemmers::{Algorithm, Stemmer};

let stemmer = Stemmer::create(Algorithm::English);
let mut index: HashMap<String, Vec<Bid>> = HashMap::new();

for (bid, node) in graph.states.iter() {
    for word in tokenize(&node.title) {
        let stem = stemmer.stem(&word);
        index.entry(stem.to_string()).or_default().push(*bid);
    }
}

let json = serde_json::to_string(&index)?;
fs::write(html_dir.join("search_index.json"), json)?;
```

**Pros**:
- Minimal dependencies (`rust-stemmers` is ~50 KB)
- Full control over format and behavior
- Tiny footprint (< 100 KB compiled)
- Easy to customize for BID-based results

**Cons**:
- No query DSL (must implement)
- No ranking/relevance scoring
- Limited to basic term matching
- More maintenance burden
- Missing features (phrase search, fuzzy match)

**Stemming**:
- Via `rust-stemmers` crate (Snowball algorithms)
- Supports 15+ languages

**Performance**:
- Indexing: ~100-300ms
- Search: ~5-20ms (simple HashMap lookup)
- Index size: ~500 KB - 1 MB

**Best for**: Minimal viable search with full control

---

## Comparison Matrix

| Criterion              | Tantivy    | Sonic      | MiniSearch | Stork      | Custom     |
|------------------------|------------|------------|------------|------------|------------|
| Tantivy    | Sonic      | MiniSearch | Stork      | Custom     |
|------------|------------|------------|------------|------------|
| Integration Complexity | Medium     | High       | Low        | Low        | Low        |
| Indexing Speed         | Fast       | Very Fast  | Medium     | Fast       | Very Fast  |
| Search Speed           | Very Fast  | Very Fast  | Fast       | Fast       | Fast       |
| Stemming Quality       | Excellent  | Poor       | Basic      | Good       | Good       |
| License Compatibility  | ✅ MIT     | ⚠️ MPL-2.0 | ✅ MIT     | ✅ Apache  | ✅ MIT     |
| Binary Size Impact     | +2-3 MB    | N/A        | +10 KB     | +5 MB      | +50 KB     |
| Query Features         | Excellent  | Basic      | Good       | Good       | Basic      |
| Maintenance Status     | Active     | Active     | Active     | Moderate   | N/A        |
| **GB-Scale Suitability** | ✅ **Excellent** | ❌ Poor | ❌ **Fatal** | ⚠️ Marginal | ❌ Poor |
| Incremental Updates    | ✅ Yes     | ❌ No      | ❌ No      | ❌ No      | ⚠️ Custom  |
| Requires Daemon        | ✅ Yes     | ✅ Yes     | ❌ No      | ❌ No      | ⚠️ Maybe   |

---

## Recommendations

### GB-Scale Changes Everything

**Critical realization**: At GB-scale (1000s of documents, 100MB+ JSON):
- ❌ **Client-side search is impossible** (browser crashes, memory exhaustion)
- ❌ **Static HTML-only deployment can't support search** (no server to query)
- ✅ **Must use daemon/service architecture** (server-side search required)
- ✅ **Incremental indexing is essential** (can't rebuild GB index on every file change)

**Existing WASM `search()` limitations at scale:**
- Loads entire `beliefbase.json` into browser memory
- O(n) linear scan through all nodes
- At GB-scale: 100MB+ JSON, 10k+ nodes, browser freezes/crashes

### For GB-Scale: **Option A (Tantivy)** is MANDATORY

**Winner: Option A (Tantivy)**

**Rationale**:
1. **Only option that scales to GB**: Designed for TB-scale, production-proven
2. **Incremental updates**: Essential at scale (can't rebuild GB index on every change)
3. **Disk-based index**: Doesn't load entire dataset into memory
4. **Fast search**: Sub-100ms even at GB-scale with memory-mapped I/O
5. **Native Rust**: Integrates cleanly with existing daemon/service architecture
6. **Rich features**: Stemming, ranking, boolean queries, fuzzy search

**Architecture Requirements**:
- **Daemon/service required**: Can't be static HTML-only
- **Index storage**: Disk-based (e.g., `~/.cache/noet/search_index/`)
- **API exposure**: HTTP endpoint or LSP protocol for search queries
- **Incremental updates**: Watch file changes, update index (don't full rebuild)

**Implementation Plan** (8-12 hours):

1. **Tantivy Integration** (4 hours)
   - Add `tantivy` dependency to `Cargo.toml` (service feature)
   - Create `SearchIndex` struct in new module `src/search/`
   - Define schema: bid (stored), title (indexed+stored), content (indexed), kind (stored), schema (stored)
   - Implement indexing in `finalize_html()` or separate daemon method

2. **Incremental Updates** (3 hours)
   - Hook into file watch system (`notify-debouncer-full`)
   - On file change: delete old document BIDs, add new ones
   - Commit index after batch of changes

3. **Search API** (2 hours)
   - Add HTTP endpoint `/api/search?q=query&limit=20&offset=0`
   - OR: Add LSP method `noet/search` with query params
   - Return results as `{ bid, title, snippet, score }`

4. **Query DSL** (2 hours)
   - Parse user queries: `"running shoes" kind:Document`
   - Support filters by kind, schema, network
   - Boolean operators: AND, OR, NOT
   - Fuzzy search: `~0.8` tolerance

5. **Frontend Integration** (1 hour)
   - Update WASM wrapper to call daemon search API
   - Fall back to existing `search()` if daemon unavailable

**Why Not MiniSearch/Stork?**
- ❌ MiniSearch: Fatal at GB-scale (browser crashes loading 100MB+ JSON)
- ❌ Stork: No incremental updates (full rebuild on any change = minutes)
- ❌ Custom: Would need to reimplement Tantivy's features
- ❌ Sonic: In-memory design, not designed for GB-scale

---

### For Small Knowledge Bases (< 1000 documents, < 10MB)

**Alternative: MiniSearch (client-side)**

If you have both large and small deployments:
- Small sites: Use MiniSearch (client-side, no daemon needed)
- Large sites: Use Tantivy (daemon required)
- Decision point: Check `beliefbase.json` size at compile time
  - If < 10MB → include MiniSearch in HTML
  - If > 10MB → require daemon with Tantivy

---

## Open Questions

1. **Index size threshold**: At what point does `beliefbase.json` become too large for client-side indexing?
   - Need benchmarks with real-world knowledge bases (100, 1000, 10000 documents)

2. **Stemming quality requirements**: Do users need perfect stemming, or is basic normalization sufficient?
   - Could A/B test MiniSearch vs. Custom with `rust-stemmers`

3. **Multi-language support**: Should Phase 0 support non-English stemming?
   - MiniSearch has limited language support
   - Custom + `rust-stemmers` supports 15+ languages

4. **Search UI integration**: Where does search fit in ISSUE_41 query builder?
   - Text search vs. structured query (by kind, schema, etc.)
   - Could be separate tab or unified interface

5. **Export format**: Should search index be separate file or embedded in `beliefbase.json`?
   - Separate: Smaller JSON, but extra HTTP request
   - Embedded: Single file, but larger payload

---

## Next Steps for GB-Scale

### Phase 1: Tantivy Integration (Required)

**Prerequisites**:
- Daemon/service architecture already exists (`service` feature with `axum`, `notify`, `sqlx`)
- File watching infrastructure in place (`notify-debouncer-full`)
- Need to expose search API (HTTP or LSP)

**Implementation** (8-12 hours):

1. **Add Tantivy dependency** (30 min)
   ```toml
   [dependencies]
   tantivy = { version = "0.22", optional = true }
   
   [features]
   service = [..., "tantivy"]
   ```

2. **Create search index module** (3 hours)
   - `src/search/mod.rs`: Main indexing logic
   - `src/search/schema.rs`: Tantivy schema definition
   - `src/search/query.rs`: Query parsing and execution
   - Index location: `~/.cache/noet/search_index/` or configurable

3. **Integrate with finalize_html** (2 hours)
   - After `export_beliefbase_json()`, index all nodes
   - First build: Full index creation
   - Subsequent builds: Incremental updates (delete + add changed documents)

4. **Add incremental updates** (2 hours)
   - Hook into file watch system
   - On file change event: Extract affected BIDs, delete from index
   - After recompilation: Add updated BIDs to index
   - Batch commits (every 5 seconds or 100 changes)

5. **Expose search API** (2 hours)
   - HTTP: `GET /api/search?q=query&limit=20&offset=0&kind=Document`
   - OR LSP: `noet/search` method with params
   - Return: `{ results: [{ bid, title, snippet, score }], total, took_ms }`

6. **Frontend integration** (1 hour)
   - Update WASM wrapper to fetch from daemon API
   - Graceful fallback if daemon unavailable

**Critical decisions**:
- **Index storage location**: User cache dir or project-local?
- **API protocol**: HTTP or LSP? (LSP fits better with existing architecture)
- **beliefbase.json handling**: Keep exporting at GB-scale, or paginate/skip?

### Decision: Daemon Dependency

**Critical question**: Is daemon/service required for typical workflows?

**Option A: Daemon Required**
- ✅ Enables GB-scale search via Tantivy
- ✅ Real-time incremental updates
- ✅ LSP integration for editor support
- ⚠️ Adds deployment complexity (must run daemon)
- ⚠️ Static HTML generation alone can't search

**Option B: Static-Only (No Search)**
- ✅ Simple deployment (just HTML files)
- ✅ Works without daemon
- ❌ No search capability at GB-scale
- ⚠️ Users must search via `grep` or IDE

**Option C: Hybrid**
- Small knowledge bases (< 10MB): MiniSearch (client-side)
- Large knowledge bases (> 10MB): Tantivy via daemon
- Complexity: Maintain two search implementations

**Recommendation**: Choose Option A (Daemon Required) if:
- Users already run daemon for watch/LSP features
- Real-time search is valuable
- GB-scale is typical use case

### Integration with ISSUE_41

Text search should be **separate from structured query builder**:
- **Query builder**: Filter by kind, schema, network (structured data via `bb.query()`)
- **Text search**: Keyword search with stemming (Tantivy full-text search)
- **Combined**: "Advanced" mode allows both: `query_text + filter(kind=Document, schema=Task)`

**Implementation**:
- Query builder UI uses existing WASM `query()` API
- Text search UI calls daemon `/api/search` endpoint
- "Advanced" tab combines both (text search results filtered by query builder criteria)

---

## References

- Tantivy: https://github.com/quickwit-oss/tantivy
- Sonic: https://github.com/valeriansaliou/sonic
- MiniSearch: https://github.com/lucaong/minisearch
- Stork: https://github.com/jameslittle230/stork
- rust-stemmers: https://github.com/CurrySoftware/rust-stemmers
- ISSUE_41: Query Builder UI (may include text search)
- `src/codec/compiler.rs`: finalize_html() integration point