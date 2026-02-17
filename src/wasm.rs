//! WASM bindings for noet-core
//!
//! This module provides JavaScript-accessible APIs for querying BeliefGraphs in the browser.
//! It's designed for static site viewers that load `beliefbase.json` and provide client-side
//! search, navigation, and backlink exploration.
//!
//! ## Usage
//!
//! ```javascript
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

#[cfg(feature = "wasm")]
use serde::{Deserialize, Serialize};

#[cfg(feature = "wasm")]
use serde_json;

#[cfg(feature = "wasm")]
use enumset::EnumSet;

#[cfg(feature = "wasm")]
use std::collections::{BTreeMap, HashMap};

/// Navigation tree structure for hierarchical document navigation
///
/// Pre-structured tree generated in Rust for better performance than client-side tree building.
/// Uses a flat map structure with child IDs for efficient lookups and intelligent expand/collapse.
/// See `docs/design/interactive_viewer.md` § Navigation Tree Generation for specification.
#[cfg(feature = "wasm")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NavTree {
    /// Flat map of all nodes by BID (O(1) lookup)
    pub nodes: BTreeMap<String, NavNode>,
    /// Root node BIDs (networks) in display order
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
    pub related_nodes: BTreeMap<Bid, RelatedNode>,
    /// Relations by weight kind: Map<WeightKind, (sources, sinks)>
    /// Sources: BIDs of nodes linking TO this one
    /// Sinks: BIDs of nodes this one links TO
    /// Both vectors are sorted by WEIGHT_SORT_KEY edge payload value
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
        crate::paths::AnchorPath::new(path).normalize()
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
            base_path.join(end_with_hash)
        } else {
            base_path.join(end)
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

    /// Create a BeliefBase from JSON string (exported beliefbase.json) and metadata
    ///
    /// # JavaScript Example
    /// ```javascript
    /// const response = await fetch('beliefbase.json');
    /// const json = await response.text();
    /// const metadataScript = document.getElementById('noet-metadata');
    /// const metadata = metadataScript.textContent;
    /// const bb = new BeliefBaseWasm(json, metadata);
    /// ```
    #[wasm_bindgen(constructor)]
    pub fn from_json(data: String, metadata: String) -> Result<BeliefBaseWasm, JsValue> {
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

        // Parse metadata to extract entry point Bid
        let metadata_value: serde_json::Value = serde_json::from_str(&metadata).map_err(|e| {
            let msg = format!("❌ Failed to parse metadata JSON: {}", e);
            console::error_1(&msg.clone().into());
            JsValue::from_str(&msg)
        })?;

        let entry_point_bid = metadata_value
            .get("bid")
            .and_then(|v| v.as_str())
            .and_then(|s| Bid::try_from(s).ok())
            .ok_or_else(|| {
                let msg = "❌ Failed to extract entry point Bid from metadata";
                console::error_1(&msg.into());
                JsValue::from_str(msg)
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
    /// ```javascript
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
    /// ```javascript
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
    /// ```javascript
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

    /// Get nodes that link TO this node (backlinks)
    ///
    /// Returns array of nodes that reference the given BID.
    ///
    /// # JavaScript Example
    /// ```javascript
    /// const backlinks = bb.get_backlinks("01234567-89ab-cdef-0123-456789abcdef");
    /// console.log(`${backlinks.length} nodes link here`);
    /// ```
    #[wasm_bindgen]
    pub fn get_backlinks(&self, bid: String) -> JsValue {
        let bid = match Bid::try_from(bid.as_str()) {
            Ok(b) => b,
            Err(_) => {
                console::warn_1(&format!("⚠️ Invalid BID format: {}", bid).into());
                return serde_wasm_bindgen::to_value(&Vec::<BeliefNode>::new()).unwrap();
            }
        };

        // Get context which includes sources (nodes that link to this one)
        let mut inner = self.inner.borrow_mut();
        let ctx = match inner.get_context(&self.entry_point_bid, &bid) {
            Some(ctx) => ctx,
            None => {
                console::warn_1(&format!("⚠️ Node not found for backlinks: {}", bid).into());
                return serde_wasm_bindgen::to_value(&Vec::<BeliefNode>::new()).unwrap();
            }
        };

        // Collect source nodes (ExtendedRelation.other is already &BeliefNode)
        let backlinks: Vec<&BeliefNode> = ctx
            .sources()
            .into_iter()
            .map(|ext_rel| ext_rel.other)
            .collect();

        console::log_1(&format!("✅ Found {} backlinks", backlinks.len()).into());

        serde_wasm_bindgen::to_value(&backlinks).unwrap_or(JsValue::NULL)
    }

    /// Get nodes that this node links TO (forward links)
    ///
    /// Returns array of nodes referenced by the given BID.
    ///
    /// # JavaScript Example
    /// ```javascript
    /// const links = bb.get_forward_links("01234567-89ab-cdef-0123-456789abcdef");
    /// console.log(`This node links to ${links.length} other nodes`);
    /// ```
    #[wasm_bindgen]
    pub fn get_forward_links(&self, bid: String) -> JsValue {
        let bid = match Bid::try_from(bid.as_str()) {
            Ok(b) => b,
            Err(_) => {
                console::warn_1(&format!("⚠️ Invalid BID format: {}", bid).into());
                return serde_wasm_bindgen::to_value(&Vec::<BeliefNode>::new()).unwrap();
            }
        };

        // Get context which includes sinks (nodes this one links to)
        let mut inner = self.inner.borrow_mut();
        let ctx = match inner.get_context(&self.entry_point_bid, &bid) {
            Some(ctx) => ctx,
            None => {
                console::warn_1(&format!("⚠️ Node not found for forward links: {}", bid).into());
                return serde_wasm_bindgen::to_value(&Vec::<BeliefNode>::new()).unwrap();
            }
        };

        // Collect sink nodes (ExtendedRelation.other is already &BeliefNode)
        let forward_links: Vec<&BeliefNode> = ctx
            .sinks()
            .into_iter()
            .map(|ext_rel| ext_rel.other)
            .collect();

        console::log_1(&format!("✅ Found {} forward links", forward_links.len()).into());

        serde_wasm_bindgen::to_value(&forward_links).unwrap_or(JsValue::NULL)
    }

    /// Get total number of nodes in the belief base
    ///
    /// # JavaScript Example
    /// ```javascript
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
    /// ```javascript
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

    /// Get all network nodes (convenience wrapper around query)
    ///
    /// Returns array of nodes with kind "Network".
    ///
    /// # JavaScript Example
    /// ```javascript
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
    /// ```javascript
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
    /// # JavaScript Example
    /// ```javascript
    /// const ctx = bb.get_context("01234567-89ab-cdef-0123-456789abcdef");
    /// console.log(`Node: ${ctx.node.title}`);
    /// console.log(`Path: ${ctx.root_path}`);
    /// ```
    #[wasm_bindgen]
    pub fn get_context(&self, bid: String) -> JsValue {
        let bid = match Bid::try_from(bid.as_str()) {
            Ok(b) => b,
            Err(_) => {
                console::warn_1(&format!("⚠️ Invalid BID format: {}", bid).into());
                return JsValue::NULL;
            }
        };

        // Collect all data while holding the borrow
        let (node, root_path, home_net, related_nodes, graph) = {
            let mut inner = self.inner.borrow_mut();
            // get_context calls index_sync internally and needs mutable access
            let ctx = match inner.get_context(&self.entry_point_bid, &bid) {
                Some(c) => c,
                None => {
                    console::warn_1(&format!("⚠️ Node not found in context: {}", bid).into());
                    console::log_1(&format!("   Entry point: {}", self.entry_point_bid).into());
                    return JsValue::NULL;
                }
            };

            // Collect all related nodes (other end of all edges)
            let mut related_nodes = BTreeMap::new();
            let mut graph: HashMap<WeightKind, (Vec<(Bid, u16)>, Vec<(Bid, u16)>)> = HashMap::new();

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
                    let sort_key: u16 = weight.get(WEIGHT_SORT_KEY).unwrap_or(0);
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
                    let sort_key: u16 = weight.get(WEIGHT_SORT_KEY).unwrap_or(0);
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

            (
                ctx.node.clone(),
                ctx.root_path.clone(),
                ctx.home_net,
                related_nodes,
                sorted_graph,
            )
        }; // Drop the borrow here

        let node_context = NodeContext {
            node,
            root_path,
            home_net,
            related_nodes,
            graph,
        };

        console::log_1(&format!("✅ Got context for node: {}", node_context.node.title).into());

        serde_wasm_bindgen::to_value(&node_context).unwrap_or(JsValue::NULL)
    }

    /// Get href namespace BID (external HTTP/HTTPS links tracking network)
    ///
    /// See `docs/design/architecture.md` § 10 for network namespace details.
    ///
    /// # JavaScript Example
    /// ```javascript
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
    /// ```javascript
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
    /// ```javascript
    /// const api_bid = BeliefBaseWasm.buildonomy_namespace();
    /// ```
    #[wasm_bindgen]
    pub fn buildonomy_namespace() -> String {
        buildonomy_namespace().to_string()
    }

    /// Get all network path maps for navigation tree generation
    ///
    /// Returns a nested map structure:
    /// - Top level: network BID → PathMap data
    /// - PathMap data: array of [path, bid, order_indices] tuples
    ///
    /// This provides the complete document hierarchy for building navigation trees.
    /// The order_indices array contains sort keys from WEIGHT_SORT_KEY (Subsection relations).
    ///
    /// See `docs/design/interactive_viewer.md` § 8 (Navigation Tree Generation) for usage.
    ///
    /// # JavaScript Example
    /// ```javascript
    /// const paths = beliefbase.get_paths();
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
        use std::collections::BTreeMap;

        let paths = self.inner.borrow().paths();

        // Build nested map: network_bid → Vec<(path, bid, order_indices)>
        let nets: BTreeMap<String, Vec<(String, String, Vec<u16>)>> = paths
            .map()
            .iter()
            .map(|(net_bid, pm_lock)| {
                let pm = pm_lock.read();
                let path_data: Vec<(String, String, Vec<u16>)> = pm
                    .map()
                    .iter()
                    .map(|(path, bid, order)| (path.clone(), bid.to_string(), order.clone()))
                    .collect();
                (net_bid.to_string(), path_data)
            })
            .collect();

        serde_wasm_bindgen::to_value(&nets).unwrap_or_else(|e| {
            console::error_1(&format!("Failed to serialize paths: {}", e).into());
            JsValue::NULL
        })
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
    /// # JavaScript Example
    /// ```javascript
    /// const tree = beliefbase.get_nav_tree();
    /// // tree.nodes[0].title => "Network Name"
    /// // tree.nodes[0].children[0].title => "Document Title"
    /// // tree.nodes[0].children[0].children[0].title => "Section Title"
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

        for (net_bref, pm_lock) in paths.map().iter() {
            // Resolve Bref to Bid
            let net_bid = match brefs.get(net_bref) {
                Some(bid) => bid,
                None => continue, // Skip if we can't resolve the Bref
            };

            // Skip reserved BIDs (system namespaces and API nodes)
            if net_bid.is_reserved() {
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

            for (path, bid, order_indices) in pm.map().iter() {
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

    /// Helper: Normalize path extension to .html
    fn normalize_path_extension(path: &str) -> String {
        use crate::codec::CODECS;
        use crate::paths::AnchorPath;

        let anchor_path = AnchorPath::new(path);
        let filepath = anchor_path.filepath();

        // Check all registered codec extensions
        let mut normalized = filepath.to_string();
        let mut found_extension = false;
        for ext in CODECS.extensions() {
            let ext_str = format!(".{}", ext);
            if filepath.ends_with(&ext_str) {
                let base = &filepath[..filepath.len() - ext_str.len()];
                normalized = format!("{}.html", base);
                found_extension = true;
                break;
            }
        }

        // If no codec extension found and no .html extension, treat as directory
        if !found_extension && !filepath.ends_with(".html") {
            // Directory path - append /index.html
            normalized = format!("{}/index.html", filepath);
        }

        // Re-attach anchor fragment if present
        if !anchor_path.anchor().is_empty() {
            normalized.push_str("#");
            normalized.push_str(anchor_path.anchor());
        }

        normalized
    }
}

// Module is only compiled when wasm feature is enabled
#[cfg(not(feature = "wasm"))]
compile_error!("wasm module should only be compiled with wasm feature enabled");
