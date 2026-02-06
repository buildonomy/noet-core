//! Document parsing and integration into BeliefBases.
//!
//! This module provides the core parsing infrastructure for converting source documents
//! (Markdown, TOML, etc.) into [`BeliefBase`](crate::beliefbase::BeliefBase) graphs.
//!
//! ## Key Components
//!
//! - [`GraphBuilder`] - Stateful BeliefBase builder that integrates documents into belief networks
//! - [`DocumentCompiler`] - Orchestrates multi-pass compilation across multiple files
//! - [`DocCodec`] trait - Implement custom document parsers for new file formats
//! - [`CodecMap`] - Global registry of available codecs (accessible via [`CODECS`])
//! - [`SchemaRegistry`](schema_registry::SchemaRegistry) - Global registry of schema definitions (accessible via [`SCHEMAS`])
//! - [`ParseDiagnostic`] - Tracks unresolved references and parsing issues
//!
//! ## Multi-Pass Compilation
//!
//! The compiler handles forward references through multi-pass resolution:
//!
//! 1. **First Pass**: Parse all files, collect unresolved references
//! 2. **Resolution Passes**: Reparse files once their dependencies are available
//! 3. **Convergence**: Iterate until all references resolve or reach max iterations
//!
//! Unresolved references are tracked via [`ParseDiagnostic::UnresolvedReference`] and
//! drive the reparse queue.
//!
//! ## Link Rewriting
//!
//! The builder automatically rewrites links in source documents to maintain consistency:
//!
//! - Injects BIDs (Belief IDs) into documents that lack them
//! - Updates link text when reference titles change
//! - Maintains bi-directional reference tracking
//!
//! The private method `GraphBuilder::cache_fetch` contains the identity resolution details.
//!
//! ## Built-in Codecs
//!
//! - **Markdown** (`.md`) - via [`md::MdCodec`]
//! - **TOML** (`.toml`) - via [`belief_ir::ProtoBeliefNode`]
//!
//! Register custom codecs via [`CodecMap::insert`]:
//!
//! ```rust
//! use noet_core::{beliefbase::BeliefContext, BuildonomyError, codec::{CODECS, DocCodec, ProtoBeliefNode}, properties::BeliefNode};
//!
//! #[derive(Default, Clone)]
//! struct MyCustomCodec;
//!
//! impl DocCodec for MyCustomCodec {
//!     fn parse(
//!         &mut self,
//!         // The source content to be parsed by the DocCodec implementation
//!         content: String,
//!         // Contains the builder root-path relative information to seed the parse with
//!         current: ProtoBeliefNode,
//!     ) -> Result<(), BuildonomyError> {
//!         todo!();
//!     }
//!
//!     fn nodes(&self) -> Vec<ProtoBeliefNode> {
//!         todo!();
//!     }
//!
//!     fn inject_context(
//!         &mut self,
//!         node: &ProtoBeliefNode,
//!         ctx: &BeliefContext<'_>,
//!     ) -> Result<Option<BeliefNode>, BuildonomyError> {
//!         todo!();
//!     }
//!
//!     fn generate_source(&self) -> Option<String> {
//!         todo!();
//!     }
//! }
//! CODECS.insert("myext".to_string(), || Box::new(MyCustomCodec));
//! ```
//!
//! ## Schema Registration
//!
//! Schemas define how TOML fields map to graph edges. Register custom schemas via [`SCHEMAS`]:
//!
//! ```rust
//! use noet_core::codec::{SCHEMAS, schema_registry::{SchemaDefinition, GraphField, EdgeDirection}};
//! use noet_core::properties::WeightKind;
//!
//! SCHEMAS.register(
//!     "my_app.task".to_string(),
//!     SchemaDefinition {
//!         graph_fields: vec![GraphField {
//!             field_name: "dependencies",
//!             direction: EdgeDirection::Downstream,
//!             weight_kind: WeightKind::Pragmatic,
//!             required: false,
//!             payload_fields: vec!["notes"],
//!         }],
//!     },
//! );
//! ```
//!
//! ## Architecture Details
//!
//! For detailed information about the parsing architecture, including:
//! - The "three sources of truth" (parsed document, local cache, global cache)
//! - Two-cache architecture (`self.doc_bb` vs `session_bb`)
//! - Link resolution protocol and relative path handling
//!
//! See `docs/design/beliefbase_architecture.md` (Section 3.2: The Codec System).
//!

use once_cell::sync::Lazy;

#[cfg(not(target_arch = "wasm32"))]
pub use assets::Layout;

#[cfg(not(target_arch = "wasm32"))]
use parking_lot::RwLock;
#[cfg(not(target_arch = "wasm32"))]
use std::{path::PathBuf, result::Result, sync::Arc, time::Duration};

#[cfg(not(target_arch = "wasm32"))]
use crate::{beliefbase::BeliefContext, error::BuildonomyError, properties::BeliefNode};

#[cfg(not(target_arch = "wasm32"))]
pub mod assets;
#[cfg(not(target_arch = "wasm32"))]
pub mod belief_ir;
#[cfg(not(target_arch = "wasm32"))]
pub mod builder;
#[cfg(not(target_arch = "wasm32"))]
pub mod compiler;
#[cfg(not(target_arch = "wasm32"))]
pub mod diagnostic;
#[cfg(not(target_arch = "wasm32"))]
pub mod md;
#[cfg(not(target_arch = "wasm32"))]
pub mod schema_registry;

// Re-export for backward compatibility
#[cfg(not(target_arch = "wasm32"))]
pub use belief_ir::ProtoBeliefNode;
#[cfg(not(target_arch = "wasm32"))]
pub use builder::GraphBuilder;
#[cfg(not(target_arch = "wasm32"))]
pub use compiler::DocumentCompiler;
#[cfg(not(target_arch = "wasm32"))]
pub use diagnostic::{ParseDiagnostic, UnresolvedReference};
#[cfg(not(target_arch = "wasm32"))]
pub use schema_registry::SCHEMAS;

/// Factory function type for creating fresh codec instances
#[cfg(not(target_arch = "wasm32"))]
pub type CodecFactory = fn() -> Box<dyn DocCodec + Send>;

/// Global codec map - creates fresh instances on demand via factory pattern
#[cfg(not(target_arch = "wasm32"))]
pub static CODECS: Lazy<CodecMap> = Lazy::new(CodecMap::create);

/// Global codec map for WASM - lightweight extension registry only
#[cfg(target_arch = "wasm32")]
pub static CODECS: Lazy<CodecMap> = Lazy::new(CodecMap::create);

/// List of built-in codec extensions (synchronized between WASM and non-WASM builds)
const BUILTIN_EXTENSIONS: &[&str] = &["md", "toml", "tml", "json", "jsn", "yaml", "yml"];

/// [ ] Need to iterate out protobeliefstate
/// [ ] Need to replace protobeliefstates
/// [ ] Need to write doc to buffer
/// [ ] Be able to publish markdown snippets -- with or without: anchors, revised src/hrefs, widget
///     configuration toml
#[cfg(not(target_arch = "wasm32"))]
pub trait DocCodec: Sync {
    fn parse(
        &mut self,
        // The source content to be parsed by the DocCodec implementation
        content: String,
        // Contains the builder root-path relative information to seed the parse with
        current: ProtoBeliefNode,
    ) -> Result<(), BuildonomyError>;

    fn nodes(&self) -> Vec<ProtoBeliefNode>;

    fn inject_context(
        &mut self,
        node: &ProtoBeliefNode,
        ctx: &BeliefContext<'_>,
    ) -> Result<Option<BeliefNode>, BuildonomyError>;

    /// Called after all inject_context() calls complete, allowing the codec to:
    /// - Perform cross-node cleanup (e.g., track unmatched sections)
    /// - Emit events for nodes modified during finalization
    /// - Log diagnostics for unmatched metadata
    ///
    /// Returns a vector of (ProtoBeliefNode, BeliefNode) pairs for nodes that were modified
    /// during finalization and need NodeUpdate events emitted.
    fn finalize(&mut self) -> Result<Vec<(ProtoBeliefNode, BeliefNode)>, BuildonomyError> {
        // Default implementation: no finalization needed
        Ok(Vec::new())
    }

    fn generate_source(&self) -> Option<String>;

    /// Signal whether this codec needs deferred generation.
    ///
    /// If true, compiler will call `generate_html()` again after all parsing completes
    /// with full BeliefContext available.
    ///
    /// # Returns
    /// - `true`: Needs full context, call generate_html() again after all files parsed
    /// - `false`: Only immediate generation needed (default)
    ///
    /// # Examples
    /// - Markdown files: `false` (can generate from parsed AST immediately)
    /// - Network indices: `true` (need to query child documents from context)
    fn should_defer(&self) -> bool {
        false // Default: no deferral needed
    }

    /// Generate HTML fragments from parsed content (immediate phase).
    ///
    /// Called immediately after parsing completes, before BeliefContext is available.
    /// Use for codecs that can generate HTML from parsed AST alone (e.g., Markdown).
    ///
    /// # Returns
    /// - `Ok(vec![(path, body), ...])`: Repo-relative output paths and HTML body content
    /// - `Ok(vec![])`: No immediate generation (may use deferred instead if should_defer == true)
    /// - `Err(_)`: Generation failed
    ///
    /// # Path Format
    /// Return repo-relative paths where `path.is_file() == true`:
    /// - `PathBuf::from("docs/guide.html")` → written to `html_output/pages/docs/guide.html`
    /// - Public URL will be `/docs/guide.html`
    ///
    /// # Body Content
    /// Return HTML body content only (no `<html>`, `<head>`, etc.):
    /// - Compiler wraps with Layout::Simple template
    /// - Template adds canonical URL and optional script injection
    ///
    /// # Link Normalization
    /// **Implementations MUST normalize document links to `.html` extension:**
    /// - Convert all registered codec extensions (`.md`, `.toml`, `.org`, etc.) to `.html`
    /// - Preserve anchors: `.md#section` → `.html#section`
    /// - Use `CODECS.extensions()` to get the list of registered extensions
    ///
    /// Default implementation returns empty vec (no HTML generation).
    fn generate_html(&self) -> Result<Vec<(PathBuf, String)>, BuildonomyError> {
        Ok(vec![])
    }

    /// Generate HTML fragments with full BeliefContext (deferred phase).
    ///
    /// Called after all parsing completes, with full context available.
    /// Use for codecs that need to query relationships (e.g., network indices listing children).
    ///
    /// Only called if `should_defer()` returns `true`.
    ///
    /// # Parameters
    /// - `ctx`: BeliefContext with full graph relationships and metadata
    ///
    /// # Returns
    /// Same format as `generate_html()` - repo-relative paths and HTML body content.
    ///
    /// # Example: Network Index
    /// ```ignore
    /// fn generate_deferred_html(&self, ctx: &BeliefContext) -> Result<Vec<(PathBuf, String)>, BuildonomyError> {
    ///     // Query child documents via Subsection edges
    ///     let mut children: Vec<_> = ctx.sources.iter()
    ///         .filter(|edge| edge.weight.get(WeightKind::Subsection).is_some())
    ///         .collect();
    ///
    ///     // Sort by WEIGHT_SORT_KEY
    ///     children.sort_by_key(|edge| {
    ///         edge.weight.get(WeightKind::Subsection)
    ///             .and_then(|w| w.get("sort"))
    ///             .and_then(|v| v.as_integer())
    ///     });
    ///
    ///     // Generate HTML list
    ///     let html = format!("<ul>{}</ul>",
    ///         children.iter()
    ///             .map(|edge| format!("<li><a href='{}'>{}</a></li>",
    ///                 edge.other_path, edge.other.display_title()))
    ///             .collect::<String>()
    ///     );
    ///
    ///     Ok(vec![(self.path.with_extension("html"), html)])
    /// }
    /// ```
    ///
    /// Default implementation returns empty vec (no deferred generation).
    fn generate_deferred_html(
        &self,
        _ctx: &BeliefContext<'_>,
    ) -> Result<Vec<(PathBuf, String)>, BuildonomyError> {
        Ok(vec![])
    }
}

/// Factory-based codec map that creates fresh instances on demand
#[cfg(not(target_arch = "wasm32"))]
pub struct CodecMap(Arc<RwLock<Vec<(String, CodecFactory)>>>);

#[cfg(not(target_arch = "wasm32"))]
impl Clone for CodecMap {
    fn clone(&self) -> Self {
        CodecMap(self.0.clone())
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl CodecMap {
    pub fn create() -> Self {
        let map = CodecMap(Arc::new(RwLock::new(vec![
            ("md".to_string(), || Box::new(md::MdCodec::new())),
            ("toml".to_string(), || Box::new(ProtoBeliefNode::default())),
            ("tml".to_string(), || Box::new(ProtoBeliefNode::default())),
            ("json".to_string(), || Box::new(ProtoBeliefNode::default())),
            ("jsn".to_string(), || Box::new(ProtoBeliefNode::default())),
            ("yaml".to_string(), || Box::new(ProtoBeliefNode::default())),
            ("yml".to_string(), || Box::new(ProtoBeliefNode::default())),
        ])));
        debug_assert!({ BUILTIN_EXTENSIONS.iter().all(|ext| map.get(ext).is_some()) });
        map
    }

    pub fn insert(&self, extension: String, factory: CodecFactory) {
        while self.0.is_locked() {
            tracing::debug!("[CodecMap::insert] Waiting for write access");
            std::thread::sleep(Duration::from_millis(100));
        }
        let mut writer = self.0.write_arc();
        if let Some(entry) = writer.iter_mut().find(|(ext, _)| ext == &extension) {
            entry.1 = factory;
        } else {
            writer.push((extension, factory));
        }
    }

    pub fn get(&self, ext: &str) -> Option<CodecFactory> {
        while self.0.is_locked_exclusive() {
            tracing::debug!("[CodecMap::get] Waiting for read access");
            std::thread::sleep(Duration::from_millis(100));
        }
        let reader = self.0.read_arc();
        reader
            .iter()
            .find(|(codec_ext, _)| ext == codec_ext)
            .map(|(_, factory)| *factory)
    }

    pub fn extensions(&self) -> Vec<String> {
        while self.0.is_locked_exclusive() {
            tracing::debug!("[CodecMap::extensions] Waiting for read access");
            std::thread::sleep(Duration::from_millis(100));
        }
        let reader = self.0.read_arc();
        reader
            .iter()
            .map(|(codec_ext, _)| codec_ext.clone())
            .collect()
    }
}

// WASM-compatible version: lightweight extension registry only (no actual codec instances)
#[cfg(target_arch = "wasm32")]
pub struct CodecMap;

#[cfg(target_arch = "wasm32")]
impl Clone for CodecMap {
    fn clone(&self) -> Self {
        CodecMap
    }
}

#[cfg(target_arch = "wasm32")]
impl CodecMap {
    pub fn create() -> Self {
        CodecMap
    }

    pub fn extensions(&self) -> Vec<String> {
        BUILTIN_EXTENSIONS.iter().map(|s| s.to_string()).collect()
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    #[test]
    fn test_codec_factory_creates_fresh_instances() {
        // Get codec factory for markdown
        let factory = CODECS.get("md").expect("md codec should exist");

        // Create two instances
        let codec1 = factory();
        let codec2 = factory();

        // Verify they are separate instances (different addresses)
        let ptr1 = &*codec1 as *const dyn DocCodec;
        let ptr2 = &*codec2 as *const dyn DocCodec;

        assert_ne!(ptr1, ptr2, "Factory should create separate instances");
    }

    #[test]
    fn test_codec_factory_extensions() {
        let extensions = CODECS.extensions();

        // Verify built-in codecs are registered
        assert!(extensions.contains(&"md".to_string()));
        assert!(extensions.contains(&"toml".to_string()));
        assert!(extensions.contains(&"json".to_string()));
        assert!(extensions.contains(&"yaml".to_string()));
    }

    #[test]
    fn test_codec_factory_get_nonexistent() {
        let result = CODECS.get("nonexistent");
        assert!(result.is_none());
    }

    #[test]
    fn test_wasm_extensions_match_builtin() {
        // Verify WASM build would have same extensions as non-WASM
        let extensions = CODECS.extensions();
        for builtin in BUILTIN_EXTENSIONS {
            assert!(
                extensions.contains(&builtin.to_string()),
                "Extension {} should be in CODECS",
                builtin
            );
        }
    }

    #[tokio::test]
    async fn test_parse_content_returns_owned_codec() {
        use crate::codec::builder::GraphBuilder;
        use tempfile::TempDir;

        // Create temporary directory with a test markdown file
        let temp_dir = TempDir::new().unwrap();
        let test_file = temp_dir.path().join("test.md");
        let content = "# Test Document\n\nThis is a test.";
        std::fs::write(&test_file, content).unwrap();

        // Create builder with directory as root
        let mut builder = GraphBuilder::new(temp_dir.path(), None).unwrap();

        // Parse with factory method - should return owned codec
        let session_bb = builder.session_bb().clone();
        let result = builder
            .parse_content(&test_file, content.to_string(), session_bb)
            .await;

        assert!(result.is_ok(), "parse_content should succeed");
        let with_codec = result.unwrap();
        let parse_result = with_codec.result;
        let codec = with_codec.codec;

        // Verify parse result
        assert!(
            parse_result.diagnostics.is_empty()
                || !parse_result
                    .diagnostics
                    .iter()
                    .any(|d| matches!(d, crate::codec::ParseDiagnostic::ParseError { .. }))
        );

        // Verify codec has parsed content
        assert!(!codec.nodes().is_empty(), "Codec should have parsed nodes");
    }

    #[tokio::test]
    async fn test_dual_phase_html_generation() {
        use crate::codec::builder::GraphBuilder;
        use tempfile::TempDir;

        // Create temporary directory with a test markdown file
        let temp_dir = TempDir::new().unwrap();
        let test_file = temp_dir.path().join("test.md");
        let content = "# Test Document\n\nThis is a test with a [link](other.md).";
        std::fs::write(&test_file, content).unwrap();

        // Create builder with directory as root
        let mut builder = GraphBuilder::new(temp_dir.path(), None).unwrap();

        // Parse with factory method - should return owned codec
        let session_bb = builder.session_bb().clone();
        let result = builder
            .parse_content(&test_file, content.to_string(), session_bb)
            .await;

        assert!(result.is_ok(), "parse_content should succeed");
        let with_codec = result.unwrap();
        let codec = with_codec.codec;

        // Test Phase 1: Immediate generation
        let immediate_result = codec.generate_html();
        assert!(
            immediate_result.is_ok(),
            "generate_html should succeed: {:?}",
            immediate_result.as_ref().err()
        );

        let fragments = immediate_result.unwrap();
        assert_eq!(fragments.len(), 1, "Should generate one fragment");

        let (output_path, html_body) = &fragments[0];
        assert_eq!(
            output_path.extension().and_then(|s| s.to_str()),
            Some("html")
        );
        assert!(
            html_body.contains("Test Document"),
            "Should contain document title"
        );
        assert!(
            html_body.contains("other.md"),
            "Unresolved links remain as-is (link rewriting only for resolved references)"
        );

        // Test deferral signal (markdown doesn't need deferral)
        assert!(!codec.should_defer(), "Markdown should not need deferral");
    }
}
