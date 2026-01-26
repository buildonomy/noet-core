# Issue 03 TDD Scaffold Complete

**Date**: 2025-01-26
**Status**: ✅ TESTS WRITTEN - Ready for Implementation

## Tests Created

### Unit Tests (6 new tests in `src/codec/md.rs`)
- `test_determine_node_id_no_collision` - Unique titles get title-derived IDs
- `test_determine_node_id_title_collision` - Collisions use Bref fallback
- `test_determine_node_id_explicit_collision` - Explicit IDs also use Bref on collision
- `test_determine_node_id_normalization` - Special chars normalized via to_anchor()
- `test_determine_node_id_normalization_collision` - Collision after normalization
- `test_to_anchor_consistency` - Verify to_anchor() behavior

All 6 tests PASSING (with stub implementation of `determine_node_id()`)

### Test Fixtures (3 new markdown files in `tests/network_1/`)
1. `anchors_collision_test.md` - Two "Details" headings to test collision
2. `anchors_explicit_test.md` - Explicit anchors that should be preserved
3. `anchors_normalization_test.md` - Special chars in anchors

### Integration Tests (4 new tests in `tests/codec_test.rs`)
- `test_anchor_collision_detection` - Verify Bref fallback for collisions
- `test_explicit_anchor_preservation` - Explicit anchors stay unchanged
- `test_anchor_normalization` - Special chars normalized properly
- `test_anchor_selective_injection` - Only inject Brefs for collisions

**Status**: Tests need API fixes (BeliefBase.graph() vs .relations().as_graph())

## What Needs Implementation

1. **Enable ENABLE_HEADING_ATTRIBUTES** (1 line uncomment)
2. **Capture id field** from MdTag::Heading during parse
3. **Implement determine_node_id()** - Replace stub with real logic
4. **Implement collision detection** - Track existing IDs during parse
5. **Implement selective injection** - Only inject {#bref} for collisions
6. **Update BeliefNode::keys()** - Include ID-based NodeKey

## Estimated Effort

**1-2 days** (down from 2-3 days - parsing is free via pulldown_cmark!)

## Next Session

1. Fix integration test API calls (use .relations().as_graph())
2. Enable ENABLE_HEADING_ATTRIBUTES
3. Implement collision detection logic
4. Watch tests turn green!
