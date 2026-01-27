# Issue 22: Duplicate Node Deduplication Bug in GraphBuilder

**Priority**: HIGH  
**Estimated Effort**: 2-3 days  
**Dependencies**: None (blocks Issue 03 full verification)  
**Status**: Open

## Summary

Two markdown headings with the same title (e.g., `## Details` twice) incorrectly create only ONE node in BeliefBase instead of two separate nodes. This is caused by premature key speculation in `GraphBuilder::push()` that matches the second heading to the first heading's cached node before considering the unique BID.

This bug blocks full verification of Issue 03's collision detection behavior, where two "Details" headings should create two nodes with different IDs (first: "details", second: Bref fallback).

## Problem Statement

**Expected Behavior**:
```markdown
## Details         <!-- Node 1: BID=aaa, ID="details" -->
Some content...

## Details         <!-- Node 2: BID=bbb, ID="a1b2c3d4e5f6" (Bref) -->
More content...
```
- Two separate nodes in BeliefBase
- Different BIDs, different IDs
- Both accessible independently

**Current Behavior**:
- Only ONE node in BeliefBase.states()
- Second "Details" heading overwrites/merges with first
- Collision detection works correctly during parse
- But nodes deduplicate during `GraphBuilder::push()`

## Root Cause

**Location**: `src/codec/builder.rs:798-850` (`GraphBuilder::push()`)

**The Problem**:

```rust
async fn push(...) -> Result<...> {
    // Line ~809: Convert ProtoBeliefNode to BeliefNode
    let mut parsed_node = BeliefNode::try_from(proto)?;
    
    // Line ~810: Generate keys from parsed node
    let mut keys = parsed_node.keys(Some(self.repo()), Some(parent_bid), self.doc_bb());
    
    // Line ~816: Cache fetch BEFORE considering BID uniqueness
    let cache_fetch_result = self
        .cache_fetch(&keys, global_bb.clone(), true, missing_structure)
        .await?;
}
```

**What Happens**:

1. **First "Details" heading**:
   - BID: `aaa` (unique)
   - Title: "Details"
   - ID: "details" (title-derived)
   - Keys: `[Bid{aaa}, Bref{...}, Id{net, "details"}, Title{net, "details"}, Path{...}]`
   - `cache_fetch()`: No match (first occurrence) → creates new node
   - ✅ Node inserted into session_bb

2. **Second "Details" heading**:
   - BID: `bbb` (unique, different from first!)
   - Title: "Details" (same as first)
   - ID: "a1b2c3d4e5f6" (Bref fallback due to collision)
   - Keys: `[Bid{bbb}, Bref{...}, Id{net, "a1b2c3d4e5f6"}, Title{net, "details"}, Path{...}]`
   - ⚠️ `cache_fetch(&keys, ...)`: **MATCHES on `Title{net, "details"}`**
   - Returns first node (BID=aaa)
   - Second node's BID (bbb) ignored
   - ❌ Nodes merge/overwrite instead of creating separate entry

**The Bug**: `cache_fetch()` matches on `Title` key before we've validated that the BID is actually different. The second heading should create a new node with a different BID, but instead it matches the cached first heading.

## Why This Is Tricky

From the issue comment:
> "This will be tricky to figure out."

**The Challenge**:

At the point where we call `cache_fetch()` (line ~816), we **cannot reliably use self.doc_bb.paths()** because:
- We're in the middle of parsing (phase 1)
- doc_bb is not balanced/complete yet
- Comment on line ~808: "Can't use self.doc_bb.paths() to generate keys here, because we can't assume that self.doc_bb is balanced until we're out of phase 1 of parse_content."

**Normally**: The only reliable key at this point is `NodeKey::Path` (filesystem path)

**But**: For markdown headings (sections):
- They don't have unique filesystem paths (multiple headings in same file)
- They share titles (causing the collision we're trying to handle)
- BID is the ONLY guaranteed unique identifier

**The Catch-22**:
1. We need to call `cache_fetch()` to find existing nodes
2. But `cache_fetch()` uses `keys` which includes `Title`
3. Title matches cause wrong cache hits
4. We can't know if it's a wrong hit without checking BID first
5. But if we already have the BID, why are we doing cache_fetch?

## Architecture Context

**Multi-Pass Compilation Model**:
- **Phase 1**: Parse all files, collect unresolved references
- **Phase 2+**: Reparse with resolved dependencies, inject BIDs
- During Phase 1, we don't have BIDs yet (they're auto-generated)
- During Phase 2+, we have BIDs and need to match against cached nodes

**GraphBuilder Role**:
- Orchestrates parsing and linking
- Maintains caches (doc_bb, session_bb) for multi-pass resolution
- `cache_fetch()` looks up nodes by keys to enable forward references

**The Speculation Problem**:
We "speculate" keys before we know if the node exists. For documents (files), this works because:
- Path is unique
- Title is typically unique
- BID uniqueness is enforced

For headings (sections), this breaks because:
- Path is NOT unique (multiple headings in same file)
- Title is NOT unique (duplicate headings are common)
- BID is unique but we're speculating OTHER keys first

## Test Evidence

**Test**: `tests/codec_test.rs::test_anchor_collision_detection`

**Test Fixture**: `tests/network_1/anchors_collision_test.md`
```markdown
## Details
First occurrence...

## Implementation
Unique title...

## Details
Second occurrence - collision!
```

**Expected**: 4 heading nodes (Details, Implementation, Details, Testing)  
**Actual**: 3 heading nodes (Details is deduplicated)

**Test Output**:
```
Found 3 heading nodes
  - Details (bid: 1f0fb19c-f674-6e1f-b0a8-e90bd62e86e8)
  - Implementation (bid: ...)
  - Testing (bid: ...)
Found 1 'Details' headings  # Should be 2!
```

## Potential Solutions

### Option 1: BID-First Matching (Preferred)

**Idea**: If parsed node has a BID, check BID match BEFORE title match in cache_fetch.

**Implementation**:
```rust
async fn push(...) -> Result<...> {
    let mut parsed_node = BeliefNode::try_from(proto)?;
    
    // If parsed node has a BID (Phase 2+), try BID-only lookup first
    let cache_fetch_result = if parsed_node.bid.initialized() {
        // Try exact BID match first
        let bid_keys = vec![NodeKey::Bid { bid: parsed_node.bid }];
        match self.cache_fetch(&bid_keys, global_bb.clone(), false, missing_structure).await? {
            GetOrCreateResult::Resolved(node, src) => {
                // Exact match - use it
                GetOrCreateResult::Resolved(node, src)
            }
            GetOrCreateResult::Unresolved(_) => {
                // No BID match - this is a NEW node, generate all keys
                let keys = parsed_node.keys(...);
                // Filter out Title key to prevent wrong matches?
                self.cache_fetch(&keys, global_bb, true, missing_structure).await?
            }
        }
    } else {
        // Phase 1: no BID yet, use normal key speculation
        let keys = parsed_node.keys(...);
        self.cache_fetch(&keys, global_bb, true, missing_structure).await?
    };
    
    // ... rest of function
}
```

**Pros**:
- BID is guaranteed unique
- Only matches when it's actually the same node
- Preserves existing Phase 1 behavior

**Cons**:
- Changes fundamental matching semantics
- May break forward references in subtle ways
- Requires careful testing of multi-pass scenarios

### Option 2: Filter Title Key for Headings

**Idea**: Don't include `Title` key in speculation for heading nodes.

**Implementation**:
```rust
async fn push(...) -> Result<...> {
    let mut parsed_node = BeliefNode::try_from(proto)?;
    let mut keys = parsed_node.keys(...);
    
    // For heading nodes (proto.heading > 2), remove Title key
    if proto.heading > 2 {
        keys.retain(|k| !matches!(k, NodeKey::Title { .. }));
    }
    
    let cache_fetch_result = self.cache_fetch(&keys, ...).await?;
    // ...
}
```

**Pros**:
- Minimal changes
- Targeted fix for specific problem

**Cons**:
- Breaks legitimate title-based matching for headings
- May need special handling for other keys too (ID, Anchor)
- Doesn't address root cause

### Option 3: Post-Match BID Validation

**Idea**: After `cache_fetch()` match, validate that BID matches (if we have one).

**Implementation**:
```rust
async fn push(...) -> Result<...> {
    let mut parsed_node = BeliefNode::try_from(proto)?;
    let mut keys = parsed_node.keys(...);
    let cache_fetch_result = self.cache_fetch(&keys, ...).await?;
    
    let (mut node, source) = match cache_fetch_result {
        GetOrCreateResult::Resolved(mut found_node, src) => {
            // NEW: Validate BID if both nodes have one
            if parsed_node.bid.initialized() && found_node.bid.initialized() {
                if parsed_node.bid != found_node.bid {
                    // Different BIDs - this is a WRONG match!
                    // Treat as unresolved and create new node
                    tracing::info!(
                        "BID mismatch: parsed={}, found={}. Creating new node.",
                        parsed_node.bid, found_node.bid
                    );
                    // Create new node with parsed_node's data
                    (parsed_node, NodeSource::Generated)
                } else {
                    // Same BID - legitimate match
                    (found_node, src)
                }
            } else {
                // One or both nodes don't have BID - proceed with match
                (found_node, src)
            }
        }
        GetOrCreateResult::Unresolved(_) => {
            (parsed_node, NodeSource::Generated)
        }
    };
    
    // ... rest of function
}
```

**Pros**:
- Minimal invasive change
- Preserves existing speculation logic
- Only rejects matches that are provably wrong

**Cons**:
- Might create duplicate entries in cache
- Unclear how to "uncache" the wrong match
- May need to update cache indices

### Option 4: Defer Heading Node Creation

**Idea**: Don't call `push()` for heading nodes during Phase 1; wait until Phase 2+ when BIDs are known.

**Pros**:
- Avoids speculation entirely for headings

**Cons**:
- Major architectural change
- Breaks forward references to headings
- Likely causes other issues

## Testing Strategy

**Unit Tests**:
1. Test `cache_fetch()` with duplicate titles but different BIDs
2. Test `push()` with two ProtoBeliefNodes with same title, different BIDs
3. Test key speculation with and without initialized BIDs

**Integration Tests**:
1. Parse document with duplicate heading titles
2. Verify two separate nodes created in BeliefBase
3. Verify both nodes have correct IDs (collision detection worked)
4. Verify both nodes accessible by their respective BIDs

**Regression Tests**:
1. Ensure forward references still work (Issue 01)
2. Ensure multi-pass compilation still works
3. Ensure BID injection still works
4. Ensure sections metadata enrichment still works (Issue 02)

## Success Criteria

- [ ] Two "Details" headings create two separate nodes in BeliefBase
- [ ] First "Details" has ID="details"
- [ ] Second "Details" has ID=<bref> (Bref fallback)
- [ ] Both nodes have different BIDs
- [ ] Both nodes are independently accessible
- [ ] All existing tests still pass (95 tests)
- [ ] test_anchor_collision_detection can uncomment detailed assertions
- [ ] test_anchor_selective_injection can verify two nodes

## Risks

**Risk**: Breaking forward reference resolution  
**Mitigation**: Extensive testing of multi-pass compilation, particularly Issue 01 scenarios

**Risk**: Performance impact from additional BID validation  
**Mitigation**: Validation only runs when both nodes have BIDs (Phase 2+)

**Risk**: Cache inconsistencies if we reject matches  
**Mitigation**: Need careful design of how to "create new node" after rejecting cache match

**Risk**: May surface other deduplication bugs  
**Mitigation**: Comprehensive test coverage, start with Option 3 (least invasive)

## Implementation Plan

### Phase 1: Investigation (0.5 days)
1. Add extensive logging to `push()` and `cache_fetch()`
2. Run test_anchor_collision_detection with tracing
3. Confirm exact sequence of events causing duplication
4. Document findings

### Phase 2: Prototype (1 day)
1. Implement Option 3 (Post-Match BID Validation)
2. Test with anchors_collision_test.md
3. Verify two "Details" nodes created
4. Check for cache issues or side effects

### Phase 3: Testing (0.5 days)
1. Run full test suite
2. Test multi-pass compilation scenarios
3. Test forward references
4. Ensure no regressions

### Phase 4: Refinement (0.5-1 day)
1. If Option 3 has issues, try Option 1 (BID-First Matching)
2. Add diagnostic logging for BID mismatches
3. Update documentation
4. Clean up any temporary debugging code

## References

- Issue 03: Section Heading Anchor Management (blocked on this)
- `src/codec/builder.rs:798-979` - GraphBuilder::push()
- `src/codec/builder.rs:1175-1267` - GraphBuilder::cache_fetch()
- `tests/codec_test.rs:605-628` - test_anchor_collision_detection
- `tests/network_1/anchors_collision_test.md` - Test fixture

## Notes

This is a subtle architectural bug that affects the fundamental node identity resolution during multi-pass compilation. The fix needs to be surgical to avoid breaking the carefully-designed forward reference resolution system.

The "tricky" part is that we're in a Catch-22: we need cache lookup to enable forward refs, but cache lookup can give wrong matches for duplicate titles. BID validation AFTER matching seems like the least invasive approach.

**Recommendation**: Start with Option 3, gather data, iterate if needed.