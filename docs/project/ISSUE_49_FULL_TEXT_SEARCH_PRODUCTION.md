# Issue 49: Full-Text Search Production - Scalable Service

**Priority**: HIGH
**Estimated Effort**: 8-12 days
**Dependencies**: ISSUE_48 (Full-Text Search MVP) complete
**Version**: 0.2+

## Summary

Scale full-text search to production readiness with GB-level corpus support, daemon integration with incremental updates via event stream, and optional standalone search server for large deployments. Builds on ISSUE_48's per-network indexing architecture.

## Goals

- Scale to GB-level document collections (1000s of documents per network)
- Daemon mode with incremental index updates (no full rebuild on file change)
- Event stream integration for real-time search updates
- Performance benchmarking and optimization (query latency, index build time, memory)
- Optional standalone search server for large static deployments
- Docker container with HTTP API for remote search
- Production deployment guide with monitoring and tuning

## Architecture

### Event-Driven Incremental Updates

```
File Change → Parser → BeliefEvent stream
                           ↓
                    SearchIndexer subscribes
                           ↓
                NodeUpdate → Reindex document
                NodesRemoved → Delete from index
                PathAdded → Update path metadata
                           ↓
                    Commit (batched)
```

**Key events**:
- `NodeUpdate(keys, node_toml, origin)` - Reindex this node
- `NodesRemoved(bids, origin)` - Delete from index
- `NodeRenamed(old_bid, new_bid, origin)` - Update BID in index
- `PathAdded/PathUpdate` - Update path field in index

**Batching strategy**:
- Accumulate events for 500ms
- Commit batch once per second (configurable)
- Balance between real-time updates and write amplification

### Deployment Modes

**Mode 1: Daemon Integrated Search**
```
noet daemon (watch + LSP + search)
    ↓
File changes → Event stream → SearchIndexer → Index updates
    ↓
Editor queries → LSP search endpoint → Results
```

**Mode 2: Standalone Search Server**
```
Docker Container:
    Tantivy Indices (read-only or read-write)
    ↓
    HTTP API (:8081/search)
    ↓
    Static SPA (GitHub Pages) ← AJAX requests
```

**Mode 3: CI/CD Build-Time Indexing**
```
CI Pipeline:
    Source → noet compile → Indices artifact
                               ↓
                          Upload to S3/CDN
                               ↓
                          WASM loads from CDN
```

### Enhanced Schema for Performance

**Additional indexed fields** (beyond ISSUE_48):
- `network_title` (STRING, STORED): For display without PathMap lookup
- `doc_path` (STRING, STORED): Full document path (not sub-network relative)
- `parent_bid` (STRING, STORED): Parent section/document BID
- `depth` (U64, INDEXED): Nesting depth for ranking boost
- `word_count` (U64, INDEXED): Content length for snippet selection
- `last_modified` (DATE, INDEXED): For recency ranking
- `cross_ref_count` (U64, INDEXED): Number of incoming links

**Faceted fields**:
- `kind` (FACET): Document/Section/Procedure
- `schema` (FACET): Schema type for filtering
- `network` (FACET): Network bref for filtering

**Performance optimizations**:
- Fast fields for sorting (depth, last_modified, cross_ref_count)
- Stored fields compression (title, content snippets)
- Skip lists for numeric range queries

### HTTP API Design

**Endpoints**:

```
POST /search
{
    "query": "authentication procedure",
    "networks": ["01abc", "02def"],  // Optional: filter by network
    "kind": ["Document", "Procedure"], // Optional: filter by kind
    "schema": "procedure",             // Optional: filter by schema
    "limit": 20,
    "offset": 0,
    "fuzzy": true,                     // Enable fuzzy matching
    "max_edits": 2,                    // Fuzzy match tolerance
    "boost": {
        "depth": 0.1,                  // Boost top-level docs
        "recency": 0.05,               // Boost recent updates
        "cross_refs": 0.15             // Boost highly-linked
    }
}

Response:
{
    "results": [
        {
            "bid": "01234567-89ab-cdef",
            "network": "01abc",
            "network_title": "Security Docs",
            "title": "Authentication Procedure",
            "snippet": "...implement <em>authentication</em> using...",
            "score": 0.95,
            "path": "/security/auth.md",
            "kind": "Procedure",
            "schema": "procedure",
            "depth": 2,
            "last_modified": "2025-01-15T10:30:00Z"
        }
    ],
    "total": 47,
    "took_ms": 23,
    "networks_searched": ["01abc", "02def"]
}

POST /reindex
{
    "network": "01abc",  // Optional: specific network
    "full": false        // false = incremental, true = full rebuild
}

GET /health
{
    "status": "healthy",
    "indices_loaded": 5,
    "total_documents": 1247,
    "index_size_mb": 45.3,
    "uptime_seconds": 3600
}

GET /stats
{
    "networks": [
        {
            "bref": "01abc",
            "title": "Security Docs",
            "document_count": 247,
            "index_size_mb": 12.5,
            "last_updated": "2025-01-15T10:30:00Z"
        }
    ],
    "query_stats": {
        "total_queries": 1523,
        "avg_latency_ms": 18.5,
        "p95_latency_ms": 45.2,
        "cache_hit_rate": 0.73
    }
}
```

### Docker Container Architecture

**Dockerfile**:
```dockerfile
FROM rust:1.75 AS builder
WORKDIR /build
COPY . .
RUN cargo build --release --features service --bin noet-search-server

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates
COPY --from=builder /build/target/release/noet-search-server /usr/local/bin/
EXPOSE 8081
CMD ["noet-search-server", "--config", "/etc/noet/search.toml"]
```

**Configuration** (`search.toml`):
```toml
[server]
host = "0.0.0.0"
port = 8081
max_connections = 100

[index]
base_path = "/var/lib/noet/indices"
mode = "read-write"  # or "read-only"
commit_interval_ms = 1000
ram_buffer_mb = 128
merge_policy = "log"  # or "no_merge" for read-only

[search]
default_limit = 20
max_limit = 100
enable_fuzzy = true
max_fuzzy_edits = 2
snippet_length = 150

[cache]
enabled = true
max_size_mb = 256
ttl_seconds = 300

[cors]
allowed_origins = ["*"]  # Configure in production
allowed_methods = ["GET", "POST"]

[monitoring]
metrics_enabled = true
metrics_port = 9090  # Prometheus endpoint
```

## Implementation Steps

### Phase 1: Event Stream Integration (3-4 days)

#### Step 1.1: SearchIndexer Event Subscriber (2 days)
- [ ] Create `SearchIndexer` struct with event channel receiver
- [ ] Subscribe to `BeliefEvent` stream in daemon mode
- [ ] Handle `NodeUpdate`: Reindex document
  - Extract text from updated node
  - Delete old document by BID
  - Add new document with updated content
- [ ] Handle `NodesRemoved`: Delete documents by BID
- [ ] Handle `NodeRenamed`: Update BID in index (delete + add)
- [ ] Handle `PathAdded/PathUpdate`: Update path field
- [ ] Batch event processing (500ms window)
- [ ] Unit tests for each event type

#### Step 1.2: Commit Strategy (1 day)
- [ ] Implement batched commits (configurable interval)
- [ ] WAL (write-ahead log) for crash recovery
- [ ] Background merge policy configuration
- [ ] Monitor index size growth
- [ ] Test: Rapid file changes don't overwhelm indexer

#### Step 1.3: Daemon Integration (1 day)
- [ ] Add `SearchIndexer` to `WatchService`
- [ ] Connect event stream to indexer
- [ ] Add LSP endpoint for search: `noet/search`
- [ ] Integration test: File change → event → index update → search

### Phase 2: HTTP Search Server (2-3 days)

#### Step 2.1: HTTP API Server (1.5 days)
- [ ] Create `src/bin/noet-search-server.rs`
- [ ] Implement POST `/search` with query parsing
- [ ] Implement POST `/reindex` with auth (API key)
- [ ] Implement GET `/health` and GET `/stats`
- [ ] CORS middleware configuration
- [ ] Request/response logging
- [ ] Error handling and validation
- [ ] API documentation (OpenAPI spec)

#### Step 2.2: Advanced Query Features (1 day)
- [ ] Boolean operators: AND, OR, NOT
- [ ] Field-specific search: `title:auth`, `schema:procedure`
- [ ] Range queries: `last_modified:[2024-01-01 TO 2024-12-31]`
- [ ] Phrase queries: `"exact phrase match"`
- [ ] Boost factors: depth, recency, cross_refs
- [ ] Query parsing tests

#### Step 2.3: Result Caching (0.5 days)
- [ ] LRU cache for frequent queries
- [ ] Configurable TTL and max size
- [ ] Cache invalidation on index updates
- [ ] Cache hit rate metrics

### Phase 3: Performance Optimization (2-3 days)

#### Step 3.1: Index Tuning (1 day)
- [ ] Configure merge policy (LogMergePolicy vs NoMergePolicy)
- [ ] RAM buffer size tuning
- [ ] Commit interval optimization
- [ ] Fast fields for sorting
- [ ] Compression settings for stored fields
- [ ] Benchmark: Index build time for 1GB corpus

#### Step 3.2: Query Optimization (1 day)
- [ ] Query result caching
- [ ] Top-K optimization (early termination)
- [ ] Multi-threaded search across networks
- [ ] Snippet generation optimization
- [ ] Benchmark: Query latency (p50, p95, p99)

#### Step 3.3: Memory Profiling (1 day)
- [ ] Index memory footprint measurement
- [ ] Query memory usage profiling
- [ ] Memory leak detection (long-running server)
- [ ] Document memory usage in production guide
- [ ] Benchmark: Memory usage for 1GB corpus

### Phase 4: Docker & Deployment (2-3 days)

#### Step 4.1: Docker Image (1 day)
- [ ] Create Dockerfile with multi-stage build
- [ ] Configuration file handling (`search.toml`)
- [ ] Volume mounts for indices (`/var/lib/noet/indices`)
- [ ] Health check configuration
- [ ] Docker Compose example (server + frontend)
- [ ] Build and push to Docker Hub

#### Step 4.2: CI/CD Integration (1 day)
- [ ] GitLab CI job: Build indices during compile
- [ ] Upload indices to artifact storage (S3, R2)
- [ ] Deploy Docker container to cloud (fly.io, Railway)
- [ ] Environment-specific configuration
- [ ] Example: GitHub Pages + search server deployment

#### Step 4.3: Monitoring & Observability (1 day)
- [ ] Prometheus metrics endpoint
- [ ] Query latency histograms
- [ ] Index size metrics
- [ ] Cache hit rate metrics
- [ ] Grafana dashboard example
- [ ] Structured logging (JSON format)
- [ ] Alert rules (high latency, low disk space)

### Phase 5: Documentation & Production Guide (1-2 days)

#### Step 5.1: Production Deployment Guide (1 day)
- [ ] README: "Deploying Search Server"
- [ ] Configuration reference (all `search.toml` options)
- [ ] Capacity planning (documents → memory/disk requirements)
- [ ] Scaling strategies (horizontal vs vertical)
- [ ] Security considerations (API keys, CORS, HTTPS)
- [ ] Backup and recovery (index snapshots)
- [ ] Troubleshooting common issues

#### Step 5.2: Performance Tuning Guide (0.5 days)
- [ ] Index optimization strategies
- [ ] Query performance tips
- [ ] Memory management
- [ ] When to use read-only vs read-write mode
- [ ] Merge policy selection

#### Step 5.3: API Documentation (0.5 days)
- [ ] OpenAPI/Swagger specification
- [ ] Client examples (curl, JavaScript, Python)
- [ ] Response format reference
- [ ] Error codes and handling

## Testing Requirements

### Performance Benchmarks

**Index Building**:
- 100 documents: < 1 second
- 1,000 documents: < 10 seconds
- 10,000 documents: < 2 minutes
- 1GB corpus: < 10 minutes

**Query Latency**:
- Simple keyword (100 docs): < 10ms (p95)
- Complex query (1000 docs): < 50ms (p95)
- Fuzzy search (10K docs): < 100ms (p95)
- Multi-network (5 networks): < 150ms (p95)

**Memory Usage**:
- 100 documents: < 10MB
- 1,000 documents: < 50MB
- 10,000 documents: < 500MB
- 1GB corpus: < 2GB

### Integration Tests

- [ ] Daemon mode: File change triggers index update
- [ ] Event stream: NodeUpdate → reindex → search finds update
- [ ] HTTP server: All endpoints return correct responses
- [ ] Multi-network search with result merging
- [ ] Cache invalidation on index updates
- [ ] Concurrent queries don't corrupt index
- [ ] Index recovery after server crash

### Load Tests

- [ ] 100 concurrent queries (measure latency degradation)
- [ ] 1000 queries/second sustained (stress test)
- [ ] Index while querying (write/read contention)
- [ ] Large result sets (10K+ matches)
- [ ] Memory usage under load

### Manual Testing

- [ ] Deploy to staging environment
- [ ] Search quality assessment (relevance, ranking)
- [ ] UI responsiveness with server backend
- [ ] Monitor metrics in Grafana
- [ ] Test backup/restore procedures

## Success Criteria

- [ ] Scales to 1GB corpus (10K+ documents)
- [ ] Query latency < 100ms (p95) for 10K documents
- [ ] Index build time < 10 minutes for 1GB corpus
- [ ] Memory usage < 2GB for 1GB corpus
- [ ] Daemon mode: File change → index update < 1 second
- [ ] HTTP server handles 100 concurrent queries
- [ ] Docker container deploys successfully to cloud
- [ ] Prometheus metrics available and accurate
- [ ] Production deployment guide complete
- [ ] All performance benchmarks documented

## Risks

### Risk 1: Write Amplification with Events
**Impact**: HIGH - Frequent commits may degrade performance
**Likelihood**: MEDIUM
**Mitigation**:
- Batch commits (1 second interval)
- Configurable merge policy
- Monitor write throughput and adjust

### Risk 2: Index Corruption on Crash
**Impact**: HIGH - Corrupted index requires full rebuild
**Likelihood**: LOW (Tantivy has WAL)
**Mitigation**:
- Tantivy's write-ahead log (WAL) for durability
- Regular index snapshots (backup strategy)
- Fast rebuild from event log replay

### Risk 3: Memory Usage Exceeds Budget
**Impact**: MEDIUM - OOM in Docker container
**Likelihood**: MEDIUM
**Mitigation**:
- Memory profiling during development
- Configurable RAM buffer size
- Document memory requirements in capacity planning
- Use read-only mode for static deployments

### Risk 4: CORS Configuration Complexity
**Impact**: LOW - Frontend can't access search API
**Likelihood**: MEDIUM
**Mitigation**:
- Sensible defaults in `search.toml`
- Environment-specific configuration examples
- Clear error messages for CORS failures

### Risk 5: Cold Start Latency
**Impact**: MEDIUM - First query slow after server restart
**Likelihood**: HIGH
**Mitigation**:
- Index preloading on startup
- Warm-up queries during health check
- Document expected cold start time

## Design Decisions

### Decision 1: Event Stream for Incremental Updates
**Rationale**: Enables real-time search in daemon mode
**Trade-offs**: More complex than full rebuild, but essential for UX
**Alternatives Considered**: Polling file system - rejected (inefficient)

### Decision 2: Batched Commits
**Rationale**: Balance between real-time updates and write performance
**Trade-offs**: 1-second lag vs continuous commits
**Configuration**: Tunable via `commit_interval_ms`

### Decision 3: Read-Write vs Read-Only Modes
**Rationale**: Different use cases (daemon vs CI/CD)
**Trade-offs**: Read-only is faster but can't update
**Guidance**: Read-write for daemon, read-only for static deployments

### Decision 4: HTTP API Over LSP Extension
**Rationale**: Broader compatibility (not just editors)
**Trade-offs**: More deployment complexity, but more flexible
**Both Available**: LSP for daemon, HTTP for remote

### Decision 5: Docker for Production
**Rationale**: Standard deployment, easy scaling
**Trade-offs**: Container overhead, but worth it for portability
**Alternative**: Systemd service for bare metal deployments

## References

- Tantivy performance guide: https://docs.rs/tantivy/latest/tantivy/
- Event-driven indexing patterns
- Docker best practices
- Prometheus metrics design
- ISSUE_48: Full-Text Search MVP (foundation)
- `docs/design/beliefbase_architecture.md`: Event system
- `.scratchpad/search_architecture_review.md`: Architecture decisions

## Notes

- Builds on ISSUE_48's per-network indexing architecture
- Event stream integration is critical for daemon mode
- Performance benchmarks guide production capacity planning
- Docker deployment optional but recommended for large deployments
- Future work: Distributed search (multiple servers), replication, advanced ranking models

## Future Enhancements (Backlog)

- Boolean operators in query syntax (AND, OR, NOT)
- Advanced ranking signals (PageRank for cross-references)
- Integration with query.rs for graph traversal + search
- Distributed search across multiple servers
- Index replication for high availability
- Machine learning ranking models
- Search analytics and query logging
- Autocomplete/suggestions API
- Related documents recommendation