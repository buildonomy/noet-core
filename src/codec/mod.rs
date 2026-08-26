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
//! - **By stem**: `(Some("index"), Some("md"))` - matches files named `index.md` (regardless of location)
//! - **By directory**: `(Some("docs"), None)` - can match directory names (if AnchorPath treats them as directories)
//!
//! This flexible registration enables:
//! - Single files: `index.md` (BeliefNetwork metadata)
//! - File patterns: `.md`, `.toml`, `.json`
//! - Directory structures: `.github/`, `node_modules/` (when treated as units)
//!
//! ## Built-in Codecs
//!
//! - **Markdown** (`.md`) — via [`md::MdCodec`]
//! - **NetworkCodec** (`index.md`) — via [`network::NetworkCodec`]
//! - **XlsxCodec** (`.xlsx`, `.ods`) — via `xlsx::codec::XlsxCodec` (requires `xlsx` feature,
//!   non-wasm only). Reads spreadsheets with a reserved `index` tab schema; emits a workbook →
//!   tab → row node hierarchy. Uses `CodecContentMode::Binary` — see below.
//!
//! ## Binary Codecs and `CodecContentMode`
//!
//! Most codecs operate on UTF-8 text (the default). Binary file formats (xlsx, ods, PDF, etc.)
//! require a different pipeline: the compiler must not attempt `String::from_utf8` on their bytes,
//! and write-back cannot use `Option<String>`.
//!
//! Codecs declare their content mode via [`CodecContentMode`]:
//!
//! - **`CodecContentMode::Text`** (default) — `parse()` receives decoded UTF-8 content.
//!   Write-back is via `generate_source() -> Option<String>`.
//! - **`CodecContentMode::Binary`** — `parse()` receives an empty string (ignored). The codec
//!   re-opens the source file from `current.path` using its own I/O. Write-back is via
//!   `generate_source_bytes() -> Option<Vec<u8>>`.
//!
//! The compiler probes the mode once per file (via a cheap factory instantiation) and branches
//! accordingly. All existing text codecs default to `Text` and require no changes.
//!
//! To implement a binary codec, override two methods:
//! ```rust
//! # use noet_core::codec::{DocCodec, CodecContentMode};
//! # struct MyBinaryCodec;
//! # impl MyBinaryCodec {
//! fn content_mode(&self) -> CodecContentMode {
//!     CodecContentMode::Binary
//! }
//!
//! fn generate_source_bytes(&self) -> Option<Vec<u8>> {
//!     // Return annotated file bytes for write-back, or None if unchanged.
//!     None
//! }
//! # }
//! ```
//!
//! Register custom codecs via [`CodecMap::insert_codec`] (by stem/extension):
//!
//! ```rust
//! use noet_core::{beliefbase::BeliefContext, BuildonomyError, codec::{CODECS, DocCodec, IRNode, ParseDiagnostic}, properties::{BeliefNode, Bid}};
//! use std::path::Path;
//!
//! #[derive(Default, Clone)]
//! struct MyCustomCodec;
//!
//! impl DocCodec for MyCustomCodec {
//!     fn proto(
//!         &self,
//!         path: &Path,
//!     ) -> Result<Option<IRNode>, BuildonomyError> {
//!         todo!();
//!     }
//!
//!     fn parse(
//!         &mut self,
//!         // The source content to be parsed by the DocCodec implementation
//!         content: &str,
//!         // Contains the builder root-path relative information to seed the parse with
//!         current: IRNode,
//!         // Any author-visible warnings discovered during parsing
//!         diagnostics: &mut Vec<ParseDiagnostic>,
//!         // Pre-built filesystem index. Most codecs ignore it.
//!         _proto_index: &noet_core::codec::proto_index::ProtoIndex,
//!     ) -> Result<(), BuildonomyError> {
//!         todo!();
//!     }
//!
//!     fn nodes(&self) -> Vec<IRNode> {
//!         todo!();
//!     }
//!
//!     fn inject_context(
//!         &mut self,
//!         proto_idx: usize,
//!         node: &IRNode,
//!         ctx: &BeliefContext<'_>,
//!         diagnostics: &mut Vec<ParseDiagnostic>,
//!     ) -> Result<Option<BeliefNode>, BuildonomyError> {
//!         todo!();
//!     }
//!
//!     fn finalize(&mut self, diagnostics: &mut Vec<ParseDiagnostic>) -> Result<std::collections::HashMap<Bid, IRNode>, BuildonomyError> {
//!         Ok(std::collections::HashMap::new())
//!     }
//!
//!     fn generate_source(&self) -> Option<String> {
//!         todo!();
//!     }
//!     // For binary codecs, override content_mode() and generate_source_bytes() instead:
//!     // fn content_mode(&self) -> CodecContentMode { CodecContentMode::Binary }
//!     // fn generate_source_bytes(&self) -> Option<Vec<u8>> { None }
//! }
//! // Register by extension (simple API)
//! CODECS.insert_codec(None, Some("myext".to_string()), || Box::new(MyCustomCodec));
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
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    result::Result,
    sync::Arc,
    time::Duration,
};

#[cfg(not(target_arch = "wasm32"))]
use crate::{
    beliefbase::BeliefContext,
    codec::network::NetworkCodec,
    error::BuildonomyError,
    paths::os_path_to_string,
    properties::{BeliefNode, Bid},
};

use crate::paths::AnchorPath;

/// Standard filename designating a directory as the root of a BeliefNetwork.
///
/// Defined here (ungated) so it is accessible on all targets including wasm32.
/// The full network codec logic lives in [`network`] (non-wasm only).
pub const NETWORK_NAME: &str = "index.md";

#[cfg(not(target_arch = "wasm32"))]
pub mod assets;
#[cfg(not(target_arch = "wasm32"))]
pub mod belief_ir;
#[cfg(not(target_arch = "wasm32"))]
pub mod builder;
#[cfg(not(target_arch = "wasm32"))]
pub mod compiler;
pub mod diagnostic;
#[cfg(not(target_arch = "wasm32"))]
pub mod git;
#[cfg(not(target_arch = "wasm32"))]
pub mod md;
pub mod md_options;
#[cfg(not(target_arch = "wasm32"))]
pub mod myst;
#[cfg(not(target_arch = "wasm32"))]
pub mod network;
#[cfg(not(target_arch = "wasm32"))]
pub mod proto_index;
#[cfg(not(target_arch = "wasm32"))]
pub mod schema_registry;
#[cfg(all(feature = "xlsx", not(target_arch = "wasm32")))]
pub mod xlsx;

// Re-export for backward compatibility
#[cfg(not(target_arch = "wasm32"))]
pub use belief_ir::{IRNode, IntermediateRelation};
#[cfg(not(target_arch = "wasm32"))]
pub use builder::GraphBuilder;
#[cfg(not(target_arch = "wasm32"))]
pub use compiler::DocumentCompiler;
pub use diagnostic::{byte_offset_to_location, ParseDiagnostic, UnresolvedReference};
pub use md_options::{buildonomy_md_options, render_markdown_snippet};
#[cfg(not(target_arch = "wasm32"))]
pub use proto_index::ProtoIndex;
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
/// - File stem: `CODECS.insert_codec(Some("index".to_string()), Some("md".to_string()), factory)`
/// - Both: `CODECS.insert_codec(Some("config".to_string()), Some("toml".to_string()), factory)`
#[cfg(not(target_arch = "wasm32"))]
pub static CODECS: Lazy<CodecMap> = Lazy::new(CodecMap::create);

/// Global codec map for WASM - lightweight extension registry only.
#[cfg(target_arch = "wasm32")]
pub static CODECS: Lazy<CodecMap> = Lazy::new(CodecMap::create);

/// List of built-in codec extensions (synchronized between WASM and non-WASM builds).
pub const BUILTIN_EXTENSIONS: &[&str] = &["md", "xlsx", "ods"];

/// Extensions handled by the xlsx codec (non-wasm only, feature-gated).
#[cfg(all(feature = "xlsx", not(target_arch = "wasm32")))]
pub const XLSX_EXTENSIONS: &[&str] = &["xlsx", "ods"];

// ── Walk-time codec registry ─────────────────────────────────────────────────

/// Walk-time file visibility predicate.
///
/// Determines whether a file should be included in [`ProtoIndex`] child lists
/// during the `net_dir_partition` WalkDir pass. Implementations must be cheap —
/// no file I/O, no content sniffing; path-based checks only.
///
/// Walk codecs govern *visibility*, not *dispatch*. A file tracked by a
/// `WalkCodec` will appear in `ProtoIndex` child lists and be passed to
/// `parse_one_path`, but the codec that actually parses it is determined by
/// [`CLAIM_MAP`] (registered during [`DocCodec::parse`] in Phase 1) or
/// [`CODECS`] (extension/stem registered).
///
/// # Extension vs. application-specific codecs
///
/// Built-in walk codecs ([`MdWalkCodec`], [`YamlWalkCodec`]) are registered in
/// [`WALK_CODECS`] at startup and are application-neutral. Application shims
/// (e.g. `vast-noet`) register additional codecs via [`WalkCodecMap::register`]
/// before [`DocumentCompiler::new`].
///
/// # Thread safety
///
/// Implementations must be `Send + Sync` — they are stored in a global
/// `Arc<RwLock<Vec<Box<dyn WalkCodec>>>>`.
#[cfg(not(target_arch = "wasm32"))]
pub trait WalkCodec: Send + Sync {
    /// Return `true` if this file should be included in ProtoIndex child lists.
    ///
    /// Called once per file during the WalkDir pass. Must be cheap — no I/O,
    /// no content sniffing. Path-based extension checks are the standard pattern.
    fn should_track(&self, path: &Path) -> bool;

    /// Return the file extensions this codec tracks (without leading dot).
    ///
    /// Used by [`collect_known_extensions`] to build the codec manifest for WASM.
    /// Implementations should return the same extensions they match in `should_track`.
    fn tracked_extensions(&self) -> Vec<&'static str>;

    /// Filenames that MAY define network roots when present in a directory.
    ///
    /// This is a superset declaration — `net_dir_partition` tentatively treats any
    /// directory containing a matching file as a subnet boundary. The definitive
    /// check happens in [`ProtoIndex::build`], which calls [`DocCodec::proto()`] on
    /// each candidate. `proto()` returns `Some(IRNode)` with [`BeliefKind::Network`](crate::properties::BeliefKind::Network)
    /// for real network roots, or `None` for files that share the name but aren't
    /// network roots.
    ///
    /// Content-based discrimination (e.g. checking for a library definition in a
    /// build manifest) belongs in the codec's `proto()` implementation, not here.
    ///
    /// Default: empty (this codec does not define network boundaries).
    fn network_filenames(&self) -> Vec<&'static str> {
        vec![]
    }
}

/// Walk codec for Markdown files (`.md`).
#[cfg(not(target_arch = "wasm32"))]
pub struct MdWalkCodec;

#[cfg(not(target_arch = "wasm32"))]
impl WalkCodec for MdWalkCodec {
    fn should_track(&self, path: &Path) -> bool {
        matches!(path.extension().and_then(|e| e.to_str()), Some("md"))
    }

    fn tracked_extensions(&self) -> Vec<&'static str> {
        vec!["md"]
    }
}

/// Walk codec for YAML files (`.yaml` / `.yml`).
#[cfg(not(target_arch = "wasm32"))]
pub struct YamlWalkCodec;

#[cfg(not(target_arch = "wasm32"))]
impl WalkCodec for YamlWalkCodec {
    fn should_track(&self, path: &Path) -> bool {
        matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("yaml" | "yml")
        )
    }

    fn tracked_extensions(&self) -> Vec<&'static str> {
        vec!["yaml", "yml"]
    }
}

/// Thread-safe registry of [`WalkCodec`] implementations.
///
/// Any registered codec whose [`WalkCodec::should_track`] returns `true` causes
/// the file to be included in [`ProtoIndex`] child lists during
/// `net_dir_partition`. Multiple codecs may track the same extension;
/// `should_track` is `true` if ANY registered codec returns `true`.
///
/// The global instance is [`WALK_CODECS`], pre-populated with [`MdWalkCodec`]
/// and [`YamlWalkCodec`]. Application shims register additional walk codecs via
/// [`WalkCodecMap::register`] before [`DocumentCompiler::new`].
#[cfg(not(target_arch = "wasm32"))]
pub struct WalkCodecMap(Arc<RwLock<Vec<Box<dyn WalkCodec>>>>);

#[cfg(not(target_arch = "wasm32"))]
impl WalkCodecMap {
    /// Create a new `WalkCodecMap` with all built-in walk codecs registered.
    pub fn create() -> Self {
        let map = WalkCodecMap(Arc::new(RwLock::new(Vec::new())));
        map.register(Box::new(MdWalkCodec));
        map.register(Box::new(YamlWalkCodec));
        map
    }

    /// Register a walk codec. Multiple walk codecs may track the same extension;
    /// `should_track` is true if ANY registered codec returns true.
    pub fn register(&self, codec: Box<dyn WalkCodec>) {
        self.0.write().push(codec);
    }

    /// True if any registered `WalkCodec` claims this path.
    pub fn should_track(&self, path: &Path) -> bool {
        self.0.read().iter().any(|c| c.should_track(path))
    }

    /// Collect all tracked extensions from registered walk codecs.
    ///
    /// Returns a deduplicated, sorted list of extensions (without leading dot).
    pub fn extensions(&self) -> Vec<String> {
        let mut exts: Vec<String> = self
            .0
            .read()
            .iter()
            .flat_map(|c| c.tracked_extensions())
            .map(|s| s.to_string())
            .collect();
        exts.sort();
        exts.dedup();
        exts
    }

    /// Filename-only check (no I/O). True if `filename` matches [`NETWORK_NAME`]
    /// or any registered walk codec's [`WalkCodec::network_filenames()`].
    ///
    /// Used for subnet detection in `net_dir_partition` and for path normalization
    /// in the builder, compiler, and DB layers (stripping the network filename to
    /// get the directory path).
    pub fn is_network_file(&self, filename: &str) -> bool {
        if filename == NETWORK_NAME {
            return true;
        }
        self.0
            .read()
            .iter()
            .any(|c| c.network_filenames().contains(&filename))
    }

    /// Returns all registered network filenames (including [`NETWORK_NAME`]),
    /// deduplicated.
    ///
    /// Used by [`detect_network_file`](crate::codec::network::detect_network_file) and by
    /// [`CodecManifest`](crate::shard::manifest::CodecManifest) to bridge the native
    /// registry to the WASM viewer.
    pub fn network_filenames(&self) -> Vec<String> {
        let mut names = vec![NETWORK_NAME.to_string()];
        for codec in self.0.read().iter() {
            for name in codec.network_filenames() {
                let s = name.to_string();
                if !names.contains(&s) {
                    names.push(s);
                }
            }
        }
        names
    }
}

/// Global walk-codec registry.
///
/// Pre-populated with [`MdWalkCodec`] and [`YamlWalkCodec`] at startup.
/// Application shims (e.g. `vast-noet`) register additional walk codecs via
/// [`WalkCodecMap::register`] before [`DocumentCompiler::new`] is called.
///
/// # WASM
///
/// `WALK_CODECS` is absent on `wasm32`. The WASM viewer loads a codec manifest
/// (`codecs.json`) at startup that lists all extensions known at build time,
/// including those registered here. See [`collect_known_extensions`] and
/// `CodecMap::set_known_extensions` (WASM-only).
#[cfg(not(target_arch = "wasm32"))]
pub static WALK_CODECS: Lazy<WalkCodecMap> = Lazy::new(WalkCodecMap::create);

/// True if `path`'s filename is a known network index filename.
///
/// Checks [`NETWORK_NAME`] first (hot path), then consults registered walk codecs
/// via [`WalkCodecMap::is_network_file`]. Used for path normalization: stripping
/// the network filename to get the directory path.
///
/// Replaces inline `== NETWORK_NAME` checks throughout the codebase.
#[cfg(not(target_arch = "wasm32"))]
pub fn is_network_index_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|n| WALK_CODECS.is_network_file(n))
        .unwrap_or(false)
}

/// Collect all document extensions known at build time.
///
/// Returns the union of [`BUILTIN_EXTENSIONS`], [`CODECS`] registered extensions,
/// and [`WALK_CODECS`] tracked extensions — sorted and deduplicated.
///
/// Used by `finalize_html` to build the [`CodecManifest`](crate::shard::manifest::CodecManifest) that bridges the
/// native codec registries to the WASM viewer.
#[cfg(not(target_arch = "wasm32"))]
pub fn collect_known_extensions() -> Vec<String> {
    let mut exts: Vec<String> = BUILTIN_EXTENSIONS.iter().map(|s| s.to_string()).collect();
    exts.extend(CODECS.extensions());
    exts.extend(WALK_CODECS.extensions());
    // Filter empty strings (e.g. from the wildcard NetworkCodec entry)
    // — they are not meaningful file extensions.
    exts.retain(|s| !s.is_empty());
    exts.sort();
    exts.dedup();
    exts
}

// ── Parse-time claim registry ─────────────────────────────────────────────────

/// Parse-time registry mapping absolute file paths to the codec that owns them.
///
/// Entries are written during [`DocCodec::parse`] (Phase 1) by network codecs
/// that discover which data files they own. `parse_one_path` consults this
/// registry before falling back to [`CODECS`].
///
/// ## Inner value semantics
///
/// - `Some(factory)` — path has been claimed by a codec; use `factory()` to
///   dispatch it.
/// - `None` — path was explicitly *rejected* by a network's whitelist/blacklist
///   filter. `parse_one_path` routes rejected paths to [`UnclaimedDataCodec`]
///   + `ParseDiagnostic::info` without falling through to `CODECS`.
///
/// The `None` sentinel distinguishes "rejected by a network that ran filtering"
/// from "never seen" (absent from the map entirely), preventing `.md` files
/// from being re-dispatched via the old `CODECS` fallback after being
/// explicitly excluded.
///
/// ## Claiming pattern
///
/// A codec that owns structured data files should call `claim()` inside its
/// [`DocCodec::parse`] implementation, using `proto_index.children_of()` to
/// discover the candidate file list:
///
/// ```rust,ignore
/// fn parse(&mut self, content: &str, current: IRNode,
///           diagnostics: &mut Vec<ParseDiagnostic>,
///           proto_index: &ProtoIndex) -> Result<(), BuildonomyError> {
///     let network_dir = string_to_os_path(&current.path);
///     for child in proto_index.children_of(&network_dir).unwrap_or_default() {
///         if self.owns(&child) {
///             CLAIM_MAP.claim(child, my_codec_factory);
///         }
///     }
///     Ok(())
/// }
/// ```
///
/// ## Thread safety
///
/// Wraps `Arc<RwLock<HashMap<PathBuf, Option<CodecFactory>>>>`. Reads and
/// writes are short-critical-section; no long-held locks.
#[cfg(not(target_arch = "wasm32"))]
pub struct ClaimMap(Arc<RwLock<HashMap<PathBuf, Option<CodecFactory>>>>);

#[cfg(not(target_arch = "wasm32"))]
impl ClaimMap {
    /// Create a new, empty `ClaimMap`.
    pub fn create() -> Self {
        ClaimMap(Arc::new(RwLock::new(HashMap::new())))
    }

    /// Claim a specific absolute path for a given codec factory.
    ///
    /// If already claimed by a different factory, overwrites and emits [`tracing::warn!`].
    pub fn claim(&self, abs_path: PathBuf, factory: CodecFactory) {
        let mut map = self.0.write();
        if let Some(Some(existing)) = map.get(&abs_path) {
            if *existing as usize != factory as usize {
                tracing::warn!(
                    path = %abs_path.display(),
                    "ClaimMap: path already claimed by a different codec factory; overwriting"
                );
            }
        }
        map.insert(abs_path, Some(factory));
    }

    /// Register an explicit rejection sentinel for a path.
    ///
    /// Called by `NetworkCodec::prepare_proto_relations` for children filtered out by
    /// whitelist/blacklist rules. Allows `parse_one_path` to distinguish "rejected by
    /// a network filter" from "not yet seen" (absent from the map entirely), routing
    /// rejected files to `UnclaimedDataCodec` rather than `CODECS.path_get`.
    pub fn reject(&self, abs_path: PathBuf) {
        self.0.write().insert(abs_path, None);
    }

    /// Returns `true` if the path has been explicitly rejected by a network filter.
    ///
    /// A path that is absent from the map returns `false` — absence means "not yet
    /// seen", not "rejected".
    ///
    /// Checks the path itself AND all ancestor directories. When a network's
    /// blacklist rejects a non-subnet directory (e.g. `report.media/`), the
    /// rejection is stored for the directory path. Files inside that directory
    /// (e.g. `report.media/ppt/media/image8.png`) must also be considered
    /// rejected — otherwise they fall through to asset processing.
    pub fn is_rejected(&self, abs_path: &Path) -> bool {
        let map = self.0.read();
        // Check the exact path first (fast path).
        if map.get(abs_path).is_some_and(|v| v.is_none()) {
            return true;
        }
        // Walk ancestor directories — if any ancestor was rejected, this
        // path is implicitly rejected too.
        let mut current = abs_path.parent();
        while let Some(ancestor) = current {
            if map.get(ancestor).is_some_and(|v| v.is_none()) {
                return true;
            }
            current = ancestor.parent();
        }
        false
    }

    /// Look up the codec factory for an absolute path.
    ///
    /// First checks the claim registry; if no explicit claim exists, falls back to
    /// `CODECS.path_get` so callers have a single unified dispatch point.
    ///
    /// Returns `None` if the path was explicitly rejected (callers should check
    /// [`ClaimMap::is_rejected`] separately) or if neither the claim registry nor
    /// `CODECS` has a factory for this path.
    pub fn get(&self, abs_path: &Path) -> Option<CodecFactory> {
        self.0
            .read()
            .get(abs_path)
            .and_then(|v| *v)
            .or_else(|| CODECS.path_get(abs_path))
    }

    /// Remove a claim or rejection sentinel (used by `on_file_deleted` in the watch loop).
    pub fn unclaim(&self, abs_path: &Path) {
        self.0.write().remove(abs_path);
    }

    /// Number of currently registered entries (claims + rejections).
    pub fn len(&self) -> usize {
        self.0.read().len()
    }

    /// Returns `true` if no entries are registered.
    pub fn is_empty(&self) -> bool {
        self.0.read().is_empty()
    }
}

/// Global parse-time claim registry.
///
/// Written during Phase 1 by [`NetworkCodec::parse`] (and any other codec that
/// claims structured data files). Read during Phase 2 by `parse_one_path`.
///
/// Call [`ClaimMap::unclaim`] from [`DocumentCompiler::on_file_deleted`] when
/// a claimed file is deleted, so stale entries do not affect future re-scans.
#[cfg(not(target_arch = "wasm32"))]
pub static CLAIM_MAP: Lazy<ClaimMap> = Lazy::new(ClaimMap::create);

// ── Codec namespace registry ─────────────────────────────────────────────────────

/// Global registry of codec namespace brefs created during parsing.
///
/// Populated by [`builder::GraphBuilder::push`] when it lazily creates a codec
/// namespace network node.  Queried by
/// [`compiler::DocumentCompiler::process_unresolved_reference`] to skip
/// filesystem resolution for synthetic namespace references, and by the wasm
/// viewer to identify codec namespace networks for display and context lookup.
///
/// Same singleton pattern as [`CODECS`], [`WALK_CODECS`], and [`CLAIM_MAP`].
static CODEC_NAMESPACES: Lazy<
    std::sync::RwLock<std::collections::HashSet<crate::properties::Bref>>,
> = Lazy::new(|| std::sync::RwLock::new(std::collections::HashSet::new()));

/// Register a codec namespace bref in the global registry.
///
/// Called by `push()` when it lazily creates a codec namespace network node.
/// Idempotent — registering the same bref multiple times is a no-op.
pub fn register_codec_namespace(bref: crate::properties::Bref) {
    CODEC_NAMESPACES.write().unwrap().insert(bref);
}

/// Check whether a bref belongs to a registered codec namespace.
///
/// Used by `process_unresolved_reference` to skip filesystem resolution for
/// synthetic namespace references.
pub fn is_codec_namespace(bref: &crate::properties::Bref) -> bool {
    CODEC_NAMESPACES.read().unwrap().contains(bref)
}

/// Return all registered codec namespace brefs.
///
/// Used by the wasm viewer to identify codec namespace networks.
pub fn codec_namespace_brefs() -> Vec<crate::properties::Bref> {
    CODEC_NAMESPACES.read().unwrap().iter().copied().collect()
}

/// Clear the codec namespace registry.  Used between test runs to avoid
/// cross-test contamination.
#[cfg(test)]
pub fn clear_codec_namespaces() {
    CODEC_NAMESPACES.write().unwrap().clear();
}

// ── No-op fallback codec ──────────────────────────────────────────────────────

/// No-op codec for walk-tracked files that no network has claimed.
///
/// Used by `parse_one_path` for two cases:
/// 1. A file is tracked by [`WALK_CODECS`] but absent from [`CLAIM_MAP`] — no
///    network codec claimed it (e.g. a stray `.yaml` file in a plain corpus).
/// 2. A file was explicitly rejected by a network's whitelist/blacklist filter
///    (stored as a `None` sentinel in [`CLAIM_MAP`]).
///
/// In both cases this codec:
/// - Emits `ParseDiagnostic::info` naming the file
/// - Produces no `IRNode`s and no `BeliefBase` nodes
/// - Does **not** reach `process_asset` — the file is identified as structured
///   text that no codec currently owns
///
/// The `parse()` method accepts a `proto_index: &ProtoIndex` parameter (part of
/// the [`DocCodec`] trait since Issue 68) but ignores it.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Default, Clone)]
pub struct UnclaimedDataCodec;

#[cfg(not(target_arch = "wasm32"))]
impl DocCodec for UnclaimedDataCodec {
    fn proto(&self, _path: &Path) -> Result<Option<IRNode>, BuildonomyError> {
        Ok(None)
    }

    fn parse(
        &mut self,
        _content: &str,
        _current: IRNode,
        _diagnostics: &mut Vec<ParseDiagnostic>,
        _proto_index: &crate::codec::proto_index::ProtoIndex,
    ) -> Result<(), BuildonomyError> {
        Ok(())
    }

    fn nodes(&self) -> Vec<IRNode> {
        vec![]
    }

    fn inject_context(
        &mut self,
        _proto_idx: usize,
        _node: &IRNode,
        _ctx: &BeliefContext<'_>,
        _diagnostics: &mut Vec<ParseDiagnostic>,
    ) -> Result<Option<BeliefNode>, BuildonomyError> {
        Ok(None)
    }

    fn finalize(
        &mut self,
        _diagnostics: &mut Vec<ParseDiagnostic>,
    ) -> Result<HashMap<Bid, IRNode>, BuildonomyError> {
        Ok(HashMap::new())
    }

    fn generate_source(&self) -> Option<String> {
        None
    }
}

/// Codec registration entry: (optional_stem, optional_extension, factory).
///
/// At least one of stem or extension must be Some.
///
/// # Examples
/// - `(None, Some("md"), factory)` - Match all `.md` files
/// - `(Some("index"), Some("md"), factory)` - Match files named `index.md`
/// - `(Some("config"), Some("toml"), factory)` - Match `config.toml` files
#[cfg(not(target_arch = "wasm32"))]
type CodecEntry = (Option<String>, Option<String>, CodecFactory);

/// Declares how a codec expects to receive file content and write it back.
///
/// The compiler reads this once per file (via a cheap probe instantiation) and
/// branches accordingly. All existing codecs default to `Text`.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CodecContentMode {
    /// Codec operates on UTF-8 text. `parse()` receives decoded string content.
    /// `generate_source()` returns `Option<String>` for write-back. (default)
    #[default]
    Text,
    /// Codec operates on raw bytes. The `content: &str` parameter passed to `parse()`
    /// is an empty string and must be ignored by the codec. The codec re-opens the
    /// file from `current.path` using its own I/O.
    /// `generate_source_bytes()` is called for binary write-back instead of
    /// `generate_source()`.
    Binary,
}

// [ ] Need to iterate out protobeliefstate
// [ ] Need to replace protobeliefstates
// [ ] Need to write doc to buffer
// [ ] Be able to publish markdown snippets -- with or without: anchors, revised src/hrefs, widget
//     configuration toml

/// Named placeholder pairs for HTML fragment generation. See [`DocCodec::generate_html`].
#[cfg(not(target_arch = "wasm32"))]
pub type HtmlFragmentPairs = Vec<(
    String,
    Vec<(String, String)>,
    Option<crate::codec::assets::Layout>,
)>;

#[cfg(not(target_arch = "wasm32"))]
pub trait DocCodec: Sync {
    /// Parse a path into a proto node by reading the metadata frontmatter (if any)
    fn proto(&self, path: &Path) -> Result<Option<IRNode>, BuildonomyError>;

    /// Populate `proto.upstream` with file-based child relations given a pre-computed
    /// list of absolute child paths.
    ///
    /// Called by [`crate::codec::proto_index::ProtoIndex`] after `proto()` returns, once per
    /// network directory entry in the index.  The `child_paths` list is the same list that
    /// `ProtoIndex::build` already computed (via `net_dir_children`) — no additional filesystem
    /// access is needed.
    ///
    /// ## Why a trait method?
    ///
    /// Placing this on the trait means:
    /// - Codec implementations are never required to touch the filesystem directly.
    /// - Future codecs with file-based relations (e.g. a TOML manifest codec) can express
    ///   those relations here without adding ad-hoc logic to `ProtoIndex`.
    /// - All `DocCodec` methods remain filesystem-free; filesystem knowledge lives solely
    ///   in `ProtoIndex::build` (one WalkDir pass) and the `net_dir_children` helper.
    ///
    /// The default implementation is a no-op, so codecs with no file-based relations require
    /// no change.
    ///
    /// ## Parameters
    /// - `proto`: the `IRNode` returned by `proto()`, mutated in place.
    /// - `network_dir`: absolute path of the network directory that owns the children.
    /// - `child_paths`: absolute paths of the direct children, in canonical DFS order
    ///   (subnet dirs first, then plain files, both groups sorted lexicographically).
    fn prepare_proto_relations(
        &self,
        _proto: &mut IRNode,
        _network_dir: &Path,
        _child_paths: &[std::path::PathBuf],
    ) -> Result<(), BuildonomyError> {
        Ok(())
    }

    /// Parse the document content into IR nodes.
    ///
    /// The `proto_index` parameter gives access to the pre-built filesystem index.
    /// Most codecs ignore it; `NetworkCodec` uses it during `parse()` to claim or
    /// reject child paths in `CLAIM_MAP` before Phase 2 dispatch.
    fn parse(
        &mut self,
        // The source content to be parsed by the DocCodec implementation
        content: &str,
        // Contains the builder root-path relative information to seed the parse with
        current: IRNode,
        // Any author-visible warnings or errors discovered during parsing (e.g. duplicate
        // heading anchors) should be pushed here rather than emitted via tracing.
        diagnostics: &mut Vec<ParseDiagnostic>,
        // Pre-built filesystem index. Most codecs ignore it; NetworkCodec uses it during
        // parse() to claim or reject child paths in CLAIM_MAP before Phase 2 dispatch.
        proto_index: &crate::codec::proto_index::ProtoIndex,
    ) -> Result<(), BuildonomyError>;

    fn nodes(&self) -> Vec<IRNode>;

    /// Write the resolved BID back into the codec's internal proto for the node at
    /// `proto_idx`. Called from the Phase 1 push loop immediately after `push()`
    /// returns, before `inject_context` runs.
    ///
    /// This ensures that when `inject_context` calls `merge_from_belief_node`, the
    /// `bid` field is already present in the proto's TOML document. Without this,
    /// section protos always have an absent `bid` key (it lives in the document-level
    /// `[sections]` table, not in the heading's own frontmatter), causing
    /// `merge_from_belief_node` to insert it on every parse → `frontmatter_changed=true`
    /// → `generate_source()` called → content rewrite loop.
    ///
    /// Default: no-op (codecs that don't need BID write-back can leave this).
    fn set_node_bid(&mut self, _proto_idx: usize, _bid: Bid) {}

    /// Inject resolved context into a parsed node, optionally returning an updated `BeliefNode`.
    ///
    /// Any author-visible warnings or errors discovered during context injection (e.g. unresolved
    /// links, malformed frontmatter) should be pushed onto `diagnostics` rather than emitted via
    /// `tracing`. This ensures they flow through `ParseContentResult` to the CLI and LSP layers.
    fn inject_context(
        &mut self,
        proto_idx: usize,
        node: &IRNode,
        ctx: &BeliefContext<'_>,
        diagnostics: &mut Vec<ParseDiagnostic>,
    ) -> Result<Option<BeliefNode>, BuildonomyError>;

    /// Called after all inject_context() calls complete, allowing the codec to:
    /// - Perform cross-node cleanup (e.g., track unmatched sections)
    /// - Emit events for nodes modified during finalization
    /// - Log diagnostics for unmatched metadata
    ///
    /// Any author-visible warnings discovered during finalization should be pushed onto
    /// `diagnostics` rather than emitted via `tracing`.
    ///
    /// Returns a `HashMap<Bid, IRNode>` for nodes whose source-file-derived fields changed
    /// during finalization.  The `Bid` key is authoritative — the IRNode is not guaranteed
    /// to embed its own BID, so the map makes the mapping explicit and infallible.
    /// The caller applies each delta to the existing `BeliefNode` in `doc_bb` via
    /// `BeliefNode::apply_source_update`, which updates only the source-file fields
    /// (`kind`, `title`, `schema`, `payload`, `id`) and leaves runtime-only fields
    /// (`bid`, `metadata`) untouched.
    ///
    /// Every implementor must explicitly handle this. Codecs that wrap other codecs (e.g.,
    /// `NetworkCodec` wrapping `MdCodec`) must delegate to the inner codec's `finalize()`.
    /// Codecs with no finalization logic should return `Ok(Vec::new())`.
    fn finalize(
        &mut self,
        diagnostics: &mut Vec<ParseDiagnostic>,
    ) -> Result<std::collections::HashMap<Bid, IRNode>, BuildonomyError>;

    fn generate_source(&self) -> Option<String>;

    /// Declare whether this codec operates on UTF-8 text or raw bytes.
    ///
    /// The compiler reads this once per file to decide whether to decode bytes
    /// to a String before calling `parse()`. Defaults to `Text`.
    fn content_mode(&self) -> CodecContentMode {
        CodecContentMode::Text
    }

    /// For binary codecs (`content_mode() == Binary`): produce the annotated
    /// file bytes for write-back (e.g. an xlsx file with injected BID columns).
    ///
    /// Called in place of `generate_source()` when `content_mode()` is `Binary`.
    /// Return `None` if the file is unchanged and no write-back is needed.
    ///
    /// Default: `None` (no binary write-back).
    fn generate_source_bytes(&self) -> Option<Vec<u8>> {
        None
    }

    /// Derived file outputs produced during `parse()`.
    ///
    /// Returns a list of `(repo_relative_path, bytes)` pairs. The compiler creates
    /// each file under `<repo_root>/<repo_relative_path>` and writes it to disk so
    /// subsequent compile passes can register a content-addressed asset node for it.
    ///
    /// Paths must be relative to the repo root. The compiler does not enforce any
    /// path convention but it is strongly recommended to avoid polluting the source tree.
    ///
    /// Default: empty — no derived outputs.
    fn derived_outputs(&self) -> Vec<(std::path::PathBuf, Vec<u8>)> {
        vec![]
    }

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
    /// - `Ok(vec![(filename, body, layout), ...])`: Output filenames, HTML body content, and
    ///   optional layout. `Some(layout)` means wrap the body in that template via
    ///   `write_fragment`. `None` means write raw (no template wrapping), used for companion
    ///   data files like JSON.
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
    /// # Placeholder Pairs
    /// Return named placeholder key-value pairs for template substitution:
    /// - Each pair is `("{{KEY}}".to_string(), value)` matching a placeholder in the layout template
    /// - For `Some(layout)`: pairs override template defaults (caller wins on collision)
    /// - For `None` layout: first pair's value is written verbatim (key ignored)
    /// - Common key: `"{{BODY}}"` for HTML body content (no `<html>`, `<head>`, etc.)
    /// - Compiler applies its own defaults first (CANONICAL, SPA_ROUTE, TITLE, BID, SCRIPTS, BODY="")
    ///   then applies caller pairs on top
    ///
    /// # Link Normalization
    /// **Implementations MUST normalize document links to `.html` extension:**
    /// - Convert all registered codec extensions (`.md`, `.toml`, `.org`, etc.) to `.html`
    /// - Preserve anchors: `.md#section` → `.html#section`
    /// - Use `CODECS.extensions()` to get the list of registered extensions
    ///
    /// Default implementation returns empty vec (no HTML generation).
    fn generate_html(&self) -> Result<HtmlFragmentPairs, BuildonomyError> {
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
    /// Note: plain `.md` files (other than `index.md`) are NOT registered in `CODECS`
    /// by extension — they are claimed per-network via `CLAIM_MAP`. This method will
    /// return `None` for arbitrary `.md` files; use `CLAIM_MAP.get()` to check those.
    ///
    /// # Example
    /// ```
    /// use std::path::Path;
    /// use noet_core::codec::CODECS;
    ///
    /// // Noet networks directories are identified by index.md file name
    /// let path = Path::new("/tmp/index.md");
    /// assert!(CODECS.path_get(&path).is_some()); // true for index.md
    ///
    /// ```
    pub fn path_get(&self, path: &std::path::Path) -> Option<CodecFactory> {
        let string_path = os_path_to_string(path);
        // Use new_file when the OS path is a known file so that extensionless filenames
        // (Gemfile, Makefile, …) are not misclassified as directories and do not
        // accidentally match the (None, None) NetworkCodec wildcard entry.
        // Actual directories (belief-network roots) are still handled by new() via the
        // is_dir() check in parse_next before path_get is ever called.
        let ap = if path.is_file() {
            AnchorPath::new_file(&string_path)
        } else {
            AnchorPath::new(&string_path)
        };
        self.get(&ap)
    }

    /// Create a new `CodecMap` with built-in codecs registered.
    ///
    /// Built-in codecs:
    /// - `index.md` (by stem+extension): registered directly → `NetworkCodec`
    /// - bare directory paths (no stem, no ext): registered via `(None, None)` → `NetworkCodec`
    /// - xlsx/ods (feature-gated): registered by extension
    ///
    /// Markdown files (`.md`) are NOT registered by extension — they are claimed
    /// per-network by `NetworkCodec::prepare_proto_relations` via `CLAIM_MAP`. Only
    /// `index.md` (by stem+extension) is registered directly.
    pub fn create() -> Self {
        #[allow(unused_mut)]
        let mut entries: Vec<CodecEntry> = vec![
            // Network files by constant filename index.md
            (Some("index".to_string()), Some("md".to_string()), || {
                Box::new(NetworkCodec::default())
            }),
            // (None, None) matches bare directory paths (no stem, no ext) so that
            // parse_content / initialize_stack can look up a codec for network root dirs
            // (e.g. `/repo/subnet/`). Extension-less *files* (Gemfile, Makefile, etc.) look
            // identical to AnchorPath, so callers that have filesystem access MUST guard
            // against them via `path.is_dir()` BEFORE calling CODECS.path_get / CODECS.get.
            (None, None, || Box::new(NetworkCodec::default())),
        ];

        #[cfg(all(feature = "xlsx", not(target_arch = "wasm32")))]
        {
            use crate::codec::xlsx::codec::XlsxCodec;
            entries.push((
                None,
                Some("xlsx".to_string()),
                || Box::new(XlsxCodec::new()),
            ));
            entries.push((None, Some("ods".to_string()), || Box::new(XlsxCodec::new())));
        }

        CodecMap(Arc::new(RwLock::new(entries)))
    }

    /// Insert a codec with optional stem and extension (advanced API).
    ///
    /// At least one of `stem` or `extension` must be `Some`. This method enables:
    /// - Registration by extension: `insert_codec(None, Some("md"), factory)`
    /// - Registration by stem: `insert_codec(Some("special-name"), None, factory)`
    /// - Registration by both: `insert_codec(Some("config"), Some("toml"), factory)`
    ///
    /// # Panics
    /// Panics if both `stem` and `extension` are `None`.
    ///
    /// # Example
    /// ```
    /// use noet_core::codec::{CODECS, md::MdCodec};
    ///
    /// // Match files named .myconfig (regardless of extension)
    /// CODECS.insert_codec(Some(".myconfig".to_string()), None, || Box::new(MdCodec::default()));
    ///
    /// // Match config.toml files specifically
    /// CODECS.insert_codec(Some("config".to_string()), Some("toml".to_string()), || Box::new(MdCodec::default()));
    /// ```
    pub fn insert_codec(
        &self,
        stem: Option<String>,
        extension: Option<String>,
        factory: CodecFactory,
    ) {
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
    /// Note: plain `.md` files (other than `index.md`) are NOT registered here — they are
    /// claimed per-network via `CLAIM_MAP`. Use `CLAIM_MAP.get(full_path)` for those.
    ///
    /// # Example
    /// ```
    /// use noet_core::{codec::CODECS, paths::AnchorPath};
    ///
    /// // Match by constant name
    /// let factory = CODECS.get(&AnchorPath::new("index.md"));
    /// assert!(factory.is_some());
    ///
    /// // Plain .md files are no longer registered by extension
    /// let factory = CODECS.get(&AnchorPath::new("README.md"));
    /// assert!(factory.is_none());
    ///
    /// // No match
    /// let factory = CODECS.get(&AnchorPath::new("unknown.xyz"));
    /// assert!(factory.is_none());
    /// ```
    pub fn get(&self, ap: &AnchorPath) -> Option<CodecFactory> {
        while self.0.is_locked_exclusive() {
            tracing::debug!("[CodecMap::get] Waiting for read access");
            std::thread::sleep(Duration::from_millis(100));
        }
        let reader = self.0.read_arc();
        let (filestem, ext) = ap.path_parts();
        reader
            .iter()
            .find(|(codec_stem, codec_ext, _)| {
                // Match if stem matches (when codec has a stem registered)
                let stem_matches = codec_stem.as_ref().is_some_and(|s| s == filestem);
                // Match if extension matches (when codec has an extension registered)
                let ext_matches =
                    codec_ext.as_ref().is_some_and(|e| e == ext) || codec_ext.is_none();
                stem_matches && ext_matches
            })
            .or(reader.iter().find(|(codec_stem, codec_ext, _)| {
                // Extension-only fallback: only match entries with no stem constraint.
                // Entries registered with a stem (e.g. `index.md`) must NOT match on
                // extension alone — that would cause `any_name.md` to resolve to the
                // `index.md`-specific codec when the bare `.md` entry is absent.
                codec_stem.is_none() && codec_ext.as_ref().is_some_and(|e| e == ext)
            }))
            .or(reader.iter().find(|(codec_stem, codec_ext, _)| {
                codec_stem.is_none() && codec_ext.is_none() && filestem.is_empty() && ext.is_empty()
            }))
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
            .filter_map(|(codec_stem, codec_ext, _)| {
                if codec_ext.is_some() {
                    codec_ext.clone()
                } else if codec_ext.is_none() && codec_stem.is_none() {
                    Some("".to_string())
                } else {
                    None
                }
            })
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

// WASM-compatible version: lightweight extension registry only (no actual codec instances).
//
// Defaults to [`BUILTIN_EXTENSIONS`] at creation, but the viewer should call
// [`set_known_extensions`] with the contents of `codecs.json` to pick up
// extensions registered by application shims at build time.
#[cfg(target_arch = "wasm32")]
pub struct CodecMap {
    /// Runtime extension set. Initialized from `BUILTIN_EXTENSIONS`, replaced
    /// by `set_known_extensions()` when the codec manifest is loaded.
    extensions: parking_lot::RwLock<Vec<String>>,
}

#[cfg(target_arch = "wasm32")]
impl Clone for CodecMap {
    fn clone(&self) -> Self {
        CodecMap {
            extensions: parking_lot::RwLock::new(self.extensions.read().clone()),
        }
    }
}

#[cfg(target_arch = "wasm32")]
impl CodecMap {
    pub fn create() -> Self {
        CodecMap {
            extensions: parking_lot::RwLock::new(
                BUILTIN_EXTENSIONS.iter().map(|s| s.to_string()).collect(),
            ),
        }
    }

    pub fn extensions(&self) -> Vec<String> {
        self.extensions.read().clone()
    }

    /// Replace the known extension set with extensions from the codec manifest.
    ///
    /// Called by the WASM viewer after fetching `codecs.json`. This bridges
    /// the gap between native-registered extensions and the WASM runtime.
    pub fn set_known_extensions(&self, extensions: Vec<String>) {
        *self.extensions.write() = extensions;
    }

    /// Check if a codec exists for the given anchor path (WASM version).
    ///
    /// Matches against the runtime extension set (defaulting to
    /// [`BUILTIN_EXTENSIONS`], updated by [`set_known_extensions`]).
    ///
    /// Empty-extension paths (network/directory entries) and non-codec extensions
    /// (.pdf, .png, etc.) deliberately return None — normalize_path_extension
    /// handles those cases itself via the ext.is_empty() branch.
    pub fn get(&self, anchor_path: &AnchorPath) -> Option<()> {
        let ext = anchor_path.ext();
        if self.extensions.read().iter().any(|e| e == ext) {
            Some(())
        } else {
            None
        }
    }
}

/// Normalise a path for the viewer to fetch as a rendered HTML document.
///
/// Rules (in priority order):
/// 1. Empty extension                    → network/directory entry, append `/index.html`
/// 2. Already `.html`                    → leave unchanged
/// 3. Known codec extension (e.g. `.md`) → replace with `.html` (anchor already included)
/// 4. Any other extension (`.pdf`, etc.) → asset path, leave unchanged
///
/// Anchor fragments (e.g. `#section`) are preserved in all cases.
/// Note: `replace_extension` already includes the anchor in its output, so we
/// must not re-attach it for case 3.
///
/// This function is always compiled (not gated on `wasm32`) so it can be
/// unit-tested with the native test runner. The `#[wasm_bindgen]` method in
/// `wasm.rs` delegates to this function.
/// Returns `true` if the given `AnchorPath` has an extension known to any registered codec
/// or walk codec.
///
/// Checks both `CODECS` (stem/extension registry) and, on native builds, `WALK_CODECS`
/// (walk-time visibility registry). This is the correct extensibility-aware gate for deciding
/// whether a link should be rewritten to `.html` — any extension with a registered codec
/// will produce a rendered HTML page; extensions known only to `WALK_CODECS` may also produce
/// pages if claimed by an application codec.
///
/// On WASM, only `CODECS` is consulted (`WALK_CODECS` is not compiled in).
#[cfg(not(target_arch = "wasm32"))]
pub fn is_known_codec_extension(ap: &AnchorPath) -> bool {
    if CODECS.get(ap).is_some() {
        return true;
    }
    WALK_CODECS.should_track(std::path::Path::new(ap.filepath()))
}

/// WASM variant: only consults `CODECS` since `WALK_CODECS` is not compiled in.
#[cfg(target_arch = "wasm32")]
pub fn is_known_codec_extension(ap: &AnchorPath) -> bool {
    CODECS.get(ap).is_some()
}

pub fn normalize_path_extension_impl(path: &str) -> String {
    let anchor_path = AnchorPath::new(path);

    // Case 1: empty extension — network/directory entry.
    // Must be checked before CODECS.get() because the non-WASM codec map
    // registers the NetworkCodec with an empty stem+ext match and would
    // otherwise intercept these paths and produce "mynetwork.html" instead
    // of "mynetwork/index.html".
    if anchor_path.ext().is_empty() {
        // Empty path is the root network — always produces "index.html".
        // Non-empty directory paths use join so any anchor fragment is preserved,
        // e.g. "mynetwork#section" → "mynetwork/index.html#section".
        if path.is_empty() {
            return "index.html".to_string();
        }
        return anchor_path.join("index.html").to_string();
    }

    // Case 2: already .html — leave unchanged (re-attach anchor below)
    if anchor_path.ext() == "html" {
        return anchor_path.to_string();
    }

    // Case 3: known codec extension — consult the runtime-aware extension check
    // so that extensions registered by application shims (e.g. .yaml, .h) are
    // normalised to .html. On WASM, this checks the CodecMap's runtime extension
    // set (populated from codecs.json); on native, it checks both CODECS and
    // WALK_CODECS registries. Falls back to BUILTIN_EXTENSIONS to cover
    // feature-gated codecs (xlsx, ods) compiled without --features xlsx.
    if is_known_codec_extension(&anchor_path) || BUILTIN_EXTENSIONS.contains(&anchor_path.ext()) {
        return anchor_path.replace_extension("html");
    }

    // Case 4: unrecognized extension — likely a dotted directory name
    // (e.g. "env.nightly.build-42") where AnchorPath::new
    // misidentified the last dot-separated segment as a file extension.
    //
    // In the HTML-output context, the only valid file extensions are "html"
    // (Case 2) and known codec extensions (Case 3). An unrecognized
    // extension cannot be a compiled document. All callers guard against
    // asset-namespace BIDs (extract_node_context, resolve_related_path)
    // before calling this function, so this path is always a directory
    // entry that needs /index.html appended.
    let dir_ap = AnchorPath::new_dir(path);
    dir_ap.join("index.html").to_string()
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use std::path::{Path, PathBuf};

    use crate::{codec::md::MdCodec, tests::helpers::init_logging};

    use super::*;

    fn test_md_factory() -> Box<dyn DocCodec + Send> {
        Box::new(MdCodec::new())
    }

    #[test]
    fn test_walk_codecs_md() {
        assert!(WALK_CODECS.should_track(Path::new("foo.md")));
        assert!(WALK_CODECS.should_track(Path::new("index.md")));
        assert!(!WALK_CODECS.should_track(Path::new("foo.rs")));
        assert!(!WALK_CODECS.should_track(Path::new("Makefile")));
    }

    #[test]
    fn test_walk_codecs_yaml() {
        assert!(WALK_CODECS.should_track(Path::new("data.yaml")));
        assert!(WALK_CODECS.should_track(Path::new("data.yml")));
        assert!(!WALK_CODECS.should_track(Path::new("data.json")));
    }

    #[test]
    fn test_claim_map_roundtrip() {
        let map = ClaimMap::create();
        let path = PathBuf::from("/tmp/test.yaml");
        assert!(map.get(&path).is_none());
        assert_eq!(map.len(), 0);
        map.claim(path.clone(), || Box::new(UnclaimedDataCodec));
        assert!(map.get(&path).is_some());
        assert_eq!(map.len(), 1);
        map.unclaim(&path);
        assert!(map.get(&path).is_none());
        assert_eq!(map.len(), 0);
    }

    #[test]
    fn test_claim_map_reject_propagates_to_descendants() {
        // When a non-subnet directory is rejected by a network's blacklist,
        // files nested inside that directory must also be considered rejected.
        // This prevents asset processing of media files in blacklisted dirs
        // like `report.media/ppt/media/image8.png`.
        let map = ClaimMap::create();

        // Reject a directory (as NetworkCodec::parse does for blacklisted children).
        let rejected_dir = PathBuf::from("/repo/catalog/widget/report.media");
        map.reject(rejected_dir.clone());

        // The directory itself is rejected.
        assert!(map.is_rejected(&rejected_dir));

        // Files inside the rejected directory must also be rejected.
        assert!(map.is_rejected(&PathBuf::from(
            "/repo/catalog/widget/report.media/ppt/media/image8.png"
        )));
        assert!(map.is_rejected(&PathBuf::from(
            "/repo/catalog/widget/report.media/image1.png"
        )));

        // Files outside the rejected directory are not affected.
        assert!(!map.is_rejected(&PathBuf::from("/repo/catalog/widget/index.md")));
        assert!(!map.is_rejected(&PathBuf::from("/repo/catalog/widget/other-doc.md")));
    }

    #[test]
    fn test_unclaimed_data_codec_produces_no_nodes() {
        let codec = UnclaimedDataCodec;
        assert!(codec.nodes().is_empty());
    }

    /// Helper: Create a test network directory with index.md file
    fn create_test_network(dir: &std::path::Path) {
        std::fs::write(
            dir.join("index.md"),
            r#"---
id: "test-network"
title: "Test Network"
---

# Test Network

Test network for unit tests.
"#,
        )
        .unwrap();
    }

    #[test]
    fn test_codec_factory_creates_fresh_instances() {
        // Plain .md files are no longer registered in CODECS by extension —
        // they are claimed per-network via CLAIM_MAP.
        let ap = AnchorPath::new("README.md");
        assert!(
            CODECS.get(&ap).is_none(),
            "README.md should NOT be in CODECS (claimed via CLAIM_MAP instead)"
        );

        // index.md IS registered directly (by stem+ext) and should return a factory.
        let index_ap = AnchorPath::new("index.md");
        let factory = CODECS.get(&index_ap).expect("index.md codec should exist");

        // Create two instances and verify they are separate (different addresses).
        let codec1 = factory();
        let codec2 = factory();
        let ptr1 = &*codec1 as *const dyn DocCodec;
        let ptr2 = &*codec2 as *const dyn DocCodec;
        assert_ne!(ptr1, ptr2, "Factory should create separate instances");
    }

    #[test]
    fn test_codec_factory_extensions() {
        let patterns = CODECS.registered_patterns();
        // The bare (None, Some("md")) wildcard entry was removed.
        // Plain .md files are claimed per-network via CLAIM_MAP, not CODECS.
        // The stem-qualified (Some("index"), Some("md")) entry still exists for index.md.
        assert!(
            !patterns
                .iter()
                .any(|(stem, ext)| stem.is_none() && ext.as_ref().is_some_and(|e| e == "md")),
            "bare (None, Some('md')) should not be registered in CODECS"
        );
    }

    // --- normalize_path_extension_impl ---
    #[test]
    fn test_normalize_codec_extension_md() {
        assert_eq!(
            normalize_path_extension_impl("docs/guide.md"),
            "docs/guide.html"
        );
    }

    #[test]
    fn test_normalize_codec_extension_md_with_anchor() {
        assert_eq!(
            normalize_path_extension_impl("net1/hsml.md#definition"),
            "net1/hsml.html#definition"
        );
    }

    #[test]
    fn test_normalize_already_html() {
        assert_eq!(
            normalize_path_extension_impl("pages/index.html"),
            "pages/index.html"
        );
    }

    #[test]
    fn test_normalize_already_html_with_anchor() {
        assert_eq!(
            normalize_path_extension_impl("pages/doc.html#section"),
            "pages/doc.html#section"
        );
    }

    #[test]
    fn test_normalize_asset_pdf() {
        // Asset paths never reach this function in practice — all callers
        // guard against asset-namespace BIDs before calling normalize.
        // With the Case 4 fix, unrecognized extensions are treated as
        // dotted directory names and get /index.html appended.
        assert_eq!(
            normalize_path_extension_impl("assets/test_doc.pdf"),
            "assets/test_doc.pdf/index.html"
        );
    }

    #[test]
    fn test_normalize_asset_png() {
        assert_eq!(
            normalize_path_extension_impl("images/photo.png"),
            "images/photo.png/index.html"
        );
    }

    #[test]
    fn test_normalize_dotted_directory_name() {
        // Dotted directory names (e.g. versioned build directories) must be
        // treated as directories, not files with exotic extensions.
        assert_eq!(
            normalize_path_extension_impl("catalog/env.nightly.build-42"),
            "catalog/env.nightly.build-42/index.html"
        );
    }

    #[test]
    fn test_normalize_dotted_directory_name_simple() {
        assert_eq!(normalize_path_extension_impl("v1.2"), "v1.2/index.html");
    }

    #[test]
    fn test_normalize_network_empty_path() {
        // Empty path — directory entry, append /index.html
        assert_eq!(normalize_path_extension_impl(""), "index.html");
    }

    #[test]
    fn test_normalize_network_dir_path() {
        assert_eq!(
            normalize_path_extension_impl("mynetwork"),
            "mynetwork/index.html"
        );
    }

    #[test]
    fn test_normalize_nested_dir_path() {
        assert_eq!(
            normalize_path_extension_impl("net/subdir"),
            "net/subdir/index.html"
        );
    }

    #[cfg(feature = "xlsx")]
    #[test]
    fn test_normalize_codec_extension_xlsx() {
        assert_eq!(
            normalize_path_extension_impl("data/requirements.xlsx"),
            "data/requirements.html"
        );
    }

    #[cfg(feature = "xlsx")]
    #[test]
    fn test_normalize_codec_extension_xlsx_with_anchor() {
        assert_eq!(
            normalize_path_extension_impl("data/requirements.xlsx#section"),
            "data/requirements.html#section"
        );
    }

    #[cfg(feature = "xlsx")]
    #[test]
    fn test_normalize_codec_extension_ods() {
        assert_eq!(
            normalize_path_extension_impl("data/items.ods"),
            "data/items.html"
        );
    }

    // --- existing tests ---

    #[test]
    fn test_builtin_extensions() {
        init_logging();
        use crate::paths::as_extension;
        let map = CodecMap::create();
        for ext in BUILTIN_EXTENSIONS.iter() {
            // "md" is no longer registered as a bare extension in CODECS —
            // plain .md files are claimed per-network via CLAIM_MAP.
            // BUILTIN_EXTENSIONS still lists "md" (used by normalize_path_extension_impl
            // and the WASM codec map), but CODECS.get() will return None for "foo.md".
            if *ext == "md" {
                let ap_str = format!("foo{}", as_extension(ext));
                let ap = AnchorPath::new(&ap_str);
                assert!(
                    map.get(&ap).is_none(),
                    "'md' extension should not be in CODECS (claimed via CLAIM_MAP)"
                );
                continue;
            }
            // xlsx/ods are only registered in CODECS when the xlsx feature is enabled.
            // BUILTIN_EXTENSIONS is always authoritative; the codec registration is feature-gated.
            #[cfg(not(feature = "xlsx"))]
            if *ext == "xlsx" || *ext == "ods" {
                continue;
            }
            let ap_str = format!("foo{}", as_extension(ext));
            let ap = AnchorPath::new(&ap_str);
            assert!(map.get(&ap).is_some());
        }
    }

    #[test]
    fn test_codec_factory_stems() {
        let patterns = CODECS.registered_patterns();

        // Verify BeliefNetwork codec is registered by stem
        assert!(patterns
            .iter()
            .any(|(stem, _)| stem.as_ref().is_some_and(|s| s == "index")));

        // The bare (None, Some("md")) extension entry has been removed.
        // Only the stem-qualified (Some("index"), Some("md")) entry should appear.
        assert!(
            !patterns
                .iter()
                .any(|(stem, ext)| stem.is_none() && ext.as_ref().is_some_and(|e| e == "md")),
            "bare (None, Some('md')) should not be registered in CODECS"
        );
    }

    #[test]
    fn test_codec_factory_get_nonexistent() {
        let ap = AnchorPath::new("nonexistent.xyz");
        let result = CODECS.get(&ap);
        assert!(result.is_none(), "result: {result:?}");
    }

    #[test]
    fn test_codec_factory_get_by_stem() {
        // Test that index.md filestem matches BeliefNetwork codec
        let ap = AnchorPath::new("index.md");
        let result = CODECS.get(&ap);
        assert!(result.is_some());
    }

    #[test]
    fn test_wasm_extensions_match_builtin() {
        // Verify WASM build would have same extensions as non-WASM.
        // xlsx/ods are only registered when the xlsx feature is active.
        // "md" is intentionally absent from CODECS.extensions() — it is in
        // BUILTIN_EXTENSIONS for normalize_path_extension_impl / WASM, but
        // plain .md files are claimed per-network via CLAIM_MAP, not CODECS.
        let extensions = CODECS.extensions();
        for builtin in BUILTIN_EXTENSIONS {
            if *builtin == "md" {
                // Intentionally not in CODECS extensions; skip.
                continue;
            }
            #[cfg(not(feature = "xlsx"))]
            if *builtin == "xlsx" || *builtin == "ods" {
                continue;
            }
            assert!(
                extensions.contains(&builtin.to_string()),
                "Missing builtin extension: '{}'",
                builtin
            );
        }
    }

    #[test]
    fn test_codec_insert_with_stem() {
        let codecs = CodecMap::create();

        // Insert a custom codec by stem
        codecs.insert_codec(Some("custom".to_string()), None, || {
            Box::new(MdCodec::default())
        });

        // Verify it can be retrieved
        let result = codecs.get(&AnchorPath::new("custom.xyz"));
        assert!(result.is_some());
    }

    #[test]
    fn test_path_get_extensionless_file_is_not_a_codec() {
        // Extensionless files like Gemfile and Makefile must NOT match the (None, None)
        // NetworkCodec wildcard. path_get uses AnchorPath::new_file when path.is_file(),
        // so this test requires a real file on disk.
        let temp_dir = tempfile::tempdir().unwrap();
        let gemfile = temp_dir.path().join("Gemfile");
        std::fs::write(&gemfile, "source 'https://rubygems.org'\n").unwrap();
        let makefile = temp_dir.path().join("Makefile");
        std::fs::write(&makefile, "all:\n\techo hi\n").unwrap();

        assert!(
            CODECS.path_get(&gemfile).is_none(),
            "Gemfile should not match any codec"
        );
        assert!(
            CODECS.path_get(&makefile).is_none(),
            "Makefile should not match any codec"
        );

        // Sanity-check: a real directory does match (via the (None,None) wildcard)
        assert!(
            CODECS.path_get(temp_dir.path()).is_some(),
            "A directory should still match the NetworkCodec wildcard"
        );
    }

    #[test]
    fn test_noet_path_with_full_path() {
        use crate::paths::path::AnchorPath;

        // Test with full path
        let ap = AnchorPath::new("/tmp/.tmpm0D4CB/index.md");
        let (stem, ext) = ap.path_parts();

        assert_eq!(stem, "index", "Codec stem should be index");
        assert_eq!(ext, "md", "Extension should be empty");

        // Verify codec lookup works
        let result = CODECS.get(&ap);
        assert!(result.is_some(), "Should find codec for index.md");
    }

    #[tokio::test]
    async fn test_parse_content_returns_owned_codec() {
        use crate::codec::builder::GraphBuilder;
        use crate::codec::proto_index::ProtoIndex;
        use tempfile::TempDir;

        // Create temporary directory with a test markdown file
        let temp_dir = TempDir::new().unwrap();
        create_test_network(temp_dir.path());
        let test_file = temp_dir.path().join("test.md");
        let content = "# Test Document\n\nThis is a test.";
        std::fs::write(&test_file, content).unwrap();
        // Seed CLAIM_MAP so parse_content can resolve a codec for test.md
        // (no bare .md entry in CODECS since removal of wildcard entry).
        // Claim both paths: initialize_stack receives the non-canonicalized path from
        // parse_content's caller, while parse_content itself canonicalizes before lookup
        // (e.g. /var -> /private/var on macOS). Both must be registered.
        let canonical_test_file = test_file.canonicalize().unwrap();
        CLAIM_MAP.claim(test_file.clone(), test_md_factory);
        CLAIM_MAP.claim(canonical_test_file, test_md_factory);

        // Create builder with directory as root
        let mut builder = GraphBuilder::new(temp_dir.path(), None).unwrap();

        // Parse with factory method - should return owned codec
        let session_bb = builder.session_bb().clone();
        let proto_index = ProtoIndex::build(builder.repo_root(), false).unwrap_or_default();
        let result = builder
            .parse_content(&test_file, content.to_string(), session_bb, proto_index, 1)
            .await;

        assert!(
            result.is_ok(),
            "parse_content should succeed: {:?}",
            result.as_ref().err()
        );
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
        init_logging();
        use crate::codec::builder::GraphBuilder;
        use crate::codec::proto_index::ProtoIndex;
        use tempfile::TempDir;

        // Create temporary directory with a test markdown file
        let temp_dir = TempDir::new().unwrap();
        create_test_network(temp_dir.path());
        let test_file = temp_dir.path().join("test.md");
        let content = "# Test Document\n\nThis is a test with a [link](other.md).";
        std::fs::write(&test_file, content).unwrap();
        // Seed CLAIM_MAP so parse_content can resolve a codec for test.md
        // (no bare .md entry in CODECS since removal of wildcard entry).
        // Claim both paths: initialize_stack receives the non-canonicalized path from
        // parse_content's caller, while parse_content itself canonicalizes before lookup
        // (e.g. /var -> /private/var on macOS). Both must be registered.
        let canonical_test_file = test_file.canonicalize().unwrap();
        CLAIM_MAP.claim(test_file.clone(), test_md_factory);
        CLAIM_MAP.claim(canonical_test_file, test_md_factory);

        // Create builder with directory as root
        let mut builder = GraphBuilder::new(temp_dir.path(), None).unwrap();

        // Parse with factory method - should return owned codec
        let session_bb = builder.session_bb().clone();
        let proto_index = ProtoIndex::build(builder.repo_root(), false).unwrap_or_default();
        let result = builder
            .parse_content(&test_file, content.to_string(), session_bb, proto_index, 1)
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

        let (output_filename, pairs, _layout) = &fragments[0];
        let (_, html_body) = &pairs[0];
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

    // --- WalkCodec::network_filenames / WalkCodecMap::is_network_file ---

    #[test]
    fn test_walk_codec_network_filenames_default_is_empty() {
        // Built-in walk codecs should not declare any network filenames.
        let md = MdWalkCodec;
        assert!(md.network_filenames().is_empty());
        let yaml = YamlWalkCodec;
        assert!(yaml.network_filenames().is_empty());
    }

    #[test]
    fn test_walk_codecs_is_network_file_index_md() {
        // NETWORK_NAME is always a network file, even without any registered codecs.
        assert!(WALK_CODECS.is_network_file("index.md"));
    }

    #[test]
    fn test_walk_codecs_is_network_file_rejects_unknown() {
        assert!(!WALK_CODECS.is_network_file("README.md"));
        assert!(!WALK_CODECS.is_network_file("data.yaml"));
        assert!(!WALK_CODECS.is_network_file(""));
    }

    #[test]
    fn test_walk_codecs_network_filenames_includes_network_name() {
        let names = WALK_CODECS.network_filenames();
        assert!(names.contains(&"index.md".to_string()));
    }

    #[test]
    fn test_walk_codec_map_custom_network_filename() {
        // Create an isolated WalkCodecMap to avoid polluting the global registry.
        let map = WalkCodecMap::create();

        struct TestNetworkWalkCodec;
        impl WalkCodec for TestNetworkWalkCodec {
            fn should_track(&self, path: &Path) -> bool {
                path.file_name().and_then(|n| n.to_str()) == Some("Manifest.toml")
            }
            fn tracked_extensions(&self) -> Vec<&'static str> {
                vec!["toml"]
            }
            fn network_filenames(&self) -> Vec<&'static str> {
                vec!["Manifest.toml"]
            }
        }

        // Before registration: only index.md is a network file.
        assert!(map.is_network_file("index.md"));
        assert!(!map.is_network_file("Manifest.toml"));

        // After registration: both are network files.
        map.register(Box::new(TestNetworkWalkCodec));
        assert!(map.is_network_file("index.md"));
        assert!(map.is_network_file("Manifest.toml"));
        assert!(!map.is_network_file("README.md"));

        // network_filenames returns both, deduplicated.
        let names = map.network_filenames();
        assert!(names.contains(&"index.md".to_string()));
        assert!(names.contains(&"Manifest.toml".to_string()));
    }

    #[test]
    fn test_is_network_index_file() {
        assert!(is_network_index_file(Path::new("/repo/docs/index.md")));
        assert!(is_network_index_file(Path::new("index.md")));
        assert!(!is_network_index_file(Path::new("README.md")));
        assert!(!is_network_index_file(Path::new("data.yaml")));
        assert!(!is_network_index_file(Path::new("")));
    }
}
