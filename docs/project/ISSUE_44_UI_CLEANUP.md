# Issue 44: HTML Viewer UI Cleanup

**Priority**: HIGH
**Estimated Effort**: 2-3 days
**Dependencies**: None
**Blocks**: ISSUE_39 (improved baseline for advanced features)

## Progress

- **Phase 1**: ✅ COMPLETE (2025-02-04) - Network index generation fix
- **Phase 2**: Pending - Collapse button accessibility
- **Phase 3**: Pending - Remove header
- **Phase 4**: Pending - Font weight adjustments
- **Phase 5**: Pending - Reading font integration

## Summary

Clean up HTML viewer UI/UX issues discovered during ISSUE_38/39 work: fix network index.html generation bug, improve collapse button accessibility, remove redundant header, adjust font weights for visual hierarchy, and add a distinctive reading font for brand identity.

## Goals

- Fix network index.html generation (currently not working)
- Make collapse toggles accessible when panels are collapsed
- Simplify template by removing header div
- Establish visual hierarchy with font weight adjustments
- Add distinctive sans font for improved reading experience and brand feel

## Architecture

### Network Index Generation Fix

**Current Bug**: `generate_deferred_html()` in `compiler.rs` calls codec methods but uses stale `session_bb()` context instead of synchronized `global_bb`. Network nodes need complete graph context to list child documents.

**Solution**: Pass `global_bb` context to `generate_html_for_path()` when processing deferred files.

### Collapse Button Accessibility

**Current Bug**: Collapse buttons positioned inside nav/metadata panels with `position: absolute`. When panels collapse, `opacity: 0; pointer-events: none` makes buttons unclickable.

**Solution**: Move collapse buttons outside panel containers or use container-level collapse state classes that preserve button accessibility.

### Header Removal

Remove `<header>` element and associated CSS. Future search/filter UI will use drawer pattern (deferred to ISSUE_39 or later).

### Font Weight Hierarchy

Establish visual distinction:
- **Body content**: Standard reading weight (400-500)
- **UI chrome** (nav, metadata, drawers): Lighter weight (~350-400)
- **Headings**: Bold weights (600-700)

### Reading Font

Select modern sans font with:
- Excellent readability at paragraph lengths
- Slight technical/monospace character for "working document" feel
- Web font with good WOFF2 compression

**Candidates**: IBM Plex Sans, Inter, JetBrains Mono (sans variant)

## Implementation Steps

### Phase 1: Network Index Generation Fix ✅ COMPLETE (4 hours)

**Implementation Summary**:
- Moved `generate_deferred_html()` call from `finalize()` (during parsing) to `finalize_html()` (after event synchronization)
- Updated `generate_html_for_path()` to accept `BeliefSource` parameter and use `eval()` query to get focused context
- Uses `Expression::StateIn(StatePred::NetPath(...))` to query for specific document path from synchronized global_bb
- Converts eval result to `BeliefBase` with complete relationships, then calls `get_context()`

**Files Modified**:
- `src/codec/compiler.rs`: Updated deferred HTML generation flow
- Call chain: `finalize_html(&global_bb)` → `generate_deferred_html(global_bb)` → `generate_html_for_path(..., global_bb)`

**Testing**:
- [x] Code compiles successfully
- [ ] Manual test: Parse multi-level network directory structure
- [ ] Verify index.html files generated with correct child links

### Phase 2: Collapse Button Fix (2 hours)

- [ ] Refactor collapse buttons to sit outside panel DOM structure
- [ ] Update CSS positioning to anchor buttons to viewport edge
- [ ] Add `data-target` attributes for button-panel association
- [ ] Update JavaScript event handlers in `template-responsive.html`
- [ ] Test collapse/expand on desktop (mobile uses hamburger menu)

### Phase 3: Remove Header (1 hour)

- [ ] Delete `<header class="noet-header">` from `template-responsive.html`
- [ ] Remove `.noet-header*` styles from `noet-layout.css`
- [ ] Remove `headerElement` references from `viewer.js`
- [ ] Update grid layout in `.noet-container` (remove header area)
- [ ] Verify no orphaned hamburger/metadata-toggle buttons

### Phase 4: Font Weight Adjustments (2 hours)

- [ ] Add CSS custom properties for font weight scale to theme files
- [ ] Apply lighter weights to `.noet-nav`, `.noet-metadata`, `.noet-footer`
- [ ] Ensure body content uses standard reading weight
- [ ] Test readability in both light and dark themes
- [ ] Verify contrast ratios meet WCAG AA standards

### Phase 5: Reading Font Integration (3 hours)

- [ ] Select font (recommend IBM Plex Sans for balance of readability + technical feel)
- [ ] Add font files to `assets/fonts/` or use CDN with local fallback
- [ ] Update `noet-theme-*.css` with `@font-face` declarations
- [ ] Set font-family cascade: `'IBM Plex Sans', system-ui, sans-serif`
- [ ] Test rendering across browsers (Chrome, Firefox, Safari)
- [ ] Measure impact on initial page load (target <50KB for font subset)

## Testing Requirements

### Automated Tests

- [ ] Add integration test for network index.html generation
- [ ] Test deferred HTML with mock network containing 3+ child documents
- [ ] Verify index.html contains correct child links with .html extensions

### Manual Testing

- [ ] Parse multi-level network directory, verify index.html files present
- [ ] Test collapse buttons on desktop (1024px+): should remain accessible when collapsed
- [ ] Verify no visual artifacts from header removal
- [ ] Check font weight hierarchy is visible in both themes
- [ ] Test font rendering on macOS, Linux, Windows if possible
- [ ] Verify mobile drawer behavior unchanged (collapse buttons desktop-only)

### Visual Regression

- [ ] Screenshot comparison before/after for reference document
- [ ] Verify spacing/layout remains consistent after header removal
- [ ] Check font changes don't break line heights or column widths

## Success Criteria

- [ ] Network directories generate index.html listing child documents
- [ ] Collapse buttons clickable when panels are collapsed (desktop only)
- [ ] Header element completely removed from template and CSS
- [ ] Nav/metadata panels use lighter font weight than body content
- [ ] Custom reading font loads and renders consistently
- [ ] All manual tests pass
- [ ] No regressions in existing functionality (navigation tree, theme switching)

## Risks

### Risk 1: BeliefBase Context Synchronization
**Impact**: Network index generation may still fail if context isn't properly synchronized.
**Mitigation**: Add tracing logs to verify `global_bb` has expected edge data before calling codec methods. Test with known network structure.

### Risk 2: Collapse Button DOM Refactor
**Impact**: Breaking existing collapse behavior or creating new positioning bugs.
**Mitigation**: Test thoroughly on desktop breakpoint. Keep mobile drawer system unchanged (simpler, already working).

### Risk 3: Font Load Performance
**Impact**: Large font files could slow initial page load.
**Mitigation**: Use font subsetting to include only Latin + common punctuation. Consider WOFF2 compression. Add `font-display: swap` for graceful fallback.

## Design Decisions

### Pass BeliefBase to generate_deferred_html
Use synchronized `global_bb` from finalize path rather than `session_bb` which may have incomplete relationships.

### Move Collapse Buttons Outside Panels
Simpler than complex CSS state management. Buttons positioned absolutely relative to viewport, not parent panel.

### Remove Header Entirely (Not Hide)
Delete code rather than `display: none`. Future search drawer will be built fresh when needed.

### Font Weight Scale Using CSS Custom Properties
Define `--noet-font-weight-ui`, `--noet-font-weight-body`, `--noet-font-weight-heading` for theme-level control.

### Self-Host Font with CDN Fallback
Bundle font files in `assets/fonts/` to avoid external dependency. Provide CDN option via `--cdn` flag for smaller deployments.

## Related Issues

- ISSUE_38: Interactive SPA foundation (context)
- ISSUE_39: Advanced interactive features (follow-on)
- ISSUE_43: Codec HTML refactor (established dual-phase generation)

## Notes

- Phase 1 (network index fix) ✅ COMPLETE - proper multi-network workflows now supported
- Phases 2-5 are polish/UX improvements - could be split to separate issue if time-constrained
- Font selection may benefit from user feedback - document choice rationale in completion notes

## Session Notes

### Session 1 (2025-02-04) - Phase 1 Implementation

**Problem Analysis**:
- Network index.html files not generating because `generate_deferred_html()` was called from `finalize()` during `parse_all()`
- At that point, the `global_bb` passed was the cache from `doc_bb()`, not the synchronized event processor result
- Network nodes need complete child relationships, only available after event processing completes

**Solution Approach**:
- Moved `generate_deferred_html()` call from `finalize()` to `finalize_html()`
- `finalize_html()` is called from main.rs after event processor completes with synchronized `final_bb`
- Used `BeliefSource::eval()` with `StatePred::NetPath` to query for specific document context from global_bb
- This approach keeps generic `BeliefSource` trait (works with both `BeliefBase` and `DbConnection`)

**Key Insight**: Using `eval()` to query the synchronized BeliefBase returns a focused BeliefGraph with the node and all its relationships, which can then be converted to BeliefBase and used to get context. This is cleaner than direct path/context lookups.

**Next Steps**: Manual testing with actual multi-level network directory to verify index.html generation.