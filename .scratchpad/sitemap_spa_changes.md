# SCRATCHPAD - NOT DOCUMENTATION

## Sitemap and SPA Integration Changes

**Session Date**: 2026-02-16

### Summary

Implemented proper sitemap generation and SPA integration following progressive enhancement principles:

1. **Sitemap points to static content pages** in `/pages/` subdirectory
2. **Static pages include "View Interactive Version" link** to SPA routes
3. **SPA extracts only `<article>` content**, excluding navigation chrome
4. **Canonical URLs point to SPA routes** for SEO
5. **Base URL support** via CLI flag or environment variable
6. **Context-aware link rewriting** - internal links work in both static and SPA contexts

### Architecture Pattern

**Static Page Structure:**
```html
<body>
    <nav class="spa-upgrade">
        <a href="/#/file1.html">📱 View Interactive Version</a>
    </nav>
    <article>
        <h1>Document Title</h1>
        <!-- Document content -->
    </article>
</body>
```

**Sitemap URLs** (with base URL):
```xml
<loc>https://example.github.io/repo/pages/file1.html</loc>
```

**Canonical URLs** (with base URL):
```html
<link rel="canonical" href="https://example.github.io/repo/#/file1.html" />
```

**SPA extraction**: `doc.querySelector('article').innerHTML` - only gets content, excludes nav link

### User Flows

1. **Search Engine → User**:
   - Google indexes `/pages/file1.html`
   - User clicks search result
   - Lands on static page (works without JS)
   - Can click "View Interactive Version" to load SPA

2. **Direct SPA Navigation**:
   - User types `/#/file1.html`
   - SPA shell loads
   - Fetches from `/pages/file1.html`
   - Extracts `<article>` content only
   - Nav link never appears in SPA

3. **Shared Static Link**:
   - Someone shares `/pages/file1.html`
   - Recipient clicks
   - Static page loads (universal access)
   - Optional upgrade to SPA

### Files Changed

1. **assets/template-simple.html**
   - Added `<nav class="spa-upgrade">` with `{{SPA_ROUTE}}` placeholder
   - Wrapped title and body in `<article>` tag
   - Added link rewriting script that detects static vs SPA context

2. **assets/viewer.js**
   - Changed extraction from `doc.body.innerHTML` to `doc.querySelector('article').innerHTML`

3. **src/codec/compiler.rs**
   - Added `base_url: Option<String>` field to `DocumentCompiler`
   - Updated `write_fragment()`: Generate SPA route, use base URL for canonical
   - Updated `generate_sitemap()`: Point to `/pages/` subdirectory, use base URL
   - Updated constructors to accept `base_url` parameter

4. **src/watch.rs**
   - Added `base_url` field to `WatchService` struct
   - Pass through to `FileUpdateSyncer` and `DocumentCompiler`

5. **src/bin/noet/main.rs**
   - Added `--base-url` CLI argument to `parse` and `watch` commands
   - Read from `NOET_BASE_URL` environment variable as fallback

### Usage

**Without base URL** (local development):
```bash
noet parse docs/ --html-output output/
```

Generates:
- Sitemap: `/pages/file1.html` (relative URLs)
- Canonical: `/#/file1.html` (relative)

**With CLI flag**:
```bash
noet parse docs/ --html-output output/ --base-url "https://example.github.io/repo"
```

**With environment variable**:
```bash
export NOET_BASE_URL="https://example.github.io/repo"
noet parse docs/ --html-output output/
```

Generates:
- Sitemap: `https://example.github.io/repo/pages/file1.html` (absolute URLs)
- Canonical: `https://example.github.io/repo/#/file1.html` (absolute)

### Benefits

✅ **SEO-friendly**: Crawlers can index static content directly  
✅ **Progressive enhancement**: Content works without JavaScript  
✅ **User choice**: Static page offers SPA upgrade  
✅ **Universal access**: Shared links work for everyone  
✅ **Semantic HTML**: `<article>` properly separates content from chrome  
✅ **Clean SPA integration**: Automatic content extraction without guards  
✅ **Smart link handling**: Internal links work correctly in both contexts

### Testing

Verified with `tests/network_1` fixtures:

1. **Without base URL**:
   - Sitemap: `/pages/file1.html`
   - Canonical: `/#/file1.html`
   - Nav link: `href="/#/file1.html"`

2. **With base URL** (`https://example.github.io/noet-test`):
   - Sitemap: `https://example.github.io/noet-test/pages/file1.html`
   - Canonical: `https://example.github.io/noet-test/#/file1.html`
   - Nav link: `href="/#/file1.html"` (always relative for navigation)

3. **Environment variable**: Works as expected

### Anchor Navigation Fix

**Issue**: When clicking anchor-only links (e.g., `<a href="#explicit-brefs">`) in the SPA, the URL would lose the document path:
- Before: On `/#link_manipulation_test.html`, click `#explicit-brefs` → URL becomes `/#explicit-brefs` ❌
- Document path lost, subsequent navigation breaks

**Root cause**: `navigateToSection()` in viewer.js used `history.replaceState(null, "", anchor)` which only set the anchor part, losing the document path.

**Solution**: Updated `navigateToSection()` to use `PathParts` from WASM module:
1. Parse current hash using `wasmModule.BeliefBaseWasm.pathParts(currentHash)`
2. Extract document path (directory + filename)
3. Combine with new anchor: `#${docPath}${anchor}`
4. Result: `/#link_manipulation_test.html#explicit-brefs` ✓

**Changes**:
- `src/wasm.rs`: Added `#[wasm_bindgen(js_name = pathParts)]` to export method
- `assets/viewer.js`: Updated `navigateToSection()` to preserve document path using PathParts
- Rebuilt WASM via `scripts/build-full.sh` to export new API

### Link Rewriting Implementation

**Problem**: Internal links in static pages are relative to `/pages/`, but when extracted into SPA context (served from `/index.html`), they would resolve incorrectly.

**Solution**: Added inline script to `template-simple.html` that:
1. Detects context by checking `window.location.pathname.startsWith("/pages/")`
2. If static page: does nothing (links work as-is)
3. If SPA context: rewrites internal links to hash routes on `DOMContentLoaded`

**Example**:
- HTML contains: `<a href="net1_dir1/hsml.html">`
- Static page view (`/pages/file1.html`): Link stays as-is, resolves to `/pages/net1_dir1/hsml.html` ✓
- SPA view (`/#/file1.html`): Script rewrites to `<a href="/#/net1_dir1/hsml.html">` ✓

**Skips**:
- External links (`http://`, `https://`)
- Anchor links (`#section`)
- Email links (`mailto:`)
- Already hash-routed links

### Next Steps

- [x] Add styling for `.spa-upgrade` nav link
- [x] Test link rewriting in actual browser (both contexts)
- [x] Fix anchor navigation to preserve document path
- [x] Standardize on pathParts API, remove getDirPath
- [ ] Consider auto-redirect option (with delay/user preference)
- [ ] Document GitHub Pages deployment workflow
- [ ] Add sitemap validation to tests

### API Cleanup: PathParts Standardization

**Issue**: `getDirPath` had ambiguous behavior:
- For `net1_dir1/hsml.html#section` → returned `net1_dir1/hsml.html` (stripped anchor)
- For `net1_dir1/hsml.html` → returned `net1_dir1` (returned directory)
- For `net1_dir1/subdir` → returned `net1_dir1/subdir` (treated as directory, returned as-is)

The third case was wrong - should return parent `net1_dir1`.

**Solution**: Removed `getDirPath`, standardized on `pathParts`:
- Returns structured object: `{path: "dir", filename: "file.html", anchor: "#section"}`
- Clear semantics - no ambiguity about what each field contains
- Used consistently in both `viewer.js` and tests

**Changes**:
- `src/wasm.rs`: Removed `get_dir_path()` method
- `assets/viewer.js`: Changed `getDirPath(currentHash)` → `pathParts(currentHash).path`
- `tests/browser/test_runner.html`: Replaced all `getDirPath` tests with `pathParts` tests

**Example**:
```javascript
const parts = BeliefBaseWasm.pathParts('net1_dir1/hsml.html#section');
// parts.path = "net1_dir1"
// parts.filename = "hsml.html"
// parts.anchor = "#section"
```

### Directory Path Handling in Nav Tree

**Issue**: Network directory nodes like `subnet1` had path `"subnet1"` without `.html` extension, failing nav tree validation.

**Solution**: Updated `normalize_path_extension()` to detect directory paths (no extension) and append `/index.html`.

**Changes**:
- `src/wasm.rs`: Modified `normalize_path_extension()` to handle directory paths
- Result: `subnet1` → `subnet1/index.html`

### Session Complete: All Tests Passing ✅

Complete progressive enhancement architecture with robust path handling:

**Core Features:**
- ✅ Sitemap points to `/pages/` static content with optional fully-qualified URLs
- ✅ Static pages work standalone with "View Interactive" upgrade link
- ✅ SPA extracts `<article>` content only, excludes chrome
- ✅ Context-aware link rewriting (static pages only)
- ✅ Anchor navigation preserves document path using PathParts
- ✅ Base URL support via CLI or `NOET_BASE_URL` environment variable
- ✅ Directory nodes properly link to `/index.html`

**Path API Improvements:**
- ✅ Removed ambiguous `getDirPath`, standardized on `pathParts`
- ✅ Added `pathParent()` for semantic parent traversal
- ✅ Added `pathFilestem()` for filename without extension
- ✅ `pathJoin()` always normalizes output (resolves `..` segments)
- ✅ All path utilities consistently handle files, directories, and anchors
- ✅ Test suite fully validates all path operations

**Files Modified:**
- `assets/template-simple.html` - Added nav link, article wrapper, link rewriting script
- `assets/viewer.js` - Fixed anchor navigation, uses pathParts consistently
- `src/wasm.rs` - Removed getDirPath, added pathParent/pathFilestem, fixed normalize_path_extension
- `src/codec/compiler.rs` - Added base_url support throughout
- `src/watch.rs` - Pass base_url through to compiler
- `src/bin/noet/main.rs` - Added --base-url CLI flag with env var fallback
- `tests/browser/test_runner.html` - Updated all path tests, fixed expectations

**Test Results:**
All browser tests passing with correct path normalization expectations.