# Issue 49: Search Feature Backlog — Future Enhancements

**Status**: BACKLOG — brainstorm of post-MVP search enhancements
**Priority**: LOW
**Dependencies**: Issue 54 (Full-Text Search MVP) complete

## Context

This issue originally proposed a "Full-Text Search Production" system with three capabilities: event-driven incremental Tantivy index updates, enhanced query features, and a standalone HTTP search server. A design review determined that the unified search architecture (Issue 54 — compile-time per-network search indices deserialized by `BeliefBaseWasm`) eliminates most of this scope:

1. **Event-driven incremental updates** — eliminated. Under the compile-time search model, search indices are rebuilt during `finalize_html` — not at runtime in WASM. No `SearchIndexer` component, no dirty flags, no lazy rebuild mechanism needed in the browser.

2. **HTTP search server** — replaced by federated data access. A search-only server solves one query type while leaving navigation, metadata, and context broken for unloaded networks. The correct abstraction is a remote `BeliefSource` (see `federated_belief_network.md` §3.6) that serves all queries — search, navigation, context — for data not loaded locally.

3. **Enhanced query features** — retained as backlog items below.

See `.scratchpad/unified_search_analysis.md` (if present) for the full analysis.

## Backlog: Enhanced Query Features

These are incremental improvements to `BeliefBase::full_text_search()` that can be added after Issue 54's MVP ships. None require architectural changes.

### Stemming Support

Add English stemming via `rust-stemmers` (~50KB crate) so that "running" matches "run", "documents" matches "document", etc.

- Add `rust-stemmers` as an optional dependency
- Insert stemming step into the tokenizer pipeline (after lowercase, before index insertion)
- Stem both index terms and query terms for consistent matching
- Estimated effort: 0.5 days

### Boolean Query Operators

Parse `AND`, `OR`, `NOT` in query strings for precise filtering.

- `authentication AND oauth` — both terms required
- `authentication OR authorization` — either term
- `authentication NOT basic` — exclude results containing "basic"
- Requires a simple query parser (split on operators, intersect/union/subtract result sets)
- Estimated effort: 1 day

### Field-Specific Search

Allow queries scoped to specific `BeliefNode` fields.

- `title:authentication` — search only in titles
- `schema:procedure` — filter by schema type
- `kind:Document` — filter by kind
- Can leverage existing `evaluate_expression` for kind/schema filtering, combined with text search for title/content
- Estimated effort: 0.5 days

### Phrase Queries

Support exact phrase matching with quoted strings.

- `"getting started"` — matches the exact phrase, not individual terms
- Requires storing term positions in the inverted index (not just frequencies)
- Check for adjacent term positions in matching documents
- Estimated effort: 1 day (requires index structure change to store positions)

### Ranking Boost Factors

Improve result ranking with additional signals beyond TF-IDF.

- **Depth boost**: top-level documents rank higher than deeply nested sections
- **Cross-reference boost**: heavily-referenced nodes rank higher (derivable from `BidGraph` edge count)
- **Kind boost**: Documents rank higher than Sections for broad queries
- These signals are all derivable from existing `BeliefBase` data — no new fields needed
- Estimated effort: 0.5 days

### Stop Word Removal

Filter common English words ("the", "a", "is", "and") from the index to reduce noise and index size.

- Standard English stop word list (~150 words)
- Applied during tokenization, before index insertion
- Query terms are also filtered so stop words in queries don't produce empty results
- Estimated effort: 0.25 days

## Backlog: Performance and Scale

### Search Performance Benchmarks

Benchmark search performance at scale using Issue 47's test fixtures.

- Index build time for 100, 500, 1000, 5000 documents
- Query latency p50/p95/p99 at each scale
- Memory overhead of inverted index relative to source data
- Document results for capacity planning
- Estimated effort: 1 day

### Lazy Search Index Loading (Deferred Corpus Search)

The MVP eagerly loads all `.idx.json` files on init (~200KB total for a typical 10-network repo). For very large repos (100+ networks), this could become a meaningful init cost. An alternative: load search indices only for loaded shards by default, and fetch indices for unloaded networks on demand when the user requests broader results.

- **Default**: search covers all networks via eagerly loaded `.idx.json` files (full corpus, even for unloaded data shards)
- **"Search all networks" button or "more results" pagination**: triggers fetching `.idx.json` files for any not yet loaded, merges them into the search, and displays additional results
- Fetched indices are cached in memory — subsequent searches include them without refetching
- UI shows "Searching 2 of 10 networks — [Search all]" to make the scope visible
- Reduces init-time network requests from N to 1 (just the manifest)
- Estimated effort: 0.5 days

## Not Planned (Superseded)

The following items from the original Issue 49 are **not backlog items** — they are superseded by architectural decisions:

| Original Item | Disposition |
|---|---|
| `SearchIndexer` event subscriber | Eliminated — compile-time `.idx.json` indices replace runtime index building entirely |
| Tantivy daemon integration | Eliminated — no Tantivy dependency |
| HTTP search server (`noet-search-server`) | Replaced by `FederatedBeliefSource` in `federated_belief_network.md` §3.6 |
| Batched commit strategy (500ms window) | Eliminated — compile-time indices; no runtime rebuild to batch |
| Production deployment guide | Deferred to federated architecture work |
| Docker deployment example | Deferred to federated architecture work |

## References

- `docs/design/search_and_sharding.md` §7 — Built-in search architecture
- `docs/design/federated_belief_network.md` §3.6 — Federated query layer (replaces HTTP search server)
- `src/beliefbase/base.rs` — `BeliefBase` struct, lazy indexing patterns
- Issue 54: Full-Text Search MVP (prerequisite — establishes built-in search)
- Issue 50: BeliefBase Sharding (establishes shard loading infrastructure)
- Issue 47: Performance Profiling (provides scale-sized test fixtures)