// query/mod.rs — Unified query infrastructure for the BeliefBase.
//
// This module re-exports all query primitives so that existing `crate::query::*`
// import paths continue to work.

use std::collections::BTreeMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;

use crate::beliefbase::BeliefGraph;
use crate::nodekey::NodeKey;
use crate::properties::{BeliefNode, Bid};
use crate::BuildonomyError;

/// Boxed future type alias for object-safe async trait methods.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Submap result: `(path, bid, order)` triples.
pub type SubmapResult = Result<Vec<(String, Bid, Vec<u16>)>, BuildonomyError>;

pub mod parser;
pub mod spec;
pub mod view;

pub use spec::*;
pub use view::*;

/// Recursion cutoff for query traversal depth.
pub const MAX_TRAVERSAL: u8 = 10;

/// Cutoff limit for balanced traversal recursion.
///
/// Each iteration walks one hop up the subnet ancestor chain (in-memory graph
/// lookup, no I/O).  Deep subnet trees (e.g. deeply nested directory-level
/// networks with section headings) can exceed 10 hops when each directory-level
/// network and its section headings each contribute a hop.
pub const BALANCE_CUTOFF: usize = 15;

pub trait BeliefSource: Send + Sync {
    /// Get all paths (including documents in subnets) for a network as `(path, bid, order)` triples.
    /// `path` is the network-relative path to start from (empty string = entire network).
    /// `bid` is the target node, and `order` is the sort key.
    /// `depth` controls subnet expansion: `0` = no expansion, `u8::MAX` = fully recursive.
    /// When `include_index` is false, entries whose order contains `NETWORK_SECTION_SORT_KEY`
    /// (index-file headings/sections) are filtered out.
    fn submap<'a>(
        &'a self,
        network_bid: Bid,
        path: &'a str,
        depth: u8,
        include_index: bool,
    ) -> BoxFuture<'a, SubmapResult>;

    /// Like [`BeliefSource::submap`] but scopes by entry [`Bid`] instead of a path string.
    /// `entry` is `None` for the entire network, or `Some(bid)` to root the submap
    /// at the subtree containing that node.
    fn submap_by_bid<'a>(
        &'a self,
        network_bid: Bid,
        entry: Option<Bid>,
        depth: u8,
        include_index: bool,
    ) -> BoxFuture<'a, SubmapResult>;

    /// Get cached file modification times for cache invalidation.
    /// Default implementation returns empty map (no cache invalidation support).
    fn get_file_mtimes(&self) -> BoxFuture<'_, Result<BTreeMap<PathBuf, i64>, BuildonomyError>> {
        tracing::warn!("This BeliefSource impl does not have a get_file_mtime implementation!");
        Box::pin(async { Ok(BTreeMap::new()) })
    }

    /// Export entire BeliefGraph for serialization (e.g., to JSON for client-side use).
    ///
    /// For BeliefBase: Returns consumed clone of the entire belief set.
    /// For DbConnection: Queries all beliefs and relations from database.
    ///
    /// Each backend must override this method. The default returns an error.
    fn export_beliefgraph(&self) -> BoxFuture<'_, Result<BeliefGraph, BuildonomyError>> {
        Box::pin(async {
            Err(BuildonomyError::Command(
                "BeliefSource::export_beliefgraph must be overridden by each backend".to_string(),
            ))
        })
    }

    /// Evaluate a query package in place. The evaluator inspects the package's
    /// spec, populates the tape, and produces the output graph.
    ///
    /// Every `BeliefSource` backend must override this method. The default
    /// returns an error.
    fn evaluate<'a>(
        &'a self,
        _package: &'a mut QueryPackage,
    ) -> BoxFuture<'a, Result<(), BuildonomyError>> {
        Box::pin(async {
            Err(BuildonomyError::Command(
                "BeliefSource::evaluate must be overridden by each backend".to_string(),
            ))
        })
    }
}

/// Look up a single node by key via the evaluate path.
///
/// This is a free-function replacement for the former `BeliefSource::get_node`
/// trait method. It builds a seed-only `QueryPackage` with a single-key subject,
/// evaluates it against `src`, and returns the first matching node (if any).
///
/// Uses `QueryPackage::new` (no halo, no ancestry) since the caller only needs
/// the node state. Callers that need edge context or ancestry should use
/// `QueryPackage::anchored` or `QueryPackage::balanced` directly.
pub async fn lookup_node<S: BeliefSource + ?Sized>(
    src: &S,
    key: &NodeKey,
) -> Result<Option<BeliefNode>, BuildonomyError> {
    let spec = QuerySpec::seed(TapeFn::Keys(vec![key.clone()]));
    let mut package = QueryPackage::new(spec);
    src.evaluate(&mut package).await?;
    // The seed was resolved to Bids during evaluation.
    // Look up the first resolved BID in the materialized graph.
    let bid = match package.spec().steps.first().map(|s| &s.input) {
        Some(TapeFn::Bids(bids)) => bids.first().copied(),
        _ => None,
    };
    match (bid, package.graph()) {
        (Some(bid), Some(graph)) => Ok(graph.states.get(&bid).cloned()),
        _ => Ok(None),
    }
}

/// Return all relations incident to the given BIDs (at least one endpoint in
/// `bids`), along with Trace-marked endpoint nodes.
///
/// This is a free-function replacement for the former `BeliefSource::get_edges`
/// trait method. It builds a balanced [`QueryPackage`] with a BID-set subject,
/// evaluates it, and returns the resulting graph.
///
/// The package must be **balanced**, not `QueryPackage::new`. `materialize_graph`
/// only copies an edge when *both* endpoints are in the result set, so a
/// seed-only package returns the seed nodes with **no edges at all** whenever a
/// neighbour is not itself in `bids` — silently, and regardless of how many
/// relations the store holds. The halo step `balanced` appends is what pulls
/// those neighbours in and makes "incident to" true as documented.
///
/// The halo is safe here **only because** const-namespace BIDs are stripped from
/// traversal frontiers (see `apply_traversal` in `beliefbase/base.rs` and
/// `apply_traversal_sql` in `db.rs`). Without that filter a balanced query fans
/// out through a hub node such as `href_namespace` to every document in the
/// corpus. If that filter is ever removed, this call site becomes a fan-out risk
/// again.
pub async fn lookup_edges<S: BeliefSource + ?Sized>(
    src: &S,
    bids: &[Bid],
) -> Result<BeliefGraph, BuildonomyError> {
    let spec = QuerySpec::seed(TapeFn::Bids(bids.to_vec()));
    let mut package = QueryPackage::balanced(spec);
    src.evaluate(&mut package).await?;
    Ok(package.into_graph())
}
