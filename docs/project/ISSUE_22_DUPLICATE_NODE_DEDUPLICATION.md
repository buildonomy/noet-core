# Issue 22: Duplicate Node Deduplication Bug in GraphBuilder

**Priority**: HIGH  
**Estimated Effort**: 2-3 days  
**Dependencies**: None (blocks Issue 03 full verification)  
**Status**: Open

## Summary

Two markdown headings with the same title (e.g., `## Details` twice) incorrectly create only ONE node in BeliefBase instead of two separate nodes. This is caused by premature key speculation in `GraphBuilder::push()` that matches the second heading to the first heading's cached node using the `Title` key before considering structural position.

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
- Collision detection works correctly during parse (MdCodec correctly sets second ID to None)
- But nodes deduplicate during `GraphBuilder::push()` due to Title key matching

## Root Cause

**Location**: `src/codec/builder.rs:798-850` (`GraphBuilder::push()`)

**The Problem**:

```rust
async fn push(...) -> Result<...> {
    // Line ~809: Convert ProtoBeliefNode to BeliefNode
    let mut parsed_node = BeliefNode::try_from(proto)?;
    
    // Line ~810: Generate keys from parsed node
    let mut keys = parsed_node.keys(Some(self.repo()), Some(parent_bid), self.doc_bb());
    
    // Line ~816: Cache fetch using ALL keys including Title
    let cache_fetch_result = self
        .cache_fetch(&keys, global_bb.clone(), true, missing_structure)
        .await?;
}
```

**What Happens**:

1. **First "Details" heading**:
   - proto.bid: `None`
   - Title: "Details"
   - ID: "details" (title-derived)
   - Keys: `[Path{net, "doc.md#details"}, Title{net, "details"}, Id{net, "details"}]`
   - `cache_fetch()`: No match → creates new node (BID=aaa)
   - ✅ Node inserted into session_bb

2. **Second "Details" heading**:
   - proto.bid: `None`
   - Title: "Details" (same as first)
   - ID: `None` (collision detected by MdCodec)
   - Keys: `[Path{net, "doc.md#<bref>"}, Title{net, "details"}, Id{net, "<none>"}]`
   - ⚠️ `cache_fetch(&keys, ...)`: **MATCHES on `Title{net, "details"}`**
   - Returns first node (BID=aaa)
   - Second node merges into first instead of creating separate entry
   - ❌ Only one "Details" node exists

**The Bug**: `cache_fetch()` matches on `Title` key, which is **not structurally unique** for section headings. Two headings can have the same title but occupy different positions in the document hierarchy.

## Why This Is Tricky

**The Challenge**:

At the point where we call `cache_fetch()` (line ~816), we **cannot reliably use self.doc_bb.paths()** because:
- We're in the middle of parsing (phase 1)
- doc_bb is not balanced/complete yet
- Comment on line ~808: "Can't use self.doc_bb.paths() to generate keys here, because we can't assume that self.doc_bb is balanced until we're out of phase 1 of parse_content."

**Key insight**: For markdown headings (sections):
- Path is NOT unique (multiple headings in same file)
- **Title is NOT unique** (duplicate headings are common) ← Root cause
- BID is unique but not available during Phase 1
- **Position in sibling order IS unique** ← Solution!

## Architecture Context

**Multi-Pass Compilation Model**:
- **Phase 1**: Parse all files, collect unresolved references, generate BIDs
- **Phase 2+**: Reparse with resolved dependencies, inject BIDs into source
- During Phase 1, nodes don't have BIDs in source yet (they're auto-generated)
- During Phase 2+, we have BIDs and need to match against cached nodes

**GraphBuilder Role**:
- Orchestrates parsing and linking
- Maintains caches (doc_bb, session_bb) for multi-pass resolution
- `cache_fetch()` looks up nodes by keys to enable forward references

**The Speculation Problem**:
We "speculate" keys before we know if the node exists. For documents (files), this works because:
- Path is unique (one file = one document)
- Title is typically unique
- BID uniqueness is enforced

For headings (sections), this breaks because:
- **Path includes anchor**, which depends on ID collision detection
- **Title is NOT unique** (causes wrong cache hits)
- Position in sibling order IS unique but not currently used

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

## Testing
Final section...
```

**Expected**: 4 heading nodes (Details, Implementation, Details, Testing)  
**Actual**: 3 heading nodes (second Details is deduplicated into first)

**Test Output**:
```
Found 3 heading nodes
  - Details (bid: 1f0fb19c-f674-6e1f-b0a8-e90bd62e86e8)
  - Implementation (bid: ...)
  - Testing (bid: ...)
Found 1 'Details' headings  # Should be 2!
```

## Solution: Speculative Path with Position-Based Disambiguation

**Core Insight**: Remove `Title` key from cache_fetch lookups for sections. Use **speculative path** instead, which includes position-based information.

### Why This Works

A path encodes:
1. Parent path (known from stack)
2. Node position via sort_key (can speculate as max+1)
3. Anchor (ID or Bref)

For the anchor part:
1. Determine candidate ID:
   - If explicit ID in proto → use it (normalized via `to_anchor()`)
   - Otherwise → use title-derived ID (via `to_anchor(title)`)
2. Check collision with siblings:
   - If candidate ID collides → use placeholder `"<bref>"` (log warning if explicit ID)
   - Otherwise → use candidate ID

**Key realization**: We don't need to know the actual Bref value! A placeholder like `"<bref>"` is sufficient because:
- Newly-generated Brefs are **guaranteed not to match** existing nodes
- `cache_fetch()` with path `"doc.md#<bref>"` will return `Unresolved`
- We create a new node in the Unresolved branch
- The real Bref gets generated there (via `Bid::new(parent_bid)`)

### Algorithm

```rust
async fn push(...) -> Result<...> {
    let mut parsed_node = BeliefNode::try_from(proto)?;
    
    // 1. Generate speculative path (without relying on doc_bb.paths())
    let speculative_path = if proto.heading > 2 {
        // This is a section heading
        speculative_section_path(proto, parent_bid, &self.session_bb)
    } else {
        // Document node - use normal path
        proto.path.clone()
    };
    
    // 2. Generate keys WITHOUT Title for sections
    let mut keys = if proto.heading > 2 {
        vec![
            NodeKey::Path { net: parent_net, path: speculative_path },
            // Include ID if present
            proto.id.as_ref().map(|id| NodeKey::Id { net: parent_net, id: id.clone() }),
            // Include BID if initialized (Phase 2+)
            if parsed_node.bid.initialized() {
                Some(NodeKey::Bid { bid: parsed_node.bid })
            } else {
                None
            }
        ].into_iter().flatten().collect()
    } else {
        // Document nodes: use all keys as before
        parsed_node.keys(Some(self.repo()), Some(parent_bid), self.doc_bb())
    };
    
    // 3. Cache fetch with position-aware keys
    let cache_fetch_result = self
        .cache_fetch(&keys, global_bb.clone(), true, missing_structure)
        .await?;
    
    // ... rest of function
}

fn speculative_section_path(
    proto: &ProtoBeliefNode,
    parent_bid: Bid,
    session_bb: &BeliefBase,
) -> String {
    // 1. Get parent node and its path
    let parent_node = session_bb.states().get(&parent_bid).unwrap();
    let parent_path = /* extract from parent_node */;
    
    // 2. Get siblings to check ID collision
    let siblings = /* query session_bb.relations() for nodes with same parent */;
    
    // 3. Determine anchor with collision detection
    let title = proto.document.get("title").and_then(|v| v.as_str()).unwrap_or("");
    
    // Determine candidate ID (explicit or title-derived)
    let (candidate_id, is_explicit) = if let Some(explicit_id) = &proto.id {
        (to_anchor(explicit_id), true)
    } else {
        (to_anchor(title), false)
    };
    
    // Check for collision with siblings
    let id_collides = siblings.iter().any(|sib| {
        sib.id() == Some(&candidate_id)
    });
    
    let anchor = if id_collides {
        if is_explicit {
            tracing::warn!(
                "Explicit ID '{}' collides with sibling. Using Bref fallback.",
                candidate_id
            );
        }
        "<bref>".to_string()  // Placeholder for collision case
    } else {
        candidate_id
    };
    
    // 4. Construct path
    format!("{}#{}", parent_path, anchor)
}
```

### How This Fixes the Bug

| Case | Speculative Path | Cache Match? | Result |
|------|------------------|--------------|---------|
| First "Details" | `doc.md#details` | No | Create new (BID=aaa) |
| Second "Details" | `doc.md#<bref>` | No | Create new (BID=bbb) |
| Phase 2 First "Details" | `doc.md#details` | **Yes** | Update existing (BID=aaa) |
| Phase 2 Second "Details" | `doc.md#<bref-actual>` | No, but BID matches | Update via BID key |

**Critical**: Remove Title from keys for sections entirely. Path is sufficient and unambiguous.

## Truth Table for Node Resolution

### Inputs:
- **parsed_node.bid.initialized()**: Does proto have a BID?
- **cache_fetch result**: Match found?
- **Match key type**: What key caused the match?
- **proto.heading**: Section (>2) or Document (≤2)?

### Cases:

| Parsed BID | Cache Match | Match Key | Proto Type | Action | Final BID | Notes |
|------------|-------------|-----------|------------|--------|-----------|-------|
| No | No | - | Any | Create new | Generate via `Bid::new(parent)` | Phase 1 first encounter |
| No | Yes | Path | Section | Update | Use found BID | Phase 1 reparse (shouldn't happen) |
| No | Yes | Path | Document | Update | Use found BID | Watch session (--write=false) |
| No | Yes | ~~Title~~ | Section | ~~Update~~ | ~~Use found~~ | **BUG - Remove Title key!** |
| Yes | No | - | Any | Create new | Use parsed BID | User added explicit BID |
| Yes | Yes | BID | Any | Update | Use parsed BID (may rename) | Phase 2+ match |
| Yes | Yes | Path | Section | Check BID match | Depends | If BIDs differ: rename; else: update |
| Yes | Yes | Path | Document | Update | Use parsed BID | Normal document update |

### Special Cases:

**Case: User explicitly replaced BID**
- Parsed: BID=newBID
- Cache finds: oldBID node via Path
- Action: Update found node's BID (rename operation)

**Case: Multiple conflicting titles, no ID match (THE BUG)**
- Parsed: BID=None, Title="Details" (second occurrence)
- ~~Cache matches: First "Details" via Title key~~ ← **REMOVED**
- Cache matches: Nothing (different speculative paths)
- Action: Create new node ✅

**Case: Section title matches parent document title**
- Document: "Introduction" (heading=2)
- Section: "## Introduction" (heading=3)
- Both have Title="Introduction", same network
- With Title key: Ambiguous match ❌
- With Path key: Different paths, no collision ✅

## Implementation Plan

### Phase 1: Speculative Path Generation using EventOrigin::Speculative (1 day)

**Approach**: Use `EventOrigin::Speculative` to speculatively insert the node and query the resulting PathMap for the actual path that would be generated.

1. Create `fn speculative_section_path()` in builder.rs
   - Create a speculative RelationInsert event with `EventOrigin::Speculative`
   - Process event through session_bb (dry-run, no mutation)
   - Query resulting PathMap for the generated path
   - Extract anchor from path (ID or Bref)
   - Return speculative path

2. Modify `push()` to use speculative path for sections
   - For proto.heading > 2: call `speculative_section_path()`
   - Build keys WITHOUT Title: only Path, ID (if present), BID (if initialized)
   - For proto.heading ≤ 2: use existing logic (documents)

3. Implement EventOrigin::Speculative handling in BeliefBase
   - ✅ Already added to event.rs
   - Modify `BeliefBase::process_event()` to return derivative events without mutation when origin is Speculative
   - PathMap should compute path normally but not update indices

4. Unit tests for speculative_section_path()
   - Test: No collision → path uses title-derived ID
   - Test: Collision detected → path uses Bref
   - Test: Explicit ID collision → path uses Bref with warning
   - Test: Multiple speculation calls don't affect session_bb state

**Benefits of this approach**:
- ✅ Reuses existing PathMap logic (no duplication)
- ✅ Guaranteed to match actual path generation
- ✅ Handles all edge cases PathMap already handles
- ✅ Less invasive than manual session_bb queries
- ✅ Future-proof: PathMap changes automatically reflected

### Phase 2: Integration and Testing (1 day)

1. Run test_anchor_collision_detection
   - Verify two "Details" nodes created
   - Verify different BIDs and IDs

2. Test multi-pass compilation
   - Phase 1: Nodes created with generated BIDs
   - Phase 2: Nodes matched via Path or BID keys
   - Verify no duplicate creation

3. Test watch session (--write=false)
   - Parse without BID in proto
   - Match existing node via Path
   - Verify found BID used

4. Regression tests
   - All 95 existing tests pass
   - Forward references still work
   - Section metadata enrichment still works

### Phase 3: Unit Tests for builder.rs (Complete ✅)

Create `src/codec/builder.rs::tests` module:
- Test speculative_section_path() logic
- Test push() with duplicate titles
- Test cache_fetch with/without Title key
- Test BID resolution truth table cases

## Success Criteria

- [x] MdCodec collision detection sets second "Details" id to None ✅ (already works)
- [ ] Two "Details" headings create two separate nodes in BeliefBase
- [ ] First "Details" has ID="details"
- [ ] Second "Details" has ID=<bref> (generated in inject_context)
- [ ] Both nodes have different BIDs
- [ ] Both nodes are independently accessible
- [ ] All existing tests still pass (95 tests)
- [ ] test_anchor_collision_detection passes with full assertions
- [ ] New unit tests for builder.rs truth table cases

## Risks

**Risk**: Breaking document-level path matching  
**Mitigation**: Only change section path logic (heading > 2), preserve document logic

**Risk**: Speculative path differs from final path  
**Mitigation**: Use same logic as PathMap for path generation, test extensively

**Risk**: Performance impact from sibling queries  
**Mitigation**: session_bb queries are fast (in-memory), only for sections

**Risk**: Phase 2+ matching breaks  
**Mitigation**: Keep BID key for Phase 2+ matching, test multi-pass scenarios

## References

- Issue 03: Section Heading Anchor Management (blocked on this)
- `src/codec/builder.rs:798-979` - GraphBuilder::push()
- `src/codec/builder.rs:1175-1267` - GraphBuilder::cache_fetch()
- `src/codec/md.rs:1083-1110` - MdCodec collision detection (already works)
- `src/paths.rs` - PathMap path generation logic
- `tests/codec_test.rs:605-628` - test_anchor_collision_detection
- `tests/network_1/anchors_collision_test.md` - Test fixture

## Notes

This solution is architecturally cleaner than BID validation because:
1. **Title is not an identity key for sections** - it's just metadata
2. **Path encodes structural position** - parent + order + anchor
3. **No special-case logic** - just use the right keys from the start
4. **Forward-compatible** - works with future PathMap enhancements

The key insight: **Position in document structure is what makes nodes unique, not their title.**