// query/view/table.rs — Table view: renders query results as HTML tables
// or structured rows.
//
// This is the default view renderer. It supports multiple display modes:
// Depth0 (node intrinsics), Columns (explicit property columns), and Connectivity
// (connectivity matrix). Owner→sink traceability is handled by `RawTapeView`.

use std::collections::{BTreeMap, BTreeSet};

use petgraph::visit::{EdgeRef, IntoEdgeReferences};
use toml::value::Table;
use toml::Value as TomlValue;

use crate::beliefbase::BeliefGraph;
use crate::properties::{BeliefNode, Bid, WeightKind};
use crate::query::spec::{
    parse_property_path, resolve_property_path, CompositionOp, PropertyPath, QueryPackage, Score,
    Tape, TapeFn,
};
use crate::query::view::{
    cells_to_json, node_info_map, Cell, EntryRow, LinkResolver, ViewOutput, ViewRenderer,
};
use crate::BuildonomyError;

/// Display mode for the table view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TableDisplayMode {
    /// Explicit column list from `params["columns"]`.
    Columns,
    /// Connectivity matrix: In/Out edges per WeightKind.
    Connectivity,
    /// Just node intrinsics: title, kind, schema. Default.
    Depth0,
}

/// Table view — renders query results as an HTML table or structured rows.
#[derive(Debug, Clone)]
pub struct TableView {
    /// Column specifications — each is a `(display_name, PropertyPath)` pair.
    /// Parsed from `params["columns"]` as an array of path strings.
    pub columns: Vec<(String, PropertyPath)>,
    /// Display mode.
    pub display: TableDisplayMode,
    /// Maximum rows to render.
    pub max_rows: Option<usize>,
    /// Optional table caption.
    pub caption: Option<String>,
    /// Raw configuration table as supplied by the surface layer.
    /// Returned by [`ViewRenderer::spec`] for opaque inspection by callers.
    pub raw: Table,
}

impl TableView {
    /// Parse a `TableView` from the opaque `params` table in a view
    /// directive.
    ///
    /// Recognized keys:
    /// - `columns`: array of dotted-path strings (e.g. `["title", "payload.status"]`)
    /// - `display`: one of `"columns"`, `"connectivity"`, `"depth0"`
    /// - `max_rows`: integer
    /// - `caption`: string
    pub fn from_params(params: &Table) -> Result<Self, BuildonomyError> {
        // Parse columns
        let columns: Vec<(String, PropertyPath)> = match params.get("columns") {
            Some(toml::Value::Array(arr)) => {
                let mut cols = Vec::with_capacity(arr.len());
                for val in arr {
                    let s = val.as_str().ok_or_else(|| {
                        BuildonomyError::Command("table view: each column must be a string".into())
                    })?;
                    let path = parse_property_path(s).map_err(|e| {
                        BuildonomyError::Command(format!(
                            "table view: invalid column path '{s}': {e}"
                        ))
                    })?;
                    // Display name is the last segment of the path
                    let display_name = s.rsplit('.').next().unwrap_or(s).to_string();
                    cols.push((display_name, path));
                }
                cols
            }
            Some(_) => {
                return Err(BuildonomyError::Command(
                    "table view: 'columns' must be an array of strings".into(),
                ));
            }
            None => vec![],
        };

        // Parse display mode
        let display = match params.get("display").and_then(|v| v.as_str()) {
            Some("columns") => TableDisplayMode::Columns,
            Some("connectivity") => TableDisplayMode::Connectivity,
            Some("depth0") => TableDisplayMode::Depth0,
            Some(other) => {
                return Err(BuildonomyError::Command(format!(
                    "table view: unknown display mode '{other}'"
                )));
            }
            None => {
                // Default: Columns if columns are specified, otherwise Depth0
                if columns.is_empty() {
                    TableDisplayMode::Depth0
                } else {
                    TableDisplayMode::Columns
                }
            }
        };

        // Parse max_rows
        let max_rows = params
            .get("max_rows")
            .and_then(|v| v.as_integer())
            .map(|n| n as usize);

        // Parse caption
        let caption = params
            .get("caption")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        Ok(TableView {
            columns,
            display,
            max_rows,
            caption,
            raw: params.clone(),
        })
    }

    /// Extract column headers for the current display mode.
    fn headers(&self) -> Vec<String> {
        match self.display {
            TableDisplayMode::Depth0 => {
                vec!["Title".into(), "Schema".into(), "Kind".into()]
            }
            TableDisplayMode::Columns => {
                self.columns.iter().map(|(name, _)| name.clone()).collect()
            }
            TableDisplayMode::Connectivity => {
                let mut headers = vec!["Node".to_string()];
                for kind in &[
                    WeightKind::Section,
                    WeightKind::Epistemic,
                    WeightKind::Pragmatic,
                ] {
                    headers.push(format!("{kind:?} In"));
                    headers.push(format!("{kind:?} Out"));
                }
                headers
            }
        }
    }

    /// Extract a row of cell values for a single node in Depth0 mode.
    fn row_depth0(node: &BeliefNode) -> Vec<String> {
        vec![
            node.title.clone(),
            node.schema.clone().unwrap_or_default(),
            format!("{}", node.kind),
        ]
    }

    /// Extract a row of cell values for a single node in Columns mode.
    fn row_columns(&self, node: &BeliefNode) -> Vec<String> {
        self.columns
            .iter()
            .map(|(_, path)| {
                let result = resolve_property_path(node, path);
                toml_values_to_display(&result.values)
            })
            .collect()
    }

    /// Extract a row of cell values for a single node in Connectivity mode.
    fn row_connectivity(node: &BeliefNode, graph: &BeliefGraph) -> Vec<String> {
        let bid = node.bid;
        let g = graph.relations.as_graph();

        // Count edges by kind and direction
        let mut counts: BTreeMap<(WeightKind, bool), usize> = BTreeMap::new(); // (kind, is_outgoing)
        for edge_ref in g.edge_references() {
            let source_bid = g[edge_ref.source()];
            let sink_bid = g[edge_ref.target()];
            let ws = edge_ref.weight();
            for kind in ws.weights.keys() {
                if source_bid == bid {
                    *counts.entry((*kind, true)).or_insert(0) += 1;
                }
                if sink_bid == bid {
                    *counts.entry((*kind, false)).or_insert(0) += 1;
                }
            }
        }

        let mut row = vec![node.title.clone()];
        for kind in &[
            WeightKind::Section,
            WeightKind::Epistemic,
            WeightKind::Pragmatic,
        ] {
            let in_count = counts.get(&(*kind, false)).copied().unwrap_or(0);
            let out_count = counts.get(&(*kind, true)).copied().unwrap_or(0);
            row.push(in_count.to_string());
            row.push(out_count.to_string());
        }
        row
    }

    /// Build structured rows (header + data) for the current display mode.
    pub(crate) fn build_rows(
        &self,
        entries: &[(Bid, Score)],
        graph: &BeliefGraph,
    ) -> Vec<Vec<String>> {
        let mut rows = vec![self.headers()];

        match self.display {
            TableDisplayMode::Depth0 => {
                for (bid, _) in entries {
                    if let Some(node) = graph.states.get(bid) {
                        rows.push(Self::row_depth0(node));
                    }
                }
            }
            TableDisplayMode::Columns => {
                for (bid, _) in entries {
                    if let Some(node) = graph.states.get(bid) {
                        rows.push(self.row_columns(node));
                    }
                }
            }
            TableDisplayMode::Connectivity => {
                for (bid, _) in entries {
                    if let Some(node) = graph.states.get(bid) {
                        rows.push(Self::row_connectivity(node, graph));
                    }
                }
            }
        }

        // Truncate if max_rows is set (header row excluded from count)
        if let Some(max) = self.max_rows {
            if rows.len() > max + 1 {
                rows.truncate(max + 1);
            }
        }

        rows
    }

    /// Render structured rows as an HTML `<table>`.
    fn rows_to_html(rows: &[Vec<String>], caption: Option<&str>) -> String {
        let mut html = String::new();

        if let Some(cap) = caption {
            html.push_str("<figure>\n<figcaption>");
            html.push_str(&html_escape(cap));
            html.push_str("</figcaption>\n");
        }

        // No data rows (only header) → render empty-state message instead of
        // a table with just headers and an empty tbody.
        let has_data = rows.len() > 1;
        if !has_data {
            html.push_str("<p class=\"noet-query-empty\"><em>No results.</em></p>");
        } else {
            html.push_str("<table class=\"noet-query-table\">\n");

            // Header row
            if let Some(header) = rows.first() {
                html.push_str("<thead><tr>");
                for cell in header {
                    html.push_str("<th>");
                    html.push_str(&html_escape(cell));
                    html.push_str("</th>");
                }
                html.push_str("</tr></thead>\n");
            }

            // Data rows
            html.push_str("<tbody>\n");
            for row in rows.iter().skip(1) {
                html.push_str("<tr>");
                for cell in row {
                    html.push_str("<td>");
                    html.push_str(&html_escape(cell));
                    html.push_str("</td>");
                }
                html.push_str("</tr>\n");
            }
            html.push_str("</tbody>\n");
            html.push_str("</table>");
        }

        if caption.is_some() {
            html.push_str("\n</figure>");
        }

        html
    }

    /// Render Depth0 mode as a definition-list-style HTML block.
    ///
    /// Each node entry renders as:
    /// - A header with `id: title` (where title is a link placeholder)
    /// - The node's `payload.text` rendered as inline markdown
    ///
    /// This mirrors the metadata panel's title + text rendering in the
    /// browser viewer.
    fn render_depth0_list(
        &self,
        entries: &[(Bid, Score)],
        graph: &BeliefGraph,
        links: Option<&dyn LinkResolver>,
    ) -> String {
        let mut html = String::new();

        html.push_str("<div class=\"noet-query-result\">\n");

        if let Some(cap) = &self.caption {
            html.push_str("<figure>\n<figcaption>");
            html.push_str(&html_escape(cap));
            html.push_str("</figcaption>\n");
        }

        // Check if there are any renderable entries
        let has_entries = entries
            .iter()
            .any(|(bid, _)| graph.states.contains_key(bid));

        if !has_entries {
            html.push_str("<p class=\"noet-query-empty\"><em>No results.</em></p>\n");
            if self.caption.is_some() {
                html.push_str("</figure>\n");
            }
            html.push_str("</div>");
            return html;
        }

        let mut rendered_count = 0;
        html.push_str("<dl class=\"noet-query-depth0\">\n");

        for (bid, _score) in entries {
            if let Some(max) = self.max_rows {
                if rendered_count >= max {
                    break;
                }
            }
            let Some(node) = graph.states.get(bid) else {
                continue;
            };

            let id = node.id();
            let title = node.display_title();

            // Term: [id: title] as a linked header
            let link_content =
                format!("<code>{}</code>: {}", html_escape(&id), html_escape(&title),);
            let dt_inner = match links {
                Some(resolver) => {
                    let mut anchor = resolver.render_anchor(node, &link_content);
                    // Append schema badge if present
                    if let Some(schema) = &node.schema {
                        anchor.push_str(&format!(
                            " <span class=\"noet-query-schema\">({})</span>",
                            html_escape(schema)
                        ));
                    }
                    anchor
                }
                None => {
                    let mut text = link_content;
                    if let Some(schema) = &node.schema {
                        text.push_str(&format!(" ({})", html_escape(schema)));
                    }
                    text
                }
            };
            html.push_str("<dt class=\"noet-query-node-title\">");
            html.push_str(&dt_inner);
            html.push_str("</dt>\n");

            // Definition: node text rendered as markdown
            html.push_str("<dd class=\"noet-query-node-text\">");
            let text_html = node.render_text_html();
            if !text_html.is_empty() {
                html.push_str(&text_html);
            }
            html.push_str("</dd>\n");

            rendered_count += 1;
        }

        html.push_str("</dl>\n");

        if self.caption.is_some() {
            html.push_str("</figure>\n");
        }

        html.push_str("</div>");

        html
    }
}

impl TableView {
    /// Extract the result entries from a package, preserving tape order.
    ///
    /// Returns BIDs in the order they appear in the tape's `output_bids`,
    /// which reflects `WEIGHT_SORT_KEY` structural ordering. Seed BIDs
    /// are prepended (they are the query's starting point but don't appear
    /// in traversal output).
    fn extract_entries(package: &QueryPackage) -> Vec<(Bid, Score)> {
        let tape = package.tape();
        let boundary = tape.graph_context_boundary();

        // Seed BIDs: the query's starting point.
        let seed: Vec<Bid> = package
            .spec()
            .steps
            .first()
            .and_then(|s| match &s.input {
                TapeFn::Bids(bids) => Some(bids.clone()),
                _ => None,
            })
            .unwrap_or_default();

        if boundary == 0 {
            return seed.into_iter().map(|bid| (bid, Some(1.0))).collect();
        }

        // Composition queries: use the compose result (last entry) only.
        if tape.has_composition() {
            return tape.steps[boundary - 1]
                .content
                .output_bids()
                .into_iter()
                .map(|bid| (bid, Some(1.0)))
                .collect();
        }

        // Non-composition: collect BIDs in tape order, deduplicating while
        // preserving structural order.
        //
        // Prepend seed BIDs only when the first user step is a traversal —
        // the seed is the structural root that should appear first. For filter
        // or identity steps, the seed is the input set and should not be
        // re-added if the step rejected it.
        let mut seen = BTreeSet::new();
        let mut ordered = Vec::new();
        let first_step_is_traverse = package
            .spec()
            .steps
            .first()
            .map(|s| matches!(s.operation, crate::query::spec::StepOperation::Traverse(_)))
            .unwrap_or(false);
        if first_step_is_traverse {
            for bid in &seed {
                if seen.insert(*bid) {
                    ordered.push(*bid);
                }
            }
        }
        for entry in &tape.steps[..boundary] {
            for bid in entry.content.output_bids() {
                if seen.insert(bid) {
                    ordered.push(bid);
                }
            }
        }
        ordered.into_iter().map(|bid| (bid, Some(1.0))).collect()
    }
}

impl ViewRenderer for TableView {
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
        let entries = Self::extract_entries(package);

        // When the tape has composition steps, render in comparison mode.
        if tape.has_composition() {
            return self.render_with_tape(&entries, tape, graph);
        }

        // Depth0 renders a definition-list-style block (id + title + text)
        // rather than a columnar table. All other modes use the row/table path.
        if self.display == TableDisplayMode::Depth0 {
            let html = self.render_depth0_list(&entries, graph, links);
            return Ok(ViewOutput::Html(html));
        }

        let rows = self.build_rows(&entries, graph);
        let html = Self::rows_to_html(&rows, self.caption.as_deref());
        Ok(ViewOutput::Html(html))
    }

    fn render_json(&self, package: &QueryPackage) -> Result<ViewOutput, BuildonomyError> {
        let graph = package
            .graph()
            .expect("render_json requires a populated graph");
        let entries = Self::extract_entries(package);

        if self.display == TableDisplayMode::Connectivity {
            return self.render_json_connectivity(&entries, graph);
        }

        // Depth0 / Columns: flat string rows (existing behavior) + nodes map.
        let rows = self.build_rows(&entries, graph);

        let headers: Vec<serde_json::Value> = rows
            .first()
            .map(|h| {
                h.iter()
                    .map(|s| serde_json::Value::String(s.clone()))
                    .collect()
            })
            .unwrap_or_default();

        let data_rows: Vec<serde_json::Value> = rows
            .iter()
            .skip(1)
            .zip(entries.iter())
            .map(|(row, (bid, _score))| {
                serde_json::json!({
                    "bid": bid.to_string(),
                    "cells": row,
                })
            })
            .collect();

        let nodes = node_info_map(entries.iter().map(|(bid, _)| *bid), graph);

        let json = serde_json::json!({
            "display": format!("{:?}", self.display),
            "headers": headers,
            "rows": data_rows,
            "nodes": nodes,
        });

        Ok(ViewOutput::Json(json))
    }
}

impl TableView {
    /// Render as structured rows instead of HTML.
    pub fn render_rows(&self, entries: &[(Bid, Score)], graph: &BeliefGraph) -> Vec<Vec<String>> {
        self.build_rows(entries, graph)
    }

    /// Render Connectivity mode as JSON with per-edge detail cells.
    ///
    /// Unlike `build_rows` (which emits edge counts as strings), this method
    /// produces one sub-row per edge endpoint, matching the JS viewer's current
    /// per-edge rendering. Each cell is a `{ "bid": "..." }` node reference.
    ///
    /// For each entry node (y-axis), edges are grouped into six columns
    /// (Section In/Out, Epistemic In/Out, Pragmatic In/Out). The sub-row count
    /// is the maximum edge count across all columns for that node.
    fn render_json_connectivity(
        &self,
        entries: &[(Bid, Score)],
        graph: &BeliefGraph,
    ) -> Result<ViewOutput, BuildonomyError> {
        let g = graph.relations.as_graph();
        let headers = self.headers();

        // Pre-compute per-node edge lists: (kind, is_outgoing) → Vec<Bid>.
        // We walk all edges once and bucket by endpoint.
        let mut edge_map: BTreeMap<Bid, BTreeMap<(WeightKind, bool), Vec<Bid>>> = BTreeMap::new();
        for edge_ref in g.edge_references() {
            let source_bid = g[edge_ref.source()];
            let sink_bid = g[edge_ref.target()];
            let ws = edge_ref.weight();
            for kind in ws.weights.keys() {
                // source_bid is the child; sink_bid is the parent.
                // "In" = sources/children (incoming), "Out" = sinks/parents (outgoing).
                // From source_bid's perspective: sink_bid is an outgoing neighbor.
                edge_map
                    .entry(source_bid)
                    .or_default()
                    .entry((*kind, true))
                    .or_default()
                    .push(sink_bid);
                // From sink_bid's perspective: source_bid is an incoming neighbor.
                edge_map
                    .entry(sink_bid)
                    .or_default()
                    .entry((*kind, false))
                    .or_default()
                    .push(source_bid);
            }
        }

        // Column order: [Node, Section In, Section Out, Epistemic In, Epistemic Out,
        //                Pragmatic In, Pragmatic Out]
        let col_keys: Vec<(WeightKind, bool)> = vec![
            (WeightKind::Section, false),
            (WeightKind::Section, true),
            (WeightKind::Epistemic, false),
            (WeightKind::Epistemic, true),
            (WeightKind::Pragmatic, false),
            (WeightKind::Pragmatic, true),
        ];

        let mut all_rows: Vec<EntryRow> = Vec::new();
        let mut all_bids: BTreeSet<Bid> = BTreeSet::new();

        for (bid, _) in entries {
            let node = match graph.states.get(bid) {
                Some(n) => n,
                None => continue,
            };
            all_bids.insert(*bid);

            let node_edges = edge_map.get(bid);

            // Compute sub-row count = max edges across all columns for this node.
            let sub_row_count = col_keys
                .iter()
                .map(|key| node_edges.and_then(|m| m.get(key)).map_or(0, |v| v.len()))
                .max()
                .unwrap_or(0)
                .max(1); // At least one row per node.

            for sub_idx in 0..sub_row_count {
                let mut cells = Vec::with_capacity(headers.len());

                // Node cell: first sub-row gets the node reference, rest empty.
                if sub_idx == 0 {
                    cells.push(Cell::node(node.title.clone(), *bid));
                } else {
                    cells.push(Cell::text(""));
                }

                // Edge columns.
                for key in &col_keys {
                    let edge_bids = node_edges.and_then(|m| m.get(key));
                    match edge_bids.and_then(|v| v.get(sub_idx)) {
                        Some(edge_bid) => {
                            all_bids.insert(*edge_bid);
                            let title = graph
                                .states
                                .get(edge_bid)
                                .map(|n| n.title.as_str())
                                .unwrap_or("");
                            cells.push(Cell::node(title, *edge_bid));
                        }
                        None => {
                            cells.push(Cell::text(""));
                        }
                    }
                }

                all_rows.push(EntryRow { cells });
            }
        }

        let json_rows = cells_to_json(&all_rows);
        let nodes = node_info_map(all_bids.into_iter(), graph);

        let json = serde_json::json!({
            "display": "Connectivity",
            "headers": headers,
            "rows": json_rows,
            "nodes": nodes,
        });

        Ok(ViewOutput::Json(json))
    }

    /// Render a composition result using tape entries for provenance.
    ///
    /// The tape layout for a Compose step is:
    ///   [ci-2] left branch result
    ///   [ci-1] right branch result
    ///   [ci]   composed (merged) result
    ///
    /// This method reads the tape to derive the "Side" column and gap-summary
    /// caption, producing the same output that `render_comparison` used to.
    fn render_with_tape(
        &self,
        entries: &[(Bid, Score)],
        tape: &Tape,
        graph: &BeliefGraph,
    ) -> Result<ViewOutput, BuildonomyError> {
        let op = tape.composition_op().unwrap_or(CompositionOp::And);

        // Build the base rows (header + data) from the entries.
        let mut rows = self.build_rows(entries, graph);

        // Prepend a "Side" column to the header.
        if let Some(header) = rows.first_mut() {
            header.insert(0, "Side".to_string());
        }

        // Prepend the provenance indicator to each data row.
        for (i, (bid, _)) in entries.iter().enumerate() {
            let row_idx = i + 1; // skip header
            if row_idx >= rows.len() {
                break; // truncated by max_rows
            }
            let indicator = tape.provenance_label(bid, op);
            rows[row_idx].insert(0, indicator);
        }

        // Build the caption. For Difference, include gap summary counts.
        let caption = match op {
            CompositionOp::Not => {
                let left_count = tape.left_count().unwrap_or(0);
                let right_count = tape.right_count().unwrap_or(0);
                let gap_count = entries.len();
                let summary = format!(
                    "{left_count} items in left, {right_count} items in right, \
                     {gap_count} items uncovered (gap)"
                );
                match &self.caption {
                    Some(cap) => Some(format!("{cap} \u{2014} {summary}")),
                    None => Some(summary),
                }
            }
            _ => self.caption.clone(),
        };

        let html = Self::comparison_rows_to_html(&rows, caption.as_deref());
        Ok(ViewOutput::Html(html))
    }

    /// Render structured rows as an HTML `<table>` with comparison styling.
    ///
    /// Identical to [`rows_to_html`](Self::rows_to_html) except it uses
    /// `class="noet-query-comparison"` on the table element.
    fn comparison_rows_to_html(rows: &[Vec<String>], caption: Option<&str>) -> String {
        let mut html = String::new();

        if let Some(cap) = caption {
            html.push_str("<figure>\n<figcaption>");
            html.push_str(&html_escape(cap));
            html.push_str("</figcaption>\n");
        }

        html.push_str("<table class=\"noet-query-comparison\">\n");

        // Header row
        if let Some(header) = rows.first() {
            html.push_str("<thead><tr>");
            for cell in header {
                html.push_str("<th>");
                html.push_str(&html_escape(cell));
                html.push_str("</th>");
            }
            html.push_str("</tr></thead>\n");
        }

        // Data rows
        html.push_str("<tbody>\n");
        for row in rows.iter().skip(1) {
            html.push_str("<tr>");
            for cell in row {
                html.push_str("<td>");
                html.push_str(&html_escape(cell));
                html.push_str("</td>");
            }
            html.push_str("</tr>\n");
        }
        html.push_str("</tbody>\n");
        html.push_str("</table>");

        if caption.is_some() {
            html.push_str("\n</figure>");
        }

        html
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// Rendering helpers
// ═════════════════════════════════════════════════════════════════════════════

/// Convert a list of resolved TOML values into a single display string.
fn toml_values_to_display(values: &[toml::Value]) -> String {
    if values.is_empty() {
        return String::new();
    }
    if values.len() == 1 {
        return toml_value_to_string(&values[0]);
    }
    // Multiple values: join with ", "
    values
        .iter()
        .map(toml_value_to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Convert a single TOML value to a display string.
fn toml_value_to_string(val: &toml::Value) -> String {
    match val {
        toml::Value::String(s) => s.clone(),
        toml::Value::Integer(n) => n.to_string(),
        toml::Value::Float(f) => f.to_string(),
        toml::Value::Boolean(b) => b.to_string(),
        toml::Value::Datetime(d) => d.to_string(),
        toml::Value::Array(arr) => {
            let inner: Vec<String> = arr.iter().map(toml_value_to_string).collect();
            format!("[{}]", inner.join(", "))
        }
        toml::Value::Table(t) => {
            // Compact representation for nested tables
            let pairs: Vec<String> = t
                .iter()
                .map(|(k, v)| format!("{k}={}", toml_value_to_string(v)))
                .collect();
            format!("{{{}}}", pairs.join(", "))
        }
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// ViewRegistry factory functions
// ═════════════════════════════════════════════════════════════════════════════

/// Factory: depth0 mode (title, schema, kind). This is the default view.
pub(crate) fn depth0_factory(
    params: &Table,
) -> Result<Box<dyn crate::query::view::ViewRenderer>, BuildonomyError> {
    let mut p = params.clone();
    p.insert("display".into(), TomlValue::String("depth0".into()));
    Ok(Box::new(TableView::from_params(&p)?))
}

/// Factory: connectivity mode (connectivity matrix: In/Out per WeightKind).
pub(crate) fn connectivity_factory(
    params: &Table,
) -> Result<Box<dyn crate::query::view::ViewRenderer>, BuildonomyError> {
    let mut p = params.clone();
    p.insert("display".into(), TomlValue::String("connectivity".into()));
    Ok(Box::new(TableView::from_params(&p)?))
}

/// Factory: columns mode (explicit column list from `params["columns"]`).
pub(crate) fn columns_factory(
    params: &Table,
) -> Result<Box<dyn crate::query::view::ViewRenderer>, BuildonomyError> {
    let mut p = params.clone();
    p.entry("display".to_string())
        .or_insert(TomlValue::String("columns".into()));
    Ok(Box::new(TableView::from_params(&p)?))
}

/// Minimal HTML escaping for table cell content.
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

// ═════════════════════════════════════════════════════════════════════════════
// Tests
// ═════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    use rustc_hash::FxHashMap;

    use crate::beliefbase::BidGraph;
    use crate::properties::{BeliefKind, Bid, NodeId, WeightKind, WeightSet};
    use crate::query::spec::{
        CompareOp, CompositionOp, NodeFilter, ProjectionStep, PropertyPredicate, PropertyValue,
        QuerySpec, Score, Tape, TapeContent, TapeEntry, TapeFn,
    };

    /// Build a minimal `BeliefGraph` from nodes and weighted edges.
    fn make_test_graph(nodes: Vec<BeliefNode>, edges: Vec<(Bid, Bid, WeightSet)>) -> BeliefGraph {
        let states: FxHashMap<Bid, BeliefNode> = nodes.iter().map(|n| (n.bid, n.clone())).collect();
        let relations = BidGraph::from_edges(edges);
        BeliefGraph { states, relations }
    }

    /// Build a test node with a specific BID, title, schema, and payload.
    fn named_node(bid: Bid, title: &str, schema: Option<&str>, payload: Table) -> BeliefNode {
        BeliefNode {
            bid,
            kind: BeliefKind::Document.into(),
            title: title.to_string(),
            schema: schema.map(|s| s.to_string()),
            payload,
            id: NodeId::Explicit(title.to_lowercase().replace(' ', "-")),
            metadata: Table::new(),
        }
    }

    /// Build a `QueryPackage` from seed BIDs and a graph. The package has
    /// no projection steps and no tape entries, so `render` will fall back
    /// to the seed BIDs.
    fn make_package(bids: Vec<Bid>, graph: BeliefGraph) -> QueryPackage {
        let spec = QuerySpec::seed(TapeFn::Bids(bids));
        let mut package = QueryPackage::new(spec);
        package.set_graph(graph);
        package
    }

    /// Build a `QueryPackage` with a tape entry so that `render` uses the
    /// tape's last entry for the BID set (preserving order and scores).
    #[allow(dead_code)]
    fn make_package_with_entries(entries: Vec<(Bid, Score)>, graph: BeliefGraph) -> QueryPackage {
        let bids: Vec<Bid> = entries.iter().map(|(bid, _)| *bid).collect();
        let spec = QuerySpec::seed_then(TapeFn::Bids(bids), vec![dummy_filter_step()]);
        let mut package = QueryPackage::new(spec);
        package.set_graph(graph);
        // Push a tape entry with the entries as the result.
        let tape = package.tape_mut();
        tape.steps.push(TapeEntry {
            label: "0".to_string(),
            content: TapeContent::Nodes(entries.iter().map(|(bid, _)| *bid).collect()),
            payload: None,
        });
        package
    }

    /// Build a `QueryPackage` with a composition tape.
    fn make_package_with_tape(
        merged_entries: Vec<(Bid, Score)>,
        tape: Tape,
        graph: BeliefGraph,
    ) -> QueryPackage {
        let bids: Vec<Bid> = merged_entries.iter().map(|(bid, _)| *bid).collect();
        // The spec needs enough steps to match the tape length.
        let steps: Vec<ProjectionStep> = tape.steps.iter().map(|_| dummy_filter_step()).collect();
        let spec = QuerySpec::seed_then(TapeFn::Bids(bids), steps);
        let mut package = QueryPackage::new(spec);
        package.set_graph(graph);
        *package.tape_mut() = tape;
        package
    }

    /// A dummy filter step for tape entries.
    fn dummy_filter_step() -> ProjectionStep {
        ProjectionStep::filter(NodeFilter::Predicate(PropertyPredicate {
            path: vec![],
            op: CompareOp::Exists,
            value: PropertyValue::None,
        }))
    }

    #[test]
    fn from_params_explicit_columns() {
        let mut params = Table::new();
        params.insert(
            "columns".into(),
            toml::Value::Array(vec![
                toml::Value::String("title".into()),
                toml::Value::String("payload.status".into()),
                toml::Value::String("schema".into()),
            ]),
        );
        params.insert("caption".into(), toml::Value::String("My Table".into()));
        params.insert("max_rows".into(), toml::Value::Integer(10));

        let instr = TableView::from_params(&params).unwrap();
        assert_eq!(instr.display, TableDisplayMode::Columns);
        assert_eq!(instr.columns.len(), 3);
        assert_eq!(instr.columns[0].0, "title");
        assert_eq!(instr.columns[1].0, "status");
        assert_eq!(instr.columns[2].0, "schema");
        assert_eq!(instr.max_rows, Some(10));
        assert_eq!(instr.caption.as_deref(), Some("My Table"));
    }

    #[test]
    fn from_params_defaults_to_depth0() {
        let params = Table::new();
        let instr = TableView::from_params(&params).unwrap();
        assert_eq!(instr.display, TableDisplayMode::Depth0);
        assert!(instr.columns.is_empty());
        assert_eq!(instr.max_rows, None);
        assert_eq!(instr.caption, None);
    }

    #[test]
    fn from_params_explicit_display_mode() {
        let mut params = Table::new();
        params.insert("display".into(), toml::Value::String("connectivity".into()));
        let instr = TableView::from_params(&params).unwrap();
        assert_eq!(instr.display, TableDisplayMode::Connectivity);
    }

    #[test]
    fn from_params_unknown_display_errors() {
        let mut params = Table::new();
        params.insert("display".into(), toml::Value::String("sparkline".into()));
        assert!(TableView::from_params(&params).is_err());
    }

    #[test]
    fn render_depth0_mode() {
        let bid_a = Bid::new(Bid::nil());
        let bid_b = Bid::new(bid_a);

        let mut payload_a = Table::new();
        payload_a.insert("status".into(), toml::Value::String("open".into()));

        let node_a = named_node(bid_a, "Alpha", Some("procedure"), payload_a);
        let node_b = named_node(bid_b, "Beta", None, Table::new());

        let graph = make_test_graph(vec![node_a, node_b], vec![]);
        let entries: Vec<(Bid, Score)> = vec![(bid_a, Some(1.0)), (bid_b, Some(0.5))];

        let instr = TableView::from_params(&Table::new()).unwrap();
        let rows = instr.render_rows(&entries, &graph);

        assert_eq!(rows.len(), 3); // header + 2 data rows
        assert_eq!(rows[0], vec!["Title", "Schema", "Kind"]);
        assert_eq!(rows[1][0], "Alpha");
        assert_eq!(rows[1][1], "procedure");
        assert_eq!(rows[2][0], "Beta");
        assert_eq!(rows[2][1], ""); // None schema
    }

    #[test]
    fn render_columns_mode_with_payload() {
        let bid = Bid::new(Bid::nil());
        let mut payload = Table::new();
        payload.insert("status".into(), toml::Value::String("closed".into()));
        payload.insert("priority".into(), toml::Value::Integer(5));

        let node = named_node(bid, "Item", Some("req"), payload);
        let graph = make_test_graph(vec![node], vec![]);
        let entries: Vec<(Bid, Score)> = vec![(bid, Some(1.0))];

        let mut params = Table::new();
        params.insert(
            "columns".into(),
            toml::Value::Array(vec![
                toml::Value::String("title".into()),
                toml::Value::String("payload.status".into()),
                toml::Value::String("payload.priority".into()),
            ]),
        );

        let instr = TableView::from_params(&params).unwrap();
        let rows = instr.render_rows(&entries, &graph);

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0], vec!["title", "status", "priority"]);
        assert_eq!(rows[1], vec!["Item", "closed", "5"]);
    }

    #[test]
    fn render_connectivity_mode() {
        let bid_a = Bid::new(Bid::nil());
        let bid_b = Bid::new(bid_a);
        let bid_c = Bid::new(bid_b);

        let node_a = named_node(bid_a, "A", None, Table::new());
        let node_b = named_node(bid_b, "B", None, Table::new());
        let node_c = named_node(bid_c, "C", None, Table::new());

        // A --(Pragmatic)--> B, A --(Section)--> C
        let ws_prag = WeightSet::from(WeightKind::Pragmatic);
        let ws_sect = WeightSet::from(WeightKind::Section);
        let graph = make_test_graph(
            vec![node_a, node_b, node_c],
            vec![(bid_a, bid_b, ws_prag), (bid_a, bid_c, ws_sect)],
        );

        let entries: Vec<(Bid, Score)> = vec![(bid_a, Some(1.0)), (bid_b, Some(1.0))];

        let mut params = Table::new();
        params.insert("display".into(), toml::Value::String("connectivity".into()));
        let instr = TableView::from_params(&params).unwrap();
        let rows = instr.render_rows(&entries, &graph);

        // Header: Node | Section In | Section Out | Epistemic In | Epistemic Out | Pragmatic In | Pragmatic Out
        assert_eq!(rows[0].len(), 7);
        assert_eq!(rows[0][0], "Node");

        // Node A: Section Out=1, Pragmatic Out=1, all In=0
        assert_eq!(rows[1][0], "A");
        assert_eq!(rows[1][1], "0"); // Section In
        assert_eq!(rows[1][2], "1"); // Section Out
        assert_eq!(rows[1][3], "0"); // Epistemic In
        assert_eq!(rows[1][4], "0"); // Epistemic Out
        assert_eq!(rows[1][5], "0"); // Pragmatic In
        assert_eq!(rows[1][6], "1"); // Pragmatic Out

        // Node B: Pragmatic In=1, all Out=0
        assert_eq!(rows[2][0], "B");
        assert_eq!(rows[2][5], "1"); // Pragmatic In
        assert_eq!(rows[2][6], "0"); // Pragmatic Out
    }

    #[test]
    fn render_max_rows_truncates() {
        let bid_a = Bid::new(Bid::nil());
        let bid_b = Bid::new(bid_a);
        let bid_c = Bid::new(bid_b);

        let node_a = named_node(bid_a, "A", None, Table::new());
        let node_b = named_node(bid_b, "B", None, Table::new());
        let node_c = named_node(bid_c, "C", None, Table::new());

        let graph = make_test_graph(vec![node_a, node_b, node_c], vec![]);
        let entries: Vec<(Bid, Score)> =
            vec![(bid_a, Some(1.0)), (bid_b, Some(1.0)), (bid_c, Some(1.0))];

        let mut params = Table::new();
        params.insert("max_rows".into(), toml::Value::Integer(2));
        let instr = TableView::from_params(&params).unwrap();
        let rows = instr.render_rows(&entries, &graph);

        // header + 2 data rows (not 3)
        assert_eq!(rows.len(), 3);
    }

    #[test]
    fn render_html_output() {
        let bid = Bid::new(Bid::nil());
        let node = named_node(bid, "Item", Some("spec"), Table::new());
        let graph = make_test_graph(vec![node], vec![]);
        let package = make_package(vec![bid], graph);

        let instr = TableView::from_params(&Table::new()).unwrap();
        let output = instr.render(&package, None).unwrap();
        match output {
            ViewOutput::Html(html) => {
                // Depth0 renders as a definition list, not a table.
                assert!(html.contains("<dl class=\"noet-query-depth0\">"));
                assert!(html.contains("Item"), "title should appear in output");
                assert!(html.contains("(spec)"), "schema should appear as badge");
            }
            other => panic!("expected Html, got {other:?}"),
        }
    }

    #[test]
    fn render_html_with_caption() {
        let bid = Bid::new(Bid::nil());
        let node = named_node(bid, "X", None, Table::new());
        let graph = make_test_graph(vec![node], vec![]);
        let package = make_package(vec![bid], graph);

        let mut params = Table::new();
        params.insert("caption".into(), toml::Value::String("Test Caption".into()));
        let instr = TableView::from_params(&params).unwrap();
        let output = instr.render(&package, None).unwrap();
        match output {
            ViewOutput::Html(html) => {
                assert!(html.contains("<figure>"));
                assert!(html.contains("<figcaption>Test Caption</figcaption>"));
                assert!(html.contains("</figure>"));
            }
            other => panic!("expected Html, got {other:?}"),
        }
    }

    #[test]
    fn html_escaping_in_cells() {
        let bid = Bid::new(Bid::nil());
        let mut payload = Table::new();
        payload.insert(
            "note".into(),
            toml::Value::String("<script>alert('x')</script>".into()),
        );
        let node = named_node(bid, "A & B", None, payload);
        let graph = make_test_graph(vec![node], vec![]);
        let package = make_package(vec![bid], graph);

        let mut params = Table::new();
        params.insert(
            "columns".into(),
            toml::Value::Array(vec![
                toml::Value::String("title".into()),
                toml::Value::String("payload.note".into()),
            ]),
        );
        let instr = TableView::from_params(&params).unwrap();
        let output = instr.render(&package, None).unwrap();
        match output {
            ViewOutput::Html(html) => {
                assert!(html.contains("A &amp; B"));
                assert!(html.contains("&lt;script&gt;"));
                assert!(!html.contains("<script>"));
            }
            other => panic!("expected Html, got {other:?}"),
        }
    }

    // ── Render with tape (comparison rendering) ────────────────────────────

    /// Build a tape simulating a Compose evaluation.
    fn make_composition_tape(
        left_entries: Vec<(Bid, Score)>,
        right_entries: Vec<(Bid, Score)>,
        merged_entries: Vec<(Bid, Score)>,
        op: CompositionOp,
    ) -> Tape {
        let left_bids: BTreeSet<Bid> = left_entries.iter().map(|(bid, _)| *bid).collect();
        let right_bids: BTreeSet<Bid> = right_entries.iter().map(|(bid, _)| *bid).collect();
        let intersection: Vec<Bid> = left_bids.intersection(&right_bids).copied().collect();
        Tape {
            steps: vec![
                TapeEntry {
                    label: "0".to_string(),
                    content: TapeContent::Nodes(
                        left_entries.into_iter().map(|(bid, _)| bid).collect(),
                    ),
                    payload: None,
                },
                TapeEntry {
                    label: "1".to_string(),
                    content: TapeContent::Nodes(
                        right_entries.into_iter().map(|(bid, _)| bid).collect(),
                    ),
                    payload: None,
                },
                TapeEntry {
                    label: "2".to_string(),
                    content: TapeContent::Compose {
                        op,
                        left: 0..1,
                        right: 1..2,
                        result: merged_entries.into_iter().map(|(bid, _)| bid).collect(),
                        intersection,
                    },
                    payload: None,
                },
            ],
        }
    }

    #[test]
    fn render_difference_has_side_column() {
        // Simulate a Difference gap analysis: left has {A,B,C}, right has {B,C}.
        // Merged (gap) = {A} — the uncovered item.
        let bid_a = Bid::new(Bid::nil());
        let bid_b = Bid::new(bid_a);
        let bid_c = Bid::new(bid_b);

        let node_a = named_node(bid_a, "Alpha", Some("req"), Table::new());
        let node_b = named_node(bid_b, "Beta", Some("req"), Table::new());
        let node_c = named_node(bid_c, "Gamma", Some("req"), Table::new());

        let graph = make_test_graph(vec![node_a, node_b, node_c], vec![]);

        let tape = make_composition_tape(
            vec![(bid_a, Some(1.0)), (bid_b, Some(1.0)), (bid_c, Some(1.0))],
            vec![(bid_b, Some(1.0)), (bid_c, Some(1.0))],
            vec![(bid_a, Some(1.0))],
            CompositionOp::Not,
        );

        let merged = vec![(bid_a, Some(1.0))];
        let package = make_package_with_tape(merged, tape, graph);

        let instr = TableView::from_params(&Table::new()).unwrap();
        let output = instr.render(&package, None).unwrap();

        match output {
            ViewOutput::Html(html) => {
                assert!(
                    html.contains("noet-query-comparison"),
                    "should use comparison class"
                );
                assert!(html.contains("<th>Side</th>"), "should have Side header");
                // The gap item should have a checkmark
                assert!(
                    html.contains("\u{2713}"),
                    "gap item should be marked with checkmark"
                );
                assert!(html.contains("Alpha"), "gap item title should appear");
                // Gap summary caption
                assert!(html.contains("3 items in left"), "should show left count");
                assert!(html.contains("2 items in right"), "should show right count");
                assert!(
                    html.contains("1 items uncovered (gap)"),
                    "should show gap count"
                );
            }
            other => panic!("expected Html, got {other:?}"),
        }
    }

    #[test]
    fn render_or_shows_left_right_both() {
        let bid_a = Bid::new(Bid::nil());
        let bid_b = Bid::new(bid_a);
        let bid_c = Bid::new(bid_b);

        let node_a = named_node(bid_a, "OnlyLeft", None, Table::new());
        let node_b = named_node(bid_b, "InBoth", None, Table::new());
        let node_c = named_node(bid_c, "OnlyRight", None, Table::new());

        let graph = make_test_graph(vec![node_a, node_b, node_c], vec![]);

        let tape = make_composition_tape(
            vec![(bid_a, Some(1.0)), (bid_b, Some(1.0))],
            vec![(bid_b, Some(1.0)), (bid_c, Some(1.0))],
            vec![(bid_a, Some(1.0)), (bid_b, Some(1.0)), (bid_c, Some(1.0))],
            CompositionOp::Or,
        );

        let merged = vec![(bid_a, Some(1.0)), (bid_b, Some(1.0)), (bid_c, Some(1.0))];
        let package = make_package_with_tape(merged, tape, graph);

        let instr = TableView::from_params(&Table::new()).unwrap();
        let output = instr.render(&package, None).unwrap();

        match output {
            ViewOutput::Html(html) => {
                assert!(html.contains("<th>Side</th>"));
                assert!(
                    html.contains("<td>Left</td>"),
                    "left-only item should say Left"
                );
                assert!(
                    html.contains("<td>Both</td>"),
                    "intersection item should say Both"
                );
                assert!(
                    html.contains("<td>Right</td>"),
                    "right-only item should say Right"
                );
            }
            other => panic!("expected Html, got {other:?}"),
        }
    }

    #[test]
    fn render_with_empty_tape_produces_standard_output() {
        let bid = Bid::new(Bid::nil());
        let node = named_node(bid, "Plain", None, Table::new());
        let graph = make_test_graph(vec![node], vec![]);
        let package = make_package(vec![bid], graph);

        let instr = TableView::from_params(&Table::new()).unwrap();
        let output = instr.render(&package, None).unwrap();

        match output {
            ViewOutput::Html(html) => {
                // Depth0 renders a definition list, not a table with Side column.
                assert!(
                    html.contains("noet-query-depth0"),
                    "should use depth0 definition list class"
                );
                assert!(
                    !html.contains("<th>Side</th>"),
                    "should not have Side column"
                );
            }
            other => panic!("expected Html, got {other:?}"),
        }
    }

    #[test]
    fn render_json_connectivity_mode() {
        let bid_a = Bid::new(Bid::nil());
        let bid_b = Bid::new(bid_a);

        let node_a = named_node(bid_a, "Alpha", Some("req"), Table::new());
        let node_b = named_node(bid_b, "Beta", Some("req"), Table::new());

        // Section edge: a → b (a is source/child, b is sink/parent).
        let ws = WeightSet::from(WeightKind::Section);
        let graph = make_test_graph(vec![node_a, node_b], vec![(bid_a, bid_b, ws)]);

        let entries: Vec<(Bid, Score)> = vec![(bid_a, Some(1.0)), (bid_b, Some(1.0))];
        let package = make_package_with_entries(entries, graph);

        let mut params = Table::new();
        params.insert("display".into(), toml::Value::String("connectivity".into()));
        let view = TableView::from_params(&params).unwrap();
        let output = view.render_json(&package).unwrap();

        match output {
            ViewOutput::Json(json) => {
                assert_eq!(json["display"], "Connectivity");
                let headers = json["headers"].as_array().unwrap();
                assert_eq!(headers[0], "Node");
                assert_eq!(headers.len(), 7); // Node + 6 edge columns

                let rows = json["rows"].as_array().unwrap();
                // Alpha: 1 Section Out (to Beta). Beta: 1 Section In (from Alpha).
                // Each produces 1 row. Total = 2 rows.
                assert_eq!(rows.len(), 2);

                // Row 0 (Alpha): Node cell is a BID reference, not a plain string.
                let alpha_cells = rows[0]["cells"].as_array().unwrap();
                assert!(
                    alpha_cells[0]["bid"].is_string(),
                    "Node cell should be a BID ref"
                );

                // Alpha has Section Out → Beta (column index 2 = Section Out).
                assert!(
                    alpha_cells[2]["bid"].is_string(),
                    "Section Out should have Beta"
                );
                // Alpha has no Section In (column index 1).
                assert_eq!(alpha_cells[1], "", "Section In should be empty for Alpha");

                // Row 1 (Beta): Section In → Alpha.
                let beta_cells = rows[1]["cells"].as_array().unwrap();
                assert!(
                    beta_cells[0]["bid"].is_string(),
                    "Node cell should be a BID ref"
                );
                assert!(
                    beta_cells[1]["bid"].is_string(),
                    "Section In should have Alpha"
                );
                assert_eq!(beta_cells[2], "", "Section Out should be empty for Beta");

                // Nodes map is present.
                assert!(json["nodes"].is_object());
                let nodes = json["nodes"].as_object().unwrap();
                assert_eq!(nodes.len(), 2);
                assert_eq!(nodes[&bid_a.to_string()]["title"], "Alpha");
                assert_eq!(nodes[&bid_b.to_string()]["title"], "Beta");
            }
            other => panic!("expected Json, got {other:?}"),
        }
    }

    #[test]
    fn render_json_connectivity_sub_rows() {
        // Node A has edges to both B and C. This should produce sub-rows.
        let bid_a = Bid::new(Bid::nil());
        let bid_b = Bid::new(bid_a);
        let bid_c = Bid::new(bid_b);

        let node_a = named_node(bid_a, "Alpha", None, Table::new());
        let node_b = named_node(bid_b, "Beta", None, Table::new());
        let node_c = named_node(bid_c, "Gamma", None, Table::new());

        // A → B (Section), A → C (Section): Alpha is child of both Beta and Gamma.
        let graph = make_test_graph(
            vec![node_a, node_b, node_c],
            vec![
                (bid_a, bid_b, WeightSet::from(WeightKind::Section)),
                (bid_a, bid_c, WeightSet::from(WeightKind::Section)),
            ],
        );

        let entries: Vec<(Bid, Score)> = vec![(bid_a, Some(1.0))];
        let package = make_package_with_entries(entries, graph);

        let mut params = Table::new();
        params.insert("display".into(), toml::Value::String("connectivity".into()));
        let view = TableView::from_params(&params).unwrap();
        let output = view.render_json(&package).unwrap();

        match output {
            ViewOutput::Json(json) => {
                let rows = json["rows"].as_array().unwrap();
                // Alpha has 2 Section Out edges → 2 sub-rows.
                assert_eq!(rows.len(), 2);

                // First sub-row has Alpha as the node cell.
                let r0 = rows[0]["cells"].as_array().unwrap();
                assert!(r0[0]["bid"].is_string(), "First sub-row has node BID");

                // Second sub-row has empty node cell.
                let r1 = rows[1]["cells"].as_array().unwrap();
                assert_eq!(r1[0], "", "Second sub-row has empty node cell");

                // Both sub-rows have a Section Out BID.
                assert!(r0[2]["bid"].is_string());
                assert!(r1[2]["bid"].is_string());

                // Nodes map includes all three nodes.
                let nodes = json["nodes"].as_object().unwrap();
                assert_eq!(nodes.len(), 3);
            }
            other => panic!("expected Json, got {other:?}"),
        }
    }
}
