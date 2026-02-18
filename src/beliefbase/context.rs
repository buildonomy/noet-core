//! Context types for navigating belief relationships.
//!
//! This module provides view types that bundle a node with its relationship context:
//! - [`ExtendedRelation`]: Tracks relation information with respect to a node
//! - [`BeliefContext`]: Provides lazy access to sources and sinks for a node

use crate::properties::{asset_namespace, href_namespace, BeliefNode, Bid, WeightSet};

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

        // Treat const namespaces differently, just return their path
        let paths_guard = set.paths();
        for const_net in [asset_namespace(), href_namespace()] {
            if let Some(pm) = paths_guard.get_map(&const_net.bref()) {
                if let Some((_bid, elem_path, _order)) = pm.path(&other_bid, &paths_guard) {
                    tracing::debug!("Found const net node. Path: {elem_path}");
                    return Some(ExtendedRelation {
                        other,
                        home_net: const_net,
                        root_path: elem_path,
                        weight,
                    });
                }
            }
        }

        // Try to get path from root network
        let (home_net, root_path) = paths_guard
            .get_map(&root_net.bref())
            .and_then(|pm| pm.path(&other_bid, &paths_guard))
            .map(|(bid, path, _order)| (bid, path))
            .unwrap_or_else(|| {
                // No path found - use empty string as fallback
                // This allows relations to nodes without paths (e.g., sections, internal nodes)
                // The viewer can decide whether to render these as links or plain text
                tracing::debug!(
                    "No path found for node {other_bid} in network {root_net}, using empty path"
                );
                (root_net, String::new())
            });

        Some(ExtendedRelation {
            home_net,
            root_path,
            other,
            weight,
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
    set: &'a BeliefBase,
    relations_guard: RelationsGuard<'a>,
}

impl<'a> BeliefContext<'a> {
    /// Create a new BeliefContext (used by BeliefBase::get_context)
    pub(super) fn new(
        node: &'a BeliefNode,
        root_path: String,
        root_net: Bid,
        home_net: Bid,
        set: &'a BeliefBase,
        relations_guard: RelationsGuard<'a>,
    ) -> Self {
        BeliefContext {
            node,
            root_path,
            root_net,
            home_net,
            set,
            relations_guard,
        }
    }

    /// Get a reference to the underlying BeliefBase
    pub fn belief_set(&self) -> &'a BeliefBase {
        self.set
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
                    ExtendedRelation::new(source_bid, self.root_net, &edge.weight, self.set)
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
                    ExtendedRelation::new(sink_bid, self.root_net, &edge.weight, self.set)
                } else {
                    None
                }
            })
            .collect()
    }
}
