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
//! ## Codec Registration
//!
//! Codecs can be registered by **file extension** or **file stem** (filename):
//!
//! - **By extension**: `(None, Some("md"))` - matches all `.md` files
//! - **By stem**: `(Some(".noet"), None)` - matches files named `.noet` (regardless of location)
//! - **By directory**: `(Some("docs"), None)` - can match directory names (if AnchorPath treats them as directories)
//!
//! This flexible registration enables:
//! - Single files: `.noet` (BeliefNetwork metadata)
//! - File patterns: `.md`, `.toml`, `.json`
//! - Directory structures: `.github/`, `node_modules/` (when treated as units)
//!
//! ## Built-in Codecs
//!
//! - **Markdown** (`.md`) - via [`md::MdCodec`]
//! - **BeliefNetwork** (`.noet`) - via [`belief_ir::ProtoBeliefNode`]
//!
//! The `.noet` file uses stem-based registration to avoid conflicts with generic `.yml`, `.json`, or `.toml` files
//! in repositories. It's a hidden file that can contain YAML, JSON, or TOML format (auto-detected via fallback parsing).
//!
//! Register custom codecs via [`CodecMap::insert`] (by extension) or [`CodecMap::insert_codec`] (by stem/extension):
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
//! // Register by extension (simple API)
//! CODECS.insert("myext".to_string(), || Box::new(MyCustomCodec));
//!
//! // Register by stem (advanced API)
//! CODECS.insert_codec(Some(".myfile".to_string()), None, || Box::new(MyCustomCodec));
//!
//! // Register by both stem and extension
//! CODECS.insert_codec(Some("config".to_string()), Some("toml".to_string()), || Box::new(MyCustomCodec));
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
use std::{result::Result, sync::Arc, time::Duration};

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
/// Factory function type for creating fresh codec instances.
///
/// Each invocation creates a new, independent codec instance to avoid state pollution
/// between document parses.
pub type CodecFactory = fn() -> Box<dyn DocCodec + Send>;

/// Global codec map - creates fresh instances on demand via factory pattern.
///
/// Access via `CODECS` to register or retrieve codecs. Supports registration by:
/// - File extension: `CODECS.insert("md".to_string(), factory)`
/// - File stem: `CODECS.insert_codec(Some(".noet".to_string()), None, factory)`
/// - Both: `CODECS.insert_codec(Some("config".to_string()), Some("toml".to_string()), factory)`
#[cfg(not(target_arch = "wasm32"))]
pub static CODECS: Lazy<CodecMap> = Lazy::new(CodecMap::create);

/// Global codec map for WASM - lightweight extension registry only.
#[cfg(target_arch = "wasm32")]
pub static CODECS: Lazy<CodecMap> = Lazy::new(CodecMap::create);

/// List of built-in codec extensions (synchronized between WASM and non-WASM builds).
const BUILTIN_EXTENSIONS: &[&str] = &["md"];

/// Codec registration entry: (optional_stem, optional_extension, factory).
///
/// At least one of stem or extension must be Some.
///
/// # Examples
/// - `(None, Some("md"), factory)` - Match all `.md` files
/// - `(Some(".noet"), None, factory)` - Match files named `.noet`
/// - `(Some("config"), Some("toml"), factory)` - Match `config.toml` files
type CodecEntry = (Option<String>, Option<String>, CodecFactory);

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
    /// - `Ok(vec![(filename, body), ...])`: Output filenames and HTML body content
    /// - `Ok(vec![])`: No immediate generation (may use deferred instead if should_defer == true)
    /// - `Err(_)`: Generation failed
    ///
    /// # Filename Format
    /// Return output filename only (not full path):
    /// - `"guide.html"` → written to source file's directory
    /// - `"subdir/index.html"` → creates subdir/ relative to source file's directory
    /// - Compiler handles directory resolution based on source file location
    ///
    /// For source file `/repo/docs/page.md`, returning `"page.html"` writes to
    /// `html_output/pages/docs/page.html` with public URL `/docs/page.html`.
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
    fn generate_html(&self) -> Result<Vec<(String, String)>, BuildonomyError> {
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
    /// Same format as `generate_html()` - output filenames and HTML body content.
    /// Filenames are resolved relative to the source file's directory (from ctx.path).
    ///
    /// # Example: Network Index
    /// ```ignore
    /// fn generate_deferred_html(&self, ctx: &BeliefContext) -> Result<Vec<(String, String)>, BuildonomyError> {
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
    ///     Ok(vec![("index.html".to_string(), html)])
    /// }
    /// ```
    ///
    /// Default implementation returns empty vec (no deferred generation).
    fn generate_deferred_html(
        &self,
        _ctx: &BeliefContext<'_>,
    ) -> Result<Vec<(String, String)>, BuildonomyError> {
        Ok(vec![])
    }
}

/// Factory-based codec map that creates fresh instances on demand
#[cfg(not(target_arch = "wasm32"))]
pub struct CodecMap(Arc<RwLock<Vec<CodecEntry>>>);

#[cfg(not(target_arch = "wasm32"))]
impl Clone for CodecMap {
    fn clone(&self) -> Self {
        CodecMap(self.0.clone())
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl CodecMap {
    /// Check if a codec exists for the given filesystem path.
    ///
    /// Extracts the filestem and extension from the `Path` and checks if any codec
    /// is registered for either component.
    ///
    /// # Example
    /// ```
    /// use std::path::Path;
    /// use noet_core::codec::CODECS;
    ///
    /// let path = Path::new("/tmp/document.md");
    /// assert!(CODECS.has_codec_for_path(&path)); // true for .md extension
    ///
    /// let path = Path::new("/tmp/.noet");
    /// assert!(CODECS.has_codec_for_path(&path)); // true for .noet stem
    /// ```
    pub fn has_codec_for_path(&self, path: &std::path::Path) -> bool {
        let filestem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        self.get(filestem, ext).is_some()
    }

    /// Check if a codec exists for the given `AnchorPath`.
    ///
    /// Uses `codec_parts()` to extract the appropriate stem/extension for codec matching,
    /// handling special cases like extensionless files (`.noet`) which AnchorPath may
    /// treat as directories.
    ///
    /// # Example
    /// ```
    /// use noet_core::codec::CODECS;
    /// use noet_core::paths::path::AnchorPath;
    ///
    /// let ap = AnchorPath::new("docs/README.md");
    /// assert!(CODECS.has_codec_for_anchor_path(&ap)); // true for .md extension
    ///
    /// let ap = AnchorPath::new("/tmp/.noet");
    /// assert!(CODECS.has_codec_for_anchor_path(&ap)); // true for .noet stem
    /// ```
    pub fn has_codec_for_anchor_path(&self, path: &crate::paths::path::AnchorPath) -> bool {
        let (filestem, ext) = path.codec_parts();
        self.get(filestem, ext).is_some()
    }

    /// Create a new `CodecMap` with built-in codecs registered.
    ///
    /// Built-in codecs:
    /// - Markdown: registered by extension `.md`
    /// - BeliefNetwork: registered by stem `.noet`
    pub fn create() -> Self {
        let map = CodecMap(Arc::new(RwLock::new(vec![
            // Markdown files by extension
            (
                None,
                Some("md".to_string()),
                || Box::new(md::MdCodec::new()),
            ),
            // BeliefNetwork files by stem (hidden file .noet)
            (Some(".noet".to_string()), None, || {
                Box::new(ProtoBeliefNode::default())
            }),
        ])));
        debug_assert!({
            BUILTIN_EXTENSIONS
                .iter()
                .all(|ext| map.get_by_extension(ext).is_some())
        });
        map
    }

    /// Insert a codec by extension (simple API).
    ///
    /// This is a convenience method that calls `insert_codec(None, Some(extension), factory)`.
    ///
    /// # Example
    /// ```
    /// use noet_core::codec::{CODECS, ProtoBeliefNode};
    ///
    /// CODECS.insert("org".to_string(), || Box::new(ProtoBeliefNode::default()));
    /// ```
    pub fn insert(&self, extension: String, factory: CodecFactory) {
        self.insert_codec(None, Some(extension), factory);
    }

    /// Insert a codec with optional stem and extension (advanced API).
    ///
    /// At least one of `stem` or `extension` must be `Some`. This method enables:
    /// - Registration by extension: `insert_codec(None, Some("md"), factory)`
    /// - Registration by stem: `insert_codec(Some(".noet"), None, factory)`
    /// - Registration by both: `insert_codec(Some("config"), Some("toml"), factory)`
    ///
    /// # Panics
    /// Panics if both `stem` and `extension` are `None`.
    ///
    /// # Example
    /// ```
    /// use noet_core::codec::{CODECS, ProtoBeliefNode};
    ///
    /// // Match files named .myconfig (regardless of extension)
    /// CODECS.insert_codec(Some(".myconfig".to_string()), None, || Box::new(ProtoBeliefNode::default()));
    ///
    /// // Match config.toml files specifically
    /// CODECS.insert_codec(Some("config".to_string()), Some("toml".to_string()), || Box::new(ProtoBeliefNode::default()));
    /// ```
    pub fn insert_codec(
        &self,
        stem: Option<String>,
        extension: Option<String>,
        factory: CodecFactory,
    ) {
        assert!(
            stem.is_some() || extension.is_some(),
            "At least one of stem or extension must be Some"
        );

        while self.0.is_locked() {
            tracing::debug!("[CodecMap::insert_codec] Waiting for write access");
            std::thread::sleep(Duration::from_millis(100));
        }
        let mut writer = self.0.write_arc();

        // Find existing entry that matches both stem and extension
        if let Some(entry) = writer
            .iter_mut()
            .find(|(s, e, _)| s == &stem && e == &extension)
        {
            entry.2 = factory;
        } else {
            writer.push((stem, extension, factory));
        }
    }

    /// Get codec factory by filestem and extension.
    ///
    /// Returns a codec factory if any registered codec matches the given stem OR extension.
    /// This is the core lookup method used by `has_codec_for_path` and `has_codec_for_anchor_path`.
    ///
    /// # Example
    /// ```
    /// use noet_core::codec::CODECS;
    ///
    /// // Match by extension
    /// let factory = CODECS.get("README", "md");
    /// assert!(factory.is_some());
    ///
    /// // Match by stem
    /// let factory = CODECS.get(".noet", "");
    /// assert!(factory.is_some());
    ///
    /// // No match
    /// let factory = CODECS.get("unknown", "xyz");
    /// assert!(factory.is_none());
    /// ```
    pub fn get(&self, filestem: &str, ext: &str) -> Option<CodecFactory> {
        while self.0.is_locked_exclusive() {
            tracing::debug!("[CodecMap::get] Waiting for read access");
            std::thread::sleep(Duration::from_millis(100));
        }
        let reader = self.0.read_arc();
        reader
            .iter()
            .find(|(codec_stem, codec_ext, _)| {
                // Match if stem matches (when codec has a stem registered)
                let stem_matches = codec_stem.as_ref().is_some_and(|s| s == filestem);
                // Match if extension matches (when codec has an extension registered)
                let ext_matches = codec_ext.as_ref().is_some_and(|e| e == ext);
                stem_matches || ext_matches
            })
            .map(|(_, _, factory)| *factory)
    }

    /// Get codec factory by extension only (simple API).
    ///
    /// This is a convenience method for retrieving codecs registered by extension.
    /// For stem-based lookups, use `get()` instead.
    pub fn get_by_extension(&self, ext: &str) -> Option<CodecFactory> {
        while self.0.is_locked_exclusive() {
            tracing::debug!("[CodecMap::get_by_extension] Waiting for read access");
            std::thread::sleep(Duration::from_millis(100));
        }
        let reader = self.0.read_arc();
        reader
            .iter()
            .find(|(_, codec_ext, _)| codec_ext.as_ref().is_some_and(|e| e == ext))
            .map(|(_, _, factory)| *factory)
    }

    /// Get all registered extensions.
    ///
    /// Returns only the extensions from registered codecs (not stems).
    /// This is used for backward compatibility with code that expects extension lists.
    pub fn extensions(&self) -> Vec<String> {
        while self.0.is_locked_exclusive() {
            tracing::debug!("[CodecMap::extensions] Waiting for read access");
            std::thread::sleep(Duration::from_millis(100));
        }
        let reader = self.0.read_arc();
        reader
            .iter()
            .filter_map(|(_, codec_ext, _)| codec_ext.clone())
            .collect()
    }

    /// Get all registered patterns (stems and extensions) for debugging.
    ///
    /// Returns a vector of tuples `(Option<stem>, Option<extension>)` for all registered codecs.
    /// Useful for debugging or introspection.
    ///
    /// # Example
    /// ```
    /// use noet_core::codec::CODECS;
    ///
    /// let patterns = CODECS.registered_patterns();
    /// for (stem, ext) in patterns {
    ///     println!("Codec registered: stem={:?}, ext={:?}", stem, ext);
    /// }
    /// ```
    pub fn registered_patterns(&self) -> Vec<(Option<String>, Option<String>)> {
        while self.0.is_locked_exclusive() {
            tracing::debug!("[CodecMap::registered_patterns] Waiting for read access");
            std::thread::sleep(Duration::from_millis(100));
        }
        let reader = self.0.read_arc();
        reader
            .iter()
            .map(|(stem, ext, _)| (stem.clone(), ext.clone()))
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
        let factory = CODECS.get("README", "md").expect("md codec should exist");

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

        // Verify built-in markdown codec is registered
        assert!(extensions.contains(&"md".to_string()));
    }

    #[test]
    fn test_codec_factory_stems() {
        let patterns = CODECS.registered_patterns();

        // Verify BeliefNetwork codec is registered by stem
        assert!(patterns
            .iter()
            .any(|(stem, _)| stem.as_ref().is_some_and(|s| s == ".noet")));

        // Verify markdown codec is registered by extension
        assert!(patterns
            .iter()
            .any(|(_, ext)| ext.as_ref().is_some_and(|e| e == "md")));
    }

    #[test]
    fn test_codec_factory_get_nonexistent() {
        let result = CODECS.get("nonexistent", "xyz");
        assert!(result.is_none());
    }

    #[test]
    fn test_codec_factory_get_by_stem() {
        // Test that .noet filestem matches BeliefNetwork codec
        let result = CODECS.get(".noet", "");
        assert!(result.is_some());
    }

    #[test]
    fn test_codec_factory_get_by_extension() {
        // Test that .md extension matches markdown codec
        let result = CODECS.get("README", "md");
        assert!(result.is_some());
    }

    #[test]
    fn test_wasm_extensions_match_builtin() {
        // Verify WASM build would have same extensions as non-WASM
        let extensions = CODECS.extensions();
        for builtin in BUILTIN_EXTENSIONS {
            assert!(
                extensions.contains(&builtin.to_string()),
                "Missing builtin extension: {}",
                builtin
            );
        }
    }

    #[test]
    fn test_codec_insert_with_stem() {
        let codecs = CodecMap::create();

        // Insert a custom codec by stem
        codecs.insert_codec(Some("custom".to_string()), None, || {
            Box::new(ProtoBeliefNode::default())
        });

        // Verify it can be retrieved
        let result = codecs.get("custom", "");
        assert!(result.is_some());
    }

    #[test]
    fn test_noet_path_with_full_path() {
        use crate::paths::path::AnchorPath;

        // Test with full path
        let ap = AnchorPath::new("/tmp/.tmpm0D4CB/.noet");
        let (stem, ext) = ap.codec_parts();

        assert_eq!(stem, ".noet", "Codec stem should be .noet");
        assert_eq!(ext, "", "Extension should be empty");

        // Verify codec lookup works
        let result = CODECS.get(stem, ext);
        assert!(result.is_some(), "Should find codec for .noet filestem");
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

        let (output_filename, html_body) = &fragments[0];
        assert!(
            output_filename.ends_with(".html"),
            "Output filename should end with .html, got: '{}'",
            output_filename
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
