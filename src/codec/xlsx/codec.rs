//! XlsxCodec — DocCodec implementation for `.xlsx` and `.ods` spreadsheet ingestion.
//!
//! ## Node hierarchy
//!
//! ```text
//! workbook.xlsx  (Document, heading=2)  ← title = WorkbookSchema.title
//!   ├── "Tab Name"  (Symbol, heading=3) ← one per sheet in workbook (workbook sheet order)
//!   │     ├── "Row Title"  (Symbol, heading=4) ← one per data row (schema-declared tabs)
//!   │     └── ...
//!   └── "Opaque Tab"  (Symbol, heading=3) ← ignored or schema-absent tabs; no row children
//! ```
//!
//! ## Column defaults (Issue 71)
//!
//! Columns not listed in a tab's `schema` receive lazy defaults:
//! 1. `__noet_<property>__` headers → reserved system columns (mapped to BeliefNode fields)
//! 2. First non-reserved column → `role: title` (when no explicit title declared)
//! 3. All remaining unlisted columns → `role: payload`
//!
//! ## BID write-back
//!
//! `generate_source_bytes()` uses `rust_xlsxwriter` to write a `__noet_bid__` column
//! back into each schema-declared tab. When `self.updated` is true (inject_context
//! mutated node state), additional reserved columns are also written.

use std::{
    collections::HashMap,
    io::Cursor,
    path::{Path, PathBuf},
    str::FromStr,
};

use calamine::{Data, DataType, Reader, Xlsx};
use rust_xlsxwriter::Workbook as XlsxWriterWorkbook;
use toml_edit::{value, DocumentMut};

use crate::{
    beliefbase::BeliefContext,
    codec::{
        belief_ir::{parse_with_fallback, IRNode, IntermediateRelation, MetadataFormat},
        md::{parse_markdown_relations, to_html},
        xlsx::schema::{
            ColumnRole, RelationDirection, RelationKeyFormat, RelationWeight, ReservedColumnKind,
            TabSchema, WorkbookSchema,
        },
        CodecContentMode, DocCodec, ParseDiagnostic,
    },
    error::BuildonomyError,
    nodekey::NodeKey,
    paths::{os_path_to_string, path::to_anchor},
    properties::{BeliefKind, BeliefKindSet, BeliefNode, Bid, NodeId, WeightKind},
};

/// Hidden column header injected by `generate_source_bytes()` to carry BIDs.
const BID_COLUMN_HEADER: &str = "__noet_bid__";

/// Prefix for hidden relation-bref columns: `__noet_relation_{ir_key}__`.
/// These columns store the resolved bref for each relation column value so that
/// subsequent parses can resolve via `NodeKey::Bref` instead of re-deriving from
/// the human-readable cell text.
const RELATION_BREF_COLUMN_PREFIX: &str = "__noet_relation_";

/// Name of the reserved index tab carrying the YAML schema declaration.
const INDEX_TAB: &str = "index";

/// Sentinel tab name that acts as a default schema for any workbook tab not
/// explicitly declared in the `tabs` list.
///
/// When a `TabSchema` with `name: "*"` is present, it is used as the schema
/// for every tab that has no exact-name match — instead of treating those tabs
/// as opaque. The wildcard entry is never itself treated as a real tab name.
///
/// Example:
/// ```yaml
/// title: "Widget Project"
/// tabs:
///   - name: "*"
///     text_template: "{{Description}}\n\n**Rationale**: {{Rationale}}"
///     schema:
///       - col: "Implements"
///         role: relation
/// ```
const WILDCARD_TAB: &str = "*";

/// Mapping from lowercase column header names to their conventional `ColumnRole`.
///
/// This is layer 2 of the three-layer column mapping:
///   1. Schema-defined (explicit `schema:` list)
///   2. Case-insensitive name match against this table  ← here
///   3. `__noet_<prop>__` reserved columns
///
/// Columns whose lowercase header appears here are promoted to the corresponding
/// role automatically when not already covered by an explicit schema declaration.
/// This avoids `__noet_*__` noise for columns authors naturally name with standard
/// vocabulary, while never conflicting with explicit schema entries.
///
/// `description` is intentionally excluded. `schema` is handled separately via
/// case-insensitive header detection
/// in the row loop (same semantics as `__noet_schema__`, injected into
/// `node.document["schema"]` rather than payload or text).
const CONVENTIONAL_ROLES: &[(&str, ColumnRole)] =
    &[("title", ColumnRole::Title), ("text", ColumnRole::Text)];

/// Maximum data rows per tab that the codec will parse.
///
/// This is a hard architectural limit imposed by `PathMap`'s `u16` sort-key space.
/// Each node's position among its siblings is stored as a `u16` edge weight, giving
/// a ceiling of `u16::MAX` (65_535) children per parent.
///
/// The `NETWORK_SECTION_SORT_KEY` sentinel (`u16::MAX`) reserved in `PathMap` for the
/// network's own `index.md` content plane does **not** apply here: row nodes are
/// children of a tab node (heading=3), which is itself a child of the workbook node
/// (heading=2). They are never direct children of the network root, so they use the
/// full `u16` address space independently.
///
/// A `ParseDiagnostic::Warning` is emitted when this limit is reached; rows beyond it
/// are silently dropped.
///
/// See `src/paths/pathmap.rs` (`NETWORK_SECTION_SORT_KEY`) for the sort-space layout.
const MAX_ROWS_PER_TAB: usize = u16::MAX as usize; // 65_535

/// Runtime role for a column, built from `ColumnRole` + `ReservedColumnKind`.
/// `Relation` carries its parameters inline so match arms are self-contained.
/// Not serialized — this is a codec-internal type.
#[derive(Debug, Clone)]
enum ColumnKind {
    Title,
    /// Cell content is parsed as Markdown — links become upstream graph edges.
    /// ir_key is always "text"; content is search-indexed via doc["text"].
    /// Produced by explicit `role: text` schema declarations, the conventional
    /// "text" header match, and (implicitly) by text_template composition.
    Markdown,
    Relation {
        weight: RelationWeight,
        direction: RelationDirection,
        key_format: RelationKeyFormat,
    },
    /// Hidden codec-managed column carrying the resolved bref for the sibling
    /// Relation column identified by `relation_ir_key`.
    ///
    /// At **read time**: the bref in the cell is pushed as a `NodeKey::Bref` edge
    /// alongside (or instead of) the text-derived NodeKey, giving stable resolution
    /// across renames without depending on slug matching.
    ///
    /// At **write time**: `write_annotated_sheet` fills this column from
    /// `XlsxCodec.row_relations` after `inject_context` resolves the edge targets.
    RelationBref {
        /// ir_key of the sibling `Relation` column this column annotates.
        relation_ir_key: String,
    },
    /// Plain string read → written to doc[ir_key].
    /// Covers user payload columns AND system/reserved fields (bid, id, schema).
    Payload,
}

/// Single source of truth for one header column, built once from the header row
/// and the tab schema.
#[derive(Debug, Clone)]
struct ColumnEntry {
    col_idx: usize,
    /// Raw header text as it appears in the sheet.
    header: String,
    /// Normalized document key: "bid", "id", "title", "text", "subsystem", etc.
    ir_key: String,
    kind: ColumnKind,
    /// True for __noet_*__ columns and codec-managed hidden columns.
    /// These are hidden in the sheet and excluded from generate_html col_defs.
    hidden: bool,
    /// Hit count: incremented per data row for non-empty cells.
    /// Used for the "never applied" diagnostic after the row loop.
    hits: usize,
}

/// Resolved relation bref for one cell, collected during inject_context for
/// write-back to a hidden __noet_relation_{col}__ column.
#[derive(Debug, Clone)]
pub(crate) struct RowRelation {
    pub(crate) tab: String,
    pub(crate) row: usize,   // 1-based
    pub(crate) col: String,  // ir_key of the sibling Relation column
    pub(crate) bref: String, // resolved bref string, e.g. "abc123def456"
}

/// Per-row BID annotation: (tab_name, 1-based row index, bid).
#[derive(Debug, Clone)]
pub(crate) struct RowBid {
    pub(crate) tab: String,
    /// 1-based data row index (row 1 = first data row after header).
    pub(crate) row: usize,
    pub(crate) bid: Bid,
    /// Column index in the source sheet that was the authoritative source for this
    /// BID (layer 1 schema, layer 2 name match, or layer 3 __noet_bid__).
    /// `None` means no source column existed at parse time — append __noet_bid__.
    pub(crate) source_col: Option<usize>,
}

/// State for one parsed node, mirroring the `(IRNode, ...)` pattern in MdCodec.
#[derive(Debug, Clone)]
struct ParsedNode {
    proto: IRNode,
    /// 1-based row index for data rows; 0 for workbook and tab container nodes.
    row_index: usize,
    /// Tab name for data rows; empty for workbook and tab container nodes.
    tab_name: String,
    /// The raw (pre-HTML) text value stored in `proto.document["text"]`, kept so
    /// `generate_source_bytes()` can write back the plain Markdown string (not the
    /// HTML-rendered form) when `inject_context` annotates link titles.
    raw_text: Option<String>,
}

/// `DocCodec` implementation for `.xlsx` and `.ods` spreadsheets.
///
/// Registered in `CODECS` for extensions `"xlsx"` and `"ods"`.
#[derive(Debug, Default)]
pub struct XlsxCodec {
    /// Absolute path to the source file. Populated during `parse()`.
    file_path: PathBuf,
    /// Parsed schema from the `index` tab. Populated during `parse()`.
    schema: Option<WorkbookSchema>,
    /// All nodes emitted during `parse()`, in emission order (workbook, then tab, then rows).
    nodes: Vec<ParsedNode>,
    /// BID annotations collected during `inject_context()` for write-back.
    row_bids: Vec<RowBid>,
    /// Set to `true` by `inject_context()` when any node's resolved state changed
    /// (e.g. a relation's target title was annotated). Causes `generate_source_bytes()`
    /// to re-emit the workbook with updated cell values even when no BID changed.
    updated: bool,
    /// Raw bytes of the source file, cached during `parse()` so
    /// `generate_source_bytes()` can skip a second `fs::read` call.
    file_bytes: Vec<u8>,
    /// Home network bref of the workbook node, captured during `inject_context()`
    /// from `ctx.home_net`. Used by `generate_html()` to populate `_net_bref` in
    /// the xlsx data block so xlsx-tabs.js can call `get_bid_from_id(net_bref, id)`.
    home_net_bref: String,
    /// Per-tab column maps built during `parse()`, keyed by actual sheet name.
    /// Used by `inject_context()` and `write_annotated_sheet()`.
    column_maps: HashMap<String, Vec<ColumnEntry>>,
    /// Resolved relation brefs collected during `inject_context()` for write-back
    /// into `__noet_relation_{ir_key}__` hidden columns.
    row_relations: Vec<RowRelation>,
}

impl XlsxCodec {
    pub fn new() -> Self {
        Self {
            column_maps: HashMap::new(),
            row_relations: Vec::new(),
            ..Self::default()
        }
    }

    /// Read raw bytes from a file path, returning a `BuildonomyError` on failure.
    fn read_file_bytes(path: &Path) -> Result<Vec<u8>, BuildonomyError> {
        std::fs::read(path).map_err(|e| {
            BuildonomyError::Codec(format!("XlsxCodec: failed to read {}: {e}", path.display()))
        })
    }

    /// Open a workbook from raw bytes using calamine and return it.
    fn open_workbook(bytes: Vec<u8>) -> Result<Xlsx<Cursor<Vec<u8>>>, BuildonomyError> {
        calamine::open_workbook_from_rs(Cursor::new(bytes))
            .map_err(|e| BuildonomyError::Codec(format!("XlsxCodec: failed to open workbook: {e}")))
    }

    /// Format a single calamine `Data` cell value as a plain string.
    /// Read the YAML schema from cell A1 of the `index` tab.
    ///
    /// Returns `None` if the `index` tab is absent (file treated as binary asset).
    /// Returns `Err` (with diagnostic) if the tab exists but A1 is not valid YAML.
    fn read_schema(
        workbook: &mut Xlsx<Cursor<Vec<u8>>>,
        path: &Path,
        diagnostics: &mut Vec<ParseDiagnostic>,
    ) -> Option<WorkbookSchema> {
        let sheet = match workbook.worksheet_range(INDEX_TAB) {
            Ok(s) => s,
            Err(_) => {
                // No index tab → treat as binary asset, no nodes emitted.
                tracing::debug!(
                    "XlsxCodec: no '{}' tab in {}, treating as asset",
                    INDEX_TAB,
                    path.display()
                );
                return None;
            }
        };

        let a1 = sheet.get((0, 0));
        let yaml_str = match a1 {
            Some(Data::String(s)) => s.clone(),
            Some(other) => other.to_string(),
            None => {
                diagnostics.push(ParseDiagnostic::warning(format!(
                    "{}: '{}' tab cell A1 is empty — no schema, treating as asset",
                    path.display(),
                    INDEX_TAB
                )));
                return None;
            }
        };

        // parse_with_fallback accepts YAML (primary), JSON, or TOML — making the
        // index tab schema format-agnostic. The resulting DocumentMut is then
        // deserialised into WorkbookSchema via the TOML string round-trip, the
        // same pattern used elsewhere in the codebase (e.g. traverse_schema).
        let doc = match parse_with_fallback(&yaml_str, MetadataFormat::Yaml) {
            Ok(d) => d,
            Err(e) => {
                diagnostics.push(ParseDiagnostic::parse_error(
                    format!(
                        "{}: could not parse '{}' tab cell A1 as YAML, JSON, or TOML: {e}",
                        path.display(),
                        INDEX_TAB
                    ),
                    0,
                ));
                return None;
            }
        };

        match toml::from_str::<WorkbookSchema>(&doc.to_string()) {
            Ok(schema) => Some(schema),
            Err(e) => {
                diagnostics.push(ParseDiagnostic::parse_error(
                    format!(
                        "{}: '{}' tab cell A1 parsed but does not match WorkbookSchema: {e}",
                        path.display(),
                        INDEX_TAB
                    ),
                    0,
                ));
                None
            }
        }
    }

    /// Emit a workbook-level (Document, heading=2) IRNode.
    ///
    /// The workbook is a Document node (heading=2), same as any `.md` file.
    /// It acts as a container for tab nodes (heading=3, Symbol) which in turn
    /// contain row nodes (heading=4, Symbol).
    fn make_workbook_node(path: &Path, schema: &WorkbookSchema) -> IRNode {
        let mut doc = DocumentMut::new();
        doc.insert("title", value(schema.title.clone()));
        if let Some(ref id) = schema.id {
            doc.insert("id", value(to_anchor(id)));
        }

        let mut kind = BeliefKindSet::default();
        kind.insert(BeliefKind::Document);

        IRNode {
            content: String::new(),
            document: doc,
            upstream: Vec::new(),
            downstream: Vec::new(),
            path: os_path_to_string(path),
            kind,
            errors: Vec::new(),
            // heading=2: workbook is a Document|Network container, same level as any
            // other document file (md.rs proto() also sets heading=2 for .md files).
            heading: 2,
            source_line: None,
            mappings: Vec::new(),
            accumulator: None,
            path_aliases: Vec::new(),
            namespace_paths: Vec::new(),
        }
    }

    /// Emit a tab container (Symbol, heading=3) IRNode.
    ///
    /// Tabs are section nodes (heading=3, Symbol) within the workbook document (heading=2).
    /// Row nodes are heading=4 sections within each tab.
    ///
    /// The tab `id` is prefixed with the workbook's own id (`workbook_prefix`) to prevent
    /// collisions with sibling network nodes or other corpus nodes that share the same
    /// name as a tab (e.g. a `power/` subdirectory network and a "Power" xlsx tab would
    /// both produce `id=power` without the prefix).
    ///
    /// `workbook_prefix` should be `workbook_proto.id().unwrap_or_default()`, computed
    /// after the workbook title and schema.id have been written into the proto document.
    /// `IRNode::id()` returns `Some` whenever `document["id"]` is set OR title is
    /// non-empty (falls back to `to_anchor(title)`), so the prefix is always available.
    fn make_tab_node(path: &Path, tab_name: &str, workbook_prefix: &str) -> IRNode {
        let mut doc = DocumentMut::new();
        doc.insert("title", value(tab_name));
        // Prefix the tab slug with the workbook id so the resulting NodeKey::Id
        // is namespaced under the workbook and cannot collide with siblings.
        let tab_slug = to_anchor(tab_name);
        if !tab_slug.is_empty() {
            let id = if workbook_prefix.is_empty() {
                tab_slug
            } else {
                format!("{}-{}", workbook_prefix, tab_slug)
            };
            doc.insert("id", value(id));
        }

        let mut kind = BeliefKindSet::default();
        kind.insert(BeliefKind::Symbol);

        IRNode {
            content: String::new(),
            document: doc,
            upstream: Vec::new(),
            downstream: Vec::new(),
            path: os_path_to_string(path),
            kind,
            errors: Vec::new(),
            heading: 3,
            source_line: None,
            mappings: Vec::new(),
            accumulator: None,
            path_aliases: Vec::new(),
            namespace_paths: Vec::new(),
        }
    }

    /// Interpolate a `text_template` string by replacing `{{ColumnName}}` placeholders
    /// with the corresponding cell values from `row_values`.
    ///
    /// Column names absent from `row_values` are replaced with an empty string silently —
    /// the template may reference optional columns. Cell values are substituted verbatim;
    /// the template string itself is what gets parsed as Markdown afterwards.
    fn interpolate_template(template: &str, row_values: &HashMap<String, String>) -> String {
        // Build a lowercase+underscore lookup so placeholder matching is
        // case-insensitive and space-tolerant.
        //
        // Template authors may write any of:
        //   {{Statement}}        — exact case, no spaces
        //   {{ statement }}      — spaces inside braces (Jinja-style)
        //   {{Source/Rationale}} — slashes preserved
        //
        // The lookup key is: trim whitespace, lowercase, replace spaces with "_".
        // The header key is normalised the same way at map-build time.
        let lower_values: HashMap<String, &String> = row_values
            .iter()
            .map(|(k, v)| (to_anchor(k.trim()), v))
            .collect();

        // Single-pass scanner: walk the template character by character, replace
        // every {{ ... }} placeholder via the normalised lookup, and emit non-
        // placeholder characters verbatim.  This supersedes the old two-pass
        // approach (literal replace + second-pass scanner) which failed to strip
        // spaces from inside {{ ... }} tokens.
        let mut out = String::with_capacity(template.len());
        let mut chars = template.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '{' && chars.peek() == Some(&'{') {
                chars.next(); // consume second '{'
                let mut key = String::new();
                let mut found_close = false;
                while let Some(inner) = chars.next() {
                    if inner == '}' && chars.peek() == Some(&'}') {
                        chars.next(); // consume second '}'
                        found_close = true;
                        break;
                    }
                    key.push(inner);
                }
                if found_close {
                    // Normalise via to_anchor so placeholder keys match header keys
                    // derived the same way (slashes, spaces, and other non-identifier
                    // characters all normalised consistently).
                    let lookup = to_anchor(key.trim());
                    if let Some(val) = lower_values.get(&lookup) {
                        out.push_str(val);
                    }
                    // Unrecognised placeholder — silently drop (empty substitution).
                }
                // Unclosed {{ — silently drop.
            } else {
                out.push(c);
            }
        }
        out
    }

    /// Return the result of rendering `template` with every column value replaced
    /// by an empty string.  Used to detect rows whose rendered text is meaningless
    /// (i.e. the template structure with no data filled in).
    fn empty_template_render(template: &str) -> String {
        // Build an all-empty row_values map — just needs the key set, values are "".
        // We pass an empty map; interpolate_template silently drops unrecognised
        // placeholders, so the output is the template with all {{ ... }} removed.
        Self::interpolate_template(template, &HashMap::new())
    }

    /// Parse a single schema-declared tab, emitting one row node per data row.
    ///
    /// Returns `(emitted_nodes, column_map)`. The column map is stored on `XlsxCodec`
    /// by the caller so `inject_context` and `write_annotated_sheet` can use it.
    fn parse_tab(
        path: &Path,
        workbook: &mut Xlsx<Cursor<Vec<u8>>>,
        tab_schema: &TabSchema,
        actual_sheet_name: &str,
        workbook_prefix: &str,
        tabs_meta: &std::collections::HashMap<String, crate::codec::xlsx::schema::TabBidEntry>,
        diagnostics: &mut Vec<ParseDiagnostic>,
    ) -> (Vec<ParsedNode>, Vec<ColumnEntry>) {
        let mut result = Vec::new();

        // Always emit the tab container node using the actual sheet name, not the
        // schema name. These differ when the wildcard schema (name: "*") is used.
        let mut tab_node = Self::make_tab_node(path, actual_sheet_name, workbook_prefix);
        // Inject pre-existing tab BID from schema.tabs_meta (written by prior --write pass).
        if let Some(entry) = tabs_meta.get(actual_sheet_name) {
            if let Some(ref tab_bid) = entry.bid {
                if tab_node.document.get("bid").is_none() {
                    tab_node.document.insert("bid", value(tab_bid.clone()));
                }
            }
        }
        result.push(ParsedNode {
            proto: tab_node,
            row_index: 0,
            tab_name: actual_sheet_name.to_string(),
            raw_text: None,
        });

        // Ignored tabs emit only the container node — no rows, no column map.
        if tab_schema.ignore {
            tracing::debug!(
                "{}: tab '{}' has ignore=true — skipping row parsing, treating as opaque",
                path.display(),
                actual_sheet_name,
            );
            return (result, Vec::new());
        }

        let sheet = match workbook.worksheet_range(actual_sheet_name) {
            Ok(s) => s,
            Err(e) => {
                diagnostics.push(ParseDiagnostic::warning(format!(
                    "{}: tab '{}' declared in schema but not found in workbook: {e}",
                    path.display(),
                    actual_sheet_name
                )));
                return (result, Vec::new());
            }
        };

        let rows: Vec<Vec<Data>> = sheet.rows().map(|r| r.to_vec()).collect();
        if rows.is_empty() {
            return (result, Vec::new());
        }

        // Row 0 is the header row.
        let header_row = &rows[0];

        // Build the column map from the header row and tab schema (single source of truth).
        let mut column_map =
            build_column_map(path, header_row, tab_schema, actual_sheet_name, diagnostics);

        // ── Row limit check ──────────────────────────────────────────────────
        let row_count = rows.len().saturating_sub(1); // exclude header
        if row_count > MAX_ROWS_PER_TAB {
            // Exceeds the u16::MAX architectural limit imposed by PathMap's sort-key
            // space. Emit only the container node — no row nodes.
            diagnostics.push(ParseDiagnostic::warning(format!(
                "{}: tab '{}' has {} data rows, exceeding the architectural limit of {} \
                 (u16::MAX) — no row nodes emitted.",
                path.display(),
                actual_sheet_name,
                row_count,
                MAX_ROWS_PER_TAB,
            )));
            return (result, column_map);
        }

        for (data_row_idx, data_row) in rows.iter().skip(1).take(MAX_ROWS_PER_TAB).enumerate() {
            // 1-based row number (row 1 = first data row).
            let row_number = data_row_idx + 1;

            // Fast skip: all spreadsheet cells are empty — nothing to emit.
            if data_row.iter().all(|c| c.is_empty()) {
                continue;
            }

            // Helper: read a cell by column index as a trimmed string.
            let cell_at = |idx: usize| -> String {
                data_row
                    .get(idx)
                    .map(|cell| match cell {
                        Data::String(s) => s.trim().to_string(),
                        Data::Float(f) => {
                            if f.fract() == 0.0 {
                                format!("{}", *f as i64)
                            } else {
                                format!("{f}")
                            }
                        }
                        Data::Int(i) => i.to_string(),
                        Data::Bool(b) => b.to_string(),
                        Data::DateTime(dt) => dt.to_string(),
                        Data::DateTimeIso(s) | Data::DurationIso(s) => s.clone(),
                        Data::Error(_) | Data::Empty => String::new(),
                    })
                    .unwrap_or_default()
            };

            // Build the IRNode document. All fields are written by the unified column
            // loop below, which iterates every ColumnEntry and writes doc[ir_key].
            let mut doc = DocumentMut::new();

            // ── Build row_values map for text_template interpolation ─────────
            // Collect every header→value pair regardless of role so the template
            // can reference any column by name.
            let row_values: HashMap<String, String> = header_row
                .iter()
                .enumerate()
                .filter_map(|(col_idx, cell)| {
                    let header = match cell {
                        Data::String(s) => s.trim().to_string(),
                        other => other.to_string(),
                    };
                    if header.is_empty() {
                        return None;
                    }
                    let val = cell_at(col_idx);
                    Some((header, val))
                })
                .collect();

            // ── Unified column loop ──────────────────────────────────────────
            // Every ColumnEntry is processed here. All doc fields are written via
            // ir_key — including system fields (bid, id, title, schema) — so there
            // is no pre-loop property extraction.
            //
            // ir_key uniqueness is guaranteed by build_column_map's post-construction
            // validation: any collision is diagnosed and demoted there, so each
            // ir_key maps to exactly one entry here.
            //
            // Special transformations per ir_key:
            //   "bid"   — validated as a UUID; invalid values are silently skipped.
            //   "id"    — passed through to_anchor() before storage.
            //   "title" — Title kind: value used directly; missing → auto-generated
            //             after the loop.
            //   all others — stored verbatim as a TOML string under doc[ir_key].
            let mut text_parts: Vec<String> = Vec::new();
            let mut upstream: Vec<IntermediateRelation> = Vec::new();
            let mut downstream: Vec<IntermediateRelation> = Vec::new();

            // Pre-build sibling lookup for RelationBref arms: ir_key → (weight, direction).
            // Must be built before iter_mut to avoid borrow conflicts inside the loop.
            let relation_sibling_map: std::collections::HashMap<
                String,
                (RelationWeight, RelationDirection),
            > = column_map
                .iter()
                .filter_map(|e| {
                    if let ColumnKind::Relation {
                        weight, direction, ..
                    } = e.kind
                    {
                        Some((e.ir_key.clone(), (weight, direction)))
                    } else {
                        None
                    }
                })
                .collect();

            for entry in column_map.iter_mut() {
                let val = cell_at(entry.col_idx);

                match entry.kind.clone() {
                    ColumnKind::RelationBref { relation_ir_key } => {
                        // Hidden bref column for a sibling Relation column.
                        // The bref in this cell was written by a prior --write pass.
                        // Push it as a NodeKey::Bref edge on the sibling column's
                        // direction — this gives stable resolution even if the
                        // human-readable cell text changes.
                        if !val.is_empty() {
                            let sibling = relation_sibling_map.get(&relation_ir_key).copied();
                            if let Some((weight, direction)) = sibling {
                                let formatted = format!("bref:{val}");
                                if let Ok(key) = NodeKey::from_str(&formatted) {
                                    let weight_kind = match weight {
                                        RelationWeight::Pragmatic => WeightKind::Pragmatic,
                                        RelationWeight::Epistemic => WeightKind::Epistemic,
                                    };
                                    let relation =
                                        IntermediateRelation::new(key, weight_kind, None);
                                    match direction {
                                        RelationDirection::Upstream => upstream.push(relation),
                                        RelationDirection::Downstream => downstream.push(relation),
                                    }
                                }
                            }
                        }
                    }
                    ColumnKind::Title => {
                        // Written into doc below after both passes; record hit.
                        if !val.is_empty() {
                            entry.hits += 1;
                            doc.insert("title", value(val));
                        }
                    }
                    ColumnKind::Markdown => {
                        if val.is_empty() {
                            continue;
                        }
                        entry.hits += 1;
                        // Extract markdown links from text cells as upstream relations.
                        let md_relations = parse_markdown_relations(
                            &val,
                            os_path_to_string(path).as_str(),
                            diagnostics,
                        );
                        upstream.extend(md_relations);
                        text_parts.push(val);
                    }
                    ColumnKind::Relation {
                        weight,
                        direction,
                        key_format,
                    } => {
                        if val.is_empty() {
                            continue;
                        }
                        entry.hits += 1;
                        // Store the raw cell text in doc[ir_key] so it appears as
                        // a normal payload field. generate_html reads it back and
                        // derives NodeKey strings ephemerally via key_format, so no
                        // separate companion "_text" key is needed.
                        if !entry.ir_key.is_empty() {
                            doc.insert(&entry.ir_key.clone(), value(val.clone()));
                        }
                        for ref_str in val.split(';') {
                            let r = ref_str.trim();
                            if r.is_empty() {
                                continue;
                            }
                            let formatted = key_format.format_value(r);
                            match NodeKey::from_str(&formatted) {
                                Ok(key) => {
                                    let weight_kind = match weight {
                                        RelationWeight::Pragmatic => WeightKind::Pragmatic,
                                        RelationWeight::Epistemic => WeightKind::Epistemic,
                                    };
                                    let relation =
                                        IntermediateRelation::new(key, weight_kind, None);
                                    match direction {
                                        RelationDirection::Upstream => upstream.push(relation),
                                        RelationDirection::Downstream => downstream.push(relation),
                                    }
                                }
                                Err(_) => {
                                    diagnostics.push(ParseDiagnostic::warning(format!(
                                        "{}: tab '{}' row {}: unresolvable relation '{}' \
                                             (formatted as '{}') in column '{}' — edge omitted",
                                        path.display(),
                                        actual_sheet_name,
                                        row_number,
                                        r,
                                        formatted,
                                        entry.header,
                                    )));
                                }
                            }
                        }
                    }
                    ColumnKind::Payload => {
                        if entry.ir_key.is_empty() {
                            continue;
                        }
                        if entry.ir_key == "bid" {
                            // Validate BID before storing — skip invalid values.
                            if val.is_empty() || Bid::try_from(val.as_str()).is_err() {
                                continue;
                            }
                        } else if entry.ir_key == "id" {
                            // Pass through to_anchor; skip if empty after anchoring.
                            if val.is_empty() {
                                continue;
                            }
                            let anchored = to_anchor(&val);
                            if anchored.is_empty() {
                                continue;
                            }
                            doc.insert("id", value(anchored));
                            if !entry.hidden {
                                entry.hits += 1;
                            }
                            continue;
                        } else if val.is_empty() {
                            continue;
                        }
                        doc.insert(&entry.ir_key.clone(), value(val));
                        if !entry.hidden {
                            entry.hits += 1;
                        }
                    }
                }
            }

            // ── Title fallback ───────────────────────────────────────────────
            // If no Title or hidden-title entry wrote a value, generate one.
            // The auto-generated form is never written to the sheet.
            if doc
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .is_empty()
            {
                diagnostics.push(ParseDiagnostic::warning(format!(
                    "{}: tab '{}' row {}: missing title value — using auto-generated title",
                    path.display(),
                    actual_sheet_name,
                    row_number,
                )));
                doc.insert(
                    "title",
                    value(format!("{}:{}", actual_sheet_name, row_number)),
                );
            }

            // ── Compose final text body ──────────────────────────────────────
            // When text_template is present, interpolate it and use the result as
            // the single Markdown body string, superseding individual text_parts.
            // When absent, join text_parts with "\n\n" (v1 behaviour).
            let raw_text: Option<String> = if let Some(ref tmpl) = tab_schema.text_template {
                let composed = Self::interpolate_template(tmpl, &row_values);
                // A rendered template is considered empty when:
                //   (a) the output is blank after trimming, OR
                //   (b) the output equals what the template produces with no data
                //       (i.e. all placeholders were present but all values were
                //       empty — the structural chrome of the template with nothing
                //       filled in).
                let empty_render = Self::empty_template_render(tmpl);
                let is_empty = composed.trim().is_empty() || composed.trim() == empty_render.trim();
                if is_empty {
                    None
                } else {
                    // Parse markdown relations from the composed template string too.
                    let tmpl_relations = parse_markdown_relations(
                        &composed,
                        os_path_to_string(path).as_str(),
                        diagnostics,
                    );
                    upstream.extend(tmpl_relations);
                    Some(composed)
                }
            } else if !text_parts.is_empty() {
                Some(text_parts.join("\n\n"))
            } else {
                None
            };

            // ── Semantic empty-row skip ──────────────────────────────────────
            // Skip rows where the rendered text body is empty (or equivalent to
            // the template rendered with no data).  This replaces the old
            // "all cells empty" fast-path for tabs with a text_template: even a
            // row with a non-empty title cell should be skipped when its
            // statement/rationale columns are blank and the template produces no
            // meaningful content.
            //
            // Rows without a text_template fall back to skipping when text_parts
            // is empty — consistent with the previous behaviour for non-template tabs.
            if raw_text.is_none() && tab_schema.text_template.is_some() {
                // Template tab with no meaningful content — skip entirely.
                // Warn when the row has a non-empty title or other detectable
                // content (relations, tags, non-empty payload cells) so the
                // author knows data was present but discarded.  Rows that are
                // truly blank (caught by the all-cells-empty fast-path above)
                // are silently skipped — no diagnostic needed there.
                let title_str = doc.get("title").and_then(|v| v.as_str()).unwrap_or("");
                let has_title = !title_str.is_empty();
                let has_relations = !upstream.is_empty() || !downstream.is_empty();
                // Any non-system doc key signals a non-empty payload cell.
                let has_payload = doc.iter().any(|(k, _)| {
                    !matches!(
                        k,
                        "title" | "bid" | "id" | "text" | "xlsx_tab" | "xlsx_row" | "schema"
                    )
                });
                if has_title || has_relations || has_payload {
                    diagnostics.push(ParseDiagnostic::warning(format!(
                        "{}: tab '{}' row {}: skipped — text_template rendered no content \
                         but row has data (title={:?}{}{}). \
                         Check that template placeholders match column headers exactly.",
                        path.display(),
                        actual_sheet_name,
                        row_number,
                        title_str,
                        if has_relations { ", has relations" } else { "" },
                        if has_payload { ", has payload" } else { "" },
                    )));
                }
                continue;
            }

            if let Some(ref text) = raw_text {
                doc.insert("text", value(text.clone()));
            }

            // Assign a stable explicit id when none was authored via __noet_id__.
            // Format: {workbook_prefix}-{tab_slug}-{row_number} (1-based).
            // This makes every row addressable by a deterministic semantic key,
            // enables cross-references like [[id:haven1-fsw-requirements-power-12]],
            // and prevents get_bid_from_id from matching unrelated nodes in the
            // same network that happen to share a similar title-derived slug.
            if doc
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .is_empty()
            {
                let tab_slug = to_anchor(actual_sheet_name);
                let row_id = if workbook_prefix.is_empty() {
                    format!("{tab_slug}-{row_number}")
                } else {
                    format!("{workbook_prefix}-{tab_slug}-{row_number}")
                };
                doc.insert("id", value(row_id));
            }

            // Provenance payload — use actual_sheet_name so wildcard tabs record the
            // real sheet name, not the "*" sentinel from the schema.
            doc.insert("xlsx_tab", value(actual_sheet_name));
            doc.insert("xlsx_row", value(row_number as i64));

            let mut kind = BeliefKindSet::default();
            kind.insert(BeliefKind::Symbol);

            let node = IRNode {
                content: String::new(),
                document: doc,
                upstream,
                downstream,
                path: os_path_to_string(path),
                kind,
                errors: Vec::new(),
                heading: 4,
                source_line: None,
                mappings: Vec::new(),
                accumulator: None,
                path_aliases: Vec::new(),
                namespace_paths: Vec::new(),
            };

            result.push(ParsedNode {
                proto: node,
                row_index: row_number,
                tab_name: actual_sheet_name.to_string(),
                raw_text,
            });
        }

        // Warn for any explicitly declared schema column that was never applied —
        // i.e. the column exists in the header row but every data row had an empty
        // cell for it. This catches schema typos and stale column names early.
        // Only explicit schema declarations (non-hidden, explicitly declared) are checked.
        let explicit_col_names: std::collections::HashSet<&str> =
            tab_schema.schema.iter().map(|c| c.col.as_str()).collect();
        for entry in &column_map {
            // Skip hidden (reserved) columns and title-role columns.
            if entry.hidden || matches!(entry.kind, ColumnKind::Title) {
                continue;
            }
            // Only warn for explicitly declared schema columns (not lazy defaults).
            if !explicit_col_names.contains(entry.header.as_str()) {
                continue;
            }
            if entry.hits == 0 {
                // Column was found in the header but never produced a value.
                diagnostics.push(ParseDiagnostic::warning(format!(
                    "{}: tab '{}': column '{}' (role: {:?}) is declared in the schema \
                     but was empty in every data row — check for a column rename or \
                     a stale schema entry",
                    path.display(),
                    actual_sheet_name,
                    entry.header,
                    entry.kind,
                )));
            }
        }

        (result, column_map)
    }

    /// All sheet names present in the workbook (excluding the `index` tab).
    fn all_sheet_names(workbook: &Xlsx<Cursor<Vec<u8>>>) -> Vec<String> {
        workbook
            .sheet_names()
            .iter()
            .filter(|n| n.as_str() != INDEX_TAB)
            .cloned()
            .collect()
    }

    /// Build the updated schema YAML string for cell A1, injecting current `bid` and
    /// `tabs_meta` values while preserving all other author-declared fields intact.
    ///
    /// Strategy: read the original A1 content and parse it as a `toml_edit::DocumentMut`,
    /// then insert/update only the `bid` and `tabs_meta` keys. Everything else (`title`,
    /// `id`, `tabs`, comments, ordering) is preserved as-is.
    fn build_updated_schema_yaml(&self, source_wb: &mut Xlsx<Cursor<Vec<u8>>>) -> String {
        let schema = match &self.schema {
            Some(s) => s,
            None => return String::new(),
        };

        // Read original A1 content.
        let original = source_wb
            .worksheet_range(INDEX_TAB)
            .ok()
            .and_then(|range| range.get((0, 0)).cloned())
            .map(|cell| match cell {
                Data::String(s) => s,
                other => other.to_string(),
            })
            .unwrap_or_default();

        // Parse the original YAML/TOML/JSON into a toml_edit DocumentMut so we can
        // surgically update only the BID fields while preserving everything else.
        let mut doc = match parse_with_fallback(&original, MetadataFormat::Yaml) {
            Ok(d) => d,
            Err(_) => DocumentMut::new(),
        };

        // Inject workbook BID.
        if let Some(ref bid) = schema.bid {
            doc.insert("bid", value(bid.clone()));
        }

        // Inject tabs_meta (tab container BIDs).
        if !schema.tabs_meta.is_empty() {
            // Retrieve or create the tabs_meta table.
            let mut tabs_meta_table = doc
                .get("tabs_meta")
                .and_then(|item| item.as_table().cloned())
                .unwrap_or_default();

            for (tab_name, entry) in &schema.tabs_meta {
                let mut entry_table = tabs_meta_table
                    .get(tab_name)
                    .and_then(|item| item.as_table().cloned())
                    .unwrap_or_default();

                if let Some(ref bid) = entry.bid {
                    entry_table.insert("bid", value(bid.clone()));
                }
                tabs_meta_table.insert(tab_name, toml_edit::Item::Table(entry_table));
            }
            doc.insert("tabs_meta", toml_edit::Item::Table(tabs_meta_table));
        }

        doc.to_string()
    }

    /// Copy the index tab verbatim into `out_wb`, replacing cell A1 with the
    /// updated schema YAML (injecting `bid` and `tabs_meta`).
    fn write_index_tab(
        &self,
        source_wb: &mut Xlsx<Cursor<Vec<u8>>>,
        out_wb: &mut rust_xlsxwriter::Workbook,
    ) {
        let ws = out_wb.add_worksheet();
        ws.set_name(INDEX_TAB).ok();
        let updated_yaml = self.build_updated_schema_yaml(source_wb);
        let _ = ws.write(0, 0, updated_yaml.as_str());
        // Copy any other cells in the index tab verbatim (row 0 col 1+, rows 1+).
        if let Ok(range) = source_wb.worksheet_range(INDEX_TAB) {
            for (row_idx, row) in range.rows().enumerate() {
                for (col_idx, cell) in row.iter().enumerate() {
                    if row_idx == 0 && col_idx == 0 {
                        continue; // already wrote A1 above
                    }
                    let _ = write_cell(ws, row_idx as u32, col_idx as u16, cell);
                }
            }
        }
    }

    /// Copy a non-schema sheet verbatim into a worksheet already added to `out_wb`.
    fn copy_verbatim_sheet(
        _sheet_name: &str,
        rows: &[Vec<Data>],
        ws: &mut rust_xlsxwriter::Worksheet,
    ) {
        for (row_idx, row) in rows.iter().enumerate() {
            for (col_idx, cell) in row.iter().enumerate() {
                let _ = write_cell(ws, row_idx as u32, col_idx as u16, cell);
            }
        }
    }

    /// Write an annotated schema-declared sheet into `ws`, injecting BID,
    /// id, and text write-back columns.
    ///
    /// `column_map` is the pre-built column map from `build_column_map`, stored on
    /// `XlsxCodec.column_maps` after `parse_tab`. When empty (tab not in column_maps),
    /// the function falls back to the header-row scan for backward compatibility.
    fn write_annotated_sheet(
        &self,
        sheet_name: &str,
        rows: &[Vec<Data>],
        column_map: &[ColumnEntry],
        ws: &mut rust_xlsxwriter::Worksheet,
    ) {
        let header_row = &rows[0];

        // ── Hidden column indices from column_map ────────────────────────────
        // All entries with hidden=true are reserved columns; collect their indices
        // so we can hide them after writing. The BID column (newly appended or
        // pre-existing) is added below.
        let mut reserved_col_indices: Vec<usize> = column_map
            .iter()
            .filter(|e| e.hidden)
            .map(|e| e.col_idx)
            .collect();

        // ── BID column location ──────────────────────────────────────────────
        // Derive from column_map first; fall back to header scan for sheets that
        // were not parsed through parse_tab (e.g. wildcard tabs whose map was not
        // stored — should not happen in practice after step 5, but guard anyway).
        let existing_bid_col = column_map
            .iter()
            .find(|e| e.ir_key == "bid" && e.hidden)
            .map(|e| e.col_idx)
            .or_else(|| {
                // Fallback: scan header for __noet_bid__.
                header_row.iter().enumerate().find_map(|(idx, cell)| {
                    let s = match cell {
                        Data::String(s) => s.trim().to_string(),
                        other => other.to_string(),
                    };
                    if s == BID_COLUMN_HEADER {
                        Some(idx)
                    } else {
                        None
                    }
                })
            });

        // Priority: source_col from RowBid (layer-1/2 column) → existing hidden bid
        // col → append new column.
        let row_source_col = self
            .row_bids
            .iter()
            .find(|rb| rb.tab == *sheet_name && rb.source_col.is_some())
            .and_then(|rb| rb.source_col);

        let bid_col_idx = row_source_col
            .or(existing_bid_col)
            .unwrap_or(header_row.len());

        let bid_col_is_new = row_source_col.is_none() && existing_bid_col.is_none();
        if bid_col_is_new {
            reserved_col_indices.push(bid_col_idx);
        }

        // ── Build data lookups ───────────────────────────────────────────────
        let bid_map: HashMap<usize, String> = self
            .row_bids
            .iter()
            .filter(|rb| rb.tab == *sheet_name)
            .map(|rb| (rb.row, rb.bid.to_string()))
            .collect();

        // id source column: derive from column_map (hidden entry with ir_key "id").
        let id_source_col = column_map
            .iter()
            .find(|e| e.ir_key == "id" && e.hidden)
            .map(|e| e.col_idx);

        let id_map: HashMap<usize, String> = if self.updated && id_source_col.is_some() {
            self.nodes
                .iter()
                .filter(|n| n.tab_name == *sheet_name && n.row_index > 0)
                .filter_map(|n| {
                    n.proto
                        .document
                        .get("id")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                        .map(|id_str| (n.row_index, id_str.to_string()))
                })
                .collect()
        } else {
            HashMap::new()
        };

        let id_col_idx = id_source_col.unwrap_or(usize::MAX); // sentinel: no write

        let text_map: HashMap<usize, String> = if self.updated {
            self.nodes
                .iter()
                .filter(|n| n.tab_name == *sheet_name && n.row_index > 0)
                .filter_map(|n| n.raw_text.as_ref().map(|t| (n.row_index, t.clone())))
                .collect()
        } else {
            HashMap::new()
        };

        // text_col_indices: derive from column_map (non-hidden Text entries).
        // When a text_template is active, no individual text columns are rewritten
        // (the text is a composition of multiple columns). Detect this by checking
        // whether there are Text entries in the map that come from the schema — if
        // none exist (only conventional/positional Text), rewriting is still correct.
        // The simplest signal: if there is exactly one Text entry and it has
        // hits > 0 (or we don't know), rewrite it; otherwise skip.
        // Conservative: collect all non-hidden Text column indices from column_map.
        // The text_template case is handled by the caller not populating text_map.
        let text_col_indices: Vec<usize> = column_map
            .iter()
            .filter(|e| matches!(e.kind, ColumnKind::Markdown) && !e.hidden)
            .map(|e| e.col_idx)
            .collect();

        // ── Write header row ─────────────────────────────────────────────────
        for (col_idx, cell) in header_row.iter().enumerate() {
            let _ = write_cell(ws, 0, col_idx as u16, cell);
        }
        if bid_col_is_new {
            let _ = ws.write(0, bid_col_idx as u16, BID_COLUMN_HEADER);
        }

        // ── Relation-bref column write-back ─────────────────────────────────
        // For each Relation column in the map, find or append the corresponding
        // __noet_relation_{ir_key}__ hidden column and collect its write targets.
        //
        // Structure: relation_bref_cols maps col_idx → (ir_key, row→bref lookup).
        let mut relation_bref_cols: Vec<(usize, String, HashMap<usize, String>)> = Vec::new();
        {
            // Collect Relation columns from the column map.
            let relation_entries: Vec<(String,)> = column_map
                .iter()
                .filter_map(|e| {
                    if matches!(e.kind, ColumnKind::Relation { .. }) {
                        Some((e.ir_key.clone(),))
                    } else {
                        None
                    }
                })
                .collect();

            for (ir_key,) in relation_entries {
                let bref_header = format!("{RELATION_BREF_COLUMN_PREFIX}{ir_key}__");

                // Check for existing __noet_relation_{ir_key}__ in column_map or header.
                let existing_col = column_map
                    .iter()
                    .find(|e| {
                        matches!(&e.kind, ColumnKind::RelationBref { relation_ir_key }
                            if *relation_ir_key == ir_key)
                    })
                    .map(|e| e.col_idx)
                    .or_else(|| {
                        header_row.iter().enumerate().find_map(|(idx, cell)| {
                            let s = match cell {
                                Data::String(s) => s.trim().to_string(),
                                other => other.to_string(),
                            };
                            if s == bref_header {
                                Some(idx)
                            } else {
                                None
                            }
                        })
                    });

                let col_idx = existing_col.unwrap_or(header_row.len() + relation_bref_cols.len());

                // Build row→bref lookup from row_relations.
                let bref_map: HashMap<usize, String> = self
                    .row_relations
                    .iter()
                    .filter(|rr| rr.tab == *sheet_name && rr.col == ir_key)
                    .map(|rr| (rr.row, rr.bref.clone()))
                    .collect();

                if !bref_map.is_empty() || existing_col.is_none() {
                    // Write header for new columns.
                    if existing_col.is_none() {
                        let _ = ws.write(0, col_idx as u16, bref_header.as_str());
                        reserved_col_indices.push(col_idx);
                    }
                    relation_bref_cols.push((col_idx, ir_key, bref_map));
                }
            }
        }

        // ── Write data rows ──────────────────────────────────────────────────
        let mut logical_data_row: usize = 0;
        for (sheet_row_idx, row) in rows.iter().enumerate().skip(1) {
            // Skip blank rows as in parse().
            if row.iter().all(|c| c.is_empty()) {
                for (col_idx, cell) in row.iter().enumerate() {
                    let _ = write_cell(ws, sheet_row_idx as u32, col_idx as u16, cell);
                }
                continue;
            }

            logical_data_row += 1;

            for (col_idx, cell) in row.iter().enumerate() {
                if col_idx == bid_col_idx {
                    continue; // written below
                }
                if self.updated && id_source_col == Some(col_idx) {
                    continue; // written below
                }
                // Skip existing relation-bref columns — written below.
                if relation_bref_cols.iter().any(|(rc, _, _)| *rc == col_idx) {
                    continue;
                }
                if self.updated && text_col_indices.contains(&col_idx) {
                    if let Some(updated_text) = text_map.get(&logical_data_row) {
                        if text_col_indices.first() == Some(&col_idx) {
                            let _ = ws.write(
                                sheet_row_idx as u32,
                                col_idx as u16,
                                updated_text.as_str(),
                            );
                        } else {
                            // Secondary text columns: leave blank after composition.
                            let _ = ws.write(sheet_row_idx as u32, col_idx as u16, "");
                        }
                        continue;
                    }
                }
                let _ = write_cell(ws, sheet_row_idx as u32, col_idx as u16, cell);
            }

            if let Some(bid_str) = bid_map.get(&logical_data_row) {
                let _ = ws.write(sheet_row_idx as u32, bid_col_idx as u16, bid_str.as_str());
            }

            // Write resolved relation brefs into __noet_relation_{ir_key}__ columns.
            for (rel_col_idx, _ir_key, bref_map) in &relation_bref_cols {
                if let Some(bref_str) = bref_map.get(&logical_data_row) {
                    let _ = ws.write(sheet_row_idx as u32, *rel_col_idx as u16, bref_str.as_str());
                }
            }

            if self.updated && id_source_col.is_some() {
                if let Some(id_str) = id_map.get(&logical_data_row) {
                    let _ = ws.write(sheet_row_idx as u32, id_col_idx as u16, id_str.as_str());
                }
            }
        }

        // Hide all reserved columns.
        for &col_idx in &reserved_col_indices {
            let _ = ws.set_column_hidden(col_idx as u16);
        }
    }
}

/// Build a `Vec<ColumnEntry>` from a header row and tab schema.
///
/// This is the single source of truth for column role assignment. It replaces the
/// old three-pass scan that mutated `effective_schema`, `reserved_col_map`, `col_index`,
/// and `explicit_col_hits` inside `parse_tab`.
///
/// Pass order:
///  1. Reserved columns (`__noet_*__`) → `kind = Payload`, `hidden = true`.
///  2. Explicit schema declarations → `kind` from `ColumnRole` + relation parameters.
///  3. Layer-2 conventional name matching (`CONVENTIONAL_ROLES` + "Schema" header).
///  4. Positional title fallback (only when no conventional title exists anywhere).
///  5. Remaining unlisted columns → `kind = Payload`, `ir_key = normalized(header)`.
///
/// All entries get `hits = 0`.
fn build_column_map(
    path: &Path,
    header_row: &[Data],
    tab_schema: &TabSchema,
    actual_sheet_name: &str,
    diagnostics: &mut Vec<ParseDiagnostic>,
) -> Vec<ColumnEntry> {
    let mut entries: Vec<ColumnEntry> = Vec::new();
    // Track which column indices have been assigned to avoid double-assignment.
    let mut assigned: std::collections::HashSet<usize> = std::collections::HashSet::new();

    // Helper: extract the trimmed string from a header cell.
    let header_str = |cell: &Data| -> String {
        match cell {
            Data::String(s) => s.trim().to_string(),
            other => other.to_string(),
        }
    };

    // ── Pass 1: Reserved columns (`__noet_*__`) ──────────────────────────────
    for (idx, cell) in header_row.iter().enumerate() {
        let h = header_str(cell);
        if !ReservedColumnKind::has_reserved_prefix(&h) {
            continue;
        }

        // Detect relation-bref columns before the standard ReservedColumnKind lookup.
        // Format: __noet_relation_{ir_key}__ — produced by write_annotated_sheet.
        if h.starts_with(RELATION_BREF_COLUMN_PREFIX) && h.ends_with("__") {
            let relation_ir_key = h[RELATION_BREF_COLUMN_PREFIX.len()..h.len() - 2].to_string();
            if !relation_ir_key.is_empty() {
                entries.push(ColumnEntry {
                    col_idx: idx,
                    header: h,
                    ir_key: format!("{RELATION_BREF_COLUMN_PREFIX}{relation_ir_key}__"),
                    kind: ColumnKind::RelationBref { relation_ir_key },
                    hidden: true,
                    hits: 0,
                });
                assigned.insert(idx);
                continue;
            }
        }

        match ReservedColumnKind::from_header(&h) {
            Some(kind) => {
                let (ir_key, col_kind) = match kind {
                    ReservedColumnKind::Bid => ("bid".to_string(), ColumnKind::Payload),
                    ReservedColumnKind::Id => ("id".to_string(), ColumnKind::Payload),
                    ReservedColumnKind::Schema => ("schema".to_string(), ColumnKind::Payload),
                    ReservedColumnKind::Title => ("title".to_string(), ColumnKind::Payload),
                    ReservedColumnKind::Kind => ("kind".to_string(), ColumnKind::Payload),
                };
                entries.push(ColumnEntry {
                    col_idx: idx,
                    header: h,
                    ir_key,
                    kind: col_kind,
                    hidden: true,
                    hits: 0,
                });
                assigned.insert(idx);
            }
            None => {
                diagnostics.push(ParseDiagnostic::warning(format!(
                    "{}: tab '{}': unrecognised reserved column '{}' — treating as payload",
                    path.display(),
                    actual_sheet_name,
                    h
                )));
                // Still assign it so it doesn't get processed again as a lazy default.
                let ir_key = to_anchor(&h);
                entries.push(ColumnEntry {
                    col_idx: idx,
                    header: h,
                    ir_key,
                    kind: ColumnKind::Payload,
                    hidden: true,
                    hits: 0,
                });
                assigned.insert(idx);
            }
        }
    }

    // ── Pass 2: Explicit schema declarations ─────────────────────────────────
    let explicit_col_names: std::collections::HashSet<&str> =
        tab_schema.schema.iter().map(|c| c.col.as_str()).collect();
    for col_schema in &tab_schema.schema {
        // Find the column index for this schema entry.
        let found = header_row.iter().enumerate().find(|(_, cell)| {
            let cell_str = header_str(cell);
            cell_str == col_schema.col.trim()
        });
        let idx = match found {
            Some((idx, _)) => idx,
            None => {
                diagnostics.push(ParseDiagnostic::warning(format!(
                    "{}: tab '{}': declared column '{}' not found in header row — skipping",
                    path.display(),
                    actual_sheet_name,
                    col_schema.col
                )));
                continue;
            }
        };
        if assigned.contains(&idx) {
            // Reserved column wins; explicit declaration is silently skipped.
            continue;
        }
        let kind = match col_schema.role {
            ColumnRole::Title => ColumnKind::Title,
            ColumnRole::Text => ColumnKind::Markdown,
            ColumnRole::Relation => ColumnKind::Relation {
                weight: col_schema.weight,
                direction: col_schema.direction,
                key_format: col_schema.key,
            },
            ColumnRole::Payload => ColumnKind::Payload,
        };
        let ir_key = match col_schema.role {
            ColumnRole::Title => "title".to_string(),
            ColumnRole::Text => "text".to_string(),
            _ => to_anchor(&col_schema.col),
        };
        entries.push(ColumnEntry {
            col_idx: idx,
            header: col_schema.col.clone(),
            ir_key,
            kind,
            hidden: false,
            hits: 0,
        });
        assigned.insert(idx);
    }

    // ── Pre-scan: conventional title detection ───────────────────────────────
    // Check whether any unlisted non-reserved column has a "title" header (layer 2).
    // When true, the positional fallback must NOT fire — even for columns that appear
    // before "Title" in left-to-right order.
    let has_explicit_title = tab_schema
        .schema
        .iter()
        .any(|c| c.role == ColumnRole::Title);
    let has_conventional_title = !has_explicit_title
        && header_row.iter().enumerate().any(|(col_idx, cell)| {
            if assigned.contains(&col_idx) {
                return false;
            }
            let h = header_str(cell);
            if explicit_col_names.contains(h.as_str()) {
                return false;
            }
            h.eq_ignore_ascii_case("title")
        });

    // Pre-scan: detect a column conventionally named "Schema" (case-insensitive).
    // This provides layer-2 schema injection without requiring __noet_schema__.
    // Only active when no __noet_schema__ reserved column is already present.
    let has_reserved_schema = entries.iter().any(|e| e.ir_key == "schema" && e.hidden);
    let conventional_schema_col_idx: Option<usize> = if !has_reserved_schema {
        header_row.iter().enumerate().find_map(|(idx, cell)| {
            if assigned.contains(&idx) {
                return None;
            }
            let h = header_str(cell);
            if explicit_col_names.contains(h.as_str()) {
                return None;
            }
            if h.eq_ignore_ascii_case("schema") {
                Some(idx)
            } else {
                None
            }
        })
    } else {
        None
    };

    // ── Pass 3 + 4 + 5: Unlisted non-reserved columns ───────────────────────
    let mut title_assigned =
        has_explicit_title || entries.iter().any(|e| matches!(e.kind, ColumnKind::Title));

    for (col_idx, cell) in header_row.iter().enumerate() {
        if assigned.contains(&col_idx) {
            continue;
        }
        let h = header_str(cell);
        if h.is_empty() {
            continue;
        }
        if explicit_col_names.contains(h.as_str()) {
            // Layer 1 column that didn't have an index entry (already warned above).
            continue;
        }

        // The conventional "Schema" column is injected via conventional_schema_col_idx,
        // not as a payload entry — skip it here to avoid double-assignment.
        if conventional_schema_col_idx == Some(col_idx) {
            // Emit it as a non-hidden Payload with ir_key="schema" so the row loop
            // can read it via conventional_schema_col_idx matching.
            entries.push(ColumnEntry {
                col_idx,
                header: h,
                ir_key: "schema".to_string(),
                kind: ColumnKind::Payload,
                hidden: false,
                hits: 0,
            });
            assigned.insert(col_idx);
            continue;
        }

        // Layer 2: case-insensitive conventional name match.
        let h_lower = h.to_ascii_lowercase();
        let conventional = CONVENTIONAL_ROLES
            .iter()
            .find(|(name, _)| *name == h_lower)
            .map(|(_, role)| *role);

        if let Some(role) = conventional {
            if role == ColumnRole::Title {
                if title_assigned {
                    // Duplicate title — demote to payload.
                    entries.push(ColumnEntry {
                        col_idx,
                        header: h,
                        ir_key: to_anchor(&h_lower),
                        kind: ColumnKind::Payload,
                        hidden: false,
                        hits: 0,
                    });
                    assigned.insert(col_idx);
                    continue;
                }
                title_assigned = true;
                entries.push(ColumnEntry {
                    col_idx,
                    header: h,
                    ir_key: "title".to_string(),
                    kind: ColumnKind::Title,
                    hidden: false,
                    hits: 0,
                });
            } else {
                // Text or other conventional role.
                let ir_key = match role {
                    ColumnRole::Text => "text".to_string(),
                    _ => to_anchor(&h_lower),
                };
                entries.push(ColumnEntry {
                    col_idx,
                    header: h,
                    ir_key,
                    kind: ColumnKind::Markdown,
                    hidden: false,
                    hits: 0,
                });
            }
            assigned.insert(col_idx);
        } else {
            // Layer 3 / positional fallback.
            if !has_conventional_title && !title_assigned {
                title_assigned = true;
                entries.push(ColumnEntry {
                    col_idx,
                    header: h,
                    ir_key: "title".to_string(),
                    kind: ColumnKind::Title,
                    hidden: false,
                    hits: 0,
                });
            } else {
                let mut ir_key = to_anchor(&h_lower);
                // Prevent implicit columns from shadowing reserved ir_keys
                // that receive special handling in the row loop (L769-787).
                // Only __noet_*__ (Pass 1) and explicit schema entries (Pass 2)
                // should drive the node's bid/id — implicit header-name matches
                // must go to regular payload so the mechanical default ID fires.
                const RESERVED_ROW_IR_KEYS: &[&str] = &["bid", "id"];
                if RESERVED_ROW_IR_KEYS.contains(&ir_key.as_str()) {
                    ir_key = format!("{ir_key}_col");
                }
                entries.push(ColumnEntry {
                    col_idx,
                    header: h,
                    ir_key,
                    kind: ColumnKind::Payload,
                    hidden: false,
                    hits: 0,
                });
            }
            assigned.insert(col_idx);
        }
    }

    if !title_assigned {
        diagnostics.push(ParseDiagnostic::warning(format!(
            "{}: tab '{}': no title column determinable (all columns reserved or header empty) \
             — rows will use auto-generated titles",
            path.display(),
            actual_sheet_name,
        )));
    }

    // Sort entries by col_idx for deterministic iteration order before validation.
    entries.sort_by_key(|e| e.col_idx);

    // ── Post-construction ir_key collision validation ─────────────────────────
    // Each ir_key must be unique across all entries. Collisions indicate a bug in
    // build_column_map — not something to silently paper over in the row loop.
    // Resolution: non-hidden entry wins over hidden; among same hidden level,
    // lower col_idx wins. The loser is demoted to a unique payload key
    // "{header_normalized}_{col_idx}" and diagnosed so the author can investigate.
    //
    // We scan in col_idx order (already sorted above). For each ir_key seen,
    // the first entry claiming it is the winner; subsequent entries are demoted.
    {
        let mut seen_ir_keys: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        for entry in entries.iter_mut() {
            let key = entry.ir_key.clone();
            if let Some(winner_col) = seen_ir_keys.get(&key) {
                // Collision: this entry loses. Demote to a unique payload key.
                let demoted_key = format!("{}_{}", to_anchor(&entry.header), entry.col_idx);
                diagnostics.push(ParseDiagnostic::warning(format!(
                    "{}: tab '{}': column '{}' (col {}) has the same ir_key '{}' as column {} \
                     — build_column_map assigned duplicate ir_keys, which is a bug. \
                     Demoting to '{}'. Please report this.",
                    path.display(),
                    actual_sheet_name,
                    entry.header,
                    entry.col_idx,
                    key,
                    winner_col,
                    demoted_key,
                )));
                entry.ir_key = demoted_key.clone();
                entry.kind = ColumnKind::Payload;
                seen_ir_keys.insert(demoted_key, entry.col_idx);
            } else {
                seen_ir_keys.insert(key, entry.col_idx);
            }
        }
    }

    entries
}

/// Return type for [`DocCodec::generate_html`] — filename, named placeholder pairs, optional layout.
type HtmlFragments = Vec<(
    String,
    Vec<(String, String)>,
    Option<crate::codec::assets::Layout>,
)>;

impl DocCodec for XlsxCodec {
    fn content_mode(&self) -> CodecContentMode {
        CodecContentMode::Binary
    }

    fn proto(&self, path: &Path) -> Result<Option<IRNode>, BuildonomyError> {
        if path.is_relative() {
            return Err(BuildonomyError::Codec(format!(
                "XlsxCodec::proto: path must be absolute, got {:?}",
                path
            )));
        }

        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if ext != "xlsx" && ext != "ods" {
            return Ok(None);
        }

        let bytes = match Self::read_file_bytes(path) {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!("XlsxCodec::proto: {e}");
                return Ok(None);
            }
        };
        let mut workbook = match Self::open_workbook(bytes) {
            Ok(wb) => wb,
            Err(e) => {
                tracing::warn!("XlsxCodec::proto: {e}");
                return Ok(None);
            }
        };

        let mut diagnostics = Vec::new();
        let schema = Self::read_schema(&mut workbook, path, &mut diagnostics);

        // Diagnostics from proto() are lost (no channel here); log them.
        for d in &diagnostics {
            tracing::warn!("XlsxCodec::proto diagnostic: {:?}", d);
        }

        match schema {
            None => Ok(None),
            Some(s) => Ok(Some(Self::make_workbook_node(path, &s))),
        }
    }

    fn parse(
        &mut self,
        // Binary codec: `content` is an empty string, ignored.
        _content: &str,
        current: IRNode,
        diagnostics: &mut Vec<ParseDiagnostic>,
        _proto_index: &crate::codec::proto_index::ProtoIndex,
    ) -> Result<(), BuildonomyError> {
        self.nodes.clear();
        self.row_bids.clear();
        self.schema = None;

        let path = PathBuf::from(&current.path);
        self.file_path = path.clone();

        let bytes = Self::read_file_bytes(&path)?;
        let mut workbook = Self::open_workbook(bytes.clone())?;
        self.file_bytes = bytes;

        let schema = match Self::read_schema(&mut workbook, &path, diagnostics) {
            None => {
                // No schema → emit workbook node only (opaque asset-like behaviour).
                self.nodes.push(ParsedNode {
                    proto: current,
                    row_index: 0,
                    tab_name: String::new(),
                    raw_text: None,
                });
                return Ok(());
            }
            Some(s) => s,
        };

        // Workbook (Document) node — use the `current` proto seeded by GraphBuilder
        // (which already has path, kind bits, etc.) but overwrite title/id from schema.
        let mut workbook_proto = current;
        workbook_proto
            .document
            .insert("title", value(schema.title.clone()));
        if let Some(ref id) = schema.id {
            if workbook_proto.document.get("id").is_none() {
                workbook_proto.document.insert("id", value(to_anchor(id)));
            }
        }
        // Inject pre-existing workbook BID from schema (written by prior --write pass).
        if let Some(ref wb_bid) = schema.bid {
            if workbook_proto.document.get("bid").is_none() {
                workbook_proto.document.insert("bid", value(wb_bid.clone()));
            }
        }

        // Compute the workbook id prefix for tab node ids.
        // IRNode::id() returns Some whenever document["id"] is set OR title is non-empty
        // (falls back to to_anchor(title)), so this is always available at this point.
        let workbook_prefix = workbook_proto.id().unwrap_or_default();

        self.nodes.push(ParsedNode {
            proto: workbook_proto,
            row_index: 0,
            tab_name: String::new(),
            raw_text: None,
        });

        // Collect the set of declared tab names for lookup.
        // Wildcard entry (name: "*") provides a default schema for unmatched tabs.
        // It is excluded from the declared_tab_names set so unmatched tabs are
        // dispatched to it rather than treated as opaque.
        let wildcard_schema: Option<&TabSchema> =
            schema.tabs.iter().find(|t| t.name == WILDCARD_TAB);

        let declared_tab_names: std::collections::HashSet<&str> = schema
            .tabs
            .iter()
            .filter(|t| t.name != WILDCARD_TAB)
            .map(|t| t.name.as_str())
            .collect();

        // Emit all workbook tabs (excluding the index tab) in workbook sheet order.
        let all_sheets = Self::all_sheet_names(&workbook);

        for sheet_name in &all_sheets {
            // Lookup order: exact name match → wildcard → opaque.
            let tab_schema_opt = schema
                .tabs
                .iter()
                .find(|t| t.name == *sheet_name && t.name != WILDCARD_TAB)
                .or(wildcard_schema);

            // All tabs — whether schema-declared, wildcard-matched, ignored, or absent
            // from the schema — are processed through parse_tab. parse_tab always emits
            // the container node and handles ignore, BID injection, and overflow internally.
            // Tabs absent from the schema use a synthetic empty TabSchema so parse_tab
            // emits only the container node (no columns → no rows).
            let effective_schema: &TabSchema;
            let absent_schema;
            if let Some(tab_schema) = tab_schema_opt {
                effective_schema = tab_schema;
            } else if !declared_tab_names.contains(sheet_name.as_str()) && wildcard_schema.is_none()
            {
                // Tab absent from schema entirely: use a bare schema so parse_tab
                // emits only the container node.
                absent_schema = TabSchema {
                    name: sheet_name.clone(),
                    ignore: true,
                    text_template: None,
                    schema: Vec::new(),
                };
                effective_schema = &absent_schema;
            } else {
                continue;
            }

            let (parsed, column_map) = Self::parse_tab(
                &path,
                &mut workbook,
                effective_schema,
                sheet_name,
                &workbook_prefix,
                &schema.tabs_meta,
                diagnostics,
            );
            self.column_maps.insert(sheet_name.clone(), column_map);
            self.nodes.extend(parsed);
        }

        self.schema = Some(schema);
        self.updated = false; // reset; inject_context sets this when state changes
                              // Annotate all ParsedNodes that have no raw_text but were opaque/container nodes
                              // (raw_text is already set for row nodes above).
        Ok(())
    }

    fn nodes(&self) -> Vec<IRNode> {
        self.nodes.iter().map(|n| n.proto.clone()).collect()
    }

    fn set_node_bid(&mut self, proto_idx: usize, bid: Bid) {
        if let Some(node) = self.nodes.get_mut(proto_idx) {
            if node.proto.document.get("bid").is_none() {
                node.proto.document.insert("bid", value(bid.to_string()));
            }
        }
    }

    fn inject_context(
        &mut self,
        proto_idx: usize,
        node: &IRNode,
        ctx: &BeliefContext<'_>,
        _diagnostics: &mut Vec<ParseDiagnostic>,
    ) -> Result<Option<BeliefNode>, BuildonomyError> {
        let nodes_len = self.nodes.len();
        let parsed_node = self.nodes.get_mut(proto_idx).ok_or_else(|| {
            BuildonomyError::Codec(format!(
                "XlsxCodec::inject_context: proto_idx {proto_idx} out of range (len={nodes_len})",
            ))
        })?;
        debug_assert_eq!(
            &parsed_node.proto, node,
            "XlsxCodec::inject_context: proto_idx {proto_idx} does not match expected node"
        );

        // Capture home_net for the workbook node (row_index == 0, empty tab_name).
        // ctx.home_net is the authoritative home network BID — use its bref string.
        if parsed_node.row_index == 0 && parsed_node.tab_name.is_empty() {
            self.home_net_bref = ctx.home_net.bref().to_string();
        }

        let updated_node = parsed_node.proto.update_from_context(ctx)?;

        // Sync doc["id"] from ctx.node.id for row nodes.
        // merge_from_belief_node skips id write-back when proto already has an id
        // (to avoid clobbering user-authored ids). But for row nodes we always set
        // a positional id at parse time, so merge_from_belief_node never updates it
        // — even if GraphBuilder::push resolved a collision and assigned a different id.
        // We bypass that guard here so generate_html always reads the canonical id.
        if parsed_node.row_index > 0 {
            if let NodeId::Explicit(ref canonical_id) = ctx.node.id {
                if !canonical_id.is_empty() {
                    let proto_id = parsed_node
                        .proto
                        .document
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if proto_id != canonical_id.as_str() {
                        parsed_node
                            .proto
                            .document
                            .insert("id", value(canonical_id.clone()));
                    }
                }
            }
        }

        // For data rows with relation columns: populate doc["{ir_key}_bids"] with
        // the semicolon-joined resolved BIDs from ctx.sinks(), and collect
        // RowRelation entries for write-back to __noet_relation_{ir_key}__ columns.
        if parsed_node.row_index > 0 && !parsed_node.tab_name.is_empty() {
            if let Some(col_map) = self.column_maps.get(&parsed_node.tab_name) {
                // Pre-collect sinks once — ctx.sinks() is O(n) over the graph.
                let sinks: Vec<_> = ctx.sinks();

                // Build lookup: NodeKey string → (resolved BID, resolved bref).
                // Pair proto.upstream edges with sinks() in emission order — the
                // graph preserves push_relation order.
                let sink_info_by_key: std::collections::HashMap<String, (String, String)> = {
                    let mut m = std::collections::HashMap::new();
                    for (rel, sink_rel) in parsed_node.proto.upstream.iter().zip(sinks.iter()) {
                        let key_str = rel.key.to_string();
                        let bid_str = sink_rel.other.bid.to_string();
                        let bref_str = sink_rel.other.bid.bref().to_string();
                        m.insert(key_str, (bid_str, bref_str));
                    }
                    m
                };

                let col_map_snapshot: Vec<(String, crate::codec::xlsx::schema::RelationKeyFormat)> =
                    col_map
                        .iter()
                        .filter_map(|e| {
                            if let ColumnKind::Relation { key_format, .. } = e.kind {
                                Some((e.ir_key.clone(), key_format))
                            } else {
                                None
                            }
                        })
                        .collect();

                for (ir_key, key_format) in col_map_snapshot {
                    let raw = parsed_node
                        .proto
                        .document
                        .get(ir_key.as_str())
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    if raw.is_empty() {
                        continue;
                    }

                    let mut bids: Vec<String> = Vec::new();
                    let mut brefs: Vec<String> = Vec::new();

                    for r in raw.split(';') {
                        let r = r.trim();
                        if r.is_empty() {
                            bids.push(String::new());
                            brefs.push(String::new());
                            continue;
                        }
                        let formatted = key_format.format_value(r);
                        if let Some((bid, bref)) = sink_info_by_key.get(&formatted) {
                            bids.push(bid.clone());
                            brefs.push(bref.clone());
                        } else {
                            bids.push(String::new());
                            brefs.push(String::new());
                        }
                    }

                    // Write _bids into doc for generate_html.
                    if bids.iter().any(|b| !b.is_empty()) {
                        let bids_key = format!("{ir_key}_bids");
                        parsed_node
                            .proto
                            .document
                            .insert(&bids_key, value(bids.join(";")));
                    }

                    // Collect RowRelation entries for __noet_relation_{ir_key}__ write-back.
                    // One entry per resolved bref (semicolon-joined for multi-value cells).
                    let joined_brefs = brefs.join(";");
                    if brefs.iter().any(|b| !b.is_empty()) {
                        self.row_relations.push(RowRelation {
                            tab: parsed_node.tab_name.clone(),
                            row: parsed_node.row_index,
                            col: ir_key.clone(),
                            bref: joined_brefs,
                        });
                    }
                }
            }
        }

        // Phase 4: detect whether any upstream relation's title was resolved for the
        // first time. `update_from_context` propagates bid/title/id/kind changes; the
        // relation weight titles are resolved by `push_relation` in `GraphBuilder` and
        // land in `ctx.node`'s weight payloads after inject_context is called. We check
        // whether the resolved text body now differs from what we stored at parse time.
        //
        // Specifically: if any upstream relation on this node now carries a weight["title"]
        // that was absent at parse time (i.e. the bref resolved to a real node title),
        // the text payload may need updating. We mark updated=true so generate_source_bytes
        // runs; the actual cell value written is the raw_text (which already contains the
        // markdown source with bref links). A future pass can rewrite link text in-line.
        if updated_node.is_some() {
            self.updated = true;
        }

        // Record the resolved BID for data rows so generate_source_bytes() can annotate.
        // update_from_context writes the bid into proto.document["bid"] when changed,
        // so we read it back from there rather than from ctx.node directly, to pick up
        // any BID that was already present from a prior write-back round.
        if parsed_node.row_index > 0 && !parsed_node.tab_name.is_empty() {
            if let Some(bid_val) = parsed_node.proto.document.get("bid") {
                if let Some(bid_str) = bid_val.as_str() {
                    if let Ok(bid) = Bid::try_from(bid_str) {
                        self.row_bids.push(RowBid {
                            tab: parsed_node.tab_name.clone(),
                            row: parsed_node.row_index,
                            bid,
                            source_col: self
                                .column_maps
                                .get(&parsed_node.tab_name)
                                .and_then(|cm| cm.iter().find(|e| e.ir_key == "bid" && e.hidden))
                                .map(|e| e.col_idx),
                        });
                    }
                }
            }
        }

        Ok(updated_node)
    }

    fn finalize(
        &mut self,
        _diagnostics: &mut Vec<ParseDiagnostic>,
    ) -> Result<HashMap<Bid, IRNode>, BuildonomyError> {
        let schema = match &mut self.schema {
            Some(s) => s,
            None => return Ok(HashMap::new()),
        };

        let mut modified = false;

        // Update workbook BID in schema.
        if let Some(wb_node) = self.nodes.first() {
            if let Some(bid_val) = wb_node.proto.document.get("bid") {
                if let Some(bid_str) = bid_val.as_str() {
                    if schema.bid.as_deref() != Some(bid_str) {
                        schema.bid = Some(bid_str.to_string());
                        modified = true;
                    }
                }
            }
        }

        // Update tab container BIDs in schema.tabs_meta.
        // Tab container nodes have row_index == 0 and a non-empty tab_name.
        for node in &self.nodes {
            if node.row_index == 0 && !node.tab_name.is_empty() {
                if let Some(bid_val) = node.proto.document.get("bid") {
                    if let Some(bid_str) = bid_val.as_str() {
                        let entry = schema.tabs_meta.entry(node.tab_name.clone()).or_default();
                        if entry.bid.as_deref() != Some(bid_str) {
                            entry.bid = Some(bid_str.to_string());
                            modified = true;
                        }
                    }
                }
            }
        }

        if modified {
            self.updated = true;
        }

        // Return the workbook IRNode as a modified node when the schema changed.
        // This mirrors MdCodec.finalize() returning the document node.
        if modified {
            if let Some(wb_node) = self.nodes.first() {
                if let Some(bid_val) = wb_node.proto.document.get("bid") {
                    if let Some(bid_str) = bid_val.as_str() {
                        if let Ok(bid) = Bid::try_from(bid_str) {
                            let mut map = HashMap::new();
                            map.insert(bid, wb_node.proto.clone());
                            return Ok(map);
                        }
                    }
                }
            }
        }

        Ok(HashMap::new())
    }

    fn generate_source(&self) -> Option<String> {
        // Binary codec — write-back is done via generate_source_bytes().
        None
    }

    /// Write an annotated copy of the workbook with BID values injected into a
    /// `__noet_bid__` column on each schema-declared tab.
    ///
    /// Returns `None` if no BID annotations were collected (no-op parse) or if
    /// the source file cannot be opened.
    fn generate_source_bytes(&self) -> Option<Vec<u8>> {
        if self.row_bids.is_empty() && !self.updated {
            return None;
        }

        // Use cached bytes from parse() when available, otherwise fall back to disk read.
        let original_bytes = if !self.file_bytes.is_empty() {
            self.file_bytes.clone()
        } else {
            match std::fs::read(&self.file_path) {
                Ok(b) => b,
                Err(e) => {
                    tracing::warn!(
                        "XlsxCodec::generate_source_bytes: failed to read {}: {e}",
                        self.file_path.display()
                    );
                    return None;
                }
            }
        };

        let mut source_wb: Xlsx<Cursor<Vec<u8>>> = match Self::open_workbook(original_bytes) {
            Ok(wb) => wb,
            Err(e) => {
                tracing::warn!("XlsxCodec::generate_source_bytes: failed to re-open workbook: {e}");
                return None;
            }
        };

        let schema = self.schema.as_ref()?;

        let mut out_wb = XlsxWriterWorkbook::new();

        self.write_index_tab(&mut source_wb, &mut out_wb);

        // Copy each non-index sheet, injecting BID annotations on schema-declared tabs.
        let all_sheets = Self::all_sheet_names(&source_wb);
        for sheet_name in &all_sheets {
            let ws = out_wb.add_worksheet();
            ws.set_name(sheet_name).ok();

            let range = match source_wb.worksheet_range(sheet_name) {
                Ok(r) => r,
                Err(_) => continue,
            };

            let rows: Vec<Vec<Data>> = range.rows().map(|r| r.to_vec()).collect();
            if rows.is_empty() {
                continue;
            }

            // A tab is "schema-declared" for write-back purposes when it either has an
            // exact name match in schema.tabs, OR it received row BIDs during parse
            // (wildcard-matched tabs accumulate row_bids with tab == actual_sheet_name).
            let is_schema_tab = schema.tabs.iter().any(|t| &t.name == sheet_name)
                || self.row_bids.iter().any(|rb| rb.tab == *sheet_name);

            if !is_schema_tab {
                Self::copy_verbatim_sheet(sheet_name, &rows, ws);
                continue;
            }

            let empty_map: Vec<ColumnEntry> = Vec::new();
            let column_map = self
                .column_maps
                .get(sheet_name.as_str())
                .map(|m| m.as_slice())
                .unwrap_or(empty_map.as_slice());
            self.write_annotated_sheet(sheet_name, &rows, column_map, ws);
        }

        match out_wb.save_to_buffer() {
            Ok(buf) => Some(buf),
            Err(e) => {
                tracing::warn!("XlsxCodec::generate_source_bytes: failed to serialize: {e}");
                None
            }
        }
    }

    fn generate_html(&self) -> Result<HtmlFragments, BuildonomyError> {
        let schema = match &self.schema {
            None => return Ok(vec![]),
            Some(s) => s,
        };

        if self.file_path.as_os_str().is_empty() {
            return Ok(vec![]);
        }

        let filestem = self
            .file_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("workbook");

        // Collect (display_name, col_schemas, row_nodes) triples in workbook sheet
        // order by iterating self.nodes, which is populated in workbook sheet order
        // during parse(). schema.tabs is a declaration of roles, not an ordering
        // directive — iterating it would produce schema-declaration order (lexical
        // or author-defined), not the actual sheet order in the workbook.
        let mut tab_groups: Vec<(
            String,
            &[crate::codec::xlsx::schema::ColumnSchema],
            Vec<&ParsedNode>,
        )> = Vec::new();

        // Collect distinct tab names in workbook sheet order from the node list.
        // Tab container nodes have row_index == 0 and a non-empty tab_name.
        let mut seen_tab_names: Vec<String> = Vec::new();
        for node in &self.nodes {
            if node.row_index != 0 || node.tab_name.is_empty() {
                continue;
            }
            if !seen_tab_names.contains(&node.tab_name) {
                seen_tab_names.push(node.tab_name.clone());
            }
        }

        let wildcard_schema: Option<&TabSchema> =
            schema.tabs.iter().find(|t| t.name == WILDCARD_TAB);

        for sheet_name in seen_tab_names {
            // Look up the schema for this tab: exact match → wildcard → skip (ignored/opaque).
            let tab_schema = schema
                .tabs
                .iter()
                .find(|t| t.name == sheet_name && t.name != WILDCARD_TAB)
                .or(wildcard_schema);

            let tab_schema = match tab_schema {
                Some(t) => t,
                None => continue, // no schema and no wildcard — opaque tab, skip
            };

            if tab_schema.ignore {
                continue;
            }

            let rows: Vec<&ParsedNode> = self
                .nodes
                .iter()
                .filter(|n| n.tab_name == sheet_name && n.row_index > 0)
                .collect();
            if !rows.is_empty() {
                tab_groups.push((sheet_name, &tab_schema.schema, rows));
            }
        }

        if tab_groups.is_empty() {
            return Ok(vec![]);
        }

        let mut fragments: HtmlFragments = Vec::with_capacity(1);
        let mut tab_sections = String::new();
        let mut tab_nav = String::new();

        // ── Per-tab sections and data (rendered into one workbook HTML file) ─
        //
        // Each tab becomes a <section id="{tab_node_id}" class="xlsx-tab"> block
        // containing only an <h2> heading and an empty container div.
        // Row data and column definitions are collected into a single JSON object
        // (xlsx_data_map) keyed by tab_node_id, serialized into the template's
        // {{XLSX_DATA}} placeholder, and read at runtime by xlsx-tabs.js.
        let mut xlsx_data_map = serde_json::Map::new();
        // Tab IDs in workbook sheet order — collected during the loop below.
        // Cannot be derived from xlsx_data_map.keys() after the fact because
        // serde_json::Map uses BTreeMap internally (alphabetical order).
        let mut tab_order: Vec<String> = Vec::new();

        for (display_name, col_schemas, row_nodes) in &tab_groups {
            let tab_anchor = to_anchor(display_name);

            // Determine whether the explicit schema already covers the title role.
            // When it doesn't, we prepend a synthetic "Title" column so the table
            // always shows a human-readable row identifier regardless of schema.
            let has_explicit_title = col_schemas.iter().any(|c| c.role == ColumnRole::Title);

            // System keys never surfaced as payload columns.
            const SYSTEM_KEYS: &[&str] = &[
                "bid", "text", "xlsx_tab", "xlsx_row", "id", "title", "schema",
            ];

            // Field names already covered by schema columns.
            let schema_fields: Vec<String> =
                col_schemas.iter().map(|c| to_anchor(&c.col)).collect();

            // Collect the union of additional payload keys across all rows, in
            // first-seen order (O(n²) is fine; tabs are small).
            // Payload keys are normalized to lowercase+underscore at storage time
            // in parse_tab, so document keys and schema_fields use the same form.
            let mut extra_keys: Vec<String> = Vec::new();
            for row_node in row_nodes.iter() {
                for (key, _) in row_node.proto.document.iter() {
                    if SYSTEM_KEYS.contains(&key) {
                        continue;
                    }
                    if schema_fields.contains(&key.to_string()) {
                        continue;
                    }
                    if !extra_keys.contains(&key.to_string()) {
                        extra_keys.push(key.to_string());
                    }
                }
            }

            // Helper: title-case a snake/kebab key → display title.
            let key_to_title = |key: &str| -> String {
                key.split(['_', '-'])
                    .map(|word| {
                        let mut chars = word.chars();
                        match chars.next() {
                            None => String::new(),
                            Some(first) => {
                                first.to_uppercase().collect::<String>() + chars.as_str()
                            }
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(" ")
            };

            // Build Tabulator column definitions as a proper JSON array.
            let mut col_defs_vec: Vec<serde_json::Value> = Vec::new();
            if !has_explicit_title {
                col_defs_vec.push(serde_json::json!({
                    "title": "Title",
                    "field": "title",
                    "role": "title",
                    "headerFilter": "input"
                }));
            }
            for col_schema in *col_schemas {
                let role_str = match col_schema.role {
                    ColumnRole::Title => "title",
                    ColumnRole::Text => "text",
                    ColumnRole::Relation => "relation",
                    ColumnRole::Payload => "payload",
                };
                let field = to_anchor(&col_schema.col);
                // For relation columns, surface the key_format so xlsx-tabs.js can
                // derive NodeKey strings ephemerally from the raw cell text stored
                // in doc[field], without needing a separate companion "_text" field.
                let key_format_str = match col_schema.key {
                    crate::codec::xlsx::schema::RelationKeyFormat::Auto => "auto",
                    crate::codec::xlsx::schema::RelationKeyFormat::Id => "id",
                    crate::codec::xlsx::schema::RelationKeyFormat::Path => "path",
                    crate::codec::xlsx::schema::RelationKeyFormat::Bid => "bid",
                    crate::codec::xlsx::schema::RelationKeyFormat::Bref => "bref",
                };
                let mut col_def = serde_json::json!({
                    "title": col_schema.col,
                    "field": field,
                    "role": role_str,
                    "wrap": col_schema.wrap,
                    "headerFilter": "input"
                });
                if col_schema.role == ColumnRole::Relation {
                    col_def["key_format"] = serde_json::Value::String(key_format_str.to_string());
                }
                col_defs_vec.push(col_def);
            }
            for key in &extra_keys {
                col_defs_vec.push(serde_json::json!({
                    "title": key_to_title(key),
                    "field": key,
                    "role": "payload",
                    "headerFilter": "input"
                }));
            }

            // Serialize row data as a proper JSON array.
            let mut rows_vec: Vec<serde_json::Value> = Vec::new();
            for row_node in row_nodes {
                let mut obj = serde_json::Map::new();

                // Row BID and ID (_bid, _id fields).
                let row_bid = row_node
                    .proto
                    .document
                    .get("bid")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                obj.insert("_bid".to_string(), serde_json::Value::String(row_bid));
                let row_id = row_node
                    .proto
                    .document
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                obj.insert("_id".to_string(), serde_json::Value::String(row_id));

                // Synthetic title field — always present so the JS field name "title"
                // resolves correctly even when the schema declares no role:title column.
                if !has_explicit_title {
                    obj.insert(
                        "title".to_string(),
                        serde_json::Value::String(row_node.proto.title().unwrap_or_default()),
                    );
                }
                for col_schema in *col_schemas {
                    let field = to_anchor(&col_schema.col);
                    let val = match col_schema.role {
                        ColumnRole::Title => row_node.proto.title().unwrap_or_default(),
                        ColumnRole::Text => {
                            let raw = row_node
                                .proto
                                .document
                                .get("text")
                                .and_then(|v| v.as_str())
                                .unwrap_or("");
                            let mut rendered = String::new();
                            let _ = to_html(raw, &mut rendered);
                            rendered
                        }
                        ColumnRole::Relation => {
                            // Emit the raw cell text stored in doc[field].
                            let normalized = to_anchor(&col_schema.col);
                            let raw = row_node
                                .proto
                                .document
                                .get(normalized.as_str())
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();

                            // Emit the companion {field}_bids field if inject_context
                            // populated it with resolved BIDs for this column.
                            let bids_key = format!("{field}_bids");
                            if let Some(bids_str) = row_node
                                .proto
                                .document
                                .get(bids_key.as_str())
                                .and_then(|v| v.as_str())
                            {
                                obj.insert(
                                    bids_key,
                                    serde_json::Value::String(bids_str.to_string()),
                                );
                            }

                            raw
                        }
                        ColumnRole::Payload => {
                            let normalized = to_anchor(&col_schema.col);
                            row_node
                                .proto
                                .document
                                .get(normalized.as_str())
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string()
                        }
                    };
                    obj.insert(field, serde_json::Value::String(val));
                }
                // Extra payload columns — keys are normalized at storage time so
                // document key and field name are identical.
                for key in &extra_keys {
                    let val = row_node
                        .proto
                        .document
                        .get(key.as_str())
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    obj.insert(key.clone(), serde_json::Value::String(val));
                }
                rows_vec.push(serde_json::Value::Object(obj));
            }

            // The section id is the tab node's prefixed id (e.g. "workbook-power").
            // This matches the path key anchor registered in the PathMap, so that
            // routing to workbook.html#workbook-power opens the correct tab.
            let tab_node = self
                .nodes
                .iter()
                .find(|n| n.row_index == 0 && n.tab_name == *display_name);
            let tab_node_id = tab_node
                .and_then(|n| n.proto.id())
                .unwrap_or_else(|| tab_anchor.clone());
            let tab_bid = tab_node
                .and_then(|n| n.proto.document.get("bid"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            // Section contains only the heading and an empty container div.
            // All data lives in the {{XLSX_DATA}} JSON block, read by xlsx-tabs.js.
            tab_sections.push_str(&format!(
                "<section id=\"{tab_node_id}\" class=\"xlsx-tab\">\n  <h2>{display_name}</h2>\n  <div class=\"xlsx-table-container\"></div>\n</section>\n",
            ));

            tab_nav.push_str(&format!(
                "  <li><a href=\"#{tab_node_id}\" class=\"xlsx-tab-link\" data-tab=\"{tab_node_id}\">{display_name}</a></li>\n",
            ));

            // Accumulate per-tab data into the shared JSON map.
            tab_order.push(tab_node_id.clone());
            xlsx_data_map.insert(
                tab_node_id,
                serde_json::json!({
                    "tab_bid": tab_bid,
                    "columns": col_defs_vec,
                    "rows": rows_vec,
                }),
            );
        }

        // Use the home network bref captured during inject_context() from ctx.home_net.
        // This is the authoritative home network of the workbook node, used by
        // xlsx-tabs.js to call get_bid_from_id(net_bref, id) for relation resolution.
        let net_bref = self.home_net_bref.clone();

        let xlsx_data_wrapper = serde_json::json!({
            "_net_bref": net_bref,
            "_tab_order": tab_order,
            "tabs": serde_json::Value::Object(xlsx_data_map),
        });
        let xlsx_data_json =
            serde_json::to_string(&xlsx_data_wrapper).unwrap_or_else(|_| "{}".to_string());

        // Single fragment using the XlsxWorkbook layout template.
        // Named pairs override the template placeholders; compiler defaults handle
        // TITLE, CANONICAL, SPA_ROUTE, SOURCE_LINK, BID, and SCRIPTS.
        let pairs = vec![
            ("{{WORKBOOK_TITLE}}".to_string(), schema.title.clone()),
            ("{{TABS_NAV}}".to_string(), tab_nav),
            ("{{TAB_SECTIONS}}".to_string(), tab_sections),
            ("{{XLSX_DATA}}".to_string(), xlsx_data_json),
        ];
        fragments.push((
            format!("{filestem}.html"),
            pairs,
            Some(crate::codec::assets::Layout::XlsxWorkbook),
        ));

        Ok(fragments)
    }
}

/// Write a single calamine `Data` cell to a `rust_xlsxwriter` worksheet.
fn write_cell(
    ws: &mut rust_xlsxwriter::Worksheet,
    row: u32,
    col: u16,
    cell: &Data,
) -> Result<(), rust_xlsxwriter::XlsxError> {
    match cell {
        Data::String(s) => ws.write(row, col, s.as_str()).map(|_| ()),
        Data::Float(f) => ws.write(row, col, *f).map(|_| ()),
        Data::Int(i) => ws.write(row, col, *i as f64).map(|_| ()),
        Data::Bool(b) => ws.write(row, col, *b).map(|_| ()),
        Data::DateTime(dt) => ws.write(row, col, dt.to_string().as_str()).map(|_| ()),
        Data::DateTimeIso(s) | Data::DurationIso(s) => ws.write(row, col, s.as_str()).map(|_| ()),
        Data::Empty | Data::Error(_) => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::io::Cursor;
    use std::path::{Path, PathBuf};

    use calamine::{open_workbook_from_rs, Reader, Xlsx};
    use rust_xlsxwriter::Workbook as XlsxWriterWorkbook;
    use tempfile::TempDir;
    use toml_edit::DocumentMut;

    use crate::{
        codec::{
            belief_ir::IRNode,
            xlsx::codec::{ColumnKind, XlsxCodec, BID_COLUMN_HEADER, INDEX_TAB},
            CodecContentMode, DocCodec, ParseDiagnostic,
        },
        properties::{BeliefKind, Bid},
    };

    // ── Fixture builders ────────────────────────────────────────────────────

    /// Minimal valid schema YAML for the index tab.
    fn simple_schema_yaml() -> &'static str {
        r#"title: "Widget Project Items"
tabs:
  - name: "Items"
    schema:
      - col: "Title"
        role: title
      - col: "Description"
        role: text
      - col: "Category"
        role: payload
  - name: "Measurements"
    schema:
      - col: "Title"
        role: title
      - col: "Description"
        role: text
"#
    }

    /// Build an xlsx file on disk and return (TempDir, PathBuf).
    /// `TempDir` must stay alive for the test duration.
    fn build_xlsx(sheets: &[(&str, &[&[&str]])]) -> (TempDir, PathBuf) {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("test.xlsx");
        let mut wb = XlsxWriterWorkbook::new();
        for (sheet_name, rows) in sheets {
            let ws = wb.add_worksheet();
            ws.set_name(*sheet_name).unwrap();
            for (row_idx, row) in rows.iter().enumerate() {
                for (col_idx, cell) in row.iter().enumerate() {
                    ws.write(row_idx as u32, col_idx as u16, *cell).unwrap();
                }
            }
        }
        wb.save(&path).expect("save xlsx");
        (dir, path)
    }

    /// Create a standard two-tab xlsx with an index schema and data rows.
    fn standard_fixture() -> (TempDir, PathBuf) {
        let index_rows: &[&[&str]] = &[&[simple_schema_yaml()]];
        let item_rows: &[&[&str]] = &[
            &["Title", "Description", "Category"],
            &[
                "Widget Alpha",
                "The system shall provide widget alpha.",
                "Category A",
            ],
            &[
                "Widget Beta",
                "The system shall provide widget beta.",
                "Category B",
            ],
            &[
                "Widget Gamma",
                "The system shall provide widget gamma.",
                "Category A",
            ],
            &[
                "Widget Delta",
                "The system shall provide widget delta.",
                "Category B",
            ],
            &[
                "Widget Epsilon",
                "The system shall provide widget epsilon.",
                "Category A",
            ],
        ];
        let meas_rows: &[&[&str]] = &[
            &["Title", "Description"],
            &["Measurement Alpha", "Measure the alpha widget output."],
            &["Measurement Beta", "Measure the beta widget output."],
        ];
        build_xlsx(&[
            (INDEX_TAB, index_rows),
            ("Items", item_rows),
            ("Measurements", meas_rows),
        ])
    }

    /// Build a proto IRNode suitable for passing to `parse()`.
    fn make_proto(path: &Path) -> IRNode {
        let mut doc = DocumentMut::new();
        doc.insert("title", toml_edit::value("placeholder"));
        IRNode {
            content: String::new(),
            document: doc,
            upstream: Vec::new(),
            downstream: Vec::new(),
            path: path.to_string_lossy().into_owned(),
            kind: crate::properties::BeliefKind::Document.into(),
            errors: Vec::new(),
            heading: 2,
            source_line: None,
            mappings: Vec::new(),
            path_aliases: Vec::new(),
            namespace_paths: Vec::new(),
            accumulator: None,
        }
    }

    // ── content_mode ────────────────────────────────────────────────────────

    #[test]
    fn test_content_mode_is_binary() {
        let codec = XlsxCodec::new();
        assert_eq!(codec.content_mode(), CodecContentMode::Binary);
    }

    // ── proto() ─────────────────────────────────────────────────────────────

    #[test]
    fn test_proto_returns_none_for_no_index_tab() {
        let rows: &[&[&str]] = &[&["Title", "Statement"], &["Row 1", "Body 1"]];
        let (_dir, path) = build_xlsx(&[("Data", rows)]);
        let codec = XlsxCodec::new();
        let result = codec.proto(&path).expect("proto should not error");
        assert!(
            result.is_none(),
            "workbook with no 'index' tab should return None from proto()"
        );
    }

    #[test]
    fn test_proto_returns_workbook_node_for_valid_schema() {
        let (_dir, path) = standard_fixture();
        let codec = XlsxCodec::new();
        let node = codec
            .proto(&path)
            .expect("proto should not error")
            .expect("proto should return Some for valid schema");
        assert_eq!(
            node.title().as_deref(),
            Some("Widget Project Items"),
            "workbook node title should match schema title field"
        );
        assert!(
            node.kind.contains(BeliefKind::Document),
            "workbook node should have Document kind"
        );
        assert_eq!(node.heading, 2, "workbook node should be heading level 2");
    }

    #[test]
    fn test_proto_returns_none_for_unparseable_schema() {
        // Content that cannot be parsed as YAML, JSON, or TOML by parse_with_fallback.
        let bad_content = "not: valid: yaml: [unclosed";
        let index_rows: &[&[&str]] = &[&[bad_content]];
        let (_dir, path) = build_xlsx(&[(INDEX_TAB, index_rows)]);
        let codec = XlsxCodec::new();
        // Unparseable content → proto() returns None (diagnostic logged, not propagated).
        let result = codec.proto(&path).expect("proto should not hard-error");
        assert!(
            result.is_none(),
            "unparseable schema content should return None"
        );
    }

    #[test]
    fn test_proto_requires_absolute_path() {
        let codec = XlsxCodec::new();
        let rel = std::path::PathBuf::from("relative/path.xlsx");
        let err = codec.proto(&rel).unwrap_err();
        assert!(
            format!("{err}").contains("absolute"),
            "proto() should reject relative paths"
        );
    }

    // ── parse() ─────────────────────────────────────────────────────────────

    #[test]
    fn test_parse_node_count_two_tabs() {
        let (_dir, path) = standard_fixture();
        let mut codec = XlsxCodec::new();
        let proto = make_proto(&path);
        let mut diagnostics = Vec::new();

        codec
            .parse(
                "",
                proto,
                &mut diagnostics,
                &crate::codec::proto_index::ProtoIndex::new(),
            )
            .expect("parse should not error");

        // Expected nodes:
        //   1  workbook (Document)
        //   1  "Items" tab container
        //   5  Items data rows
        //   1  "Measurements" tab container
        //   2  Measurements data rows
        // = 10 total
        let nodes = codec.nodes();
        assert_eq!(
            nodes.len(),
            10,
            "expected 10 nodes (1 workbook + 2 tabs + 5+2 rows), got {}",
            nodes.len()
        );
        assert!(
            diagnostics.is_empty(),
            "expected no diagnostics for valid fixture, got: {:?}",
            diagnostics
        );
    }

    #[test]
    fn test_parse_workbook_node_is_first_and_document() {
        let (_dir, path) = standard_fixture();
        let mut codec = XlsxCodec::new();
        let proto = make_proto(&path);
        codec
            .parse(
                "",
                proto,
                &mut Vec::new(),
                &crate::codec::proto_index::ProtoIndex::new(),
            )
            .unwrap();

        let nodes = codec.nodes();
        let workbook = &nodes[0];
        assert_eq!(workbook.title().as_deref(), Some("Widget Project Items"));
        assert!(workbook.kind.contains(BeliefKind::Document));
        assert_eq!(workbook.heading, 2);
    }

    #[test]
    fn test_parse_tab_nodes_are_heading_3() {
        let (_dir, path) = standard_fixture();
        let mut codec = XlsxCodec::new();
        codec
            .parse(
                "",
                make_proto(&path),
                &mut Vec::new(),
                &crate::codec::proto_index::ProtoIndex::new(),
            )
            .unwrap();
        let nodes = codec.nodes();

        // heading=3 tab nodes: one per declared tab ("Items", "Measurements").
        // Workbook node is heading=2; row nodes are heading=4.
        let tabs: Vec<_> = nodes.iter().filter(|n| n.heading == 3).collect();
        assert_eq!(tabs.len(), 2, "expected 2 tab container nodes");
    }

    #[test]
    fn test_parse_row_nodes_are_heading_4() {
        let (_dir, path) = standard_fixture();
        let mut codec = XlsxCodec::new();
        codec
            .parse(
                "",
                make_proto(&path),
                &mut Vec::new(),
                &crate::codec::proto_index::ProtoIndex::new(),
            )
            .unwrap();
        let nodes = codec.nodes();

        let rows: Vec<_> = nodes.iter().filter(|n| n.heading == 4).collect();
        assert_eq!(rows.len(), 7, "expected 5 + 2 = 7 row nodes");
    }

    #[test]
    fn test_parse_row_titles_correct() {
        let (_dir, path) = standard_fixture();
        let mut codec = XlsxCodec::new();
        codec
            .parse(
                "",
                make_proto(&path),
                &mut Vec::new(),
                &crate::codec::proto_index::ProtoIndex::new(),
            )
            .unwrap();
        let nodes = codec.nodes();

        let row_titles: Vec<String> = nodes
            .iter()
            .filter(|n| n.heading == 4)
            .filter_map(|n| n.title())
            .collect();

        assert!(row_titles.contains(&"Widget Alpha".to_string()));
        assert!(row_titles.contains(&"Measurement Beta".to_string()));
    }

    #[test]
    fn test_parse_category_column_stored_as_payload() {
        // Category is declared role:payload; its value lands in doc["category"].
        let (_dir, path) = standard_fixture();
        let mut codec = XlsxCodec::new();
        codec
            .parse(
                "",
                make_proto(&path),
                &mut Vec::new(),
                &crate::codec::proto_index::ProtoIndex::new(),
            )
            .unwrap();
        let nodes = codec.nodes();

        let init_node = nodes
            .iter()
            .find(|n| n.title().as_deref() == Some("Widget Alpha"))
            .expect("Widget Alpha row not found");

        let category = init_node
            .document
            .get("category")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        assert_eq!(
            category, "Category A",
            "expected category payload 'Category A', got {:?}",
            category
        );
    }

    #[test]
    fn test_parse_provenance_payload() {
        let (_dir, path) = standard_fixture();
        let mut codec = XlsxCodec::new();
        codec
            .parse(
                "",
                make_proto(&path),
                &mut Vec::new(),
                &crate::codec::proto_index::ProtoIndex::new(),
            )
            .unwrap();
        let nodes = codec.nodes();

        let init_node = nodes
            .iter()
            .find(|n| n.title().as_deref() == Some("Widget Alpha"))
            .expect("Widget Alpha row not found");

        let tab = init_node.document.get("xlsx_tab").and_then(|v| v.as_str());
        assert_eq!(tab, Some("Items"));

        let row = init_node
            .document
            .get("xlsx_row")
            .and_then(|v| v.as_integer());
        assert_eq!(row, Some(1), "first data row should be row index 1");
    }

    #[test]
    fn test_parse_tab_absent_from_schema_emits_opaque_tab_node() {
        // Add an extra "Raw Export" tab not in the schema.
        let index_rows: &[&[&str]] = &[&[simple_schema_yaml()]];
        let item_rows: &[&[&str]] = &[
            &["Title", "Description", "Category"],
            &["Row 1", "Body 1", "Cat1"],
        ];
        let raw_rows: &[&[&str]] = &[&["Col A", "Col B"], &["val 1", "val 2"]];
        let (_dir, path) = build_xlsx(&[
            (INDEX_TAB, index_rows),
            ("Items", item_rows),
            ("Measurements", &[&["Title", "Description"]]),
            ("Raw Export", raw_rows),
        ]);

        let mut codec = XlsxCodec::new();
        codec
            .parse(
                "",
                make_proto(&path),
                &mut Vec::new(),
                &crate::codec::proto_index::ProtoIndex::new(),
            )
            .unwrap();
        let nodes = codec.nodes();

        // "Raw Export" should appear as exactly one heading-3 node with no row children.
        let raw_tabs: Vec<_> = nodes
            .iter()
            .filter(|n| n.title().as_deref() == Some("Raw Export"))
            .collect();
        assert_eq!(
            raw_tabs.len(),
            1,
            "expected exactly one 'Raw Export' tab node"
        );
        assert_eq!(raw_tabs[0].heading, 3, "opaque tab should be heading 3");

        // No row nodes should be emitted for Raw Export.
        let raw_rows_emitted: Vec<_> = nodes
            .iter()
            .filter(|n| {
                n.heading == 4
                    && n.document.get("xlsx_tab").and_then(|v| v.as_str()) == Some("Raw Export")
            })
            .collect();
        assert!(
            raw_rows_emitted.is_empty(),
            "opaque tab should emit no row nodes"
        );
    }

    #[test]
    fn test_parse_missing_column_emits_warning_not_error() {
        // Schema declares "Missing Col" which is absent from the header row.
        let schema_yaml = r#"title: "Test"
tabs:
  - name: "Data"
    schema:
      - col: "Title"
        role: title
      - col: "Missing Col"
        role: text
"#;
        let index_rows: &[&[&str]] = &[&[schema_yaml]];
        let data_rows: &[&[&str]] = &[&["Title"], &["Row One"]];
        let (_dir, path) = build_xlsx(&[(INDEX_TAB, index_rows), ("Data", data_rows)]);

        let mut codec = XlsxCodec::new();
        let mut diagnostics = Vec::new();
        codec
            .parse(
                "",
                make_proto(&path),
                &mut diagnostics,
                &crate::codec::proto_index::ProtoIndex::new(),
            )
            .unwrap();

        assert!(
            diagnostics.iter().any(|d| {
                matches!(d, ParseDiagnostic::Warning { message, .. } if message.contains("Missing Col"))
            }),
            "expected a Warning about 'Missing Col', got: {:?}",
            diagnostics
        );

        // Should still emit the row node for the present title column.
        let nodes = codec.nodes();
        let row_nodes: Vec<_> = nodes.iter().filter(|n| n.heading == 4).collect();
        assert_eq!(
            row_nodes.len(),
            1,
            "row node should still be emitted despite missing column"
        );
    }

    #[test]
    fn test_parse_empty_rows_skipped() {
        let index_rows: &[&[&str]] = &[&[simple_schema_yaml()]];
        let item_rows: &[&[&str]] = &[
            &["Title", "Description", "Category"],
            &["Row One", "Body", "Cat"],
            &["", "", ""], // empty row — should be skipped
            &["Row Two", "Body 2", "Cat2"],
        ];
        let (_dir, path) = build_xlsx(&[
            (INDEX_TAB, index_rows),
            ("Items", item_rows),
            ("Measurements", &[&["Title", "Description"]]),
        ]);

        let mut codec = XlsxCodec::new();
        codec
            .parse(
                "",
                make_proto(&path),
                &mut Vec::new(),
                &crate::codec::proto_index::ProtoIndex::new(),
            )
            .unwrap();
        let nodes = codec.nodes();

        let item_rows_emitted: Vec<_> = nodes
            .iter()
            .filter(|n| {
                n.heading == 4
                    && n.document.get("xlsx_tab").and_then(|v| v.as_str()) == Some("Items")
            })
            .collect();
        assert_eq!(
            item_rows_emitted.len(),
            2,
            "empty row should be skipped; expected 2 data rows"
        );
    }

    #[test]
    fn test_set_node_bid_injects_bid() {
        let (_dir, path) = standard_fixture();
        let mut codec = XlsxCodec::new();
        codec
            .parse(
                "",
                make_proto(&path),
                &mut Vec::new(),
                &crate::codec::proto_index::ProtoIndex::new(),
            )
            .unwrap();

        let test_bid = Bid::new(Bid::nil());
        codec.set_node_bid(2, test_bid); // index 2 = first row node

        let nodes = codec.nodes();
        let bid_val = nodes[2]
            .document
            .get("bid")
            .and_then(|v| v.as_str())
            .and_then(|s| Bid::try_from(s).ok());
        assert_eq!(
            bid_val,
            Some(test_bid),
            "set_node_bid should inject the BID"
        );
    }

    #[test]
    fn test_set_node_bid_does_not_overwrite_existing() {
        let (_dir, path) = standard_fixture();
        let mut codec = XlsxCodec::new();
        codec
            .parse(
                "",
                make_proto(&path),
                &mut Vec::new(),
                &crate::codec::proto_index::ProtoIndex::new(),
            )
            .unwrap();

        let first_bid = Bid::new(Bid::nil());
        let second_bid = Bid::new(Bid::nil());
        codec.set_node_bid(2, first_bid);
        codec.set_node_bid(2, second_bid); // should not overwrite

        let nodes = codec.nodes();
        let bid_val = nodes[2]
            .document
            .get("bid")
            .and_then(|v| v.as_str())
            .and_then(|s| Bid::try_from(s).ok());
        assert_eq!(
            bid_val,
            Some(first_bid),
            "set_node_bid should not overwrite an already-set BID"
        );
    }

    // ── generate_source_bytes() ─────────────────────────────────────────────

    #[test]
    fn test_generate_source_bytes_returns_none_without_bids() {
        let (_dir, path) = standard_fixture();
        let mut codec = XlsxCodec::new();
        codec
            .parse(
                "",
                make_proto(&path),
                &mut Vec::new(),
                &crate::codec::proto_index::ProtoIndex::new(),
            )
            .unwrap();
        // No inject_context called → no row_bids collected.
        assert!(
            codec.generate_source_bytes().is_none(),
            "generate_source_bytes should return None when no BIDs have been collected"
        );
    }

    #[test]
    fn test_generate_source_bytes_injects_bid_column() {
        let (_dir, path) = standard_fixture();
        let mut codec = XlsxCodec::new();
        codec
            .parse(
                "",
                make_proto(&path),
                &mut Vec::new(),
                &crate::codec::proto_index::ProtoIndex::new(),
            )
            .unwrap();

        // Manually inject a row BID (simulating what inject_context would do).
        let test_bid = Bid::new(Bid::nil());
        codec.set_node_bid(2, test_bid); // first row node
                                         // Directly push a RowBid to simulate inject_context collecting it.
        codec.row_bids.push(crate::codec::xlsx::codec::RowBid {
            tab: "Items".to_string(),
            row: 1,
            bid: test_bid,
            source_col: None,
        });

        let bytes = codec
            .generate_source_bytes()
            .expect("should produce bytes when BIDs are present");

        // Re-open the produced workbook and verify the BID column is present.
        let mut wb: Xlsx<Cursor<Vec<u8>>> =
            open_workbook_from_rs(Cursor::new(bytes)).expect("re-open annotated workbook");
        let sheet = wb.worksheet_range("Items").expect("Items sheet");
        let header_row: Vec<String> = sheet
            .rows()
            .next()
            .unwrap_or(&[])
            .iter()
            .map(|c| c.to_string())
            .collect();

        assert!(
            header_row.contains(&BID_COLUMN_HEADER.to_string()),
            "annotated workbook should contain '{}' column in header, got: {:?}",
            BID_COLUMN_HEADER,
            header_row
        );

        // Verify the BID value in row 1 (second row, 0-based).
        let bid_col_idx = header_row
            .iter()
            .position(|h| h == BID_COLUMN_HEADER)
            .expect("BID column index");
        let data_row_1: Vec<String> = sheet
            .rows()
            .nth(1)
            .unwrap_or(&[])
            .iter()
            .map(|c| c.to_string())
            .collect();
        let written_bid = data_row_1.get(bid_col_idx).cloned().unwrap_or_default();
        assert_eq!(
            written_bid,
            test_bid.to_string(),
            "BID column value should match the injected BID"
        );
    }

    // ── generate_source() ───────────────────────────────────────────────────

    #[test]
    fn test_generate_source_returns_none() {
        // Binary codec always returns None from generate_source().
        let codec = XlsxCodec::new();
        assert!(codec.generate_source().is_none());
    }

    // ── Never-applied explicit column warning ────────────────────────────────

    #[test]
    fn test_never_applied_explicit_column_emits_warning() {
        // A column declared in the schema exists in the header but is always empty.
        let schema_yaml = r#"title: "Widget Project"
tabs:
  - name: "Data"
    schema:
      - col: "Title"
        role: title
      - col: "Implements"
        role: relation
"#;
        let index_rows: &[&[&str]] = &[&[schema_yaml]];
        // "Implements" column is present in header but all cells are empty.
        let data_rows: &[&[&str]] = &[&["Title", "Implements"], &["Row One", ""], &["Row Two", ""]];
        let (_dir, path) = build_xlsx(&[(INDEX_TAB, index_rows), ("Data", data_rows)]);

        let mut codec = XlsxCodec::new();
        let mut diagnostics = Vec::new();
        codec
            .parse(
                "",
                make_proto(&path),
                &mut diagnostics,
                &crate::codec::proto_index::ProtoIndex::new(),
            )
            .unwrap();

        let has_never_applied = diagnostics.iter().any(|d| {
            matches!(d, ParseDiagnostic::Warning { message, .. }
                if message.contains("Implements") && message.contains("empty in every data row"))
        });
        assert!(
            has_never_applied,
            "expected a never-applied warning for 'Implements', got: {diagnostics:?}"
        );

        // Row nodes should still be emitted despite the warning.
        let nodes = codec.nodes();
        let row_nodes: Vec<_> = nodes.iter().filter(|n| n.heading == 4).collect();
        assert_eq!(row_nodes.len(), 2, "row nodes should still be emitted");
    }

    #[test]
    fn test_never_applied_warning_not_fired_for_populated_column() {
        // When at least one row has a non-empty value, no warning should be emitted.
        let schema_yaml = r#"title: "Widget Project"
tabs:
  - name: "Data"
    schema:
      - col: "Title"
        role: title
      - col: "Category"
        role: payload
"#;
        let index_rows: &[&[&str]] = &[&[schema_yaml]];
        let data_rows: &[&[&str]] = &[
            &["Title", "Category"],
            &["Row One", ""],       // empty
            &["Row Two", "Safety"], // non-empty — suppresses warning
        ];
        let (_dir, path) = build_xlsx(&[(INDEX_TAB, index_rows), ("Data", data_rows)]);

        let mut codec = XlsxCodec::new();
        let mut diagnostics = Vec::new();
        codec
            .parse(
                "",
                make_proto(&path),
                &mut diagnostics,
                &crate::codec::proto_index::ProtoIndex::new(),
            )
            .unwrap();

        let has_never_applied = diagnostics.iter().any(|d| {
            matches!(d, ParseDiagnostic::Warning { message, .. }
                if message.contains("empty in every data row"))
        });
        assert!(
            !has_never_applied,
            "should not warn when at least one row has a value"
        );
    }

    #[test]
    fn test_never_applied_warning_not_fired_for_title_role() {
        // Title-role columns are consumed outside the hit-counting loop;
        // they must never trigger the never-applied warning even when all title
        // cells are technically skipped by the loop.
        let schema_yaml = r#"title: "Widget Project"
tabs:
  - name: "Data"
    schema:
      - col: "Title"
        role: title
"#;
        let index_rows: &[&[&str]] = &[&[schema_yaml]];
        let data_rows: &[&[&str]] = &[&["Title"], &["Row One"], &["Row Two"]];
        let (_dir, path) = build_xlsx(&[(INDEX_TAB, index_rows), ("Data", data_rows)]);

        let mut codec = XlsxCodec::new();
        let mut diagnostics = Vec::new();
        codec
            .parse(
                "",
                make_proto(&path),
                &mut diagnostics,
                &crate::codec::proto_index::ProtoIndex::new(),
            )
            .unwrap();

        assert!(
            diagnostics.is_empty(),
            "title-role column must not trigger never-applied warning, got: {diagnostics:?}"
        );
    }

    // ── Wildcard tab (name: "*") ─────────────────────────────────────────────

    #[test]
    fn test_wildcard_tab_applies_to_unmatched_tabs() {
        // A schema with only name: "*" should apply to every non-index tab.
        let schema_yaml = r#"title: "Widget Project"
tabs:
  - name: "*"
    schema:
      - col: "Description"
        role: text
      - col: "Category"
        role: payload
"#;
        let index_rows: &[&[&str]] = &[&[schema_yaml]];
        let tab_a_rows: &[&[&str]] = &[
            &["Title", "Description", "Category"],
            &["Row A1", "Body A1", "Safety"],
            &["Row A2", "Body A2", "Performance"],
        ];
        let tab_b_rows: &[&[&str]] = &[
            &["Title", "Description", "Category"],
            &["Row B1", "Body B1", "Reliability"],
        ];
        let (_dir, path) = build_xlsx(&[
            (INDEX_TAB, index_rows),
            ("Alpha", tab_a_rows),
            ("Beta", tab_b_rows),
        ]);

        let mut codec = XlsxCodec::new();
        let mut diagnostics = Vec::new();
        codec
            .parse(
                "",
                make_proto(&path),
                &mut diagnostics,
                &crate::codec::proto_index::ProtoIndex::new(),
            )
            .unwrap();

        assert!(
            diagnostics.is_empty(),
            "no diagnostics expected for wildcard schema, got: {diagnostics:?}"
        );

        let nodes = codec.nodes();

        // Expect: 1 workbook + 2 tab containers + 2 + 1 row nodes = 6 total.
        assert_eq!(
            nodes.len(),
            6,
            "expected 1 workbook + 2 tabs + 3 rows = 6 nodes, got {}",
            nodes.len()
        );

        // Tab containers should exist for both Alpha and Beta (heading=3, Document).
        let tab_titles: Vec<_> = nodes
            .iter()
            .filter(|n| n.heading == 3)
            .filter_map(|n| n.title())
            .collect();
        assert!(
            tab_titles.contains(&"Alpha".to_string()),
            "Alpha tab node missing"
        );
        assert!(
            tab_titles.contains(&"Beta".to_string()),
            "Beta tab node missing"
        );

        // Row nodes should be heading=4.
        let row_nodes: Vec<_> = nodes.iter().filter(|n| n.heading == 4).collect();
        assert_eq!(
            row_nodes.len(),
            3,
            "expected 3 row nodes (2 from Alpha, 1 from Beta)"
        );

        // Category stored as payload via wildcard schema.
        let a1 = row_nodes
            .iter()
            .find(|n| n.title().as_deref() == Some("Row A1"))
            .expect("Row A1 should exist");
        let category = a1
            .document
            .get("category")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        assert_eq!(
            category, "Safety",
            "wildcard schema category column should be stored as payload, got {category:?}"
        );
    }

    #[test]
    fn test_wildcard_tab_does_not_override_exact_match() {
        // An exact-name tab schema takes priority over the wildcard.
        let schema_yaml = r#"title: "Widget Project"
tabs:
  - name: "Special"
    schema:
      - col: "Title"
        role: title
      - col: "Notes"
        role: text
  - name: "*"
    schema:
      - col: "Category"
        role: payload
"#;
        let index_rows: &[&[&str]] = &[&[schema_yaml]];
        let special_rows: &[&[&str]] = &[
            &["Title", "Notes", "Category"],
            &["Item 1", "Some notes.", "Alpha"],
        ];
        let other_rows: &[&[&str]] = &[&["Title", "Category"], &["Item 2", "Beta"]];
        let (_dir, path) = build_xlsx(&[
            (INDEX_TAB, index_rows),
            ("Special", special_rows),
            ("Other", other_rows),
        ]);

        let mut codec = XlsxCodec::new();
        codec
            .parse(
                "",
                make_proto(&path),
                &mut Vec::new(),
                &crate::codec::proto_index::ProtoIndex::new(),
            )
            .unwrap();
        let nodes = codec.nodes();

        // "Special" row should have text from "Notes" column (exact schema).
        let special_row = nodes
            .iter()
            .find(|n| n.title().as_deref() == Some("Item 1"))
            .expect("Item 1 should exist");
        let text = special_row
            .document
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        assert_eq!(
            text, "Some notes.",
            "exact schema should apply to Special tab"
        );

        // "Other" row should have tag from wildcard schema.
        let other_row = nodes
            .iter()
            .find(|n| n.title().as_deref() == Some("Item 2"))
            .expect("Item 2 should exist");
        let category = other_row
            .document
            .get("category")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        assert_eq!(
            category, "Beta",
            "wildcard schema should apply to Other tab, got {category:?}"
        );
    }

    #[test]
    fn test_wildcard_tab_no_opaque_fallback_when_wildcard_present() {
        // When a wildcard is present, no tab should be treated as opaque
        // (no CSV exported, no opaque tab node without row children).
        let schema_yaml = r#"title: "Widget Project"
tabs:
  - name: "*"
    schema: []
"#;
        let index_rows: &[&[&str]] = &[&[schema_yaml]];
        let data_rows: &[&[&str]] = &[&["Title", "Value"], &["Row 1", "Val 1"]];
        let (_dir, path) = build_xlsx(&[(INDEX_TAB, index_rows), ("AnyTab", data_rows)]);

        let mut codec = XlsxCodec::new();
        codec
            .parse(
                "",
                make_proto(&path),
                &mut Vec::new(),
                &crate::codec::proto_index::ProtoIndex::new(),
            )
            .unwrap();

        // Row node should exist (first col → title default).
        let nodes = codec.nodes();
        let row_nodes: Vec<_> = nodes.iter().filter(|n| n.heading == 4).collect();
        assert_eq!(
            row_nodes.len(),
            1,
            "wildcard schema should produce row nodes"
        );
    }

    #[test]
    fn test_wildcard_tab_row_bids_written_back() {
        let schema_yaml = r#"title: "Widget Project"
tabs:
  - name: "*"
    schema:
      - col: "Description"
        role: text
      - col: "Category"
        role: payload
"#;
        let index_rows: &[&[&str]] = &[&[schema_yaml]];
        let data_rows: &[&[&str]] = &[
            &["Title", "Description", "Category"],
            &["Row A1", "Body A1", "Safety"],
            &["Row A2", "Body A2", "Performance"],
        ];
        let (_dir, path) = build_xlsx(&[(INDEX_TAB, index_rows), ("Alpha", data_rows)]);

        let mut codec = XlsxCodec::new();
        codec
            .parse(
                "",
                make_proto(&path),
                &mut Vec::new(),
                &crate::codec::proto_index::ProtoIndex::new(),
            )
            .unwrap();

        // Verify row nodes are emitted for wildcard-matched tab.
        let nodes = codec.nodes();
        let row_nodes: Vec<_> = nodes.iter().filter(|n| n.heading == 4).collect();
        assert_eq!(
            row_nodes.len(),
            2,
            "wildcard schema should emit 2 row nodes"
        );

        // Simulate inject_context collecting row BIDs (tab name must be "Alpha", not "*").
        let test_bid = Bid::new(Bid::nil());
        codec.row_bids.push(crate::codec::xlsx::codec::RowBid {
            tab: "Alpha".to_string(), // actual sheet name, not "*"
            row: 1,
            bid: test_bid,
            source_col: None,
        });

        // generate_source_bytes must return Some and inject __noet_bid__ into "Alpha".
        let bytes = codec
            .generate_source_bytes()
            .expect("generate_source_bytes should return Some for wildcard tab with row BIDs");

        // Re-open and verify.
        let mut wb: Xlsx<std::io::Cursor<Vec<u8>>> =
            open_workbook_from_rs(std::io::Cursor::new(bytes)).unwrap();
        let sheet = wb
            .worksheet_range("Alpha")
            .expect("Alpha sheet must be present");
        let header: Vec<String> = sheet
            .rows()
            .next()
            .unwrap_or(&[])
            .iter()
            .map(|c| c.to_string())
            .collect();
        assert!(
            header.contains(&BID_COLUMN_HEADER.to_string()),
            "wildcard-matched tab 'Alpha' must receive __noet_bid__ column after write-back; \
             header: {header:?}"
        );
    }

    // ── Phase 3: text_template interpolation ────────────────────────────────

    #[test]
    fn test_interpolate_template_basic() {
        let mut row = HashMap::new();
        row.insert(
            "Description".to_string(),
            "The system shall initialise.".to_string(),
        );
        row.insert(
            "Rationale".to_string(),
            "Required for safe startup.".to_string(),
        );

        let tmpl = "{{Description}}\n\n**Rationale**: {{Rationale}}";
        let result = XlsxCodec::interpolate_template(tmpl, &row);
        assert_eq!(
            result,
            "The system shall initialise.\n\n**Rationale**: Required for safe startup."
        );
    }

    #[test]
    fn test_interpolate_template_missing_col_replaced_with_empty() {
        let mut row = HashMap::new();
        row.insert("Description".to_string(), "Body text.".to_string());
        // "Rationale" is absent from the row.

        let tmpl = "{{Description}}\n\n**Rationale**: {{Rationale}}";
        let result = XlsxCodec::interpolate_template(tmpl, &row);
        assert_eq!(result, "Body text.\n\n**Rationale**: ");
    }

    #[test]
    fn test_interpolate_template_all_missing_leaves_empty() {
        let row: HashMap<String, String> = HashMap::new();
        let tmpl = "{{Missing}} and {{AlsoMissing}}";
        let result = XlsxCodec::interpolate_template(tmpl, &row);
        assert_eq!(
            result, " and ",
            "both placeholders replaced with empty string"
        );
    }

    #[test]
    fn test_interpolate_template_no_placeholders() {
        let row: HashMap<String, String> = HashMap::new();
        let tmpl = "Plain text with no placeholders.";
        let result = XlsxCodec::interpolate_template(tmpl, &row);
        assert_eq!(result, "Plain text with no placeholders.");
    }

    #[test]
    fn test_text_template_applied_in_parse() {
        let schema_yaml = r#"title: "Widget Project"
tabs:
  - name: "Requirements"
    text_template: "{{Description}}\n\n**Rationale**: {{Rationale}}"
    schema:
      - col: "Category"
        role: payload
"#;
        let index_rows: &[&[&str]] = &[&[schema_yaml]];
        let data_rows: &[&[&str]] = &[
            &["Title", "Description", "Rationale", "Category"],
            &[
                "Req One",
                "The system shall do X.",
                "Safety critical.",
                "Safety",
            ],
        ];
        let (_dir, path) = build_xlsx(&[(INDEX_TAB, index_rows), ("Requirements", data_rows)]);

        let mut codec = XlsxCodec::new();
        let proto = make_proto(&path);
        let mut diagnostics = Vec::new();
        codec
            .parse(
                "",
                proto,
                &mut diagnostics,
                &crate::codec::proto_index::ProtoIndex::new(),
            )
            .unwrap();

        let nodes = codec.nodes();
        let row_node = nodes
            .iter()
            .find(|n| n.title().as_deref() == Some("Req One"))
            .expect("Req One row node should exist");

        let text = row_node
            .document
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        // The template should have been interpolated and the result stored.
        assert!(
            text.contains("The system shall do X."),
            "text should contain Description value, got: {text:?}"
        );
        assert!(
            text.contains("Rationale"),
            "text should contain the Rationale label from template, got: {text:?}"
        );
        assert!(
            text.contains("Safety critical."),
            "text should contain Rationale value, got: {text:?}"
        );
        assert!(
            diagnostics.is_empty(),
            "no diagnostics expected for valid template, got: {diagnostics:?}"
        );
    }

    #[test]
    fn test_text_template_markdown_links_extracted_as_relations() {
        let schema_yaml = r#"title: "Widget Project"
tabs:
  - name: "Requirements"
    text_template: "{{Description}} See [[abc123de]]."
    schema: []
"#;
        let index_rows: &[&[&str]] = &[&[schema_yaml]];
        let data_rows: &[&[&str]] = &[
            &["Title", "Description"],
            &["Req One", "The system shall do X."],
        ];
        let (_dir, path) = build_xlsx(&[(INDEX_TAB, index_rows), ("Requirements", data_rows)]);

        let mut codec = XlsxCodec::new();
        codec
            .parse(
                "",
                make_proto(&path),
                &mut Vec::new(),
                &crate::codec::proto_index::ProtoIndex::new(),
            )
            .unwrap();

        let nodes = codec.nodes();
        let row_node = nodes
            .iter()
            .find(|n| n.title().as_deref() == Some("Req One"))
            .expect("row node should exist");

        assert!(
            !row_node.upstream.is_empty(),
            "wikilink in text_template should produce an upstream relation"
        );
    }

    // ── Phase 4: updated flag and generate_source_bytes gating ──────────────

    #[test]
    fn test_updated_flag_false_after_parse() {
        let (_dir, path) = standard_fixture();
        let mut codec = XlsxCodec::new();
        codec
            .parse(
                "",
                make_proto(&path),
                &mut Vec::new(),
                &crate::codec::proto_index::ProtoIndex::new(),
            )
            .unwrap();
        assert!(
            !codec.updated,
            "updated flag should be false immediately after parse"
        );
    }

    #[test]
    fn test_generate_source_bytes_none_when_no_bids_and_not_updated() {
        let (_dir, path) = standard_fixture();
        let mut codec = XlsxCodec::new();
        codec
            .parse(
                "",
                make_proto(&path),
                &mut Vec::new(),
                &crate::codec::proto_index::ProtoIndex::new(),
            )
            .unwrap();
        // No inject_context called, no BIDs, updated=false → None.
        assert!(
            codec.generate_source_bytes().is_none(),
            "generate_source_bytes should return None when no changes"
        );
    }

    #[test]
    fn test_generate_source_bytes_triggered_by_updated_flag() {
        let (_dir, path) = standard_fixture();
        let mut codec = XlsxCodec::new();
        codec
            .parse(
                "",
                make_proto(&path),
                &mut Vec::new(),
                &crate::codec::proto_index::ProtoIndex::new(),
            )
            .unwrap();

        // Manually set updated=true (simulating inject_context detecting a change).
        codec.updated = true;

        let bytes = codec.generate_source_bytes();
        assert!(
            bytes.is_some(),
            "generate_source_bytes should return Some when updated=true"
        );
    }

    #[test]
    fn test_raw_text_stored_on_row_nodes() {
        let schema_yaml = r#"title: "Widget Project"
tabs:
  - name: "Requirements"
    schema:
      - col: "Title"
        role: title
      - col: "Description"
        role: text
"#;
        let index_rows: &[&[&str]] = &[&[schema_yaml]];
        let data_rows: &[&[&str]] = &[
            &["Title", "Description"],
            &["Req One", "The system shall do X."],
        ];
        let (_dir, path) = build_xlsx(&[(INDEX_TAB, index_rows), ("Requirements", data_rows)]);

        let mut codec = XlsxCodec::new();
        codec
            .parse(
                "",
                make_proto(&path),
                &mut Vec::new(),
                &crate::codec::proto_index::ProtoIndex::new(),
            )
            .unwrap();

        // The ParsedNode for the row should have raw_text populated.
        let row_parsed = codec
            .nodes
            .iter()
            .find(|n| n.proto.title().as_deref() == Some("Req One"))
            .expect("row ParsedNode should exist");

        assert_eq!(
            row_parsed.raw_text.as_deref(),
            Some("The system shall do X."),
            "raw_text should hold the plain Markdown text (not HTML-rendered)"
        );
    }

    // ── finalize() BID collection ────────────────────────────────────────────

    #[test]
    fn test_finalize_collects_workbook_bid_into_schema() {
        let (_dir, path) = standard_fixture();
        let mut codec = XlsxCodec::new();
        codec
            .parse(
                "",
                make_proto(&path),
                &mut Vec::new(),
                &crate::codec::proto_index::ProtoIndex::new(),
            )
            .unwrap();

        // Simulate inject_context setting document["bid"] on the workbook node (index 0).
        let wb_bid = Bid::new(Bid::nil());
        let bid_str = wb_bid.to_string();
        codec.nodes[0]
            .proto
            .document
            .insert("bid", toml_edit::value(bid_str.clone()));

        // finalize() should detect the bid is new and store it in schema.
        let mut diags = Vec::new();
        let result = codec.finalize(&mut diags).unwrap();

        // schema.bid must now be set.
        let schema = codec
            .schema
            .as_ref()
            .expect("schema must be Some after parse");
        assert_eq!(
            schema.bid.as_deref(),
            Some(bid_str.as_str()),
            "finalize() should store workbook BID in schema.bid"
        );

        // updated flag must be true.
        assert!(
            codec.updated,
            "finalize() should set updated=true when schema.bid changed"
        );

        // The workbook IRNode should be returned as a modified node.
        assert!(
            result.contains_key(&wb_bid),
            "finalize() should return the workbook IRNode keyed by its BID"
        );
    }

    #[test]
    fn test_finalize_collects_tab_bids_into_schema_tabs_meta() {
        let (_dir, path) = standard_fixture();
        let mut codec = XlsxCodec::new();
        codec
            .parse(
                "",
                make_proto(&path),
                &mut Vec::new(),
                &crate::codec::proto_index::ProtoIndex::new(),
            )
            .unwrap();

        // Simulate inject_context setting document["bid"] on both tab container nodes.
        // Tab container nodes: row_index == 0 and tab_name is non-empty.
        let req_bid = Bid::new(Bid::nil());
        let sensor_bid = Bid::new(Bid::nil());

        // Find the Items tab container (row_index == 0, tab_name == "Items").
        let req_idx = codec
            .nodes
            .iter()
            .position(|n| n.row_index == 0 && n.tab_name == "Items")
            .expect("Items tab container must exist");
        let sensor_idx = codec
            .nodes
            .iter()
            .position(|n| n.row_index == 0 && n.tab_name == "Measurements")
            .expect("Measurements tab container must exist");

        codec.nodes[req_idx]
            .proto
            .document
            .insert("bid", toml_edit::value(req_bid.to_string()));
        codec.nodes[sensor_idx]
            .proto
            .document
            .insert("bid", toml_edit::value(sensor_bid.to_string()));

        let mut diags = Vec::new();
        codec.finalize(&mut diags).unwrap();

        let schema = codec.schema.as_ref().expect("schema must be Some");

        let req_entry = schema
            .tabs_meta
            .get("Items")
            .expect("tabs_meta must have Items entry");
        assert_eq!(
            req_entry.bid.as_deref(),
            Some(req_bid.to_string().as_str()),
            "tabs_meta[Items].bid must match injected BID"
        );

        let sensor_entry = schema
            .tabs_meta
            .get("Measurements")
            .expect("tabs_meta must have Measurements entry");
        assert_eq!(
            sensor_entry.bid.as_deref(),
            Some(sensor_bid.to_string().as_str()),
            "tabs_meta[Measurements].bid must match injected BID"
        );
    }

    #[test]
    fn test_finalize_no_modification_when_bids_match_schema() {
        let (_dir, path) = standard_fixture();
        let mut codec = XlsxCodec::new();
        codec
            .parse(
                "",
                make_proto(&path),
                &mut Vec::new(),
                &crate::codec::proto_index::ProtoIndex::new(),
            )
            .unwrap();

        let wb_bid = Bid::new(Bid::nil());
        let bid_str = wb_bid.to_string();

        // Pre-populate schema.bid to match what would be in document["bid"].
        codec.schema.as_mut().unwrap().bid = Some(bid_str.clone());
        codec.nodes[0]
            .proto
            .document
            .insert("bid", toml_edit::value(bid_str.clone()));

        let mut diags = Vec::new();
        codec.finalize(&mut diags).unwrap();

        // updated should remain false (no change detected).
        assert!(
            !codec.updated,
            "finalize() must not set updated=true when schema.bid already matches"
        );
    }

    #[test]
    fn test_build_updated_schema_yaml_injects_bid() {
        let (_dir, path) = standard_fixture();
        let mut codec = XlsxCodec::new();
        codec
            .parse(
                "",
                make_proto(&path),
                &mut Vec::new(),
                &crate::codec::proto_index::ProtoIndex::new(),
            )
            .unwrap();

        let wb_bid = Bid::new(Bid::nil());
        codec.schema.as_mut().unwrap().bid = Some(wb_bid.to_string());

        // Re-open the workbook to pass to build_updated_schema_yaml.
        let bytes = std::fs::read(&path).unwrap();
        let mut source_wb: calamine::Xlsx<std::io::Cursor<Vec<u8>>> =
            calamine::open_workbook_from_rs(std::io::Cursor::new(bytes)).unwrap();

        let yaml = codec.build_updated_schema_yaml(&mut source_wb);

        assert!(
            yaml.contains("bid"),
            "build_updated_schema_yaml should inject 'bid' key; got:\n{yaml}"
        );
        assert!(
            yaml.contains(&wb_bid.to_string()),
            "build_updated_schema_yaml should inject the workbook BID value; got:\n{yaml}"
        );
        // Original content must be preserved.
        assert!(
            yaml.contains("Widget Project Items"),
            "build_updated_schema_yaml should preserve title; got:\n{yaml}"
        );
    }

    #[test]
    fn test_generate_source_bytes_injects_bid_into_index_tab_schema() {
        let (_dir, path) = standard_fixture();
        let mut codec = XlsxCodec::new();
        codec
            .parse(
                "",
                make_proto(&path),
                &mut Vec::new(),
                &crate::codec::proto_index::ProtoIndex::new(),
            )
            .unwrap();

        // Simulate inject_context + finalize by manually setting schema.bid and updated.
        let wb_bid = Bid::new(Bid::nil());
        codec.schema.as_mut().unwrap().bid = Some(wb_bid.to_string());
        codec.updated = true;

        let bytes = codec
            .generate_source_bytes()
            .expect("generate_source_bytes should return Some when updated=true");

        // Re-open the produced workbook and verify cell A1 of the index tab.
        let mut wb: calamine::Xlsx<std::io::Cursor<Vec<u8>>> =
            calamine::open_workbook_from_rs(std::io::Cursor::new(bytes)).unwrap();
        let sheet = wb
            .worksheet_range(INDEX_TAB)
            .expect("index tab must be present");
        let a1 = sheet
            .get((0, 0))
            .and_then(|c| match c {
                calamine::Data::String(s) => Some(s.clone()),
                _ => None,
            })
            .expect("cell A1 must be a string");

        assert!(
            a1.contains("bid"),
            "index tab cell A1 must contain 'bid' key; got:\n{a1}"
        );
        assert!(
            a1.contains(&wb_bid.to_string()),
            "index tab cell A1 must contain the workbook BID; got:\n{a1}"
        );
    }
    #[test]
    fn test_ignore_tab_emits_opaque_not_rows() {
        let schema_yaml = r#"title: "Widget Project"
tabs:
  - name: "Template"
    ignore: true
    schema:
      - col: "Title"
        role: title
  - name: "Items"
    schema:
      - col: "Title"
        role: title
"#;
        let index_rows: &[&[&str]] = &[&[schema_yaml]];
        let template_rows: &[&[&str]] = &[&["Title"], &["Template Row A"], &["Template Row B"]];
        let item_rows: &[&[&str]] = &[&["Title"], &["Widget Alpha"], &["Widget Beta"]];
        let (_dir, path) = build_xlsx(&[
            (INDEX_TAB, index_rows),
            ("Template", template_rows),
            ("Items", item_rows),
        ]);

        let mut codec = XlsxCodec::new();
        let mut diagnostics = Vec::new();
        codec
            .parse(
                "",
                make_proto(&path),
                &mut diagnostics,
                &crate::codec::proto_index::ProtoIndex::new(),
            )
            .unwrap();

        let nodes = codec.nodes();

        // "Template" tab: exactly one heading-3 container node, no heading-4 row nodes.
        let template_tabs: Vec<_> = nodes
            .iter()
            .filter(|n| n.title().as_deref() == Some("Template"))
            .collect();
        assert_eq!(
            template_tabs.len(),
            1,
            "ignored tab should emit exactly one container node"
        );
        assert_eq!(
            template_tabs[0].heading, 3,
            "ignored tab container should be heading 3"
        );

        let template_rows_emitted: Vec<_> = nodes
            .iter()
            .filter(|n| {
                n.heading == 4
                    && n.document.get("xlsx_tab").and_then(|v| v.as_str()) == Some("Template")
            })
            .collect();
        assert!(
            template_rows_emitted.is_empty(),
            "ignored tab must not emit any row nodes; got: {:?}",
            template_rows_emitted
                .iter()
                .filter_map(|n| n.title())
                .collect::<Vec<_>>()
        );

        // "Items" tab: two row nodes normally parsed.
        let item_rows_emitted: Vec<_> = nodes.iter().filter(|n| n.heading == 4).collect();
        assert_eq!(
            item_rows_emitted.len(),
            2,
            "Items tab should still emit 2 row nodes; got {}",
            item_rows_emitted.len()
        );

        assert!(
            diagnostics.is_empty(),
            "no diagnostics expected for valid ignore=true tab; got: {diagnostics:?}"
        );
    }

    /// `generate_html` with a wildcard schema must emit one Tabulator table per actual
    /// sheet name rather than a single table labelled `"*"` (which would produce an
    /// empty result because no `tab_name` is literally `"*"` after the F10 fix).
    #[test]
    fn test_generate_html_wildcard_emits_per_sheet_tables() {
        let schema_yaml = r#"title: "Widget Project"
tabs:
  - name: "*"
    schema:
      - col: "Title"
        role: title
      - col: "Description"
        role: text
"#;
        let index_rows: &[&[&str]] = &[&[schema_yaml]];
        let alpha_rows: &[&[&str]] = &[
            &["Title", "Description"],
            &["Widget Alpha", "Alpha description."],
            &["Widget Beta", "Beta description."],
        ];
        let bravo_rows: &[&[&str]] = &[
            &["Title", "Description"],
            &["Widget Gamma", "Gamma description."],
        ];
        let (_dir, path) = build_xlsx(&[
            (INDEX_TAB, index_rows),
            ("Alpha", alpha_rows),
            ("Bravo", bravo_rows),
        ]);

        let mut codec = XlsxCodec::new();
        let mut diagnostics = Vec::new();
        codec
            .parse(
                "",
                make_proto(&path),
                &mut diagnostics,
                &crate::codec::proto_index::ProtoIndex::new(),
            )
            .unwrap();
        assert!(
            diagnostics.is_empty(),
            "unexpected diagnostics: {diagnostics:?}"
        );

        let fragments = codec.generate_html().expect("generate_html should succeed");

        // Single workbook HTML file containing all tab sections inline.
        assert_eq!(
            fragments.len(),
            1,
            "expected exactly one workbook HTML fragment; got filenames: {:?}",
            fragments.iter().map(|(f, _, _)| f).collect::<Vec<_>>()
        );

        let (filename, pairs, layout) = &fragments[0];

        // Output filename must be the workbook stem + .html, never "*".
        assert!(
            filename.ends_with(".html"),
            "output filename must end with .html; got: {filename}"
        );
        assert!(
            !filename.contains('*'),
            "output filename must not contain '*'; got: {filename}"
        );

        // Layout must be XlsxWorkbook (dedicated workbook template).
        assert!(
            matches!(layout, Some(crate::codec::assets::Layout::XlsxWorkbook)),
            "workbook fragment must use Layout::XlsxWorkbook; got: {layout:?}"
        );

        // Collect pairs into a map for easy lookup.
        let pair_map: std::collections::HashMap<&str, &str> = pairs
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();

        // WORKBOOK_TITLE pair must carry the schema title.
        let workbook_title = pair_map.get("{{WORKBOOK_TITLE}}").copied().unwrap_or("");
        assert!(
            !workbook_title.is_empty(),
            "{{WORKBOOK_TITLE}} pair must be present and non-empty"
        );

        // TABS_NAV pair must contain entries for both real sheet names.
        let tabs_nav = pair_map.get("{{TABS_NAV}}").copied().unwrap_or("");
        assert!(
            tabs_nav.contains("Alpha"),
            "TABS_NAV should contain 'Alpha' entry; tabs_nav:\n{tabs_nav}"
        );
        assert!(
            tabs_nav.contains("Bravo"),
            "TABS_NAV should contain 'Bravo' entry; tabs_nav:\n{tabs_nav}"
        );
        // Nav must not contain a literal "*" entry.
        assert!(
            !tabs_nav.contains(">*<"),
            "TABS_NAV must not contain literal '*' entry; tabs_nav:\n{tabs_nav}"
        );

        // TAB_SECTIONS pair must contain a section for each real sheet.
        let tab_sections = pair_map.get("{{TAB_SECTIONS}}").copied().unwrap_or("");
        assert!(
            tab_sections.contains("Alpha"),
            "TAB_SECTIONS should contain 'Alpha' section; tab_sections:\n{tab_sections}"
        );
        assert!(
            tab_sections.contains("Bravo"),
            "TAB_SECTIONS should contain 'Bravo' section; tab_sections:\n{tab_sections}"
        );
        assert!(
            tab_sections.contains("class=\"xlsx-tab\""),
            "tab sections must have xlsx-tab class; tab_sections:\n{tab_sections}"
        );
        // Sections must NOT contain inline row data (that lives in XLSX_DATA now).
        assert!(
            !tab_sections.contains("Widget Alpha"),
            "TAB_SECTIONS must not contain inline row data; tab_sections:\n{tab_sections}"
        );

        // XLSX_DATA pair must be valid JSON containing row data for both sheets.
        let xlsx_data = pair_map.get("{{XLSX_DATA}}").copied().unwrap_or("");
        let parsed: serde_json::Value =
            serde_json::from_str(xlsx_data).expect("XLSX_DATA must be valid JSON");
        assert!(
            parsed.is_object(),
            "XLSX_DATA must be a JSON object; got:\n{xlsx_data}"
        );
        // Row data must appear somewhere in the JSON blob.
        assert!(
            xlsx_data.contains("Widget Alpha"),
            "XLSX_DATA should contain 'Widget Alpha' row data; xlsx_data:\n{xlsx_data}"
        );
        assert!(
            xlsx_data.contains("Widget Gamma"),
            "XLSX_DATA should contain 'Widget Gamma' row data; xlsx_data:\n{xlsx_data}"
        );
    }

    // ── interpolate_template ──────────────────────────────────────────────────

    #[test]
    fn test_interpolate_template_spaced_placeholders() {
        // Template authors may write {{ Statement }} (Jinja-style spaces inside
        // braces). The interpolator must trim the key before lookup.
        let schema_yaml = r#"title: "Widget Project"
tabs:
  - name: "Items"
    text_template: "{{ Statement }}\n\n**Rationale**: {{ Source/Rationale }}"
    schema:
      - col: "Statement"
        wrap: true
      - col: "Source/Rationale"
        wrap: true
"#;
        let index_rows: &[&[&str]] = &[&[schema_yaml]];
        let data_rows: &[&[&str]] = &[
            &["Title", "Statement", "Source/Rationale"],
            &["Req 1", "The system shall do X.", "Because Y."],
            &["Req 2", "The system shall do Z.", "Because W."],
        ];
        let (_dir, path) = build_xlsx(&[(INDEX_TAB, index_rows), ("Items", data_rows)]);

        let mut codec = XlsxCodec::new();
        let mut diagnostics = Vec::new();
        codec
            .parse(
                "",
                make_proto(&path),
                &mut diagnostics,
                &crate::codec::proto_index::ProtoIndex::new(),
            )
            .unwrap();
        assert!(
            diagnostics.is_empty(),
            "unexpected diagnostics: {diagnostics:?}"
        );

        let nodes = codec.nodes();
        let row_nodes: Vec<_> = nodes.iter().filter(|n| n.heading == 4).collect();
        assert_eq!(row_nodes.len(), 2, "expected 2 row nodes");

        // Verify the text field contains the interpolated template, not a bare placeholder.
        let text0 = row_nodes[0]
            .document
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        assert!(
            text0.contains("The system shall do X."),
            "row 0 text should contain Statement value; got: {text0:?}"
        );
        assert!(
            text0.contains("Because Y."),
            "row 0 text should contain Source/Rationale value; got: {text0:?}"
        );
        assert!(
            !text0.contains("{{"),
            "row 0 text must not contain unresolved placeholders; got: {text0:?}"
        );
    }

    #[test]
    fn test_interpolate_template_slash_in_key() {
        // Keys containing slashes (e.g. "Source/Rationale") must round-trip through
        // the normaliser correctly: slash is preserved, spaces are replaced with "_".
        let mut row_values = HashMap::new();
        row_values.insert("Source/Rationale".to_string(), "Because Y.".to_string());

        let result = XlsxCodec::interpolate_template("{{ Source/Rationale }}", &row_values);
        assert_eq!(
            result, "Because Y.",
            "slash in key should resolve correctly"
        );
    }

    #[test]
    fn test_interpolate_template_case_insensitive_spaced() {
        // A placeholder like {{ statement }} (lowercase, spaces) should match
        // a header "Statement" (title-case, no spaces).
        let mut row_values = HashMap::new();
        row_values.insert("Statement".to_string(), "Alpha requirement.".to_string());

        let result = XlsxCodec::interpolate_template("{{ statement }}", &row_values);
        assert_eq!(result, "Alpha requirement.");
    }

    // ── Semantic empty-row skip ───────────────────────────────────────────────

    #[test]
    fn test_text_template_empty_rows_skipped_when_content_blank() {
        // Rows where all template-relevant columns are blank must be skipped.
        // The title column alone is non-empty — without a text body the row
        // has no meaningful content for a template-driven tab.
        let schema_yaml = r#"title: "Widget Project"
tabs:
  - name: "Items"
    text_template: "{{ Statement }}\n\n**Rationale**: {{ Source/Rationale }}"
    schema:
      - col: "Statement"
        wrap: true
      - col: "Source/Rationale"
        wrap: true
"#;
        let index_rows: &[&[&str]] = &[&[schema_yaml]];
        // Row 1 has content; Row 2 has only a title (both template columns blank).
        let data_rows: &[&[&str]] = &[
            &["Title", "Statement", "Source/Rationale"],
            &["Req 1", "Do X.", "Because Y."],
            &["Req 2", "", ""], // title only — should be skipped
        ];
        let (_dir, path) = build_xlsx(&[(INDEX_TAB, index_rows), ("Items", data_rows)]);

        let mut codec = XlsxCodec::new();
        let mut diagnostics = Vec::new();
        codec
            .parse(
                "",
                make_proto(&path),
                &mut diagnostics,
                &crate::codec::proto_index::ProtoIndex::new(),
            )
            .unwrap();

        let nodes = codec.nodes();
        let row_nodes: Vec<_> = nodes.iter().filter(|n| n.heading == 4).collect();
        assert_eq!(
            row_nodes.len(),
            1,
            "only the row with non-blank template content should be emitted; \
             got titles: {:?}",
            row_nodes
                .iter()
                .filter_map(|n| n.title())
                .collect::<Vec<_>>()
        );
        assert_eq!(
            row_nodes[0].title().as_deref(),
            Some("Req 1"),
            "the emitted row should be Req 1"
        );
    }

    #[test]
    fn test_text_template_empty_row_with_title_emits_diagnostic() {
        // A row that has a non-empty title but blank template columns must be
        // skipped AND emit a warning so the author knows data was discarded.
        let schema_yaml = r#"title: "Widget Project"
tabs:
  - name: "Items"
    text_template: "{{ Statement }}\n\n**Rationale**: {{ Source/Rationale }}"
    schema:
      - col: "Statement"
        wrap: true
      - col: "Source/Rationale"
        wrap: true
"#;
        let index_rows: &[&[&str]] = &[&[schema_yaml]];
        let data_rows: &[&[&str]] = &[
            &["Title", "Statement", "Source/Rationale"],
            &["Req 1", "Do X.", "Because Y."],
            // Req 2 has a title but both template columns are blank — should
            // be skipped with a warning.
            &["Req 2", "", ""],
        ];
        let (_dir, path) = build_xlsx(&[(INDEX_TAB, index_rows), ("Items", data_rows)]);

        let mut codec = XlsxCodec::new();
        let mut diagnostics = Vec::new();
        codec
            .parse(
                "",
                make_proto(&path),
                &mut diagnostics,
                &crate::codec::proto_index::ProtoIndex::new(),
            )
            .unwrap();

        // Only Req 1 should produce a row node.
        let nodes = codec.nodes();
        let row_nodes: Vec<_> = nodes.iter().filter(|n| n.heading == 4).collect();
        assert_eq!(
            row_nodes.len(),
            1,
            "only the row with non-blank template content should be emitted"
        );
        assert_eq!(row_nodes[0].title().as_deref(), Some("Req 1"));

        // A warning must have been emitted for Req 2.
        assert!(
            !diagnostics.is_empty(),
            "expected a diagnostic for the skipped row with a non-empty title"
        );
        let msg = match &diagnostics[0] {
            ParseDiagnostic::Warning { message, .. } => message.clone(),
            other => panic!("expected Warning diagnostic, got: {other:?}"),
        };
        assert!(
            msg.contains("row 2") || msg.contains("skipped"),
            "diagnostic should identify the skipped row; got: {msg:?}"
        );
        assert!(
            msg.contains("Req 2"),
            "diagnostic should include the row title; got: {msg:?}"
        );
        assert!(
            msg.contains("text_template"),
            "diagnostic should mention text_template; got: {msg:?}"
        );
    }

    #[test]
    fn test_text_template_row_with_content_not_skipped() {
        // Sanity: a row whose template renders to a non-empty, non-default string
        // must NOT be skipped, even when some columns are blank.
        let schema_yaml = r#"title: "Widget Project"
tabs:
  - name: "Items"
    text_template: "{{ Statement }}"
    schema:
      - col: "Statement"
        wrap: true
      - col: "Source/Rationale"
        wrap: true
"#;
        let index_rows: &[&[&str]] = &[&[schema_yaml]];
        let data_rows: &[&[&str]] = &[
            &["Title", "Statement", "Source/Rationale"],
            &["Req 1", "Do X.", ""], // rationale blank, statement present
        ];
        let (_dir, path) = build_xlsx(&[(INDEX_TAB, index_rows), ("Items", data_rows)]);

        let mut codec = XlsxCodec::new();
        codec
            .parse(
                "",
                make_proto(&path),
                &mut Vec::new(),
                &crate::codec::proto_index::ProtoIndex::new(),
            )
            .unwrap();

        let nodes = codec.nodes();
        let row_nodes: Vec<_> = nodes.iter().filter(|n| n.heading == 4).collect();
        assert_eq!(
            row_nodes.len(),
            1,
            "row with partial content must not be skipped"
        );

        let text = row_nodes[0]
            .document
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        assert!(
            text.contains("Do X."),
            "text should contain Statement value"
        );
    }

    // ── column_map tests ─────────────────────────────────────────────────────

    #[test]
    fn test_build_column_map_reserved_detection() {
        // Verify that __noet_bid__, __noet_id__, and __noet_schema__ are detected
        // as hidden Payload entries with the correct ir_keys, even when the tab
        // schema declares no explicit columns.
        let schema_yaml = r#"title: "Widget Project"
tabs:
  - name: "Items"
    schema: []
"#;
        let index_rows: &[&[&str]] = &[&[schema_yaml]];
        let data_rows: &[&[&str]] = &[
            &["Title", "__noet_bid__", "__noet_id__", "__noet_schema__"],
            &["Row 1", "", "", ""],
        ];
        let (_dir, path) = build_xlsx(&[(INDEX_TAB, index_rows), ("Items", data_rows)]);
        let mut codec = XlsxCodec::new();
        codec
            .parse(
                "",
                make_proto(&path),
                &mut Vec::new(),
                &crate::codec::proto_index::ProtoIndex::new(),
            )
            .unwrap();

        let col_map = codec
            .column_maps
            .get("Items")
            .expect("Items column map must exist");

        let bid_entry = col_map
            .iter()
            .find(|e| e.ir_key == "bid")
            .expect("bid entry missing");
        assert!(bid_entry.hidden, "bid entry must be hidden");
        assert!(
            matches!(bid_entry.kind, ColumnKind::Payload),
            "bid kind must be Payload"
        );

        let id_entry = col_map
            .iter()
            .find(|e| e.ir_key == "id")
            .expect("id entry missing");
        assert!(id_entry.hidden, "id entry must be hidden");
        assert!(
            matches!(id_entry.kind, ColumnKind::Payload),
            "id kind must be Payload"
        );

        let schema_entry = col_map
            .iter()
            .find(|e| e.ir_key == "schema")
            .expect("schema entry missing");
        assert!(schema_entry.hidden, "schema entry must be hidden");
        assert!(
            matches!(schema_entry.kind, ColumnKind::Payload),
            "schema kind must be Payload"
        );
    }

    #[test]
    fn test_build_column_map_conventional_and_positional() {
        // Verify layer-2 conventional name matching: "Title" → ColumnKind::Title,
        // "Text" → ColumnKind::Markdown, and an unrecognised header → Payload.
        // No explicit schema declarations are present.
        let schema_yaml = r#"title: "Widget Project"
tabs:
  - name: "Items"
    schema: []
"#;
        let index_rows: &[&[&str]] = &[&[schema_yaml]];
        let data_rows: &[&[&str]] = &[&["Title", "Text", "Notes"], &["Row 1", "Body 1", "Note A"]];
        let (_dir, path) = build_xlsx(&[(INDEX_TAB, index_rows), ("Items", data_rows)]);
        let mut codec = XlsxCodec::new();
        codec
            .parse(
                "",
                make_proto(&path),
                &mut Vec::new(),
                &crate::codec::proto_index::ProtoIndex::new(),
            )
            .unwrap();

        let col_map = codec
            .column_maps
            .get("Items")
            .expect("Items column map must exist");

        let title_entry = col_map
            .iter()
            .find(|e| e.ir_key == "title")
            .expect("title entry missing");
        assert!(
            matches!(title_entry.kind, ColumnKind::Title),
            "Title column must have Title kind"
        );
        assert!(!title_entry.hidden, "Title column must not be hidden");

        let text_entry = col_map
            .iter()
            .find(|e| e.ir_key == "text")
            .expect("text entry missing");
        assert!(
            matches!(text_entry.kind, ColumnKind::Markdown),
            "Text column must have Markdown kind"
        );
        assert!(!text_entry.hidden, "Text column must not be hidden");

        // "Notes" has no conventional match → must fall through to Payload.
        let notes_entry = col_map
            .iter()
            .find(|e| e.header == "Notes")
            .expect("Notes entry missing");
        assert!(
            matches!(notes_entry.kind, ColumnKind::Payload),
            "Notes column must be Payload"
        );
        assert!(!notes_entry.hidden, "Notes column must not be hidden");
    }

    #[test]
    fn test_relation_bref_column_round_trip() {
        // Step 1: parse a workbook with a relation column and confirm a Relation
        // entry is produced in the column map.
        let schema_yaml = r#"title: "Widget Project"
tabs:
  - name: "Items"
    schema:
      - col: "Subsystem"
        role: relation
        key: id
"#;
        let index_rows: &[&[&str]] = &[&[schema_yaml]];
        let data_rows: &[&[&str]] = &[&["Title", "Subsystem"], &["Row 1", "power-manager"]];
        let (_dir, path) = build_xlsx(&[(INDEX_TAB, index_rows), ("Items", data_rows)]);
        let mut codec = XlsxCodec::new();
        codec
            .parse(
                "",
                make_proto(&path),
                &mut Vec::new(),
                &crate::codec::proto_index::ProtoIndex::new(),
            )
            .unwrap();

        let col_map = codec
            .column_maps
            .get("Items")
            .expect("Items column map must exist");
        let rel_entry = col_map
            .iter()
            .find(|e| matches!(e.kind, ColumnKind::Relation { .. }))
            .expect("Relation entry must exist for Subsystem column");
        // to_anchor("Subsystem") == "subsystem"
        assert_eq!(rel_entry.ir_key, "subsystem");

        // Step 2: build a workbook that already carries the __noet_relation_subsystem__
        // column (simulating what write_annotated_sheet produces after inject_context).
        let data_rows_with_bref: &[&[&str]] = &[
            &["Title", "Subsystem", "__noet_relation_subsystem__"],
            &["Row 1", "power-manager", "abc123def456"],
        ];
        let (_dir2, path2) = build_xlsx(&[(INDEX_TAB, index_rows), ("Items", data_rows_with_bref)]);
        let mut codec2 = XlsxCodec::new();
        codec2
            .parse(
                "",
                make_proto(&path2),
                &mut Vec::new(),
                &crate::codec::proto_index::ProtoIndex::new(),
            )
            .unwrap();

        let col_map2 = codec2
            .column_maps
            .get("Items")
            .expect("Items column map must exist on second parse");
        let bref_entry = col_map2
            .iter()
            .find(|e| {
                matches!(
                    &e.kind,
                    ColumnKind::RelationBref { relation_ir_key }
                        if relation_ir_key == "subsystem"
                )
            })
            .expect("RelationBref entry must exist after write-back");
        assert!(bref_entry.hidden, "RelationBref column must be hidden");
    }
}
