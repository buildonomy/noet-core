//! XLSX/ODS codec — structured spreadsheet ingestion.
//!
//! Reads `.xlsx` and `.ods` files as first-class corpus nodes.
//! A reserved `index` tab carries a YAML schema declaration; all other
//! schema-declared tabs are parsed into node hierarchies (workbook → tab → row).
//! Tabs absent from the schema emit a single opaque tab node with no row children.
//!
//! See `docs/project/ISSUE_69_XLSX_CODEC.md` for full design rationale.

pub mod codec;
pub mod schema;
