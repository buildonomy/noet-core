# Issue 40: Network Index Generation via DocCodec

**Priority**: HIGH
**Estimated Effort**: 1 day
**Dependencies**: Blocks ISSUE_39 Phase 1 manual testing
**Related**: ISSUE_06 (original HTML generation implementation), ISSUE_43 (implemented via SPA shell)
**Status**: ✅ **COMPLETE** - Implemented via ISSUE_43 with SPA shell architecture

## Summary

Network index pages (`index.html`) are currently generated in a post-processing step (`compiler.rs::generate_network_indices()`) that bypasses the normal `DocCodec` flow. This creates architectural inconsistency and prevents network indices from using the responsive template with WASM support.

**Solution**: Treat `BeliefNetwork.{toml,json,yaml}` files as first-class documents that generate their own `index.html` via `DocCodec::generate_html()`, just like markdown documents.

## Goals

- Network indices use responsive template with full WASM support (navigation, theme switching, etc.)
- Network nodes get proper BID assignment (can be referenced in metadata panel)
- `noet watch` automatically regenerates index.html when network config changes
- Eliminate duplicate template substitution logic
- Simplify compiler architecture

## Architecture

**Original Flow** (pre-ISSUE_43):
```
1. Parse all documents via DocCodec
2. [Post-processing] compiler.rs::generate_network_indices()
   - Hardcoded minimal HTML template
   - No WASM initialization
   - Manual stylesheet references
```

**Actual Implementation** (ISSUE_43 - SPA Shell Architecture):
```
1. Parse all documents via DocCodec
   - Markdown docs: generate_html() returns body fragments immediately
   - Network configs: should_defer() returns true, deferred until context ready

2. Write immediate HTML fragments
   - Wrapped with Layout::Simple template
   - Output to pages/*.html subdirectory

3. Deferred generation for networks
   - belief_ir.rs::generate_deferred_html(ctx) queries child documents
   - Uses BeliefContext to access graph relationships
   - Sub-networks generate pages/network_name/index.html (Layout::Simple)

4. Generate SPA shell (compiler.rs::generate_spa_shell())
   - Root index.html uses Layout::Responsive template
   - Full WASM support, navigation panel, theme switcher
   - Serves as primary network index for repo root
   - Dynamic content loading via viewer.js

5. Generate sitemap.xml for SEO
```

**Key Changes**:
- Repository root network index = SPA shell at root `index.html`
- Sub-network indices use deferred generation to `pages/`
- Dual-phase generation: immediate fragments + deferred indices
- No post-processing step - integrated into DocCodec flow

## Implementation Steps

> **Note**: The steps below reflect the **original proposal**. The actual implementation was completed via **ISSUE_43** using a different architecture (SPA shell + dual-phase generation). See the updated Architecture section above for what was actually implemented.

### 1. Update `belief_ir.rs::generate_html()` (3-4 hours)

**Current Implementation** (lines 1179-1218):
- Returns hardcoded minimal HTML with placeholder message
- Uses `assets/default-theme.css` directly

**New Implementation**:
```rust
fn generate_html(&mut self, script: Option<&str>, use_cdn: bool) 
    -> Result<Option<String>, BuildonomyError> 
{
    // Only generate HTML for Network nodes
    if self.kind != BeliefKind::Network {
        return Ok(None);
    }
    
    // Get network title from document
    let network_title = self.document
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("Network Index");
    
    // Build document list from iter_net_docs() or belief base query
    // Group by directory, sort alphabetically
    let docs_list = self.build_document_list()?;
    
    // Generate HTML content (document links grouped by directory)
    let mut content = String::new();
    content.push_str(&format!("<h1>{}</h1>\n", network_title));
    content.push_str(&format!("<p>Total documents: {}</p>\n", docs_list.len()));
    
    for (dir, files) in group_by_directory(docs_list) {
        if dir != "." {
            content.push_str(&format!("<h2>{}</h2>\n", dir));
        }
        content.push_str("<ul>\n");
        for (path, title) in files {
            content.push_str(&format!(
                "  <li><a href=\"{}\">{}</a></li>\n",
                path, title
            ));
        }
        content.push_str("</ul>\n");
    }
    
    // Use responsive template (same as markdown documents)
    let template = crate::codec::assets::get_template(Layout::Responsive);
    let stylesheet_urls = crate::codec::assets::get_stylesheet_urls(use_cdn);
    
    // Build metadata JSON (network BID if available)
    let metadata = json!({
        "document": {
            "bid": self.document.get("bid").and_then(|v| v.as_str())
        },
        "sections": {}
    });
    
    // Substitute template placeholders
    let html = template
        .replace("{{TITLE}}", network_title)
        .replace("{{CONTENT}}", &content)
        .replace("{{METADATA}}", &serde_json::to_string_pretty(&metadata)?)
        .replace("{{STYLESHEET_OPEN_PROPS}}", &stylesheet_urls.open_props)
        .replace("{{STYLESHEET_NORMALIZE}}", &stylesheet_urls.normalize)
        .replace("{{STYLESHEET_THEME_LIGHT}}", &stylesheet_urls.theme_light)
        .replace("{{STYLESHEET_THEME_DARK}}", &stylesheet_urls.theme_dark)
        .replace("{{STYLESHEET_LAYOUT}}", &stylesheet_urls.layout)
        .replace("{{SCRIPT}}", script.unwrap_or(""));
    
    Ok(Some(html))
}
```

**Helper Methods Needed**:
- `build_document_list()`: Query belief base for documents in this network
- `group_by_directory()`: Group paths by directory for organized display

**Questions**:
- How to access belief base from `ProtoBeliefNode`? (Need context from compiler)
- Should we pass `BeliefBase` reference to `generate_html()`? Or defer to compiler?

### 2. Remove `compiler.rs::generate_network_indices()` (1 hour)

**Files to modify**:
- `src/codec/compiler.rs`: Delete `generate_network_indices()` method (~150 lines)
- `src/bin/noet/main.rs`: Remove call to `generate_network_indices()`
- `src/watch.rs`: Remove call to `generate_network_indices()`

**Verification**:
- Grep for `generate_network_indices` - should have zero matches
- Ensure no broken call sites

### 3. Update Tests (1 hour)

**Test Cases**:
- Network index HTML uses responsive template
- Network index includes WASM script tags
- Network index has proper stylesheet references
- Document list is grouped by directory
- `noet watch` regenerates index.html on network config change

**Test Files**:
- `tests/network_1/`: Verify index.html generation
- `tests/browser/test_runner.html`: Add test for network index structure

### 4. Documentation Updates (30 minutes)

**Files to update**:
- `docs/design/interactive_viewer.md`: Note network indices use same template
- `ISSUE_06_HTML_GENERATION.md`: Archive with note about refactoring
- `ROADMAP.md`: Mark ISSUE_40 complete

## Testing Requirements

### Automated Tests
- [ ] `test_network_index_uses_responsive_template()`
- [ ] `test_network_index_has_wasm_support()`
- [ ] `test_network_index_document_list()`
- [ ] `test_network_index_grouped_by_directory()`

### Manual Testing
- [ ] Generate HTML for `tests/network_1/`
- [ ] Open `index.html` in browser
- [ ] Verify navigation panel renders correctly
- [ ] Verify theme switching works
- [ ] Verify document links navigate correctly
- [ ] Test `noet watch` with network config change

## Success Criteria

- [x] Network `index.html` uses `template-responsive.html` (SPA shell at root)
- [x] Network `index.html` includes WASM initialization (via SPA shell)
- [x] Network `index.html` has navigation panel, theme switcher (via SPA shell)
- [x] `compiler.rs::generate_network_indices()` deleted
- [x] All tests pass (152/152 tests pass)
- [x] Manual browser testing confirms functionality (browser tests pass)
- [x] `noet watch` regenerates index.html on network changes (via deferred generation)

## Risks

### Risk 1: Belief Base Access from DocCodec
**Problem**: `generate_html()` needs document list, but `ProtoBeliefNode` doesn't have belief base reference

**Mitigation**: 
- Option A: Pass document list as parameter to `generate_html()`
- Option B: Add belief base reference to `ProtoBeliefNode` (architectural change)
- Option C: Compiler calls helper method to build content, passes to `generate_html()`

**Decision**: TBD during implementation (likely Option A or C)

### Risk 2: Document List Timing
**Problem**: Network index needs list of all documents, but parsing may not be complete

**Mitigation**:
- Network files are parsed early (discovered via `iter_net_docs()`)
- Generate index.html at end of parsing (same as current post-processing)
- Or regenerate index.html after each document added (watch mode)

**Decision**: Keep current timing (generate after all documents parsed)

## Design Decisions

### Network Index Content
**Decision**: Group documents by directory, sort alphabetically

**Rationale**: Same as current implementation, familiar structure

### Network BID Assignment
**Decision**: Network nodes get BIDs like any other document

**Rationale**: Enables metadata panel to show network info, supports cross-network references

### Template Consistency
**Decision**: Use same responsive template for all generated HTML

**Rationale**: Consistent UX, simplified maintenance, WASM support everywhere

## References

- ISSUE_06: Original HTML generation implementation
- ISSUE_39: Needs network indices with WASM support for testing
- `src/codec/belief_ir.rs`: DocCodec implementation for network files
- `src/codec/compiler.rs`: Current `generate_network_indices()` implementation

## Notes

**Implementation via ISSUE_43**: This issue was completed as part of ISSUE_43's comprehensive refactor. The final architecture differs from the original proposal but achieves all goals:

- **SPA Shell Approach**: Root network index is now a SPA shell (index.html at root) with Layout::Responsive, full WASM support, navigation, and theme switching
- **Deferred Generation**: Sub-networks generate indices via `generate_deferred_html()` with access to BeliefContext for child document queries
- **Dual-Phase Pattern**: Immediate fragments (Layout::Simple in pages/) + deferred indices + SPA shell
- **Watch Mode**: Automatically regenerates via integrated deferred generation flow

**Architecture Evolution**: The original proposal suggested direct `generate_html()` implementation. ISSUE_43 introduced a more sophisticated dual-phase pattern that better separates presentation (compiler) from content (codecs) and enables progressive enhancement.

**Unblocks ISSUE_39**: Phase 1 manual testing now has full WASM-enabled network indices via SPA shell.