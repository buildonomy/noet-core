//! This codec module provides the [BeliefSetAccumulator] parser interface, the [DocCodec] trait,
//! and other support structures for parsing and integrating source documents into a BeliefNetwork.
//!
//! ## The Three Sources of Truth
//!
//! The `BeliefSetAccumulator` is a long-running, stateful service responsible for parsing
//! source documents and reconciling them into a coherent, queryable `BeliefSet`. Its core
//! challenge is to manage the "three sources of truth" that exist in the system:
//!
//! 1.  **The Parsed Document:** When a file is read from the filesystem, its content is
//!     considered the absolute source of truth for its own text and the **order** of its
//!     internal components (e.g., the sequence of subsections). The accumulator must
//!     trust this order implicitly.
//!
//! 2.  **The Local Cache (`self.set`):** This is the accumulator's in-memory representation
//!     of the entire filesystem tree it has parsed. It acts as a cache to resolve
//!     cross-document links within the same filesystem without needing to query the
//!     database. It is the source of truth for the **context** of a document within its
//!     local network.
//!
//! 3.  **The Global Cache (Database):** This is the persistent, canonical store of all
//!     `BeliefNode`s. It is the ultimate source of truth for the **identity** (the BID)
//!     of any belief that has been seen before, potentially across different filesystems
//!     or networks. The accumulator queries the global cache to canonicalize references
//!     and injects newly discovered BIDs back into source documents.
//!
//! The accumulator's primary job is to mediate between these three sources, generating: 1)
//! `BeliefEvent`s that update the global cache to reflect the current state of the source
//! documents; and 2) The context necessary to inject BIDs back into source documents in order to
//! maintain absolute references.
//!
//! Maintaining this synchronization enables cross-document and cross project synchronization,
//! enabling tools to inform upstream consumers of document information about internal changes to
//! those documents. For example, if a subsection title changes within a document, it's possible to
//! re-write that title as link text within external documents.
//!
//! ## The Parsing Flow: `self.set` vs `stack_cache`
//!
//! The `BeliefSetAccumulator` maintains two separate `BeliefSet` instances during parsing:
//!
//! - **`self.set`**: The accumulator's local cache representing the NEW state being built from
//!   parsing. This is the source of truth for what the documents currently contain.
//!
//! - **`stack_cache`**: A temporary cache representing the OLD state - what existed in the global
//!   cache before this parse operation. This is populated via `merge()` operations in
//!   `cache_fetch()` when resolving node identities.
//!
//! ### Parsing Lifecycle:
//!
//! 1. **`initialize_stack`**: Clears `stack_cache` to start fresh for this parse operation.
//!
//! 2. **During parsing (`push`)**:
//!    - `cache_fetch` queries the global cache and populates `stack_cache` via `merge()`,
//!      which includes both nodes and their relationships. This builds a snapshot of the old state.
//!    - Remote events are processed into `self.set` only, building the new truth from document content.
//!    - `self.set` and `stack_cache` intentionally diverge during this phase.
//!
//! 3. **`terminate_stack`**: Reconciles the two caches:
//!    - Compares `self.set` (new parsed state) against `stack_cache` (old cached state)
//!    - Identifies nodes that existed before but are no longer referenced in the parsed content
//!    - Generates `NodesRemoved` events for the differences
//!    - Sends these reconciliation events to both `stack_cache` and the transmitter (for global cache)
//!
//! This two-cache architecture enables the accumulator to detect what was removed from a document
//! by comparing the old and new manifolds, then propagating those removals to other caches.
//!
//! ## Parsing and Re-Writing Links in Source Materials
//!
//! Links are super important. Buildonomy treats all links within source material as a
//! bi-directional reference. Links are one of the only places Buildonomy will edit a source
//! document directly, the other being metadata blocks. The intentions of how Buildonomy treats
//! links are to simultaneously satisfy the following constraints:
//!
//! - Preserve legibility of the raw source document. participants should be able to manually navigate to a
//!   referenced source document without complicated tools. participants should be able to infer what the
//!   link contains based on the link reference description.
//!
//! - Auto-update link descriptions when the reference title changes, unless the link description
//!   is explicitly specified separate from the link reference.
//!
//! - track references-to (sinks) for everything important enough to put in a doc,
//!   even for external sources.
//!
//! - Be able to cache_fetch a node that can navigate to an external reference
//!   simply by failing resolution of the reference's NodeKey. (preserve schema,
//!   host, etc.)
//!
//! - Treat url anchors as unique nodes, not just the anchored document.
//!
//! Links are all epistemic within the text of a node. Pragmatic and/or Subsection references will
//! appear in the metadata. Implementation of these features is handled within the interaction
//! between [BeliefSetAccumulator::cache_fetch] and [crate::nodekey::href_to_nodekey].
//!
//! ## On Linking
//!
//! Buildonomy requires links to be easily interpretable by practitioners reading raw source
//! documents as well by the software parsing those documents into a Belief Network. Source
//! documents are assumed to be constantly evolving, and the links must remain interpretable even as
//! either source or reference material evolves.
//!
//! Beliefs are inter-related in many different ways. Belief Networks are beliefs defined by
//! sub-beliefs. Beliefs are represented across different media, A BeliefNetwork captures the
//! inter-relations between these elements and imbues them with contextual meaning.  Documents
//! encode procedures of enacting some portion of that intention. Procedure documents are
//! constituted from the relationship of the belief symbols encoded in the underlying source
//! document. Belief networks may depend on other networks, sourcing their material in order to
//! construct more complex relationships within their primary procedures.
//!
//! Each of these types of beliefs (networks, procedure documents, and procedure sub-elements (symbols))
//! is constantly changing and their inter-relationships must stay coherent as the beliefs and their
//! relationships evolve.
//!
//! Within source documents, relative links should be prioritized such that the meaning of a
//! reference is easily understood when reading the source. Titles are preferred anchors, unless
//! those titles are not unique, in which case `/source/network/relative/doc_path#node_index` should
//! be used.
//!
//! Within the instantiated network cache, nodes should be referenced by [crate::properties::Bid].
//! If a Bid is not available within a source, one should be created and then inserted back into the
//! source. [BeliefSetAccumulator::cache_fetch] is responsible for generating an appropriate
//! [BeliefNode] when necessary.
//!
//! Relative paths are complicated because networks containing sub-networks should be able to access
//! their dependencies via relative paths. Similar to git submodules, Sub-Networks are installed
//! within the primary network by a relative path. [crate::paths] is responsible for parsing and
//! accessing nodes based on relative path information.
//!
//! The paths should be independent of any [crate::properties::Bid], so that a query can resolve
//! references across different contexts. For URIs and unresolved references,
//! [BeliefSetAccumulator::cache_fetch] returns an `UnresolvedReference` diagnostic rather than
//! creating a placeholder node. These unresolved references are tracked in `ParseDiagnostic` enums
//! and drive the multi-pass resolution algorithm in `BeliefSetParser`. In this way, a belief
//! network can model references to external resources or not-yet-parsed documents without polluting
//! the cache with incomplete nodes.
//!
//! We cannot assume that all relations are immediately accessible during parsing. Unresolved
//! references represent *promises* that something useful to the network exists and will be resolved
//! in subsequent parse passes. The `BeliefSetParser` maintains a two-queue architecture (primary
//! queue for never-parsed files, reparse queue for files with unresolved dependencies) to handle
//! this multi-pass resolution efficiently.
//!
//! ## Relative paths
//!
//! [crate::beliefset::BeliefSet::paths], which is cached/instantiated with respect to the loaded
//! network(s) tracks:
//!
//! - relative paths, anchored with respect to each network 'sink', that depends on its immediate network
//! - external URLs: Treated as absolute paths. If not resolvable, returned as `UnresolvedReference`.
//! - resolved references: When a reference is resolved (BID found), it is synchronized with the
//!   source document and the cache. The parser tracks which files need reparsing when dependencies
//!   become available.
//! - relative paths are not intrinsic to any node. Instead they are a *property relative to the spatial
//!   structure of the network*. They aren't a node property, they are a relation property.
//!
//! Relative paths change when documents are restructured or renamed, so they cannot be relied on to
//! compare between document versions automatically. THIS is where the most complexity lies. If
//! sections are re-ordered, document indexing won't stay consistent for anchors. If titles are
//! non-unique or changed we cannot rely on title slugs as anchors. We must rely on their BIDs, but
//! those BIDs are human-illegible, so after querying based on bid we must translate back into a
//! relative link format.
//!
//! SO. The protocol around references requires the following:
//!
//! - If a parsed node (proto node) does not have a BID encoded in the source material, one must be
//!   generated and written back to the source.
//!
//! - When parsing a link, if the encoded path is not resolvable, an `UnresolvedReference` diagnostic
//!   is returned. The parser uses this to queue the referenced file for parsing and to track which
//!   files need reparsing once the reference is resolved.
//!
//! - When mapping a reference to an ID, the nearest network must be specified, such that only paths
//!   relative to that network location are considered.
//!
//! - When a subsection reference path changes between version_a and version_b of a document, then
//!   the accumulator must proactively find all sink relationships containing the old relative path
//!   and then propagate event(s) back to the source documents in order to re-write them with the
//!   updated relative links.
//!
//!
//! Why?
//!
//! ## TODO
//!
//! - [ ] enable it to read and write from a text buffer (so it can be installed on the
//!       front-end).
//!

use once_cell::sync::Lazy;
use parking_lot::{Mutex, RwLock};
/// Utilities for parsing various document types into BeliefSets
use std::{result::Result, sync::Arc, time::Duration};

use crate::{
    beliefset::BeliefContext, codec::lattice_toml::ProtoBeliefNode, error::BuildonomyError,
    properties::BeliefNode,
};

pub mod accumulator;
pub mod diagnostic;
pub mod lattice_toml;
pub mod md;
pub mod parser;
pub mod schema_registry;

// Re-export for backward compatibility
pub use accumulator::BeliefSetAccumulator;
pub use diagnostic::{ParseDiagnostic, UnresolvedReference};
pub use parser::BeliefSetParser;

/// Global default codec map with builtin codecs (md, toml)
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
        // Contains the accumulator root-path relative information to seed the parse with
        current: ProtoBeliefNode,
    ) -> Result<(), BuildonomyError>;

    fn nodes(&self) -> Vec<ProtoBeliefNode>;

    fn inject_context(
        &mut self,
        node: &ProtoBeliefNode,
        ctx: &BeliefContext<'_>,
    ) -> Result<Option<BeliefNode>, BuildonomyError>;

    fn generate_source(&self) -> Option<String>;
}

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
            tracing::info!("[DocCodec::insert] Waiting for write access to the codec map");
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
            tracing::info!("[DocCodec::insert] Waiting for read access to the codec map");
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
            tracing::info!("[DocCodec::insert] Waiting for read access to the codec map");
            std::thread::sleep(Duration::from_millis(100));
        }
        let reader = self.0.read_arc();
        reader
            .iter()
            .map(|(codec_ext, _value)| codec_ext.clone())
            .collect::<Vec<String>>()
    }
}
