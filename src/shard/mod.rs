//! # Shard Module
//!
//! Implements per-network BeliefBase sharding for large repositories.
//!
//! ## Overview
//!
//! When a BeliefBase export exceeds [`SHARD_THRESHOLD`] (default 10MB), the compiler
//! splits the data into per-network JSON shards instead of writing a single
//! `beliefbase.json`. Regardless of whether sharding occurs, this module also
//! generates per-network compile-time search indices (`search/{bref}.idx.json`) so
//! the viewer can search the entire corpus from the moment it loads.
//!
//! ## Output Structure
//!
//! ```text
//! html_output_dir/
//! ├── beliefbase.json              # Only if NOT sharded (backward compat)
//! ├── search/
//! │   ├── manifest.json            # Search index manifest (always generated)
//! │   ├── {bref_a}.idx.json        # Network A search index (always generated)
//! │   └── {bref_b}.idx.json        # Network B search index
//! └── beliefbase/                  # Only if sharded
//!     ├── manifest.json            # Shard metadata + memory budget
//!     ├── global.json              # API node + cross-network relations
//!     └── networks/
//!         ├── {bref_a}.json        # Network A BeliefGraph shard
//!         └── {bref_b}.json        # Network B BeliefGraph shard
//! ```
//!
//! ## Module Structure
//!
//! - [`wire`]: Target-independent shard wire types (available on all targets including wasm32)
//! - [`manifest`]: Shard config, manifest types, size estimation (native only)
//! - [`search`]: Compile-time search index building (native only)
//! - `export`: Sharded BeliefGraph export and `finalize_html` integration (native only)
//!
//! ## References
//!
//! - `docs/design/search_and_sharding.md` — Full architecture specification
//! - Issue 50: BeliefBase Sharding
//! - Issue 54: Full-Text Search MVP (uses the `.idx.json` files built here)

/// Wire format types for shard JSON serialization/deserialization.
/// Available on all targets (including wasm32) so BeliefBaseWasm can deserialize shards.
pub mod wire;

#[cfg(not(target_arch = "wasm32"))]
pub mod export;
#[cfg(not(target_arch = "wasm32"))]
pub mod manifest;
#[cfg(not(target_arch = "wasm32"))]
pub mod search;

#[cfg(not(target_arch = "wasm32"))]
pub use export::{export_beliefbase, ExportMode};
#[cfg(not(target_arch = "wasm32"))]
pub use manifest::{SearchManifest, ShardConfig, ShardManifest, SHARD_THRESHOLD};
#[cfg(not(target_arch = "wasm32"))]
pub use search::{build_search_indices, SearchIndex};

pub use wire::{GlobalShard, NetworkShard, SerializableBidGraph, SerializableEdge};
