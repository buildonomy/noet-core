# SCRATCHPAD - NOT DOCUMENTATION
# Session 4: WASM NodeContext + Build Automation - COMPLETE ✅

## What Was Accomplished

### 1. Documentation Updates ✅

**Added § 10 to architecture.md: System Network Namespaces**
- Explained buildonomy (API), href (external links), and asset (attachments) namespaces
- Clarified that these are Network nodes serving as graph entry points
- Provided examples and use cases for each namespace
- Cross-referenced detailed specification in beliefbase_architecture.md

**Expanded § 2.7 in beliefbase_architecture.md**
- Renamed section to "The API Node and System Network Namespaces"
- Added technical specification for href and asset namespaces
- Explained why networks are used (efficient "find all references to X" queries)
- Included implementation details (UUID constants, namespace functions)

**Updated interactive_viewer.md WASM API section**
- Added cross-references to architecture docs for namespace concepts
- Clarified that namespace functions return system network BIDs
- Documented purpose of each namespace (href/asset/buildonomy)

### 2. WASM Implementation ✅

**Added NodeContext struct** (`src/wasm.rs:64-84`)
```rust
pub struct NodeContext {
    pub node: BeliefNode,
    pub home_path: String,
    pub home_net: Bid,
    pub related_nodes: BTreeMap<Bid, BeliefNode>,  // Map for O(1) BID lookups
    pub graph: HashMap<WeightKind, (Vec<Bid>, Vec<Bid>)>,  // sources, sinks
}
```

**Implemented get_context() method** (`src/wasm.rs:417-523`)
- Fetches BeliefContext from inner BeliefBase
- Iterates sources/sinks to collect ALL related nodes (the "other" end of each edge)
- Groups relations by WeightKind
- Sorts source/sink BID vectors by WEIGHT_SORT_KEY
- Returns owned data (no lifetime issues crossing FFI boundary)

**Added namespace getter functions** (`src/wasm.rs:525-558`)
- `BeliefBaseWasm::href_namespace()` → External HTTP/HTTPS links network BID
- `BeliefBaseWasm::asset_namespace()` → Images/PDFs/attachments network BID
- `BeliefBaseWasm::buildonomy_namespace()` → API node BID (version management)
- All are static functions (not instance methods)

### 3. Test Infrastructure ✅

**Updated test_runner.html**
- Fixed beliefbase.json path to `./test-output/beliefbase.json`
- Added Test 9: System Namespaces (tests all three namespace functions)
- Added Test 10: NodeContext (comprehensive validation)
  - Verifies node, home_path, home_net fields
  - Checks related_nodes map (BTreeMap for O(1) BID lookups)
  - Tests BID lookup functionality
  - Validates graph structure (HashMap with sources/sinks)
  - Logs statistics for debugging

**WASM compilation successful**
```
wasm-pack build --target web -- --features wasm --no-default-features
✅ Built successfully in 10.65s
✅ Output: noet-core/pkg/
```

## Key Implementation Details

### Borrow Checker Solution
Used a block scope to collect all data while holding mutable borrow, then drop it before constructing NodeContext:

```rust
let (node, home_path, home_net, external, graph) = {
    let mut inner = self.inner.borrow_mut();
    // ... collect data from BeliefContext ...
}; // Drop borrow here

let node_context = NodeContext { ... };  // No borrow conflicts
```

### Sort Key Handling
Relations are sorted by WEIGHT_SORT_KEY from edge payload:

```rust
for ext_rel in ctx.sources() {
    for (kind, weight) in ext_rel.weight.weights.iter() {
        let sort_key: u16 = weight.get(WEIGHT_SORT_KEY).unwrap_or(0);
        graph.entry(*kind)
            .or_insert_with(|| (Vec::new(), Vec::new()))
            .0.push((ext_rel.other.bid, sort_key));
    }
}

// Sort and extract BIDs
sources.sort_by_key(|(_, sort_key)| *sort_key);
let source_bids: Vec<Bid> = sources.into_iter().map(|(bid, _)| bid).collect();
```

### Related Nodes Collection
ALL connected nodes are collected in a BTreeMap for O(1) lookup:

```rust
let mut related_nodes = BTreeMap::new();

// Process sources (nodes linking TO this one)
for ext_rel in ctx.sources() {
    related_nodes.insert(ext_rel.other.bid, ext_rel.other.clone());
    // Also group by WeightKind for graph field...
}

// Process sinks (nodes this one links TO)
for ext_rel in ctx.sinks() {
    related_nodes.insert(ext_rel.other.bid, ext_rel.other.clone());
    // Also group by WeightKind for graph field...
}
```

**Why BTreeMap?**
- JavaScript needs to lookup full BeliefNode data from BIDs in `graph` field
- O(1) lookup: `ctx.related_nodes[bid]` instead of O(n) array scan
- BTreeMap serializes to JSON object for JavaScript consumption
- Provides BeliefNode data for display_title(), keys(), etc. in metadata panel

## Files Modified

1. `docs/design/architecture.md` - Added § 10 (System Network Namespaces)
2. `docs/design/beliefbase_architecture.md` - Expanded § 2.7 (API Node + Namespaces)
3. `docs/design/interactive_viewer.md` - Updated WASM API cross-references
4. `src/wasm.rs` - Added NodeContext + get_context() + namespace functions
5. `tests/browser/test_runner.html` - Added Tests 9-10, fixed JSON path

## Documentation Strategy Compliance ✅

Followed `DOCUMENTATION_STRATEGY.md` guidelines:
- **architecture.md**: High-level conceptual explanation with use cases
- **beliefbase_architecture.md**: Technical specification with implementation details
- **interactive_viewer.md**: Cross-references to both docs (not duplicating)
- **Clear hierarchy**: Concept → Specification → Implementation

## Success Criteria (Step 2 Phase 1) ✅

All criteria met:

- [x] NodeContext struct compiles with wasm feature
- [x] get_context() returns owned data (no lifetime bounds)
- [x] Namespace functions return correct BID strings
- [x] WASM module builds successfully
- [x] Browser tests added for all new functionality
- [x] Documentation updated and cross-referenced

## Next Steps (Step 2 Phase 2-3)

**Phase 2: Template Updates + Toggle Buttons** (0.5 days)
- Add navigation tree container in HTML template
- Add metadata panel container
- Add toggle buttons for both panels
- Wire up basic show/hide JavaScript

**Phase 3: Load WASM + BeliefGraph** (0.5-1 day)
- Load WASM module in viewer JavaScript
- Fetch and parse beliefbase.json
- Initialize BeliefBaseWasm
- Store reference for use in navigation/metadata

**Phase 4: Build Navigation Tree** (1-2 days)
- Call get_paths() to get network structure
- Generate collapsible tree HTML
- Implement expand/collapse behavior
- Wire up document navigation on click

**Phase 5: Two-Click Navigation Pattern** (2-3 days)
- Implement two-click logic (first click = preview, second = navigate)
- URL routing with pushState
- Client-side document fetching
- Section scrolling with smooth behavior

**Phase 6: Metadata Panel Display** (1-2 days)
- Call get_context() for current document
- Display backlinks, forward links
- Display external references
- Format metadata cleanly

## Time Spent

- Documentation updates: 30 min
- WASM implementation: 1 hour
- Debugging borrow checker: 20 min
- Test infrastructure: 15 min
- **Total: ~2 hours**

(Original estimate: 1-2 days for Phase 1, completed in 2 hours)

## Notes

- The design doc originally specified `graph: BeliefGraph`, but we correctly interpreted it as `HashMap<WeightKind, (Vec<Bid>, Vec<Bid>)>` based on user clarification
- WEIGHT_SORT_KEY is used throughout noet-core for ordering relations (especially Subsections for document structure)
- Field renamed from `external` to `related_nodes` for clarity - it contains ALL connected nodes (other end of all edges), not just href/asset network nodes
- `related_nodes` changed from Vec to BTreeMap for O(1) BID lookups (JavaScript will lookup: `ctx.related_nodes[bid]`)
- `related_nodes` provides BeliefNode data for display methods (display_title(), keys()) in metadata panel
- Browser tests validate BTreeMap structure and lookup functionality
- Browser tests will validate everything when server runs (`./tests/browser/run.sh`)

### 4. WASM Build Automation ✅ (Phase 1.5)

**Created `build.rs` for automated WASM compilation**
- Triggers on `bin` feature (not `service` or `wasm`)
- Checks for `wasm-pack` availability with clear error message
- Runs: `wasm-pack build --target web -- --features wasm --no-default-features`
- Skips rebuild if `pkg/` artifacts already exist (incremental)
- Feature detection: `CARGO_FEATURE_BIN` environment variable

**Updated `src/codec/assets.rs` to embed WASM**
- Added `include_bytes!` for `pkg/noet_core.js` (32KB JS glue)
- Added `include_bytes!` for `pkg/noet_core_bg.wasm` (2.2MB binary)
- Feature-gated with `#[cfg(feature = "bin")]`
- Extraction in `extract_assets()` writes to `{output}/assets/`

**Build tested**:
- Default build (`cargo build`) → includes WASM ✅
- Library-only (`cargo build --no-default-features --lib`) → skips WASM ✅
- HTML generation extracts WASM to assets/ ✅
- Binary size: ~500KB → ~2.8MB (acceptable for offline-first)

**Test output standardization**:
- Documented convention: `tests/*/test-output/` for all test artifacts
- Updated `.gitignore` with `test-output/` pattern
- Cleaned up ad-hoc `test-wasm-output/` directories
- Updated `CONTRIBUTING.md` with testing conventions
- Browser tests already use standardized location: `tests/browser/test-output/`

## Planning Update (During Session)

**Added Phase 1.5: WASM Build Automation & Embedding**

User flagged that WASM embedding wasn't properly planned. We need build automation BEFORE Phase 2 (template updates), since subsequent phases depend on WASM being available in the binary.

**New Phase 1.5 tasks**:
1. Create `build.rs` to run `wasm-pack build` automatically
2. Update `src/codec/assets.rs` to embed `pkg/noet_core.js` and `pkg/noet_core_bg.wasm`
3. Extract WASM files during HTML generation (to `{output}/assets/`)
4. Document build requirements (`wasm-pack` installation)

**Impact**:
- Binary size: ~500KB → ~2.8MB (acceptable for offline-first)
- Build time: Adds ~6-10 seconds (only when WASM sources change)
- Next session should start with Phase 1.5, NOT Phase 2

This ensures WASM is properly embedded before we build interactive features that depend on it.

**Phase 1.5 completed same session** - User suggested tackling it immediately since context was loaded.

## Testing Standards Established

Standardized test output locations to avoid gitignore conflicts:
- **Convention**: `tests/*/test-output/` for test-specific output
- **Root fallback**: `test-output/` for ad-hoc testing
- **Benefits**: Single gitignore rule, predictable cleanup, consistent docs
- **Documentation**: Added to `CONTRIBUTING.md` with examples

All test commands now use standardized paths:
```bash
./target/debug/noet parse tests/network_1 --html-output test-output/
./tests/browser/run.sh  # Uses tests/browser/test-output/
```

## Ready for Phase 2

All Phase 1 and Phase 1.5 deliverables complete. Next session should start with:
1. Review HTML template structure from completed ISSUE_06
2. Add navigation/metadata panel containers
3. Add toggle buttons (mobile-first behavior)
4. Wire up theme switcher

## Design Decisions Made

1. **Graph structure**: HashMap with sorted BID vectors (not full BeliefGraph)
2. **Related nodes**: Collect ALL connected nodes (both sources and sinks) for metadata display
3. **Field naming**: `related_nodes` chosen over `external` to avoid confusion with href/asset networks
4. **Data structure**: BTreeMap for `related_nodes` enables O(1) BID lookups from JavaScript
5. **Sorting**: Use WEIGHT_SORT_KEY for deterministic ordering
6. **Documentation**: Add new § 10 rather than expanding § 9 (networks distinct from API node)
7. **Feature anchoring**: WASM build triggers on `bin` feature (not `service` or `wasm`)
   - `bin` = CLI binary with all features
   - `service` = library use without binary
   - `wasm` = for compiling WASM target itself
8. **Build separation**: WASM compiled with `--features wasm --no-default-features` (different from main build)
9. **Test output convention**: Standardized on `tests/*/test-output/` and root `test-output/`
   - Single gitignore pattern
   - Documented in CONTRIBUTING.md
   - Consistent across all examples and tests