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
 *   viewer/wasm.js             — WASM init, getBidFromPath (detects sharded vs monolithic)
 *   viewer/shard-manager.js    — ShardManager: memory-budgeted shard load/unload, search index loading
 *   viewer/network-selector.js — Network selector panel UI (sharded mode only)
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
import {
  updateNavTreeHighlight,
  initShardLoadListener,
  handleNavLinkClick,
} from "./viewer/navigation.js";
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
import { initializeWasm, readBaseUrl } from "./viewer/wasm.js";
import { initNetworkSelector } from "./viewer/network-selector.js";
import { initVersionSelector } from "./viewer/version-selector.js";
import { initResizeHandles } from "./viewer/resize.js";
import { initSearch } from "./viewer/search.js";
import {
  openTraceabilityModal,
  openTraceabilitySearch,
  closeTraceabilityModal,
} from "./viewer/traceability.js";

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
  callbacks.openTraceabilityModal = openTraceabilityModal;
  callbacks.openTraceabilitySearch = openTraceabilitySearch;
  callbacks.closeTraceabilityModal = closeTraceabilityModal;
  callbacks.handleNodeLinkClick = function (bid, href, element) {
    // Two-click pattern for node links outside .noet-content (e.g. Tabulator cells).
    // First click on a BID: show metadata panel, record selection.
    // Second click on same BID: navigate, clear selection.
    if (bid && state.selectedNodeBid === bid) {
      // Second click — navigate.
      state.selectedNodeBid = null;
      if (element) clearSelectedLinkHighlight();
      if (href) {
        navigateToLink(href, element, bid);
      }
    } else {
      // First click — show metadata.
      if (bid && state.beliefbase) {
        showMetadataPanel(bid);
        state.selectedNodeBid = bid;
        if (element) highlightSelectedLink(element);
      } else if (href) {
        // No BID available — navigate directly.
        navigateToLink(href, element, null);
      }
    }
  };

  // 4. Attach DOM event listeners
  setupEventListeners();

  // 5. Theme, panel state, and resize handles (work without WASM)
  initializeTheme();
  initializePanelState();
  initResizeHandles({ toggleNav: toggleNavPanel, toggleMetadata: toggleMetadataPanel });

  // 5b. Prefetch AND render the initial page HTML before WASM init.
  // The URL hash is known immediately but the full loadDocument() flow
  // (path normalization, BID extraction, metadata panel) requires WASM.
  // We do a best-effort fetch + render here so the user sees content
  // instantly, then loadDocument() re-renders with full metadata when
  // WASM is ready.  The HTML body is identical both times — no flash.
  {
    const hash = window.location.hash.substring(1);
    if (hash && hash !== "/") {
      // Best-effort path normalisation without WASM: .md → .html
      let pagePath = hash.replace(/\.md(#|$)/, ".html$1");
      if (!pagePath.startsWith("/")) pagePath = "/" + pagePath;
      const fetchPath = pagePath.replace(/#.*$/, "");
      const baseUrl = readBaseUrl();
      const fullUrl = `${baseUrl}/pages${fetchPath}`;
      console.log(`[Noet] Prefetching initial page: ${fullUrl}`);

      const prefetchPromise = fetch(fullUrl).catch((err) => {
        console.warn("[Noet] Page prefetch failed:", err);
        return null;
      });

      state.prefetchedPage = { path: fetchPath, promise: prefetchPromise };

      // Render the prefetched HTML into the content area immediately.
      // This gives the user visible content while WASM loads in the background.
      // Target .noet-content__inner (same node loadDocument uses) so the
      // layout wrapper stays intact.
      prefetchPromise
        .then(async (resp) => {
          if (!resp || !resp.ok || !state.contentElement) return;
          // Clone the response so loadDocument can still consume the original.
          const html = await resp.clone().text();
          const parser = new DOMParser();
          const doc = parser.parseFromString(html, "text/html");
          const article = doc.querySelector("article");
          if (article) {
            const inner = state.contentElement.querySelector(".noet-content__inner");
            const target = inner || state.contentElement;
            let existingArticle = target.querySelector("article");
            if (!existingArticle) {
              existingArticle = document.createElement("article");
              target.appendChild(existingArticle);
            }
            existingArticle.innerHTML = article.innerHTML;
            console.log("[Noet] Early page render complete (pre-WASM)");
          }
        })
        .catch(() => {
          /* swallow — loadDocument will handle errors */
        });
    }
  }

  // 6. Load WASM and BeliefBase (non-blocking — theme/basic features still work if this fails)
  // Show the progress bar while WASM + shards load.  The user already sees
  // page content (from the early render above), but nav and metadata panels
  // are still populating.  The bar gives visual feedback that work is ongoing.
  {
    const bar = document.createElement("div");
    bar.id = "noet-shard-progress";
    bar.className = "noet-shard-progress";
    bar.setAttribute("role", "progressbar");
    bar.setAttribute("aria-label", "Loading navigation data\u2026");
    document.body.appendChild(bar);
  }
  try {
    await initializeWasm();
    // Expose BeliefBaseWasm on window.noet for browser console use.
    // Usage: noet.set_log_level('debug')
    //        noet.href_namespace()
    window.noet = state.wasmModule.BeliefBaseWasm;
    // Initialize network selector panel (sharded mode only; no-op in monolithic mode).
    initNetworkSelector();
    // Initialize version selector dropdown (no-op in single-version deployments).
    initVersionSelector();
    // Initialize full-text search (requires state.searchIndex populated by initializeWasm).
    initSearch();
    // Register background shard-load listener — rebuilds nav tree when a shard finishes loading.
    initShardLoadListener();
  } catch (error) {
    console.error(
      "[Noet] WASM initialization failed (theme and basic features still work):",
      error,
    );
    // Pass the error so the banner can name the cause when it recognises one.
    showNavError(error);
  }

  // 7. Load the initial document (populates metadata panel via showMetadataPanel)
  const initialHash = window.location.hash;
  if (initialHash && initialHash !== "#") {
    await handleHashChange();
  } else {
    await loadDefaultDocument();
  }

  // 8. Handle ?q= URL parameter — open traceability panel with the query.
  // Must run after WASM init (step 6) and initial document load (step 7)
  // so that the beliefbase and nav tree are available.
  {
    const urlParams = new URLSearchParams(window.location.search);
    const qParam = urlParams.get("q");
    if (qParam && state.beliefbase) {
      console.log("[Noet] Opening traceability from ?q= parameter:", qParam);
      // Open the traceability panel in search mode with the query text.
      // The query text may be a plain text search, an id:// anchored query
      // grammar string, or any other query form. Search mode handles all
      // of these by running TF-IDF against the query tokens.
      openTraceabilitySearch(qParam);
    }
  }

  // 9. Remove the init progress bar — nav, content, and metadata are now
  // populated (or errored).  Subsequent shard loads will show their own
  // progress bar via the noet:shard-loading event listener.
  {
    const bar = document.getElementById("noet-shard-progress");
    if (bar) bar.remove();
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

  state.searchInput = document.getElementById("search-input");
  state.searchClear = document.getElementById("search-clear");

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
  if (!state.themeLightLink)
    console.error("[Noet] Light theme stylesheet link not found");
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

  // Footer links (e.g. cover page reference) should route through the SPA
  const footer = document.querySelector(".noet-footer");
  if (footer) {
    footer.addEventListener("click", (e) => {
      const link = e.target.closest("a");
      if (!link) return;
      const href = link.getAttribute("href");
      if (href && href.startsWith("#/")) {
        e.preventDefault();
        window.location.hash = href.substring(1);
      }
    });
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
      // For network nodes: expand the subtree and load the shard in addition
      // to navigating.  handleNavLinkClick is a no-op for non-network nodes.
      if (targetBid) {
        handleNavLinkClick(targetBid);
      }
      // Nav links emit hash-based hrefs (#/path or #/path#anchor) so that
      // clicking updates window.location.hash and fires hashchange → handleHashChange,
      // rather than issuing a real HTTP request. navigateToLink treats "#"-prefixed
      // hrefs as same-page section anchors (navigateToSection), which is wrong here.
      if (href.startsWith("#/")) {
        window.location.hash = href.substring(1); // strip leading "#" — hash setter adds it
      } else {
        navigateToLink(href, target, targetBid);
      }
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

  let linkBid = extractBidFromLink(link);
  const href = link.getAttribute("href");

  // Resolve section BID from anchor href when the link has no explicit bref.
  // This lets in-page anchor links (e.g. source-view line-number links) participate
  // in the two-click metadata pattern without requiring a bref:// title.
  if (!linkBid && href && href.startsWith("#") && state.beliefbase) {
    const sectionId = href.substring(1);
    const lookupBref = state.currentDocNetworkBref ?? state.beliefbase.entryPoint().bref;
    const result = state.beliefbase.get_bid_from_id(lookupBref, sectionId);
    console.log(
      `[Noet] Anchor BID resolution: sectionId="${sectionId}" lookupBref="${lookupBref}"`,
      `result=`,
      result,
      `currentDocNetworkBref=`,
      state.currentDocNetworkBref,
      `entryBref=`,
      state.beliefbase.entryPoint().bref,
    );
    if (result) {
      linkBid = result.bid;
    }
  }

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

  const bid = state.beliefbase.get_bid_from_bref(match[1]);
  if (!bid) {
    // Node is in an unloaded shard. Return null so the caller falls through to
    // navigateToLink, where the hasSchema guard will open external URLs in a
    // new tab. Returning the raw bref would break showMetadataPanel (which
    // requires a full BID, not a bref string).
    const loadedShards = JSON.parse(state.beliefbase.loaded_shards());
    console.warn(
      `[Noet] Bref not resolved (unloaded shard?): ${match[1]}\n` +
        `  Loaded shards: ${JSON.stringify(loadedShards)}\n` +
        `  Total nodes in beliefbase: ${state.beliefbase.node_count()}`,
    );
    return null;
  }
  return bid;
}

// =============================================================================
// Graph view (Step 4 placeholder)
// =============================================================================

function closeGraphView() {
  if (state.graphContainer) {
    state.graphContainer.hidden = true;
  }
}
