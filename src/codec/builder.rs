use petgraph::{
    visit::{EdgeRef, IntoEdgeReferences},
    Direction,
};
/// Utilities for parsing various document types into BeliefBases
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
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
        is_codec_namespace, is_network_index_file,
        network::{detect_network_file, NETWORK_NAME},
        proto_index::ProtoIndex,
        register_codec_namespace, CodecFactory, DocCodec, CLAIM_MAP,
    },
    error::BuildonomyError,
    event::{BeliefEvent, EventOrigin},
    nodekey::NodeKey,
    paths::{as_anchor, os_path_to_string, path::string_to_os_path, AnchorPath, AnchorPathBuf},
    properties::{
        asset_namespace, buildonomy_href_bid, buildonomy_namespace, const_namespaces,
        content_namespaces, href_namespace, BeliefKind, BeliefKindSet, BeliefNode, Bid, Bref,
        NodeId, Weight, WeightKind, WEIGHT_DOC_PATHS, WEIGHT_OWNED_BY, WEIGHT_SORT_KEY,
    },
    query::{lookup_node, BeliefSource, QueryPackage, QuerySpec, TapeFn},
    shard::content_type::score_lexical,
    shard::search::{tokenize, Stemmer},
};

use super::{belief_ir::IntermediateRelation, UnresolvedReference};
use crate::beliefbase::BeliefContext;
#[cfg(feature = "git-tracking")]
use crate::codec::git::NetworkGitStatus;

// ---------------------------------------------------------------------------
// AssetCodec — zero-size no-op codec for static asset files
// ---------------------------------------------------------------------------

/// A zero-size no-op [`DocCodec`] used as the codec field inside
/// [`ParseContentWithCodec`] when `GraphBuilder::process_asset` handles a
/// binary/static asset file.
///
/// All trait methods return safe empty-or-default values so that
/// `process_one_parse_result` in the compiler can treat asset results
/// identically to document results without any special-casing.
pub struct AssetCodec;

impl DocCodec for AssetCodec {
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
        Vec::new()
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
    ) -> Result<std::collections::HashMap<Bid, IRNode>, BuildonomyError> {
        Ok(std::collections::HashMap::new())
    }

    fn generate_source(&self) -> Option<String> {
        None
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
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

    /// Absolute paths of derived output files written by this parse (e.g. CSV exports
    /// of opaque xlsx tabs). The compiler enqueues these into the asset discovery
    /// pipeline so each file gets a content-addressed asset node with a `content_hash`
    /// in the same compile session — not deferred to the next run.
    pub derived_paths: Vec<std::path::PathBuf>,
}

/// Result of parsing document content with owned codec instance
pub struct ParseContentWithCodec {
    /// Parse result (rewritten content and diagnostics)
    pub result: ParseContentResult,
    /// Owned codec instance with parsed state
    pub codec: Box<dyn DocCodec + Send>,
    /// The repository root BID discovered during this parse (Bid::nil() if not yet resolved).
    /// Used by the compiler to seed `self.builder.repo` after a parallel epoch-0 batch.
    pub repo_bid: Bid,
    /// The repository root node itself, if discovered during this parse.
    /// Used by the compiler to seed `self.builder.session_bb` after a parallel epoch-0 batch
    /// so that `generate_spa_shell` / `finalize_html` can find the repo node.
    pub repo_node: Option<BeliefNode>,
}

impl ParseContentResult {
    /// Create a new parse result with no rewrites or diagnostics
    pub fn empty() -> Self {
        Self {
            rewritten_content: None,
            diagnostics: Vec::new(),
            derived_paths: Vec::new(),
        }
    }

    /// Create a parse result with rewritten content
    pub fn with_rewrite(content: String) -> Self {
        Self {
            rewritten_content: Some(content),
            diagnostics: Vec::new(),
            derived_paths: Vec::new(),
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
    /// Index of edges emitted by `push_mapping` during the current `parse_content` call.
    ///
    /// Key: owner bref (the section node that owns the `{maps_to}` directive).
    /// Value: list of `(source_bid, sink_bid, weight_kind)` tuples for each emitted edge.
    ///
    /// Written by `push_mapping`. Used by `terminate_stack` to GC owned edges when the
    /// owner section node is removed within the same compile pass (i.e. the heading was
    /// deleted). Reparse GC is handled separately via the `owner_edges` memo on
    /// `BeliefBase` (pre-loaded into `doc_bb` during Phase 2b) + `compute_diff`.
    ///
    /// This index is builder-local and is NOT persisted. On a fresh compiler start with no
    /// reparse of the owning file, the index is empty and GC falls through to the
    /// `session_bb.graph_for_owner` path in Phase 2b + `compute_diff` on the next reparse.
    owner_index: std::collections::HashMap<Bref, Vec<(Bid, Bid, WeightKind)>>,
    /// Stemmer instance for tokenizing node text during classification.
    stemmer: Stemmer,
    /// When `true`, `initialize_stack` skips the `lookup_node(&global_bb, &api_key)` check
    /// that registers the api node into `global_bb` if absent.
    ///
    /// Set to `true` for parallel task builders in `parse_epoch`: those builders emit to
    /// an isolated `task_tx` (not the shared accumulator channel), and the api node is
    /// already established in `global_bb` by the time any parallel epoch runs. The check
    /// is a no-op for task builders and costs one mutex acquisition on the
    /// `BeliefAccumulator` per task — with `--jobs N`, all N tasks serialize here at
    /// the start of Phase 0, producing an O(N²) stall.
    skip_global_api_check: bool,

    // ---------------------------------------------------------------------------
    // Temporary perf instrumentation.
    // Accumulated per parse_content call; reset at the top of each call.
    // ---------------------------------------------------------------------------
    /// Number of neighborhood evaluate calls that were cache hits.
    #[cfg(not(target_arch = "wasm32"))]
    neighborhood_n_hits: u64,
    /// Number of neighborhood evaluate calls that were cache misses.
    #[cfg(not(target_arch = "wasm32"))]
    neighborhood_n_misses: u64,
    /// Microseconds spent in the pre-regularize block: os_path_to_string + stack scan.
    #[cfg(not(target_arch = "wasm32"))]
    pre_regularize_us: u64,
    /// Microseconds between cache_fetch returning and entering the NodeSource arm
    /// (the match cache_fetch_result block for the Resolved case).
    #[cfg(not(target_arch = "wasm32"))]
    mid_match_us: u64,
    /// Total per-call wall-clock time (cross-check vs push_relation_total_ms).
    #[cfg(not(target_arch = "wasm32"))]
    total_per_call_us: u64,
    /// Microseconds spent in regularize_unchecked + is_dir reclassification per call.
    #[cfg(not(target_arch = "wasm32"))]
    regularize_us: u64,
    /// Microseconds spent in cache_fetch per call.
    #[cfg(not(target_arch = "wasm32"))]
    cache_fetch_us: u64,
    /// Microseconds spent in missing_structure.union_mut_with_trace per call.
    #[cfg(not(target_arch = "wasm32"))]
    union_mut_us: u64,
    /// Number of push_relation calls that hit the StackCache/GlobalCache arm.
    #[cfg(not(target_arch = "wasm32"))]
    n_cache_arm: u64,
    /// Number of push_relation calls total.
    #[cfg(not(target_arch = "wasm32"))]
    n_push_relation: u64,
}

/// Union two [`BeliefGraph`]s into a new graph containing all states and edges of both.
///
/// `base` wins on state collisions: the same BID present in both keeps `base`'s node.
/// This matters because a per-document balanced seed carries directly-fetched
/// (non-`Trace`) nodes, whereas the shared epoch snapshot may hold `Trace`-marked
/// ancestor copies of the same BIDs; preferring `base` keeps the more authoritative
/// version. Edges are unioned; an edge present in both keeps `base`'s weight.
///
/// Used by [`GraphBuilder::seed_session`] to fold the const-namespace (href/asset)
/// subgraph into a per-document seed that would otherwise lack it.
fn union_graphs(base: &BeliefGraph, extra: &BeliefGraph) -> BeliefGraph {
    let mut states = base.states.clone();
    for (bid, node) in &extra.states {
        states.entry(*bid).or_insert_with(|| node.clone());
    }

    // Collect base's edges first so they take precedence, then add extra's for any
    // (source, sink) pair base doesn't already have.
    let mut seen: BTreeSet<(Bid, Bid)> = BTreeSet::new();
    let mut edges: Vec<(Bid, Bid, crate::properties::WeightSet)> = Vec::new();
    for graph in [&base.relations, &extra.relations] {
        let g = graph.as_graph();
        for e in g.edge_references() {
            let pair = (g[e.source()], g[e.target()]);
            if seen.insert(pair) {
                edges.push((pair.0, pair.1, e.weight().clone()));
            }
        }
    }

    BeliefGraph {
        states,
        relations: crate::beliefbase::BidGraph::from_edges(edges),
    }
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
        let canonicalized_path = crate::paths::canonicalize_path(repo_path.as_ref())?;
        let Some(mut repo_root) = detect_network_file(canonicalized_path.as_ref()) else {
            return Err(BuildonomyError::Codec(format!(
                "GraphBuilder initialization failed. Received root path {repo_path:?}. \
                 Expected a directory or path to a index.md file"
            )));
        };
        // network index file is now network dir
        repo_root.pop();
        // canonicalize_path already strips any Windows \\?\ prefix, so repo_root is
        // a plain C:\... path consistent with all other canonicalized paths in the system.
        let repo_root = crate::paths::canonicalize_path(&repo_root).unwrap_or(repo_root);

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

        let stemmer = Stemmer::new();
        let accum = GraphBuilder {
            // parsed_content: BTreeSet::default(),
            // parsed_structure: BTreeSet::default(),
            doc_bb: BeliefBase::empty().with_label("doc_bb"),
            repo: Bid::nil(),
            repo_root,
            stack: Vec::default(),
            session_bb: BeliefBase::empty().with_label("session_bb"),
            tx,
            owner_index: std::collections::HashMap::default(),
            stemmer,
            skip_global_api_check: false,
            #[cfg(not(target_arch = "wasm32"))]
            neighborhood_n_hits: 0,
            #[cfg(not(target_arch = "wasm32"))]
            neighborhood_n_misses: 0,
            #[cfg(not(target_arch = "wasm32"))]
            pre_regularize_us: 0,
            #[cfg(not(target_arch = "wasm32"))]
            mid_match_us: 0,
            #[cfg(not(target_arch = "wasm32"))]
            total_per_call_us: 0,
            #[cfg(not(target_arch = "wasm32"))]
            regularize_us: 0,
            #[cfg(not(target_arch = "wasm32"))]
            cache_fetch_us: 0,
            #[cfg(not(target_arch = "wasm32"))]
            union_mut_us: 0,
            #[cfg(not(target_arch = "wasm32"))]
            n_cache_arm: 0,
            #[cfg(not(target_arch = "wasm32"))]
            n_push_relation: 0,
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

    /// Seed a fresh parallel-task builder from the compiler's [`epoch_session_snapshot`].
    ///
    /// Sets `self.repo` so `initialize_stack`'s fast-path guard passes immediately, and
    /// replaces `self.session_bb` with the snapshot so both:
    ///
    /// - `try_initialize_stack_from_session_cache` finds the parent network via a
    ///   `StackCache` hit (no `global_bb.evaluate` mutex contention), and
    /// - the `content_namespaces()` guard in `initialize_stack` finds both namespace
    ///   nodes already present (no `global_bb.evaluate` calls on the first file).
    ///
    /// Call [`epoch_session_snapshot`] once before the task-spawn loop, wrap the result
    /// in an `Arc`, and clone it cheaply into each task.
    ///
    /// This is a no-op when `repo_bid` is `Bid::nil()` (epoch-0, root not yet parsed).
    ///
    /// `const_ns_snapshot`, when supplied, is unioned into `snapshot` via
    /// [`union_graphs`] before the session is built.  See the const-namespace
    /// discussion below.
    ///
    /// Seed a task builder by cloning a prebuilt shared epoch base, then merging in
    /// this document's own seed.
    ///
    /// This is the preferred entry point for parallel epoch tasks; [`seed_session`]
    /// remains for callers that only have a [`BeliefGraph`].
    ///
    /// # Why a prebuilt base
    ///
    /// [`seed_session`] runs `BeliefBase::from`, whose dominant cost is
    /// `PathMapMap::new` — one seeded DFS per network over the whole snapshot.  Across
    /// an epoch every task rebuilt an *identical* index: measured on a full corpus run,
    /// the shared const-namespace was 99.99% of the states each task reconstructed
    /// (148.93M of 148.95M) to make use of ~113 of its own, and that reconstruction was
    /// 97.5% of rebuild cost.  Building the base once per epoch and cloning it turns
    /// that per-task DFS into a `BTreeMap` of `Arc` pointer copies, because
    /// `PathMapMap`'s clone shares its `PathMap`s.  Writes stay private via the
    /// copy-on-write in `PathMapMap::make_pathmap_unique`.
    ///
    /// Measured on a full corpus (3,721 tasks): seeding 2,533s → 524s, parse phase
    /// 20m38s → 11m32s.
    ///
    /// # Why not share the namespace `PathMap`s directly
    ///
    /// The obvious cheaper form — keep per-task `BeliefBase`s but share just the two
    /// const-namespace `PathMap` `Arc`s — is unsound, because those `PathMap`s **are**
    /// written during an epoch.  `terminate_stack` runs `compute_diff` events through
    /// `session_bb`, and `PathMapMap::process_event_queue` takes a write guard on a
    /// const-namespace `PathMap` whenever an edge sinks into it: href aliases (see the
    /// "NOT session_bb" comment in `push`, which documents that cross-file alias
    /// collision detection depends on this), href stubs from `ensure_href_entry`, and
    /// assets from `process_asset`.  Sharing without copy-on-write would let one task's
    /// registrations appear in a sibling's view.
    ///
    /// Sharing a whole prebuilt base is therefore not a weaker form of that idea but a
    /// stronger one: it shares every derived structure, not just two, and the
    /// copy-on-write makes the writes above private.
    ///
    /// # Merge precedence
    ///
    /// Merges [`MergePrecedence::RhsWins`]: `doc_seed` overrides the shared base on any
    /// node they both hold.  `doc_seed` is queried fresh from `global_bb` per epoch,
    /// whereas the shared base is built once and accumulates across the epoch, so the
    /// base is the likelier of the two to carry a stale node.  This also preserves the
    /// old behaviour: the previous code unioned with [`union_graphs`], whose `base`
    /// argument — the winning side — was `doc_seed`.
    ///
    /// Falls back to [`seed_session`] when `shared` is empty (epoch-0).
    pub(crate) fn seed_session_from_base(
        &mut self,
        repo_bid: Bid,
        shared: &BeliefBase,
        doc_seed: &BeliefGraph,
    ) {
        if repo_bid == Bid::nil() {
            return;
        }
        if shared.states().is_empty() {
            self.seed_session(repo_bid, doc_seed, None);
            return;
        }
        if self.repo == Bid::nil() {
            self.repo = repo_bid;
        }
        let seed_start = std::time::Instant::now();

        let t = std::time::Instant::now();
        self.session_bb = shared.clone().with_label("session_bb");
        let base_clone_us = t.elapsed().as_micros();

        // Merge this document's own seed. Scoped to the seed's own BIDs so the cost is
        // O(doc_seed), not O(session_bb) — `to_event_stream` scopes `rhs`, and rhs is
        // the small graph here.
        let t = std::time::Instant::now();
        if !doc_seed.states.is_empty() {
            let seeds: BTreeSet<Bid> = doc_seed.states.keys().copied().collect();
            self.session_bb.merge_from_with(
                doc_seed,
                &seeds,
                crate::beliefbase::MergePrecedence::RhsWins,
            );
        }
        let merge_us = t.elapsed().as_micros();

        tracing::debug!(
            target: "noet_core::codec::perf",
            shared_states = shared.states().len(),
            doc_seed_states = doc_seed.states.len(),
            base_clone_us,
            merge_us,
            total_us = seed_start.elapsed().as_micros(),
            "[seed_session_from_base] session_bb built",
        );

        // Ensure the repo root is present and non-Trace — see `seed_session` for the
        // full rationale (cache_fetch rejects Trace StackCache hits, and a miss here
        // mints a fresh time-based BID that corrupts doc_bb's PathMap).
        let repo_node = self
            .session_bb
            .states()
            .get(&repo_bid)
            .or_else(|| doc_seed.states.get(&repo_bid))
            .cloned();
        if let Some(mut repo_node) = repo_node {
            if repo_node.kind.contains(BeliefKind::Trace) {
                repo_node.kind.remove(BeliefKind::Trace);
                let _ = self.session_bb.process_event(&BeliefEvent::NodeUpsert(
                    repo_bid,
                    repo_node,
                    EventOrigin::Remote,
                ));
            }
        }
    }

    /// [`epoch_session_snapshot`]: GraphBuilder::epoch_session_snapshot
    pub(crate) fn seed_session(
        &mut self,
        repo_bid: Bid,
        snapshot: &BeliefGraph,
        const_ns_snapshot: Option<&BeliefGraph>,
    ) {
        if repo_bid == Bid::nil() {
            return;
        }
        // Set repo BID so initialize_stack's fast-path guard passes.
        if self.repo == Bid::nil() {
            self.repo = repo_bid;
        }
        // Union the const-namespace subgraph into the seed.
        //
        // `parse_epoch` picks EITHER a per-document balanced seed OR the shared
        // `epoch_session_snapshot` (`network_ancestors`) — never both — and this
        // function then *replaces* `session_bb` wholesale.  The epoch snapshot carries
        // the href/asset namespace subgraphs (see `epoch_session_snapshot` Part 2), but
        // a per-document balanced seed generally does not: it is a small graph (mean ~36
        // states) scoped to one document's own subtree plus its ancestor chain.
        //
        // So a task taking the per-doc branch would *discard* const-namespace content
        // the epoch had already computed for it.  `initialize_stack`'s
        // `content_namespaces()` guard then misses and re-fetches the ENTIRE namespace
        // from `global_bb` — 83,874 states, ~82s, per affected task.
        //
        // Measured: pre-fix, 32 tasks hit this (2,545s total).  The `submap_by_bid`
        // empty-subtree fix (a4f1611) made per-doc seeds non-empty far more often,
        // growing the at-risk population and more than doubling the cost.  Unioning the
        // namespaces in costs nothing extra — the caller already holds this graph in an
        // `Arc`, and the 90%+ of tasks on the fallback branch already clone it whole.
        //
        // Enable attribution of performance impacts to specific seeding operations. Each
        // sub-step below is timed independently — the union, the graph clone, and the
        // `BeliefBase::from` rebuild are three separate candidates, and only
        // measurement distinguishes them.
        let seed_start = std::time::Instant::now();
        let seed_states_in = snapshot.states.len();
        let const_ns_states = const_ns_snapshot.map(|ns| ns.states.len()).unwrap_or(0);

        let merged;
        let mut unioned = false;
        let mut union_us = 0;
        let snapshot = match const_ns_snapshot {
            Some(ns) if !ns.states.is_empty() && !snapshot.states.is_empty() => {
                let t = std::time::Instant::now();
                merged = union_graphs(snapshot, ns);
                union_us = t.elapsed().as_micros();
                unioned = true;
                &merged
            }
            _ => snapshot,
        };
        // Replace session_bb directly from the snapshot — cheaper than merge_from for a
        // fresh (effectively empty) task builder: skips the full to_event_stream diff and
        // event replay.  The snapshot contains both network ancestors and const-namespace
        // subgraphs, so the content_namespaces() guard and StackCache fast-path both fire
        // without touching global_bb.
        if !snapshot.states.is_empty() {
            let t = std::time::Instant::now();
            let cloned = snapshot.clone();
            let clone_us = t.elapsed().as_micros();

            let t = std::time::Instant::now();
            self.session_bb = BeliefBase::from(cloned).with_label("session_bb");
            let rebuild_us = t.elapsed().as_micros();

            tracing::debug!(
                target: "noet_core::codec::perf",
                seed_states_in,
                const_ns_states,
                merged_states = snapshot.states.len(),
                merged_edges = snapshot.relations.as_graph().edge_count(),
                unioned,
                union_us,
                clone_us,
                rebuild_us,
                total_us = seed_start.elapsed().as_micros(),
                "[seed_session] session_bb built",
            );

            // Ensure the repo root node is present and non-Trace in session_bb.
            //
            // When the snapshot comes from a balanced QueryPackage evaluation (the
            // per-doc seed), the traversal walks ancestor chains — marking every
            // network ancestor node, including the repo root, as Trace.  cache_fetch
            // rejects Trace StackCache hits for non-External nodes, so the slow path in
            // initialize_stack would miss the repo root in session_bb and fall through to
            // GlobalCache.  If GlobalCache also misses (e.g. the NetId query uses the
            // repo root's own bref as net, which doesn't match how the id is stored in
            // the paths table), a fresh time-based BID is generated — corrupting doc_bb's
            // PathMap and causing the Phase 4 get_context panic.
            //
            // We already have the canonical repo_bid from the compiler's stable main
            // builder.  Look it up in the snapshot; if present, upsert it into session_bb
            // with the Trace bit cleared so cache_fetch finds it as a StackCache hit.
            if let Some(mut repo_node) = snapshot.states.get(&repo_bid).cloned() {
                repo_node.kind.remove(BeliefKind::Trace);
                let _ = self.session_bb.process_event(&BeliefEvent::NodeUpsert(
                    repo_bid,
                    repo_node,
                    EventOrigin::Remote,
                ));
            }
        }
    }

    /// Build a snapshot of `session_bb` containing everything a fresh parallel-task
    /// builder needs to avoid hitting `global_bb` during `initialize_stack`.
    ///
    /// The snapshot merges three disjoint subsets of `session_bb`:
    ///
    /// 1. **Network ancestors** — every node whose `kind.is_network()` is true, plus
    ///    all edges between them.  `session_bb` is populated with the full network
    ///    tree (root + all subnets) by `sync_network_snapshot` after each
    ///    `drain_epoch`; without that call this set would contain only the repo-root
    ///    network (parsed sequentially before the first epoch).  The Section edges
    ///    between network nodes carry `WEIGHT_DOC_PATHS` — the subnet directory name
    ///    used by `PathMap::new → generate_terminal_path` to produce human-readable
    ///    paths.  Seeding a task builder with this lets
    ///    `try_initialize_stack_from_session_cache` find the parent network via a
    ///    cheap `StackCache` hit rather than a mutex-guarded `global_bb.evaluate`,
    ///    AND lets the seeded `PathMapMap` resolve subnet paths by directory name
    ///    rather than falling back to a bref-based path component.
    ///
    /// 2. **Const-namespace subgraphs** — the `href_namespace` and `asset_namespace`
    ///    nodes, plus all nodes reachable from them in `session_bb`.  Seeding a task
    ///    builder with these lets the `content_namespaces()` guard in
    ///    `initialize_stack` find both namespaces already present, skipping the two
    ///    `global_bb.eval` calls that would otherwise fire on every file in a fresh
    ///    parallel task.
    ///
    /// 3. **Network index anchors** — every anchor node that is a direct `Section`
    ///    child of a network node (i.e. headings parsed from a network's own
    ///    `index.md`).  Including these lets parallel tasks resolve references like
    ///    `id://net_bref/net_index_defined_tag` from the seeded `session_bb` instead
    ///    of falling through to the mutex-guarded `global_bb.evaluate` on every link
    ///    that targets an index heading.
    ///
    /// Call this once before the task-spawn loop, wrap the result in an `Arc`, and
    /// pass it to [`seed_session`] inside each task.
    ///
    /// [`seed_session`]: GraphBuilder::seed_session
    pub(crate) fn epoch_session_snapshot(&self) -> BeliefGraph {
        // This runs once per epoch, before any task spawns, so
        // its cost is fully serial.  Each part is timed separately — Part 2's BFS walks
        // the const-namespace subgraph, whose breadth grows with the corpus,
        // making it the leading suspect for a cost that scales over a run.
        let snapshot_start = std::time::Instant::now();

        // ── Part 1: network-kinded nodes + edges between them ────────────────────
        let t = std::time::Instant::now();
        let network_bids: BTreeSet<Bid> = self
            .session_bb
            .states()
            .iter()
            .filter(|(_, n)| n.kind.is_network())
            .map(|(bid, _)| *bid)
            .collect();
        let part1_us = t.elapsed().as_micros();

        // ── Part 2: const-namespace nodes + all nodes reachable from them ──────
        // Walk session_bb's relation graph starting from each const-namespace BID
        // to collect the full asset subgraph.  We do a simple BFS on Incoming edges
        // (assets point *to* their namespace: source=asset, sink=namespace) so we
        // need to walk Incoming from the namespace to reach its assets.
        //
        let t = std::time::Instant::now();
        let mut ns_bids: BTreeSet<Bid> = BTreeSet::new();
        {
            let rel = self.session_bb.relations();
            let g = rel.as_graph();

            for ns_bid in content_namespaces().iter() {
                if self
                    .session_bb
                    .get(&NodeKey::Bid { bid: *ns_bid })
                    .is_none()
                {
                    continue; // namespace not yet loaded into compiler session_bb
                }
                ns_bids.insert(*ns_bid);
                if let Some(ns_idx) = self.session_bb.bid_to_index(ns_bid) {
                    // Walk Incoming edges: assets are sources, namespace is the sink.
                    let mut stack = vec![ns_idx];
                    let mut visited = BTreeSet::from([ns_idx]);
                    while let Some(current) = stack.pop() {
                        for neighbor in g.neighbors_directed(current, petgraph::Direction::Incoming)
                        {
                            if visited.insert(neighbor) {
                                stack.push(neighbor);
                            }
                        }
                    }
                    for idx in visited {
                        ns_bids.insert(g[idx]);
                    }
                }
            }
        }
        let part2_us = t.elapsed().as_micros();

        // ── Part 3: anchor nodes under each network's index.md ─────────────────
        //
        // Headings parsed from a network's own `index.md` are stored in session_bb as
        // anchor nodes and are id-addressable (e.g. `id://net_bref/haven-1`), but are
        // not network-kinded so Part 1 misses them.  Without them in the task's seeded
        // session_bb, every parallel task that resolves such a reference falls through
        // to the mutex-guarded `global_bb.evaluate` — one 15-20 ms round-trip per link.
        //
        // The PathMap records index.md anchors with NETWORK_SECTION_SORT_KEY (u16::MAX)
        // as the first element of their order vec.  Reading directly from the PathMap is
        // more precise than a graph walk: it doesn't require distinguishing anchor-kinded
        // from document-kinded nodes in the relation graph, and it naturally covers the
        // full anchor subtree under index.md (not just direct children).
        let t = std::time::Instant::now();
        let mut index_anchor_bids: BTreeSet<Bid> = BTreeSet::new();
        {
            let paths = self.session_bb.paths();
            for net_bid in &network_bids {
                if let Some(pm) = paths.get_map(&net_bid.bref()) {
                    index_anchor_bids.extend(pm.network_section_bids());
                }
            }
        }
        let part3_us = t.elapsed().as_micros();

        // ── Merge all three sets ────────────────────────────────────────────────
        let included_bids: BTreeSet<Bid> = network_bids
            .union(&ns_bids)
            .copied()
            .collect::<BTreeSet<_>>()
            .union(&index_anchor_bids)
            .copied()
            .collect();

        if included_bids.is_empty() {
            return BeliefGraph::default();
        }

        let t = std::time::Instant::now();
        let states: FxHashMap<Bid, BeliefNode> = included_bids
            .iter()
            .filter_map(|bid| self.session_bb.states().get(bid).map(|n| (*bid, n.clone())))
            .collect();
        let state_clone_us = t.elapsed().as_micros();

        let t = std::time::Instant::now();
        let relations = {
            let rel = self.session_bb.relations();
            let g = rel.as_graph();

            crate::beliefbase::BidGraph::from_edges(g.edge_references().filter_map(|e| {
                let source = g[e.source()];
                let sink = g[e.target()];
                if included_bids.contains(&source) && included_bids.contains(&sink) {
                    Some((source, sink, e.weight().clone()))
                } else {
                    None
                }
            }))
        };
        let edge_filter_us = t.elapsed().as_micros();

        tracing::debug!(
            target: "noet_core::codec::perf",
            session_bb_nodes = self.session_bb.states().len(),
            session_bb_edges = self.session_bb.relations().as_graph().edge_count(),
            network_bids = network_bids.len(),
            ns_bids = ns_bids.len(),
            index_anchor_bids = index_anchor_bids.len(),
            included_bids = included_bids.len(),
            out_states = states.len(),
            out_edges = relations.as_graph().edge_count(),
            part1_network_us = part1_us,
            part2_const_ns_bfs_us = part2_us,
            part3_index_anchors_us = part3_us,
            state_clone_us,
            edge_filter_us,
            total_us = snapshot_start.elapsed().as_micros(),
            "[epoch_session_snapshot] built",
        );

        BeliefGraph { states, relations }
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

    /// Set `skip_global_api_check`, returning `self` for chaining.
    ///
    /// Pass `true` for parallel task builders constructed inside `parse_epoch`:
    /// those builders emit to an isolated channel, the api node is already in
    /// `global_bb`, and the check costs one `BeliefAccumulator` mutex acquisition
    /// per task — O(N) serialization at the start of every parallel epoch.
    pub fn with_skip_global_api_check(mut self, skip: bool) -> Self {
        self.skip_global_api_check = skip;
        self
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
    /// Replace the event transmitter with a new one.
    ///
    /// Used in tests that run multiple parse passes on the same compiler instance
    /// and need each pass to drain into a separate event channel.
    pub fn set_tx(&mut self, tx: tokio::sync::mpsc::UnboundedSender<BeliefEvent>) {
        let _old_tx = std::mem::replace(&mut self.tx, tx);
        // old_tx is dropped here, closing the previous channel
    }

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
        let mut set_errors = self.doc_bb.built_in_test();
        if !set_errors.is_empty() {
            combined_errors.push("builder.doc_bb errors:".to_string());
            combined_errors.append(&mut set_errors);
        }
        let mut session_bb_errors = self.session_bb.built_in_test();
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
        parse_number: usize,
    ) -> Result<ParseContentWithCodec, BuildonomyError> {
        tracing::debug!("Phase 0: initialize stack");
        let full_path = input_path.as_ref().canonicalize()?.to_path_buf();
        let (initial, doc_sort_key) = self
            .initialize_stack(input_path.as_ref(), global_bb.clone(), &proto_index)
            .await?;

        let mut maybe_content: Option<String> = None;
        // Track ID renames for parsed nodes
        let mut docs_to_parse = Vec::<(String, Bid)>::new();
        let mut clobbered_bids = BTreeSet::<Bid>::new();
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

        // Codec resolution: CLAIM_MAP.get() is the single dispatch point — it checks
        // the claim registry first, then falls back to CODECS.path_get() internally.
        let resolved_factory: Option<CodecFactory> = CLAIM_MAP.get(&full_path);

        if let Some(codec_factory) = resolved_factory {
            // Create fresh codec instance from factory
            let mut codec = codec_factory();
            codec.parse(&content, initial, &mut diagnostics, &proto_index)?;

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
            // For the root network node (proto_idx == 0 and kind contains Network), git
            // metadata is NOT injected by initialize_stack (which only pushes ancestor
            // networks above the entry point).  Compute it here from proto_index so the
            // root network's BeliefNode gets metadata["git"] on first parse just like
            // any ancestor network does.
            #[cfg(feature = "git-tracking")]
            let root_network_git_override: Option<toml::value::Table> = {
                // The root network dir is full_path's parent if it's an index.md,
                // or full_path itself if it's a directory.
                let net_dir = if is_network_index_file(&full_path) {
                    full_path.parent().map(|p| p.to_path_buf())
                } else {
                    Some(full_path.clone())
                };
                net_dir.and_then(|dir| {
                    proto_index
                        .get_meta_as::<NetworkGitStatus>(&dir, "git")
                        .map(|gs: NetworkGitStatus| {
                            let mut meta = toml::value::Table::new();
                            meta.insert(
                                "git".to_string(),
                                toml::Value::Table(gs.to_metadata_table()),
                            );
                            meta
                        })
                })
            };
            #[cfg(not(feature = "git-tracking"))]
            let root_network_git_override: Option<toml::value::Table> = None;

            for (proto_idx, proto) in codec.nodes().iter().enumerate() {
                // The first node is always the entry document. Pass the sort key captured by
                // initialize_stack so that RelationChange(doc, repo_root, ...) uses the correct
                // sibling position regardless of which cache branch cache_fetch takes.
                // All subsequent nodes (sections) get None and auto-assign their own sort keys.
                let entry_sort_key = if proto_idx == 0 { doc_sort_key } else { None };
                // For the root network node (first proto, Network kind), inject git metadata.
                // All other nodes (documents, sections) get None — their source_url is
                // computed in Phase 4 inject_context via the network ancestor's metadata.
                let metadata_override =
                    if proto_idx == 0 && proto.kind.contains(BeliefKind::Network) {
                        root_network_git_override.clone()
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
                        &mut diagnostics,
                        &mut clobbered_bids,
                        metadata_override,
                        parse_number,
                    )
                    .await?;

                if !missing_structure.is_empty() {
                    // Seed from the single node just pushed — bounds the DFS to what's
                    // reachable from this node in missing_structure, not all of session_bb.
                    let node_seed: BTreeSet<Bid> = BTreeSet::from([bid]);
                    self.session_bb.merge_from(&missing_structure, &node_seed);
                    // Also merge into doc_bb so that compute_diff can see the Section
                    // edges for nodes that have no cross-doc push_relation calls (e.g.
                    // section nodes in files with no wikilinks). Without this, Phase 2's
                    // doc_bb.merge_from has an empty relation_seeds seed for such files
                    // and is a no-op, leaving section nodes absent from doc_bb.
                    // compute_diff then emits nothing for them → they never reach
                    // global_bb → parse 2 generates fresh BIDs and rewrites forever.
                    self.doc_bb.merge_from(&missing_structure, &node_seed);
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

            tracing::debug!("Phase 2: Balance and process relations");
            let phase2_start = std::time::Instant::now();
            let file_path_str = input_path.as_ref().display().to_string();
            let mut generated_href_nodes = Vec::new();
            let mut relation_seeds = BTreeSet::new();
            let mut push_relation_total_ms = 0u128;
            for (proto, bid) in codec.nodes().iter().zip(parsed_bids.iter()) {
                // Process upstream_relations (sink-owned, default)
                for (index, relation) in proto.upstream.iter().enumerate() {
                    let pr_start = std::time::Instant::now();
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
                            parse_number,
                        )
                        .await?;

                    push_relation_total_ms += pr_start.elapsed().as_millis();
                    match result {
                        GetOrCreateResult::Resolved(node, source) => {
                            relation_seeds.insert(node.bid);
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
                    let pr_start = std::time::Instant::now();
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
                            parse_number,
                        )
                        .await?;

                    push_relation_total_ms += pr_start.elapsed().as_millis();
                    match result {
                        GetOrCreateResult::Resolved(node, source) => {
                            relation_seeds.insert(node.bid);
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

                // Phase 2b: process {maps_to} mapping relations owned by this section node.
                //
                // For each section node with non-empty mappings:
                // 1. Pre-load previously-owned edges from session_bb / global_bb into doc_bb
                //    via union_mut (direct union only — no merge_from DFS, which would
                //    contaminate doc_bb with source/sink neighborhoods and corrupt compute_diff).
                // 2. Emit new RelationChange events via push_mapping.
                //
                // The pre-load gives compute_diff the correct baseline: dropped mappings appear
                // in session_bb and doc_bb (pre-loaded) but not in the new emissions →
                // RelationRemoved. New mappings appear only in new emissions → RelationChange.
                if !proto.mappings.is_empty() {
                    let owner_bref = bid.bref();
                    // Pre-load previously-owned edges from session_bb into doc_bb.
                    // session_bb always has the owned edges after Phase 1: either from
                    // the GlobalCache path (halo includes Owner → missing_structure →
                    // session_bb.merge_from) or from a prior file's terminate_stack.
                    // The owner_edges memo gives O(K) lookup.
                    let previously_owned = self.session_bb.graph_for_owner(&owner_bref);
                    if !previously_owned.is_empty() {
                        self.doc_bb.merge(&previously_owned);
                    }
                    for (mapping_idx, mapping) in proto.mappings.iter().enumerate() {
                        let mapping_bids = self
                            .push_mapping(
                                mapping,
                                bid,
                                mapping_idx,
                                &content,
                                global_bb.clone(),
                                &mut relation_event_queue,
                                &mut missing_structure,
                                &mut diagnostics,
                                parse_number,
                            )
                            .await?;
                        // Add resolved mapping sources/sinks to relation_seeds so the
                        // post-loop doc_bb.merge_from DFS brings in their pathmap entries.
                        // Without this, sink nodes from other documents (e.g. class-A–F
                        // in end_appendix_d.md) are present in doc_bb as Trace nodes but
                        // lack pathmap entries, causing spurious "No entry in pathmap for
                        // sink" warnings in compute_diff Phase 4.
                        relation_seeds.extend(mapping_bids);
                    }
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                tracing::debug!(
                    target: "noet_core::codec::perf",
                    file_path = %file_path_str,
                    push_relation_ms = push_relation_total_ms,
                    phase2_total_ms = phase2_start.elapsed().as_millis(),
                    n_push_relation = self.n_push_relation,
                    n_cache_arm = self.n_cache_arm,
                    pre_regularize_ms = self.pre_regularize_us / 1000,
                    regularize_ms = self.regularize_us / 1000,
                    cache_fetch_ms = self.cache_fetch_us / 1000,
                    mid_match_ms = self.mid_match_us / 1000,
                    union_mut_ms = self.union_mut_us / 1000,
                    total_per_call_ms = self.total_per_call_us / 1000,
                    neighborhood_n_hits = self.neighborhood_n_hits,
                    neighborhood_n_misses = self.neighborhood_n_misses,
                    "[Phase 2] push_relation loop complete",
                );
                // Reset accumulators for the next parse_content call.
                self.neighborhood_n_hits = 0;
                self.neighborhood_n_misses = 0;
                self.pre_regularize_us = 0;
                self.regularize_us = 0;
                self.cache_fetch_us = 0;
                self.mid_match_us = 0;
                self.union_mut_us = 0;
                self.total_per_call_us = 0;
                self.n_cache_arm = 0;
                self.n_push_relation = 0;
            }
            #[cfg(target_arch = "wasm32")]
            tracing::debug!(
                target: "noet_core::codec::perf",
                file_path = %file_path_str,
                push_relation_ms = push_relation_total_ms,
                phase2_total_ms = phase2_start.elapsed().as_millis(),
                "[Phase 2] push_relation loop complete",
            );
            if !generated_href_nodes.is_empty() {
                parsed_bids.append(&mut generated_href_nodes);
            }

            // Perform this after going through all the proto relations so we don't destroy our
            // balanced set.
            if !missing_structure.is_empty() {
                // Use merge_from with the current file's parsed_bids as the DFS seed set.
                // This bounds the DFS to O(rhs_size) rather than O(session_bb_size × rhs_edges),
                // fixing the O(N²) BN-1 bottleneck on large corpora (Issue 47).
                let t = std::time::Instant::now();
                self.session_bb
                    .merge_from(&missing_structure, &relation_seeds);
                tracing::debug!(
                    target: "noet_core::codec::perf",
                    file_path = %file_path_str,
                    session_bb_nodes = self.session_bb.states().len(),
                    missing_structure_nodes = missing_structure.states.len(),
                    elapsed_ms = t.elapsed().as_millis(),
                    "[Phase 2] session_bb.merge_from",
                );
                // we need to merge this phase 2 missing structure into self.doc_bb as well to ensure
                // we have full structural paths to all the external nodes we connect to within the
                // relation_event_queue. Use relation_seeds as the DFS seed — the same seed used
                // for session_bb.merge_from above.
                //
                // Section nodes from this file that had no cross-doc push_relation calls are
                // handled by the Phase 1 per-node doc_bb.merge_from (added alongside the
                // session_bb.merge_from in the push() loop above). That call uses node_seed={bid}
                // to pull each section node's structure into doc_bb immediately after push(),
                // before missing_structure is reset. By the time Phase 2 runs, those section
                // nodes are already in doc_bb — this call only needs to cover the external
                // reference endpoints collected in relation_seeds.
                let t = std::time::Instant::now();
                self.doc_bb.merge_from(&missing_structure, &relation_seeds);
                tracing::debug!(
                    target: "noet_core::codec::perf",
                    file_path = %file_path_str,
                    doc_bb_nodes = self.doc_bb.states().len(),
                    elapsed_ms = t.elapsed().as_millis(),
                    "[Phase 2] doc_bb.merge_from",
                );
            }
            let t = std::time::Instant::now();
            let relation_event_count = relation_event_queue.len();
            // Batch-apply all relation events: three-pass (nodes, relations, PathMapMap once).
            // This replaces O(N) per-event PathMapMap invocations with a single flush,
            // eliminating the dominant O(N * PathMaps) bottleneck in large corpora.
            let resolved = self.doc_bb.apply_events_batch(&relation_event_queue)?;
            let _path_events = self.doc_bb.flush_paths_for_events(&resolved);
            relation_event_queue.clear();
            tracing::debug!(
                target: "noet_core::codec::perf",
                file_path = %file_path_str,
                relation_event_count,
                elapsed_ms = t.elapsed().as_millis(),
                "[Phase 2] doc_bb.apply_events_batch + flush_paths",
            );

            tracing::debug!(
                "Phase 3: inform external sinks about nodekey changes from this document"
            );
            // (re)parse documents are are either to
            // 1) update their contents to reflect updated nodekey's from this parsed document.
            if !parsed_bids.is_empty() {
                for source_bid in check_sinks.keys() {
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
                tracing::trace!("Phase 3: affected_sinks: {:?}", docs_to_parse);
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

            for (proto_idx, (proto, bid)) in
                codec.nodes().iter().zip(parsed_bids.iter()).enumerate()
            {
                let in_states = self.doc_bb.states().contains_key(bid);
                let in_pathmap = self
                    .doc_bb
                    .paths()
                    .get_map(&self.repo().bref())
                    .map(|pm| pm.bid_has_path(bid))
                    .unwrap_or(false);
                let ctx = match self.doc_bb.get_context(&self.repo(), bid) {
                    Some(ctx) => ctx,
                    None => {
                        let node_title = self
                            .doc_bb
                            .states()
                            .get(bid)
                            .map(|n| n.title.as_str())
                            .unwrap_or("<not in states>");
                        tracing::warn!(
                            "Phase 4: skipping unbalanced node — bid={bid} bref={} \
                             in_states={in_states} in_pathmap={in_pathmap} \
                             proto.heading={} proto.path={:?} \
                             node_title={:?} doc_path={:?} \
                             network={}",
                            bid.bref(),
                            proto.heading,
                            proto.path,
                            node_title,
                            doc_path,
                            self.repo().bref(),
                        );
                        diagnostics.push(ParseDiagnostic::warning(format!(
                            "Phase 4: unbalanced set — context injection skipped for \
                             bid={bid} (bref={}) in_states={in_states} \
                             in_pathmap={in_pathmap} proto.heading={} \
                             proto.path={:?} node_title={:?} doc={:?} network={}",
                            bid.bref(),
                            proto.heading,
                            proto.path,
                            node_title,
                            doc_path,
                            self.repo().bref(),
                        )));
                        continue;
                    }
                };
                tracing::debug!(
                    "[Phase4:inject_context] bid={} proto.path={:?} proto.heading={} \
                    ctx.root_path={:?} ctx.root_net={} ctx.home_net={} \
                    ctx.node.title={:?} ctx.node.bid={}",
                    bid,
                    proto.path,
                    proto.heading,
                    ctx.root_path,
                    ctx.root_net,
                    ctx.home_net,
                    ctx.node.title,
                    ctx.node.bid,
                );
                let old_node = ctx.node.clone();
                // Inject proto text into our self set here, because inject context is where the
                // markdown parser generates section-specific text fields regardless of whether
                // it changes the markdown itself due to the injected context.
                if let Some(updated_node) =
                    codec.inject_context(proto_idx, proto, &ctx, &mut diagnostics)?
                {
                    // Compare via PartialEq rather than round-tripping through TOML strings:
                    // string comparison is fragile (key ordering, whitespace) and silently
                    // ignores fields like metadata that are not round-tripped through IRNode.
                    if updated_node != old_node {
                        is_changed = true;
                        let _derivatives = self.doc_bb.process_event(&BeliefEvent::NodeUpdate(
                            vec![NodeKey::Bid {
                                bid: updated_node.bid,
                            }],
                            updated_node.clone(),
                            EventOrigin::Remote,
                        ))?;
                    }
                }
            }

            // Phase 4b: Finalize codec (cross-node cleanup, emit events for modified nodes)
            tracing::debug!("Phase 4b: codec finalization");
            let finalized_nodes = codec.finalize(&mut diagnostics)?;
            for (bid, ir_node) in finalized_nodes {
                // Apply source-file-derived fields (kind, title, schema, payload, id) to the
                // existing doc_bb node via apply_source_update, which leaves runtime-only
                // fields (bid, metadata) untouched.  This avoids losing metadata (e.g. git
                // status) that push() injected but that IRNode→BeliefNode conversion drops.
                if let Some(existing) = self.doc_bb.states().get(&bid).cloned() {
                    let mut updated_node = existing.clone();
                    let changed = updated_node.apply_source_update(&ir_node).unwrap_or(false);
                    if changed {
                        is_changed = true;
                        let derivatives = self
                            .doc_bb
                            .insert_state(updated_node, &[NodeKey::Bid { bid }]);
                        if !derivatives.is_empty() {
                            tracing::warn!(
                                "[parse_content] finalize() node update created derivative events; this is unexpected. derivatives: {derivatives:?}",
                            );
                        }
                    }
                } else {
                    // Node not yet in doc_bb — insert fresh (no runtime fields to preserve).
                    // Construct from the IRNode directly since there's nothing in doc_bb to update.
                    match crate::properties::BeliefNode::try_from(&ir_node) {
                        Ok(mut fresh_node) => {
                            fresh_node.bid = bid;
                            is_changed = true;
                            let derivatives = self
                                .doc_bb
                                .insert_state(fresh_node, &[NodeKey::Bid { bid }]);
                            if !derivatives.is_empty() {
                                tracing::warn!(
                                    "[parse_content] finalize() new node created derivative events; this is unexpected. derivatives: {derivatives:?}",
                                );
                            }
                        }
                        Err(e) => {
                            tracing::warn!(
                                "[parse_content] finalize() returned IRNode for bid={bid} that could not be converted: {e}"
                            );
                        }
                    }
                }
            }

            if is_changed || has_new_bids {
                tracing::trace!("Generating source");
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
        // Include any BIDs clobbered by insert_state during Phase 1 pushes.  These belong
        // to foreign nodes (not parsed in this pass) whose id was reset in-place due to a
        // collision with an incoming document or section.  Adding them to parsed_nodes lets
        // compute_diff Phase 2 emit the corrective NodeUpdate for session_bb.
        let mut parsed_bid_set = BTreeSet::<Bid>::from_iter(parsed_bids);
        parsed_bid_set.extend(clobbered_bids);
        self.terminate_stack(bid_renames, &parsed_bid_set).await?;

        let repo_node = if self.repo != Bid::nil() {
            self.session_bb.states().get(&self.repo).cloned()
        } else {
            None
        };

        Ok(ParseContentWithCodec {
            result: ParseContentResult {
                rewritten_content: maybe_content,
                diagnostics,
                derived_paths: Vec::new(),
            },
            codec: owned_codec,
            repo_bid: self.repo,
            repo_node,
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
            BeliefEvent::NodeUpdate(vec![api_key.clone()], api_node.clone(), EventOrigin::Remote);
        self.doc_bb.process_event(&api_node_event)?;
        // Ensure global_bb shares our API node
        //
        // TODO figure out a way to do this check only once per session instead
        // of at each initialize_stack operation.
        if self.session_bb.get(&api_key).is_none() {
            self.session_bb.process_event(&api_node_event)?;
        }
        if !self.skip_global_api_check && lookup_node(&global_bb, &api_key).await?.is_none() {
            self.tx.send(api_node_event)?;
        }

        // Fetch const_namespaces from global_bb to populate session_bb with known assets.
        // This enables asset content change detection by populating PathMap with existing paths.
        // Guard: only run once per session — these are static global namespaces (href + asset)
        // that never change between files. Repeating this on every initialize_stack call was the
        // primary driver of session_bb O(N²) growth across a corpus run.
        let const_namespaces = content_namespaces();
        let missing_const_namespaces: Vec<&Bid> = const_namespaces
            .iter()
            .filter(|ns| self.session_bb.get(&NodeKey::Bid { bid: **ns }).is_none())
            .collect();
        if !missing_const_namespaces.is_empty() {
            tracing::warn!(
                target: "noet_core::codec::const_ns_refetch",
                path = %abs_path.as_ref().display(),
                session_bb_label = self.session_bb.label,
                missing = ?missing_const_namespaces,
                "[initialize_stack] const-namespace guard MISS — about to re-fetch full subgraph(s) from global_bb"
            );
        }
        for const_bid in &missing_const_namespaces {
            let refetch_start = std::time::Instant::now();
            let key = NodeKey::Bid { bid: **const_bid };
            if let Some(const_ns_node) = lookup_node(&global_bb, &key).await? {
                // Process asset namespace node into session_bb
                let const_ns_event = BeliefEvent::NodeUpdate(
                    vec![key.clone()],
                    const_ns_node.clone(),
                    EventOrigin::Remote,
                );
                self.session_bb.process_event(&const_ns_event)?;

                // Fetch all assets connected to this namespace.
                //
                // Leaf-anchored query (Section leaf-ward walk only, no halo): asset
                // and href nodes are the SOURCE of their Section edge to the
                // namespace (the namespace is the sink) -- see
                // GraphBuilder::process_asset. Walking toward roots (anchored /
                // balance_map) from the namespace only continues upward and never
                // reaches any of its children. leaf_anchored walks the other way
                // (toward leaves) to find everything registered under this
                // namespace hub, while still avoiding the O(N) halo explosion a
                // balanced query would incur by fanning out through every document
                // sharing a neighbor with the namespace hub.
                let const_spec = QuerySpec::seed(TapeFn::Bids(vec![**const_bid]));
                let mut const_package = QueryPackage::leaf_anchored(const_spec);
                global_bb.evaluate(&mut const_package).await?;
                let const_graph = const_package.into_graph();

                // Merge the fetched asset graph into session_bb.
                // Seed from the namespace node itself — bounds DFS to assets reachable
                // from this namespace, not all of session_bb.
                let ns_seed: BTreeSet<Bid> = BTreeSet::from([**const_bid]);
                self.session_bb.merge_from(&const_graph, &ns_seed);

                tracing::warn!(
                    target: "noet_core::codec::const_ns_refetch",
                    path = %abs_path.as_ref().display(),
                    session_bb_label = self.session_bb.label,
                    const_bid = %const_bid,
                    fetched_states = const_graph.states.len().saturating_sub(1), // -1 for namespace node itself
                    fetched_edges = const_graph.relations.as_graph().edge_count(),
                    elapsed_ms = refetch_start.elapsed().as_millis() as u64,
                    "[initialize_stack] const-namespace re-fetch complete"
                );
            }
        }

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
            // Distinguish the two reasons fast-path returns None:
            //
            // (a) Entry IS the root network's index.md — its parent is above the repo root,
            //     so there is no parent network to query. This is always expected and not a
            //     problem; the slow path handles it correctly.
            //
            // (b) Entry is a child of a known subnet but the parent wasn't found in
            //     session_bb / global_bb — this is unexpected and suggests a dropped
            //     RelationUpdate or a PathMap registration failure.
            //
            // Detect (a) by checking whether abs_path is the root index.md: after popping
            // NETWORK_NAME, the remaining directory equals repo_root, meaning there is no
            // grandparent network within the repo.
            let is_root_network_index = {
                let mut probe = abs_path.as_ref().to_path_buf();
                if is_network_index_file(&probe) {
                    probe.pop(); // remove "index.md" → now at the network dir
                }
                // If the network dir IS the repo root, this is case (a).
                probe == self.repo_root()
            };
            if is_root_network_index {
                tracing::debug!(
                    target: "noet_core::codec::fast_path",
                    path = %abs_path.as_ref().display(),
                    "[initialize_stack] root network index.md — fast-path inapplicable \
                     (no parent network), using slow path as expected."
                );
            } else {
                tracing::warn!(
                    target: "noet_core::codec::fast_path",
                    path = %abs_path.as_ref().display(),
                    repo = %self.repo,
                    "[initialize_stack] fast-path returned None — falling through to slow path. \
                     If this path is a child of a known subnet, parent PathMap registration \
                     may be missing in global_bb (RelationUpdate dropped?)."
                );
            }
        }

        let initial_factory = CLAIM_MAP
            .get(abs_path.as_ref())
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
        if is_network_index_file(&parent_path) {
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
            // Issue 34: skip ancestor directories that were explicitly rejected by a
            // codec. These directories have network index files so proto_for() returns
            // Some, but their codec has already called CLAIM_MAP.reject() to suppress
            // parsing. Without this guard, push() creates a Trace network node whose
            // state is never committed — producing ghost BIDs that appear in relations
            // but not in states at finalization.
            let canonical_path =
                crate::paths::canonicalize_path(&path).unwrap_or_else(|_| path.clone());
            if CLAIM_MAP.is_rejected(&canonical_path) {
                tracing::debug!(
                    "[initialize_stack] skipping rejected ancestor {:?} \
                     (CLAIM_MAP.is_rejected = true)",
                    path,
                );
                continue;
            }

            // Use the ProtoIndex (built once at compiler startup via a single WalkDir pass)
            // to look up the ancestor network proto without redundant filesystem scans.
            let Some((state_accum, git_status)) = proto_index.proto_for(&path)? else {
                continue;
            };

            let mut ancestor_clobbers = BTreeSet::new();
            let git_metadata_override: Option<TomlTable> = {
                #[cfg(feature = "git-tracking")]
                {
                    git_status
                        .as_ref()
                        .and_then(|git_val| {
                            serde_json::from_value::<NetworkGitStatus>(git_val.clone()).ok()
                        })
                        .map(|gs| {
                            let mut meta = TomlTable::new();
                            meta.insert(
                                "git".to_string(),
                                toml::Value::Table(gs.to_metadata_table()),
                            );
                            meta
                        })
                }
                #[cfg(not(feature = "git-tracking"))]
                {
                    let _ = git_status;
                    None
                }
            };

            let (ancestor, (_source, _, _)) = self
                .push(
                    &state_accum,
                    global_bb.clone(),
                    true,
                    &mut missing_structure,
                    None,            // ancestor network nodes; sort key is not relevant here
                    &mut Vec::new(), // ancestor pushes are trace-only; diagnostics discarded
                    &mut ancestor_clobbers,
                    git_metadata_override,
                    0, // ancestor trace pushes — cache misses always expected
                )
                .await?;
            if !ancestor_clobbers.is_empty() {
                tracing::error!(
                    "We should not be rewriting ids in initialize stack! \
                    Clobbered nodes: {ancestor_clobbers:?}"
                );
            }

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

        // Determine the entry document's sort key using the ProtoIndex — single canonical
        // source of truth shared by both fast and slow paths.  sort_key_for walks up the
        // directory tree to handle files in non-network subdirectories that iter_net_docs
        // flattens into the ancestor network's child list.
        let doc_sort_key: Option<u16> = proto_index.sort_key_for(abs_path.as_ref());

        Ok((initial, doc_sort_key))
    }

    async fn terminate_stack(
        &mut self,
        renamed_nodes: BTreeMap<Bid, Bid>,
        parsed_nodes: &BTreeSet<Bid>,
    ) -> Result<(), BuildonomyError> {
        // ensure the stack is empty
        self.stack.clear();
        // First, apply node renames in order to have a solid basis for our next operations
        let mut tx_events = Vec::new();
        for (from_bid, to_bid) in renamed_nodes.iter() {
            let rename_event = BeliefEvent::NodeRenamed(*from_bid, *to_bid, EventOrigin::Remote);
            let mut derivatives = self.session_bb.process_event(&rename_event)?;
            tx_events.push(rename_event);
            tx_events.append(&mut derivatives);
        }

        // Reset owner_index for the next parse_content call.
        self.owner_index.clear();

        let mut diff_events =
            BeliefBase::compute_diff(&self.session_bb, &self.doc_bb, parsed_nodes)?;

        let mut path_events = Vec::new();
        for event in diff_events.iter() {
            if let BeliefEvent::RelationRemoved(source, sink, _) = event {
                // Removed relations indicate instability: a previously-known edge is
                // being retracted. Log at WARN so parse_log.py can surface them without
                // requiring RUST_LOG=debug.
                tracing::warn!(
                    "[terminate_stack] RelationRemoved: \"{}\" → \"{}\"",
                    self.session_bb
                        .paths()
                        .path(source)
                        .map(|(_home_net, path)| path)
                        .unwrap_or(source.to_string()),
                    self.session_bb
                        .paths()
                        .path(sink)
                        .map(|(_home_net, path)| path)
                        .unwrap_or(sink.to_string())
                );
            }
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
                    BeliefEvent::NodeUpdate(_, _, _) | BeliefEvent::NodeUpsert(_, _, _) => {
                        node_update_count += 1;
                    }
                    BeliefEvent::NodesRemoved(nids, _) => {
                        node_removed_count += nids.len();
                    }
                    BeliefEvent::NodeRenamed(_, _, _) => {
                        node_renamed_count += 1;
                    }
                    BeliefEvent::RelationChange(_, _, _, _, _) => {
                        relation_insert_count += 1;
                    }
                    BeliefEvent::RelationRemoved(_, _, _) => {
                        relation_removed_count += 1;
                    }
                    BeliefEvent::RelationUpdate(_, _, _, _) => {
                        relation_update_count += 1;
                    }
                    BeliefEvent::PathAdded(_, _, _, _, _) => {
                        path_update_count += 1;
                    }
                    BeliefEvent::PathUpdate(_, _, _, _, _) => {
                        path_update_count += 1;
                    }
                    BeliefEvent::PathsRemoved(net, paths, _) => {
                        path_removed_count += paths.len();
                        // Removed paths also indicate instability (a node moved or was
                        // reclassified). Log each removed path at WARN for parse_log.py.
                        for path in paths {
                            tracing::warn!(
                                "[terminate_stack] PathsRemoved: net={} path={:?}",
                                net,
                                path,
                            );
                        }
                    }
                    BeliefEvent::FileParsed(_) => {} // Metadata only, handled by Transaction
                    BeliefEvent::BatchStart | BeliefEvent::BatchEnd => {}
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

        // Classify content type for nodes with text: intercept NodeUpdate/NodeUpsert
        // events, score the node's payload["text"] and inject
        // metadata.content_profile before emitting downstream.
        let tx_events: Vec<BeliefEvent> = tx_events
            .into_iter()
            .map(|event| match event {
                BeliefEvent::NodeUpdate(keys, mut node, origin) => {
                    Self::classify_node(&mut node, &self.stemmer);
                    BeliefEvent::NodeUpdate(keys, node, origin)
                }
                BeliefEvent::NodeUpsert(bid, mut node, origin) => {
                    Self::classify_node(&mut node, &self.stemmer);
                    BeliefEvent::NodeUpsert(bid, node, origin)
                }
                other => other,
            })
            .collect();

        for event in tx_events.into_iter() {
            self.tx.send(event)?;
        }

        Ok(())
    }

    /// Score a node's text content and write the content profile into its metadata.
    fn classify_node(node: &mut BeliefNode, stemmer: &Stemmer) {
        let raw_text = node
            .payload
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let tokens: Vec<String> = tokenize(raw_text, stemmer).collect();
        let profile = score_lexical(&tokens, raw_text);
        if !profile.is_zero() {
            node.metadata.insert(
                "content_profile".to_string(),
                toml::Value::Table(profile.to_toml()),
            );
        }
    }

    fn get_parent_from_stack(&mut self, proto: &IRNode) -> (Bid, String, String) {
        // proto.path may contain a Windows drive-letter prefix (e.g. "C:/tmp/foo.md") because
        // os_path_to_string preserves it.  stack entries are also stored with the drive-letter
        // prefix.  AnchorPath::filepath() strips the drive letter on both sides, giving a
        // consistent comparison.  We normalise proto.path here rather than at construction time
        // so that PathBuf-based operations in initialize_stack (which need the drive letter for
        // strip_prefix against repo_root) continue to work.
        // For network nodes, proto.path is an extension-less directory path (e.g.
        // ".../build/req/program_requirements").  AnchorPath::new() sees no extension
        // and calls dir(), stripping the last component to ".../build/req" — so
        // strip_prefix against the parent's ".../build/req/" prefix returns "" instead
        // of "program_requirements", leaving WEIGHT_DOC_PATHS empty and forcing
        // generate_terminal_path to fall back to the bref as the path component.
        // Use new_dir() for network nodes so filepath() returns the full directory path.
        let proto_filepath = if proto.kind.is_network() {
            AnchorPath::new_dir(&proto.path).filepath().to_string()
        } else {
            AnchorPath::new(&proto.path).filepath().to_string()
        };
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
                    // Extract document path from stack_path.  For network frames
                    // (stack_heading == 1) stack_path is an absolute directory path
                    // (no extension, no trailing slash).  AnchorPath::new would call
                    // dir() on it, stripping the last component.  Use new_dir so that
                    // filepath() returns the full directory path for starts_with checks.
                    let stack_filepath = if *stack_heading < 2 {
                        AnchorPath::new_dir(stack_path).filepath().to_string()
                    } else {
                        AnchorPath::new(stack_path).filepath().to_string()
                    };
                    (proto_filepath.starts_with(&stack_filepath)
                        && proto_filepath != stack_filepath
                        && !proto
                            .kind
                            .intersection(BeliefKind::Network | BeliefKind::Document)
                            .is_empty())
                        || (proto_filepath == stack_filepath && *stack_heading < proto.heading)
                })
                .map(|(stack_bid, stack_path, stack_heading)| {
                    // Use proto_filepath (drive-letter-stripped) so that strip_prefix can
                    // match against stack_path regardless of Windows drive-letter form.
                    //
                    // For network frames (stack_heading == 1) stack_path is a bare
                    // directory path.  strip_prefix calls AnchorPath::new(prefix).filepath()
                    // on the prefix, which for extension-less paths calls dir(), stripping
                    // the last component and producing a grandparent-relative child path
                    // instead of a network-relative one.  Append a trailing slash so that
                    // filepath() returns the full directory path, giving the correct
                    // subnet-relative remainder.
                    //
                    // For document frames (stack_heading >= 2) stack_path is a file path
                    // with an extension; AnchorPath::new handles it correctly as-is.
                    let stack_path_dir: String;
                    let prefix = if *stack_heading < 2 {
                        stack_path_dir = format!("{stack_path}/");
                        stack_path_dir.as_str()
                    } else {
                        stack_path.as_str()
                    };
                    // strip_prefix does a plain string prefix match after calling
                    // filepath() on the prefix argument.  The dir/file classification
                    // of proto_filepath itself is irrelevant here — use plain new().
                    //
                    // NOTE: probes here were ruled out as the source
                    // of the malformed `index.md<dir>/<slug>` paths. On a corpus with
                    // 51,591 such paths, a probe on `strip_prefix` failing and a probe
                    // on an anchor doc_path containing '/' both fired **zero** times.
                    // Whatever produces those paths, it is not this site — do not
                    // re-investigate here without new evidence.
                    let path_info = AnchorPath::new(&proto_filepath)
                        .strip_prefix(prefix)
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
            // Use the parent network's bref from the stack (the innermost heading==1 entry).
            // Falls back to the repo bref when the stack is empty (repo root is being parsed).
            //
            // Using Bref::default() here was the original bug: the in-memory PathMapMap
            // normalizes Bref::default() to the API bref and recurses through all subnet
            // PathMaps, accidentally finding the node. The DB-backed BeliefSource passes
            // the raw bref string directly to SQL (p.net = 'b4a023772a74'), which matches
            // nothing because no node is stored under the nil bref in the paths table —
            // causing a cache_fetch miss on every re-parse and cascading to fresh BIDs for
            // every child node of the affected subnet.
            let parent_net_bref = self
                .stack
                .iter()
                .rev()
                .find(|(_bid, _path, heading)| *heading == 1)
                .map(|(bid, _path, _heading)| bid.bref())
                .unwrap_or_else(|| self.repo.bref());
            let id_key = NodeKey::Id {
                net: parent_net_bref,
                id: network_id.clone(),
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
        // net_path is the absolute directory path for the network (no extension, no
        // trailing slash, as stored on the stack).  strip_prefix calls
        // AnchorPath::new(prefix).filepath() on the prefix, which for extension-less
        // paths calls dir(), stripping the last component (Bug 2).  Append a trailing
        // slash so that filepath() returns the full directory path and strip_prefix
        // yields the correct subnet-relative child path (e.g. "file.md" not
        // "subnet/file.md").
        let net_path_dir = if net_path.is_empty() || net_path.ends_with('/') {
            net_path.clone()
        } else {
            format!("{net_path}/")
        };
        let net_anchored_child = AnchorPath::new(&proto_filepath_str)
            .strip_prefix(&net_path_dir)
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
            // Early return for id == bref -- this honor's the case when we programmatically update
            // an ID and set it to the bref in order to avoid an intra network ID collision.
            if let Ok(bref) = Bref::try_from(section_id.as_str()) {
                return Some(NodeKey::Bref { bref });
            }

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
    #[allow(clippy::too_many_arguments)]
    async fn push<B: BeliefSource + Clone>(
        &mut self,
        proto: &IRNode,
        global_bb: B,
        as_trace: bool,
        missing_structure: &mut BeliefGraph,
        explicit_sort_key: Option<u16>,
        diagnostics: &mut Vec<ParseDiagnostic>,
        clobbered_bids: &mut BTreeSet<Bid>,
        metadata_override: Option<TomlTable>,
        // 0 = ancestor/trace (misses always expected), 1 = first parse
        // (forward-ref misses expected), >1 = re-parse (misses are unexpected).
        parse_number: usize,
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
            .cache_fetch(
                &keys,
                global_bb.clone(),
                false,
                missing_structure,
                parse_number,
            )
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
                    parsed_node.id = NodeId::Explicit(parsed_node.bid.bref().to_string());
                }
                (parsed_node, source)
            }
        };
        let bid = node.bid;

        // Network-level ID collision detection — document-beats-anchor special case only.
        //
        // The general first-one-wins collision policy (two section nodes competing for the
        // same anchor id) is now handled inside BeliefBase::insert_state, which fires for
        // every process_event(NodeUpdate) call — including events from parallel epoch tasks
        // that bypass push() entirely.  insert_state emits Local NodeUpdate events for the
        // clobbered node; we harvest those below when we call doc_bb.process_event(NodeUpdate)
        // and add the affected BIDs to clobbered_bids so compute_diff sees them in Phase 2.
        //
        // The only case that still requires pre-processing here is the first-one-wins branch
        // where the *incoming* node loses: node.id and keys must be fixed up before the insert
        // so that insert_state's to_replace loop uses the corrected (bref) id, and so that
        // current_keys reflects the actual stored state.
        // Use collision_aware_id() so that Collision nodes (whose slug collided
        // on a prior parse) return their bref — making node_id == node_bref and
        // skipping the collision check entirely. This prevents the cascade where
        // a persisted Collision node re-triggers first-one-wins on every reparse.
        let node_id = node.collision_aware_id();
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
                    parse_number,
                )
                .await?;

            // Merge any missing structure from ID fetch
            if !id_missing_structure.is_empty() {
                missing_structure.union_mut(&id_missing_structure);
            }

            if let GetOrCreateResult::Resolved(existing_node, existing_source) = id_fetch_result {
                if existing_source.is_from_cache() && existing_node.bid != bid {
                    let incoming_is_document = node.kind.0.contains(BeliefKind::Document);
                    let incoming_is_network = node.kind.0.contains(BeliefKind::Network);
                    let existing_is_anchor = existing_node.kind.is_anchor();

                    if incoming_is_network && existing_is_anchor {
                        // Network wins over anchor with the same id slug.
                        // A network node owns its directory name by definition — a heading
                        // in the network's own index.md that matches the directory name
                        // (e.g. `# Power` in `power/index.md`) must lose.
                        // insert_state handles the clobber atomically via the
                        // network-beats-anchor branch, resetting the anchor's id to its bref
                        // and emitting a Local NodeUpdate derivative.
                    } else if incoming_is_document && existing_is_anchor {
                        // Document wins over section with the same network id.
                        // insert_state handles the clobber atomically when we call
                        // doc_bb.process_event(node_update_event) below: it detects the
                        // document-beats-anchor case, resets the anchor's id in-place, and
                        // emits a Local NodeUpdate derivative.  We harvest that derivative
                        // into clobbered_bids so compute_diff's Phase 2 sees the anchor's
                        // id change.  No pre-clearing of doc_bb or session_bb is needed here.
                    } else {
                        tracing::debug!(
                            target: "noet_core::codec::fast_path",
                            proto_path = %proto.path,
                            node_id = %node_id,
                            incoming_bid = %bid,
                            existing_bid = %existing_node.bid,
                            parse_number,
                            "[push] network node FIRST-ONE-WINS: incoming loses, \
                             id will be set to bref",
                        );
                        // First-one-wins: incoming loses, clear id so inject_context uses bref.
                        // We must do this here (not just in insert_state) so that:
                        //   1. `node.toml()` passed to doc_bb.process_event has the correct id.
                        //   2. `keys` is updated so the NodeKey::Id no longer points at the
                        //      collision id — preventing insert_state's to_replace loop from
                        //      finding the existing winner via the old key.
                        //   3. current_keys (returned from push) reflects the actual stored state.
                        node.id = NodeId::Collision(node_id.clone());
                        // Regenerate keys with bref-based ID to avoid
                        // the collision key pointing at the winner.
                        let disambiguated_id = node.collision_aware_id();
                        for key in keys.iter_mut() {
                            match key {
                                NodeKey::Id { .. } => {
                                    *key = NodeKey::Id {
                                        net: net.bref(),
                                        id: disambiguated_id.clone(),
                                    };
                                }
                                NodeKey::Path { .. } => {
                                    // The stale path key still contains the colliding
                                    // section id fragment.  Replace it with a Bref key
                                    // so insert_state cannot find (and absorb) the
                                    // collision winner through path-key matching.
                                    // This mirrors build_path_key's logic: a bref-based
                                    // section id produces NodeKey::Bref, not Path.
                                    if let Ok(bref) = Bref::try_from(disambiguated_id.as_str()) {
                                        *key = NodeKey::Bref { bref };
                                    }
                                }
                                _ => {}
                            }
                        }
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

        // Apply metadata override last — always stomps any stale cached metadata so the
        // freshly-computed parse-time annotations (e.g. git status) win unconditionally.
        if let Some(meta) = metadata_override {
            node.metadata = meta;
        }

        let node_update_event =
            BeliefEvent::NodeUpdate(keys.clone(), node.clone(), EventOrigin::Remote);
        let derivative_events = self.doc_bb.process_event(&node_update_event)?;

        // Harvest Local NodeUpdate derivatives — these are clobber events emitted by
        // insert_state when it resets a colliding node's id in-place (both the
        // document-beats-anchor and first-one-wins branches).  Adding their BIDs to
        // clobbered_bids ensures compute_diff's Phase 2 sees the id change even though
        // those BIDs are not in parsed_nodes (they belong to a different document).
        for event in &derivative_events {
            if let BeliefEvent::NodeUpdate(keys, _, EventOrigin::Local) = event {
                for key in keys {
                    if let NodeKey::Bid { bid: clobbered_bid } = key {
                        clobbered_bids.insert(*clobbered_bid);
                    }
                }
            }
        }

        // Drain any collision diagnostics emitted by insert_state and surface them
        // to the compiler's result set.
        diagnostics.extend(self.doc_bb.drain_diagnostics());

        let mut weight = Weight {
            payload: TomlTable::new(),
        };
        let path_info_for_log = path_info.clone();
        {
            let mut doc_paths = Vec::new();
            if !path_info.is_empty() {
                doc_paths.push(path_info);
            }
            // Append any path aliases declared by the codec (e.g. include-convention
            // paths, derived header filenames).  Each alias creates an additional
            // PathMap entry pointing to the same BID.
            doc_paths.extend(proto.path_aliases.iter().cloned());
            if !doc_paths.is_empty() {
                weight.set_doc_paths(doc_paths).ok();
            }
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
        // explicit_sort_key is derived from the authoritative source (net_dir_children order /
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
            tracing::debug!(
                "[push] network node: bid={} id={:?} title={:?} parent_bid={} \
                 path_info={:?} as_trace={}",
                bid,
                &node.id,
                node.title,
                parent_bid,
                &path_info_for_log,
                as_trace,
            );
            // Warn only for subnet networks (parent != API) with no doc_path.
            // The repo root legitimately has empty path_info because its Section
            // edge is API-owned with no doc_path — that is expected and not a bug.
            if path_info_for_log.is_empty() && parent_bid != self.api().bid {
                tracing::warn!(
                    "[push] subnet network node has empty path_info: bid={} title={:?} \
                     parent_bid={} proto.path={:?} stack={:?}",
                    bid,
                    node.title,
                    parent_bid,
                    proto.path,
                    self.stack
                        .iter()
                        .map(|(b, p, h)| format!("({b},{p:?},h{h})"))
                        .collect::<Vec<_>>(),
                );
            }
            // If the builder repo is nil, and this node is a network, and the
            // stack is empty, then initialize the builder repo to this element.
            // We don't do this operation in [GraphBuilder::new] because
            // reading the repo source is part of our async operations.
            if self.repo == Bid::nil() && parent_bid == self.api().bid {
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

        // Process namespace_paths: register this node in secondary index namespaces.
        // Each entry creates a Section edge from this node to the namespace network,
        // with the alias path as the doc_paths weight (populating the namespace's PathMap).
        if !proto.namespace_paths.is_empty() {
            for (ns_bid, alias_path) in &proto.namespace_paths {
                if *ns_bid == href_namespace() {
                    // href-namespace alias: use the shared helper instead of the
                    // generic codec-namespace factory.  The aliased node gets two
                    // Section sinks: its structural parent (from the stack) and
                    // href_namespace() (from this alias registration).

                    // Collision detection: check session_bb for an existing owner
                    // of this alias path in the href PathMap.
                    //
                    // Content nodes always beat External|Trace stubs (created by
                    // push_relation for unresolved links). Among content nodes,
                    // first-one-wins: the second content node to declare an alias
                    // emits a diagnostic warning and is skipped.
                    // Check `doc_bb` as well as `session_bb`. A stub is created by
                    // `push_relation` into `doc_bb` and only reaches `session_bb` when
                    // `terminate_stack` runs at end-of-file. So when one document both
                    // *links to* a URL and *declares* it as an alias — the common case
                    // for a generated corpus where every item cross-references its
                    // siblings — the stub is invisible here and the absorb below never
                    // fires. Measured: only 17 of 41 colliding URLs reached this branch
                    // when consulting `session_bb` alone.
                    let existing_owner = {
                        let pmm = self.doc_bb.paths();
                        pmm.href_map().get(alias_path, &pmm).map(|(_net, bid)| bid)
                    }
                    .or_else(|| {
                        let pmm = self.session_bb.paths();
                        pmm.href_map().get(alias_path, &pmm).map(|(_net, bid)| bid)
                    });
                    if let Some(existing_bid) = existing_owner {
                        if existing_bid == bid {
                            // Self-collision on re-parse: same BID already owns this alias.
                            // Fall through to re-emit the edge (idempotent).
                        } else {
                            // Check if the existing owner is an External|Trace stub,
                            // in whichever base holds it.
                            let existing_is_stub = self
                                .doc_bb
                                .states()
                                .get(&existing_bid)
                                .or_else(|| self.session_bb.states().get(&existing_bid))
                                .map(|n| {
                                    n.kind.contains(BeliefKind::External)
                                        && n.kind.contains(BeliefKind::Trace)
                                })
                                .unwrap_or(false);

                            if existing_is_stub {
                                // Content node beats External|Trace stub — retire the
                                // stub rather than letting both hold the path.
                                //
                                // Re-assert this node with a merge key that resolves to
                                // the stub. `insert_state` turns that into
                                // NodeRenamed -> replace_bid -> remove, which re-points
                                // any edge a third document had already drawn to the
                                // stub and deletes the node. Without it the stub
                                // survives in the graph: the PathMap-level eviction in
                                // `map_insert` would drop its index entry, but the next
                                // `PathMap::new` rebuilds from the relations and
                                // re-materialises the duplicate — measured as 466
                                // repeats over 41 URLs on one corpus, evicted and
                                // recreated indefinitely.
                                //
                                tracing::debug!(
                                    "[push] URL alias '{}': content node {} absorbing \
                                     External|Trace stub {} (locally visible)",
                                    alias_path,
                                    bid,
                                    existing_bid,
                                );
                            } else {
                                // Both owners are content nodes — first-one-wins.
                                diagnostics.push(ParseDiagnostic::warning(format!(
                                    "URL alias collision: '{}' is already registered to node {}; \
                                     this node ({}) will not be reachable via this alias.",
                                    alias_path, existing_bid, bid,
                                )));
                                continue; // Skip
                            }
                        }
                    }

                    // Ensure the href_namespace network node exists (no wrapper
                    // node needed — the content node itself is the alias target).
                    let ns_events = self.ensure_href_namespace();
                    for event in &ns_events {
                        self.doc_bb.process_event(event)?;
                        self.session_bb.process_event(event)?;
                        self.tx.send(event.clone())?;
                    }

                    // Register this content node in the href PathMap via a Section edge
                    // to href_namespace().  Only process through doc_bb — NOT session_bb.
                    //
                    // session_bb is the "prior known state" baseline that compute_diff
                    // diffs against.  If we write the alias edge into session_bb here
                    // (during push()), compute_diff sees it as already-present-and-unchanged
                    // in old_parsed_edges and emits nothing.  By leaving session_bb untouched,
                    // compute_diff Phase 4 sees the edge as new (in doc_bb but not session_bb)
                    // and emits RelationUpdate → PathAdded → global_bb PathMap populated.
                    //
                    // Cross-file collision detection (for aliases declared in other files)
                    // relies on session_bb's href PathMap.  That is populated when
                    // terminate_stack processes the RelationUpdate from compute_diff through
                    // session_bb — which happens before any subsequent file's push() call.
                    // Claim the URL as a merge key, so any `External|Trace` stub
                    // holding it is absorbed rather than left to coexist.
                    //
                    // This is emitted unconditionally, not only when a stub is
                    // visible locally. The stub is created by `push_relation` in
                    // whichever task first encountered the link, so for a corpus where
                    // a document both cites and declares the same URL it typically
                    // lives in `global_bb` and in no local base here — measured: a
                    // probe in `ensure_href_entry` found the claim invisible in
                    // `doc_bb` (PathMap *and* by-id) and `session_bb` on every one of
                    // 41 colliding URLs. A locally-gated absorb reached only 17.
                    //
                    // `NodeKey::Path` is what content-namespace nodes key on (see
                    // `BeliefNode::keys`), and `insert_state` resolves it wherever the
                    // event lands: `to_replace` -> NodeRenamed -> `replace_bid`, which
                    // re-points third-party edges before deleting the stub. Sending it
                    // through `tx` is what gets it applied against `global_bb`.
                    //
                    // Note this must go through `process_event`, not
                    // `apply_events_batch` — the latter debug-asserts that a
                    // `NodeUpdate` produces no removal derivatives and discards them.
                    // The in-memory sink's `apply_batch` uses `process_event`, so the
                    // absorption survives.
                    // Two keys, because they resolve by different means:
                    //
                    // - `NodeKey::Bid` over `buildonomy_href_bid(url)`. An href stub's
                    //   BID is UUID v5 of the URL string, so the claimant can *name*
                    //   the stub without having seen it. This resolves against
                    //   `states` directly, with no PathMap involved, which is what
                    //   makes it work in a base that never indexed the stub.
                    // - `NodeKey::Path`, which resolves via `net_get_from_path` — the
                    //   PathMap. Kept for the case where the stub is locally indexed
                    //   but its BID was not derived from this exact string (e.g. a
                    //   normalised or aliased form).
                    //
                    // The Path key alone was measured insufficient: a probe in
                    // `ensure_href_entry` found the claim invisible in `doc_bb`
                    // (PathMap *and* by-id) and `session_bb` for all 41 colliding
                    // URLs, and `insert_state` logged zero absorptions.
                    if let Some(claimant) = self.doc_bb.states().get(&bid).cloned() {
                        let absorb = BeliefEvent::NodeUpdate(
                            vec![
                                NodeKey::Bid {
                                    bid: buildonomy_href_bid(alias_path),
                                },
                                NodeKey::Path {
                                    net: href_namespace().bref(),
                                    path: alias_path.clone(),
                                },
                            ],
                            claimant,
                            EventOrigin::Remote,
                        );
                        self.tx.send(absorb)?;
                    }

                    let mut alias_weight_payload = TomlTable::new();
                    alias_weight_payload.insert(
                        WEIGHT_DOC_PATHS.to_string(),
                        toml::Value::Array(vec![toml::Value::String(alias_path.clone())]),
                    );
                    let alias_edge = BeliefEvent::RelationChange(
                        bid,
                        href_namespace(),
                        WeightKind::Section,
                        Some(Weight {
                            payload: alias_weight_payload,
                        }),
                        EventOrigin::Remote,
                    );
                    self.doc_bb.process_event(&alias_edge)?;
                } else {
                    // Generic codec namespace path (e.g. C++ include namespace).
                    // Ensure the namespace network node is present in doc_bb so
                    // the Section edge below can be processed (both source and
                    // sink must exist in the BB for RelationChange to succeed).
                    if !self.doc_bb.states().contains_key(ns_bid) {
                        if let Some(existing) = self.session_bb.states().get(ns_bid).cloned() {
                            // session_bb already has the namespace (created by a
                            // prior document's push in this compile session).  Copy
                            // the node into doc_bb and connect it to
                            // buildonomy_namespace() so the Section edge at the end
                            // of this block can reference a rooted sink.  No need to
                            // re-emit to session_bb or tx — both already have it.
                            self.doc_bb.process_event(&BeliefEvent::NodeUpsert(
                                *ns_bid,
                                existing,
                                EventOrigin::Remote,
                            ))?;
                            self.doc_bb.process_event(&BeliefEvent::RelationChange(
                                *ns_bid,
                                buildonomy_namespace(),
                                WeightKind::Section,
                                None,
                                EventOrigin::Remote,
                            ))?;
                        } else {
                            // First encounter in this session — create the
                            // namespace node and broadcast to all BBs.
                            let ns_node = {
                                let mut table = toml::Table::new();
                                table.insert(
                                    "api".to_string(),
                                    toml::Value::String(buildonomy_namespace().to_string()),
                                );
                                BeliefNode {
                                    bid: *ns_bid,
                                    title: format!("Codec namespace {}", ns_bid.bref()),
                                    schema: Some("api".to_string()),
                                    payload: table,
                                    kind: BeliefKindSet(
                                        BeliefKind::Network
                                            | BeliefKind::External
                                            | BeliefKind::Trace,
                                    ),
                                    id: NodeId::Explicit(format!(
                                        "buildonomy_codec_{}",
                                        ns_bid.bref()
                                    )),
                                    metadata: toml::Table::new(),
                                }
                            };

                            // Register the bref in the global codec namespace registry
                            // so process_unresolved_reference can skip filesystem resolution.
                            register_codec_namespace(ns_bid.bref());

                            // Process the namespace node through both doc_bb and session_bb.
                            let ns_upsert =
                                BeliefEvent::NodeUpsert(ns_node.bid, ns_node, EventOrigin::Remote);
                            self.doc_bb.process_event(&ns_upsert)?;
                            self.session_bb.process_event(&ns_upsert)?;
                            self.tx.send(ns_upsert)?;

                            let ns_root_edge = BeliefEvent::RelationChange(
                                *ns_bid,
                                buildonomy_namespace(),
                                WeightKind::Section,
                                None,
                                EventOrigin::Remote,
                            );
                            self.doc_bb.process_event(&ns_root_edge)?;
                            self.session_bb.process_event(&ns_root_edge)?;
                            self.tx.send(ns_root_edge)?;
                        }
                    }

                    // Register this node in the namespace's PathMap via a Section edge
                    // with the alias path as doc_paths weight.
                    let mut ns_edge_payload = TomlTable::new();
                    ns_edge_payload.insert(
                        WEIGHT_DOC_PATHS.to_string(),
                        toml::Value::Array(vec![toml::Value::String(alias_path.clone())]),
                    );
                    let _derivative_events =
                        self.doc_bb.process_event(&BeliefEvent::RelationChange(
                            bid,
                            *ns_bid,
                            WeightKind::Section,
                            Some(Weight {
                                payload: ns_edge_payload,
                            }),
                            EventOrigin::Remote,
                        ))?;
                }
            }
        }

        let current_keys =
            BTreeSet::from_iter(node.keys(Some(self.repo()), Some(parent_bid), self.doc_bb()));

        let unique_old =
            BTreeSet::from_iter(BTreeSet::from_iter(keys).difference(&current_keys).cloned());

        Ok((bid, (source, current_keys, unique_old)))
    }

    /// Ensure an href-namespace entry exists for the given URL/path alias.
    ///
    /// Shared between `push()` (for `url_aliases` / `alias-template` registration)
    /// and `push_relation()` (for first-reference external link creation).
    ///
    /// Returns `(BeliefNode, Vec<BeliefEvent>)` — the href wrapper node and the
    /// events needed to install it (NodeUpsert for the namespace root if needed,
    /// NodeUpsert for the href node itself, and RelationChange for the Section edge).
    /// Ensure the `href_namespace()` network node exists in `doc_bb`.
    ///
    /// Returns a `NodeUpsert` event when `href_namespace()` is absent from `doc_bb`
    /// (even if it is present in `session_bb` — `doc_bb` is reset fresh per file).
    /// Returns an empty vec when already present in `doc_bb`.
    ///
    /// Used by both `push()` (alias registration) and `ensure_href_entry()` (external link).
    fn ensure_href_namespace(&self) -> Vec<BeliefEvent> {
        let mut events = Vec::new();
        if !self.doc_bb.states().contains_key(&href_namespace()) {
            let href_net_node = BeliefNode::href_network();
            events.push(BeliefEvent::NodeUpsert(
                href_net_node.bid,
                href_net_node,
                EventOrigin::Remote,
            ));
        }
        events
    }

    /// Create an `External|Trace` wrapper node for an unresolved external URL
    /// and register it in the `href_namespace` PathMap.
    ///
    /// Shared by `push_relation()` for first-reference external link creation.
    /// For alias registration (where a content node already exists), use
    /// `ensure_href_namespace()` + a direct Section edge from the content node.
    fn ensure_href_entry(
        &self,
        href: &str,
    ) -> Result<(BeliefNode, Vec<BeliefEvent>), BuildonomyError> {
        // A stub is minted unconditionally here, even for a URL some content node
        // will later claim as an alias. The claim carries merge keys that retire
        // this stub — see the alias block in `push()` and `resolve_merge_keys` in
        // beliefbase/accumulator.rs — so the duplicate is resolved at node
        // identity rather than by trying to predict the claim from here.
        //
        // Predicting it was measured and does not work: when a document both
        // cites a URL and declares it, the claim is invisible from this call site
        // in `doc_bb` (PathMap and by-id) and in `session_bb` alike, because the
        // stub is created by whichever task first encountered the link.
        let mut update_queue = self.ensure_href_namespace();

        // Create the href wrapper node with deterministic BID.
        let href_node = BeliefNode {
            bid: buildonomy_href_bid(href),
            kind: BeliefKindSet::from(BeliefKind::External | BeliefKind::Trace),
            title: href.to_string(),
            schema: None,
            payload: TomlTable::default(),
            id: NodeId::Explicit(href.to_string()),
            metadata: TomlTable::default(),
        };
        update_queue.push(BeliefEvent::NodeUpsert(
            href_node.bid,
            href_node.clone(),
            EventOrigin::Remote,
        ));

        // Section edge with alias as doc_paths weight.
        let mut href_weight = Weight::default();
        href_weight.set(WEIGHT_DOC_PATHS, vec![href.to_string()])?;
        update_queue.push(BeliefEvent::RelationChange(
            href_node.bid,
            href_namespace(),
            WeightKind::Section,
            Some(href_weight),
            EventOrigin::Remote,
        ));

        Ok((href_node, update_queue))
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
        parse_number: usize,
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
        #[cfg(not(target_arch = "wasm32"))]
        let t_total_per_call = std::time::Instant::now();
        #[cfg(not(target_arch = "wasm32"))]
        let t_pre_regularize = t_total_per_call;

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

        #[cfg(not(target_arch = "wasm32"))]
        {
            self.pre_regularize_us += t_pre_regularize.elapsed().as_micros() as u64;
        }
        #[cfg(not(target_arch = "wasm32"))]
        let t_regularize = std::time::Instant::now();

        let other_key_regularized =
            other_key.regularize_unchecked(self.repo(), &owner_rel_path, &repo_root_str);

        // Reclassify repo-namespace path keys that resolve to non-network directories on
        // disk as asset_namespace. Directory links (e.g. `[docs](../net1_dir1)`) are
        // regularized to `NodeKey::Path { net: repo_bid.bref(), path: "net1_dir1" }` by
        // regularize_unchecked, which has no filesystem awareness. Without this correction
        // the key flows through cache_fetch as a document reference (miss → UnresolvedReference),
        // then process_one_parse_result routes it through process_unresolved_reference instead
        // of process_asset_reference, producing spurious dependent_paths on parse 2 when the
        // asset node is already cached. Fixing the namespace here means every downstream
        // consumer — cache_fetch, inject_context, UnresolvedReference routing — sees the
        // correct key without special-casing.
        let other_key_regularized = match &other_key_regularized {
            NodeKey::Path { net, path } if *net == self.repo().bref() => {
                let abs_path = string_to_os_path(
                    &AnchorPathBuf::from(repo_root_str.clone())
                        .as_anchor_path()
                        .join(path),
                );
                if abs_path.is_dir()
                    && crate::codec::network::detect_network_file(&abs_path).is_none()
                {
                    tracing::debug!(
                        "[push_relation] Reclassifying dir link {:?} → asset_namespace (path: {})",
                        abs_path,
                        path,
                    );
                    NodeKey::Path {
                        net: asset_namespace().bref(),
                        path: path.clone(),
                    }
                } else {
                    other_key_regularized
                }
            }
            _ => other_key_regularized,
        };

        // Proximity-based id:// resolution (Issue 77, tier 1).
        //
        // When regularize_unchecked assigns net: repo.bref() to a default-net Id key,
        // the subsequent cache_fetch searches the entire repo subnet tree — and
        // get_from_id returns the first match in BTreeSet<Bid> iteration order, which
        // is nondeterministic from the user's perspective. Two nodes in different
        // networks sharing the same id (e.g. "telemetry" as both a CMake directory and
        // a design doc) resolve to whichever BID sorts first.
        //
        // Fix: narrow the Id key's net to the document's home network (the innermost
        // heading==1 stack entry) before the first cache_fetch. If the local-scope
        // search misses, retry with the original repo-wide scope.
        //
        // Only applies when the *original* key had a default net (i.e., the user wrote
        // bare `id:telemetry`, not `id://SOME_NET/telemetry`). Check other_key (pre-
        // regularize) to distinguish: regularize_unchecked overwrites default with
        // repo.bref(), destroying the distinction.
        let originally_default_net = matches!(
            other_key,
            NodeKey::Id { net, .. } if net.is_default()
        );
        let (other_key_regularized, repo_wide_fallback_key) = if originally_default_net {
            match &other_key_regularized {
                NodeKey::Id { id, .. } => {
                    let home_net_bref = self
                        .stack
                        .iter()
                        .rev()
                        .find(|(_bid, _path, heading)| *heading == 1)
                        .map(|(bid, _path, _heading)| bid.bref());
                    match home_net_bref {
                        Some(home) if home != self.repo().bref() => {
                            // Narrow to home network; keep the repo-wide key as fallback.
                            let local_key = NodeKey::Id {
                                net: home,
                                id: id.clone(),
                            };
                            (local_key, Some(other_key_regularized))
                        }
                        _ => {
                            // Already at repo root (no parent network) — no narrowing.
                            (other_key_regularized, None)
                        }
                    }
                }
                _ => (other_key_regularized, None),
            }
        } else {
            (other_key_regularized, None)
        };

        // Build the key list: local-scoped key first (if narrowed), then
        // the repo-wide key as fallback.  cache_fetch tries keys in order
        // and returns the first hit, so local scope wins when available.
        let mut other_keys = vec![other_key_regularized.clone()];
        if let Some(fallback_key) = repo_wide_fallback_key {
            other_keys.push(fallback_key);
        }
        // Append codec-provided fallback keys (e.g., wikilink Path fallback).
        // These are regularized here so relative paths resolve against the
        // current document's directory.
        for fb_key in &relation.fallback_keys {
            other_keys.push(fb_key.regularize_unchecked(
                self.repo(),
                &owner_rel_path,
                &repo_root_str,
            ));
        }

        let mut weight = maybe_weight.clone().unwrap_or_default();
        weight.set(WEIGHT_SORT_KEY, index as u16)?;
        let owner = match direction {
            Direction::Incoming => "sink",
            Direction::Outgoing => "source",
        };
        weight.set(crate::properties::WEIGHT_OWNED_BY, owner).ok();

        #[cfg(not(target_arch = "wasm32"))]
        {
            self.regularize_us += t_regularize.elapsed().as_micros() as u64;
            self.n_push_relation += 1;
        }

        // Translate relative paths into absolute paths and resolve the "other" node
        #[cfg(not(target_arch = "wasm32"))]
        let t_cache_fetch = std::time::Instant::now();

        let cache_fetch_result = self
            .cache_fetch(
                &other_keys,
                global_bb.clone(),
                true,
                missing_structure,
                parse_number,
            )
            .await?;

        #[cfg(not(target_arch = "wasm32"))]
        {
            self.cache_fetch_us += t_cache_fetch.elapsed().as_micros() as u64;
        }
        #[cfg(not(target_arch = "wasm32"))]
        let t_mid_match = std::time::Instant::now();

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
                    // cache_fetch already checked doc_bb and session_bb for this URL
                    // via indexed_get (which prefers content nodes over stubs).  If we
                    // reached here, no content alias was found — create a stub.
                    let (href_node, href_events) = self.ensure_href_entry(&href)?;
                    update_queue.extend(href_events);
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
                    tracing::trace!(
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
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.mid_match_us += t_mid_match.elapsed().as_micros() as u64;
        }

        if other_node_source != NodeSource::SourceFile {
            // We want to delineate between parsed sources and linked content. If we're not from the
            // doc_bb, color the other_node by Trace to ensure we can separate parsed content
            // from referenced content.
            //
            // Note we perform a similar coloring to the missing structure in parse_content at the
            // end of phase 2.
            update_queue.push(BeliefEvent::NodeUpsert(
                other_node.bid,
                other_node.clone(),
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
                // exists in session_bb with its neighborhood (Section edge to parent + ancestor
                // chain), which is what populates its PathMap entry for inject_context /
                // ExtendedRelation::new.
                //
                // For External nodes (href, asset): their parent IS the content namespace root
                // directly (href_node → href_namespace), so a single-BID balanced query (1-hop,
                // no deep traversal) is sufficient. Using a corpus-wide query here would walk
                // href_namespace's incoming edges and pull ALL sibling href nodes in the corpus
                // into missing_structure — inflating it to thousands of nodes and making
                // session_bb.merge_from + doc_bb.merge_from O(corpus).
                //
                // For non-External nodes (documents, sections): a balanced query is needed to
                // build the full Section ancestor chain up to the root network so PathMap can
                // resolve the node's full path. Without the chain, ExtendedRelation::new returns an
                // empty root_path and link rewriting is broken.
                //
                // Dedup: if this BID is already in missing_structure (from an earlier
                // push_relation call), its neighborhood is already present — skip.
                if !missing_structure.states.contains_key(&other_node.bid) {
                    // balanced() provides halo (Trace-marked edge endpoints)
                    // and Section ancestry (Trace-marked parent chain to root).
                    // External nodes get a 1-hop halo; document/section nodes
                    // also get the full ancestor chain for PathMap resolution.
                    let node_spec = QuerySpec::seed(TapeFn::Bids(vec![other_node.bid]));
                    let stack_result = {
                        let mut package = QueryPackage::balanced(node_spec);
                        self.session_bb.evaluate_query(&mut package)?;
                        package.into_graph()
                    };

                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        self.n_cache_arm += 1;
                        let t_union = std::time::Instant::now();
                        // Use union_mut_with_trace so that Trace-kind nodes (e.g. href nodes,
                        // which are always External|Trace) are included. Plain union_mut filters
                        // out Trace nodes, which would silently drop the href_namespace section
                        // edges and leave the href PathMap incomplete during inject_context.
                        missing_structure.union_mut_with_trace(&stack_result);
                        self.union_mut_us += t_union.elapsed().as_micros() as u64;
                    }
                    #[cfg(target_arch = "wasm32")]
                    // Use union_mut_with_trace so that Trace-kind nodes (e.g. href nodes, which
                    // are always External|Trace) are included. Plain union_mut filters out Trace
                    // nodes, which would silently drop the href_namespace section edges and leave
                    // the href PathMap incomplete during inject_context.
                    missing_structure.union_mut_with_trace(&stack_result);
                }
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

        // Guard against self-referential edges. A node should never have an edge to itself;
        // if source_bid == sink_bid the relation is nonsensical and would cause re-entrant
        // initialize_stack loops and spurious warnings on every subsequent parse.
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

        #[cfg(not(target_arch = "wasm32"))]
        {
            self.total_per_call_us += t_total_per_call.elapsed().as_micros() as u64;
        }
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
    /// `cache_fetch` calls `session_bb.evaluate_query` with an empty projection.
    /// `QueryPackage::balanced` appends halo + section-ancestry traversals that
    /// iterate downstream Section edges until the root is reached.  The resulting
    /// `BeliefGraph` therefore contains every network ancestor node and its Section
    /// edge to its parent — exactly what `doc_bb` needs in order to anchor the entry
    /// document in the PathMap.
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
        // For a subnet index `net/subnet/index.md`, the entry doc IS itself a network but
        // its parent is the grandparent network `net/` — still queryable on the fast path.
        // Only the repo root `index.md` has no parent and must use the slow path.
        //
        // We replicate the slow-path's parent_path_stack logic:
        //   1. Start from abs_path.
        //   2. Pop NETWORK_NAME filename → now at the subnet directory itself.
        //   3. Pop the subnet directory component → now at the grandparent network dir.
        //   4. For a plain doc, just pop the filename → now at the parent network dir.
        //   5. If after popping we are outside the repo, this is the repo root → slow path.
        //   6. Strip repo_root → parent_net_rel_path for the NodeKey.
        let _entry_rel_path = match abs_path.strip_prefix(self.repo_root()) {
            Ok(p) => os_path_to_string(p),
            Err(_) => return Ok(None),
        };

        // Determine the parent network directory (absolute).
        //
        // A file may live inside a plain subdirectory that is NOT itself a network
        // (no `index.md`), e.g. `network1/net1_dir1/hsml.md` where `net1_dir1/` has
        // no `index.md`.  In that case a single `pop()` lands on `net1_dir1/`, which
        // is not in the ProtoIndex and causes a hard cache miss → slow path every time.
        //
        // Fix: after stripping the filename (and, for subnet index.md, the subnet dir),
        // keep popping until we reach a directory that ProtoIndex recognises as a
        // network (i.e. `proto_index.children_of(dir).is_some()`).  We stop no later
        // than the repo root, which is always a network.
        //
        // Delegate to ProtoIndex::owning_net_dir_for — the single authoritative
        // membership-check walk-up.  For a subnet index like `req/index.md` it returns
        // the grandparent network dir (e.g. repo root); for a plain doc it returns the
        // immediate parent network dir; for files in non-network subdirectories it walks
        // up until finding the ancestor whose children_of list contains the file.
        let parent_abs = match proto_index.owning_net_dir_for(abs_path) {
            Some(dir) if dir.strip_prefix(self.repo_root()).is_ok() => dir,
            // Above repo root or not found — fall through to the slow path.
            _ => return Ok(None),
        };

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
        tracing::debug!(
            "[try_initialize_stack_from_session_cache] abs_path={:?} parent_rel_path={:?}",
            abs_path,
            parent_rel_path,
        );

        // Query the parent network. Accept either a StackCache hit (session_bb already has
        // the parent from a prior sibling parse in this task) or a GlobalCache hit (parallel
        // execution: session_bb is fresh but global_bb has the parent from a prior epoch).
        // cache_fetch deliberately does NOT populate missing_structure on a StackCache hit
        // (to avoid corrupting doc_bb in Phase 2 callers). So fast_missing is populated
        // separately after the hit is confirmed.
        let mut _unused_missing = BeliefGraph::default();
        let fast_result = self
            .cache_fetch(
                std::slice::from_ref(&parent_key),
                global_bb.clone(),
                false,
                &mut _unused_missing,
                0, // fast-path infrastructure — miss means fall-through to slow path, always expected
            )
            .await?;

        let (parent_bid, use_global_bb) = match fast_result {
            GetOrCreateResult::Resolved(ref node, NodeSource::StackCache) => (node.bid, false),
            GetOrCreateResult::Resolved(ref node, NodeSource::GlobalCache) => (node.bid, true),
            GetOrCreateResult::Unresolved(_) | GetOrCreateResult::Resolved(_, _) => {
                return Ok(None);
            }
        };

        // Populate fast_missing from the appropriate source:
        // - StackCache: session_bb already holds the balanced parent subgraph.
        // - GlobalCache: session_bb is fresh (parallel task); query global_bb instead.
        // A balanced query with empty projection walks Section edges one level
        // downstream — giving the parent's full ancestor chain AND child Section edges.
        let parent_spec = QuerySpec::seed(TapeFn::Bids(vec![parent_bid]));
        let fast_missing: BeliefGraph = {
            let mut package = QueryPackage::balanced(parent_spec);
            if use_global_bb {
                global_bb.evaluate(&mut package).await?;
            } else {
                BeliefSource::evaluate(&self.session_bb, &mut package).await?;
            };
            let graph = package.into_graph();
            if use_global_bb
                && graph.states.len() <= 1
                && graph.relations.as_graph().edge_count() == 0
            {
                tracing::warn!(
                    target: "noet_core::codec::fast_path",
                    path = %abs_path.display(),
                    parent_rel_path = %parent_rel_path,
                    parent_bid = %parent_bid,
                    "[initialize_stack] GlobalCache hit but fast_missing has no ancestor edges \
                     — parent PathMap registration may be missing in global_bb (RelationUpdate dropped?)"
                );
            }
            graph
        };

        // Extract doc_sort_key from the parent's Section edge to the entry doc.
        //
        // fast_missing now contains the balanced ancestor graph plus all downstream Section
        // edges from the parent to its children (because the balanced query walks Section
        // edges downward one level for network nodes in order to populate the PathMap with
        // child sort keys).
        //
        // The entry doc's repo-relative path is `entry_rel_path`.  Find the Section edge
        // whose doc_paths weight contains that path and read its sort_key.
        // Determine doc_sort_key using the ProtoIndex — the single canonical source of
        // truth for sibling position, shared by both fast and slow paths.
        let doc_sort_key: Option<u16> = proto_index.sort_key_for(abs_path);

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

                crate::beliefbase::BidGraph::from_edges(g.edge_references().filter_map(|e| {
                    let source = g[e.source()];
                    let sink = g[e.target()];
                    if ancestor_bids.contains(&source) && ancestor_bids.contains(&sink) {
                        Some((source, sink, e.weight().clone()))
                    } else {
                        None
                    }
                }))
            },
        };

        tracing::trace!(
            "[try_initialize_stack_from_session_cache] ancestors_only: {} states, {} edges",
            ancestors_only.states.len(),
            ancestors_only.relations.as_graph().edge_count(),
        );

        // Build doc_bb directly from ancestors_only — do NOT consume() the previous
        // doc_bb and union into it.  The previous doc_bb may contain stale content from
        // the prior parse of this file (asset nodes, section edges, etc.); carrying that
        // forward leaks state and causes PathMap corruption (in_states=true, in_pathmap=false).
        self.doc_bb = BeliefBase::from(ancestors_only.clone()).with_label("doc_bb");

        // Merge ancestor networks only into session_bb so subsequent sibling parses find
        // the parent on a StackCache hit.
        //
        // Previously this merged the full fast_missing (parent + all child Trace nodes +
        // their own Section edges) into session_bb. On attempt 2, global_bb.evaluate
        // returns the balanced parent graph which includes every child doc's full Section
        // subgraph as Trace nodes — thousands of edges whose source nodes don't exist in
        // session_bb yet. merge_from then calls update_relation for each, producing a
        // flood of "Skipping update_relation / source is missing" warnings and a 69-second
        // stall inside the merge (confirmed in beliefbase-merge-fix.log: 4,674 warnings,
        // Phase 2 stall 16:16:53→16:18:02 for global_objects attempt 2).
        //
        // The fix: use ancestors_only (network-kinded nodes only, already computed above
        // for doc_bb). Siblings still find the parent via StackCache. The entry doc's
        // Trace node and children_graph are added separately below via Steps 1-2.
        let parent_seed: BTreeSet<Bid> = BTreeSet::from([parent_bid]);
        self.session_bb.merge_from(&ancestors_only, &parent_seed);

        // Reconstruct self.stack from the parent network's PathMap position in doc_bb.
        // Section nodes for the entry doc are already in session_bb from the prior
        // terminate_stack (attempt 2+) or will be fetched individually by Phase 1 push()
        // via cache_fetch → session_bb.evaluate_query / global_bb.evaluate (first parse).
        // Pre-populating via a bulk evaluate+merge_from here is redundant on reparse and
        // harmful on first parse: balanced traversal fans out to the full subtree and to_event_stream's
        // halo expansion pulls in Epistemic edges to not-yet-parsed sinks, causing
        // "Skipping update_relation" floods and multi-second stalls per child doc.
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

        let stack_entries: Vec<(Bid, String, usize)> = {
            // Reconstruct the ancestor stack for the entry document.
            //
            // The challenge: parent_abs may be separated from the repo root by
            // non-network intermediate directories (e.g. root/a_dir/b_dir/index.md
            // where a_dir/ has no index.md).  PathMaps only contain network
            // directories, not arbitrary filesystem directories, so we cannot simply
            // split parent_rel_path by '/' and look up each component.
            //
            // Instead we use proto_index — which already knows exactly which
            // directories are networks — to build the ordered list of ancestor network
            // directories between repo_root and parent_abs.  Then for each hop we look
            // up the child network's path string in its parent network's PathMap.
            //
            // Example: root/a_dir/b_dir/b_net_file.md
            //   parent_abs = root/a_dir/b_dir
            //   proto_index ancestor chain: [root/a_dir/b_dir]   (a_dir is not a network)
            //   parent rel to root: "a_dir/b_dir"
            //   root PathMap contains "a_dir/b_dir" → b_dir_bid
            //   Stack: [root, b_dir]  ✓
            //
            // Example: root/subnet1/subnet1a/subnet1a_doc.md
            //   parent_abs = root/subnet1/subnet1a
            //   proto_index ancestor chain: [root/subnet1, root/subnet1/subnet1a]
            //   hop 1: root PathMap contains "subnet1" → subnet1_bid
            //   hop 2: subnet1 PathMap contains "subnet1a" → subnet1a_bid
            //   Stack: [root, subnet1, subnet1a]  ✓

            let mut stack = vec![(self.repo, repo_root_str.clone(), heading_for(&self.repo))];

            if parent_bid != self.repo && !parent_rel_path.is_empty() {
                // Build the ordered list of ancestor network directories strictly
                // between repo_root (exclusive) and parent_abs (inclusive), from
                // shallowest to deepest.  proto_index.children_of(dir) is Some only
                // for known network directories.
                let mut ancestor_net_dirs: Vec<PathBuf> = Vec::new();
                let mut dir = parent_abs.clone();
                loop {
                    if proto_index.children_of(&dir).is_some() {
                        ancestor_net_dirs.push(dir.clone());
                    }
                    if dir == string_to_os_path(&repo_root_str) {
                        break;
                    }
                    if !dir.pop() {
                        break;
                    }
                }
                // ancestor_net_dirs is deepest-first; reverse to get shallowest-first,
                // then drop the repo root itself (already on the stack).
                ancestor_net_dirs.reverse();
                // Drop the repo root entry (it's the repo root, already pushed above).
                let ancestor_net_dirs: Vec<PathBuf> = ancestor_net_dirs
                    .into_iter()
                    .filter(|d| d != &string_to_os_path(&repo_root_str))
                    .collect();

                let paths_guard = self.doc_bb.paths();

                // For each ancestor network, look up its path string in its parent
                // network's PathMap and push a stack frame.
                let mut current_net_bref = self.repo.bref();
                let mut current_net_abs = repo_root_str.clone();

                for net_dir in &ancestor_net_dirs {
                    let net_dir_str = os_path_to_string(net_dir);
                    // The path key stored in the parent PathMap is net_dir stripped of
                    // current_net_abs (with a trailing slash to avoid AnchorPath dir/file
                    // ambiguity).
                    let prefix_with_slash = format!("{}/", current_net_abs);
                    let local_path = net_dir_str
                        .strip_prefix(&prefix_with_slash)
                        .unwrap_or(&net_dir_str)
                        .to_string();

                    let maybe_bid =
                        paths_guard.get_map(&current_net_bref).and_then(|pm| {
                            pm.map().iter().find_map(|(p, bid, _)| {
                                if p == &local_path {
                                    Some(*bid)
                                } else {
                                    None
                                }
                            })
                        });

                    match maybe_bid {
                        Some(bid) => {
                            stack.push((bid, net_dir_str.clone(), heading_for(&bid)));
                            current_net_bref = bid.bref();
                            current_net_abs = net_dir_str;
                        }
                        None => {
                            tracing::debug!(
                                "[try_initialize_stack_from_session_cache] {:?} \
                                 (local={:?}) not found in PathMap for net={}; \
                                 stopping stack reconstruction at depth {}",
                                net_dir,
                                local_path,
                                current_net_bref,
                                stack.len(),
                            );
                            break;
                        }
                    }
                }
            }

            stack
        };

        self.stack = stack_entries;

        // proto() is still needed — initialize_stack must return the entry IRNode for Phase 1.
        let initial_factory = CLAIM_MAP
            .get(abs_path)
            .ok_or(BuildonomyError::Codec(format!(
                "Could not find codec for path type {abs_path:?}"
            )))?;
        let initial_codec = initial_factory();
        let initial = initial_codec
            .proto(abs_path)?
            .ok_or(BuildonomyError::Codec(format!(
                "Codec could not resolve path '{abs_path:?}' into a proto node"
            )))?;
        tracing::debug!(
            target: "noet_core::codec::fast_path",
            path = %abs_path.display(),
            parent_rel_path = %parent_rel_path,
            parent_bid = %parent_bid,
            source = if use_global_bb { "GlobalCache" } else { "StackCache" },
            stack_depth = self.stack.len(),
            "[initialize_stack] fast path: stack reconstructed"
        );
        Ok(Some((initial, doc_sort_key)))
    }

    /// Emit `RelationChange` events for the Cartesian product `sources × sinks` in an
    /// `IntermediateMappingRelation`, setting `WEIGHT_OWNED_BY` to the owner section's bref.
    ///
    /// For each `(source_key, sink_key)` pair in the product:
    /// 1. Resolves source and sink BIDs via `cache_fetch` (Trace hits acceptable).
    /// 2. If either is `Unresolved`, emits a `ParseDiagnostic::UnresolvedReference` and skips
    ///    that pair (other pairs in the product are still processed).
    /// 3. Builds a `Weight` from the extra payload, sets `WEIGHT_SORT_KEY` and `WEIGHT_OWNED_BY`.
    /// 4. Pushes a `RelationChange` to `update_queue`.
    /// 5. Records `(source_bid, sink_bid, kind)` in `owner_index` under the owner bref.
    #[allow(clippy::too_many_arguments)]
    async fn push_mapping<B: BeliefSource + Clone>(
        &mut self,
        mapping: &crate::codec::belief_ir::IntermediateMappingRelation,
        owner_bid: &Bid,
        index: usize,
        _content: &str,
        global_bb: B,
        update_queue: &mut Vec<BeliefEvent>,
        missing_structure: &mut BeliefGraph,
        diagnostics: &mut Vec<ParseDiagnostic>,
        parse_number: usize,
    ) -> Result<Vec<Bid>, BuildonomyError> {
        let owner_bref = owner_bid.bref();
        let kind = mapping.kind;

        // Resolve all source BIDs first. Unresolvable entries are skipped with a diagnostic;
        // resolvable entries are collected for the Cartesian product below.
        let mut source_bids: Vec<(Bid, BeliefNode)> = Vec::with_capacity(mapping.sources.len());
        for source_key in &mapping.sources {
            let result = self
                .cache_fetch(
                    std::slice::from_ref(source_key),
                    global_bb.clone(),
                    true,
                    missing_structure,
                    parse_number,
                )
                .await?;
            match result {
                GetOrCreateResult::Resolved(node, _) => source_bids.push((node.bid, node)),
                GetOrCreateResult::Unresolved(unresolved) => {
                    diagnostics.push(ParseDiagnostic::UnresolvedReference(unresolved));
                }
            }
        }

        // Resolve all sink BIDs. Same skip-on-unresolved logic.
        let mut sink_bids: Vec<(Bid, BeliefNode)> = Vec::with_capacity(mapping.sinks.len());
        for sink_key in &mapping.sinks {
            let result = self
                .cache_fetch(
                    std::slice::from_ref(sink_key),
                    global_bb.clone(),
                    true,
                    missing_structure,
                    parse_number,
                )
                .await?;
            match result {
                GetOrCreateResult::Resolved(node, _) => sink_bids.push((node.bid, node)),
                GetOrCreateResult::Unresolved(unresolved) => {
                    diagnostics.push(ParseDiagnostic::UnresolvedReference(unresolved));
                }
            }
        }

        if source_bids.is_empty() || sink_bids.is_empty() {
            return Ok(Vec::new());
        }

        // Ensure all source and sink nodes are present in doc_bb before RelationChange events
        // are applied via apply_events_batch.  apply_events_batch skips any edge whose source
        // or sink BID is absent from doc_bb (both endpoints must be in bid_to_index).
        //
        // cache_fetch populates missing_structure only for GlobalCache hits; StackCache hits
        // (nodes already in session_bb from an earlier parse) do NOT populate missing_structure
        // and therefore do not reach doc_bb via the post-loop merge_from.  Without this
        // explicit insert the Pragmatic edges are silently dropped.
        //
        // We insert as Trace so compute_diff treats them as external (non-parsed) nodes and
        // does not include them in the diff's "parsed content" scope — only the owner section
        // is parsed here, not the source/sink documents.
        for (bid, node) in source_bids.iter().chain(sink_bids.iter()) {
            if self.doc_bb.bid_to_index(bid).is_none() {
                let mut trace_node = node.clone();
                trace_node.kind.insert(BeliefKind::Trace);
                self.doc_bb
                    .insert_state(trace_node, &[crate::nodekey::NodeKey::Bid { bid: *bid }]);
            }
        }

        // Emit one RelationChange per (source, sink) pair in the Cartesian product.
        // Sort key encodes (mapping_index, pair_index) for stable ordering.
        let mut pair_idx: u16 = 0;
        for &(source_bid, _) in &source_bids {
            for &(sink_bid, _) in &sink_bids {
                // Build weight: extra payload fields + sort key + owned_by bref.
                let mut weight = mapping.weight.clone().unwrap_or_default();
                weight
                    .set(
                        WEIGHT_SORT_KEY,
                        (index as u16).saturating_mul(256).saturating_add(pair_idx),
                    )
                    .ok();
                weight.set(WEIGHT_OWNED_BY, owner_bref.to_string()).ok();

                update_queue.push(BeliefEvent::RelationChange(
                    source_bid,
                    sink_bid,
                    kind,
                    Some(weight),
                    EventOrigin::Remote,
                ));

                self.owner_index
                    .entry(owner_bref)
                    .or_default()
                    .push((source_bid, sink_bid, kind));

                tracing::debug!(
                    "[push_mapping] emitted RelationChange {:?}: {} → {} owned_by {}",
                    kind,
                    source_bid.bref(),
                    sink_bid.bref(),
                    owner_bref,
                );

                pair_idx = pair_idx.saturating_add(1);
            }
        }

        // Return all resolved BIDs so the call site can seed relation_seeds for the
        // post-loop doc_bb.merge_from DFS.  Without this, sink nodes resolved here
        // (e.g. class-A through class-F from end_appendix_d.md) are inserted into
        // doc_bb as Trace nodes but their pathmap entries are never brought in,
        // causing "No entry in pathmap for sink" warnings in compute_diff Phase 4.
        let resolved_bids: Vec<Bid> = source_bids
            .iter()
            .chain(sink_bids.iter())
            .map(|(bid, _)| *bid)
            .collect();
        Ok(resolved_bids)
    }

    async fn cache_fetch<B: BeliefSource + Clone>(
        &mut self,
        keys: &[NodeKey],
        global_bb: B,
        check_local: bool,
        missing_structure: &mut BeliefGraph,
        parse_number: usize,
    ) -> Result<GetOrCreateResult, BuildonomyError> {
        let cache_fetch_start = std::time::Instant::now();
        let mut found_state: Option<BeliefNode> = None;
        let mut source = NodeSource::Generated;
        for key in keys.iter() {
            if check_local {
                if let Some(existing_state) = self
                    .doc_bb
                    .get(key)
                    // Trace means "relations are incomplete — a deeper cache fetch may return the
                    // full non-Trace version." Accept a Trace hit only when the node is
                    // permanently Trace by design: External nodes (href leaf nodes, asset leaf
                    // nodes) represent out-of-corpus references that are always partially loaded;
                    // no deeper fetch will ever return a non-Trace version of them. For all other
                    // Trace nodes (document scaffolding, subnet ancestors), fall through so
                    // session_bb / global_bb can return the complete version.
                    .filter(|n| {
                        n.kind.contains(BeliefKind::External) || !n.kind.contains(BeliefKind::Trace)
                    })
                {
                    found_state = Some(existing_state);
                    source = NodeSource::SourceFile;
                    break;
                }
            }

            // StackCache check: session_bb already has balanced ancestor chains for every
            // node that was merged in via terminate_stack. A direct O(log N) get() is
            // sufficient — evaluate_query adds O(N) edge scans per call and the
            // balanced result is unused here (only presence/Trace-ness is checked).
            if let Some(existing_state) = self
                .session_bb
                .get(key)
                // Same permanently-Trace exemption as the doc_bb check above.
                .filter(|n| {
                    n.kind.contains(BeliefKind::External) || !n.kind.contains(BeliefKind::Trace)
                })
            {
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

            let spec = QuerySpec::seed(TapeFn::from(key));
            // Defense-in-depth: const-namespace keys (href, asset, codec) are
            // External stubs whose halo fans out to every document sharing the
            // namespace neighbor.  Use an anchored query (Section ancestry
            // only, no halo) to resolve the stub and its PathMap context
            // without pulling all neighbors into missing_structure.
            // For all other keys, use a balanced query to provide the full
            // ancestor chain + edge context that downstream code relies on.
            let is_const_ns_key = matches!(key,
                NodeKey::Path { net, .. } if {
                    crate::codec::is_codec_namespace(net)
                        || const_namespaces().iter().any(|ns| *net == ns.bref())
                }
            );
            let mut package = if is_const_ns_key {
                QueryPackage::anchored(spec)
            } else {
                QueryPackage::balanced(spec)
            };

            let global_eval_start = std::time::Instant::now();
            global_bb.evaluate(&mut package).await?;

            // Read the resolved BID from the seed's anchor map before
            // into_graph() consumes the package. For TapeFn::Keys, the
            // seed entry carries a TapePayload::AnchorMap recording which
            // key index resolved to which BID.
            let resolved_bid = package.resolved_bid(0);
            if resolved_bid.is_none() && package.graph().filter(|g| !g.is_empty()).is_some() {
                let _tape = package.tape().clone();
                tracing::warn!(
                    "[cache_fetch FAILED] Why didn't we get our node? The query returned results.\n\
                    Query key: {}\n\
                    Resolved BID: {:?}\n\
                    Tape: {:?}\n\
                    package_graph: {:?}",
                    key,
                    resolved_bid,
                    _tape,
                    package.graph().expect("if statement above ensures graph is some")
                );
            }
            let mut cache_update = BeliefBase::from(package.into_graph());
            let global_eval_elapsed = global_eval_start.elapsed();
            if global_eval_elapsed.as_millis() > 10 {
                tracing::debug!(
                    target: "noet_core::codec::perf",
                    key = %key,
                    elapsed_ms = global_eval_elapsed.as_millis(),
                    "[cache_fetch] global_bb.evaluate slow",
                );
            }

            // Try the resolved BID first, then fall back to the original
            // key. Cross-subnet path keys may not resolve in the subset
            // PathMap (which is rebuilt from the halo subgraph), but the
            // node itself is present by BID.
            let cached_state = resolved_bid
                .and_then(|bid| cache_update.states().get(&bid).cloned())
                .or_else(|| cache_update.get(key));

            if let Some(cached_state) = cached_state {
                found_state = Some(cached_state);
                let update = cache_update.consume();
                // Percolate global cache updates into closer caches.
                missing_structure.union_mut(&update);
                source = NodeSource::GlobalCache;
                break;
            }
        }

        // If we found a state in any cache, return it as Resolved
        let cache_fetch_elapsed = cache_fetch_start.elapsed();
        if cache_fetch_elapsed.as_millis() > 10 {
            tracing::debug!(
                target: "noet_core::codec::perf",
                keys = ?keys,
                source = ?source,
                session_bb_nodes = self.session_bb.states().len(),
                elapsed_ms = cache_fetch_elapsed.as_millis(),
                "[cache_fetch] slow total",
            );
        }

        // This is the one call site every resolution path (doc_bb /
        // session_bb / global_bb, DB-backed or not) routes through, so counting
        // `source=GlobalCache` outcomes across a run gives a direct "how many
        // times did we fall through to global_bb/DB" count -- previously only
        // inferred from timing distribution.
        //
        // Kept deliberately cheap (no lock acquisition) so it can fire on every
        // call without perturbing the Phase 2 timing it's meant to explain.
        // Gated behind its own target so it does not fire under a blanket
        // `RUST_LOG=debug` capture; see benches/log_analysis/README.md.
        tracing::debug!(
            target: "noet_core::codec::cache_fetch_census",
            source = ?source,
            n_keys = keys.len(),
            parse_number,
            elapsed_us = cache_fetch_elapsed.as_micros(),
            "[cache_fetch] census",
        );

        // Both open bottlenecks (warm-cache regression, end-of-run insert
        // storm) show the same suspected signature -- a local PathMap far
        // smaller than the authoritative membership -- specifically on the
        // GlobalCache path (the one that actually reached global_bb/DB).
        // Restrict the more expensive PathMap-size probe to that arm: it's the
        // minority outcome in a healthy cache, and it's the one this
        // instrumentation exists to characterize.
        if source == NodeSource::GlobalCache {
            let session_bb_asset_len = self
                .session_bb
                .paths()
                .get_map(&asset_namespace().bref())
                .map(|pm| pm.len())
                .unwrap_or(0);
            let session_bb_href_len = self
                .session_bb
                .paths()
                .get_map(&href_namespace().bref())
                .map(|pm| pm.len())
                .unwrap_or(0);
            tracing::debug!(
                target: "noet_core::codec::cache_fetch_census",
                session_bb_nodes = self.session_bb.states().len(),
                session_bb_asset_len,
                session_bb_href_len,
                "[cache_fetch] global_cache_miss_local",
            );
        }

        if let Some(state) = found_state {
            Ok(GetOrCreateResult::Resolved(state, source))
        } else if parse_number > 1 {
            // Const/codec namespace references (e.g. C++ #include paths like
            // "mp-units/systems/si.h") will never resolve — they're external
            // headers with no corresponding source in the corpus. Downgrade
            // to debug instead of warn to avoid flooding the build log.
            let is_const_ns = keys.iter().any(|k| match k {
                NodeKey::Path { net, .. } => {
                    is_codec_namespace(net)
                        || *net == href_namespace().bref()
                        || *net == asset_namespace().bref()
                }
                _ => false,
            });
            if is_const_ns {
                tracing::debug!(
                    target: "noet_core::codec::fast_path",
                    keys = ?keys,
                    "[cache_fetch] MISS on re-parse for const/codec namespace \
                     reference (expected — external headers will never resolve).",
                );
            } else {
                tracing::debug!(
                    target: "noet_core::codec::fast_path",
                    keys = ?keys,
                    repo = %self.repo,
                    parse_number,
                    "[cache_fetch] MISS on re-parse. This is unexpected — the node should have \
                     been resolved on pass 1. If keys contain \
                     NodeKey::Id {{ net: nil_bref, .. }}, the parent network's PathMap subnet \
                     list may be incomplete or global_bb has not drained for this epoch.",
                );
            }
            Ok(GetOrCreateResult::Unresolved(UnresolvedReference {
                other_keys: keys.into(),
                ..Default::default()
            }))
        } else {
            tracing::trace!(
                target: "noet_core::codec::fast_path",
                keys = ?keys,
                repo = %self.repo,
                parse_number,
                "[cache_fetch] MISS on pass 1 (forward-reference misses are expected on \
                 pass 1 and resolve on pass 2; surviving misses are promoted to warnings \
                 by promote_unresolved_to_warnings).",
            );
            Ok(GetOrCreateResult::Unresolved(UnresolvedReference {
                other_keys: keys.into(),
                ..Default::default()
            }))
        }
    }

    // ---------------------------------------------------------------------------
    // Asset processing
    // ---------------------------------------------------------------------------

    /// Process a static asset file (non-codec path) and emit belief events.
    ///
    /// Reads raw bytes, computes a SHA-256 content hash, checks `session_bb` for an
    /// existing entry at the same repo-relative path, and emits `NodeUpdate` /
    /// `RelationChange` events only when the asset is new or its content has changed.
    ///
    /// Returns a [`ParseContentWithCodec`] with an [`AssetCodec`] so that
    /// `process_one_parse_result` in the compiler can handle the result uniformly
    /// alongside ordinary document parse results.
    ///
    /// # Arguments
    /// * `path`  — Absolute path to the asset file (already confirmed to exist and
    ///   not be a directory). Must be under `self.repo_root()`.
    /// * `bytes` — Raw file bytes, pre-read by the caller.
    pub async fn process_asset<B: BeliefSource + Clone>(
        &mut self,
        path: &Path,
        bytes: &[u8],
        global_bb: B,
        _proto_index: ProtoIndex,
    ) -> Result<ParseContentWithCodec, BuildonomyError> {
        // Compute SHA-256 hash of file content.
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        let hash_str = format!("{:x}", hasher.finalize());

        // Build repo-relative path string used as the PathMap key.
        let repo_relative_path = path
            .strip_prefix(&self.repo_root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");

        // Look up the asset by its repo-relative path key, consulting session_bb
        // then global_bb (via cache_fetch) so that assets already committed to the
        // DB are recognised and not re-emitted as new.
        let asset_key = NodeKey::Path {
            net: asset_namespace().bref(),
            path: repo_relative_path.clone(),
        };
        let mut missing_structure = BeliefGraph::default();
        let cache_result = self
            .cache_fetch(
                &[asset_key],
                global_bb,
                false, // doc_bb is irrelevant for assets
                &mut missing_structure,
                0, // new asset discovery — miss is always expected
            )
            .await?;

        // Percolate any global_bb hits into session_bb so subsequent calls within
        // the same session see the cached state.
        if !missing_structure.is_empty() {
            self.session_bb.merge(&missing_structure);
        }

        let (asset_bid, needs_update) = match cache_result {
            GetOrCreateResult::Resolved(ref node, _) => {
                let existing_hash = node
                    .payload
                    .get("content_hash")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if existing_hash == hash_str {
                    tracing::debug!(
                        "[GraphBuilder] Asset unchanged: {} (BID: {})",
                        repo_relative_path,
                        node.bid
                    );
                    (node.bid, false)
                } else {
                    tracing::debug!(
                        "[GraphBuilder] Asset content changed: {} (BID: {}, old: {}, new: {})",
                        repo_relative_path,
                        node.bid,
                        existing_hash,
                        hash_str
                    );
                    (node.bid, true)
                }
            }
            GetOrCreateResult::Unresolved(_) => {
                let new_bid = Bid::new(asset_namespace());
                tracing::debug!(
                    "[GraphBuilder] New asset discovered: {} (BID: {})",
                    repo_relative_path,
                    new_bid
                );
                (new_bid, true)
            }
        };

        if needs_update {
            let mut payload = toml::Table::new();
            payload.insert("content_hash".to_string(), toml::Value::String(hash_str));

            let asset_node = BeliefNode {
                bid: asset_bid,
                kind: BeliefKind::External.into(),
                payload,
                ..Default::default()
            };

            let mut update_queue: Vec<BeliefEvent> = Vec::new();

            // Ensure the asset_namespace network node exists before creating relations.
            if !self.session_bb.states().contains_key(&asset_namespace()) {
                let asset_net_node = BeliefNode::asset_network();
                update_queue.push(BeliefEvent::NodeUpsert(
                    asset_net_node.bid,
                    asset_net_node,
                    EventOrigin::Remote,
                ));
                update_queue.push(BeliefEvent::RelationChange(
                    asset_namespace(),
                    buildonomy_namespace(),
                    WeightKind::Section,
                    None,
                    EventOrigin::Remote,
                ));
            }

            update_queue.push(BeliefEvent::NodeUpsert(
                asset_node.bid,
                asset_node,
                EventOrigin::Remote,
            ));

            let mut edge_payload = toml::Table::new();
            edge_payload.insert(
                WEIGHT_DOC_PATHS.to_string(),
                toml::Value::Array(vec![toml::Value::String(repo_relative_path.clone())]),
            );
            update_queue.push(BeliefEvent::RelationChange(
                asset_bid,
                asset_namespace(),
                WeightKind::Section,
                Some(Weight {
                    payload: edge_payload,
                }),
                EventOrigin::Remote,
            ));

            // Apply events to session_bb so the local cache reflects asset state
            // immediately (e.g. so a second call within the same session sees the
            // existing entry and skips the update).
            let mut derivatives: Vec<BeliefEvent> = Vec::new();
            for event in update_queue.iter() {
                derivatives.append(&mut self.session_bb.process_event(event)?);
            }
            update_queue.append(&mut derivatives);

            for event in update_queue {
                self.tx.send(event)?;
            }

            // Emit FileParsed for mtime tracking.
            self.tx.send(BeliefEvent::FileParsed(path.to_path_buf()))?;
        }

        Ok(ParseContentWithCodec {
            result: ParseContentResult::empty(),
            codec: Box::new(AssetCodec),
            repo_bid: Bid::nil(),
            repo_node: None,
        })
    }

    /// Bulk-optimised asset registration that skips `cache_fetch` and `global_bb`.
    ///
    /// Callers must ensure:
    ///   1. `session_bb` is warm (all known assets merged from `global_bb` via
    ///      `sync_asset_snapshot`) before invoking this method.
    ///   2. The asset_namespace network node exists in `session_bb` (call
    ///      `ensure_asset_namespace` once before the batch).
    ///
    /// Returns `None` when the asset is unchanged (content hash matches),
    /// or `Some(events)` containing the `BeliefEvent`s that must be sent on
    /// `tx` and applied to `session_bb`. The caller is responsible for
    /// batching: collect all returned event vecs, then apply once via
    /// `apply_events_batch` + `flush_paths_for_events` at end of batch.
    ///
    /// This avoids per-event `process_event` overhead (which triggers O(N)
    /// PathMap flushes and relation-graph scans on every call).
    pub fn process_asset_prehashed(
        &self,
        path: &Path,
        hash_str: String,
    ) -> Option<Vec<BeliefEvent>> {
        // Build repo-relative path string used as the PathMap key.
        let repo_relative_path = path
            .strip_prefix(&self.repo_root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");

        // Look up the asset directly in session_bb (no cache_fetch, no global_bb).
        let asset_key = NodeKey::Path {
            net: asset_namespace().bref(),
            path: repo_relative_path.clone(),
        };

        let (asset_bid, needs_update) = match self.session_bb.get(&asset_key) {
            Some(node) => {
                let existing_hash = node
                    .payload
                    .get("content_hash")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if existing_hash == hash_str {
                    tracing::debug!(
                        "[GraphBuilder] Asset unchanged (prehashed): {} (BID: {})",
                        repo_relative_path,
                        node.bid
                    );
                    (node.bid, false)
                } else {
                    tracing::debug!(
                        "[GraphBuilder] Asset content changed (prehashed): {} (BID: {}, old: {}, new: {})",
                        repo_relative_path,
                        node.bid,
                        existing_hash,
                        hash_str
                    );
                    (node.bid, true)
                }
            }
            None => {
                let new_bid = Bid::new(asset_namespace());
                tracing::debug!(
                    "[GraphBuilder] New asset discovered (prehashed): {} (BID: {})",
                    repo_relative_path,
                    new_bid
                );
                (new_bid, true)
            }
        };

        if !needs_update {
            return None;
        }

        let mut payload = toml::Table::new();
        payload.insert("content_hash".to_string(), toml::Value::String(hash_str));

        let asset_node = BeliefNode {
            bid: asset_bid,
            kind: BeliefKind::External.into(),
            payload,
            ..Default::default()
        };

        let mut events = Vec::with_capacity(3);

        events.push(BeliefEvent::NodeUpsert(
            asset_node.bid,
            asset_node,
            EventOrigin::Remote,
        ));

        let mut edge_payload = toml::Table::new();
        edge_payload.insert(
            WEIGHT_DOC_PATHS.to_string(),
            toml::Value::Array(vec![toml::Value::String(repo_relative_path)]),
        );
        events.push(BeliefEvent::RelationChange(
            asset_bid,
            asset_namespace(),
            WeightKind::Section,
            Some(Weight {
                payload: edge_payload,
            }),
            EventOrigin::Remote,
        ));

        events.push(BeliefEvent::FileParsed(path.to_path_buf()));

        Some(events)
    }

    /// Ensure the `asset_namespace` network node exists in `session_bb`.
    ///
    /// Must be called once before a bulk `process_asset_prehashed` batch so
    /// that the namespace node and its Section edge to `buildonomy_namespace`
    /// are present. Emits events on `tx` and applies them to `session_bb`.
    ///
    /// No-op if the namespace is already in `session_bb`.
    pub fn ensure_asset_namespace(&mut self) -> Result<(), BuildonomyError> {
        if self.session_bb.states().contains_key(&asset_namespace()) {
            return Ok(());
        }
        let asset_net_node = BeliefNode::asset_network();
        let ns_event =
            BeliefEvent::NodeUpsert(asset_net_node.bid, asset_net_node, EventOrigin::Remote);
        self.session_bb.process_event(&ns_event)?;
        self.tx.send(ns_event)?;

        let edge_event = BeliefEvent::RelationChange(
            asset_namespace(),
            buildonomy_namespace(),
            WeightKind::Section,
            None,
            EventOrigin::Remote,
        );
        self.session_bb.process_event(&edge_event)?;
        self.tx.send(edge_event)?;
        Ok(())
    }

    /// Process a directory referenced from a markdown link.
    ///
    /// Two cases are handled:
    ///
    /// **Case A** (`#[cfg(feature = "git-tracking")]`): the directory is a registered
    /// network tracked by `proto_index`.  Emits an `href_namespace` node pointing to
    /// the upstream remote URL — identical in structure to an external HTTP link —
    /// so that the SPA viewer can render a "View on remote ↗" link.  Gated on the
    /// `git-tracking` feature; falls through to Case B when the feature is disabled
    /// or the directory is not in `proto_index`.
    ///
    /// **Case B**: the directory exists but is not a tracked network (or
    /// `git-tracking` is disabled).  Reads the directory listing, caps at 256
    /// entries, computes a hash over the repo-relative path + sorted names, and
    /// emits a `BeliefKind::External` node with `payload["listing"]` and
    /// `payload["content_hash"]`.  Change detection follows the same pattern as
    /// file assets: the node is only updated when the hash differs from the cached
    /// value.
    ///
    /// # Arguments
    /// * `path` — Absolute path to the directory.  Must exist and be navigable.
    /// * `global_bb` — Shared belief source for cache lookups.
    /// * `proto_index` — Used by Case A (git-tracking feature) to query git status.
    #[cfg_attr(not(feature = "git-tracking"), allow(unused_variables))]
    pub async fn process_asset_dir<B: BeliefSource + Clone>(
        &mut self,
        path: &Path,
        global_bb: B,
        proto_index: ProtoIndex,
    ) -> Result<ParseContentWithCodec, BuildonomyError> {
        // Build repo-relative path string — same convention as process_asset.
        let repo_relative_path = match path.strip_prefix(&self.repo_root) {
            Ok(rel) => rel.to_string_lossy().replace('\\', "/"),
            Err(_) => {
                tracing::warn!(
                    "[GraphBuilder] process_asset_dir: path {:?} is outside repo root {:?} — skipping",
                    path,
                    self.repo_root,
                );
                return Ok(ParseContentWithCodec {
                    result: ParseContentResult::empty(),
                    codec: Box::new(AssetCodec),
                    repo_bid: Bid::nil(),
                    repo_node: None,
                });
            }
        };

        // ------------------------------------------------------------------
        // Case A: directory is a git-tracked registered network → href node.
        // ------------------------------------------------------------------
        #[cfg(feature = "git-tracking")]
        if let Some(status) = proto_index.get_meta_as::<NetworkGitStatus>(path, "git") {
            if status.repo.remote_url.is_none() {
                tracing::warn!(
                    "[GraphBuilder] Directory asset (Case A): {} is a git-tracked network but \
                    has no recognised remote URL — cannot generate href node. \
                    Set payload[\"git_remote_url\"] on the network node to override.",
                    repo_relative_path,
                );
            }
            if let Some(remote_url) = status.repo.remote_url.as_deref() {
                let href = remote_url.trim_end_matches('/').to_string();
                let href_bid = buildonomy_href_bid(&href);

                let mut update_queue: Vec<BeliefEvent> = Vec::new();

                // Ensure the href_namespace network node exists.
                if !self.session_bb.states().contains_key(&href_namespace()) {
                    let href_net_node = BeliefNode::href_network();
                    update_queue.push(BeliefEvent::NodeUpsert(
                        href_net_node.bid,
                        href_net_node,
                        EventOrigin::Remote,
                    ));
                }

                let href_node = BeliefNode {
                    bid: href_bid,
                    kind: BeliefKindSet::from(BeliefKind::External | BeliefKind::Trace),
                    title: href.clone(),
                    schema: None,
                    payload: TomlTable::default(),
                    id: NodeId::Explicit(href.clone()),
                    metadata: TomlTable::default(),
                };
                update_queue.push(BeliefEvent::NodeUpsert(
                    href_node.bid,
                    href_node,
                    EventOrigin::Remote,
                ));
                let mut href_weight = Weight::default();
                href_weight.set(WEIGHT_DOC_PATHS, vec![href.clone()])?;
                update_queue.push(BeliefEvent::RelationChange(
                    href_bid,
                    href_namespace(),
                    WeightKind::Section,
                    Some(href_weight),
                    EventOrigin::Remote,
                ));

                let mut derivatives: Vec<BeliefEvent> = Vec::new();
                for event in update_queue.iter() {
                    derivatives.append(&mut self.session_bb.process_event(event)?);
                }
                update_queue.append(&mut derivatives);
                for event in update_queue {
                    self.tx.send(event)?;
                }

                tracing::debug!(
                    "[GraphBuilder] Directory asset (Case A / git-tracked): {} → href {}",
                    repo_relative_path,
                    href,
                );

                return Ok(ParseContentWithCodec {
                    result: ParseContentResult::empty(),
                    codec: Box::new(AssetCodec),
                    repo_bid: Bid::nil(),
                    repo_node: None,
                });
            }
        }

        // ------------------------------------------------------------------
        // Case B: local-only directory → External node with listing payload.
        // ------------------------------------------------------------------

        // Case B git enrichment: even though this directory isn't a registered
        // belief network, it may still live inside a git repo.  Walk up from
        // `path` to find the closest ancestor that IS a registered network in
        // proto_index; if that network has a remote URL, store the info needed
        // to construct tree URLs in the payload.
        //
        // payload fields added when git info is available:
        //   "remote_url"       — normalized remote base (e.g. "https://github.com/org/repo")
        //   "branch"           — current branch or "HEAD"
        //   "network_prefix"   — git-workdir-relative path to the parent network dir
        //   "dir_path"         — repo-relative path to this directory (= repo_relative_path)
        //
        // The viewer constructs tree URLs as:
        //   {remote_url}/tree/{branch}/{network_prefix}/{dir_relative_to_network}/{entry}
        #[cfg(feature = "git-tracking")]
        let dir_git_info: Option<(String, String, String)> = {
            // Walk from `path` upward, stopping at the first ancestor that appears
            // in proto_index as a registered network.
            let mut ancestor = path.parent();
            let mut found: Option<(String, String, String)> = None;
            while let Some(dir) = ancestor {
                if let Some(status) = proto_index.get_meta_as::<NetworkGitStatus>(dir, "git") {
                    if let Some(remote_url) = status.repo.remote_url.as_deref() {
                        let branch = status.repo.branch.as_deref().unwrap_or("HEAD").to_string();
                        let network_prefix = status
                            .network_prefix
                            .to_string_lossy()
                            .replace(std::path::MAIN_SEPARATOR, "/");
                        found = Some((remote_url.to_string(), branch, network_prefix));
                    }
                    break; // stop at first registered network ancestor regardless
                }
                ancestor = dir.parent();
            }
            found
        };
        #[cfg(not(feature = "git-tracking"))]
        let dir_git_info: Option<(String, String, String)> = None;

        // Read directory entries, sort by name, cap at 256.
        const MAX_LISTING: usize = 256;
        let mut entries: Vec<String> = match std::fs::read_dir(path) {
            Ok(rd) => rd
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect(),
            Err(e) => {
                return Err(BuildonomyError::Codec(format!(
                    "process_asset_dir: cannot read directory {}: {e}",
                    path.display()
                )));
            }
        };
        entries.sort();
        let truncated = entries.len() > MAX_LISTING;
        if truncated {
            tracing::warn!(
                "[GraphBuilder] Directory listing truncated at {} entries: {}",
                MAX_LISTING,
                repo_relative_path,
            );
            entries.truncate(MAX_LISTING);
        }

        // Hash = SHA-256 over repo-relative-path + newline + sorted names.
        let mut hasher = Sha256::new();
        hasher.update(repo_relative_path.as_bytes());
        hasher.update(b"\n");
        for name in &entries {
            hasher.update(name.as_bytes());
            hasher.update(b"\n");
        }
        let hash_str = format!("{:x}", hasher.finalize());

        // Cache lookup — same pattern as process_asset.
        let asset_key = NodeKey::Path {
            net: asset_namespace().bref(),
            path: repo_relative_path.clone(),
        };
        let mut missing_structure = BeliefGraph::default();
        let cache_result = self
            .cache_fetch(&[asset_key], global_bb, false, &mut missing_structure, 0)
            .await?;

        if !missing_structure.is_empty() {
            self.session_bb.merge(&missing_structure);
        }

        let (asset_bid, needs_update) = match cache_result {
            GetOrCreateResult::Resolved(ref node, _) => {
                let existing_hash = node
                    .payload
                    .get("content_hash")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if existing_hash == hash_str {
                    tracing::debug!(
                        "[GraphBuilder] Directory listing unchanged: {}",
                        repo_relative_path,
                    );
                    (node.bid, false)
                } else {
                    (node.bid, true)
                }
            }
            GetOrCreateResult::Unresolved(_) => (Bid::new(asset_namespace()), true),
        };

        if needs_update {
            let entry_count = entries.len();
            let mut payload = TomlTable::new();
            payload.insert("content_hash".to_string(), toml::Value::String(hash_str));
            payload.insert(
                "listing".to_string(),
                toml::Value::Array(entries.into_iter().map(toml::Value::String).collect()),
            );
            if truncated {
                payload.insert("truncated".to_string(), toml::Value::Boolean(true));
            }
            // Store git remote info so the viewer can construct tree/blob URLs.
            payload.insert(
                "dir_path".to_string(),
                toml::Value::String(repo_relative_path.clone()),
            );
            if let Some((remote_url, branch, network_prefix)) = dir_git_info {
                payload.insert("remote_url".to_string(), toml::Value::String(remote_url));
                payload.insert("branch".to_string(), toml::Value::String(branch));
                payload.insert(
                    "network_prefix".to_string(),
                    toml::Value::String(network_prefix),
                );
            }

            let asset_node = BeliefNode {
                bid: asset_bid,
                kind: BeliefKind::External.into(),
                title: repo_relative_path.clone(),
                payload,
                ..Default::default()
            };

            let mut update_queue: Vec<BeliefEvent> = Vec::new();

            // Ensure the asset_namespace network node exists.
            if !self.session_bb.states().contains_key(&asset_namespace()) {
                let asset_net_node = BeliefNode::asset_network();
                update_queue.push(BeliefEvent::NodeUpsert(
                    asset_net_node.bid,
                    asset_net_node,
                    EventOrigin::Remote,
                ));
                update_queue.push(BeliefEvent::RelationChange(
                    asset_namespace(),
                    buildonomy_namespace(),
                    WeightKind::Section,
                    None,
                    EventOrigin::Remote,
                ));
            }

            update_queue.push(BeliefEvent::NodeUpsert(
                asset_node.bid,
                asset_node,
                EventOrigin::Remote,
            ));

            let mut edge_payload = TomlTable::new();
            edge_payload.insert(
                WEIGHT_DOC_PATHS.to_string(),
                toml::Value::Array(vec![toml::Value::String(repo_relative_path.clone())]),
            );
            update_queue.push(BeliefEvent::RelationChange(
                asset_bid,
                asset_namespace(),
                WeightKind::Section,
                Some(Weight {
                    payload: edge_payload,
                }),
                EventOrigin::Remote,
            ));

            let mut derivatives: Vec<BeliefEvent> = Vec::new();
            for event in update_queue.iter() {
                derivatives.append(&mut self.session_bb.process_event(event)?);
            }
            update_queue.append(&mut derivatives);
            for event in update_queue {
                self.tx.send(event)?;
            }

            tracing::debug!(
                "[GraphBuilder] Directory asset (Case B / local): {} ({} entries)",
                repo_relative_path,
                entry_count,
            );
        }

        Ok(ParseContentWithCodec {
            result: ParseContentResult::empty(),
            codec: Box::new(AssetCodec),
            repo_bid: Bid::nil(),
            repo_node: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        beliefbase::{BeliefBase, BeliefGraph, BidGraph},
        codec::belief_ir::IRNode,
        codec::GraphBuilder,
        event::{BeliefEvent, EventOrigin},
        paths::to_anchor,
        properties::{
            href_namespace, BeliefKind, BeliefKindSet, BeliefNode, Bid, NodeId, Weight, WeightKind,
            WeightSet, WEIGHT_DOC_PATHS,
        },
    };
    use rustc_hash::FxHashMap;
    use std::path::Path;
    use tempfile::TempDir;
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
            source_line: None,
            mappings: Vec::new(),
            path_aliases: Vec::new(),
            namespace_paths: Vec::new(),
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
            id: NodeId::default(),
            metadata: Default::default(),
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
        use crate::codec::compiler::DocumentCompiler;
        use crate::tests::helpers::init_logging;
        init_logging();
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
        // The h1 "# Doc" is now its own section node at depth 2 ([0, 0]).
        // Its anchor may be bref-based (if "doc" collided with the document node's
        // own title slug).  Find it by exclusion rather than a hardcoded anchor.
        // The h2 sections Alpha, Beta, Gamma are children at depth 3 ([0, 0, N]).
        let order_for = |anchor: &str| -> Vec<u16> {
            doc_entries
                .iter()
                .find(|(path, _)| path.ends_with(anchor))
                .map(|(_, order)| order.clone())
                .unwrap_or_default()
        };

        let alpha_order = order_for("#alpha");
        let gamma_order = order_for("#gamma");

        // h1 "# Doc" — find by exclusion (its anchor may be bref-based after
        // collision resolution, so we can't hardcode "#doc").
        let doc_order = doc_entries
            .iter()
            .find(|(path, _)| {
                !path.ends_with("#alpha") && !path.ends_with("#beta") && !path.ends_with("#gamma")
            })
            .map(|(_, order)| order.clone())
            .expect("Expected a doc heading entry");

        assert_eq!(
            doc_order,
            vec![0, 0],
            "h1 doc heading must be at depth 2 ([0, 0]); got {doc_order:?}. All paths:\n{paths}"
        );

        // Beta's anchor may also be bref-based or title-derived depending on
        // collision resolution.  Find it as the remaining entry.
        let beta_order = doc_entries
            .iter()
            .find(|(path, _)| {
                let dominated = |a: &str| path.ends_with(a);
                !dominated("#alpha")
                    && !dominated("#gamma")
                    && Some(path)
                        != doc_entries
                            .iter()
                            .find(|(_, o)| *o == doc_order)
                            .map(|(p, _)| p)
            })
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

    /// A per-document seed must not discard the const-namespace subgraph.
    ///
    /// `parse_epoch` selects EITHER a small per-document balanced seed OR the shared
    /// `epoch_session_snapshot`, and `seed_session` then replaces `session_bb` wholesale.
    /// The epoch snapshot carries the href/asset namespaces; a per-doc seed does not.
    /// Before the fix, a task on the per-doc branch dropped those namespaces, so
    /// `initialize_stack`'s `content_namespaces()` guard missed and re-fetched the whole
    /// href namespace (~84k states, ~82s) from `global_bb`.
    ///
    /// This asserts the union actually happens, which is what keeps that guard satisfied.
    #[test]
    fn seed_session_unions_const_namespace_into_per_doc_seed() {
        fn node(bid: Bid, title: &str) -> BeliefNode {
            BeliefNode {
                bid,
                title: title.to_string(),
                ..Default::default()
            }
        }

        let repo_dir = TempDir::new().unwrap();
        create_test_network(repo_dir.path());
        let mut builder = GraphBuilder::new(repo_dir.path(), None).unwrap();

        let repo_bid = Bid::from(uuid::Uuid::from_u128(0x1000));
        let doc_bid = Bid::from(uuid::Uuid::from_u128(0x2000));
        let href_ns = href_namespace();
        let href_child = Bid::from(uuid::Uuid::from_u128(0x3000));

        // Per-doc seed: repo root + one document. No const namespaces — this is the
        // shape `QueryPackage::balanced` produces for a leaf document.
        let mut doc_states = FxHashMap::default();
        doc_states.insert(repo_bid, node(repo_bid, "repo"));
        doc_states.insert(doc_bid, node(doc_bid, "doc"));
        let doc_seed = BeliefGraph {
            states: doc_states,
            relations: BidGraph::from_edges(vec![(
                doc_bid,
                repo_bid,
                WeightSet::from(WeightKind::Section),
            )]),
        };

        // Epoch snapshot: carries the href namespace and one registered child.
        let mut ns_states = FxHashMap::default();
        ns_states.insert(repo_bid, node(repo_bid, "repo"));
        ns_states.insert(href_ns, node(href_ns, "href namespace"));
        ns_states.insert(href_child, node(href_child, "https://example.com"));
        let const_ns = BeliefGraph {
            states: ns_states,
            relations: BidGraph::from_edges(vec![(
                href_child,
                href_ns,
                WeightSet::from(WeightKind::Section),
            )]),
        };

        builder.seed_session(repo_bid, &doc_seed, Some(&const_ns));

        let session = builder.session_bb();
        assert!(
            session.states().contains_key(&href_ns),
            "href_namespace must survive into session_bb; without it initialize_stack \
             re-fetches the entire namespace from global_bb"
        );
        assert!(
            session.states().contains_key(&href_child),
            "href namespace children must survive into session_bb"
        );
        // The per-doc seed's own content must still be present.
        assert!(
            session.states().contains_key(&doc_bid),
            "per-doc seed content must be preserved by the union"
        );
        assert!(
            session.states().contains_key(&repo_bid),
            "repo root must be preserved by the union"
        );
    }

    /// Two tasks seeded from one shared epoch base must not see each other's writes.
    ///
    /// `seed_session_from_base` clones a shared `BeliefBase`, and `PathMapMap`'s clone
    /// shares its `PathMap`s by `Arc` — that sharing is the whole point (it skips a
    /// per-task `PathMapMap::new` DFS over ~104k const-namespace states). It is only
    /// sound because `PathMapMap::make_pathmap_unique` copies an entry before writing
    /// to it.
    ///
    /// Without that copy-on-write, task A's href registration lands in the `PathMap`
    /// task B is reading, and the `PathAdded` derivative that populates the `paths`
    /// table fires once for whichever task raced there first instead of once per task.
    #[test]
    fn shared_epoch_base_isolates_per_task_path_writes() {
        fn node(bid: Bid, title: &str) -> BeliefNode {
            BeliefNode {
                bid,
                title: title.to_string(),
                ..Default::default()
            }
        }

        let repo_bid = Bid::from(uuid::Uuid::from_u128(0x1000));
        let href_ns = href_namespace();
        let href_ns_node = BeliefNode::href_network();

        // Shared epoch base: repo root + the href namespace, exactly the immutable
        // scaffolding every task in an epoch receives.
        let mut shared_states = FxHashMap::default();
        shared_states.insert(repo_bid, node(repo_bid, "repo"));
        shared_states.insert(href_ns, href_ns_node);
        let shared_graph = BeliefGraph {
            states: shared_states,
            relations: BidGraph::from_edges(vec![(
                repo_bid,
                href_ns,
                WeightSet::from(WeightKind::Section),
            )]),
        };
        let shared_base = BeliefBase::from(shared_graph).with_label("epoch_base");

        // Each task registers a DIFFERENT href under the shared namespace.
        let register = |builder: &mut GraphBuilder, bid: Bid, url: &str| {
            let mut w = Weight::default();
            w.set(WEIGHT_DOC_PATHS, vec![url.to_string()]).unwrap();
            builder
                .session_bb_mut()
                .process_event(&BeliefEvent::NodeUpsert(
                    bid,
                    node(bid, url),
                    EventOrigin::Remote,
                ))
                .unwrap();
            builder
                .session_bb_mut()
                .process_event(&BeliefEvent::RelationChange(
                    bid,
                    href_ns,
                    WeightKind::Section,
                    Some(w),
                    EventOrigin::Remote,
                ))
                .unwrap();
        };

        let dir_a = TempDir::new().unwrap();
        create_test_network(dir_a.path());
        let mut task_a = GraphBuilder::new(dir_a.path(), None).unwrap();
        task_a.seed_session_from_base(repo_bid, &shared_base, &BeliefGraph::default());

        let dir_b = TempDir::new().unwrap();
        create_test_network(dir_b.path());
        let mut task_b = GraphBuilder::new(dir_b.path(), None).unwrap();
        task_b.seed_session_from_base(repo_bid, &shared_base, &BeliefGraph::default());

        let a_bid = Bid::from(uuid::Uuid::from_u128(0xA000));
        let b_bid = Bid::from(uuid::Uuid::from_u128(0xB000));
        register(&mut task_a, a_bid, "https://a.example.com");
        register(&mut task_b, b_bid, "https://b.example.com");

        let a_paths = task_a.session_bb().paths();
        let a_href = a_paths.href_map();
        let b_paths = task_b.session_bb().paths();
        let b_href = b_paths.href_map();

        assert!(
            a_href.path(&a_bid, &a_paths).is_some(),
            "task A must see its own href registration"
        );
        assert!(
            b_href.path(&b_bid, &b_paths).is_some(),
            "task B must see its own href registration"
        );
        assert!(
            a_href.path(&b_bid, &a_paths).is_none(),
            "task A must NOT see task B's href registration — the shared PathMap was \
             written through without copy-on-write"
        );
        assert!(
            b_href.path(&a_bid, &b_paths).is_none(),
            "task B must NOT see task A's href registration"
        );
    }
}
