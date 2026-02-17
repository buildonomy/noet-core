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
 *
 * ⚠️ CRITICAL: WASM Data Type Patterns
 * =====================================
 *
 * Rust BTreeMap/HashMap serialize to JavaScript **Map objects**, NOT plain objects!
 *
 * WRONG (will fail):
 *   const data = beliefbase.get_something();
 *   Object.keys(data);        // ❌ Returns [] (empty!)
 *   data[key];                // ❌ Returns undefined
 *   Object.entries(data);     // ❌ Returns [] (empty!)
 *
 * CORRECT (use Map methods):
 *   const data = beliefbase.get_something();
 *   data.size;                // ✅ Number of entries
 *   data.get(key);            // ✅ Get value by key
 *   data.has(key);            // ✅ Check if key exists
 *   data.entries();           // ✅ Iterator of [key, value]
 *   Array.from(data.entries()); // ✅ Convert to array
 *
 * Current WASM Function Return Types:
 * - get_paths()          → Plain Object (uses serde_json) ✅ Use obj[key]
 * - get_nav_tree()       → NavTree { nodes: Map, roots: Array }
 *   - navTree.nodes      → Map ⚠️ Use .get(bid)
 *   - navTree.roots      → Array ✅ Use [index]
 * - get_context()        → NodeContext { related_nodes: Map, graph: Map, ... }
 *   - related_nodes      → Map ⚠️ Use .get(bid)
 *   - graph              → Map ⚠️ Use .get(weightKind)
 * - query()              → BeliefGraph { states: Map, relations: ... }
 *   - states             → Map ⚠️ Use .get(bid) (not currently used)
 *
 * When adding new WASM calls:
 * 1. Check src/wasm.rs documentation for return type
 * 2. If function returns BTreeMap/HashMap → expect JavaScript Map
 * 3. Use Map methods (.get, .size, .entries, .has)
 * 4. Never use Object.keys() or bracket notation on Maps
 *
 * See src/wasm.rs header for Rust-side serialization patterns.
 */

// =============================================================================
// DOM References
// =============================================================================

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

/** Entry point metadata (Network node that roots this beliefbase) */
let entryPoint = null;

/** Navigation tree data (flat map structure from get_nav_tree()) */
let navTree = null;

/** Set of expanded node BIDs for navigation tree */
let expandedNodes = new Set();

/** Track first navigation render to auto-expand roots only once */
let isFirstNavRender = true;

/** Pending metadata BID to show after navigation completes */
let pendingMetadataBid = null;

/** Panel collapse state (persisted to localStorage) */
let panelState = {
  navCollapsed: false,
  metadataCollapsed: true, // Start collapsed
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

  // Handle initial document load based on hash
  const initialHash = window.location.hash;
  if (initialHash && initialHash !== "#") {
    // Hash present: Load that document
    await handleHashChange();
  } else {
    // No hash: Load default document (pages/index.html)
    await loadDefaultDocument();
  }

  console.log("[Noet] Viewer initialized successfully");
});

/**
 * Cache all DOM element references
 */
function initializeDOMReferences() {
  // Container elements
  containerElement = document.querySelector(".noet-container");
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

  // Content link click handlers (two-click pattern)
  if (contentElement) {
    contentElement.addEventListener("click", handleContentClick);
  }

  // Hash change handler for client-side navigation
  window.addEventListener("hashchange", handleHashChange);

  // Click outside content to reset selected link
  document.addEventListener("click", handleDocumentClick);
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
    event.preventDefault();
    const href = target.getAttribute("href");
    const targetBid = target.getAttribute("data-bid");
    if (href) {
      console.log("[Noet] Navigating to:", href);
      navigateToLink(href, target, targetBid);
    }
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
 * Handle clicks in content area (two-click navigation pattern)
 * @param {Event} e - Click event
 */
function handleContentClick(e) {
  // Find closest <a> element (handles clicks on child elements)
  const link = e.target.closest("a");
  if (!link) {
    return; // Not a link, ignore
  }

  // Check if link is within .noet-content
  if (!link.closest(".noet-content")) {
    return; // Link is in nav/metadata/footer, ignore
  }

  e.preventDefault();

  // Extract BID from title attribute (format: "bref://[bref]")
  const linkBid = extractBidFromLink(link);
  const href = link.getAttribute("href");

  if (!linkBid && !href) {
    console.warn("[Noet] Link has no BID or href, ignoring");
    return;
  }

  // Two-click logic
  if (selectedNodeBid === linkBid) {
    // Second click: Navigate
    if (href) {
      navigateToLink(href, link, linkBid);
    }
    selectedNodeBid = null; // Reset for next interaction
    clearSelectedLinkHighlight();
  } else {
    // First click: Show metadata panel
    if (linkBid && beliefbase) {
      showMetadataPanel(linkBid);
      selectedNodeBid = linkBid; // Track for potential second click
      highlightSelectedLink(link);
    } else if (href) {
      // No BID available, just navigate
      navigateToLink(href, link, null);
    }
  }
}

/**
 * Extract BID from link's title attribute
 * @param {HTMLElement} link - Link element
 * @returns {string|null} BID or null
 */
function extractBidFromLink(link) {
  const title = link.getAttribute("title");
  if (!title) {
    return null;
  }

  // Format: "bref://[bref]" - extract bref and resolve to BID
  const match = title.match(/^bref:\/\/(.+?)(?:\s|$)/);
  if (!match) {
    return null;
  }

  const bref = match[1];

  // Use WASM to resolve bref to BID
  if (!beliefbase) {
    console.warn("[Noet] Cannot resolve bref - BeliefBase not initialized");
    return null;
  }

  const bid = beliefbase.get_bid_from_bref(bref);
  return bid;
}

/**
 * Navigate to link target (document or section)
 * @param {string} href - Link href attribute
 * @param {HTMLElement} link - Link element (for context)
 * @param {string|null} targetBid - Optional BID of target node to show metadata
 */
function navigateToLink(href, link, targetBid = null) {
  // Check if it's an external link
  if (href.startsWith("http://") || href.startsWith("https://")) {
    // External link: Do nothing on first click
    console.log("[Noet] External link - click again to show metadata");
    return;
  }

  // Check if it's a section/anchor link within current document (starts with #)
  if (href.startsWith("#")) {
    navigateToSection(href, targetBid);
    return;
  }

  // Check if it's an asset (non-document file) - open directly
  // Check for common document extensions (fallback if normalize not available)
  const hrefWithoutAnchor = href.split("#")[0];
  const isDocument =
    hrefWithoutAnchor.includes(".html") ||
    hrefWithoutAnchor.includes(".md") ||
    hrefWithoutAnchor.includes(".org") ||
    hrefWithoutAnchor.includes(".rst");

  if (!isDocument) {
    // Asset link (PDF, image, etc.) - resolve path and open directly
    let assetPath = href;

    // Resolve relative asset paths
    if (!href.startsWith("/") && wasmModule) {
      const currentHash = window.location.hash.substring(1);
      if (currentHash) {
        const parts = wasmModule.BeliefBaseWasm.pathParts(currentHash);
        const parentDir = parts.path;
        assetPath = wasmModule.BeliefBaseWasm.pathJoin(parentDir, href, false);
      }
    }

    // Open asset in new tab or download
    console.log(`[Noet] Opening asset: ${assetPath}`);
    window.open(`/pages/${assetPath}`, "_blank");
    return;
  }

  // Resolve relative paths against current document location
  let resolvedPath = href;
  if (!href.startsWith("/") && wasmModule) {
    // Get current document path from hash (e.g., "/net1_dir1/hsml.html")
    const currentHash = window.location.hash.substring(1); // Remove leading #

    if (currentHash) {
      // Get parent directory of current document using pathParts
      const parts = wasmModule.BeliefBaseWasm.pathParts(currentHash);
      const parentDir = parts.path;
      // Join with the relative href
      resolvedPath = wasmModule.BeliefBaseWasm.pathJoin(parentDir, href, false);
      console.log(
        `[Noet] Resolved relative path: ${href} -> ${resolvedPath} (from ${currentHash})`,
      );
    }
  }

  // Check if it's a document link with anchor (e.g., "path/file.html#section")
  const hashIndex = resolvedPath.indexOf("#");
  if (hashIndex > 0) {
    // Split into document path and anchor
    const docPath = resolvedPath.substring(0, hashIndex);
    const anchor = resolvedPath.substring(hashIndex + 1);
    // Navigate with full path in hash: #/path/file.html#anchor
    // Ensure docPath starts with / (don't double it)
    const hashPath = docPath.startsWith("/") ? `${docPath}#${anchor}` : `/${docPath}#${anchor}`;
    window.location.hash = hashPath;
    // Store targetBid for use after navigation completes
    if (targetBid) {
      pendingMetadataBid = targetBid;
    }
    return;
  }

  // Internal document link without anchor: Navigate via hash routing
  navigateToDocument(resolvedPath, targetBid);
}

/**
 * Navigate to a document via hash routing
 * @param {string} path - Document path (e.g., "/file1.html")
 * @param {string|null} targetBid - Optional BID of target node to show metadata
 */
function navigateToDocument(path, targetBid = null) {
  // Store targetBid for use after navigation completes
  if (targetBid) {
    pendingMetadataBid = targetBid;
  }
  // Update hash to trigger navigation
  window.location.hash = path;
}

/**
 * Navigate to a section within current document
 * @param {string} anchor - Section anchor (e.g., "#section-id")
 * @param {string|null} targetBid - Optional BID of target node to show metadata
 */
function navigateToSection(anchor, targetBid = null) {
  const sectionId = anchor.substring(1); // Remove leading #
  const targetElement = document.getElementById(sectionId);

  if (targetElement) {
    targetElement.scrollIntoView({ behavior: "smooth", block: "start" });

    // Preserve current document path in hash when navigating to section
    const currentHash = window.location.hash.substring(1); // Remove leading #
    let newHash = anchor;

    if (currentHash && wasmModule) {
      // Use PathParts to properly parse the current path
      const parts = wasmModule.BeliefBaseWasm.pathParts(currentHash);

      // If we have a filename, reconstruct with document path + new anchor
      if (parts.filename) {
        const docPath = parts.path ? `${parts.path}/${parts.filename}` : parts.filename;
        newHash = `#${docPath}${anchor}`;
      }
    }

    // Update URL hash without triggering hashchange
    // Ensure hash starts with # for proper URL construction
    if (!newHash.startsWith("#")) {
      newHash = "#" + newHash;
    }
    window.location.hash = newHash;

    // Show metadata for section if BID provided
    if (targetBid) {
      showMetadataPanel(targetBid);
    }
  } else {
    console.warn(`[Noet] Section not found: ${sectionId}`);
  }
}

/**
 * Handle hash change events (client-side navigation)
 */
async function handleHashChange() {
  const hash = window.location.hash;

  // Reset selected link on navigation
  selectedNodeBid = null;
  clearSelectedLinkHighlight();

  if (!hash || hash === "#") {
    // No hash: Load default document (network root)
    await loadDefaultDocument();
    return;
  }

  // Remove leading # and check if it's a section or document
  let path = hash.substring(1);

  if (path.startsWith("#")) {
    // Double hash (shouldn't happen, but handle gracefully)
    return;
  }

  // Check if path contains a section anchor (e.g., /file.html#section-id)
  let sectionAnchor = null;
  const anchorIndex = path.indexOf("#");
  if (anchorIndex > 0) {
    sectionAnchor = path.substring(anchorIndex);
    path = path.substring(0, anchorIndex);
  }

  // Normalize path to resolve any .. or . segments
  // Note: normalizePath now preserves leading slashes
  if (wasmModule) {
    path = wasmModule.BeliefBaseWasm.normalizePath(path);
  }

  // Normalize path extension (.md -> .html, etc.) for document check
  let normalizedPath = path;
  if (
    wasmModule &&
    wasmModule.BeliefBaseWasm &&
    wasmModule.BeliefBaseWasm.normalize_path_extension
  ) {
    normalizedPath = wasmModule.BeliefBaseWasm.normalize_path_extension(path);
  }

  // If normalized path doesn't contain .html, treat as section anchor in current doc
  if (!normalizedPath.includes(".html")) {
    navigateToSection("#" + path, pendingMetadataBid);
    pendingMetadataBid = null;
    return;
  }

  // Document path: Fetch and display
  await loadDocument(path, sectionAnchor, pendingMetadataBid);
  pendingMetadataBid = null;
}

/**
 * Load default document (network root index)
 */
async function loadDefaultDocument() {
  await loadDocument("/index.html");
}

/**
 * Load a document from /pages/ directory
 * @param {string} path - Document path (e.g., "/file1.html" or "file1.html")
 * @param {string|null} sectionAnchor - Optional section anchor to scroll to after load
 */
async function loadDocument(path, sectionAnchor = null, targetBid = null) {
  if (!contentElement) {
    console.error("[Noet] Content element not found");
    return;
  }

  try {
    // Normalize path extension (.md -> .html, etc.)
    let normalizedPath = path;
    if (
      wasmModule &&
      wasmModule.BeliefBaseWasm &&
      wasmModule.BeliefBaseWasm.normalize_path_extension
    ) {
      normalizedPath = wasmModule.BeliefBaseWasm.normalize_path_extension(path);
      console.log(`[Noet] Normalized path: ${path} -> ${normalizedPath}`);
    } else {
      // Fallback: simple .md to .html conversion
      normalizedPath = path.replace(/\.md(#|$)/, ".html$1");
    }

    // Ensure path starts with /
    normalizedPath = normalizedPath.startsWith("/") ? normalizedPath : "/" + normalizedPath;

    // Fetch from /pages/ directory
    const fetchPath = `/pages${normalizedPath}`;
    console.log(`[Noet] Fetching document: ${fetchPath}`);

    const response = await fetch(fetchPath);

    if (!response.ok) {
      throw new Error(`HTTP ${response.status}: ${response.statusText}`);
    }

    const html = await response.text();

    // Parse HTML and extract article content (excludes nav and other non-content elements)
    const parser = new DOMParser();
    const doc = parser.parseFromString(html, "text/html");
    const articleElement = doc.querySelector("article");
    const bodyContent = articleElement ? articleElement.innerHTML : doc.body.innerHTML;

    if (!bodyContent) {
      throw new Error("No content found in fetched document");
    }

    // Extract and update document metadata
    const metadataScript = doc.querySelector('script[type="application/json"]#noet-metadata');
    if (metadataScript) {
      try {
        const metadata = JSON.parse(metadataScript.textContent);
        documentMetadata = metadata;
        console.log(`[Noet] Updated document metadata:`, metadata);
      } catch (e) {
        console.warn("[Noet] Failed to parse document metadata:", e);
      }
    }

    // Update page title
    const titleElement = doc.querySelector("title");
    if (titleElement) {
      document.title = titleElement.textContent;
    }

    // Replace content (find the inner article/content container)
    const contentInner = contentElement.querySelector(".noet-content__inner");
    if (contentInner) {
      contentInner.innerHTML = `<article>${bodyContent}</article>`;
    } else {
      contentElement.innerHTML = bodyContent;
    }

    console.log(`[Noet] Document loaded: ${path}`);

    // Update navigation tree highlighting
    updateNavTreeHighlight();

    // Scroll to section if anchor provided, otherwise scroll to top
    if (sectionAnchor) {
      setTimeout(() => {
        navigateToSection(sectionAnchor, targetBid);
      }, 100); // Brief delay to ensure content is rendered
    } else {
      contentElement.scrollTo({ top: 0, behavior: "smooth" });
      // Show metadata for document if BID provided, or try to look it up
      const bidToShow = targetBid || getBidFromPath(path);
      if (bidToShow) {
        showMetadataPanel(bidToShow);
      }
    }
  } catch (error) {
    console.error(`[Noet] Failed to load document: ${path}`, error);

    const contentInner = contentElement.querySelector(".noet-content__inner");
    const target = contentInner || contentElement;

    target.innerHTML = `
      <article>
        <div class="noet-error">
          <h3 class="noet-error__title">Document Not Found</h3>
          <p class="noet-error__message">Failed to load: ${escapeHtml(path)}</p>
          <p class="noet-error__details">${escapeHtml(error.message)}</p>
          <button class="noet-error__action" onclick="window.location.hash = ''">
            Back to Home
          </button>
        </div>
      </article>
    `;
  }
}

/**
 * Lookup BID from path using beliefbase paths
 * @param {string} path - Document path (e.g., "net1_dir1/hsml.html" or "net1_dir1/hsml.html#section")
 * @returns {string|null} BID if found, null otherwise
 */
function getBidFromPath(path) {
  if (!beliefbase || !entryPoint) {
    return null;
  }

  try {
    // Get the paths map for the entry point network
    const paths = beliefbase.get_paths();
    const pathsMap = paths[entryPoint.bid];

    if (!pathsMap) {
      console.warn("[Noet] No paths found for entry point:", entryPoint.bid);
      return null;
    }

    // Strip any section anchors and leading slash from the path
    // PathMap keys don't include leading slashes
    let cleanPath = path.split("#")[0];
    if (cleanPath.startsWith("/")) {
      cleanPath = cleanPath.substring(1);
    }

    // Try to find the path in the map
    const bid = pathsMap[cleanPath];

    if (bid) {
      console.log(`[Noet] Found BID for path ${cleanPath}:`, bid);
      return bid;
    } else {
      console.log(`[Noet] No BID found for path: ${cleanPath}`);
      return null;
    }
  } catch (error) {
    console.error("[Noet] Error looking up BID from path:", error);
    return null;
  }
}

/**
 * Handle clicks outside content area (reset selected link)
 * @param {Event} e - Click event
 */
function handleDocumentClick(e) {
  // Check if click is outside .noet-content
  if (!e.target.closest(".noet-content")) {
    selectedNodeBid = null;
    clearSelectedLinkHighlight();
  }
}

/**
 * Highlight selected link for two-click pattern
 * @param {HTMLElement} link - Link element to highlight
 */
function highlightSelectedLink(link) {
  clearSelectedLinkHighlight();
  link.classList.add("noet-link-selected");
}

/**
 * Clear selected link highlight
 */
function clearSelectedLinkHighlight() {
  const selected = document.querySelector(".noet-link-selected");
  if (selected) {
    selected.classList.remove("noet-link-selected");
  }
}

/**
 * Update navigation tree to highlight active document
 */
function updateNavTreeHighlight() {
  if (!navContent) {
    return;
  }

  // Get current active BID from document metadata
  const activeBid = getActiveBid();
  if (!activeBid) {
    return;
  }

  // Remove all active classes
  const allLinks = navContent.querySelectorAll(".noet-nav-link");
  allLinks.forEach((link) => {
    link.classList.remove("active");
  });

  // Add active class to current document's link
  const activeLink = navContent.querySelector(`.noet-nav-link[data-bid="${activeBid}"]`);
  if (activeLink) {
    activeLink.classList.add("active");

    // Ensure parent nodes are expanded
    let parent = activeLink.closest("li");
    while (parent) {
      const toggle = parent.querySelector(".noet-nav-toggle");
      if (toggle && !expandedNodes.has(toggle.dataset.bid)) {
        expandedNodes.add(toggle.dataset.bid);
      }
      parent = parent.parentElement?.closest("li");
    }
  }
}

/**
 * Initialize theme based on saved preference or system preference
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
    wasmModule = await import("/assets/noet_core.js");
    await wasmModule.default();

    console.log("[Noet] WASM module loaded successfully");

    // Load beliefbase.json
    console.log("[Noet] Loading beliefbase.json...");
    const response = await fetch("/beliefbase.json");
    if (!response.ok) {
      throw new Error(`Failed to fetch beliefbase.json: ${response.status}`);
    }

    const beliefbaseJson = await response.text();
    console.log("[Noet] BeliefBase JSON loaded successfully");

    // Get metadata JSON from script tag
    const metadataScript = document.getElementById("noet-metadata");
    if (!metadataScript) {
      throw new Error("Missing #noet-metadata script tag");
    }
    const metadataJson = metadataScript.textContent;

    // Parse and store entry point metadata
    try {
      entryPoint = JSON.parse(metadataJson);
      console.log("[Noet] Entry point parsed:", entryPoint);

      if (!entryPoint.bid) {
        throw new Error("Entry point missing 'bid' field");
      }
    } catch (e) {
      throw new Error(`Failed to parse entry point metadata: ${e.message}`);
    }

    // Initialize BeliefBaseWasm (from_json is a constructor in WASM bindings)
    // Pass both beliefbase JSON and metadata JSON (for entry point Bid)
    beliefbase = new wasmModule.BeliefBaseWasm(beliefbaseJson, metadataJson);
    console.log("[Noet] BeliefBaseWasm initialized");

    // Validate entry point exists in beliefbase
    console.log("[Noet] Validating entry point...");

    // Check 1: Entry point node exists in states
    const entryPointNode = beliefbase.get_by_bid(entryPoint.bid);
    if (!entryPointNode) {
      console.error("[Noet] ❌ Entry point BID not found in beliefbase.states:", entryPoint.bid);
      throw new Error(`Entry point node ${entryPoint.bid} not found in beliefbase`);
    }
    console.log("[Noet] ✓ Entry point node exists:", entryPointNode.title);

    // Check 2: Entry point has a network path map
    const paths = beliefbase.get_paths();
    if (!paths[entryPoint.bid]) {
      console.warn("[Noet] ⚠️ Entry point has no path map (expected for Network nodes)");
      console.log("[Noet] Available path maps:", Object.keys(paths));
    } else {
      console.log("[Noet] ✓ Entry point has path map with", paths[entryPoint.bid].length, "paths");
    }

    // Check 3: Validate node count and relation count
    const nodeCount = beliefbase.node_count();
    console.log("[Noet] ✓ BeliefBase loaded:", nodeCount, "nodes");

    console.log("[Noet] BeliefBase validation complete");

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
    // Ensure absolute path for hash routing
    const absolutePath = node.path.startsWith("/") ? node.path : `/${node.path}`;
    html += `
      <a href="${escapeHtml(absolutePath)}"
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
  if (!metadataPanel) return;

  // Toggle collapse state
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
    if (metadataCollapseBtn) metadataCollapseBtn.setAttribute("aria-label", "Show metadata panel");
  } else {
    containerElement.classList.remove("metadata-collapsed");
    if (metadataCollapseBtn) metadataCollapseBtn.textContent = "▶";
    if (metadataCollapseBtn) metadataCollapseBtn.setAttribute("aria-label", "Hide metadata panel");
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
 */
function showMetadataPanel(nodeBid) {
  if (!metadataPanel || !metadataContent || !beliefbase) {
    console.warn("[Noet] Cannot show metadata: missing panel or beliefbase");
    return;
  }

  selectedNodeBid = nodeBid;

  try {
    // Get node context from WASM
    const context = beliefbase.get_context(nodeBid);

    if (!context) {
      showMetadataError();
      console.warn(`[Noet] No context found for BID: ${nodeBid}`);
      return;
    }

    // Hide error, show content
    if (metadataError) {
      metadataError.hidden = true;
    }

    // Render metadata content
    metadataContent.innerHTML = renderNodeContext(context);

    // Ensure panel is expanded
    panelState.metadataCollapsed = false;
    applyPanelState();

    // Attach event handlers to links
    updateMetadataPanel();
  } catch (error) {
    console.error("[Noet] Failed to load metadata:", error);
    showMetadataError();
  }
}

/**
 * Render NodeContext as HTML
 * @param {Object} context - NodeContext from WASM
 * @returns {string} HTML string
 */
function renderNodeContext(context) {
  const { node, root_path, home_net, related_nodes, graph } = context;

  let html = '<div class="noet-metadata-section">';

  // Node Information
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

  // Payload (if present)
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

  // Graph Relations (organized by WeightKind)
  if (graph && graph.size > 0) {
    html += '<div class="noet-metadata-section">';
    html += "<h3>Relations</h3>";

    for (const [weightKind, [sources, sinks]] of graph.entries()) {
      if (sources.length > 0 || sinks.length > 0) {
        html += `<h4>${escapeHtml(weightKind)}</h4>`;

        // Render sources (incoming links)
        if (sources.length > 0) {
          html += '<div class="noet-relation-group">';
          html += '<p class="noet-metadata-label"><strong>Sources (incoming):</strong></p>';
          html += '<ul class="noet-relation-list">';

          for (const sourceBid of sources) {
            const sourceNode = related_nodes.get(sourceBid);
            if (sourceNode) {
              const sourceTitle = escapeHtml(sourceNode.node.title || sourceBid);
              const sourcePath = sourceNode.root_path || null;

              if (sourcePath) {
                // Ensure absolute path for hash routing
                const absolutePath = sourcePath.startsWith("/") ? sourcePath : `/${sourcePath}`;
                html += `<li><a href="${absolutePath}" class="noet-metadata-link" data-bid="${sourceBid}">${sourceTitle}</a></li>`;
              } else {
                html += `<li><span title="BID: ${sourceBid}">${sourceTitle}</span></li>`;
              }
            } else {
              html += `<li><span title="BID: ${sourceBid}">${formatBid(sourceBid)}</span></li>`;
            }
          }

          html += "</ul>";
          html += "</div>";
        }

        // Render sinks (outgoing links)
        if (sinks.length > 0) {
          html += '<div class="noet-relation-group">';
          html += '<p class="noet-metadata-label"><strong>Sinks (outgoing):</strong></p>';
          html += '<ul class="noet-relation-list">';

          for (const sinkBid of sinks) {
            const sinkNode = related_nodes.get(sinkBid);
            if (sinkNode) {
              const sinkTitle = escapeHtml(sinkNode.node.title || sinkBid);
              const sinkPath = sinkNode.root_path || null;

              if (sinkPath) {
                // Ensure absolute path for hash routing
                const absolutePath = sinkPath.startsWith("/") ? sinkPath : `/${sinkPath}`;
                html += `<li><a href="${absolutePath}" class="noet-metadata-link" data-bid="${sinkBid}">${sinkTitle}</a></li>`;
              } else {
                html += `<li><span title="BID: ${sinkBid}">${sinkTitle}</span></li>`;
              }
            } else {
              html += `<li><span title="BID: ${sinkBid}">${formatBid(sinkBid)}</span></li>`;
            }
          }

          html += "</ul>";
          html += "</div>";
        }
      }
    }

    html += "</div>";
  }

  return html;
}

/**
 * Close metadata panel
 */
function closeMetadataPanel() {
  // Collapse the panel
  panelState.metadataCollapsed = true;
  applyPanelState();
  savePanelState();
  selectedNodeBid = null;
}

/**
 * Update metadata panel content after rendering
 * Call this after innerHTML updates to attach event handlers
 */
function updateMetadataPanel() {
  attachMetadataLinkHandlers();
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
 * Format BID for display (show first 8 and last 4 chars)
 * @param {string} bid - BID string
 * @returns {string} Formatted BID
 */
function formatBid(bid) {
  if (!bid || typeof bid !== "string") {
    return "";
  }
  if (bid.length <= 13) {
    return bid;
  }
  return `${bid.substring(0, 8)}...${bid.substring(bid.length - 4)}`;
}

/**
 * Attach click handlers to node links in metadata panel
 * Enables two-click navigation from metadata panel links
 */
function attachMetadataLinkHandlers() {
  if (!metadataContent) {
    return;
  }

  // Handle both .noet-node-link and .noet-metadata-link classes
  const metadataLinks = metadataContent.querySelectorAll(".noet-node-link, .noet-metadata-link");
  metadataLinks.forEach((link) => {
    link.addEventListener("click", (e) => {
      e.preventDefault();
      // Use navigateToLink for consistent path handling (like content links)
      const href = link.getAttribute("href");
      const targetBid = link.getAttribute("data-bid");
      if (href) {
        console.log("[Noet] Navigating to related node:", href);
        navigateToLink(href, link, targetBid);
      }
    });
  });
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
