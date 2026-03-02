/**
 * viewer/metadata.js — Metadata panel display and node context rendering
 *
 * Responsible for:
 *   - showMetadataPanel(nodeBid): fetch NodeContext from WASM, render, expand panel
 *   - closeMetadataPanel(): collapse panel, clear selection
 *   - renderNodeContext(context): pure HTML-string renderer for a NodeContext
 *   - attachMetadataLinkHandlers(): wire up click handlers after innerHTML update
 *
 * Circular-import note:
 *   navigateToLink lives in routing.js. Rather than importing it here (which
 *   would create a routing↔metadata cycle), it is invoked via
 *   callbacks.navigateToLink registered in viewer.js at startup.
 *
 *   highlightAssetInContent lives in content.js. Same pattern applies.
 */

import { state, callbacks } from "./state.js";
import { escapeHtml, formatBid } from "./utils.js";
import { applyPanelState, savePanelState, showMetadataError } from "./panels.js";

// =============================================================================
// Public API
// =============================================================================

/**
 * Show the metadata panel populated with context for the given BID.
 * Expands the panel if it is currently collapsed.
 * @param {string} nodeBid
 */
export function showMetadataPanel(nodeBid) {
  if (!state.metadataPanel || !state.metadataContent || !state.beliefbase) {
    console.warn("[Noet] Cannot show metadata: missing panel or beliefbase");
    return;
  }

  state.selectedNodeBid = nodeBid;

  try {
    const context = state.beliefbase.get_context(nodeBid);

    if (!context) {
      showMetadataError();
      console.warn(`[Noet] No context found for BID: ${nodeBid}`);
      return;
    }

    if (state.metadataError) {
      state.metadataError.hidden = true;
    }

    state.metadataContent.innerHTML = renderNodeContext(context);

    // Expand panel
    state.panelState.metadataCollapsed = false;
    applyPanelState();

    // Wire up links inside the freshly rendered content
    attachMetadataLinkHandlers();
  } catch (error) {
    console.error("[Noet] Failed to load metadata:", error);
    showMetadataError();
  }
}

/**
 * Collapse the metadata panel and clear the selected node.
 */
export function closeMetadataPanel() {
  state.panelState.metadataCollapsed = true;
  applyPanelState();
  savePanelState();
  state.selectedNodeBid = null;
}

/**
 * Re-attach event handlers after an innerHTML update.
 * Call this whenever metadataContent is rewritten externally.
 */
export function updateMetadataPanel() {
  attachMetadataLinkHandlers();
}

// =============================================================================
// Rendering
// =============================================================================

/**
 * Render a NodeContext object as an HTML string.
 * Pure function — no DOM side-effects.
 *
 * @param {Object} context - NodeContext from BeliefBaseWasm.get_context()
 * @returns {string} HTML string ready to assign to innerHTML
 */
function renderNodeContext(context) {
  const { node, root_path, home_net, related_nodes, graph } = context;

  const hrefNamespace = state.wasmModule
    ? state.wasmModule.BeliefBaseWasm.href_namespace()
    : null;
  const assetNamespace = state.wasmModule
    ? state.wasmModule.BeliefBaseWasm.asset_namespace()
    : null;

  let html = '<div class="noet-metadata-section">';

  // ---- Node Information ----
  html += "<h3>Node Information</h3>";
  html += '<dl class="noet-metadata-list">';
  html += `<dt>Title</dt><dd>${escapeHtml(node.title)}</dd>`;
  html += `<dt>BID</dt><dd><code>${formatBid(node.bid)}</code></dd>`;

  if (node.kind && node.kind.length > 0) {
    const kinds = Array.isArray(node.kind) ? node.kind.join(", ") : node.kind;
    html += `<dt>Kind</dt><dd><code>${escapeHtml(kinds)}</code></dd>`;
  }

  if (node.schema) {
    html += `<dt>Schema</dt><dd><code>${escapeHtml(node.schema)}</code></dd>`;
  }

  if (node.id) {
    html += `<dt>ID</dt><dd><code>${escapeHtml(node.id)}</code></dd>`;
  }

  html += `<dt>Path</dt><dd><code>${escapeHtml(root_path)}</code></dd>`;
  html += `<dt>Network</dt><dd><code>${formatBid(home_net)}</code></dd>`;
  html += "</dl>";
  html += "</div>";

  // ---- Payload ----
  if (node.payload && Object.keys(node.payload).length > 0) {
    html += '<div class="noet-metadata-section">';
    html += "<h3>Payload</h3>";
    html += '<dl class="noet-metadata-list">';
    for (const [key, value] of Object.entries(node.payload)) {
      const valueStr = typeof value === "object" ? JSON.stringify(value) : String(value);
      html += `<dt>${escapeHtml(key)}</dt><dd>${escapeHtml(valueStr)}</dd>`;
    }
    html += "</dl>";
    html += "</div>";
  }

  // ---- Graph Relations ----
  if (graph && graph.size > 0) {
    html += '<div class="noet-metadata-section">';
    html += "<h3>Relations</h3>";

    for (const [weightKind, [sources, sinks]] of graph.entries()) {
      if (sources.length > 0 || sinks.length > 0) {
        html += `<h4>${escapeHtml(weightKind)}</h4>`;
        html += renderRelationGroup(sources, "Dependencies", related_nodes, hrefNamespace, assetNamespace);
        html += renderRelationGroup(sinks, "Referenced by", related_nodes, hrefNamespace, assetNamespace);
      }
    }

    html += "</div>";
  }

  return html;
}

/**
 * Render a single group of relation BIDs (sources or sinks) as an HTML fragment.
 * Returns empty string when bids is empty.
 *
 * @param {string[]} bids
 * @param {string} label - Section heading text ("Dependencies" or "Referenced by")
 * @param {Map<string, Object>} related_nodes
 * @param {Object|null} hrefNamespace
 * @param {Object|null} assetNamespace
 * @returns {string}
 */
function renderRelationGroup(bids, label, related_nodes, hrefNamespace, assetNamespace) {
  if (bids.length === 0) return "";

  let html = '<div class="noet-relation-group">';
  html += `<p class="noet-metadata-label"><strong>${label}:</strong></p>`;
  html += '<ul class="noet-relation-list">';

  for (const bid of bids) {
    const relNode = related_nodes.get(bid);
    if (relNode) {
      const title = escapeHtml(relNode.node.title || bid);
      const path = state.wasmModule
        ? state.wasmModule.BeliefBaseWasm.normalize_path_extension(relNode.root_path)
        : relNode.root_path;
      const homeNet = relNode.home_net;

      if (homeNet === hrefNamespace || homeNet === assetNamespace) {
        const icon = homeNet === hrefNamespace ? "🔗" : "📎";
        if (path) {
          html += `<li><a href="#" class="noet-asset-metadata-link" data-bid="${bid}" data-asset-path="${path}">${icon} ${title}</a></li>`;
        } else {
          html += `<li><span class="noet-asset-ref" title="Asset: ${bid}">📎 ${title}</span></li>`;
        }
      } else if (path) {
        const absolutePath = path.startsWith("/") ? path : `/${path}`;
        html += `<li><a href="${absolutePath}" class="noet-metadata-link" data-bid="${bid}">${title}</a></li>`;
      } else {
        html += `<li><span title="BID: ${bid}">${title}</span></li>`;
      }
    } else {
      html += `<li><span title="BID: ${bid}">${formatBid(bid)}</span></li>`;
    }
  }

  html += "</ul>";
  html += "</div>";
  return html;
}

// =============================================================================
// Event handlers
// =============================================================================

/**
 * Attach click handlers to all actionable links inside the metadata panel.
 * Must be called after every metadataContent innerHTML update.
 */
function attachMetadataLinkHandlers() {
  if (!state.metadataContent) return;

  // Internal document / section links
  const metadataLinks = state.metadataContent.querySelectorAll(
    ".noet-node-link, .noet-metadata-link",
  );
  metadataLinks.forEach((link) => {
    link.addEventListener("click", (e) => {
      e.preventDefault();
      const href = link.getAttribute("href");
      const targetBid = link.getAttribute("data-bid");
      if (href && callbacks.navigateToLink) {
        console.log("[Noet] Navigating to related node:", href);
        callbacks.navigateToLink(href, link, targetBid);
      }
    });
  });

  // Asset / href metadata links (highlight image or show asset metadata)
  const assetLinks = state.metadataContent.querySelectorAll(
    ".noet-asset-metadata-link, .noet-href-link",
  );
  assetLinks.forEach((link) => {
    link.addEventListener("click", (e) => {
      e.preventDefault();
      const targetBid = link.getAttribute("data-bid");
      const assetPath = link.getAttribute("data-asset-path");

      if (targetBid && assetPath) {
        if (callbacks.highlightAssetInContent) {
          callbacks.highlightAssetInContent(assetPath);
        }
        showMetadataPanel(targetBid);
      }
    });
  });
}
