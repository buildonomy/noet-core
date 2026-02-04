# SCRATCHPAD - NOT DOCUMENTATION
# Phase 0 Testing Checklist

## Quick Start

```bash
cd test-output/
python3 -m http.server 8765
```

Open: http://localhost:8765/tests/browser/test_runner.html

---

## Test 1: Browser Test Runner

**URL**: http://localhost:8765/tests/browser/test_runner.html

**Expected**:
- ✅ All 12 tests pass (green summary at bottom)
- ✅ Test 12 specifically validates NavTree flat map structure
- ✅ Console shows no errors

**If fails**: Check console for details, report error message

---

## Test 2: Navigation Tree Rendering

**URL**: Any generated HTML (e.g., http://localhost:8765/index.html)

**Expected**:
- ✅ Navigation panel shows hierarchical tree
- ✅ Network at top (may have no link)
- ✅ Documents under network
- ✅ Sections under documents
- ✅ Toggle buttons (▶/▼) on items with children

**Test actions**:
1. Click toggle button on collapsed branch → should expand
2. Click toggle button on expanded branch → should collapse
3. Check console for "[Noet] Navigation tree built successfully"

---

## Test 3: Intelligent Expand/Collapse

**URL**: Any document with sections (e.g., http://localhost:8765/sections_test.html)

**Expected**:
- ✅ Current document branch is expanded
- ✅ Active section highlighted (if on section anchor)
- ✅ Other documents collapsed by default
- ✅ Parent chain visible (network → document → section)

**Test actions**:
1. Click link to another document
2. Verify new document branch expands
3. Verify old document branch collapses
4. Scroll to section with anchor (#section-id)
5. Verify that section highlighted in nav tree

---

## Test 4: Desktop Collapse Buttons (min-width: 1024px)

**Setup**: Resize browser to > 1024px width (desktop mode)

**Expected**:
- ✅ Collapse button visible on right edge of nav panel (◀)
- ✅ Collapse button visible on left edge of metadata panel (▶)
- ✅ Buttons have hover effect

**Test actions**:

### Nav Panel Collapse
1. Click nav collapse button (◀)
   - ✅ Nav panel fades out
   - ✅ Content expands to fill space
   - ✅ Button icon changes to ▶
2. Click again (▶)
   - ✅ Nav panel fades in
   - ✅ Content shrinks back
   - ✅ Button icon changes to ◀
3. Reload page
   - ✅ Nav panel state persisted (still collapsed if was collapsed)

### Metadata Panel Collapse
1. Click metadata collapse button (▶)
   - ✅ Metadata panel fades out
   - ✅ Content expands to fill space
   - ✅ Button icon changes to ◀
2. Click again (◀)
   - ✅ Metadata panel fades in
   - ✅ Content shrinks back
   - ✅ Button icon changes to ▶
3. Reload page
   - ✅ Metadata panel state persisted

### Both Collapsed (Reading Mode)
1. Collapse both panels
   - ✅ Content centered in viewport
   - ✅ Max-width: 800px for readability
   - ✅ Full reading mode achieved
2. Reload page
   - ✅ Both panels still collapsed

---

## Test 5: Keyboard Shortcuts (Desktop Only)

**Setup**: Desktop mode (> 1024px width)

**Test actions**:
1. Press `Ctrl+\` (Ctrl+Backslash)
   - ✅ Nav panel toggles
2. Press `Ctrl+]` (Ctrl+RightBracket)
   - ✅ Metadata panel toggles
3. Press shortcuts repeatedly
   - ✅ Panels toggle smoothly
4. Check console for no errors

---

## Test 6: Mobile Drawer Behavior (max-width: 1023px)

**Setup**: Resize browser to < 1024px width (mobile/tablet mode)

**Expected**:
- ✅ Nav panel is drawer (slides in from left)
- ✅ Metadata panel is drawer (slides up from bottom)
- ✅ Collapse buttons hidden (not needed for drawers)
- ✅ Hamburger menu visible in header
- ✅ Metadata drawer height: 40vh (not 60vh - Phase 0.1 fix)

**Test actions**:
1. Click hamburger menu (☰)
   - ✅ Nav drawer slides in from left
   - ✅ Backdrop appears behind
2. Click backdrop or close button
   - ✅ Nav drawer slides out
3. Click metadata toggle button
   - ✅ Metadata drawer slides up from bottom
   - ✅ Occupies 40vh of viewport
   - ✅ Content still visible above (60% of screen)
4. Swipe down or click close
   - ✅ Metadata drawer slides down

---

## Test 7: Error States

**Setup**: Simulate WASM failure

**Test actions**:
1. Stop server
2. Rename `test-output/pkg/` to `test-output/pkg-backup/`
3. Start server again
4. Reload page

**Expected**:
- ✅ Red error box appears in nav panel
- ✅ Error title: "Navigation Error"
- ✅ Error message: "Failed to load navigation data."
- ✅ Reload button present
- ✅ Console shows "[Noet] WASM initialization failed..."

**Test actions**:
1. Click "Reload Page" button
   - ✅ Page reloads (error persists since pkg still missing)
2. Restore pkg: `mv test-output/pkg-backup/ test-output/pkg/`
3. Reload page
   - ✅ Error gone
   - ✅ Navigation tree renders successfully

---

## Test 8: Theme System (Still Works)

**Test actions**:
1. Click theme selector in nav footer
2. Select "Dark"
   - ✅ Theme changes to dark
3. Reload page
   - ✅ Dark theme persisted
4. Select "System"
   - ✅ Theme matches OS preference
5. Select "Light"
   - ✅ Theme changes to light

---

## Test 9: Content Reading Mode

**URL**: Any document with long content

**Expected**:
- ✅ Content has max-width: 800px
- ✅ Content centered when panels collapsed
- ✅ Readable line length (not full viewport width)

**Test actions**:
1. Collapse both panels (reading mode)
   - ✅ Content remains centered
   - ✅ Max-width still applied
   - ✅ Comfortable reading experience

---

## Test 10: Cross-Browser (Optional but Recommended)

**Browsers to test**:
- Chrome/Chromium
- Firefox
- Safari (if on macOS)

**Check**:
- ✅ Navigation tree renders
- ✅ Collapse buttons work
- ✅ Keyboard shortcuts work
- ✅ Theme switching works
- ✅ No console errors

---

## Common Issues and Fixes

### Navigation Tree Not Rendering
- **Check**: Console for WASM errors
- **Fix**: Ensure `pkg/` directory exists and contains `noet_core_bg.wasm`
- **Fix**: Run `wasm-pack build --target web --out-dir pkg` if needed

### Collapse Buttons Not Visible
- **Check**: Browser width (must be > 1024px)
- **Check**: CSS loaded correctly (check Network tab)

### Keyboard Shortcuts Not Working
- **Check**: Desktop mode (> 1024px)
- **Check**: Focus not in input field
- **Check**: Console for JavaScript errors

### Panel State Not Persisting
- **Check**: localStorage enabled in browser
- **Check**: Console for localStorage errors
- **Clear**: Run `localStorage.clear()` in console to reset

### Mobile Drawer Too Tall
- **Check**: Metadata drawer height should be 40vh (not 60vh)
- **If wrong**: Check `noet-layout.css` line ~298-299

---

## Success Criteria

**Phase 0 is COMPLETE when**:
- ✅ All 12 automated tests pass
- ✅ Navigation tree renders with intelligent expand/collapse
- ✅ Desktop collapse buttons work (nav + metadata)
- ✅ Keyboard shortcuts work (Ctrl+\ and Ctrl+])
- ✅ Panel state persists across reload
- ✅ Mobile drawers work unchanged (40vh height)
- ✅ Error states display when WASM fails
- ✅ Reading mode works (both panels collapsed, centered content)
- ✅ No console errors during normal operation

**If all pass**: Ready for Phase 1 (Two-Click Navigation + Metadata Panel)

**If any fail**: Debug and fix before proceeding

---

## Reporting Issues

If you find bugs, note:
1. Which test failed
2. Expected behavior
3. Actual behavior
4. Console errors (if any)
5. Browser and OS
6. Screenshot (if relevant)

This helps diagnose and fix issues quickly.