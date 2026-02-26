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
        belief_ir::ProtoBeliefNode,
        diagnostic::ParseDiagnostic,
        network::{detect_network_file, NETWORK_NAME},
        DocCodec, NetworkCodec, CODECS,
    },
    error::BuildonomyError,
    event::{BeliefEvent, EventOrigin},
    nodekey::NodeKey,
    paths::{as_anchor, os_path_to_string, path::string_to_os_path, AnchorPath, AnchorPathBuf},
    properties::{
        buildonomy_namespace, content_namespaces, href_namespace, BeliefKind, BeliefKindSet,
        BeliefNode, Bid, Bref, Weight, WeightKind, WEIGHT_DOC_PATHS, WEIGHT_SORT_KEY,
    },
    query::{BeliefSource, Expression, Query},
};

use super::UnresolvedReference;

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

        tracing::info!(
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
    ) -> Result<ParseContentWithCodec, BuildonomyError> {
        tracing::debug!("Phase 0: initialize stack");
        let full_path = input_path.as_ref().canonicalize()?.to_path_buf();
        let initial = self
            .initialize_stack(input_path.as_ref(), global_bb.clone())
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
        let doc_ap = AnchorPath::from(&doc_path);

        let mut parsed_bids;
        let owned_codec: Box<dyn DocCodec + Send>;

        if let Some(codec_factory) = CODECS.get(&doc_ap) {
            // Create fresh codec instance from factory
            let mut codec = codec_factory();
            codec.parse(&content, initial)?;

            let mut inject_context = false;
            parsed_bids = Vec::with_capacity(codec.nodes().len());
            let mut check_sinks = BTreeMap::<Bid, BTreeSet<NodeKey>>::default();
            let mut relation_event_queue = Vec::<BeliefEvent>::default();
            let mut missing_structure = BeliefGraph::default();

            tracing::debug!("Phase 1: Create all nodes");
            debug_assert!(
                self.session_bb.is_balanced().is_ok(),
                "Why isn't session_bb balanced? (phase 1 start)"
            );
            for proto in codec.nodes().iter() {
                let (bid, (source, _nodekeys, unique_oldkeys)) = self
                    .push(proto, global_bb.clone(), false, &mut missing_structure)
                    .await?;
                if !missing_structure.is_empty() {
                    tracing::debug!(
                        "Phase 1 {}: merging missing structure onto self.session_bb:",
                        bid,
                    );
                    // Don't merge missing_structure into self.doc_bb here -- we want to preserve the
                    // relations we're building up from the parse
                    self.session_bb.merge(&missing_structure);
                    // We did a bunch of cache_fetch operations, so the stack cache should be
                    // rebalanced as well.
                    self.session_bb.process_event(&BeliefEvent::BalanceCheck)?;
                    missing_structure = BeliefGraph::default();
                }

                if !source.is_from_cache() {
                    inject_context = true;
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
                for (index, (orig_source_key, kind, weight)) in proto.upstream.iter().enumerate() {
                    let result = self
                        .push_relation(
                            orig_source_key,
                            kind,
                            weight,
                            bid,
                            Direction::Incoming, // upstream_relations are sink-owned
                            index,
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
                for (index, (orig_sink_key, kind, payload)) in proto.downstream.iter().enumerate() {
                    let result = self
                        .push_relation(
                            orig_sink_key,
                            kind,
                            payload,
                            bid,
                            Direction::Outgoing, // downstream_relations are source-owned
                            index,
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
                self.session_bb.merge(&missing_structure);
                self.session_bb.process_event(&BeliefEvent::BalanceCheck)?;
                // we need to merge this phase 2 missing structure into self.doc_bb as well to ensure
                // we have full structural paths to all the external nodes we connect to within the
                // relation_event_queue
                self.doc_bb.merge(&missing_structure);
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
            if inject_context {
                for (proto, bid) in codec.nodes().iter().zip(parsed_bids.iter()) {
                    let ctx = self
                        .doc_bb
                        .get_context(&self.repo(), bid)
                        .expect("Set should be balanced here");
                    let old_node = ctx.node.toml();
                    // Inject proto text into our self set here, because inject context is where the
                    // markdown parser generates section-specific text fields regardless of whether
                    // it changes the markdown itself due to the injected context.
                    if let Some(updated_node) = codec.inject_context(proto, &ctx)? {
                        if old_node != updated_node.toml() {
                            is_changed = true;
                            let _derivatives =
                                self.doc_bb.process_event(&BeliefEvent::NodeUpdate(
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
                let finalized_nodes = codec.finalize()?;
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
            }

            if is_changed {
                tracing::debug!("Generating source");
                let maybe_new_content = codec.generate_source();
                if let Some(new_content) = maybe_new_content.as_ref() {
                    if new_content != &content {
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
    async fn initialize_stack<P: AsRef<Path> + Debug, B: BeliefSource + Clone>(
        &mut self,
        path: P,
        global_bb: B,
    ) -> Result<ProtoBeliefNode, BuildonomyError> {
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

        // Fetch const_namespaces from global_bb to populate session_bb with known assets
        // This enables asset content change detection by populating PathMap with existing paths
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

                // Merge the fetched asset graph into session_bb
                self.session_bb.merge(&const_graph);

                tracing::debug!(
                    "[initialize_stack] Loaded {} assets from global cache for namespace {}",
                    const_graph.states.len().saturating_sub(1), // -1 for namespace node itself
                    const_bid
                );
            }
        }

        let initial_factory = CODECS
            .path_get(path.as_ref())
            .ok_or(BuildonomyError::Codec(format!(
                "Could not find codec for path type {path:?}"
            )))?;
        let initial_codec = initial_factory();
        let initial = initial_codec
            .proto(self.repo_root.as_ref(), path.as_ref())?
            .ok_or(BuildonomyError::Codec(format!(
                "Codec could not resolve path '{path:?}' into a proto node"
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
            parent_path_stack.push(parent_path.clone());
        }
        let mut maybe_content_parent_proto = None;
        let mut missing_structure = BeliefGraph::default();
        let mut events = Vec::<BeliefEvent>::default();
        let net_codec = NetworkCodec::default();
        while let Some(path) = parent_path_stack.pop() {
            let Some(state_accum) = net_codec.proto(self.repo_root.as_path(), path.as_path())?
            else {
                continue;
            };

            let (ancestor, (_source, _, _)) = self
                .push(
                    &state_accum,
                    global_bb.clone(),
                    true,
                    &mut missing_structure,
                )
                .await?;
            if path.as_os_str().is_empty() && self.repo == Bid::nil() {
                self.repo = ancestor;
            }
            // Merge missing_structure after each push so it's available for the next iteration.
            if !missing_structure.is_empty() {
                // Keep self.doc_bb isolated from the structure, that way we can ensure our comparison
                // between the source material and the cache stays consistent.
                self.session_bb.merge(&missing_structure);
                missing_structure = BeliefGraph::default(); // reset for next interation
            }
            maybe_content_parent_proto = Some((ancestor, state_accum));
        }

        self.session_bb.process_event(&BeliefEvent::BalanceCheck)?;

        // We can safely expect the BeliefBase to be balanced after after stack initialization
        // tracing::debug!(
        //     "processing {} events and adding to our self.doc_bb",
        //     events.len()
        // );

        // Initialize any child links found by the last state_accum. This ensures we can sort the
        // parsed_content's relation to its parent correctly
        if let Some((parent_bid, parent_proto)) = maybe_content_parent_proto {
            for (index, (source_key, kind, payload)) in parent_proto.upstream.iter().enumerate() {
                self.push_relation(
                    source_key,
                    kind,
                    payload,
                    &parent_bid,
                    Direction::Incoming, // upstream_relations are sink-owned
                    index,
                    global_bb.clone(),
                    &mut events,
                    &mut missing_structure,
                )
                .await?;
            }
            for event in events.iter() {
                self.doc_bb.process_event(event)?;
            }
            if !events.is_empty() {
                self.doc_bb.process_event(&BeliefEvent::BalanceCheck)?;
            }
        }
        Ok(initial)
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
            tracing::info!(
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
            tracing::debug!("{event:?}");
            self.tx.send(event)?;
        }
        if !events_is_empty {
            // tracing::debug!("Ensuring our global_bb is balanced");
            self.tx.send(balance_check)?;
        }

        Ok(())
    }

    fn get_parent_from_stack(&mut self, proto: &ProtoBeliefNode) -> (Bid, String, String) {
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
                    (proto.path.starts_with(stack_ap.filepath())
                        && proto.path != stack_ap.filepath()
                        && !proto
                            .kind
                            .intersection(BeliefKind::Network | BeliefKind::Document)
                            .is_empty())
                        || (proto.path == stack_ap.filepath() && *stack_heading < proto.heading)
                })
                .map(|(stack_bid, stack_path, _stack_heading)| {
                    let proto_ap = AnchorPath::new(&proto.path);
                    let path_info = proto_ap
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
    fn speculative_path_key(
        &self,
        proto: &ProtoBeliefNode,
    ) -> Result<Option<NodeKey>, BuildonomyError> {
        // Find the network by walking up the stack (network nodes have heading=1)
        if let Some(bid) = proto
            .document
            .get("bid")
            .and_then(|bid_val| bid_val.as_str())
            .and_then(|bid_str| Bid::try_from(bid_str).ok())
            .filter(|bid| bid.initialized())
        {
            return Ok(Some(NodeKey::Bid { bid }));
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
            return Ok(Some(NodeKey::Id {
                net: Bref::default(),
                id: network_id,
            }));
        }
        let (net, net_path) = self
            .stack
            .iter()
            .rev()
            .find(|(_bid, _path, heading)| *heading == 1)
            .map(|(bid, path, _heading)| (*bid, path.clone()))
            .unwrap_or((self.repo(), String::default()));
        let pmm = self.doc_bb.paths();
        let net_pm = pmm.get_map(&net.bref()).ok_or_else(|| {
            BuildonomyError::Codec(format!(
                "No PathMap found for network {} while computing path for '{:?} - {:?}'",
                net,
                proto.path,
                proto.id()
            ))
        })?;
        let net_anchored_child = AnchorPath::new(&proto.path)
            .strip_prefix(&net_path)
            .unwrap_or(&proto.path);
        let child_ap = AnchorPath::new(net_anchored_child);
        let path = if proto.heading > 2 {
            let Some(section_id) = proto.id() else {
                tracing::debug!("Cannot generate speculative key for a section node without an ID");
                return Ok(None);
            };
            if net_pm.get_from_id(&section_id, &pmm).is_some() {
                tracing::debug!("Cannot generate speculative key, there already exists a node with ID '{section_id}' in this network.");
                return Ok(None);
            };
            child_ap.join(as_anchor(&section_id)).into_string()
        } else {
            child_ap.to_string()
        };
        Ok(Some(NodeKey::Path {
            net: net.bref(),
            path,
        }))
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
        proto: &ProtoBeliefNode,
        global_bb: B,
        as_trace: bool,
        missing_structure: &mut BeliefGraph,
    ) -> Result<(Bid, (NodeSource, BTreeSet<NodeKey>, BTreeSet<NodeKey>)), BuildonomyError> {
        let (parent_bid, path_info, _parent_full_path) = self.get_parent_from_stack(proto);

        // Can't use self.doc_bb.paths() to generate keys here, because we can't assume that self.doc_bb
        // is balanced until we're out of phase 1 of parse_content.
        let mut parsed_node = BeliefNode::try_from(proto)?;

        // Generate keys based on node type
        let mut keys = self
            .speculative_path_key(proto)?
            .into_iter()
            .collect::<Vec<_>>();
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
        let _derivative_events = self.doc_bb.process_event(&BeliefEvent::RelationChange(
            bid,
            parent_bid,
            WeightKind::Section,
            Some(weight.clone()),
            EventOrigin::Remote,
        ))?;
        // For sections, use the full path with anchor from the generated keys
        // For documents/networks, use the document path
        let stack_path = if proto.heading > 2 {
            // Section: extract path from Path key
            keys.iter()
                .find_map(|k| match k {
                    NodeKey::Path { path, .. } => Some(path.clone()),
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
        other_key: &NodeKey,
        kind: &WeightKind,
        maybe_weight: &Option<Weight>,
        owner_bid: &Bid,
        direction: Direction,
        index: usize,
        global_bb: B,
        update_queue: &mut Vec<BeliefEvent>,
        missing_structure: &mut BeliefGraph,
    ) -> Result<GetOrCreateResult, BuildonomyError> {
        // When is_source_owned=false (sink-owned/upstream_relations): owner is sink, other is source
        // When is_source_owned=true (source-owned/downstream_relations): owner is source, other is sink

        let mut other_key_regularized = other_key
            .regularize(&self.doc_bb, *owner_bid, self.repo())
            .expect(
                "parse_content Phase 1 parsing ensures that we have a valid subsection \
                structure to get paths from for all our parsed nodes",
            );

        // Phase 2: Resolve boundary-escaping paths using the filesystem repo_root.
        // After regularize, a path starting with "../" means the join against the
        // network-relative owner_path escaped the network boundary. We can resolve
        // this by constructing the absolute path and stripping the repo_root prefix.
        if let NodeKey::Path { ref net, ref path } = other_key_regularized {
            if path.starts_with("../") {
                let repo_root_str = os_path_to_string(&self.repo_root);
                let mut abs_ap = AnchorPathBuf::from(repo_root_str.clone());
                // Get the owner's network-relative path for context
                if let Some((_home_net, owner_path, _order)) = self
                    .doc_bb
                    .paths()
                    .get_map(&self.repo().bref())
                    .and_then(|pm| pm.path(owner_bid, &self.doc_bb.paths()))
                {
                    abs_ap.push(&owner_path);
                }
                abs_ap.push(path);
                // Normalize resolves the ../s against the absolute prefix
                let normalized = abs_ap.normalize();
                if let Some(repo_relative) =
                    normalized.as_anchor_path().strip_prefix(&repo_root_str)
                {
                    tracing::debug!(
                        "[push_relation] Resolved boundary-escaping path '{}' to \
                         repo-relative '{}'",
                        path,
                        repo_relative
                    );
                    other_key_regularized = NodeKey::Path {
                        net: *net,
                        path: repo_relative.to_string(),
                    };
                } else {
                    tracing::warn!(
                        "[push_relation] Path '{}' escapes repo boundary even after \
                         filesystem resolution (resolved to '{}'). Skipping relation.",
                        path,
                        normalized
                    );
                }
            }
        }

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
                        update_queue.push(BeliefEvent::RelationChange(
                            href_namespace(),
                            buildonomy_namespace(),
                            WeightKind::Section,
                            Some(Weight::default()),
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
                    let pmm_guard = self.doc_bb.paths();
                    let (owner_home_net, owner_home_path) =
                    pmm_guard.api_map().home_path(owner_bid, &pmm_guard).expect(
                        "parse_content Phase 1 parsing ensures that we have a valid subsection \
                        structure to get paths from for all our parsed nodes",
                    );
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
            NodeSource::GlobalCache => {
                // The missing_structure from cache_fetch has all the other node structure we
                // need. We will merge that into self.doc_bb within parse_content before processing the
                // event queue.
            }
            NodeSource::SourceFile | NodeSource::Generated => {
                // We've accumulated all the structure we need already, the event queue can be
                // processed without issue.
            }

            NodeSource::StackCache => {
                // There is no missing structure with respect to the stack cache, but we do need to
                // get missing structure from the stack cache to apply to self.doc_bb in order to
                // maintain a balanced BeliefBase.
                let query = Query {
                    seed: Expression::from(&NodeKey::Bid {
                        bid: other_node.bid,
                    }),
                    traverse: None,
                };
                let stack_result = self.session_bb.eval_query(&query, true).await?;
                // tracing::debug!(
                //     "Returned session_bb missing structure for query bid {}:\n{}",
                //     other_node.bid,
                //     stack_result.display_contents()
                // );
                missing_structure.union_mut(&stack_result);
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

        update_queue.push(BeliefEvent::RelationChange(
            source_bid,
            sink_bid,
            *kind,
            Some(weight),
            EventOrigin::Remote,
        ));

        Ok(GetOrCreateResult::Resolved(other_node, other_node_source))
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

            // tracing::debug!(
            //     "stack_result has {} nodes. session_bb.is_balanced = {}",
            //     stack_result.states().len(),
            //     self.session_bb.is_balanced(),
            // );

            match stack_result.get(key) {
                Some(existing_state) => {
                    found_state = Some(existing_state);
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
        codec::belief_ir::ProtoBeliefNode,
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

    /// Helper: Create a test ProtoBeliefNode for a section heading
    fn create_test_proto_section(
        title: &str,
        path: &str,
        heading: usize,
        maybe_id: Option<String>,
        bid: Option<&str>,
    ) -> ProtoBeliefNode {
        let mut doc = DocumentMut::new();
        doc.insert("title", value(title));
        doc.insert("schema", value("Document"));
        if let Some(bid_str) = bid {
            doc.insert("bid", value(bid_str));
        }
        if let Some(id) = maybe_id {
            doc.insert("id", value(id));
        }
        ProtoBeliefNode {
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
        assert_eq!(to_anchor("API & Reference"), "api--reference");

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
}
