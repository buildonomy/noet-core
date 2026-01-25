# Issue 7: HTML Generation CLI Integration

**Priority**: HIGH - Complements HTML Rendering roadmap
**Estimated Effort**: 2-3 days
**Dependencies**: Issue 6 (HTML Generation basics), Issue 10 (CLI/Daemon)
**Target Version**: v0.2.0 (post-v0.1.0 soft open source)
**Context**: Integrates HTML generation with CLI and daemon for continuous site generation

## Summary

Integrate HTML generation from Issue 6 into the `noet` CLI tool and daemon from Issue 10. Add `--html` flag to both one-shot parsing (`noet parse`) and continuous watching (`noet watch`), enabling static site generation workflows. When enabled, the compiler runs parsed content through a configured markdown renderer and writes HTML output to a specified directory, maintaining the same directory structure as source documents.

**User Experience**: 
- `noet parse docs/ --html ./site` - one-shot: parse and generate HTML
- `noet watch docs/ --html ./site` - continuous: watch files, regenerate HTML on changes
- `noet daemon` (future) with HTML generation in background

**Post-Implementation**: noet becomes a complete static site generator with live reload capability, suitable for documentation sites, knowledge bases, and networked note systems.

## Goals

1. Add `--html <output_dir>` flag to `noet parse` subcommand
2. Add `--html <output_dir>` flag to `noet watch` subcommand
3. Integrate HTML generation into `FileUpdateSyncer` pipeline
4. Maintain source directory structure in HTML output
5. Support HTML generation configuration via `.noet/config.toml`
6. Generate index pages for directories (optional)
7. Copy static assets (CSS, JS, images) to output directory
8. Provide live reload server for development (optional, via `--serve` flag)

## Architecture

### CLI Integration

**Extended subcommands**:
```bash
# One-shot HTML generation
noet parse docs/ --html ./site
noet parse docs/ --html ./site --html-config custom.toml

# Continuous HTML generation
noet watch docs/ --html ./site
noet watch docs/ --html ./site --serve 8080  # with live reload

# Configuration via file
noet parse docs/ --config .noet/config.toml  # includes html settings
```

### Configuration Structure

**`.noet/config.toml`**:
```toml
[html]
enabled = true
output_dir = "./site"
metadata_mode = "collapsible"  # hidden, collapsible, visible, data-attributes
include_bid_attributes = true
css_class_prefix = "noet-"
inject_viewer_script = true

[html.renderer]
# Markdown renderer configuration
engine = "pulldown-cmark"  # or "comrak", "markdown-it", etc.
syntax_highlighting = true
math_rendering = true

[html.assets]
# Static assets to copy
css = ["./assets/style.css", "./assets/theme.css"]
js = ["./assets/viewer.js"]
copy_directories = ["./assets/images"]

[html.index]
# Auto-generate index pages
enabled = true
sort_by = "title"  # title, date, filename
show_metadata = true
```

### HTML Generation Pipeline

```
File Change Event
      ↓
DocumentCompiler parses markdown
      ↓
Generate BeliefNode with BID
      ↓
[HTML GENERATION STAGE - NEW]
      ↓
DocCodec::generate_html()
      ↓
Apply template/theme
      ↓
Inject data-bid attributes
      ↓
Write to output_dir
      ↓
[If --serve] Trigger browser reload
```

### Integration Points

**In `DocumentCompiler`**:
```rust
pub struct DocumentCompiler {
    // ... existing fields
    html_generator: Option<Arc<HtmlGenerator>>,
}

impl DocumentCompiler {
    pub fn with_html_output(mut self, config: HtmlConfig) -> Self {
        self.html_generator = Some(Arc::new(HtmlGenerator::new(config)));
        self
    }
}
```

**In `FileUpdateSyncer`**:
```rust
// After successful parse
if let Some(html_gen) = &compiler.html_generator {
    let html = codec.generate_html(&content, &options)?;
    let output_path = html_gen.resolve_output_path(&source_path)?;
    html_gen.write_html(&output_path, &html).await?;
}
```

### Directory Structure Mapping

**Source**:
```
docs/
├── index.md
├── architecture/
│   ├── overview.md
│   └── design.md
└── tutorials/
    └── getting-started.md
```

**HTML Output**:
```
site/
├── index.html
├── architecture/
│   ├── overview.html
│   └── design.html
├── tutorials/
│   └── getting-started.html
└── _assets/
    ├── noet.css
    ├── viewer.js
    └── images/
```

## Implementation Steps

### 1. Create HTML Generator Module (0.5 days)

**New file**: `src/html/mod.rs`

**Implementation**:
- [ ] Create `HtmlGenerator` struct
- [ ] Implement path resolution (source → output)
- [ ] Implement directory structure preservation
- [ ] Handle relative link rewriting (`.md` → `.html`)
- [ ] Asset copying functionality
- [ ] Configuration loading from `.noet/config.toml`

**Submodules**:
- `src/html/generator.rs` - core generation logic
- `src/html/config.rs` - configuration structs
- `src/html/assets.rs` - asset management
- `src/html/templates.rs` - HTML templates

### 2. Extend CLI Arguments (0.5 days)

**Update `src/bin/noet.rs`**:

- [ ] Add `--html <output_dir>` flag to `parse` subcommand
- [ ] Add `--html <output_dir>` flag to `watch` subcommand
- [ ] Add `--html-config <path>` for custom HTML config
- [ ] Add `--serve <port>` flag for live reload server (optional)
- [ ] Add `--no-html` flag to disable when enabled in config
- [ ] Validate output directory is writable
- [ ] Show HTML generation progress/stats

**CLI help text**:
```
noet parse <path> [OPTIONS]
    --html <dir>         Generate HTML output to directory
    --html-config <cfg>  Use custom HTML configuration
    --serve <port>       Start live reload server (with --html)
```

### 3. Integrate with DocumentCompiler (0.5 days)

**Update `src/codec/compiler.rs`**:

- [ ] Add `html_generator` field to `DocumentCompiler`
- [ ] Add `with_html_output()` builder method
- [ ] Trigger HTML generation after successful parse
- [ ] Handle HTML generation errors gracefully (don't block parsing)
- [ ] Log HTML generation events
- [ ] Include HTML stats in `ParseResult`

**Modified `ParseResult`**:
```rust
pub struct ParseResult {
    pub path: PathBuf,
    pub rewritten_content: Option<String>,
    pub dependent_paths: Vec<PathBuf>,
    pub diagnostics: Vec<ParseDiagnostic>,
    pub html_output: Option<PathBuf>,  // NEW: path to generated HTML
}
```

### 4. Integrate with FileUpdateSyncer (0.5 days)

**Update `src/daemon.rs`**:

- [ ] Pass HTML config to `FileUpdateSyncer`
- [ ] Generate HTML after each successful parse
- [ ] Handle HTML generation in transaction thread
- [ ] Clean up stale HTML files when source deleted
- [ ] Track HTML output in events

**HTML-aware events**:
```rust
pub enum BeliefEvent {
    // ... existing variants
    HtmlGenerated {
        source_path: PathBuf,
        html_path: PathBuf,
    },
    HtmlError {
        source_path: PathBuf,
        error: String,
    },
}
```

### 5. Implement Link Rewriting (0.5 days)

**Objective**: Convert markdown links to HTML links

**Implementation**:
- [ ] Parse links in HTML output
- [ ] Rewrite `.md` → `.html`
- [ ] Preserve anchors: `doc.md#section` → `doc.html#section`
- [ ] Handle relative paths correctly
- [ ] Handle external links (no rewriting)
- [ ] Handle NodeKey links (resolve to HTML paths)

**Example**:
- Source: `[Guide](./tutorials/guide.md#intro)`
- HTML: `<a href="./tutorials/guide.html#intro">Guide</a>`

### 6. Asset Management (0.5 days)

**Implementation**:
- [ ] Copy configured CSS files to `_assets/`
- [ ] Copy configured JS files to `_assets/`
- [ ] Copy image directories
- [ ] Inject asset links into generated HTML
- [ ] Watch asset files for changes (in watch mode)
- [ ] Cache asset hashes to avoid redundant copies

**Default assets** (if not provided):
- [ ] Bundle default `noet.css` (embedded in binary)
- [ ] Bundle default `viewer.js` (embedded in binary)
- [ ] Write default assets to `_assets/` if none configured

### 7. Live Reload Server (Optional, 0.5 days)

**Implementation** (if `--serve` flag present):
- [ ] Start HTTP server on specified port using `axum` or `tiny_http`
- [ ] Serve files from HTML output directory
- [ ] WebSocket connection for live reload
- [ ] Inject reload script into HTML pages
- [ ] Trigger reload on HTML generation
- [ ] Graceful shutdown on Ctrl-C

**Simple server**:
```rust
// Serve static files + WebSocket for reload
async fn serve_html(output_dir: PathBuf, port: u16) {
    let app = Router::new()
        .route("/", get(serve_file))
        .route("/*path", get(serve_file))
        .route("/ws", get(websocket_handler));
    
    axum::Server::bind(&([127, 0, 0, 1], port).into())
        .serve(app.into_make_service())
        .await;
}
```

### 8. Testing and Documentation (0.5 days)

**Testing**:
- [ ] Test one-shot HTML generation (`noet parse --html`)
- [ ] Test continuous generation (`noet watch --html`)
- [ ] Test directory structure preservation
- [ ] Test link rewriting (relative, absolute, anchors)
- [ ] Test asset copying
- [ ] Test with various HTML configs
- [ ] Test error handling (unwritable directory, etc.)

**Documentation**:
- [ ] Add HTML generation section to `docs/tutorials/`
- [ ] Document configuration options
- [ ] Provide example `.noet/config.toml`
- [ ] Document `--html` flag in CLI help
- [ ] Add HTML workflow examples to README

## Testing Requirements

### Unit Tests
- Path resolution (source → HTML output)
- Link rewriting (`.md` → `.html`)
- Asset path resolution
- Configuration parsing

### Integration Tests
- End-to-end: parse → generate HTML → verify output
- Watch mode: file change → HTML regeneration
- Asset copying and injection
- Multiple documents with cross-references

### Manual Testing
- Generate HTML for example document set
- Verify HTML renders correctly in browser
- Test links navigate correctly
- Test live reload (if implemented)
- Test with custom CSS/JS assets

### Performance Testing
- HTML generation adds < 20% overhead to parse time
- Large document sets (100+ files) complete in reasonable time
- Watch mode responds to changes within 1 second

## Success Criteria

- [ ] `noet parse --html` generates HTML output
- [ ] `noet watch --html` continuously generates HTML
- [ ] Directory structure preserved in output
- [ ] Links rewritten correctly (`.md` → `.html`)
- [ ] Assets copied to output directory
- [ ] Configuration via `.noet/config.toml` works
- [ ] HTML generation errors don't block parsing
- [ ] Documentation complete with examples
- [ ] Live reload server working (if implemented)
- [ ] All tests passing

## Risks

**Risk**: HTML generation slows down parsing significantly  
**Mitigation**: Run HTML generation in separate thread; make it optional; profile and optimize hot paths

**Risk**: Link rewriting breaks complex relative paths  
**Mitigation**: Extensive testing with various link patterns; validate against source directory structure

**Risk**: Asset management becomes complex with nested directories  
**Mitigation**: Keep asset copying simple; document limitations; provide escape hatches

**Risk**: Live reload server adds too much complexity  
**Mitigation**: Make it optional (`--serve` flag); use simple HTTP server library; defer to v0.3.0 if needed

**Risk**: Configuration options too numerous/confusing  
**Mitigation**: Provide sensible defaults; comprehensive examples; progressive disclosure in docs

## Open Questions

1. **Should HTML generation be synchronous or async relative to parsing?**
   - **Recommendation**: Async (separate thread) to avoid blocking parse pipeline
   - Parse errors should always be reported immediately
   - HTML generation errors are warnings, not failures

2. **What markdown renderer should we use?**
   - **Options**: pulldown-cmark (pure Rust), comrak (CommonMark), custom
   - **Recommendation**: pulldown-cmark (already used in core parsing)
   - Allow configuration for advanced users

3. **Should we generate directory index pages automatically?**
   - **Recommendation**: Yes, with opt-out via config
   - Show list of documents in directory
   - Include metadata (title, BID, schema)

4. **How to handle HTML templates/themes?**
   - **Phase 1** (this issue): Minimal template (header, content, footer)
   - **Phase 2** (future): Theme system with custom templates
   - **Recommendation**: Start simple, iterate based on feedback

5. **Should live reload be in Issue 7 or deferred?**
   - **Recommendation**: Include basic implementation if time permits
   - Use simple WebSocket-based reload
   - Defer advanced features (selective reload, etc.) to future

## Future Enhancements (Post-v0.2.0)

**Theme System**:
- Pluggable HTML templates
- CSS theme gallery
- Custom layouts per schema type

**Advanced Features**:
- Search index generation (JSON)
- RSS feed generation
- Sitemap generation
- Dynamic content blocks (TOC, backlinks, graph viz)

**Performance**:
- Parallel HTML generation
- Incremental regeneration (only changed files)
- Asset fingerprinting and caching

**Developer Experience**:
- Hot module reload (HMR) for assets
- Browser sync across multiple devices
- Source maps for debugging

## Decision Log

**Decision 1: HTML generation optional, non-blocking**
- Date: [To be filled]
- Rationale: Parsing is core feature, HTML is enhancement
- HTML errors should be warnings, not failures

**Decision 2: Preserve source directory structure**
- Date: [To be filled]
- Rationale: Intuitive, maintains relative link semantics
- Alternative: flat structure (rejected - breaks relative paths)

**Decision 3: Bundle default assets**
- Date: [To be filled]
- Rationale: Zero-config HTML generation for basic use cases
- Users can override with custom CSS/JS

**Decision 4: Link rewriting happens at HTML generation time**
- Date: [To be filled]
- Rationale: Keep markdown source clean, rewrite only in output
- Markdown links remain `.md` for maximum compatibility

## References

- **Depends On**: 
  - [`ISSUE_06_HTML_GENERATION.md`](./ISSUE_06_HTML_GENERATION.md) - HTML generation basics
  - [`ISSUE_10_DAEMON_TESTING.md`](./ISSUE_10_DAEMON_TESTING.md) - CLI and daemon infrastructure
- **Roadmap**: [`ROADMAP_HTML_RENDERING.md`](./ROADMAP_HTML_RENDERING.md) - overall HTML strategy
- **Integration**: Bridges Issues 1-4 (clean markdown) with Issue 6 (HTML generation)
- **Static Site Generators** (inspiration):
  - mdBook: https://github.com/rust-lang/mdBook
  - Zola: https://github.com/getzola/zola
  - Cobalt: https://github.com/cobalt-org/cobalt.rs
- **Code Changes**:
  - `src/html/` - new module for HTML generation
  - `src/bin/noet.rs` - add `--html` flag
  - `src/codec/compiler.rs` - integrate HTML generator
  - `src/daemon.rs` - HTML generation in FileUpdateSyncer
- **Configuration**: `.noet/config.toml` - HTML generation settings
- **Dependencies** (new):
  - `pulldown-cmark` (already have) - markdown to HTML
  - `axum` or `tiny_http` (optional) - live reload server
  - `tokio-tungstenite` (optional) - WebSocket for reload
