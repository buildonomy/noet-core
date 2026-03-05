/**
 * Noet Viewer — Entry point
 *
 * This file is intentionally thin. All logic lives in ./viewer/ modules:
 *
 *   viewer/state.js      — Shared mutable state object and DOM references
 *   viewer/utils.js      — Pure helpers: escapeHtml, formatBid
 *   viewer/theme.js      — Theme switching (light / dark / system)
 *   viewer/panels.js     — Panel collapse/expand, keyboard shortcuts, error display
 *   viewer/navigation.js — Nav tree build, render, toggle
 *   viewer/content.js    — processLoadedContent, image modal, link highlighting
 *   viewer/metadata.js   — Metadata panel: showMetadataPanel, renderNodeContext
 *   viewer/routing.js    — Hash routing, loadDocument, navigateToLink
 *   viewer/resize.js     — Draggable resize handles for nav, metadata, and content panels
 *   viewer/wasm.js       — WASM init, getBidFromPath
 *
 * ⚠️  WASM Data Type Patterns
 * ===========================
 * Rust BTreeMap/HashMap serialize to JavaScript **Map objects**, NOT plain objects.
 *
 *   WRONG:  Object.keys(data)       // ❌ always []
 *           data[key]               // ❌ undefined
 *   RIGHT:  data.get(key)           // ✅
 *           data.size               // ✅
 *           data.entries()          // ✅ iterator of [key, value]
 *
 * Exception: get_paths() returns a plain Object (uses serde_json).
 *   RIGHT:  paths[bid]              // ✅
 *
 * See src/wasm.rs header for full Rust-side serialization patterns.
 */

import { state, callbacks } from "./viewer/state.js";
import { initializeTheme, handleThemeChange } from "./viewer/theme.js";
import {
  initializePanelState,
  toggleNavPanel,
  toggleMetadataPanel,
  handleKeyboardShortcuts,
  showNavError,
} from "./viewer/panels.js";
import { updateNavTreeHighlight } from "./viewer/navigation.js";
import {
  clearSelectedLinkHighlight,
  highlightSelectedLink,
  highlightExternalInContent,
} from "./viewer/content.js";
import { showMetadataPanel, closeMetadataPanel } from "./viewer/metadata.js";
import {
  handleHashChange,
  loadDefaultDocument,
  navigateToLink,
  navigateToSection,
} from "./viewer/routing.js";
import { initializeWasm } from "./viewer/wasm.js";
import { initResizeHandles } from "./viewer/resize.js";

// =============================================================================
// Bootstrap
// =============================================================================

document.addEventListener("DOMContentLoaded", async () => {
  console.log("[Noet] Initializing viewer...");

  // 1. Cache DOM references
  initializeDOMReferences();

  // 2. Load document metadata from embedded JSON script tag
  loadMetadata();

  // 3. Wire up cross-module callbacks (breaks routing↔metadata↔navigation cycles)
  callbacks.showMetadataPanel = showMetadataPanel;
  callbacks.updateNavTreeHighlight = updateNavTreeHighlight;
  callbacks.navigateToLink = navigateToLink;
  callbacks.highlightExternalInContent = highlightExternalInContent;

  // 4. Attach DOM event listeners
  setupEventListeners();

  // 5. Theme, panel state, and resize handles (work without WASM)
  initializeTheme();
  initializePanelState();
  initResizeHandles();

  // 6. Load WASM and BeliefBase (non-blocking — theme/basic features still work if this fails)
  try {
    await initializeWasm();
    // Expose BeliefBaseWasm on window.noet for browser console use.
    // Usage: noet.set_log_level('debug')
    //        noet.href_namespace()
    window.noet = state.wasmModule.BeliefBaseWasm;
  } catch (error) {
    console.error(
      "[Noet] WASM initialization failed (theme and basic features still work):",
      error,
    );
    showNavError();
  }

  // 7. Load the initial document
  const initialHash = window.location.hash;
  if (initialHash && initialHash !== "#") {
    await handleHashChange();
  } else {
    await loadDefaultDocument();
  }

  console.log("[Noet] Viewer initialized successfully");
});

// =============================================================================
// DOM reference initialization
// =============================================================================

function initializeDOMReferences() {
  state.containerElement = document.querySelector(".noet-container");
  state.navElement = document.querySelector(".noet-nav");
  state.navContent = document.getElementById("nav-content");
  state.navError = document.getElementById("nav-error");
  state.contentElement = document.querySelector(".noet-content");
  state.metadataPanel = document.getElementById("metadata-panel");
  state.metadataContent = document.getElementById("metadata-content");
  state.metadataError = document.getElementById("metadata-error");
  state.graphContainer = document.getElementById("graph-container");
  state.graphCanvas = document.getElementById("graph-canvas");
  state.footerElement = document.querySelector(".noet-footer");

  state.themeSelect = document.getElementById("theme-select");
  state.metadataClose = document.getElementById("metadata-close");
  state.graphClose = document.getElementById("graph-close");
  state.navCollapseBtn = document.getElementById("nav-collapse");
  state.metadataCollapseBtn = document.getElementById("metadata-collapse");

  state.themeLightLink = document.getElementById("theme-light");
  state.themeDarkLink = document.getElementById("theme-dark");

  if (
    !state.navContent ||
    !state.metadataPanel ||
    !state.metadataContent ||
    !state.containerElement
  ) {
    console.error("[Noet] Critical DOM elements missing — viewer may not work correctly");
  }
  if (!state.themeSelect) console.error("[Noet] Theme select element not found");
  if (!state.themeLightLink) console.error("[Noet] Light theme stylesheet link not found");
  if (!state.themeDarkLink) console.error("[Noet] Dark theme stylesheet link not found");
}

// =============================================================================
// Document metadata
// =============================================================================

function loadMetadata() {
  const metadataScript = document.getElementById("noet-metadata");
  if (!metadataScript) {
    console.warn("[Noet] No metadata found in document");
    return;
  }

  try {
    state.documentMetadata = JSON.parse(metadataScript.textContent);
    console.log("[Noet] Loaded metadata:", state.documentMetadata);
  } catch (error) {
    console.error("[Noet] Failed to parse metadata:", error);
  }
}

// =============================================================================
// Event listeners
// =============================================================================

function setupEventListeners() {
  if (state.themeSelect) {
    state.themeSelect.addEventListener("change", handleThemeChange);
  }

  if (state.metadataClose) {
    state.metadataClose.addEventListener("click", closeMetadataPanel);
  }

  if (state.graphClose) {
    state.graphClose.addEventListener("click", closeGraphView);
  }

  if (state.navCollapseBtn) {
    state.navCollapseBtn.addEventListener("click", toggleNavPanel);
  }

  if (state.metadataCollapseBtn) {
    state.metadataCollapseBtn.addEventListener("click", toggleMetadataPanel);
  }

  document.addEventListener("keydown", handleKeyboardShortcuts);

  if (state.navContent) {
    state.navContent.addEventListener("click", handleNavClick);
  }

  if (state.contentElement) {
    state.contentElement.addEventListener("click", handleContentClick);
  }

  window.addEventListener("hashchange", handleHashChange);

  // Reset two-click selection on click outside content
  document.addEventListener("click", (e) => {
    if (!e.target.closest(".noet-content") && !e.target.closest(".noet-metadata")) {
      state.selectedNodeBid = null;
      clearSelectedLinkHighlight();
    }
  });
}

// =============================================================================
// Navigation tree click handler (delegated)
// =============================================================================

function handleNavClick(event) {
  const target = event.target;

  if (target.classList.contains("noet-nav-tree__toggle")) {
    event.preventDefault();
    const parentLi = target.closest("li");
    const childrenContainer = parentLi?.querySelector(".noet-nav-tree__children");

    if (childrenContainer) {
      const isExpanded = parentLi.classList.toggle("is-expanded");
      target.textContent = isExpanded ? "▼" : "▶";
      target.setAttribute("aria-expanded", isExpanded);
    }
  }

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
// Content area click handler (two-click navigation pattern)
// =============================================================================

function handleContentClick(e) {
  const link = e.target.closest("a");
  if (!link) return;

  // Ignore links outside .noet-content (nav, metadata, footer)
  if (!link.closest(".noet-content")) return;

  // Header anchors (🔗) are direct-navigation links — call navigateToSection
  // immediately without the two-click metadata pattern. We must NOT let the
  // browser follow the bare href because the page is served from /pages/ and
  // the browser would resolve #id relative to that origin instead of the SPA root.
  if (link.classList.contains("noet-header-anchor")) {
    e.preventDefault();
    const headerId = link.getAttribute("href"); // bare "#id"
    if (headerId && headerId.startsWith("#")) {
      // Resolve the section BID from the title attribute so the metadata panel syncs.
      const sectionBid = extractBidFromLink(link);
      navigateToSection(headerId, sectionBid);
    }
    return;
  }

  const linkBid = extractBidFromLink(link);
  const href = link.getAttribute("href");

  if (!linkBid && !href) {
    console.warn("[Noet] Link has no BID or href, ignoring");
    e.preventDefault();
    return;
  }

  e.preventDefault();

  if (state.selectedNodeBid === linkBid) {
    // Second click — navigate
    if (href) {
      navigateToLink(href, link, linkBid);
    }
    state.selectedNodeBid = null;
    clearSelectedLinkHighlight();
  } else {
    // First click — show metadata
    if (linkBid && state.beliefbase) {
      showMetadataPanel(linkBid);
      state.selectedNodeBid = linkBid;
      highlightSelectedLink(link);
    } else if (href) {
      navigateToLink(href, link, null);
    }
  }
}

/**
 * Extract BID from a link's title attribute ("bref://...").
 * @param {HTMLElement} link
 * @returns {string|null}
 */
function extractBidFromLink(link) {
  const title = link.getAttribute("title");
  if (!title) return null;

  const match = title.match(/^bref:\/\/(.+?)(?:\s|$)/);
  if (!match) return null;

  if (!state.beliefbase) {
    console.warn("[Noet] Cannot resolve bref — BeliefBase not initialized");
    return null;
  }

  return state.beliefbase.get_bid_from_bref(match[1]);
}

// =============================================================================
// Graph view (Step 4 placeholder)
// =============================================================================

function closeGraphView() {
  if (state.graphContainer) {
    state.graphContainer.hidden = true;
  }
}
