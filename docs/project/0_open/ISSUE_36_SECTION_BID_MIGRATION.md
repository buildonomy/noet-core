# Issue 36: Content-Based Section Identity (BID Migration on Move / Shared-Section Unification)

**Priority**: MEDIUM
**Estimated Effort**: 2-3 days
**Dependencies**: Issue 34 (Cache Stability), Issue 35 (Cache Invalidation)
**Blocks**: None (quality-of-life improvement)

## Summary

When users move a section from one document to another (cut/paste), the system currently treats this as a delete + create operation, generating a new BID for the "new" section. This breaks all existing links to that section. We should detect content-based moves and migrate the BID automatically to preserve link stability.

Beyond move detection, content-addressed section identity enables a second capability: **shared-section unification**. If two documents contain structurally identical sections (same title + same content), they are referencing the same concept. The graph should model this as a single shared node with two parent documents, not two disconnected nodes that happen to have identical text. This is the same insight as content-addressed storage — hash equality implies identity.

**Core Issue**: BID assignment is location-based (new parse = new BID), but section identity should be content-based (same content = same BID, regardless of where or how many times it appears).

**User Impact**:
- After reorganizing documentation (moving sections between files), all cross-references break and must be manually updated.
- Identical sections copied across documents (e.g., a standard disclaimer, a shared interface spec) accumulate as disconnected duplicates rather than converging on a single authoritative node with multiple parents.

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

### Use Case 3: Shared Section (Copy-in-Place)
- User copies (not moves) a standard section into multiple documents
  (e.g., a safety disclaimer, a standard interface contract, a test protocol)
- System detects identical content hash across documents
- Rather than emitting two nodes, the compiler emits **one shared node** with
  `WeightKind::Section` edges from both parent documents
- Queries against either document surface the shared node; backlinks show all parents
- No user prompt required — hash equality is sufficient confidence for unification

### Use Case 4: Shared Section Diverges
- User edits one copy of a previously-shared section
- Content hash changes; the edited copy gets a new BID
- The other copy retains the original shared BID
- System emits a `ParseDiagnostic::Info` noting the divergence

### Use Case 5: Partial Content Match (Move with Edits)
- User moves section but edits content slightly
- System detects fuzzy match (e.g., 95% similarity)
- User confirms whether to migrate BID or assign new one

## Architecture: Content Hash as First-Class Identity

The unifying model: **a section's BID is derived from a hash of its normalized content** (title + body text, whitespace-normalized). This hash is computed during `IRNode` construction and stored in `node.payload["content_hash"]`. The compiler's deduplication pass then:

1. **Indexes** all section nodes by content hash within a compile session.
2. **On collision** (two nodes share a hash):
   - If one was previously persisted (known BID in cache) and the other is new: assign the known BID to the new node (move detection).
   - If both are new (first time seeing this content): assign one BID and make the second node a reference to the first, adding a second parent `Section` edge to the shared node.
   - If both were previously persisted with different BIDs: flag as a merge candidate (requires explicit resolution).
3. **On divergence** (a previously shared node's hash no longer appears at one location): emit `ParseDiagnostic::Info`; the remaining location retains the BID.

This generalizes move detection and copy-unification into a single mechanism. The hash is the identity; location is merely where a node is anchored.

## Integration Points

### 1. Event Stream Analysis (Compiler Level)

**File**: `src/codec/compiler.rs`

During `finish_parse_session()` or event stream processing:
- Collect `BeliefEvent::NodeDelete` events (sections removed)
- Collect `BeliefEvent::NodeCreate` events (sections added)
- Index all live section nodes by content hash; detect collisions
- Correlate deleted → created pairs by content hash (exact match = move)
- For surviving collisions (both nodes present): unify into single shared node
- Emit `BeliefEvent::BidMigration` when move detected
- Emit `BeliefEvent::NodeUnified` when duplicate content converges

### 2. Content Hashing

**File**: `src/properties.rs` or `src/codec/belief_ir.rs`

Add content hash to `IRNode`:
- Hash section title + body text, whitespace-normalized (stable, reproducible across editors)
- Use SHA256 (already a project dependency via `sha2`) to avoid adding Blake3
- Store as `node.payload["content_hash"]` in both `IRNode` and persisted `BeliefNode`
- Hashing is performed in the codec during `parse()`, before BID assignment

### 2a. Shared-Node Graph Model

When two section nodes share a content hash, the compiler emits a single `BeliefNode` with
two incoming `WeightKind::Section` edges — one from each parent document. This is structurally
identical to how a subsection is a child of multiple parents today, just with the shared node
having multiple Section-weight parents rather than one.

```
[Doc A] --Section--> [§Installation (BID: abc)]
[Doc B] --Section--> [§Installation (BID: abc)]   ← same node, two parents
```

Queries against Doc A or Doc B both surface the shared node. Backlink queries against the
shared node return both parents. No special-casing needed in the query layer.

### 3. New Event Types

**File**: `src/event.rs`

```rust
pub enum BeliefEvent {
    // ... existing variants

    /// A section moved between documents; the original BID is preserved.
    BidMigration {
        old_bid: Bid,
        new_bid: Bid,
        confidence: f32,  // 1.0 for exact hash match, <1.0 for fuzzy
        reason: String,
    },

    /// Two section nodes with identical content hash were unified into one shared node.
    /// `retained_bid` is the BID kept; `merged_bid` is retired. All edges to `merged_bid`
    /// are rewritten to `retained_bid`.
    NodeUnified {
        retained_bid: Bid,
        merged_bid: Bid,
        content_hash: String,
        parent_paths: Vec<PathBuf>,
    },
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
- Track all live section nodes (present in both before and after)
- Store per event: (BID, title, content_hash, document_path)

**Phase 2: Hash Index** (after parse session)
- Build a `HashMap<content_hash, Vec<(BID, path)>>` over all live + created section nodes
- Identify collisions (multiple entries per hash)

**Phase 3a: Move Detection** (deleted hash appears in created set)
- Exact content hash match between a deleted node and a created node → confidence 1.0
- Assign deleted BID to created node; emit `BidMigration`
- Fuzzy match (title match + high text similarity, no hash match) → confidence < 1.0;
  defer to user confirmation

**Phase 3b: Shared-Section Unification** (same hash, both nodes live)
- Two or more live nodes share a content hash (copy-in-place scenario)
- Retain the BID of the node with the earlier `created_at` (or lowest BID value as
  tiebreaker for determinism)
- Rewrite all edges from `merged_bid` to `retained_bid`
- Emit `NodeUnified`

**Phase 4: Garbage Collection** (fallback)
- If no match found, proceed with delete + create (current behavior)

## Success Criteria

- [ ] Section moved between documents preserves original BID
- [ ] Cross-document links continue working after section move
- [ ] Two documents containing identical section content produce one shared `BeliefNode`
      with two parent `Section` edges, not two disconnected nodes
- [ ] Backlink query on a shared node returns all parent documents
- [ ] Content hash calculated efficiently (< 1ms per section, SHA256 of normalized text)
- [ ] False positive unification rate < 1% (short/generic sections are a risk; see Risks)
- [ ] User can override automatic migration or unification when needed
- [ ] Logging shows which sections were migrated or unified and why

## Risks

- **False positive unification on short/generic sections**: A section titled "Notes" with
  body "TBD" will hash-collide across many documents. **Mitigation**: require a minimum
  content length (e.g., >100 normalized characters) before triggering unification; below
  that threshold, treat as independent nodes even on hash match.
- **Unification surprises authors**: two authors independently write identical content and
  expect separate nodes. **Mitigation**: emit `ParseDiagnostic::Info` on every unification
  so it is visible; provide a frontmatter opt-out (`no_unify: true` on a section).
- **BID tiebreaker non-determinism**: if both nodes are new in the same session, neither
  has an earlier `created_at`. **Mitigation**: use lexicographic minimum of the two BID
  values as the deterministic tiebreaker.

## Open Questions

### Q1: Matching Threshold
- What confidence score triggers automatic migration?
- Should user confirm migrations < 100% confidence?
- **Proposed**: exact hash match → automatic (confidence 1.0); fuzzy match → log warning,
  no automatic action until user confirms.

### Q2: Multi-Hop Moves
- What if section moved twice in one session? (A → B → C)
- Should we track BID history/lineage?

### Q3: Title Changes
- What if section title changes during move?
- Should we still detect based on content alone?

### Q4: Performance
- How expensive is content hashing for large documents?
- Should we cache hashes between parse sessions?
- **Note**: SHA256 of a typical section body is < 1 µs; not a concern.

### Q5: Conflict Resolution
- What if two sections deleted, both match one created section?
- Which BID wins?
- **Proposed**: retain the BID with the earliest `created_at`; log the conflict.

### Q6: Unification scope
- Should unification apply within a single network only, or across the entire corpus?
- Cross-network unification (e.g., a shared interface spec referenced in both a design
  doc network and a requirements network) could be powerful but requires careful edge
  semantics. **Proposed**: within-network only for initial implementation; cross-network
  as a follow-on.

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
- `src/codec/belief_ir.rs` - IRNode structure
- `docs/design/section_metadata_manifest.md` - Section tracking architecture