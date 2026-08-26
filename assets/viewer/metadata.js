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
import { escapeHtml, brefFromBid } from "./utils.js";
import { applyPanelState, savePanelState, showMetadataError } from "./panels.js";
import { adjustGutterForCollapse } from "./resize.js";
import { ensureShardForBid } from "./shard-manager.js";

// =============================================================================
// Public API
// =============================================================================

/**
 * Show the metadata panel populated with context for the given BID.
 * Expands the panel if it is currently collapsed.
 *
 * When the node's shard is not yet loaded, attempts to load it on-demand
 * (using the global shard's bref_index to identify the correct network)
 * before falling back to the error state.
 *
 * @param {string} nodeBid
 */
export function showMetadataPanel(nodeBid) {
  console.log(
    `[Noet] showMetadataPanel: bid=${nodeBid}` +
      ` panel=${!!state.metadataPanel}` +
      ` content=${!!state.metadataContent}` +
      ` beliefbase=${!!state.beliefbase}`,
  );
  if (!state.metadataPanel || !state.metadataContent || !state.beliefbase) {
    console.warn("[Noet] Cannot show metadata: missing panel or beliefbase");
    return;
  }

  state.selectedNodeBid = nodeBid;
  if (callbacks.updateNavTreeHighlight) {
    callbacks.updateNavTreeHighlight();
  }

  try {
    const byBid = state.beliefbase.get_by_bid(nodeBid);
    const context = state.beliefbase.get_context(nodeBid);
    console.log(
      `[Noet] showMetadataPanel: get_by_bid=${JSON.stringify(byBid)}` +
        ` get_context=${context ? `{title: ${JSON.stringify(context.node?.title)}, root_path: ${JSON.stringify(context.root_path)}}` : "null"}`,
    );

    if (!context) {
      // The node's shard may not be loaded yet. Attempt to load it, then retry.
      _loadShardAndRetry(nodeBid);
      return;
    }

    _renderMetadataContext(nodeBid, context);
  } catch (error) {
    console.error("[Noet] Failed to load metadata:", error, error?.stack);
    showMetadataError();
  }
}

/**
 * Attempt to load the shard containing `nodeBid`, then retry showMetadataPanel.
 * If the shard cannot be identified or loaded, shows the error state.
 * @private
 */
function _loadShardAndRetry(nodeBid) {
  ensureShardForBid(nodeBid, state).then((loaded) => {
    if (loaded) {
      const context = state.beliefbase.get_context(nodeBid);
      if (context) {
        console.log(`[Noet] showMetadataPanel: shard loaded, retrying for ${nodeBid}`);
        _renderMetadataContext(nodeBid, context);
        return;
      }
    }
    console.warn(`[Noet] No context found for BID after shard load attempt: ${nodeBid}`);
    showMetadataError();
  });
}

/**
 * Navigate to a node by BID, loading its shard if necessary.
 *
 * All non-external metadata relation links use this path — no PathMap-derived
 * hrefs, no title-derived anchors.  The canonical path is resolved fresh from
 * get_context after the shard is confirmed loaded, ensuring the anchor matches
 * the bref-based id in the rendered HTML.
 * @private
 */
function _navigateByBid(targetBid, link) {
  ensureShardForBid(targetBid, state).then((loaded) => {
    if (loaded) {
      const context = state.beliefbase ? state.beliefbase.get_context(targetBid) : null;
      if (context && context.root_path && callbacks.navigateToLink) {
        const resolvedPath = context.root_path.startsWith("/")
          ? context.root_path
          : `/${context.root_path}`;
        callbacks.navigateToLink(resolvedPath, link, targetBid);
        return;
      }
    }
    // Shard couldn't load or path unresolvable — show metadata panel
    // so the user at least sees the node's context.
    showMetadataPanel(targetBid);
  });
}

/**
 * Render a NodeContext into the metadata panel. Extracted from showMetadataPanel
 * so it can be called both synchronously (shard already loaded) and after an
 * async shard load.
 * @private
 */
function _renderMetadataContext(nodeBid, context) {
  if (state.metadataError) {
    state.metadataError.hidden = true;
  }

  // Update the panel title: "[Node Title]: Details" with the title as a
  // navigable link to the node's root path. Falls back to "Details" if the
  // title span is absent (e.g. simple template).
  const titleSpan = document.getElementById("metadata-panel-title");
  if (titleSpan) {
    const displayTitle =
      context.node.title && context.node.title.length > 0
        ? context.node.title
        : context.node.bid;
    const rootPath = context.root_path || "";
    const absolutePath = rootPath.startsWith("/") ? rootPath : `/${rootPath}`;
    titleSpan.innerHTML =
      `<a class="noet-metadata-title-link noet-metadata-link" ` +
      `href="${escapeHtml(absolutePath)}" ` +
      `data-bid="${escapeHtml(nodeBid)}"` +
      `>${escapeHtml(displayTitle)}</a>: Details`;
  }

  const rendered = renderNodeContext(context);
  state.metadataContent.innerHTML = rendered;

  // Expand panel
  console.log(
    `[Noet] showMetadataPanel: expanding panel, panelState=${JSON.stringify(state.panelState)}` +
      ` metadataPanel.hidden=${state.metadataPanel.hidden}`,
  );
  if (state.panelState.metadataCollapsed) {
    state.panelState.metadataCollapsed = false;
    adjustGutterForCollapse("metadata", false);
  }
  applyPanelState();
  console.log(
    `[Noet] showMetadataPanel: after applyPanelState, metadataPanel.hidden=${state.metadataPanel.hidden}`,
  );

  // Wire up links inside the freshly rendered content (and the title link)
  attachMetadataLinkHandlers();
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
  // Reset the panel title to its neutral state.
  const titleSpan = document.getElementById("metadata-panel-title");
  if (titleSpan) {
    titleSpan.textContent = "Details";
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
  const { node, root_path, home_net, related_nodes, graph, metadata } = context;

  let html = "";

  // ---- Traceability button (first) ----
  // entry_bid: scope the submap to this node's subtree. Works for all node kinds —
  // Document nodes scope to the document, section nodes scope to that section and
  // its child headings, Network nodes scope to the whole network (entry "" equivalent).
  const entry_bid = node.bid;
  html += `<div class="noet-metadata-section">`;
  html += `<button class="noet-traceability-btn noet-metadata-action-btn"`;
  html += ` data-bid="${escapeHtml(node.bid)}"`;
  html += ` data-home-net="${escapeHtml(String(home_net))}"`;
  html += ` data-entry-bid="${escapeHtml(entry_bid)}"`;
  html += ` title="Toggle traceability view">`;
  html += `← Traceability View`;
  html += `</button>`;
  html += `</div>`;

  // ---- Node text hero block ----
  const text = typeof node.payload?.text === "string" ? node.payload.text.trim() : "";
  if (text) {
    const rendered =
      state.wasmModule?.BeliefBaseWasm?.render_markdown(text) ?? escapeHtml(text);
    html += `<div class="noet-metadata-section">`;
    html += `<div class="noet-node-text-wrap" data-base-path="${escapeHtml(root_path)}">`;
    html += `<div class="noet-node-text">${rendered}</div>`;
    html += `<button class="noet-copy-source-btn" data-source="${escapeHtml(text)}" `;
    html += `title="Copy source markdown" aria-label="Copy source markdown">📋</button>`;
    html += `</div>`;
    html += `</div>`;
  }

  // ---- Node Information ----
  html += '<div class="noet-metadata-section">';
  html += "<details>";
  html += '<summary><h3 style="display:inline">Node Information</h3></summary>';
  html += '<dl class="noet-metadata-list">';
  const displayTitle = node.title && node.title.length > 0 ? node.title : node.bid;
  html += `<dt>Title</dt><dd>${escapeHtml(displayTitle)}</dd>`;
  html += `<dt>BID</dt><dd><span style="white-space:nowrap"><code>${escapeHtml(node.bid)}</code></span></dd>`;
  html += `<dt>Bref</dt><dd><code>${escapeHtml(brefFromBid(node.bid))}</code></dd>`;

  if (node.kind && node.kind.length > 0) {
    const kinds = Array.isArray(node.kind) ? node.kind.join(", ") : node.kind;
    html += `<dt>Kind</dt><dd><code>${escapeHtml(kinds)}</code></dd>`;
  }

  if (node.schema) {
    html += `<dt>Schema</dt><dd><code>${escapeHtml(node.schema)}</code></dd>`;
  }

  {
    // Render explicit id when set; fall back to the implicit anchor slug derived
    // from the title (same as to_anchor(title) in Rust — the id used for NodeKey
    // lookups even when no explicit id was authored). Helps authors construct
    // semantic cross-references like [[id:load-switch-controller]].
    const displayId =
      node.id ||
      (node.title && typeof window.__noetToAnchor === "function"
        ? window.__noetToAnchor(node.title)
        : null);
    if (displayId) {
      const isImplicit = !node.id;
      html +=
        `<dt>ID</dt><dd><code>${escapeHtml(displayId)}</code>` +
        (isImplicit
          ? ` <span class="noet-metadata-implicit" title="Implicit — derived from title">〜</span>`
          : "") +
        `</dd>`;
    }
  }

  html += `<dt>Path</dt><dd><code>${escapeHtml(root_path)}</code></dd>`;
  const netNode = state.beliefbase ? state.beliefbase.get_by_bid(home_net) : null;
  const netTitle =
    netNode && netNode.title && netNode.title.length > 0 ? netNode.title : home_net;
  html += `<dt>Network</dt><dd>${escapeHtml(netTitle)}</dd>`;
  html += "</dl>";
  html += "</details>";
  html += "</div>";

  // ---- External URLs ----
  // When a content node is aliased to href_namespace (via url_aliases or
  // alias-template), the WASM get_context extracts the alias URLs from
  // the Section edge weight (WEIGHT_DOC_PATHS) and exposes them as
  // context.alias_urls.  Surface them as clickable external links.
  {
    const aliasUrls = context.alias_urls || [];
    if (aliasUrls.length > 0) {
      html += '<div class="noet-metadata-section">';
      html += `<h3>External ${aliasUrls.length === 1 ? "Link" : "Links"}</h3>`;
      html += '<ul class="noet-relation-list">';
      for (const url of aliasUrls) {
        html += `<li><a href="${escapeHtml(url)}" target="_blank" rel="noopener noreferrer" class="noet-metadata-link">🔗 ${escapeHtml(url)}</a></li>`;
      }
      html += "</ul>";
      html += "</div>";
    }
  }

  // ---- Source ----
  if (state.currentDocSourceLink || metadata?.source_url) {
    html += '<div class="noet-metadata-section">';
    html += "<h3>Source</h3>";
    html += '<dl class="noet-metadata-list">';
    if (state.currentDocSourceLink) {
      if (state.currentDocSourceBinary) {
        html +=
          `<dt>File</dt><dd><a href="${escapeHtml(state.currentDocSourceLink)}" ` +
          `download class="noet-metadata-link">⬇ Download Source</a></dd>`;
      } else {
        html +=
          `<dt>File</dt><dd><a href="${escapeHtml(state.currentDocSourceLink)}" ` +
          `target="_blank" rel="noopener noreferrer" class="noet-metadata-link">📄 View Source</a></dd>`;
      }
    }
    if (metadata?.source_url) {
      html +=
        `<dt>Edit</dt><dd><a href="${escapeHtml(metadata.source_url)}" ` +
        `target="_blank" rel="noopener noreferrer">View on remote ↗</a></dd>`;
    }
    html += "</dl>";
    html += "</div>";
  }

  // ---- Git Status (network nodes only) ----
  const isNetwork = Array.isArray(node.kind)
    ? node.kind.includes("Network")
    : node.kind === "Network";
  if (isNetwork && metadata?.git) {
    const git = metadata.git;
    html += '<div class="noet-metadata-section">';
    html += "<details>";
    html += '<summary><h3 style="display:inline">Git Status</h3></summary>';
    html += '<dl class="noet-metadata-list">';
    if (git.branch) {
      html += `<dt>Branch</dt><dd><code>${escapeHtml(git.branch)}</code></dd>`;
    }
    if (git.commit_short) {
      html += `<dt>Commit</dt><dd><code>${escapeHtml(git.commit_short)}</code></dd>`;
    }
    html += `<dt>Dirty</dt><dd>${git.dirty ? "Yes ⚠️" : "No ✓"}</dd>`;
    if (git.upstream) {
      html += `<dt>Upstream</dt><dd><code>${escapeHtml(git.upstream)}</code></dd>`;
    }
    if (git.ahead !== undefined || git.behind !== undefined) {
      html += `<dt>Ahead / Behind</dt><dd>${git.ahead ?? 0} / ${git.behind ?? 0}</dd>`;
    }
    if (git.last_commit_date) {
      html += `<dt>Last Commit</dt><dd>${escapeHtml(git.last_commit_date)}</dd>`;
    }
    html += "</dl>";
    html += "</details>";
    html += "</div>";
  }

  // ---- Directory listing (asset nodes with payload.listing) ----
  const listing = node.payload?.listing;
  if (Array.isArray(listing)) {
    const remoteUrl = node.payload.remote_url;
    const branch = node.payload.branch || "HEAD";
    const networkPrefix = node.payload.network_prefix || "";
    const dirPath = node.payload.dir_path || node.title || "";

    // Construct the base tree URL for this directory:
    //   {remote_url}/tree/{branch}/{network_prefix}/{dirPath}
    // network_prefix may be empty (network IS repo root).
    const remotePathParts = [networkPrefix, dirPath].filter(Boolean).join("/");
    const dirRemoteUrl = remoteUrl
      ? `${remoteUrl.replace(/\/$/, "")}/tree/${branch}/${remotePathParts}`
      : null;

    html += '<div class="noet-metadata-section">';
    html += "<h3>Directory</h3>";
    if (dirRemoteUrl) {
      html +=
        `<p><a href="${escapeHtml(dirRemoteUrl)}" target="_blank" rel="noopener noreferrer">` +
        `View on remote ↗</a></p>`;
    }
    if (node.payload.truncated) {
      html += `<p><em>Listing truncated at ${listing.length} entries.</em></p>`;
    }
    if (listing.length === 0) {
      html += "<p><em>Empty directory.</em></p>";
    } else {
      html += '<ul class="noet-relation-list">';
      for (const entry of listing) {
        if (remoteUrl) {
          const entryRemoteParts = [networkPrefix, dirPath, entry]
            .filter(Boolean)
            .join("/");
          // Use /blob/ for files (have an extension), /tree/ for subdirs (no dot after last slash).
          const entryType = entry.includes(".") ? "blob" : "tree";
          const entryUrl = `${remoteUrl.replace(/\/$/, "")}/${entryType}/${branch}/${entryRemoteParts}`;
          html +=
            `<li><a href="${escapeHtml(entryUrl)}" target="_blank" rel="noopener noreferrer">` +
            `${escapeHtml(entry)}</a></li>`;
        } else {
          html += `<li>${escapeHtml(entry)}</li>`;
        }
      }
      html += "</ul>";
    }
    html += "</div>";
  } else if (node.payload && Object.keys(node.payload).length > 0) {
    // ---- Payload (generic fallback for non-listing nodes) ----
    // Skip "text" (rendered as hero block above) and "listing"-related keys
    // (handled by the directory branch). Only render if remaining keys exist.
    const payloadEntries = Object.entries(node.payload).filter(([key]) => key !== "text");
    if (payloadEntries.length > 0) {
      html += '<div class="noet-metadata-section">';
      html += "<details>";
      html += '<summary><h3 style="display:inline">Payload</h3></summary>';
      html += '<dl class="noet-metadata-list">';
      for (const [key, value] of payloadEntries) {
        const valueStr =
          typeof value === "object" ? JSON.stringify(value, null, 2) : String(value);
        html += `<dt>${escapeHtml(key)}</dt><dd><pre><code>${escapeHtml(valueStr)}</code></pre></dd>`;
      }
      html += "</dl>";
      html += "</details>";
      html += "</div>";
    }
  }

  // ---- Graph Relations ----
  if (graph && graph.size > 0) {
    html += '<div class="noet-metadata-section">';
    html += "<h3>Relations</h3>";

    const RELATION_ORDER = ["Pragmatic", "Epistemic", "Section"];
    const allKinds = [...graph.keys()];
    // Known kinds in preferred order, then any unknown kinds alphabetically
    const orderedKinds = [
      ...RELATION_ORDER.filter((k) => graph.has(k)),
      ...allKinds.filter((k) => !RELATION_ORDER.includes(k)).sort(),
    ];

    for (const weightKind of orderedKinds) {
      const [sources, sinks] = graph.get(weightKind);
      if (sources.length > 0 || sinks.length > 0) {
        const isSection = weightKind === "Section";
        const openAttr = isSection ? "" : " open";
        html += `<details${openAttr}>`;
        html += `<summary><h4 style="display:inline">${escapeHtml(weightKind)}</h4></summary>`;
        html += renderRelationGroup(sources, "Dependencies", related_nodes);
        html += renderRelationGroup(sinks, "Referenced by", related_nodes);
        html += "</details>";
      }
    }

    html += "</div>";
  }

  return html;
}

/**
 * Render a single group of relation EdgeEntries (sources or sinks) as an HTML fragment.
 * Returns empty string when entries is empty.
 *
 * Each entry is an object `{ bid: string, owner_bid: string | null }` produced by the
 * Rust `EdgeEntry` struct. When `owner_bid` is present, the edge is owned by a third-party
 * section node (a `{maps_to}` directive); a "via <title>" annotation is appended.
 *
 * @param {Array<{bid: string, owner_bid: string|null}>} entries
 * @param {string} label - Section heading text ("Dependencies" or "Referenced by")
 * @param {Map<string, Object>} related_nodes
 * @returns {string}
 */
function renderRelationGroup(entries, label, related_nodes) {
  if (entries.length === 0) return "";

  let html = '<div class="noet-relation-group">';
  html += `<p class="noet-metadata-label"><strong>${label}:</strong></p>`;
  html += '<ul class="noet-relation-list">';
  const hrefNamespace = state.wasmModule
    ? state.wasmModule.BeliefBaseWasm.href_namespace()
    : null;

  for (const entry of entries) {
    // Support both plain BID strings (legacy) and EdgeEntry objects.
    const bid = typeof entry === "string" ? entry : entry.bid;
    const ownerBid = typeof entry === "string" ? null : (entry.owner_bid ?? null);

    const relNode = related_nodes.get(bid);
    let itemHtml = "";

    if (relNode) {
      const title = escapeHtml(relNode.node.title || relNode.link_title || bid);
      const kinds = Array.isArray(relNode.node.kind) ? relNode.node.kind : [];
      const isExternal = kinds.includes("External");
      const isHref = relNode.home_net === hrefNamespace?.bid;
      const path = relNode.root_path;

      if (isExternal) {
        const icon = isHref ? "🔗" : "📎";
        if (path) {
          itemHtml = `<span role="button" tabindex="0" class="noet-external-link" data-bid="${bid}" data-asset-path="${path}">${icon} ${title}</span>`;
        } else {
          itemHtml = `<span class="noet-asset-ref" title="Asset: ${bid}">📎 ${title}</span>`;
        }
      } else {
        // All non-external relation links use bref-based lazy resolution.
        // The click handler loads the target's shard (if needed), resolves the
        // canonical path via get_context, and navigates.  This avoids stale or
        // collided title-derived anchors from PathMap root_path.
        itemHtml = `<a href="#" class="noet-metadata-link noet-bref-link" data-bid="${bid}">${title}</a>`;
      }
    } else {
      itemHtml = `<span title="BID: ${bid}"><code>${escapeHtml(brefFromBid(bid))}</code></span>`;
    }

    // Append "via <owner title>" annotation for third-party owned edges.
    let viaHtml = "";
    if (ownerBid) {
      const ownerNode = related_nodes.get(ownerBid);
      if (ownerNode) {
        const ownerTitle = escapeHtml(
          ownerNode.node.title || ownerNode.link_title || ownerBid,
        );
        viaHtml = ` <span class="noet-via-owner">(<a href="#" class="noet-metadata-link noet-bref-link" data-bid="${ownerBid}">via</a>)</span>`;
      }
    }

    html += `<li>${itemHtml}${viaHtml}</li>`;
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

  // Panel title link — navigates to the node's root path via SPA routing.
  // The link lives outside metadataContent (in the header), so we query from
  // the whole document rather than from metadataContent.
  // Guard: showMetadataPanel replaces the <a> element via innerHTML each time
  // it runs, so the old listener is naturally discarded. updateMetadataPanel
  // does NOT replace the element, so we use a data-sentinel to prevent stacking
  // listeners on the same element across repeated updateMetadataPanel calls.
  const titleLink = document
    .getElementById("metadata-panel-title")
    ?.querySelector(".noet-metadata-title-link");
  if (titleLink && !titleLink.dataset.handlerAttached) {
    titleLink.dataset.handlerAttached = "1";
    titleLink.addEventListener("click", (e) => {
      e.preventDefault();
      const path = titleLink.getAttribute("href");
      const bid = titleLink.getAttribute("data-bid");
      if (path && callbacks.navigateToLink) {
        callbacks.navigateToLink(path, titleLink, bid);
      }
    });
  }

  // Internal document / section links
  const metadataLinks = state.metadataContent.querySelectorAll(
    ".noet-node-link, .noet-metadata-link",
  );
  metadataLinks.forEach((link) => {
    link.addEventListener("click", (e) => {
      // Let external links (target="_blank") and download links open normally.
      if (link.getAttribute("target") === "_blank" || link.hasAttribute("download"))
        return;
      e.preventDefault();
      const targetBid = link.getAttribute("data-bid");
      if (!targetBid) return;

      // All non-external relation links use bref-based lazy resolution:
      // load the target's shard if needed, resolve the canonical path
      // via get_context, then navigate.
      _navigateByBid(targetBid, link);
    });
  });

  // Asset / href relation links — if the owner document is currently loaded,
  // highlight directly. If not, navigate to the owner doc first and defer the
  // highlight via pending state so loadDocument() executes it after injection.
  const assetLinks = state.metadataContent.querySelectorAll(
    ".noet-external-link[role='button']",
  );
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
        const navPath = ownerFullPath.startsWith("/")
          ? ownerFullPath
          : `/${ownerFullPath}`;
        console.log(
          "[Noet] External link: owner doc not loaded, navigating to:",
          navPath,
        );
        if (callbacks.navigateToLink) {
          callbacks.navigateToLink(navPath, null, ownerBid);
        }
      }
    });
  });

  // Copy source button — copy raw markdown to clipboard
  // Links inside .noet-node-text-wrap — resolve relative hrefs against the node's
  // root_path (not window.location.hash which reflects the body document, not the
  // metadata-focused node).
  state.metadataContent.querySelectorAll(".noet-node-text-wrap").forEach((wrap) => {
    const basePath = wrap.getAttribute("data-base-path") || "";
    // Compute parent directory of the node's root_path (strip filename).
    let baseDir = "";
    if (basePath && state.wasmModule) {
      const parts = state.wasmModule.BeliefBaseWasm.pathParts(basePath);
      baseDir = parts.path || "";
    }
    wrap.querySelectorAll("a[href]").forEach((link) => {
      link.addEventListener("click", (e) => {
        e.preventDefault();
        e.stopPropagation(); // don't bubble to the generic metadataLinks handler
        let href = link.getAttribute("href");
        if (!href) return;
        // Pre-resolve relative hrefs against the node's directory so navigateToLink
        // receives an already-absolute path and skips its hash-based resolution.
        if (state.wasmModule && !href.startsWith("/") && !href.startsWith("#")) {
          const hrefParts = state.wasmModule.BeliefBaseWasm.pathParts(href);
          if (!hrefParts.hasSchema && baseDir) {
            const joined = state.wasmModule.BeliefBaseWasm.pathJoin(baseDir, href, false);
            href = joined.startsWith("/") ? joined : `/${joined}`;
          }
        }
        if (callbacks.navigateToLink) {
          callbacks.navigateToLink(href, link, link.getAttribute("data-bid") ?? null);
        }
      });
    });
  });

  const copySourceBtns = state.metadataContent.querySelectorAll(".noet-copy-source-btn");
  copySourceBtns.forEach((btn) => {
    btn.addEventListener("click", () => {
      const source = btn.getAttribute("data-source") || "";
      navigator.clipboard
        .writeText(source)
        .then(() => {
          btn.textContent = "✓";
          setTimeout(() => {
            btn.textContent = "📋";
          }, 1500);
        })
        .catch(() => {
          btn.textContent = "✗";
          setTimeout(() => {
            btn.textContent = "📋";
          }, 1500);
        });
    });
  });

  // Traceability button — toggles the traceability panel open/closed.
  const traceabilityBtns = state.metadataContent.querySelectorAll(
    ".noet-traceability-btn",
  );
  traceabilityBtns.forEach((btn) => {
    btn.addEventListener("click", () => {
      const panelEl = document.getElementById("noet-focused-panel");
      const isOpen = panelEl?.classList.contains("is-open");
      if (isOpen) {
        if (callbacks.closeTraceabilityModal) {
          callbacks.closeTraceabilityModal();
        }
      } else {
        const bid = btn.getAttribute("data-bid");
        const homeNet = btn.getAttribute("data-home-net");
        const entryBid = btn.getAttribute("data-entry-bid") || "";
        if (bid && homeNet && callbacks.openTraceabilityModal) {
          callbacks.openTraceabilityModal(bid, homeNet, entryBid);
        }
      }
    });
  });

  // Sync button visual state with current panel open state.
  syncTraceabilityBtnState();
}

/**
 * Update all .noet-traceability-btn elements to reflect whether the traceability
 * panel is currently open. Called after render and should be called by
 * openTraceabilityModal / closeTraceabilityModal via the callback.
 */
export function syncTraceabilityBtnState() {
  const panelEl = document.getElementById("noet-focused-panel");
  const isOpen = panelEl?.classList.contains("is-open") ?? false;
  document.querySelectorAll(".noet-traceability-btn").forEach((btn) => {
    if (isOpen) {
      btn.classList.add("is-active");
      btn.textContent = "→ Traceability View";
    } else {
      btn.classList.remove("is-active");
      btn.textContent = "← Traceability View";
    }
  });
}
