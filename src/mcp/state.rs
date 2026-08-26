//! MCP server state: the loaded BeliefBase and shard manifest.
//!
//! ## Source kinds
//!
//! `McpState` holds a [`BeliefSourceKind`] which is either:
//!
//! - **`Static(BeliefBase)`** — loaded from pre-built shard files via `--output-dir`.
//!   No DB required. Fast to start. Results reflect the last `noet parse` run.
//!
//! - **`Live(DbConnection)`** — cloned from `WatchService::db_connection()` via
//!   `--watch`. The transaction task continuously commits compiled nodes to
//!   `belief_cache.db`; MCP reads whatever has been committed so far. Slightly
//!   stale data during a mid-compile pass is acceptable — identical to any other
//!   DB reader's behaviour. `DbConnection` is `Clone` (wraps a `sqlx::Pool` which
//!   is `Arc`-backed and cheap to clone). No subscriber task, no in-memory rebuild,
//!   no Arc-swap machinery.
//!
//! Both variants implement `BeliefSource`. Tool handlers call `state.source_ref()`
//! to get a `&dyn BeliefSource` and dispatch to the right implementation.
//!
//! ## Wire type → BeliefGraph conversion (static mode)
//!
//! `NetworkShard.states` is `BTreeMap<String, BeliefNode>` (BID string keys).
//! `GlobalShard.states` is the same. `SerializableEdge` holds BID strings for
//! source and sink. All strings are parsed to `Bid` via `Bid::try_from(&str)`
//! before constructing the `BeliefGraph`.
//!
//! ## TODO(Issue 66)
//!
//! Replace the inline shard loading logic here with `ShardBeliefSource` once that
//! type is defined. The `BeliefSource` trait in `src/query.rs` is already the
//! query-execution trait; the new shard-loading abstraction will use a different
//! name (see Issue 66 open questions).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::beliefbase::{BeliefBase, BeliefGraph, BidGraph};
#[cfg(feature = "service")]
use crate::db::DbConnection;
use crate::error::BuildonomyError;
use crate::properties::{BeliefKind, BeliefRelation, Bid};
use crate::query::BeliefSource;
use crate::shard::manifest::{GlobalShardMeta, NetworkShardMeta, ShardManifest};
use crate::shard::wire::{GlobalShard, NetworkShard};

// ── BeliefSourceKind ──────────────────────────────────────────────────────────

/// The backing query source for the MCP server.
///
/// Both variants implement `BeliefSource`. Tool handlers use `McpState::with_source`
/// to dispatch to the appropriate implementation without duplicating logic.
#[derive(Debug)]
pub enum BeliefSourceKind {
    /// Static mode: in-memory `BeliefBase` loaded from shard files.
    /// Used with `--output-dir`. No DB required.
    Static(Box<BeliefBase>),
    /// Live mode: `DbConnection` cloned from `WatchService::db_connection()`.
    /// Used with `--watch`. The DB is kept current by the WatchService transaction task.
    #[cfg(feature = "service")]
    Live(DbConnection),
}

// ── McpState ──────────────────────────────────────────────────────────────────

/// Shared server state passed to every MCP tool handler.
///
/// Constructed once at startup and wrapped in `Arc` for cheap cloning across
/// handler invocations. All fields are read-only after construction.
///
/// Tool handlers call async BeliefSource methods via `state.source`. Both
/// `BeliefBase` (static) and `DbConnection` (live) implement `BeliefSource`,
/// so tool logic is identical in both modes.
#[derive(Debug)]
pub struct McpState {
    /// The output directory (static mode) or `None` (live mode).
    /// Used by the `search` tool to locate `.idx.msgpack` files.
    pub output_dir: Option<PathBuf>,
    /// The shard manifest — network list, shard paths, memory estimates.
    /// In live mode this is populated from `WatchService`'s known networks.
    pub manifest: ShardManifest,
    /// The backing query source — either a static `BeliefBase` or a live `DbConnection`.
    pub source: BeliefSourceKind,
}

impl McpState {
    /// Load server state from a pre-built output directory (static mode).
    ///
    /// Handles both output layouts produced by `noet parse`:
    ///
    /// - **Sharded** (`beliefbase/manifest.json` present): reads manifest, deserializes
    ///   `global.msgpack` + per-network shards, merges into a `BeliefBase`.
    /// - **Monolithic** (`beliefbase.msgpack` present, no manifest): deserializes the
    ///   single file directly. The manifest is synthesized as empty (no per-network
    ///   metadata available); `get_networks` will return an empty list in this mode.
    ///
    /// # Errors
    ///
    /// Returns `BuildonomyError` if neither layout is found, or if deserialization fails.
    pub fn load_static(output_dir: &Path) -> Result<Arc<Self>, BuildonomyError> {
        let manifest_path = output_dir.join("beliefbase").join("manifest.json");
        let monolithic_path = output_dir.join("beliefbase.msgpack");

        if manifest_path.exists() {
            // Sharded layout.
            let manifest = load_manifest(output_dir)?;
            let belief_base = load_belief_base(output_dir, &manifest)?;
            Ok(Arc::new(McpState {
                output_dir: Some(output_dir.to_path_buf()),
                manifest,
                source: BeliefSourceKind::Static(Box::new(belief_base)),
            }))
        } else if monolithic_path.exists() {
            // Monolithic layout — corpus was below the 2MB shard threshold.
            tracing::info!(
                path = %monolithic_path.display(),
                "loading monolithic beliefbase.msgpack (corpus below shard threshold)"
            );
            let belief_base = load_monolithic(&monolithic_path)?;
            let manifest = manifest_from_beliefbase(&belief_base);
            tracing::info!(
                networks = manifest.networks.len(),
                nodes = manifest.global.node_count,
                "built manifest from monolithic BeliefBase"
            );
            Ok(Arc::new(McpState {
                output_dir: Some(output_dir.to_path_buf()),
                manifest,
                source: BeliefSourceKind::Static(Box::new(belief_base)),
            }))
        } else {
            Err(BuildonomyError::Io(format!(
                "No BeliefBase output found in {}. \
                 Expected either beliefbase/manifest.json (sharded) or \
                 beliefbase.msgpack (monolithic). Run `noet parse --html-output <dir>` first.",
                output_dir.display()
            )))
        }
    }

    /// Create server state backed by a live `DbConnection` (live mode).
    ///
    /// `db` should be a clone of `WatchService::db_connection()`. The manifest is
    /// built from `networks` — each entry provides the bref, bid, title, and stats
    /// needed for `get_networks` output.
    ///
    /// The `output_dir` is optional: if provided, the `search` tool will load
    /// `.idx.msgpack` files from `<output_dir>/search/`. If absent, search returns
    /// empty results (acceptable when no `--html-output` has been run).
    #[cfg(feature = "service")]
    pub fn from_db(
        db: DbConnection,
        manifest: ShardManifest,
        output_dir: Option<PathBuf>,
    ) -> Arc<Self> {
        Arc::new(McpState {
            output_dir,
            manifest,
            source: BeliefSourceKind::Live(db),
        })
    }

    /// Construct a minimal `McpState` for unit tests.
    ///
    /// Uses an empty manifest and an empty static `BeliefBase`. Sufficient for
    /// tests that exercise tool handler logic without real shard or DB data.
    /// Return `Some(&BeliefBase)` if this state is in static mode, `None` otherwise.
    ///
    /// Used by tool handlers that need direct `BeliefBase` access (e.g. `evaluate_query`
    /// which is not on the `BeliefSource` trait). In live mode those handlers fall back to
    /// `BeliefSource::evaluate`.
    #[cfg(test)]
    pub fn empty_for_test() -> Arc<Self> {
        Arc::new(McpState {
            output_dir: None,
            manifest: ShardManifest {
                version: "1.0".to_string(),
                sharded: true,
                memory_budget_mb: 200.0,
                networks: vec![],
                global: GlobalShardMeta {
                    node_count: 0,
                    estimated_size_mb: 0.0,
                    path: "global.msgpack".to_string(),
                },
            },
            source: BeliefSourceKind::Static(Box::default()),
        })
    }
}

// ── Private loading helpers (static mode) ─────────────────────────────────────

/// Load the monolithic `beliefbase.msgpack` into a `BeliefBase`.
fn load_monolithic(path: &Path) -> Result<BeliefBase, BuildonomyError> {
    let bytes = std::fs::read(path).map_err(|e| {
        BuildonomyError::Io(format!(
            "Failed to read monolithic shard at {}: {e}",
            path.display()
        ))
    })?;
    let graph: BeliefGraph = rmp_serde::from_slice(&bytes).map_err(|e| {
        BuildonomyError::Serialization(format!(
            "Failed to deserialize monolithic shard at {}: {e}",
            path.display()
        ))
    })?;
    Ok(BeliefBase::from(graph))
}

/// Build a `ShardManifest` for monolithic mode by scanning the loaded
/// BeliefBase for user-facing network nodes (Network kind, complete).
fn manifest_from_beliefbase(bb: &BeliefBase) -> ShardManifest {
    let networks: Vec<NetworkShardMeta> = bb
        .states()
        .values()
        .filter(|node| {
            node.kind.contains(BeliefKind::Network)
                && node.kind.is_complete()
                && !node.kind.contains(BeliefKind::API)
        })
        .map(|node| NetworkShardMeta {
            bref: node.bid.bref().to_string(),
            bid: node.bid.to_string(),
            title: node.title.clone(),
            node_count: 0,
            relation_count: 0,
            estimated_size_mb: 0.0,
            path: String::new(),
            search_index_path: format!("../search/{}.idx.msgpack", node.bid.bref()),
            search_index_size_kb: 0.0,
        })
        .collect();

    let node_count = bb.states().len();

    ShardManifest {
        version: "1.0".to_string(),
        sharded: false,
        memory_budget_mb: 0.0,
        networks,
        global: GlobalShardMeta {
            node_count,
            estimated_size_mb: 0.0,
            path: "beliefbase.msgpack".to_string(),
        },
    }
}

/// Read and deserialize `beliefbase/manifest.json`.
fn load_manifest(output_dir: &Path) -> Result<ShardManifest, BuildonomyError> {
    let manifest_path = output_dir.join("beliefbase").join("manifest.json");
    let bytes = std::fs::read(&manifest_path).map_err(|e| {
        BuildonomyError::Io(format!(
            "Failed to read shard manifest at {}: {e}",
            manifest_path.display()
        ))
    })?;
    serde_json::from_slice(&bytes).map_err(|e| {
        BuildonomyError::Serialization(format!(
            "Failed to parse shard manifest at {}: {e}",
            manifest_path.display()
        ))
    })
}

/// Deserialize all shards listed in the manifest and merge them into a [`BeliefBase`].
///
/// Loading order:
/// 1. Global shard — always loaded first (cross-network edges + global nodes)
/// 2. Per-network shards in manifest order
fn load_belief_base(
    output_dir: &Path,
    manifest: &ShardManifest,
) -> Result<BeliefBase, BuildonomyError> {
    let beliefbase_dir = output_dir.join("beliefbase");

    // ── Global shard ──────────────────────────────────────────────────────────
    let global_path = beliefbase_dir.join(&manifest.global.path);
    let global_shard = load_msgpack::<GlobalShard>(&global_path)?;

    tracing::debug!(
        nodes = global_shard.states.len(),
        edges = global_shard.relations.edges.len(),
        "loaded global shard"
    );

    // ── Per-network shards ────────────────────────────────────────────────────
    let mut all_shards: Vec<NetworkShard> = Vec::with_capacity(manifest.networks.len());
    for network_meta in &manifest.networks {
        let shard_path = beliefbase_dir.join(&network_meta.path);
        let shard = load_msgpack::<NetworkShard>(&shard_path)?;
        tracing::debug!(
            bref = %network_meta.bref,
            nodes = shard.states.len(),
            edges = shard.relations.edges.len(),
            "loaded network shard"
        );
        all_shards.push(shard);
    }

    // ── Merge into BeliefGraph then BeliefBase ────────────────────────────────
    let graph = merge_shards_into_graph(global_shard, all_shards);
    Ok(BeliefBase::from(graph))
}

/// Deserialize a MessagePack file into type `T`.
fn load_msgpack<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, BuildonomyError> {
    let bytes = std::fs::read(path).map_err(|e| {
        BuildonomyError::Io(format!(
            "Failed to read shard file at {}: {e}",
            path.display()
        ))
    })?;
    rmp_serde::from_slice(&bytes).map_err(|e| {
        BuildonomyError::Serialization(format!(
            "Failed to deserialize msgpack shard at {}: {e}",
            path.display()
        ))
    })
}

/// Merge a global shard and all per-network shards into a single [`BeliefGraph`].
///
/// The resulting `BeliefGraph` contains the union of all states and relations.
/// Cross-network edges from the global shard are included.
///
/// Wire types use `String` BID keys; this function parses them to [`Bid`] and
/// logs a warning for any that fail to parse (malformed shard data).
fn merge_shards_into_graph(global: GlobalShard, networks: Vec<NetworkShard>) -> BeliefGraph {
    // Collect all string-keyed states from global + per-network shards.
    // Per-network entries overwrite global on collision (network is authoritative
    // for its own nodes).
    let mut raw_states = global.states;
    let mut raw_edges = global.relations.edges;

    for shard in networks {
        raw_states.extend(shard.states);
        raw_edges.extend(shard.relations.edges);
    }

    // Convert string-keyed states → FxHashMap<Bid, BeliefNode>.
    let mut states = rustc_hash::FxHashMap::default();
    for (bid_str, node) in raw_states {
        match Bid::try_from(bid_str.as_str()) {
            Ok(bid) => {
                states.insert(bid, node);
            }
            Err(_) => {
                tracing::warn!(bid = %bid_str, "skipping node with unparseable BID in shard");
            }
        }
    }

    // Convert SerializableEdge list → BeliefRelation list → BidGraph.
    let relations_iter = raw_edges.into_iter().filter_map(|edge| {
        let source = match Bid::try_from(edge.source.as_str()) {
            Ok(b) => b,
            Err(_) => {
                tracing::warn!(bid = %edge.source, "skipping edge with unparseable source BID");
                return None;
            }
        };
        let sink = match Bid::try_from(edge.sink.as_str()) {
            Ok(b) => b,
            Err(_) => {
                tracing::warn!(bid = %edge.sink, "skipping edge with unparseable sink BID");
                return None;
            }
        };
        Some(BeliefRelation {
            source,
            sink,
            weights: edge.weights,
        })
    });
    let relations = BidGraph::from_edges(relations_iter);

    BeliefGraph { states, relations }
}

impl McpState {
    /// Return a `&dyn BeliefSource` for the active source variant.
    ///
    /// Use this in tool handlers instead of matching on `state.source` directly,
    /// so handlers stay free of `#[cfg(feature = "service")]` match arms.
    pub fn source_ref(&self) -> &dyn BeliefSource {
        match &self.source {
            BeliefSourceKind::Static(bb) => bb.as_ref(),
            #[cfg(feature = "service")]
            BeliefSourceKind::Live(db) => db,
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_for_test_has_zero_networks() {
        let state = McpState::empty_for_test();
        assert!(state.manifest.networks.is_empty());
        assert!(matches!(state.source, BeliefSourceKind::Static(_)));
    }

    #[test]
    fn test_load_static_missing_manifest_errors() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let result = McpState::load_static(tmp.path());
        assert!(
            result.is_err(),
            "should error when beliefbase/manifest.json is absent"
        );
    }
}
