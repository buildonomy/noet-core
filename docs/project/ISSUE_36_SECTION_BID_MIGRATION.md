# Issue 36: Content-Based Section Identity (BID Migration on Move)

**Priority**: MEDIUM
**Estimated Effort**: 2-3 days
**Dependencies**: Issue 34 (Cache Stability), Issue 35 (Cache Invalidation)
**Blocks**: None (quality-of-life improvement)

## Summary

When users move a section from one document to another (cut/paste), the system currently treats this as a delete + create operation, generating a new BID for the "new" section. This breaks all existing links to that section. We should detect content-based moves and migrate the BID automatically to preserve link stability.

**Core Issue**: BID assignment is location-based (new parse = new BID), but section identity should be content-based (same content = same BID).

**User Impact**: After reorganizing documentation (moving sections between files), all cross-references break and must be manually updated.

## User Scenario

**Before** (Doc A):
```markdown
---
title = "Getting Started"

[sections.installation]
bid = "section-1234"
id = "installation"
---

## Installation

Follow these steps to install...
```

**User Action**: Cut "Installation" section, paste into "Setup Guide" document

**After** (Doc B):
```markdown
---
title = "Setup Guide"

[sections.installation]
bid = "section-5678"  # ← NEW BID! Links break!
id = "installation"
---

## Installation

Follow these steps to install...  # ← Same content
```

**Problem**: 
- Old BID `section-1234` no longer exists
- New BID `section-5678` assigned
- All links like `[[getting-started#installation]]` now point to deleted node

## Goals

1. **Detect section moves**: Identify when a section deleted from Doc A appears in Doc B with identical content
2. **Preserve BID**: Migrate the original BID to the new location
3. **Update references**: Ensure cross-document links continue working
4. **Confidence scoring**: Distinguish true moves from coincidental duplicates
5. **User control**: Allow manual override or confirmation for ambiguous cases

## Use Cases

### Use Case 1: Simple Section Move
- User cuts section from one file, pastes into another
- Content and title identical
- System detects move, preserves BID automatically

### Use Case 2: Section Refactoring
- User splits large document into multiple smaller documents
- Several sections moved to new files
- System detects bulk move, preserves all BIDs

### Use Case 3: Ambiguous Duplicate
- User copies (not moves) section to multiple documents
- System detects duplicate content, prompts for action:
  - Keep separate BIDs (true duplicate)
  - Merge BIDs (consolidate content)

### Use Case 4: Partial Content Match
- User moves section but edits content slightly
- System detects fuzzy match (e.g., 95% similarity)
- User confirms whether to migrate BID or assign new one

## Integration Points

### 1. Event Stream Analysis (Compiler Level)

**File**: `src/codec/compiler.rs`

During `finish_parse_session()` or event stream processing:
- Collect `BeliefEvent::NodeDelete` events (sections removed)
- Collect `BeliefEvent::NodeCreate` events (sections added)
- Correlate deleted → created pairs by content similarity
- Emit `BeliefEvent::BidMigration` when match detected

### 2. Content Hashing

**File**: `src/properties.rs` or `src/codec/belief_ir.rs`

Add content hash to `ProtoBeliefNode`:
- Hash section title + text content (stable, reproducible)
- Use Blake3 or similar fast hash
- Store in `BeliefNode` for comparison

### 3. BID Migration Event

**File**: `src/event.rs`

New event type:
```rust
pub enum BeliefEvent {
    // ... existing variants
    BidMigration {
        old_bid: Bid,
        new_bid: Bid,
        confidence: f32,  // 0.0-1.0
        reason: String,
    }
}
```

### 4. Reference Update

**File**: `src/db.rs` or `src/beliefbase.rs`

When BID migration detected:
- Update all relations referencing old BID to point to new BID
- Update cache entries
- Optionally: emit warning if confidence < 1.0

### 5. Section Metadata Manifest

**Integration with Issue 02**: Update `sections` table to include content hash:

```toml
[sections.installation]
bid = "section-1234"
id = "installation"
content_hash = "blake3:abc123..."  # Optional: for move detection
```

## Detection Algorithm (High-Level)

**Phase 1: Collection** (during parse session)
- Track all `NodeDelete` events for sections
- Track all `NodeCreate` events for sections
- Store: (BID, title, content_hash, document_path)

**Phase 2: Matching** (after parse session)
- For each deleted section:
  - Find created sections with matching title
  - Compare content hashes
  - Calculate confidence score (0.0-1.0)
    - 1.0: Exact title + content match
    - 0.9-0.99: Title match + high content similarity (fuzzy)
    - < 0.9: Unlikely to be same section

**Phase 3: Migration** (if confidence > threshold)
- Replace new BID with old BID in created node
- Update document's `sections` table with old BID
- Emit `BidMigration` event
- Update all references in cache

**Phase 4: Garbage Collection** (fallback)
- If no match found, proceed with delete + create (current behavior)

## Success Criteria

- [ ] Section moved between documents preserves original BID
- [ ] Cross-document links continue working after section move
- [ ] Content hash calculated efficiently (< 1ms per section)
- [ ] False positive rate < 1% (no accidental BID merges)
- [ ] User can override automatic migration when needed
- [ ] Logging shows which sections were migrated and why

## Open Questions

### Q1: Matching Threshold
- What confidence score triggers automatic migration?
- Should user confirm migrations < 100% confidence?

### Q2: Multi-Hop Moves
- What if section moved twice in one session? (A → B → C)
- Should we track BID history/lineage?

### Q3: Title Changes
- What if section title changes during move?
- Should we still detect based on content alone?

### Q4: Performance
- How expensive is content hashing for large documents?
- Should we cache hashes between parse sessions?

### Q5: Conflict Resolution
- What if two sections deleted, both match one created section?
- Which BID wins?

## Implementation Estimate

- Phase 1: Content hashing infrastructure (1 day)
- Phase 2: Event correlation and detection (1 day)
- Phase 3: BID migration logic (1 day)
- Phase 4: Testing and edge cases (1 day)
- Phase 5: User confirmation UI (optional, 1 day)

**Total**: 2-4 days depending on scope

## Out of Scope (Future Enhancements)

- Machine learning for fuzzy content matching
- Undo/redo for BID migrations
- Migration across network boundaries (different projects)
- Automatic conflict resolution without user input
- BID lineage tracking (full history of moves)

## Related Issues

- **Issue 02**: Section Metadata Manifest (foundation for section tracking)
- **Issue 15**: Filtered Event Streaming (event consumption pattern)
- **Issue 34**: Cache Stability (prerequisite - cache must work correctly)
- **Issue 35**: Cache Invalidation (interacts with content hashing)

## References

- `src/codec/compiler.rs` - Event stream processing
- `src/event.rs` - Belief event types
- `src/codec/belief_ir.rs` - ProtoBeliefNode structure
- `docs/design/section_metadata_manifest.md` - Section tracking architecture