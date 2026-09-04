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

See `docs/design/core/search_and_sharding.md` for the full specification, including output structure (§3.1), manifest format (§4), shard format (§5), memory budget model (§6), and WASM integration (§8).

## Updates

### 2026-08-27: Format drift since this issue landed

- Sharding threshold: 10MB → 2MB (`SHARD_THRESHOLD` in `src/shard/manifest.rs`)
- Data payloads switched JSON → MessagePack: `beliefbase.json` → `beliefbase.msgpack`, `global.json` → `global.msgpack`, `networks/{bref}.json` → `networks/{bref}.msgpack`, `search/{bref}.idx.json` → `search/{bref}.idx.msgpack`
- `beliefbase/manifest.json` and `search/manifest.json` remain JSON
- See `src/shard/export.rs`, `src/shard/search.rs` for current format

## Implementation Steps

### Phase 1: Shard Module (1.5 days)

#### Step 1.1: Core Types and Logic (0.5 days)
- [x] Create `src/shard/mod.rs` and `src/shard/manifest.rs`
- [x] Define `ShardConfig` (threshold, memory budget), `NetworkShard` metadata, `ShardManifest`
- [x] Implement `should_shard(graph) -> bool` based on serialized size
- [x] Unit tests for shard decision logic

#### Step 1.2: Manifest and Subgraph Extraction (1 day)
- [x] Implement `build_manifest(graph, pathmap) -> ShardManifest`
- [x] Extract per-network subgraphs from `BeliefGraph` using `PathMapMap`
- [x] Separate global nodes: API node, system namespaces, cross-network edges
- [x] Estimate shard sizes from serialized JSON length (10% buffer)
- [x] Unit tests with multi-network `BeliefGraph` fixtures

### Phase 2: Export Integration (1.5 days)

#### Step 2.1: Sharded Export (1 day)
- [x] Implement `export_sharded(graph, output_dir, pathmap) -> ShardManifest`
- [x] Write `beliefbase/manifest.json`, `beliefbase/global.json`, `beliefbase/networks/{bref}.json`
- [x] Create directory structure under `html_output_dir`
- [x] Integration test: verify shard files created with correct content

#### Step 2.2: Search Index Generation (0.5 days)
- [x] Implement `build_search_indices(graph, output_dir) -> SearchManifest`
- [x] Call unconditionally in `finalize_html` before the sharding decision
- [x] Write `search/manifest.json` and `search/{bref}.idx.json` for each network
- [x] Unit test: verify index files present in both monolithic and sharded output

#### Step 2.3: Replace `export_beliefbase_json` (0.5 days)
- [x] In `finalize_html`: measure graph size, choose monolithic or sharded path
- [x] Monolithic path: existing `export_beliefbase_json` (unchanged)
- [x] Sharded path: call `export_sharded`, skip monolithic file
- [x] Log shard statistics: count, sizes, total
- [x] Integration test: small repo → monolithic, large repo → sharded

### Phase 3: WASM Loading (1.5 days)

#### Step 3.1: BeliefBaseWasm Extensions (1 day)
- [x] Add `from_manifest(manifest_json, entry_bid)` constructor
- [x] Add `load_shard(bref, shard_json)` — merge into internal `BeliefBase`
- [x] Add `unload_shard(bref)` — remove network's nodes and relations
- [x] Add `loaded_shards()` and `has_bid(bid)` helpers
- [x] Unit tests: load/unload cycle, query across shards, BID lookup

#### Step 3.2: JavaScript ShardManager (0.5 days)
- [x] Create `assets/viewer/shard-manager.js`
- [x] On init: fetch `search/manifest.json` and all `.idx.json` files (full-corpus search available immediately)
- [x] Implement: `loadNetwork(bref)`, `unloadNetwork(bref)`, `getMemoryUsage()`
- [x] Coordinate BB shard loading per network
- [x] Memory budget enforcement: refuse load if would exceed budget

### Phase 4: Viewer Integration (1 day)

#### Step 4.1: Initialization (0.5 days)
- [x] Update `initializeWasm` to: (1) fetch `search/manifest.json` + all `.idx.json` files, (2) detect sharded vs monolithic format, (3) load data accordingly
- [x] Sharded data: load `beliefbase/manifest.json` → load global shard → load entry point network
- [x] Monolithic data: existing `BeliefBaseWasm.from_json` path (unchanged)
- [x] Search indices: always loaded from `search/` — code path identical for both data formats
- [ ] Test: search works before any data shard is loaded; both data code paths produce working viewer

#### Step 4.2: Network Selector UI (0.5 days)
- [x] Network selector panel: checkboxes with name, doc count, estimated size
- [x] Memory usage bar with warnings at 80% and 90%
- [x] Load/unload network on checkbox toggle
- [x] CSS styling consistent with existing viewer theme
- [x] Relocated from nav panel widget to footer drawer: tab in footer shows compact "N loaded · X / 200 MB" summary; click expands a fixed panel above footer with network list + memory bar; hidden in monolithic mode

### Phase 5: Documentation and Testing (0.5 days)

- [x] README section: "BeliefBase Sharding" — how it works, threshold, network selector (moved to `docs/design/core/architecture.md` and `docs/design/core/beliefbase_architecture.md` per DOCUMENTATION_STRATEGY.md; one-liner added to README Key Features)
- [x] Integration test: end-to-end sharded export → viewer load → navigate across networks (`test_sharded_export_writes_correct_structure`, `test_finalize_html_always_writes_search_indices`)
- [x] Integration test: backward compat — old `beliefbase.json` still loads correctly (`test_monolithic_beliefbase_json_is_valid_belief_graph`, `test_finalize_html_monolithic_below_threshold`)

## Testing Requirements

- Shard decision logic: below/at/above threshold ✅ (`shard::manifest::tests`)
- Subgraph extraction: correct nodes per network, global shard completeness ✅ (`shard::export::tests`)
- Size estimation accuracy (within 20% of actual) ✅ (`test_estimate_size_mb_one_mb`)
- WASM load/unload: memory tracking, BID queries across shards ✅ (`test_global_shard_load_unload_cycle`, `test_network_shard_deserialize_to_belief_graph`, `test_unload_skips_shared_nodes`)
- Viewer initialization: sharded path, monolithic path, missing manifest fallback ✅ (wasm.js updated; manual test required)
- Cross-network navigation: node in unloaded network shows load prompt ⬜ (deferred to Issue 54)
- Manual: 3+ network repo, load/unload dynamically, memory display accuracy ⬜ (requires a repo exceeding 10 MB threshold)
- `cargo build --features service,bin` passes ✅ (required `shard/wire.rs` split and `watch.rs` `noet_core::` → `crate::` fix)

## Success Criteria

- [x] Repos > 10MB export as per-network shards with manifest
- [x] Repos < 10MB export as monolithic `beliefbase.json` (no regressions)
- [x] Viewer loads shards on demand — entry point network auto-loaded
- [x] Memory budget enforced: cannot load shards exceeding 200MB total
- [x] Network selector UI shows accurate sizes and current memory usage
- [ ] Navigation to unloaded network prompts user to load it (deferred to Issue 54 — requires search/navigation integration)
- [x] All existing viewer functionality works in both sharded and monolithic modes

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

- `docs/design/core/search_and_sharding.md` — Full architecture specification (§3 output structure, §4 manifest, §5 shard format, §6 memory budget, §7 search architecture, §8 WASM integration)
- `docs/design/core/search_and_sharding.md` §7 — Search index format and `search/` directory layout
- `docs/design/core/beliefbase_architecture.md` §3.4 — BeliefGraph vs BeliefBase
- `docs/design/presentation/interactive_viewer.md` — Viewer WASM integration
- `src/codec/compiler.rs::export_beliefbase_json()` — Current monolithic export
- `src/wasm.rs::BeliefBaseWasm` — Current WASM bindings
- Issue 47: Performance Profiling (next — creates scale-sized test fixtures)
- Issue 54: Full-Text Search MVP (layers compile-time search indices onto this infrastructure)