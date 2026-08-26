//! XLSX codec integration tests.
//!
//! These tests verify that `XlsxCodec` is correctly wired into the full compilation
//! pipeline — from `ProtoIndex` discovery through `parse_sequential` to `BeliefBase`
//! population.
//!
//! Three scenarios are covered:
//!
//! 1. **Schema xlsx** (`requirements.xlsx`): a workbook with a valid `index` tab schema.
//!    After compilation, the `BeliefBase` must contain the correct node hierarchy:
//!    one workbook node, one tab node per declared tab, and one row node per data row.
//!
//! 2. **No-index xlsx** (`raw_export.xlsx`): a workbook with no `index` tab.
//!    `proto()` returns `None` — the file is treated as a binary asset. No document
//!    nodes are emitted; the file must appear in the asset map, not the document graph.
//!
//! 3. **BID persistence**: after a `write=true` compile, the workbook and tab container
//!    BIDs are stored in the schema YAML in cell A1. On re-parse they are read back so
//!    `speculative_path_key` resolves stably via `NodeKey::Bid` (no time-based drift).
//!
//! ## Feature gates
//!
//! These tests require both `service` (for `parse_sequential` / `BeliefBase`) and
//! `xlsx` (for `XlsxCodec` and its dependencies). They are skipped entirely in
//! non-xlsx builds, so the default `cargo test` suite is unaffected.

#![cfg(all(feature = "service", feature = "xlsx"))]

use std::{io::Cursor, path::PathBuf};

use calamine::{open_workbook_from_rs, Reader};
use noet_core::{
    beliefbase::BeliefBase, codec::DocumentCompiler, error::BuildonomyError, event::BeliefEvent,
    properties::BeliefKind,
};
use rust_xlsxwriter::Workbook as XlsxWriterWorkbook;
use tempfile::TempDir;
use tokio::sync::mpsc::unbounded_channel;

// ── Fixture builders ─────────────────────────────────────────────────────────

/// YAML schema for the `index` tab of the schema fixture workbook.
///
/// Note: no `id` column is declared. Tabs contain a column literally named
/// "ID" (see `build_test_network`), but it is not listed here. The codec's
/// layer-3/4 default used to silently promote it to `ir_key = "id"`, which
/// caused cross-sheet collisions when two tabs shared the same cell values.
/// After the `RESERVED_ROW_IR_KEYS` fix, implicit "ID" columns are renamed
/// to `ir_key = "id_col"` (regular payload), and the mechanical default
/// (`{workbook_prefix}-{tab_slug}-{row_number}`) generates the node id.
const SCHEMA_YAML: &str = r#"title: "Widget Project Items"
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
"#;

/// Build an xlsx file and write it to `path`.
fn write_xlsx(path: &std::path::Path, sheets: &[(&str, &[&[&str]])]) {
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
    wb.save(path).expect("write xlsx fixture");
}

/// Create a minimal noet network root in `dir` with an `index.md`.
fn write_index_md(dir: &std::path::Path) {
    std::fs::write(
        dir.join("index.md"),
        "---\ntitle = \"xlsx integration test network\"\n---\n\n````{network_children}\n````\n",
    )
    .expect("write index.md");
}

/// Build the full test directory tree and return `(TempDir, network_root_path)`.
///
/// Layout:
/// ```
/// <tmp>/
///   network_root/
///     index.md                ← noet network root
///     items.xlsx              ← schema-declared workbook (should produce nodes)
///     raw_export.xlsx         ← no index tab (should be treated as asset)
/// ```
fn build_test_network() -> (TempDir, PathBuf) {
    let tmp = TempDir::new().expect("tempdir");
    let root = tmp.path().join("network_root");
    std::fs::create_dir_all(&root).expect("create network root");

    write_index_md(&root);

    // items.xlsx — schema-declared workbook.
    // Both tabs have an "ID" column with deliberately overlapping values
    // ("1.0", "2.0") to exercise cross-sheet collision handling.
    write_xlsx(
        &root.join("items.xlsx"),
        &[
            ("index", &[&[SCHEMA_YAML]]),
            (
                "Items",
                &[
                    &["ID", "Title", "Description", "Category"],
                    &[
                        "1.0",
                        "Widget Alpha",
                        "The system shall provide widget alpha.",
                        "Category A",
                    ],
                    &[
                        "2.0",
                        "Widget Beta",
                        "The system shall provide widget beta.",
                        "Category B",
                    ],
                    &[
                        "3.0",
                        "Widget Gamma",
                        "The system shall provide widget gamma.",
                        "Category A",
                    ],
                ],
            ),
            (
                "Measurements",
                &[
                    &["ID", "Title", "Description"],
                    &[
                        "1.0",
                        "Measurement Alpha",
                        "Measure the alpha widget output.",
                    ],
                    &["2.0", "Measurement Beta", "Measure the beta widget output."],
                ],
            ),
        ],
    );

    // raw_export.xlsx — no index tab, should be treated as binary asset
    write_xlsx(
        &root.join("raw_export.xlsx"),
        &[(
            "Sheet1",
            &[
                &["Col A", "Col B", "Col C"],
                &["val1", "val2", "val3"],
                &["val4", "val5", "val6"],
            ],
        )],
    );

    (tmp, root)
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Run `parse_sequential` on `root` and return the populated `BeliefBase`.
async fn compile(root: &std::path::Path) -> Result<BeliefBase, BuildonomyError> {
    let (tx, mut rx) = unbounded_channel::<BeliefEvent>();
    let mut global_bb = BeliefBase::empty();
    let mut compiler = DocumentCompiler::new(root, Some(tx), None, false)?;

    compiler
        .parse_sequential(&mut global_bb, false, Some(&mut rx))
        .await?;

    Ok(global_bb)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// Schema-declared xlsx produces the correct node hierarchy:
///   1 workbook node (Document, heading=2)
///   2 tab nodes (heading=3)
///   3 + 2 = 5 row nodes (heading=4)
#[test_log::test(tokio::test)]
async fn test_schema_xlsx_produces_node_hierarchy() -> Result<(), Box<dyn std::error::Error>> {
    let (_tmp, root) = build_test_network();
    let bb = compile(&root).await?;

    // Collect all non-network, non-API nodes by title for inspection.
    let states = bb.states();

    let workbook_nodes: Vec<_> = states
        .values()
        .filter(|n| n.kind.contains(BeliefKind::Document) && n.title == "Widget Project Items")
        .collect();
    assert_eq!(
        workbook_nodes.len(),
        1,
        "expected exactly one workbook node with title 'Widget Project Items', got {}",
        workbook_nodes.len()
    );

    // Tab nodes: titled "Items" and "Measurements"
    let items_tab: Vec<_> = states.values().filter(|n| n.title == "Items").collect();
    assert_eq!(
        items_tab.len(),
        1,
        "expected one 'Items' tab node, got {}",
        items_tab.len()
    );

    let meas_tab: Vec<_> = states
        .values()
        .filter(|n| n.title == "Measurements")
        .collect();
    assert_eq!(
        meas_tab.len(),
        1,
        "expected one 'Measurements' tab node, got {}",
        meas_tab.len()
    );

    // Row nodes
    let row_titles = [
        "Widget Alpha",
        "Widget Beta",
        "Widget Gamma",
        "Measurement Alpha",
        "Measurement Beta",
    ];
    for title in &row_titles {
        let matches: Vec<_> = states.values().filter(|n| n.title == *title).collect();
        assert_eq!(
            matches.len(),
            1,
            "expected exactly one node with title '{title}', got {}",
            matches.len()
        );
    }

    Ok(())
}

/// Schema-declared xlsx row nodes carry the correct `BeliefKind::Symbol` (not Document).
#[test_log::test(tokio::test)]
async fn test_schema_xlsx_row_nodes_are_symbol_kind() -> Result<(), Box<dyn std::error::Error>> {
    let (_tmp, root) = build_test_network();
    let bb = compile(&root).await?;

    let row_node = bb
        .states()
        .values()
        .find(|n| n.title == "Widget Alpha")
        .expect("Widget Alpha row node should exist");

    assert!(
        row_node.kind.contains(BeliefKind::Symbol),
        "row node should have BeliefKind::Symbol, got {:?}",
        row_node.kind
    );
    assert!(
        !row_node.kind.contains(BeliefKind::Document),
        "row node should NOT have BeliefKind::Document"
    );

    Ok(())
}

/// Schema-declared xlsx row nodes carry category as a payload field (role: payload).
#[test_log::test(tokio::test)]
async fn test_schema_xlsx_category_payload() -> Result<(), Box<dyn std::error::Error>> {
    let (_tmp, root) = build_test_network();
    let bb = compile(&root).await?;

    let row_node = bb
        .states()
        .values()
        .find(|n| n.title == "Widget Alpha")
        .expect("Widget Alpha row node should exist");

    let category = row_node
        .payload
        .get("category")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    assert_eq!(
        category, "Category A",
        "expected category payload 'Category A' on 'Widget Alpha', got {:?}",
        category
    );

    Ok(())
}

/// No-index xlsx (`raw_export.xlsx`) must not produce any document nodes.
/// The workbook node count should be zero for that file.
#[test_log::test(tokio::test)]
async fn test_no_index_xlsx_produces_no_document_nodes() -> Result<(), Box<dyn std::error::Error>> {
    let (_tmp, root) = build_test_network();
    let bb = compile(&root).await?;

    // raw_export.xlsx has no index tab, so proto() returns None — no xlsx nodes at all.
    // Confirm no node carries an xlsx_tab value from Sheet1 (the only sheet in that file).
    // We detect xlsx-originated nodes by the xlsx_tab payload key and exclude tab names
    // from our schema workbook (items.xlsx), which are "Items" and "Measurements".
    let unexpected_xlsx_nodes: Vec<_> = bb
        .states()
        .values()
        .filter(|n| {
            matches!(
                n.payload.get("xlsx_tab").and_then(|v| v.as_str()),
                Some(tab) if !["Items", "Measurements"].contains(&tab)
            )
        })
        .collect();

    assert!(
        unexpected_xlsx_nodes.is_empty(),
        "no-index xlsx should produce no document nodes, but found nodes with unexpected xlsx_tab values: {:?}",
        unexpected_xlsx_nodes
            .iter()
            .map(|n| (n.title.as_str(), n.payload.get("xlsx_tab").and_then(|v| v.as_str()).unwrap_or("?")))
            .collect::<Vec<_>>()
    );

    Ok(())
}

/// No-index xlsx is not routed as a corpus asset by the current pipeline.
///
/// When `proto()` returns `None` for an xlsx file (no `index` tab), `initialize_stack`
/// in `GraphBuilder` errors with "Codec could not resolve path into a proto node".
/// The compiler logs a WARN and records a parse-error diagnostic, but the file is
/// never inserted into the asset map.
///
/// This is a known architectural limitation (AGENTS review finding F2-adjacent):
/// a binary codec that returns `None` from `proto()` should fall back to the asset
/// pipeline the same way a file with no registered codec does. Fixing this requires
/// either a fallback in `parse_one_path`/`initialize_stack` or a sentinel value from
/// `proto()` that distinguishes "not my format" from "my format but no content".
///
/// For now we verify only that no xlsx-originated document nodes are emitted for
/// the no-index file (covered by `test_no_index_xlsx_produces_no_document_nodes`).
/// The asset-map routing test is deferred until the pipeline fallback is implemented.
#[test_log::test(tokio::test)]
async fn test_no_index_xlsx_parse_error_produces_no_nodes() -> Result<(), Box<dyn std::error::Error>>
{
    let (_tmp, root) = build_test_network();
    let bb = compile(&root).await?;

    // Confirm raw_export.xlsx produced no document nodes in the belief graph.
    // (It fails with a parse error, so no nodes are emitted at all.)
    let raw_nodes: Vec<_> = bb
        .states()
        .values()
        .filter(|n| {
            // raw_export.xlsx only has "Sheet1" — if any node has xlsx_tab == "Sheet1"
            // it came from that file.
            n.payload
                .get("xlsx_tab")
                .and_then(|v| v.as_str())
                .map(|t| t == "Sheet1")
                .unwrap_or(false)
        })
        .collect();

    assert!(
        raw_nodes.is_empty(),
        "raw_export.xlsx (no index tab) must not produce any document nodes; \
         found: {:?}",
        raw_nodes
            .iter()
            .map(|n| n.title.as_str())
            .collect::<Vec<_>>()
    );

    Ok(())
}

/// Compiling the same network twice produces no rewrites on the second pass
/// (BIDs are stable after write-back).
///
/// Also verifies BID identity: the workbook and tab nodes in parse 2 have the
/// same BIDs as in parse 1 — confirming that `schema.bid` and `schema.tabs_meta`
/// round-trip correctly through the cell A1 schema YAML.
#[test_log::test(tokio::test)]
async fn test_schema_xlsx_stable_on_second_parse() -> Result<(), Box<dyn std::error::Error>> {
    let (_tmp, root) = build_test_network();

    // Parse 1 — write BIDs back into the xlsx (write=true via DocumentCompiler::new).
    let (wb_bid_parse1, tab_bid_parse1) = {
        let (tx, mut rx) = unbounded_channel::<BeliefEvent>();
        let mut global_bb = BeliefBase::empty();
        let mut compiler = DocumentCompiler::new(&root, Some(tx), None, true)?;
        compiler
            .parse_sequential(&mut global_bb, false, Some(&mut rx))
            .await?;

        let states = global_bb.states();

        let wb_bid = states
            .values()
            .find(|n| n.kind.contains(BeliefKind::Document) && n.title == "Widget Project Items")
            .map(|n| n.bid)
            .expect("workbook node should exist after parse 1");

        let tab_bid = states
            .values()
            .find(|n| n.title == "Items")
            .map(|n| n.bid)
            .expect("Items tab node should exist after parse 1");

        (wb_bid, tab_bid)
    };

    // Parse 2 — BIDs already in the xlsx; expect no rewrites and identical BIDs.
    {
        let (tx, mut rx) = unbounded_channel::<BeliefEvent>();
        let mut global_bb = BeliefBase::empty();
        let mut compiler = DocumentCompiler::new(&root, Some(tx), None, false)?;
        let results = compiler
            .parse_sequential(&mut global_bb, false, Some(&mut rx))
            .await?;

        let xlsx_rewrites: Vec<_> = results
            .iter()
            .filter(|r| {
                r.path
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e == "xlsx")
                    .unwrap_or(false)
                    && r.rewritten_content.is_some()
            })
            .collect();

        assert!(
            xlsx_rewrites.is_empty(),
            "parse 2 should not rewrite xlsx files (BIDs already stable), \
             but got rewrites for: {:?}",
            xlsx_rewrites.iter().map(|r| &r.path).collect::<Vec<_>>()
        );

        // Verify BID identity across parses.
        let states = global_bb.states();

        let wb_bid_parse2 = states
            .values()
            .find(|n| n.kind.contains(BeliefKind::Document) && n.title == "Widget Project Items")
            .map(|n| n.bid)
            .expect("workbook node should exist after parse 2");

        assert_eq!(
            wb_bid_parse1, wb_bid_parse2,
            "workbook BID must be stable across parses (schema.bid round-trip)"
        );

        let tab_bid_parse2 = states
            .values()
            .find(|n| n.title == "Items")
            .map(|n| n.bid)
            .expect("Items tab node should exist after parse 2");

        assert_eq!(
            tab_bid_parse1, tab_bid_parse2,
            "tab container BID must be stable across parses (schema.tabs_meta round-trip)"
        );
    }

    Ok(())
}

/// After `write=true`, cell A1 of the index tab must contain a TOML `bid` key
/// matching the workbook node's BID.
#[test_log::test(tokio::test)]
async fn test_schema_xlsx_workbook_bid_persisted_in_schema_yaml(
) -> Result<(), Box<dyn std::error::Error>> {
    let (_tmp, root) = build_test_network();

    // Compile with write=true (passed via DocumentCompiler::new) so generate_source_bytes() runs.
    let wb_bid = {
        let (tx, mut rx) = unbounded_channel::<BeliefEvent>();
        let mut global_bb = BeliefBase::empty();
        let mut compiler = DocumentCompiler::new(&root, Some(tx), None, true)?;
        compiler
            .parse_sequential(&mut global_bb, false, Some(&mut rx))
            .await?;

        global_bb
            .states()
            .values()
            .find(|n| n.kind.contains(BeliefKind::Document) && n.title == "Widget Project Items")
            .map(|n| n.bid)
            .expect("workbook node must exist")
    };

    // Re-open the written xlsx and read cell A1.
    let xlsx_path = root.join("items.xlsx");
    let bytes = std::fs::read(&xlsx_path)?;
    let mut wb: calamine::Xlsx<Cursor<Vec<u8>>> =
        open_workbook_from_rs(Cursor::new(bytes)).expect("re-open written xlsx");
    let sheet = wb
        .worksheet_range("index")
        .expect("index tab must be present");
    let a1 = sheet
        .get((0, 0))
        .and_then(|c| {
            if let calamine::Data::String(s) = c {
                Some(s.clone())
            } else {
                None
            }
        })
        .expect("cell A1 must be a string");

    // The schema YAML must contain a `bid` key matching the workbook BID.
    assert!(
        a1.contains("bid"),
        "cell A1 must contain a 'bid' key after write; got:\n{a1}"
    );
    assert!(
        a1.contains(&wb_bid.to_string()),
        "cell A1 must contain the workbook BID '{}'; got:\n{a1}",
        wb_bid
    );

    // Original fields must be preserved.
    assert!(
        a1.contains("Widget Project Items"),
        "cell A1 must still contain the original title; got:\n{a1}"
    );

    Ok(())
}

/// After `write=true`, cell A1 of the index tab must contain `tabs_meta.<tab>.bid`
/// entries matching each tab container node's BID.
#[test_log::test(tokio::test)]
async fn test_schema_xlsx_tab_bids_persisted_in_tabs_meta() -> Result<(), Box<dyn std::error::Error>>
{
    let (_tmp, root) = build_test_network();

    // Compile with write=true (passed via DocumentCompiler::new) so generate_source_bytes() runs.
    let (items_bid, meas_bid) = {
        let (tx, mut rx) = unbounded_channel::<BeliefEvent>();
        let mut global_bb = BeliefBase::empty();
        let mut compiler = DocumentCompiler::new(&root, Some(tx), None, true)?;
        compiler
            .parse_sequential(&mut global_bb, false, Some(&mut rx))
            .await?;

        let states = global_bb.states();

        let items_bid = states
            .values()
            .find(|n| n.title == "Items")
            .map(|n| n.bid)
            .expect("Items tab node must exist");

        let meas_bid = states
            .values()
            .find(|n| n.title == "Measurements")
            .map(|n| n.bid)
            .expect("Measurements tab node must exist");

        (items_bid, meas_bid)
    };

    // Re-open the written xlsx and read cell A1.
    let xlsx_path = root.join("items.xlsx");
    let bytes = std::fs::read(&xlsx_path)?;
    let mut wb: calamine::Xlsx<Cursor<Vec<u8>>> =
        open_workbook_from_rs(Cursor::new(bytes)).expect("re-open written xlsx");
    let sheet = wb
        .worksheet_range("index")
        .expect("index tab must be present");
    let a1 = sheet
        .get((0, 0))
        .and_then(|c| {
            if let calamine::Data::String(s) = c {
                Some(s.clone())
            } else {
                None
            }
        })
        .expect("cell A1 must be a string");

    // tabs_meta section must be present.
    assert!(
        a1.contains("tabs_meta"),
        "cell A1 must contain 'tabs_meta' after write; got:\n{a1}"
    );

    // Each tab BID must appear in the cell.
    assert!(
        a1.contains(&items_bid.to_string()),
        "cell A1 must contain the Items tab BID '{}'; got:\n{a1}",
        items_bid
    );
    assert!(
        a1.contains(&meas_bid.to_string()),
        "cell A1 must contain the Measurements tab BID '{}'; got:\n{a1}",
        meas_bid
    );

    Ok(())
}

/// Two sheets with a column named "ID" containing duplicate values ("1.0", "2.0")
/// must not panic during compilation and must produce distinct nodes per sheet.
///
/// Regression test for the stale-parsed-BIDs panic: before the fix, the "ID"
/// column was implicitly promoted to `ir_key = "id"`, causing cross-sheet
/// `NodeKey::Id` and `NodeKey::Path` collisions. `insert_state`'s `to_replace`
/// loop absorbed the first sheet's node through a stale path key, removing it
/// from `states`. Phase 4's `get_context` then panicked on the orphaned BID.
///
/// The fix has two parts:
///   1. `build_column_map` renames implicit "ID" columns to `ir_key = "id_col"`
///      so the mechanical default ID (`{prefix}-{tab}-{row}`) fires.
///   2. `push()` FIRST-ONE-WINS now updates `NodeKey::Path` keys (not just
///      `NodeKey::Id`), preventing `insert_state` from absorbing the winner
///      through path-key matching.
#[test_log::test(tokio::test)]
async fn test_cross_sheet_duplicate_id_column_no_panic() -> Result<(), Box<dyn std::error::Error>> {
    let (_tmp, root) = build_test_network();
    let bb = compile(&root).await?;

    let states = bb.states();

    // All 5 row nodes must exist — no absorption across sheets.
    let row_titles = [
        "Widget Alpha",
        "Widget Beta",
        "Widget Gamma",
        "Measurement Alpha",
        "Measurement Beta",
    ];
    for title in &row_titles {
        let matches: Vec<_> = states.values().filter(|n| n.title == *title).collect();
        assert_eq!(
            matches.len(),
            1,
            "expected exactly one node with title '{title}' (cross-sheet collision \
             should not absorb nodes), got {}",
            matches.len()
        );
    }

    // Every row node must have a distinct BID.
    let row_bids: Vec<_> = row_titles
        .iter()
        .map(|title| states.values().find(|n| n.title == *title).unwrap().bid)
        .collect();
    let unique_bids: std::collections::HashSet<_> = row_bids.iter().collect();
    assert_eq!(
        unique_bids.len(),
        row_bids.len(),
        "all row BIDs must be distinct; got {:?}",
        row_bids
    );

    // Node IDs must use the mechanical format (tab-scoped), not the raw "ID"
    // column values. The "ID" column values ("1.0", "2.0", "3.0") should NOT
    // appear as the `id` field — the mechanical format includes the tab slug.
    let alpha_node = states.values().find(|n| n.title == "Widget Alpha").unwrap();
    let alpha_id = &alpha_node.id;
    assert!(
        !alpha_id.to_string().contains("10"),
        "node id should be mechanical (tab-scoped), not derived from the implicit \
         'ID' column; got id={alpha_id:?}"
    );
    assert!(
        alpha_id.to_string().contains("items"),
        "mechanical id should contain the tab slug 'items'; got id={alpha_id:?}"
    );

    // The "ID" column values should appear as payload under "id_col"
    // (renamed from "id" by the RESERVED_ROW_IR_KEYS guard).
    let id_col_value = alpha_node
        .payload
        .get("id_col")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert_eq!(
        id_col_value, "1.0",
        "implicit 'ID' column value should be stored as payload 'id_col', got {:?}",
        id_col_value
    );

    Ok(())
}
