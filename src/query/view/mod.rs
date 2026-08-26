// query/view/mod.rs — ViewRenderer trait and ViewRegistry for rendering query output.
//
// The view layer sits between the query evaluation pipeline and the output
// format. Each view renderer implementation takes a `QueryPackage` (which
// bundles the tape, optional graph, and spec) and produces either HTML or
// structured rows.
//
// ## ViewRegistry
//
// The registry maps view keys (short strings like `"connectivity"`, `"depth0"`)
// to factory functions that construct concrete [`ViewRenderer`] implementations.
// View keys are supplied by surface-specific mechanisms:
//   - `view=` URL parameter (viewer)
//   - `:view:` directive option (`{query}` MyST directive)
//   - `view` field in MCP tool input
//
// See `query_model.md` §9.5 for the surface binding contract.

pub mod raw_tape;
pub mod table;

use once_cell::sync::Lazy;
use parking_lot::RwLock;
use std::sync::Arc;
use toml::value::Table;

use crate::beliefbase::BeliefGraph;
use crate::properties::{BeliefNode, Bid, WeightKind};
use crate::query::spec::QueryPackage;
use crate::BuildonomyError;

pub use raw_tape::RawTapeView;
pub use table::{TableDisplayMode, TableView};

// ═════════════════════════════════════════════════════════════════════════════
// Shared cell types (used by raw_tape and table)
// ═════════════════════════════════════════════════════════════════════════════

/// Ownership model for an edge.
pub(crate) enum EdgeOwnership {
    /// `owned_by: "source"` — the source endpoint owns this edge.
    Source,
    /// `owned_by: "sink"` — the sink endpoint owns this edge.
    Sink,
    /// `owned_by: <bref>` — a third-party node owns this edge.
    ThirdParty(Bid),
    /// Missing `owned_by` field — data integrity error.
    Missing,
}

/// Structured data for an edge cell.
///
/// Rendered as:
/// - `KIND(s)` when owned by source
/// - `KIND(k)` when owned by sink
/// - `KIND(@)` when owned by third party
/// - `KIND(⚠)` when owned_by is missing (error)
pub(crate) struct EdgeCellData {
    pub(crate) source: Bid,
    pub(crate) sink: Bid,
    pub(crate) ownership: EdgeOwnership,
    pub(crate) kind: WeightKind,
}

/// A single cell in the rendered output.
pub(crate) struct Cell {
    /// Display text (for plain text cells or the kind label for edge cells).
    pub(crate) text: String,
    /// Optional BID — when present, the cell references a single node.
    pub(crate) bid: Option<Bid>,
    /// Optional edge data — when present, the cell represents an edge
    /// rendered as `KIND(s/k)` or `KIND(s/k/@)` with linked roles.
    pub(crate) edge: Option<EdgeCellData>,
}

impl Cell {
    pub(crate) fn text(s: impl Into<String>) -> Self {
        Self {
            text: s.into(),
            bid: None,
            edge: None,
        }
    }

    pub(crate) fn node(title: impl Into<String>, bid: Bid) -> Self {
        Self {
            text: title.into(),
            bid: Some(bid),
            edge: None,
        }
    }
}

/// A single row in the rendered output.
pub(crate) struct EntryRow {
    /// Cell values in column order.
    pub(crate) cells: Vec<Cell>,
}

/// Serialize rows of cells to the JSON cell format.
///
/// Each row becomes `{ "cells": [...] }` where each cell is either a plain
/// string, a `{ "bid": "..." }` node reference, or an `{ "edge": { ... } }`
/// object with ownership annotation.
pub(crate) fn cells_to_json(rows: &[EntryRow]) -> Vec<serde_json::Value> {
    rows.iter()
        .map(|row| {
            let cells: Vec<serde_json::Value> = row
                .cells
                .iter()
                .map(|cell| {
                    if let Some(edge) = &cell.edge {
                        let mut obj = serde_json::json!({
                            "edge": {
                                "kind": format!("{:?}", edge.kind),
                                "source": { "bid": edge.source.to_string() },
                                "sink": { "bid": edge.sink.to_string() },
                            }
                        });
                        match &edge.ownership {
                            EdgeOwnership::Source => {
                                obj["edge"]["owned_by"] = serde_json::json!("source");
                            }
                            EdgeOwnership::Sink => {
                                obj["edge"]["owned_by"] = serde_json::json!("sink");
                            }
                            EdgeOwnership::ThirdParty(owner) => {
                                obj["edge"]["owner"] =
                                    serde_json::json!({ "bid": owner.to_string() });
                            }
                            EdgeOwnership::Missing => {
                                obj["edge"]["error"] = serde_json::json!("missing owned_by field");
                            }
                        }
                        obj
                    } else if let Some(bid) = &cell.bid {
                        serde_json::json!({ "bid": bid.to_string() })
                    } else {
                        serde_json::Value::String(cell.text.clone())
                    }
                })
                .collect();
            serde_json::json!({ "cells": cells })
        })
        .collect()
}

/// Build a JSON map of `{ bid_string → { "title": ..., "id": ... } }` from graph states.
pub(crate) fn node_info_map(
    bids: impl Iterator<Item = Bid>,
    graph: &BeliefGraph,
) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    for bid in bids {
        if let Some(node) = graph.states.get(&bid) {
            let id_str = match &node.id {
                crate::properties::NodeId::Slug => bid.bref().to_string(),
                crate::properties::NodeId::Explicit(s) => s.clone(),
                crate::properties::NodeId::Collision(s) => s.clone(),
            };
            map.insert(
                bid.to_string(),
                serde_json::json!({
                    "title": node.title,
                    "id": id_str,
                }),
            );
        }
    }
    serde_json::Value::Object(map)
}

// ═════════════════════════════════════════════════════════════════════════════
// ViewOutput
// ═════════════════════════════════════════════════════════════════════════════

/// Output produced by a view renderer.
#[derive(Debug, Clone, PartialEq)]
pub enum ViewOutput {
    /// HTML string (for compile-time directive rendering and static export).
    Html(String),
    /// Structured JSON (for programmatic consumers: WASM viewer, MCP, tests).
    /// The shape is view-specific — `TableView` emits `{ headers, rows }`,
    /// `RawTapeView` emits a vec of entry objects, etc.
    Json(serde_json::Value),
}

// ═════════════════════════════════════════════════════════════════════════════
// LinkResolver trait
// ═════════════════════════════════════════════════════════════════════════════

/// Trait for resolving node BIDs to navigable HTML links.
///
/// Implemented by BeliefBase-backed resolvers in all contexts (native, WASM, MCP).
/// Passed to [`ViewRenderer::render`] so renderers can produce navigable links
/// matching the SPA's two-click navigation contract:
/// `<a href="relative.html" title="bref://BREF">content</a>`
pub trait LinkResolver: Send + Sync {
    /// Resolve a BID to a relative HTML href, or `None` if path unknown.
    fn resolve_href(&self, bid: &Bid) -> Option<String>;

    /// Render a node as a navigable `<a>` tag.
    ///
    /// When path resolution succeeds, produces:
    /// `<a href="relative.html" title="bref://BREF">content</a>`
    ///
    /// When path resolution fails, produces plain text (no `<a>` tag).
    fn render_anchor(&self, node: &BeliefNode, content: &str) -> String {
        let bref = node.bid.bref();
        match self.resolve_href(&node.bid) {
            Some(href) => format!(
                "<a href=\"{}\" title=\"bref://{}\">{}</a>",
                html_escape_attr(&href),
                bref,
                content
            ),
            None => content.to_string(),
        }
    }
}

/// Minimal HTML attribute escaping.
fn html_escape_attr(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

// ═════════════════════════════════════════════════════════════════════════════
// BeliefBaseLinkResolver
// ═════════════════════════════════════════════════════════════════════════════

/// A [`LinkResolver`] backed by a [`BeliefBase`](crate::beliefbase::BeliefBase) reference.
///
/// Created once per rendering scope (document compilation, panel render)
/// and shared across all view renderers for that scope.
///
/// Only available on native targets — WASM builds use `Rc<RefCell<>>` for
/// `BeliefBase` internals, which doesn't satisfy the `Sync` bound.
#[cfg(not(target_arch = "wasm32"))]
pub struct BeliefBaseLinkResolver<'a> {
    bb: &'a crate::beliefbase::BeliefBase,
    from_path: String,
}

#[cfg(not(target_arch = "wasm32"))]
impl<'a> BeliefBaseLinkResolver<'a> {
    /// Create a resolver for links relative to `from_path`.
    ///
    /// `from_path` is the network-relative path of the document being rendered
    /// (e.g., `"query_directive_test.md"`).
    pub fn new(bb: &'a crate::beliefbase::BeliefBase, from_path: &str) -> Self {
        Self {
            bb,
            from_path: from_path.to_string(),
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl LinkResolver for BeliefBaseLinkResolver<'_> {
    fn resolve_href(&self, bid: &Bid) -> Option<String> {
        crate::beliefbase::context::resolve_node_href(self.bb, bid, &self.from_path)
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// ViewRenderer trait
// ═════════════════════════════════════════════════════════════════════════════

/// The view renderer trait: takes a query package and produces rendered output.
///
/// The package bundles the tape (intermediate projection results), the
/// materialized graph, and the spec. When the tape contains composition
/// steps, `render` reads the tape to produce A/B comparison views
/// (e.g., gap analysis side columns). See `query_model.md` §6.
pub trait ViewRenderer: Send {
    /// Render the query package.
    ///
    /// `package` — the evaluated query package, containing the tape of
    ///             intermediate results and the materialized graph.
    /// `links`   — optional link resolver for producing navigable `<a>` tags.
    ///             When `None`, renderers fall back to plain text or `data-bid`
    ///             attributes.
    fn render(
        &self,
        package: &QueryPackage,
        links: Option<&dyn LinkResolver>,
    ) -> Result<ViewOutput, BuildonomyError>;

    /// Render the query package as structured JSON.
    ///
    /// Default implementation returns an error. Concrete views opt in by
    /// overriding this method. The JSON shape is view-specific.
    fn render_json(&self, _package: &QueryPackage) -> Result<ViewOutput, BuildonomyError> {
        Err(BuildonomyError::Command(
            "this view does not support JSON output".to_string(),
        ))
    }

    /// Return the raw configuration table used to construct this renderer.
    ///
    /// The table is an opaque bag of key-value pairs populated by the surface
    /// layer (URL params, directive options, MCP fields). Common keys include
    /// `"sort"`, `"display"`, `"max_rows"`, `"caption"`, `"columns"`.
    ///
    /// Callers that need the sort spec can read `spec().get("sort")` and
    /// parse via `SortSpec::from_str`. The default implementation returns an
    /// empty table for renderers that carry no configuration.
    fn spec(&self) -> &Table {
        static EMPTY: Lazy<Table> = Lazy::new(Table::new);
        &EMPTY
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// ViewFactory and ViewRegistry
// ═════════════════════════════════════════════════════════════════════════════

/// Factory function type: creates a [`ViewRenderer`] from a configuration table.
///
/// The table is an opaque params bag assembled by the surface layer. The
/// factory may read or augment keys before constructing the view. Returns an
/// error if the params are structurally invalid (e.g., malformed column paths).
pub type ViewFactory = fn(&Table) -> Result<Box<dyn ViewRenderer>, BuildonomyError>;

/// Registry mapping view keys to [`ViewFactory`] functions.
///
/// The view key identifies the rendering mode. Built-in keys:
///
/// | Key          | Description                                  |
/// |--------------|----------------------------------------------|
/// | `"depth0"`   | Node intrinsics: title, schema, kind (default) |
/// | `"connectivity"` | Connectivity matrix: In/Out per WeightKind |
/// | `"columns"`  | Explicit column list from `params["columns"]` |
/// | `"raw_tape"` | Per-entry tape rendering (edges, nodes, etc.) |
///
/// Application shims may register additional view keys at startup via
/// [`ViewRegistry::register`].
pub struct ViewRegistry(Arc<RwLock<Vec<(&'static str, ViewFactory)>>>);

impl ViewRegistry {
    /// Create a new registry with the built-in views registered.
    pub fn create() -> Self {
        let registry = Self(Arc::new(RwLock::new(Vec::new())));
        registry.register("depth0", table::depth0_factory);
        registry.register("connectivity", table::connectivity_factory);
        registry.register("columns", table::columns_factory);
        registry.register("raw_tape", raw_tape::raw_tape_factory);
        registry
    }

    /// Register a view key with a factory function.
    ///
    /// If the key is already registered, the new factory replaces the old one.
    pub fn register(&self, key: &'static str, factory: ViewFactory) {
        let mut entries = self.0.write();
        if let Some(entry) = entries.iter_mut().find(|(k, _)| *k == key) {
            entry.1 = factory;
        } else {
            entries.push((key, factory));
        }
    }

    /// Look up a factory by view key. Returns `None` if the key is unknown.
    pub fn get(&self, key: &str) -> Option<ViewFactory> {
        self.0
            .read()
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, f)| *f)
    }

    /// Build a default view (depth0 mode, empty params).
    ///
    /// Used when no view key is specified by the surface layer.
    pub fn default_view() -> Result<Box<dyn ViewRenderer>, BuildonomyError> {
        table::depth0_factory(&Table::new())
    }

    /// List all registered view keys.
    pub fn known_keys(&self) -> Vec<&'static str> {
        self.0.read().iter().map(|(k, _)| *k).collect()
    }
}

/// Global view registry — maps view keys to renderer factories.
///
/// Initialized on first access with the built-in [`TableView`] variants.
/// Application shims may call [`ViewRegistry::register`] at startup to add
/// custom view implementations.
pub static VIEWS: Lazy<ViewRegistry> = Lazy::new(ViewRegistry::create);
