//! Shared pulldown-cmark parser options for Buildonomy markdown.
//!
//! This module is unconditionally compiled on all targets (native and wasm32)
//! so that both the native codec pipeline and the WASM `render_markdown` endpoint
//! use an identical, single-source option set.
//!
//! **Why a separate file?**
//! `codec/md.rs` is gated `#[cfg(not(target_arch = "wasm32"))]` because it
//! depends on `std::fs`, `std::io`, `path::Path`, and the full codec/builder
//! stack — none of which are available in the WASM sandbox. The options
//! function itself only needs `pulldown_cmark::Options`, which is a plain
//! bitflag type with no platform dependencies. Isolating it here keeps the
//! WASM binary free of native-only code while eliminating the duplication that
//! would otherwise arise from inlining the flags in `wasm.rs`.
//!
//! **Maintenance rule:** any change to the parser extension set must be made
//! here and here only. `md.rs` and `wasm.rs` both import from this module;
//! neither maintains its own copy.

use pulldown_cmark::{BrokenLink, Options, Parser};

/// Returns the pulldown-cmark [`Options`] used throughout the Buildonomy
/// compilation pipeline.
///
/// This is the canonical, single-source definition of which Markdown extensions
/// are enabled. It is used by:
///
/// - The native codec (`codec/md.rs`) when parsing and rendering documents.
/// - The WASM `BeliefBaseWasm::render_markdown` endpoint when rendering
///   `payload.text` for the metadata panel preview.
///
/// # Extension rationale
///
/// | Option | Reason enabled |
/// |---|---|
/// | `ENABLE_DEFINITION_LIST` | Used in spec/requirements documents |
/// | `ENABLE_FOOTNOTES` | Used in long-form content |
/// | `ENABLE_GFM` | GitHub-Flavored Markdown baseline (autolinks, strikethrough table syntax, etc.) |
/// | `ENABLE_HEADING_ATTRIBUTES` | `{#id}` syntax used for BID injection |
/// | `ENABLE_MATH` | KaTeX/MathJax blocks in technical docs |
/// | `ENABLE_STRIKETHROUGH` | Common inline annotation |
/// | `ENABLE_SUBSCRIPT` | Chemical/mathematical notation |
/// | `ENABLE_SUPERSCRIPT` | Same |
/// | `ENABLE_TABLES` | GFM tables |
/// | `ENABLE_TASKLISTS` | GFM task lists |
/// | `ENABLE_WIKILINKS` | `[[wikilink]]` syntax for internal cross-references |
/// | `ENABLE_YAML_STYLE_METADATA_BLOCKS` | Front-matter blocks (silently dropped on HTML render) |
///
/// # Security
///
/// `ENABLE_HTML` is intentionally absent. Raw HTML passthrough is suppressed,
/// making rendered output safe for direct `innerHTML` injection without an
/// additional sanitizer or iframe sandbox.
pub fn buildonomy_md_options() -> Options {
    let mut opts = Options::empty();
    // Enabled explicitly rather than via Options::all() for reproducibility —
    // new upstream flags will not silently activate.
    opts.insert(Options::ENABLE_DEFINITION_LIST);
    opts.insert(Options::ENABLE_FOOTNOTES);
    opts.insert(Options::ENABLE_GFM);
    opts.insert(Options::ENABLE_HEADING_ATTRIBUTES);
    opts.insert(Options::ENABLE_MATH);
    // Intentionally omitted:
    //   Options::ENABLE_OLD_FOOTNOTES          — superseded by ENABLE_FOOTNOTES
    //   Options::ENABLE_PLUSES_DELIMITED_METADATA_BLOCKS — not used
    //   Options::ENABLE_SMART_PUNCTUATION      — not used
    //   Options::ENABLE_HTML                   — security: raw HTML suppressed
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_SUBSCRIPT);
    opts.insert(Options::ENABLE_SUPERSCRIPT);
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_TASKLISTS);
    opts.insert(Options::ENABLE_WIKILINKS);
    opts.insert(Options::ENABLE_YAML_STYLE_METADATA_BLOCKS);
    opts
}

/// Render a markdown snippet to HTML using the canonical Buildonomy parser options.
///
/// Handles broken `[reference]` links by treating the reference text as both the
/// URL and title, producing clickable `<a>` tags that the SPA router can resolve.
///
/// This is the shared rendering function used by:
/// - `DocumentCompiler::render_markdown_snippet` (compile-time deferred HTML)
/// - `TableView::render_depth0_list` (query result text rendering)
/// - `BeliefBaseWasm::render_markdown` (browser-side metadata panel)
pub fn render_markdown_snippet(md: &str) -> String {
    let mut html = String::new();
    let parser = Parser::new_with_broken_link_callback(
        md,
        buildonomy_md_options(),
        Some(|link: BrokenLink<'_>| {
            let reference = link.reference.into_static();
            Some((reference.clone(), reference))
        }),
    );
    pulldown_cmark::html::push_html(&mut html, parser);
    html
}
