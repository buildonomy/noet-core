# START HERE - Next Session: Issue 02 Implementation

**Status**: Ready to implement section metadata enrichment  
**Estimated Effort**: 2-3 days  
**Last Updated**: 2025-01-27

---

## Context: What's Complete ✅

### Issue 22 (Duplicate Node Bug) ✅ COMPLETE
- Fixed duplicate node deduplication in GraphBuilder
- Two "Details" headings now create 2 separate nodes (not 1)
- Solution: Speculative path computation without Title key for sections
- All tests passing (83 lib + 9 integration)
- **Moved to**: `docs/project/completed/ISSUE_22_DUPLICATE_NODE_DEDUPLICATION.md`

### Issue 03 (Anchor Management) ✅ COMPLETE  
- Anchor parsing, collision detection, selective injection all working
- Document-level and network-level collision detection implemented
- Bref fallback for collisions operational
- All tests passing with proper assertions
- **Moved to**: `docs/project/completed/ISSUE_03_HEADING_ANCHORS.md`

### Issue 01 (Schema Registry) ⚠️ STATUS UNKNOWN
- Need to verify if complete before starting Issue 02
- Issue 02 depends on schema validation for `sections` field

---

## Next Task: Issue 02 - Section Metadata Enrichment

**File**: `docs/project/ISSUE_02_MULTINODE_TOML_PARSING.md`

### What Issue 02 Does
Enables TOML frontmatter `sections` field to enrich heading nodes with metadata:
- Match section metadata to heading nodes by **BID > Anchor > Title** (priority order)
- Enrich matched nodes with schema types and custom fields
- Garbage collect unmatched sections (heading was removed)
- Maintain 1:1 correspondence between headings and sections entries

### Dependencies Status
- ✅ **Issue 03 (Anchors)** - NOW COMPLETE - anchor matching infrastructure available
- ⚠️ **Issue 01 (Schema Registry)** - Status unclear, need to verify

---

## Implementation Checklist

### Step 1: Verify Prerequisites (30 min)
- [ ] Check if Issue 01 (Schema Registry) is complete
- [ ] Verify `matched_sections: HashSet<NodeKey>` field exists in MdCodec
- [ ] Review existing unit tests mentioned in Issue 02

### Step 2: Implement Helper Functions (1 day) ⚠️ VERIFY IF DONE
**File**: `src/codec/md.rs`

Document says "already tested with comprehensive unit tests" - need to check if implementations exist:

- [ ] `get_sections_table_mut()` - Access document's sections table
- [ ] `find_metadata_in_sections()` - Priority matching BID > Anchor > Title
- [ ] `merge_metadata()` or `merge_metadata_from_table()` - Merge metadata into proto

**Action**: Search for these functions to see if they're implemented or just test scaffolds

### Step 3: Implement "Look Up" in inject_context() (1 day) ❌ NOT DONE
**File**: `src/codec/md.rs` (inject_context function)

For heading nodes (heading > 2):
1. Access document node's sections table (first in current_events)
2. Call `find_metadata_in_sections()` with priority matching
3. If match found: merge metadata into proto.document
4. Track matched key in `self.matched_sections`

**Key insight**: Use Issue 03's anchor infrastructure for anchor-based matching

### Step 4: Implement finalize() for Garbage Collection (0.5 days) ❌ NOT DONE
**File**: `src/codec/md.rs` (finalize function)

For document node:
1. Get mutable access to sections table
2. Iterate through sections entries
3. Remove entries where key NOT in `self.matched_sections`
4. Log info about removed orphaned sections

### Step 5: Testing and Edge Cases (0.5 days) ❌ NOT DONE
- [ ] Test priority matching: BID > Anchor > Title
- [ ] Test unmatched sections (garbage collection)
- [ ] Test unmatched headings (nodes created with defaults)
- [ ] Test schema validation on sections field
- [ ] Round-trip preservation test

### Step 6: Logging and Diagnostics ❌ NOT DONE
- [ ] Log info when sections don't match headings
- [ ] Log info when ambiguous title matches occur
- [ ] Suggest using BID/anchor for explicit matching

---

## Key Architecture Notes

### Matching Priority (from Issue 02)
1. **BID URL** - Most explicit: `"bid://doc-123/section-456"`
2. **Anchor** - Explicit from markdown: `"introduction"` from `{#introduction}`
3. **Title slug** - Fallback: slugified title via `to_anchor(title)`

### Current Events Structure
- `current_events[0]` = document node (always first)
- `current_events[i]` (i > 0) = heading nodes in document order
- Headings "look up" to document for sections metadata

### Authority Model
- **Markdown** = structure (which nodes exist)
- **Frontmatter sections** = metadata enrichment (what fields they have)

---

## Success Criteria (from Issue 02)

- [ ] All markdown headings create nodes (cross-reference tracking works)
- [ ] Sections metadata enriches matched nodes
- [ ] Priority matching: BID > Anchor > Title
- [ ] Unmatched sections: garbage collected (heading was removed)
- [ ] Unmatched headings: nodes created with defaults
- [ ] Schema validates sections field structure
- [ ] Clean round-trip: sections maintains 1:1 mapping
- [ ] Backward compatible with existing documents
- [ ] Tests pass

---

## Quick Start Commands

```bash
# Check for existing helper functions
rg "find_metadata_in_sections|get_sections_table_mut|merge_metadata" noet-core/src/

# Check for matched_sections field
rg "matched_sections" noet-core/src/codec/md.rs

# Check Issue 01 status
cat noet-core/docs/project/ISSUE_01_*.md

# Run existing tests
cargo test --lib
cargo test --test codec_test
```

---

## Files to Modify

**Primary**:
- `src/codec/md.rs` - inject_context(), finalize(), helper functions

**Testing**:
- `tests/codec_test.rs` - Add section metadata enrichment tests
- Create test fixtures in `tests/network_1/`

**Documentation**:
- Update `docs/project/ISSUE_02_MULTINODE_TOML_PARSING.md` as implementation progresses

---

## Potential Blockers

1. **Issue 01 incomplete** - Need schema validation for sections field
2. **Helper functions missing** - Document claims they're "already tested" but may just be scaffolds
3. **Ambiguous title matching** - Need good logging/diagnostics when titles aren't unique

---

## Recent Accomplishments (Session Summary)

This session (2025-01-27):
- ✅ Fixed Issue 22 (duplicate node bug) - major architecture breakthrough
- ✅ Verified Issue 03 complete, cleaned up test comments
- ✅ Moved both issues to `completed/` directory
- ✅ All 83 lib tests + 9 integration tests passing
- ✅ Identified Issue 02 as next critical task

**Key insight**: The speculative path computation approach from Issue 22 avoided BeliefBase cloning issues by passing parent path directly to PathMap. This architectural lesson may apply to other areas.

---

**Time Estimate for Issue 02**: 2-3 days (verify dependencies, then implement Steps 2-6)  
**Confidence**: MEDIUM - dependency on Issue 01 needs verification  
**Priority**: CRITICAL - Blocks Issue 4

Good luck! 🚀