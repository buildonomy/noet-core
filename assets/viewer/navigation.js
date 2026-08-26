/**
 * viewer/navigation.js — Navigation tree build, render, and toggle
 *
 * Consumes the NavTree { nodes: Map<bid, NavNode>, roots: Array<bid> }
 * returned by BeliefBaseWasm.get_nav_tree().
 *
 * Rendering is purely functional (returns HTML strings); the DOM is updated
 * once per buildNavigation() call. Toggle clicks re-invoke buildNavigation()
 * which re-renders the whole tree — acceptable given typical tree sizes.
 *
 * Only expanded nodes have their children rendered into the DOM. Collapsed
 * nodes render a toggle button and their own label/link, but their children
 * subtree is omitted entirely. This keeps the DOM size proportional to the
 * number of visible nodes rather than the total corpus size, which is critical
 * for large corpora like MDN (~14k files).
 *
 * Expand/collapse state is tracked in state.expandedNodes (a Set of BIDs).
 * Root nodes are auto-expanded on the first render only.
 */

import { state, callbacks } from "./state.js";
import { escapeHtml } from "./utils.js";
import { ensureNetworkLoaded } from "./shard-manager.js";

// =============================================================================
// Public API
// =============================================================================

/**
 * Build and render the navigation tree from state.navTree.
 * Safe to call multiple times (e.g. after toggle).
 */
let _buildNavigationPending = false;

/**
 * Schedule a navigation rebuild on the next microtask.
 * Rapid sequential calls (e.g. shard-load → loadDocument → showMetadataPanel
 * all firing in the same event loop turn) collapse into a single render.
 */
function scheduleBuildNavigation() {
  if (_buildNavigationPending) return;
  _buildNavigationPending = true;
  Promise.resolve().then(() => {
    _buildNavigationPending = false;
    buildNavigation();
  });
}

export function buildNavigation() {
  if (!state.navContent) {
    console.warn("[Noet] Nav content container not found");
    return;
  }

  if (!state.navTree || !state.navTree.nodes || !state.navTree.roots) {
    console.error("[Noet] Navigation data incomplete:", {
      hasNavTree: !!state.navTree,
      hasNodes: !!state.navTree?.nodes,
      hasRoots: !!state.navTree?.roots,
    });
    state.navContent.innerHTML =
      '<p class="noet-placeholder">Navigation data not loaded</p>';
    return;
  }

  const nodeCount = state.navTree.nodes.size;
  const rootCount = state.navTree.roots.length;
  console.log(`[Noet] Building navigation: ${nodeCount} nodes, ${rootCount} roots`);

  // Auto-expand root nodes on first render only
  if (state.isFirstNavRender && state.navTree.roots.length > 0) {
    for (const rootBid of state.navTree.roots) {
      state.expandedNodes.add(rootBid);
    }
    state.isFirstNavRender = false;
  }

  // Cache getActiveBid for this render pass — it reads window.location and iterates
  // all navTree nodes, so calling it once per node (inside renderNavNode) would be O(n²).
  const activeBid = getActiveBid();

  // Expand ancestors of the active node, current document, and selected metadata node
  if (activeBid) {
    buildParentChain(activeBid);
    console.log(
      `[Noet] Active BID: ${activeBid}, expanded ${state.expandedNodes.size} ancestors`,
    );
  }
  if (state.currentDocBid && state.currentDocBid !== activeBid) {
    expandAncestors(state.currentDocBid);
  }
  if (
    state.selectedNodeBid &&
    state.selectedNodeBid !== activeBid &&
    state.selectedNodeBid !== state.currentDocBid
  ) {
    expandAncestors(state.selectedNodeBid);
  }

  const treeHtml = renderNavTree(activeBid);
  state.navContent.innerHTML = treeHtml;

  attachNavToggleListeners();
  scrollNavToActive();

  if (state.navError) {
    state.navError.hidden = true;
  }

  console.log("[Noet] Navigation tree built successfully");
}

/**
 * Re-render the navigation tree without touching state.expandedNodes.
 *
 * Use this when the user has manually toggled a node — we must not
 * re-expand ancestors and undo the user's intent. Also used by
 * updateNavTreeHighlight() to refresh highlights without side-effects.
 */
function rerenderNavTree() {
  if (!state.navContent) return;
  const activeBid = getActiveBid();
  const treeHtml = renderNavTree(activeBid);
  state.navContent.innerHTML = treeHtml;
  attachNavToggleListeners();
}

/**
 * Re-render the navigation tree to reflect the current active node,
 * current document (is-current-doc), and selected metadata node (is-selected).
 *
 * Previously this did a partial DOM patch, but with three independent highlight
 * axes a full rebuild is simpler and correct. Called after document loads and
 * metadata panel changes.
 */
export function updateNavTreeHighlight() {
  if (!state.navTree) return;
  scheduleBuildNavigation();
}

/**
 * Walk the nav tree in DFS order (same child order as renderNavTree/renderNavNode)
 * and return the document nodes immediately before and after `currentBid`.
 *
 * Only nodes where `node.is_document === true && node.path && node.path.length > 0`
 * are considered. Uses an iterative stack to avoid call-stack overflow on large corpora.
 *
 * @param {string} currentBid
 * @returns {{ prev: object|null, next: object|null }}
 */
export function getPrevNextDocs(currentBid) {
  if (!state.navTree || !state.navTree.nodes || !state.navTree.roots) {
    return { prev: null, next: null };
  }

  // Collect all document nodes in DFS order (iterative, pre-order, children in forward order).
  // We push roots right-to-left onto the stack so the leftmost root is processed first.
  const docNodes = [];
  const stack = [];

  for (let i = state.navTree.roots.length - 1; i >= 0; i--) {
    stack.push(state.navTree.roots[i]);
  }

  while (stack.length > 0) {
    const bid = stack.pop();
    const node = state.navTree.nodes.get(bid);
    if (!node) continue;

    if (node.is_document && node.path && node.path.length > 0) {
      docNodes.push({ bid, ...node });
    }

    // Push children right-to-left so the first child is processed next
    if (node.children && node.children.length > 0) {
      for (let i = node.children.length - 1; i >= 0; i--) {
        stack.push(node.children[i]);
      }
    }
  }

  const idx = docNodes.findIndex((n) => n.bid === currentBid);
  if (idx === -1) {
    return { prev: null, next: null };
  }

  return {
    prev: idx > 0 ? docNodes[idx - 1] : null,
    next: idx < docNodes.length - 1 ? docNodes[idx + 1] : null,
  };
}

/**
 * Update the `prev-next-top` and `prev-next-bottom` DOM elements with
 * previous/next document navigation links based on the current document BID.
 *
 * Links use `callbacks.navigateToLink` for click handling (same as the rest
 * of the viewer's SPA routing). Elements are hidden when there is no prev/next.
 */
export function updatePrevNext() {
  const topEl = document.getElementById("prev-next-top");
  const bottomEl = document.getElementById("prev-next-bottom");
  if (!topEl && !bottomEl) return;

  const { prev, next } = getPrevNextDocs(state.currentDocBid);

  /**
   * Build the innerHTML for a prev/next container and attach listeners.
   * @param {HTMLElement} el
   */
  function populate(el) {
    el.innerHTML = "";

    if (!prev && !next) {
      el.hidden = true;
      return;
    }

    el.hidden = false;

    // Prev link or placeholder (keeps grid columns stable)
    if (prev) {
      const prevPath = prev.path.startsWith("/") ? prev.path : `/${prev.path}`;
      const prevLink = document.createElement("a");
      prevLink.href = prevPath;
      prevLink.className = "noet-prev-next__prev";
      prevLink.innerHTML = `&#8592; ${escapeHtml(prev.title)}`;
      prevLink.addEventListener("click", (e) => {
        e.preventDefault();
        if (callbacks.navigateToLink) {
          callbacks.navigateToLink(prevPath, prevLink, prev.bid);
        }
      });
      el.appendChild(prevLink);
    } else {
      const placeholder = document.createElement("div");
      placeholder.className = "noet-prev-next__prev";
      el.appendChild(placeholder);
    }

    // Current page title in the center
    const titleSpan = document.createElement("span");
    titleSpan.className = "noet-prev-next__current";
    titleSpan.textContent = document.title || "";
    el.appendChild(titleSpan);

    // Next link or placeholder
    if (next) {
      const nextPath = next.path.startsWith("/") ? next.path : `/${next.path}`;
      const nextLink = document.createElement("a");
      nextLink.href = nextPath;
      nextLink.className = "noet-prev-next__next";
      nextLink.innerHTML = `${escapeHtml(next.title)} &#8594;`;
      nextLink.addEventListener("click", (e) => {
        e.preventDefault();
        if (callbacks.navigateToLink) {
          callbacks.navigateToLink(nextPath, nextLink, next.bid);
        }
      });
      el.appendChild(nextLink);
    } else {
      const placeholder = document.createElement("div");
      placeholder.className = "noet-prev-next__next";
      el.appendChild(placeholder);
    }
  }

  if (topEl) populate(topEl);
  if (bottomEl) populate(bottomEl);
}

/**
 * Get the BID that corresponds to the currently displayed document or section.
 * Tries multiple strategies in order.
 * @returns {string|null}
 */
export function getActiveBid() {
  // Strategy 1: body data-bid attribute (set by page template)
  if (document.body.dataset.bid) {
    return document.body.dataset.bid;
  }

  if (!state.navTree) return null;

  const currentPath = window.location.pathname;
  const currentHash = window.location.hash;

  // Strategy 2: match hash fragment against NavTree node paths. Use pathParts to canonicalize the
  // hash content rather than ad-hoc string stripping. pathParts("/net/doc.html#section") gives:
  //
  //   path="net", filename="doc.html", anchor="section"
  //
  // which we reconstruct as "net/doc.html#section" — the same form that get_nav_tree() writes into
  // NavNode.path (root-relative, no leading slash).
  if (currentHash && state.wasmModule) {
    const raw = currentHash.substring(1); // strip leading "#"
    const parts = state.wasmModule.BeliefBaseWasm.pathParts(raw);
    // canonicalize() reassembles the path without any leading slash
    const canonicalFull = parts.canonicalize();
    // filepath() may return a leading slash — strip it to match NavNode.path format
    const canonicalDocRaw = parts.filepath();
    const canonicalDoc =
      canonicalDocRaw && canonicalDocRaw.startsWith("/")
        ? canonicalDocRaw.substring(1)
        : canonicalDocRaw;

    console.log(
      `[Noet] getActiveBid strategy2: hash=${JSON.stringify(currentHash)} canonicalFull=${JSON.stringify(canonicalFull)} canonicalDoc=${JSON.stringify(canonicalDoc)}`,
    );

    for (const [bid, node] of state.navTree.nodes) {
      if (node.path && node.path === canonicalFull) {
        return bid;
      }
    }

    // Fallback: match doc path only (no anchor) — covers the case where the hash points at a
    // document but the active node is the document node itself.
    if (canonicalDoc) {
      for (const [bid, node] of state.navTree.nodes) {
        if (node.path && node.path === canonicalDoc) {
          return bid;
        }
      }
    }

    // Strategy 2b: no path match found — fall back to currentDocBid set by
    // loadDocument(). Handles xlsx tab HTML files whose path is
    // "workbook-tab.html" but whose node is registered as
    // "workbook.xlsx#workbook-tab-id" in the nav tree.
    if (state.currentDocBid) {
      console.log(
        `[Noet] getActiveBid strategy2b: no path match, using currentDocBid=${state.currentDocBid}`,
      );
      return state.currentDocBid;
    }

    console.log(`[Noet] getActiveBid strategy2: no match found`);
  }

  // Strategy 3: match pathname against NavTree node paths
  for (const [bid, node] of state.navTree.nodes) {
    if (node.path && currentPath.endsWith(node.path)) {
      return bid;
    }
  }

  // Strategy 4: section BID mapping stored in body dataset
  if (currentHash && document.body.dataset.sectionBids) {
    try {
      const sectionMap = JSON.parse(document.body.dataset.sectionBids);
      const sectionId = currentHash.substring(1);
      if (sectionMap[sectionId]) {
        return sectionMap[sectionId];
      }
    } catch (e) {
      console.warn("[Noet] Failed to parse section BID mapping:", e);
    }
  }

  return null;
}

// =============================================================================
// Internal — tree logic
// =============================================================================

/**
 * Rebuild expandedNodes to contain only the ancestor chain of activeBid.
 * Root nodes added by the first-render logic are preserved separately via
 * isFirstNavRender; here we only clear and rebuild for the active path.
 * @param {string} activeBid
 */
function buildParentChain(activeBid) {
  // Preserve root expansions that were set on first render
  const roots = new Set(state.navTree.roots || []);
  const preservedRoots = new Set(
    [...state.expandedNodes].filter((bid) => roots.has(bid)),
  );

  state.expandedNodes.clear();

  // Restore root expansions
  for (const bid of preservedRoots) {
    state.expandedNodes.add(bid);
  }

  // Walk parent chain upward
  expandAncestors(activeBid);
}

/**
 * Walk the parent chain of a BID upward, adding each ancestor to expandedNodes.
 * Safe to call multiple times for different BIDs — additive, does not clear.
 * @param {string} bid
 */
function expandAncestors(bid) {
  let currentBid = bid;
  while (currentBid) {
    state.expandedNodes.add(currentBid);
    const node = state.navTree.nodes.get(currentBid);
    if (!node) break;

    // If this node is an anchor child of a network node, also expand the
    // synthetic "index" stem so the anchor is visible without manual toggle.
    if (node.parent) {
      const parentNode = state.navTree.nodes.get(node.parent);
      if (parentNode && parentNode.is_network && !node.is_document && !node.is_network) {
        state.expandedNodes.add(node.parent + ":index");
      }
    }

    currentBid = node.parent;
  }
}

/**
 * Toggle expand/collapse for a nav node and re-render the tree.
 *
 * Because collapsed nodes no longer have their children in the DOM, a class
 * flip is insufficient — we must re-render to inject or remove the subtree.
 *
 * @param {string} bid
 */
function toggleNode(bid) {
  const isExpanded = state.expandedNodes.has(bid);
  console.log(`[Noet] Toggling node: ${bid}, currently expanded: ${isExpanded}`);

  if (isExpanded) {
    state.expandedNodes.delete(bid);
  } else {
    state.expandedNodes.add(bid);
    // If this is a network node and its shard isn't loaded, start loading it.
    // Suppress the scroll-to-active that would fire after the shard loads —
    // the user is exploring this subtree, not navigating to the active page.
    _suppressNavScroll = true;
    ensureNetworkLoaded(bid, state);
  }

  // Because collapsed nodes no longer have their children in the DOM, a class
  // flip is insufficient — we must re-render to inject or remove the subtree.
  // Use rerenderNavTree() here (not buildNavigation()) so that auto-expand
  // logic does not immediately undo the user's manual collapse.
  rerenderNavTree();
}

// =============================================================================
// Internal — HTML rendering
// =============================================================================

/**
 * Render the full navigation tree as an HTML string.
 * @param {string|null} activeBid - Pre-computed active BID (avoids O(n²) recomputation)
 * @returns {string}
 */
function renderNavTree(activeBid = null) {
  if (!state.navTree.roots || state.navTree.roots.length === 0) {
    console.error("[Noet] No roots to render");
    return '<p class="noet-placeholder">No networks found</p>';
  }

  // When there is a single root (the entry point), render its title as a
  // drawer heading and promote its children to top-level <li> elements.
  // This saves one level of horizontal indentation across the entire tree.
  if (state.navTree.roots.length === 1) {
    const rootBid = state.navTree.roots[0];
    const rootNode = state.navTree.nodes.get(rootBid);
    if (rootNode && rootNode.children && rootNode.children.length > 0) {
      const isActive =
        activeBid && (activeBid === rootBid || rootNode.path === activeBid);
      const hashHref = rootNode.path ? `#/${rootNode.path}` : "#";
      let html = `
        <h3 class="noet-nav-tree__heading">
          <a href="${escapeHtml(hashHref)}"
             class="noet-nav-tree__heading-link${isActive ? " active" : ""}"
             data-bid="${escapeHtml(rootBid)}"
             title="${escapeHtml(rootNode.title)}">
            ${escapeHtml(rootNode.title)}
          </a>
        </h3>`;
      html += '<ul class="noet-nav-tree">';
      const visited = new Set([rootBid]);
      if (rootNode.is_network) {
        html += renderNetworkChildren(rootBid, rootNode, 0, visited, activeBid);
      } else {
        for (const childBid of rootNode.children) {
          html += renderNavNode(childBid, 0, visited, activeBid);
        }
      }
      html += "</ul>";
      return html;
    }
  }

  // Multiple roots (fallback during partial shard loading): render each as
  // a normal tree node.
  let html = '<ul class="noet-nav-tree">';
  for (const rootBid of state.navTree.roots) {
    html += renderNavNode(rootBid, 0, new Set(), activeBid);
  }
  html += "</ul>";
  return html;
}

/**
 * Render a single navigation node and its children (recursive).
 * @param {string} bid
 * @param {number} depth - Current recursion depth (cycle/depth guard)
 * @param {Set<string>} visited - BIDs already rendered in this chain
 * @param {string|null} activeBid - Pre-computed active BID from buildNavigation()
 * @returns {string}
 */
function renderNavNode(bid, depth = 0, visited = new Set(), activeBid = null) {
  if (visited.has(bid)) {
    console.error(`[Noet] Cycle detected: node ${bid} already visited in this chain`);
    return `<li class="noet-nav-tree__item noet-error">⚠ Cycle detected: ${escapeHtml(bid)}</li>`;
  }

  if (depth > 50) {
    console.error(`[Noet] Max depth exceeded at node ${bid}`);
    return `<li class="noet-nav-tree__item noet-error">⚠ Max depth exceeded</li>`;
  }

  const node = state.navTree.nodes.get(bid);
  if (!node) {
    console.warn(`[Noet] Node not found for BID: ${bid}`);
    return "";
  }

  // Network nodes may have children that aren't loaded yet (unloaded shard).
  // Always treat them as having children so the toggle button is rendered —
  // clicking it triggers ensureNetworkLoaded, and when noet:shard-loaded fires,
  // buildNavigation() repopulates the children from the newly-loaded PathMap.
  const hasChildren = (node.children && node.children.length > 0) || node.is_network;
  const isExpanded = state.expandedNodes.has(bid);
  const isActive = bid === activeBid;
  const isCurrentDoc = !!state.currentDocBid && bid === state.currentDocBid;
  const isSelected = !!state.selectedNodeBid && bid === state.selectedNodeBid;

  let itemClass = "noet-nav-tree__item";
  if (hasChildren) itemClass += " has-children";
  if (isExpanded) itemClass += " is-expanded";
  if (isActive) itemClass += " active";
  if (isCurrentDoc) itemClass += " is-current-doc";
  if (isSelected) itemClass += " is-selected";
  if (node.is_network) itemClass += " is-network";
  else if (node.is_document) itemClass += " is-document";
  else itemClass += " is-anchor";

  let html = `<li class="${itemClass}" data-bid="${escapeHtml(bid)}">`;

  // Toggle button for nodes with children — icon is driven by CSS so no re-render needed on toggle
  if (hasChildren) {
    html += `
      <button class="noet-nav-tree__toggle"
              data-bid="${escapeHtml(bid)}"
              aria-label="Toggle ${escapeHtml(node.title)}"
              aria-expanded="${isExpanded}">
      </button>
    `;
  }

  // Link for nodes with a path, plain label for network roots without one.
  // Emit hash-based href (#/path) so clicking updates window.location.hash
  // via the SPA routing loop rather than issuing a real HTTP request.
  // A bare /path href would break on hard-refresh because static servers
  // only serve the SPA shell at the root — /pages/ is not a valid entry point.
  if (node.path && node.path.length > 0) {
    const hashHref = "#/" + node.path;
    html += `
      <a href="${escapeHtml(hashHref)}"
         class="noet-nav-tree__link${isActive ? " active" : ""}"
         data-bid="${escapeHtml(bid)}"
         title="${escapeHtml(node.title)}">
        ${escapeHtml(node.title)}
      </a>
    `;
  } else {
    html += `
      <span class="noet-nav-tree__label"
            title="${escapeHtml(node.title)}">
        ${escapeHtml(node.title)}
      </span>
    `;
  }

  // Only render children into the DOM when this node is expanded. Collapsed
  // nodes emit no children markup at all, keeping DOM size proportional to
  // the number of visible nodes rather than the full corpus size.
  if (hasChildren && isExpanded) {
    const newVisited = new Set(visited);
    newVisited.add(bid);

    html += '<ul class="noet-nav-tree__children">';

    // For network nodes, partition children into anchor children (section
    // headings from the network's index.md) and doc/subnet children. When
    // both groups exist, wrap the anchors in a collapsible "index" stem so
    // the tree stays concise by default.
    if (node.is_network) {
      if (node.children && node.children.length > 0) {
        html += renderNetworkChildren(bid, node, depth, newVisited, activeBid);
      }
      // If this network's shard hasn't loaded, its children list is
      // incomplete (may be empty or partial).  Append an ellipsis
      // placeholder and trigger shard loading.  When noet:shard-loaded
      // fires, the nav tree re-renders with the full children.
      if (state.shardManager) {
        const meta = state.shardManager.findNetworkForBid(bid);
        if (meta && !state.shardManager.isNetworkLoaded(meta.bref)) {
          html += `<li class="noet-nav-tree__item noet-nav-tree__shard-placeholder">`;
          html += `<span class="noet-nav-tree__label noet-nav-tree__ellipsis"
                         data-bid="${escapeHtml(bid)}"
                         title="Click to load">&hellip;</span>`;
          html += `</li>`;
          ensureNetworkLoaded(bid, state);
        }
      }
    } else {
      for (const childBid of node.children) {
        if (childBid === bid) {
          console.error(
            `[Noet] Self-reference detected: node ${bid} references itself as child`,
          );
          html += `<li class="noet-nav-tree__item noet-error">⚠ Self-reference detected</li>`;
          continue;
        }
        html += renderNavNode(childBid, depth + 1, newVisited, activeBid);
      }
    }

    html += "</ul>";
  }

  html += "</li>";
  return html;
}

/**
 * Render children of a network node, wrapping anchor children (index.md
 * section headings) in a collapsible "index" stem when doc/subnet children
 * also exist. This keeps the expanded network view concise.
 *
 * When only anchor children exist (no doc/subnet children), they render
 * directly without the stem wrapper.
 *
 * @param {string} networkBid - BID of the network node
 * @param {object} networkNode - NavNode for the network
 * @param {number} depth - Current recursion depth
 * @param {Set<string>} visited - BIDs already rendered in this chain
 * @param {string|null} activeBid - Pre-computed active BID
 * @returns {string} HTML for the network's children
 */
function renderNetworkChildren(networkBid, networkNode, depth, visited, activeBid) {
  const anchorBids = [];
  const docBids = [];

  for (const childBid of networkNode.children) {
    if (childBid === networkBid) {
      continue; // self-reference guard
    }
    const childNode = state.navTree.nodes.get(childBid);
    if (!childNode) continue;

    if (childNode.is_document || childNode.is_network) {
      docBids.push(childBid);
    } else {
      anchorBids.push(childBid);
    }
  }

  let html = "";

  // Only wrap in a stem when both groups exist; anchor-only networks
  // render their children directly (no empty stem).
  if (anchorBids.length > 0 && docBids.length > 0) {
    html += renderIndexStem(
      networkBid,
      networkNode,
      anchorBids,
      depth,
      visited,
      activeBid,
    );
  } else {
    // Anchor-only: render directly without stem wrapper
    for (const childBid of anchorBids) {
      html += renderNavNode(childBid, depth + 1, visited, activeBid);
    }
  }

  // Doc/subnet children always render directly
  for (const childBid of docBids) {
    html += renderNavNode(childBid, depth + 1, visited, activeBid);
  }

  return html;
}

/**
 * Render a synthetic "index" stem node that wraps a network's anchor children.
 * The stem is a collapsible tree item linking to the network's own index page.
 * Its expand/collapse state is tracked under the key "${networkBid}:index" in
 * state.expandedNodes — no fabricated BID needed.
 *
 * @param {string} networkBid - BID of the parent network node
 * @param {object} networkNode - NavNode for the network
 * @param {Array<string>} anchorBids - BIDs of anchor children to wrap
 * @param {number} depth - Current recursion depth
 * @param {Set<string>} visited - BIDs already rendered in this chain
 * @param {string|null} activeBid - Pre-computed active BID
 * @returns {string} HTML for the index stem <li>
 */
function renderIndexStem(networkBid, networkNode, anchorBids, depth, visited, activeBid) {
  const stemKey = networkBid + ":index";
  const isExpanded = state.expandedNodes.has(stemKey);

  let itemClass = "noet-nav-tree__item has-children is-index-stem";
  if (isExpanded) itemClass += " is-expanded";

  let html = `<li class="${itemClass}" data-bid="${escapeHtml(stemKey)}">`;

  html += `
    <button class="noet-nav-tree__toggle"
            data-bid="${escapeHtml(stemKey)}"
            aria-label="Toggle index sections"
            aria-expanded="${isExpanded}">
    </button>
  `;

  // Link to the network's own index page
  if (networkNode.path && networkNode.path.length > 0) {
    const hashHref = "#/" + networkNode.path;
    html += `
      <a href="${escapeHtml(hashHref)}"
         class="noet-nav-tree__link"
         title="Index sections for ${escapeHtml(networkNode.title)}">
        index
      </a>
    `;
  } else {
    html += `
      <span class="noet-nav-tree__label"
            title="Index sections for ${escapeHtml(networkNode.title)}">
        index
      </span>
    `;
  }

  if (isExpanded) {
    html += '<ul class="noet-nav-tree__children">';
    for (const childBid of anchorBids) {
      html += renderNavNode(childBid, depth + 1, visited, activeBid);
    }
    html += "</ul>";
  }

  html += "</li>";
  return html;
}

/**
 * Handle a click on a nav tree link for a network node.
 *
 * When the user clicks the link (not the toggle button) of a network node,
 * we ensure the network's shard is loaded and expand the node in the tree so
 * its children become visible — mirroring what the toggle button does.
 *
 * Non-network nodes are left to the normal navigateToLink path in viewer.js.
 *
 * @param {string} bid - BID of the clicked nav node
 */
export function handleNavLinkClick(bid) {
  const node = state.navTree?.nodes.get(bid);
  if (!node || !node.is_network) return;

  // Expand the node and load its shard if not already done.
  if (!state.expandedNodes.has(bid)) {
    state.expandedNodes.add(bid);
    ensureNetworkLoaded(bid, state);
  }

  // Re-render so the children subtree appears immediately.
  // Use rerenderNavTree() (not buildNavigation()) to avoid overriding the
  // user's manual expand with auto-expand ancestor logic.
  rerenderNavTree();
}

/**
 * Attach click handlers to all .noet-nav-tree__toggle buttons in navContent.
 * Called after every full re-render.
 */
function attachNavToggleListeners() {
  const toggleButtons = state.navContent.querySelectorAll(".noet-nav-tree__toggle");
  console.log(`[Noet] Attaching listeners to ${toggleButtons.length} toggle buttons`);

  toggleButtons.forEach((button) => {
    button.addEventListener("click", (e) => {
      e.preventDefault();
      e.stopPropagation();
      const bid = button.dataset.bid;
      if (bid) {
        toggleNode(bid);
      }
    });
  });
}

/**
 * Scroll the nav drawer so the active node is visible.
 *
 * Debounced: multiple rapid calls (e.g. from successive shard loads that
 * each rebuild the tree) coalesce into a single scroll after the DOM has
 * settled.  The 150 ms delay is long enough to absorb a burst of
 * shard-loaded events but short enough to feel responsive.
 */
let _scrollNavTimer = null;
let _suppressNavScroll = false;
function scrollNavToActive() {
  if (_scrollNavTimer) clearTimeout(_scrollNavTimer);
  _scrollNavTimer = setTimeout(() => {
    _scrollNavTimer = null;
    if (_suppressNavScroll) {
      _suppressNavScroll = false;
      return;
    }
    if (!state.navContent) return;
    const activeLink =
      state.navContent.querySelector(".noet-nav-tree__link.active") ||
      state.navContent.querySelector(".noet-nav-tree__item.is-current-doc");
    if (activeLink) {
      activeLink.scrollIntoView({ block: "center", behavior: "instant" });
    }
  }, 150);
}

// =============================================================================
// Shard-load event listener
// =============================================================================

/**
 * Register a listener for the `noet:shard-loaded` custom event.
 * Called from viewer.js after WASM init. When a background shard load
 * completes, rebuilds the nav tree so newly-available nodes appear.
 */
export function initShardLoadListener() {
  document.addEventListener("noet:shard-loaded", (e) => {
    console.log(`[Noet] Nav rebuild triggered by shard load: ${e.detail.bref}`);
    // Refresh the nav tree from WASM — the shard load updated PathMap entries
    // for the newly-loaded network's documents and sections, but state.navTree
    // was built before the load and does not reflect them.
    if (state.beliefbase) {
      state.navTree = state.beliefbase.get_nav_tree();
    }
    // Defer to the next animation frame so the hash change from any
    // concurrent navigation click has propagated before getActiveBid()
    // reads window.location.hash.
    requestAnimationFrame(() => {
      // Additive expand: keep all existing user expansions and just add
      // ancestors of the current active node.  buildNavigation() calls
      // buildParentChain() which clears expandedNodes — that would
      // collapse nodes the user manually expanded.  Instead, expand
      // ancestors additively and re-render without clearing.
      const activeBid = getActiveBid();
      if (activeBid) {
        expandAncestors(activeBid);
      }
      if (state.currentDocBid && state.currentDocBid !== activeBid) {
        expandAncestors(state.currentDocBid);
      }
      rerenderNavTree();
      scrollNavToActive();
    });
  });
}
