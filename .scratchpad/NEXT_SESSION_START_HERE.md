# START HERE - Next Session Quick Reference

**Goal**: Implement speculative path solution to fix duplicate node bug

---

## The Bug (5 second version)

Two `## Details` headings → only 1 node created (wrong!)

**Why**: `GraphBuilder::push()` matches on `Title` key, which isn't unique for sections.

**Fix**: Use `EventOrigin::Speculative` to generate position-aware paths, remove Title key.

---

## Implementation Checklist

### Step 1: BeliefBase Speculative Events (30 min)
**File**: `src/beliefbase.rs`

Add to `process_event()`:
```rust
match event.origin() {
    Some(EventOrigin::Speculative) => {
        // Process on clone, return events, DON'T mutate self
    }
    _ => {
        // Existing logic (mutate self)
    }
}
```

**Test**: Unit test verifies state unchanged after speculative event

### Step 2: Speculative Section Path (1 hour)
**File**: `src/codec/builder.rs`

Add function:
```rust
fn speculative_section_path(
    proto: &ProtoBeliefNode,
    parent_bid: Bid,
    session_bb: &mut BeliefBase,
) -> Result<String> {
    // 1. Create temp BeliefNode
    // 2. Create RelationInsert with EventOrigin::Speculative
    // 3. Process event, get derivative PathAdded
    // 4. Extract path from PathAdded
    // 5. Return path
}
```

**Test**: Call with duplicate titles, verify different paths returned

### Step 3: Modify push() (30 min)
**File**: `src/codec/builder.rs` line ~810

Replace:
```rust
let mut keys = parsed_node.keys(...);
```

With:
```rust
let mut keys = if proto.heading > 2 {
    // Section: use speculative path, NO Title key
    let path = speculative_section_path(proto, parent_bid, &mut self.session_bb)?;
    vec![
        NodeKey::Path { net, path },
        // Add ID if present
        // Add BID if initialized
    ]
} else {
    // Document: existing logic
    parsed_node.keys(...)
};
```

### Step 4: Verify Fix (30 min)
```bash
cargo test test_anchor_collision_detection --test codec_test -- --nocapture
```

**Expected**: "Found 4 heading nodes" (not 3!)

### Step 5: Full Regression (15 min)
```bash
cargo test --lib        # 83 tests
cargo test --test       # 9 tests
```

---

## Quick References

**Implementation Guide**: `.scratchpad/issue_22_implementation_handoff.md` (all details)
**Architecture**: `docs/project/ISSUE_22_DUPLICATE_NODE_DEDUPLICATION.md` (full design)
**Test fixture**: `tests/network_1/anchors_collision_test.md`

---

## Key Insights

1. **Title is NOT unique** for sections → can't use as identity key
2. **Path IS unique** because it includes parent + position + anchor
3. **Speculative events** let us reuse PathMap logic (no duplication!)
4. **Explicit IDs can collide too** → must check and warn

---

## If Something Goes Wrong

**Nodes still deduplicate?**
- Check Title key NOT in section keys
- Log: `tracing::info!("Keys: {:?}", keys);`

**Speculative events mutate state?**
- Verify Speculative check in process_event()
- Add assertion: state size before == after

**Paths don't match?**
- Compare speculative path to final path after parse
- Check sort_key logic

---

**Time Estimate**: 2-3 hours total
**Confidence**: HIGH - all prep work done, just execute!
**Status**: All 83 tests passing, ready to implement

---

Good luck! 🚀