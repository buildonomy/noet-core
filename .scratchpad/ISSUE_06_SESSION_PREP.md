# SCRATCHPAD - NOT DOCUMENTATION

**Session**: Preparing for Issue 06 (HTML Generation) Implementation
**Date**: 2025-01-23
**Purpose**: Context and planning notes for next session

---

## Session Goal

Implement **Issue 06: HTML Generation and Interactive Viewer**
- Focus on **Phase 1 (Static HTML)** first - get something working
- Defer WASM (Phase 2) until Phase 1 complete

---

## Key Decisions from This Session

### 1. Theming Strategy (Decided)
- **MVP**: Bundle ONE good default CSS (just-the-docs or minima)
- Add `--css` flag for custom override
- Generate clean semantic HTML
- ~5 hours total effort for theming MVP
- **Defer to Issue 25**: Per-network theming (uses BeliefNetwork payload)

### 2. Architecture Confirmed
- Static HTML generation (Jekyll-style)
- WASM-powered SPA for progressive enhancement
- **SPA navigation critical**: Prevents WASM reload between pages
- Pure Rust toolchain (no language mixing)

### 3. Libraries to Use
- `pulldown-cmark` (already in deps) - Markdown parsing
- `maud` - Compile-time HTML templates (recommended)
- `wasm-bindgen` (already in deps) - WASM bindings
- **NOT using Leptos** (overkill for static docs)

### 4. Integration with Other Features
- **Flutter app**: Separate use case (native widgets, not WebView)
- **LSP (Issues 11-12)**: Can reuse logic via FFI in Flutter later
- **Git-aware networks (Issue 26)**: Future - enables version diffing in browser

### 5. Dogfooding: CI/CD Integration (CRITICAL)
**As soon as Phase 1 complete, add to CI:**
- Export `docs/design/` to HTML on every push to main
- Deploy to GitHub Pages (or artifact)
- **Benefits**: Validate HTML export works, showcase feature, provide browsable docs
- **File**: `.github/workflows/test.yml` (add new job) or create `.github/workflows/docs-deploy.yml`

---

## Implementation Order (Phase 1 Focus)

### Step 1: Extend DocCodec Trait (START HERE)
**File**: `src/codec/mod.rs`
```rust
pub trait DocCodec {
    // ... existing methods
    
    fn generate_html(&self, options: &HtmlGenerationOptions) -> Result<String, BuildonomyError>;
}

pub struct HtmlGenerationOptions {
    pub render_metadata: MetadataRenderMode,
    pub include_bid_attributes: bool,
    pub css_class_prefix: String,
    pub inject_viewer_script: bool,
    pub custom_css: Option<PathBuf>,
}
```

### Step 2: Implement for MdCodec
**File**: `src/codec/md.rs`
- Use `pulldown-cmark::html::push_html()` for markdown → HTML
- Wrap with document structure using `maud`
- Generate `data-bid` attributes on elements
- Handle heading hierarchy

### Step 3: Bundle Default Theme
- Pick one theme (just-the-docs recommended)
- Copy CSS to `assets/default-theme.css`
- Adjust HTML class names to match theme

### Step 4: CLI Command
**File**: `src/bin/noet.rs` or new `src/export/html.rs`
```rust
#[derive(Parser)]
struct ExportHtmlArgs {
    /// Input directory
    input: PathBuf,
    
    /// Output directory
    #[arg(long, short)]
    output: PathBuf,
    
    /// Custom CSS file
    #[arg(long)]
    css: Option<PathBuf>,
}
```

### Step 5: Test with noet-core's Own Docs
- Export `docs/design/*.md` to HTML
- Validate links work
- Check styling renders correctly

---

## Files to Review First

### Core Infrastructure
1. `src/codec/mod.rs` - DocCodec trait definition
2. `src/codec/md.rs` - MdCodec implementation (markdown parsing)
3. `src/beliefbase.rs` - BeliefBase structure (for export)
4. `Cargo.toml` - Check existing dependencies

### Reference Examples
1. `docs/design/architecture.md` - Example markdown to export
2. `docs/design/link_format.md` - Another test document

---

## Dependencies to Add

```toml
[dependencies]
maud = "0.26"  # HTML templating
```

Already have:
- `pulldown-cmark = "0.13.0"` ✓
- `serde_json = "1.0"` ✓
- `wasm-bindgen` (for Phase 2)

---

## Dogfooding: GitHub Actions Workflow

**After Phase 1 complete, add job to `.github/workflows/test.yml`:**

```yaml
# Export design docs to HTML (dogfooding Issue 06)
export-docs:
  name: Export Documentation to HTML
  runs-on: ubuntu-latest
  if: github.event_name == 'push' && github.ref == 'refs/heads/main'
  needs: [test-summary]
  steps:
    - uses: actions/checkout@v4
    
    - name: Install Rust
      uses: dtolnay/rust-toolchain@stable
    
    - name: Build noet
      run: cargo build --release
    
    - name: Export docs/design to HTML
      run: |
        ./target/release/noet export-html ./docs/design --output ./docs-html
    
    - name: Upload HTML artifact
      uses: actions/upload-artifact@v4
      with:
        name: design-docs-html
        path: docs-html
        retention-days: 30
    
    # Optional: Deploy to GitHub Pages
    - name: Deploy to GitHub Pages
      uses: peaceiris/actions-gh-pages@v3
      with:
        github_token: ${{ secrets.GITHUB_TOKEN }}
        publish_dir: ./docs-html
        publish_branch: gh-pages
```

**Result**: Every push to main exports design docs to HTML and deploys to `https://buildonomy.github.io/noet-core/`

---

## Quick Wins to Start With

1. **Minimal HTML generator**: Just wrap markdown output in basic HTML structure
2. **Test with single file**: Don't worry about multi-file export yet
3. **Inline CSS first**: Embed default theme inline, optimize later
4. **Skip WASM initially**: Get static HTML working first

---

## Potential Blockers to Watch

1. **Link format handling**: Need to convert NodeKey links to HTML anchors
2. **BeliefNetwork path resolution**: Multi-network exports might be complex
3. **Frontmatter rendering**: How to display YAML/TOML metadata in HTML
4. **CSS class consistency**: Keep HTML structure simple to avoid theme conflicts

---

## Testing Strategy

### Manual Testing First
```bash
# Test workflow
cargo run -- export-html ./docs/design --output ./test-output
open ./test-output/architecture.html
# Verify: renders, links work, styling looks good
```

### Unit Tests Later
- `generate_html()` with various markdown inputs
- Link conversion (NodeKey → HTML anchors)
- Metadata rendering modes

---

## Related Issues (Context)

- **Issue 25**: Per-network theming (deferred, uses BeliefNetwork payload)
- **Issue 26**: Git-aware networks (future, enables version diffing)
- **Issues 11-12**: LSP (separate, but logic can integrate with Flutter later)

---

## Session End Notes

**What we explored:**
- HTML generation architecture and theming strategy
- Confirmed pure Rust approach (no Leptos, no language mixing)
- Clarified Flutter app is separate use case (not using HTML viewer)
- Validated LSP can integrate with Flutter via FFI (future work)
- Created Issues 25 (theming) and 26 (git-aware networks)
- **Identified future enhancement**: Cross-link `cargo doc` (API docs) with `docs/design/` (design docs)

**Next session should:**
1. Read existing DocCodec/MdCodec implementation
2. Start with Step 1 (extend DocCodec trait)
3. Build minimal HTML generator for single file
4. Test with one markdown doc from `docs/design/`
5. **Add GitHub Actions workflow** to dogfood the feature (export docs on every push)

**Estimated Phase 1 completion**: 7-10 days (per Issue 06)

**Dogfooding validation**: As soon as export works, add to CI to ensure it keeps working

---

## Future Enhancement: Unified Rustdoc + Design Docs

**Idea**: Cross-link `cargo doc` API documentation with `docs/design/` HTML export

**Use Cases**:
- Click from `MdCodec` rustdoc → architecture.md design doc
- Click from design doc → specific API types in rustdoc
- Query "which types relate to this design concept?"

**Potential Approaches**:

1. **Manual linking (Phase 1)**: Add links in doc comments
   ```rust
   /// See [Architecture](https://buildonomy.github.io/noet-core/architecture.html)
   pub struct MdCodec { }
   ```

2. **RsCodec (Phase 2)**: Parse `.rs` files as documents
   - Create codec that extracts doc comments and API structure
   - Generate BeliefNodes for modules, structs, functions
   - Cross-link via BIDs in doc comments: `See {#bid:550e8400-...}`
   - Export unified HTML that links both directions

3. **Post-process rustdoc (Phase 3)**: 
   - Generate rustdoc HTML normally
   - Parse HTML to find BID references
   - Inject links to design docs
   - Update design doc export with rustdoc links

**Action**: Created separate issue (Issue 27) for this feature

**Benefits**:
- Unified documentation site (API + design)
- Navigate between "what" (rustdoc) and "why" (design docs)
- BeliefBase queries across code and docs
- Natural extension of codec system (Rust as another doc format)

---

**Delete this file after Issue 06 Phase 1 is complete**
