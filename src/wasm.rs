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
use crate::{
    beliefbase::{BeliefBase, BeliefGraph},
    nodekey::NodeKey,
    properties::{BeliefKind, BeliefNode, Bid},
    query::{BeliefSource, Expression, StatePred},
};

#[cfg(feature = "wasm")]
use serde_json;

#[cfg(feature = "wasm")]
use enumset::EnumSet;

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
        let graph: BeliefGraph = serde_json::from_str(&data)
            .map_err(|e| JsValue::from_str(&format!("Failed to parse JSON: {}", e)))?;

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
        let expr: Expression = serde_wasm_bindgen::from_value(expr_js)
            .map_err(|e| JsValue::from_str(&format!("Failed to parse Expression: {}", e)))?;

        // Use BeliefSource trait to evaluate
        let inner = self.inner.borrow();
        let graph = inner
            .eval_unbalanced(&expr)
            .await
            .map_err(|e| JsValue::from_str(&format!("Query failed: {}", e)))?;

        // Serialize result back to JavaScript
        serde_wasm_bindgen::to_value(&graph)
            .map_err(|e| JsValue::from_str(&format!("Failed to serialize result: {}", e)))
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
            Err(_) => return JsValue::NULL,
        };

        let inner = self.inner.borrow();
        let node_key = NodeKey::Bid { bid };
        match inner.get(&node_key) {
            Some(node) => serde_wasm_bindgen::to_value(&node).unwrap_or(JsValue::NULL),
            None => JsValue::NULL,
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
            Err(_) => return serde_wasm_bindgen::to_value(&Vec::<BeliefNode>::new()).unwrap(),
        };

        // Get context which includes sources (nodes that link to this one)
        let mut inner = self.inner.borrow_mut();
        let ctx = match inner.get_context(&bid) {
            Some(ctx) => ctx,
            None => return serde_wasm_bindgen::to_value(&Vec::<BeliefNode>::new()).unwrap(),
        };

        // Collect source nodes (ExtendedRelation.other is already &BeliefNode)
        let backlinks: Vec<&BeliefNode> = ctx
            .sources()
            .into_iter()
            .map(|ext_rel| ext_rel.other)
            .collect();

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
            Err(_) => return serde_wasm_bindgen::to_value(&Vec::<BeliefNode>::new()).unwrap(),
        };

        // Get context which includes sinks (nodes this one links to)
        let mut inner = self.inner.borrow_mut();
        let ctx = match inner.get_context(&bid) {
            Some(ctx) => ctx,
            None => return serde_wasm_bindgen::to_value(&Vec::<BeliefNode>::new()).unwrap(),
        };

        // Collect sink nodes (ExtendedRelation.other is already &BeliefNode)
        let forward_links: Vec<&BeliefNode> = ctx
            .sinks()
            .into_iter()
            .map(|ext_rel| ext_rel.other)
            .collect();

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
            Err(_) => return serde_wasm_bindgen::to_value(&Vec::<BeliefNode>::new()).unwrap(),
        };

        let networks: Vec<&BeliefNode> = graph.states.values().collect();
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
            Err(_) => return serde_wasm_bindgen::to_value(&Vec::<BeliefNode>::new()).unwrap(),
        };

        let documents: Vec<&BeliefNode> = graph.states.values().collect();
        serde_wasm_bindgen::to_value(&documents).unwrap_or(JsValue::NULL)
    }
}

// Module is only compiled when wasm feature is enabled
#[cfg(not(feature = "wasm"))]
compile_error!("wasm module should only be compiled with wasm feature enabled");
