# Issue 46: Full-Text Search with Tantivy

**STATUS**: SUPERSEDED - Split into ISSUE_48 (MVP) and ISSUE_49 (Production)

**Priority**: HIGH
**Estimated Effort**: 16-24 days (SPLIT: 5-7 days MVP + 8-12 days Production)
**Dependencies**: None (integrates with existing daemon/service architecture)
**Version**: 0.1

---

## Superseded Notice

This issue has been split into two focused issues:

1. **ISSUE_48: Full-Text Search MVP - Embedded WASM** (5-7 days)
   - Per-network indexing architecture
   - Embedded Tantivy in WASM for static HTML output
   - Keyword + fuzzy search
   - Memory budget management (50/50 BeliefBase + search)
   - Works with `noet parse` and `noet watch`

2. **ISSUE_49: Full-Text Search Production - Scalable Service** (8-12 days)
   - Event stream integration for incremental updates
   - Daemon mode integration
   - HTTP API with Docker container
   - GB-scale performance benchmarking
   - Production deployment guide

**Rationale for split**: 
- MVP establishes per-network indexing architecture (critical foundation)
- Production adds scaling, daemon integration, and deployment
- Allows earlier delivery of basic search functionality
- Cleaner separation of concerns (embedded vs. service)

See `.scratchpad/search_architecture_review.md` for architectural deep dive.

---

## Original Issue Content (For Reference)

## Summary (Original)

Implement full-text search capability using Tantivy to support both local daemon (editor workflow) and CI/CD deployment (GitHub Pages + search container) use cases. The system must scale to GB-level document collections with incremental indexing, stemming, and rich query features.

**Note**: This has been refined and split into ISSUE_48 (MVP) and ISSUE_49 (Production) based on architectural review.

## Goals

- Full-text search with stemming/lemmatization across all document content
- Scalable to GB-level document collections (1000s of documents)
- Support two deployment modes:
  1. **Local daemon**: Integrated search for editor workflow
  2. **CI/CD**: Standalone search container for static site deployment
- Incremental indexing on file changes (no full rebuild)
- Rich query features: boolean operators, fuzzy search, filtering by kind/schema
- Fast search: < 100ms even at GB-scale
- BID-based results for precise link generation

## Architecture

### Two Deployment Modes

**Mode 1: Local Daemon (Editor Workflow)**
```
Local Files → noet daemon (watch + LSP + search) → Tantivy Index (local)
                    ↓
            Editor + Browser (http://localhost:8080/api/search)
```

**Mode 2: CI/CD (GitHub Pages + Container)**
```
CI/CD Pipeline:
  Source → noet compile → Tantivy Index Artifact
                       ↓                    ↓
                  Static SPA          Search Container
                 (GitHub Pages)      (https://search.example.com)
                       ↓                    ↓
                  Browser ←─────AJAX────────┘
```

### Tantivy Schema

**Indexed Fields**:
- `bid` (STRING, STORED): Stable identifier for results
- `title` (TEXT, STORED): Document/section title
- `content` (TEXT): Extracted from payload (body text, metadata)
- `kind` (STRING, STORED): BeliefKind for filtering
- `schema` (STRING, STORED): Optional schema for filtering
- `id` (STRING, STORED): Optional semantic identifier

**Query Capabilities**:
- Text search with stemming: "running shoes" → matches "run", "shoe"
- Boolean operators: `"machine learning" AND (tutorial OR guide)`
- Fuzzy search: `~0.8` tolerance for typos
- Filters: `kind:Document`, `schema:Task`
- Relevance ranking with BM25

### Search API

**HTTP Endpoint**: `GET /api/search`

**Query Parameters**:
- `q` (required): Search query text
- `kind` (optional): Filter by BeliefKind (Document, Section, etc.)
- `schema` (optional): Filter by schema name
- `limit` (optional, default 20): Results per page
- `offset` (optional, default 0): Pagination offset

**Response**:
```json
{
  "results": [
    {
      "bid": "01234567-89ab-cdef-0123-456789abcdef",
      "title": "Getting Started Guide",
      "snippet": "...learn how to <em>run</em> the application...",
      "score": 0.87,
      "kind": "Document",
      "schema": null
    }
  ],
  "total": 142,
  "took_ms": 23
}
```

## Implementation Steps

### Phase 1: Core Tantivy Integration (8-12 days)

#### Step 1.1: Add Dependencies (0.5 days)
- [ ] Add `tantivy = "0.22"` to `[dependencies]` (optional, service feature)
- [ ] Update `[features]` service line to include `"tantivy"`
- [ ] Verify compilation with `cargo build --features service`

#### Step 1.2: Create Search Module Structure (1 day)
- [ ] Create `src/search/mod.rs` with public API
- [ ] Create `src/search/schema.rs` for Tantivy schema definition
- [ ] Create `src/search/indexer.rs` for index building logic
- [ ] Create `src/search/query.rs` for query parsing and execution
- [ ] Create `src/search/server.rs` for HTTP API
- [ ] Add module declaration in `src/lib.rs`: `#[cfg(feature = "service")] pub mod search;`

#### Step 1.3: Implement Tantivy Schema (1 day)
- [ ] Define `create_schema()` function with fields: bid, title, content, kind, schema, id
- [ ] Configure tokenizers: English stemmer, lowercase filter, stop words
- [ ] Add schema validation tests

#### Step 1.4: Implement SearchIndexer (3 days)
- [ ] `SearchIndexer::new(index_path)`: Initialize or open existing index
- [ ] `index_graph(&mut self, graph: &BeliefGraph)`: Full index build from BeliefGraph
- [ ] `delete_documents(&mut self, bids: &[Bid])`: Remove documents by BID
- [ ] `add_documents(&mut self, nodes: &[BeliefNode])`: Add/update documents
- [ ] `commit(&mut self)`: Commit changes to disk
- [ ] Extract text from `payload` fields (handle TOML table structure)
- [ ] Add unit tests with small BeliefGraph samples

#### Step 1.5: Implement Query API (2 days)
- [ ] Define `SearchQuery` struct with text, kind, schema, limit, offset
- [ ] Define `SearchResult` struct with bid, title, snippet, score
- [ ] `search(&self, query: &SearchQuery) -> Result<Vec<SearchResult>>`: Execute search
- [ ] Parse boolean operators: AND, OR, NOT
- [ ] Apply filters for kind and schema
- [ ] Generate snippets with highlighted matches (HTML `<em>` tags)
- [ ] Add query parsing tests

#### Step 1.6: Implement HTTP Server (2 days)
- [ ] Create `serve_search_api(index_path, addr)` using Axum
- [ ] `GET /api/search` handler with query parameter parsing
- [ ] CORS headers for cross-origin requests (CI/CD mode)
- [ ] Error handling and logging
- [ ] Health check endpoint: `GET /api/health`
- [ ] Integration tests with test index

### Phase 2: Daemon Integration (3-4 days)

#### Step 2.1: Integrate into finalize_html (1 day)
- [ ] Add `search_index_path: Option<PathBuf>` to `DocumentCompiler` config
- [ ] After `export_beliefbase_json()`, call `build_search_index(graph, index_path)`
- [ ] Create `build_search_index()` method that initializes SearchIndexer
- [ ] Index all nodes from BeliefGraph
- [ ] Log indexing statistics (documents indexed, time taken, index size)

#### Step 2.2: Daemon Search Service (1 day)
- [ ] Add `--enable-search` flag to daemon CLI
- [ ] Default index location: `~/.cache/noet/search_index/` or project-local `.noet/search_index/`
- [ ] Start HTTP search server on daemon startup (default port 8080)
- [ ] Add search endpoint to daemon's existing Axum server (if present)
- [ ] Graceful shutdown handling (commit pending index changes)

#### Step 2.3: Incremental Updates on File Change (2 days)
- [ ] Hook into existing file watch system (`notify-debouncer-full`)
- [ ] On file change: Determine affected BIDs from file path
- [ ] Before recompilation: Delete affected BIDs from search index
- [ ] After recompilation: Extract updated BeliefNodes
- [ ] Add updated nodes to search index
- [ ] Batch commits: Commit after 5 seconds idle or 100 changes
- [ ] Test with rapid file changes (ensure no index corruption)

### Phase 3: CI/CD Support (4-6 days)

#### Step 3.1: Build-Time Index Generation (2 days)
- [ ] Add CLI flags to `noet compile`:
  - `--build-search-index`: Enable search index generation
  - `--search-index-output <PATH>`: Where to write index (default: `./search_index`)
  - `--search-api-url <URL>`: Search API endpoint for SPA config
- [ ] Generate index from exported BeliefGraph
- [ ] Write index to specified output directory
- [ ] Support incremental builds (reuse existing index if available)

#### Step 3.2: Configuration Injection (1 day)
- [ ] Define `NoetConfig` struct with `search_api_url: Option<String>`
- [ ] Write `noet_config.json` to HTML output directory during `finalize_html()`
- [ ] Include search API URL from CLI flag or env var
- [ ] Document environment variable: `NOET_SEARCH_API_URL`

#### Step 3.3: Standalone Search Server Binary (1 day)
- [ ] Create `src/bin/noet-search-server.rs`
- [ ] CLI arguments: `--index-path <PATH>`, `--addr <IP:PORT>`
- [ ] Read-only mode (no indexing, just serving)
- [ ] Graceful shutdown on SIGTERM/SIGINT
- [ ] Logging configuration (tracing-subscriber)

#### Step 3.4: Docker Image (2 days)
- [ ] Create `Dockerfile.search-server` with multi-stage build
- [ ] Build `noet-search-server` binary in builder stage
- [ ] Copy binary and index to slim runtime image (debian:bookworm-slim)
- [ ] Expose port 8080
- [ ] Health check configuration
- [ ] Create `docker-compose.yml` example for local testing
- [ ] Document deployment to Kubernetes, AWS ECS, GCP Cloud Run

### Phase 4: Frontend Integration (2-3 days)

#### Step 4.1: SPA Search UI (1.5 days)
- [ ] Load `noet_config.json` on SPA initialization
- [ ] Determine search API URL (from config or default to localhost:8080)
- [ ] Add search input to header or sidebar
- [ ] Implement search results display with snippets
- [ ] Click result → navigate to BID (existing metadata panel integration)
- [ ] Loading indicator during search
- [ ] Error handling (API unreachable, timeout)

#### Step 4.2: WASM Wrapper Enhancement (0.5 days)
- [ ] Add `search_via_api(query, api_url)` method to BeliefBaseWasm
- [ ] Fetch from search API, parse JSON response
- [ ] Fallback to existing `search()` method if API unavailable
- [ ] Expose to JavaScript for SPA consumption

#### Step 4.3: Integration with ISSUE_41 Query Builder (1 day)
- [ ] Add "Text Search" tab to query builder UI
- [ ] Separate from structured query (by kind, schema, network)
- [ ] "Advanced" mode: Combine text search + filters
- [ ] Example: `"machine learning" + kind:Document + schema:Tutorial`
- [ ] Display results in unified results panel

### Phase 5: Documentation & Testing (2-3 days)

#### Step 5.1: User Documentation (1 day)
- [ ] Update `docs/design/` with search architecture
- [ ] Create deployment guide: `docs/SEARCH_DEPLOYMENT.md`
- [ ] Local daemon setup instructions
- [ ] CI/CD pipeline examples (GitHub Actions, GitLab CI)
- [ ] Docker deployment guide (Compose, Kubernetes)
- [ ] Configuration options reference
- [ ] Troubleshooting section

#### Step 5.2: Integration Tests (1 day)
- [ ] Test full indexing workflow with sample knowledge base
- [ ] Test incremental updates (add, modify, delete documents)
- [ ] Test search queries with various operators
- [ ] Test filtering by kind and schema
- [ ] Test pagination (offset and limit)
- [ ] Test with GB-scale data (performance benchmarks)

#### Step 5.3: CI/CD Example (1 day)
- [ ] Create `.github/workflows/search-deploy.yml` example
- [ ] Example Dockerfile for search server
- [ ] Example Kubernetes manifests (`search-deployment.yaml`, `search-service.yaml`)
- [ ] Example `docker-compose.yml` for local testing
- [ ] Document CORS configuration for cross-origin requests

## Testing Requirements

### Unit Tests
- [ ] Tantivy schema creation and validation
- [ ] Indexer: add, delete, commit operations
- [ ] Query parser: boolean operators, filters, fuzzy search
- [ ] Text extraction from BeliefNode payload
- [ ] Snippet generation with highlighting

### Integration Tests
- [ ] Full indexing from BeliefGraph
- [ ] Search API HTTP endpoints
- [ ] Incremental updates on file change
- [ ] Index persistence and reload
- [ ] GB-scale performance (10k+ documents)

### Manual Testing
- [ ] Local daemon with live file watching
- [ ] Search via browser (localhost)
- [ ] CI/CD pipeline (build → deploy → search)
- [ ] Docker container deployment
- [ ] Cross-origin requests from static site

## Success Criteria

- [ ] Tantivy integration compiles with `service` feature
- [ ] Full indexing of BeliefGraph completes successfully
- [ ] Search returns relevant results with snippets
- [ ] Stemming works correctly ("running" matches "run")
- [ ] Boolean operators and filters work as expected
- [ ] Incremental updates on file change (< 1 second latency)
- [ ] Search API responds in < 100ms for typical queries
- [ ] Local daemon mode works out-of-box
- [ ] CI/CD build generates search index artifact
- [ ] Docker container serves search API successfully
- [ ] SPA integrates with search API (both local and remote)
- [ ] All automated tests pass
- [ ] Documentation complete and tested

## Risks

### Risk 1: Tantivy Learning Curve
**Problem**: Team unfamiliar with Tantivy API, indexing concepts

**Mitigation**:
- Start with simple schema, add features incrementally
- Refer to Tantivy examples and documentation
- Use existing Quickwit integration as reference

### Risk 2: GB-Scale Performance
**Problem**: Indexing or search may be slow at GB-scale

**Mitigation**:
- Benchmark early with realistic data
- Use Tantivy's memory-mapped I/O (default)
- Batch commits for incremental updates
- Add query timeout (10 seconds)
- Profile with `cargo flamegraph` if needed

### Risk 3: Docker Image Size
**Problem**: Search container may be large (Rust binary + index)

**Mitigation**:
- Use multi-stage build (builder + slim runtime)
- Strip binary symbols: `cargo build --release`
- Compress index with `gzip` or `zstd`
- Document expected size (binary ~10-20 MB, index variable)

### Risk 4: CORS and Security
**Problem**: Cross-origin requests from static site to search API

**Mitigation**:
- Configure CORS headers in search server
- Allow configurable origins via env var
- Document security considerations
- Consider API key authentication for production (future enhancement)

### Risk 5: Index Corruption on Crash
**Problem**: Daemon crash during indexing may corrupt index

**Mitigation**:
- Tantivy uses write-ahead log (WAL) for durability
- Test crash recovery (kill -9 daemon during indexing)
- Document recovery process (delete index, rebuild)
- Add `--rebuild-index` flag for manual recovery

## Design Decisions

### Decision 1: Tantivy vs Alternatives
**Decision**: Use Tantivy for both local and CI/CD deployments

**Rationale**:
- Only Rust-native option that scales to GB
- Single implementation for both use cases
- Production-proven (Quickwit, Meilisearch uses Tantivy fork)
- Active development and maintenance
- Rich query features out-of-box

**Alternatives Considered**:
- MiniSearch: Client-side only, fails at GB-scale
- Stork: No incremental updates, static-only
- Sonic: In-memory, doesn't scale to GB
- Custom: Months of work to match Tantivy features

### Decision 2: HTTP API vs LSP Protocol
**Decision**: Use HTTP API for search queries

**Rationale**:
- Works for both local daemon and remote container
- Simple to call from browser (fetch API)
- Standard tooling (curl, Postman for testing)
- CORS support for cross-origin requests

**LSP Integration** (future):
- Can add LSP `noet/search` method later
- Maps to same HTTP endpoint internally
- Useful for editor integration

### Decision 3: Index Storage Location
**Decision**: 
- Local daemon: `~/.cache/noet/search_index/` (per-project subdirectory)
- CI/CD: User-specified path (default `./search_index`)

**Rationale**:
- User cache dir follows XDG spec (Linux), platform conventions
- Per-project subdirectory avoids collisions
- CI/CD needs explicit artifact path for deployment
- Configurable via CLI flag

### Decision 4: Configuration Injection
**Decision**: Write `noet_config.json` to HTML output directory

**Rationale**:
- Simple, no build-time JS manipulation
- SPA fetches config at runtime
- Easy to override via env var or CLI flag
- Supports multiple deployment environments (dev, staging, prod)

### Decision 5: Read-Only vs Read-Write Container
**Decision**: Search container is read-only (no indexing)

**Rationale**:
- CI/CD generates index as build artifact
- Container immutability (12-factor app)
- Simpler deployment (no persistence volume needed)
- Re-deploy container to update index

**Future Enhancement**: Support read-write mode for live indexing

## References

- **Trade Study**: `docs/project/trades/TEXT_SEARCH_INDEXER.md` - Comparison of search options
- **Tantivy**: https://github.com/quickwit-oss/tantivy
- **BeliefBase Architecture**: `docs/design/beliefbase_architecture.md` - § 3.4 BeliefGraph export
- **Compiler**: `src/codec/compiler.rs::finalize_html()` - Integration point
- **ISSUE_41**: Query Builder UI (text search integration)
- **ISSUE_39**: Interactive viewer (SPA frontend)

## Notes

**Phase Dependencies**:
- Phase 1 (Core) must complete before Phase 2 (Daemon)
- Phase 3 (CI/CD) can start after Phase 1
- Phase 4 (Frontend) depends on Phase 2 or 3 API availability

**Parallel Work**:
- Backend (Phases 1-3) and Frontend (Phase 4) can be developed in parallel
- Documentation (Phase 5) can be written alongside implementation

**Future Enhancements** (deferred):
- Incremental indexing in CI/CD (reuse previous index)
- Advanced query syntax (regex, proximity search)
- Search analytics (popular queries, click-through)
- Multi-language stemming configuration
- API key authentication for production
- Rate limiting and caching