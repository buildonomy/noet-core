//! MCP-specific output types for BeliefBase tool responses.
//!
//! These types are **independent** of the WASM serialization types in `src/wasm.rs`.
//! The WASM types carry JS-specific constraints (Map vs. plain-object semantics,
//! `Reflect::set` patches, sorted graph construction for JS consumers). MCP JSON-RPC
//! output has none of those constraints — it is plain `serde_json` serialization
//! targeting LLM agent consumers.
//!
//! ## Type map
//!
//! | MCP Tool       | Output type                  |
//! |----------------|------------------------------|
//! | `get_networks` | [`NetworksOutput`]           |
//! | `search`       | `Vec<`[`SearchHit`]`>`       |
//! | `get_context`  | [`NodeContextOutput`]        |
//! | `get_submap`   | [`SubmapOutput`]             |
//! | `query`        | [`QueryOutput`]              |
//! | `check_consistency` | [`ConsistencyOutput`]   |

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// ── get_networks ──────────────────────────────────────────────────────────────

/// Output of the `get_networks` MCP tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworksOutput {
    /// All compiled networks visible in the loaded BeliefBase.
    pub networks: Vec<NetworkEntry>,
    /// Brief inline orientation note for LLM consumers.
    ///
    /// Always present regardless of whether the client injected the
    /// `noet://help/orientation` resource. Covers: what BIDs/brefs are,
    /// the source=child/sink=parent direction convention, and which tools
    /// to call next.
    pub orientation: String,
}

/// Metadata for a single compiled network.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkEntry {
    /// Short reference string (5 hex chars). Use this in tool calls that accept `network`.
    pub bref: String,
    /// Full UUID-format belief identifier.
    pub bid: String,
    /// Human-readable network title.
    pub title: String,
    /// Number of nodes in this network's shard.
    pub node_count: usize,
    /// Number of intra-network edges in this network's shard.
    pub relation_count: usize,
    /// Path to the shard file, relative to the output directory.
    pub path: String,
}

// ── search ────────────────────────────────────────────────────────────────────

/// One result from the `search` MCP tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchHit {
    /// Full BID of the matching node. Pass to `get_context` for details.
    pub bid: String,
    /// Node title.
    pub title: String,
    /// Short excerpt from the node's text content. Empty if the node's shard
    /// is not loaded in the current session.
    pub snippet: String,
    /// TF-IDF relevance score (higher is more relevant).
    pub score: f32,
    /// Bref of the network this node belongs to.
    pub network: String,
    /// Viewer-relative path for this node (e.g. `"networks/abc12.msgpack"`).
    pub path: String,
}

// ── get_context ───────────────────────────────────────────────────────────────

/// Output of the `get_context` MCP tool.
///
/// Mirrors the semantics of `extract_node_context` in `wasm.rs` but uses plain
/// Rust/JSON types instead of JS-compatibility shims.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeContextOutput {
    /// The node itself.
    pub node: NodeSummary,
    /// BID of the network that owns this node's path entry.
    pub home_net: String,
    /// Path of this node within its home network (e.g. `"doc/section"`).
    pub root_path: String,
    /// Arbitrary key/value metadata from the node's TOML frontmatter.
    pub metadata: BTreeMap<String, serde_json::Value>,
    /// Nodes directly related to this node (sources and sinks).
    pub related_nodes: Vec<RelatedNodeEntry>,
    /// All edges incident to this node, grouped by weight kind.
    pub edges: Vec<EdgeEntry>,
    /// Edges owned by this node via `{maps_to}` directives (owner perspective).
    pub owned_edges: Vec<OwnedEdgeEntry>,
    /// Full text content of the node, if `include_content` was `true` in the request.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

/// Compact node summary used inside context responses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeSummary {
    pub bid: String,
    pub bref: String,
    pub title: String,
    /// Schema kind string from the node's `kind` field, if present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

/// One related node (source or sink) of the context node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelatedNodeEntry {
    /// `"source"` or `"sink"` relative to the context node.
    pub direction: String,
    pub bid: String,
    pub bref: String,
    pub title: String,
    pub home_net: String,
    pub root_path: String,
    /// Weight kinds active on the edge connecting this node to the context node.
    pub weight_kinds: Vec<String>,
    /// Optional display text if the link title differed from the node's title.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link_title: Option<String>,
}

/// One edge incident to the context node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeEntry {
    pub source_bid: String,
    pub sink_bid: String,
    pub weight_kind: String,
    /// Optional bref of the section node that owns this edge via `{maps_to}`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owned_by: Option<String>,
}

/// One edge declared (owned) by the context node via a `{maps_to}` directive.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OwnedEdgeEntry {
    pub owner_bid: String,
    pub source_bid: String,
    pub sink_bid: String,
    pub weight_kind: String,
}

// ── get_submap ────────────────────────────────────────────────────────────────

/// Output of the `get_submap` MCP tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmapOutput {
    /// Nodes in the traceability subgraph, in path order.
    pub nodes: Vec<SubmapNode>,
    /// Edges within the subgraph.
    pub edges: Vec<SubmapEdge>,
}

/// One node in a submap result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmapNode {
    pub bid: String,
    pub bref: String,
    pub title: String,
    pub path: String,
    pub depth: usize,
}

/// One edge in a submap result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmapEdge {
    pub source_bid: String,
    pub sink_bid: String,
    pub weight_kind: String,
}

// ── query ─────────────────────────────────────────────────────────────────────

/// Output of the `query` MCP tool.
///
/// A subset of the BeliefGraph matching the provided QuerySpec.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryOutput {
    /// Matching nodes, keyed by BID string.
    pub states: BTreeMap<String, serde_json::Value>,
    /// Edges among matching nodes.
    pub edges: Vec<EdgeEntry>,
}

// ── check_consistency ─────────────────────────────────────────────────────────

/// Output of the `check_consistency` MCP tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsistencyOutput {
    /// Cross-references that failed to resolve (broken `id://` sinks, etc.).
    pub unresolved_refs: Vec<UnresolvedRef>,
    /// Edges whose source or sink BID is absent from all loaded networks.
    pub orphaned_edges: Vec<OrphanedEdge>,
    /// Human-readable summary string.
    pub summary: String,
    /// ISO 8601 UTC timestamp of when the loaded shards were compiled.
    /// Use this to reason about freshness in static mode.
    pub compiled_at: Option<String>,
}

/// One unresolved cross-reference diagnostic.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnresolvedRef {
    /// Source file path where the broken reference appears.
    pub path: String,
    /// BID of the node that declared the broken reference, if resolved.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_bid: Option<String>,
    /// The raw unresolved key (e.g. `"id://some-anchor"`).
    pub raw_key: String,
    /// Weight kind of the edge that failed to resolve.
    pub weight_kind: String,
    /// Line number in the source file, if available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
}

/// One orphaned edge (both endpoints must exist in the loaded graph for an edge
/// to be valid; an orphaned edge has at least one missing endpoint).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrphanedEdge {
    pub source_bid: String,
    pub sink_bid: String,
    pub weight_kind: String,
    /// Human-readable reason: `"source missing"`, `"sink missing"`, or `"both missing"`.
    pub reason: String,
}

// ── Tool input types ──────────────────────────────────────────────────────────
//
// Defined here alongside outputs so tool schemas are co-located.

/// Input for the `get_context` tool.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct GetContextInput {
    /// BID of the node to retrieve context for.
    pub bid: String,
    /// If true, include the full text content of the node in the response.
    /// Defaults to false to keep token usage low for large documents.
    #[serde(default)]
    pub include_content: bool,
}

/// Input for the `search` tool.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SearchInput {
    /// Full-text search query string. Supports natural language; stemming and
    /// fuzzy matching are applied automatically.
    pub query: String,
    /// Maximum number of results to return. Defaults to 20.
    #[serde(default = "default_search_limit")]
    pub limit: usize,
    /// Optional network bref to restrict search to a single network.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network: Option<String>,
}

fn default_search_limit() -> usize {
    20
}

/// Input for the `get_submap` tool.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct GetSubmapInput {
    /// BID of the entry node for the submap traversal.
    pub bid: String,
    /// How many hops to traverse from the entry node. Defaults to 3.
    #[serde(default = "default_submap_depth")]
    pub depth: u8,
    /// Traversal direction: `"upstream"`, `"downstream"`, or `"both"`.
    /// Defaults to `"both"`.
    #[serde(default = "default_submap_direction")]
    pub direction: String,
}

fn default_submap_depth() -> u8 {
    3
}

fn default_submap_direction() -> String {
    "both".to_string()
}

/// Input for the `query` tool.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct QueryInput {
    /// Query string in the noet textual grammar.
    /// See docs/design/query_model.md §9.5 for the full syntax.
    /// Examples: `"id://my-network composed_of(*)"`, `"title:auth AND schema:procedure"`,
    /// `"id:class-a uses(1) NOT id:class-b uses(1)"`.
    pub query_string: String,
    /// Optional network bref to restrict the query scope.
    #[serde(default)]
    pub network: Option<String>,
}

/// Input for the `check_consistency` tool.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CheckConsistencyInput {
    /// Optional network bref to restrict the check to a single network.
    /// If absent, all loaded networks are checked.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network: Option<String>,
}

/// Input for `get_networks` (no parameters required).
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct GetNetworksInput {}

/// Input for the `bref` tool.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct BrefInput {
    /// The full UUID BID to compute the bref of (hyphenated UUID format,
    /// e.g. `"550e8400-e29b-41d4-a716-446655440000"`).
    pub bid: String,
}

/// Output of the `bref` tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrefOutput {
    /// The input BID, echoed back for clarity.
    pub bid: String,
    /// The 5-character hex bref derived from the BID.
    ///
    /// A bref is the short alias used in paths, edge ownership fields, search
    /// results, and network identifiers throughout the BeliefBase. It is stable
    /// for the lifetime of the BID and can be used interchangeably with the full
    /// BID in most display contexts (but tool inputs expect the full BID).
    pub bref: String,
}

// ── get_traceability ──────────────────────────────────────────────────────────

/// Input for the `get_traceability` tool.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct GetTraceabilityInput {
    /// BID of the entry node. Its home network defines the row set via Section-edge
    /// traversal (same as `get_submap` with Section edges).
    pub bid: String,
    /// Section-edge traversal depth for the row set. `0` = this level only
    /// (subnets appear as opaque single rows). Defaults to 0.
    #[serde(default)]
    pub depth: u8,
    /// Weight kinds to include in the column counts. Defaults to all three.
    /// Valid values: `"section"`, `"epistemic"`, `"pragmatic"`.
    #[serde(default = "default_weight_kinds")]
    pub weight_kinds: Vec<String>,
}

fn default_weight_kinds() -> Vec<String> {
    vec![
        "section".to_string(),
        "epistemic".to_string(),
        "pragmatic".to_string(),
    ]
}

/// Output of the `get_traceability` tool.
///
/// A matrix whose rows are the nodes in the entry node's structural submap
/// (reachable via Section edges) and whose columns are edge counts per
/// WeightKind per direction. This is the direct-mode traceability table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceabilityOutput {
    /// The entry BID that was used to define the scope.
    pub entry_bid: String,
    /// BID of the home network.
    pub entry_home_net: String,
    /// Ordered rows — one per node in the structural submap.
    pub rows: Vec<TraceabilityRow>,
}

/// One row in the traceability matrix.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceabilityRow {
    /// Network-relative path of this node.
    pub path: String,
    pub bid: String,
    pub bref: String,
    pub label: String,
    /// Section edges pointing TO this node (it consists of these nodes as children).
    pub section_in: usize,
    /// Section edges pointing FROM this node (it is a component of these nodes).
    pub section_out: usize,
    /// Epistemic edges pointing TO this node (it draws from these nodes).
    pub epistemic_in: usize,
    /// Epistemic edges pointing FROM this node (it underlies these nodes).
    pub epistemic_out: usize,
    /// Pragmatic edges pointing TO this node (it uses these nodes).
    pub pragmatic_in: usize,
    /// Pragmatic edges pointing FROM this node (it is used by these nodes).
    pub pragmatic_out: usize,
}

// ── get_maps_to ───────────────────────────────────────────────────────────────

/// Input for the `get_maps_to` tool.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct GetMapsToInput {
    /// BIDs of owner nodes to inspect. Each node's `owned_edges` is scanned for
    /// `{maps_to}` claims. Typically the BID list comes from a prior `get_submap`
    /// call on a gap-analysis or traceability network.
    pub bids: Vec<String>,
    /// Weight kinds to include. Defaults to all three.
    #[serde(default = "default_weight_kinds")]
    pub weight_kinds: Vec<String>,
}

/// Output of the `get_maps_to` tool.
///
/// A flat list of owned-edge claims, one entry per (owner, sink, weight_kind)
/// triple. Answers: "what does each of these nodes claim to cover?"
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MapsToOutput {
    /// All claims found, in owner-BID input order.
    pub claims: Vec<MapsToClaim>,
    /// Number of owner BIDs that had at least one claim.
    pub owner_count: usize,
}

/// One `{maps_to}` claim: the owner node asserts that source relates to sink.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MapsToClaim {
    /// The section node that owns this edge via a `{maps_to}` directive.
    pub owner_bid: String,
    pub owner_bref: String,
    pub owner_label: String,
    /// The source endpoint of the owned edge (the "internal" node doing the covering).
    pub source_bid: String,
    pub source_bref: String,
    pub source_label: String,
    /// The sink endpoint of the owned edge (the external thing being covered).
    pub sink_bid: String,
    pub sink_bref: String,
    pub sink_label: String,
    /// The weight kind of this edge: `"section"`, `"epistemic"`, or `"pragmatic"`.
    pub weight_kind: String,
}

// ── get_maps_to_traceability ──────────────────────────────────────────────────

/// Input for the `get_maps_to_traceability` tool.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct GetMapsToTraceabilityInput {
    /// BID of the entry node. Its structural submap (Section-edge traversal) defines
    /// the owner set — these are the nodes whose `{maps_to}` claims are collected.
    pub bid: String,
    /// Section-edge traversal depth. Defaults to 0 (subnets opaque).
    #[serde(default)]
    pub depth: u8,
    /// Weight kinds to include. Defaults to all three.
    #[serde(default = "default_weight_kinds")]
    pub weight_kinds: Vec<String>,
}

/// Output of the `get_maps_to_traceability` tool.
///
/// The full three-level claim index: for each owner node in the structural
/// submap, what does it claim to cover (sinks), and via which source nodes
/// per WeightKind? This is the `maps_to` mode of the traceability matrix —
/// the compliance-review primitive that answers "does every item in my standard
/// have coverage, and what covers it?"
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MapsToTraceabilityOutput {
    /// The entry BID used to define the owner scope.
    pub entry_bid: String,
    /// BID of the home network.
    pub entry_home_net: String,
    /// Owner entries in submap order. Owners with no claims are omitted.
    pub owners: Vec<MapsToOwner>,
}

/// One owner node and all the claims it makes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MapsToOwner {
    pub bid: String,
    pub bref: String,
    pub label: String,
    /// Grouped by sink — what each claim points to.
    pub sink_groups: Vec<MapsToSinkGroup>,
}

/// All claims from one owner that share the same sink.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MapsToSinkGroup {
    pub sink_bid: String,
    pub sink_bref: String,
    pub sink_label: String,
    /// Sources per WeightKind. Only kinds with at least one source are present.
    /// Keys: `"section"`, `"epistemic"`, `"pragmatic"`.
    pub by_kind: std::collections::BTreeMap<String, Vec<MapsToSource>>,
}

/// One source node participating in a (owner → sink) claim.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MapsToSource {
    pub bid: String,
    pub bref: String,
    pub label: String,
}
