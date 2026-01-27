# SCRATCHPAD - Issue 22 Implementation Handoff

**Date**: 2025-01-27
**Status**: READY FOR IMPLEMENTATION
**Next Session Goal**: Implement speculative path solution to fix duplicate node deduplication bug

---

## Quick Context

**Problem**: Two markdown headings with same title (e.g., `## Details` twice) create only ONE node instead of two.

**Root Cause**: `GraphBuilder::push()` uses `Title` key for cache_fetch, which matches both headings incorrectly.

**Solution**: Use `EventOrigin::Speculative` to generate position-aware paths, remove Title key from section lookups.

---

## What's Already Done ✅

### 1. Analysis Complete
- Full truth table documented in `ISSUE_22_DUPLICATE_NODE_DEDUPLICATION.md`
- Root cause identified and validated
- Solution approach designed and reviewed

### 2. Infrastructure Ready
- ✅ `EventOrigin::Speculative` added to `src/event.rs`
- ✅ 13 unit tests created in `src/codec/builder.rs::tests` (all passing)
- ✅ Tests cover all truth table cases including explicit ID collisions

### 3. Related Work Complete (Issue 03)
- ✅ MdCodec collision detection works (sets `id = None` for collisions)
- ✅ MdCodec inject_context() generates Bref when `id = None`
- These work correctly; GraphBuilder just needs to stop deduplicating nodes

### 4. Documentation
- ✅ `ISSUE_22_DUPLICATE_NODE_DEDUPLICATION.md` - Complete with solution
- ✅ This handoff document (contains all analysis + implementation details)

---

## What Needs Implementation

### Phase 1: EventOrigin::Speculative in BeliefBase (Core Infrastructure)

**File**: `src/beliefbase.rs`

**Task**: Modify `BeliefBase::process_event()` to handle `EventOrigin::Speculative`

**Requirements**:
1. When origin is `Speculative`:
   - Process event logic normally
   - Generate derivative events (PathAdded, etc.)
   - **DO NOT mutate state** (no insertions, no index updates)
   - Return derivative events to caller
   
2. When origin is `Local` or `Remote`:
   - Existing behavior (mutate state)

**Pseudocode**:
```rust
pub fn process_event(&mut self, event: &BeliefEvent) -> Result<Vec<BeliefEvent>> {
    match event.origin() {
        Some(EventOrigin::Speculative) => {
            // Clone current state
            // Process event on cloned state
            // Collect derivative events
            // Return events WITHOUT applying to self
        }
        Some(EventOrigin::Local) | Some(EventOrigin::Remote) | None => {
            // Existing logic (mutate self)
        }
    }
}
```

**Testing**:
- Create unit test: process speculative event, verify state unchanged
- Verify derivative events returned correctly
- Verify subsequent non-speculative events work normally

---

### Phase 2: Speculative Section Path in GraphBuilder

**File**: `src/codec/builder.rs`

**Task 1**: Create `fn speculative_section_path()` helper

**Signature**:
```rust
fn speculative_section_path(
    proto: &ProtoBeliefNode,
    parent_bid: Bid,
    session_bb: &mut BeliefBase,
) -> Result<String, BuildonomyError>
```

**Algorithm**:
```rust
fn speculative_section_path(...) -> Result<String> {
    // 1. Create a temporary BeliefNode from proto
    let temp_node = BeliefNode::try_from(proto)?;
    
    // 2. Create speculative RelationInsert event
    //    (section -> parent with Section weight kind)
    let event = BeliefEvent::RelationInsert(
        temp_node.bid,
        parent_bid,
        WeightKind::Section,
        Weight::default(), // sort_key will be auto-assigned
        EventOrigin::Speculative,
    );
    
    // 3. Process speculative event to get derivative events
    let derivative_events = session_bb.process_event(&event)?;
    
    // 4. Find PathAdded event in derivatives
    let path_added = derivative_events.iter().find_map(|e| {
        if let BeliefEvent::PathAdded(_, path, _, _, _) = e {
            Some(path.clone())
        } else {
            None
        }
    });
    
    // 5. Return path or error
    path_added.ok_or_else(|| BuildonomyError::Codec("No path generated".into()))
}
```

**Task 2**: Modify `push()` to use speculative path for sections

**Location**: `src/codec/builder.rs::push()` around line 810

**Current code**:
```rust
let mut keys = parsed_node.keys(Some(self.repo()), Some(parent_bid), self.doc_bb());
```

**New code**:
```rust
let mut keys = if proto.heading > 2 {
    // Section heading - use speculative path, NO Title key
    let speculative_path = speculative_section_path(proto, parent_bid, &mut self.session_bb)?;
    
    let net = self.repo().bid;
    let mut section_keys = vec![
        NodeKey::Path { net, path: speculative_path },
    ];
    
    // Add ID key if present
    if let Some(ref id) = proto.id {
        section_keys.push(NodeKey::Id { net, id: id.clone() });
    }
    
    // Add BID key if initialized (Phase 2+)
    if parsed_node.bid.initialized() {
        section_keys.push(NodeKey::Bid { bid: parsed_node.bid });
    }
    
    section_keys
} else {
    // Document node - use existing logic with all keys
    parsed_node.keys(Some(self.repo()), Some(parent_bid), self.doc_bb())
};
```

---

### Phase 3: Testing and Validation

**Integration Test**: `tests/codec_test.rs::test_anchor_collision_detection`

**Expected change**:
- Before: "Found 3 heading nodes" (bug)
- After: "Found 4 heading nodes" (fixed)

**Steps**:
1. Run `cargo test test_anchor_collision_detection --test codec_test -- --nocapture`
2. Verify output shows 4 nodes
3. Verify two "Details" nodes with different BIDs
4. Uncomment TODO assertions in test

**Regression Testing**:
```bash
cargo test --lib                    # All 83 unit tests should pass
cargo test --test codec_test        # All 9 integration tests should pass
cargo test --doc                    # All doc tests should pass
```

**Watch for**:
- Document-level nodes still work (heading ≤ 2)
- Forward references still resolve
- Multi-pass compilation works
- Section metadata enrichment (Issue 02) still works

---

## Current Bug Evidence

**Test**: `tests/codec_test.rs::test_anchor_collision_detection`

**Fixture**: `tests/network_1/anchors_collision_test.md`
```markdown
## Details
First occurrence...

## Implementation
...

## Details
Second occurrence - collision!

## Testing
...
```

**Current Output**:
```
Found 3 heading nodes
  - Details (bid: 1f0fb19c-f674-6e1f-b0a8-e90bd62e86e8)
  - Implementation (bid: ...)
  - Testing (bid: ...)
Found 1 'Details' headings  # ❌ Should be 2!
```

**After Fix**:
```
Found 4 heading nodes
  - Details (bid: aaa...)
  - Implementation (bid: ...)
  - Details (bid: bbb...)  # ✅ Second Details node!
  - Testing (bid: ...)
Found 2 'Details' headings  # ✅ Correct!
```

---

## Key Architectural Insights

1. **Title is NOT an identity key for sections**
   - It's just metadata
   - Two sections can have same title (e.g., "Details")
   - Position in structure is what makes them unique

2. **Path encodes structural identity**
   - Format: `parent_path#anchor`
   - Anchor is ID (if unique) or Bref (if collision)
   - Includes parent hierarchy + sibling order

3. **Speculative events guarantee correctness**
   - Use same PathMap logic as actual insertion
   - No code duplication
   - Future-proof (PathMap changes automatically reflected)

4. **Explicit IDs can collide too**
   - User can manually add `{#intro}` to multiple headings
   - Must detect and warn, use Bref fallback
   - Test: `test_speculative_path_explicit_id_collision`

---

## Files to Modify

### Must Change:
1. `src/beliefbase.rs` - Add Speculative event handling
2. `src/codec/builder.rs` - Add `speculative_section_path()` and modify `push()`

### Will Change (automatically via tests):
3. `tests/codec_test.rs` - Uncomment TODO assertions after fix

### Already Changed (no further action):
- ✅ `src/event.rs` - EventOrigin::Speculative added
- ✅ `src/codec/md.rs` - Collision detection works
- ✅ `docs/project/ISSUE_22_DUPLICATE_NODE_DEDUPLICATION.md` - Fully documented

---

## Potential Issues and Solutions

### Issue: Speculative events affect performance
**Mitigation**: Only used during parse (Phase 1), not in hot paths. Session_bb is small (current document only).

### Issue: Derivative events don't include path
**Solution**: PathMap MUST generate PathAdded events. Check PathMapMap::process_event() logic.

### Issue: BID not initialized during speculation
**Expected**: First parse (Phase 1), BID not initialized yet. Generate temp BID for speculation, discard after.

### Issue: Speculative event mutates state accidentally
**Critical**: Ensure deep clone or isolated processing. Unit test this carefully!

---

## Success Criteria

- [ ] test_anchor_collision_detection shows 4 heading nodes
- [ ] Two "Details" nodes have different BIDs
- [ ] First "Details" has ID="details"
- [ ] Second "Details" has ID=<bref> (12 hex chars)
- [ ] All 83 unit tests pass
- [ ] All 9 integration tests pass
- [ ] No regressions in Issue 02 (section metadata)
- [ ] No regressions in Issue 03 (anchor injection)

---

## Debugging Tips

### If nodes still deduplicate:
1. Check that Title key is NOT in section keys
2. Verify speculative_section_path() returns different paths for duplicate titles
3. Add logging: `tracing::info!("Section keys: {:?}", keys);`
4. Check cache_fetch is actually using Path key for match

### If speculative events mutate state:
1. Verify EventOrigin::Speculative check in process_event()
2. Add assertion: `let before = session_bb.states().len(); // ... assert_eq!(before, after);`
3. Check PathMap doesn't update indices on Speculative

### If paths don't match final paths:
1. Compare speculative path to actual path after full parse
2. Check sort_key assignment logic
3. Verify ID collision detection matches between speculation and actual

---

## Next Steps for Implementation

1. Start with BeliefBase::process_event() Speculative handling
2. Write unit test for Speculative event (no state mutation)
3. Implement speculative_section_path() with logging
4. Test speculative path generation in isolation
5. Modify push() to use speculative path for sections
6. Run test_anchor_collision_detection - should pass!
7. Run full test suite for regressions
8. Uncomment TODO assertions in tests
9. Clean up logging if verbose
10. Update ISSUE_22 status to COMPLETE

---

**Estimated Time**: 2-3 hours for implementation + testing

**Confidence**: HIGH - Solution is well-designed, infrastructure is ready, tests are in place.

**Blocker**: None - all dependencies resolved.

---

## Additional Consideration: PathMap title_map Collisions

**Context**: During session review, user raised concern about `title_map` collision handling in `src/paths.rs`.

**Current Behavior** (lines 810-820):
```rust
let mut title_map = IdMap::default();
for (_, bid, _) in map.iter() {
    if let Some(title) = nets.anchors().get(bid) {
        if !nets.is_anchor(bid) && !title.is_empty() {
            title_map.insert(title.clone(), *bid);
        }
    }
}
```

**Key Constraint**: `!nets.is_anchor(bid)` means only **document nodes** (not section headings) are added to title_map.
- `nets.is_anchor(bid)` returns `true` for sections (heading > 2)
- Only documents (heading ≤ 2) have titles in title_map

**IdMap::insert Behavior** (lines 57-72):
- **Last-write-wins**: If two documents have same title, second one overwrites first
- No warning or error on collision
- Silently discards previous mapping

**Analysis**:
1. **Is this a problem?** Yes, if two document files have same title:
   - `NodeKey::Title { net, title }` lookups will only find the last document
   - First document becomes unreachable via title key
   - This violates uniqueness assumption for NodeKey::Title

2. **Does Issue 22 fix this?** No, Issue 22 only addresses sections (heading > 2)
   - Documents still use Title key in cache_fetch
   - Document title collisions would still cause wrong matches

3. **Is this realistic?** Possibly:
   - Two files: `intro.md` and `getting-started.md`
   - Both have frontmatter: `title = "Introduction"`
   - title_map would only keep one of them

**Recommendation**:
- **Option A**: Warn on document title collision during title_map.insert()
  - Add logging: `tracing::warn!("Document title collision: '{}' used by multiple files", title)`
  - Keep last-write-wins behavior (safest for now)
  
- **Option B**: Make document titles network-unique (validation)
  - Return error if two documents have same title
  - Force users to disambiguate
  
- **Option C**: Don't use Title key for document cache_fetch either
  - Similar to Issue 22 solution for sections
  - Only use Path and BID keys for documents too
  
**For This Session**: 
- Issue 22 implementation should proceed as planned (sections only)
- Create **separate issue** for document title collision handling
- Document this as known limitation in Issue 22 notes

**Related Code**:
- `src/paths.rs:810-820` - title_map insertion (no collision detection)
- `src/paths.rs:55-72` - IdMap::insert (last-write-wins)
- `src/codec/builder.rs:810` - push() uses Title key for documents

---

## Future Enhancement: Diagnostic Stream for LSP Integration

**Context**: User identified that tracing logs (warn, info, debug) should be converted to ParseDiagnostic for LSP integration (Issue 11).

**Opportunity**: The title collision warnings we're adding in Issue 22 are perfect candidates:

```rust
// Current approach (tracing):
tracing::warn!(
    "Explicit ID '{}' collides with sibling. Using Bref fallback.",
    candidate_id
);

// Future approach (diagnostic stream):
diagnostics.push(ParseDiagnostic::warning(
    format!("Explicit ID '{}' collides with sibling. Using Bref fallback.", candidate_id)
).with_location(line, column));
```

**Benefits for LSP**:
1. **IDE Integration**: Warnings appear inline in editor
2. **Actionable**: User can click to see exact location
3. **Structured**: Machine-readable for tooling
4. **Persistent**: Survives across parse passes

**Examples of logs to convert** (audit in Issue 11):
- `src/codec/builder.rs`: BID mismatch warnings
- `src/codec/md.rs`: ID normalization, collision detection
- `src/paths.rs`: Title collision in title_map (future issue)
- `src/codec/md.rs::inject_context()`: Network-level collision warnings

### Integration with Instrumentation Design

**Reference**: `docs/design/instrumentation_design.md` (v0.1)

The instrumentation system provides a proven pattern for structured data capture using `tracing`:

**Key Concepts from Instrumentation Design**:
1. **RoutingLayer Pattern**: Single subscriber routes events to specialized layers
   - `CsvCaptureLayer` for sensor data → CSV
   - `AnnotationLayer` for metadata
   - `fmt::Layer` for standard logs
   
2. **Structured Event Metadata**: Events tagged with `header` field identifying type
   - `NOET_DATA_CAPTURE` for sensor data
   - `NOET_ANNOTATION` for metadata
   - Could add: `NOET_PARSE_DIAGNOSTIC` for compilation diagnostics

3. **Zero-cost when disabled**: Layers check active state before processing

**Proposed Architecture for Diagnostics**:

```rust
// Add diagnostic capture using instrumentation pattern
#[macro_export]
macro_rules! capture_diagnostic {
    ($diagnostic:expr) => {
        tracing::event!(
            target: "noet_diagnostics",
            tracing::Level::INFO,
            header = "NOET_PARSE_DIAGNOSTIC",
            data = ?serde_json::to_string(&$diagnostic).unwrap_or_default()
        );
    };
}

// Custom layer for diagnostic collection
struct DiagnosticCaptureLayer {
    state: Arc<Mutex<DiagnosticState>>,
}

struct DiagnosticState {
    active: bool, // Enables collection
    diagnostics: Vec<ParseDiagnostic>,
}
```

**Dual Output Pattern**:
```rust
// In builder.rs or md.rs:
if parsed_node.bid.initialized() && found_node.bid.initialized() 
    && parsed_node.bid != found_node.bid 
{
    let diagnostic = ParseDiagnostic::warning(
        format!("BID mismatch: parsed={}, found={}", 
                parsed_node.bid, found_node.bid)
    ).with_location(line, column);
    
    // Option 1: Traditional logging (always)
    tracing::info!(
        "BID mismatch: parsed={}, found={}. Creating new node.",
        parsed_node.bid, found_node.bid
    );
    
    // Option 2: Structured diagnostic capture (when enabled)
    capture_diagnostic!(diagnostic);
    
    // Option 3: Store for return (requires DocCodec changes)
    diagnostics.push(diagnostic);
}
```

**RoutingLayer Integration**:
```rust
// In subscriber setup (similar to instrumentation_design.md):
let subscriber = tracing_subscriber::registry()
    .with(RoutingLayer {
        diagnostic_layer: DiagnosticCaptureLayer::new(state.clone()),
        fmt_layer: fmt::layer().with_target(false),
    });
```

**Benefits of Instrumentation Pattern**:
- ✅ Proven design from production system
- ✅ Separates instrumentation (where) from collection (what)
- ✅ Zero-cost when disabled (crucial for noet-core performance)
- ✅ Extensible to multiple outputs (LSP, file, analytics)
- ✅ Thread-safe via Arc<Mutex> pattern

**Implementation Strategy** (for Issue 11):
1. Audit codebase for `tracing::{warn, info}` calls
2. Identify those related to parse-time issues (not runtime logs)
3. Create `DiagnosticCaptureLayer` following instrumentation pattern
4. Add `capture_diagnostic!` macro following `capture_data!` pattern
5. Dual-emit: traditional log + structured diagnostic
6. Thread diagnostics through DocCodec trait (breaking change)
7. Surface diagnostics to LSP via `textDocument/publishDiagnostics`

**Migration Path**:
- **Phase 1**: Add diagnostic capture alongside existing tracing (non-breaking)
- **Phase 2**: Make DocCodec return diagnostics (breaking, requires Issue 11)
- **Phase 3**: Remove redundant tracing calls once diagnostics working

**For Issue 22**:
- Use tracing for now (simpler, non-blocking)
- Mark locations with comments for future conversion:
  ```rust
  // TODO(Issue-11-diagnostics): Convert to capture_diagnostic!()
  tracing::warn!("...");
  ```

**Instrumentation Design Summary**:

The `instrumentation_design.md` provides a battle-tested pattern for structured data capture:
- **RoutingLayer**: Single subscriber distributes events to specialized layers
- **Typed Events**: Events tagged with `header` field (e.g., `NOET_DATA_CAPTURE`, `NOET_ANNOTATION`)
- **State Management**: Arc<Mutex<State>> for thread-safe control
- **Zero-cost**: Layers check `active` flag before processing
- **FFI Control**: start/stop_session functions for external control

This pattern maps perfectly to diagnostic capture:
- Parse-time logs → `NOET_PARSE_DIAGNOSTIC` events
- `DiagnosticCaptureLayer` collects structured ParseDiagnostic objects
- LSP queries diagnostics for `textDocument/publishDiagnostics`
- Development mode writes diagnostics to CSV for analysis

**Migration Strategy**:
1. Add `capture_diagnostic!` macro alongside existing `tracing::warn!`
2. Create `DiagnosticCaptureLayer` following instrumentation pattern
3. Collect diagnostics in thread-safe store
4. Expose via LSP when ready (Issue 11)
5. Remove redundant tracing once LSP working

**Related**:
- Issue 11: Basic LSP Implementation (added diagnostic capture task)
- `src/codec/diagnostic.rs` - ParseDiagnostic types already defined
- `docs/design/instrumentation_design.md` - Proven tracing-based pattern
- DocCodec trait would need `diagnostics: Vec<ParseDiagnostic>` return (breaking change)

---

## References

- **Issue Doc**: `docs/project/ISSUE_22_DUPLICATE_NODE_DEDUPLICATION.md` (full architecture)
- **Tests**: `src/codec/builder.rs::tests` (13 tests, all passing)
- **Fixture**: `tests/network_1/anchors_collision_test.md`
- **Quick Start**: `.scratchpad/NEXT_SESSION_START_HERE.md`
- **Related**: `src/paths.rs` - PathMap title_map collision handling (needs separate issue)
