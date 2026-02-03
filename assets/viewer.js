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
let contentElement;
/** @type {HTMLElement} */
let metadataPanel;
/** @type {HTMLElement} */
let metadataContent;
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

/** @type {HTMLLinkElement} */
let themeLightLink;
/** @type {HTMLLinkElement} */
let themeDarkLink;

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

/** Navigation tree data (network_bid -> paths) */
let pathsData = null;

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

  // Load WASM and BeliefBase (non-blocking - theme should work even if this fails)
  try {
    await initializeWasm();
  } catch (error) {
    console.error(
      "[Noet] WASM initialization failed (theme and basic features still work):",
      error,
    );
  }

  console.log("[Noet] Viewer initialized successfully");
});

/**
 * Cache all DOM element references
 */
function initializeDOMReferences() {
  // Container elements
  headerElement = document.querySelector(".noet-header");
  navElement = document.querySelector(".noet-nav");
  navContent = document.getElementById("nav-content");
  contentElement = document.querySelector(".noet-content");
  metadataPanel = document.getElementById("metadata-panel");
  metadataContent = document.getElementById("metadata-content");
  graphContainer = document.getElementById("graph-container");
  graphCanvas = document.getElementById("graph-canvas");
  footerElement = document.querySelector(".noet-footer");

  // Interactive elements
  themeSelect = document.getElementById("theme-select");
  metadataClose = document.getElementById("metadata-close");
  graphClose = document.getElementById("graph-close");

  // Theme stylesheets
  themeLightLink = document.getElementById("theme-light");
  themeDarkLink = document.getElementById("theme-dark");

  // Verify critical elements exist
  if (!navContent || !metadataPanel || !metadataContent) {
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

    // Get paths data for navigation
    pathsData = beliefbase.get_paths();
    console.log("[Noet] Paths data loaded:", pathsData);

    // Build navigation tree
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
 * Build navigation tree from paths data
 */
function buildNavigation() {
  if (!pathsData || !navContent) {
    console.warn("[Noet] Cannot build navigation: missing data or container");
    return;
  }

  console.log("[Noet] Building navigation tree...");

  // Find the main network (usually the first non-system network)
  const systemNamespaces = [
    wasmModule.BeliefBaseWasm.href_namespace(),
    wasmModule.BeliefBaseWasm.asset_namespace(),
    wasmModule.BeliefBaseWasm.buildonomy_namespace(),
  ];

  let mainNetwork = null;
  let mainNetworkBid = null;

  for (const [netBid, paths] of Array.from(pathsData)) {
    if (!systemNamespaces.includes(netBid) && paths.length > 0) {
      mainNetwork = paths;
      mainNetworkBid = netBid;
      break;
    }
  }

  if (!mainNetwork) {
    navContent.innerHTML = '<p class="noet-placeholder">No documents found in network</p>';
    return;
  }

  console.log(`[Noet] Using network ${mainNetworkBid} with ${mainNetwork.length} paths`);

  // Build hierarchical tree structure
  const tree = buildTreeStructure(mainNetwork);

  // Render tree to HTML
  const treeHtml = renderTree(tree);
  navContent.innerHTML = treeHtml;

  console.log("[Noet] Navigation tree built successfully");
}

/**
 * Build hierarchical tree structure from flat paths array
 * @param {Array} paths - Array of [path, bid, order_indices] tuples
 * @returns {Array} Tree structure with nested children
 */
function buildTreeStructure(paths) {
  const tree = [];
  const pathMap = new Map();

  // Sort paths by order_indices (documents before sections, then by sort key)
  const sortedPaths = [...paths].sort((a, b) => {
    const [, , orderA] = a;
    const [, , orderB] = b;

    // Compare order indices lexicographically
    for (let i = 0; i < Math.max(orderA.length, orderB.length); i++) {
      const valA = orderA[i] || 0;
      const valB = orderB[i] || 0;
      if (valA !== valB) {
        return valA - valB;
      }
    }
    return 0;
  });

  for (const [path, bid, orderIndices] of sortedPaths) {
    const node = {
      path,
      bid,
      orderIndices,
      children: [],
      isSection: path.includes("#"),
      title: extractTitle(path),
      href: path.replace(/\.md(#|$)/, ".html$1"), // Convert .md to .html
    };

    pathMap.set(path, node);

    // Determine parent based on path structure
    if (node.isSection) {
      // Section: parent is the document
      const docPath = path.split("#")[0];
      const parent = pathMap.get(docPath);
      if (parent) {
        parent.children.push(node);
      } else {
        tree.push(node);
      }
    } else {
      // Document: top-level
      tree.push(node);
    }
  }

  return tree;
}

/**
 * Extract display title from path
 * @param {string} path - File path or path#anchor
 * @returns {string} Display title
 */
function extractTitle(path) {
  if (path.includes("#")) {
    // Section: use anchor as title (will be cleaned up)
    const anchor = path.split("#")[1];
    return anchor.replace(/-/g, " ").replace(/\b\w/g, (c) => c.toUpperCase());
  } else {
    // Document: use filename without extension
    const filename = path.split("/").pop();
    return filename.replace(/\.md$/, "").replace(/_/g, " ");
  }
}

/**
 * Render tree structure to HTML
 * @param {Array} nodes - Tree nodes to render
 * @param {number} level - Nesting level (for indentation)
 * @returns {string} HTML string
 */
function renderTree(nodes, level = 0) {
  if (!nodes || nodes.length === 0) {
    return "";
  }

  let html = '<ul class="noet-nav-tree">';

  for (const node of nodes) {
    const hasChildren = node.children && node.children.length > 0;
    const itemClass = hasChildren ? "noet-nav-tree__item has-children" : "noet-nav-tree__item";

    html += `<li class="${itemClass}">`;

    if (hasChildren) {
      // Collapsible parent
      html += `
                <button class="noet-nav-tree__toggle" aria-label="Toggle ${escapeHtml(node.title)}">
                    ›
                </button>
            `;
    }

    // Link
    html += `
            <a href="${escapeHtml(node.href)}"
               class="noet-nav-tree__link"
               data-bid="${escapeHtml(node.bid)}"
               data-path="${escapeHtml(node.path)}">
                ${escapeHtml(node.title)}
            </a>
        `;

    // Render children recursively
    if (hasChildren) {
      html += '<div class="noet-nav-tree__children">';
      html += renderTree(node.children, level + 1);
      html += "</div>";
    }

    html += "</li>";
  }

  html += "</ul>";
  return html;
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
