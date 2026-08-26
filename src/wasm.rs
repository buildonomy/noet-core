//! WASM bindings for noet-core
//!
//! This module provides JavaScript-accessible APIs for querying BeliefGraphs in the browser.
//! It's designed for static site viewers that load `beliefbase.json` and provide client-side
//! search, navigation, and backlink exploration.
//!
//! ## Usage
//!
//! ```javascript,ignore
//! import init, { BeliefBaseWasm } from './noet_wasm.js';
//!
//! async function main() {
//!     await init();
//!
//!     // Load beliefbase.json
//!     const response = await fetch('beliefbase.json');
//!     const json = await response.text();
//!
//!     // Create WASM BeliefBase
//!     const bb = BeliefBaseWasm.from_json(json);
//!
//!     // Query a node
//!     const node = bb.get_by_bid("01234567-89ab-cdef-0123-456789abcdef");
//!     console.log(node);
//!
//!     // Search
//!     const results = bb.search("documentation");
//!     console.log(results);
//!
//!     // Get backlinks
//!     const backlinks = bb.get_backlinks("01234567-89ab-cdef-0123-456789abcdef");
//!     console.log(backlinks);
//! }
//! ```
//!
//! # ⚠️ CRITICAL: Rust→JavaScript Serialization Patterns
//!
//! **Problem**: `serde_wasm_bindgen::to_value()` dispatches through serde's generic
//! serialization traits. Any type whose `Serialize` impl calls `serialize_map` —
//! including `BTreeMap`, `HashMap`, AND `serde_json::Value::Object` — will produce
//! a JavaScript `Map`, not a plain object. This breaks JS code expecting plain objects.
//!
//! **Key distinction — top-level vs. nested**:
//! - `serde_wasm_bindgen::to_value(&serde_json::Value::Object(...))` **at the top level**
//!   produces a plain JS object ✅ (the top-level dispatcher handles `Value` specially)
//! - A `serde_json::Value::Object` field **nested inside a `#[derive(Serialize)]` struct**
//!   still calls `serialize_map` → JS `Map` ❌
//!
//! This means Option A below only works when `serde_json::Value` is the *return type*
//! of the function, not when it is a field inside another serialized struct.
//!
//! ## Symptoms
//! ```javascript,ignore
//! // JavaScript receives a Map, not a plain object:
//! const data = wasm_function();
//! Object.keys(data);        // Returns [] (empty array!)
//! data[key];                // Returns undefined (bracket notation fails!)
//! Object.entries(data);     // Returns [] (empty array!)
//! // Object.prototype.toString.call(data) === "[object Map]"  ← diagnostic
//! ```
//!
//! ## Solutions
//!
//! ### Option A: Return serde_json::Value directly (top-level only)
//! Only works when the entire return value is `serde_json::Value`. Do NOT use for
//! fields nested inside a struct — see Option D for that case.
//! ```rust,ignore
//! use serde_json::json;
//!
//! #[wasm_bindgen]
//! pub fn get_data(&self) -> JsValue {
//!     let mut map = serde_json::Map::new();
//!     for (key, value) in rust_btreemap.iter() {
//!         map.insert(key.to_string(), json!(value));
//!     }
//!     let obj = serde_json::Value::Object(map);
//!     serde_wasm_bindgen::to_value(&obj).unwrap()  // ✅ Plain object (top-level only)
//! }
//! ```
//!
//! ### Option B: Return JavaScript Map (When Map semantics are needed)
//! ```rust,ignore
//! #[wasm_bindgen]
//! pub fn get_data(&self) -> JsValue {
//!     let data: BTreeMap<String, Value> = ...;
//!     serde_wasm_bindgen::to_value(&data).unwrap()  // ✅ JavaScript Map
//! }
//! ```
//! **IMPORTANT**: Document in function JSDoc that it returns a Map:
//! ```rust
//! /// Returns a Map<string, Value> (use .get(), .size, .entries())
//! ```
//!
//! ### Option C: Return Array of Tuples (Simple alternative)
//! ```rust,ignore
//! #[wasm_bindgen]
//! pub fn get_data(&self) -> JsValue {
//!     let data: Vec<(String, Value)> = btreemap.into_iter().collect();
//!     serde_wasm_bindgen::to_value(&data).unwrap()  // ✅ Array
//! }
//! ```
//!
//! ### Option D: Patch a nested field via js_sys::JSON::parse (plain object inside a struct)
//! When a field inside a serialized struct must be a plain JS object, serialize that
//! field to a JSON string in Rust and parse it via the JS JSON engine after the fact.
//! `js_sys::JSON::parse` always produces plain objects regardless of serde dispatch.
//! ```rust,ignore
//! use js_sys::{Reflect, JSON};
//!
//! #[wasm_bindgen]
//! pub fn get_data(&self) -> JsValue {
//!     let ctx = build_node_context(); // struct with a serde_json::Value metadata field
//!     let js_val = serde_wasm_bindgen::to_value(&ctx).unwrap_or(JsValue::NULL);
//!     // metadata came out as a JS Map — replace it with a plain object via JSON roundtrip
//!     if js_val.is_object() {
//!         if let Ok(json_str) = serde_json::to_string(&ctx.metadata) {
//!             if let Ok(parsed) = JSON::parse(&json_str) {
//!                 let _ = Reflect::set(&js_val, &JsValue::from_str("metadata"), &parsed);
//!             }
//!         }
//!     }
//!     js_val  // ✅ metadata is now a plain JS object
//! }
//! ```
//!
//! ## Checklist for New Functions
//! - [ ] Does this function return or contain BTreeMap/HashMap/serde_json::Value::Object?
//! - [ ] Is the map-like type the top-level return value, or nested inside a struct?
//!   - Top-level → Option A (serde_json::Value) or Option B/C
//!   - Nested inside struct → Option D (JSON string shim) or restructure to avoid nesting
//! - [ ] Does JavaScript need plain object access (obj[key], Object.keys)?
//!   - YES → Option A (if top-level) or Option D (if nested)
//!   - NO → Option B and document as Map
//! - [ ] Add JSDoc comment showing JavaScript type
//! - [ ] Verify viewer.js uses correct access pattern
//!
//! ## Current Functions Status
//! - ✅ `get_paths()` - Returns plain object (serde_json::Value top-level, Option A)
//! - ✅ `get_context()` - Returns NodeContext; `metadata` patched via Option D (JSON shim)
//! - ⚠️ `get_context()` - `related_nodes` and `graph` fields are intentional JS Maps
//! - ⚠️ `get_nav_tree()` - Returns NavTree with Map field (nodes) — intentional
//! - ⚠️ `query()` - Returns BeliefGraph with Map field (states) — intentional
//!
//! See viewer.js header for JavaScript-side usage patterns.

#[cfg(feature = "wasm")]
use wasm_bindgen::prelude::*;

#[cfg(feature = "wasm")]
use web_sys::console;

#[cfg(feature = "wasm")]
use std::sync::OnceLock;

#[cfg(feature = "wasm")]
use tracing_subscriber::reload;

// Reload handle for runtime log level control.
// Stores the handle as a type-erased boxed trait so we don't expose the full
// subscriber type in the module signature.
#[cfg(feature = "wasm")]
static LOG_LEVEL_HANDLE: OnceLock<
    reload::Handle<tracing_subscriber::filter::LevelFilter, tracing_subscriber::Registry>,
> = OnceLock::new();

/// Initialise the tracing→console subscriber.
///
/// Called automatically via `#[wasm_bindgen(start)]`. Safe to call multiple
/// times — subsequent calls are no-ops.
///
/// Default log level:
/// - **debug builds** (`debug_assertions` on): `DEBUG`
/// - **release builds**: `WARN`
#[cfg(feature = "wasm")]
#[wasm_bindgen(start)]
pub fn init_tracing() {
    use tracing_subscriber::prelude::*;

    #[cfg(debug_assertions)]
    let default_level = tracing_subscriber::filter::LevelFilter::INFO;
    #[cfg(not(debug_assertions))]
    let default_level = tracing_subscriber::filter::LevelFilter::WARN;

    let wasm_layer = tracing_wasm::WASMLayer::new(
        tracing_wasm::WASMLayerConfigBuilder::new()
            .set_max_level(tracing::Level::TRACE) // actual gate is the reload filter below
            .build(),
    );

    let (filter, handle) = reload::Layer::new(default_level);

    // Ignore error — if a subscriber is already set (e.g. in tests) we just skip.
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(wasm_layer)
        .try_init();

    // Store handle regardless of whether try_init succeeded; on failure the
    // OnceLock stays empty and set_log_level becomes a no-op.
    let _ = LOG_LEVEL_HANDLE.set(handle);
}

#[cfg(feature = "wasm")]
use crate::{
    beliefbase::{BeliefBase, BeliefGraph, BidGraph},
    codec::normalize_path_extension_impl,
    nodekey::NodeKey,
    paths::AnchorPath,
    properties::{
        asset_namespace, buildonomy_namespace, content_namespaces, href_namespace, BeliefKind,
        BeliefNode, Bid, Bref, WeightKind, WEIGHT_DOC_PATHS, WEIGHT_SORT_KEY,
    },
    query::{
        spec::TextSearchProvider,
        view::{ViewOutput, VIEWS},
        QueryPackage, QuerySpec,
    },
    shard::search::{query_search_index, SearchIndex},
};

#[cfg(feature = "wasm")]
use serde::{Deserialize, Serialize};

#[cfg(feature = "wasm")]
use serde_json;

#[cfg(feature = "wasm")]
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet, HashMap},
    str::FromStr,
};

#[cfg(feature = "wasm")]
use js_sys::{Object, Reflect, Uint8Array};

#[cfg(feature = "wasm")]
use rust_xlsxwriter::{Format, Workbook, XlsxError};

#[cfg(feature = "wasm")]
/// Result of a query evaluation, returned to JavaScript as `{ graph, tape_indices }`.
///
/// `graph` is the `BeliefGraph` containing matching nodes and their relations.
/// `tape_indices` maps each result BID to the tape entry index where it was
/// first discovered — enabling the viewer to show traversal depth.
///
/// # JavaScript Example
/// ```javascript,ignore
/// const result = bb.query(spec);
/// const graph = result.graph;            // BeliefGraph
/// const indices = result.tape_indices;   // Map<bid_string, number>
/// ```
#[cfg(feature = "wasm")]
#[derive(Serialize)]
struct QueryResult {
    graph: BeliefGraph,
    tape_indices: BTreeMap<Bid, usize>,
}

/// Plain serialisable result returned as a JS object `{ bid, bref }`.
///
/// Returned as `JsValue` (via `serde_wasm_bindgen::to_value`) so JavaScript
/// receives a real plain object rather than an opaque wasm-bindgen handle.
///
/// # JavaScript Example
/// ```javascript,ignore
/// const result = beliefbase.entryPoint();
/// console.log(result.bid);   // "1f10cfd9-1cc3-6a93-86f9-0e90d9cb2fdb"
/// console.log(result.bref);  // "0e90d9cb2fdb"
/// ```
#[cfg(feature = "wasm")]
#[derive(Serialize)]
pub struct BidBrefResult {
    pub bid: String,
    pub bref: String,
}

#[cfg(feature = "wasm")]
impl BidBrefResult {
    pub fn from_bid(bid: Bid) -> Self {
        BidBrefResult {
            bid: bid.to_string(),
            bref: bid.bref().to_string(),
        }
    }

    pub fn to_js(&self) -> JsValue {
        serde_wasm_bindgen::to_value(self).unwrap_or(JsValue::NULL)
    }
}

/// Navigation tree structure for hierarchical document navigation
///
/// Pre-structured tree generated in Rust for better performance than client-side tree building.
/// Uses a flat map structure with child IDs for efficient lookups and intelligent expand/collapse.
/// See `docs/design/interactive_viewer.md` § Navigation Tree Generation for specification.
#[cfg(feature = "wasm")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NavTree {
    /// Flat map of all nodes by BID (O(1) lookup)
    /// ⚠️ JavaScript: This is a Map object! Use `.get(bid)`, `.size`, `.entries()`
    pub nodes: BTreeMap<String, NavNode>,
    /// Root node BIDs (networks) in display order
    /// ✅ JavaScript: This is an Array. Use `[index]`
    pub roots: Vec<String>,
}

/// Unified navigation node (can be network, document, or section)
///
/// Stores only child BIDs, not nested nodes. This enables:
/// - O(1) lookup by path/BID for active node highlighting
/// - Easy parent chain traversal (path -> node -> parent via path lookup)
/// - Intelligent expand/collapse (expand parent chain, collapse siblings)
#[cfg(feature = "wasm")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NavNode {
    /// Node BID
    pub bid: String,
    /// Node title (from BeliefNode state)
    pub title: String,
    /// Full path with extension normalized to .html (e.g., "docs/guide.html" or "docs/guide.html#intro")
    pub path: String,
    /// Parent node BID (None for root nodes)
    pub parent: Option<String>,
    /// Child node BIDs (ordered by WEIGHT_SORT_KEY)
    pub children: Vec<String>,
    /// True if this node has API or Network kind (is_network ⊆ is_document)
    pub is_network: bool,
    /// True if this node has a standalone document identity (API, Network, or Document kind)
    pub is_document: bool,
}

/// WASM-compatible node context (no lifetimes, fully owned)
///
/// This is a serializable version of BeliefContext that can cross the FFI boundary.
/// Owned version of ExtendedRelation for WASM serialization (no lifetimes)
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RelatedNode {
    /// The related node
    pub node: BeliefNode,
    /// Home network BID for this node
    pub home_net: Bid,
    /// Path relative to the home network root
    pub root_path: String,
    /// The link display text stored on the edge during parse, if it differed from the target's title.
    /// Use as fallback when `node.title` is empty: `node.title || link_title || node.bid`
    pub link_title: Option<String>,
}

/// An edge endpoint entry in the `NodeContext.graph`, optionally annotated with the
/// BID of a third-party owner node (a section containing a `{maps_to}` directive).
///
/// When `owner_bid` is `Some`, the edge is owned by a section node rather than by the
/// source or sink endpoint. The viewer renders "via <section title>" in that case.
/// When `owner_bid` is `None`, the edge uses the standard source/sink ownership model.
#[cfg(feature = "wasm")]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EdgeEntry {
    /// The BID of the source or sink node at the other end of this edge.
    pub bid: Bid,
    /// The BID of the section node that owns this edge via a `{maps_to}` directive,
    /// if any. `None` for standard source-owned or sink-owned edges.
    pub owner_bid: Option<Bid>,
}

#[cfg(feature = "wasm")]
impl EdgeEntry {
    pub fn new(bid: Bid, owner_bid: Option<Bid>) -> Self {
        Self { bid, owner_bid }
    }
}

/// See `docs/design/interactive_viewer.md` § WASM Integration for specification.
#[cfg(feature = "wasm")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeContext {
    /// The node itself
    pub node: BeliefNode,
    /// path relative to the home network root (e.g., "/docs/guide.md#section")
    pub root_path: String,
    /// Home network BID (which Network node owns this document)
    pub home_net: Bid,
    /// Runtime metadata for this node (e.g. `source_url`, `git` status).
    /// Mirrors `node.metadata` as a top-level field for convenient JS access:
    /// `context.metadata?.source_url` rather than `context.node.metadata?.source_url`.
    ///
    /// Stored as `serde_json::Value` (not `toml::Table`) so that
    /// `serde_wasm_bindgen::to_value` produces a plain JavaScript object rather
    /// than a `Map`. A `toml::Table` serialized via `serde_wasm_bindgen` becomes
    /// a JS `Map`, making `metadata?.source_url` silently return `undefined`.
    pub metadata: serde_json::Value,
    /// All nodes related to this one (other end of all edges, both sources and sinks)
    /// Map from BID to RelatedNode for O(1) lookup when displaying graph relations
    /// Each RelatedNode includes the root_path needed for href generation
    /// ⚠️ JavaScript: This is a Map object! Use `.get(bid)`, `.size`, `.entries()`
    pub related_nodes: BTreeMap<Bid, RelatedNode>,
    /// Relations by weight kind: Map<WeightKind, (sources, sinks)>
    /// Sources: EdgeEntries for nodes linking TO this one
    /// Sinks: EdgeEntries for nodes this one links TO
    /// Both vectors are sorted by WEIGHT_SORT_KEY edge payload value
    /// ⚠️ JavaScript: This is a Map object! Use `.get(weightKind)`, `.size`, `.entries()`
    pub graph: HashMap<WeightKind, (Vec<EdgeEntry>, Vec<EdgeEntry>)>,
    /// All edges owned by a third-party section node (a `{maps_to}` directive).
    /// Each entry identifies the owning section BID, source BID, sink BID, and weight kind.
    /// ✅ JavaScript: This is an Array. Use `[index]`, `.length`, `.forEach()`
    pub owned_edges: Vec<crate::beliefbase::OwnedEdge>,
    /// External alias URLs for this node (from `url_aliases` or `alias-template`).
    /// Collected from the `WEIGHT_DOC_PATHS` payload on Section edges to
    /// `href_namespace`. Empty for nodes without aliases.
    /// ✅ JavaScript: This is an Array of strings.
    pub alias_urls: Vec<String>,
}

/// WASM-compatible path context
#[cfg(feature = "wasm")]
#[wasm_bindgen]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathParts {
    /// Original (undecomposed) path string -- used by canonicalize() to delegate to
    /// AnchorPath::canonicalize() without re-implementing the logic
    original: String,
    path: String,
    filename: String,
    anchor: String,
    has_schema: bool,
    schema: String,
}

#[cfg(feature = "wasm")]
#[wasm_bindgen]
impl PathParts {
    #[wasm_bindgen(getter)]
    pub fn path(&self) -> String {
        self.path.clone()
    }

    #[wasm_bindgen(getter)]
    pub fn filename(&self) -> String {
        self.filename.clone()
    }

    #[wasm_bindgen(getter)]
    pub fn anchor(&self) -> String {
        self.anchor.clone()
    }

    /// Whether the path contains a URL schema (e.g. `https`, `file`, `mailto`).
    /// True for any path of the form `schema:...` — use this to distinguish
    /// external URLs from internal document paths before attempting path joining.
    #[wasm_bindgen(getter, js_name = hasSchema)]
    pub fn has_schema(&self) -> bool {
        self.has_schema
    }

    /// The schema portion of the path (e.g. `"https"` for `https://example.com`).
    /// Returns an empty string when `has_schema` is false.
    #[wasm_bindgen(getter)]
    pub fn schema(&self) -> String {
        self.schema.clone()
    }

    /// Reassemple a canonical root-relative path string with no leading slash.
    #[wasm_bindgen]
    pub fn canonicalize(&self) -> String {
        AnchorPath::new(&self.original).canonicalize()
    }

    /// Use AnchorPath to return the filepath (no anchor, no params, no schema)
    #[wasm_bindgen]
    pub fn filepath(&self) -> String {
        AnchorPath::new(&self.original).filepath().to_string()
    }
}

/// WASM wrapper around BeliefBase for browser use
///
/// Provides JavaScript-accessible methods for querying beliefs loaded from JSON.
#[cfg(feature = "wasm")]
#[wasm_bindgen]
pub struct BeliefBaseWasm {
    inner: RefCell<BeliefBase>,
    entry_point_bid: Bid,
    /// Tracks which BIDs were loaded from which shard (keyed by bref string).
    /// Used by `unload_shard` to know which nodes to remove.
    /// The special key `"global"` is used for the global shard.
    loaded_shards: RefCell<HashMap<String, BTreeSet<Bid>>>,
    /// Loaded search indices keyed by network bref string.
    /// Populated by `load_search_index`; queried by `search`.
    search_indices: RefCell<HashMap<String, SearchIndex>>,
    /// Maps every node's bref (12 hex chars) to its home network's bref.
    /// Populated from the global shard's `bref_index` during `load_shard("global", ...)`.
    /// Queried by `network_bref_for_bref` to resolve which shard to load for
    /// an arbitrary node.
    bref_index: RefCell<BTreeMap<String, String>>,
}

#[cfg(feature = "wasm")]
#[wasm_bindgen]
impl BeliefBaseWasm {
    /// Set the WASM tracing log level at runtime.
    ///
    /// # JavaScript Example
    /// ```javascript,ignore
    /// BeliefBaseWasm.set_log_level("debug");  // verbose
    /// BeliefBaseWasm.set_log_level("info");
    /// BeliefBaseWasm.set_log_level("warn");   // default in release builds
    /// BeliefBaseWasm.set_log_level("error");
    /// BeliefBaseWasm.set_log_level("off");    // silence all tracing
    /// ```
    #[wasm_bindgen]
    pub fn set_log_level(level: &str) {
        use FromStr;
        let filter = tracing_subscriber::filter::LevelFilter::from_str(level)
            .unwrap_or(tracing_subscriber::filter::LevelFilter::WARN);
        if let Some(handle) = LOG_LEVEL_HANDLE.get() {
            if let Err(e) = handle.modify(|f| *f = filter) {
                console::warn_1(&format!("⚠️ Failed to set log level: {e}").into());
            } else {
                console::log_1(&format!("[Noet] Log level set to: {level}").into());
            }
        }
    }

    /// Get the entry point as a plain JS object `{ bid, bref }`.
    ///
    /// # JavaScript Example
    /// ```javascript,ignore
    /// const entryPoint = beliefbase.entryPoint();
    /// console.log(entryPoint.bid, entryPoint.bref);
    /// ```
    #[wasm_bindgen(js_name = entryPoint)]
    pub fn entry_point(&self) -> JsValue {
        BidBrefResult::from_bid(self.entry_point_bid).to_js()
    }
}

#[cfg(feature = "wasm")]
#[wasm_bindgen]
impl BeliefBaseWasm {
    /// Normalize a URL path by resolving `..` and `.` segments.
    ///
    /// This uses the `path_normalize` function from `paths.rs` which is designed
    /// for URL paths (always uses `/` separator, cross-platform safe).
    ///
    /// # Arguments
    /// * `path` - The path to normalize (e.g., "dir/../file.html" -> "file.html")
    ///
    /// # Returns
    /// The normalized path as a string
    #[wasm_bindgen(js_name = normalizePath)]
    pub fn normalize_path(path: &str) -> String {
        AnchorPath::new(path).normalize().to_string()
    }

    /// Parse a path into its components: directory, filename, and anchor.
    ///
    /// Returns a PathParts object with `path` (directory), `filename`, and `anchor` properties.
    ///
    /// # Arguments
    /// * `path` - The path to parse (e.g., "dir/file.html#section")
    ///
    /// # Returns
    /// PathParts object with path, filename, and anchor components
    #[wasm_bindgen(js_name = pathParts)]
    pub fn path_parts(path: &str) -> PathParts {
        let anchor_path = AnchorPath::new(path);
        PathParts {
            original: path.to_string(),
            path: anchor_path.dir().to_string(),
            filename: anchor_path.filename().to_string(),
            anchor: anchor_path.anchor().to_string(),
            has_schema: anchor_path.has_schema(),
            schema: anchor_path.schema().to_string(),
        }
    }

    /// Join two URL paths safely.
    ///
    /// # Arguments
    /// * `base` - The base path (e.g., "/dir/doc.html" or "/dir/")
    /// * `end` - The path to append (e.g., "other.html" or "../file.html")
    /// * `end_is_anchor` - Whether `end` is an anchor/section (uses # separator)
    ///
    /// # Returns
    /// The joined path as a string
    #[wasm_bindgen(js_name = pathJoin)]
    pub fn path_join(base: &str, end: &str, end_is_anchor: bool) -> String {
        let base_path = AnchorPath::new(base);
        if end_is_anchor {
            let end_with_hash = if end.starts_with('#') {
                end.to_string()
            } else {
                format!("#{}", end)
            };
            base_path.join(end_with_hash).to_string()
        } else {
            base_path.join(end).to_string()
        }
    }

    /// Get the file extension from a path, ignoring any anchor.
    ///
    /// # Arguments
    /// * `path` - The path to extract extension from (e.g., "file.html#section")
    ///
    /// # Returns
    /// The extension (e.g., "html") or empty string if none
    #[wasm_bindgen(js_name = pathExtension)]
    pub fn path_extension(path: &str) -> String {
        AnchorPath::new(path).ext().to_string()
    }

    /// Get the parent path (directory or document path without anchor).
    ///
    /// - For paths with anchors: returns path without anchor (e.g., "dir/file.html#section" → "dir/file.html")
    /// - For file paths: returns directory (e.g., "dir/file.html" → "dir")
    /// - For directory paths: returns parent directory (e.g., "dir/subdir" → "dir")
    ///
    /// # Arguments
    /// * `path` - The path to get parent of
    ///
    /// # Returns
    /// The parent path as a string
    #[wasm_bindgen(js_name = pathParent)]
    pub fn path_parent(path: &str) -> String {
        AnchorPath::new(path).parent().to_string()
    }

    /// Get the filename without extension (stem).
    ///
    /// # Arguments
    /// * `path` - The path to extract stem from (e.g., "dir/file.html#section")
    ///
    /// # Returns
    /// The filename without extension (e.g., "file")
    #[wasm_bindgen(js_name = pathFilestem)]
    pub fn path_filestem(path: &str) -> String {
        AnchorPath::new(path).filestem().to_string()
    }

    /// Create a BeliefBase from JSON string (exported beliefbase.json) and entry point BID
    ///
    /// # JavaScript Example
    /// ```javascript,ignore
    /// const response = await fetch('beliefbase.json');
    /// const json = await response.text();
    /// const entryBidScript = document.getElementById('noet-entry-bid');
    /// const entryBidStr = JSON.parse(entryBidScript.textContent);
    /// const bb = new BeliefBaseWasm(json, entryBidStr);
    /// ```
    #[wasm_bindgen(constructor)]
    pub fn from_json(data: String, entry_bid_str: String) -> Result<BeliefBaseWasm, JsValue> {
        // Parse JSON into BeliefGraph
        let graph: BeliefGraph = serde_json::from_str(&data).map_err(|e| {
            let msg = format!("❌ Failed to parse BeliefGraph JSON: {}", e);
            console::error_1(&msg.clone().into());
            JsValue::from_str(&msg)
        })?;

        let node_count = graph.states.len();
        let relation_count = graph.relations.0.edge_count();

        tracing::debug!(
            "Loaded BeliefGraph: {} nodes, {} relations",
            node_count,
            relation_count
        );

        // Parse entry point BID string directly
        let entry_point_bid = Bid::try_from(entry_bid_str.as_str()).map_err(|e| {
            let msg = format!("❌ Failed to parse entry point BID: {}", e);
            console::error_1(&msg.clone().into());
            JsValue::from_str(&msg)
        })?;

        tracing::debug!("Entry point BID: {}", entry_point_bid);

        // Convert BeliefGraph to BeliefBase
        let inner = BeliefBase::from(graph);

        Ok(BeliefBaseWasm {
            inner: RefCell::new(inner),
            entry_point_bid,
            loaded_shards: RefCell::new(HashMap::new()),
            search_indices: RefCell::new(HashMap::new()),
            bref_index: RefCell::new(BTreeMap::new()),
        })
    }

    #[wasm_bindgen]
    pub fn from_msgpack(
        data: Uint8Array,
        entry_bid_str: String,
    ) -> Result<BeliefBaseWasm, JsValue> {
        let bytes = data.to_vec();

        // Deserialize MessagePack bytes into BeliefGraph
        let graph: BeliefGraph = rmp_serde::from_slice(&bytes).map_err(|e| {
            let msg = format!("❌ Failed to parse BeliefGraph msgpack: {}", e);
            console::error_1(&msg.clone().into());
            JsValue::from_str(&msg)
        })?;

        let node_count = graph.states.len();
        let relation_count = graph.relations.0.edge_count();

        tracing::debug!(
            "Loaded BeliefGraph (msgpack): {} nodes, {} relations",
            node_count,
            relation_count
        );

        // Parse entry point BID string directly
        let entry_point_bid = Bid::try_from(entry_bid_str.as_str()).map_err(|e| {
            let msg = format!("❌ Failed to parse entry point BID: {}", e);
            console::error_1(&msg.clone().into());
            JsValue::from_str(&msg)
        })?;

        tracing::debug!("Entry point BID: {}", entry_point_bid);

        // Convert BeliefGraph to BeliefBase
        let inner = BeliefBase::from(graph);

        Ok(BeliefBaseWasm {
            inner: RefCell::new(inner),
            entry_point_bid,
            loaded_shards: RefCell::new(HashMap::new()),
            search_indices: RefCell::new(HashMap::new()),
            bref_index: RefCell::new(BTreeMap::new()),
        })
    }

    // =========================================================================
    // Shard-aware constructor and shard management
    // =========================================================================

    /// Construct a `BeliefBaseWasm` from a shard manifest JSON (sharded export mode).
    ///
    /// This creates an **empty** `BeliefBase` with the given entry point. The
    /// caller must subsequently call `load_shard` with `"global"` (the global
    /// shard JSON) and then with the entry-network bref (the per-network shard
    /// JSON) to populate the belief base with usable data.
    ///
    /// # Arguments
    ///
    /// * `manifest_json` — Contents of `beliefbase/manifest.json`
    /// * `entry_bid_str` — Full BID string of the entry-point network node
    ///
    /// # JavaScript Example
    /// ```javascript,ignore
    /// const manifestResp = await fetch("/beliefbase/manifest.json");
    /// const manifestJson = await manifestResp.text();
    /// const bb = BeliefBaseWasm.from_manifest(manifestJson, entryBidStr);
    /// // Now load the global shard and entry network shard:
    /// const globalResp = await fetch("/beliefbase/global.msgpack");
    /// await bb.load_shard("global", await globalResp.arrayBuffer());
    /// const entryResp = await fetch(`/beliefbase/networks/${entryBref}.msgpack`);
    /// await bb.load_shard(entryBref, await entryResp.arrayBuffer());
    /// ```
    #[wasm_bindgen]
    pub fn from_manifest(
        manifest_json: String,
        entry_bid_str: String,
    ) -> Result<BeliefBaseWasm, JsValue> {
        // Validate manifest JSON is well-formed (the JS ShardManager handles
        // full manifest parsing; we only need to confirm it's valid JSON here).
        let _manifest: serde_json::Value = serde_json::from_str(&manifest_json).map_err(|e| {
            let msg = format!("❌ Failed to parse shard manifest: {}", e);
            console::error_1(&msg.clone().into());
            JsValue::from_str(&msg)
        })?;

        let entry_point_bid = Bid::try_from(entry_bid_str.as_str()).map_err(|e| {
            let msg = format!("❌ Failed to parse entry point BID: {}", e);
            console::error_1(&msg.clone().into());
            JsValue::from_str(&msg)
        })?;

        tracing::debug!(
            "Shard manifest parsed. Entry point: {}. Call load_shard() to populate.",
            entry_point_bid
        );

        Ok(BeliefBaseWasm {
            inner: RefCell::new(BeliefBase::default()),
            entry_point_bid,
            loaded_shards: RefCell::new(HashMap::new()),
            search_indices: RefCell::new(HashMap::new()),
            bref_index: RefCell::new(BTreeMap::new()),
        })
    }

    /// Load a shard into the belief base, merging its nodes and relations.
    ///
    /// The shard JSON may be either:
    /// - A **global shard** (`beliefbase/global.json`) — use `bref = "global"`
    /// - A **per-network shard** (`beliefbase/networks/{bref}.json`) — use the
    ///   5-hex-char bref of that network as the key.
    ///
    /// Loading the same bref twice is a no-op (idempotent): the shard is
    /// unloaded first, then re-loaded from the new JSON.
    ///
    /// # Arguments
    ///
    /// * `bref_key` — `"global"` or a 5-hex-char network bref
    /// * `shard_json` — Raw JSON string for the shard
    ///
    /// # Returns
    ///
    /// The number of nodes now present in the belief base after loading.
    ///
    /// # JavaScript Example
    /// ```javascript,ignore
    /// const count = await bb.load_shard("global", globalBytes);
    /// console.log(`BeliefBase now has ${count} nodes`);
    /// ```
    #[wasm_bindgen]
    pub fn load_shard(&self, bref_key: String, data: Uint8Array) -> Result<usize, JsValue> {
        // If already loaded, unload first (idempotent reload).
        {
            let loaded = self.loaded_shards.borrow();
            if loaded.contains_key(&bref_key) {
                drop(loaded);
                self.unload_shard(bref_key.clone())?;
            }
        }

        // Convert Uint8Array → Vec<u8> for rmp_serde deserialization.
        let bytes = data.to_vec();

        // Deserialize: global shard and network shards have different schemas.
        #[allow(clippy::type_complexity)]
        let (states, edges): (
            BTreeMap<String, BeliefNode>,
            Vec<(Bid, Bid, crate::properties::WeightSet)>,
        ) = if bref_key == "global" {
            let shard: crate::shard::GlobalShard = rmp_serde::from_slice(&bytes).map_err(|e| {
                let msg = format!("❌ Failed to parse global shard: {}", e);
                console::error_1(&msg.clone().into());
                JsValue::from_str(&msg)
            })?;
            let edges = shard
                .relations
                .edges
                .into_iter()
                .filter_map(|e| {
                    let src = Bid::try_from(e.source.as_str()).ok()?;
                    let snk = Bid::try_from(e.sink.as_str()).ok()?;
                    Some((src, snk, e.weights))
                })
                .collect();

            // Store the bref→network_bref index for shard resolution queries.
            *self.bref_index.borrow_mut() = shard.bref_index;
            tracing::debug!(
                "Loaded bref_index with {} entries",
                self.bref_index.borrow().len()
            );

            (shard.states, edges)
        } else {
            let shard: crate::shard::NetworkShard = rmp_serde::from_slice(&bytes).map_err(|e| {
                let msg = format!("❌ Failed to parse network shard '{}': {}", bref_key, e);
                console::error_1(&msg.clone().into());
                JsValue::from_str(&msg)
            })?;
            let edges = shard
                .relations
                .edges
                .into_iter()
                .filter_map(|e| {
                    let src = Bid::try_from(e.source.as_str()).ok()?;
                    let snk = Bid::try_from(e.sink.as_str()).ok()?;
                    Some((src, snk, e.weights))
                })
                .collect();
            (shard.states, edges)
        };

        // Collect the BIDs being added so we can track them for unloading.
        let shard_bids: BTreeSet<Bid> = states
            .keys()
            .filter_map(|s| Bid::try_from(s.as_str()).ok())
            .collect();

        // Build a BeliefGraph from the shard data and merge it into the inner BeliefBase.
        let graph = {
            let relations = BidGraph::from_edges(edges);
            BeliefGraph {
                states: states
                    .into_iter()
                    .filter_map(|(k, v)| Some((Bid::try_from(k.as_str()).ok()?, v)))
                    .collect(),
                relations,
            }
        };

        let added_count = graph.states.len();
        let edge_count = graph.relations.as_graph().edge_count();

        self.inner.borrow_mut().merge(&graph);

        // Record the BIDs for this shard key.
        self.loaded_shards
            .borrow_mut()
            .insert(bref_key.clone(), shard_bids);

        let total = self.inner.borrow().states().len();
        tracing::debug!(
            "Loaded shard '{}': +{} nodes, {} edges → {} total nodes",
            bref_key,
            added_count,
            edge_count,
            total
        );
        Ok(total)
    }

    /// Unload a previously-loaded shard, removing its nodes from the belief base.
    ///
    /// Nodes that appear in multiple loaded shards (e.g. cross-references
    /// duplicated in the global shard) are **not** removed if they are still
    /// tracked by another loaded shard.
    ///
    /// # Arguments
    ///
    /// * `bref_key` — `"global"` or the 5-hex-char network bref used in `load_shard`
    ///
    /// # Returns
    ///
    /// The number of nodes remaining in the belief base after unloading.
    ///
    /// # JavaScript Example
    /// ```javascript,ignore
    /// const remaining = bb.unload_shard("abc12");
    /// console.log(`BeliefBase now has ${remaining} nodes`);
    /// ```
    #[wasm_bindgen]
    pub fn unload_shard(&self, bref_key: String) -> Result<usize, JsValue> {
        let shard_bids = {
            let mut loaded = self.loaded_shards.borrow_mut();
            match loaded.remove(&bref_key) {
                Some(bids) => bids,
                None => {
                    console::warn_1(
                        &format!("⚠️ unload_shard: '{}' was not loaded", bref_key).into(),
                    );
                    return Ok(self.inner.borrow().states().len());
                }
            }
        };

        // Do not remove BIDs that are still referenced by another loaded shard.
        let still_needed: BTreeSet<Bid> = self
            .loaded_shards
            .borrow()
            .values()
            .flat_map(|bids| bids.iter().copied())
            .collect();

        let to_remove: BTreeSet<Bid> = shard_bids
            .into_iter()
            .filter(|bid| !still_needed.contains(bid))
            .collect();

        let remove_count = to_remove.len();
        if !to_remove.is_empty() {
            let event = crate::event::BeliefEvent::NodesRemoved(
                to_remove.into_iter().collect(),
                crate::event::EventOrigin::Remote,
            );
            if let Err(e) = self.inner.borrow_mut().process_event(&event) {
                let msg = format!("❌ unload_shard '{}': remove failed: {}", bref_key, e);
                console::error_1(&msg.clone().into());
                return Err(JsValue::from_str(&msg));
            }

            // Clear the bref_index when the global shard is unloaded.
            if bref_key == "global" {
                self.bref_index.borrow_mut().clear();
            }
        }

        let total = self.inner.borrow().states().len();
        tracing::debug!(
            "Unloaded shard '{}': -{} nodes → {} total nodes",
            bref_key,
            remove_count,
            total
        );
        Ok(total)
    }

    /// Return the list of currently-loaded shard keys.
    ///
    /// Returns a JSON array of bref key strings (e.g. `["global", "abc12"]`).
    ///
    /// # JavaScript Example
    /// ```javascript,ignore
    /// const shards = JSON.parse(bb.loaded_shards());
    /// console.log(shards); // ["global", "abc12"]
    /// ```
    #[wasm_bindgen]
    pub fn loaded_shards(&self) -> String {
        let loaded = self.loaded_shards.borrow();
        let keys: Vec<&str> = loaded.keys().map(|s| s.as_str()).collect();
        serde_json::to_string(&keys).unwrap_or_else(|_| "[]".to_string())
    }

    /// Load a pre-built search index for one network from msgpack bytes.
    ///
    /// Call this once per network after fetching `search/{bref}.idx.msgpack`.
    /// The index is held in memory and queried by `search()`.
    ///
    /// # JavaScript Example
    /// ```javascript,ignore
    /// const resp = await fetch(`search/${bref}.idx.msgpack`);
    /// const buf  = await resp.arrayBuffer();
    /// beliefbase.load_search_index(bref, new Uint8Array(buf));
    /// ```
    ///
    /// # Arguments
    /// * `bref` — 5-hex-char network bref (the key used to store and retrieve the index)
    /// * `data` — Raw msgpack bytes of the serialized [`SearchIndex`]
    ///
    /// # Returns
    /// The number of indexed documents in the loaded index.
    #[wasm_bindgen]
    pub fn load_search_index(&self, bref: String, data: &[u8]) -> Result<usize, JsValue> {
        let index: SearchIndex = rmp_serde::from_slice(data).map_err(|e| {
            JsValue::from_str(&format!(
                "load_search_index: failed to deserialize msgpack for bref '{}': {}",
                bref, e
            ))
        })?;
        let doc_count = index.doc_count;
        self.search_indices.borrow_mut().insert(bref, index);
        Ok(doc_count)
    }

    /// Unload the search index for a network, freeing its memory.
    ///
    /// # JavaScript Example
    /// ```javascript,ignore
    /// beliefbase.unload_search_index(bref);
    /// ```
    #[wasm_bindgen]
    pub fn unload_search_index(&self, bref: String) {
        self.search_indices.borrow_mut().remove(&bref);
    }

    /// Returns the set of brefs for which a search index is currently loaded.
    ///
    /// # JavaScript Example
    /// ```javascript,ignore
    /// const loaded = beliefbase.loaded_search_indices();
    /// // loaded is a JS Set<string>
    /// ```
    #[wasm_bindgen]
    pub fn loaded_search_indices(&self) -> JsValue {
        let indices = self.search_indices.borrow();
        let arr = js_sys::Array::new();
        for key in indices.keys() {
            arr.push(&JsValue::from_str(key));
        }
        js_sys::Set::new(&arr.into()).into()
    }

    /// Return whether a BID is currently present in the loaded belief base.
    ///
    /// Useful for checking if a node is available before navigating to it,
    /// or for prompting the user to load a network shard.
    ///
    /// # JavaScript Example
    /// ```javascript,ignore
    /// if (!bb.has_bid(targetBid)) {
    ///   showLoadNetworkPrompt(targetBref);
    /// }
    /// ```
    #[wasm_bindgen]
    pub fn has_bid(&self, bid_str: String) -> bool {
        match Bid::try_from(bid_str.as_str()) {
            Ok(bid) => self.inner.borrow().states().contains_key(&bid),
            Err(_) => false,
        }
    }

    /// Return the estimated total memory used by loaded data shards (MB).
    ///
    /// Approximated as: `node_count * AVG_NODE_BYTES_MB`. This is a rough
    /// heuristic for UI display — not used for correctness-critical decisions.
    ///
    /// # JavaScript Example
    /// ```javascript,ignore
    /// const usedMb = bb.memory_usage_mb();
    /// ```
    #[wasm_bindgen]
    pub fn memory_usage_mb(&self) -> f64 {
        // Approximate: 2KB per node (title, payload, BID strings, graph overhead).
        const AVG_NODE_BYTES: f64 = 2048.0;
        let node_count = self.inner.borrow().states().len() as f64;
        (node_count * AVG_NODE_BYTES) / (1024.0 * 1024.0)
    }

    /// Query nodes using QuerySpec syntax.
    ///
    /// Returns `{ graph, tape_indices }` where `graph` is a `BeliefGraph`
    /// and `tape_indices` maps each result BID to the tape entry index
    /// where it was first discovered (for traversal depth display).
    ///
    /// # JavaScript Example
    /// ```javascript,ignore
    /// const result = bb.query(spec);
    /// const graph = result.graph;          // BeliefGraph
    /// const indices = result.tape_indices; // Map<bid, tape_index>
    /// ```
    #[wasm_bindgen]
    pub fn query(&self, spec_js: JsValue) -> Result<JsValue, JsValue> {
        // Deserialize QuerySpec from JavaScript
        let spec: QuerySpec = serde_wasm_bindgen::from_value(spec_js).map_err(|e| {
            let msg = format!("❌ Failed to parse QuerySpec: {}", e);
            console::error_1(&msg.clone().into());
            JsValue::from_str(&msg)
        })?;

        tracing::debug!("Query: {:?}", spec);

        // Evaluate synchronously — WASM has no async runtime.
        // Provide a TextSearchProvider backed by the loaded search indices
        // so that TextMatch filter steps can be evaluated.
        let indices = self.search_indices.borrow();
        let search_provider = WasmTextSearchProvider { indices: &indices };
        let inner = self.inner.borrow();
        let mut package = QueryPackage::new(spec);
        inner
            .evaluate_query_with_search(&mut package, Some(&search_provider))
            .map_err(|e| {
                let msg = format!("❌ Query evaluation failed: {}", e);
                console::error_1(&msg.clone().into());
                JsValue::from_str(&msg)
            })?;

        // Build tape index before consuming the package.
        let tape_indices = package.tape().bid_tape_indices();
        let graph = package.into_graph();
        let result_count = graph.states.len();
        tracing::debug!("Query returned {} nodes", result_count);

        // Return { graph, tape_indices } so the viewer can show traversal depth.
        let result = QueryResult {
            graph,
            tape_indices,
        };
        serde_wasm_bindgen::to_value(&result).map_err(|e| {
            let msg = format!("\u{274c} Failed to serialize result: {}", e);
            console::error_1(&msg.clone().into());
            JsValue::from_str(&msg)
        })
    }

    /// Evaluate a query and render the result through a view as JSON.
    ///
    /// `spec_js` — the `QuerySpec` (from `parseQuery` or manual construction).
    /// `view_key` — registered view key (`"depth0"`, `"connectivity"`,
    ///              `"maps_to"`, `"columns"`, `"raw_tape"`).
    /// `view_params_js` — optional view configuration table (JS object).
    ///                    Pass `null` or `undefined` for defaults.
    ///
    /// Returns the view's JSON output. Shape is view-specific.
    ///
    /// # JavaScript Example
    /// ```javascript,ignore
    /// const spec = BeliefBaseWasm.parseQuery("bref:abc composed_of(1)");
    /// const json = bb.queryView(spec, "connectivity", null);
    /// // json = { display: "Connectivity", headers: [...], rows: [...] }
    /// ```
    #[wasm_bindgen(js_name = queryView)]
    pub fn query_view(
        &self,
        spec_js: JsValue,
        view_key: &str,
        view_params_js: JsValue,
    ) -> Result<JsValue, JsValue> {
        let spec: QuerySpec = serde_wasm_bindgen::from_value(spec_js)
            .map_err(|e| JsValue::from_str(&format!("Failed to parse QuerySpec: {}", e)))?;

        // Build view params from JS object (or empty table).
        let params: toml::Table = if view_params_js.is_null() || view_params_js.is_undefined() {
            toml::Table::new()
        } else {
            serde_wasm_bindgen::from_value(view_params_js).unwrap_or_default()
        };

        // Look up the view factory.
        let factory = VIEWS
            .get(view_key)
            .ok_or_else(|| JsValue::from_str(&format!("Unknown view key: {view_key}")))?;
        let renderer =
            factory(&params).map_err(|e| JsValue::from_str(&format!("View factory error: {e}")))?;

        // Evaluate the query.
        let indices = self.search_indices.borrow();
        let search_provider = WasmTextSearchProvider { indices: &indices };
        let inner = self.inner.borrow();
        let mut package = QueryPackage::balanced(spec);
        inner
            .evaluate_query_with_search(&mut package, Some(&search_provider))
            .map_err(|e| JsValue::from_str(&format!("Query evaluation failed: {e}")))?;

        // Render as JSON.
        let output = renderer
            .render_json(&package)
            .map_err(|e| JsValue::from_str(&format!("View render error: {e}")))?;

        match output {
            ViewOutput::Json(mut json) => {
                // Inject pathmap order and tape depth for connectivity gutter.
                if let Some(obj) = json.as_object_mut() {
                    let mut order_map = serde_json::Map::new();
                    let tape_indices = package.tape().bid_tape_indices();
                    let mut depth_map = serde_json::Map::new();

                    // Anchor pathmap lookups to the SPA entry-point network
                    // so that cross-network queries produce contiguous orders.
                    let entry_bref = self.entry_point_bid.bref();

                    if let Some(nodes) = obj.get("nodes").and_then(|n| n.as_object()) {
                        for bid_str in nodes.keys() {
                            if let Ok(bid) = Bid::try_from(bid_str.as_str()) {
                                if let Some((_net, _path, order)) =
                                    inner.paths().net_indexed_path(&entry_bref, &bid)
                                {
                                    let order_str = order
                                        .iter()
                                        .map(|i| {
                                            if *i == crate::paths::NETWORK_SECTION_SORT_KEY {
                                                "\u{b7}".to_string() // middle dot: gateway index slot
                                            } else {
                                                i.to_string()
                                            }
                                        })
                                        .collect::<Vec<_>>()
                                        .join(".");
                                    order_map.insert(
                                        bid_str.clone(),
                                        serde_json::Value::String(order_str),
                                    );
                                }
                                if let Some(&depth) = tape_indices.get(&bid) {
                                    depth_map.insert(bid_str.clone(), serde_json::json!(depth));
                                }
                            }
                        }
                    }

                    obj.insert("order".to_string(), serde_json::Value::Object(order_map));
                    obj.insert(
                        "tape_depth".to_string(),
                        serde_json::Value::Object(depth_map),
                    );
                }

                // Use serialize_maps_as_objects so JSON objects become plain
                // JS objects (not Map instances), enabling dot-notation access.
                let serializer =
                    serde_wasm_bindgen::Serializer::new().serialize_maps_as_objects(true);
                json.serialize(&serializer)
                    .map_err(|e| JsValue::from_str(&format!("JSON serialization error: {e}")))
            }
            _ => Err(JsValue::from_str("View did not return JSON output")),
        }
    }

    /// Parse a query grammar string into a `QuerySpec` JS value.
    ///
    /// Returns the `QuerySpec` as a JS value (via `serde_wasm_bindgen`),
    /// directly compatible with `query()`. Throws on parse failure.
    ///
    /// # JavaScript Example
    /// ```javascript,ignore
    /// const spec = BeliefBaseWasm.parseQuery("bref:abc123 composed_of(1)");
    /// const graph = bb.query(spec);
    /// ```
    #[wasm_bindgen(js_name = parseQuery)]
    pub fn parse_query(input: &str) -> Result<JsValue, JsValue> {
        let spec = crate::query::parser::parse(input)
            .map_err(|e| JsValue::from_str(&format!("Query parse error: {}", e)))?;
        serde_wasm_bindgen::to_value(&spec)
            .map_err(|e| JsValue::from_str(&format!("Query serialization error: {}", e)))
    }

    /// Serialize a `QuerySpec` JSON object back to the textual query grammar.
    ///
    /// # JavaScript Example
    /// ```javascript,ignore
    /// const queryText = BeliefBaseWasm.serializeQuery(spec);
    /// // queryText = "id://my-node k-pragmatic-s(1)"
    /// ```
    #[wasm_bindgen(js_name = serializeQuery)]
    pub fn serialize_query(spec_js: JsValue) -> Result<String, JsValue> {
        let spec: QuerySpec = serde_wasm_bindgen::from_value(spec_js)
            .map_err(|e| JsValue::from_str(&format!("Failed to parse QuerySpec: {}", e)))?;
        Ok(crate::query::parser::serialize(&spec))
    }

    /// Get a node by BID (convenience wrapper around query)
    ///
    /// Returns null if node doesn't exist.
    ///
    /// # JavaScript Example
    /// ```javascript,ignore
    /// const node = bb.get_by_bid("01234567-89ab-cdef-0123-456789abcdef");
    /// if (node) {
    ///     console.log(node.title);
    /// }
    /// ```
    #[wasm_bindgen]
    pub fn get_by_bid(&self, bid: String) -> JsValue {
        let bid = match Bid::try_from(bid.as_str()) {
            Ok(b) => b,
            Err(_) => {
                console::warn_1(&format!("⚠️ Invalid BID format: {}", bid).into());
                return JsValue::NULL;
            }
        };

        let inner = self.inner.borrow();
        let node_key = NodeKey::Bid { bid };
        match inner.get(&node_key) {
            Some(node) => {
                tracing::debug!("Found node: {}", node.title);
                let js_val = serde_wasm_bindgen::to_value(&node).unwrap_or(JsValue::NULL);
                // Patch payload: toml::value::Table serializes as a JS Map via
                // serde_wasm_bindgen; JSON roundtrip produces a plain object so that
                // payload?.listing (and other plain-property accesses) work correctly.
                if js_val.is_object() {
                    if let Ok(payload_json) = serde_json::to_string(&node.payload) {
                        if let Ok(payload_js) = js_sys::JSON::parse(&payload_json) {
                            let _ =
                                Reflect::set(&js_val, &JsValue::from_str("payload"), &payload_js);
                        }
                    }
                }
                js_val
            }
            None => {
                console::warn_1(&format!("⚠️ Node not found: {}", bid).into());
                JsValue::NULL
            }
        }
    }

    /// Full-text TF-IDF search across all loaded search indices.
    ///
    /// Tokenizes the query using the same rules as the compile-time index builder
    /// (split on non-alphanumeric, lowercase, stop-word filter, Snowball English
    /// stemming). Returns up to `limit` results sorted by descending TF-IDF score.
    ///
    /// Requires search indices to be loaded via `load_search_index` first. If no
    /// indices are loaded, returns an empty array.
    ///
    /// Snippet extraction is not performed here — call `get_by_bid` on results
    /// whose network is loaded to access `payload["text"]` for snippet rendering.
    ///
    /// # JavaScript Example
    /// ```javascript,ignore
    /// const results = JSON.parse(beliefbase.search("installation guide", 20));
    /// results.forEach(r => console.log(r.score, r.title, r.bid));
    /// ```
    ///
    /// # Arguments
    /// * `query` — Raw query string (tokenized internally)
    /// * `limit` — Maximum results to return (0 = no limit)
    ///
    /// # Returns
    /// JSON string: `[{ bid, network_bref, title, path, score }, ...]`
    #[wasm_bindgen]
    pub fn search(&self, query: String, limit: usize) -> String {
        let indices = self.search_indices.borrow();
        let idx_refs: Vec<&SearchIndex> = indices.values().collect();
        if idx_refs.is_empty() {
            return "[]".to_string();
        }
        let results = query_search_index(&idx_refs, &query, limit);
        serde_json::to_string(&results).unwrap_or_else(|_| "[]".to_string())
    }

    /// Get total number of nodes in the belief base
    ///
    /// # JavaScript Example
    /// ```javascript,ignore
    /// console.log(`Loaded ${bb.node_count()} nodes`);
    /// ```
    #[wasm_bindgen]
    pub fn node_count(&self) -> usize {
        self.inner.borrow().states().len()
    }

    /// Get BID from a bref string
    ///
    /// Returns the BID corresponding to the given bref, or null if not found.
    ///
    /// # JavaScript Example
    /// ```javascript,ignore
    /// const bid = bb.get_bid_from_bref("abc123456789");
    /// if (bid) {
    ///     const node = bb.get_by_bid(bid);
    ///     console.log(node.title);
    /// }
    /// ```
    #[wasm_bindgen]
    pub fn get_bid_from_bref(&self, bref: String) -> JsValue {
        let bref = match Bref::try_from(bref.as_str()) {
            Ok(b) => b,
            Err(_) => {
                console::warn_1(&format!("⚠️ Invalid bref format: {}", bref).into());
                return JsValue::NULL;
            }
        };

        let inner = self.inner.borrow();
        match inner.brefs().get(&bref) {
            Some(bid) => {
                tracing::debug!("Resolved bref to BID: {}", bid);
                JsValue::from_str(&bid.to_string())
            }
            None => {
                console::warn_1(&format!("⚠️ Bref not found: {}", bref).into());
                JsValue::NULL
            }
        }
    }

    /// Get bref from BID
    ///
    /// Converts a full BID (36 chars) to its compact bref (12 chars).
    /// Useful for generating `bref://` links from BIDs.
    ///
    /// # JavaScript Example
    /// ```javascript,ignore
    /// const bid = "1f10cfd9-1cc3-6a93-86f9-0e90d9cb2fdb";
    /// const bref = beliefbase.get_bref_from_bid(bid);
    /// console.log(`bref://${bref}`); // "bref://1f10cfd91cc3"
    /// ```
    #[wasm_bindgen]
    pub fn get_bref_from_bid(&self, bid: String) -> JsValue {
        let bid = match Bid::try_from(bid.as_str()) {
            Ok(b) => b,
            Err(_) => {
                console::warn_1(&format!("⚠️ Invalid BID format: {}", bid).into());
                return JsValue::NULL;
            }
        };

        // Use Bid.bref() method directly
        let bref = bid.bref();
        tracing::debug!("Converted BID to bref: {}", bref);
        JsValue::from_str(&bref.to_string())
    }

    /// Get BID and bref from node ID (e.g., section header id attribute)
    ///
    /// Looks up a node by its `id` field within a specific network.
    /// Returns an object with both `bid` and `bref` to avoid double WASM calls.
    ///
    /// # JavaScript Example
    /// ```javascript,ignore
    /// // Get BID and bref for a section with id="introduction"
    /// const result = beliefbase.get_bid_from_id(entryPoint.bref, "introduction");
    /// if (result) {
    ///     console.log(`BID: ${result.bid}, bref: ${result.bref}`);
    ///     const context = beliefbase.get_context(result.bid);
    /// }
    /// ```
    #[wasm_bindgen]
    pub fn get_bid_from_id(&self, net_bref: String, id: String) -> JsValue {
        let bref = match Bref::try_from(net_bref.as_str()) {
            Ok(b) => b,
            Err(_) => {
                console::warn_1(&format!("⚠️ Invalid bref format: {}", net_bref).into());
                return JsValue::NULL;
            }
        };

        let inner = self.inner.borrow();
        let paths = inner.paths();

        match paths.net_get_from_id(&bref, &id) {
            Some((net_bid, node_bid)) => {
                let node_bref = node_bid.bref();
                tracing::debug!(
                    "Resolved id '{}' to BID: {} (bref: {}, net: {})",
                    id,
                    node_bid,
                    node_bref,
                    net_bid
                );

                BidBrefResult::from_bid(node_bid).to_js()
            }
            None => {
                tracing::debug!("No node found with id '{}' in network {}", id, bref);
                JsValue::NULL
            }
        }
    }

    /// Get all network nodes (convenience wrapper around query)
    ///
    /// Returns array of nodes with kind "Network".
    ///
    /// # JavaScript Example
    /// ```javascript,ignore
    /// const networks = bb.get_networks();
    /// networks.forEach(net => console.log(net.title));
    /// ```
    #[wasm_bindgen]
    pub fn get_networks(&self) -> JsValue {
        let inner = self.inner.borrow();
        let networks: Vec<&BeliefNode> = inner
            .states()
            .values()
            .filter(|n| n.kind.contains(BeliefKind::Network))
            .collect();
        tracing::debug!("Found {} networks", networks.len());

        serde_wasm_bindgen::to_value(&networks).unwrap_or(JsValue::NULL)
    }

    /// Get all document nodes (convenience wrapper around query)
    ///
    /// Returns array of nodes with kind "Document".
    ///
    /// # JavaScript Example
    /// ```javascript,ignore
    /// const docs = bb.get_documents();
    /// console.log(`${docs.length} documents`);
    /// ```
    #[wasm_bindgen]
    pub fn get_documents(&self) -> JsValue {
        let inner = self.inner.borrow();
        let documents: Vec<&BeliefNode> = inner
            .states()
            .values()
            .filter(|n| n.kind.contains(BeliefKind::Document))
            .collect();
        tracing::debug!("Found {} documents", documents.len());

        serde_wasm_bindgen::to_value(&documents).unwrap_or(JsValue::NULL)
    }

    /// Get full context for a node (NodeContext with relations and external refs)
    ///
    /// Returns NodeContext with:
    /// - The node itself
    /// - Home network path
    /// - External references (href/asset networks)
    /// - Full relation graph (sources, sinks)
    ///
    /// ⚠️ **JavaScript**: `related_nodes` and `graph` are Map objects (not plain objects)!
    ///
    /// # JavaScript Example
    /// ```javascript,ignore
    /// const ctx = bb.get_context("01234567-89ab-cdef-0123-456789abcdef");
    /// console.log(`Node: ${ctx.node.title}`);
    /// console.log(`Path: ${ctx.root_path}`);
    ///
    /// // ⚠️ IMPORTANT: Use Map methods, NOT Object methods
    /// console.log(`Related nodes: ${ctx.related_nodes.size}`);  // ✅ Correct
    /// const relNode = ctx.related_nodes.get(someBid);           // ✅ Correct
    /// for (const [bid, relNode] of ctx.related_nodes.entries()) { ... }  // ✅ Correct
    ///
    /// // ❌ WRONG: Object.keys(ctx.related_nodes) returns []
    /// // ❌ WRONG: ctx.related_nodes[bid] returns undefined
    /// ```
    fn extract_node_context(&self, ns: &Bid, bid: &Bid) -> Option<NodeContext> {
        /// Convert a `toml::value::Table` to a `serde_json::Value::Object` so it
        /// serializes as a plain JS object (not a Map) via `serde_wasm_bindgen`.
        ///
        /// On wasm32, `crate::codec::belief_ir` is not compiled, so this is a local
        /// copy. The canonical native implementation lives in
        /// `crate::codec::belief_ir::toml_value_to_json`.
        fn toml_value_to_json(v: &toml::Value) -> serde_json::Value {
            match v {
                toml::Value::String(s) => serde_json::Value::String(s.clone()),
                toml::Value::Integer(i) => serde_json::Value::Number((*i).into()),
                toml::Value::Float(f) => serde_json::Number::from_f64(*f)
                    .map(serde_json::Value::Number)
                    .unwrap_or(serde_json::Value::Null),
                toml::Value::Boolean(b) => serde_json::Value::Bool(*b),
                toml::Value::Array(arr) => {
                    serde_json::Value::Array(arr.iter().map(toml_value_to_json).collect())
                }
                toml::Value::Table(t) => serde_json::Value::Object(
                    t.iter()
                        .map(|(k, v)| (k.clone(), toml_value_to_json(v)))
                        .collect(),
                ),
                toml::Value::Datetime(dt) => serde_json::Value::String(dt.to_string()),
            }
        }

        // Single shared borrow: get_context now takes &self, so we can hold
        // the BeliefContext and bref_index simultaneously.
        let inner = self.inner.borrow();
        let ctx = inner.get_context(ns, bid)?;
        let bref_idx = self.bref_index.borrow();

        // Owned edges (endpoint + declared perspectives, deduplicated).
        let owned_edges = ctx.all_owned_edges();

        // Build a lookup index: (source_bid, sink_bid, weight_kind) → owner_bid
        // for O(1) lookup during the sources/sinks graph-building loops below.
        let owned_edge_index: HashMap<(Bid, Bid, WeightKind), Bid> = owned_edges
            .iter()
            .map(|oe| ((oe.source_bid, oe.sink_bid, oe.weight_kind), oe.owner_bid))
            .collect();

        // Collect all related nodes (other end of all edges)
        let mut related_nodes = BTreeMap::new();
        type GraphMap = HashMap<WeightKind, (Vec<(EdgeEntry, u16)>, Vec<(EdgeEntry, u16)>)>;
        let mut graph: GraphMap = HashMap::new();

        // Process sources (nodes linking TO this one).
        // Resolve home_net and root_path using the bref_index as the
        // authoritative source.  The bref_index maps every node's bref to its
        // true home network bref, preventing PathMap pollution from extern
        // Section edges.
        for ext_rel in ctx.sources() {
            let (home_net, root_path) = Self::resolve_related_path(
                ext_rel.other.bid,
                ext_rel.home_net,
                &ext_rel.root_path,
                &bref_idx,
                &inner,
            );

            let related_node = RelatedNode {
                node: ext_rel.other.clone(),
                home_net,
                root_path,
                link_title: ext_rel.link_title.clone(),
            };
            related_nodes.insert(ext_rel.other.bid, related_node);

            // Group by weight kind and collect with sort_key
            for (kind, weight) in ext_rel.weight.weights.iter() {
                let sort_key: u16 = weight.get::<u16>(WEIGHT_SORT_KEY).unwrap_or(0);
                let owner_bid = owned_edge_index
                    .get(&(ext_rel.other.bid, *bid, *kind))
                    .copied();
                let entry = EdgeEntry::new(ext_rel.other.bid, owner_bid);
                graph
                    .entry(*kind)
                    .or_insert_with(|| (Vec::new(), Vec::new()))
                    .0
                    .push((entry, sort_key));
            }
        }

        // Process sinks (nodes this one links TO)
        let mut alias_urls: Vec<String> = Vec::new();
        for ext_rel in ctx.sinks() {
            let (home_net, root_path) = Self::resolve_related_path(
                ext_rel.other.bid,
                ext_rel.home_net,
                &ext_rel.root_path,
                &bref_idx,
                &inner,
            );

            let related_node = RelatedNode {
                node: ext_rel.other.clone(),
                home_net,
                root_path,
                link_title: ext_rel.link_title.clone(),
            };
            related_nodes.insert(ext_rel.other.bid, related_node);

            // Collect alias URLs from Section sinks to href_namespace.
            // The alias URL is stored in WEIGHT_DOC_PATHS on the edge weight.
            if ext_rel.other.bid == href_namespace() {
                if let Some(section_weight) = ext_rel.weight.weights.get(&WeightKind::Section) {
                    if let Some(paths) = section_weight.get::<Vec<String>>(WEIGHT_DOC_PATHS) {
                        alias_urls.extend(paths);
                    }
                }
            }

            // Group by weight kind and collect with sort_key
            for (kind, weight) in ext_rel.weight.weights.iter() {
                let sort_key: u16 = weight.get::<u16>(WEIGHT_SORT_KEY).unwrap_or(0);
                let owner_bid = owned_edge_index
                    .get(&(*bid, ext_rel.other.bid, *kind))
                    .copied();
                let entry = EdgeEntry::new(ext_rel.other.bid, owner_bid);
                graph
                    .entry(*kind)
                    .or_insert_with(|| (Vec::new(), Vec::new()))
                    .1
                    .push((entry, sort_key));
            }
        }

        // Sort all vectors by sort_key and extract just the EdgeEntries
        let sorted_graph: HashMap<WeightKind, (Vec<EdgeEntry>, Vec<EdgeEntry>)> = graph
            .into_iter()
            .map(|(kind, (mut sources, mut sinks))| {
                sources.sort_by_key(|(_, sort_key)| *sort_key);
                sinks.sort_by_key(|(_, sort_key)| *sort_key);
                (
                    kind,
                    (
                        sources.into_iter().map(|(entry, _)| entry).collect(),
                        sinks.into_iter().map(|(entry, _)| entry).collect(),
                    ),
                )
            })
            .collect();

        Some(NodeContext {
            node: ctx.node.clone(),
            root_path: if *bid == asset_namespace() || bid.parent_bref() == asset_namespace().bref()
            {
                ctx.root_path.clone()
            } else {
                normalize_path_extension_impl(&ctx.root_path)
            },
            home_net: ctx.home_net,
            metadata: serde_json::Value::Object(
                ctx.node
                    .metadata
                    .iter()
                    .map(|(k, v)| (k.clone(), toml_value_to_json(v)))
                    .collect(),
            ),
            related_nodes,
            graph: sorted_graph,
            owned_edges,
            alias_urls,
        })
    }

    /// Patch a serialized `NodeContext` `JsValue` so that `metadata` and `node.payload`
    /// are plain JS objects rather than `Map` instances.
    ///
    /// `serde_wasm_bindgen` serializes map-like types (including `serde_json::Value::Object`
    /// nested inside a struct) as JS `Map` objects. This helper re-serializes those two
    /// fields through `js_sys::JSON::parse` so callers get plain objects instead.
    fn patch_node_context_js(js_val: &JsValue, node_context: &NodeContext) {
        if !js_val.is_object() {
            return;
        }
        // Patch metadata: serde_json::Value::Object → plain JS object.
        if let Ok(metadata_json) = serde_json::to_string(&node_context.metadata) {
            if let Ok(metadata_js) = js_sys::JSON::parse(&metadata_json) {
                let _ = Reflect::set(js_val, &JsValue::from_str("metadata"), &metadata_js);
            }
        }
        // Patch node.payload: toml::value::Table → plain JS object.
        // Without this, payload?.listing returns undefined (it's a Map entry, not a
        // plain property) and the directory listing panel never renders.
        if let Ok(payload_json) = serde_json::to_string(&node_context.node.payload) {
            if let Ok(payload_js) = js_sys::JSON::parse(&payload_json) {
                if let Ok(node_js) = Reflect::get(js_val, &JsValue::from_str("node")) {
                    if node_js.is_object() {
                        let _ = Reflect::set(&node_js, &JsValue::from_str("payload"), &payload_js);
                    }
                }
            }
        }
    }

    /// Resolve the authoritative home_net and root_path for a related node.
    ///
    /// Uses the `bref_index` to find the node's true home network, then looks
    /// up the path directly from that network's PathMap.  Falls back to the
    /// caller-provided values when the bref_index is empty (monolithic mode)
    /// or when the node is not in the index (global-only nodes like namespace
    /// roots).
    ///
    /// This prevents the PathMap pollution problem where extern Section edges
    /// cause a node to appear in the wrong network's PathMap, producing
    /// incorrect root_path values (e.g. "/index.html" instead of the correct
    /// cross-network path).
    fn resolve_related_path(
        other_bid: Bid,
        fallback_home_net: Bid,
        fallback_root_path: &str,
        bref_idx: &BTreeMap<String, String>,
        inner: &BeliefBase,
    ) -> (Bid, String) {
        // Asset-namespace paths are opaque repo-relative identifiers (e.g.
        // "net1_dir1", "assets/img.png") — not navigable HTML paths.
        if fallback_home_net == asset_namespace() {
            return (fallback_home_net, fallback_root_path.to_string());
        }

        // If the bref_index is available, use it as the authoritative source.
        if !bref_idx.is_empty() {
            let node_bref_str = other_bid.bref().to_string();
            if let Some(authoritative_net_bref) = bref_idx.get(&node_bref_str) {
                // Convert the network bref string to a Bref and use it directly
                // as the PathMap key.  This avoids the brefs→BID→bref() round-trip
                // that fails when the network node's shard isn't loaded (its BID
                // wouldn't be in inner.brefs()).
                if let Ok(net_bref) = Bref::try_from(authoritative_net_bref.as_str()) {
                    let paths = inner.paths();
                    let root_path = paths
                        .get_map(&net_bref)
                        .and_then(|pm| pm.path(&other_bid, &paths))
                        .map(|(_home, path, _order)| normalize_path_extension_impl(&path))
                        .unwrap_or_default();

                    // Resolve the network BID if available (for home_net field);
                    // fall back to ext_rel.home_net when the network node itself
                    // isn't loaded.
                    let home_net = inner
                        .brefs()
                        .get(&net_bref)
                        .copied()
                        .unwrap_or(fallback_home_net);

                    return (home_net, root_path);
                }
            }
        }

        // Fallback: use the caller-provided values (monolithic mode or
        // global-only nodes not in the bref_index).
        (
            fallback_home_net,
            normalize_path_extension_impl(fallback_root_path),
        )
    }

    /// Look up the home network bref for a given node BID.
    ///
    /// Computes the node's bref from the BID, then consults the `bref_index`
    /// (populated from the global shard) to resolve which network shard
    /// contains the node. Returns `JsValue::NULL` if the BID is invalid or
    /// the node is not in the index (e.g. global-only nodes).
    ///
    /// # JavaScript Example
    /// ```javascript,ignore
    /// const netBref = bb.network_bref_for_bid("1f1401ac-a462-67c0-b3ff-34a7b264ac4f");
    /// if (netBref) {
    ///     await shardManager.loadNetwork(netBref);
    /// }
    /// ```
    #[wasm_bindgen]
    pub fn network_bref_for_bid(&self, bid: String) -> JsValue {
        let bid = match Bid::try_from(bid.as_str()) {
            Ok(b) => b,
            Err(_) => return JsValue::NULL,
        };
        let bref_str = bid.bref().to_string();
        let index = self.bref_index.borrow();
        match index.get(&bref_str) {
            Some(net_bref) => JsValue::from_str(net_bref),
            None => JsValue::NULL,
        }
    }

    #[wasm_bindgen]
    pub fn get_context(&self, bid: String) -> JsValue {
        let bid = match Bid::try_from(bid.as_str()) {
            Ok(b) => b,
            Err(_) => {
                console::warn_1(&format!("⚠️ Invalid BID format: {}", bid).into());
                return JsValue::NULL;
            }
        };

        // Try multiple networks with helper function to extract data immediately
        let Some(node_context) = self
            .extract_node_context(&self.entry_point_bid, &bid)
            .or_else(|| self.extract_node_context(&href_namespace(), &bid))
            .or_else(|| self.extract_node_context(&asset_namespace(), &bid))
        else {
            // Not found in any namespace
            console::warn_1(&format!("⚠️ Node not found in any context: {}", bid).into());
            tracing::debug!(
                "Entry point: {}; tried namespaces: href, asset, buildonomy",
                self.entry_point_bid
            );
            return JsValue::NULL;
        };

        tracing::debug!("Got context for node: {}", node_context.node.title);

        // serde_wasm_bindgen v0.6 serializes any map-like Serde type (including
        // serde_json::Value::Object) as a JS Map rather than a plain object.
        // To guarantee `metadata` is a plain JS object (so `metadata?.source_url`
        // works), we:
        //   1. Serialize the full NodeContext via serde_wasm_bindgen (gets us the
        //      struct fields as a plain JS object, but metadata and node.payload come
        //      out as JS Maps because toml::value::Table serializes via serialize_map).
        //   2. Re-serialize `metadata` and `node.payload` to JSON strings and parse via
        //      js_sys::JSON::parse — the JS JSON parser always produces plain objects.
        //   3. Patch both properties on the returned JS object.
        let js_val = serde_wasm_bindgen::to_value(&node_context).unwrap_or(JsValue::NULL);
        Self::patch_node_context_js(&js_val, &node_context);
        js_val
    }

    /// Get all paths in a network as an Array of `{ path, bid, order, is_network }` objects.
    ///
    /// - `network_bid`: BID string of the network to query.
    /// - `entry_bid`: BID string of the node to scope the submap from (empty string = entire network).
    /// - `depth`: subnet expansion depth. `0` = no subnet expansion, `255` = fully recursive.
    /// - `include_index`: if `false`, index-file headings/sections are filtered out.
    ///
    /// Returns a JS Array of plain objects:
    /// ```javascript,ignore
    /// [{ path: "docs/guide.md", bid: "...", order: [1, 2], is_network: false }, ...]
    /// ```
    #[wasm_bindgen]
    pub fn get_submap(
        &self,
        network_bid: String,
        entry_bid: String,
        depth: u8,
        include_index: bool,
    ) -> JsValue {
        let net_bid = match Bid::try_from(network_bid.as_str()) {
            Ok(b) => b,
            Err(_) => {
                console::warn_1(
                    &format!("⚠️ get_submap: invalid network BID: {}", network_bid).into(),
                );
                return JsValue::NULL;
            }
        };

        let entry: Option<Bid> = if entry_bid.is_empty() {
            None
        } else {
            match Bid::try_from(entry_bid.as_str()) {
                Ok(b) => Some(b),
                Err(_) => {
                    console::warn_1(
                        &format!("⚠️ get_submap: invalid entry BID: {}", entry_bid).into(),
                    );
                    return JsValue::NULL;
                }
            }
        };

        let inner = self.inner.borrow();
        let paths = inner.paths();
        let entries = paths.submap_by_bid(&net_bid.bref(), entry, depth, include_index);

        let arr = js_sys::Array::new();
        for (entry_path, entry_bid, order) in entries {
            let is_network = paths.get_map(&entry_bid.bref()).is_some();
            let obj = Object::new();
            let _ = Reflect::set(
                &obj,
                &JsValue::from_str("path"),
                &JsValue::from_str(&entry_path),
            );
            let _ = Reflect::set(
                &obj,
                &JsValue::from_str("bid"),
                &JsValue::from_str(&entry_bid.to_string()),
            );
            // Serialize order as a JS Array of numbers
            let order_arr = js_sys::Array::new();
            for v in &order {
                order_arr.push(&JsValue::from_f64(*v as f64));
            }
            let _ = Reflect::set(&obj, &JsValue::from_str("order"), &order_arr);
            let _ = Reflect::set(
                &obj,
                &JsValue::from_str("is_network"),
                &JsValue::from_bool(is_network),
            );
            arr.push(&obj);
        }
        arr.into()
    }

    /// Get context for multiple BIDs in a single call.
    ///
    /// Accepts a list of BID strings. Invalid BIDs are warned and skipped.
    /// Returns a JS `Map` keyed by BID string, valued by serialized `NodeContext`
    /// (same shape as `get_context`).
    ///
    /// ```javascript,ignore
    /// const map = beliefbase.get_context_bulk(["bid1", "bid2"]);
    /// const ctx = map.get("bid1");  // same shape as get_context()
    /// ```
    #[wasm_bindgen]
    pub fn get_context_bulk(&self, bids: Vec<String>) -> JsValue {
        let result = js_sys::Map::new();

        for bid_str in bids {
            let bid = match Bid::try_from(bid_str.as_str()) {
                Ok(b) => b,
                Err(_) => {
                    console::warn_1(
                        &format!("⚠️ get_context_bulk: invalid BID: {}", bid_str).into(),
                    );
                    continue;
                }
            };

            let node_context = self
                .extract_node_context(&self.entry_point_bid, &bid)
                .or_else(|| self.extract_node_context(&href_namespace(), &bid))
                .or_else(|| self.extract_node_context(&asset_namespace(), &bid));

            if let Some(nc) = node_context {
                let js_val = serde_wasm_bindgen::to_value(&nc).unwrap_or(JsValue::NULL);
                Self::patch_node_context_js(&js_val, &nc);
                result.set(&JsValue::from_str(&bid_str), &js_val);
            }
        }

        result.into()
    }

    /// Export an XLSX spreadsheet from a pre-built export row array.
    ///
    /// `headers` is a JS `Array<string>` giving the ordered column keys.
    /// `rows` is a JS `Array<Object>` where each object maps column key → cell
    /// value (string). This mirrors what `buildExportRows()` in `traceability.js`
    /// already produces for the CSV path.
    ///
    /// Returns a `Uint8Array` containing the raw `.xlsx` bytes, ready to be
    /// wrapped in a `Blob` and triggered as a download.
    ///
    /// # JavaScript Example
    /// ```javascript,ignore
    /// const rows  = buildExportRows();          // [{path:"...", section_in:"..."}, ...]
    /// const keys  = Object.keys(rows[0]);       // ["path", "section_in", ...]
    /// const hdrs  = js_sys::Array::from(&keys); // already a JS Array
    /// const bytes = bb.export_xlsx(hdrs, rows_array);
    /// const blob  = new Blob([bytes], { type: "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" });
    /// const url   = URL.createObjectURL(blob);
    /// ```
    ///
    /// Returns an empty `Uint8Array` on error (errors are logged to console).
    #[cfg(feature = "wasm")]
    #[wasm_bindgen]
    pub fn export_xlsx(headers: js_sys::Array, rows: js_sys::Array) -> Uint8Array {
        match Self::build_xlsx_bytes(headers, rows) {
            Ok(bytes) => {
                let arr = Uint8Array::new_with_length(bytes.len() as u32);
                arr.copy_from(&bytes);
                arr
            }
            Err(e) => {
                console::error_1(&format!("export_xlsx error: {e}").into());
                Uint8Array::new_with_length(0)
            }
        }
    }

    #[cfg(feature = "wasm")]
    fn build_xlsx_bytes(headers: js_sys::Array, rows: js_sys::Array) -> Result<Vec<u8>, XlsxError> {
        let mut workbook = Workbook::new();
        let worksheet = workbook.add_worksheet();

        let bold = Format::new().set_bold();

        // Collect header key order
        let keys: Vec<String> = headers
            .iter()
            .map(|v| v.as_string().unwrap_or_default())
            .collect();

        // Write header row
        for (col_idx, key) in keys.iter().enumerate() {
            worksheet.write_with_format(0, col_idx as u16, key.as_str(), &bold)?;
        }

        // Write data rows
        for (row_idx, row_val) in rows.iter().enumerate() {
            let row_obj = js_sys::Object::from(row_val);
            for (col_idx, key) in keys.iter().enumerate() {
                let cell_val = Reflect::get(&row_obj, &JsValue::from_str(key))
                    .ok()
                    .and_then(|v| v.as_string())
                    .unwrap_or_default();
                worksheet.write(1 + row_idx as u32, col_idx as u16, cell_val.as_str())?;
            }
        }

        workbook.save_to_buffer()
    }

    /// Export multiple sheets as a single `.xlsx` workbook.
    ///
    /// `sheets` is a JS Array of objects, each with `{ name, headers, rows }`.
    /// Each sheet gets a bold header row and plain data rows.
    /// Returns a `Uint8Array` containing the raw `.xlsx` bytes.
    #[cfg(feature = "wasm")]
    #[wasm_bindgen]
    pub fn export_xlsx_multi(sheets: js_sys::Array) -> Uint8Array {
        match Self::build_xlsx_multi_bytes(sheets) {
            Ok(bytes) => {
                let arr = Uint8Array::new_with_length(bytes.len() as u32);
                arr.copy_from(&bytes);
                arr
            }
            Err(e) => {
                console::error_1(&format!("export_xlsx_multi error: {e}").into());
                Uint8Array::new_with_length(0)
            }
        }
    }

    #[cfg(feature = "wasm")]
    fn build_xlsx_multi_bytes(sheets: js_sys::Array) -> Result<Vec<u8>, XlsxError> {
        let mut workbook = Workbook::new();
        let bold = Format::new().set_bold();

        for sheet_val in sheets.iter() {
            let sheet_obj = js_sys::Object::from(sheet_val);
            let name = Reflect::get(&sheet_obj, &JsValue::from_str("name"))
                .ok()
                .and_then(|v| v.as_string())
                .unwrap_or_else(|| "Sheet".to_string());
            let headers = Reflect::get(&sheet_obj, &JsValue::from_str("headers"))
                .ok()
                .map(|v| js_sys::Array::from(&v))
                .unwrap_or_default();
            let rows = Reflect::get(&sheet_obj, &JsValue::from_str("rows"))
                .ok()
                .map(|v| js_sys::Array::from(&v))
                .unwrap_or_default();

            // Truncate sheet name to 31 chars (Excel limit).
            let sheet_name = if name.len() > 31 {
                name[..31].to_string()
            } else {
                name
            };

            let worksheet = workbook.add_worksheet();
            worksheet.set_name(&sheet_name)?;

            let keys: Vec<String> = headers
                .iter()
                .map(|v| v.as_string().unwrap_or_default())
                .collect();

            for (col_idx, key) in keys.iter().enumerate() {
                worksheet.write_with_format(0, col_idx as u16, key.as_str(), &bold)?;
            }

            for (row_idx, row_val) in rows.iter().enumerate() {
                let row_obj = js_sys::Object::from(row_val);
                for (col_idx, key) in keys.iter().enumerate() {
                    let cell_val = Reflect::get(&row_obj, &JsValue::from_str(key))
                        .ok()
                        .and_then(|v| v.as_string())
                        .unwrap_or_default();
                    worksheet.write(1 + row_idx as u32, col_idx as u16, cell_val.as_str())?;
                }
            }
        }

        workbook.save_to_buffer()
    }

    /// Get href namespace BID (external HTTP/HTTPS links tracking network)
    ///
    /// See `docs/design/architecture.md` § 10 for network namespace details.
    ///
    /// # JavaScript Example
    /// ```javascript,ignore
    /// const href_bid = BeliefBaseWasm.href_namespace();
    /// ```
    #[wasm_bindgen]
    pub fn href_namespace() -> JsValue {
        BidBrefResult::from_bid(href_namespace()).to_js()
    }

    /// Get all content namespace BIDs (href + asset — namespaces that track external
    /// content anchored to the parsed repo). Path resolution for these namespaces is
    /// relative to the entry-point network root, unlike most sub-networks.
    ///
    /// # JavaScript Example
    /// ```javascript,ignore
    /// const contentNets = BeliefBaseWasm.content_namespaces();
    /// // contentNets is an Array of { bid, bref } objects
    /// const isContent = contentNets.some(ns => ns.bid === node.home_net);
    /// ```
    #[wasm_bindgen]
    pub fn content_namespaces() -> JsValue {
        let results: Vec<BidBrefResult> = content_namespaces()
            .iter()
            .map(|bid| BidBrefResult::from_bid(*bid))
            .collect();
        serde_wasm_bindgen::to_value(&results).unwrap_or(JsValue::NULL)
    }

    /// Get asset namespace BID (images/PDFs/attachments tracking network)
    ///
    /// See `docs/design/architecture.md` § 10 for network namespace details.
    ///
    /// # JavaScript Example
    /// ```javascript,ignore
    /// const asset_bid = BeliefBaseWasm.asset_namespace();
    /// ```
    #[wasm_bindgen]
    pub fn asset_namespace() -> JsValue {
        BidBrefResult::from_bid(asset_namespace()).to_js()
    }

    /// Get buildonomy namespace BID (API node for version management)
    ///
    /// See `docs/design/architecture.md` § 10 for network namespace details.
    ///
    /// # JavaScript Example
    /// ```javascript,ignore
    /// const api_bid = BeliefBaseWasm.buildonomy_namespace();
    /// ```
    #[wasm_bindgen]
    pub fn buildonomy_namespace() -> JsValue {
        BidBrefResult::from_bid(buildonomy_namespace()).to_js()
    }

    /// Get all network path maps for navigation tree generation
    ///
    /// Returns a plain JavaScript object (NOT a Map):
    /// - Top level: network BID → PathMap data
    /// - PathMap data: array of [path, bid, order_indices] tuples
    ///
    /// This provides the complete document hierarchy for building navigation trees.
    /// The order_indices array contains sort keys from WEIGHT_SORT_KEY (Subsection relations).
    ///
    /// See `docs/design/interactive_viewer.md` § 8 (Navigation Tree Generation) for usage.
    ///
    /// ✅ **JavaScript**: This returns a plain object (uses serde_json serialization)
    ///
    /// # JavaScript Example
    /// ```javascript,ignore
    /// const paths = beliefbase.get_paths();
    /// // ✅ Plain object - use bracket notation
    /// const networkPaths = paths[networkBid];  // ✅ Works!
    /// Object.keys(paths);                       // ✅ Works!
    /// // paths = {
    /// //   "network_bid_1": [
    /// //     ["path/to/doc.md", "doc_bid", [0]],
    /// //     ["path/to/doc.md#section", "section_bid", [0, 1]],
    /// //     ...
    /// //   ],
    /// //   "network_bid_2": [...],
    /// //   ...
    /// // }
    /// ```
    #[wasm_bindgen]
    pub fn get_paths(&self) -> JsValue {
        let inner = self.inner.borrow();
        let paths = inner.paths();

        // Build plain JavaScript object using js_sys::Object
        // Structure: { "network-bid": { "path": "bid", ... }, ... }
        // IMPORTANT: Don't use serde_wasm_bindgen - it creates Maps, not plain objects
        let result = Object::new();

        for net_bid in paths.nets().iter() {
            // Get PathMap using Bref lookup
            if let Some(pm_lock) = paths.map().get(&net_bid.bref()) {
                let pm = pm_lock.read();

                // Build nested object mapping path → bid
                let path_obj = Object::new();
                for (path, bid, _order) in pm.map().iter() {
                    let path_key = JsValue::from_str(path);
                    let bid_value = JsValue::from_str(&bid.to_string());
                    let _ = Reflect::set(&path_obj, &path_key, &bid_value);
                }

                // Use full BID (36 chars) as key, not Bref (12 chars)
                let net_key = JsValue::from_str(&net_bid.to_string());
                let _ = Reflect::set(&result, &net_key, &path_obj);
            }
        }

        result.into()
    }

    /// Get pre-structured navigation tree (hierarchical, ready to render)
    ///
    /// Returns a hierarchical navigation tree with networks, documents, and sections.
    /// Uses a stack-based algorithm to build the tree structure based on order_indices depth.
    /// This is more efficient than `get_paths()` because the tree is built in Rust
    /// with proper title extraction from BeliefNode states.
    ///
    /// See `docs/design/interactive_viewer.md` § 8 (Navigation Tree Generation) for usage.
    ///
    /// ⚠️ **JavaScript**: `tree.nodes` is a Map object (not plain object)!
    ///
    /// # JavaScript Example
    /// ```javascript,ignore
    /// const tree = beliefbase.get_nav_tree();
    ///
    /// // ⚠️ IMPORTANT: tree.nodes is a Map, tree.roots is an Array
    /// const node = tree.nodes.get(someBid);     // ✅ Correct
    /// const count = tree.nodes.size;             // ✅ Correct
    /// const firstRoot = tree.roots[0];           // ✅ Array access
    ///
    /// // ❌ WRONG: tree.nodes[bid] returns undefined
    /// // ❌ WRONG: Object.keys(tree.nodes) returns []
    /// ```
    #[wasm_bindgen]
    pub fn get_nav_tree(&self) -> JsValue {
        let base = self.inner.borrow();
        let paths = base.paths();
        let states = base.states();
        let brefs = base.brefs();

        // ── Strategy ─────────────────────────────────────────────────────────
        //
        // Each network's PathMap (pm.map()) stores entries with *network-relative*
        // path strings.  Subnet directories appear as single opaque entries; their
        // internal nodes are not inlined.  However, Section relations with pre-joined
        // paths mean that a document owned by a deeply nested subnet (e.g.
        // "subnet1/subnet1a/doc.md") can appear in an ancestor's pm.map() too.
        //
        // Two derived facts we need per network:
        //
        //   1. Mount path  — the repo-root-relative prefix for this network.
        //      Root network → "".  subnet1 (entry "subnet1" in root) → "subnet1".
        //      subnet1a (entry "subnet1a" in subnet1) → "subnet1/subnet1a".
        //      Used to:
        //        (a) prefix local path strings so NavNode.path is repo-root-relative, and
        //        (b) identify which entries in pm.map() belong to THIS network vs. a
        //            deeper subnet (ownership = path does NOT start with any direct
        //            subnet's local entry path + "/").
        //
        //   2. Direct-subnet local paths — the path strings under which direct subnets
        //      appear in THIS network's pm.map() (e.g. "subnet1", "subnet2" for root).
        //      Any pm.map() entry whose path starts with "<subnet_local_path>/" is owned
        //      by that subnet and must be skipped here.
        //
        // Algorithm:
        //   Pass 0 — build net_mount_path: BTreeMap<Bid, String> by scanning every
        //            non-reserved PathMap for subnet entries and propagating prefixes.
        //   Pass 1 — for every non-reserved network, iterate pm.map(), skip entries
        //            owned by direct subnets (path-prefix test), skip gateway aliases
        //            (u16::MAX), build NavNodes with correctly prefixed paths.
        //   Pass 2 — wire subnet parent/child edges recorded during Pass 1.

        // Collect all non-reserved network BIDs.
        let all_net_bids: BTreeSet<Bid> = paths
            .map()
            .keys()
            .filter_map(|bref| {
                let bid = brefs.get(bref)?;
                if bid.is_reserved() {
                    None
                } else {
                    Some(*bid)
                }
            })
            .collect();

        // Pass 0 — build net_mount_path.
        //
        // Start with every non-reserved network mapped to "" (unknown / root candidate).
        // Then, for each network's pm.map(), find entries whose BID is itself a network
        // (subnet entries) and set that subnet's mount path to
        //   parent_mount_path + subnet_local_path
        // We iterate in BTreeMap order (by Bref), which processes shallower networks
        // first because shallower networks were registered earlier and have smaller Brefs
        // in practice.  A fixpoint loop would be more robust; two passes is sufficient
        // for any finite nesting depth because we process parent before child when the
        // parent's mount path is already known.
        let mut net_mount_path: BTreeMap<Bid, String> = all_net_bids
            .iter()
            .map(|bid| (*bid, String::new()))
            .collect();

        // Fixpoint: iterate until no mount path changes.  Each pass can propagate
        // mount paths one level deeper through the subnet hierarchy.  For a corpus
        // with N nesting levels, up to N passes may be needed when BTreeMap iteration
        // order processes a child network before its parent.  The loop is bounded by
        // the number of networks (each pass must make progress or terminate).
        let max_passes = all_net_bids.len().max(1);
        for _fixpoint_pass in 0..max_passes {
            let mut changed = false;
            for (net_bref, pm_lock) in paths.map().iter() {
                let net_bid = match brefs.get(net_bref) {
                    Some(bid) => *bid,
                    None => continue,
                };
                if net_bid.is_reserved() {
                    continue;
                }
                let parent_mount = match net_mount_path.get(&net_bid) {
                    Some(m) => m.clone(),
                    None => continue,
                };
                let pm = pm_lock.read();
                for (local_path, entry_bid, _order) in pm.map().iter() {
                    if !all_net_bids.contains(entry_bid) || *entry_bid == net_bid {
                        continue;
                    }
                    // This is a direct subnet entry.  Compute its mount path.
                    let subnet_mount = if parent_mount.is_empty() {
                        local_path.clone()
                    } else {
                        format!("{}/{}", parent_mount, local_path)
                    };
                    net_mount_path
                        .entry(*entry_bid)
                        .and_modify(|existing| {
                            // Keep the longer (more specific) mount path when there are
                            // multiple routes to the same subnet.
                            if subnet_mount.len() > existing.len() {
                                *existing = subnet_mount.clone();
                                changed = true;
                            }
                        })
                        .or_insert_with(|| {
                            changed = true;
                            subnet_mount
                        });
                }
            }
            if !changed {
                break;
            }
        }

        // Pass 1 — per-network node construction.
        let mut root_nodes_map: BTreeMap<String, NavNode> = BTreeMap::new();
        let mut root_net_bids: Vec<Bid> = Vec::new();
        // subnet_bid → parent_bid recorded while scanning parent networks.
        let mut subnet_parent_edges: BTreeMap<Bid, Bid> = BTreeMap::new();

        for (net_bref, pm_lock) in paths.map().iter() {
            let net_bid = match brefs.get(net_bref) {
                Some(bid) => *bid,
                None => continue,
            };
            if net_bid.is_reserved() {
                continue;
            }

            let pm = pm_lock.read();
            let mount = net_mount_path.get(&net_bid).cloned().unwrap_or_default();

            // Helper: convert a network-relative path string to a repo-root-relative
            // .html path.  The empty string (the network's own "" key) maps to
            // "<mount>/index.html" or just "index.html" for the root network.
            let make_html_path = |local: &str| -> String {
                let repo_relative = if mount.is_empty() {
                    local.to_string()
                } else if local.is_empty() {
                    mount.clone()
                } else {
                    format!("{}/{}", mount, local)
                };
                Self::normalize_path_extension(&repo_relative)
            };

            // Collect the local path strings of direct subnets so we can skip
            // pm.map() entries owned by them (path-prefix ownership rule).
            let direct_subnet_local_paths: Vec<String> = pm
                .map()
                .iter()
                .filter_map(|(local_path, bid, _order)| {
                    if all_net_bids.contains(bid) && *bid != net_bid {
                        Some(local_path.clone())
                    } else {
                        None
                    }
                })
                .collect();

            let net_root_path = make_html_path("");
            let net_bid_str = net_bid.to_string();
            let net_kind = states
                .get(&net_bid)
                .map(|node| node.kind.clone())
                .unwrap_or_default();
            let net_title = states
                .get(&net_bid)
                .map(|node| node.title.clone())
                .unwrap_or_else(|| net_bid_str.clone());

            // Insert the network root node.  parent is set in Pass 2 for subnet roots.
            root_nodes_map.insert(
                net_bid_str.clone(),
                NavNode {
                    bid: net_bid_str.clone(),
                    title: net_title,
                    path: net_root_path,
                    parent: None,
                    children: Vec::new(),
                    is_network: net_kind.is_network(),
                    is_document: net_kind.is_document(),
                },
            );

            // Stack of (bid, bid_str, depth) for parent-chain tracking.
            // Network root sits at depth 0.
            let mut stack: Vec<(Bid, String, usize)> = vec![(net_bid, net_bid_str.clone(), 0)];

            for (local_path, bid, order_indices) in pm.map().iter() {
                let bid_str = bid.to_string();

                // Skip the network's own root entry (already inserted above).
                if *bid == net_bid {
                    continue;
                }

                let is_subnet_bid = all_net_bids.contains(bid);

                // Ownership filter (non-subnet entries only):
                // skip any entry whose local path starts with a direct subnet's local
                // path + "/" — it belongs to that subnet and will be processed there.
                if !is_subnet_bid {
                    let owned_by_subnet = direct_subnet_local_paths
                        .iter()
                        .any(|subnet_local| local_path.starts_with(&format!("{}/", subnet_local)));
                    if owned_by_subnet {
                        continue;
                    }
                }

                // Gateway alias handling.
                //
                // PathMap stores two entries per network BID:
                //   ("",         net_bid, [])           — canonical doc-slot (skipped above)
                //   ("index.md", net_bid, [u16::MAX])   — gateway alias → skip
                //
                // Section headings from the network's own index.md are stored as:
                //   ("index.md#slug", sec_bid, [u16::MAX, N])
                // These are the network's own index sections and SHOULD appear in the tree
                // as children of the network node.  We keep them but collapse the u16::MAX
                // prefix level so their depth is 1 (direct children of the network).
                //
                // For subnet entries like ("subnet1/index.md#slug", sec_bid, [N, u16::MAX, M])
                // that propagated upward: the ownership filter above already skips them
                // (they start with "subnet1/").  No extra handling needed here.
                if order_indices.is_empty() {
                    continue;
                }
                let mut depth = order_indices.len();
                if order_indices[depth - 1] == u16::MAX {
                    // Pure gateway alias ("index.md" entry for the net BID) — skip.
                    continue;
                }
                if order_indices[0] == u16::MAX {
                    // index.md#slug section: strip the leading u16::MAX level.
                    // depth becomes order_indices.len() - 1, but we want these as
                    // depth-1 children of the network node, so clamp to 1.
                    depth = depth.saturating_sub(1).max(1);
                } else if order_indices.len() > 1
                    && order_indices[order_indices.len() - 2] == u16::MAX
                {
                    // Section reached through the gateway plane inside a subnet's subtree
                    // (e.g. "subnet/index.md#slug" with order [N, u16::MAX, M]).
                    // The ownership filter handles these for non-subnet entries; for subnet
                    // entries we collapse one level so sections sit under the subnet node.
                    depth -= 1;
                }

                let html_path = make_html_path(local_path);

                let (node_title, node_kind) = states
                    .get(bid)
                    .map(|node| (node.title.clone(), node.kind.clone()))
                    .unwrap_or_else(|| (local_path.clone(), Default::default()));

                // Pop stack to the correct parent depth.
                while stack.len() > 1 && stack.last().unwrap().2 >= depth {
                    stack.pop();
                }
                let (parent_bid, parent_bid_str) = {
                    let top = stack.last().unwrap();
                    (top.0, top.1.clone())
                };

                if is_subnet_bid {
                    // Record cross-network parent edge; wire children in Pass 2.
                    subnet_parent_edges.insert(*bid, parent_bid);
                    root_nodes_map
                        .entry(bid_str.clone())
                        .or_insert_with(|| NavNode {
                            bid: bid_str.clone(),
                            title: node_title,
                            path: html_path,
                            parent: None,
                            children: Vec::new(),
                            is_network: node_kind.is_network(),
                            is_document: node_kind.is_document(),
                        });
                    stack.push((*bid, bid_str, depth));
                    continue;
                }

                // Skip BIDs already inserted (multiple paths in a PathMap → take first).
                if root_nodes_map.contains_key(&bid_str) {
                    continue;
                }

                root_nodes_map.insert(
                    bid_str.clone(),
                    NavNode {
                        bid: bid_str.clone(),
                        title: node_title,
                        path: html_path,
                        parent: Some(parent_bid_str.clone()),
                        children: Vec::new(),
                        is_network: node_kind.is_network(),
                        is_document: node_kind.is_document(),
                    },
                );
                if let Some(parent_node) = root_nodes_map.get_mut(&parent_bid_str) {
                    parent_node.children.push(bid_str.clone());
                }
                stack.push((*bid, bid_str, depth));
            }
        }

        // Pass 2 — wire cross-network parent/child edges.
        for (subnet_bid, parent_bid) in &subnet_parent_edges {
            let subnet_bid_str = subnet_bid.to_string();
            let parent_bid_str = parent_bid.to_string();
            if let Some(subnet_node) = root_nodes_map.get_mut(&subnet_bid_str) {
                subnet_node.parent = Some(parent_bid_str.clone());
            }
            if let Some(parent_node) = root_nodes_map.get_mut(&parent_bid_str) {
                if !parent_node.children.contains(&subnet_bid_str) {
                    parent_node.children.push(subnet_bid_str);
                }
            }
        }

        // Determine root networks.
        //
        // When the entry point network is loaded, it is the sole root.  In sharded
        // mode, partially-loaded corpora can produce orphan network nodes (networks
        // whose parent network shard hasn't loaded yet).  These orphans have PathMap
        // entries — typically from cross-network edges in the global shard (e.g.
        // codec namespace registrations) — but no subnet_parent_edge claiming them
        // as children of a loaded parent.  Without this filter they appear as
        // spurious top-level roots in the nav tree (e.g. "algorithm", "math",
        // "fsw_solution" appearing as peers of "Haven Systems" on a deep-URL
        // refresh).
        //
        // The nav tree is a DAG rooted at the entry point; we only show paths
        // reachable from that root through loaded parent chains.  Orphan networks
        // will appear naturally once their parent network's shard loads and the nav
        // tree is rebuilt.
        //
        // If the entry point hasn't loaded yet (e.g. the target network shard loaded
        // first), fall back to showing all unclaimed networks as roots.  The entry
        // network loads in the background and triggers a nav tree rebuild via
        // noet:shard-loaded, at which point this function runs again with the entry
        // point present and the orphans get pruned.
        let entry_bid_str = self.entry_point_bid.to_string();
        let entry_is_loaded = root_nodes_map.contains_key(&entry_bid_str);

        if entry_is_loaded {
            root_net_bids.push(self.entry_point_bid);
        } else {
            // Fallback: show all unclaimed networks as roots (pre-fix behavior).
            for (net_bref, _) in paths.map().iter() {
                let net_bid = match brefs.get(net_bref) {
                    Some(bid) => *bid,
                    None => continue,
                };
                if net_bid.is_reserved() {
                    continue;
                }
                if !subnet_parent_edges.contains_key(&net_bid) {
                    root_net_bids.push(net_bid);
                }
            }
        }

        // When we have the entry point as root, prune orphan networks and their
        // exclusive descendants from the node map.  Walk from each root downward
        // to collect reachable BID strings, then remove everything else.  This
        // keeps the serialized tree lean and prevents the JS renderer from showing
        // disconnected subtrees.
        if entry_is_loaded {
            let reachable: BTreeSet<String> = {
                let mut visited = BTreeSet::new();
                let mut queue: Vec<String> =
                    root_net_bids.iter().map(|bid| bid.to_string()).collect();
                while let Some(bid_str) = queue.pop() {
                    if !visited.insert(bid_str.clone()) {
                        continue;
                    }
                    if let Some(node) = root_nodes_map.get(&bid_str) {
                        queue.extend(node.children.iter().cloned());
                    }
                }
                visited
            };
            root_nodes_map.retain(|bid_str, _| reachable.contains(bid_str));
        }

        let root_nodes: Vec<String> = root_net_bids.iter().map(|bid| bid.to_string()).collect();

        let tree = NavTree {
            nodes: root_nodes_map,
            roots: root_nodes,
        };

        serde_wasm_bindgen::to_value(&tree).unwrap_or_else(|e| {
            console::error_1(&format!("Failed to serialize nav tree: {}", e).into());
            JsValue::NULL
        })
    }

    /// Render a markdown string to HTML using the same parser options as the compiler.
    ///
    /// Uses the same extension set as `buildonomy_md_options()`: GFM, tables, footnotes,
    /// math, strikethrough, subscript, superscript, task lists, wiki links, heading
    /// attributes, definition lists, and YAML-style metadata blocks.
    /// Render a markdown string to HTML using the canonical Buildonomy parser options.
    ///
    /// Delegates to `crate::codec::render_markdown_snippet` — the shared utility
    /// that uses canonical parser options and a broken-link callback.
    ///
    /// # Arguments
    /// * `text` - Raw markdown source string
    ///
    /// # Returns
    /// HTML string ready for innerHTML injection. Returns empty string on parse/render error.
    #[wasm_bindgen]
    pub fn render_markdown(text: &str) -> String {
        crate::codec::render_markdown_snippet(text)
    }

    /// Load the codec extension manifest from `codecs.json`.
    ///
    /// Replaces the default `BUILTIN_EXTENSIONS` set with the full list of
    /// document extensions that were registered at build time (including
    /// extensions from application shims like `.yaml`, `.h`). This must be
    /// called before any `normalize_path_extension` or link-resolution calls
    /// to ensure custom extensions are correctly rewritten to `.html`.
    ///
    /// Treat a missing or unparseable manifest as fatal rather than falling
    /// back to `BUILTIN_EXTENSIONS`: with only the built-ins, links to
    /// shim-extension documents normalise to directory URLs that 404, which
    /// fails silently and per-link instead of once at startup. `codecs.json`
    /// is written on every export, so its absence means a broken deployment.
    ///
    /// # Arguments
    /// * `json` — The raw JSON string from `codecs.json`
    ///
    /// # Example
    /// ```javascript,ignore
    /// const resp = await fetch('codecs.json');
    /// if (!resp.ok) {
    ///     throw new Error(`codec manifest missing (${resp.status})`);
    /// }
    /// BeliefBaseWasm.set_known_extensions(await resp.text());
    /// ```
    #[wasm_bindgen(js_name = "setKnownExtensions")]
    pub fn set_known_extensions(json: &str) -> Result<(), JsValue> {
        #[derive(Deserialize)]
        struct CodecManifest {
            document_extensions: Vec<String>,
        }
        let manifest: CodecManifest = serde_json::from_str(json)
            .map_err(|e| JsValue::from_str(&format!("Failed to parse codecs.json: {}", e)))?;
        crate::codec::CODECS.set_known_extensions(manifest.document_extensions);
        tracing::info!(
            "[WASM] Loaded codec manifest: {} extensions",
            crate::codec::CODECS.extensions().len(),
        );
        Ok(())
    }

    /// Normalize path extension to .html for fetching rendered documents
    ///
    /// Converts source file extensions (.md, .org, etc.) to .html for the viewer to fetch.
    /// Also handles directory paths by appending /index.html.
    ///
    /// # Arguments
    /// * `path` - Path with source extension (e.g., "docs/guide.md#section")
    ///
    /// # Returns
    /// Path with .html extension (e.g., "docs/guide.html#section")
    ///
    /// # Examples
    /// ```javascript,ignore
    /// const htmlPath = BeliefBaseWasm.normalizePathExtension("net1_dir1/hsml.md#definition");
    /// // Returns: "net1_dir1/hsml.html#definition"
    /// ```
    #[wasm_bindgen]
    pub fn normalize_path_extension(path: &str) -> String {
        normalize_path_extension_impl(path)
    }

    /// Convert a string to an anchor/id slug using the same algorithm as Rust's `to_anchor()`:
    /// NFKC lowercase, whitespace → "-", strip non-`[a-z0-9-._()[\]@]`, collapse consecutive "-".
    ///
    /// Exposed so that JS clients (e.g. xlsx-tabs.js relation formatter) can derive
    /// canonical NodeKey id strings from raw cell text without duplicating the logic.
    ///
    /// # Example
    /// ```javascript
    /// BeliefBaseWasm.toAnchor("Load Switch Controller") // → "load-switch-controller"
    /// BeliefBaseWasm.toAnchor("Source/Rationale")       // → "sourcerationale"
    /// ```
    #[wasm_bindgen(js_name = toAnchor)]
    pub fn to_anchor(s: &str) -> String {
        crate::paths::path::to_anchor(s)
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// TextSearchProvider implementation for WASM
// ═══════════════════════════════════════════════════════════════════════════════

/// Bridges the loaded search indices to the `TextSearchProvider` trait
/// so that `BeliefBase::apply_filter` can evaluate `TextMatch` filter steps.
///
/// Uses a default limit of 1000 results to prevent pathological performance
/// on very common query terms. TF-IDF scoring naturally filters to documents
/// containing at least one query term, so the limit only applies when the
/// corpus has many partial matches.
#[cfg(feature = "wasm")]
struct WasmTextSearchProvider<'a> {
    indices: &'a HashMap<String, SearchIndex>,
}

/// Maximum results returned by the WASM text search provider.
/// Prevents pathological memory/CPU usage on common single-term queries.
#[cfg(feature = "wasm")]
const TEXT_SEARCH_LIMIT: usize = 1000;

#[cfg(feature = "wasm")]
impl TextSearchProvider for WasmTextSearchProvider<'_> {
    fn text_search(&self, query: &str) -> Vec<(Bid, f64)> {
        let idx_refs: Vec<&SearchIndex> = self.indices.values().collect();
        if idx_refs.is_empty() {
            return Vec::new();
        }
        // Request one extra result so we can detect truncation.
        let results = query_search_index(&idx_refs, query, TEXT_SEARCH_LIMIT + 1);
        if results.len() > TEXT_SEARCH_LIMIT {
            console::warn_1(
                &format!(
                    "⚠️ TextMatch query '{}' matched {} results, truncated to {}. \
                     Results may be incomplete.",
                    query,
                    results.len(),
                    TEXT_SEARCH_LIMIT,
                )
                .into(),
            );
        }
        results
            .into_iter()
            .take(TEXT_SEARCH_LIMIT)
            .filter_map(|r| Bid::try_from(r.bid.as_str()).ok().map(|bid| (bid, r.score)))
            .collect()
    }
}

// Module is only compiled when wasm feature is enabled
#[cfg(not(feature = "wasm"))]
compile_error!("wasm module should only be compiled with wasm feature enabled");

// Canonical tests for normalize_path_extension_impl live in codec/mod.rs
// where the function is defined, and run with `cargo test --features wasm`.
