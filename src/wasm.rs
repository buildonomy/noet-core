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
    let default_level = tracing_subscriber::filter::LevelFilter::DEBUG;
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
    beliefbase::{BeliefBase, BeliefGraph},
    codec::normalize_path_extension_impl,
    nodekey::NodeKey,
    paths::AnchorPath,
    properties::{
        asset_namespace, buildonomy_namespace, content_namespaces, href_namespace, BeliefKind,
        BeliefNode, Bid, Bref, WeightKind, WEIGHT_SORT_KEY,
    },
    query::{Expression, StatePred},
};

#[cfg(feature = "wasm")]
use serde::{Deserialize, Serialize};

#[cfg(feature = "wasm")]
use serde_json;

#[cfg(feature = "wasm")]
use enumset::EnumSet;

#[cfg(feature = "wasm")]
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet, HashMap},
    str::FromStr,
};

#[cfg(feature = "wasm")]
use js_sys::{Object, Reflect};

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
    /// Sources: BIDs of nodes linking TO this one
    /// Sinks: BIDs of nodes this one links TO
    /// Both vectors are sorted by WEIGHT_SORT_KEY edge payload value
    /// ⚠️ JavaScript: This is a Map object! Use `.get(weightKind)`, `.size`, `.entries()`
    pub graph: HashMap<WeightKind, (Vec<Bid>, Vec<Bid>)>,
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

        console::log_1(
            &format!(
                "✅ Loaded BeliefGraph: {} nodes, {} relations",
                node_count, relation_count
            )
            .into(),
        );

        // Parse entry point BID string directly
        let entry_point_bid = Bid::try_from(entry_bid_str.as_str()).map_err(|e| {
            let msg = format!("❌ Failed to parse entry point BID: {}", e);
            console::error_1(&msg.clone().into());
            JsValue::from_str(&msg)
        })?;

        console::log_1(&format!("✅ Entry point Bid: {}", entry_point_bid).into());

        // Convert BeliefGraph to BeliefBase
        let inner = BeliefBase::from(graph);

        Ok(BeliefBaseWasm {
            inner: RefCell::new(inner),
            entry_point_bid,
            loaded_shards: RefCell::new(HashMap::new()),
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
    /// const globalResp = await fetch("/beliefbase/global.json");
    /// await bb.load_shard("global", await globalResp.text());
    /// const entryResp = await fetch(`/beliefbase/networks/${entryBref}.json`);
    /// await bb.load_shard(entryBref, await entryResp.text());
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

        console::log_1(
            &format!(
                "✅ Shard manifest parsed. Entry point: {}. Call load_shard() to populate.",
                entry_point_bid
            )
            .into(),
        );

        Ok(BeliefBaseWasm {
            inner: RefCell::new(BeliefBase::default()),
            entry_point_bid,
            loaded_shards: RefCell::new(HashMap::new()),
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
    /// const count = await bb.load_shard("global", globalJson);
    /// console.log(`BeliefBase now has ${count} nodes`);
    /// ```
    #[wasm_bindgen]
    pub fn load_shard(&self, bref_key: String, shard_json: String) -> Result<usize, JsValue> {
        // If already loaded, unload first (idempotent reload).
        {
            let loaded = self.loaded_shards.borrow();
            if loaded.contains_key(&bref_key) {
                drop(loaded);
                self.unload_shard(bref_key.clone())?;
            }
        }

        // Deserialize: global shard and network shards have different schemas.
        #[allow(clippy::type_complexity)]
        let (states, edges): (
            BTreeMap<String, BeliefNode>,
            Vec<(Bid, Bid, crate::properties::WeightSet)>,
        ) = if bref_key == "global" {
            let shard: crate::shard::GlobalShard =
                serde_json::from_str(&shard_json).map_err(|e| {
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
            (shard.states, edges)
        } else {
            let shard: crate::shard::NetworkShard =
                serde_json::from_str(&shard_json).map_err(|e| {
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
            use crate::beliefbase::BidGraph;
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
        console::log_1(
            &format!(
                "✅ Loaded shard '{}': +{} nodes, {} edges → {} total nodes",
                bref_key, added_count, edge_count, total
            )
            .into(),
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
        }

        let total = self.inner.borrow().states().len();
        console::log_1(
            &format!(
                "✅ Unloaded shard '{}': -{} nodes → {} total nodes",
                bref_key, remove_count, total
            )
            .into(),
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

    /// Query nodes using Expression syntax
    ///
    /// This exposes the full query API to JavaScript.
    /// Returns a BeliefGraph with matching nodes and their relations.
    ///
    /// # JavaScript Example
    /// ```javascript,ignore
    /// // Query by BID
    /// const expr = { StateIn: { Bid: ["01234567-89ab-cdef-0123-456789abcdef"] } };
    /// const graph = await bb.query(expr);
    ///
    /// // Query by title regex
    /// const expr = { StateIn: { Title: "documentation.*" } };
    /// const graph = await bb.query(expr);
    ///
    /// // Query documents only
    /// const expr = { StateIn: { Kind: "Document" } };
    /// const graph = await bb.query(expr);
    /// ```
    #[wasm_bindgen]
    pub async fn query(&self, expr_js: JsValue) -> Result<JsValue, JsValue> {
        // Deserialize Expression from JavaScript
        let expr: Expression = serde_wasm_bindgen::from_value(expr_js).map_err(|e| {
            let msg = format!("❌ Failed to parse Expression: {}", e);
            console::error_1(&msg.clone().into());
            JsValue::from_str(&msg)
        })?;

        console::log_1(&format!("🔍 Query: {:?}", expr).into());

        // Evaluate expression directly (BeliefSource trait not available in WASM)
        let inner = self.inner.borrow();
        let graph = inner.evaluate_expression(&expr);

        let result_count = graph.states.len();
        console::log_1(&format!("✅ Query returned {} nodes", result_count).into());

        // Serialize result back to JavaScript
        serde_wasm_bindgen::to_value(&graph).map_err(|e| {
            let msg = format!("❌ Failed to serialize result: {}", e);
            console::error_1(&msg.clone().into());
            JsValue::from_str(&msg)
        })
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
                console::log_1(&format!("✅ Found node: {}", node.title).into());
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

    /// Search for nodes by title substring
    ///
    /// Returns array of matching nodes. Uses case-insensitive substring matching.
    /// For more advanced queries, use `query()` with Expression syntax.
    ///
    /// # JavaScript Example
    /// ```javascript,ignore
    /// const results = bb.search("documentation");
    /// results.forEach(node => console.log(node.title));
    /// ```
    #[wasm_bindgen]
    pub fn search(&self, query: String) -> JsValue {
        console::log_1(&format!("🔍 Search query: '{}'", query).into());

        let query_lower = query.to_lowercase();
        let inner = self.inner.borrow();

        let results: Vec<&BeliefNode> = inner
            .states()
            .values()
            .filter(|node| {
                // Search in title
                node.title.to_lowercase().contains(&query_lower)
                    // Search in node ID if present
                    || node.payload
                        .get("id")
                        .and_then(|v| v.as_str())
                        .map(|id| id.to_lowercase().contains(&query_lower))
                        .unwrap_or(false)
            })
            .collect();

        console::log_1(&format!("✅ Found {} matching nodes", results.len()).into());

        serde_wasm_bindgen::to_value(&results).unwrap_or(JsValue::NULL)
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
                console::log_1(&format!("✅ Resolved bref to BID: {}", bid).into());
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
        console::log_1(&format!("✅ Converted BID to bref: {}", bref).into());
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
                console::log_1(
                    &format!(
                        "✅ Resolved id '{}' to BID: {} (bref: {}, net: {})",
                        id, node_bid, node_bref, net_bid
                    )
                    .into(),
                );

                BidBrefResult::from_bid(node_bid).to_js()
            }
            None => {
                console::warn_1(
                    &format!("⚠️ No node found with id '{}' in network {}", id, bref).into(),
                );
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
        let mut kind_set = EnumSet::new();
        kind_set.insert(BeliefKind::Network);
        let expr = Expression::StateIn(StatePred::Kind(kind_set));
        let inner = self.inner.borrow();

        let graph = inner.evaluate_expression(&expr);

        let networks: Vec<&BeliefNode> = graph.states.values().collect();
        console::log_1(&format!("✅ Found {} networks", networks.len()).into());

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
        let mut kind_set = EnumSet::new();
        kind_set.insert(BeliefKind::Document);
        let expr = Expression::StateIn(StatePred::Kind(kind_set));
        let inner = self.inner.borrow();

        let graph = inner.evaluate_expression(&expr);

        let documents: Vec<&BeliefNode> = graph.states.values().collect();
        console::log_1(&format!("✅ Found {} documents", documents.len()).into());

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
        fn toml_table_to_json(table: &toml::value::Table) -> serde_json::Value {
            let mut map = serde_json::Map::new();
            for (k, v) in table {
                map.insert(k.clone(), toml_value_to_json(v));
            }
            serde_json::Value::Object(map)
        }

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
                toml::Value::Table(t) => toml_table_to_json(t),
                toml::Value::Datetime(dt) => serde_json::Value::String(dt.to_string()),
            }
        }

        let mut inner = self.inner.borrow_mut();

        inner.get_context(ns, bid).map(|ctx| {
            // Collect all related nodes (other end of all edges)
            let mut related_nodes = BTreeMap::new();
            type GraphMap = HashMap<WeightKind, (Vec<(Bid, u16)>, Vec<(Bid, u16)>)>;
            let mut graph: GraphMap = HashMap::new();

            // Process sources (nodes linking TO this one)
            for ext_rel in ctx.sources() {
                // Collect all related nodes with their path information
                let related_node = RelatedNode {
                    node: ext_rel.other.clone(),
                    home_net: ext_rel.home_net,
                    // Asset-namespace paths are opaque repo-relative identifiers (e.g.
                    // "net1_dir1", "assets/img.png") — not navigable HTML paths.
                    // normalize_path_extension_impl would incorrectly convert "net1_dir1"
                    // to "net1_dir1/index.html", treating it as a network directory.
                    root_path: if ext_rel.home_net == asset_namespace() {
                        ext_rel.root_path.clone()
                    } else {
                        normalize_path_extension_impl(&ext_rel.root_path)
                    },
                    link_title: ext_rel.link_title.clone(),
                };
                related_nodes.insert(ext_rel.other.bid, related_node);

                // Group by weight kind and collect with sort_key
                for (kind, weight) in ext_rel.weight.weights.iter() {
                    let sort_key: u16 = weight.get::<u16>(WEIGHT_SORT_KEY).unwrap_or(0);
                    graph
                        .entry(*kind)
                        .or_insert_with(|| (Vec::new(), Vec::new()))
                        .0
                        .push((ext_rel.other.bid, sort_key));
                }
            }

            // Process sinks (nodes this one links TO)
            for ext_rel in ctx.sinks() {
                // Collect all related nodes with their path information
                let related_node = RelatedNode {
                    node: ext_rel.other.clone(),
                    home_net: ext_rel.home_net,
                    root_path: if ext_rel.home_net == asset_namespace() {
                        ext_rel.root_path.clone()
                    } else {
                        normalize_path_extension_impl(&ext_rel.root_path)
                    },
                    link_title: ext_rel.link_title.clone(),
                };
                related_nodes.insert(ext_rel.other.bid, related_node);

                // Group by weight kind and collect with sort_key
                for (kind, weight) in ext_rel.weight.weights.iter() {
                    let sort_key: u16 = weight.get::<u16>(WEIGHT_SORT_KEY).unwrap_or(0);
                    graph
                        .entry(*kind)
                        .or_insert_with(|| (Vec::new(), Vec::new()))
                        .1
                        .push((ext_rel.other.bid, sort_key));
                }
            }

            // Sort all vectors by sort_key and extract just the BIDs
            let sorted_graph: HashMap<WeightKind, (Vec<Bid>, Vec<Bid>)> = graph
                .into_iter()
                .map(|(kind, (mut sources, mut sinks))| {
                    sources.sort_by_key(|(_, sort_key)| *sort_key);
                    sinks.sort_by_key(|(_, sort_key)| *sort_key);
                    (
                        kind,
                        (
                            sources.into_iter().map(|(bid, _)| bid).collect(),
                            sinks.into_iter().map(|(bid, _)| bid).collect(),
                        ),
                    )
                })
                .collect();

            NodeContext {
                node: ctx.node.clone(),
                root_path: if *bid == asset_namespace()
                    || bid.parent_bref() == asset_namespace().bref()
                {
                    ctx.root_path.clone()
                } else {
                    normalize_path_extension_impl(&ctx.root_path)
                },
                home_net: ctx.home_net,
                metadata: toml_table_to_json(&ctx.node.metadata),
                related_nodes,
                graph: sorted_graph,
            }
        })
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
            console::log_1(&format!("   Entry point: {}", self.entry_point_bid).into());
            console::log_1(
                &"   Tried namespaces: href, asset, buildonomy"
                    .to_string()
                    .into(),
            );
            return JsValue::NULL;
        };

        console::log_1(&format!("✅ Got context for node: {}", node_context.node.title).into());

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

        if js_val.is_object() {
            // Patch metadata: toml::value::Table → plain JS object.
            if let Ok(metadata_json) = serde_json::to_string(&node_context.metadata) {
                if let Ok(metadata_js) = js_sys::JSON::parse(&metadata_json) {
                    let _ = Reflect::set(&js_val, &JsValue::from_str("metadata"), &metadata_js);
                }
            }

            // Patch node.payload: toml::value::Table → plain JS object.
            // Without this, payload?.listing returns undefined (it's a Map entry, not a
            // plain property) and the directory listing panel never renders.
            if let Ok(payload_json) = serde_json::to_string(&node_context.node.payload) {
                if let Ok(payload_js) = js_sys::JSON::parse(&payload_json) {
                    // js_val.node is itself a plain JS object (BeliefNode struct fields
                    // serialize correctly except for the toml::value::Table payload field).
                    if let Ok(node_js) = Reflect::get(&js_val, &JsValue::from_str("node")) {
                        if node_js.is_object() {
                            let _ =
                                Reflect::set(&node_js, &JsValue::from_str("payload"), &payload_js);
                        }
                    }
                }
            }
        }

        js_val
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

        // Two-pass fixpoint: iterate until no mount path changes.  In practice one pass
        // suffices for the vast majority of corpora; two passes handle edge cases where
        // BTreeMap iteration order puts a child before its parent.
        for _fixpoint_pass in 0..2 {
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
                            }
                        })
                        .or_insert(subnet_mount);
                }
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

        // Determine root networks (those not claimed as subnets by any parent).
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
}

// Module is only compiled when wasm feature is enabled
#[cfg(not(feature = "wasm"))]
compile_error!("wasm module should only be compiled with wasm feature enabled");

// Canonical tests for normalize_path_extension_impl live in codec/mod.rs
// where the function is defined, and run with `cargo test --features wasm`.
