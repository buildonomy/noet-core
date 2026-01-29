# Issue 19: Per-Network HTML Theming

**Priority**: LOW - Post-MVP enhancement (after Issue 06)
**Estimated Effort**: 2-3 days
**Dependencies**: Issue 06 (HTML Generation) must be complete

## Summary

Enable different BeliefNetworks to have distinct visual styling when exported to HTML, allowing multi-project documentation sites with clear visual differentiation. Theme configuration is stored in each network's `BeliefNetwork.yaml` payload, supporting network-relative CSS paths, absolute URLs, and CSS variable customization.

**Use Case**: A documentation site combining noet-core docs (blue theme), tutorial examples (green theme), and blog posts (minimal theme) - each network visually distinct while sharing site navigation.

## Goals

1. Support theme configuration in BeliefNetwork payload (YAML/JSON/TOML)
2. Resolve network-relative CSS paths and absolute URLs
3. Generate HTML with per-network CSS scoping via `data-network` attributes
4. Implement theme cascade: CLI override → network theme → entry theme → default
5. Support CSS variable customization for quick style tweaks
6. Copy network-specific theme assets during HTML export

## Architecture

### BeliefNetwork Theme Configuration

Store theme config in the BeliefNetwork file's payload:

```yaml
# BeliefNetwork.yaml
bid: "550e8400-e29b-41d4-a716-446655440000"
kind: "Document"
schema: "buildonomy.network"
title: "noet-core Documentation"

# Theme configuration in payload
theme:
  # Option 1: Built-in theme name
  name: "just-the-docs"
  
  # Option 2: Network-relative CSS path
  css: "./theme/custom.css"
  
  # Option 3: Absolute URL
  css: "https://cdn.example.com/theme.css"
  
  # Additional stylesheets (cascade order)
  additional_css:
    - "./overrides.css"
    - "https://fonts.googleapis.com/css2?family=Inter"
  
  # CSS variables for quick customization
  variables:
    primary-color: "#0066cc"
    heading-font: "Inter, sans-serif"
    border-color: "#e0e0e0"
```

### HTML Structure with Network Scoping

```html
<!DOCTYPE html>
<html>
<head>
  <title>Document Title</title>
  
  <!-- Site-wide base theme (from entry network) -->
  <link rel="stylesheet" href="/themes/site-base.css">
  
  <!-- Network-specific theme -->
  <link rel="stylesheet" href="/themes/network-noet-core.css">
  
  <!-- CSS variables for this network -->
  <style>
    [data-network="550e8400-e29b-41d4-a716-446655440000"] {
      --primary-color: #0066cc;
      --heading-font: Inter, sans-serif;
      --border-color: #e0e0e0;
    }
  </style>
</head>
<body>
  <!-- Site navigation uses entry network theme -->
  <nav class="site-nav">
    <a href="/">Home</a>
    <a href="/docs/">Docs</a>
  </nav>
  
  <!-- Document content uses network-specific theme -->
  <article class="noet-document" 
           data-network="550e8400-e29b-41d4-a716-446655440000"
           data-bid="...">
    <h1>Architecture</h1>
    <!-- Content styled with network theme -->
  </article>
</body>
</html>
```

### CSS Scoping Strategy

Network-specific styles use `data-network` attribute selectors:

```css
/* Site-wide base styles */
body {
  font-family: system-ui;
  line-height: 1.6;
}

/* Network-specific customization via CSS variables */
[data-network="550e8400-e29b-41d4-a716-446655440000"] {
  --primary-color: #0066cc;
  --border-color: #e0e0e0;
}

[data-network="abc12345-6789-0abc-def0-123456789abc"] {
  --primary-color: #00cc66;
  --border-color: #c0ffc0;
}

/* Generic styles use CSS variables */
.noet-document h1 {
  color: var(--primary-color);
  border-bottom: 2px solid var(--border-color);
}

.noet-document a {
  color: var(--primary-color);
}
```

### Theme Resolution Order

1. **CLI override**: `--css custom.css` (applies site-wide, highest priority)
2. **Network theme**: From BeliefNetwork payload in document's network
3. **Entry network theme**: Site-wide default from export entry point
4. **Built-in default**: Bundled theme (fallback)

## Implementation Steps

### 1. Add Theme Config to BeliefNode Payload Schema (0.5 days)

- [ ] Define `ThemeConfig` struct for deserialization
- [ ] Document expected payload structure in architecture.md
- [ ] Add validation for theme config during network parsing

**Data Structures**:
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThemeConfig {
    /// Built-in theme name
    pub name: Option<String>,
    
    /// CSS file path (network-relative) or URL
    pub css: Option<String>,
    
    /// Additional CSS files (cascade order)
    #[serde(default)]
    pub additional_css: Vec<String>,
    
    /// CSS variable overrides
    #[serde(default)]
    pub variables: HashMap<String, String>,
}

impl ThemeConfig {
    pub fn from_payload(payload: &toml::Table) -> Option<Self> {
        payload.get("theme")
            .and_then(|v| v.clone().try_into().ok())
    }
}
```

### 2. Extend HtmlGenerationOptions with Network Context (0.5 days)

- [ ] Add `network_id` field for CSS scoping
- [ ] Add `network_theme` field for per-network styling
- [ ] Add `site_theme` field for entry network base theme

```rust
pub struct HtmlGenerationOptions {
    // ... existing fields from Issue 06
    
    /// Network ID for CSS scoping (used in data-network attribute)
    pub network_id: Option<Uuid>,
    
    /// Theme config for this document's network
    pub network_theme: Option<ThemeConfig>,
    
    /// Entry network theme (site-wide base)
    pub site_theme: Option<ThemeConfig>,
}
```

### 3. Implement Theme Path Resolution (1 day)

- [ ] Distinguish URLs from filesystem paths
- [ ] Resolve network-relative paths (relative to BeliefNetwork file)
- [ ] Handle cross-network CSS references
- [ ] Validate CSS file existence during export

```rust
pub enum ThemeSource {
    /// Absolute URL (e.g., "https://cdn.example.com/theme.css")
    Url(String),
    
    /// Filesystem path (resolved relative to network root)
    File(PathBuf),
    
    /// Built-in theme name (e.g., "just-the-docs")
    BuiltIn(String),
}

impl ThemeSource {
    pub fn resolve(css_path: &str, network_root: &Path) -> Result<Self, BuildonomyError> {
        if css_path.starts_with("http://") || css_path.starts_with("https://") {
            Ok(ThemeSource::Url(css_path.to_string()))
        } else if css_path.starts_with("./") || css_path.starts_with("../") {
            let resolved = network_root.join(css_path).canonicalize()?;
            Ok(ThemeSource::File(resolved))
        } else {
            // Assume built-in theme name
            Ok(ThemeSource::BuiltIn(css_path.to_string()))
        }
    }
}
```

### 4. Update HTML Generation with Network Scoping (1 day)

- [ ] Add `data-network` attribute to document wrapper
- [ ] Generate CSS variable declarations per network
- [ ] Include network-specific stylesheets in `<head>`
- [ ] Maintain backwards compatibility (single-theme mode)

```rust
impl MdCodec {
    fn generate_html(&self, options: &HtmlGenerationOptions) -> Result<String> {
        let network_id_str = options.network_id
            .map(|id| id.to_string())
            .unwrap_or_default();
        
        let css_vars = options.network_theme
            .as_ref()
            .map(|theme| self.generate_css_variables(&theme.variables))
            .unwrap_or_default();
        
        html! {
            (DOCTYPE)
            html {
                head {
                    // Site-wide base theme
                    @if let Some(site_theme) = &options.site_theme {
                        link rel="stylesheet" href=(self.theme_path(site_theme));
                    }
                    
                    // Network-specific theme
                    @if let Some(net_theme) = &options.network_theme {
                        link rel="stylesheet" href=(self.theme_path(net_theme));
                    }
                    
                    // CSS variables
                    @if !css_vars.is_empty() {
                        style { (PreEscaped(css_vars)) }
                    }
                }
                body {
                    article class="noet-document" 
                            data-network=(network_id_str) 
                            data-bid=(self.belief_id) {
                        // Content
                    }
                }
            }
        }
    }
    
    fn generate_css_variables(&self, vars: &HashMap<String, String>) -> String {
        let mut css = format!("[data-network=\"{}\"] {{\n", self.network_id);
        for (key, value) in vars {
            css.push_str(&format!("  --{}: {};\n", key, value));
        }
        css.push_str("}\n");
        css
    }
}
```

### 5. Update HTML Exporter to Load Network Themes (1 day)

- [ ] Scan all networks for theme configs during export init
- [ ] Build `network_id → ThemeConfig` map
- [ ] Pass network-specific themes to HTML generation
- [ ] Copy theme CSS files to output directory

```rust
pub struct HtmlExporter {
    belief_base: BeliefBase,
    entry_network_id: Uuid,
    network_themes: HashMap<Uuid, ThemeConfig>,
    output_dir: PathBuf,
}

impl HtmlExporter {
    pub fn new(belief_base: BeliefBase, entry_network_id: Uuid, output_dir: PathBuf) -> Self {
        Self {
            belief_base,
            entry_network_id,
            network_themes: HashMap::new(),
            output_dir,
        }
    }
    
    pub fn load_network_themes(&mut self) -> Result<()> {
        // Find all network nodes
        for node in self.belief_base.query(Query::all()) {
            if node.schema == Some("buildonomy.network".to_string()) {
                if let Some(theme) = ThemeConfig::from_payload(&node.payload) {
                    self.network_themes.insert(node.bid.uuid(), theme);
                }
            }
        }
        Ok(())
    }
    
    pub fn export(&mut self) -> Result<()> {
        // 1. Load all network theme configs
        self.load_network_themes()?;
        
        // 2. Copy theme assets
        self.copy_theme_assets()?;
        
        // 3. Generate HTML for each document
        for doc in self.belief_base.documents() {
            let network_theme = self.network_themes.get(&doc.network_id);
            let entry_theme = self.network_themes.get(&self.entry_network_id);
            
            let options = HtmlGenerationOptions {
                network_id: Some(doc.network_id),
                network_theme: network_theme.cloned(),
                site_theme: entry_theme.cloned(),
                ..Default::default()
            };
            
            let html = self.generate_html(&doc, &options)?;
            self.write_html(&doc.path, html)?;
        }
        
        Ok(())
    }
    
    fn copy_theme_assets(&self) -> Result<()> {
        let themes_dir = self.output_dir.join("themes");
        std::fs::create_dir_all(&themes_dir)?;
        
        for (network_id, theme) in &self.network_themes {
            if let Some(css_path) = &theme.css {
                let source = ThemeSource::resolve(css_path, &self.network_root(network_id))?;
                match source {
                    ThemeSource::File(path) => {
                        let dest = themes_dir.join(format!("network-{}.css", network_id));
                        std::fs::copy(path, dest)?;
                    }
                    ThemeSource::Url(url) => {
                        // Optionally fetch and cache URL-based themes
                        // Or just reference URL directly in HTML
                    }
                    ThemeSource::BuiltIn(name) => {
                        // Copy from bundled themes
                    }
                }
            }
        }
        
        Ok(())
    }
}
```

### 6. CLI Support for Theme Override (0.5 days)

- [ ] Add `--theme` flag to export command (per-network override)
- [ ] Support `--network-theme <network-id> <css-path>` for specific networks
- [ ] Document CLI theme options in help text

```rust
#[derive(Parser)]
struct ExportHtmlArgs {
    /// Custom CSS for all networks (overrides all network themes)
    #[arg(long)]
    css: Option<PathBuf>,
    
    /// Override theme for specific network: --network-theme <network-id> <css-path>
    #[arg(long = "network-theme", value_names = ["NETWORK_ID", "CSS_PATH"])]
    network_themes: Vec<(Uuid, PathBuf)>,
}
```

## Testing Requirements

### Unit Tests

- [ ] `ThemeConfig::from_payload` deserialization from YAML/JSON/TOML
- [ ] `ThemeSource::resolve` path resolution (relative, absolute, URL)
- [ ] CSS variable generation with various input formats
- [ ] Theme resolution order (CLI > network > entry > default)

### Integration Tests

- [ ] Multi-network export with different themes per network
- [ ] Network-relative CSS path resolution
- [ ] URL-based theme references
- [ ] CSS variable cascade and scoping
- [ ] CLI theme overrides

### Manual Testing

- [ ] Export multi-network documentation site
- [ ] Verify visual differentiation between networks
- [ ] Test theme asset copying (CSS files in output)
- [ ] Validate CSS scoping in browser DevTools
- [ ] Check backwards compatibility (single-theme mode)

## Success Criteria

- [ ] BeliefNetwork payload can specify theme configuration
- [ ] HTML export generates `data-network` attributes on documents
- [ ] CSS variables are properly scoped per network
- [ ] Network-relative CSS paths resolve correctly
- [ ] URL-based themes are referenced in HTML
- [ ] Theme assets are copied to output directory
- [ ] CLI can override themes per network
- [ ] Multi-network sites show visual differentiation
- [ ] Single-network exports work unchanged (backwards compatible)
- [ ] Documentation includes theming examples and use cases

## Risks

**Risk 1: CSS Conflicts Between Networks**
- **Impact**: Styles from one network bleed into another
- **Mitigation**: Strict CSS scoping with `data-network` attributes, test isolation

**Risk 2: Theme Path Resolution Complexity**
- **Impact**: Network-relative paths fail, cross-network references break
- **Mitigation**: Clear resolution rules, comprehensive path tests, error messages

**Risk 3: Performance with Many Networks**
- **Impact**: Loading multiple stylesheets per page slows rendering
- **Mitigation**: CSS concatenation/minification in Phase 2, HTTP/2 multiplexing

**Risk 4: URL-Based Themes Unavailable at Build Time**
- **Impact**: Export fails if remote CSS unreachable
- **Mitigation**: Optional URL caching, graceful fallback, clear error messages

## Open Questions

1. **Network ID format in HTML**: Use full UUID or short Bref?
   - **Decision needed**: UUID (unique, stable) vs Bref (shorter, more readable)

2. **CSS variable naming convention**: Prefix all with `--network-` or allow bare names?
   - **Recommendation**: Allow bare names, namespace automatically: `primary-color` → `--primary-color`

3. **Theme asset optimization**: Concatenate/minify CSS during export?
   - **Recommendation**: Defer to Phase 3, ship unoptimized first

4. **Built-in theme registry**: How to discover/list available themes?
   - **Recommendation**: Simple HashMap in code, expand to filesystem scan later

5. **Cross-network CSS sharing**: Should networks be able to reference other network's themes?
   - **Recommendation**: Support via absolute paths, copy to shared themes dir

## Future Work (Post-Issue 19)

- **Theme marketplace**: Community-contributed themes
- **CSS preprocessing**: Sass/SCSS compilation support
- **Theme preview**: Visual theme picker in CLI
- **CSS optimization**: Concatenation, minification, purging unused styles
- **Dark mode toggle**: Automatic dark/light theme variants
- **Theme inheritance**: Network themes extend base themes

## References

- Issue 06: HTML Generation and Interactive Viewer
- `docs/design/architecture.md` - BeliefNode payload structure
- `docs/design/beliefbase_architecture.md` - Network node specification
- Jekyll themes: https://jekyllrb.com/docs/themes/
- CSS Scoping: https://developer.mozilla.org/en-US/docs/Web/CSS/Attribute_selectors