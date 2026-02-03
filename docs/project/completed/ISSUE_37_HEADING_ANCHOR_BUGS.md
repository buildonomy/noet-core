# Issue 37: Heading Anchor Bugs in Markdown Codec

**Priority**: HIGH
**Estimated Effort**: 3 days (2 days spent)
**Dependencies**: Builds on ISSUE_03_HEADING_ANCHORS (completed)
**Version**: 0.1
**Status**: IN PROGRESS (Fixes 1 & 3 complete, Fix 2 remaining)

## Summary

Three related bugs discovered during `noet watch` testing that affect heading anchor functionality implemented in Issue 3:

1. ✅ **FIXED: Anchor IDs not written to markdown**: Headings lack `{#anchor-id}` syntax despite being tracked internally
2. **Non-unique section IDs across network**: Same IDs (e.g., "method") appear in different files, violating network-unique constraint
3. ✅ **FIXED: InlineHtml content ignored in titles**: Headings like `### <Method Title>` produce empty IDs instead of valid slugs

These issues break the core contract established in Issue 3: section IDs must be network-unique and reliably derived from heading content.

## Goals

- Fix heading ID injection so `{#anchor-id}` syntax appears in markdown output
- Enforce network-unique IDs with Bref fallback for cross-file collisions
- Include InlineHtml and Code events in title accumulation for ID generation
- Maintain compatibility with existing frontmatter `sections` table format

## Architecture

### Problem 1: Missing Anchor Syntax in Output

**Current State**: 
- `inject_context()` correctly modifies `MdEvent::Start(MdTag::Heading { id, .. })` events
- Sets `id_changed = true` when ID differs from original
- But `pulldown_cmark_to_cmark` doesn't write `{#id}` syntax back to markdown

**Investigation Needed**:
- Unit test at `md.rs:2139-2158` uses `pulldown_cmark_to_cmark::cmark()` directly and passes
- Production code uses `cmark_resume_with_source_range_and_options()` with source ranges
- Key difference: resume function may prioritize source content over event modifications
- Check if `CmarkToCmarkOptions` has heading attribute flags
- Verify heading events reach `events_to_text()` with correct ID field populated

### Problem 2: Cross-File ID Collisions

**Example from database**:
```
1f1007f3-4478-6bc1-b60b-db1c8a5d190f|method|Method  (in sym/hsml.md)
1f1007f3-48b0-614e-b61c-d3397ed10883|method|Method  (in sym/hsml.md)
1f1007f3-4a0d-6227-b629-1d4867f9fa5f|method|Method  (in sym/hstp.md)
```

Three different BIDs with same ID "method" violates network-unique constraint from Issue 3.

**Current Behavior**:
- `seen_ids` (L957) only tracks within single document (parse-time)
- Network-level check in `inject_context()` (L1114-1129) checks PathMap but misses batch collisions
- If two files parse simultaneously, both get same ID before network registration

**Solution Approach**:
Two-phase collision detection:
1. **Parse**: Document-scoped collision detection (keep current `seen_ids`)
2. **Finalize**: Network-scoped collision detection before writing sections table
   - Check all candidate IDs against network PathMap
   - Replace colliding IDs with Brefs (using section's BID)
   - Update both `ProtoBeliefNode.id` and heading events

### Problem 3: InlineHtml Not Accumulated for Titles

**Example**: `### <Method Title>` in `hstp.md:55`

Produces:
- Frontmatter: `[sections."id://"]` with empty `id = ""`
- Database: Empty ID field
- Expected: `id = "method-title"` derived from `<Method Title>`

**Root Cause**: `md.rs:1604-1614` only handles `MdEvent::Text`

```rust
MdEvent::Text(cow_str) => {
    // accumulate title content
}
// Missing: MdEvent::InlineHtml(cow_str)
// Missing: MdEvent::Code(cow_str)
```

**Solution**: Add InlineHtml and Code to pattern match for title accumulation.

## Implementation Steps

### 1. Fix InlineHtml/Code Title Accumulation ✅ COMPLETE (1 day)
- [x] Add `MdEvent::InlineHtml` and `MdEvent::Code` to title accumulation match at L1604
- [x] Use same accumulation logic as `MdEvent::Text`
- [x] Add test case for InlineHtml in headings (e.g., `### <Custom> Title`)
- [x] Add test case for Code in headings (e.g., `### Using \`code\` in Titles`)
- [x] Verify generated IDs are valid slugs

**Result**: Single line change to pattern match - `MdEvent::Text(cow_str) | MdEvent::InlineHtml(cow_str) | MdEvent::Code(cow_str)`. Test added and passing. InlineHtml/Code now contribute to title and ID generation.

### 2. Debug Heading ID Writing ✅ COMPLETE (1 day)
- [x] Add logging in `events_to_text()` to dump heading events before cmark call
- [x] Create test using `cmark_resume_with_source_range_and_options()` (not just `cmark()`)
- [x] Check `CmarkToCmarkOptions::default()` for heading attribute flags
- [x] Test with `None` source ranges to see if resume prioritizes source over events
- [x] Compare behavior: `cmark()` vs `cmark_resume_with_source_range_and_options()`
- [x] Clear source ranges for modified heading events
- [x] Fix event queue targeting - modify `current_events` not `proto_events.1`

**Root Cause**: Two issues found:
1. `cmark_resume_with_source_range_and_options()` prioritizes source over events when ranges present - **fixed by clearing range**
2. Code was modifying `proto_events.1` AFTER it was taken via `std::mem::take(&mut proto_events.1)` into `current_events` - **fixed by modifying `current_events` instead**

**Result**: Heading IDs now write correctly: `## Definition { #definition }`. Test added and passing.

### 3. Network-Unique ID Enforcement (1 day)
- [ ] In `finalize()`, after building sections table, scan all section IDs
- [ ] For each ID, check `ctx.belief_set().paths().net_get_from_id()` across network
- [ ] Track collisions: if ID exists with different BID, mark for Bref replacement
- [ ] Generate Brefs for colliding sections (use `section_bid.namespace().to_string()`)
- [ ] Update `ProtoBeliefNode.id` field for affected sections
- [ ] Update heading events to inject corrected IDs
- [ ] Add test: two files with same section titles, verify Brefs assigned
- [ ] Add test: verify existing unique IDs preserved

## Testing Requirements

### Title Accumulation Tests
- InlineHtml in headings: `### <HTML> Content` → `id = "html-content"`
- Code in headings: `### Using \`code\`` → `id = "using-code"`
- Mixed content: `### Text <HTML> \`code\`` → `id = "text-html-code"`
- Empty after stripping: `### <>` → fallback to Bref

### Heading ID Round-Trip Tests
- Parse heading with explicit ID: `## Test {#my-id}` → verify ID preserved
- Parse heading without ID: `## Test` → verify slug generated
- Write heading with ID → verify `{#id}` syntax in output
- Collision case → verify Bref injected and written

### Network-Unique ID Tests
- Two files, same section title → second gets Bref
- Two files, different titles but same slug → collision detected
- Single file, duplicate headings → document-level collision (existing behavior)
- Network-level lookup across multiple parse batches

## Success Criteria

- [x] Headings with IDs display `{#anchor-id}` syntax in markdown output
- [x] InlineHtml and Code content contributes to title-derived IDs
- [ ] Network-wide ID uniqueness enforced: no duplicate IDs across files
- [ ] Colliding IDs automatically replaced with Brefs
- [x] All existing ISSUE_03 tests continue passing
- [x] New tests cover all three bug scenarios (2/3 complete)

## Risks

**Risk 1**: Heading ID writing may require pulldown_cmark_to_cmark version upgrade
**Mitigation**: Check library version and changelog; prepare for dependency update if needed

**Risk 2**: Network-level collision detection in `finalize()` may not have access to BeliefContext
**Mitigation**: May need to restructure to perform network check during `inject_context()` instead of `finalize()`

**Risk 3**: Bref generation for collisions could create confusing IDs for users
**Mitigation**: Add warning logs when Brefs are auto-generated; document in ISSUE_03

## Open Questions

- Should we emit warnings when InlineHtml is used in headings? (May indicate markdown formatting issue)
- Should network collision Brefs be stable across re-parses or deterministic from content?
- Do we need to update ISSUE_03 documentation with cross-file collision behavior?

## References

- ISSUE_03_HEADING_ANCHORS.md - Original implementation
- `src/codec/md.rs:1604` - Title accumulation logic (FIXED: added InlineHtml/Code)
- `src/codec/md.rs:1160-1170` - Heading ID injection (FIXED: target current_events, clear range)
- `src/codec/md.rs:1042-1250` - `inject_context()` implementation
- `src/codec/md.rs:1379-1500` - `finalize()` sections table building (TODO: network collision detection)
- `belief-network-sm-test/sym/hsml.md` - Test file with collisions (definition, justification, method, log, method-title)
- `belief-network-sm-test/sym/hstp.md` - Test file with collisions (definition, justification, method, log, method-title)

## Progress Notes

**2025-02-03**: Completed Fixes 1 & 3
- Fix 3 was straightforward pattern match addition
- Fix 1 required two corrections:
  1. Clear source range when modifying heading ID to force cmark_resume to use event data
  2. Modify `current_events` instead of empty `proto_events.1` (which was taken via mem::take)
- Verified working: `hsml.md` and `hstp.md` now have `{ #id }` syntax in headings
- InlineHtml headings like `### <Method Title>` now generate proper IDs (`method-title`)
- Remaining: Fix 2 (network-unique ID enforcement) requires `finalize()` implementation