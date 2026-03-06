---
title = "Search and Sharding Architecture"
version = "0.1"
---

# Search and Sharding Architecture

Design document for full-text search and per-network data sharding in noet-core's static HTML output.

## 1. Problem Statement

noet-core compiles document repositories into a `BeliefGraph` exported as `beliefbase.json` for the interactive HTML viewer. This works well for small repositories, but creates two scaling problems:

1. **Large monolithic export**: A repository with 1000+ documents produces a `beliefbase.json` exceeding 10MB. The viewer loads this entirely into WASM memory on page load — slow on mobile, wasteful when the user only needs one network.

2. **No search capability**: Users cannot search document content in the static HTML output. The viewer supports navigation and metadata inspection, but not full-text search.

Both problems share a structural insight: **networks are the natural sharding boundary**. The `PathMapMap` architecture already isolates data per-network. Sharding the export along this boundary requires no new abstractions.

A further insight drives the search architecture: **every field needed for full-text search is already present in the BeliefBase export**. The `BeliefNode` struct contains `bid`, `title`, `kind`, `schema`, `id`, and `payload["text"]` (the full markdown content). Rather than building inverted indices at runtime in WASM, the compiler builds compact per-network search indices at compile time and ships them alongside the data. The WASM side only deserializes and queries — zero index construction in the browser.

## 2. Design Principles

**Network-aligned sharding.** Networks (`BeliefNetwork.toml` roots) are already the top-level organizational unit. PathMapMap indexes by network. The viewer's navigation tree is network-rooted. Sharding along this boundary requires no new abstractions.

**Lazy loading with memory budgets.** The browser has limited memory. Users choose which networks to load. The system tracks memory consumption and refuses to load shards that would exceed the budget.

**Compile-time search indices.** Search indices are built during `finalize_html` — always, regardless of whether the export is monolithic or sharded. The WASM module never builds an inverted index; it only deserializes the pre-built `.idx.json` files and runs queries against them. This keeps the WASM binary small and the browser-side logic simple.

**Unified data model.** Loading a data shard makes its content navigable, inspectable, and enriches search results with snippets. But search works across the entire corpus from the moment the viewer loads — even for networks whose data shards haven't been loaded — because the lightweight search indices are always available.

## 3. Architecture Overview

### 3.1. Output Structure

`finalize_html` always produces per-network search indices. When sharding is also active (total export > threshold), it additionally splits the BeliefBase data into per-network shards:

```
html_output_dir/
├── beliefbase.json              # Only if NOT sharded (backward compat)
├── search/
│   ├── {bref_a}.idx.json        # Network A search index (always generated)
│   ├── {bref_b}.idx.json        # Network B search index
│   └── {bref_c}.idx.json        # Network C search index
└── beliefbase/                  # Only if sharded
    ├── manifest.json            # Shard metadata, memory budget, search index refs
    ├── global.json              # API node + cross-network relations
    └── networks/
        ├── {bref_a}.json        # Network A BeliefGraph shard (full data)
        ├── {bref_b}.json        # Network B BeliefGraph shard
        └── {bref_c}.json        # Network C BeliefGraph shard
```

The `search/` directory is always generated. The `beliefbase/` directory is only generated when the total export exceeds the sharding threshold. In monolithic mode, `beliefbase.json` contains all the data, and the `search/*.idx.json` files provide search capability over it.

### 3.2. Data Flow

```
Source Files
    │
    ▼
DocumentCompiler::parse_all()
    │
    ▼
BeliefEvent stream → BeliefBase (synchronized via Option G pattern)
    │
    ▼
DocumentCompiler::finalize_html(global_bb)
    ├── export_beliefbase() ──→ beliefbase.json OR beliefbase/shards
    └── build_search_indices() ──→ search/{bref}.idx.json (ALWAYS)
    │
    ▼
Viewer (browser)
    ├── Load search indices (.idx.json) → search entire corpus immediately
    ├── Load manifest (if sharded) → show network selector
    ├── Load data shards on demand → BeliefBaseWasm (for navigation, context, display)
    └── Search queries the compile-time indices; snippets extracted from loaded data
```

The critical constraint: `global_bb` (not `session_bb`) must be used for export and index building, because the compiler is lazy and `session_bb` may be incomplete. See `beliefbase_architecture.md` §3.7 for the synchronization pattern.

### 3.3. Sharding Decision

```
total_size = serialize(global_bb.export_beliefgraph()).len()

ALWAYS:
    for each network:
        write search/{bref}.idx.json     # compile-time search index

if total_size < SHARD_THRESHOLD:
    write beliefbase.json                # monolithic, backward compatible
else:
    write beliefbase/manifest.json       # shard manifest
    write beliefbase/global.json         # API node, cross-network edges
    for each network:
        write beliefbase/networks/{bref}.json
```

`SHARD_THRESHOLD` defaults to 10MB. Configurable via `ShardConfig`. Search indices are always generated regardless of the sharding threshold.

## 4. Manifest Format

The manifest describes BeliefBase shards and references the search indices. The viewer reads it to populate the network selector UI and to locate search index files.

### 4.1. BeliefBase Manifest

`beliefbase/manifest.json` (only in sharded mode):

```json
{
  "version": "1.0",
  "sharded": true,
  "memoryBudgetMB": 200,
  "networks": [
    {
      "bref": "01abc",
      "bid": "01234567-89ab-cdef-0123-456789abcdef",
      "title": "Main Documentation",
      "nodeCount": 247,
      "relationCount": 512,
      "estimatedSizeMB": 3.2,
      "path": "networks/01abc.json",
      "searchIndexPath": "../search/01abc.idx.json",
      "searchIndexSizeKB": 85
    }
  ],
  "global": {
    "nodeCount": 5,
    "estimatedSizeMB": 0.02,
    "path": "global.json"
  }
}
```

In monolithic mode there is no manifest. The viewer discovers search indices by fetching `search/` contents or by a lightweight `search/manifest.json` listing the available `.idx.json` files:

```json
{
  "version": "1.0",
  "networks": [
    { "bref": "01abc", "title": "Main Documentation", "path": "01abc.idx.json", "sizeKB": 85 }
  ]
}
```

### 4.2. Size Estimation

Estimated sizes for data shards are computed during export by measuring serialized JSON byte length, with a 10% buffer for in-memory overhead.

Search index size is typically **3-5%** of the full shard size. A 3MB shard produces a ~100KB index. This is because the index stores only `term → [(bid, frequency)]` plus minimal per-document metadata (title, path) — no `payload["text"]`, no relations, no full `BeliefNode` data. All search indices for a large multi-network repo fit comfortably in a single round of fetches (~200KB total for 10 networks).

## 5. Per-Network Shard Format

Each BeliefBase shard is a `BeliefGraph` scoped to one network:

```json
{
  "network_bref": "01abc",
  "network_bid": "01234567-89ab-cdef-0123-456789abcdef",
  "states": {
    "<bid>": { "bid": "...", "kind": "...", "title": "...", "payload": {...} }
  },
  "relations": {
    "edges": [...]
  }
}
```

The `global.json` shard contains:
- The API node (`buildonomy_api_bid`)
- Cross-network relations (epistemic/pragmatic edges between networks)
- System namespace nodes (href, asset namespaces)

This separation ensures the viewer can always resolve cross-network links by loading only the global shard plus the networks the user selects.

## 6. Memory Budget Model

### 6.1. Budget Allocation

Total browser memory budget: **200MB** (conservative for modern devices including smartphones).

The budget is a single pool for loaded data shards. Search indices are loaded outside the budget — they are a fixed, small overhead (~3-5% of shard size per network).

```
Available = 200MB
Used = Σ(loaded BB data shards)
Remaining = Available - Used

Load request for shard S:
  if S.estimatedSizeMB <= Remaining:
    load S, update Used
  else:
    refuse with warning, suggest unloading networks
```

### 6.2. Loading Strategy

1. **Always loaded**: Search indices for all networks (lightweight, outside memory budget)
2. **Always loaded** (sharded mode): `global.json` (typically < 0.1MB)
3. **Auto-loaded**: The data shard for the network containing the entry point document
4. **User-selected**: Additional data shards via the network selector UI
5. **Memory display**: UI shows current/max usage with visual warnings at 80% and 90%

### 6.3. Unloading

Users can uncheck a network in the selector to unload its data shard, freeing memory. The viewer must handle gracefully that queries against unloaded networks return limited results (title + path from the search index, but no context or snippet), and navigation to unloaded nodes shows a "load this network" prompt. Unloading a data shard does **not** remove the network from search — the compile-time index is always available.

## 7. Search Architecture

### 7.1. Design Decision: Compile-Time Built-In Search

Search is implemented as a compile-time inverted index built during `finalize_html`, with no external search engine dependency and no index construction in the browser.

**Key insight**: all indexing happens at compile time in native Rust. The WASM side only deserializes the pre-built `.idx.json` and runs TF-IDF queries against it. This means:
- Zero index-building code in the WASM binary
- Zero index-build latency in the browser
- Search is available the instant the `.idx.json` files are loaded
- The same tokenizer runs once (at compile time), not twice

**Rationale — why not Tantivy?**

Every field needed for search is already present in the `BeliefNode` struct:

| Search Field | Source in BeliefNode |
|---|---|
| `bid` | `BeliefNode.bid` |
| `network` | Derivable from `PathMapMap` |
| `title` | `BeliefNode.title` |
| `content` | `BeliefNode.payload["text"]` |
| `kind` | `BeliefNode.kind` |
| `schema` | `BeliefNode.schema` |
| `id` | `BeliefNode.id` |
| `path` | `PathMapMap` resolution |

A separate Tantivy index would add 2-4MB to the WASM binary and introduce a significant WASM compilation risk (Tantivy's WASM support is experimental). The built-in approach eliminates both concerns.

**What the built-in approach trades away vs. Tantivy:**

| Feature | Tantivy | Built-in | Impact |
|---|---|---|---|
| BM25 ranking | ✓ | Simple TF-IDF | LOW — docs are short, ranking difference is minimal |
| Stemming | ✓ (configurable) | Optional via `rust-stemmers` (~50KB) | MEDIUM — "running" won't match "run" without stemmer |
| Fuzzy matching | ✓ (Levenshtein) | Implementable (~50 lines) | LOW |
| Snippet generation | ✓ (all results) | Loaded networks only (simple string search) | LOW |
| WASM binary size | +2-4MB | +0 | Significant win |
| WASM compile risk | HIGH | None | **Major win** |

For an MVP searching 100s of documents per network, these tradeoffs are strongly favorable. Stemming can be added later via `rust-stemmers` without architectural change.

### 7.2. Compile-Time Search Index Format

Each `.idx.json` file is a compact per-network search index built during `finalize_html`:

```json
{
  "network_bref": "01abc",
  "doc_count": 247,
  "docs": {
    "<bid>": { "title": "Installation Guide", "path": "docs/install.html", "term_count": 342 }
  },
  "index": {
    "install": [["<bid>", 12], ["<bid2>", 3]],
    "guide": [["<bid>", 8]],
    "configur": [["<bid>", 5], ["<bid3>", 2]]
  }
}
```

**`docs`**: Minimal per-document metadata — just enough to display a search result row (title, path) and compute TF-IDF (term_count for length normalization). No `payload`, no `kind`, no relations.

**`index`**: The inverted index — `term → [(bid, frequency)]`. Terms are tokenized at compile time (split on whitespace/punctuation, lowercase, optional stemming). Title terms are included with a 3x weight multiplier baked into the frequency count.

**Size characteristics**: For a network with 250 documents averaging 2KB of text each (500KB total content), the `.idx.json` is typically 15-30KB. The `docs` map stores ~100 bytes per document (bid + title + path). The `index` map stores unique terms — English text has roughly 5-10 unique terms per 100 words, and terms are shared across documents, so deduplication keeps the index compact. All search indices for a 10-network repo fit in a single round of fetches (~200KB total).

### 7.3. Index Building (Compile Time Only)

Index building happens in `finalize_html`, always, for every network:

```
for each network_bref in pathmap.nets():
    query global_bb for all nodes in this network
    for each (bid, node) in network_nodes:
        tokenize node.title (weight: 3x) and node.payload["text"] (weight: 1x)
        record (bid, title, path, term_count) in docs map
        for each term: index[term].push((bid, frequency))
    serialize as search/{bref}.idx.json
```

This runs during the same `finalize_html` pass that writes the BB export (monolithic or sharded). It queries `global_bb` per-network using `Expression::StateIn(StatePred::InNamespace(bref))`, the same pattern used elsewhere in the codebase.

There is no index building in WASM. The browser deserializes the `.idx.json` and queries it directly.

### 7.4. Snippet Generation (From Loaded Data)

Snippets require `payload["text"]`, which is only available for loaded data shards (or in monolithic mode where everything is loaded). Generating a snippet is a simple string search — not an inverted index operation:

```
fn extract_snippet(query_terms: &[String], text: &str, window: usize) -> String {
    // Find the first occurrence of any query term in the text
    // Return a ~window-character excerpt centered on the match
}
```

This runs in WASM when displaying search results. For results from loaded networks, the snippet is populated by looking up `BeliefNode.payload["text"]` for the matching BID and running the string search. For results from unloaded networks, the snippet field is empty — the UI shows title + path instead.

This is intentionally not an index operation. The inverted index tells us *which* documents match; the snippet extraction just needs to find *where* in the text the match occurs for display. A linear scan of a single document's text is fast enough (~microseconds).

### 7.5. Query Model

When the user searches, `BeliefBaseWasm` queries the compile-time indices:

1. **Tokenize the query** using the same tokenizer as compile time (split, lowercase, optional stemming).

2. **Look up each term** in every loaded `.idx.json` inverted index. Compute TF-IDF scores: `score = Σ(tf(term, doc) × idf(term))`. The IDF calculation spans the total `doc_count` across all loaded search indices, giving accurate global term rarity.

3. **Fuzzy matching** — for queries under 20 characters, also match terms within Levenshtein distance ≤ 2.

4. **Merge and rank** — combine results from all networks, return top N sorted by score.

5. **Enrich results** — for each result, check if the network's data shard is loaded. If yes, extract a snippet from `payload["text"]` via string search. If no, leave the snippet empty and tag the result as `loaded: false`.

6. **Field filtering** — the existing `evaluate_expression` API handles filtering by kind, schema, network before or after search.

### 7.6. Search Result Format

```
SearchResult {
    bid: String,        // For fetching full node from BeliefBase
    network: String,    // Which network (bref)
    title: String,      // Display title (always available from .idx.json)
    snippet: String,    // Content excerpt — empty string for unloaded networks
    score: f64,         // TF-IDF relevance score
    path: String,       // Display path (always available from .idx.json)
    loaded: bool,       // Whether the network's data shard is currently loaded
}
```

Results from loaded networks include content snippets. Results from unloaded networks show title + path + score but no snippet. Clicking a result from an unloaded network triggers `ShardManager.loadNetwork(bref)`, after which the viewer navigates to the document.

### 7.7. Daemon Mode

In daemon/watch mode (`noet watch`), the `.idx.json` files are rebuilt on each `finalize_html` pass. The viewer detects updated files and reloads the search indices.

There is no runtime index building and no dirty-flag mechanism for search. The compile-time indices are the single source of truth. Between `finalize_html` passes, the search indices may be slightly stale for recently edited content — this is acceptable for the development workflow, as the user is typically navigating to the document they just edited rather than searching for it.

## 8. WASM Integration

### 8.1. BeliefBaseWasm Extensions

The existing `BeliefBaseWasm` (currently loads monolithic `beliefbase.json`) gains shard-aware and search methods:

**Shard management (Issue 50):**
- `from_manifest(manifest_json, entry_bid)` — Initialize from manifest instead of full JSON
- `load_shard(bref, shard_json)` — Merge a network shard into the internal BeliefBase
- `unload_shard(bref)` — Remove a network's nodes and relations
- `loaded_shards()` — List currently loaded network brefs
- `has_bid(bid)` — Check if a BID is in any loaded shard

**Search (Issue 54):**
- `load_search_index(bref, idx_json)` — Deserialize a compile-time `.idx.json` into the in-memory search structure
- `search(query, limit)` — Full-text search across all networks with loaded search indices, returns JSON array of `SearchResult`
- `search_in_network(bref, query, limit)` — Search within a specific network

The existing `from_json(data, entry_bid)` constructor remains for backward compatibility with monolithic exports. The existing `search()` method (currently title/id substring matching) is replaced by TF-IDF search over the compile-time indices.

Internally, `BeliefBaseWasm` holds the deserialized compile-time index data (from `.idx.json` files). It does **not** build any inverted index at runtime. Snippet extraction is a simple string search against `payload["text"]` for loaded nodes — not an index operation.

### 8.2. JavaScript ShardManager

A `ShardManager` JavaScript class coordinates shard loading under a memory budget:

```
ShardManager
  ├── beliefbase: BeliefBaseWasm instance
  ├── memoryBudget: { totalMB, usedMB }
  ├── loadNetwork(bref) → loads BB data shard (for navigation, context, snippets)
  ├── unloadNetwork(bref) → unloads data shard (search still works via .idx.json)
  ├── getMemoryUsage() → { current, max, percentage }
  └── init: loads search/manifest.json + ALL .idx.json files
```

On initialization, the `ShardManager` fetches all `.idx.json` files (discovered via `search/manifest.json` or the BB manifest). These are small enough (~3-5% of shard size) to load eagerly without counting against the memory budget. This one-time cost enables full-corpus search from the moment the viewer loads. For very large repos (100+ networks), a lazy alternative is available as a backlog item: load indices only for loaded shards by default, and fetch unloaded network indices on demand via a "search all networks" interaction (see Issue 49 backlog).

The memory budget applies only to full data shards (which contain `payload["text"]`, relations, and all `BeliefNode` fields). Search indices are a fixed, small overhead.

## 9. Viewer Integration

### 9.1. Initialization Flow

```
1. Fetch search/manifest.json (or beliefbase/manifest.json in sharded mode)
2. Load ALL .idx.json files → BeliefBaseWasm.load_search_index() per network
3. If sharded:
   a. Load global.json → BeliefBaseWasm.from_manifest()
   b. Determine entry point network from entry_bid
   c. Load entry point network data shard
   d. Build network selector UI
4. If monolithic:
   a. Load beliefbase.json → BeliefBaseWasm.from_json() (existing path)
5. Build navigation tree, render initial document
```

Search UI is always available after step 2. Full-corpus search works immediately — before any data shard is loaded in sharded mode, and alongside the monolithic load in non-sharded mode.

Step 2 is the key addition: all search indices are loaded eagerly. For a 10-network repo, this is typically ~200KB total — one small batch of fetches. After this step the user can search every document in every network.

### 9.2. Network Selector UI

The network selector is a panel (collapsed by default) showing:
- Checkbox per network: name, document count, estimated size
- Current memory usage bar
- "Load All" / "Unload All" controls
- Visual warning when approaching budget limit

Selecting a network loads its data shard (for navigation, context, and snippet enrichment). Deselecting unloads it. Neither action affects search availability — the compile-time indices are always present.

### 9.3. Search UI

Search is available immediately after initialization (full corpus):
- Search input field in the viewer header
- Debounced input (300ms) to avoid excessive queries
- Results panel with: title, snippet (if data loaded), network name, path, loaded indicator
- Results from unloaded networks show title + path but no snippet, with a subtle "load to see full content" indicator
- Click result from loaded network → navigate to document
- Click result from unloaded network → prompt to load the network shard (showing estimated size), then navigate
- Network filter: search within selected networks only

## 10. Scaling Beyond Browser Memory

For corpora that exceed the browser memory budget, the per-network sharding model provides graceful degradation: search always works across the full corpus (via compile-time indices), but navigation and context are available only for loaded networks.

For deployments requiring full-corpus navigation without manual shard loading, the architecture naturally extends to **federated data access**: a remote `BeliefSource` serves queries for data not loaded locally. This is the same `FederatedBeliefSource` pattern described in `federated_belief_network.md` §3.6 — the viewer queries loaded shards locally and falls back to a remote HTTP API for unloaded networks.

This is explicitly **not** a search-specific server. Navigation, metadata, context, and search all benefit equally from remote data access. The federated approach provides a single query interface (`BeliefSource`) over both local and remote data.

See `docs/design/federated_belief_network.md` for the full federated architecture.

## 11. Backward Compatibility

### 11.1. Detection Logic

The viewer detects the export format on load:

1. Try `fetch("search/manifest.json")`
2. If 200 → load search indices listed in manifest
3. Try `fetch("beliefbase/manifest.json")`
4. If 200 → sharded mode, read manifest, load global + entry point shard
5. If 404 → try `fetch("beliefbase.json")`
6. If 200 → monolithic mode (legacy)
7. If 404 → error: no beliefbase found

### 11.2. Migration Path

- Existing outputs with only `beliefbase.json` (no `search/`) continue to work — the viewer falls back to the existing substring search on title/id
- New builds always generate `search/*.idx.json`, enabling full-text search
- New builds automatically shard data when above threshold
- No user action required — the viewer handles all format combinations
- Search is progressively enhanced: old outputs get substring search, new outputs get full-text TF-IDF search

## 12. Implementation Sequencing

The architecture described in this document is implemented across three issues:

```
Issue 50: BeliefBase Sharding
    │   Establishes: finalize_html export hooks, ShardManager JS class,
    │   network selector UI, memory budget display, WASM shard loading,
    │   compile-time search index generation (.idx.json per network)
    ▼
Issue 47: Performance Profiling
    │   Establishes: realistic corpus generator, scale-sized test fixtures
    │   (10KB → 100MB+), macro-benchmarks, memory profiling infrastructure
    ▼
Issue 54: Full-Text Search MVP
        Adds: search index deserialization and TF-IDF query in
        BeliefBaseWasm, snippet extraction from loaded data,
        search UI in viewer, fuzzy matching
```

**Why this order:**

1. **Issue 50 first** — BeliefBase sharding establishes all the shared infrastructure: export hooks in `finalize_html`, the `ShardManager` JS class, network selector UI, memory budget display, and compile-time search index generation. The `.idx.json` files are a natural extension of the per-network export — same iteration over `global_bb` per network, just a different output format. Index generation is part of the export pipeline, not the search feature.

2. **Issue 47 second** — Performance profiling creates realistic test corpora at various scales. These fixtures are essential for validating that sharding works at scale *and* for benchmarking search performance afterward.

3. **Issue 54 third** — By this point, the export infrastructure (including `.idx.json` generation), viewer UI, and scale-sized test fixtures are all in place. The search implementation adds deserialization of `.idx.json` into `BeliefBaseWasm`, TF-IDF query execution, snippet extraction from loaded data, and the viewer search UI — no external dependencies, no WASM compilation risks, no runtime index building.

## 13. Open Design Questions

1. **Auto-load on search result click**: When a user clicks a search result from an unloaded network, should we auto-load silently, or confirm first ("Load 'Network B' (3.2MB) to view this result?")? Auto-loading is better UX but risks exceeding the memory budget. A confirmation that shows estimated size is the safer default.

2. **Shard granularity**: Per-network is the starting point. If a single network has 10,000+ documents and exceeds the memory budget alone, per-document sharding within a network may be needed. Defer this until we have real-world data from Issue 47's profiling.

3. **Stemming**: Should the MVP include a stemming library (`rust-stemmers`, ~50KB) for better recall ("running" matches "run"), or is simple lowercase tokenization sufficient? Stemming can be added later without architectural change. Since the tokenizer only runs at compile time, adding it later just requires regenerating the `.idx.json` files.

4. **Eager vs lazy search index loading**: The MVP loads all `.idx.json` files on init (~200KB for 10 networks). For very large repos this could be a meaningful init cost. A backlog alternative: load search indices only for loaded shards, show "Searching 2 of 10 networks — [Search all]", and fetch remaining indices on demand when the user requests broader results. See Issue 49 backlog for details.

5. **Search index freshness in daemon mode**: In `noet watch`, the `.idx.json` files are rebuilt on each `finalize_html` pass. Between passes, the indices may be slightly stale for recently edited content. This is acceptable — the user is typically navigating to the document they just edited, not searching for it.

## 14. References

- `docs/design/beliefbase_architecture.md` — BeliefGraph/BeliefBase data model, PathMapMap, event system
- `docs/design/interactive_viewer.md` — Viewer architecture, WASM integration, navigation
- `docs/design/federated_belief_network.md` — Federated data access for large-corpus scaling
- `src/codec/compiler.rs::finalize_html()` — Export entry point and search index generation
- `src/codec/compiler.rs::export_beliefbase_json()` — Current monolithic export
- `src/wasm.rs::BeliefBaseWasm` — Current WASM bindings (including existing `search()` method)
- `assets/viewer/wasm.js::initializeWasm()` — Current viewer initialization
- Issue 50: BeliefBase Sharding (first in sequence — export infrastructure and index generation)
- Issue 47: Performance Profiling (second — scale-sized test fixtures)
- Issue 54: Full-Text Search MVP (third — search query and UI)