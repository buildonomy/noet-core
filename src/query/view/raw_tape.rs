// query/view/raw_tape.rs — RawTapeView: renders the tape entry-by-entry.
//
// Each tape entry is rendered according to its `TapeContent` variant:
// - `Edges` (with owner annotation) → rows of (owner, sink, source)
// - `Edges` (no owner) → rows of (source, sink, kind)
// - `Nodes` → rows of (title, schema, kind)
// - `Compose` → result table with Side provenance column
// - `Corpus` → summary marker
//
// The view walks every tape entry before the graph context boundary,
// producing one section (table) per entry. Graph context entries
// (halo, balance) are excluded from the output.

use std::collections::HashSet;

use toml::value::Table;

use crate::beliefbase::BeliefGraph;
use crate::properties::{Bid, WeightKind, WEIGHT_OWNED_BY};
use crate::query::spec::{
    CompositionOp, ProjectionStep, QueryPackage, StepOperation, TapeContent, TapeEntry,
    TraversalSpec,
};
use crate::query::view::{LinkResolver, ViewOutput, ViewRenderer};
use crate::BuildonomyError;

use super::{cells_to_json, node_info_map, Cell, EdgeCellData, EdgeOwnership, EntryRow};

/// A view renderer that walks the tape entry-by-entry, rendering each
/// `TapeContent` variant in its natural shape.
///
/// For `Edges` entries with owner annotations, each edge produces a row
/// with (owner, sink, source) columns. For plain `Edges`, the columns
/// are (source, sink, kind). `Nodes` entries render as (title, schema, kind).
///
/// The HTML output consists of serial `<table>` elements, one per tape entry,
/// each with a caption showing the step label and entry index.
///
/// The JSON output is a vec of entry objects, each containing headers and rows
/// appropriate to the content type.
pub struct RawTapeView {
    raw: Table,
}

impl RawTapeView {
    /// Construct from a params table.
    pub fn from_params(params: &Table) -> Result<Self, BuildonomyError> {
        Ok(Self {
            raw: params.clone(),
        })
    }
}

impl ViewRenderer for RawTapeView {
    fn spec(&self) -> &Table {
        &self.raw
    }

    fn render(
        &self,
        package: &QueryPackage,
        links: Option<&dyn LinkResolver>,
    ) -> Result<ViewOutput, BuildonomyError> {
        let graph = package.graph().expect("render requires a populated graph");
        let tape = package.tape();
        let spec_steps = &package.spec().steps;

        let mut html = String::new();
        for (idx, entry) in tape.steps.iter().enumerate() {
            let step = find_step_for_entry(entry, idx, spec_steps);
            let step_desc = step.map(format_step).unwrap_or_default();
            let caption = format!("[{}] {} {}", idx, entry.label, step_desc);
            let section_html = render_entry_html(entry, idx, &caption, graph, links, step);
            html.push_str(&section_html);
        }

        if html.is_empty() {
            html.push_str("<p class=\"noet-query-empty\">No tape entries.</p>");
        }

        Ok(ViewOutput::Html(html))
    }

    fn render_json(&self, package: &QueryPackage) -> Result<ViewOutput, BuildonomyError> {
        let graph = package
            .graph()
            .expect("render_json requires a populated graph");
        let tape = package.tape();
        let spec_steps = &package.spec().steps;

        let mut entries = Vec::new();
        for (idx, entry) in tape.steps.iter().enumerate() {
            let step = find_step_for_entry(entry, idx, spec_steps);
            entries.push(render_entry_json(entry, idx, graph, step));
        }

        // Build a nodes lookup from all BIDs in the graph.
        let nodes = node_info_map(graph.states.keys().copied(), graph);

        let json = serde_json::json!({
            "display": "Tape",
            "entries": entries,
            "nodes": nodes,
        });

        Ok(ViewOutput::Json(json))
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// Per-entry rendering
// ═════════════════════════════════════════════════════════════════════════════

/// Render a single tape entry as an HTML `<table>`.
fn render_entry_html(
    entry: &TapeEntry,
    idx: usize,
    caption: &str,
    graph: &BeliefGraph,
    links: Option<&dyn LinkResolver>,
    step: Option<&ProjectionStep>,
) -> String {
    let (headers, rows) = entry_rows(entry, idx, graph, step);

    let mut html = String::new();
    html.push_str("<figure>\n<figcaption>");
    html.push_str(&html_escape(caption));
    html.push_str("</figcaption>\n<table class=\"noet-query-raw-tape\">\n<thead><tr>");
    for h in &headers {
        html.push_str("<th>");
        html.push_str(&html_escape(h));
        html.push_str("</th>");
    }
    html.push_str("</tr></thead>\n<tbody>\n");
    for row in &rows {
        html.push_str("<tr>");
        for cell in &row.cells {
            html.push_str("<td>");
            if let Some(edge) = &cell.edge {
                // Edge cell: KIND(s), KIND(k), KIND(s/k/@), or KIND(s/k)
                html.push_str(&format!("{:?}(", edge.kind));
                match &edge.ownership {
                    EdgeOwnership::Source => {
                        html.push_str(&render_role_link("s", &edge.source, graph, links));
                    }
                    EdgeOwnership::Sink => {
                        html.push_str(&render_role_link("k", &edge.sink, graph, links));
                    }
                    EdgeOwnership::ThirdParty(owner) => {
                        html.push_str(&render_role_link("@", owner, graph, links));
                    }
                    EdgeOwnership::Missing => {
                        html.push_str("<span title=\"missing owned_by field\">⚠</span>");
                    }
                }
                html.push(')');
            } else if let (Some(bid), Some(resolver)) = (&cell.bid, links) {
                if let Some(node) = graph.states.get(bid) {
                    html.push_str(&resolver.render_anchor(node, &html_escape(&cell.text)));
                } else {
                    html.push_str(&html_escape(&cell.text));
                }
            } else {
                html.push_str(&html_escape(&cell.text));
            }
            html.push_str("</td>");
        }
        html.push_str("</tr>\n");
    }
    html.push_str("</tbody>\n</table>\n</figure>\n");
    html
}

/// Render a single tape entry as a JSON object.
///
/// Each cell is either a plain string (for labels like "Kind", "Side") or
/// a `{ "bid": "uuid" }` object for node references. The consumer resolves
/// display text from the BID.
fn render_entry_json(
    entry: &TapeEntry,
    idx: usize,
    graph: &BeliefGraph,
    step: Option<&ProjectionStep>,
) -> serde_json::Value {
    let (headers, rows) = entry_rows(entry, idx, graph, step);
    let content_type = match &entry.content {
        TapeContent::Edges { .. } => "edges",
        TapeContent::Nodes(_) => "nodes",
        TapeContent::Compose { .. } => "compose",
        TapeContent::Corpus => "corpus",
    };

    let json_rows = cells_to_json(&rows);

    let step_desc = step.map(format_step).unwrap_or_default();

    serde_json::json!({
        "step_label": entry.label,
        "entry_index": idx,
        "content_type": content_type,
        "step_operation": step_desc,
        "headers": headers,
        "rows": json_rows,
    })
}

// ═════════════════════════════════════════════════════════════════════════════
// Row extraction per TapeContent variant
// ═════════════════════════════════════════════════════════════════════════════

/// Extract headers and rows from a tape entry.
fn entry_rows(
    entry: &TapeEntry,
    _idx: usize,
    graph: &BeliefGraph,
    step: Option<&ProjectionStep>,
) -> (Vec<String>, Vec<EntryRow>) {
    // Extract kind_filter from the step's traversal spec (if any).
    let kind_filter = step.and_then(|s| match &s.operation {
        StepOperation::Traverse(t) => Some(&t.kind_filter),
        _ => None,
    });
    match &entry.content {
        TapeContent::Edges { edges, .. } => edges_rows(edges, graph, kind_filter),
        TapeContent::Nodes(bids) => nodes_rows(bids, graph),
        TapeContent::Compose {
            op,
            left,
            right,
            result,
            ..
        } => compose_rows(*op, left, right, result, graph),
        TapeContent::Corpus => corpus_rows(),
    }
}

/// Render `Edges` content using the tape's edge indices (which reference
/// the package graph after `materialize_graph` remaps them).
///
/// If any edge has an owner annotation, renders as
/// (Owner, Sink, Source, Kind). Otherwise renders as (Source, Sink, Kind).
fn edges_rows(
    edges: &[petgraph::graph::EdgeIndex],
    graph: &BeliefGraph,
    kind_filter: Option<&enumset::EnumSet<WeightKind>>,
) -> (Vec<String>, Vec<EntryRow>) {
    let g = graph.relations.as_graph();

    // Collect resolved edges, applying kind filter if specified.
    struct ResolvedEdge {
        source_bid: Bid,
        sink_bid: Bid,
        weights: crate::properties::WeightSet,
    }
    let mut resolved: Vec<ResolvedEdge> = Vec::new();
    let mut seen = HashSet::new();
    for &eidx in edges {
        if !seen.insert(eidx) {
            continue;
        }
        let Some((src_idx, snk_idx)) = g.edge_endpoints(eidx) else {
            continue;
        };
        let Some(ws) = g.edge_weight(eidx) else {
            continue;
        };
        // Apply kind filter: skip edges that don't match.
        if let Some(filter) = kind_filter {
            if !ws.weights.keys().any(|k| filter.contains(*k)) {
                continue;
            }
        }
        resolved.push(ResolvedEdge {
            source_bid: g[src_idx],
            sink_bid: g[snk_idx],
            weights: ws.clone(),
        });
    }

    // One row per (edge, weight_kind) pair.
    // Columns: Sink, Source, Edge (compact ownership notation).
    let headers = vec!["Sink".into(), "Source".into(), "Edge".into()];
    let mut rows = Vec::new();
    for edge in &resolved {
        for (kind, weight) in &edge.weights.weights {
            let owner_val: Option<String> = weight.get(WEIGHT_OWNED_BY);
            let ownership = match owner_val.as_deref() {
                Some("source") => EdgeOwnership::Source,
                Some("sink") => EdgeOwnership::Sink,
                Some(bref_str) => {
                    match graph
                        .states
                        .keys()
                        .find(|bid| bid.bref().to_string() == bref_str)
                        .copied()
                    {
                        Some(bid) => EdgeOwnership::ThirdParty(bid),
                        None => EdgeOwnership::Missing,
                    }
                }
                None => EdgeOwnership::Missing,
            };

            rows.push(EntryRow {
                cells: vec![
                    Cell::node(node_title(graph, &edge.sink_bid), edge.sink_bid),
                    Cell::node(node_title(graph, &edge.source_bid), edge.source_bid),
                    Cell {
                        text: format!("{kind:?}"),
                        bid: None,
                        edge: Some(EdgeCellData {
                            source: edge.source_bid,
                            sink: edge.sink_bid,
                            ownership,
                            kind: *kind,
                        }),
                    },
                ],
            });
        }
    }
    (headers, rows)
}

/// Render `Nodes` content: one row per BID with title, schema, kind.
fn nodes_rows(bids: &[Bid], graph: &BeliefGraph) -> (Vec<String>, Vec<EntryRow>) {
    let headers = vec!["Title".into(), "Schema".into(), "Kind".into()];
    let rows = bids
        .iter()
        .filter_map(|bid| {
            let node = graph.states.get(bid)?;
            Some(EntryRow {
                cells: vec![
                    Cell::node(node.title.clone(), *bid),
                    Cell::text(node.schema.clone().unwrap_or_default()),
                    Cell::text(format!("{}", node.kind)),
                ],
            })
        })
        .collect();
    (headers, rows)
}

/// Render `Compose` content: result BIDs with a Side provenance column.
fn compose_rows(
    op: CompositionOp,
    left: &std::ops::Range<usize>,
    right: &std::ops::Range<usize>,
    result: &[Bid],
    graph: &BeliefGraph,
) -> (Vec<String>, Vec<EntryRow>) {
    // Collect BIDs from left and right branches to compute provenance.
    // Note: we don't have access to the full tape here, just the Compose
    // entry's metadata. The `left`/`right` ranges tell us which tape entries
    // belong to each branch, but we only have the `result` BIDs. We use the
    // graph states to get node info, and the op to determine provenance labels.
    //
    // For accurate Left/Right/Both provenance, we'd need the full tape. Since
    // Compose.result already contains only the merged set, we label all as the
    // op result type. This matches the simplified rendering — full provenance
    // is handled by TableView::render_with_tape when the full tape is available.
    let _ = (left, right); // ranges are for tape-level provenance; unused here

    let side_label = match op {
        CompositionOp::And => "Both",
        CompositionOp::Or => "Merged",
        CompositionOp::Not => "Gap",
    };

    let headers = vec![
        "Side".into(),
        "Title".into(),
        "Schema".into(),
        "Kind".into(),
    ];
    let rows = result
        .iter()
        .filter_map(|bid| {
            let node = graph.states.get(bid)?;
            Some(EntryRow {
                cells: vec![
                    Cell::text(side_label),
                    Cell::node(node.title.clone(), *bid),
                    Cell::text(node.schema.clone().unwrap_or_default()),
                    Cell::text(format!("{}", node.kind)),
                ],
            })
        })
        .collect();
    (headers, rows)
}

/// Render `Corpus` content: a single summary row.
fn corpus_rows() -> (Vec<String>, Vec<EntryRow>) {
    let headers = vec!["Info".into()];
    let rows = vec![EntryRow {
        cells: vec![Cell::text("All nodes in scope (corpus)")],
    }];
    (headers, rows)
}

// ═════════════════════════════════════════════════════════════════════════════
// Helpers
// ═════════════════════════════════════════════════════════════════════════════

/// Find the `ProjectionStep` that produced a given tape entry.
///
/// Tape entries share labels with their step. Multi-hop traversals produce
/// multiple entries with the same label. We match by label, falling back
/// to index-based matching for auto-labeled steps (label = step index string).
fn find_step_for_entry<'a>(
    entry: &TapeEntry,
    _tape_idx: usize,
    steps: &'a [ProjectionStep],
) -> Option<&'a ProjectionStep> {
    // Match by label: find the spec step whose effective label matches.
    for (step_idx, step) in steps.iter().enumerate() {
        let effective_label = if step.label.is_empty() {
            step_idx.to_string()
        } else {
            step.label.clone()
        };
        if effective_label == entry.label {
            return Some(step);
        }
    }
    None
}

/// Format a `ProjectionStep` for display.
fn format_step(step: &ProjectionStep) -> String {
    match &step.operation {
        StepOperation::Identity => "Identity".to_string(),
        StepOperation::Filter(_) => "Filter".to_string(),
        StepOperation::Traverse(t) => format_traversal(t),
        StepOperation::Compose(c) => format!("Compose({:?})", c.op),
    }
}

/// Format a `TraversalSpec` as a concise shorthand string.
fn format_traversal(t: &TraversalSpec) -> String {
    let input: Vec<&str> = t
        .input_roles
        .iter()
        .map(|r| match r {
            crate::query::spec::Role::Source => "s",
            crate::query::spec::Role::Sink => "k",
            crate::query::spec::Role::Owner => "o",
        })
        .collect();
    let kinds: Vec<String> = t
        .kind_filter
        .iter()
        .map(|k| format!("{k:?}").to_lowercase())
        .collect();
    let output: Vec<&str> = t
        .output_roles
        .iter()
        .map(|r| match r {
            crate::query::spec::Role::Source => "s",
            crate::query::spec::Role::Sink => "k",
            crate::query::spec::Role::Owner => "o",
        })
        .collect();
    let depth = match t.depth.count {
        crate::query::spec::DepthCount::N(n) => format!("{n}"),
        crate::query::spec::DepthCount::Max => "*".to_string(),
    };
    format!(
        "{}-{}-{}({})",
        input.join(""),
        kinds.join(","),
        output.join(""),
        depth
    )
}

/// Render a role label ("s", "k", "@") as a linked anchor or plain text.
fn render_role_link(
    role: &str,
    bid: &Bid,
    graph: &BeliefGraph,
    links: Option<&dyn LinkResolver>,
) -> String {
    if let (Some(node), Some(resolver)) = (graph.states.get(bid), links) {
        resolver.render_anchor(node, role)
    } else {
        let title = node_title(graph, bid);
        format!("<span title=\"{}\">{}</span>", html_escape(&title), role)
    }
}

fn node_title(graph: &BeliefGraph, bid: &Bid) -> String {
    graph
        .states
        .get(bid)
        .map(|n| n.title.clone())
        .unwrap_or_else(|| format!("?{}", bid.bref()))
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

// ═════════════════════════════════════════════════════════════════════════════
// Factory
// ═════════════════════════════════════════════════════════════════════════════

pub(crate) fn raw_tape_factory(params: &Table) -> Result<Box<dyn ViewRenderer>, BuildonomyError> {
    Ok(Box::new(RawTapeView::from_params(params)?))
}

// ═════════════════════════════════════════════════════════════════════════════
// Tests
// ═════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    use rustc_hash::FxHashMap;

    use crate::beliefbase::BidGraph;
    use crate::properties::{BeliefKind, BeliefNode, NodeId, WeightKind, WeightSet};
    use crate::query::spec::{ProjectionStep, QuerySpec, StepOperation, Tape, TapeFn};

    fn make_graph(nodes: Vec<BeliefNode>, edges: Vec<(Bid, Bid, WeightSet)>) -> BeliefGraph {
        let states: FxHashMap<Bid, BeliefNode> = nodes.iter().map(|n| (n.bid, n.clone())).collect();
        let relations = BidGraph::from_edges(edges);
        BeliefGraph { states, relations }
    }

    fn test_node(bid: Bid, title: &str, schema: Option<&str>) -> BeliefNode {
        BeliefNode {
            bid,
            kind: BeliefKind::Document.into(),
            title: title.to_string(),
            schema: schema.map(|s| s.to_string()),
            id: NodeId::Explicit(title.to_lowercase().replace(' ', "-")),
            ..Default::default()
        }
    }

    fn make_package_with_tape(tape: Tape, graph: BeliefGraph) -> QueryPackage {
        // Build a spec with enough steps to match the tape entries.
        let steps: Vec<ProjectionStep> = tape
            .steps
            .iter()
            .map(|_| ProjectionStep {
                label: String::new(),
                input: TapeFn::Then(None),
                operation: StepOperation::Identity,
            })
            .collect();
        let spec = if steps.is_empty() {
            QuerySpec::seed(TapeFn::Bids(vec![]))
        } else {
            QuerySpec { steps }
        };
        let mut package = QueryPackage::new(spec);
        package.set_graph(graph);
        *package.tape_mut() = tape;
        package
    }

    #[test]
    fn render_nodes_entry() {
        let bid_a = Bid::new(Bid::nil());
        let bid_b = Bid::new(bid_a);
        let graph = make_graph(
            vec![
                test_node(bid_a, "Alpha", Some("req")),
                test_node(bid_b, "Beta", None),
            ],
            vec![],
        );
        let tape = Tape {
            steps: vec![TapeEntry {
                label: "0".into(),
                content: TapeContent::Nodes(vec![bid_a, bid_b]),
                payload: None,
            }],
        };
        let package = make_package_with_tape(tape, graph);

        let view = RawTapeView::from_params(&Table::new()).unwrap();
        let output = view.render_json(&package).unwrap();

        match output {
            ViewOutput::Json(json) => {
                let entries = json["entries"].as_array().unwrap();
                assert_eq!(entries.len(), 1);
                assert_eq!(entries[0]["content_type"], "nodes");
                assert_eq!(entries[0]["step_label"], "0");
                assert_eq!(entries[0]["entry_index"], 0);

                let headers = entries[0]["headers"].as_array().unwrap();
                assert_eq!(headers, &["Title", "Schema", "Kind"]);

                let rows = entries[0]["rows"].as_array().unwrap();
                assert_eq!(rows.len(), 2);
                // Title cell is a node reference (BID object).
                assert!(rows[0]["cells"][0]["bid"].is_string());
                // Schema cell is plain text.
                assert_eq!(rows[0]["cells"][1], "req");
                // Kind cell is plain text.
                assert!(rows[0]["cells"][2].is_string());
            }
            other => panic!("expected Json, got {other:?}"),
        }
    }

    #[test]
    fn render_edges_with_owner() {
        let owner_bid = Bid::new(Bid::nil());
        let source_bid = Bid::new(owner_bid);
        let sink_bid = Bid::new(source_bid);

        let mut ws = WeightSet::from(WeightKind::Pragmatic);
        ws.weights
            .get_mut(&WeightKind::Pragmatic)
            .unwrap()
            .set(WEIGHT_OWNED_BY, owner_bid.bref().to_string())
            .unwrap();

        let graph = make_graph(
            vec![
                test_node(owner_bid, "Owner", None),
                test_node(source_bid, "Source", None),
                test_node(sink_bid, "Sink", None),
            ],
            vec![(source_bid, sink_bid, ws)],
        );

        // Find the edge index from the graph.
        let g = graph.relations.as_graph();
        let edge_idx = g.edge_indices().next().unwrap();

        let tape = Tape {
            steps: vec![TapeEntry {
                label: "0".into(),
                content: TapeContent::Edges {
                    edges: vec![edge_idx],
                    output_bids: vec![source_bid, sink_bid],
                },
                payload: None,
            }],
        };
        let package = make_package_with_tape(tape, graph);

        let view = RawTapeView::from_params(&Table::new()).unwrap();
        let output = view.render_json(&package).unwrap();

        match output {
            ViewOutput::Json(json) => {
                assert_eq!(json["display"], "Tape");
                assert!(json["nodes"].is_object());
                let entries = json["entries"].as_array().unwrap();
                assert_eq!(entries[0]["content_type"], "edges");
                let headers = entries[0]["headers"].as_array().unwrap();
                assert_eq!(headers, &["Sink", "Source", "Edge"]);

                let rows = entries[0]["rows"].as_array().unwrap();
                assert_eq!(rows.len(), 1);
                // Sink and Source are BID node refs.
                assert!(rows[0]["cells"][0]["bid"].is_string()); // Sink
                assert!(rows[0]["cells"][1]["bid"].is_string()); // Source
                                                                 // Edge cell with structured edge data + third-party owner.
                let edge = &rows[0]["cells"][2]["edge"];
                assert_eq!(edge["kind"], "Pragmatic");
                assert!(
                    edge["owner"]["bid"].is_string(),
                    "should have third-party owner"
                );
            }
            other => panic!("expected Json, got {other:?}"),
        }
    }

    #[test]
    fn render_edges_without_owner() {
        let bid_a = Bid::new(Bid::nil());
        let bid_b = Bid::new(bid_a);

        let ws = WeightSet::from(WeightKind::Section);
        let graph = make_graph(
            vec![
                test_node(bid_a, "Parent", None),
                test_node(bid_b, "Child", None),
            ],
            vec![(bid_a, bid_b, ws)],
        );

        let g = graph.relations.as_graph();
        let edge_idx = g.edge_indices().next().unwrap();

        let tape = Tape {
            steps: vec![TapeEntry {
                label: "0".into(),
                content: TapeContent::Edges {
                    edges: vec![edge_idx],
                    output_bids: vec![bid_b],
                },
                payload: None,
            }],
        };
        let package = make_package_with_tape(tape, graph);

        let view = RawTapeView::from_params(&Table::new()).unwrap();
        let output = view.render_json(&package).unwrap();

        match output {
            ViewOutput::Json(json) => {
                let entries = json["entries"].as_array().unwrap();
                let headers = entries[0]["headers"].as_array().unwrap();
                assert_eq!(headers, &["Sink", "Source", "Edge"]);

                let rows = entries[0]["rows"].as_array().unwrap();
                assert_eq!(rows.len(), 1);
                // Sink and Source are BID node refs.
                assert!(rows[0]["cells"][0]["bid"].is_string()); // Sink
                assert!(rows[0]["cells"][1]["bid"].is_string()); // Source
                                                                 // Edge cell — test fixture has no owned_by field.
                let edge = &rows[0]["cells"][2]["edge"];
                assert_eq!(edge["kind"], "Section");
                // Test fixture edges have no owned_by, so error is flagged.
                assert!(edge["error"].is_string(), "should flag missing owned_by");
            }
            other => panic!("expected Json, got {other:?}"),
        }
    }

    #[test]
    fn render_corpus_entry() {
        let graph = make_graph(vec![], vec![]);
        let tape = Tape {
            steps: vec![TapeEntry {
                label: "seed".into(),
                content: TapeContent::Corpus,
                payload: None,
            }],
        };
        let package = make_package_with_tape(tape, graph);

        let view = RawTapeView::from_params(&Table::new()).unwrap();
        let output = view.render_json(&package).unwrap();

        match output {
            ViewOutput::Json(json) => {
                let entries = json["entries"].as_array().unwrap();
                assert_eq!(entries[0]["content_type"], "corpus");
                let rows = entries[0]["rows"].as_array().unwrap();
                assert_eq!(rows.len(), 1);
                assert!(rows[0]["cells"][0].as_str().unwrap().contains("corpus"));
            }
            other => panic!("expected Json, got {other:?}"),
        }
    }

    #[test]
    fn render_compose_entry() {
        let bid_a = Bid::new(Bid::nil());
        let bid_b = Bid::new(bid_a);
        let graph = make_graph(
            vec![
                test_node(bid_a, "Left Only", Some("req")),
                test_node(bid_b, "Both", Some("req")),
            ],
            vec![],
        );
        let tape = Tape {
            steps: vec![TapeEntry {
                label: "compose".into(),
                content: TapeContent::Compose {
                    op: CompositionOp::Not,
                    left: 0..1,
                    right: 1..2,
                    result: vec![bid_a],
                    intersection: vec![],
                },
                payload: None,
            }],
        };
        let package = make_package_with_tape(tape, graph);

        let view = RawTapeView::from_params(&Table::new()).unwrap();
        let output = view.render_json(&package).unwrap();

        match output {
            ViewOutput::Json(json) => {
                let entries = json["entries"].as_array().unwrap();
                assert_eq!(entries[0]["content_type"], "compose");
                let headers = entries[0]["headers"].as_array().unwrap();
                assert_eq!(headers[0], "Side");

                let rows = entries[0]["rows"].as_array().unwrap();
                assert_eq!(rows.len(), 1);
                assert_eq!(rows[0]["cells"][0], "Gap"); // Side is plain text
                assert!(rows[0]["cells"][1]["bid"].is_string()); // Title is BID ref
            }
            other => panic!("expected Json, got {other:?}"),
        }
    }

    #[test]
    fn render_html_produces_tables() {
        let bid = Bid::new(Bid::nil());
        let graph = make_graph(vec![test_node(bid, "Node", Some("doc"))], vec![]);
        let tape = Tape {
            steps: vec![TapeEntry {
                label: "step0".into(),
                content: TapeContent::Nodes(vec![bid]),
                payload: None,
            }],
        };
        let package = make_package_with_tape(tape, graph);

        let view = RawTapeView::from_params(&Table::new()).unwrap();
        let output = view.render(&package, None).unwrap();

        match output {
            ViewOutput::Html(html) => {
                assert!(html.contains("<table"), "should contain a table");
                assert!(
                    html.contains("[0] step0"),
                    "caption should show index and label"
                );
                assert!(html.contains("Node"), "should contain the node title");
            }
            other => panic!("expected Html, got {other:?}"),
        }
    }

    #[test]
    fn render_multiple_entries() {
        let bid_a = Bid::new(Bid::nil());
        let bid_b = Bid::new(bid_a);
        let graph = make_graph(
            vec![
                test_node(bid_a, "Alpha", None),
                test_node(bid_b, "Beta", None),
            ],
            vec![],
        );
        let tape = Tape {
            steps: vec![
                TapeEntry {
                    label: "identity".into(),
                    content: TapeContent::Nodes(vec![bid_a]),
                    payload: None,
                },
                TapeEntry {
                    label: "filter".into(),
                    content: TapeContent::Nodes(vec![bid_a, bid_b]),
                    payload: None,
                },
            ],
        };
        let package = make_package_with_tape(tape, graph);

        let view = RawTapeView::from_params(&Table::new()).unwrap();
        let output = view.render_json(&package).unwrap();

        match output {
            ViewOutput::Json(json) => {
                let entries = json["entries"].as_array().unwrap();
                assert_eq!(entries.len(), 2);
                assert_eq!(entries[0]["entry_index"], 0);
                assert_eq!(entries[1]["entry_index"], 1);
                assert_eq!(
                    entries[0]["rows"].as_array().unwrap().len(),
                    1,
                    "first entry has 1 node"
                );
                assert_eq!(
                    entries[1]["rows"].as_array().unwrap().len(),
                    2,
                    "second entry has 2 nodes"
                );
            }
            other => panic!("expected Json, got {other:?}"),
        }
    }
}
