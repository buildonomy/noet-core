//! Context types for navigating belief relationships.
//!
//! This module provides view types that bundle a node with its relationship context:
//! - [`ExtendedRelation`]: Tracks relation information with respect to a node
//! - [`BeliefContext`]: Provides lazy access to sources, sinks, and owned edges
//!   for a node (borrowed, wasm-safe)
//! - [`OwnedEdge`]: An edge with its third-party owner resolved
//!
//! ## Usage pattern
//!
//! Callers evaluate a `QueryPackage` to obtain a `BeliefGraph`, convert it
//! to a local `BeliefBase` via `BeliefBase::from(graph)`, then call
//! `BeliefBase::get_context(&self, root_net, bid)` to get a `BeliefContext<'a>`.
//! The `BeliefContext` provides lazy access to sources, sinks, and owned edges
//! without eagerly materializing owned copies.

use crate::paths::AnchorPath;
use crate::properties::{
    content_namespaces, BeliefNode, Bid, Bref, WeightKind, WeightSet, WEIGHT_OWNED_BY,
};
use petgraph::visit::{EdgeRef, IntoEdgeReferences};
use serde::{Deserialize, Serialize};

#[cfg(not(target_arch = "wasm32"))]
use parking_lot::{ArcRwLockReadGuard, RawRwLock};

#[cfg(target_arch = "wasm32")]
use std::cell::Ref;

use super::{BeliefBase, BidGraph};

// Conditional type alias for the relations guard
#[cfg(not(target_arch = "wasm32"))]
type RelationsGuard<'a> = ArcRwLockReadGuard<RawRwLock, BidGraph>;

#[cfg(target_arch = "wasm32")]
type RelationsGuard<'a> = Ref<'a, BidGraph>;

// ── Owned-edge type ───────────────────────────────────────────────────────────────

/// An edge with its owner resolved, suitable for traceability queries.
///
/// Emitted by [`BeliefContext::owned_edges`] for every (relation × weight_kind) pair
/// where `WEIGHT_OWNED_BY` identifies a third-party section node (not `"source"`,
/// `"sink"`, or absent).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct OwnedEdge {
    /// The section node that owns this edge via a `{maps_to}` directive.
    pub owner_bid: Bid,
    /// The source endpoint of the edge.
    pub source_bid: Bid,
    /// The sink endpoint of the edge.
    pub sink_bid: Bid,
    /// The weight kind for this edge.
    pub weight_kind: WeightKind,
}

// ExtendedRelation tracks relation information with respect to a node. 'Other' refers to the
// external node. The self node is specified by the struture holding the ExtendedRelation (e.g. a
// [BeliefContext]).
#[derive(Debug)]
pub struct ExtendedRelation<'a> {
    pub other: &'a BeliefNode,
    pub home_net: Bid,
    pub root_path: String,
    pub weight: &'a WeightSet,
    /// The link display text stored on the edge during parse, if it differed from the target's title.
    /// Populated from the `WEIGHT_LINK_TITLE` ("title") key in the edge Weight payload.
    pub link_title: Option<String>,
}

impl<'a> ExtendedRelation<'a> {
    pub fn new(
        other_bid: Bid,
        root_net: Bid,
        weight: &'a WeightSet,
        set: &'a BeliefBase,
    ) -> Option<ExtendedRelation<'a>> {
        let Some(other) = set.states().get(&other_bid) else {
            tracing::debug!(
                label = set.label,
                "Could not find 'other' node: {other_bid}"
            );
            return None;
        };

        let paths_guard = set.paths();
        // Try to get path from root network, then content networks, then all remaining path maps.
        // This ensures that when looking up a document node's path from an asset/href context,
        // we still find the correct home_net rather than incorrectly inheriting root_net.
        let fallback_nets = content_namespaces();
        let (home_net, root_path) = std::iter::once(root_net)
            .chain(fallback_nets.iter().copied())
            .find_map(|ns| {
                paths_guard
                    .get_map(&ns.bref())
                    .and_then(|pm| pm.path(&other_bid, &paths_guard))
                    .map(|(home_network, path, _order)| (home_network, path))
            })
            .or_else(|| {
                // Search all path maps to find the node's home network, then look up
                // its local path within that network. Using indexed_path directly would
                // return a cross-network path with subnet prefixes (e.g.
                // "1f117143-.../asset_tracking_test.html") when the node is found via
                // subnet traversal. Instead we do two steps:
                // 1. indexed_path to discover home_net
                // 2. net_indexed_path to get the path local to that network
                paths_guard.indexed_path(&other_bid).and_then(
                    |(home_network, _cross_net_path, _order)| {
                        paths_guard
                            .net_indexed_path(&home_network.bref(), &other_bid)
                            .map(|(net, local_path, _order)| (net, local_path))
                    },
                )
            })
            .unwrap_or_else(|| {
                // No path found in any PathMap — determine home_net from the node itself.
                // Content namespace nodes (href, asset) may not be in a PathMap yet
                // during incremental parsing, but we can detect them via parent_bref.
                let is_content_ns = content_namespaces()
                    .iter()
                    .any(|cns| other.bid.parent_bref() == cns.bref());
                let fallback_net = if is_content_ns {
                    content_namespaces()
                        .iter()
                        .find(|cns| other.bid.parent_bref() == cns.bref())
                        .copied()
                        .unwrap_or(root_net)
                } else {
                    root_net
                };
                let other_node_title = set
                    .states()
                    .get(&other_bid)
                    .map(|n| n.display_title())
                    .unwrap_or(other_bid.to_string());
                // Content namespace nodes (href_namespace, asset_namespace) are
                // expected to have no PathMap entry — they are External|Trace
                // nodes without filesystem paths.  Log at debug, not warn.
                if is_content_ns {
                    tracing::debug!(
                        "No path found for content-namespace node \"{other_node_title}\" \
                        (root_net={root_net}), using fallback net {fallback_net}"
                    );
                } else {
                    tracing::warn!(
                        "No path found for node \"{other_node_title}\" in any path map\
                        (root_net={root_net}), using fallback net {fallback_net} with empty path"
                    );
                }
                (fallback_net, String::new())
            });

        let link_title = weight
            .weights
            .values()
            .find_map(|w| w.get::<String>(crate::properties::WEIGHT_LINK_TITLE));

        Some(ExtendedRelation {
            home_net,
            root_path,
            other,
            weight,
            link_title,
        })
    }

    pub fn as_link_ref(&self) -> String {
        format!(
            "{}{}{}",
            self.other.bid.bref(),
            if !self.other.title.is_empty() {
                ":"
            } else {
                ""
            },
            self.other.title
        )
    }
}

#[derive(Debug)]
pub struct BeliefContext<'a> {
    pub node: &'a BeliefNode,
    pub root_path: String,
    pub root_net: Bid,
    pub home_net: Bid,
    bb: &'a BeliefBase,
    relations_guard: RelationsGuard<'a>,
}

impl<'a> BeliefContext<'a> {
    /// Create a new BeliefContext (used by BeliefBase::get_context)
    pub(super) fn new(
        node: &'a BeliefNode,
        root_path: String,
        root_net: Bid,
        home_net: Bid,
        bb: &'a BeliefBase,
        relations_guard: RelationsGuard<'a>,
    ) -> Self {
        BeliefContext {
            node,
            root_path,
            root_net,
            home_net,
            bb,
            relations_guard,
        }
    }

    /// Get a reference to the underlying BeliefBase
    pub fn beliefbase(&self) -> &'a BeliefBase {
        self.bb
    }

    /// Construct a minimal `BeliefContext` for unit tests.
    ///
    /// The relations guard is empty (no edges), which is sufficient for functions
    /// that only read `ctx.root_net` and `ctx.beliefbase()` (e.g. `compute_source_url`).
    #[cfg(test)]
    pub fn new_for_test(
        node: &'a BeliefNode,
        root_net: Bid,
        root_path: String,
        bb: &'a BeliefBase,
    ) -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        let relations_guard = bb.relations();
        #[cfg(target_arch = "wasm32")]
        let relations_guard = bb.relations();
        BeliefContext {
            node,
            root_path,
            root_net,
            home_net: root_net,
            bb,
            relations_guard,
        }
    }

    /// Lazily compute source relations for this node
    pub fn sources(&'a self) -> Vec<ExtendedRelation<'a>> {
        let graph = self.relations_guard.as_graph();

        let edges: Vec<_> = graph.edge_references().collect();
        edges
            .iter()
            .filter_map(|edge_ref| {
                let source_bid = graph[edge_ref.source()];
                let sink_bid = graph[edge_ref.target()];
                if sink_bid == self.node.bid {
                    ExtendedRelation::new(source_bid, self.root_net, edge_ref.weight(), self.bb)
                } else {
                    None
                }
            })
            .collect()
    }

    /// Collect all edges owned by a third-party section node (a `{maps_to}` directive owner).
    ///
    /// Iterates over all source and sink relations of this node. For each (relation × weight_kind)
    /// pair, checks `WEIGHT_OWNED_BY`. Entries where `WEIGHT_OWNED_BY` is absent, `"source"`, or
    /// `"sink"` are skipped — only third-party bref owners produce an `OwnedEdge`.
    ///
    /// Uses `self.bb.brefs()` (a plain `&BTreeMap`, no lock) to resolve bref strings to BIDs.
    pub fn owned_edges(&self) -> Vec<OwnedEdge> {
        let mut result = Vec::new();

        // Process sources: ext_rel.other is the source, self.node is the sink.
        for ext_rel in self.sources() {
            for (weight_kind, weight) in ext_rel.weight.weights.iter() {
                let owned_by: Option<String> = weight.get(WEIGHT_OWNED_BY);
                let owner_bid = match owned_by.as_deref() {
                    Some("source") | Some("sink") | None => continue,
                    Some(bref_str) => {
                        match Bref::try_from(bref_str)
                            .ok()
                            .and_then(|bref| self.bb.brefs().get(&bref).copied())
                        {
                            Some(bid) => bid,
                            None => continue,
                        }
                    }
                };
                result.push(OwnedEdge {
                    owner_bid,
                    source_bid: ext_rel.other.bid,
                    sink_bid: self.node.bid,
                    weight_kind: *weight_kind,
                });
            }
        }

        // Process sinks: self.node is the source, ext_rel.other is the sink.
        for ext_rel in self.sinks() {
            for (weight_kind, weight) in ext_rel.weight.weights.iter() {
                let owned_by: Option<String> = weight.get(WEIGHT_OWNED_BY);
                let owner_bid = match owned_by.as_deref() {
                    Some("source") | Some("sink") | None => continue,
                    Some(bref_str) => {
                        match Bref::try_from(bref_str)
                            .ok()
                            .and_then(|bref| self.bb.brefs().get(&bref).copied())
                        {
                            Some(bid) => bid,
                            None => continue,
                        }
                    }
                };
                result.push(OwnedEdge {
                    owner_bid,
                    source_bid: self.node.bid,
                    sink_bid: ext_rel.other.bid,
                    weight_kind: *weight_kind,
                });
            }
        }

        result
    }

    /// Collect all edges in the full graph that are **declared by** this node (i.e. where
    /// `WEIGHT_OWNED_BY` resolves to this node's bref). This is the owner perspective —
    /// complementary to `owned_edges()` which is the endpoint perspective.
    ///
    /// Used to populate `NodeContext.owned_edges` for owner sections (e.g. `{maps_to}`
    /// directive nodes) whose own `sources()` and `sinks()` are empty because they are
    /// neither source nor sink of the edges they declare.
    pub fn declared_edges(&self) -> Vec<OwnedEdge> {
        use petgraph::visit::{EdgeRef, IntoEdgeReferences};

        let owner_bref = self.node.bid.bref().to_string();
        let graph = self.relations_guard.as_graph();
        let mut result = Vec::new();

        for edge_ref in graph.edge_references() {
            let source_bid = graph[edge_ref.source()];
            let sink_bid = graph[edge_ref.target()];
            let weights = edge_ref.weight();
            for (weight_kind, weight) in weights.weights.iter() {
                let owned_by: Option<String> = weight.get(WEIGHT_OWNED_BY);
                if owned_by.as_deref() == Some(&owner_bref) {
                    result.push(OwnedEdge {
                        owner_bid: self.node.bid,
                        source_bid,
                        sink_bid,
                        weight_kind: *weight_kind,
                    });
                }
            }
        }

        result
    }

    /// Return the union of `owned_edges` and `declared_edges` with deduplication.
    ///
    /// This is the canonical way to obtain all edges associated with a node from an
    /// ownership perspective:
    ///
    /// - **`owned_edges`** — endpoint perspective: this node is source or sink of an
    ///   edge that is owned by some third-party section node.
    /// - **`declared_edges`** — owner perspective: this node declared the edge via a
    ///   `{maps_to}` directive, even though it is neither source nor sink of that edge.
    ///
    /// The two sets overlap for nodes that are simultaneously an owner and an endpoint
    /// (uncommon but possible). Duplicates are removed by comparing all four fields
    /// `(owner_bid, source_bid, sink_bid, weight_kind)`.
    ///
    /// Both `wasm.rs::extract_node_context` and `src/mcp/tools.rs::get_context` call
    /// this method rather than duplicating the merge logic.
    pub fn all_owned_edges(&self) -> Vec<OwnedEdge> {
        let mut result = self.owned_edges();
        let declared = self.declared_edges();
        for de in declared {
            if !result.iter().any(|oe| {
                oe.owner_bid == de.owner_bid
                    && oe.source_bid == de.source_bid
                    && oe.sink_bid == de.sink_bid
                    && oe.weight_kind == de.weight_kind
            }) {
                result.push(de);
            }
        }
        result
    }

    /// Lazily compute sink relations for this node
    pub fn sinks(&'a self) -> Vec<ExtendedRelation<'a>> {
        let graph = self.relations_guard.as_graph();

        let edges: Vec<_> = graph.edge_references().collect();
        edges
            .iter()
            .filter_map(|edge_ref| {
                let source_bid = graph[edge_ref.source()];
                let sink_bid = graph[edge_ref.target()];
                if source_bid == self.node.bid {
                    ExtendedRelation::new(sink_bid, self.root_net, edge_ref.weight(), self.bb)
                } else {
                    None
                }
            })
            .collect()
    }
}

// ── Standalone href resolution helpers ────────────────────────────────────────

/// Resolve a node's BID to a relative HTML href from `from_path`.
///
/// 1. PathMap lookup via `bb.paths().path(bid)` → `(net_bid, root_path)`
/// 2. Extension rewrite: `.md` → `.html`, directories → `dir/index.html`
/// 3. Relative path computation via `AnchorPath::path_to`
///
/// Returns `None` if the BID has no PathMap entry.
#[cfg(not(target_arch = "wasm32"))]
pub fn resolve_node_href(bb: &super::BeliefBase, bid: &Bid, from_path: &str) -> Option<String> {
    let pmm = bb.paths();
    // Two-step lookup (same pattern as ExtendedRelation::new):
    // 1. indexed_path to discover the home network
    // 2. net_indexed_path to get the path local to that network
    // Using pmm.path() directly can return cross-network paths with BID
    // prefixes from subnet traversal, producing invalid hrefs.
    let (_home_net, local_path) = pmm
        .indexed_path(bid)
        .and_then(|(home_net, _cross_path, _order)| {
            pmm.net_indexed_path(&home_net.bref(), bid)
                .map(|(net, local_path, _order)| (net, local_path))
        })
        .or_else(|| pmm.path(bid))?;
    Some(resolve_href_from_root_path(&local_path, from_path))
}

/// Given a target's network-relative root_path and the current document's
/// network-relative path, compute a relative HTML href with extension rewriting.
///
/// This is the shared kernel used by `resolve_node_href` and (in Phase 2)
/// `ExtendedRelation::render_anchor`.
#[cfg(not(target_arch = "wasm32"))]
pub fn resolve_href_from_root_path(target_root_path: &str, from_path: &str) -> String {
    let link_ap = AnchorPath::from(target_root_path);
    // Extension rewrite: .md → .html, directories → index.html
    let html_path = if link_ap.is_dir() || link_ap.ext().is_empty() {
        link_ap.join("index.html").into_string()
    } else if link_ap.ext() == "md" {
        // Common case — .md → .html
        link_ap.replace_extension("html")
    } else {
        target_root_path.to_string()
    };

    let from_ap = AnchorPath::from(from_path);
    from_ap.path_to(&html_path, false)
}
