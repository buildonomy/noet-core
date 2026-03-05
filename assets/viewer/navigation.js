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
 * Expand/collapse state is tracked in state.expandedNodes (a Set of BIDs).
 * Root nodes are auto-expanded on the first render only.
 */

import { state } from "./state.js";
import { escapeHtml } from "./utils.js";

// =============================================================================
// Public API
// =============================================================================

/**
 * Build and render the navigation tree from state.navTree.
 * Safe to call multiple times (e.g. after toggle).
 */
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
    state.navContent.innerHTML = '<p class="noet-placeholder">Navigation data not loaded</p>';
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

  // Expand ancestors of the active node, current document, and selected metadata node
  const activeBid = getActiveBid();
  if (activeBid) {
    buildParentChain(activeBid);
    console.log(`[Noet] Active BID: ${activeBid}, expanded ${state.expandedNodes.size} ancestors`);
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

  const treeHtml = renderNavTree();
  state.navContent.innerHTML = treeHtml;

  attachNavToggleListeners();

  if (state.navError) {
    state.navError.hidden = true;
  }

  console.log("[Noet] Navigation tree built successfully");
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
  buildNavigation();
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
  // hash content rather than ad-hoc string stripping. pathParts("/net/doc.html#section) gives:
  //
  //   path="net", filename="doc.html", anchor="section"
  //
  // which we reconstruct as "net/doc.html#section -- the same form that get_nav_tree() writes into
  // NavNode.path (root-relative, no leading slash).
  if (currentHash && state.wasmModule) {
    const raw = currentHash.substring(1); //strip leading "#"
    const parts = state.wasmModule.BeliefBaseWasm.pathParts(raw);
    // Canonicalize() reassmbles the path without any leading slash
    const canonicalFull = parts.canonicalize();
    const canonicalDoc = parts.filepath();

    for (const [bid, node] of state.navTree.nodes) {
      if (node.path && node.path === canonicalFull) {
        return bid;
      }
    }

    // Fallback: match doc path only (no anchor) -- covers the case where the hash points at a
    // document but hte active node is the document node itself
    if (canonicalDoc) {
      for (const [bid, node] of state.navTree.nodes) {
        if (node.path && node.path === canonicalDoc) {
          return bid;
        }
      }
    }
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
  const preservedRoots = new Set([...state.expandedNodes].filter((bid) => roots.has(bid)));

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
    currentBid = node.parent;
  }
}

/**
 * Toggle expand/collapse for a node and re-render.
 * @param {string} bid
 */
function toggleNode(bid) {
  console.log(`[Noet] Toggling node: ${bid}, currently expanded: ${state.expandedNodes.has(bid)}`);
  if (state.expandedNodes.has(bid)) {
    state.expandedNodes.delete(bid);
  } else {
    state.expandedNodes.add(bid);
  }
  buildNavigation();
}

// =============================================================================
// Internal — HTML rendering
// =============================================================================

/**
 * Render the full navigation tree as an HTML string.
 * @returns {string}
 */
function renderNavTree() {
  if (!state.navTree.roots || state.navTree.roots.length === 0) {
    console.error("[Noet] No roots to render");
    return '<p class="noet-placeholder">No networks found</p>';
  }

  let html = '<ul class="noet-nav-tree">';
  for (const rootBid of state.navTree.roots) {
    html += renderNavNode(rootBid);
  }
  html += "</ul>";
  return html;
}

/**
 * Render a single navigation node and its children (recursive).
 * @param {string} bid
 * @param {number} depth - Current recursion depth (cycle/depth guard)
 * @param {Set<string>} visited - BIDs already rendered in this chain
 * @returns {string}
 */
function renderNavNode(bid, depth = 0, visited = new Set()) {
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

  const hasChildren = node.children && node.children.length > 0;
  const isExpanded = state.expandedNodes.has(bid);
  const isActive = bid === getActiveBid();
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

  // Toggle button for nodes with children
  if (hasChildren) {
    const toggleIcon = isExpanded ? "▼" : "▶";
    html += `
      <button class="noet-nav-tree__toggle"
              data-bid="${escapeHtml(bid)}"
              aria-label="Toggle ${escapeHtml(node.title)}"
              aria-expanded="${isExpanded}">
        ${toggleIcon}
      </button>
    `;
  }

  // Link for nodes with a path, plain label for network roots without one
  if (node.path && node.path.length > 0) {
    const absolutePath = node.path.startsWith("/") ? node.path : `/${node.path}`;
    html += `
      <a href="${escapeHtml(absolutePath)}"
         class="noet-nav-tree__link${isActive ? " active" : ""}"
         data-bid="${escapeHtml(bid)}">
        ${escapeHtml(node.title)}
      </a>
    `;
  } else {
    html += `
      <span class="noet-nav-tree__label">
        ${escapeHtml(node.title)}
      </span>
    `;
  }

  // Recursively render children when expanded
  if (hasChildren && isExpanded) {
    const newVisited = new Set(visited);
    newVisited.add(bid);

    html += '<ul class="noet-nav-tree__children">';
    for (const childBid of node.children) {
      if (childBid === bid) {
        console.error(`[Noet] Self-reference detected: node ${bid} references itself as child`);
        html += `<li class="noet-nav-tree__item noet-error">⚠ Self-reference detected</li>`;
        continue;
      }
      html += renderNavNode(childBid, depth + 1, newVisited);
    }
    html += "</ul>";
  }

  html += "</li>";
  return html;
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
