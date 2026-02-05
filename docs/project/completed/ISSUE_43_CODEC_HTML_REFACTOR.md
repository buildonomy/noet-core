# Issue 43: Refactor DocCodec HTML Generation - Factory Pattern and Dual-Phase Output

**Priority**: HIGH  
**Estimated Effort**: 3-4 days  
**Dependencies**: Issue 40 (complete)  
**Blocks**: Future codec implementations, report generation

**Status**: ✅ **COMPLETE** - All Phases Complete

## Progress

### ✅ Phase 1 Complete: Factory Pattern (2 hours actual)
**Completed**: Replaced singleton pattern with factory-based codec creation

**Changes:**
- Replaced `CODECS: Lazy<CodecMap>` singleton with factory function map
- Type: `fn() -> Box<dyn DocCodec + Send>` (function pointers are Copy + Send + Sync)
- Updated `parse_content()` to return `ParseContentWithCodec { result, codec }`
- Updated compiler `parse_next()` to destructure and use owned codec instance
- Fixed all breaking changes (compiler, tests, examples)

**Files Modified:**
- `src/codec/mod.rs` - CodecFactory type, CodecMap with factory functions
- `src/codec/builder.rs` - ParseContentWithCodec struct, parse_content returns owned instance
- `src/codec/compiler.rs` - parse_next destructures, generate_html_for_path uses factory
- `src/codec/belief_ir.rs` - Updated trait impl
- `src/codec/md.rs` - Updated trait impl

**Result**: ✅ No stale codec state, thread-safe by design, 152/152 tests pass

### ✅ Phase 2 Complete: Dual-Phase API (3 hours actual)
**Completed**: Simplified generate_html signature, added deferral signal

**Changes:**
- Added `should_defer()` method to DocCodec trait (default: false)
- Changed `generate_html()` signature:
  - **Before**: `fn generate_html(&self, script: Option<&str>, use_cdn: bool, ctx: Option<&BeliefContext>) -> Result<Option<String>, BuildonomyError>`
  - **After**: `fn generate_html(&self) -> Result<Vec<(PathBuf, String)>, BuildonomyError>`
- Returns Vec of (repo-relative-path, html-body-content) tuples
- Removed all parameters (script, use_cdn, ctx) - presentation moved to compiler

**Implementations:**
- `MdCodec`: Returns body HTML from markdown AST, link rewriting for resolved refs
- `ProtoBeliefNode`: `should_defer()` returns true for networks, stubbed for Phase 4

**Compiler Integration:**
- `parse_next()` calls `codec.generate_html()` immediately after parsing
- Queues path for deferred if `codec.should_defer()` returns true
- `write_fragment()` helper writes to `pages/` subdirectory

**Result**: ✅ Clean API, codecs return body fragments only, 152/152 tests pass

### ✅ Phase 3 Complete: Fragment Wrapping (2.5 hours actual)
**Completed**: Self-contained HTML fragments with Layout::Simple template

**Template Structure (template-simple.html):**
```html
<!doctype html>
<html lang="en">
    <head>
        <title>{{TITLE}}</title>
        <link rel="canonical" href="{{CANONICAL}}" />
        {{SCRIPT}}
    </head>
    <body>
        <h1 class="document-title">{{TITLE}}</h1>
        {{BODY}}
    </body>
</html>
```

**Compiler Changes:**
- `write_fragment(html_dir, rel_path, html_body, title)` wraps body with template
- Title extracted from:
  - Immediate: `codec.nodes()[0].document["title"]` (proto metadata)
  - Deferred: `ctx.node.display_title()` (BeliefContext node)
- Canonical URL: `/{rel_path}` (public URL, not internal `/pages/` path)
- Optional script injection maintained

**Output Structure:**
```
html_output/
  pages/
    docs/guide.html  ← Layout::Simple wrapped fragment
    index.html       ← Network index (future)
```

**Result**: ✅ Self-contained HTML fragments, SEO-friendly, 152/152 tests pass

### ✅ Phase 4 Complete: Deferred Generation (3 hours actual)
**Completed**: Context-aware HTML generation for network indices

**Changes:**
- Added `generate_deferred_html(&self, ctx: &BeliefContext)` to DocCodec trait (default: empty vec)
- Updated `generate_html_for_path()` to call `generate_deferred_html(ctx)` instead of `generate_html()`
- Deferred generation receives full BeliefContext with graph relationships

**ProtoBeliefNode Implementation:**
- Queries `ctx.sources()` for incoming edges with `WeightKind::Section` (subsection relationships)
- Sorts children by `WEIGHT_SORT_KEY` for deterministic ordering
- Generates HTML list with:
  - Network title and description from document metadata
  - `<ul>` of child documents with links (normalized to `.html` extension)
  - Displays `display_title()` for each child
  - Empty state message when network has no children
- Output path: `{network_path}.html` (e.g., `docs/index.html` for network at `docs/`)

**Files Modified:**
- `src/codec/mod.rs` - Added `generate_deferred_html()` trait method with documentation
- `src/codec/compiler.rs` - Updated `generate_html_for_path()` to use deferred method
- `src/codec/belief_ir.rs` - Implemented network index generation in ProtoBeliefNode

**Result**: ✅ Networks generate index pages listing child documents, 152/152 tests pass

### ✅ Phase 5 Complete: SPA Shell + Sitemap (2.5 hours actual)
**Completed**: Root index.html with Responsive template and sitemap.xml for SEO

**Changes:**
- Added `generate_spa_shell()` method to DocumentCompiler
- Added `generate_sitemap()` method to DocumentCompiler
- Both called from `finalize()` after deferred HTML generation completes
- Uses `Layout::Responsive` template for full SPA interface
- Extracts repo root node from belief base using `builder.repo()`
- Serializes repo node as JSON metadata for viewer.js consumption

**SPA Shell Implementation:**
- Template placeholders replaced:
  - `{{CONTENT}}`: Loading placeholder div for dynamic content
  - `{{TITLE}}`: Repo root node display title
  - `{{METADATA}}`: Serialized BeliefNode JSON for navigation/metadata panels
  - `{{SCRIPT}}`: Empty (viewer.js loaded via template)
- Output: `html_output/index.html` at root (not in pages/ subdirectory)
- Stylesheet placeholders replaced using `get_stylesheet_urls(use_cdn)`
- Script injection via `html_script` field
- Gracefully skips if no HTML output configured

**Sitemap Implementation:**
- Queries `global_bb.get_network_paths(builder.repo())` for all documents
- Converts repo-relative paths to HTML paths (replaces codec extensions with `.html`)
- Generates standard XML sitemap format
- Public URLs use `/path.html` format (not internal `/pages/` structure)
- Output: `html_output/sitemap.xml` at root

**Files Modified:**
- `src/codec/compiler.rs` - Added `generate_spa_shell()`, `generate_sitemap()`, integrated into finalize flow

**Output Structure:**
```
html_output/
  index.html              ← SPA shell (Responsive template, repo metadata)
  sitemap.xml             ← SEO sitemap with all document URLs
  beliefbase.json         ← Graph data
  assets/                 ← Static assets (hardlinked)
  pages/
    docs/guide.html       ← Fragment (Simple template)
    docs/tutorial.html
    network1/index.html   ← Network index (deferred generation)
```

**Result**: ✅ Complete SPA architecture with SEO support, ready for viewer.js integration

**Note**: Sitemap includes all network paths including subnets via recursive `get_network_paths()` implementation in db.rs (with cycle detection and path prefixing).

**Browser Test Fix**: Fixed `beliefbase.json` export in parse command by restructuring event loop management - parse command now spawns background task to process events into `BeliefBase`, waits for all events to drain after parsing completes, then exports from synchronized state.

### ✅ Phase 6 Complete: Architecture Ready (0 hours)
**Status**: Core refactor complete, cleanup deferred to separate issue

**Completed:**
- Factory pattern eliminates stale codec state ✅
- Dual-phase API (immediate + deferred with context) ✅
- Fragment wrapping with Layout::Simple ✅
- Network index generation with context queries ✅
- SPA shell with repo metadata ✅
- Sitemap generation for SEO ✅

**Additional Fixes:**
- Event loop synchronization in parse command (Option G implementation)
- Parse command now properly exports beliefbase.json with all nodes
- Browser tests fixed (57 nodes exported correctly)

**Deferred to Future Issues:**
- Remove unused `petgraph::data::Build` import

## Summary of Changes

This refactor successfully transformed the DocCodec HTML generation system from a fragile singleton pattern to a robust factory-based architecture with dual-phase generation:

**Key Achievements:**
1. **Factory Pattern**: Eliminated stale codec state via `fn() -> Box<dyn DocCodec>` factories
2. **Dual-Phase API**: Separate immediate (`generate_html()`) and deferred (`generate_deferred_html(ctx)`) generation
3. **Context-Aware Generation**: Network indices can query graph relationships via BeliefContext
4. **SPA Architecture**: Complete output structure with shell, fragments, and sitemap
5. **SEO Support**: Self-contained fragments with canonical URLs and XML sitemap

**Impact:**
- Thread-safe codec creation (no more singleton races)
- Codecs return simple body fragments (presentation in compiler)
- Networks generate index pages listing child documents
- Complete SPA shell ready for viewer.js integration
- Stylesheet URLs properly configured (CDN or vendored via `use_cdn` field)
- Event synchronization ensures beliefbase.json export correctness
- All 112 codec tests passing, browser tests working

## Original Summary

Replace codec singleton pattern with factory pattern where `parse_content()` returns owned codec instances. Enable dual-phase HTML generation: immediate (uses parsed state) and deferred (uses BeliefContext). Separate content generation (codecs) from presentation wrapping (compiler). Generate SPA architecture with shell + fragments + sitemap.

## Core Problems

### Problem 1: Singleton Codec Pattern is Fragile

**Current Flow**:
```
1. Compiler calls builder.parse_content(path, content)
2. Builder gets codec from CODECS singleton map
3. Codec parses content, updates internal state
4. Builder returns parse results
5. Later: Compiler grabs same codec singleton
6. Compiler calls codec.generate_html() 
7. Hope & pray codec still has right content loaded! ❌
```

**Issues**:
- ❌ **No Guarantees**: Codec might have been reused for different file
- ❌ **Race Conditions**: Parallel parsing could clobber state
- ❌ **Implicit Contract**: Relies on compiler calling generate_html() "soon enough"
- ❌ **Hard to Reason About**: Temporal coupling between parse and generate

### Problem 2: Codecs Handle Presentation

Codecs currently handle:
- Template selection (Simple vs Responsive)
- CDN strategy (local vs CDN assets)
- Script injection (live reload, etc.)
- Complete HTML page generation

**Issues**:
- ❌ **Coupling**: Content generation tied to presentation
- ❌ **Duplication**: Every codec reimplements templating
- ❌ **Inconsistency**: Different codecs can produce different page structures

### Problem 3: Compiler-Driven Deferral

Compiler hardcodes which file types need deferral:
```rust
if self.is_network_file(&file_path) {
    self.deferred_html.insert(file_path);
}
```

**Issues**:
- ❌ **Not Extensible**: Future codecs can't opt in
- ❌ **Compiler Knowledge**: Compiler must know codec internals

## Desired Architecture

### Factory Pattern: Parse Returns Owned Instance

```
builder.parse_content(path, content) → Returns owned codec instance
    ↓
Codec instance has parsed state (content, metadata, etc.)
    ↓
compiler.codec.generate_html() → Uses instance's parsed state
    ↓
Instance discarded after use (no stale state risk)
```

### Dual-Phase Generation

**Key Innovation**: Codecs can output in BOTH phases:

```
Phase 1 (Immediate):
  codec.generate_html() → Vec<(PathBuf, html_body)>
  Write all fragments to pages/
  
If codec.should_defer():
  Add to deferred queue
  
Phase 2 (Deferred):
  codec.generate_deferred_html(ctx) → Vec<(PathBuf, html_body)>
  Write all fragments to pages/ (overwrite or add new)
```

**Use Cases**:
- **Markdown**: Immediate-only (render from parsed AST)
- **Network Index**: Deferred-only (needs child document context)
- **Cross-Referenced Docs**: Both! (basic render immediate, enriched version deferred)

### URL Routing Scheme

**Codec returns public URL path** (repo-relative):
```rust
(PathBuf::from("docs/guide.html"), html_body)
```

**Compiler writes to pages/ subdirectory**:
```
html_output/pages/docs/guide.html
```

**Public URL** (user-facing, in sitemap):
```
/docs/guide.html
```

**Server routing**:
```
/ → index.html (SPA shell)
/docs/guide.html → pages/docs/guide.html
/assets/* → assets/*
/beliefbase.json → beliefbase.json
```

**SPA client routing**:
```javascript
// URL: /docs/guide.html
// SPA fetches: /pages/docs/guide.html
// Injects body into shell
```

### Separation: Content vs Presentation

**Codecs generate fragments**:
```rust
codec.generate_html() → Vec<(PathBuf, html_body_content)>
```

**Compiler wraps with Layout::Simple**:
```html
<!DOCTYPE html>
<html>
<head>
  <meta charset="utf-8">
  <link rel="canonical" href="/docs/guide.html">
  {{OPTIONAL_SCRIPT}}
</head>
<body>
{{BODY}}
</body>
</html>
```

**Compiler creates SPA shell with Layout::Responsive**:
```html
<!DOCTYPE html>
<html>
<head>
  <script type="application/json" id="repo-metadata">{repo_node}</script>
  <script src="/assets/viewer.js"></script>
</head>
<body>
  <div id="content-root"></div>
</body>
</html>
```

## Proposed API

### DocCodec Trait Changes

```rust
pub trait DocCodec: Send + Sync {
    fn nodes(&self) -> Vec<ProtoBeliefNode>;
    
    fn inject_context(
        &mut self,
        node: &ProtoBeliefNode,
        ctx: &BeliefContext<'_>,
    ) -> Result<Option<BeliefNode>, BuildonomyError>;
    
    /// Signal whether this codec needs deferred generation.
    /// 
    /// If true, compiler will call generate_deferred_html() after all parsing completes.
    /// Codec can still output from generate_html() - both phases can produce output.
    fn should_defer(&self) -> bool {
        false // Default: no deferral needed
    }
    
    /// Generate HTML body content using parsed state.
    /// 
    /// Called immediately after parsing while instance has parsed content.
    /// Can generate partial output even if should_defer() returns true.
    /// 
    /// Returns:
    /// - `Ok(vec![(path, body), ...])`: Output paths and HTML bodies (not full pages)
    /// - `Ok(vec![])`: No immediate output
    /// - `Err(_)`: Generation failed
    fn generate_html(&self) -> Result<Vec<(PathBuf, String)>, BuildonomyError> {
        Ok(vec![]) // Default: no immediate HTML
    }
    
    /// Generate HTML body content using BeliefContext.
    /// 
    /// Only called if should_defer() returns true.
    /// Instance method but conceptually stateless - should only use ctx parameter.
    /// 
    /// Returns:
    /// - `Ok(vec![(path, body), ...])`: Output paths and HTML bodies
    /// - `Ok(vec![])`: No deferred output
    /// - `Err(_)`: Generation failed
    fn generate_deferred_html(
        &self,
        ctx: &BeliefContext<'_>,
    ) -> Result<Vec<(PathBuf, String)>, BuildonomyError> {
        let _ = ctx;
        Ok(vec![]) // Default: no deferred generation
    }
}
```

### Builder Changes

```rust
impl GraphBuilder {
    pub async fn parse_content(
        &mut self,
        path: &Path,
        content: String,
        global_bb: B,
    ) -> Result<(ParseResult, Box<dyn DocCodec>), BuildonomyError> {
        // Get codec factory function
        let codec_factory = CODECS.get(ext)?;
        
        // Create NEW instance (not singleton)
        let mut codec = codec_factory();
        
        // Parse with this instance
        let parse_result = codec.parse(path, content)?;
        
        // Return instance to compiler
        Ok((parse_result, codec))
    }
}
```

### Compiler Changes

```rust
impl DocumentCompiler {
    async fn parse_next(&mut self) -> Result<Option<ParseResult>, BuildonomyError> {
        // ... parsing logic ...
        
        // Get owned codec instance from builder
        let (parse_result, codec) = self.builder
            .parse_content(&path, content, global_bb)
            .await?;
        
        // Phase 1: Try immediate HTML generation
        if let Some(html_dir) = &self.html_output_dir {
            let fragments = codec.generate_html()?;
            for (rel_path, html_body) in fragments {
                self.write_fragment(&rel_path, html_body).await?;
                self.generated_fragments.insert(rel_path);
            }
            
            // Queue for deferred generation if codec requests it
            if codec.should_defer() {
                self.deferred_html.insert(path.clone());
            }
        }
        
        // codec instance dropped here - no stale state
        Ok(Some(parse_result))
    }
    
    async fn write_fragment(&self, rel_path: &Path, html_body: String) -> Result<(), BuildonomyError> {
        let pages_dir = self.html_output_dir.join("pages");
        let output_path = pages_dir.join(rel_path);
        
        // Ensure parent directories exist
        if let Some(parent) = output_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        
        // Wrap body with Layout::Simple template
        let html = self.wrap_fragment(html_body, rel_path)?;
        tokio::fs::write(&output_path, html).await?;
        
        Ok(())
    }
    
    fn wrap_fragment(&self, body: String, rel_path: &Path) -> Result<String, BuildonomyError> {
        let template = get_template(Layout::Simple);
        
        // Generate canonical URL from relative path
        let canonical_url = format!("/{}", rel_path.display());
        
        let html = template
            .replace("{{BODY}}", &body)
            .replace("{{CANONICAL}}", &canonical_url);
        
        // Inject optional script if configured
        let html = if let Some(script) = &self.html_script {
            html.replace("{{SCRIPT}}", &format!("<script>{}</script>", script))
        } else {
            html.replace("{{SCRIPT}}", "")
        };
        
        Ok(html)
    }
    
    async fn generate_deferred_html(&mut self) -> Result<(), BuildonomyError> {
        for path in &self.deferred_html.clone() {
            // Get extension to look up codec factory
            let Some(ext) = path.extension().and_then(|s| s.to_str()) else {
                tracing::warn!("No extension for deferred path: {}", path.display());
                continue;
            };
            
            let Some(codec_factory) = CODECS.get(ext) else {
                tracing::warn!("No codec for extension '{}': {}", ext, path.display());
                continue;
            };
            
            // Create fresh codec instance
            let codec = codec_factory();
            
            // Get BeliefContext for this path using existing lookup
            let path_str = path.to_string_lossy().to_string();
            let paths = self.builder.paths();
            let Some((_net_bid, node_bid)) = paths.get(&path_str) else {
                tracing::warn!("No BID found for deferred path: {}", path_str);
                continue;
            };
            drop(paths);
            
            let Some(ctx) = self.builder.session_bb_mut().get_context(&node_bid) else {
                tracing::warn!("No context for BID {:?}", node_bid);
                continue;
            };
            
            // Phase 2: Generate using context
            match codec.generate_deferred_html(&ctx) {
                Ok(fragments) => {
                    for (rel_path, html_body) in fragments {
                        self.write_fragment(&rel_path, html_body).await?;
                        self.generated_fragments.insert(rel_path);
                    }
                }
                Err(e) => {
                    // Skip and warn on error
                    tracing::warn!("Deferred generation failed for {}: {}", path.display(), e);
                    continue;
                }
            }
        }
        Ok(())
    }
    
    async fn generate_spa_shell(&self) -> Result<(), BuildonomyError> {
        // Get repository root network node for metadata
        let Some(repo_root_node) = self.builder.session_bb().states().get(self.builder.repo) else {
            return Err(BuildonomyError::Codec("never set our repo root network!"));
        };
        
        // Generate SPA shell with responsive template
        let template = get_template(Layout::Responsive);
        
        // Serialize repo root node as metadata for SPA
        let metadata = serde_json::to_string(repo_root_node)?;
        let metadata_script = format!(
            r#"<script type="application/json" id="repo-metadata">{}</script>"#,
            metadata
        );
        
        let html = template
            .replace("{{CONTENT}}", r#"<div id="content-root"></div>"#)
            .replace("{{TITLE}}", repo_root_node.display_title())
            .replace("{{METADATA}}", &metadata_script);
        
        // Apply CDN strategy if configured
        let html = if self.use_cdn {
            self.apply_cdn_urls(html)
        } else {
            html
        };
        
        // Inject SPA script (loads fragments dynamically)
        let html = html.replace("</body>", r#"<script src="/assets/viewer.js"></script></body>"#);
        
        let index_path = self.html_output_dir.join("index.html");
        tokio::fs::write(index_path, html).await?;
        
        // Generate sitemap.xml
        self.generate_sitemap().await?;
        
        Ok(())
    }
    
    async fn generate_sitemap(&self) -> Result<(), BuildonomyError> {
        let mut sitemap = String::from(r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
"#);
        
        // Add all generated fragment public URLs
        let mut paths: Vec<_> = self.generated_fragments.iter().collect();
        paths.sort();
        
        for fragment_path in paths {
            let public_url = format!("/{}", fragment_path.display());
            sitemap.push_str(&format!("  <url><loc>{}</loc></url>\n", public_url));
        }
        
        sitemap.push_str("</urlset>");
        
        let sitemap_path = self.html_output_dir.join("sitemap.xml");
        tokio::fs::write(sitemap_path, sitemap).await?;
        
        Ok(())
    }
}
```

### CODECS Map Changes

```rust
// Before: Singleton instances (stateful)
lazy_static! {
    static ref CODECS: HashMap<&'static str, Arc<Mutex<Box<dyn DocCodec>>>> = {
        let mut map = HashMap::new();
        map.insert("md", Arc::new(Mutex::new(Box::new(MdCodec::default()))));
        // ...
    };
}

// After: Factory functions (stateless)
lazy_static! {
    static ref CODECS: HashMap<&'static str, CodecFactory> = {
        let mut map = HashMap::new();
        map.insert("md", || Box::new(MdCodec::default()));
        map.insert("toml", || Box::new(ProtoBeliefNode::default()));
        // ...
        map
    };
}

type CodecFactory = fn() -> Box<dyn DocCodec>;
```

### Template Changes

**Layout::Simple** (for fragments):
```html
<!DOCTYPE html>
<html>
<head>
  <meta charset="utf-8">
  <link rel="canonical" href="{{CANONICAL}}">
  {{SCRIPT}}
</head>
<body>
{{BODY}}
</body>
</html>
```

**Layout::Responsive** (for SPA shell):
```html
<!DOCTYPE html>
<html>
<head>
  <meta charset="utf-8">
  <title>{{TITLE}}</title>
  {{METADATA}}
  <link rel="stylesheet" href="/assets/style.css">
</head>
<body>
  {{CONTENT}}
</body>
</html>
```

## Key Design Principles

### 1. Dual-Phase Generation Enables Progressive Enhancement

**Immediate Phase**:
- Uses codec's parsed state
- Generates basic version quickly
- Example: Markdown renders from AST

**Deferred Phase** (if `should_defer() == true`):
- Uses BeliefContext (full graph)
- Generates enriched version
- Example: Adds backlinks, resolves cross-refs, builds indices

**Both phases can output** - immediate version gets overwritten by enriched version.

### 2. Codec Decides Deferral

Codec returns `true` from `should_defer()` to signal: "I need full context, call me again after parsing completes."

Compiler doesn't need to know which codecs need deferral.

### 3. Ownership Makes Guarantees

Compiler gets owned codec instance → guaranteed fresh state.
No temporal coupling, no stale state risk.

### 4. Fragments are Standalone Documents

Each fragment in `pages/` is valid HTML with:
- Canonical URL (for SEO)
- Optional script injection (for future use)
- No presentation logic (codecs return body only)

Works without JavaScript (progressive enhancement).

### 5. Single Source of Truth for Presentation

Only compiler applies templates and wrapping:
- Layout::Simple for all fragments (consistent structure)
- Layout::Responsive for SPA shell (consistent presentation)

Codecs focus on content generation only.

## Implementation Plan

### Phase 1: Add Factory Pattern (2 hours)

1. Add `CodecFactory` type alias
2. Create new `CODECS` map with factory functions
3. Update `parse_content()` to return codec instance
4. Update compiler to receive owned instance
5. Test that parsing still works

**Result**: Factory pattern infrastructure in place

### Phase 2: Add Dual-Phase API (3 hours)

1. Add `should_defer()` method to trait (default false)
2. Add `generate_html()` returning `Vec<(PathBuf, String)>`
3. Add `generate_deferred_html(&self, ctx)` to trait
4. Update `parse_next()` to call both methods appropriately
5. Test immediate generation with MdCodec

**Result**: Dual-phase API defined, immediate generation works

### Phase 3: Implement Fragment Wrapping (2 hours)

1. Update Layout::Simple template (canonical + optional script)
2. Implement `wrap_fragment()` method
3. Update `write_fragment()` to use Layout::Simple
4. Test fragments are valid standalone HTML

**Result**: All fragments wrapped consistently

### Phase 4: Implement Deferred Generation (4 hours)

1. Add `generated_fragments` tracking to compiler
2. Implement `generate_deferred_html()` with context lookup
3. Update ProtoBeliefNode to use deferred generation for networks
4. Add error handling (skip and warn)
5. Test network indices generate correctly

**Result**: Deferred generation works, networks produce indices

### Phase 5: SPA Shell + Sitemap (3 hours)

1. Update `generate_spa_shell()` to use repo root node metadata
2. Generate single `index.html` with responsive template
3. Implement `generate_sitemap()` with all fragment URLs
4. Add canonical URLs to fragments
5. Test complete output structure

**Output Structure**:
```
html_output/
  index.html              ← SPA shell (responsive template, repo metadata)
  sitemap.xml             ← All fragment public URLs
  beliefbase.json         ← Graph data
  assets/
    style.css
    viewer.js                ← Fragment loader (future)
  pages/
    docs/
      guide.html          ← Fragment (Simple template, canonical URL)
      tutorial.html
    network1/
      index.html          ← Network index fragment (deferred)
```

**Result**: Complete SPA architecture with SEO support

### Phase 6: Remove Old Code (2 hours)

1. Remove singleton CODECS patterns
2. Remove old template parameters (script, cdn, ctx)
3. Clean up unused code
4. Update documentation

**Result**: Clean, simple API

## Examples

### MdCodec (Immediate-Only Generation)

```rust
impl DocCodec for MdCodec {
    fn should_defer(&self) -> bool {
        false // Markdown doesn't need context
    }
    
    fn generate_html(&self) -> Result<Vec<(PathBuf, String)>, BuildonomyError> {
        // Use parsed AST to generate HTML body
        let body = self.render_markdown_to_html();
        
        // Return repo-relative public URL path
        let output_path = self.source_path.with_extension("html");
        Ok(vec![(output_path, body)])
    }
    
    // No deferred generation needed
}
```

### ProtoBeliefNode (Deferred-Only for Networks)

```rust
impl DocCodec for ProtoBeliefNode {
    fn should_defer(&self) -> bool {
        self.kind.contains(BeliefKind::Network)
    }
    
    fn generate_html(&self) -> Result<Vec<(PathBuf, String)>, BuildonomyError> {
        // No immediate output for networks
        Ok(vec![])
    }
    
    fn generate_deferred_html(
        &self,
        ctx: &BeliefContext<'_>,
    ) -> Result<Vec<(PathBuf, String)>, BuildonomyError> {
        if !ctx.node.kind.contains(BeliefKind::Network) {
            return Ok(vec![]);
        }
        
        // Build index using only context data
        let mut body = format!("<h1>{}</h1>\n", ctx.node.title);
        
        let children: Vec<_> = ctx.sources()
            .filter(|r| r.weight.contains(&WeightKind::Section))
            .filter(|r| r.other.kind.contains(BeliefKind::Document))
            .collect();
        
        body.push_str(&format!("<p>Total documents: {}</p>\n<ul>\n", children.len()));
        for child in children {
            body.push_str(&format!(
                "  <li><a href=\"/{}\">{}</a></li>\n",
                child.other.home_path.display(),
                child.other.title
            ));
        }
        body.push_str("</ul>\n");
        
        // Output to index.html in network's directory
        let output_path = ctx.node.home_path.join("index.html");
        Ok(vec![(output_path, body)])
    }
}
```

### Enhanced MdCodec (Dual-Phase with Cross-References)

```rust
impl DocCodec for MdCodec {
    fn should_defer(&self) -> bool {
        self.has_cross_references() // Defer if needs link resolution
    }
    
    fn generate_html(&self) -> Result<Vec<(PathBuf, String)>, BuildonomyError> {
        // Phase 1: Generate basic rendered markdown
        let body = self.render_markdown_to_html();
        let output_path = self.source_path.with_extension("html");
        Ok(vec![(output_path, body)])
    }
    
    fn generate_deferred_html(
        &self,
        ctx: &BeliefContext<'_>,
    ) -> Result<Vec<(PathBuf, String)>, BuildonomyError> {
        // Phase 2: Regenerate with resolved cross-references and backlinks
        let enriched_body = self.render_with_context(ctx);
        let output_path = self.source_path.with_extension("html");
        Ok(vec![(output_path, enriched_body)])
    }
}
```

## Benefits

### For Correctness
- ✅ **No Stale State**: Fresh instance per parse
- ✅ **No Race Conditions**: Each parse gets own instance
- ✅ **Explicit Contract**: Ownership makes guarantees clear
- ✅ **Thread Safe**: Function pointers are Copy + Send + Sync

### For Codecs
- ✅ **Simpler**: Just return body content (no templates)
- ✅ **Flexible**: Choose immediate, deferred, or both
- ✅ **Clear**: Deferred phase uses only context
- ✅ **Progressive**: Basic version available immediately

### For Compiler
- ✅ **Consistent**: All pages use same templates
- ✅ **Generic**: Any codec can defer
- ✅ **Maintainable**: Change template once, affects all output
- ✅ **SEO-Friendly**: Sitemap + canonical URLs

### For Users
- ✅ **Fast Initial Load**: SPA shell loads quickly
- ✅ **Progressive Enhancement**: Fragments work without JS
- ✅ **Crawlable**: Search engines index fragment pages
- ✅ **Modern UX**: SPA navigation when JS available

## Success Criteria

**Phase 1-3 (Complete):**
- [x] CODECS map stores factory functions (not singletons)
- [x] `parse_content()` returns owned codec instance via `ParseContentWithCodec` struct
- [x] `should_defer()` method signals deferral need
- [x] `generate_html()` returns `Vec<(PathBuf, String)>` with no parameters
- [x] Layout::Simple wraps all fragments (TITLE, CANONICAL, BODY, SCRIPT placeholders)
- [x] All fragments in `pages/` subdirectory
- [x] Canonical URLs in all fragments
- [x] MdCodec uses immediate generation
- [x] ProtoBeliefNode.should_defer() returns true for networks
- [x] No singleton state bugs
- [x] All 152 tests pass

**Phase 4-6 (Remaining):**
- [ ] `generate_deferred_html()` implemented in compiler
- [ ] ProtoBeliefNode generates network indices using BeliefContext
- [ ] Layout::Responsive used only for SPA shell
- [ ] Single `index.html` at root with repo metadata
- [ ] `sitemap.xml` generated with public URLs
- [ ] Dual-phase codec can output in both phases (if needed)
- [ ] Deferred generation errors logged and skipped
- [ ] Old API code removed

## Open Questions

None - design is complete and ready for implementation.

## Related Issues

- **ISSUE_40**: Network index generation (foundation)
- **ISSUE_41**: Stream BeliefEvents to SPA (benefits from consistent templates)
- **Future**: SPA JavaScript implementation (fragment loading, routing)
- **Future**: Server configuration guide (nginx/Apache routing for History API)
