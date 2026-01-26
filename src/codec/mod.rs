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
//! CODECS.insert::<MyCustomCodec>("myext".to_string());
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
use parking_lot::{Mutex, RwLock};
/// Utilities for parsing various document types into BeliefBases
use std::{result::Result, sync::Arc, time::Duration};

use crate::{beliefbase::BeliefContext, error::BuildonomyError, properties::BeliefNode};

pub mod belief_ir;
pub mod builder;
pub mod compiler;
pub mod diagnostic;
pub mod md;
pub mod schema_registry;

// Re-export for backward compatibility
pub use belief_ir::ProtoBeliefNode;
pub use builder::GraphBuilder;
pub use compiler::DocumentCompiler;
pub use diagnostic::{ParseDiagnostic, UnresolvedReference};
pub use schema_registry::SCHEMAS;

/// Global singleton codec map with builtin codecs (md, toml)
pub static CODECS: Lazy<CodecMap> = Lazy::new(CodecMap::create);

/// [ ] Need to iterate out protobeliefstate
/// [ ] Need to replace protobeliefstates
/// [ ] Need to write doc to buffer
/// [ ] Be able to publish markdown snippets -- with or without: anchors, revised src/hrefs, widget
///     configuration toml
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
}

// It is better to express the complexity of the singleton than hide it. Also the CodecMap methods
// are used to properly unwrap this structure.
#[allow(clippy::type_complexity)]
pub struct CodecMap(Arc<RwLock<Vec<(String, Arc<Mutex<dyn DocCodec + Send>>)>>>);

impl Clone for CodecMap {
    fn clone(&self) -> Self {
        CodecMap(self.0.clone())
    }
}

impl CodecMap {
    pub fn create() -> Self {
        CodecMap(Arc::new(RwLock::new(vec![
            ("md".to_string(), Arc::new(Mutex::new(md::MdCodec::new()))),
            (
                "toml".to_string(),
                Arc::new(Mutex::new(ProtoBeliefNode::default())),
            ),
        ])))
    }

    pub fn insert<T: DocCodec + Clone + Default + Send + Sync + 'static>(&self, extension: String) {
        while self.0.is_locked() {
            tracing::info!("[CodecMap::insert] Waiting for write access to the codec map");
            std::thread::sleep(Duration::from_millis(100));
        }
        let mut writer = self.0.write_arc();
        if let Some(entry) = writer.iter_mut().find(|(ext, _)| ext == &extension) {
            entry.1 = Arc::new(Mutex::new(T::default()));
        } else {
            writer.push((extension, Arc::new(Mutex::new(T::default()))));
        }
    }

    pub fn get(&self, ext: &str) -> Option<Arc<Mutex<dyn DocCodec + Send>>> {
        while self.0.is_locked_exclusive() {
            tracing::info!("[CodecMap::insert] Waiting for read access to the codec map");
            std::thread::sleep(Duration::from_millis(100));
        }
        let reader = self.0.read_arc();
        let res = reader
            .iter()
            .find(|(codec_ext, _value)| ext == codec_ext)
            .map(|(_codec_ext, value)| value.clone());
        res
    }

    pub fn extensions(&self) -> Vec<String> {
        while self.0.is_locked_exclusive() {
            tracing::info!("[CodecMap::insert] Waiting for read access to the codec map");
            std::thread::sleep(Duration::from_millis(100));
        }
        let reader = self.0.read_arc();
        reader
            .iter()
            .map(|(codec_ext, _value)| codec_ext.clone())
            .collect::<Vec<String>>()
    }
}
