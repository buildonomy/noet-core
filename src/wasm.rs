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
//! **Problem**: Rust `BTreeMap` and `HashMap` serialize to JavaScript `Map` objects (not plain objects)
//! when using `serde_wasm_bindgen::to_value()`. This breaks JavaScript code expecting plain objects.
//!
//! ## Symptoms
//! ```javascript,ignore
//! // JavaScript receives a Map, not a plain object:
//! const data = wasm_function();
//! Object.keys(data);        // Returns [] (empty array!)
//! data[key];                // Returns undefined (bracket notation fails!)
//! Object.entries(data);     // Returns [] (empty array!)
//! ```
//!
//! ## Solutions
//!
//! ### Option A: Return Plain JavaScript Object (Recommended for dictionary-like data)
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
//!     serde_wasm_bindgen::to_value(&obj).unwrap()  // ✅ Plain object
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
//! ## Checklist for New Functions
//! - [ ] Does this function return BTreeMap/HashMap?
//! - [ ] Does JavaScript need plain object access (obj[key], Object.keys)?
//!   - YES → Use Option A (serde_json::Map)
//!   - NO → Use Option B and document as Map
//! - [ ] Add JSDoc comment showing JavaScript type
//! - [ ] Verify viewer.js uses correct access pattern
//!
//! ## Current Functions Status
//! - ✅ `get_paths()` - Returns plain object (uses serde_json)
//! - ⚠️ `get_context()` - Returns NodeContext with Map fields (related_nodes, graph)
//! - ⚠️ `get_nav_tree()` - Returns NavTree with Map field (nodes)
//! - ⚠️ `query()` - Returns BeliefGraph with Map field (states)
//!
//! See viewer.js header for JavaScript-side usage patterns.

#[cfg(feature = "wasm")]
use wasm_bindgen::prelude::*;

#[cfg(feature = "wasm")]
use web_sys::console;

#[cfg(feature = "wasm")]
use crate::{
    beliefbase::{BeliefBase, BeliefGraph},
    nodekey::NodeKey,
    properties::{
        asset_namespace, buildonomy_namespace, href_namespace, BeliefKind, BeliefNode, Bid, Bref,
        WeightKind, WEIGHT_SORT_KEY,
    },
    query::{Expression, StatePred},
};

/// Result containing both BID and bref for a node
#[cfg(feature = "wasm")]
#[wasm_bindgen]
pub struct BidBrefResult {
    bid: Bid,
}

#[cfg(feature = "wasm")]
#[wasm_bindgen]
impl BidBrefResult {
    /// Create a new BidBrefResult from a BID string
    ///
    /// # JavaScript Example
    /// ```javascript,ignore
    /// // Static method, not constructor
    /// const result = BidBrefResult.from_bid_string("1f10cfd9-1cc3-6a93-86f9-0e90d9cb2fdb");
    /// if (result) {
    ///     console.log(result.bid, result.bref);
    /// }
    /// ```
    pub fn from_bid_string(bid_str: String) -> Option<BidBrefResult> {
        match Bid::try_from(bid_str.as_str()) {
            Ok(bid) => Some(BidBrefResult { bid }),
            Err(_) => {
                console::warn_1(&format!("⚠️ Invalid BID format: {}", bid_str).into());
                None
            }
        }
    }

    #[wasm_bindgen(getter)]
    pub fn bid(&self) -> String {
        self.bid.to_string()
    }

    #[wasm_bindgen(getter)]
    pub fn bref(&self) -> String {
        self.bid.bref().to_string()
    }
}

#[cfg(feature = "wasm")]
use serde::{Deserialize, Serialize};

#[cfg(feature = "wasm")]
use serde_json;

#[cfg(feature = "wasm")]
use enumset::EnumSet;

#[cfg(feature = "wasm")]
use std::collections::{BTreeMap, BTreeSet, HashMap};

#[cfg(feature = "wasm")]
use js_sys::{Object, Reflect};

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
    path: String,
    filename: String,
    anchor: String,
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
}

/// WASM wrapper around BeliefBase for browser use
///
/// Provides JavaScript-accessible methods for querying beliefs loaded from JSON.
#[cfg(feature = "wasm")]
#[wasm_bindgen]
pub struct BeliefBaseWasm {
    inner: std::cell::RefCell<BeliefBase>,
    entry_point_bid: Bid,
}

#[cfg(feature = "wasm")]
#[wasm_bindgen]
impl BeliefBaseWasm {
    /// Get the entry point as a BidBrefResult
    ///
    /// # JavaScript Example
    /// ```javascript,ignore
    /// const entryPoint = beliefbase.entry_point();
    /// console.log(entryPoint.bid, entryPoint.bref);
    /// ```
    #[wasm_bindgen(js_name = entryPoint)]
    pub fn entry_point(&self) -> BidBrefResult {
        BidBrefResult {
            bid: self.entry_point_bid,
        }
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
        crate::paths::AnchorPath::new(path).normalize().to_string()
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
        let anchor_path = crate::paths::AnchorPath::new(path);
        PathParts {
            path: anchor_path.dir().to_string(),
            filename: anchor_path.filename().to_string(),
            anchor: anchor_path.anchor().to_string(),
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
        let base_path = crate::paths::AnchorPath::new(base);
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
        crate::paths::AnchorPath::new(path).ext().to_string()
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
        crate::paths::AnchorPath::new(path).parent().to_string()
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
        crate::paths::AnchorPath::new(path).filestem().to_string()
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
            inner: std::cell::RefCell::new(inner),
            entry_point_bid,
        })
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
                serde_wasm_bindgen::to_value(&node).unwrap_or(JsValue::NULL)
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
    pub fn get_bid_from_id(&self, net_bref: String, id: String) -> Option<BidBrefResult> {
        let bref = match Bref::try_from(net_bref.as_str()) {
            Ok(b) => b,
            Err(_) => {
                console::warn_1(&format!("⚠️ Invalid bref format: {}", net_bref).into());
                return None;
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

                // Return struct with bid (bref computed on demand)
                Some(BidBrefResult { bid: node_bid })
            }
            None => {
                console::warn_1(
                    &format!("⚠️ No node found with id '{}' in network {}", id, bref).into(),
                );
                None
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
                    root_path: ext_rel.root_path.clone(),
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
                    root_path: ext_rel.root_path.clone(),
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
                root_path: ctx.root_path.clone(),
                home_net: ctx.home_net,
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

        serde_wasm_bindgen::to_value(&node_context).unwrap_or(JsValue::NULL)
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
    pub fn href_namespace() -> String {
        href_namespace().to_string()
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
    pub fn asset_namespace() -> String {
        asset_namespace().to_string()
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
    pub fn buildonomy_namespace() -> String {
        buildonomy_namespace().to_string()
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

        // Build navigation tree from PathMapMap using stack-based algorithm
        let mut root_nodes_map: BTreeMap<String, NavNode> = BTreeMap::new();
        let mut root_nodes: Vec<String> = Vec::new();

        // First pass: Build map of subnet BIDs to their parent path prefixes
        // This is needed because subnet documents have paths relative to the subnet,
        // but NavTree needs full paths including the subnet directory prefix
        let mut subnet_prefixes: BTreeMap<Bid, String> = BTreeMap::new();
        let mut visited = BTreeSet::default();

        for (net_bref, pm_lock) in paths.map().iter() {
            let net_bid_for_prefix = match brefs.get(net_bref) {
                Some(bid) => bid,
                None => continue,
            };

            if net_bid_for_prefix.is_reserved() {
                continue;
            }

            let pm = pm_lock.read();

            // Find subnet entries in this PathMap and record their prefixes
            for (path, bid, _order_indices) in pm.map().iter() {
                // Check if this bid is a network (subnet)
                if paths.nets().contains(bid) && *bid != *net_bid_for_prefix {
                    subnet_prefixes.insert(*bid, path.clone());
                }
            }
        }

        for (net_bref, pm_lock) in paths.map().iter() {
            // Resolve Bref to Bid
            let net_bid = match brefs.get(net_bref) {
                Some(bid) => bid,
                None => continue, // Skip if we can't resolve the Bref
            };

            // Skip reserved BIDs (system namespaces and API nodes)
            if net_bid.is_reserved() || subnet_prefixes.contains_key(net_bid) {
                continue;
            }

            let pm = pm_lock.read();

            // Get network title from BeliefNode
            let net_title = states
                .get(net_bid)
                .map(|node| node.title.clone())
                .unwrap_or_else(|| net_bid.to_string());

            // Flat map for all nodes in this network
            let mut nodes_map: BTreeMap<String, NavNode> = BTreeMap::new();

            // Create network node
            let network_bid_str = net_bid.to_string();
            nodes_map.insert(
                network_bid_str.clone(),
                NavNode {
                    bid: network_bid_str.clone(),
                    title: net_title,
                    path: String::new(), // Networks don't have paths
                    parent: None,
                    children: Vec::new(),
                },
            );

            // Stack of (bid, depth) for tracking parent hierarchy
            let mut stack: Vec<(String, usize)> = Vec::new();
            stack.push((network_bid_str.clone(), 0)); // Network is at depth 0

            for (path, bid, order_indices) in pm.recursive_map(&paths, &mut visited).iter() {
                let depth = order_indices.len();
                let bid_str = bid.to_string();

                // Skip the network node itself (prevents self-reference)
                if bid_str == network_bid_str {
                    continue;
                }

                // Get node title from BeliefNode
                let node_title = states
                    .get(bid)
                    .map(|node| node.title.clone())
                    .unwrap_or_else(|| path.clone());

                // Normalize extension to .html
                let html_path = Self::normalize_path_extension(path);

                // Pop stack until we reach the parent level
                while stack.len() > 1 && stack.last().unwrap().1 >= depth {
                    stack.pop();
                }

                // Parent is the last item on stack
                let parent_bid = stack.last().unwrap().0.clone();

                // Create new node
                let new_node = NavNode {
                    bid: bid_str.clone(),
                    title: node_title,
                    path: html_path,
                    parent: Some(parent_bid.clone()),
                    children: Vec::new(),
                };

                // Add node to map
                nodes_map.insert(bid_str.clone(), new_node);

                // Add this node as child to its parent
                if let Some(parent_node) = nodes_map.get_mut(&parent_bid) {
                    parent_node.children.push(bid_str.clone());
                }

                // Push to stack for potential children
                stack.push((bid_str, depth));
            }

            // Merge this network's nodes into global map
            root_nodes.push(network_bid_str.clone());
            for (bid, mut node) in nodes_map {
                // Remove network from its own children list (prevents self-reference)
                if bid == network_bid_str {
                    node.children
                        .retain(|child_bid| child_bid != &network_bid_str);
                }

                // Update parent references for network nodes (should be None, not parent = self)
                if node.parent.as_ref() == Some(&node.bid) {
                    node.parent = None;
                }
                root_nodes_map.insert(bid, node);
            }
        }

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
        use crate::codec::CODECS;
        use crate::paths::AnchorPath;

        let anchor_path = AnchorPath::new(path);
        let filepath = anchor_path.filepath();

        // Check if this is a codec document path
        let mut normalized = if CODECS.get(&anchor_path).is_some() {
            // Replace codec extension with .html
            anchor_path.replace_extension("html")
        } else if !filepath.ends_with(".html") {
            // No codec extension and no .html - treat as directory
            format!("{}/index.html", filepath)
        } else {
            // Already has .html extension
            filepath.to_string()
        };

        // Re-attach anchor fragment if present
        if !anchor_path.anchor().is_empty() {
            normalized.push('#');
            normalized.push_str(anchor_path.anchor());
        }

        normalized
    }
}

// Module is only compiled when wasm feature is enabled
#[cfg(not(feature = "wasm"))]
compile_error!("wasm module should only be compiled with wasm feature enabled");
