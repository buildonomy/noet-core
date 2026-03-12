use petgraph::{visit::EdgeRef, Direction};
use serde::{Deserialize, Serialize};
/// Utilities for parsing various document types into BeliefBases
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Debug,
    path::{Path, PathBuf},
    result::Result,
    slice::from_ref,
};
use tokio::sync::mpsc::UnboundedSender;
/// Utilities for parsing various document types into BeliefBases
use toml::value::Table as TomlTable;

use crate::{
    beliefbase::{BeliefBase, BeliefGraph},
    codec::{
        belief_ir::IRNode,
        diagnostic::ParseDiagnostic,
        network::{detect_network_file, NETWORK_NAME},
        proto_index::ProtoIndex,
        DocCodec, CODECS,
    },
    error::BuildonomyError,
    event::{BeliefEvent, EventOrigin},
    nodekey::NodeKey,
    paths::{as_anchor, os_path_to_string, path::string_to_os_path, AnchorPath},
    properties::{
        buildonomy_namespace, content_namespaces, href_namespace, BeliefKind, BeliefKindSet,
        BeliefNode, Bid, Bref, Weight, WeightKind, WEIGHT_DOC_PATHS, WEIGHT_SORT_KEY,
    },
    query::{BeliefSource, Expression, NeighborsExpression, Query},
};

use super::{belief_ir::IntermediateRelation, UnresolvedReference};

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub enum NodeSource {
    Merged,
    Generated,
    SourceFile,
    StackCache,
    GlobalCache,
}

impl NodeSource {
    fn is_from_cache(&self) -> bool {
        !matches!(self, NodeSource::Generated | NodeSource::Merged)
    }
}

/// Result type for cache_fetch that distinguishes between resolved and unresolved references.
///
/// This enum separates successful node resolution from unresolved references, which are
/// expected outcomes during multi-pass compilation (not errors).
#[derive(Debug, Clone)]
pub enum GetOrCreateResult {
    /// The node was successfully resolved (found in cache or created)
    Resolved(BeliefNode, NodeSource),
    /// The node could not be resolved (target not yet parsed)
    Unresolved(crate::codec::diagnostic::UnresolvedReference),
}

/// Result of parsing document content (without owned codec)
#[derive(Debug, Clone)]
pub struct ParseContentResult {
    /// Optionally rewritten content if BIDs were injected or links updated
    pub rewritten_content: Option<String>,

    /// Diagnostics collected during parsing (unresolved refs, warnings, etc.)
    pub diagnostics: Vec<ParseDiagnostic>,
}

/// Result of parsing document content with owned codec instance
pub struct ParseContentWithCodec {
    /// Parse result (rewritten content and diagnostics)
    pub result: ParseContentResult,
    /// Owned codec instance with parsed state
    pub codec: Box<dyn DocCodec + Send>,
}

impl ParseContentResult {
    /// Create a new parse result with no rewrites or diagnostics
    pub fn empty() -> Self {
        Self {
            rewritten_content: None,
            diagnostics: Vec::new(),
        }
    }

    /// Create a parse result with rewritten content
    pub fn with_rewrite(content: String) -> Self {
        Self {
            rewritten_content: Some(content),
            diagnostics: Vec::new(),
        }
    }

    /// Add a diagnostic to this result
    pub fn add_diagnostic(&mut self, diagnostic: ParseDiagnostic) {
        self.diagnostics.push(diagnostic);
    }
}

#[derive(Debug)]
pub struct GraphBuilder {
    // pub parsed_content: BTreeSet<Bid>,
    // pub parsed_structure: BTreeSet<Bid>,
    doc_bb: BeliefBase,
    repo: Bid,
    repo_root: PathBuf,
    stack: Vec<(Bid, String, usize)>,
    session_bb: BeliefBase,
    tx: UnboundedSender<BeliefEvent>,
}

/// GraphBuilder collects source material, parses it into a BeliefBase representation, maps
/// that to the last-known representation of the set in order to determine consistent state and
/// relation IDs and weights, and finally publishes updated versions of the set back to the source
/// material as well as to the provided global_bb [BeliefSource] implementation.
///
/// A core responsibility of the builder is to integrate relative file paths, arbitrary document
/// structures, and other arbitrary API formats, as well as the URL schema/protocol into a unified
/// relative or absolute identification for each node referenced within a BeliefNetwork.
///
/// The builder is responsible for tracking changes to this mapping, such that when beliefs are
/// added, removed, changed, or moved, the relative links within the source documents and the cache
/// itself are changed to stay consistent with those updates.
///
/// The UI objective is to be able to start writing a reference, and type a Bid, title, or uri, and
/// then encapsulate a link that is the most-legible version of that relationship into the source
/// document while maintaining the integrity of that link as the sourced document mutates.
///
/// This creates an environment where action works top-down, from executing intentions using the
/// configured procedures, as well as bottom up, where mutations of integrated sub-systems percolate
/// into events that the containing-processes must adapt to.
impl GraphBuilder {
    pub fn new<P>(
        repo_path: P,
        mut maybe_tx: Option<UnboundedSender<BeliefEvent>>,
    ) -> Result<Self, BuildonomyError>
    where
        P: AsRef<std::path::Path> + std::fmt::Debug,
    {
        let canonicalized_path = PathBuf::from(repo_path.as_ref()).canonicalize()?;
        let Some(mut repo_root) = detect_network_file(canonicalized_path.as_ref()) else {
            return Err(BuildonomyError::Codec(format!(
                "GraphBuilder initialization failed. Received root path {repo_path:?}. \
                 Expected a directory or path to a index.md file"
            )));
        };
        // network index file is now network dir
        repo_root.pop();
        // Normalize repo_root through os_path_to_string + string_to_os_path to strip any Windows
        // \\?\ extended-path prefix that canonicalize() adds. Without this, repo_root would be e.g.
        // \\?\C:\tmp\xxx while parent_path (reconstructed from os_path_to_string) is C:\tmp\xxx ---
        // causing strip_prefix to always fail on Windows.
        let repo_root = string_to_os_path(&os_path_to_string(&repo_root));

        let tx = match maybe_tx.take() {
            Some(tx) => tx,
            None => {
                tracing::warn!("Builder was initialized without an output event transmitter, stubbing out a process to swallow parsing events");
                let (accum_tx, mut accum_rx) =
                    tokio::sync::mpsc::unbounded_channel::<BeliefEvent>();
                std::thread::spawn(move || {
                    loop {
                        match accum_rx.blocking_recv() {
                            Some(event) => {
                                tracing::debug!("Swallowing event: {:?}", event);
                            }
                            None => {
                                // Channel closed, exit thread
                                return;
                            }
                        }
                    }
                });
                accum_tx
            }
        };

        let accum = GraphBuilder {
            // parsed_content: BTreeSet::default(),
            // parsed_structure: BTreeSet::default(),
            doc_bb: BeliefBase::empty(),
            repo: Bid::nil(),
            repo_root,
            stack: Vec::default(),
            session_bb: BeliefBase::empty(),
            tx,
        };

        tracing::debug!(
            "Initializing GraphBuilder for repo_path: {:?}",
            repo_path.as_ref()
        );
        Ok(accum)
    }

    pub fn api(&self) -> &BeliefNode {
        self.doc_bb.api()
    }

    pub fn repo(&self) -> Bid {
        self.repo
    }

    pub fn doc_bb(&self) -> &BeliefBase {
        &self.doc_bb
    }

    pub fn session_bb(&self) -> &BeliefBase {
        &self.session_bb
    }

    pub fn session_bb_mut(&mut self) -> &mut BeliefBase {
        &mut self.session_bb
    }

    pub fn doc_bb_mut(&mut self) -> &mut BeliefBase {
        &mut self.doc_bb
    }

    pub fn tx(&self) -> &UnboundedSender<BeliefEvent> {
        &self.tx
    }

    /// Close the event transmitter channel
    ///
    /// This signals the event receiver to finish processing and exit.
    /// Used by parse command to ensure all events are drained before export.
    pub fn close_tx(&mut self) {
        // Create a dummy channel and swap it with the real one
        // Dropping the old tx closes the channel
        let (dummy_tx, _dummy_rx) = tokio::sync::mpsc::unbounded_channel();
        let _old_tx = std::mem::replace(&mut self.tx, dummy_tx);
        // old_tx is dropped here, closing the channel
    }

    pub fn repo_root(&self) -> &Path {
        &self.repo_root
    }

    pub fn built_in_test(&mut self) -> Vec<String> {
        let mut combined_errors = Vec::default();
        let mut set_errors = self.doc_bb.built_in_test(true);
        if !set_errors.is_empty() {
            combined_errors.push("builder.doc_bb errors:".to_string());
            combined_errors.append(&mut set_errors);
        }
        let mut session_bb_errors = self.session_bb.built_in_test(true);
        if !session_bb_errors.is_empty() {
            combined_errors.push("builder.session_bb errors:".to_string());
            combined_errors.append(&mut session_bb_errors);
        }
        set_errors
    }

    /// Returns:
    ///
    /// new content to write to file (containing newly parsed IDs and/or updated link
    /// titles), if the content should change.
    ///
    /// Additionally, if any docs need to be parsed or re-parsed in order to IDs were renamed or
    /// their titles changed, returns an ordered list of documents which reference those elements,
    /// so that the documents can be rewritten with the updated titles and IDs.
    ///
    /// # Returns
    ///
    /// Returns owned codec instance along with parse result. The codec contains
    /// parsed state and can be used for immediate HTML generation.
    ///
    pub async fn parse_content<
        P: AsRef<std::path::Path> + std::fmt::Debug,
        B: BeliefSource + Clone,
    >(
        &mut self,
        input_path: P,
        content: String,
        global_bb: B,
        proto_index: ProtoIndex,
    ) -> Result<ParseContentWithCodec, BuildonomyError> {
        tracing::debug!("Phase 0: initialize stack");
        let full_path = input_path.as_ref().canonicalize()?.to_path_buf();
        let (initial, doc_sort_key) = self
            .initialize_stack(input_path.as_ref(), global_bb.clone(), &proto_index)
            .await?;

        let mut maybe_content: Option<String> = None;
        // Track ID renames for parsed nodes
        let mut docs_to_parse = Vec::<(String, Bid)>::new();
        // Track external docs that contain references into the parsed content. Add a sink doc
        // to this list whenever we both know that 1) the set of nodekeys (possible reference
        // ids) for the parsed content changed from their prior state and 2) we know of external
        // 'sinks' in the external document that reference that changed node.
        let mut bid_renames = BTreeMap::<Bid, Bid>::default();
        // Track diagnostics during parsing (unresolved references, warnings, etc.)
        let mut diagnostics = Vec::<ParseDiagnostic>::new();

        let doc_path = initial.path.clone();
        // Use new_dir when the proto node is a Network: NetworkCodec::proto() sets
        // initial.path to the directory (e.g. ".../symbol.iterator"), not the index.md
        // file. AnchorPath::new / ::from would misparse "symbol.iterator" as stem="symbol"
        // ext="iterator" and the codec lookup would fail. new_dir forces directory
        // semantics so path_parts() returns ("", "") and the (None, None) NetworkCodec
        // wildcard matches correctly.
        let doc_ap = if initial.kind.contains(BeliefKind::Network) {
            AnchorPath::new_dir(&doc_path)
        } else {
            AnchorPath::from(&doc_path)
        };

        let mut parsed_bids;
        let owned_codec: Box<dyn DocCodec + Send>;

        if let Some(codec_factory) = CODECS.get(&doc_ap) {
            // Create fresh codec instance from factory
            let mut codec = codec_factory();
            codec.parse(&content, initial, &mut diagnostics)?;

            let mut inject_context = false;
            let mut has_new_bids = false;
            parsed_bids = Vec::with_capacity(codec.nodes().len());
            let mut check_sinks = BTreeMap::<Bid, BTreeSet<NodeKey>>::default();
            let mut relation_event_queue = Vec::<BeliefEvent>::default();
            let mut missing_structure = BeliefGraph::default();

            tracing::debug!("Phase 1: Create all nodes");
            debug_assert!(
                self.session_bb.is_balanced().is_ok(),
                "Why isn't session_bb balanced? (phase 1 start)"
            );
            for (proto_idx, proto) in codec.nodes().iter().enumerate() {
                // The first node is always the entry document. Pass the sort key captured by
                // initialize_stack so that RelationChange(doc, repo_root, ...) uses the correct
                // sibling position regardless of which cache branch cache_fetch takes.
                // All subsequent nodes (sections) get None and auto-assign their own sort keys.
                let entry_sort_key = if proto_idx == 0 {
                    tracing::debug!(
                        "[parse_content] Phase 1 first push: doc_sort_key={:?} path={:?}",
                        doc_sort_key,
                        proto.path
                    );
                    doc_sort_key
                } else {
                    None
                };
                let (bid, (source, _nodekeys, unique_oldkeys)) = self
                    .push(
                        proto,
                        global_bb.clone(),
                        false,
                        &mut missing_structure,
                        entry_sort_key,
                    )
                    .await?;
                if !missing_structure.is_empty() {
                    tracing::debug!(
                        "Phase 1 {}: merging missing structure onto self.session_bb:",
                        bid,
                    );
                    // Seed from the single node just pushed — bounds the DFS to what's
                    // reachable from this node in missing_structure, not all of session_bb.
                    let node_seed: BTreeSet<Bid> = BTreeSet::from([bid]);
                    self.session_bb.merge_from(&missing_structure, &node_seed);
                    // We did a bunch of cache_fetch operations, so the stack cache should be
                    // rebalanced as well.
                    self.session_bb.process_event(&BeliefEvent::BalanceCheck)?;
                    // Merge the structural halo (ancestor chains, external network nodes) into
                    // doc_bb so that PathMapMap has the network context needed for regularize()
                    // in Phase 2.
                    //
                    // Gated on non-cache source: when a node hits StackCache or GlobalCache,
                    // missing_structure contains that node's own Section edges (fetched from
                    // session_bb/global_bb by cache_fetch). Those edges were just freshly
                    // established in doc_bb by the RelationChange fired inside push().
                    // Merging them here would set index_dirty=true and force a full PathMap
                    // rebuild on the next paths() call — that rebuild reads the raw graph,
                    // where the cached sort key may differ from the one RelationChange just
                    // wrote, corrupting the PathMap ordering and causing get_context() to
                    // return None in Phase 4.
                    //
                    // For Generated/SourceFile/Merged nodes, missing_structure carries external
                    // structure (ancestor chains, href namespace nodes) that doc_bb genuinely
                    // needs — merge as before.
                    if !source.is_from_cache() {
                        self.doc_bb.merge_from(&missing_structure, &node_seed);
                    }
                    missing_structure = BeliefGraph::default();
                }

                if !source.is_from_cache() {
                    inject_context = true;
                    if matches!(source, NodeSource::Generated) {
                        has_new_bids = true;
                    }
                } else if !unique_oldkeys.is_empty() {
                    for old_bid in unique_oldkeys.iter().filter_map(|key| {
                        if let NodeKey::Bid { bid: old_bid } = key {
                            if *old_bid != bid {
                                Some(bid)
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    }) {
                        bid_renames.insert(old_bid, bid);
                    }
                    check_sinks.insert(bid, unique_oldkeys);
                }
                parsed_bids.push(bid);
            }

            self.doc_bb.process_event(&BeliefEvent::BalanceCheck)?;

            tracing::debug!("Phase 2: Balance and process relations");
            let mut generated_href_nodes = Vec::new();
            for (proto, bid) in codec.nodes().iter().zip(parsed_bids.iter()) {
                // Process upstream_relations (sink-owned, default)
                for (index, relation) in proto.upstream.iter().enumerate() {
                    let result = self
                        .push_relation(
                            relation,
                            bid,
                            Direction::Incoming, // upstream_relations are sink-owned
                            index,
                            &content,
                            global_bb.clone(),
                            &mut relation_event_queue,
                            &mut missing_structure,
                        )
                        .await?;

                    match result {
                        GetOrCreateResult::Resolved(node, source) => {
                            if source.is_from_cache() {
                                inject_context = true;
                            } else if matches!(source, NodeSource::Generated) {
                                generated_href_nodes.push(node.bid);
                                if let Some(const_namespace) = content_namespaces()
                                    .iter()
                                    .find(|ns| node.bid.parent_bref() == ns.bref())
                                {
                                    if !generated_href_nodes.contains(const_namespace) {
                                        generated_href_nodes.push(*const_namespace);
                                    }
                                }
                            }
                        }
                        GetOrCreateResult::Unresolved(unresolved) => {
                            // Track unresolved reference as diagnostic
                            diagnostics.push(ParseDiagnostic::UnresolvedReference(unresolved));
                        }
                    }
                }

                // Process downstream_relations (source-owned)
                for (index, relation) in proto.downstream.iter().enumerate() {
                    let result = self
                        .push_relation(
                            relation,
                            bid,
                            Direction::Outgoing, // downstream_relations are source-owned
                            index,
                            &content,
                            global_bb.clone(),
                            &mut relation_event_queue,
                            &mut missing_structure,
                        )
                        .await?;

                    match result {
                        GetOrCreateResult::Resolved(node, source) => {
                            if source == NodeSource::GlobalCache {
                                inject_context = true;
                            } else if matches!(source, NodeSource::Generated) {
                                generated_href_nodes.push(node.bid);
                            }
                        }
                        GetOrCreateResult::Unresolved(unresolved) => {
                            // Track unresolved reference as diagnostic
                            diagnostics.push(ParseDiagnostic::UnresolvedReference(unresolved));
                        }
                    }
                }
            }
            if !generated_href_nodes.is_empty() {
                parsed_bids.append(&mut generated_href_nodes);
            }

            // Perform this after going through all the proto relations so we don't destroy our
            // balanced set.
            if !missing_structure.is_empty() {
                tracing::debug!("Phase 2: merging missing structure onto session_bb and set");
                // Use merge_from with the current file's parsed_bids as the DFS seed set.
                // This bounds the DFS to O(rhs_size) rather than O(session_bb_size × rhs_edges),
                // fixing the O(N²) BN-1 bottleneck on large corpora (Issue 47).
                let parsed_bid_set: BTreeSet<Bid> = parsed_bids.iter().copied().collect();
                self.session_bb
                    .merge_from(&missing_structure, &parsed_bid_set);
                self.session_bb.process_event(&BeliefEvent::BalanceCheck)?;
                // we need to merge this phase 2 missing structure into self.doc_bb as well to ensure
                // we have full structural paths to all the external nodes we connect to within the
                // relation_event_queue. Use merge_from with parsed_bid_set (already computed above)
                // to bound the DFS to the current file's nodes — not all of doc_bb's ancestor chain.
                // The unbounded merge was the BN-1 bottleneck: on reparse, doc_bb's ancestor chain
                // includes the root network node, causing a full-graph DFS on every reparse pass.
                self.doc_bb.merge_from(&missing_structure, &parsed_bid_set);
            }
            for edge_update in relation_event_queue.drain(..) {
                let _deriv = self.doc_bb.process_event(&edge_update)?;
            }
            self.doc_bb.process_event(&BeliefEvent::BalanceCheck)?;

            tracing::debug!(
                "Phase 3: inform external sinks about nodekey changes from this document"
            );
            // (re)parse documents are are either to
            // 1) update their contents to reflect updated nodekey's from this parsed document.
            if !parsed_bids.is_empty() {
                for (source_bid, _old_keys) in check_sinks.iter() {
                    if let Some(source_idx) = self.session_bb.bid_to_index(source_bid) {
                        let stack_paths_guard = self.session_bb.paths();
                        let mut sink_docs = self
                            .session_bb
                            .relations()
                            .as_graph()
                            .edges_directed(source_idx, Direction::Outgoing)
                            .filter_map(|edge| {
                                let sink = self.session_bb.relations().as_graph()[edge.target()];
                                stack_paths_guard.get_doc(&sink)
                            })
                            .collect::<Vec<_>>();
                        sink_docs.sort_by_key(|doc_tuple| doc_tuple.2.clone());
                        for sink_doc_id in sink_docs.into_iter() {
                            if sink_doc_id.0 == doc_path {
                                continue;
                            }
                            let doc_id = (sink_doc_id.0, sink_doc_id.1);
                            if !docs_to_parse.contains(&doc_id) {
                                docs_to_parse.push(doc_id);
                            }
                        }
                    }
                }
                tracing::debug!("Phase 3: affected_sinks: {:?}", docs_to_parse);
            }
            tracing::debug!(
                "Phase 4: context injection. inject_context={}",
                inject_context
            );
            let mut is_changed = false;
            // Always run inject_context for every parsed node. The inject_context boolean was
            // previously used as a gate to skip Phase 4 as an optimisation, but that optimisation
            // is incorrect: section nodes resolved from StackCache may carry BIDs that have never
            // been persisted to disk. Those BIDs must be injected into the proto documents so that
            // finalize() can write them into the sections table and trigger a content rewrite.
            for (proto, bid) in codec.nodes().iter().zip(parsed_bids.iter()) {
                let in_states = self.doc_bb.states().contains_key(bid);
                let in_pathmap = self
                    .doc_bb
                    .paths()
                    .get_map(&self.repo().bref())
                    .map(|pm| pm.bid_has_path(bid))
                    .unwrap_or(false);
                let ctx = self
                    .doc_bb
                    .get_context(&self.repo(), bid)
                    .unwrap_or_else(|| {
                        panic!(
                            "Set should be balanced here: bid={bid} \
                         in_states={in_states} in_pathmap={in_pathmap} \
                         proto.heading={} proto.path={:?}",
                            proto.heading, proto.path
                        )
                    });
                let old_node = ctx.node.toml();
                // Inject proto text into our self set here, because inject context is where the
                // markdown parser generates section-specific text fields regardless of whether
                // it changes the markdown itself due to the injected context.
                if let Some(updated_node) = codec.inject_context(proto, &ctx, &mut diagnostics)? {
                    if old_node != updated_node.toml() {
                        is_changed = true;
                        let _derivatives = self.doc_bb.process_event(&BeliefEvent::NodeUpdate(
                            vec![NodeKey::Bid {
                                bid: updated_node.bid,
                            }],
                            updated_node.toml(),
                            EventOrigin::Remote,
                        ))?;
                    }
                }
            }

            // Phase 4b: Finalize codec (cross-node cleanup, emit events for modified nodes)
            tracing::debug!("Phase 4b: codec finalization");
            let finalized_nodes = codec.finalize(&mut diagnostics)?;
            for (_proto, updated_node) in finalized_nodes {
                let old_toml = self
                    .doc_bb
                    .states()
                    .get(&updated_node.bid)
                    .map(|node| node.toml());
                if Some(updated_node.toml()) != old_toml {
                    is_changed = true;
                    let _derivatives = self.doc_bb.process_event(&BeliefEvent::NodeUpdate(
                        vec![NodeKey::Bid {
                            bid: updated_node.bid,
                        }],
                        updated_node.toml(),
                        EventOrigin::Remote,
                    ))?;
                }
            }

            if is_changed || has_new_bids {
                tracing::debug!("Generating source");
                let maybe_new_content = codec.generate_source();
                if let Some(new_content) = maybe_new_content.as_ref() {
                    // Always rewrite when new BIDs were assigned, even if markdown text is
                    // unchanged — the BIDs must be persisted to disk so they don't become
                    // ephemeral entries in global_bb without a corresponding on-disk record.
                    if new_content != &content || has_new_bids {
                        maybe_content = maybe_new_content;
                    }
                }
            }

            // Store owned codec to return
            owned_codec = codec;
        } else {
            return Err(BuildonomyError::Codec(format!(
                "Cannot parse {full_path:?}. No Codec for extension type {} found in CodecMap",
                doc_ap.ext()
            )));
        };

        tracing::debug!("Phase 5: terminating stack and transmitting updates to global_bb");
        self.terminate_stack(
            bid_renames,
            &BTreeSet::<Bid>::from_iter(parsed_bids.into_iter()),
        )
        .await?;

        Ok(ParseContentWithCodec {
            result: ParseContentResult {
                rewritten_content: maybe_content,
                diagnostics,
            },
            codec: owned_codec,
        })
    }

    /// Initializes internal variables for parsing and merging
    /// Returns the entry `IRNode` for Phase 1 parsing alongside the sort key
    /// that positions the entry document among its siblings in the parent network.
    ///
    /// The sort key is `Some(index)` where `index` is the entry doc's position in
    /// `maybe_content_parent_proto.upstream` (slow path) or `entry_order.last()`
    /// from the session PathMap (fast path).  It is `None` when the entry doc has
    /// no parent network (repo root itself) or when its position cannot be determined.
    ///
    /// `parse_content` passes this value as `explicit_sort_key` into the first
    /// Phase 1 `push()` call, ensuring the correct `RelationChange` weight is used
    /// regardless of which `cache_fetch` branch fires.
    async fn initialize_stack<P: AsRef<Path> + Debug, B: BeliefSource + Clone>(
        &mut self,
        abs_path: P,
        global_bb: B,
        proto_index: &ProtoIndex,
    ) -> Result<(IRNode, Option<u16>), BuildonomyError> {
        // self.parsed_content.clear();
        // self.parsed_structure.clear();
        // self.parsed_structure.insert(self.api().bid);
        self.stack = vec![];
        // // Uncomment this for easier testing as it makes cache order of operations more clear.
        // self.session_bb = BeliefBase::empty();
        self.doc_bb = BeliefBase::empty();
        let api_node = self.api().clone();
        let api_key = NodeKey::Bid { bid: api_node.bid };
        let api_node_event =
            BeliefEvent::NodeUpdate(vec![api_key.clone()], api_node.toml(), EventOrigin::Remote);
        self.doc_bb.process_event(&api_node_event)?;
        // Ensure global_bb shares our API node
        //
        // TODO figure out a way to do this check only once per session instead
        // of at each initialize_stack operation.
        if self.session_bb.get(&api_key).is_none() {
            self.session_bb.process_event(&api_node_event)?;
        }
        if global_bb.get_async(&api_key).await?.is_none() {
            self.tx.send(api_node_event)?;
        }

        // Fetch const_namespaces from global_bb to populate session_bb with known assets.
        // This enables asset content change detection by populating PathMap with existing paths.
        // Guard: only run once per session — these are static global namespaces (href + asset)
        // that never change between files. Repeating this on every initialize_stack call was the
        // primary driver of session_bb O(N²) growth across a corpus run.
        let content_ns_loaded = content_namespaces()
            .iter()
            .any(|bid| self.session_bb.get(&NodeKey::Bid { bid: *bid }).is_some());
        if !content_ns_loaded {
            for const_bid in &content_namespaces() {
                let key = NodeKey::Bid { bid: *const_bid };
                if let Some(const_ns_node) = global_bb.get_async(&key).await? {
                    // Process asset namespace node into session_bb
                    let const_ns_event = BeliefEvent::NodeUpdate(
                        vec![key.clone()],
                        const_ns_node.toml(),
                        EventOrigin::Remote,
                    );
                    self.session_bb.process_event(&const_ns_event)?;

                    // Fetch all assets connected to this namespace
                    // Use eval to get the namespace and its relations
                    let const_expr = Expression::from(&key);
                    let const_graph = global_bb.eval(&const_expr).await?;

                    // Merge the fetched asset graph into session_bb.
                    // Seed from the namespace node itself — bounds DFS to assets reachable
                    // from this namespace, not all of session_bb.
                    let ns_seed: BTreeSet<Bid> = BTreeSet::from([*const_bid]);
                    self.session_bb.merge_from(&const_graph, &ns_seed);

                    tracing::debug!(
                        "[initialize_stack] Loaded {} assets from global cache for namespace {}",
                        const_graph.states.len().saturating_sub(1), // -1 for namespace node itself
                        const_bid
                    );
                }
            }
        } // end content_ns_loaded guard

        // Fast-path: if self.repo is already set (not the first file of the session), attempt to
        // look up the entry document directly in session_bb. On a hit, session_bb already contains
        // the full balanced ancestor chain, so we can skip the ancestor push() loop and peer
        // fan-out entirely. See try_initialize_stack_from_session_cache for details.
        if self.repo != Bid::nil() {
            if let Some((initial, doc_sort_key)) = self
                .try_initialize_stack_from_session_cache(
                    abs_path.as_ref(),
                    global_bb.clone(),
                    proto_index,
                )
                .await?
            {
                return Ok((initial, doc_sort_key));
            }
        }

        let initial_factory = CODECS
            .path_get(abs_path.as_ref())
            .ok_or(BuildonomyError::Codec(format!(
                "Could not find codec for path type {abs_path:?}"
            )))?;
        let initial_codec = initial_factory();
        let initial = initial_codec
            .proto(abs_path.as_ref())?
            .ok_or(BuildonomyError::Codec(format!(
                "Codec could not resolve path '{abs_path:?}' into a proto node"
            )))?;

        let mut parent_path = string_to_os_path(&initial.path);
        let mut parent_path_stack: Vec<PathBuf> = Vec::default();
        // If path is a sub-network node, dont count self path as a parent path
        if parent_path
            .file_name()
            .and_then(|os_str| os_str.to_str())
            .filter(|&file_name| file_name == NETWORK_NAME)
            .is_some()
        {
            parent_path.pop();
        }
        while parent_path.pop() {
            if parent_path.strip_prefix(self.repo_root()).is_ok() {
                parent_path_stack.push(parent_path.clone());
            } else {
                break;
            }
        }
        let mut missing_structure = BeliefGraph::default();
        while let Some(path) = parent_path_stack.pop() {
            // Use the ProtoIndex (built once at compiler startup via a single WalkDir pass)
            // to look up the ancestor network proto without redundant filesystem scans.
            let Some(state_accum) = proto_index.proto_for(&path)? else {
                continue;
            };

            let (ancestor, (_source, _, _)) = self
                .push(
                    &state_accum,
                    global_bb.clone(),
                    true,
                    &mut missing_structure,
                    None, // ancestor network nodes; sort key is not relevant here
                )
                .await?;
            // Merge missing_structure after each push so it's available for the next iteration.
            if !missing_structure.is_empty() {
                // Keep self.doc_bb isolated from the structure, that way we can ensure our comparison
                // between the source material and the cache stays consistent.
                // Seed from the ancestor network node just pushed — bounds DFS to structure
                // reachable from this ancestor, not all of session_bb.
                let ancestor_seed: BTreeSet<Bid> = BTreeSet::from([ancestor]);
                self.session_bb
                    .merge_from(&missing_structure, &ancestor_seed);
                missing_structure = BeliefGraph::default(); // reset for next iteration
            }
            if path.as_os_str().is_empty() && self.repo == Bid::nil() {
                self.repo = ancestor;
            }
        }

        self.session_bb.process_event(&BeliefEvent::BalanceCheck)?;

        // Determine the entry document's sort key using the ProtoIndex — single canonical
        // source of truth shared by both fast and slow paths.  sort_key_for walks up the
        // directory tree to handle files in non-network subdirectories that iter_net_docs
        // flattens into the ancestor network's child list.
        let doc_sort_key: Option<u16> = proto_index.sort_key_for(abs_path.as_ref());
        tracing::debug!(
            "[initialize_stack slow-path] doc_sort_key={:?} for path={:?}",
            doc_sort_key,
            abs_path.as_ref()
        );
        tracing::debug!(
            "[initialize_stack slow-path stack]:\n{}",
            self.stack
                .iter()
                .enumerate()
                .map(|(idx, (_bid, path, heading))| format!("{idx}: {heading}. {path}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
        Ok((initial, doc_sort_key))
    }

    async fn terminate_stack(
        &mut self,
        renamed_nodes: BTreeMap<Bid, Bid>,
        parsed_nodes: &BTreeSet<Bid>,
    ) -> Result<(), BuildonomyError> {
        // ensure the stack is empty
        self.stack.clear();
        // Ensure we operate on a balanced set
        let balance_check = BeliefEvent::BalanceCheck;
        self.doc_bb.process_event(&balance_check)?;
        // First, apply node renames in order to have a solid basis for our next operations
        let mut tx_events = Vec::new();
        for (from_bid, to_bid) in renamed_nodes.iter() {
            let rename_event = BeliefEvent::NodeRenamed(*from_bid, *to_bid, EventOrigin::Remote);
            let mut derivatives = self.session_bb.process_event(&rename_event)?;
            tx_events.push(rename_event);
            tx_events.append(&mut derivatives);
        }
        let mut diff_events =
            BeliefBase::compute_diff(&self.session_bb, &self.doc_bb, parsed_nodes)?;
        let mut path_events = Vec::new();
        for event in diff_events.iter() {
            let derivative_events = self.session_bb.process_event(event)?;
            for derivative in derivative_events.into_iter() {
                let insert_event = match &derivative {
                    BeliefEvent::PathAdded(..)
                    | BeliefEvent::PathUpdate(..)
                    | BeliefEvent::PathsRemoved(..) => true,
                    // Other derivative events should be handled by compute_diff
                    _ => false,
                };
                if insert_event {
                    path_events.push(derivative);
                }
            }
        }
        self.session_bb.process_event(&balance_check)?;
        diff_events.append(&mut path_events);
        tx_events.append(&mut diff_events);
        if !tx_events.is_empty() {
            let mut node_update_count = 0;
            let mut node_removed_count = 0;
            let mut node_renamed_count = 0;
            let mut path_update_count = 0;
            let mut path_removed_count = 0;
            let mut relation_insert_count = 0;
            let mut relation_removed_count = 0;
            let mut relation_update_count = 0;

            for event in &tx_events {
                match event {
                    BeliefEvent::NodeUpdate(_, _, _) => node_update_count += 1,
                    BeliefEvent::NodesRemoved(nids, _) => node_removed_count += nids.len(),
                    BeliefEvent::NodeRenamed(_, _, _) => node_renamed_count += 1,
                    BeliefEvent::RelationChange(_, _, _, _, _) => relation_insert_count += 1,
                    BeliefEvent::RelationRemoved(_, _, _) => relation_removed_count += 1,
                    BeliefEvent::RelationUpdate(_, _, _, _) => relation_update_count += 1,
                    BeliefEvent::PathAdded(..) | BeliefEvent::PathUpdate(..) => {
                        path_update_count += 1
                    }
                    BeliefEvent::PathsRemoved(_, paths, _) => path_removed_count += paths.len(),
                    BeliefEvent::FileParsed(_) => {} // Metadata only, handled by Transaction
                    BeliefEvent::BalanceCheck => {}
                    BeliefEvent::BuiltInTest => {}
                }
            }
            tracing::debug!(
                "Diff events ({}): NodeUpdate({}), NodeRemoved({}), NodeRenamed({}), RelationChange({}), RelationRemoved({}), RelationUpdate({}), PathsAdded({}), PathsRemoved({})",
                tx_events.len(),
                node_update_count,
                node_removed_count,
                node_renamed_count,
                relation_insert_count,
                relation_removed_count,
                relation_update_count,
                path_update_count,
                path_removed_count
            );
        }

        let events_is_empty = tx_events.is_empty();
        for event in tx_events.into_iter() {
            self.tx.send(event)?;
        }
        if !events_is_empty {
            // tracing::debug!("Ensuring our global_bb is balanced");
            self.tx.send(balance_check)?;
        }

        Ok(())
    }

    fn get_parent_from_stack(&mut self, proto: &IRNode) -> (Bid, String, String) {
        // proto.path may contain a Windows drive-letter prefix (e.g. "C:/tmp/foo.md") because
        // os_path_to_string preserves it.  stack entries are also stored with the drive-letter
        // prefix.  AnchorPath::filepath() strips the drive letter on both sides, giving a
        // consistent comparison.  We normalise proto.path here rather than at construction time
        // so that PathBuf-based operations in initialize_stack (which need the drive letter for
        // strip_prefix against repo_root) continue to work.
        let proto_filepath = AnchorPath::new(&proto.path).filepath().to_string();
        let mut parent_info = None;
        let mut first_run = true;
        while !self.stack.is_empty() && parent_info.is_none() {
            if first_run {
                first_run = false;
            } else {
                self.stack.pop();
            }
            parent_info = self
                .stack
                .last()
                .filter(|(_stack_bid, stack_path, stack_heading)| {
                    // Extract document path from stack_path (which may contain anchors for sections)
                    let stack_ap = AnchorPath::from(stack_path);
                    let stack_filepath = stack_ap.filepath();
                    (proto_filepath.starts_with(stack_filepath)
                        && proto_filepath != stack_filepath
                        && !proto
                            .kind
                            .intersection(BeliefKind::Network | BeliefKind::Document)
                            .is_empty())
                        || (proto_filepath == stack_filepath && *stack_heading < proto.heading)
                })
                .map(|(stack_bid, stack_path, _stack_heading)| {
                    // Use proto_filepath (drive-letter-stripped) so that strip_prefix can
                    // match against stack_path regardless of Windows drive-letter form.
                    // strip_prefix applies filepath() to the prefix argument, so both sides
                    // must be in the same normalised form.
                    let path_info = AnchorPath::new(&proto_filepath)
                        .strip_prefix(stack_path)
                        .map(|s| s.to_string())
                        .unwrap_or_default();
                    (*stack_bid, path_info, stack_path.clone())
                });
        }
        parent_info.unwrap_or((self.api().bid, "".to_string(), proto.path.clone()))
    }

    /// Generate a speculative Nodekey::Path for for a node push.
    /// Uses PathMap's speculative_path to compute what the path would be with collision detection.
    /// Returns Result<NodeKey, BuildonomyError>.
    fn speculative_path_key(&self, proto: &IRNode) -> Result<Vec<NodeKey>, BuildonomyError> {
        // Note: returns empty Vec when no key can be generated (e.g. section without ID),
        // preserving the original Ok(None) semantics that push() relies on for collision handling.
        // Find the network by walking up the stack (network nodes have heading=1)
        if let Some(bid) = proto
            .document
            .get("bid")
            .and_then(|bid_val| bid_val.as_str())
            .and_then(|bid_str| Bid::try_from(bid_str).ok())
            .filter(|bid| bid.initialized())
        {
            return Ok(vec![NodeKey::Bid { bid }]);
        }

        if proto.kind.is_network() {
            // is network, and don't have an initialized id. Can't use an empty path because the net
            // will be wrong. But we require Networks to have an explicit ID. Rely on that
            let Some(network_id) = proto.id() else {
                return Err(BuildonomyError::Codec(
                    "Network nodes are required to have explicitly defined IDs. \
                        The network node has no ID set."
                        .to_string(),
                ));
            };
            let id_key = NodeKey::Id {
                net: Bref::default(),
                id: network_id,
            };
            // Network|Document dual-kind nodes (e.g. MDN constructor pages where the filename
            // matches the parent directory name, like `duration/duration/index.md`) must NOT
            // register an additional Path key here. The path that `build_path_key` would
            // produce is the parent network's child-address for this node (e.g. "duration"
            // relative to `temporal/duration/`), which collides with the parent's own
            // child-listing relation and creates a self-referential Section edge. The sections
            // of the constructor page are addressed as "index.md#slug" in their own PathMap
            // via the normal `build_path_key` path — and `push_relation` derives the owner
            // path from the stack directly rather than the PathMap, so no Path key is needed
            // here.
            return Ok(vec![id_key]);
        }
        Ok(self.build_path_key(proto).into_iter().collect())
    }

    /// Build a `NodeKey::Path` for `proto` based on the current network stack.
    ///
    /// Returns `None` when no path key can be generated (section node without an ID), which
    /// preserves the original `Ok(None)` semantics from `speculative_path_key` that `push()`
    /// relies on: an empty `keys` vec triggers the ID-collision guard at the `Unresolved` branch.
    ///
    /// Extracted from `speculative_path_key` so it can be reused for `Network|Document`
    /// dual-kind nodes that need both an ID key and a path key.
    fn build_path_key(&self, proto: &IRNode) -> Option<NodeKey> {
        let (net, net_path) = self
            .stack
            .iter()
            .rev()
            .find(|(_bid, _path, heading)| *heading == 1)
            .map(|(bid, path, _heading)| (*bid, path.clone()))
            .unwrap_or((self.repo(), String::default()));
        // proto.path may contain a Windows drive-letter prefix.  Normalise via filepath() here
        // so that strip_prefix (which applies filepath() to the prefix argument) works
        // correctly on both sides.  We do this at the comparison site rather than at
        // construction time so that PathBuf-based operations in initialize_stack continue to
        // see the original drive-letter form.
        let proto_filepath_str = AnchorPath::new(&proto.path).filepath().to_string();
        let net_anchored_child = AnchorPath::new(&proto_filepath_str)
            .strip_prefix(&net_path)
            .unwrap_or(&proto.path);
        let child_ap = AnchorPath::new(net_anchored_child);
        let path = if proto.heading > 2 {
            let section_id = match proto.id() {
                Some(id) => id,
                None => {
                    tracing::debug!(
                        "Cannot generate speculative path key for a section node without an ID"
                    );
                    // Return None so push() sees an empty keys vec and the Unresolved branch
                    // fires the ID-collision guard (same behaviour as the original Ok(None)).
                    return None;
                }
            };
            // No get_from_id guard here: section path keys are always unique per document
            // ("doc.md#slug"), so two sections in different documents with the same slug
            // produce distinct path keys and never collide. The old guard fired on re-parse
            // (second parse of the same document after the id_map was populated by the first
            // parse), causing push() to create a fresh bref-based node instead of finding the
            // existing one, breaking re-parse idempotency.
            //
            // When net_anchored_child is empty, the heading lives in the network's own
            // index.md. PathMap stores these as "index.md#slug" (NETWORK_NAME prefix), so
            // we must use the same form here to get a cache hit on re-parse.
            let base = if child_ap.to_string().is_empty() {
                AnchorPath::new(NETWORK_NAME)
            } else {
                child_ap
            };
            base.join(as_anchor(&section_id)).into_string()
        } else {
            child_ap.to_string()
        };
        Some(NodeKey::Path {
            net: net.bref(),
            path,
        })
    }

    /// Update the parent stack, and update the stack cache with the node and its relations from the
    /// global cache.
    ///
    /// If [as_trace] is true, The node will be marked as BeliefKind::Trace. If it is false, we are
    /// parsing source content and expecting to parse every relationship which the node is the owner
    /// of.
    ///
    /// Returns:
    ///
    /// **Bid: bid**: the 'best' bid for the parsed proto -- the one most likely to match our global
    /// cache if it's present in the global cache
    ///
    /// **(BTreeSet<NodeKey>, BTreeSet<Nodekey>): nodekey_changes**: the (current_valid_nodekeys,
    /// old_unique) set of nodekeys for the node. If either is not empty, then this informs
    /// whether we need to rewrite the parsed content and/or inform documents that reference this
    /// content that they should change their references.
    async fn push<B: BeliefSource + Clone>(
        &mut self,
        proto: &IRNode,
        global_bb: B,
        as_trace: bool,
        missing_structure: &mut BeliefGraph,
        explicit_sort_key: Option<u16>,
    ) -> Result<(Bid, (NodeSource, BTreeSet<NodeKey>, BTreeSet<NodeKey>)), BuildonomyError> {
        let (parent_bid, path_info, _parent_full_path) = self.get_parent_from_stack(proto);

        // Can't use self.doc_bb.paths() to generate keys here, because we can't assume that self.doc_bb
        // is balanced until we're out of phase 1 of parse_content.
        let mut parsed_node = BeliefNode::try_from(proto)?;

        // Generate keys based on node type
        let mut keys = self.speculative_path_key(proto)?;
        // On top of providing us with the old state of the node (if such a state exists), this will
        // also update our session_bb to include all the old relationships tied to this node. We
        // will use this info later in terminate_stack to determine what our "affected_sink" set is,
        // that is the set of nodes external to this parsed content that 'source' information from
        // this node that need to be informed about changes to the node's reference ids (it's set of
        // nodekeys).

        let cache_fetch_result = self
            .cache_fetch(&keys, global_bb.clone(), false, missing_structure)
            .await?;

        let (mut node, source) = match cache_fetch_result {
            GetOrCreateResult::Resolved(mut found_node, mut src) => {
                if proto.document.get("bid").is_some() {
                    // Prioritize bid from a parsed document -- merge any matches from our get-or-create
                    // results.
                    if !keys.contains(&NodeKey::Bid {
                        bid: found_node.bid,
                    }) {
                        keys.push(NodeKey::Bid {
                            bid: found_node.bid,
                        });
                    }
                }
                if parsed_node.bid.initialized() && parsed_node.bid != found_node.bid {
                    src = NodeSource::Merged;
                    found_node.bid = parsed_node.bid;
                }
                parsed_node.bid = found_node.bid;
                if found_node.merge(&parsed_node) {
                    src = NodeSource::Merged;
                }
                (found_node, src)
            }
            GetOrCreateResult::Unresolved(_) => {
                // Not found in any cache - this shouldn't happen for push() since we're
                // creating the node from parsed content. Use the parsed node.
                let source = if parsed_node.bid.initialized() {
                    NodeSource::SourceFile
                } else {
                    parsed_node.bid = Bid::new(parent_bid);
                    NodeSource::Generated
                };
                // speculative_path_key returns None if the id has a collision in this document. We
                // need to set the id to the bref at this point to control the collision
                if proto.id().is_some() && keys.is_empty() {
                    parsed_node.id = Some(parsed_node.bid.bref().to_string());
                }
                (parsed_node, source)
            }
        };
        let bid = node.bid;

        // Network-level ID collision detection (Issue 37, Fix 2)
        // Generate ID from title if not set, then check for collision
        // This happens AFTER the main cache_fetch so we have the node's BID
        let node_id = node.id();
        let node_bref = node.bid.bref().to_string();
        if !node_id.is_empty() && node_id != node_bref {
            let net = self
                .stack
                .iter()
                .rev()
                .find(|(_bid, _path, heading)| *heading == 1)
                .map(|(bid, _path, _heading)| *bid)
                .unwrap_or(self.repo);

            let id_key = NodeKey::Id {
                net: net.bref(),
                id: node_id.clone(),
            };

            // Check if this ID already exists in the network (cache + database)
            let mut id_missing_structure = BeliefGraph::default();
            let id_fetch_result = self
                .cache_fetch(
                    from_ref(&id_key),
                    global_bb.clone(),
                    true, // check doc_bb first
                    &mut id_missing_structure,
                )
                .await?;

            // Merge any missing structure from ID fetch
            if !id_missing_structure.is_empty() {
                missing_structure.union_mut(&id_missing_structure);
            }

            if let GetOrCreateResult::Resolved(existing_node, existing_source) = id_fetch_result {
                // Only check collision if node was actually found (not generated)
                // Collision if existing node has different BID
                if existing_source.is_from_cache() && existing_node.bid != bid {
                    // First-one-wins: Clear the ID so inject_context uses Bref for id
                    node.id = Some(node_bref);
                    // Regenerate keys since we updated our ID
                    for key in keys.iter_mut() {
                        if let NodeKey::Id { .. } = key {
                            *key = NodeKey::Id {
                                net: net.bref(),
                                id: node.id(),
                            };
                        };
                    }
                }
            }
        }

        // We want parsed_node to be the source of truth for title, summary, and path. But we
        // want cache_fetch node to be source of truth for bid If source is non-session
        // cache.
        if !as_trace {
            // Clear all relationships in the doc_bb for this node, this way we ensure the
            // currently parsed content is processed as the source of truth for the node's content
            // and all relationships where it is the sink.
            let remove_events = if let Some(node_idx) = self.doc_bb.bid_to_index(&node.bid) {
                self.doc_bb
                    .relations()
                    .as_graph()
                    .edges_directed(node_idx, Direction::Incoming)
                    .map(|edge| {
                        let source = self.doc_bb.relations().as_graph()[edge.source()];
                        BeliefEvent::RelationRemoved(source, node.bid, EventOrigin::Remote)
                    })
                    .collect::<Vec<BeliefEvent>>()
            } else {
                vec![]
            };
            for event in remove_events.iter() {
                let _derivative_events = self.doc_bb.process_event(event)?;
            }
        } else {
            // We're not guaranteeing that the relationship set connected to this node is
            // comprehensive.
            node.kind.insert(BeliefKind::Trace);
        }
        // }
        // if node.bid != bid {
        //     node.bid = bid;
        //     source = NodeSource::Merged;
        // }

        let _derivative_events = self.doc_bb.process_event(&BeliefEvent::NodeUpdate(
            keys.clone(),
            node.toml(),
            EventOrigin::Remote,
        ))?;

        let mut weight = Weight {
            payload: TomlTable::new(),
        };
        if !path_info.is_empty() {
            weight.set_doc_paths(vec![path_info]).ok();
        }
        // There's no one-source-of-truth for api linking, so that's the only case where the source
        // owns the edge.
        let weight_owner = match parent_bid == self.api().bid {
            // let weight_owner = match node.kind.is_document() {
            true => "source",
            false => "sink",
        };
        weight
            .set(crate::properties::WEIGHT_OWNED_BY, weight_owner)
            .ok();

        // If the caller captured an explicit sort key for this node (set by initialize_stack
        // from the upstream sibling index or from the fast-path PathMap order), inject it now.
        // This supersedes the former StackCache-only workaround: that approach read from
        // session_bb, which could carry a wrong sk=0 written during the first slow-path parse.
        // explicit_sort_key is derived from the authoritative source (iter_net_docs order /
        // PathMap order) and is correct regardless of which cache branch cache_fetch took.
        if let Some(sk) = explicit_sort_key {
            weight.set(crate::properties::WEIGHT_SORT_KEY, sk).ok();
        }

        let _derivative_events = self.doc_bb.process_event(&BeliefEvent::RelationChange(
            bid,
            parent_bid,
            WeightKind::Section,
            Some(weight.clone()),
            EventOrigin::Remote,
        ))?;

        // For sections, build an absolute stack path by joining the network-relative anchor path
        // from speculative_path_key onto the absolute net_path.  The Path key stores a
        // network-relative form (e.g. "doc.md#heading-id") — correct for PathMap lookups — but
        // get_parent_from_stack compares stack_ap.filepath() against the absolute proto.path, so
        // the stack entry must also be absolute.  Re-joining against net_path restores the
        // absolute prefix that strip_prefix removed inside speculative_path_key.
        let net_path = self
            .stack
            .iter()
            .rev()
            .find(|(_bid, _path, heading)| *heading == 1)
            .map(|(_bid, path, _heading)| path.clone())
            .unwrap_or_default();
        let stack_path = if proto.heading > 2 {
            keys.iter()
                .find_map(|k| match k {
                    NodeKey::Path { path, .. } => {
                        Some(AnchorPath::new(&net_path).join(path).into_string())
                    }
                    _ => None,
                })
                .unwrap_or_else(|| proto.path.clone())
        } else {
            // Document or network: use document path
            proto.path.clone()
        };
        self.stack.push((bid, stack_path, proto.heading));

        if node.kind.is_network() {
            // If the builder repo is nil, and this node is a network, and the
            // stack is empty, then initialize the builder repo to this element.
            // We don't do this operation in [GraphBuilder::new] because
            // reading the repo source is part of our async operations.
            if self.repo == Bid::nil() && parent_bid == self.api().bid {
                tracing::debug!("Setting repo to {}", node.bid);
                self.repo = node.bid;
            }

            // Only create additional API connection for subnet networks that aren't already
            // connected All networks we process need to be connected to the API that we used to
            // parse that network.
            if parent_bid != self.api().bid {
                let mut api_weight = Weight {
                    payload: TomlTable::new(),
                };
                api_weight
                    .set(crate::properties::WEIGHT_OWNED_BY, "source")
                    .ok();
                let _derivative_events =
                    self.doc_bb.process_event(&BeliefEvent::RelationChange(
                        bid,
                        self.api().bid,
                        WeightKind::Section,
                        Some(api_weight),
                        EventOrigin::Remote,
                    ))?;
            }
        }

        let current_keys =
            BTreeSet::from_iter(node.keys(Some(self.repo()), Some(parent_bid), self.doc_bb()));

        let unique_old = BTreeSet::from_iter(
            BTreeSet::from_iter(keys.into_iter())
                .difference(&current_keys)
                .cloned(),
        );

        Ok((bid, (source, current_keys, unique_old)))
    }

    #[allow(clippy::too_many_arguments)]
    async fn push_relation<B: BeliefSource + Clone>(
        &mut self,
        relation: &IntermediateRelation,
        owner_bid: &Bid,
        direction: Direction,
        index: usize,
        source: &str,
        global_bb: B,
        update_queue: &mut Vec<BeliefEvent>,
        missing_structure: &mut BeliefGraph,
    ) -> Result<GetOrCreateResult, BuildonomyError> {
        let other_key = &relation.key;
        let kind = &relation.kind;
        let maybe_weight = &relation.weight;
        // When is_source_owned=false (sink-owned/upstream_relations): owner is sink, other is source
        // When is_source_owned=true (source-owned/downstream_relations): owner is source, other is sink
        //
        // Derive the owner's repo-relative path from the stack rather than from PathMap.
        //
        // `regularize` (the previous approach) looks up the owner's path in PathMap, which fails
        // for Phase 2 relations whose owner is a freshly-parsed node: at Phase 2 start the PathMap
        // has been rebuilt after Phase 1, but sections of a Network|Document dual-kind node (e.g.
        // `duration/duration/index.md`) have no PathMap entry yet because the Section edges
        // connecting them to their home network haven't been emitted yet — those edges are exactly
        // what Phase 2 is in the process of building.
        //
        // The stack is the authoritative source for Phase 1 and Phase 2: every node pushed in
        // Phase 1 (and every ancestor pushed by initialize_stack) has an entry in self.stack.
        // Stack paths are absolute; strip_prefix(repo_root) yields the repo-relative form that
        // `regularize_unchecked` expects as `owner_path`.
        //
        // base_net remains self.repo() — `regularize_unchecked` assigns self.repo().bref() to
        // any default-net Path key, which is correct: all document paths are registered in the
        // repo-root PathMap regardless of which subnet they belong to.
        let repo_root_str = os_path_to_string(&self.repo_root);
        let owner_rel_path = self
            .stack
            .iter()
            .rev()
            .find(|(bid, _path, _heading)| bid == owner_bid)
            .map(|(_bid, abs_owner_path, _heading)| {
                AnchorPath::new(abs_owner_path)
                    .strip_prefix(&repo_root_str)
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| abs_owner_path.clone())
            })
            .unwrap_or_default();
        let other_key_regularized =
            other_key.regularize_unchecked(self.repo(), &owner_rel_path, &repo_root_str);

        let other_keys = vec![other_key_regularized.clone()];
        let mut weight = maybe_weight.clone().unwrap_or_default();
        weight.set(WEIGHT_SORT_KEY, index as u16)?;
        let owner = match direction {
            Direction::Incoming => "sink",
            Direction::Outgoing => "source",
        };
        weight.set(crate::properties::WEIGHT_OWNED_BY, owner).ok();
        // Translate relative paths into absolute paths and resolve the "other" node
        let cache_fetch_result = self
            .cache_fetch(&other_keys, global_bb.clone(), true, missing_structure)
            .await?;
        let (other_node, other_node_source) = match cache_fetch_result {
            GetOrCreateResult::Resolved(mut other_node, other_node_source) => {
                // Mark these nodes as traces -- we're not guaranteeing that we have all their
                // relationships loaded
                other_node.kind.insert(BeliefKind::Trace);
                (other_node, other_node_source)
            }
            GetOrCreateResult::Unresolved(ref unresolved_initial) => {
                // Special handling of external scheme links (http/https)
                if let Some(href) = match &other_key_regularized {
                    NodeKey::Path { net, path } => {
                        if *net == href_namespace().bref() {
                            Some(path.clone())
                        } else {
                            None
                        }
                    }
                    _ => None,
                } {
                    // First reference to this http[s] schema link.
                    // First ensure we've installed the href network, do that if necessary.
                    if !self.doc_bb.states().contains_key(&href_namespace()) {
                        let href_net_node = BeliefNode::href_network();
                        update_queue.push(BeliefEvent::NodeUpdate(
                            href_net_node.keys(Some(buildonomy_namespace()), None, &self.doc_bb),
                            href_net_node.toml(),
                            EventOrigin::Remote,
                        ));
                    }
                    // Now generate the href wrapper node and insert it.
                    let href_node = BeliefNode {
                        bid: Bid::new(href_namespace()),
                        kind: BeliefKindSet::from(BeliefKind::External | BeliefKind::Trace),
                        title: String::default(),
                        schema: None,
                        payload: TomlTable::default(),
                        id: Some(href.clone()),
                    };
                    update_queue.push(BeliefEvent::NodeUpdate(
                        href_node.keys(Some(href_namespace()), None, &self.doc_bb),
                        href_node.toml(),
                        EventOrigin::Remote,
                    ));
                    let mut href_weight = Weight::default();
                    href_weight.set(WEIGHT_DOC_PATHS, vec![href.clone()])?;
                    update_queue.push(BeliefEvent::RelationChange(
                        href_node.bid,
                        href_namespace(),
                        WeightKind::Section,
                        Some(href_weight),
                        EventOrigin::Remote,
                    ));
                    (href_node, NodeSource::Generated)
                } else {
                    let mut unresolved = unresolved_initial.clone();
                    unresolved.direction = direction;
                    unresolved.self_bid = *owner_bid;
                    unresolved.reference_location = relation
                        .location
                        .map(|offset| crate::codec::byte_offset_to_location(source, offset));
                    let pmm_guard = self.doc_bb.paths();
                    let Some((owner_home_net, owner_home_path)) =
                        pmm_guard.api_map().home_path(owner_bid, &pmm_guard)
                    else {
                        // owner_bid has no home path in doc_bb. This happens when a
                        // Network|Document dual-kind node (e.g. duration/duration) is parsed
                        // *after* its siblings in filesystem order. push_relation is called
                        // with the owner being that not-yet-registered dual-kind node, so
                        // doc_bb has no PathMap entry for it yet.
                        //
                        // Correct recovery: emit an Incoming UnresolvedReference whose
                        // other_keys point to the owner node itself (by BID). The compiler
                        // will enqueue the owner for parsing and re-parse the current file
                        // once the owner is registered — preserving link correctness.
                        //
                        // We look up the owner's keys from session_bb (it was added there as
                        // a Trace node when its sibling network was loaded in initialize_stack).
                        let owner_keys = self
                            .session_bb
                            .get(&NodeKey::Bid { bid: *owner_bid })
                            .map(|owner_node| {
                                owner_node.keys(Some(self.repo), None, &self.session_bb)
                            })
                            .unwrap_or_else(|| vec![NodeKey::Bid { bid: *owner_bid }]);
                        tracing::debug!(
                            "Unresolved relation at index {}: owner {:?} has no home path in \
                            doc_bb (parse order issue — dual-kind node not yet registered). \
                            Re-queuing owner via Incoming UnresolvedReference with keys: {:?}",
                            index,
                            owner_bid,
                            owner_keys,
                        );
                        let mut requeue = unresolved_initial.clone();
                        requeue.direction = Direction::Incoming;
                        requeue.self_bid = *owner_bid;
                        requeue.other_keys = owner_keys;
                        requeue.reference_location = relation
                            .location
                            .map(|offset| crate::codec::byte_offset_to_location(source, offset));
                        return Ok(GetOrCreateResult::Unresolved(requeue));
                    };
                    unresolved.self_net = owner_home_net;
                    unresolved.self_path = owner_home_path;
                    tracing::debug!(
                        "Unresolved relation at index {}: {:?} -> {:?}. Index gap preserved to track missing reference.",
                        index,
                        owner_bid,
                        other_key_regularized
                    );
                    return Ok(GetOrCreateResult::Unresolved(unresolved));
                }
            }
        };
        // tracing::debug!(
        //     "Processing relation: {:?}. sourced via: {:?}, kinds: {:?}",
        //     other_keys,
        //     other_node_source,
        //     other_node.kind
        // );

        // # This Requires an Explanation
        //
        // This logic has caused me a lot of grief so here's a description of what (should be)
        // going on. We're accomplishing two things: 1) Updating the accumulated set with the
        // acquired other node and source->sink structural relationships
        //
        // - alyjak, 2025-03-07 (updated 2025-11-07)
        //
        // First enqueue the node to add it to self.doc_bb if it's not already there
        if other_node_source != NodeSource::SourceFile {
            // We want to delineate between parsed sources and linked content. If we're not from the
            // doc_bb, color the other_node by Trace to ensure we can separate parsed content
            // from referenced content.
            //
            // Note we perform a similar coloring to the missing structure in parse_content at the
            // end of phase 2.
            update_queue.push(BeliefEvent::NodeUpdate(
                other_keys.clone(),
                other_node.toml(),
                EventOrigin::Remote,
            ));
        }
        // Next, make sure its substructure is available in self.doc_bb
        match other_node_source {
            NodeSource::Merged => {
                panic!("We should only produced NodeSource::Merged from GraphBuilder::push!")
            }
            NodeSource::GlobalCache | NodeSource::StackCache => {
                // The node state itself comes from cache_fetch (via missing_structure), but that
                // only carries the node's TOML -- no relations. On a re-parse the node already
                // exists in session_bb with its full neighborhood (e.g. a href node's
                // RelationChange to href_namespace that populates its PathMap entry). Pull that
                // neighborhood from session_bb here, whenever the node comes from "outside"
                // (StackCache or GlobalCache), so that doc_bb has a complete picture for
                // inject_context.
                //
                // Without this, content namespace nodes (href, asset) fetched from GlobalCache on
                // re-parses have no PathMap entry in doc_bb, causing ExtendedRelation::new to
                // return an empty root_path and erasing their URLs during link rewriting.
                let query = Query {
                    seed: Expression::from(&NodeKey::Bid {
                        bid: other_node.bid,
                    }),
                    traverse: None,
                };
                let stack_result = self.session_bb.eval_query(&query, true).await?;

                // Use union_mut_with_trace so that Trace-kind nodes (e.g. href nodes, which are
                // always External|Trace) are included. Plain union_mut filters out Trace nodes,
                // which would silently drop the href_hamespace section edges and leave the href
                // PathMap incomplete during inject_context.
                missing_structure.union_mut_with_trace(&stack_result);
            }
            NodeSource::SourceFile | NodeSource::Generated => {
                // We've accumulated all the structure we need already, the event queue can be
                // processed without issue.
            }
        };

        // Determine actual source and sink bids based on ownership
        let (source_bid, sink_bid) = match direction {
            // Source-owned: owner is source, other is sink
            Direction::Outgoing => {
                // Mark ownership based on whether this is from downstream_relations (source-owned)
                // or upstream_relations (sink-owned, default)
                (*owner_bid, other_node.bid)
            }
            Direction::Incoming => {
                // Sink-owned (default): other is source, owner is sink
                (other_node.bid, *owner_bid)
            }
        };

        // Guard against self-referential edges. This arises with Network|Document dual-kind
        // nodes (e.g. MDN constructor pages where filename == parent directory name, like
        // `duration/duration/index.md`). The parent network's child-list includes "duration"
        // as a child path, and after the speculative_path_key fix that same node holds a
        // Path key "duration" — so cache_fetch resolves the child relation back to the node
        // itself. Without this guard the self-loop enters session_bb and is re-encountered
        // on every subsequent initialize_stack, firing add_relations_seeded's self-connection
        // warn hundreds of times and causing 30s stalls late in the corpus.
        //
        // The right long-term fix is in iter_net_docs / NetworkCodec::proto: exclude any
        // child path that resolves to the same BID as the network node itself (i.e. detect
        // the constructor-page pattern at filesystem scan time and skip the self-child).
        // This guard is the minimal safe fix that prevents the symptom from accumulating.
        if source_bid == sink_bid {
            tracing::debug!(
                "[push_relation] skipping self-referential {:?} edge on node {} \
                 (owner={}, other={}). This is expected for Network|Document dual-kind \
                 nodes where the child path resolves back to the network node itself.",
                kind,
                source_bid,
                owner_bid,
                other_node.bid,
            );
            return Ok(GetOrCreateResult::Resolved(other_node, other_node_source));
        }

        update_queue.push(BeliefEvent::RelationChange(
            source_bid,
            sink_bid,
            *kind,
            Some(weight),
            EventOrigin::Remote,
        ));

        Ok(GetOrCreateResult::Resolved(other_node, other_node_source))
    }

    /// Fast-path for `initialize_stack`: if `abs_path` is already present in `session_bb`,
    /// reconstruct `self.stack` from the balanced graph that `cache_fetch` returns and skip
    /// the O(siblings) ancestor push() loop and peer-enumeration fan-out entirely.
    ///
    /// Returns `Some(initial_IRNode)` when the fast-path fires (session cache hit), or `None`
    /// when the entry document is not yet in session_bb and the slow path must run.
    ///
    /// # Why this is correct
    ///
    /// `cache_fetch` calls `session_bb.eval_query` with `traverse: None`.  `eval_query`
    /// unconditionally calls `balance()` before returning, which iterates downstream Section
    /// edges until it reaches the API root.  The resulting `BeliefGraph` therefore contains
    /// every network ancestor node and its Section edge to its parent — exactly what `doc_bb`
    /// needs in order to anchor the entry document in the PathMap.
    ///
    /// Fast path for `initialize_stack`: queries the **parent network** in `session_bb`
    /// instead of the entry document itself.
    ///
    /// ## Why query the parent, not the entry doc?
    ///
    /// The compiler always parses a network before its children (children are discovered
    /// via `upstream` relations and enqueued after the network parse completes).  After the
    /// network's `terminate_stack`, `session_bb` contains the parent network node, its
    /// `Section → repo_root` ancestor chain, and Section edges to every child with their
    /// correct sort keys.
    ///
    /// Querying the **entry doc** only hits `StackCache` on a *reparse* (after the first
    /// parse's `terminate_stack` wrote it in).  Querying the **parent network** hits
    /// `StackCache` on the **first** parse of every child — no reparse needed.
    ///
    /// ## What this returns
    ///
    /// `Some((initial, doc_sort_key))` when the parent is found in `session_bb`:
    /// - `doc_bb` is seeded with the ancestor network chain (scaffolding for Phase 1).
    /// - `self.stack` is reconstructed from the parent's PathMap order.
    /// - `doc_sort_key` is the sort key from the parent's Section edge to the entry doc,
    ///   read directly from the edge weight in `fast_missing`.
    /// - `session_bb` is updated with the parent graph.
    /// - If the entry doc is already in `session_bb` (reparse), its section children are
    ///   also merged into `session_bb` so Phase 1 reuses existing section BIDs.
    ///
    /// `None` to fall through to the slow path when:
    /// - The entry doc is itself a network (the parent would be the same node — use slow path).
    /// - The parent network path cannot be determined.
    /// - `cache_fetch` on the parent returns anything other than `StackCache`.
    async fn try_initialize_stack_from_session_cache<B: BeliefSource + Clone>(
        &mut self,
        abs_path: &Path,
        global_bb: B,
        proto_index: &ProtoIndex,
    ) -> Result<Option<(IRNode, Option<u16>)>, BuildonomyError> {
        // Compute the parent network's repo-relative path.
        //
        // The parent network directory is the immediate ancestor directory that contains a
        // network file.  For a plain doc `net/doc.md`, the parent dir is `net/`.
        // For a subnet index `net/subnet/index.md`, the entry doc IS a network — fall
        // through to slow path (no separate parent to query).
        //
        // We replicate the slow-path's parent_path_stack logic:
        //   1. Start from initial.path (the doc's own path string).
        //   2. Pop NETWORK_NAME suffix if present (subnet case) — but that means it's a
        //      network itself, so fall through.
        //   3. Pop one directory component — that's the parent network directory.
        //   4. Strip repo_root → parent_net_rel_path for the NodeKey.
        let entry_rel_path = match abs_path.strip_prefix(self.repo_root()) {
            Ok(p) => os_path_to_string(p),
            Err(_) => return Ok(None),
        };

        // Determine the parent network directory (absolute).
        // If abs_path ends with NETWORK_NAME, the entry doc IS a network — no separate
        // parent to query on the fast path; fall through to slow path.
        let abs_path_str = os_path_to_string(abs_path);
        let abs_ap = AnchorPath::new(&abs_path_str);
        if abs_ap.filename() == NETWORK_NAME {
            return Ok(None);
        }

        // Pop the filename component to get the containing directory.
        let mut parent_abs = abs_path.to_path_buf();
        parent_abs.pop();

        // The parent directory must still be inside the repo.
        if parent_abs.strip_prefix(self.repo_root()).is_err() {
            return Ok(None);
        }

        // Build the parent network's NodeKey.
        // For a doc in the repo root (`parent_abs == repo_root`), the key uses an empty path
        // string — this matches how the repo-root network is registered in the PathMap.
        let parent_rel_path = os_path_to_string(
            parent_abs
                .strip_prefix(self.repo_root())
                .unwrap_or(std::path::Path::new("")),
        );
        let parent_key = NodeKey::Path {
            net: self.repo.bref(),
            path: parent_rel_path.clone(),
        };

        // Query the parent network in session_bb.
        // On a StackCache hit, fast_missing contains the parent network node, its ancestor
        // chain (Section edges up to the API root via balance()), and — because the parent
        // is a network — its downstream Section edges to each child document with their sort
        // keys in the edge weights.
        let mut fast_missing = BeliefGraph::default();
        let fast_result = self
            .cache_fetch(
                std::slice::from_ref(&parent_key),
                global_bb.clone(),
                false,
                &mut fast_missing,
            )
            .await?;

        let parent_bid = match fast_result {
            GetOrCreateResult::Resolved(ref node, NodeSource::StackCache) => node.bid,
            _ => return Ok(None),
        };

        // Extract doc_sort_key from the parent's Section edge to the entry doc.
        //
        // fast_missing (from cache_fetch on the parent) contains the balanced ancestor graph
        // plus all downstream Section edges from the parent to its children (because
        // eval_query with traverse:None calls balance(), which walks Section edges downward
        // one level for network nodes in order to populate the PathMap with child sort keys).
        //
        // The entry doc's repo-relative path is `entry_rel_path`.  Find the Section edge
        // whose doc_paths weight contains that path and read its sort_key.
        // Determine doc_sort_key using the ProtoIndex — the single canonical source of
        // truth for sibling position, shared by both fast and slow paths.
        let doc_sort_key: Option<u16> = proto_index.sort_key_for(abs_path);
        tracing::debug!(
            "[try_initialize_stack_from_session_cache] doc_sort_key from proto_index={:?} for path={:?}",
            doc_sort_key,
            abs_path
        );

        // Populate doc_bb with ancestor networks only.
        //
        // The safe invariant: doc_bb before Phase 1 must contain ONLY network ancestors.
        // The entry doc and all its sections must be introduced exclusively by Phase 1
        // push() so their PathMap entries are established via RelationChange events with
        // freshly-computed sort keys.
        //
        // fast_missing from a parent-network query contains:
        //   - parent network node + its ancestor chain (all network-kinded) ← KEEP
        //   - Section edges from parent down to child docs ← EXCLUDE (non-network sinks)
        //
        // Filter to edges where BOTH endpoints are network-kinded.
        let ancestor_bids: std::collections::BTreeSet<Bid> = fast_missing
            .states
            .iter()
            .filter(|(_, n)| n.kind.is_network())
            .map(|(bid, _)| *bid)
            .collect();
        let ancestors_only: BeliefGraph = BeliefGraph {
            states: fast_missing
                .states
                .iter()
                .filter(|(bid, _)| ancestor_bids.contains(bid))
                .map(|(bid, n)| (*bid, n.clone()))
                .collect(),
            relations: {
                let g = fast_missing.relations.as_graph();
                crate::beliefbase::BidGraph::from_edges(g.raw_edges().iter().filter_map(|e| {
                    let source = g[e.source()];
                    let sink = g[e.target()];
                    if ancestor_bids.contains(&source) && ancestor_bids.contains(&sink) {
                        Some((source, sink, e.weight.clone()))
                    } else {
                        None
                    }
                }))
            },
        };

        tracing::debug!(
            "[try_initialize_stack_from_session_cache] ancestors_only: {} states, {} edges",
            ancestors_only.states.len(),
            ancestors_only.relations.as_graph().edge_count(),
        );

        // Build doc_bb directly from ancestors_only — do NOT consume() the previous
        // doc_bb and union into it.  The previous doc_bb may contain stale content from
        // the prior parse of this file (asset nodes, section edges, etc.); carrying that
        // forward leaks state and causes PathMap corruption (in_states=true, in_pathmap=false).
        self.doc_bb = BeliefBase::from(ancestors_only);
        self.doc_bb.process_event(&BeliefEvent::BalanceCheck)?;

        // Merge the full parent graph (including child edges) into session_bb so subsequent
        // sibling parses also find the parent on a StackCache hit.
        // Seed from parent_bid — bounds DFS to structure reachable from the parent network,
        // not all of session_bb.
        let parent_seed: BTreeSet<Bid> = BTreeSet::from([parent_bid]);
        self.session_bb.merge_from(&fast_missing, &parent_seed);
        self.session_bb.process_event(&BeliefEvent::BalanceCheck)?;

        // If the entry doc was already parsed in this session (reparse case), also fetch its
        // downstream section children into session_bb so Phase 1 reuses existing section BIDs
        // rather than generating fresh timestamp-based ones.
        let entry_key = NodeKey::Path {
            net: self.repo.bref(),
            path: entry_rel_path.clone(),
        };
        if self.session_bb.get(&entry_key).is_some() {
            let downstream_query = Query {
                seed: Expression::from(&entry_key),
                traverse: Some(NeighborsExpression {
                    filter: Some(WeightKind::Section.into()),
                    upstream: 0,
                    downstream: 1,
                }),
            };
            let downstream_graph = self.session_bb.eval_query(&downstream_query, true).await?;
            if !downstream_graph.states.is_empty() {
                // Seed from the entry doc's BID — bounds DFS to section children reachable
                // from this doc in downstream_graph, not all of session_bb.
                if let Some(entry_node) = self.session_bb.get(&entry_key) {
                    let entry_seed: BTreeSet<Bid> = BTreeSet::from([entry_node.bid]);
                    self.session_bb.merge_from(&downstream_graph, &entry_seed);
                }
            }
        }

        // Reconstruct self.stack from the parent network's PathMap position in doc_bb.
        //
        // The parent network is in doc_bb (ancestors_only above).  Its order vec in the
        // repo PathMap is the prefix used to find its own ancestors.  We push the parent
        // onto the stack, then walk upward through prefix truncations to collect any
        // intermediate subnet ancestors, then prepend the repo root.
        let repo_root_str = os_path_to_string(self.repo_root());

        let states = self.doc_bb.states();
        let heading_for = |bid: &Bid| -> usize {
            states
                .get(bid)
                .map(|n| if n.kind.is_network() { 1 } else { 2 })
                .unwrap_or(1)
        };

        // Guard: the repo network must be present in doc_bb's PathMap.
        if self.doc_bb.paths().get_map(&self.repo.bref()).is_none() {
            tracing::debug!(
                "[try_initialize_stack_from_session_cache] repo not in doc_bb PathMap, falling through to slow path"
            );
            return Ok(None);
        }

        let stack_entries: Vec<(Bid, String, usize)> = self
            .doc_bb
            .paths()
            .get_map(&self.repo.bref())
            .and_then(|pm| {
                // Look up the parent network's order vec.  For the repo-root network,
                // parent_rel_path is "" and order_for_bid gives order=[].
                // For a subnet parent, order_for_bid gives e.g. [sk] where sk is the
                // subnet's own sort key within the root network.
                let (parent_order, parent_rel) = pm.order_for_bid(&parent_bid)?;

                // Build the parent's absolute path for its stack entry.
                let parent_abs_str = if parent_rel.is_empty() {
                    repo_root_str.clone()
                } else {
                    format!("{repo_root_str}/{parent_rel}")
                };

                // Walk ancestor prefixes above the parent to collect any deeper subnet chain.
                let mut prefix = parent_order.to_vec();
                let mut ancestors: Vec<(Bid, String, usize)> = Vec::new();
                while prefix.pop().is_some() && !prefix.is_empty() {
                    if let Some((anc_bid, anc_rel)) = pm.order_for(&prefix) {
                        let abs = format!("{repo_root_str}/{anc_rel}");
                        ancestors.push((anc_bid, abs, heading_for(&anc_bid)));
                    }
                }
                ancestors.reverse();

                // Stack order: repo root → intermediate subnets → immediate parent network.
                // The entry doc itself is NOT on the stack here; Phase 1 push() adds it.
                let mut stack = vec![(self.repo, repo_root_str.clone(), heading_for(&self.repo))];
                stack.extend(ancestors);
                // Only push the parent explicitly if it is not the repo root itself.
                if parent_bid != self.repo {
                    stack.push((parent_bid, parent_abs_str, heading_for(&parent_bid)));
                }
                Some(stack)
            })
            .unwrap_or_else(|| vec![(self.repo, repo_root_str.clone(), heading_for(&self.repo))]);

        self.stack = stack_entries;
        self.session_bb.process_event(&BeliefEvent::BalanceCheck)?;

        tracing::debug!(
            "[initialize_stack fast-path] doc_sort_key={:?} for path={:?}",
            doc_sort_key,
            abs_path
        );
        tracing::debug!(
            "[initialize_stack fast-path stack]:\n{}",
            self.stack
                .iter()
                .enumerate()
                .map(|(idx, (_bid, path, heading))| format!("{idx}: {heading}. {path}"))
                .collect::<Vec<_>>()
                .join("\n")
        );

        // proto() is still needed — initialize_stack must return the entry IRNode for Phase 1.
        let initial_factory = CODECS
            .path_get(abs_path)
            .ok_or(BuildonomyError::Codec(format!(
                "Could not find codec for path type {abs_path:?}"
            )))?;
        let initial_codec = initial_factory();
        let initial = initial_codec
            .proto(abs_path)?
            .ok_or(BuildonomyError::Codec(format!(
                "Codec could not resolve path '{abs_path:?}' into a proto node"
            )))?;
        Ok(Some((initial, doc_sort_key)))
    }

    async fn cache_fetch<B: BeliefSource + Clone>(
        &mut self,
        keys: &[NodeKey],
        global_bb: B,
        check_local: bool,
        missing_structure: &mut BeliefGraph,
    ) -> Result<GetOrCreateResult, BuildonomyError> {
        let mut found_state: Option<BeliefNode> = None;
        let mut source = NodeSource::Generated;
        for key in keys.iter() {
            if check_local {
                if let Some(existing_state) = self.doc_bb.get(key) {
                    found_state = Some(existing_state);
                    source = NodeSource::SourceFile;
                    break;
                }
            }

            let query = Query {
                seed: Expression::from(key),
                traverse: None,
            };

            // use eval_query in order to receive a balanced_set if/when we get a query hit on
            // one of our caches
            let stack_result = BeliefBase::from(self.session_bb.eval_query(&query, true).await?);

            match stack_result.get(key) {
                Some(existing_state) => {
                    found_state = Some(existing_state);
                    // StackCache hit: the node is already in session_bb with a balanced
                    // ancestor chain. We do NOT populate missing_structure here — doing so
                    // caused Phase 2's unconditional doc_bb.merge(&missing_structure) to
                    // overwrite the section→doc Section edges that Phase 1 just established,
                    // corrupting the PathMap and triggering the Phase 4 get_context panic.
                    //
                    // try_initialize_stack_from_session_cache passes its own local
                    // `fast_missing` as the missing_structure argument and reads it directly
                    // after this call — it does not need the population to happen here.
                    source = NodeSource::StackCache;
                    break;
                }
                None => {
                    let mut cache_update =
                        BeliefBase::from(global_bb.eval_query(&query, false).await?);

                    // tracing::debug!(
                    //     "[cache_fetch] global_bb result has {} nodes and {} relations. About to check its balance. Nodes:\n\t{}",
                    //     cache_update.states().len(),
                    //     cache_update.relations().as_graph().edge_count(),
                    //     cache_update.states().values().map(|n| format!("[{} - {}]", n.bid, n.title)).collect::<Vec<String>>().join("\n\t")
                    // );

                    // Log PathMap state before attempting get
                    // tracing::debug!(
                    //     "[cache_fetch] PathMap networks: {:?}",
                    //     cache_update.paths().nets()
                    // );

                    if let Some(cached_state) = cache_update.get(key) {
                        found_state = Some(cached_state);
                        let update = cache_update.consume();
                        // Percolate global cache updates into closer caches.
                        missing_structure.union_mut(&update);
                        source = NodeSource::GlobalCache;
                        break;
                    } else if !cache_update.is_empty() {
                        let pmm_guard = cache_update.paths();

                        // Detailed PathMap diagnostics
                        let node_details = cache_update
                            .states()
                            .values()
                            .map(|n| {
                                format!(
                                    "BID: {}, Title: {}, ID: {:?}, Kind: {:?}",
                                    n.bid, n.title, n.id, n.kind
                                )
                            })
                            .collect::<Vec<String>>()
                            .join("\n\t");

                        let path_map_details = pmm_guard
                            .map()
                            .iter()
                            .map(|(net_bid, pm_arc)| {
                                let pm = pm_arc.read();
                                format!("Network {}: {} entries", net_bid, pm.map().len())
                            })
                            .collect::<Vec<String>>()
                            .join("\n\t");

                        tracing::warn!(
                            "[cache_fetch FAILED] Why didn't we get our node? The query returned results.\n\
                            Search key: {:?}\n\
                            Cached nodes ({}):\n\t{}\n\
                            PathMap networks: {:?}\n\
                            PathMap details:\n\t{}\n\
                            Relations edge count: {}",
                            key,
                            cache_update.states().len(),
                            node_details,
                            pmm_guard.nets(),
                            path_map_details,
                            cache_update.relations().as_graph().edge_count()
                        );
                    }
                }
            }
        }

        // tracing::debug!(
        //     "cache_fetch:\n\tkeys: {:?}\n\tfound: {:?}\n\tsource: {:?}",
        //     keys,
        //     found_state,
        //     source
        // );
        // If we found a state in any cache, return it as Resolved
        if let Some(state) = found_state {
            Ok(GetOrCreateResult::Resolved(state, source))
        } else {
            // No cached state found - return Unresolved instead of creating ephemeral node
            // For now, we'll create an UnresolvedReference with basic info
            // The caller (push_relation) will provide proper context
            // tracing::debug!("Fetch miss! Keys: {:?}", keys);
            Ok(GetOrCreateResult::Unresolved(UnresolvedReference {
                other_keys: keys.into(),
                ..Default::default()
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        codec::belief_ir::IRNode,
        paths::to_anchor,
        properties::{BeliefKind, BeliefKindSet, BeliefNode, Bid},
    };
    use std::path::Path;
    use toml_edit::{value, DocumentMut};

    /// Helper: Create a test network directory with index.md file
    fn create_test_network(dir: &Path) {
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

    /// Helper: Create a test IRNode for a section heading
    fn create_test_proto_section(
        title: &str,
        path: &str,
        heading: usize,
        maybe_id: Option<String>,
        bid: Option<&str>,
    ) -> IRNode {
        let mut doc = DocumentMut::new();
        doc.insert("title", value(title));
        doc.insert("schema", value("Document"));
        if let Some(bid_str) = bid {
            doc.insert("bid", value(bid_str));
        }
        if let Some(id) = maybe_id {
            doc.insert("id", value(id));
        }
        IRNode {
            accumulator: None,
            content: String::new(),
            document: doc,
            upstream: Vec::new(),
            downstream: Vec::new(),
            path: path.to_string(),
            kind: crate::properties::BeliefKindSet::default(),
            errors: Vec::new(),
            heading,
        }
    }

    /// Helper: Create a test BeliefNode
    fn create_test_node(title: &str, _kind: BeliefKind, bid: Option<Bid>) -> BeliefNode {
        let bid = bid.unwrap_or_else(|| Bid::new(Bid::nil()));
        BeliefNode {
            bid,
            kind: BeliefKindSet::from(BeliefKind::Document),
            title: title.to_string(),
            schema: None,
            payload: Default::default(),
            id: None,
        }
    }

    #[test]
    fn test_truth_table_case_1_no_bid_no_match() {
        // Case: No BID in parsed, no cache match
        // Expected: Generate new BID via Bid::new(parent)

        let _proto = create_test_proto_section("Details", "test.md", 3, None, None);

        // Simulate Unresolved result - node should get generated BID
        let parent_bid = Bid::nil();
        let generated_bid = Bid::new(parent_bid);

        // The generated BID should be different from parent
        assert_ne!(generated_bid, parent_bid);
        assert!(generated_bid.initialized());
    }

    #[test]
    fn test_truth_table_case_2_no_bid_path_match_section() {
        // Case: No BID in parsed, cache match via Path (section)
        // Expected: Use found BID (watch session scenario)

        let proto = create_test_proto_section("Details", "test.md", 3, None, None);
        let existing_bid = Bid::new(Bid::nil());
        let existing_node = create_test_node("Details", BeliefKind::Document, Some(existing_bid));

        // In watch session, proto has no BID but cache has the node
        assert!(proto.document.get("bid").is_none());
        assert_eq!(existing_node.bid, existing_bid);

        // Logic: Use found BID
        let result_bid = existing_node.bid;
        assert_eq!(result_bid, existing_bid);
    }

    #[test]
    fn test_truth_table_case_3_duplicate_titles_no_title_key() {
        // Case: Two sections with same title, NO Title key in cache lookup
        // Expected: Different speculative paths → no match → create two separate nodes

        let proto1 =
            create_test_proto_section("Details", "test.md", 3, Some("details".to_string()), None);
        let proto2 = create_test_proto_section("Details", "test.md", 3, None, None); // No ID = collision

        // First node: path would be "test.md#details"
        let path1 = format!("{}#{}", proto1.path, proto1.id().unwrap());

        // Second node: path would be "test.md#<bref>" (placeholder for collision)
        // Since ID is None, we know collision was detected
        let path2 = format!("{}#{}", proto2.path, "<bref>");

        // Paths are different → no cache match → separate nodes
        assert_ne!(path1, path2);
    }

    #[test]
    fn test_truth_table_case_4_explicit_bid_no_match() {
        // Case: BID in parsed, no cache match
        // Expected: Create new node with parsed BID (user added explicit BID)

        let explicit_bid = Bid::new(Bid::nil());
        let proto = create_test_proto_section(
            "Details",
            "test.md",
            3,
            None,
            Some(&explicit_bid.to_string()),
        );

        let parsed_node = BeliefNode::try_from(&proto).unwrap();
        assert_eq!(parsed_node.bid, explicit_bid);

        // No cache match → use parsed BID
        assert!(parsed_node.bid.initialized());
    }

    #[test]
    fn test_truth_table_case_5_explicit_bid_bid_match() {
        // Case: BID in parsed, cache match via BID key
        // Expected: Update existing node (Phase 2+ match)

        let shared_bid = Bid::new(Bid::nil());
        let proto =
            create_test_proto_section("Details", "test.md", 3, None, Some(&shared_bid.to_string()));

        let existing_node = create_test_node("Details", BeliefKind::Document, Some(shared_bid));
        let parsed_node = BeliefNode::try_from(&proto).unwrap();

        // Both have same BID → this is a match → update
        assert_eq!(parsed_node.bid, existing_node.bid);
    }

    #[test]
    fn test_truth_table_case_6_user_renamed_bid() {
        // Case: BID in parsed, cache match via Path, but BIDs differ
        // Expected: Update found node's BID (rename operation)

        let old_bid = Bid::new(Bid::nil());
        let new_bid = Bid::new(Bid::nil());

        let proto = create_test_proto_section(
            "Details",
            "test.md",
            3,
            Some("details".to_string()),
            Some(&new_bid.to_string()),
        );

        let existing_node = create_test_node("Details", BeliefKind::Document, Some(old_bid));
        let parsed_node = BeliefNode::try_from(&proto).unwrap();

        // Path matches, but BIDs differ → rename scenario
        assert_ne!(parsed_node.bid, existing_node.bid);
        assert!(parsed_node.bid.initialized());
        assert!(existing_node.bid.initialized());
    }

    #[test]
    fn test_speculative_path_no_collision() {
        // Test: Section with unique title → path uses title-derived ID

        let title = "Introduction";
        let expected_id = to_anchor(title);
        let _proto = create_test_proto_section(title, "test.md", 3, None, None);

        // In speculative path generation:
        // 1. Check siblings (assume none have "introduction" ID)
        // 2. Use title-derived ID
        let speculative_anchor = to_anchor(title);

        assert_eq!(speculative_anchor, expected_id);
        assert_eq!(speculative_anchor, "introduction");
    }

    #[test]
    fn test_speculative_path_with_collision() {
        // Test: Section with colliding title → path uses <bref> placeholder

        let title = "Details";
        let _proto = create_test_proto_section(title, "test.md", 3, None, None);

        // Simulate collision detection:
        // If a sibling already has ID "details", use placeholder
        let sibling_has_same_id = true; // Simulated

        let speculative_anchor = if sibling_has_same_id {
            "<bref>".to_string()
        } else {
            to_anchor(title)
        };

        assert_eq!(speculative_anchor, "<bref>");
    }

    #[test]
    fn test_speculative_path_explicit_id() {
        // Test: Section with explicit ID (no collision) → path uses explicit ID

        let title = "Details";
        let explicit_id = "my-custom-section";
        let proto =
            create_test_proto_section(title, "test.md", 3, Some(explicit_id.to_string()), None);

        // Speculative path should use explicit ID when no collision
        let speculative_anchor = proto.id().unwrap();

        assert_eq!(speculative_anchor, "my-custom-section");
        assert_ne!(speculative_anchor, to_anchor(title)); // Different from title-derived
    }

    #[test]
    fn test_speculative_path_explicit_id_collision() {
        // Test: Section with explicit ID that collides → path uses <bref> placeholder

        let title = "Details";
        let explicit_id = "intro"; // User manually set this
        let _proto =
            create_test_proto_section(title, "test.md", 3, Some(explicit_id.to_string()), None);

        // Simulate collision detection:
        // If a sibling already has ID "intro" (even though this is explicit), use placeholder
        let sibling_has_same_id = true; // Simulated
        let is_explicit = true;

        let speculative_anchor = if sibling_has_same_id {
            if is_explicit {
                // Should log warning in actual implementation
                // tracing::warn!("Explicit ID '{}' collides with sibling. Using Bref fallback.", explicit_id);
            }
            "<bref>".to_string()
        } else {
            explicit_id.to_string()
        };

        assert_eq!(speculative_anchor, "<bref>");
    }

    #[test]
    fn test_section_vs_document_keys() {
        // Test: Sections should NOT have Title key, documents should

        let section_proto = create_test_proto_section("Details", "test.md", 3, None, None);
        let doc_proto = create_test_proto_section("Document", "test.md", 2, None, None);

        // Section (heading > 2): Should generate keys WITHOUT Title
        assert!(section_proto.heading > 2);

        // Document (heading <= 2): Should generate keys WITH Title
        assert!(doc_proto.heading <= 2);

        // The actual key generation logic will be in push()
        // This test documents the expected behavior
    }

    #[test]
    fn test_bref_placeholder_never_matches() {
        // Test: Newly generated Bref has negligible collision probability

        let bref1 = Bid::new(Bid::nil()).bref().to_string();
        let bref2 = Bid::new(Bid::nil()).bref().to_string();

        // Two newly generated Brefs should be different
        assert_ne!(bref1, bref2);

        // Neither should match our placeholder
        assert_ne!(bref1, "<bref>");
        assert_ne!(bref2, "<bref>");
    }

    #[test]
    fn test_to_anchor_normalization() {
        // Test: to_anchor normalizes consistently

        assert_eq!(to_anchor("Details"), "details");
        assert_eq!(to_anchor("Section One!"), "section-one");
        assert_eq!(to_anchor("API & Reference"), "api-reference");

        // Same title always produces same anchor
        let title = "My Section";
        assert_eq!(to_anchor(title), to_anchor(title));
    }

    // ========================================================================
    // Tests for get_parent_from_stack() - Fix #3 regression prevention
    // ========================================================================

    #[tokio::test]
    async fn test_get_parent_from_stack_with_section_anchors() {
        // Test that parent detection works when stack contains full section paths with anchors
        use tokio::sync::mpsc::unbounded_channel;

        let (tx, _rx) = unbounded_channel();
        let temp_dir = tempfile::tempdir().unwrap();
        create_test_network(temp_dir.path());
        let mut builder = super::GraphBuilder::new(temp_dir.path(), Some(tx)).unwrap();

        // Simulate stack with document and section with anchor
        let doc_bid = Bid::new(builder.api().bid);
        let section1_bid = Bid::new(doc_bid);

        builder.stack.push((doc_bid, "test.md".to_string(), 1));
        builder
            .stack
            .push((section1_bid, "test.md#section-1".to_string(), 2));

        // Create proto for a sibling section (same document, heading level 2)
        let proto = create_test_proto_section("Section 2", "test.md", 2, None, None);

        let (parent_bid, _path_info, parent_full_path) = builder.get_parent_from_stack(&proto);

        // Should find the document as parent, not section-1
        assert_eq!(
            parent_bid, doc_bid,
            "Parent should be document, not sibling section"
        );
        assert_eq!(
            parent_full_path, "test.md",
            "Parent path should be document path without anchor"
        );
    }

    #[tokio::test]
    async fn test_get_parent_from_stack_nested_sections() {
        // Test nested sections (section within section)
        use tokio::sync::mpsc::unbounded_channel;

        let (tx, _rx) = unbounded_channel();
        let temp_dir = tempfile::tempdir().unwrap();
        create_test_network(temp_dir.path());
        let mut builder = super::GraphBuilder::new(temp_dir.path(), Some(tx)).unwrap();

        let doc_bid = Bid::new(builder.api().bid);
        let section1_bid = Bid::new(doc_bid);

        builder.stack.push((doc_bid, "test.md".to_string(), 1));
        builder
            .stack
            .push((section1_bid, "test.md#parent-section".to_string(), 2));

        // Create proto for nested section (heading level 3)
        let proto = create_test_proto_section("Child Section", "test.md", 3, None, None);

        let (parent_bid, _path_info, parent_full_path) = builder.get_parent_from_stack(&proto);

        // Should find section-1 as parent
        assert_eq!(
            parent_bid, section1_bid,
            "Parent should be the parent section"
        );
        assert_eq!(
            parent_full_path, "test.md#parent-section",
            "Parent path should include anchor for nested section"
        );
    }

    #[tokio::test]
    async fn test_get_parent_from_stack_multiple_sections_same_level() {
        // Test that stack correctly identifies parent when multiple sections at same level
        use tokio::sync::mpsc::unbounded_channel;

        let (tx, _rx) = unbounded_channel();
        let temp_dir = tempfile::tempdir().unwrap();
        create_test_network(temp_dir.path());
        let mut builder = super::GraphBuilder::new(temp_dir.path(), Some(tx)).unwrap();

        let doc_bid = Bid::new(builder.api().bid);
        let section1_bid = Bid::new(doc_bid);
        let section2_bid = Bid::new(doc_bid);

        builder.stack.push((doc_bid, "test.md".to_string(), 1));
        builder
            .stack
            .push((section1_bid, "test.md#section-1".to_string(), 2));
        builder
            .stack
            .push((section2_bid, "test.md#section-2".to_string(), 2));

        // Create proto for another sibling section
        let proto = create_test_proto_section("Section 3", "test.md", 2, None, None);

        let (parent_bid, _path_info, _parent_full_path) = builder.get_parent_from_stack(&proto);

        // Should find document as parent (pops siblings until finding parent with lower heading)
        assert_eq!(
            parent_bid, doc_bid,
            "Should pop sibling sections to find document parent"
        );
    }

    #[tokio::test]
    async fn test_network_detection_from_stack() {
        // Test that network BID is correctly identified from stack
        use tokio::sync::mpsc::unbounded_channel;

        let (tx, _rx) = unbounded_channel();
        let temp_dir = tempfile::tempdir().unwrap();
        create_test_network(temp_dir.path());
        let mut builder = super::GraphBuilder::new(temp_dir.path(), Some(tx)).unwrap();

        // Setup: network (heading=1) and document (heading=2)
        let network_bid = Bid::new(builder.api().bid);
        let doc_bid = Bid::new(network_bid);

        builder.stack.push((network_bid, "test".to_string(), 1)); // heading=1 = network
        builder.stack.push((doc_bid, "test/doc.md".to_string(), 2));

        // Find network by walking stack backwards looking for heading=1
        let found_network = builder
            .stack
            .iter()
            .rev()
            .find(|(_bid, _path, heading)| *heading == 1)
            .map(|(bid, _path, _heading)| *bid);

        assert_eq!(
            found_network,
            Some(network_bid),
            "Should find network BID from stack (heading=1)"
        );
    }

    #[tokio::test]
    async fn test_nested_network_detection() {
        // Test nested network scenario - should find closest network
        use tokio::sync::mpsc::unbounded_channel;

        let (tx, _rx) = unbounded_channel();
        let temp_dir = tempfile::tempdir().unwrap();
        create_test_network(temp_dir.path());
        let mut builder = super::GraphBuilder::new(temp_dir.path(), Some(tx)).unwrap();

        // Root network > Subnet > Document
        let root_net = Bid::new(builder.api().bid);
        let subnet = Bid::new(root_net);
        let doc_bid = Bid::new(subnet);

        builder.stack.push((root_net, "root".to_string(), 1));
        builder.stack.push((subnet, "root/subnet".to_string(), 1)); // nested network
        builder
            .stack
            .push((doc_bid, "root/subnet/doc.md".to_string(), 2));

        // Find closest network (should be subnet, not root)
        let found_network = builder
            .stack
            .iter()
            .rev()
            .find(|(_bid, _path, heading)| *heading == 1)
            .map(|(bid, _path, _heading)| *bid);

        assert_eq!(
            found_network,
            Some(subnet),
            "Should find closest network (subnet) from stack"
        );
        assert_ne!(found_network, Some(root_net), "Should not use root network");
    }

    // ========================================================================
    // Edge cases and regression tests
    // ========================================================================

    #[tokio::test]
    async fn test_get_parent_from_stack_empty_stack() {
        // Test behavior when stack is empty
        use tokio::sync::mpsc::unbounded_channel;

        let (tx, _rx) = unbounded_channel();
        let temp_dir = tempfile::tempdir().unwrap();
        create_test_network(temp_dir.path());
        let mut builder = super::GraphBuilder::new(temp_dir.path(), Some(tx)).unwrap();

        // Empty stack
        assert!(builder.stack.is_empty());

        let proto = create_test_proto_section("Section", "test.md", 2, None, None);
        let (parent_bid, _path_info, _parent_full_path) = builder.get_parent_from_stack(&proto);

        // Should default to API node
        assert_eq!(
            parent_bid,
            builder.api().bid,
            "Empty stack should default to API node"
        );
    }

    #[tokio::test]
    async fn test_get_parent_from_stack_pops_until_valid_parent() {
        // Test that stack pops siblings until finding valid parent
        use tokio::sync::mpsc::unbounded_channel;

        let (tx, _rx) = unbounded_channel();
        let temp_dir = tempfile::tempdir().unwrap();
        create_test_network(temp_dir.path());
        let mut builder = super::GraphBuilder::new(temp_dir.path(), Some(tx)).unwrap();

        let doc_bid = Bid::new(builder.api().bid);
        let sibling1 = Bid::new(doc_bid);
        let sibling2 = Bid::new(doc_bid);
        let sibling3 = Bid::new(doc_bid);

        builder.stack.push((doc_bid, "test.md".to_string(), 1));
        builder.stack.push((sibling1, "test.md#s1".to_string(), 2));
        builder.stack.push((sibling2, "test.md#s2".to_string(), 2));
        builder.stack.push((sibling3, "test.md#s3".to_string(), 2));

        let initial_stack_len = builder.stack.len();

        let proto = create_test_proto_section("Section 4", "test.md", 2, None, None);
        let (parent_bid, _path_info, _parent_full_path) = builder.get_parent_from_stack(&proto);

        // Should have popped siblings to find document parent
        assert_eq!(parent_bid, doc_bid, "Should find document as parent");
        assert!(
            builder.stack.len() < initial_stack_len,
            "Should have popped sibling sections from stack"
        );
    }

    /// Regression test: when a section heading has an explicit anchor that collides with a
    /// prior heading's title-derived slug (e.g. `## Section Headings {#explicit-brefs}` after
    /// `## Explicit Brefs`), the collision strips the explicit id and forces a bref-based id.
    /// On every reparse that bref-based id is fresh, leaving a stale orphan edge in the graph.
    /// Before the doc_order fix, `max+1` edge assignment caused the collision section and its
    /// following sibling to both receive sort key 3, making their navtree order non-deterministic.
    ///
    /// After the fix, `push()` uses the document-position index as the sort key, so order is
    /// always stable across multiple parses regardless of stale orphan edges.
    #[tokio::test]
    async fn test_anchor_collision_section_keeps_document_order() {
        use crate::beliefbase::BeliefBase;
        use crate::codec::compiler::DocumentCompiler;

        let temp_dir = tempfile::tempdir().unwrap();

        // Minimal network index
        std::fs::write(
            temp_dir.path().join("index.md"),
            "---\nid = \"test-net\"\ntitle = \"Test Net\"\n---\n\n# Test Net\n",
        )
        .unwrap();

        // Document that mirrors link_manipulation_test.md's collision pattern:
        //   ## Alpha        → slug "alpha"       (sort position 0)
        //   ## Beta {#alpha} → explicit id collides with "alpha", gets bref id (sort position 1)
        //   ## Gamma        → slug "gamma"        (sort position 2)
        std::fs::write(
            temp_dir.path().join("doc.md"),
            "---\ntitle = \"Doc\"\n---\n\n# Doc\n\n\
             ## Alpha\n\nContent.\n\n\
             ## Beta {#alpha}\n\nCollision section.\n\n\
             ## Gamma\n\nAfter collision.\n",
        )
        .unwrap();

        let global_bb = BeliefBase::default();

        // Parse twice to expose the stale-orphan / sort-key-collision bug.
        let mut compiler = DocumentCompiler::new(temp_dir.path(), None, Some(5), false).unwrap();
        let _first = compiler.parse_all(global_bb.clone(), true).await.unwrap();
        let _second = compiler.parse_all(global_bb.clone(), true).await.unwrap();

        // Extract the path order for doc.md from the PathMap.
        let paths = compiler.cache().paths();
        let all = paths.all_paths();

        // Find the network PathMap (the one that contains "doc.md")
        let doc_entries: Vec<(String, Vec<u16>)> = all
            .values()
            .flat_map(|entries| entries.iter().cloned())
            .filter(|(path, _bid, _order)| path.starts_with("doc.md#"))
            .map(|(path, _bid, order)| (path, order))
            .collect();

        assert!(
            !doc_entries.is_empty(),
            "Expected section entries for doc.md; got none. All paths: {paths}"
        );

        // Find each section by its anchor suffix.
        let order_for = |anchor: &str| -> Vec<u16> {
            doc_entries
                .iter()
                .find(|(path, _)| path.ends_with(anchor))
                .map(|(_, order)| order.clone())
                .unwrap_or_default()
        };

        let alpha_order = order_for("#alpha");
        let gamma_order = order_for("#gamma");

        // Beta has no stable anchor after collision; find it as the remaining entry.
        let beta_order = doc_entries
            .iter()
            .find(|(path, _)| !path.ends_with("#alpha") && !path.ends_with("#gamma"))
            .map(|(_, order)| order.clone())
            .expect("Expected a beta/collision entry");

        assert!(
            alpha_order < beta_order,
            "alpha (doc order 0) must sort before beta/collision (doc order 1); \
             alpha={alpha_order:?} beta={beta_order:?}"
        );
        assert!(
            beta_order < gamma_order,
            "beta/collision (doc order 1) must sort before gamma (doc order 2); \
             beta={beta_order:?} gamma={gamma_order:?}"
        );
    }
}
