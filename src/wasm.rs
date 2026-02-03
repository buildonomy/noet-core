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
        asset_namespace, buildonomy_namespace, href_namespace, BeliefKind, BeliefNode, Bid,
        WeightKind, WEIGHT_SORT_KEY,
    },
    query::{BeliefSource, Expression, StatePred},
};

#[cfg(feature = "wasm")]
use serde::{Deserialize, Serialize};

#[cfg(feature = "wasm")]
use serde_json;

#[cfg(feature = "wasm")]
use enumset::EnumSet;

#[cfg(feature = "wasm")]
use std::collections::{BTreeMap, HashMap};

/// WASM-compatible node context (no lifetimes, fully owned)
///
/// This is a serializable version of BeliefContext that can cross the FFI boundary.
/// See `docs/design/interactive_viewer.md` § WASM Integration for specification.
#[cfg(feature = "wasm")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeContext {
    /// The node itself
    pub node: BeliefNode,
    /// Relative path within home network (e.g., "docs/guide.md#section")
    pub home_path: String,
    /// Home network BID (which Network node owns this document)
    pub home_net: Bid,
    /// All nodes related to this one (other end of all edges, both sources and sinks)
    /// Map from BID to BeliefNode for O(1) lookup when displaying graph relations
    pub related_nodes: BTreeMap<Bid, BeliefNode>,
    /// Relations by weight kind: Map<WeightKind, (sources, sinks)>
    /// Sources: BIDs of nodes linking TO this one
    /// Sinks: BIDs of nodes this one links TO
    /// Both vectors are sorted by WEIGHT_SORT_KEY edge payload value
    pub graph: HashMap<WeightKind, (Vec<Bid>, Vec<Bid>)>,
}

/// WASM wrapper around BeliefBase for browser use
///
/// Provides JavaScript-accessible methods for querying beliefs loaded from JSON.
#[cfg(feature = "wasm")]
#[wasm_bindgen]
pub struct BeliefBaseWasm {
    inner: std::cell::RefCell<BeliefBase>,
}

#[cfg(feature = "wasm")]
#[wasm_bindgen]
impl BeliefBaseWasm {
    /// Create a BeliefBase from JSON string (exported beliefbase.json)
    ///
    /// # JavaScript Example
    /// ```javascript
    /// const response = await fetch('beliefbase.json');
    /// const json = await response.text();
    /// const bb = BeliefBaseWasm.from_json(json);
    /// ```
    #[wasm_bindgen(constructor)]
    pub fn from_json(data: String) -> Result<BeliefBaseWasm, JsValue> {
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

        // Convert BeliefGraph to BeliefBase
        let inner = BeliefBase::from(graph);

        Ok(BeliefBaseWasm {
            inner: std::cell::RefCell::new(inner),
        })
    }

    /// Query nodes using Expression syntax
    ///
    /// This exposes the full BeliefSource query API to JavaScript.
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

        // Use BeliefSource trait to evaluate
        let inner = self.inner.borrow();
        let graph = inner.eval_unbalanced(&expr).await.map_err(|e| {
            let msg = format!("❌ Query failed: {}", e);
            console::error_1(&msg.clone().into());
            JsValue::from_str(&msg)
        })?;

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
        let ctx = match inner.get_context(&bid) {
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
        let ctx = match inner.get_context(&bid) {
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

        let graph = match futures::executor::block_on(inner.eval_unbalanced(&expr)) {
            Ok(g) => g,
            Err(e) => {
                console::error_1(&format!("❌ Failed to query networks: {}", e).into());
                return serde_wasm_bindgen::to_value(&Vec::<BeliefNode>::new()).unwrap();
            }
        };

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

        let graph = match futures::executor::block_on(inner.eval_unbalanced(&expr)) {
            Ok(g) => g,
            Err(e) => {
                console::error_1(&format!("❌ Failed to query documents: {}", e).into());
                return serde_wasm_bindgen::to_value(&Vec::<BeliefNode>::new()).unwrap();
            }
        };

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
    /// console.log(`Path: ${ctx.home_path}`);
    /// console.log(`Related nodes: ${ctx.related_nodes.length}`);
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
        let (node, home_path, home_net, related_nodes, graph) = {
            let mut inner = self.inner.borrow_mut();
            let ctx = match inner.get_context(&bid) {
                Some(c) => c,
                None => {
                    console::warn_1(&format!("⚠️ Node not found: {}", bid).into());
                    return JsValue::NULL;
                }
            };

            // Collect all related nodes (other end of all edges)
            let mut related_nodes = BTreeMap::new();
            let mut graph: HashMap<WeightKind, (Vec<(Bid, u16)>, Vec<(Bid, u16)>)> = HashMap::new();

            // Process sources (nodes linking TO this one)
            for ext_rel in ctx.sources() {
                // Collect all related nodes (the "other" end of the edge)
                related_nodes.insert(ext_rel.other.bid, ext_rel.other.clone());

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
                // Collect all related nodes (the "other" end of the edge)
                related_nodes.insert(ext_rel.other.bid, ext_rel.other.clone());

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
                ctx.home_path.clone(),
                ctx.home_net,
                related_nodes,
                sorted_graph,
            )
        }; // Drop the borrow here

        let node_context = NodeContext {
            node,
            home_path,
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
}

// Module is only compiled when wasm feature is enabled
#[cfg(not(feature = "wasm"))]
compile_error!("wasm module should only be compiled with wasm feature enabled");
