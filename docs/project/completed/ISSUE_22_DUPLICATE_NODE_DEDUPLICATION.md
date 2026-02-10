# Issue 22: Duplicate Node Deduplication Bug in GraphBuilder

**Priority**: HIGH  
**Estimated Effort**: 2-3 days  
**Dependencies**: None (blocks Issue 03 full verification)  
**Status**: ✅ Complete

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

**Actual Behavior (FIXED)**:
- Two separate nodes in BeliefBase.states()
- Second "Details" heading creates a new node with Bref-based path
- Collision detection works correctly during parse (MdCodec correctly sets second ID to None)
- Nodes use position-aware Path keys that include collision detection

## Root Cause

**Location**: `src/codec/builder.rs:~860` (`GraphBuilder::push()`)

**The Problem (FIXED)**:

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

**The Bug (FIXED)**: `cache_fetch()` was matching on `Title` key, which is **not structurally unique** for section headings. Two headings can have the same title but occupy different positions in the document hierarchy. Now sections use only Path keys (no Title key) with proper collision detection.

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
**Actual (FIXED)**: ✅ 4 heading nodes (all separate)

**Test Output**:
```
Found 4 heading nodes
  - Details (bid: 1f0fb9ba-149c-6225-a725-46833bdd2166)
  - Implementation (bid: 1f0fb9ba-149e-6016-a727-46833bdd2166)
  - Details (bid: 1f0fb9ba-149f-6e4f-a729-46833bdd2166)
  - Testing (bid: 1f0fb9ba-14a1-6dc4-a72b-46833bdd2166)
Found 2 'Details' headings  # ✅ Correct!
```

## Solution Implemented: Speculative Path with Position-Based Disambiguation

**Core Insight**: Remove `Title` key from cache_fetch lookups for sections. Use **speculative path computation** instead, which includes position-based collision detection.

### Why This Works

A path encodes:
1. Parent path (known from stack)
2. Node position via sort_key (can speculate as max+1)
3. Anchor (ID or Bref)

For the anchor part (handled by `PathMap::speculative_path` and `generate_path_name_with_collision_check`):
1. Determine candidate anchor:
   - If explicit ID in proto → use it
   - Otherwise → use title-derived anchor
2. Check collision with siblings in PathMap:
   - If anchor collides → use Bref (BID namespace) as fallback
   - Otherwise → use candidate anchor

**Key realization**: Use existing PathMap collision detection logic (`generate_path_name_with_collision_check`) by providing the parent's path directly instead of looking it up in PathMap, since the parent may not be in PathMap yet during Phase 1.

### Algorithm (As Implemented)

```rust
// In builder.rs::push()
let mut keys = if proto.heading > 2 && !parsed_node.bid.initialized() {
    // Section in Phase 1: use speculative path computation
    let speculative_path = self.speculative_section_path(&parsed_node, parent_bid, &proto.path)?;
    
    // Generate keys WITHOUT Title for sections (only Path key, no ID key to avoid collision issues)
    vec![NodeKey::Path { net: parent_net, path: speculative_path }]
} else {
    // Document OR section in Phase 2+ (with BID): use existing logic
    parsed_node.keys(Some(self.repo()), Some(parent_bid), self.doc_bb())
};

// In builder.rs::speculative_section_path()
fn speculative_section_path(
    &self,
    parsed_node: &BeliefNode,
    parent_bid: Bid,
    parent_path: &str,  // Use proto.path directly instead of looking up in PathMap
) -> Result<String, BuildonomyError> {
    // Find network by walking up the stack (network nodes have heading=1)
    let parent_net = self.stack.iter().rev()
        .find(|(_, _, heading)| *heading == 1)
        .map(|(bid, _, _)| *bid)
        .unwrap_or(self.repo());
    
    // Get PathMap for this network and compute speculative path
    let paths = self.doc_bb.paths();
    let pathmap = paths.get_map(&parent_net)?;
    
    // Generate temporary BID for collision detection
    let temp_bid = if parsed_node.bid.initialized() {
        parsed_node.bid
    } else {
        Bid::new(&parent_bid)
    };
    
    // Use PathMap::speculative_path with parent_path directly
    pathmap.speculative_path(&temp_bid, parent_path, None, &paths)
}

// In paths.rs::PathMap::speculative_path() [NEW METHOD]
pub fn speculative_path(
    &self,
    source: &Bid,
    parent_path: &str,  // Parent path passed directly, not looked up
    explicit_path: Option<&str>,
    nets: &PathMapMap,
) -> Option<String> {
    // Count existing children to determine sort_key
    let new_index = self.map.iter()
        .filter(|(path, _, _)| path.starts_with(parent_path) && path != parent_path)
        .count() as u16;
    
    // Use existing collision detection logic
    let path = generate_path_name_with_collision_check(
        source,
        &Bid::nil(),  // Dummy sink - only used for API special case
        parent_path,
        explicit_path,
        new_index,
        nets,
        &self.map,
    );
    
    Some(path)
}

// In paths.rs::generate_path_name_with_collision_check() [MODIFIED]
// Changed collision fallback from index-based to Bref-based
if has_collision {
    // Use Bref (BID namespace) as fallback for collision
    terminal_path = source.bref().to_string();
}
```

### How This Fixes the Bug

| Case | Speculative Path | Cache Match? | Result |
|------|------------------|--------------|---------|
| First "Details" | `anchors_collision_test.md#details` | No | Create new (BID=aaa) |
| Second "Details" | `anchors_collision_test.md#{bref}` | No | Create new (BID=bbb) |
| Phase 2 First "Details" | `anchors_collision_test.md#details` | **Yes** | Update existing (BID=aaa) |
| Phase 2 Second "Details" | `anchors_collision_test.md#{bref}` | **Yes** | Update existing (BID=bbb) via BID key |

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

## Implementation Summary (Completed)

### What Was Implemented

1. **Added `PathMap::speculative_path()` method** (`src/paths.rs`)
   - Computes what path would be generated for a child node given parent's path
   - Takes parent path as parameter (not looked up in PathMap)
   - Reuses existing `generate_path_name_with_collision_check()` logic
   - Counts existing children in PathMap to determine sort_key
   - No mutation, purely computational

2. **Modified `generate_path_name_with_collision_check()`** (`src/paths.rs`)
   - Changed collision fallback from index-based (`{index}-{title}`) to Bref-based (`{namespace}`)
   - Uses `source.bref().to_string()` for guaranteed uniqueness
   - Aligns with architectural principle of using BID-derived values for collision resolution

3. **Added `GraphBuilder::speculative_section_path()`** (`src/codec/builder.rs`)
   - Finds network by walking up stack (network nodes have heading=1)
   - Gets PathMap for network
   - Calls `PathMap::speculative_path()` with `proto.path` directly
   - Returns collision-aware path without mutating any state

4. **Modified `GraphBuilder::push()`** (`src/codec/builder.rs`)
   - For sections in Phase 1 (heading > 2, no BID): use speculative path computation
   - Build keys with ONLY Path key (no Title, no ID to avoid pre-collision issues)
   - For documents OR Phase 2+ sections: use existing `parsed_node.keys()` logic
   - Preserves all existing behavior for non-section nodes

5. **Added `From<BeliefKind>` for `BeliefKindSet`** (`src/properties.rs`)
   - Quality-of-life improvement for test code
   - Allows `.into()` conversion from single BeliefKind

### Key Architectural Decisions

- **No cloning approach**: Avoided cloning BeliefBase/PathMap (shallow Arc cloning issue)
- **Direct path passing**: Pass parent path from `proto.path` instead of looking up in PathMap
- **Reuse existing logic**: Leverage `generate_path_name_with_collision_check()` instead of duplicating
- **No EventOrigin::Speculative needed**: Computed paths directly without event processing
- **Minimal key set**: Sections in Phase 1 use only Path key for uniqueness

## Success Criteria (All Met ✅)

- [x] MdCodec collision detection sets second "Details" id to None ✅
- [x] Two "Details" headings create two separate nodes in BeliefBase ✅
- [x] First "Details" has ID="details" ✅
- [x] Second "Details" has ID=Bref (collision fallback) ✅
- [x] Both nodes have different BIDs ✅
- [x] Both nodes are independently accessible ✅
- [x] All existing tests still pass (85 lib + 9 integration tests) ✅
- [x] test_anchor_collision_detection passes with full assertions ✅
- [x] Collision detection uses Bref (not index) for fallback ✅

## Next Steps for Issue Closure

1. **Update Issue 03 verification** - Unblock full verification now that duplicate nodes work correctly
2. **Consider edge cases**:
   - Explicit IDs that collide should log warnings (currently handled by MdCodec)
   - Very deep nesting (many heading levels) - verify PathMap performance
3. **Documentation**:
   - Update architecture docs to explain Phase 1 section key generation
   - Document the parent path passing approach
4. **Potential future improvements**:
   - Consider adding unit tests specifically for `PathMap::speculative_path()`
   - Profile performance with documents containing hundreds of sections

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

## Actual Implementation Lessons

1. **Avoid premature cloning**: BeliefBase cloning seemed like a good approach but shallow Arc cloning caused subtle state corruption
2. **Trust existing abstractions**: PathMap already had the right collision detection logic via `generate_path_name_with_collision_check()`
3. **Pass data, don't look it up**: Since parent isn't in PathMap yet during Phase 1, pass `proto.path` directly
4. **Minimal keys are best**: Using only Path key for Phase 1 sections avoids pre-collision ID matching issues
5. **Bref > index for collisions**: Using BID namespace for collision fallback is architecturally cleaner than numeric indices
