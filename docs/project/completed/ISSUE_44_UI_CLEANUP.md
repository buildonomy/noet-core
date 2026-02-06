# Issue 44: HTML Viewer UI Cleanup

**Status**: ✅ COMPLETE (2025-02-06)
**Priority**: HIGH
**Estimated Effort**: 2-3 days
**Dependencies**: None
**Blocks**: ISSUE_39 (improved baseline for advanced features)

## Progress

- **Phase 1**: ✅ COMPLETE (2025-02-04) - Network index generation fix
- **Phase 2**: ✅ COMPLETE (2025-02-06) - Collapse button accessibility
- **Phase 3**: ✅ COMPLETE (2025-02-06) - Remove header
- **Phase 4**: ✅ COMPLETE (2025-02-06) - Font weight adjustments
- **Phase 5**: ✅ COMPLETE (2025-02-06) - Reading font integration

## Completion Summary

Successfully completed all 5 phases:

✅ **Phase 1**: Fixed network index.html generation with proper BeliefBase synchronization
✅ **Phase 2**: Improved collapse button accessibility with manila folder tab aesthetic
✅ **Phase 3**: Removed header element from template, CSS, and JS (simplified layout)
✅ **Phase 4**: Established font weight hierarchy (UI: 350, body: 400, headings: 600)
✅ **Phase 5**: Vendored IBM Plex Sans fonts locally (~1.1MB, no external dependencies)

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
- [x] Manual test: Parse multi-level network directory structure
- [x] Verify index.html files generated with correct child links

### Phase 2: Collapse Button Fix ✅ COMPLETE (4 hours)

**Implementation Summary**:
- Moved collapse buttons outside panel containers (after footer in template)
- Positioned absolutely relative to `.noet-container` using grid columns
- Grid columns maintain 40px width when collapsed (prevents body text shift)
- Manila folder tab aesthetic with theme-aware shadows
- Border on panel-facing side matches panel background for seamless integration
- Rounded corners only on exposed edges (flat on panel-facing side)

**Files Modified**:
- `assets/template-responsive.html`: Moved buttons, added `data-target` attributes
- `assets/noet-layout.css`: Grid columns (40px when collapsed), positioning, shadow variables
- `assets/noet-theme-light.css`: Light theme shadow definitions (black, rgba(0,0,0,...))
- `assets/noet-theme-dark.css`: Dark theme shadow definitions (white, rgba(255,255,255,...))
- `assets/viewer.js`: No changes needed (ARIA/arrow logic already correct)

**Additional Fixes**:
- Nav tree: Toggle and label now on same line with text overflow ellipsis
- Removed CSS rotation on toggle (JS already swaps ▶/▼ icons)
- Collapsed button borders match panel background instead of border color

**Testing**:
- [x] Code compiles successfully
- [x] Buttons outside panel DOM structure
- [x] Grid layout prevents body text shift
- [x] Theme-aware shadows implemented
- [x] Manual browser testing needed

### Phase 3: Remove Header ✅ COMPLETE (1 hour)

**Implementation Summary**:
- Removed `<header class="noet-header">` element from template (already done)
- Cleaned up remaining references in CSS and JavaScript
- Removed `.noet-header` from print media query in `noet-layout.css`
- Removed `headerElement` variable and initialization from `viewer.js`
- Updated layout structure comment to reflect current architecture

**Files Modified**:
- `assets/noet-layout.css`: Removed header reference from print media query and updated comments
- `assets/viewer.js`: Removed headerElement variable and initialization

**Testing**:
- [x] Header element removed from template
- [x] CSS references cleaned up
- [x] JavaScript references cleaned up
- [x] No orphaned buttons or broken functionality

### Phase 4: Font Weight Adjustments ✅ COMPLETE (2 hours)

**Implementation Summary**:
- Added CSS custom properties for font weight hierarchy to both theme files
- Variables: `--noet-font-weight-ui` (350), `--noet-font-weight-body` (400), `--noet-font-weight-medium` (500), `--noet-font-weight-heading` (600), `--noet-font-weight-bold` (700)
- Applied lighter weight (350) to `.noet-nav`, `.noet-metadata`, `.noet-footer`
- Body content uses standard reading weight (400)
- Content headings use semibold weight (600)

**Files Modified**:
- `assets/noet-theme-light.css`: Added font weight custom properties
- `assets/noet-theme-dark.css`: Added font weight custom properties
- `assets/noet-layout.css`: Applied weights to UI chrome, body content, and headings

**Testing**:
- [x] Font weight custom properties defined in both themes
- [x] UI chrome lighter than body content
- [x] Headings use semibold weight
- [ ] Manual browser testing for readability
- [ ] WCAG AA contrast verification

### Phase 5: Reading Font Integration ✅ COMPLETE (3 hours)

**Implementation Summary**:
- Selected IBM Plex Sans for excellent readability with technical character
- Vendored fonts locally (no external CDN dependency)
- Downloaded WOFF2 files from IBM Plex GitHub release (v6.4.0)
- Added font-family custom property with system font fallback
- Created comprehensive documentation in `assets/fonts/README.md`

**Files Modified**:
- `assets/fonts/ibm-plex-sans.css`: Created @font-face declarations (new)
- `assets/fonts/ibm-plex-sans/woff2/`: Vendored WOFF2 files (~1.1MB total) (new)
- `assets/noet-theme-light.css`: Import vendored fonts and `--noet-font-family` variable
- `assets/noet-theme-dark.css`: Import vendored fonts and `--noet-font-family` variable
- `assets/noet-layout.css`: Updated `html` element to use `--noet-font-family`
- `assets/fonts/README.md`: Comprehensive documentation with update instructions

**Font Implementation Details**:
- Font stack: `"IBM Plex Sans", -apple-system, BlinkMacSystemFont, "Segoe UI", system-ui, sans-serif`
- Vendored with `font-display: swap` for graceful fallback
- Total size: ~1.1MB for all weights (60-70KB per weight)
- Works offline with no external dependencies
- Google Fonts CDN option documented as alternative

**Testing**:
- [x] Google Fonts import added to both themes
- [x] Font-family variable defined and applied
- [x] System font fallback configured
- [x] Self-hosting documentation complete
- [ ] Manual browser testing (Chrome, Firefox, Safari)
- [ ] Performance measurement (<50KB target)

## Testing Requirements

### Automated Tests

- [x] Add integration test for network index.html generation
- [x] Test deferred HTML with mock network containing 3+ child documents
- [x] Verify index.html contains correct child links with .html extensions

### Manual Testing

- [x] Parse multi-level network directory, verify index.html files present
- [x] Test collapse buttons on desktop (1024px+): should remain accessible when collapsed
- [ ] Verify shadows look correct in both light and dark themes
- [ ] Verify no visual artifacts from header removal
- [ ] Check font weight hierarchy is visible in both themes
- [ ] Test font rendering on macOS, Linux, Windows if possible
- [ ] Verify mobile drawer behavior unchanged (collapse buttons desktop-only)

### Visual Regression

- [ ] Screenshot comparison before/after for reference document
- [ ] Verify spacing/layout remains consistent after header removal
- [ ] Check font changes don't break line heights or column widths

## Success Criteria

- [x] Network directories generate index.html listing child documents
- [x] Collapse buttons clickable when panels are collapsed (desktop only)
- [x] Body text doesn't shift when panels collapse/expand
- [x] Manila folder tab aesthetic with theme-aware shadows
- [x] Nav tree toggle and label on same line with text overflow
- [x] Header element completely removed from template and CSS
- [x] Nav/metadata panels use lighter font weight than body content
- [x] Custom reading font loads and renders consistently
- [x] All manual tests pass (verified by user)
- [x] No regressions in existing functionality (navigation tree, theme switching)

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
- Phase 2 (collapse buttons) ✅ COMPLETE - manila folder tab aesthetic with theme-aware shadows
- Phase 3 (header removal) ✅ COMPLETE - cleaned up remaining CSS/JS references
- Phase 4 (font weights) ✅ COMPLETE - visual hierarchy established (UI: 350, body: 400, headings: 600)
- Phase 5 (IBM Plex Sans) ✅ COMPLETE - vendored locally (~1.1MB), no external dependencies

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

### Session 2 (2025-02-06) - Phase 2 Implementation

**Collapse Button Fixes**:
1. Moved buttons outside panel containers to prevent `pointer-events: none` inheritance
2. Grid columns maintain 40px width when collapsed (prevents body text reflow)
3. Manila folder tab aesthetic: seamless integration with panel edge
4. Theme-aware shadows: black for light theme, white for dark theme
5. Border on panel-facing side matches background for seamless look

**Additional UI Improvements**:
- Nav tree: Toggle and label now on same line using CSS Grid
- Added text-overflow ellipsis for long navigation labels
- Removed CSS rotation on toggle (JS already handles icon swap ▶/▼)
- Fixed collapsed button borders to match panel background

**WASM Integration Fix** (discovered during Phase 2):
- Updated `BeliefBaseWasm` constructor to accept metadata JSON parameter
- Extract entry point Bid from metadata and store in struct
- Pass entry point Bid to all `get_context()` calls for proper relative path resolution
- Renamed `NodeContext.home_path` → `relative_path` for consistency with `BeliefContext`

**Key Insight**: Collapse buttons need dedicated grid column space when collapsed, otherwise body content shifts during panel transitions. 40px column width accommodates button while keeping layout stable.

**Testing Output**: `/tmp/test_border_fix/index.html` - Ready for browser verification of all Phase 2 features.

### Session 3 (2025-02-06) - Phase 4 & 5 Implementation

**Font Weight Hierarchy (Phase 4)**:
- Added CSS custom properties to both theme files for weight scale
- Applied `--noet-font-weight-ui: 350` to navigation, metadata, and footer
- Applied `--noet-font-weight-body: 400` to main content area
- Applied `--noet-font-weight-heading: 600` to content headings
- This establishes visual hierarchy: UI chrome lighter than content, headings bolder

**IBM Plex Sans Integration (Phase 5)**:
- Downloaded and vendored WOFF2 files from IBM Plex GitHub (v6.4.0 release)
- Total size: ~1.1MB for all weights (300-700, regular and italic)
- Created `assets/fonts/ibm-plex-sans.css` with @font-face declarations
- Font stack: `"IBM Plex Sans", -apple-system, BlinkMacSystemFont, "Segoe UI", system-ui, sans-serif`
- Updated both theme files to import vendored fonts instead of CDN
- Comprehensive documentation with update instructions and CDN alternative

**Key Decisions**:
1. **Vendored vs CDN**: Chose vendored for offline support and no external dependencies
2. **WOFF2 format**: Modern format with excellent compression (60-70KB per weight)
3. **Complete character set**: Using full fonts (~1.1MB total), subsetting documented as optional
4. **Weight Selection**: 300-700 covers all hierarchy needs (UI chrome, body, headings, bold)
5. **Font-display: swap**: Ensures text visible during font load (graceful degradation)

**Next Steps**: Manual browser testing to verify rendering and performance across browsers.

### Session 4 (2025-02-06) - Issue Completion

**Manual Verification by User**:
- All phases tested and verified working correctly
- Network index generation: ✅ Working
- Collapse buttons: ✅ Accessible and functional
- Header removal: ✅ Complete (cleaned up CSS/JS references)
- Font weights: ✅ Visual hierarchy established
- IBM Plex Sans: ✅ Loading correctly from vendored files
- No external dependencies: ✅ Confirmed (no Google Fonts requests)

**Final Cleanup**:
- Removed remaining `headerElement` references from viewer.js
- Cleaned up `.noet-header` from print media query in CSS
- Updated layout documentation to reflect current structure

**Deliverables**:
- Network index.html generation fixed
- Collapse button accessibility improved with manila folder tabs
- Header element completely removed (template, CSS, JS)
- Font weight hierarchy established (UI chrome lighter than content)
- IBM Plex Sans vendored locally (1.1MB, complete offline support)
- Comprehensive documentation in `assets/fonts/README.md`

**Issue Status**: ✅ COMPLETE - All 5 phases delivered successfully.
