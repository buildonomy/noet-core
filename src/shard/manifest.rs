//! Shard manifest types and size estimation.
//!
//! Defines the configuration and metadata types for BeliefBase sharding:
//! - [`ShardConfig`]: Tunable parameters (threshold, memory budget)
//! - [`ShardManifest`]: Written to `beliefbase/manifest.json` in sharded mode
//! - [`SearchManifest`]: Written to `search/manifest.json` always
//!
//! ## References
//!
//! - `docs/design/search_and_sharding.md` §4 — Manifest format specification

use crate::properties::{Bid, Bref};
use serde::{Deserialize, Serialize};

/// Default sharding threshold: 10MB of serialized BeliefGraph JSON.
///
/// Repos below this threshold write a monolithic `beliefbase.json`.
/// Repos at or above this threshold write `beliefbase/` with per-network shards.
pub const SHARD_THRESHOLD: usize = 10 * 1024 * 1024; // 10MB in bytes

/// Default browser memory budget for loaded BeliefBase data shards.
pub const DEFAULT_MEMORY_BUDGET_MB: f64 = 200.0;

/// 10% buffer added to serialized size to estimate in-memory overhead.
const SIZE_BUFFER_FACTOR: f64 = 1.1;

/// Configuration for sharding behavior.
///
/// Created once and passed through the export pipeline. All fields have
/// sensible defaults via [`ShardConfig::default()`].
#[derive(Debug, Clone)]
pub struct ShardConfig {
    /// Byte threshold above which the export is split into per-network shards.
    /// Default: [`SHARD_THRESHOLD`] (10MB).
    pub shard_threshold: usize,
    /// Browser memory budget for loaded data shards (MB).
    /// Default: [`DEFAULT_MEMORY_BUDGET_MB`] (200MB).
    pub memory_budget_mb: f64,
}

impl Default for ShardConfig {
    fn default() -> Self {
        Self {
            shard_threshold: SHARD_THRESHOLD,
            memory_budget_mb: DEFAULT_MEMORY_BUDGET_MB,
        }
    }
}

impl ShardConfig {
    /// Returns `true` if the serialized graph JSON exceeds the sharding threshold.
    pub fn should_shard(&self, serialized_bytes: usize) -> bool {
        serialized_bytes >= self.shard_threshold
    }
}

/// Metadata for a single per-network BeliefBase data shard.
///
/// Stored in `beliefbase/manifest.json` under the `networks` array.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkShardMeta {
    /// Short reference string (5 hex chars) for this network.
    pub bref: String,
    /// Full BID of the network node.
    pub bid: String,
    /// Human-readable network title.
    pub title: String,
    /// Number of `BeliefNode` states in this shard.
    pub node_count: usize,
    /// Number of edges in this shard's `BidGraph`.
    pub relation_count: usize,
    /// Estimated in-memory size when loaded (MB), with 10% overhead buffer.
    pub estimated_size_mb: f64,
    /// Path to the shard JSON file, relative to `beliefbase/`.
    /// Always `networks/{bref}.json`.
    pub path: String,
    /// Path to the search index for this network, relative to `beliefbase/`.
    /// Always `../search/{bref}.idx.json`.
    pub search_index_path: String,
    /// Approximate size of the search index in KB.
    pub search_index_size_kb: f64,
}

/// Metadata for the global shard (`beliefbase/global.json`).
///
/// The global shard contains the API node, system namespace nodes, and
/// cross-network relations. It is always loaded in sharded mode because
/// it is needed to resolve cross-network links.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlobalShardMeta {
    /// Number of nodes in the global shard.
    pub node_count: usize,
    /// Estimated in-memory size (MB).
    pub estimated_size_mb: f64,
    /// Path to the global shard JSON, relative to `beliefbase/`.
    /// Always `global.json`.
    pub path: String,
}

/// The BeliefBase shard manifest, written to `beliefbase/manifest.json`.
///
/// Only present in sharded mode (total export >= threshold). The viewer reads
/// this to populate the network selector UI and locate per-network shard files.
///
/// See `docs/design/search_and_sharding.md` §4.1 for the JSON schema.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShardManifest {
    /// Format version for forward compatibility.
    pub version: String,
    /// Always `true` — this file only exists in sharded mode.
    pub sharded: bool,
    /// Browser memory budget for data shards (MB).
    #[serde(rename = "memoryBudgetMB")]
    pub memory_budget_mb: f64,
    /// Per-network shard metadata, in stable iteration order.
    pub networks: Vec<NetworkShardMeta>,
    /// Global shard metadata.
    pub global: GlobalShardMeta,
}

impl ShardManifest {
    /// Construct an empty manifest with the given memory budget.
    pub fn new(memory_budget_mb: f64) -> Self {
        Self {
            version: "1.0".to_string(),
            sharded: true,
            memory_budget_mb,
            networks: Vec::new(),
            global: GlobalShardMeta {
                node_count: 0,
                estimated_size_mb: 0.0,
                path: "global.json".to_string(),
            },
        }
    }
}

/// Metadata for one network's search index, stored in `search/manifest.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkSearchMeta {
    /// Short reference string (5 hex chars) for this network.
    pub bref: String,
    /// Human-readable network title.
    pub title: String,
    /// Filename of the `.idx.json`, relative to `search/`.
    /// Always `{bref}.idx.json`.
    pub path: String,
    /// Approximate size of the index file in KB.
    pub size_kb: f64,
}

/// The search index manifest, written to `search/manifest.json`.
///
/// Always generated, regardless of whether data is sharded or monolithic.
/// The viewer fetches this first, then loads all `.idx.json` files listed here,
/// enabling full-corpus search before any data shard is loaded.
///
/// See `docs/design/search_and_sharding.md` §4.1 for the JSON schema.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchManifest {
    /// Format version.
    pub version: String,
    /// Per-network search index entries, in stable iteration order.
    pub networks: Vec<NetworkSearchMeta>,
}

impl SearchManifest {
    pub fn new() -> Self {
        Self {
            version: "1.0".to_string(),
            networks: Vec::new(),
        }
    }
}

impl Default for SearchManifest {
    fn default() -> Self {
        Self::new()
    }
}

/// Estimate the in-memory size of a shard from its serialized JSON byte length.
///
/// Applies a `SIZE_BUFFER_FACTOR` overhead (10%) to account for the difference
/// between JSON byte count and actual heap allocation.
pub fn estimate_size_mb(serialized_bytes: usize) -> f64 {
    (serialized_bytes as f64 * SIZE_BUFFER_FACTOR) / (1024.0 * 1024.0)
}

/// Build a `NetworkShardMeta` entry given the network identifiers, shard
/// serialization size, and search index size.
pub fn network_shard_meta(
    bref: Bref,
    bid: Bid,
    title: String,
    node_count: usize,
    relation_count: usize,
    shard_bytes: usize,
    search_index_bytes: usize,
) -> NetworkShardMeta {
    let bref_str = bref.to_string();
    NetworkShardMeta {
        bref: bref_str.clone(),
        bid: bid.to_string(),
        title,
        node_count,
        relation_count,
        estimated_size_mb: estimate_size_mb(shard_bytes),
        path: format!("networks/{}.json", bref_str),
        search_index_path: format!("../search/{}.idx.json", bref_str),
        search_index_size_kb: search_index_bytes as f64 / 1024.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_should_shard_below_threshold() {
        let config = ShardConfig::default();
        assert!(!config.should_shard(SHARD_THRESHOLD - 1));
    }

    #[test]
    fn test_should_shard_at_threshold() {
        let config = ShardConfig::default();
        assert!(config.should_shard(SHARD_THRESHOLD));
    }

    #[test]
    fn test_should_shard_above_threshold() {
        let config = ShardConfig::default();
        assert!(config.should_shard(SHARD_THRESHOLD + 1));
    }

    #[test]
    fn test_should_shard_custom_threshold() {
        let config = ShardConfig {
            shard_threshold: 1024,
            memory_budget_mb: DEFAULT_MEMORY_BUDGET_MB,
        };
        assert!(!config.should_shard(1023));
        assert!(config.should_shard(1024));
    }

    #[test]
    fn test_estimate_size_mb_zero() {
        assert_eq!(estimate_size_mb(0), 0.0);
    }

    #[test]
    fn test_estimate_size_mb_one_mb() {
        // 1MB JSON → ~1.1MB estimated
        let one_mb = 1024 * 1024;
        let estimated = estimate_size_mb(one_mb);
        assert!(
            (estimated - 1.1).abs() < 0.001,
            "expected ~1.1 MB, got {estimated}"
        );
    }

    #[test]
    fn test_shard_manifest_roundtrip() {
        let manifest = ShardManifest {
            version: "1.0".to_string(),
            sharded: true,
            memory_budget_mb: 200.0,
            networks: vec![NetworkShardMeta {
                bref: "01abc".to_string(),
                bid: "00000000-0000-0000-0000-000000000001".to_string(),
                title: "Test Network".to_string(),
                node_count: 10,
                relation_count: 5,
                estimated_size_mb: 0.5,
                path: "networks/01abc.json".to_string(),
                search_index_path: "../search/01abc.idx.json".to_string(),
                search_index_size_kb: 12.5,
            }],
            global: GlobalShardMeta {
                node_count: 3,
                estimated_size_mb: 0.01,
                path: "global.json".to_string(),
            },
        };
        let json = serde_json::to_string_pretty(&manifest).unwrap();
        let roundtripped: ShardManifest = serde_json::from_str(&json).unwrap();
        assert_eq!(roundtripped.version, "1.0");
        assert_eq!(roundtripped.networks.len(), 1);
        assert_eq!(roundtripped.networks[0].bref, "01abc");
        assert_eq!(roundtripped.global.node_count, 3);
    }

    #[test]
    fn test_search_manifest_roundtrip() {
        let manifest = SearchManifest {
            version: "1.0".to_string(),
            networks: vec![NetworkSearchMeta {
                bref: "01abc".to_string(),
                title: "Test Network".to_string(),
                path: "01abc.idx.json".to_string(),
                size_kb: 42.0,
            }],
        };
        let json = serde_json::to_string_pretty(&manifest).unwrap();
        let roundtripped: SearchManifest = serde_json::from_str(&json).unwrap();
        assert_eq!(roundtripped.networks.len(), 1);
        assert_eq!(roundtripped.networks[0].path, "01abc.idx.json");
    }
}
