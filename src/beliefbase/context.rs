//! Context types for navigating belief relationships.
//!
//! This module provides view types that bundle a node with its relationship context:
//! - [`ExtendedRelation`]: Tracks relation information with respect to a node
//! - [`BeliefContext`]: Provides lazy access to sources and sinks for a node

use crate::properties::{content_namespaces, BeliefNode, Bid, WeightSet};

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
            tracing::info!("Could not find 'other' node: {other_bid}");
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
                let fallback_net = content_namespaces()
                    .iter()
                    .find(|cns| other.bid.parent_bref() == cns.bref())
                    .copied()
                    .unwrap_or(root_net);
                tracing::debug!(
                    "No path found for node {other_bid} in any path map (root_net={root_net}), \
                     using fallback net {fallback_net} with empty path"
                );
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

    /// Lazily compute source relations for this node
    pub fn sources(&'a self) -> Vec<ExtendedRelation<'a>> {
        let graph = self.relations_guard.as_graph();

        graph
            .raw_edges()
            .iter()
            .filter_map(|edge| {
                let source_bid = graph[edge.source()];
                let sink_bid = graph[edge.target()];
                if sink_bid == self.node.bid {
                    ExtendedRelation::new(source_bid, self.root_net, &edge.weight, self.bb)
                } else {
                    None
                }
            })
            .collect()
    }

    /// Lazily compute sink relations for this node
    pub fn sinks(&'a self) -> Vec<ExtendedRelation<'a>> {
        let graph = self.relations_guard.as_graph();

        graph
            .raw_edges()
            .iter()
            .filter_map(|edge| {
                let source_bid = graph[edge.source()];
                let sink_bid = graph[edge.target()];
                if source_bid == self.node.bid {
                    ExtendedRelation::new(sink_bid, self.root_net, &edge.weight, self.bb)
                } else {
                    None
                }
            })
            .collect()
    }
}
