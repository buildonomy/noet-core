# Issue 50: BeliefBase Sharding — Per-Network Export and Loading

**Priority**: HIGH
**Estimated Effort**: 4–6 days
**Dependencies**: None (first issue in the search/sharding sequence)

## Summary

Replace the monolithic `beliefbase.json` export with per-network JSON shards, enabling the viewer to load only the networks a user needs. Establishes the `ShardManager` abstraction, network selector UI, and memory budget display that Issue 54 (search) builds on. Always generates per-network `search/*.idx.json` files so full-corpus search works regardless of whether data is sharded or monolithic. Small repositories (< 10MB) continue using monolithic export for backward compatibility.

## Goals

- Export BeliefBase as per-network JSON shards when total size exceeds threshold
- Always generate per-network `search/*.idx.json` files during `finalize_html` (monolithic and sharded modes)
- Load BeliefBase shards on-demand in WASM based on user network selection
- Memory budget (200MB) for loaded BeliefBase shards
- Graceful degradation: refuse loads that exceed budget, suggest unloading
- Backward compatibility: monolithic `beliefbase.json` for small repos (< 10MB)
- Viewer detects export format automatically — no user configuration

## Architecture

See `docs/design/search_and_sharding.md` for the full specification, including output structure (§3.1), manifest format (§4), shard format (§5), memory budget model (§6), and WASM integration (§8).

**This is the first issue in the sequence.** It establishes the export infrastructure, viewer UI, and memory management that Issue 54 (search) layers onto. After this issue, Issue 47 (Performance Profiling) creates scale-sized test fixtures that validate sharding behavior and provide scaffolding for search performance testing in Issue 54.

**Key points**:

1. **Sharding decision in `finalize_html`.** After serializing the full `BeliefGraph`, measure its size. If below `SHARD_THRESHOLD` (default 10MB), write `beliefbase.json` as today. If above, write the `beliefbase/` directory with manifest, global shard, and per-network shards.

2. **Per-network subgraph extraction.** Each shard contains the `BeliefGraph` subset for one network: its states and intra-network relations. The `global.json` shard contains the API node, system namespace nodes, and cross-network relations. This ensures cross-network link resolution works with only the global shard loaded.

3. **`BeliefBaseWasm` shard-aware API.** Extends the existing WASM struct with `from_manifest`, `load_shard`, `unload_shard` methods. The existing `from_json` constructor remains for monolithic format. Internally, loading a shard merges nodes/relations into the single `BeliefBase` instance; unloading removes them.

4. **Search indices always generated.** `finalize_html` calls `build_search_indices()` unconditionally — before the sharding decision. This writes `search/{bref}.idx.json` (one per network) and `search/manifest.json`. The `search/` directory is always present in the HTML output regardless of export mode.

5. **JavaScript `ShardManager`.** Manages loading of BB shards under a memory budget. On init, it fetches `search/manifest.json` and all `.idx.json` files so full-corpus search is available immediately — even for networks whose data shards haven't been loaded yet.

## Implementation Steps

### Phase 1: Shard Module (1.5 days)

#### Step 1.1: Core Types and Logic (0.5 days)
- [ ] Create `src/shard/mod.rs` and `src/shard/manifest.rs`
- [ ] Define `ShardConfig` (threshold, memory budget), `NetworkShard` metadata, `ShardManifest`
- [ ] Implement `should_shard(graph) -> bool` based on serialized size
- [ ] Unit tests for shard decision logic

#### Step 1.2: Manifest and Subgraph Extraction (1 day)
- [ ] Implement `build_manifest(graph, pathmap) -> ShardManifest`
- [ ] Extract per-network subgraphs from `BeliefGraph` using `PathMapMap`
- [ ] Separate global nodes: API node, system namespaces, cross-network edges
- [ ] Estimate shard sizes from serialized JSON length (10% buffer)
- [ ] Unit tests with multi-network `BeliefGraph` fixtures

### Phase 2: Export Integration (1.5 days)

#### Step 2.1: Sharded Export (1 day)
- [ ] Implement `export_sharded(graph, output_dir, pathmap) -> ShardManifest`
- [ ] Write `beliefbase/manifest.json`, `beliefbase/global.json`, `beliefbase/networks/{bref}.json`
- [ ] Create directory structure under `html_output_dir`
- [ ] Integration test: verify shard files created with correct content

#### Step 2.2: Search Index Generation (0.5 days)
- [ ] Implement `build_search_indices(graph, output_dir) -> SearchManifest`
- [ ] Call unconditionally in `finalize_html` before the sharding decision
- [ ] Write `search/manifest.json` and `search/{bref}.idx.json` for each network
- [ ] Unit test: verify index files present in both monolithic and sharded output

#### Step 2.3: Replace `export_beliefbase_json` (0.5 days)
- [ ] In `finalize_html`: measure graph size, choose monolithic or sharded path
- [ ] Monolithic path: existing `export_beliefbase_json` (unchanged)
- [ ] Sharded path: call `export_sharded`, skip monolithic file
- [ ] Log shard statistics: count, sizes, total
- [ ] Integration test: small repo → monolithic, large repo → sharded

### Phase 3: WASM Loading (1.5 days)

#### Step 3.1: BeliefBaseWasm Extensions (1 day)
- [ ] Add `from_manifest(manifest_json, entry_bid)` constructor
- [ ] Add `load_shard(bref, shard_json)` — merge into internal `BeliefBase`
- [ ] Add `unload_shard(bref)` — remove network's nodes and relations
- [ ] Add `loaded_shards()` and `has_bid(bid)` helpers
- [ ] Unit tests: load/unload cycle, query across shards, BID lookup

#### Step 3.2: JavaScript ShardManager (0.5 days)
- [ ] Create `assets/viewer/shard-manager.js`
- [ ] On init: fetch `search/manifest.json` and all `.idx.json` files (full-corpus search available immediately)
- [ ] Implement: `loadNetwork(bref)`, `unloadNetwork(bref)`, `getMemoryUsage()`
- [ ] Coordinate BB shard loading per network
- [ ] Memory budget enforcement: refuse load if would exceed budget

### Phase 4: Viewer Integration (1 day)

#### Step 4.1: Initialization (0.5 days)
- [ ] Update `initializeWasm` to: (1) fetch `search/manifest.json` + all `.idx.json` files, (2) detect sharded vs monolithic format, (3) load data accordingly
- [ ] Sharded data: load `beliefbase/manifest.json` → load global shard → load entry point network
- [ ] Monolithic data: existing `BeliefBaseWasm.from_json` path (unchanged)
- [ ] Search indices: always loaded from `search/` — code path identical for both data formats
- [ ] Test: search works before any data shard is loaded; both data code paths produce working viewer

#### Step 4.2: Network Selector UI (0.5 days)
- [ ] Network selector panel: checkboxes with name, doc count, estimated size
- [ ] Memory usage bar with warnings at 80% and 90%
- [ ] Load/unload network on checkbox toggle
- [ ] CSS styling consistent with existing viewer theme

### Phase 5: Documentation and Testing (0.5 days)

- [ ] README section: "BeliefBase Sharding" — how it works, threshold, network selector
- [ ] Integration test: end-to-end sharded export → viewer load → navigate across networks
- [ ] Integration test: backward compat — old `beliefbase.json` still loads correctly

## Testing Requirements

- Shard decision logic: below/at/above threshold
- Subgraph extraction: correct nodes per network, global shard completeness
- Size estimation accuracy (within 20% of actual)
- WASM load/unload: memory tracking, BID queries across shards
- Viewer initialization: sharded path, monolithic path, missing manifest fallback
- Cross-network navigation: node in unloaded network shows load prompt
- Manual: 3+ network repo, load/unload dynamically, memory display accuracy

## Success Criteria

- [ ] Repos > 10MB export as per-network shards with manifest
- [ ] Repos < 10MB export as monolithic `beliefbase.json` (no regressions)
- [ ] Viewer loads shards on demand — entry point network auto-loaded
- [ ] Memory budget enforced: cannot load shards exceeding 200MB total
- [ ] Network selector UI shows accurate sizes and current memory usage
- [ ] Navigation to unloaded network prompts user to load it
- [ ] All existing viewer functionality works in both sharded and monolithic modes

## Risks

### Risk 1: Cross-Network Query Performance
**Impact**: MEDIUM — queries may need to check multiple shards
**Likelihood**: MEDIUM
**Mitigation**: Loaded shards merge into a single `BeliefBase` instance, so queries operate on the unified graph. Only cross-references to *unloaded* networks degrade (by design).

### Risk 2: Shard Load/Unload Correctness
**Impact**: HIGH — incorrect merge or removal corrupts the graph
**Likelihood**: MEDIUM
**Mitigation**: `BeliefBase` already supports incremental updates via `process_event`. Model shard loading as a batch of `NodeAdded` events and unloading as `NodesRemoved`. Extensive unit tests for load/unload cycles.

### Risk 3: Backward Compatibility
**Impact**: HIGH — old viewers can't load new format
**Likelihood**: LOW (tested explicitly)
**Mitigation**: Below-threshold repos produce identical output to today. Viewer tries manifest first, falls back to `beliefbase.json`. Both code paths tested in CI.

### Risk 4: Size Estimation Drift
**Impact**: LOW — memory display inaccurate
**Likelihood**: MEDIUM
**Mitigation**: Estimate from serialized JSON + 10% buffer. Acceptable for UI display. Not used for correctness-critical decisions.

## Design Decisions

| Decision | Rationale |
|----------|-----------|
| 10MB sharding threshold | Large enough to avoid overhead for small repos; small enough to keep individual shards fast to load. Configurable for tuning. |
| Per-network, not per-document | Matches PathMapMap architecture and search index sharding. Per-document sharding deferred unless real-world networks prove too large. |
| Always generate search indices | Search is a read path over compile-time data. Making `.idx.json` generation unconditional means the viewer search path is identical regardless of data format — no conditional "build in WASM if monolithic" branch. |
| Single memory pool | All loaded data draws from one budget. Search indices (~200KB total) are loaded eagerly on init; data shards are loaded on demand. |
| Lazy loading, not eager (data) | User controls what data shards are loaded. Conserves memory, avoids loading networks the user doesn't need. Search indices are the exception — loaded eagerly for immediate full-corpus search. |
| Merge into single BeliefBase | Simpler query model than maintaining separate BeliefBase instances per shard. Leverages existing graph operations. |

## References

- `docs/design/search_and_sharding.md` — Full architecture specification (§3 output structure, §4 manifest, §5 shard format, §6 memory budget, §7 search architecture, §8 WASM integration)
- `docs/design/search_and_sharding.md` §7 — Search index format and `search/` directory layout
- `docs/design/beliefbase_architecture.md` §3.4 — BeliefGraph vs BeliefBase
- `docs/design/interactive_viewer.md` — Viewer WASM integration
- `src/codec/compiler.rs::export_beliefbase_json()` — Current monolithic export
- `src/wasm.rs::BeliefBaseWasm` — Current WASM bindings
- Issue 47: Performance Profiling (next — creates scale-sized test fixtures)
- Issue 54: Full-Text Search MVP (layers compile-time search indices onto this infrastructure)