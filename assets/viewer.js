/**
 * Noet Viewer - Interactive SPA for HTML documents
 *
 * This script provides:
 * - Theme switching (light/dark mode)
 * - WASM-powered navigation tree
 * - Two-click navigation (Step 2)
 * - Metadata panel display (Step 2)
 * - Query builder UI (Step 3)
 * - Force-directed graph visualization (Step 4)
 *
 * Architecture:
 * - Loads WASM module (BeliefBaseWasm) for graph queries
 * - Reads metadata from #noet-metadata JSON script tag
 * - Manipulates DOM containers defined in template-responsive.html
 * - Uses CSS custom properties from noet-theme-*.css
 */

// =============================================================================
// DOM References
// =============================================================================

/** @type {HTMLElement} */
let headerElement;
/** @type {HTMLElement} */
let navElement;
/** @type {HTMLElement} */
let navContent;
/** @type {HTMLElement} */
let navError;
/** @type {HTMLElement} */
let contentElement;
/** @type {HTMLElement} */
let metadataPanel;
/** @type {HTMLElement} */
let metadataContent;
/** @type {HTMLElement} */
let metadataError;
/** @type {HTMLElement} */
let graphContainer;
/** @type {HTMLElement} */
let graphCanvas;
/** @type {HTMLElement} */
let footerElement;

/** @type {HTMLSelectElement} */
let themeSelect;
/** @type {HTMLButtonElement} */
let metadataClose;
/** @type {HTMLButtonElement} */
let graphClose;
/** @type {HTMLButtonElement} */
let navCollapseBtn;
/** @type {HTMLButtonElement} */
let metadataCollapseBtn;

/** @type {HTMLLinkElement} */
let themeLightLink;
/** @type {HTMLLinkElement} */
let themeDarkLink;

/** @type {HTMLElement} */
let containerElement;

// =============================================================================
// State
// =============================================================================

/** Document metadata loaded from JSON */
let documentMetadata = null;

/** Current theme: "system", "light", or "dark" */
let currentTheme = "system";

/** Currently selected node BID (for metadata display) */
let selectedNodeBid = null;

/** WASM module instance */
let wasmModule = null;

/** BeliefBaseWasm instance */
let beliefbase = null;

/** Navigation tree data (flat map structure from get_nav_tree()) */
let navTree = null;

/** Set of expanded node BIDs for navigation tree */
let expandedNodes = new Set();

/** Track first navigation render to auto-expand roots only once */
let isFirstNavRender = true;

/** Panel collapse state (persisted to localStorage) */
let panelState = {
  navCollapsed: false,
  metadataCollapsed: false,
};

// =============================================================================
// Initialization
// =============================================================================

/**
 * Initialize viewer on DOMContentLoaded
 */
document.addEventListener("DOMContentLoaded", async () => {
  console.log("[Noet] Initializing viewer...");

  // Cache DOM references
  initializeDOMReferences();

  // Load document metadata
  loadMetadata();

  // Set up event listeners
  setupEventListeners();

  // Initialize theme (read from localStorage or default to light)
  initializeTheme();

  // Initialize panel collapse state
  initializePanelState();

  // Load WASM and BeliefBase (non-blocking - theme should work even if this fails)
  try {
    await initializeWasm();
  } catch (error) {
    console.error(
      "[Noet] WASM initialization failed (theme and basic features still work):",
      error,
    );
    // Show error state in navigation panel
    showNavError();
  }

  console.log("[Noet] Viewer initialized successfully");
});

/**
 * Cache all DOM element references
 */
function initializeDOMReferences() {
  // Container elements
  containerElement = document.querySelector(".noet-container");
  headerElement = document.querySelector(".noet-header");
  navElement = document.querySelector(".noet-nav");
  navContent = document.getElementById("nav-content");
  navError = document.getElementById("nav-error");
  contentElement = document.querySelector(".noet-content");
  metadataPanel = document.getElementById("metadata-panel");
  metadataContent = document.getElementById("metadata-content");
  metadataError = document.getElementById("metadata-error");
  graphContainer = document.getElementById("graph-container");
  graphCanvas = document.getElementById("graph-canvas");
  footerElement = document.querySelector(".noet-footer");

  // Interactive elements
  themeSelect = document.getElementById("theme-select");
  metadataClose = document.getElementById("metadata-close");
  graphClose = document.getElementById("graph-close");
  navCollapseBtn = document.getElementById("nav-collapse");
  metadataCollapseBtn = document.getElementById("metadata-collapse");

  // Theme stylesheets
  themeLightLink = document.getElementById("theme-light");
  themeDarkLink = document.getElementById("theme-dark");

  // Verify critical elements exist
  if (!navContent || !metadataPanel || !metadataContent || !containerElement) {
    console.error("[Noet] Critical DOM elements missing - viewer may not work correctly");
  }

  // Verify theme elements exist
  if (!themeSelect) {
    console.error("[Noet] Theme select element not found");
  }
  if (!themeLightLink) {
    console.error("[Noet] Light theme stylesheet link not found");
  }
  if (!themeDarkLink) {
    console.error("[Noet] Dark theme stylesheet link not found");
  }
}

/**
 * Load document metadata from embedded JSON
 */
function loadMetadata() {
  const metadataScript = document.getElementById("noet-metadata");
  if (!metadataScript) {
    console.warn("[Noet] No metadata found in document");
    return;
  }

  try {
    documentMetadata = JSON.parse(metadataScript.textContent);
    console.log("[Noet] Loaded metadata:", documentMetadata);
  } catch (error) {
    console.error("[Noet] Failed to parse metadata:", error);
  }
}

/**
 * Set up all event listeners
 */
function setupEventListeners() {
  // Theme select
  if (themeSelect) {
    themeSelect.addEventListener("change", handleThemeChange);
  }

  // Metadata panel close button
  if (metadataClose) {
    metadataClose.addEventListener("click", closeMetadataPanel);
  }

  // Graph close button
  if (graphClose) {
    graphClose.addEventListener("click", closeGraphView);
  }

  // Panel collapse buttons (desktop only)
  if (navCollapseBtn) {
    navCollapseBtn.addEventListener("click", toggleNavPanel);
  }

  if (metadataCollapseBtn) {
    metadataCollapseBtn.addEventListener("click", toggleMetadataPanel);
  }

  // Keyboard shortcuts for panel collapse (desktop only)
  document.addEventListener("keydown", handleKeyboardShortcuts);

  // Navigation tree toggle/collapse (delegated event listener)
  if (navContent) {
    navContent.addEventListener("click", handleNavClick);
  }

  // TODO (Step 2.6): Add click handlers for content links (two-click pattern)
}

/**
 * Handle clicks in navigation tree (delegated event handling)
 * @param {Event} event - Click event
 */
function handleNavClick(event) {
  const target = event.target;

  // Handle toggle button clicks
  if (target.classList.contains("noet-nav-tree__toggle")) {
    event.preventDefault();
    const parentLi = target.closest("li");
    const childrenContainer = parentLi.querySelector(".noet-nav-tree__children");

    if (childrenContainer) {
      const isExpanded = parentLi.classList.toggle("is-expanded");
      target.textContent = isExpanded ? "▾" : "›";
      target.setAttribute("aria-expanded", isExpanded);
    }
  }

  // Handle navigation link clicks
  if (target.classList.contains("noet-nav-tree__link")) {
    // For now, allow default navigation behavior
    // TODO (Phase 2.6): Implement two-click pattern with client-side fetching
    console.log("[Noet] Navigating to:", target.getAttribute("data-path"));
  }
}

// =============================================================================
// Theme Switching
// =============================================================================

/**
 * Initialize theme from localStorage or default to system
 */
function initializeTheme() {
  // Check localStorage for saved preference
  const savedTheme = localStorage.getItem("noet-theme");
  if (savedTheme === "system" || savedTheme === "dark" || savedTheme === "light") {
    currentTheme = savedTheme;
  } else {
    // Default to system preference
    currentTheme = "system";
  }

  // Update select dropdown
  if (themeSelect) {
    themeSelect.value = currentTheme;
  }

  applyTheme(currentTheme);

  // Listen for system theme changes (when theme is set to "system")
  const mediaQuery = window.matchMedia("(prefers-color-scheme: dark)");
  mediaQuery.addEventListener("change", handleSystemThemeChange);
}

/**
 * Handle system theme preference change
 */
function handleSystemThemeChange() {
  // Only respond if theme is set to "system"
  if (currentTheme === "system") {
    applyTheme("system");
  }
}

/**
 * Handle theme select change
 */
function handleThemeChange(event) {
  currentTheme = event.target.value;
  console.log(`[Noet] Theme changed to: ${currentTheme}`);
  applyTheme(currentTheme);
  localStorage.setItem("noet-theme", currentTheme);
}

/**
 * Apply theme by enabling/disabling stylesheets
 * @param {string} theme - "system", "light", or "dark"
 */
function applyTheme(theme) {
  // Safety check
  if (!themeLightLink || !themeDarkLink) {
    console.error("[Noet] Theme stylesheet links not found, cannot apply theme");
    return;
  }

  let effectiveTheme = theme;

  // Resolve "system" to actual theme based on system preference
  if (theme === "system") {
    const prefersDark = window.matchMedia("(prefers-color-scheme: dark)").matches;
    effectiveTheme = prefersDark ? "dark" : "light";
    console.log(`[Noet] System preference detected: ${effectiveTheme}`);
  }

  // Apply the effective theme
  if (effectiveTheme === "dark") {
    themeLightLink.disabled = true;
    themeDarkLink.disabled = false;
    document.documentElement.setAttribute("data-theme", "dark");
  } else {
    themeLightLink.disabled = false;
    themeDarkLink.disabled = true;
    document.documentElement.setAttribute("data-theme", "light");
  }

  console.log(`[Noet] Theme applied: ${theme} (effective: ${effectiveTheme})`);
  console.log(
    `[Noet] Light stylesheet disabled: ${themeLightLink.disabled}, Dark stylesheet disabled: ${themeDarkLink.disabled}`,
  );
}

// =============================================================================
// WASM Initialization
// =============================================================================

/**
 * Initialize WASM module and BeliefBase
 */
async function initializeWasm() {
  try {
    console.log("[Noet] Loading WASM module...");

    // Dynamically import WASM module
    wasmModule = await import("./noet_core.js");
    await wasmModule.default();

    console.log("[Noet] WASM module loaded successfully");

    // Load beliefbase.json
    console.log("[Noet] Loading beliefbase.json...");
    const response = await fetch("./beliefbase.json");
    if (!response.ok) {
      throw new Error(`Failed to fetch beliefbase.json: ${response.status}`);
    }

    const beliefbaseJson = await response.text();
    console.log("[Noet] BeliefBase JSON loaded successfully");

    // Initialize BeliefBaseWasm (from_json is a constructor in WASM bindings)
    beliefbase = new wasmModule.BeliefBaseWasm(beliefbaseJson);
    console.log("[Noet] BeliefBaseWasm initialized");
    console.log("[Noet] BeliefBase loaded successfully");

    // Get navigation tree (flat map structure)
    navTree = beliefbase.get_nav_tree();
    console.log("[Noet] NavTree loaded:", navTree);

    // Build navigation UI
    buildNavigation();
  } catch (error) {
    console.error("[Noet] Failed to initialize WASM:", error);
    if (navContent) {
      navContent.innerHTML =
        '<p class="noet-placeholder" style="color: var(--noet-text-error);">Failed to load navigation. Check console for details.</p>';
    }
  }
}

// =============================================================================
// Navigation Tree Generation
// =============================================================================

/**
 * Build navigation tree from NavTree (flat map structure)
 * Uses intelligent expand/collapse based on active document
 */
function buildNavigation() {
  if (!navContent) {
    console.warn("[Noet] Nav content container not found");
    return;
  }

  console.log("[Noet] DEBUG: navTree =", navTree);
  console.log("[Noet] DEBUG: navTree.nodes =", navTree?.nodes);
  console.log("[Noet] DEBUG: navTree.roots =", navTree?.roots);

  if (!navTree || !navTree.nodes || !navTree.roots) {
    console.error("[Noet] Navigation data incomplete:", {
      hasNavTree: !!navTree,
      hasNodes: !!navTree?.nodes,
      hasRoots: !!navTree?.roots,
    });
    navContent.innerHTML = '<p class="noet-placeholder">Navigation data not loaded</p>';
    return;
  }

  const nodeCount = navTree.nodes.size;
  const rootCount = navTree.roots.length;
  console.log(`[Noet] Building navigation: ${nodeCount} nodes, ${rootCount} roots`);

  // Log first few nodes for debugging
  const firstFewNodes = Array.from(navTree.nodes.entries()).slice(0, 3);
  console.log("[Noet] Sample nodes:", firstFewNodes);

  // Auto-expand root nodes (networks) by default - only on first render
  if (isFirstNavRender && navTree.roots && navTree.roots.length > 0) {
    for (const rootBid of navTree.roots) {
      expandedNodes.add(rootBid);
    }
    isFirstNavRender = false;
  }

  // Determine active BID from current document
  const activeBid = getActiveBid();

  // Build parent chain for intelligent expand/collapse
  if (activeBid) {
    buildParentChain(activeBid);
    console.log(`[Noet] Active BID: ${activeBid}, expanded ${expandedNodes.size} ancestors`);
  }

  // Render tree to HTML
  const treeHtml = renderNavTree();
  console.log("[Noet] Generated HTML length:", treeHtml.length);
  console.log("[Noet] First 500 chars of HTML:", treeHtml.substring(0, 500));

  navContent.innerHTML = treeHtml;

  // Attach toggle event listeners
  attachNavToggleListeners();

  console.log("[Noet] Navigation tree built successfully");

  // Hide error state if navigation loaded successfully
  if (navError) {
    navError.hidden = true;
  }
}

/**
 * Get active BID from current page
 * Tries multiple strategies: URL path, data attribute, section mapping
 * @returns {string|null} Active BID or null if not found
 */
function getActiveBid() {
  // Strategy 1: Check body data-bid attribute
  if (document.body.dataset.bid) {
    return document.body.dataset.bid;
  }

  // Strategy 2: Check current path against NavTree paths
  const currentPath = window.location.pathname;
  const currentHash = window.location.hash;

  // Try exact match with hash first (section)
  if (currentHash) {
    const fullPath = currentPath + currentHash;
    for (const [bid, node] of Object.entries(navTree.nodes)) {
      if (node.path && node.path.includes(currentHash.substring(1))) {
        return bid;
      }
    }
  }

  // Try document path match
  for (const [bid, node] of Object.entries(navTree.nodes)) {
    if (node.path && currentPath.endsWith(node.path)) {
      return bid;
    }
  }

  // Strategy 3: Check for section BID mapping in page
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

/**
 * Build parent chain and populate expandedNodes set
 * Expands all ancestors of activeBid, collapses everything else
 * @param {string} activeBid - BID of active node
 */
function buildParentChain(activeBid) {
  expandedNodes.clear();

  let currentBid = activeBid;
  while (currentBid) {
    expandedNodes.add(currentBid);
    const node = navTree.nodes.get(currentBid);
    if (!node) break;
    currentBid = node.parent;
  }
}

/**
 * Toggle expand/collapse state for a node
 * @param {string} bid - BID of node to toggle
 */
function toggleNode(bid) {
  console.log(`[Noet] Toggling node: ${bid}, currently expanded: ${expandedNodes.has(bid)}`);
  if (expandedNodes.has(bid)) {
    expandedNodes.delete(bid);
  } else {
    expandedNodes.add(bid);
  }

  // Re-render navigation
  buildNavigation();
}

/**
 * Render navigation tree from NavTree flat map
 * @returns {string} HTML string
 */
function renderNavTree() {
  if (!navTree.roots || navTree.roots.length === 0) {
    console.error("[Noet] No roots to render");
    return '<p class="noet-placeholder">No networks found</p>';
  }

  console.log(`[Noet] Rendering ${navTree.roots.length} root nodes:`, navTree.roots);

  let html = '<ul class="noet-nav-tree">';

  for (const rootBid of navTree.roots) {
    console.log(`[Noet] Rendering root node: ${rootBid}`);
    const nodeHtml = renderNavNode(rootBid);
    console.log(`[Noet] Root node HTML length: ${nodeHtml.length}`);
    html += nodeHtml;
  }

  html += "</ul>";
  console.log(`[Noet] Total tree HTML length: ${html.length}`);
  return html;
}

/**
 * Render a single navigation node
 * @param {string} bid - BID of node to render
 * @returns {string} HTML string
 */
function renderNavNode(bid, depth = 0, visited = new Set()) {
  // Cycle detection: prevent infinite recursion
  if (visited.has(bid)) {
    console.error(`[Noet] Cycle detected: node ${bid} already visited in this chain`);
    return `<li class="noet-nav-tree__item noet-error">⚠ Cycle detected: ${escapeHtml(bid)}</li>`;
  }

  // Depth limit: prevent stack overflow
  if (depth > 50) {
    console.error(`[Noet] Max depth exceeded at node ${bid}`);
    return `<li class="noet-nav-tree__item noet-error">⚠ Max depth exceeded</li>`;
  }

  const node = navTree.nodes.get(bid);
  if (!node) {
    console.warn(`[Noet] Node not found for BID: ${bid}`);
    return "";
  }

  console.log(`[Noet] Rendering node: ${node.title} (${bid}) at depth ${depth}`);

  const hasChildren = node.children && node.children.length > 0;
  const isExpanded = expandedNodes.has(bid);
  const isActive = bid === getActiveBid();

  let itemClass = "noet-nav-tree__item";
  if (hasChildren) itemClass += " has-children";
  if (isExpanded) itemClass += " is-expanded";
  if (isActive) itemClass += " active";

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

  // Link (or span for networks with no path)
  if (node.path && node.path.length > 0) {
    html += `
      <a href="${escapeHtml(node.path)}"
         class="noet-nav-tree__link${isActive ? " active" : ""}"
         data-bid="${escapeHtml(bid)}">
        ${escapeHtml(node.title)}
      </a>
    `;
  } else {
    // Network node (no direct link)
    html += `
      <span class="noet-nav-tree__label">
        ${escapeHtml(node.title)}
      </span>
    `;
  }

  // Render children if expanded
  if (hasChildren && isExpanded) {
    // Add current node to visited set for cycle detection
    const newVisited = new Set(visited);
    newVisited.add(bid);

    html += '<ul class="noet-nav-tree__children">';
    for (const childBid of node.children) {
      // Skip if child is same as parent (self-reference)
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
 * Attach click event listeners to toggle buttons
 */
function attachNavToggleListeners() {
  const toggleButtons = navContent.querySelectorAll(".noet-nav-tree__toggle");
  console.log(`[Noet] Attaching listeners to ${toggleButtons.length} toggle buttons`);

  toggleButtons.forEach((button) => {
    button.addEventListener("click", (e) => {
      e.preventDefault();
      e.stopPropagation();

      const bid = button.dataset.bid;
      console.log(`[Noet] Toggle button clicked: ${bid}`);
      if (bid) {
        toggleNode(bid);
      }
    });
  });
}

// =============================================================================
// Panel Collapse Management
// =============================================================================

/**
 * Initialize panel collapse state from localStorage
 */
function initializePanelState() {
  const saved = localStorage.getItem("noet-panel-state");
  if (saved) {
    try {
      panelState = JSON.parse(saved);
    } catch (e) {
      console.warn("[Noet] Failed to parse saved panel state");
    }
  }

  // Apply saved state
  applyPanelState();
}

/**
 * Toggle navigation panel (desktop only)
 */
function toggleNavPanel() {
  panelState.navCollapsed = !panelState.navCollapsed;
  applyPanelState();
  savePanelState();
}

/**
 * Toggle metadata panel (desktop only)
 */
function toggleMetadataPanel() {
  panelState.metadataCollapsed = !panelState.metadataCollapsed;
  applyPanelState();
  savePanelState();
}

/**
 * Apply panel collapse state to DOM
 */
function applyPanelState() {
  if (!containerElement) return;

  // Apply nav collapse state
  if (panelState.navCollapsed) {
    containerElement.classList.add("nav-collapsed");
    if (navCollapseBtn) navCollapseBtn.textContent = "▶";
    if (navCollapseBtn) navCollapseBtn.setAttribute("aria-label", "Expand navigation panel");
  } else {
    containerElement.classList.remove("nav-collapsed");
    if (navCollapseBtn) navCollapseBtn.textContent = "◀";
    if (navCollapseBtn) navCollapseBtn.setAttribute("aria-label", "Collapse navigation panel");
  }

  // Apply metadata collapse state
  if (panelState.metadataCollapsed) {
    containerElement.classList.add("metadata-collapsed");
    if (metadataCollapseBtn) metadataCollapseBtn.textContent = "◀";
    if (metadataCollapseBtn)
      metadataCollapseBtn.setAttribute("aria-label", "Expand metadata panel");
  } else {
    containerElement.classList.remove("metadata-collapsed");
    if (metadataCollapseBtn) metadataCollapseBtn.textContent = "▶";
    if (metadataCollapseBtn)
      metadataCollapseBtn.setAttribute("aria-label", "Collapse metadata panel");
  }
}

/**
 * Save panel state to localStorage
 */
function savePanelState() {
  localStorage.setItem("noet-panel-state", JSON.stringify(panelState));
}

// =============================================================================
// Error State Management
// =============================================================================

/**
 * Show navigation error message
 */
function showNavError() {
  if (navError) {
    navError.hidden = false;
  }
  if (navContent) {
    navContent.innerHTML = "";
  }
}

/**
 * Show metadata error message
 */
function showMetadataError() {
  if (metadataError) {
    metadataError.hidden = false;
  }
}

/**
 * Handle keyboard shortcuts
 * @param {KeyboardEvent} e - Keyboard event
 */
function handleKeyboardShortcuts(e) {
  // Only on desktop (min-width: 1024px check via matchMedia)
  const isDesktop = window.matchMedia("(min-width: 1024px)").matches;
  if (!isDesktop) return;

  // Ctrl+\ (Ctrl+Backslash) - Toggle navigation panel
  if (e.ctrlKey && e.key === "\\") {
    e.preventDefault();
    toggleNavPanel();
  }

  // Ctrl+] (Ctrl+RightBracket) - Toggle metadata panel
  if (e.ctrlKey && e.key === "]") {
    e.preventDefault();
    toggleMetadataPanel();
  }
}

// =============================================================================
// Metadata Panel (Step 2 - Placeholder)
// =============================================================================

/**
 * Show metadata panel with node details
 * @param {string} nodeBid - Node BID to display metadata for
 * TODO (Step 2): Implement metadata display
 */
function showMetadataPanel(nodeBid) {
  if (!metadataPanel || !metadataContent) {
    return;
  }

  selectedNodeBid = nodeBid;

  // Placeholder - will be implemented in Step 2
  metadataContent.innerHTML = `
        <p><strong>Node:</strong> ${nodeBid}</p>
        <p class="noet-placeholder">Metadata display coming in Step 2</p>
    `;

  metadataPanel.hidden = false;
}

/**
 * Close metadata panel
 */
function closeMetadataPanel() {
  if (metadataPanel) {
    metadataPanel.hidden = true;
  }
  selectedNodeBid = null;
}

// =============================================================================
// Graph Visualization (Step 4 - Placeholder)
// =============================================================================

/**
 * Show graph visualization
 * TODO (Step 4): Implement force-directed graph
 */
function showGraphView() {
  if (!graphContainer) {
    return;
  }

  // Placeholder - will be implemented in Step 4
  graphCanvas.innerHTML = '<p class="noet-placeholder">Graph visualization coming in Step 4</p>';
  graphContainer.hidden = false;
}

/**
 * Close graph view
 */
function closeGraphView() {
  if (graphContainer) {
    graphContainer.hidden = true;
  }
}

// =============================================================================
// Query Builder (Step 3 - Placeholder)
// =============================================================================

/**
 * Initialize query builder UI
 * TODO (Step 3): Implement query builder
 */
function initializeQueryBuilder() {
  // Placeholder - will be implemented in Step 3
  console.log("[Noet] Query builder coming in Step 3");
}

// =============================================================================
// Utility Functions
// =============================================================================

/**
 * Escape HTML to prevent XSS
 * @param {string} text - Text to escape
 * @returns {string} Escaped HTML
 */
function escapeHtml(text) {
  if (text === null || text === undefined) {
    return "";
  }
  const div = document.createElement("div");
  div.textContent = String(text);
  return div.innerHTML;
}

/**
 * Format BID for display
 * @param {string} bid - BID string
 * @returns {string} Formatted BID
 */
function formatBid(bid) {
  // TODO: Add BID formatting logic
  return bid;
}

// =============================================================================
// Export for testing (if in module context)
// =============================================================================

if (typeof module !== "undefined" && module.exports) {
  module.exports = {
    handleThemeChange,
    applyTheme,
    showMetadataPanel,
    closeMetadataPanel,
    showGraphView,
    closeGraphView,
  };
}
