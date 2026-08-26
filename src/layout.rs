//! Compile-time layout computation for the 3D credibility map viewer.
//!
//! Computes metadata fields on each [`BeliefNode`](crate::properties::BeliefNode) for the 3D viewer:
//!
//! - **`metadata.assembly_index`** — upstream network count: how many distinct
//!   network-kinded ancestors are reachable via any edge type.
//!
//! - **`metadata.render_position`** — force-settled 3D coordinates in \[0,1\]³
//!   N/S/P space, computed via a two-level force simulation:
//!   1. **Bubble layout**: network-level graph positioned by aggregate profiles.
//!   2. **Intra-bubble layout**: per-network nodes positioned by merged
//!      (lexical + structural) content profiles.
//!
//! - **`metadata.structural_weight`** — per-node structural weight: fraction
//!   of the network's section edges owned by this node.
//!
//! - **`metadata.structural_depth`** (network nodes only) — ratio of maximum
//!   pathmap order depth to node count. High = deeply hierarchical, low = flat.
//!
//! The pipeline:
//! 1. Per-node: compute edge counts → `score_structural` → `score_merge` with
//!    existing lexical `content_profile` → merged profile with S-axis signal.
//! 2. Per-network: aggregate merged profiles + structural depth metric.
//! 3. Level 1 layout: force simulation on condensed network graph.
//! 4. Level 2 layout: per-network force simulation on constituent nodes.
//!
//! # Scope: which networks get laid out
//!
//! Layout applies the N/S/P content-type ontology (see
//! `docs/essays/engineering_model_ontology.md` §3). That ontology's assumptions
//! only hold for user-authored engineering content, so two classes of network
//! are excluded:
//!
//! - **Reserved/const namespaces** ([`Bid::is_reserved`]) — the href-tracking,
//!   asset-tracking, API and codec namespaces. These hold synthetic
//!   `External|Trace` bookkeeping nodes (one per outbound hyperlink, one per
//!   asset, …). They are not authored, are not browsable in the viewer, and
//!   carry no meaningful normative/structural/procedural signal — scoring them
//!   against the ontology is category error, not merely wasteful.
//!
//! - **Networks above [`LayoutConfig::max_nodes`]** — the intra-bubble
//!   simulation is `O(iterations · n²)` in a network's node count, so a single
//!   oversized network can dominate the entire build. Such networks are skipped
//!   with a warning rather than silently stalling the build.
//!
//! Both exclusions are *whole-network*: excluded networks receive no
//! `render_position`, `structural_weight` or `structural_depth`. Consumers must
//! treat these fields as optional.
//!
//! Called from [`crate::codec::compiler::DocumentCompiler::finalize_html`] after
//! the full [`BeliefGraph`] is materialized and before it is serialized to
//! msgpack.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use petgraph::visit::{EdgeRef, IntoEdgeReferences};

use crate::{
    beliefbase::BeliefGraph,
    paths::pathmap::PathMapMap,
    properties::{Bid, WeightKind},
    shard::content_type::{score_merge, score_structural, ContentProfile, EdgeCounts},
};

// ───────────────────────────────────────────────────────────────────────────
// Configuration
// ───────────────────────────────────────────────────────────────────────────

/// Default ceiling on a network's node count before layout is skipped for it.
///
/// The intra-bubble force simulation is `O(iterations · n²)`, so cost grows
/// quadratically in the largest network. This default is chosen so that a
/// single network contributes at most a few seconds: at n = 5,000 the pair
/// loop is ~2.5e9 operations across all iterations.
///
/// Override with `--layout-max-nodes N` or `NOET_LAYOUT_MAX_NODES=N`.
pub const DEFAULT_LAYOUT_MAX_NODES: usize = 5_000;

/// Environment variable overriding [`DEFAULT_LAYOUT_MAX_NODES`].
pub const LAYOUT_MAX_NODES_ENV: &str = "NOET_LAYOUT_MAX_NODES";

/// Controls whether and how layout metadata is computed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LayoutConfig {
    /// When false, [`compute_layout_metadata`] is a no-op (`--no-layout`).
    pub enabled: bool,
    /// Networks with more than this many nodes are skipped with a warning.
    pub max_nodes: usize,
}

impl Default for LayoutConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_nodes: DEFAULT_LAYOUT_MAX_NODES,
        }
    }
}

impl LayoutConfig {
    /// Resolve the node ceiling: explicit value > env var > default.
    ///
    /// A malformed or zero env value is ignored (with a warning) rather than
    /// disabling layout by accident.
    pub fn resolve_max_nodes(explicit: Option<usize>) -> usize {
        if let Some(n) = explicit {
            return n;
        }
        match std::env::var(LAYOUT_MAX_NODES_ENV) {
            Ok(raw) => match raw.trim().parse::<usize>() {
                Ok(n) if n > 0 => n,
                _ => {
                    tracing::warn!(
                        "{LAYOUT_MAX_NODES_ENV}={raw:?} is not a positive integer — \
                         using default {DEFAULT_LAYOUT_MAX_NODES}"
                    );
                    DEFAULT_LAYOUT_MAX_NODES
                }
            },
            Err(_) => DEFAULT_LAYOUT_MAX_NODES,
        }
    }
}

// ───────────────────────────────────────────────────────────────────────────
// Public entry point
// ───────────────────────────────────────────────────────────────────────────

/// Compute layout metadata and write it into `graph.states` in place.
///
/// This is the single call site from `finalize_html`. It computes assembly
/// indices, structural scores, and render positions, then mutates each
/// node's `metadata` table.
pub fn compute_layout_metadata(
    graph: &mut BeliefGraph,
    pathmap: &PathMapMap,
    config: &LayoutConfig,
) {
    if !config.enabled {
        tracing::debug!("[layout] disabled — skipping layout metadata");
        return;
    }

    // Step 1: Resolve every node's home network and decide which networks are
    // in scope, in a single pass. `pathmap.path()` is O(networks) — it probes
    // every PathMap and takes the minimum — so it must be called at most once
    // per node. Selection and mapping are fused for exactly that reason; see
    // `resolve_scope`.
    let (home_networks, in_scope) = resolve_scope(graph, pathmap, config);

    // Steps 2-7 are individually timed. This stage was previously attributed as
    // a single opaque block, which led to the wrong cost being optimised; keep
    // the per-step timers so any future regression is attributable on sight.
    macro_rules! timed {
        ($name:literal, $work:expr) => {{
            let start = std::time::Instant::now();
            let out = $work;
            tracing::debug!(
                target: "noet_core::codec::perf",
                elapsed_ms = start.elapsed().as_millis(),
                concat!("[layout step] ", $name),
            );
            out
        }};
    }

    // Step 2: Compute per-node edge counts and structural scores.
    let edge_counts = timed!(
        "compute_edge_counts",
        compute_edge_counts(graph, &home_networks)
    );
    let merged_profiles = timed!(
        "compute_merged_profiles",
        compute_merged_profiles(graph, &edge_counts)
    );

    // Step 3: Build the condensed network-level graph.
    let condensed = timed!(
        "build_condensed_network_graph",
        build_condensed_network_graph(graph, &home_networks, &in_scope)
    );

    // Step 4: Compute assembly indices.
    let assembly_indices = timed!(
        "compute_assembly_indices",
        compute_assembly_indices(&condensed)
    );

    // Step 5: Compute per-network aggregates (mean profile + structural depth).
    let network_aggregates = timed!(
        "compute_network_aggregates",
        compute_network_aggregates(&merged_profiles, &home_networks, pathmap, &condensed)
    );

    // Step 6: Two-level layout.
    let render_positions = timed!(
        "compute_render_positions",
        compute_render_positions(
            graph,
            &home_networks,
            &condensed,
            &merged_profiles,
            &network_aggregates,
        )
    );

    // Step 7: Compute per-node structural weight within each network.
    let structural_weights = timed!(
        "compute_structural_weights",
        compute_structural_weights(&edge_counts, &home_networks)
    );

    // Step 8: Write everything into node metadata.
    for (bid, node) in graph.states.iter_mut() {
        if let Some(&ai) = assembly_indices.get(bid) {
            node.metadata.insert(
                "assembly_index".to_string(),
                toml::Value::Integer(ai as i64),
            );
        }
        if let Some(pos) = render_positions.get(bid) {
            let mut table = toml::Table::new();
            table.insert("n".to_string(), toml::Value::Float(pos[0]));
            table.insert("s".to_string(), toml::Value::Float(pos[1]));
            table.insert("p".to_string(), toml::Value::Float(pos[2]));
            node.metadata
                .insert("render_position".to_string(), toml::Value::Table(table));
        }
        if let Some(&sw) = structural_weights.get(bid) {
            node.metadata
                .insert("structural_weight".to_string(), toml::Value::Float(sw));
        }
        if let Some(agg) = network_aggregates.get(bid) {
            node.metadata.insert(
                "structural_depth".to_string(),
                toml::Value::Float(agg.structural_depth),
            );
        }
    }
}

// ───────────────────────────────────────────────────────────────────────────
// Network selection
// ───────────────────────────────────────────────────────────────────────────

/// Resolve each node's home network and the set of networks in layout scope.
///
/// Returns `(home_networks, in_scope)` where `home_networks` contains only
/// nodes whose home network survived selection — that omission is what keeps
/// excluded networks out of every downstream step.
///
/// # Why this is one function
///
/// [`PathMapMap::path`] is `O(networks)`: it probes every network's `PathMap`
/// and takes the minimum by path order. On a large corpus that is the dominant
/// cost of this whole stage — far larger than the force simulation — so it must
/// be called **at most once per node**.
///
/// Selection needs per-network node counts, and mapping needs per-node home
/// networks; both derive from the same lookup. Splitting them into two passes
/// doubles the stage's dominant cost, so they are deliberately fused. Do not
/// separate them for tidiness.
fn resolve_scope(
    graph: &BeliefGraph,
    pathmap: &PathMapMap,
    config: &LayoutConfig,
) -> (BTreeMap<Bid, Bid>, BTreeSet<Bid>) {
    // Single O(nodes x networks) pass: resolve home network and tally counts.
    let resolve_start = std::time::Instant::now();
    let mut resolved: Vec<(Bid, Bid)> = Vec::with_capacity(graph.states.len());
    let mut node_counts: BTreeMap<Bid, usize> = BTreeMap::new();
    for &bid in graph.states.keys() {
        if let Some((net_bid, _path)) = pathmap.path(&bid) {
            *node_counts.entry(net_bid).or_default() += 1;
            resolved.push((bid, net_bid));
        }
    }

    let mut selected = BTreeSet::new();
    for &net_bid in pathmap.nets() {
        if net_bid.is_reserved() {
            tracing::debug!(
                net = %net_bid.bref(),
                nodes = node_counts.get(&net_bid).copied().unwrap_or(0),
                "[layout] skipping reserved namespace — no N/S/P semantics",
            );
            continue;
        }
        let count = node_counts.get(&net_bid).copied().unwrap_or(0);
        if count > config.max_nodes {
            tracing::warn!(
                net = %net_bid.bref(),
                nodes = count,
                max_nodes = config.max_nodes,
                "[layout] skipping oversized network — layout is O(n^2) in node \
                 count; raise --layout-max-nodes / {LAYOUT_MAX_NODES_ENV} to include it",
            );
            continue;
        }
        selected.insert(net_bid);
    }

    // Reuse the already-resolved pairs; no second pathmap traversal.
    let home_networks: BTreeMap<Bid, Bid> = resolved
        .into_iter()
        .filter(|(_, net_bid)| selected.contains(net_bid))
        .collect();

    // `subnet_holding_nets` is the width of `indexed_path`'s candidate set: a
    // node's lookup probes its direct networks plus every subnet-holding parent.
    // If this approaches `total`, the narrowing has stopped paying and the
    // reverse index is no longer buying anything.
    let subnet_holding_nets = pathmap
        .map()
        .values()
        .filter(|pm| !pm.read_arc().subnets().is_empty())
        .count();
    // Split the resolution cost across `indexed_path`'s two routes. The
    // narrowed route probes a handful of candidates; the fallback probes every
    // network. Reporting only the total hides which one to attack — the
    // mistake this stage has already made twice.
    let (ix_calls, fb_calls, ix_probes) = crate::paths::pathmap::indexed_path_stats();
    tracing::debug!(
        target: "noet_core::codec::perf",
        selected = selected.len(),
        total = pathmap.nets().len(),
        subnet_holding_nets,
        mapped_nodes = home_networks.len(),
        indexed_calls = ix_calls,
        fallback_calls = fb_calls,
        indexed_probes = ix_probes,
        elapsed_ms = resolve_start.elapsed().as_millis(),
        "[layout] scope resolution complete",
    );
    (home_networks, selected)
}

// ───────────────────────────────────────────────────────────────────────────
// Per-node edge counts and structural scoring
// ───────────────────────────────────────────────────────────────────────────

/// Count edges per node by WeightKind and direction.
///
/// Only counts intra-network edges (source and sink in the same network)
/// for structural scoring. Inter-network edges contribute to the condensed
/// graph instead.
fn compute_edge_counts(
    graph: &BeliefGraph,
    home_networks: &BTreeMap<Bid, Bid>,
) -> BTreeMap<Bid, EdgeCounts> {
    let mut counts: BTreeMap<Bid, EdgeCounts> = BTreeMap::new();

    let g = graph.relations.as_graph();
    for edge_ref in g.edge_references() {
        let source_bid = g[edge_ref.source()];
        let sink_bid = g[edge_ref.target()];

        // Only count intra-network edges for structural scoring.
        let source_net = home_networks.get(&source_bid);
        let sink_net = home_networks.get(&sink_bid);
        if source_net != sink_net {
            continue;
        }

        for &kind in edge_ref.weight().weights.keys() {
            // Source has an outgoing edge, sink has an incoming edge.
            let src = counts.entry(source_bid).or_default();
            match kind {
                WeightKind::Section => src.section_out += 1,
                WeightKind::Epistemic => src.epistemic_out += 1,
                WeightKind::Pragmatic => src.pragmatic_out += 1,
            }

            let snk = counts.entry(sink_bid).or_default();
            match kind {
                WeightKind::Section => snk.section_in += 1,
                WeightKind::Epistemic => snk.epistemic_in += 1,
                WeightKind::Pragmatic => snk.pragmatic_in += 1,
            }
        }
    }

    counts
}

/// Compute merged (lexical + structural) profiles for each node.
///
/// Uses the existing `metadata.content_profile` (from Issue 91) as the
/// lexical channel, and `score_structural` on edge counts as the structural
/// channel. Blends via `score_merge` to produce a profile with S-axis signal.
fn compute_merged_profiles(
    graph: &BeliefGraph,
    edge_counts: &BTreeMap<Bid, EdgeCounts>,
) -> BTreeMap<Bid, ContentProfile> {
    let mut profiles = BTreeMap::new();

    for (&bid, node) in &graph.states {
        let lexical = read_content_profile(node).unwrap_or_default();

        let structural = edge_counts
            .get(&bid)
            .map(score_structural)
            .unwrap_or_default();

        let merged = score_merge(&lexical, &structural);
        profiles.insert(bid, merged.profile);
    }

    profiles
}

// ───────────────────────────────────────────────────────────────────────────
// Per-node structural weight
// ───────────────────────────────────────────────────────────────────────────

/// Compute per-node structural weight: this node's section edge count divided
/// by the total section edges in its home network.
///
/// A node with many section edges (many children/parents in the document tree)
/// "owns" a larger share of the network's skeleton.
fn compute_structural_weights(
    edge_counts: &BTreeMap<Bid, EdgeCounts>,
    home_networks: &BTreeMap<Bid, Bid>,
) -> BTreeMap<Bid, f64> {
    // Sum total section edges per network.
    let mut net_section_totals: BTreeMap<Bid, u32> = BTreeMap::new();
    for (&bid, ec) in edge_counts {
        if let Some(&net_bid) = home_networks.get(&bid) {
            *net_section_totals.entry(net_bid).or_default() += ec.section_in + ec.section_out;
        }
    }

    let mut weights = BTreeMap::new();
    for (&bid, ec) in edge_counts {
        if let Some(&net_bid) = home_networks.get(&bid) {
            let total = net_section_totals.get(&net_bid).copied().unwrap_or(0);
            if total > 0 {
                let node_section = (ec.section_in + ec.section_out) as f64;
                weights.insert(bid, node_section / total as f64);
            } else {
                weights.insert(bid, 0.0);
            }
        }
    }

    weights
}

// ───────────────────────────────────────────────────────────────────────────
// Condensed network graph
// ───────────────────────────────────────────────────────────────────────────

/// The condensed network-level graph with typed edges.
struct CondensedNetworkGraph {
    graph: petgraph::stable_graph::StableGraph<Bid, BTreeSet<WeightKind>>,
    idx_map: BTreeMap<Bid, petgraph::stable_graph::NodeIndex>,
    net_bids: BTreeSet<Bid>,
}

/// Build a condensed network-level directed graph from the full belief graph.
///
/// For each inter-network edge (source in network A, sink in network B, A≠B),
/// adds a directed edge A→B with the union of WeightKinds observed on edges
/// between those networks.
fn build_condensed_network_graph(
    graph: &BeliefGraph,
    home_networks: &BTreeMap<Bid, Bid>,
    in_scope: &BTreeSet<Bid>,
) -> CondensedNetworkGraph {
    let net_bids: BTreeSet<Bid> = in_scope.clone();

    let mut net_edge_kinds: BTreeMap<(Bid, Bid), BTreeSet<WeightKind>> = BTreeMap::new();
    let g = graph.relations.as_graph();
    for edge_ref in g.edge_references() {
        let source_bid = g[edge_ref.source()];
        let sink_bid = g[edge_ref.target()];
        let source_net = home_networks.get(&source_bid).copied();
        let sink_net = home_networks.get(&sink_bid).copied();
        if let (Some(sn), Some(dn)) = (source_net, sink_net) {
            if sn != dn {
                let kinds = net_edge_kinds.entry((sn, dn)).or_default();
                for &kind in edge_ref.weight().weights.keys() {
                    kinds.insert(kind);
                }
            }
        }
    }

    let mut net_graph = petgraph::stable_graph::StableGraph::<Bid, BTreeSet<WeightKind>>::new();
    let mut idx_map: BTreeMap<Bid, petgraph::stable_graph::NodeIndex> = BTreeMap::new();

    for &net_bid in &net_bids {
        let idx = net_graph.add_node(net_bid);
        idx_map.insert(net_bid, idx);
    }

    for ((src, dst), kinds) in &net_edge_kinds {
        if let (Some(&src_idx), Some(&dst_idx)) = (idx_map.get(src), idx_map.get(dst)) {
            net_graph.add_edge(src_idx, dst_idx, kinds.clone());
        }
    }

    CondensedNetworkGraph {
        graph: net_graph,
        idx_map,
        net_bids,
    }
}

// ───────────────────────────────────────────────────────────────────────────
// Assembly index
// ───────────────────────────────────────────────────────────────────────────

/// Compute the assembly index for each network: the count of distinct upstream
/// networks reachable via any edge type in the condensed graph.
fn compute_assembly_indices(condensed: &CondensedNetworkGraph) -> BTreeMap<Bid, u64> {
    let mut network_assembly: BTreeMap<Bid, u64> = BTreeMap::new();

    for &net_bid in &condensed.net_bids {
        let Some(&start_idx) = condensed.idx_map.get(&net_bid) else {
            continue;
        };
        let mut visited = BTreeSet::new();
        let mut queue = VecDeque::new();
        queue.push_back(start_idx);
        visited.insert(start_idx);

        while let Some(current) = queue.pop_front() {
            for neighbor in condensed
                .graph
                .neighbors_directed(current, petgraph::Direction::Incoming)
            {
                if visited.insert(neighbor) {
                    queue.push_back(neighbor);
                }
            }
        }

        let upstream_count = (visited.len() - 1) as u64;
        network_assembly.insert(net_bid, upstream_count);
    }

    network_assembly
}

// ───────────────────────────────────────────────────────────────────────────
// Network-level aggregates
// ───────────────────────────────────────────────────────────────────────────

/// Aggregate metrics for a single network, used for bubble positioning.
#[derive(Debug, Clone)]
struct NetworkAggregate {
    /// Mean merged content profile of all constituent nodes.
    mean_profile: ContentProfile,
    /// Structural depth: max(order_vec_length) / node_count.
    /// High = deeply hierarchical, low = flat.
    structural_depth: f64,
    /// Number of constituent nodes (used by viewer for bubble sizing).
    #[allow(dead_code)]
    node_count: usize,
}

/// Compute per-network aggregate metrics.
fn compute_network_aggregates(
    merged_profiles: &BTreeMap<Bid, ContentProfile>,
    home_networks: &BTreeMap<Bid, Bid>,
    pathmap: &PathMapMap,
    condensed: &CondensedNetworkGraph,
) -> BTreeMap<Bid, NetworkAggregate> {
    // Group nodes by network and accumulate profiles.
    let mut net_profiles: BTreeMap<Bid, Vec<&ContentProfile>> = BTreeMap::new();
    for (&bid, &net_bid) in home_networks {
        if condensed.net_bids.contains(&bid) {
            continue; // Skip network nodes themselves.
        }
        if let Some(profile) = merged_profiles.get(&bid) {
            net_profiles.entry(net_bid).or_default().push(profile);
        }
    }

    let mut aggregates = BTreeMap::new();

    for &net_bid in &condensed.net_bids {
        let profiles = net_profiles.get(&net_bid);
        let node_count = profiles.map(|p| p.len()).unwrap_or(0);

        // Mean profile.
        let mean_profile = if let Some(profiles) = profiles {
            if profiles.is_empty() {
                ContentProfile::default()
            } else {
                let count = profiles.len() as f32;
                let mut sum_n = 0.0f32;
                let mut sum_s = 0.0f32;
                let mut sum_p = 0.0f32;
                let mut sum_r = 0.0f32;
                for p in profiles {
                    sum_n += p.n;
                    sum_s += p.s;
                    sum_p += p.p;
                    sum_r += p.r;
                }
                ContentProfile {
                    n: sum_n / count,
                    s: sum_s / count,
                    p: sum_p / count,
                    r: sum_r / count,
                }
            }
        } else {
            ContentProfile::default()
        };

        // Structural depth: max order vec length in this network's own
        // pathmap (depth=0 keeps subnets opaque so we measure only this
        // network's hierarchy, not its children's).
        let bref = net_bid.bref();
        let max_depth = pathmap
            .submap(&bref, "", 0, true)
            .iter()
            .map(|(_path, _bid, order)| order.len())
            .max()
            .unwrap_or(0);

        let structural_depth = if node_count > 0 {
            max_depth as f64 / node_count as f64
        } else {
            0.0
        };

        aggregates.insert(
            net_bid,
            NetworkAggregate {
                mean_profile,
                structural_depth,
                node_count,
            },
        );
    }

    aggregates
}

// ───────────────────────────────────────────────────────────────────────────
// Force-directed layout
// ───────────────────────────────────────────────────────────────────────────

/// 3D position vector.
#[derive(Debug, Clone, Copy)]
struct Vec3 {
    x: f64,
    y: f64,
    z: f64,
}

impl Vec3 {
    fn new(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }

    fn zero() -> Self {
        Self::new(0.0, 0.0, 0.0)
    }

    fn add(self, other: Self) -> Self {
        Self::new(self.x + other.x, self.y + other.y, self.z + other.z)
    }

    fn sub(self, other: Self) -> Self {
        Self::new(self.x - other.x, self.y - other.y, self.z - other.z)
    }

    fn scale(self, s: f64) -> Self {
        Self::new(self.x * s, self.y * s, self.z * s)
    }

    fn length(self) -> f64 {
        (self.x * self.x + self.y * self.y + self.z * self.z).sqrt()
    }

    fn clamp_to_unit(self) -> Self {
        Self::new(
            self.x.clamp(0.0, 1.0),
            self.y.clamp(0.0, 1.0),
            self.z.clamp(0.0, 1.0),
        )
    }
}

/// Gravity impulse direction for each WeightKind.
fn gravity_impulse(kind: WeightKind) -> Vec3 {
    match kind {
        WeightKind::Epistemic => Vec3::new(1.0, 0.0, 0.0), // push N higher
        WeightKind::Section => Vec3::new(0.0, 1.0, 0.0),   // push S higher
        WeightKind::Pragmatic => Vec3::new(0.0, 0.0, 1.0), // push P higher
    }
}

/// Read a node's content_profile from its metadata.
fn read_content_profile(node: &crate::properties::BeliefNode) -> Option<ContentProfile> {
    let cp = node.metadata.get("content_profile")?.as_table()?;
    Some(ContentProfile {
        n: cp.get("n")?.as_float()? as f32,
        s: cp.get("s")?.as_float()? as f32,
        p: cp.get("p")?.as_float()? as f32,
        r: cp.get("r").and_then(|v| v.as_float()).unwrap_or(0.0) as f32,
    })
}

/// Convert a ContentProfile to a Vec3 position (n, s, p).
fn profile_to_vec3(p: &ContentProfile) -> Vec3 {
    Vec3::new(p.n as f64, p.s as f64, p.p as f64)
}

/// Tunable parameters for the force simulation.
struct ForceParams {
    iterations: usize,
    repulsion: f64,
    spring: f64,
    gravity: f64,
    center_pull: f64,
    initial_temp: f64,
    min_temp: f64,
    cooling: f64,
    damping: f64,
}

/// Parameters for intra-network (node-level) layout.
///
/// Tuning notes (v2): gravity reduced and centering increased relative to
/// v1 to prevent positions saturating at [0,1] boundaries.  The gravity
/// impulse is per-edge, so nodes with many edges accumulated too much
/// directional force and slammed into walls.  Weaker gravity + stronger
/// centering keeps nodes differentiated within the interior of the space.
const INTRA_PARAMS: ForceParams = ForceParams {
    iterations: 200,
    repulsion: 0.001,
    spring: 0.003,
    gravity: 0.005,
    center_pull: 0.008,
    initial_temp: 0.08,
    min_temp: 0.001,
    cooling: 0.98,
    damping: 0.85,
};

/// Parameters for inter-network (bubble-level) layout.
/// Fewer nodes, stronger forces, quicker convergence.
const INTER_PARAMS: ForceParams = ForceParams {
    iterations: 150,
    repulsion: 0.003,
    spring: 0.005,
    gravity: 0.008,
    center_pull: 0.006,
    initial_temp: 0.1,
    min_temp: 0.002,
    cooling: 0.97,
    damping: 0.8,
};

// ───────────────────────────────────────────────────────────────────────────
// Two-level layout
// ───────────────────────────────────────────────────────────────────────────

/// Compute force-settled render positions using a two-level approach:
///
/// 1. **Bubble layout**: force simulation on the condensed network graph.
///    Network positions seeded from aggregate merged profiles.
///
/// 2. **Intra-bubble layout**: per-network force simulation on constituent
///    nodes, centered on the bubble position. Node positions seeded from
///    merged (lexical + structural) profiles.
fn compute_render_positions(
    graph: &BeliefGraph,
    home_networks: &BTreeMap<Bid, Bid>,
    condensed: &CondensedNetworkGraph,
    merged_profiles: &BTreeMap<Bid, ContentProfile>,
    network_aggregates: &BTreeMap<Bid, NetworkAggregate>,
) -> BTreeMap<Bid, [f64; 3]> {
    let mut result = BTreeMap::new();

    // Level 1: Bubble layout.
    let bubble_positions = run_bubble_layout(condensed, network_aggregates);
    for (&net_bid, &pos) in &bubble_positions {
        result.insert(net_bid, pos);
    }

    // Level 2: Intra-bubble layout.
    let mut network_nodes: BTreeMap<Bid, Vec<Bid>> = BTreeMap::new();
    for (&bid, &net_bid) in home_networks {
        if condensed.net_bids.contains(&bid) {
            continue;
        }
        network_nodes.entry(net_bid).or_default().push(bid);
    }

    // Bucket intra-network edges by home network in a single pass over the
    // graph. Previously each network rescanned the whole edge list, making this
    // O(networks x E); it is now O(E) total.
    let mut edges_by_net: BTreeMap<Bid, Vec<(Bid, Bid, WeightKind)>> = BTreeMap::new();
    let g = graph.relations.as_graph();
    for edge_ref in g.edge_references() {
        let source_bid = g[edge_ref.source()];
        let sink_bid = g[edge_ref.target()];
        let (Some(&src_net), Some(&sink_net)) =
            (home_networks.get(&source_bid), home_networks.get(&sink_bid))
        else {
            continue;
        };
        if src_net != sink_net {
            continue;
        }
        let bucket = edges_by_net.entry(src_net).or_default();
        for &kind in edge_ref.weight().weights.keys() {
            bucket.push((source_bid, sink_bid, kind));
        }
    }

    for (&net_bid, nodes) in &network_nodes {
        if nodes.is_empty() {
            continue;
        }
        let bubble_center = bubble_positions
            .get(&net_bid)
            .copied()
            .unwrap_or([0.5, 0.5, 0.5]);

        let net_edges = edges_by_net.get(&net_bid).map_or(&[][..], |v| v.as_slice());
        let positions = run_intra_bubble_layout(nodes, net_edges, bubble_center, merged_profiles);
        for (&bid, pos) in nodes.iter().zip(positions.iter()) {
            result.insert(bid, *pos);
        }
    }

    // Default for in-scope networks that ended up with no position.
    for &net_bid in &condensed.net_bids {
        result.entry(net_bid).or_insert([0.5, 0.5, 0.5]);
    }

    result
}

/// Run force simulation on the condensed network graph to position bubbles.
fn run_bubble_layout(
    condensed: &CondensedNetworkGraph,
    network_aggregates: &BTreeMap<Bid, NetworkAggregate>,
) -> BTreeMap<Bid, [f64; 3]> {
    let net_list: Vec<Bid> = condensed.net_bids.iter().copied().collect();
    let n = net_list.len();
    if n == 0 {
        return BTreeMap::new();
    }

    let bid_to_local: BTreeMap<Bid, usize> = net_list
        .iter()
        .enumerate()
        .map(|(i, &bid)| (bid, i))
        .collect();

    // Seed from aggregate profiles.
    let mut positions: Vec<Vec3> = net_list
        .iter()
        .enumerate()
        .map(|(i, net_bid)| {
            network_aggregates
                .get(net_bid)
                .map(|agg| profile_to_vec3(&agg.mean_profile))
                .unwrap_or_else(|| golden_spiral_position(i, n))
        })
        .collect();

    // Collect condensed edges with WeightKinds.
    let mut edges: Vec<(usize, usize, WeightKind)> = Vec::new();
    for edge_ref in condensed.graph.edge_references() {
        let src_bid = condensed.graph[edge_ref.source()];
        let dst_bid = condensed.graph[edge_ref.target()];
        if let (Some(&si), Some(&di)) = (bid_to_local.get(&src_bid), bid_to_local.get(&dst_bid)) {
            for &kind in edge_ref.weight() {
                edges.push((si, di, kind));
            }
        }
    }

    run_force_simulation_core(&mut positions, &edges, &INTER_PARAMS);

    net_list
        .into_iter()
        .zip(positions)
        .map(|(bid, p)| (bid, [p.x, p.y, p.z]))
        .collect()
}

/// Run force simulation on nodes within a single network bubble.
///
/// Nodes start at `bubble_center + (merged_profile - 0.5) * spread`.
fn run_intra_bubble_layout(
    nodes: &[Bid],
    net_edges: &[(Bid, Bid, WeightKind)],
    bubble_center: [f64; 3],
    merged_profiles: &BTreeMap<Bid, ContentProfile>,
) -> Vec<[f64; 3]> {
    let n = nodes.len();
    if n == 0 {
        return Vec::new();
    }

    let center = Vec3::new(bubble_center[0], bubble_center[1], bubble_center[2]);
    let spread = 0.3;

    let bid_to_local: BTreeMap<Bid, usize> =
        nodes.iter().enumerate().map(|(i, &bid)| (bid, i)).collect();

    // Seed from merged profiles offset from bubble center.
    let mut positions: Vec<Vec3> = nodes
        .iter()
        .enumerate()
        .map(|(i, bid)| {
            let profile_vec = merged_profiles
                .get(bid)
                .map(profile_to_vec3)
                .unwrap_or_else(|| golden_spiral_position(i, n));

            let offset = profile_vec.sub(Vec3::new(0.5, 0.5, 0.5)).scale(spread);
            center.add(offset).clamp_to_unit()
        })
        .collect();

    // Map this network's pre-bucketed edges into local indices.
    let mut local_edges: Vec<(usize, usize, WeightKind)> = Vec::new();
    for &(source_bid, sink_bid, kind) in net_edges {
        if let (Some(&si), Some(&di)) = (bid_to_local.get(&source_bid), bid_to_local.get(&sink_bid))
        {
            local_edges.push((si, di, kind));
        }
    }

    run_force_simulation_core(&mut positions, &local_edges, &INTRA_PARAMS);

    positions.into_iter().map(|p| [p.x, p.y, p.z]).collect()
}

/// Deterministic fallback position using a golden-ratio spiral in 3D.
fn golden_spiral_position(i: usize, n: usize) -> Vec3 {
    let phi = std::f64::consts::PI * (3.0 - 5.0_f64.sqrt());
    let t = i as f64 / n.max(1) as f64;
    Vec3::new(
        0.3 + 0.4 * t,
        0.3 + 0.4 * (phi * i as f64).sin() * 0.5,
        0.3 + 0.4 * (phi * i as f64).cos() * 0.5,
    )
}

// ───────────────────────────────────────────────────────────────────────────
// Core force simulation
// ───────────────────────────────────────────────────────────────────────────

/// Core velocity-Verlet force simulation shared by both layout levels.
fn run_force_simulation_core(
    positions: &mut [Vec3],
    edges: &[(usize, usize, WeightKind)],
    params: &ForceParams,
) {
    let n = positions.len();
    if n == 0 {
        return;
    }

    let mut velocities = vec![Vec3::zero(); n];
    let mut temperature = params.initial_temp;

    // Center = centroid of initial positions.
    let mut center = Vec3::zero();
    for p in positions.iter() {
        center = center.add(*p);
    }
    center = center.scale(1.0 / n as f64);

    // Pre-compute per-node gravity bias: accumulate the weighted direction
    // from all edges, then normalize so each node gets one unit of bias
    // regardless of how many edges it has. This prevents high-degree nodes
    // from saturating at [0,1] boundaries.
    let mut gravity_accum = vec![Vec3::zero(); n];
    let mut gravity_count = vec![0u32; n];
    for &(si, di, kind) in edges {
        let impulse = gravity_impulse(kind);
        gravity_accum[si] = gravity_accum[si].add(impulse);
        gravity_accum[di] = gravity_accum[di].add(impulse);
        gravity_count[si] += 1;
        gravity_count[di] += 1;
    }
    // Normalize: each node's gravity bias is the mean direction of its edges.
    let gravity_bias: Vec<Vec3> = gravity_accum
        .into_iter()
        .zip(gravity_count.iter())
        .map(|(acc, &count)| {
            if count > 0 {
                acc.scale(1.0 / count as f64)
            } else {
                Vec3::zero()
            }
        })
        .collect();

    for _iter in 0..params.iterations {
        let mut forces = vec![Vec3::zero(); n];

        // 1. Charge repulsion (O(n²)).
        for i in 0..n {
            for j in (i + 1)..n {
                let delta = positions[i].sub(positions[j]);
                let dist = delta.length().max(0.001);
                let repulsion = params.repulsion / (dist * dist);
                let force = delta.scale(repulsion / dist);
                forces[i] = forces[i].add(force);
                forces[j] = forces[j].sub(force);
            }
        }

        // 2. Edge spring attraction.
        for &(si, di, _kind) in edges {
            let delta = positions[di].sub(positions[si]);
            let dist = delta.length().max(0.001);
            let spring = delta.scale(params.spring * dist);
            forces[si] = forces[si].add(spring);
            forces[di] = forces[di].sub(spring);
        }

        // 3. Typed gravity: apply pre-computed normalized bias.
        for i in 0..n {
            forces[i] = forces[i].add(gravity_bias[i].scale(params.gravity));
        }

        // 4. Centering force.
        for i in 0..n {
            let to_center = center.sub(positions[i]).scale(params.center_pull);
            forces[i] = forces[i].add(to_center);
        }

        // 5. Apply forces with temperature-limited displacement.
        for i in 0..n {
            velocities[i] = velocities[i].add(forces[i]).scale(params.damping);
            let speed = velocities[i].length();
            if speed > temperature {
                velocities[i] = velocities[i].scale(temperature / speed);
            }
            positions[i] = positions[i].add(velocities[i]).clamp_to_unit();
        }

        temperature = (temperature * params.cooling).max(params.min_temp);
    }
}

// ───────────────────────────────────────────────────────────────────────────
// Tests
// ───────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::beliefbase::{BeliefGraph, BidGraph};
    use crate::properties::{BeliefKindSet, BeliefNode, Bid, NodeId, Weight, WeightSet};

    fn make_node(bid: Bid, profile: Option<(f64, f64, f64)>) -> BeliefNode {
        let mut metadata = toml::Table::new();
        if let Some((n, s, p)) = profile {
            let mut cp = toml::Table::new();
            cp.insert("n".to_string(), toml::Value::Float(n));
            cp.insert("s".to_string(), toml::Value::Float(s));
            cp.insert("p".to_string(), toml::Value::Float(p));
            cp.insert("r".to_string(), toml::Value::Float(0.0));
            metadata.insert("content_profile".to_string(), toml::Value::Table(cp));
        }
        BeliefNode {
            bid,
            kind: BeliefKindSet::default(),
            title: String::new(),
            schema: None,
            payload: toml::Table::new(),
            id: NodeId::Slug,
            metadata,
        }
    }

    fn make_weight(kind: WeightKind) -> WeightSet {
        let mut ws = WeightSet {
            weights: BTreeMap::new(),
        };
        ws.weights.insert(kind, Weight::default());
        ws
    }

    fn make_graph(
        bids: &[Bid],
        profiles: &[Option<(f64, f64, f64)>],
        edges: Vec<(Bid, Bid, WeightSet)>,
    ) -> BeliefGraph {
        let mut states = rustc_hash::FxHashMap::default();
        for (bid, profile) in bids.iter().zip(profiles.iter()) {
            states.insert(*bid, make_node(*bid, *profile));
        }
        let relations = BidGraph::from_edges(edges);
        BeliefGraph { states, relations }
    }

    // ── Scope / configuration ───────────────────────────────────────────────

    /// Every const namespace must be recognised as reserved, and so excluded
    /// from layout. This is the property `select_networks` relies on.
    #[test]
    fn const_namespaces_are_reserved() {
        for ns in crate::properties::const_namespaces() {
            assert!(
                ns.is_reserved(),
                "const namespace {ns} must be reserved so layout excludes it"
            );
        }
    }

    /// Nodes minted *inside* a reserved namespace (e.g. one per outbound href)
    /// must also be reserved — otherwise exclusion would only drop the network
    /// node and still lay out its 78k children.
    #[test]
    fn children_of_reserved_namespaces_are_reserved() {
        let child = Bid::new(crate::properties::href_namespace());
        assert!(
            child.is_reserved(),
            "nodes within href_namespace must be reserved"
        );
    }

    /// A plain user-authored BID must NOT be reserved, or the exclusion would
    /// swallow real content.
    #[test]
    fn user_bids_are_not_reserved() {
        let user_net = Bid::new(Bid::nil());
        let user_child = Bid::new(user_net);
        assert!(
            !user_child.is_reserved(),
            "user content must not be excluded"
        );
    }

    #[test]
    fn layout_config_defaults_to_enabled() {
        let cfg = LayoutConfig::default();
        assert!(cfg.enabled);
        assert_eq!(cfg.max_nodes, DEFAULT_LAYOUT_MAX_NODES);
    }

    #[test]
    fn explicit_max_nodes_wins_over_env() {
        // Explicit value short-circuits before the env var is consulted, so
        // this is safe regardless of ambient environment.
        assert_eq!(LayoutConfig::resolve_max_nodes(Some(42)), 42);
    }

    /// `--no-layout` must leave the graph completely untouched.
    #[test]
    fn disabled_config_writes_no_metadata() {
        let parent = Bid::nil();
        let bids: Vec<Bid> = (0..3).map(|_| Bid::new(parent)).collect();
        let profiles = vec![Some((0.5, 0.5, 0.5)); 3];
        let mut graph = make_graph(&bids, &profiles, vec![]);
        let pathmap = PathMapMap::default();

        let cfg = LayoutConfig {
            enabled: false,
            max_nodes: DEFAULT_LAYOUT_MAX_NODES,
        };
        compute_layout_metadata(&mut graph, &pathmap, &cfg);

        for bid in &bids {
            let md = &graph.states[bid].metadata;
            assert!(
                !md.contains_key("render_position"),
                "--no-layout must not write render_position"
            );
            assert!(!md.contains_key("structural_weight"));
            assert!(!md.contains_key("assembly_index"));
        }
    }

    fn test_force_layout(graph: &BeliefGraph, bids: &[Bid], params: &ForceParams) -> Vec<[f64; 3]> {
        let bid_to_local: BTreeMap<Bid, usize> =
            bids.iter().enumerate().map(|(i, &bid)| (bid, i)).collect();

        let mut positions: Vec<Vec3> = bids
            .iter()
            .enumerate()
            .map(|(i, bid)| {
                graph
                    .states
                    .get(bid)
                    .and_then(read_content_profile)
                    .map(|cp| profile_to_vec3(&cp))
                    .unwrap_or_else(|| golden_spiral_position(i, bids.len()))
            })
            .collect();

        let g = graph.relations.as_graph();
        let mut local_edges: Vec<(usize, usize, WeightKind)> = Vec::new();
        for edge_ref in g.edge_references() {
            let source_bid = g[edge_ref.source()];
            let sink_bid = g[edge_ref.target()];
            if let (Some(&si), Some(&di)) =
                (bid_to_local.get(&source_bid), bid_to_local.get(&sink_bid))
            {
                for &kind in edge_ref.weight().weights.keys() {
                    local_edges.push((si, di, kind));
                }
            }
        }

        run_force_simulation_core(&mut positions, &local_edges, params);
        positions.into_iter().map(|p| [p.x, p.y, p.z]).collect()
    }

    #[test]
    fn force_layout_pragmatic_edges_cluster_toward_p() {
        let parent = Bid::nil();
        let bids: Vec<Bid> = (0..4).map(|_| Bid::new(parent)).collect();
        let profiles: Vec<Option<(f64, f64, f64)>> = vec![Some((0.5, 0.5, 0.5)); 4];
        let edges = vec![
            (bids[0], bids[1], make_weight(WeightKind::Pragmatic)),
            (bids[1], bids[2], make_weight(WeightKind::Pragmatic)),
            (bids[2], bids[3], make_weight(WeightKind::Pragmatic)),
        ];
        let graph = make_graph(&bids, &profiles, edges);
        let positions = test_force_layout(&graph, &bids, &INTRA_PARAMS);

        for (i, pos) in positions.iter().enumerate() {
            assert!(
                pos[2] > 0.55,
                "Node {i} should drift toward P axis (z > 0.55), got z={:.3}",
                pos[2]
            );
        }
    }

    #[test]
    fn force_layout_epistemic_edges_cluster_toward_n() {
        let parent = Bid::nil();
        let bids: Vec<Bid> = (0..4).map(|_| Bid::new(parent)).collect();
        let profiles: Vec<Option<(f64, f64, f64)>> = vec![Some((0.5, 0.5, 0.5)); 4];
        let edges = vec![
            (bids[0], bids[1], make_weight(WeightKind::Epistemic)),
            (bids[1], bids[2], make_weight(WeightKind::Epistemic)),
            (bids[2], bids[3], make_weight(WeightKind::Epistemic)),
        ];
        let graph = make_graph(&bids, &profiles, edges);
        let positions = test_force_layout(&graph, &bids, &INTRA_PARAMS);

        for (i, pos) in positions.iter().enumerate() {
            assert!(
                pos[0] > 0.55,
                "Node {i} should drift toward N axis (x > 0.55), got x={:.3}",
                pos[0]
            );
        }
    }

    #[test]
    fn force_layout_deterministic() {
        let parent = Bid::nil();
        let bids: Vec<Bid> = (0..6).map(|_| Bid::new(parent)).collect();
        let profiles: Vec<Option<(f64, f64, f64)>> = (0..6)
            .map(|i| {
                let t = i as f64 / 5.0;
                Some((t, 1.0 - t, 0.5))
            })
            .collect();
        let edges = vec![
            (bids[0], bids[1], make_weight(WeightKind::Section)),
            (bids[1], bids[2], make_weight(WeightKind::Pragmatic)),
            (bids[3], bids[4], make_weight(WeightKind::Epistemic)),
            (bids[4], bids[5], make_weight(WeightKind::Epistemic)),
        ];
        let graph = make_graph(&bids, &profiles, edges);

        let pos1 = test_force_layout(&graph, &bids, &INTRA_PARAMS);
        let pos2 = test_force_layout(&graph, &bids, &INTRA_PARAMS);

        for (i, (a, b)) in pos1.iter().zip(pos2.iter()).enumerate() {
            assert!(
                (a[0] - b[0]).abs() < 1e-10
                    && (a[1] - b[1]).abs() < 1e-10
                    && (a[2] - b[2]).abs() < 1e-10,
                "Node {i} positions differ between runs: {a:?} vs {b:?}"
            );
        }
    }

    #[test]
    fn force_layout_positions_bounded() {
        let parent = Bid::nil();
        let bids: Vec<Bid> = (0..10).map(|_| Bid::new(parent)).collect();
        let profiles: Vec<Option<(f64, f64, f64)>> = vec![None; 10];
        let mut edges = Vec::new();
        for i in 0..bids.len() {
            for j in (i + 1)..bids.len() {
                edges.push((bids[i], bids[j], make_weight(WeightKind::Pragmatic)));
            }
        }
        let graph = make_graph(&bids, &profiles, edges);
        let positions = test_force_layout(&graph, &bids, &INTRA_PARAMS);

        for (i, pos) in positions.iter().enumerate() {
            for (axis, &val) in ["n", "s", "p"].iter().zip(pos.iter()) {
                assert!(
                    (0.0..=1.0).contains(&val),
                    "Node {i} {axis} out of bounds: {val:.6}"
                );
            }
        }
    }

    #[test]
    fn single_node_stays_near_content_profile() {
        let bid = Bid::new(Bid::nil());
        let graph = make_graph(&[bid], &[Some((0.8, 0.1, 0.3))], vec![]);
        let positions = test_force_layout(&graph, &[bid], &INTRA_PARAMS);
        assert_eq!(positions.len(), 1);
        for &val in &positions[0] {
            assert!((0.0..=1.0).contains(&val));
        }
    }

    #[test]
    fn assembly_index_linear_chain() {
        let parent = Bid::nil();
        let a = Bid::new(parent);
        let b = Bid::new(parent);
        let c = Bid::new(parent);

        let net_bids: BTreeSet<Bid> = [a, b, c].into();
        let mut net_graph = petgraph::stable_graph::StableGraph::<Bid, BTreeSet<WeightKind>>::new();
        let mut idx_map = BTreeMap::new();
        for &net in &net_bids {
            let idx = net_graph.add_node(net);
            idx_map.insert(net, idx);
        }
        net_graph.add_edge(
            idx_map[&a],
            idx_map[&b],
            BTreeSet::from([WeightKind::Epistemic]),
        );
        net_graph.add_edge(
            idx_map[&b],
            idx_map[&c],
            BTreeSet::from([WeightKind::Pragmatic]),
        );

        let condensed = CondensedNetworkGraph {
            graph: net_graph,
            idx_map,
            net_bids,
        };

        let indices = compute_assembly_indices(&condensed);
        assert_eq!(indices[&a], 0, "A has no upstream");
        assert_eq!(indices[&b], 1, "B has A upstream");
        assert_eq!(indices[&c], 2, "C has A and B upstream");
    }

    #[test]
    fn assembly_index_isolated_network() {
        let parent = Bid::nil();
        let a = Bid::new(parent);

        let net_bids: BTreeSet<Bid> = [a].into();
        let mut net_graph = petgraph::stable_graph::StableGraph::<Bid, BTreeSet<WeightKind>>::new();
        let mut idx_map = BTreeMap::new();
        let idx = net_graph.add_node(a);
        idx_map.insert(a, idx);

        let condensed = CondensedNetworkGraph {
            graph: net_graph,
            idx_map,
            net_bids,
        };

        let indices = compute_assembly_indices(&condensed);
        assert_eq!(indices[&a], 0, "Isolated network has zero upstream");
    }

    #[test]
    fn structural_weight_proportional_to_section_edges() {
        let parent = Bid::nil();
        let bids: Vec<Bid> = (0..3).map(|_| Bid::new(parent)).collect();
        let net_bid = Bid::new(parent);

        let home_networks: BTreeMap<Bid, Bid> = bids.iter().map(|&b| (b, net_bid)).collect();

        // Node 0 has 4 section edges, node 1 has 2, node 2 has 0.
        let mut edge_counts: BTreeMap<Bid, EdgeCounts> = BTreeMap::new();
        edge_counts.insert(
            bids[0],
            EdgeCounts {
                section_in: 2,
                section_out: 2,
                ..EdgeCounts::default()
            },
        );
        edge_counts.insert(
            bids[1],
            EdgeCounts {
                section_in: 1,
                section_out: 1,
                ..EdgeCounts::default()
            },
        );
        // Node 2 has no section edges.

        let weights = compute_structural_weights(&edge_counts, &home_networks);

        // Total section edges for this network: 4 + 2 = 6.
        let w0 = weights[&bids[0]];
        let w1 = weights[&bids[1]];
        assert!(
            (w0 - 4.0 / 6.0).abs() < 1e-10,
            "Node 0 structural weight should be 4/6, got {w0}"
        );
        assert!(
            (w1 - 2.0 / 6.0).abs() < 1e-10,
            "Node 1 structural weight should be 2/6, got {w1}"
        );
        // Node 2 has no entry in edge_counts, so no weight entry.
        assert!(
            !weights.contains_key(&bids[2]),
            "Node 2 should have no structural weight entry"
        );
    }

    #[test]
    fn edge_counts_only_intra_network() {
        let parent = Bid::nil();
        let a = Bid::new(parent); // network A
        let b = Bid::new(parent); // network B
        let node_a1 = Bid::new(parent);
        let node_a2 = Bid::new(parent);
        let node_b1 = Bid::new(parent);

        let mut home_networks = BTreeMap::new();
        home_networks.insert(node_a1, a);
        home_networks.insert(node_a2, a);
        home_networks.insert(node_b1, b);

        let edges = vec![
            // Intra-network edge (should be counted).
            (node_a1, node_a2, make_weight(WeightKind::Section)),
            // Inter-network edge (should NOT be counted).
            (node_a1, node_b1, make_weight(WeightKind::Epistemic)),
        ];
        let mut states = rustc_hash::FxHashMap::default();
        for &bid in &[node_a1, node_a2, node_b1] {
            states.insert(bid, make_node(bid, None));
        }
        let graph = BeliefGraph {
            states,
            relations: BidGraph::from_edges(edges),
        };

        let counts = compute_edge_counts(&graph, &home_networks);

        // node_a1 should have 1 section_out (intra-network edge to a2).
        let c_a1 = counts.get(&node_a1).unwrap();
        assert_eq!(c_a1.section_out, 1);
        assert_eq!(c_a1.epistemic_out, 0, "Inter-network edges excluded");

        // node_a2 should have 1 section_in.
        let c_a2 = counts.get(&node_a2).unwrap();
        assert_eq!(c_a2.section_in, 1);

        // node_b1 should have no edges counted (inter-network excluded).
        assert!(
            !counts.contains_key(&node_b1),
            "node_b1 should have no intra-network edges"
        );
    }

    #[test]
    fn merged_profiles_blend_lexical_and_structural() {
        let parent = Bid::nil();
        let bid = Bid::new(parent);

        // Node with high lexical N, but structurally it's S-like (many
        // incoming epistemic + pragmatic edges).
        let graph = make_graph(&[bid], &[Some((0.9, 0.1, 0.1))], vec![]);

        let mut edge_counts: BTreeMap<Bid, EdgeCounts> = BTreeMap::new();
        edge_counts.insert(
            bid,
            EdgeCounts {
                epistemic_in: 5,
                pragmatic_in: 3,
                ..EdgeCounts::default()
            },
        );

        let profiles = compute_merged_profiles(&graph, &edge_counts);
        let merged = &profiles[&bid];

        // Structural profile: raw_s = 8, raw_n = 0, raw_p = 0 → s = 1.0.
        // Merged with alpha=0.7: s = 0.7*0.1 + 0.3*1.0 = 0.37.
        // Should be higher than the lexical-only 0.1.
        assert!(
            merged.s > 0.2,
            "Merged S should be boosted by structural signal, got {:.3}",
            merged.s
        );
    }
}
