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
 *   highlightExternalInContent lives in content.js. Same pattern applies.
 *
 * Asset link click behaviour (attachMetadataLinkHandlers):
 *   When the user clicks a relation link for an external node (asset or href),
 *   the metadata panel is always showing a node from the currently loaded
 *   document (opening the metadata panel requires navigating to that document
 *   first). We therefore always highlight the external element directly in the
 *   current document content, then update the panel to show the asset's own
 *   metadata so the user sees it in context.
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
  if (callbacks.updateNavTreeHighlight) {
    callbacks.updateNavTreeHighlight();
  }

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
  if (callbacks.updateNavTreeHighlight) {
    callbacks.updateNavTreeHighlight();
  }
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

  let html = '<div class="noet-metadata-section">';

  // ---- Node Information ----
  html += "<h3>Node Information</h3>";
  html += '<dl class="noet-metadata-list">';
  const displayTitle = node.title && node.title.length > 0 ? node.title : formatBid(node.bid);
  html += `<dt>Title</dt><dd>${escapeHtml(displayTitle)}</dd>`;
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
  const netNode = state.beliefbase ? state.beliefbase.get_by_bid(home_net) : null;
  const netTitle =
    netNode && netNode.title && netNode.title.length > 0 ? netNode.title : formatBid(home_net);
  html += `<dt>Network</dt><dd>${escapeHtml(netTitle)}</dd>`;
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
        html += renderRelationGroup(sources, "Dependencies", related_nodes);
        html += renderRelationGroup(sinks, "Referenced by", related_nodes);
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
 * @returns {string}
 */
function renderRelationGroup(bids, label, related_nodes) {
  if (bids.length === 0) return "";

  let html = '<div class="noet-relation-group">';
  html += `<p class="noet-metadata-label"><strong>${label}:</strong></p>`;
  html += '<ul class="noet-relation-list">';
  const hrefNamespace = state.wasmModule ? state.wasmModule.BeliefBaseWasm.href_namespace() : null;

  for (const bid of bids) {
    const relNode = related_nodes.get(bid);
    if (relNode) {
      const title = escapeHtml(relNode.node.title || relNode.link_title || bid);
      const kinds = Array.isArray(relNode.node.kind) ? relNode.node.kind : [];
      const isExternal = kinds.includes("External");
      const isHref = relNode.home_net === hrefNamespace?.bid;
      const path = relNode.root_path;

      if (isExternal) {
        const icon = isHref ? "🔗" : "📎";
        if (path) {
          html += `<li><span role="button" tabindex="0" class="noet-external-link" data-bid="${bid}" data-asset-path="${path}">${icon} ${title}</span></li>`;
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

  // Asset / href relation links — if the owner document is currently loaded,
  // highlight directly. If not, navigate to the owner doc first and defer the
  // highlight via pending state so loadDocument() executes it after injection.
  const assetLinks = state.metadataContent.querySelectorAll(".noet-external-link[role='button']");
  assetLinks.forEach((link) => {
    link.addEventListener("keydown", (e) => {
      if (e.key === "Enter" || e.key === " ") {
        e.preventDefault();
        link.click();
      }
    });
    link.addEventListener("click", (e) => {
      e.stopPropagation();
      const targetBid = link.getAttribute("data-bid");

      if (!targetBid) {
        console.warn("[Noet] External link: missing data-bid on element", link);
        return;
      }

      // Determine whether the owner document (the node whose metadata panel is
      // currently open) is the same as the loaded document.
      const ownerBid = state.selectedNodeBid;
      const ownerContext =
        ownerBid && state.beliefbase ? state.beliefbase.get_context(ownerBid) : null;
      // root_path is already normalized to .html by Rust (may include #anchor).
      // Strip leading slash for comparison with state.currentDocPath.
      const ownerFullPath = ownerContext
        ? ownerContext.root_path.startsWith("/")
          ? ownerContext.root_path.substring(1)
          : ownerContext.root_path
        : null;
      // Strip anchor for doc-level comparison only.
      const ownerDocPath = ownerFullPath ? ownerFullPath.split("#")[0] : null;

      const ownerIsLoaded = !ownerDocPath || ownerDocPath === state.currentDocPath;

      if (ownerIsLoaded) {
        // Owner doc is loaded — highlight directly, then show asset metadata.
        if (callbacks.highlightExternalInContent) {
          callbacks.highlightExternalInContent(targetBid);
        }
        showMetadataPanel(targetBid);
      } else {
        // Owner doc not loaded — navigate there, deferring highlight + metadata.
        state.pendingHighlightPath = targetBid;
        state.pendingMetadataBid = targetBid;
        // Navigate to the owner doc, landing at the section anchor if available
        const navPath = ownerFullPath.startsWith("/") ? ownerFullPath : `/${ownerFullPath}`;
        console.log("[Noet] External link: owner doc not loaded, navigating to:", navPath);
        if (callbacks.navigateToLink) {
          callbacks.navigateToLink(navPath, null, ownerBid);
        }
      }
    });
  });
}
