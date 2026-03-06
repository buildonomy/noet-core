# Issue 54: Full-Text Search MVP — Compile-Time Search Indices

**Priority**: HIGH
**Estimated Effort**: 4–5 days
**Dependencies**: Issue 50 (BeliefBase Sharding) — establishes export infrastructure, `.idx.json` generation, and viewer UI that search layers onto

## Summary

Implement full-text search for the interactive HTML viewer using compile-time per-network search indices (`.idx.json`). The compiler builds lightweight inverted indices during `finalize_html` — always, regardless of whether the export is monolithic or sharded. The WASM side only deserializes these pre-built indices and runs TF-IDF queries against them. Zero index construction in the browser, zero additional WASM binary size, zero WASM compilation risk. Search covers the *entire* corpus from the moment the viewer loads, including networks whose data shards haven't been loaded.

## Goals

- Full-text search across **all** networks immediately on viewer init (not just loaded ones)
- Compile-time `.idx.json` per network: `term → [(bid, frequency)]` + minimal doc metadata (title, path), ~3-5% of shard size
- TF-IDF ranking with title boost (title terms weighted 3x)
- Keyword search with fuzzy matching (Levenshtein ≤ 2) for typo tolerance
- Snippet extraction from loaded data via simple string search (not an index operation)
- Results from unloaded networks show title + path but no snippet, with `loaded: false` flag
- Click result from unloaded network → prompt to load shard → navigate
- No runtime index building in WASM — all indexing happens at compile time in native Rust
- No new WASM dependencies — zero binary size increase

## Architecture

See `docs/design/search_and_sharding.md` §7 for the full search design.

**Key architectural decisions:**

1. **All indexing at compile time.** The `.idx.json` files are built during `finalize_html` in native Rust. The WASM module only deserializes and queries — no `SearchIndex::build()`, no dirty flags, no runtime index construction. The tokenizer runs once at compile time, not twice.

2. **`.idx.json` always generated.** Even in monolithic mode (below sharding threshold), the `search/*.idx.json` files are written. This means the WASM search path is identical regardless of export mode — always deserialize `.idx.json`, always query the same data structure.

3. **Index generation lives in Issue 50's export pipeline.** The `.idx.json` files are built alongside the BB export — same iteration over `global_bb` per network, different output format. Issue 50 generates them; Issue 54 consumes them.

4. **Snippets via string search, not index lookup.** For loaded networks, snippets are extracted by finding query terms in `BeliefNode.payload["text"]` — a simple string search, not an inverted index operation. For unloaded networks, the snippet field is empty.

5. **IDF spans full corpus.** Each `.idx.json` includes `doc_count`. Combined across all loaded search indices, the IDF calculation reflects the entire corpus, giving accurate term-rarity scoring even when most data shards are unloaded.

### What this trades away vs. Tantivy

| Feature | Tantivy | Built-in | Impact |
|---|---|---|---|
| BM25 ranking | ✓ | Simple TF-IDF | LOW — docs are short, ranking difference is minimal |
| Stemming | ✓ (configurable) | Deferred (add `rust-stemmers` ~50KB later) | MEDIUM — "running" won't match "run" initially |
| Fuzzy matching | ✓ (Levenshtein) | Implemented directly (~50 lines) | LOW |
| Snippet generation | ✓ (all results) | Loaded networks only (simple string search) | LOW — unloaded results still show title + path |
| WASM binary size | +2-4MB | +0 | Significant win |
| WASM compile risk | HIGH | None | **Major win** — eliminates biggest risk |

## Implementation Steps

### Phase 1: Core Search Module (1.5 days)

#### Step 1.1: Tokenizer and Data Structures (0.5 days)
- [ ] Create `src/search/mod.rs` with `CompactSearchIndex` struct (deserialized from `.idx.json`)
- [ ] `CompactSearchIndex`: `doc_count: usize`, `docs: BTreeMap<Bid, DocStub>`, `index: BTreeMap<String, Vec<(Bid, u32)>>`
- [ ] `DocStub`: `{ title: String, path: String, term_count: u32 }` — minimal metadata for displaying a search result
- [ ] Implement `tokenize(text) -> Vec<String>`: split on whitespace/punctuation, lowercase
- [ ] This tokenizer is the single source of truth — used by the compiler (index build) and WASM (query parsing)
- [ ] Unit tests for tokenizer with markdown content, edge cases (empty, unicode, punctuation)

#### Step 1.2: Index Generation (compile-time, native Rust) (0.5 days)
- [ ] Implement `build_search_index(global_bb, pathmap, network_bref) -> CompactSearchIndex`
- [ ] Query `global_bb` per network using `Expression::StateIn(StatePred::InNamespace(bref))`
- [ ] Tokenize `title` (3x weight baked into frequency) and `payload["text"]` (1x weight) per node
- [ ] Record `(bid, title, path, term_count)` in `docs` map
- [ ] Populate `index` map: `term → [(bid, frequency)]`
- [ ] Serialize to JSON matching the `.idx.json` format
- [ ] Unit tests: correct terms, frequencies, title weighting, missing `payload["text"]` handled

#### Step 1.3: TF-IDF Query and Fuzzy Matching (0.5 days)
- [ ] Implement `CompactSearchIndex::search(query_terms, limit) -> Vec<SearchHit>` with TF-IDF scoring
- [ ] Multi-index search: accept `&[CompactSearchIndex]`, compute IDF across total `doc_count` from all indices
- [ ] Implement fuzzy matching: for queries < 20 characters, also match terms within Levenshtein distance ≤ 2
- [ ] Unit tests: keyword search, multi-term ranking, fuzzy matching, empty query, cross-network ranking

### Phase 2: Export Pipeline Integration (0.5 days)

*This step integrates into Issue 50's export pipeline. Listed here because the format and tokenizer must be consistent.*

- [ ] Add `build_search_indices()` call to `DocumentCompiler::finalize_html()` — runs ALWAYS (monolithic and sharded)
- [ ] Write `search/{bref}.idx.json` per network
- [ ] Write `search/manifest.json` listing all `.idx.json` files with `bref`, `title`, `path`, `sizeKB`
- [ ] Integration test: verify `.idx.json` files produced for both monolithic and sharded repos
- [ ] Integration test: verify `search/manifest.json` lists all networks

### Phase 3: WASM Bindings (0.5 days)

- [ ] Add `load_search_index(bref, idx_json)` to `BeliefBaseWasm` — deserialize `.idx.json` into `CompactSearchIndex`, store in internal map
- [ ] Replace existing `BeliefBaseWasm::search()` (title/id substring) with TF-IDF search across loaded `CompactSearchIndex` instances
- [ ] Add `search_in_network(bref, query, limit)` for network-scoped search
- [ ] Snippet extraction: for results where the network's data shard is loaded, find query terms in `BeliefNode.payload["text"]` via simple string search, return ~100 char window
- [ ] Tag each result with `loaded: bool` based on whether the network's data shard is in memory
- [ ] Return plain JS objects (not Maps)
- [ ] WASM tests: search returns results from all networks (loaded + unloaded), snippets only for loaded

### Phase 4: Viewer Search UI (1 day)

- [ ] Update `ShardManager` init to fetch `search/manifest.json`, then all `.idx.json` files, call `load_search_index()` per network
- [ ] Search input in viewer header with debounced input (300ms)
- [ ] Results panel: title, snippet (if data loaded), network name, path, loaded indicator
- [ ] Results from unloaded networks show title + path but no snippet, with "load to see full content" hint
- [ ] Click result from loaded network → navigate to document
- [ ] Click result from unloaded network → prompt with estimated shard size, load on confirm, then navigate
- [ ] Network filter: search within selected networks only
- [ ] CSS styling consistent with existing viewer theme

### Phase 5: Documentation and Testing (0.5 days)

- [ ] README section: "Full-Text Search" — usage, search syntax, network filtering, unloaded results
- [ ] Integration test: monolithic export → `.idx.json` files generated → search works
- [ ] Integration test: sharded export → `.idx.json` files generated → search finds results from unloaded networks
- [ ] Integration test: load data shard → results gain snippets → unload → snippets gone, results still present
- [ ] Integration test: fuzzy search finds results with 1–2 character typos
- [ ] Performance test: search latency across 10 networks with 100 docs each

## Testing Requirements

- Tokenizer: whitespace splitting, punctuation handling, lowercase, unicode, empty input
- Index generation: correct term frequencies, title weighting (3x), docs map has title + path + term_count
- Index generation: missing `payload["text"]` produces index with title terms only (no crash)
- Index generation: runs for both monolithic and sharded export modes
- TF-IDF ranking: title matches rank above body matches, multi-term queries
- Cross-network ranking: IDF uses global doc_count across all loaded indices
- Fuzzy matching: Levenshtein distance 1 and 2, disabled for long queries
- Snippet extraction: correct window around match (loaded only), empty string for unloaded
- `loaded` flag: results correctly tagged based on data shard state
- Backward compat: old outputs without `search/` directory fall back to existing substring search
- Manual: search UI responsiveness, result relevance, unloaded result UX, 100+ document sharded repo

## Success Criteria

- [ ] Search returns results from **all** networks immediately after viewer init
- [ ] `.idx.json` files generated for every network in both monolithic and sharded modes
- [ ] Search indices are ~3-5% the size of their corresponding data
- [ ] Fuzzy search finds results with 1–2 character typos
- [ ] Search latency < 200ms for typical queries across 10 networks
- [ ] Results from loaded networks include content snippets
- [ ] Results from unloaded networks show title + path with `loaded: false`
- [ ] Clicking unloaded result prompts shard load (with size), then navigates
- [ ] Old outputs without `search/` directory still work (substring search fallback)
- [ ] No new WASM dependencies — zero binary size increase for search
- [ ] No index building in WASM — all indexing is compile-time

## Risks

### Risk 1: Search Quality Without Stemming
**Impact**: MEDIUM — "running" won't match "run", reducing recall
**Likelihood**: HIGH — stemming genuinely helps for English text
**Mitigation**: Accept for MVP. Adding `rust-stemmers` (~50KB) is a backward-compatible enhancement — add to the tokenizer, regenerate `.idx.json` files, done. Since the tokenizer only runs at compile time, there is no WASM-side change needed.

### Risk 2: Index Staleness in Daemon Mode
**Impact**: LOW — search indices may be slightly stale between `finalize_html` passes
**Likelihood**: MEDIUM — daemon rebuilds periodically but not on every keystroke
**Mitigation**: Acceptable for the development workflow. The user is typically navigating to the document they just edited, not searching for it. Indices are rebuilt on each `noet parse` or `finalize_html` cycle.

### Risk 3: Eager Index Loading for Large Repos
**Impact**: LOW — init fetches many small files for repos with 100+ networks
**Likelihood**: LOW — most repos have < 20 networks
**Mitigation**: For the MVP, load all indices eagerly (~200KB for 10 networks). A backlog item (see Issue 49) provides lazy loading: search only loaded networks by default, fetch unloaded indices on demand via "search all networks" interaction.

## Design Decisions

| Decision | Rationale |
|----------|-----------|
| Compile-time only — no runtime index building | Eliminates an entire code path from WASM. One tokenizer invocation at compile time, not two. Simpler binary, simpler mental model. |
| Always generate `.idx.json` (even monolithic) | Identical search path regardless of export mode. No conditional "build index in WASM if monolithic" branch. |
| `search/` directory separate from `beliefbase/` | Search indices are always generated; data shards are conditional on size. Separate directories reflect this. |
| Snippets via string search, not index | The inverted index tells us *which* documents match. Finding *where* in the text the match occurs for a snippet is a linear scan of one document — fast enough (~microseconds) and avoids storing term positions in the index. |
| `loaded` flag on SearchResult | Lets the viewer UI distinguish loaded vs unloaded results without checking shard state separately. |
| Eager load all search indices | ~200KB total for 10 networks. One batch of fetches on init buys full-corpus search. Lazy alternative is a backlog item. |
| Replace existing `search()` | The existing `search()` does substring matching on title/id. TF-IDF over compile-time indices strictly supersedes it. One API, not two. |

## References

- `docs/design/search_and_sharding.md` §7 — Compile-time search architecture specification
- `docs/design/beliefbase_architecture.md` §3.4 — BeliefGraph vs BeliefBase
- `docs/design/interactive_viewer.md` — Viewer architecture and WASM integration
- `src/codec/compiler.rs::finalize_html()` — Integration point for index generation
- `src/wasm.rs::BeliefBaseWasm::search()` — Current search implementation to replace
- `src/properties.rs::BeliefNode` — Struct with `title`, `payload["text"]`, `kind`, `schema`, `id`
- Issue 50: BeliefBase Sharding (prerequisite — provides export pipeline and `.idx.json` generation)
- Issue 47: Performance Profiling (provides scale-sized test fixtures for search validation)
- Issue 49: Search Feature Backlog (post-MVP enhancements: stemming, boolean queries, lazy index loading)