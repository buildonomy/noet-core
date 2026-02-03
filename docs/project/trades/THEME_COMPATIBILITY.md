# Trade Study: HTML Template Compatibility with Existing Theme Ecosystems

**Date**: 2026-02-03
**Author**: AI Agent (with human guidance)
**Status**: DRAFT - Awaiting Decision
**Related Issue**: ISSUE_38_INTERACTIVE_SPA.md

## Summary

Evaluate whether noet-core should generate HTML compatible with existing documentation theme ecosystems (Jekyll, Sphinx, MkDocs, etc.) to leverage their CSS themes, or build custom themes from scratch.

**Key Question**: Can we structure our HTML output to be compatible with one or more established theme ecosystems while preserving noet's unique capabilities (section nodes, WASM SPA, graph visualization)?

## Motivation

**Current Situation**:
- We generate custom HTML structure (`src/codec/md.rs`)
- Plan to build custom CSS (`assets/spa.css`) for SPA features
- Would be solely responsible for theming, accessibility, cross-browser support

**Potential Benefits of Theme Compatibility**:
1. **Reduce maintenance burden**: Leverage well-tested, accessible CSS
2. **More theming options**: Users can choose from existing theme libraries
3. **Faster development**: Don't reinvent layout/typography/responsive design
4. **Community integration**: Easier adoption if familiar to Jekyll/Sphinx users
5. **Better defaults**: Professional themes with years of UX refinement

**Potential Costs**:
1. **HTML structure constraints**: Must match theme expectations
2. **Limited customization**: May not support noet-specific features (metadata panel, graph view)
3. **JavaScript conflicts**: Themes with bundled JS may clash with our WASM SPA
4. **Maintenance dependency**: Theme updates could break our HTML

## Current HTML Structure

From `src/codec/md.rs` (lines 1335-1354):

```html
<!DOCTYPE html>
<html>
<head>
  <meta charset="utf-8">
  <title>{title}</title>
  <link rel="stylesheet" href="assets/default-theme.css">
  <script type="application/json" id="noet-metadata">
    {metadata}
  </script>
  {script}
</head>
<body>
  <article>
    <h1 class="document-title">{title}</h1>
    {body}
  </article>
</body>
</html>
```

**Characteristics**:
- Minimal semantic structure (`<article>` wrapper)
- Embedded JSON metadata (`#noet-metadata` script tag)
- Single stylesheet link
- Optional script injection (WASM module)
- No navigation, header, footer, or sidebar markup

**Noet-Specific Requirements**:
- Must inject WASM module (`pkg/viewer.js`)
- Must embed `beliefbase.json` reference or inline data
- Must support SPA navigation (client-side routing)
- Must accommodate metadata panel (backlinks/forward links)
- Must support graph view toggle
- Sections are first-class nodes (not just TOC entries)

## Option 1: Jekyll Theme Compatibility

### Overview

Jekyll is the most popular static site generator, with hundreds of themes. Key themes to consider:
- **just-the-docs**: Documentation-focused, clean, responsive
- **minimal-mistakes**: Flexible, feature-rich
- **Bulma Clean Theme**: Modern, Bulma CSS framework

### HTML Structure Requirements

Jekyll themes expect Liquid template output with specific structure:

```html
<!DOCTYPE html>
<html lang="{{ site.lang | default: "en-US" }}">
  <head>
    <meta charset="UTF-8">
    <meta http-equiv="X-UA-Compatible" content="IE=edge">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <link rel="stylesheet" href="{{ '/assets/css/style.css' | relative_url }}">
  </head>
  <body>
    <div class="wrapper">
      <header>
        <h1>{{ site.title }}</h1>
        <p>{{ site.description }}</p>
      </header>
      <section>
        {{ content }}
      </section>
      <footer>
        <p>{{ site.author }}</p>
      </footer>
    </div>
  </body>
</html>
```

**Just-the-Docs Specific Structure**:

```html
<div class="side-bar">
  <nav>
    <!-- Navigation tree -->
  </nav>
</div>
<div class="main" id="top">
  <div id="main-header" class="main-header">
    <div class="search">
      <input type="text" placeholder="Search">
    </div>
  </div>
  <div id="main-content-wrap" class="main-content-wrap">
    <div id="main-content" class="main-content">
      {{ content }}
    </div>
  </div>
</div>
```

### Compatibility Analysis

**Compatible Elements**:
- ✅ Basic HTML5 structure
- ✅ Semantic `<article>` or `<section>` for content
- ✅ External CSS linking
- ✅ Responsive viewport meta tag
- ✅ Can inject custom `<div>` containers for layout

**Incompatible/Challenging Elements**:
- ❌ Liquid templating (we generate from Rust, not Jekyll)
- ❌ Jekyll front matter expectations (we use TOML)
- ❌ Site-wide navigation generated from `_config.yml` (we use beliefbase.json)
- ❌ JavaScript bundled with themes (may conflict with WASM)
- ⚠️ Search functionality (themes use Lunr.js, we use WASM queries)
- ⚠️ Theme configuration via `_config.yml` (we'd need alternative)

**Adaptation Strategy**:
1. Generate HTML structure matching Jekyll theme expectations (e.g., just-the-docs div classes)
2. Inject navigation from `beliefbase.json` instead of Jekyll's site map
3. Include theme CSS but override/extend with custom CSS for noet features
4. Replace theme JavaScript with our WASM SPA controller
5. Use data attributes for compatibility hooks

**Example Adapted Output**:

```html
<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>{{ title }}</title>
  <!-- Just-the-Docs CSS -->
  <link rel="stylesheet" href="assets/just-the-docs/just-the-docs.css">
  <!-- Noet custom CSS for SPA features -->
  <link rel="stylesheet" href="assets/noet-spa-extensions.css">
  <!-- Embedded metadata -->
  <script type="application/json" id="noet-metadata">{{ metadata }}</script>
  <!-- WASM SPA module -->
  <script type="module" src="pkg/viewer.js"></script>
</head>
<body>
  <div class="side-bar">
    <nav id="noet-nav-tree">
      <!-- Populated by viewer.js from beliefbase.json -->
    </nav>
  </div>
  
  <div class="main" id="top">
    <div id="main-header" class="main-header">
      <div class="search">
        <input type="search" id="noet-search" placeholder="Search...">
      </div>
      <button id="graph-toggle">Graph View</button>
    </div>
    
    <div id="main-content-wrap" class="main-content-wrap">
      <div id="main-content" class="main-content">
        <h1>{{ title }}</h1>
        {{ content }}
      </div>
      
      <!-- Noet-specific metadata panel -->
      <aside id="noet-metadata-panel" class="metadata-panel">
        <!-- Populated by viewer.js -->
      </aside>
    </div>
  </div>
  
  <!-- Noet-specific graph view -->
  <div id="noet-graph-container" class="hidden">
    <svg id="graph-canvas"></svg>
  </div>
</body>
</html>
```

**Pros**:
- ✅ Leverage just-the-docs CSS (responsive, accessible, well-tested)
- ✅ Familiar structure for Jekyll users
- ✅ Professional typography and layout out-of-box
- ✅ Can extend with custom CSS for noet features
- ✅ Active theme maintenance (bug fixes, improvements)

**Cons**:
- ❌ Tight coupling to theme's HTML structure (fragile if theme updates)
- ❌ Need to maintain compatibility layer as theme evolves
- ❌ Theme CSS may not cover noet-specific features (metadata panel, graph)
- ❌ JavaScript conflicts (must remove/replace theme's JS)
- ❌ May inherit unwanted features (theme switcher, etc.)

**Effort Estimate**: 3-4 days
- 1 day: Modify HTML generation to match just-the-docs structure
- 1 day: Integrate theme CSS and test responsive behavior
- 1-2 days: Write custom CSS for noet-specific features (metadata panel, graph)

## Option 2: Sphinx/Read the Docs Theme Compatibility

### Overview

Sphinx is Python's documentation generator, widely used for technical docs. The Read the Docs theme is extremely popular.

### HTML Structure Requirements

Sphinx generates this structure:

```html
<!DOCTYPE html>
<html class="no-js" lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width,initial-scale=1">
  <link rel="stylesheet" href="_static/css/theme.css">
</head>
<body class="wy-body-for-nav">
  <nav data-toggle="wy-nav-shift" class="wy-nav-side">
    <div class="wy-side-scroll">
      <div class="wy-side-nav-search">
        <!-- Search box -->
      </div>
      <div class="wy-menu wy-menu-vertical">
        <!-- Navigation tree -->
      </div>
    </div>
  </nav>
  
  <section data-toggle="wy-nav-shift" class="wy-nav-content-wrap">
    <nav class="wy-nav-top">
      <!-- Top nav -->
    </nav>
    
    <div class="wy-nav-content">
      <div class="rst-content">
        <div role="main" class="document">
          {{ body }}
        </div>
      </div>
    </div>
  </section>
</body>
</html>
```

### Compatibility Analysis

**Compatible Elements**:
- ✅ Similar three-column layout (nav, content, metadata)
- ✅ Semantic HTML5 structure
- ✅ External CSS system
- ✅ Responsive design built-in

**Incompatible/Challenging Elements**:
- ❌ Expects reStructuredText output (not Markdown)
- ❌ Sphinx-specific CSS classes and data attributes
- ❌ JavaScript dependencies (ReadTheDocs theme.js)
- ❌ Built for Python ecosystem (less familiar to general users)
- ⚠️ Search expects Sphinx's searchindex.js format

**Adaptation Strategy**:
Similar to Jekyll approach:
1. Generate HTML matching RTD theme structure
2. Include RTD CSS
3. Populate nav from beliefbase.json
4. Replace/extend JavaScript for WASM SPA

**Pros**:
- ✅ Excellent technical documentation styling
- ✅ Very popular in developer tools space
- ✅ Strong accessibility features
- ✅ Mobile-friendly

**Cons**:
- ❌ Less flexible than Jekyll themes
- ❌ Tighter coupling to Sphinx ecosystem
- ❌ Heavier JavaScript dependencies
- ❌ May feel "Python-specific" to non-Python users

**Effort Estimate**: 4-5 days (similar to Jekyll but less familiar)

## Option 3: MkDocs Material Theme Compatibility

### Overview

MkDocs is a Python-based static site generator focused on project documentation. Material theme is modern and feature-rich.

### HTML Structure Requirements

Material theme uses this structure:

```html
<!DOCTYPE html>
<html lang="en" class="no-js">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width,initial-scale=1">
  <link rel="stylesheet" href="assets/stylesheets/main.css">
</head>
<body>
  <input class="md-toggle" type="checkbox" id="__drawer">
  <input class="md-toggle" type="checkbox" id="__search">
  
  <header class="md-header">
    <!-- Header content -->
  </header>
  
  <div class="md-container">
    <nav class="md-tabs"></nav>
    <main class="md-main">
      <div class="md-sidebar md-sidebar--primary">
        <!-- Navigation -->
      </div>
      <div class="md-sidebar md-sidebar--secondary">
        <!-- TOC -->
      </div>
      <div class="md-content">
        <article class="md-content__inner">
          {{ page.content }}
        </article>
      </div>
    </main>
  </div>
</body>
</html>
```

### Compatibility Analysis

**Compatible Elements**:
- ✅ Modern, clean design
- ✅ Excellent mobile responsiveness
- ✅ Built-in search
- ✅ Flexible layout system

**Incompatible/Challenging Elements**:
- ❌ Heavy JavaScript framework (Material Design components)
- ❌ Expects MkDocs YAML configuration
- ❌ Custom search implementation
- ❌ May conflict heavily with WASM SPA

**Pros**:
- ✅ Beautiful, modern design
- ✅ Great UX patterns
- ✅ Strong mobile support

**Cons**:
- ❌ Heaviest JavaScript dependency of all options
- ❌ Likely conflicts with WASM SPA controller
- ❌ Material Design may not match all use cases

**Effort Estimate**: 5-6 days (most complex integration)

## Option 4: Custom CSS Based on Theme Patterns

### Overview

Build custom CSS but borrow design patterns, color schemes, and component styles from existing themes without strict HTML compatibility.

### Approach

1. **Analyze popular themes** (just-the-docs, RTD, Material) for:
   - Color palettes
   - Typography scales
   - Component patterns (nav, search, buttons)
   - Responsive breakpoints
   - Accessibility features

2. **Generate clean, semantic HTML** optimized for noet's needs:
   - No unnecessary divs for theme compatibility
   - Custom structure for metadata panel
   - Graph view integration from the start
   - Section nodes as first-class navigation

3. **Write custom CSS** inspired by theme patterns:
   - CSS custom properties (variables) for theming
   - Component-based architecture
   - Responsive grid system
   - Accessible focus states, ARIA support

4. **Provide theme variants**:
   - Default theme (just-the-docs-inspired colors/layout)
   - Dark mode toggle
   - User-customizable via CSS variables

### Example HTML Structure

```html
<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>{{ title }}</title>
  <link rel="stylesheet" href="assets/noet-theme.css">
  <script type="application/json" id="noet-metadata">{{ metadata }}</script>
  <script type="module" src="pkg/viewer.js"></script>
</head>
<body class="noet-spa">
  <div class="spa-container">
    <header class="spa-header">
      <h1 class="site-title">{{ network.title }}</h1>
      <input type="search" class="search-input" placeholder="Search...">
      <button class="btn-graph-toggle">Graph</button>
    </header>
    
    <nav class="spa-nav">
      <div id="nav-tree"></div>
    </nav>
    
    <main class="spa-content">
      <article class="document">
        <h1>{{ title }}</h1>
        {{ content }}
      </article>
    </main>
    
    <aside class="spa-metadata">
      <div id="metadata-panel"></div>
    </aside>
    
    <div class="spa-graph hidden">
      <svg id="graph-canvas"></svg>
    </div>
  </div>
</body>
</html>
```

**CSS Organization**:

```css
/* noet-theme.css */

/* 1. Custom Properties (inspired by just-the-docs colors) */
:root {
  --color-bg: #ffffff;
  --color-text: #27272a;
  --color-accent: #2563eb;
  /* ... */
}

/* 2. Layout Grid (custom for noet's three-column + graph needs) */
.spa-container {
  display: grid;
  grid-template-areas: 
    "header header header"
    "nav content metadata";
  /* ... */
}

/* 3. Components (patterns from just-the-docs, custom implementation) */
.spa-nav { /* ... */ }
.spa-metadata { /* ... */ }
.spa-graph { /* ... */ }

/* 4. Typography (just-the-docs scale) */
h1 { font-size: 2rem; /* ... */ }

/* 5. Responsive (breakpoints from just-the-docs) */
@media (max-width: 768px) { /* ... */ }
```

**Pros**:
- ✅ Full control over HTML structure (no compatibility constraints)
- ✅ Optimized for noet's unique features from the start
- ✅ No JavaScript conflicts (designed for WASM SPA)
- ✅ Cleaner, more maintainable code
- ✅ Can still borrow design patterns from popular themes
- ✅ Easier to extend and customize
- ✅ No dependency on external theme updates

**Cons**:
- ❌ Responsible for all CSS development and maintenance
- ❌ Must implement accessibility features ourselves
- ❌ Must test across browsers/devices
- ❌ Users don't get "theme library" to choose from
- ❌ May not look as polished initially (takes iteration)

**Effort Estimate**: 4-5 days
- 2 days: Core CSS (layout, typography, components)
- 1 day: Responsive design and testing
- 1 day: Accessibility features (keyboard nav, focus states, ARIA)
- 1 day: Polish and refinement

## Comparison Matrix

| Criterion | Jekyll (just-the-docs) | Sphinx (RTD) | MkDocs Material | Custom CSS | Weight |
|-----------|------------------------|--------------|-----------------|------------|--------|
| **Development Speed** | ⭐⭐⭐⭐ (reuse existing) | ⭐⭐⭐ (familiar patterns) | ⭐⭐ (complex integration) | ⭐⭐⭐ (build from scratch) | HIGH |
| **Maintenance Burden** | ⭐⭐ (track theme updates) | ⭐⭐ (track theme updates) | ⭐ (heavy dependencies) | ⭐⭐⭐⭐ (full control) | HIGH |
| **Noet Feature Support** | ⭐⭐⭐ (can extend) | ⭐⭐⭐ (can extend) | ⭐⭐ (conflicts likely) | ⭐⭐⭐⭐⭐ (designed for noet) | CRITICAL |
| **Customization Flexibility** | ⭐⭐ (constrained by theme) | ⭐⭐ (constrained by theme) | ⭐⭐ (constrained by theme) | ⭐⭐⭐⭐⭐ (unlimited) | HIGH |
| **JavaScript Compatibility** | ⭐⭐⭐ (must replace theme JS) | ⭐⭐ (heavy JS deps) | ⭐ (framework conflicts) | ⭐⭐⭐⭐⭐ (designed for WASM) | CRITICAL |
| **Professional Polish** | ⭐⭐⭐⭐⭐ (years of refinement) | ⭐⭐⭐⭐⭐ (industry standard) | ⭐⭐⭐⭐⭐ (modern design) | ⭐⭐⭐ (requires iteration) | MEDIUM |
| **Accessibility** | ⭐⭐⭐⭐⭐ (WCAG compliant) | ⭐⭐⭐⭐⭐ (WCAG compliant) | ⭐⭐⭐⭐⭐ (WCAG compliant) | ⭐⭐⭐ (must implement) | HIGH |
| **User Familiarity** | ⭐⭐⭐⭐ (very popular) | ⭐⭐⭐⭐ (dev tools standard) | ⭐⭐⭐ (growing popularity) | ⭐⭐ (new to users) | LOW |
| **Theme Ecosystem** | ⭐⭐⭐⭐⭐ (hundreds of themes) | ⭐⭐⭐ (fewer options) | ⭐⭐⭐ (Material variants) | ⭐ (just ours) | LOW |
| **Code Cleanliness** | ⭐⭐ (compatibility divs) | ⭐⭐ (compatibility divs) | ⭐⭐ (heavy markup) | ⭐⭐⭐⭐⭐ (semantic HTML) | MEDIUM |

**Scoring**: ⭐ (poor) to ⭐⭐⭐⭐⭐ (excellent)

## Hybrid Option: Conditional Theme Support

### Concept

Generate HTML with **configurable structure** that can target different themes based on user preference:

1. **Default mode**: Custom noet HTML + custom CSS (optimized for WASM SPA)
2. **Jekyll mode**: HTML matching just-the-docs structure (for users wanting Jekyll integration)
3. **Sphinx mode**: HTML matching RTD structure (for Python docs users)

**Implementation**:
- Add `theme_compat` option to HTML generation config
- Conditional HTML templates based on theme target
- Ship default custom CSS plus compatibility CSS for each theme
- User chooses via CLI flag or config file

**Example**:
```bash
noet html generate --theme custom  # Default, optimized for noet
noet html generate --theme jekyll  # Jekyll-compatible output
noet html generate --theme sphinx  # Sphinx-compatible output
```

**Pros**:
- ✅ Best of both worlds (custom + compatibility)
- ✅ Users can integrate into existing Jekyll/Sphinx sites
- ✅ Still get optimized experience with custom theme
- ✅ Flexibility for different use cases

**Cons**:
- ❌ Significantly more complex to maintain (3+ HTML templates)
- ❌ Testing burden (must test all theme modes)
- ❌ Documentation overhead (explain theme options)
- ❌ Code duplication (HTML generation logic)

**Effort Estimate**: 8-10 days (effectively building multiple options)

## Recommendation

**Recommended Approach**: **Option 4: Custom CSS Based on Theme Patterns**

### Rationale

1. **Critical: Noet Feature Support**
   - Metadata panel with WeightKind grouping is unique to noet
   - Graph view requires custom layout (not in any existing theme)
   - Section nodes as first-class navigation elements (unique architecture)
   - WASM SPA controller needs clean HTML without theme JavaScript conflicts

2. **High: Maintenance Burden**
   - External themes update independently (breaking changes possible)
   - Compatibility layer becomes ongoing maintenance
   - Full control means faster bug fixes and feature additions

3. **High: Customization Flexibility**
   - Can optimize HTML structure for performance
   - Can add noet-specific features without workarounds
   - Can evolve architecture without theme constraints

4. **Medium: Professional Polish**
   - Can borrow proven patterns from just-the-docs (colors, typography, responsive breakpoints)
   - First iteration may be simpler, but can refine over time
   - User feedback will guide improvements specific to noet's use cases

5. **High: Accessibility**
   - Must implement ourselves, but can follow WCAG guidelines
   - Can reference just-the-docs/RTD implementations as examples
   - Easier to test and validate when we control all HTML/CSS

### Implementation Plan

**Phase 1: Core CSS (2 days)**
- Set up CSS custom properties (colors, spacing, typography) inspired by just-the-docs
- Implement three-column grid layout
- Basic component styles (nav, content, metadata panel)

**Phase 2: Responsive Design (1 day)**
- Breakpoints matching just-the-docs (mobile <768px, tablet 768-1024px, desktop >1024px)
- Hamburger menu, collapsible panels
- Test on real devices

**Phase 3: Accessibility (1 day)**
- Keyboard navigation
- Focus states
- ARIA labels
- Screen reader testing

**Phase 4: Polish (1 day)**
- Typography refinement
- Color contrast validation
- Cross-browser testing
- Dark mode (optional)

**Total**: 4-5 days (same as original Step 1 estimate in ISSUE_38)

### What We Borrow from just-the-docs

✅ **Design Patterns** (analyze and adapt):
- Color palette (blues, grays, white backgrounds)
- Typography scale (font sizes, line heights)
- Spacing system (consistent margins/padding)
- Component patterns (nav tree, search box, buttons)
- Responsive breakpoints (mobile/tablet/desktop)

❌ **Do Not Use Directly**:
- Actual CSS files (licensing and compatibility issues)
- HTML structure (incompatible with our needs)
- JavaScript (conflicts with WASM SPA)
- Liquid templates (we're not Jekyll)

### Migration Path

If in the future we want to support Jekyll theme compatibility:
1. Custom CSS establishes our baseline (works well, proven)
2. Can add `--theme jekyll` mode as optional feature (hybrid approach)
3. Users who need Jekyll integration can opt-in
4. Default experience remains optimized for noet

This keeps options open without premature complexity.

## Open Questions

### Q1: Should we support CSS theming via custom properties?
Allow users to customize colors/fonts without editing CSS?

**Options**:
- A) Yes, expose CSS variables in documentation
- B) No, ship single theme (simpler)
- C) Provide 2-3 preset themes (light, dark, high-contrast)

**Recommendation**: Option A (low cost, high value)

### Q2: Should we bundle fonts or use system fonts?
just-the-docs uses system fonts for performance.

**Options**:
- A) System fonts (fast, no network requests)
- B) Web fonts (consistent appearance, slower)

**Recommendation**: Option A (matches just-the-docs philosophy)

### Q3: Should we provide theme preview/demo page?
Help users see what noet output looks like before generating?

**Options**:
- A) Yes, publish demo site with sample network
- B) No, users see it when they generate
- C) Include demo in test infrastructure

**Recommendation**: Option C initially, Option A when ready to publicize

### Q4: How much should we document CSS customization?
Help users modify styles for their needs?

**Options**:
- A) Full CSS documentation (class names, custom properties, examples)
- B) Minimal (just reference CSS file)
- C) Guided customization (common tasks only)

**Recommendation**: Option C (most common use cases documented)

## Decision

**DECIDED**: ✅ **Option F: Custom CSS + Open Props (Hybrid)**

Selected approach combines custom CSS with Open Props design tokens:
- Vendor Open Props into `assets/` (offline-first, CLI flag for CDN override)
- Write custom CSS using Open Props variables
- Provide light/dark theme presets
- JavaScript theme switcher with system default support
- Clean semantic HTML (perfect for generated output)
- Deep user customization via CSS variables

**Next Steps**: Update ISSUE_38 with finalized implementation plan and proceed with Step 1.

## Addendum: UI Framework Integration Analysis

**Added**: 2026-02-03 (per user request)

### Question

Could integrating a UI framework (Tailwind, Open Props, Pico CSS, etc.) help:
1. Lower development burden (pre-built components/utilities)
2. Enable deeper user customization
3. Provide stunning defaults out-of-box

### Framework Options Comparison

#### Option A: Tailwind CSS

**Overview**: Utility-first CSS framework with composable class names.

**Example Usage**:
```html
<div class="flex flex-col md:flex-row gap-4 p-6 bg-white rounded-lg shadow-md">
  <nav class="w-64 border-r border-gray-200">...</nav>
  <main class="flex-1 max-w-3xl">...</main>
  <aside class="w-80 bg-gray-50">...</aside>
</div>
```

**Pros**:
- ✅ Extremely popular (huge community, plugins, resources)
- ✅ Highly customizable via `tailwind.config.js`
- ✅ Responsive utilities built-in (`md:`, `lg:` prefixes)
- ✅ Design system baked in (spacing scale, colors, typography)
- ✅ Can extend with custom utilities
- ✅ JIT mode generates only used classes

**Cons**:
- ❌ **Requires build step** (PostCSS for optimal usage)
- ❌ **HTML verbosity** - generates strings like `class="flex items-center justify-between px-4 py-2 bg-blue-500 text-white rounded hover:bg-blue-600"`
- ❌ Awkward for server-side HTML generation (we'd concatenate class strings in Rust)
- ❌ Purging unused CSS requires build pipeline
- ❌ CDN version is massive (3MB+) without purging
- ❌ Users must learn Tailwind's class naming conventions

**Generated HTML Compatibility**: ⭐⭐ (possible but clunky)

**Customization Mechanism**: `tailwind.config.js` (requires build step)

**Bundle Size**: 
- With JIT + purge: ~10-50KB (optimal)
- CDN full build: 3MB+ (impractical)

**Build Step Required**: Yes (PostCSS + Tailwind CLI)

**Effort Estimate**: 3-4 days
- 1 day: Set up Tailwind build pipeline
- 1 day: Modify HTML generation to emit Tailwind classes
- 1 day: Configure theme and purge settings
- 1 day: Test and document customization

**Recommendation**: ❌ **Not ideal for static HTML generation**. Build step adds complexity, and emitting utility class strings from Rust is awkward.

---

#### Option B: UnoCSS

**Overview**: Instant on-demand atomic CSS engine (Tailwind-compatible syntax, faster).

**Pros**:
- ✅ Tailwind-compatible syntax (easy migration path)
- ✅ Faster than Tailwind (instant on-demand)
- ✅ Better for static generation (can scan HTML files)
- ✅ Flexible presets (icons, typography, etc.)
- ✅ Smaller bundle size than Tailwind

**Cons**:
- ❌ Still requires build step (Vite/Webpack plugin)
- ❌ Smaller community than Tailwind
- ❌ Same HTML verbosity issue
- ❌ Still awkward for server-generated HTML

**Recommendation**: ❌ **Same issues as Tailwind** for our use case.

---

#### Option C: Open Props

**Overview**: CSS custom properties (variables) design system. No framework, just a comprehensive set of design tokens.

**Example Usage**:
```css
/* Import Open Props */
@import "https://unpkg.com/open-props";

/* Use variables in custom CSS */
.spa-header {
  background: var(--surface-1);
  padding: var(--size-3);
  border-bottom: var(--border-size-1) solid var(--surface-3);
}

.btn-primary {
  background: var(--blue-6);
  color: var(--gray-0);
  padding: var(--size-2) var(--size-4);
  border-radius: var(--radius-2);
}
```

**HTML Structure** (clean, semantic):
```html
<div class="spa-container">
  <header class="spa-header">...</header>
  <nav class="spa-nav">...</nav>
  <main class="spa-content">...</main>
</div>
```

**Pros**:
- ✅ **No build step required** (just CSS imports)
- ✅ **Clean HTML** (semantic class names, not utility bloat)
- ✅ **Perfect for generated HTML** (Rust emits simple class names)
- ✅ **Deep customization** (users override CSS variables)
- ✅ Comprehensive design tokens (colors, spacing, shadows, animations)
- ✅ Small bundle (~20KB gzipped)
- ✅ Works with vanilla CSS (no framework lock-in)
- ✅ Responsive utilities available (media query variables)

**Cons**:
- ❌ Not a component library (we still write CSS)
- ❌ Less opinionated (more design decisions on us)
- ❌ Smaller community than Tailwind

**Customization Mechanism**: CSS variables (user overrides in custom CSS)

```css
/* User customization example */
:root {
  --brand-color: #2563eb; /* Override Open Props blue */
  --font-sans: "Inter", system-ui; /* Custom font */
  --size-header: 64px; /* Custom spacing */
}
```

**Bundle Size**: ~20KB (core), ~35KB (with all extras)

**Build Step Required**: No (can use CDN or local copy)

**Effort Estimate**: 3-4 days (same as custom CSS, but with better defaults)

**Recommendation**: ✅ **Strong candidate** - Combines best of both worlds (design system + clean HTML)

---

#### Option D: Pico CSS

**Overview**: Minimal semantic/classless CSS framework. Styles raw HTML elements beautifully.

**Example Usage**:
```html
<!-- Just write semantic HTML, Pico styles it -->
<nav>
  <ul>
    <li><a href="/">Home</a></li>
    <li><a href="/docs">Docs</a></li>
  </ul>
</nav>

<main>
  <article>
    <h1>Document Title</h1>
    <p>Content here...</p>
  </article>
</main>
```

**CSS** (minimal class usage):
```css
/* Pico handles most styling automatically */
/* Add custom classes only for layout */
.container { max-width: 1200px; }
.grid { display: grid; grid-template-columns: 1fr 3fr 1fr; }
```

**Pros**:
- ✅ **Beautiful defaults** (looks great with zero classes)
- ✅ **Minimal HTML** (mostly semantic tags)
- ✅ **Perfect for generated content** (Rust emits clean HTML)
- ✅ **Small bundle** (~10KB minified)
- ✅ **Responsive** built-in
- ✅ **Accessible** (ARIA-friendly, keyboard nav)
- ✅ **No build step**
- ✅ **Customizable via CSS variables**
- ✅ Dark mode built-in

**Cons**:
- ❌ Less control over component styling (opinionated defaults)
- ❌ May need custom CSS for complex layouts (metadata panel, graph)
- ❌ Limited to semantic HTML patterns
- ❌ Smaller ecosystem than Tailwind

**Customization Mechanism**: CSS variables

```css
:root {
  --primary: #2563eb;
  --spacing: 1rem;
  --border-radius: 0.25rem;
}
```

**Bundle Size**: ~10KB

**Build Step Required**: No

**Effort Estimate**: 2-3 days
- 1 day: Integrate Pico CSS, test defaults
- 1 day: Add custom CSS for noet-specific components
- 0.5 day: Test responsive behavior

**Recommendation**: ✅ **Strong candidate** - Great defaults, minimal effort, clean HTML

---

#### Option E: Bulma (CSS-only)

**Overview**: Component library with utility classes, no JavaScript.

**Pros**:
- ✅ Component library (nav, cards, modals, etc.)
- ✅ No JavaScript (pure CSS)
- ✅ Flexbox-based (modern)
- ✅ Good documentation

**Cons**:
- ❌ Heavier bundle (~200KB minified)
- ❌ More opinionated (less flexible than Open Props)
- ❌ Still requires classes (not semantic like Pico)
- ❌ Less popular than Tailwind

**Recommendation**: ⚠️ **Possible but not optimal** - Heavier than needed, less flexible

---

#### Option F: Custom CSS + Open Props (Hybrid)

**Concept**: Write custom CSS using Open Props design tokens as foundation.

**Implementation**:
```css
/* Import Open Props for design tokens */
@import "https://unpkg.com/open-props";
@import "https://unpkg.com/open-props/normalize.min.css";

/* Custom CSS using Open Props variables */
.spa-container {
  display: grid;
  grid-template-areas: 
    "header header header"
    "nav content metadata";
  grid-template-columns: var(--size-content-1) 1fr var(--size-content-2);
  gap: var(--size-3);
  height: 100vh;
}

.spa-header {
  grid-area: header;
  background: var(--surface-1);
  border-bottom: var(--border-size-1) solid var(--surface-3);
  padding: var(--size-3);
}

.spa-nav {
  grid-area: nav;
  background: var(--surface-2);
  overflow-y: auto;
  padding: var(--size-3);
}

/* Responsive using Open Props media queries */
@media (max-width: 768px) {
  .spa-container {
    grid-template-areas: "header" "content";
    grid-template-columns: 1fr;
  }
}
```

**Generated HTML** (clean, semantic):
```html
<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <title>{{ title }}</title>
  <link rel="stylesheet" href="https://unpkg.com/open-props">
  <link rel="stylesheet" href="assets/noet-theme.css">
  <script type="module" src="pkg/viewer.js"></script>
</head>
<body>
  <div class="spa-container">
    <header class="spa-header">...</header>
    <nav class="spa-nav">...</nav>
    <main class="spa-content">...</main>
    <aside class="spa-metadata">...</aside>
  </div>
</body>
</html>
```

**Pros**:
- ✅ **Best of both worlds**: Design system + full control
- ✅ **Clean HTML**: Simple semantic class names
- ✅ **Easy customization**: Users override CSS variables
- ✅ **No build step**: Just CSS imports
- ✅ **Small bundle**: Open Props (~20KB) + custom CSS (~10-20KB)
- ✅ **Perfect for generated HTML**: Rust emits clean classes
- ✅ **Professional defaults**: Open Props provides proven design tokens
- ✅ **Maintainable**: Custom CSS for noet-specific needs

**Cons**:
- ❌ Still requires writing CSS (less than fully custom, more than Tailwind)
- ❌ No pre-built components (build our own)

**Effort Estimate**: 3-4 days (similar to pure custom, but faster with Open Props tokens)

**Recommendation**: ✅✅ **BEST OPTION** - Optimal balance of control, customization, and development speed

---

### UI Framework Comparison Matrix

| Criterion | Tailwind | UnoCSS | Open Props | Pico CSS | Bulma | Custom + Open Props |
|-----------|----------|---------|------------|----------|-------|---------------------|
| **Build Step Required** | ❌ Yes | ❌ Yes | ✅ No | ✅ No | ✅ No | ✅ No |
| **HTML Cleanliness** | ⭐⭐ (verbose) | ⭐⭐ (verbose) | ⭐⭐⭐⭐⭐ (semantic) | ⭐⭐⭐⭐⭐ (semantic) | ⭐⭐⭐ (classes) | ⭐⭐⭐⭐⭐ (semantic) |
| **Generated HTML Compatibility** | ⭐⭐ (awkward) | ⭐⭐ (awkward) | ⭐⭐⭐⭐⭐ (perfect) | ⭐⭐⭐⭐⭐ (perfect) | ⭐⭐⭐⭐ (good) | ⭐⭐⭐⭐⭐ (perfect) |
| **User Customization** | ⭐⭐⭐⭐⭐ (config) | ⭐⭐⭐⭐⭐ (config) | ⭐⭐⭐⭐⭐ (CSS vars) | ⭐⭐⭐⭐ (CSS vars) | ⭐⭐⭐ (Sass) | ⭐⭐⭐⭐⭐ (CSS vars) |
| **Default Beauty** | ⭐⭐⭐ (needs work) | ⭐⭐⭐ (needs work) | ⭐⭐⭐ (tokens only) | ⭐⭐⭐⭐⭐ (gorgeous) | ⭐⭐⭐⭐ (nice) | ⭐⭐⭐⭐ (depends on us) |
| **Bundle Size** | ⭐⭐ (10-50KB purged) | ⭐⭐⭐ (smaller) | ⭐⭐⭐⭐⭐ (20KB) | ⭐⭐⭐⭐⭐ (10KB) | ⭐⭐ (200KB) | ⭐⭐⭐⭐⭐ (30-40KB) |
| **Development Speed** | ⭐⭐⭐⭐ (fast once set up) | ⭐⭐⭐⭐ (fast once set up) | ⭐⭐⭐ (write CSS) | ⭐⭐⭐⭐⭐ (minimal CSS) | ⭐⭐⭐⭐ (components) | ⭐⭐⭐⭐ (tokens help) |
| **Noet Feature Support** | ⭐⭐⭐ (utility classes) | ⭐⭐⭐ (utility classes) | ⭐⭐⭐⭐⭐ (full control) | ⭐⭐⭐ (need custom CSS) | ⭐⭐⭐ (adapt components) | ⭐⭐⭐⭐⭐ (full control) |
| **Long-term Maintenance** | ⭐⭐⭐ (track updates) | ⭐⭐⭐ (smaller team) | ⭐⭐⭐⭐⭐ (just CSS vars) | ⭐⭐⭐⭐ (stable) | ⭐⭐⭐ (updates) | ⭐⭐⭐⭐⭐ (our code) |
| **Community/Ecosystem** | ⭐⭐⭐⭐⭐ (huge) | ⭐⭐⭐ (growing) | ⭐⭐⭐ (niche) | ⭐⭐⭐ (smaller) | ⭐⭐⭐ (declining) | ⭐⭐⭐⭐ (Open Props + CSS) |

---

### Updated Recommendation

**Primary Recommendation**: **Option F: Custom CSS + Open Props**

**Rationale**:

1. **No Build Step** - Critical for simplicity
   - Users can customize without PostCSS/build pipeline
   - We can iterate faster (no compilation)
   - Deployment is simpler (just static files)

2. **Perfect for Generated HTML** - Critical for our architecture
   - Rust generates clean semantic class names: `class="spa-header"`
   - Not awkward utility string concatenation: `class="flex items-center justify-between px-4 py-2 bg-blue-500"`
   - Easy to read generated HTML
   - Users can inspect/understand structure

3. **Deep Customization via CSS Variables** - Key user benefit
   ```css
   /* User's custom-theme.css */
   :root {
     --brand-primary: #ff6b35;
     --spacing-unit: 1.2rem;
     --font-heading: "Merriweather", serif;
   }
   ```
   - No build step for theme changes
   - Live preview of customization
   - Can layer multiple theme files

4. **Professional Design System** - Lower development burden
   - Open Props provides 700+ CSS variables
   - Proven design tokens (colors, spacing, shadows, animations)
   - Responsive media query variables
   - Accessibility built-in (focus states, contrast)
   - We don't reinvent the wheel

5. **Small Bundle** - Performance
   - Open Props: ~20KB gzipped
   - Our custom CSS: ~10-20KB (optimized)
   - Total: ~30-40KB (smaller than Tailwind purged output)

6. **Future-Proof** - Maintainability
   - Standard CSS (no framework lock-in)
   - Can migrate to any future system
   - Open Props is just variables (minimal dependency)
   - Our custom CSS is fully under our control

**Secondary Recommendation**: **Option D: Pico CSS** (if we want even less CSS work)

Good if:
- We want gorgeous defaults with minimal effort
- Content is mostly semantic HTML (which it is)
- We're okay with Pico's opinionated styling
- We only need custom CSS for layout (grid, metadata panel, graph)

Trade-off:
- Less control over component appearance
- May need to override Pico styles for noet-specific features

---

### Implementation Plan (Updated)

**Phase 1: Set Up Open Props (0.5 days)**
```html
<!-- Add to HTML template -->
<link rel="stylesheet" href="https://unpkg.com/open-props">
<link rel="stylesheet" href="https://unpkg.com/open-props/normalize.min.css">
```

**Phase 2: Write Custom CSS Using Open Props (2-3 days)**
```css
/* assets/noet-theme.css */

/* Use Open Props variables throughout */
.spa-container {
  display: grid;
  gap: var(--size-3);
  background: var(--surface-1);
}

.btn-primary {
  background: var(--blue-6);
  color: var(--gray-0);
  padding: var(--size-2) var(--size-4);
  border-radius: var(--radius-2);
  box-shadow: var(--shadow-2);
}

/* Custom variables for noet-specific needs */
:root {
  --noet-sidebar-width: 280px;
  --noet-metadata-width: 320px;
  --noet-graph-bg: var(--surface-2);
}
```

**Phase 3: Test Customization (0.5 days)**
- Verify users can override variables
- Test responsive behavior
- Validate accessibility

**Phase 4: Documentation (1 day)**
- Document available CSS variables
- Provide theme customization examples
- Show how to create custom themes

**Total Effort**: 4-5 days (same as original estimate, better result)

---

### Open Questions (Updated)

#### Q1: Should we vendor Open Props or use CDN?

**Options**:
- A) CDN (`https://unpkg.com/open-props`) - Simple, always up-to-date
- B) Vendor into `assets/` - No external dependency, offline-friendly
- C) Hybrid (CDN with fallback) - Best of both

**Decision**: ✅ **Option B (vendor) with CLI flag for CDN override**
- Default: Vendored Open Props in `assets/open-props/` (offline-first)
- CLI flag: `--open-props-cdn` to use CDN version
- Positioning: noet as offline-first product
- Implementation: Download Open Props CSS files during build, include in assets

#### Q2: Should we provide multiple theme presets?

Using Open Props, we could easily provide:
- `noet-theme-default.css` (just-the-docs inspired colors)
- `noet-theme-dark.css` (dark mode)
- `noet-theme-minimal.css` (grayscale, minimal)

**Decision**: ✅ **Yes, provide multiple theme presets**
- Ship with light and dark theme CSS files
- Users can create custom themes by overriding CSS variables
- Document theme creation process

#### Q3: Should we expose theme switching UI?

**Options**:
- A) Theme switcher in header (light/dark toggle)
- B) No UI, users choose via config/CSS link
- C) JavaScript theme switcher (localStorage persistence)

**Decision**: ✅ **Option C: JavaScript theme switcher with system default**
- Toggle button in header (light/dark/auto modes)
- Default: Respect system preference (`prefers-color-scheme`)
- Persist user choice in localStorage
- Auto mode follows system dark/light setting
- Implementation: Add to viewer.js SPA controller

---

## References

- just-the-docs: https://github.com/just-the-docs/just-the-docs
- Sphinx RTD Theme: https://github.com/readthedocs/sphinx_rtd_theme
- MkDocs Material: https://github.com/squidfunk/mkdocs-material
- WCAG 2.1 Guidelines: https://www.w3.org/WAI/WCAG21/quickref/
- Current HTML generation: `src/codec/md.rs` lines 1335-1354
- Issue 38: `docs/project/ISSUE_38_INTERACTIVE_SPA.md`
- **Tailwind CSS**: https://tailwindcss.com
- **UnoCSS**: https://unocss.dev
- **Open Props**: https://open-props.style
- **Pico CSS**: https://picocss.com
- **Bulma**: https://bulma.io
