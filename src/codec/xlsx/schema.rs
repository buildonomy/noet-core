//! Schema types for the XLSX/ODS codec's `index` tab declaration.
//!
//! ## System-managed BID fields
//!
//! `WorkbookSchema` carries two system-managed fields that are injected by
//! `generate_source_bytes()` on every `--write` pass and read back on the next parse:
//!
//! - `bid` — the workbook node's BID (UUID string).
//! - `tabs_meta` — a map from tab name → `TabBidEntry` carrying the tab container BID.
//!
//! These fields must never be authored manually. They mirror the `[sections]` table
//! that `MdCodec` persists in markdown frontmatter for section BID stability.
//!
//! The schema is stored as YAML, JSON, or TOML in cell A1 of the reserved `index`
//! worksheet. The codec tries YAML first, then JSON, then TOML.
//!
//! ## Minimal example (YAML)
//!
//! ```yaml
//! title: "Widget Project Requirements"
//! tabs:
//!   - name: "Functional Requirements"
//!     schema:
//!       - col: "Description"
//!         role: text
//!       - col: "Implements"
//!         role: relation
//!     # "Title" → title (first column default)
//!     # "Category", "Priority" → payload (unlisted default)
//! ```
//!
//! ## Column defaults
//!
//! Columns not listed in `schema` receive lazy defaults:
//! 1. Columns whose header matches `__noet_<property>__` → reserved system column.
//! 2. The first non-reserved column in the header row → `role: title` (when no
//!    explicit `title`-role column has been declared).
//! 3. All remaining unlisted columns → `role: payload`.
//!
//! A tab with no `schema` key at all is valid.
//!
//! ## Reserved columns
//!
//! Columns named `__noet_<property>__` are detected automatically and mapped to
//! `BeliefNode` fields without appearing in the `schema` list:
//!
//! | Header             | BeliefNode field |
//! |--------------------|-----------------|
//! | `__noet_bid__`     | `bid`           |
//! | `__noet_id__`      | `id`            |
//! | `__noet_title__`   | `title`         |
//! | `__noet_schema__`  | `schema`        |
//! | `__noet_kind__`    | `kind`          |

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Root schema deserialized from cell A1 of the `index` tab.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct WorkbookSchema {
    /// Human-readable name for the workbook node in the belief graph.
    /// Becomes the corpus container title displayed in search and the HTML viewer.
    #[serde(default)]
    pub title: String,

    /// Optional stable semantic identifier for the workbook node.
    ///
    /// Consistent with `BeliefNode::id` and processed through `to_anchor()` before
    /// storage, making it idempotent with the rest of the identity system. When
    /// provided, `GraphBuilder` uses it as a collision-resistant lookup key so the
    /// workbook node retains the same BID even if the file is renamed or moved.
    ///
    /// `bref` is never declared — it is derived from `bid` by the runtime.
    /// `bid` appears only in the `__noet_bid__` reserved column, injected by `--write`.
    #[serde(default)]
    pub id: Option<String>,

    /// System-managed BID for the workbook node itself.
    ///
    /// Injected by `--write` via `generate_source_bytes()`. Never authored manually.
    /// Stored as a UUID string, matching the format used in markdown frontmatter.
    /// On next parse, injected into `workbook_proto.document["bid"]` so
    /// `speculative_path_key` takes the stable `NodeKey::Bid` path.
    #[serde(default)]
    pub bid: Option<String>,

    /// Ordered list of tab declarations.
    ///
    /// Tabs present in the workbook but absent from this list are "opaque": they
    /// receive a single container node and their content is exported to
    /// `.noet/derived/<workbook>__<tab>.csv`.
    #[serde(default)]
    pub tabs: Vec<TabSchema>,

    /// System-managed BID map for tab container nodes.
    ///
    /// Keys are tab names as they appear in the workbook. Values are `TabBidEntry`.
    /// Injected by `--write` via `generate_source_bytes()`. Never authored manually.
    /// On next parse, each entry's `bid` is injected into the corresponding tab
    /// container node's `document["bid"]` so it resolves stably via `NodeKey::Bid`.
    #[serde(default)]
    pub tabs_meta: HashMap<String, TabBidEntry>,
}

/// System-managed metadata for a tab container node.
///
/// Stored in `WorkbookSchema.tabs_meta` keyed by tab name.
/// Populated by `generate_source_bytes()` and read back during `parse()`.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct TabBidEntry {
    /// BID of the tab container node, injected by `--write`.
    #[serde(default)]
    pub bid: Option<String>,
}

/// Schema declaration for one worksheet tab.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TabSchema {
    /// Tab name as it appears in the workbook (case-sensitive, no leading/trailing spaces).
    pub name: String,

    /// When `true`, this tab is excluded from row-node parsing regardless of any
    /// `schema:` list declared for it.
    ///
    /// An ignored tab still emits a single tab container node (heading=3) with an
    /// upstream `Epistemic` relation to its CSV export in `.noet/derived/`, exactly
    /// like a tab absent from the schema — the difference is that `ignore: true`
    /// lets authors explicitly name the tab in the index schema so the wildcard
    /// (`name: "*"`) does not pick it up, and documents the intent clearly.
    ///
    /// Default: `false`.
    #[serde(default)]
    pub ignore: bool,

    /// Optional Mustache-style template for composing multiple columns into a single
    /// Markdown text body.
    ///
    /// References column names with `{{ColumnName}}` syntax. When present, supersedes
    /// individual `role: text` column declarations for body text composition.
    /// Interpolation happens before Markdown parsing, so the composed string is
    /// processed as a single Markdown fragment. Column names absent from the header
    /// row are replaced with an empty string silently.
    ///
    /// Example:
    /// ```yaml
    /// text_template: "{{Description}}\n\n**Rationale**: {{Rationale}}"
    /// ```
    ///
    /// When absent, all `role: text` column values are joined with `\n\n` (v1 behaviour).
    #[serde(default)]
    pub text_template: Option<String>,

    /// Explicit column role declarations.
    ///
    /// Columns not listed here receive lazy defaults (see module-level docs).
    /// An empty or absent `schema` is valid — the codec infers roles from position
    /// and reserved column headers.
    #[serde(default)]
    pub schema: Vec<ColumnSchema>,
}

/// Declaration for one column in a tab's schema.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ColumnSchema {
    /// Column header string as it appears in row 1 of the tab.
    /// Case-sensitive. Must match exactly (no leading/trailing spaces).
    pub col: String,

    /// Semantic role of this column's data in the belief graph.
    ///
    /// Defaults to `payload` when absent from the YAML declaration. This allows
    /// `ignore: true` tabs to declare partial schema entries (e.g. just `col:`)
    /// without causing a deserialization error — the schema is never consulted
    /// for ignored tabs anyway.
    #[serde(default)]
    pub role: ColumnRole,

    /// Edge weight kind for `role: relation` columns. Defaults to `Pragmatic`.
    #[serde(default)]
    pub weight: RelationWeight,

    /// Edge direction for `role: relation` columns.
    ///
    /// - `upstream` (default): the cell value identifies a node that this row
    ///   **derives from or is constrained by** — the more abstract/parent end.
    ///   Stored in `IRNode::upstream`. Example: a requirement row citing the
    ///   top-level requirement it implements.
    /// - `downstream`: the cell value identifies a node that **derives from or
    ///   is constrained by** this row — the more concrete/child end.
    ///   Stored in `IRNode::downstream`. Example: a requirement row listing the
    ///   test cases that verify it.
    ///
    /// The terms match `IRNode::upstream` / `IRNode::downstream` directly.
    #[serde(default)]
    pub direction: RelationDirection,

    /// Explicit `NodeKey` type for `role: relation` columns.
    ///
    /// When set, the cell value is wrapped in `{key}://{value}` before being
    /// passed to `NodeKey::from_str`, bypassing the bare-string heuristic and
    /// producing the exact `NodeKey` variant requested.
    ///
    /// | `key:` value | NodeKey variant  | Use when cell contains          |
    /// |--------------|------------------|---------------------------------|
    /// | `id`         | `NodeKey::Id`    | Semantic slug (`code-generation`) |
    /// | `path`       | `NodeKey::Path`  | Repo-relative file path         |
    /// | `bid`        | `NodeKey::Bid`   | Full UUID string                |
    /// | `bref`       | `NodeKey::Bref`  | 8-char hex bref                 |
    ///
    /// When absent (default), `NodeKey::from_str` uses its bare-string
    /// heuristic: BID → Bref → Id (for strings without `/`, `#`, `.`).
    /// This works well for brefs and BIDs but produces `NodeKey::Id` for
    /// plain-text labels like `"Code Generation"`, which may be the desired
    /// behaviour (resolve to a corpus node by semantic id) or may not.
    ///
    /// Example — subsystem labels that should resolve as semantic ids:
    /// ```yaml
    /// - col: "Subsystem"
    ///   role: relation
    ///   key: id
    ///   direction: upstream
    /// ```
    #[serde(default)]
    pub key: RelationKeyFormat,

    /// When `true`, the column's cell content wraps to multiple lines in the
    /// rendered HTML table. Defaults to `false` (single-line, truncated with
    /// ellipsis by Tabulator's default behaviour).
    ///
    /// Useful for long-text columns such as requirement rationale or description
    /// where truncation loses important context.
    ///
    /// Example:
    /// ```yaml
    /// - col: "Description"
    ///   role: text
    ///   wrap: true
    /// ```
    #[serde(default)]
    pub wrap: bool,
}

/// Semantic role of a column's data in the node graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ColumnRole {
    /// Maps to `node.title`. At most one per tab schema.
    ///
    /// When absent from the explicit schema, the first non-reserved column in the
    /// header row is promoted to this role automatically.
    Title,

    /// Maps to `node.payload["text"]`. Multiple allowed; joined with `\n\n`.
    ///
    /// Cell content is parsed as Markdown — links become upstream graph edges
    /// (resolved at `inject_context` time). Search-indexed.
    ///
    /// Superseded by `text_template` when that field is present on the tab.
    Text,

    /// Parsed as a noet node reference and emitted as an upstream graph edge.
    ///
    /// Cell value formats accepted: BID, bref, `id://slug`, or a plain title slug.
    /// Multiple references may be separated by semicolons: `"abc123de; def456fg"`.
    /// The edge weight kind is controlled by the `weight` field (default: `Pragmatic`).
    ///
    /// Unresolvable references emit a `Warning` and are omitted from the edge list;
    /// the row node is still emitted.
    Relation,

    /// Stored in `node.payload["<col_name>"]` with the column header as the key.
    ///
    /// Not included in the search index. Use for structured metadata that tooling
    /// needs but that noet does not need to reason about (e.g. ticket numbers,
    /// revision letters, external tool IDs).
    #[default]
    Payload,
}

/// Edge weight kind for `role: relation` columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RelationWeight {
    /// Assertion-backed link (e.g. requirements traceability, implementation claims).
    /// Default.
    #[default]
    Pragmatic,

    /// Evidence-backed link (e.g. design cross-references, rationale citations).
    Epistemic,
}

/// Explicit `NodeKey` type to use when parsing a `role: relation` cell value.
///
/// Maps directly onto the `NodeKey` enum variants. When set, the codec prefixes
/// the cell value with `{scheme}://` so that `NodeKey::from_str` produces the
/// exact variant without relying on the bare-string heuristic.
///
/// Semicolon-separated values in a single cell are each wrapped individually:
/// `"abc; def"` with `key: id` → `["id://abc", "id://def"]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RelationKeyFormat {
    /// No explicit format — use `NodeKey::from_str` bare-string heuristic.
    /// Works correctly for BIDs and brefs. Produces `NodeKey::Id` for plain
    /// text strings without path indicators. Default.
    #[default]
    Auto,
    /// Wrap value as `id://{value}` → `NodeKey::Id`.
    /// Use when cell values are semantic slugs or human-readable labels that
    /// should resolve to corpus nodes by their semantic identifier.
    Id,
    /// Wrap value as `path://{value}` → `NodeKey::Path`.
    /// Use when cell values are repo-relative file paths.
    Path,
    /// Wrap value as `bid://{value}` → `NodeKey::Bid`.
    /// Use when cell values are full UUID BID strings.
    Bid,
    /// Wrap value as `bref://{value}` → `NodeKey::Bref`.
    /// Use when cell values are 8-char hex bref strings.
    Bref,
}

impl RelationKeyFormat {
    /// Return the URL scheme string for this format, or `None` for `Auto`.
    pub fn scheme(self) -> Option<&'static str> {
        match self {
            Self::Auto => None,
            Self::Id => Some("id"),
            Self::Path => Some("path"),
            Self::Bid => Some("bid"),
            Self::Bref => Some("bref"),
        }
    }

    /// Format a cell value for `NodeKey::from_str`.
    ///
    /// For `Auto`, returns the value unchanged.
    /// For all others, returns `"{scheme}://{value}"`.
    pub fn format_value(self, value: &str) -> String {
        match self.scheme() {
            None => value.to_string(),
            Some(scheme) => format!("{scheme}://{value}"),
        }
    }
}

/// Edge direction for `role: relation` columns.
///
/// Determines whether the resolved `NodeKey` is pushed onto `IRNode::upstream`
/// (the default) or `IRNode::downstream`.
///
/// The vocabulary deliberately matches `IRNode`'s field names:
/// - **upstream**: this row is the more concrete end; the referenced node is the
///   more abstract/parent end. Use for "implements", "traces-to", "satisfies".
/// - **downstream**: this row is the more abstract end; the referenced node is
///   the more concrete/child end. Use for "verified-by", "implemented-by".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RelationDirection {
    /// Push the resolved key into `IRNode::upstream`. Default.
    #[default]
    Upstream,
    /// Push the resolved key into `IRNode::downstream`.
    Downstream,
}

/// The set of column headers that are reserved for system-managed `BeliefNode` fields.
///
/// Reserved columns are detected automatically in the header row regardless of whether
/// they appear in the tab's `schema` list. They are never included in `payload` and
/// never emitted as `text` or `tag` content.
///
/// An unrecognised `__noet_<x>__` pattern emits a `ParseDiagnostic::Warning` and
/// falls back to `payload`.
pub const RESERVED_COLUMNS: &[(&str, ReservedColumnKind)] = &[
    ("__noet_bid__", ReservedColumnKind::Bid),
    ("__noet_id__", ReservedColumnKind::Id),
    ("__noet_title__", ReservedColumnKind::Title),
    ("__noet_schema__", ReservedColumnKind::Schema),
    ("__noet_kind__", ReservedColumnKind::Kind),
];

/// The semantic meaning of a recognised reserved column header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReservedColumnKind {
    /// `__noet_bid__` — injected by `--write`; read back on next parse for BID stability.
    Bid,
    /// `__noet_id__` — user-authored stable semantic ID; processed by `to_anchor()`.
    Id,
    /// `__noet_title__` — overrides the title-role column when present.
    Title,
    /// `__noet_schema__` — schema string for schema-aware nodes.
    Schema,
    /// `__noet_kind__` — `BeliefKind` set; parsed as comma-separated kind names.
    Kind,
}

impl ReservedColumnKind {
    /// Return the `ReservedColumnKind` for a header string, or `None` if the header
    /// is not a recognised reserved column.
    ///
    /// Unrecognised `__noet_<x>__` patterns (those with the prefix but an unknown
    /// property name) return `None` and should be handled by the caller as a warning.
    pub fn from_header(header: &str) -> Option<Self> {
        RESERVED_COLUMNS
            .iter()
            .find(|(h, _)| *h == header)
            .map(|(_, kind)| *kind)
    }

    /// Return `true` if the header string uses the `__noet_` prefix, regardless of
    /// whether the property name is recognised. Used to detect unknown reserved
    /// column patterns that should emit a warning.
    pub fn has_reserved_prefix(header: &str) -> bool {
        header.starts_with("__noet_") && header.ends_with("__")
    }
}
