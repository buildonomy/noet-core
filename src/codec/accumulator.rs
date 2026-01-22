use petgraph::{visit::EdgeRef, Direction};
use serde::{Deserialize, Serialize};
/// Utilities for parsing various document types into BeliefSets
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Debug,
    path::{Path, PathBuf},
    result::Result,
    time::Duration,
};
use tokio::{sync::mpsc::UnboundedSender, time::sleep};
/// Utilities for parsing various document types into BeliefSets
use toml::value::Table as TomlTable;

use crate::{
    beliefset::{BeliefSet, Beliefs},
    codec::{
        diagnostic::ParseDiagnostic,
        lattice_toml::{ProtoBeliefNode, NETWORK_CONFIG_NAME},
        CODECS,
    },
    error::BuildonomyError,
    event::{BeliefEvent, EventOrigin},
    nodekey::{trim_path_sep, NodeKey},
    paths::relative_path,
    properties::{
        buildonomy_namespace, href_namespace, BeliefKind, BeliefKindSet, BeliefNode, Bid, Weight,
        WeightKind, WEIGHT_SORT_KEY,
    },
    query::{BeliefCache, Expression, Query},
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

/// Result of parsing document content
#[derive(Debug, Clone)]
pub struct ParseContentResult {
    /// Optionally rewritten content if BIDs were injected or links updated
    pub rewritten_content: Option<String>,

    /// Diagnostics collected during parsing (unresolved refs, warnings, etc.)
    pub diagnostics: Vec<ParseDiagnostic>,
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
pub struct BeliefSetAccumulator {
    // pub parsed_content: BTreeSet<Bid>,
    // pub parsed_structure: BTreeSet<Bid>,
    set: BeliefSet,
    repo: Bid,
    repo_root: PathBuf,
    stack: Vec<(Bid, String, usize)>,
    stack_cache: BeliefSet,
    tx: UnboundedSender<BeliefEvent>,
}

/// BeliefSetAccumulator collects source material, parses it into a BeliefSet representation, maps
/// that to the last-known representation of the set in order to determine consistent state and
/// relation IDs and weights, and finally publishes updated versions of the set back to the source
/// material as well as to the provided global_cache [BeliefCache] implementation.
///
/// A core responsibility of the accumulator is to integrate relative file paths, arbitrary document
/// structures, and other arbitrary API formats, as well as the URL schema/protocol into a unified
/// relative or absolute identification for each node referenced within a BeliefNetwork.
///
/// The accumulator is responsible for tracking changes to this mapping, such that when beliefs are
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
impl BeliefSetAccumulator {
    pub fn new<P>(
        repo_path: P,
        mut maybe_tx: Option<UnboundedSender<BeliefEvent>>,
    ) -> Result<Self, BuildonomyError>
    where
        P: AsRef<std::path::Path> + std::fmt::Debug,
    {
        let mut repo_root = PathBuf::from(repo_path.as_ref()).canonicalize()?;
        match repo_root.is_dir() {
            true => Ok(()),
            false => {
                let invalid_err = BuildonomyError::Codec(format!(
                    "BeliefSetAccumulator initialization failed. Received root path {repo_root:?}. \
                     Expected a directory or path to a {NETWORK_CONFIG_NAME} file"
                ));
                if let Some(path_name) = repo_root.file_name() {
                    if path_name.to_string_lossy()[..] == NETWORK_CONFIG_NAME[..] {
                        repo_root.pop();
                        Ok(())
                    } else {
                        tracing::warn!("{:?}", &invalid_err);
                        Err(invalid_err)
                    }
                } else {
                    tracing::warn!("{:?}", &invalid_err);
                    Err(invalid_err)
                }
            }
        }?;

        let tx = match maybe_tx.take() {
            Some(tx) => tx,
            None => {
                tracing::warn!("Accumulator was initialized without an output event transmitter, stubbing out a process to swallow parsing events");
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

        let accum = BeliefSetAccumulator {
            // parsed_content: BTreeSet::default(),
            // parsed_structure: BTreeSet::default(),
            set: BeliefSet::empty(),
            repo: Bid::nil(),
            repo_root,
            stack: Vec::default(),
            stack_cache: BeliefSet::empty(),
            tx,
        };

        tracing::info!(
            "Initializing BeliefSetAccumulator for repo_path: {:?}",
            repo_path.as_ref()
        );
        Ok(accum)
    }

    pub fn api(&self) -> &BeliefNode {
        self.set.api()
    }

    pub fn repo(&self) -> Bid {
        self.repo
    }

    pub fn set(&self) -> &BeliefSet {
        &self.set
    }

    pub fn stack_cache(&self) -> &BeliefSet {
        &self.stack_cache
    }

    pub fn repo_root(&self) -> &Path {
        &self.repo_root
    }

    pub fn built_in_test(&mut self) -> Vec<String> {
        let mut combined_errors = Vec::default();
        let mut set_errors = self.set.built_in_test(true);
        if !set_errors.is_empty() {
            combined_errors.push("accumulator.set errors:".to_string());
            combined_errors.append(&mut set_errors);
        }
        let mut stack_cache_errors = self.stack_cache.built_in_test(true);
        if !stack_cache_errors.is_empty() {
            combined_errors.push("accumulator.stack_cache errors:".to_string());
            combined_errors.append(&mut stack_cache_errors);
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
    pub async fn parse_content<
        P: AsRef<std::path::Path> + std::fmt::Debug,
        B: BeliefCache + Clone,
    >(
        &mut self,
        input_path: P,
        content: String,
        global_cache: B,
    ) -> Result<ParseContentResult, BuildonomyError> {
        tracing::debug!("Phase 0: initialize stack");
        let mut full_path = input_path.as_ref().canonicalize()?.to_path_buf();
        let initial = self
            .initialize_stack(input_path.as_ref(), global_cache.clone())
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

        if input_path.as_ref().is_dir() {
            full_path.push(NETWORK_CONFIG_NAME);
        }
        let file_err = BuildonomyError::Codec(format!(
            "Cannot parse {full_path:?}. Path has no extention type",
        ));
        let doc_home_path =
            trim_path_sep(&full_path.strip_prefix(&self.repo_root)?.to_string_lossy()).to_string();
        let ext = full_path
            .extension()
            .ok_or(file_err.clone())?
            .to_str()
            .ok_or(file_err)?;

        let mut parsed_bids;
        if let Some(codec_lock) = CODECS.get(ext) {
            while codec_lock.is_locked() {
                tracing::info!("Waiting for lock access to the codec map");
                sleep(Duration::from_millis(100)).await;
            }
            let mut codec = codec_lock.lock_arc();
            codec.parse(content, initial)?;

            let mut inject_context = false;
            parsed_bids = Vec::with_capacity(codec.nodes().len());
            let mut check_sinks = BTreeMap::<Bid, BTreeSet<NodeKey>>::default();
            let mut relation_event_queue = Vec::<BeliefEvent>::default();
            let mut missing_structure = Beliefs::default();

            tracing::debug!("Phase 1: Create all nodes");
            debug_assert!(
                self.stack_cache.is_balanced().is_ok(),
                "Why isn't stack_cache balanced? (phase 1 start)"
            );
            for proto in codec.nodes().iter() {
                let (bid, (source, _nodekeys, unique_oldkeys)) = self
                    .push(
                        proto,
                        global_cache.clone(),
                        false,
                        &mut relation_event_queue,
                        &mut missing_structure,
                    )
                    .await?;
                if !missing_structure.is_empty() {
                    tracing::debug!(
                        "Phase 1 {}: merging missing structure onto self.stack_cache",
                        bid
                    );
                    // Don't merge missing_structure into self.set here -- we want to preserve the
                    // relations we're building up from the parse
                    self.stack_cache.merge(&missing_structure);
                    // We did a bunch of cache_fetch operations, so the stack cache should be
                    // rebalanced as well.
                    self.stack_cache.process_event(&BeliefEvent::BalanceCheck)?;
                    missing_structure = Beliefs::default();
                }

                for edge_update in relation_event_queue.drain(..) {
                    let _deriv = self.set.process_event(&edge_update)?;
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

            self.set.process_event(&BeliefEvent::BalanceCheck)?;

            tracing::debug!("Phase 2: Balance and process relations");
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
                            global_cache.clone(),
                            &mut relation_event_queue,
                            &mut missing_structure,
                        )
                        .await?;

                    match result {
                        GetOrCreateResult::Resolved(_, source) => {
                            if source == NodeSource::GlobalCache {
                                inject_context = true;
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
                            global_cache.clone(),
                            &mut relation_event_queue,
                            &mut missing_structure,
                        )
                        .await?;

                    match result {
                        GetOrCreateResult::Resolved(_node, source) => {
                            if source == NodeSource::GlobalCache {
                                inject_context = true;
                            }
                        }
                        GetOrCreateResult::Unresolved(unresolved) => {
                            // Track unresolved reference as diagnostic
                            diagnostics.push(ParseDiagnostic::UnresolvedReference(unresolved));
                        }
                    }
                }
            }

            // Perform this after going through all the proto relations so we don't destroy our
            // balanced set.
            if !missing_structure.is_empty() {
                tracing::debug!("Phase 2: merging missing structure onto stack_cache and set");
                self.stack_cache.merge(&missing_structure);
                self.stack_cache.process_event(&BeliefEvent::BalanceCheck)?;
                // we need to merge this phase 2 missing structure into self.set as well to ensure
                // we have full structural paths to all the external nodes we connect to within the
                // relation_event_queue
                self.set.merge(&missing_structure);
            }
            for edge_update in relation_event_queue.drain(..) {
                let _deriv = self.set.process_event(&edge_update)?;
            }
            self.set.process_event(&BeliefEvent::BalanceCheck)?;

            tracing::debug!(
                "Phase 3: inform external sinks about nodekey changes from this document"
            );
            // (re)parse documents are are either to
            // 1) update their contents to reflect updated nodekey's from this parsed document.
            if !parsed_bids.is_empty() {
                for (source_bid, _old_keys) in check_sinks.iter() {
                    if let Some(source_idx) = self.stack_cache.bid_to_index(source_bid) {
                        let stack_paths_guard = self.stack_cache.paths();
                        let mut sink_docs = self
                            .stack_cache
                            .relations()
                            .as_graph()
                            .edges_directed(source_idx, Direction::Outgoing)
                            .filter_map(|edge| {
                                let sink = self.stack_cache.relations().as_graph()[edge.target()];
                                stack_paths_guard.get_doc(&sink)
                            })
                            .collect::<Vec<_>>();
                        sink_docs.sort_by_key(|doc_tuple| doc_tuple.2.clone());
                        for sink_doc_id in sink_docs.into_iter() {
                            if sink_doc_id.0 == doc_home_path {
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
                        .set
                        .get_context(bid)
                        .expect("Set should be balanced here");
                    // Inject proto text into our self set here, because inject context is where the
                    // markdown parser generates section-specific text fields regardless of whether
                    // it changes the markdown itself due to the injected context.
                    if let Some(updated_node) = codec.inject_context(proto, &ctx)? {
                        is_changed = true;
                        let _derivatives = self.set.process_event(&BeliefEvent::NodeUpdate(
                            vec![NodeKey::Bid {
                                bid: updated_node.bid,
                            }],
                            updated_node.toml(),
                            EventOrigin::Remote,
                        ))?;
                        tracing::debug!("phase 4 node update derivs: {:?}", _derivatives);
                    }
                }
            }

            if is_changed {
                tracing::debug!("Generating source");
                maybe_content = codec.generate_source();
            }
        } else {
            return Err(BuildonomyError::Codec(format!(
                "Cannot parse {full_path:?}. No Codec for extension type {ext} found in CodecMap"
            )));
        };

        tracing::debug!("Phase 5: terminating stack and transmitting updates to global_cache");
        self.terminate_stack(
            bid_renames,
            &BTreeSet::<Bid>::from_iter(parsed_bids.into_iter()),
        )
        .await?;

        Ok(ParseContentResult {
            rewritten_content: maybe_content,
            diagnostics,
        })
    }

    /// Initializes internal variables for parsing and merging
    async fn initialize_stack<P: AsRef<Path> + Debug, B: BeliefCache + Clone>(
        &mut self,
        path: P,
        global_cache: B,
    ) -> Result<ProtoBeliefNode, BuildonomyError> {
        // self.parsed_content.clear();
        // self.parsed_structure.clear();
        // self.parsed_structure.insert(self.api().bid);
        self.stack = vec![];
        // // Uncomment this for easier testing as it makes cache order of operations more clear.
        // self.stack_cache = BeliefSet::empty();
        self.set = BeliefSet::empty();
        let api_node = self.api().clone();
        let api_key = NodeKey::Bid { bid: api_node.bid };
        let api_node_event =
            BeliefEvent::NodeUpdate(vec![api_key.clone()], api_node.toml(), EventOrigin::Remote);
        self.set.process_event(&api_node_event)?;
        // Ensure global_cache shares our API node
        //
        // TODO figure out a way to do this check only once per Accumulator initialization instead
        // of at each initialize_stack operation.
        if self.stack_cache.get(&api_key).is_none() {
            self.stack_cache.process_event(&api_node_event)?;
        }
        if global_cache.get_async(&api_key).await?.is_none() {
            self.tx.send(api_node_event)?;
        }

        let initial = ProtoBeliefNode::new(self.repo_root.as_ref(), path.as_ref())?;

        let mut parent_path = PathBuf::from(&initial.path);
        let mut parent_path_stack: Vec<PathBuf> = Vec::default();
        // If path is a sub-network node, dont count self path as a parent path
        if parent_path.ends_with(NETWORK_CONFIG_NAME) {
            parent_path.pop();
        }
        while parent_path.pop() {
            let parent_yaml = self.repo_root.join(&parent_path).join(NETWORK_CONFIG_NAME);
            if parent_yaml.is_file() {
                parent_path_stack.push(parent_path.clone());
            }
        }
        parent_path_stack.reverse();
        let maybe_content_parent_path = parent_path_stack.last();
        let mut maybe_content_parent_proto = None;
        let mut missing_structure = Beliefs::default();
        let mut events = Vec::<BeliefEvent>::default();
        for path in parent_path_stack.iter() {
            let state_accum = ProtoBeliefNode::new(self.repo_root.as_path(), path.as_path())?;
            let (ancestor, (_source, _, _)) = self
                .push(
                    &state_accum,
                    global_cache.clone(),
                    true,
                    &mut events,
                    &mut missing_structure,
                )
                .await?;
            if path.as_os_str().is_empty() && self.repo == Bid::nil() {
                self.repo = ancestor;
            }
            // Merge missing_structure after each push so it's available for the next iteration.
            if !missing_structure.is_empty() {
                // Keep self.set isolated from the structure, that way we can ensure our comparison
                // between the source material and the cache stays consistent.
                self.stack_cache.merge(&missing_structure);
                missing_structure = Beliefs::default(); // reset for next interation
            }
            if Some(path) == maybe_content_parent_path {
                maybe_content_parent_proto = Some((ancestor, state_accum));
            }
        }

        self.stack_cache.process_event(&BeliefEvent::BalanceCheck)?;

        // We can safely expect the beliefset to be balanced after after stack initialization
        // tracing::debug!(
        //     "processing {} events and adding to our self.set",
        //     events.len()
        // );
        for event in events.iter() {
            self.set.process_event(event)?;
        }
        events.clear();
        self.set.process_event(&BeliefEvent::BalanceCheck)?;

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
                    global_cache.clone(),
                    &mut events,
                    &mut missing_structure,
                )
                .await?;
            }
            for event in events.iter() {
                self.set.process_event(event)?;
            }
            if !events.is_empty() {
                self.set.process_event(&BeliefEvent::BalanceCheck)?;
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
        self.set.process_event(&balance_check)?;
        // First, apply node renames in order to have a solid basis for our next operations
        let mut tx_events = Vec::new();
        for (from_bid, to_bid) in renamed_nodes.iter() {
            let rename_event = BeliefEvent::NodeRenamed(*from_bid, *to_bid, EventOrigin::Remote);
            let mut derivatives = self.stack_cache.process_event(&rename_event)?;
            tx_events.push(rename_event);
            tx_events.append(&mut derivatives);
        }
        let mut diff_events = BeliefSet::compute_diff(&self.stack_cache, &self.set, parsed_nodes)?;
        let mut path_events = Vec::new();
        for event in diff_events.iter() {
            let derivative_events = self.stack_cache.process_event(event)?;
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
        self.stack_cache.process_event(&balance_check)?;
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
                    BeliefEvent::RelationInsert(_, _, _, _, _) => relation_insert_count += 1,
                    BeliefEvent::RelationRemoved(_, _, _) => relation_removed_count += 1,
                    BeliefEvent::RelationUpdate(_, _, _, _) => relation_update_count += 1,
                    BeliefEvent::PathAdded(..) | BeliefEvent::PathUpdate(..) => {
                        path_update_count += 1
                    }
                    BeliefEvent::PathsRemoved(_, paths, _) => path_removed_count += paths.len(),
                    BeliefEvent::BalanceCheck => {}
                    BeliefEvent::BuiltInTest => {}
                }
            }
            tracing::info!(
                "Diff events ({}): NodeUpdate({}), NodeRemoved({}), NodeRenamed({}), RelationInsert({}), RelationRemoved({}), RelationUpdate({}), PathsAdded({}), PathsRemoved({})",
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
            tracing::debug!("{:?}", event);
            self.tx.send(event)?;
        }
        if !events_is_empty {
            // tracing::debug!("Ensuring our global_cache is balanced");
            tracing::debug!("{:?}", balance_check);
            self.tx.send(balance_check)?;
        }
        Ok(())
    }

    fn get_parent_from_stack(&mut self, proto: &ProtoBeliefNode) -> (Bid, Option<String>) {
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
                    (proto.path.starts_with(stack_path)
                        && proto.path != *stack_path
                        && !proto
                            .kind
                            .intersection(BeliefKind::Network | BeliefKind::Document)
                            .is_empty())
                        || (proto.path == *stack_path && *stack_heading < proto.heading)
                })
                .map(|(stack_bid, stack_path, _stack_heading)| {
                    let path_info = relative_path(&proto.path, stack_path)
                        .ok()
                        .filter(|rel_path| !rel_path.is_empty());
                    (*stack_bid, path_info)
                });
        }
        parent_info.unwrap_or((self.api().bid, None))
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
    async fn push<B: BeliefCache + Clone>(
        &mut self,
        proto: &ProtoBeliefNode,
        global_cache: B,
        as_trace: bool,
        event_queue: &mut Vec<BeliefEvent>,
        missing_structure: &mut Beliefs,
    ) -> Result<(Bid, (NodeSource, BTreeSet<NodeKey>, BTreeSet<NodeKey>)), BuildonomyError> {
        let (parent_bid, path_info) = self.get_parent_from_stack(proto);

        // Can't use self.set.paths() to generate keys here, because we can't assume that self.set
        // is balanced until we're out of phase 1 of parse_content.

        let mut parsed_node = BeliefNode::try_from(proto)?;
        let mut keys = parsed_node.keys(Some(self.repo()), Some(parent_bid), self.set());

        // On top of providing us with the old state of the node (if such a state exists), this will
        // also update our stack_cache to include all the old relationships tied to this node. We
        // will use this info later in terminate_stack to determine what our "affected_sink" set is,
        // that is the set of nodes external to this parsed content that 'source' information from
        // this node that need to be informed about changes to the node's reference ids (it's set of
        // nodekeys).
        let cache_fetch_result = self
            .cache_fetch(&keys, global_cache.clone(), true, missing_structure)
            .await?;

        let (mut node, source) = match cache_fetch_result {
            GetOrCreateResult::Resolved(mut found_node, mut src) => {
                if proto.document.get("bid").is_some() {
                    // Prioritize bid from a parsed document -- merge any matches from our get-or-create
                    // results.
                    if !keys.contains(&NodeKey::Bid {
                        bid: found_node.bid,
                    }) {
                        tracing::debug!(
                            "Adding cached node BID {} to old_keys for parsed node {}. Keys before: {:?}",
                            found_node.bid,
                            parsed_node.bid,
                            keys
                        );
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
                (parsed_node, source)
            }
        };
        let bid = node.bid;

        // We want parsed_node to be the source of truth for title, summary, and path. But we
        // want cache_fetch node to be source of truth for bid If source is non-accumulator
        // cache.
        if !as_trace {
            // Clear all relationships in the accumulator for this node, this way we ensure the
            // currently parsed content is processed as the source of truth for the node's content
            // and all relationships where it is the sink.
            let mut remove_events = if let Some(node_idx) = self.set.bid_to_index(&node.bid) {
                self.set
                    .relations()
                    .as_graph()
                    .edges_directed(node_idx, Direction::Incoming)
                    .map(|edge| {
                        let source = self.set.relations().as_graph()[edge.source()];
                        BeliefEvent::RelationRemoved(source, node.bid, EventOrigin::Remote)
                    })
                    .collect::<Vec<BeliefEvent>>()
            } else {
                vec![]
            };
            event_queue.append(&mut remove_events);
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

        // Always process NodeUpdate for network nodes to preserve their kind information, even when
        // used as scaffolding
        event_queue.push(BeliefEvent::NodeUpdate(
            keys.clone(),
            node.toml(),
            EventOrigin::Remote,
        ));

        let mut weight = Weight {
            payload: TomlTable::new(),
        };
        if let Some(path) = path_info {
            weight.set(crate::properties::WEIGHT_DOC_PATH, path).ok();
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
        event_queue.push(BeliefEvent::RelationInsert(
            bid,
            parent_bid,
            WeightKind::Section,
            weight.clone(),
            EventOrigin::Remote,
        ));
        self.stack.push((bid, proto.path.clone(), proto.heading));

        if node.kind.is_network() {
            // If the accumulator repo is nil, and this node is a network, and the
            // stack is empty, then initialize the accumulator repo to this element.
            // We don't do this operation in [BeliefAccumulator::new] because
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
                event_queue.push(BeliefEvent::RelationInsert(
                    bid,
                    self.api().bid,
                    WeightKind::Section,
                    api_weight,
                    EventOrigin::Remote,
                ));
            }
        }

        let current_keys =
            BTreeSet::from_iter(node.keys(Some(self.repo()), Some(parent_bid), self.set()));

        let unique_old = BTreeSet::from_iter(
            BTreeSet::from_iter(keys.into_iter())
                .difference(&current_keys)
                .cloned(),
        );
        // tracing::debug!(
        //     "push: final bid={}, parsed_bid={}, got_or_created_bid={}, kind={:?}, source={:?}",
        //     node.bid,
        //     parsed_bid,
        //     got_or_created_bid,
        //     node.kind,
        //     source
        // );
        Ok((bid, (source, current_keys, unique_old)))
    }

    #[allow(clippy::too_many_arguments)]
    async fn push_relation<B: BeliefCache + Clone>(
        &mut self,
        other_key: &NodeKey,
        kind: &WeightKind,
        maybe_weight: &Option<Weight>,
        owner_bid: &Bid,
        direction: Direction,
        index: usize,
        global_cache: B,
        update_queue: &mut Vec<BeliefEvent>,
        missing_structure: &mut Beliefs,
    ) -> Result<GetOrCreateResult, BuildonomyError> {
        // When is_source_owned=false (sink-owned/upstream_relations): owner is sink, other is source
        // When is_source_owned=true (source-owned/downstream_relations): owner is source, other is sink

        let other_key_regularized = other_key.regularize(&self.set, *owner_bid).expect(
            "parse_content Phase 1 parsing ensures that we have a valid subsection \
            structure to get paths from for all our parsed nodes",
        );
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
            .cache_fetch(&other_keys, global_cache.clone(), true, missing_structure)
            .await?;
        let (other_node, other_node_source) = match cache_fetch_result {
            GetOrCreateResult::Resolved(mut other_node, other_node_source) => {
                // Mark these nodes as traces -- we're not guaranteeing that we have all their
                // relationships loaded
                other_node.kind.insert(BeliefKind::Trace);
                (other_node, other_node_source)
            }
            GetOrCreateResult::Unresolved(ref unresolved_initial) => {
                // Special handling of external scheme links
                if let Some(href) = match other_key_regularized {
                    NodeKey::Id { net, id } => {
                        if net == href_namespace() {
                            Some(id)
                        } else {
                            None
                        }
                    }
                    _ => None,
                } {
                    // First reference to this http[s] schema link.
                    // First ensure we've installed the href network, do that if necessary.
                    if !self.set.states().contains_key(&href_namespace()) {
                        let href_net_node = BeliefNode::href_network();
                        update_queue.push(BeliefEvent::NodeUpdate(
                            href_net_node.keys(Some(buildonomy_namespace()), None, &self.set),
                            href_net_node.toml(),
                            EventOrigin::Remote,
                        ));
                        update_queue.push(BeliefEvent::RelationInsert(
                            href_namespace(),
                            buildonomy_namespace(),
                            WeightKind::Section,
                            Weight::default(),
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
                        href_node.keys(Some(href_namespace()), None, &self.set),
                        href_node.toml(),
                        EventOrigin::Remote,
                    ));
                    update_queue.push(BeliefEvent::RelationInsert(
                        href_node.bid,
                        href_namespace(),
                        WeightKind::Section,
                        weight.clone(),
                        EventOrigin::Remote,
                    ));
                    (href_node, NodeSource::Generated)
                } else {
                    let mut unresolved = unresolved_initial.clone();
                    unresolved.direction = direction;
                    unresolved.self_bid = *owner_bid;
                    let pmm_guard = self.set.paths();
                    let (owner_home_net, owner_home_path) =
                    pmm_guard.api_map().home_path(owner_bid, &pmm_guard).expect(
                        "parse_content Phase 1 parsing ensures that we have a valid subsection \
                        structure to get paths from for all our parsed nodes",
                    );
                    unresolved.self_net = owner_home_net;
                    unresolved.self_path = owner_home_path;
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
        // First enqueue the node to add it to self.set if it's not already there
        if other_node_source != NodeSource::SourceFile {
            // We want to delineate between parsed sources and linked content. If we're not from the
            // accumulator, color the other_node by Trace to ensure we can separate parsed content
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
        // Next, make sure its substructure is available in self.set
        match other_node_source {
            NodeSource::Merged => panic!(
                "We should only produced NodeSource::Merged from BeliefSetAccumulator::push!"
            ),
            NodeSource::GlobalCache => {
                // The missing_structure from cache_fetch has all the other node structure we
                // need. We will merge that into self.set within parse_content before processing the
                // event queue.
            }
            NodeSource::SourceFile | NodeSource::Generated => {
                // We've accumulated all the structure we need already, the event queue can be
                // processed without issue.
            }

            NodeSource::StackCache => {
                // There is no missing structure with respect to the stack cache, but we do need to
                // get missing structure from the stack cache to apply to self.set in order to
                // maintain a balanced beliefset.
                let query = Query {
                    seed: Expression::from(&NodeKey::Bid {
                        bid: other_node.bid,
                    }),
                    traverse: None,
                };
                let stack_result = self.stack_cache.eval_query(&query, true).await?;
                // tracing::debug!(
                //     "Returned stack_cache missing structure for query bid {}:\n{}",
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

        update_queue.push(BeliefEvent::RelationInsert(
            source_bid,
            sink_bid,
            *kind,
            weight,
            EventOrigin::Remote,
        ));
        Ok(GetOrCreateResult::Resolved(other_node, other_node_source))
    }

    async fn cache_fetch<B: BeliefCache + Clone>(
        &mut self,
        keys: &Vec<NodeKey>,
        global_cache: B,
        check_local: bool,
        missing_structure: &mut Beliefs,
    ) -> Result<GetOrCreateResult, BuildonomyError> {
        let mut found_state: Option<BeliefNode> = None;
        let mut source = NodeSource::Generated;
        for key in keys.iter() {
            if check_local {
                if let Some(existing_state) = self.set.get(key) {
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
            let stack_result = BeliefSet::from(self.stack_cache.eval_query(&query, true).await?);

            // tracing::debug!(
            //     "stack_result has {} nodes. stack_cache.is_balanced = {}",
            //     stack_result.states().len(),
            //     self.stack_cache.is_balanced(),
            // );

            match stack_result.get(key) {
                Some(existing_state) => {
                    found_state = Some(existing_state);
                    source = NodeSource::StackCache;
                    break;
                }
                None => {
                    let mut cache_update =
                        BeliefSet::from(global_cache.eval_query(&query, false).await?);

                    // tracing::debug!(
                    //     "global_cache result has {} nodes and {} relations. About to check its balance. Nodes:\n\t{}",
                    //     cache_update.states().len(),
                    //     cache_update.relations().as_graph().edge_count(),
                    //     cache_update.states().values().map(|n| format!("[{} - {}]", n.bid, n.title)).collect::<Vec<String>>().join("\n\t")
                    // );
                    if let Some(cached_state) = cache_update.get(key) {
                        found_state = Some(cached_state);
                        let update = cache_update.consume();
                        // Percolate global cache updates into closer caches.
                        missing_structure.union_mut(&update);
                        source = NodeSource::GlobalCache;
                        break;
                        // tracing::debug!("source: {:?}", source);
                    } else if !cache_update.states().is_empty() {
                        let pmm_guard = cache_update.paths();
                        tracing::warn!(
                            "Why didn't we get our node? The query returned results. \
                            our key: {:?}. query result paths:\n\t{}",
                            key,
                            pmm_guard
                                .api_map()
                                .all_paths(&pmm_guard, &mut BTreeSet::default())
                                .join("\n\t")
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
            tracing::debug!("Fetch miss! Keys: {:?}", keys);
            Ok(GetOrCreateResult::Unresolved(UnresolvedReference {
                other_keys: keys.clone(),
                ..Default::default()
            }))
        }
    }
}
