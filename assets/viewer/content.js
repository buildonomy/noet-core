/**
 * viewer/content.js — Content post-processing, image modal, link highlighting
 *
 * Responsible for all DOM mutations on the loaded article content:
 *   1. Wrapping <img> elements in modal-capable divs (two-click pattern)
 *   2. Injecting anchor links on <h1>–<h6> elements
 *   3. Opening a full-screen image modal
 *   4. Tracking and clearing the "selected link" highlight for two-click nav
 *
 * Two-click pattern for images:
 *   First click  → showMetadataPanel(bid)  [via callbacks.showMetadataPanel]
 *   Second click → openImageModal(img)
 *
 * Circular-import note:
 *   showMetadataPanel lives in metadata.js. Rather than importing it directly
 *   (which would create a metadata↔content cycle through routing), it is
 *   invoked via callbacks.showMetadataPanel registered in viewer.js at startup.
 */

import { state, callbacks } from "./state.js";
import { escapeHtml } from "./utils.js";

// =============================================================================
// Public API
// =============================================================================

/**
 * Post-process a freshly loaded article container:
 *   - Wrap images for the two-click / modal pattern
 *   - Inject anchor links on section headers
 *
 * @param {HTMLElement} container - Element whose first <article> child to process
 */
export function processLoadedContent(container) {
  if (!container) return;

  const article = container.querySelector("article");
  if (!article) return;

  wrapImages(article);
  injectHeaderAnchors(article);
}

/**
 * Highlight a link element for the two-click pattern.
 * Clears any previous highlight first.
 * @param {HTMLElement} link
 */
export function highlightSelectedLink(link) {
  clearSelectedLinkHighlight();
  link.classList.add("noet-link-selected");
}

/**
 * Remove the two-click selection highlight from whichever element currently has it.
 */
export function clearSelectedLinkHighlight() {
  const selected = document.querySelector(".noet-link-selected");
  if (selected) {
    selected.classList.remove("noet-link-selected");
  }
}

/**
 * Highlight a document section by element ID (used after section navigation).
 * @param {string} elementId
 */
export function highlightElementById(elementId) {
  clearSelectedLinkHighlight();
  const element = document.getElementById(elementId);
  if (element) {
    element.classList.add("noet-link-selected");
  }
}

/**
 * Highlight an asset (image) in the main content area and scroll it into view.
 * @param {string} assetPath - Partial or full path matching the image src
 */
export function highlightAssetInContent(assetPath) {
  if (!state.contentElement) return;

  const images = state.contentElement.querySelectorAll("img");
  for (const img of images) {
    if (img.src.includes(assetPath) || img.getAttribute("src") === assetPath) {
      const wrapper = img.closest(".noet-image-wrapper") || img;
      clearSelectedLinkHighlight();
      wrapper.classList.add("noet-link-selected");
      wrapper.scrollIntoView({ behavior: "smooth", block: "center" });
      break;
    }
  }
}

// =============================================================================
// Internal — image wrapping
// =============================================================================

/**
 * Wrap every unwrapped <img> inside article in a .noet-image-wrapper div.
 * Images with a bref:// title participate in the two-click pattern.
 * @param {HTMLElement} article
 */
function wrapImages(article) {
  const images = article.querySelectorAll("img");
  images.forEach((img) => {
    // Skip if already wrapped
    if (img.parentElement.classList.contains("noet-image-wrapper")) return;

    const wrapper = document.createElement("div");
    wrapper.className = "noet-image-wrapper";

    const imgTitle = img.getAttribute("title");
    const hasBref = imgTitle && imgTitle.includes("bref://");

    if (hasBref) {
      wrapper.setAttribute("data-two-click", "true");
      wrapper.setAttribute("data-image-title", imgTitle);
    }

    img.parentNode.insertBefore(wrapper, img);
    wrapper.appendChild(img);

    wrapper.addEventListener("click", () => handleImageClick(wrapper, img));
  });
}

/**
 * Handle a click on an image wrapper.
 * @param {HTMLDivElement} wrapper
 * @param {HTMLImageElement} img
 */
function handleImageClick(wrapper, img) {
  const isTwoClick = wrapper.getAttribute("data-two-click") === "true";
  const wrapperBid = extractBidFromImageTitle(wrapper.getAttribute("data-image-title"));

  if (isTwoClick && wrapperBid) {
    if (state.selectedNodeBid === wrapperBid) {
      // Second click — open modal
      openImageModal(img);
      state.selectedNodeBid = null;
      clearSelectedLinkHighlight();
    } else {
      // First click — show metadata
      if (callbacks.showMetadataPanel) {
        callbacks.showMetadataPanel(wrapperBid);
      }
      state.selectedNodeBid = wrapperBid;
      highlightSelectedLink(wrapper);
    }
  } else {
    openImageModal(img);
  }
}

/**
 * Extract BID from an image title attribute containing "bref://...".
 * @param {string|null} title
 * @returns {string|null}
 */
function extractBidFromImageTitle(title) {
  if (!title) return null;

  const match = title.match(/bref:\/\/(.+?)(?:\s|$)/);
  if (!match) return null;

  const bref = match[1];
  if (!state.beliefbase) return null;

  return state.beliefbase.get_bid_from_bref(bref);
}

// =============================================================================
// Internal — header anchors
// =============================================================================

/**
 * Inject a 🔗 anchor link after the text of each h1–h6 that has an id.
 * @param {HTMLElement} article
 */
function injectHeaderAnchors(article) {
  const headers = article.querySelectorAll("h1, h2, h3, h4, h5, h6");
  headers.forEach((header) => {
    const headerId = header.getAttribute("id");
    if (!headerId) return;

    // Skip if anchor already injected
    if (header.querySelector(".noet-header-anchor")) return;

    // Resolve the current document path for constructing the href
    const currentPath = getCurrentDocPath();

    // Attempt to resolve section bref via WASM for two-click support
    let sectionBref = null;
    if (state.beliefbase) {
      const entryPoint = state.beliefbase.entryPoint();
      const result = state.beliefbase.get_bid_from_id(entryPoint.bref, headerId);
      if (result && result.bref) {
        sectionBref = result.bref;
      }
    }

    const anchor = document.createElement("a");
    anchor.className = "noet-header-anchor";
    anchor.href = currentPath ? `${currentPath}#${headerId}` : `#${headerId}`;
    anchor.textContent = "🔗";
    anchor.setAttribute("aria-label", "Link to this section");

    if (sectionBref) {
      anchor.setAttribute("title", `bref://${sectionBref}`);
    }

    header.appendChild(anchor);
  });
}

// =============================================================================
// Internal — image modal
// =============================================================================

/**
 * Open a full-screen modal displaying the given image.
 * Closes on overlay click, close button click, or Escape key.
 * @param {HTMLImageElement} img
 */
function openImageModal(img) {
  const modal = document.createElement("div");
  modal.className = "noet-image-modal";
  modal.innerHTML = `
    <div class="noet-image-modal__overlay"></div>
    <div class="noet-image-modal__content">
      <button class="noet-image-modal__close" aria-label="Close">&times;</button>
      <img src="${escapeHtml(img.src)}" alt="${escapeHtml(img.alt || "")}" />
    </div>
  `;

  document.body.appendChild(modal);

  const closeModal = () => modal.remove();

  modal.querySelector(".noet-image-modal__close").addEventListener("click", closeModal);
  modal.querySelector(".noet-image-modal__overlay").addEventListener("click", closeModal);

  const handleEscape = (e) => {
    if (e.key === "Escape") {
      closeModal();
      document.removeEventListener("keydown", handleEscape);
    }
  };
  document.addEventListener("keydown", handleEscape);
}

// =============================================================================
// Internal — path helper (local copy to avoid importing routing.js)
// =============================================================================

/**
 * Get the current document path from the URL hash, without anchor.
 * Returns empty string if WASM is not yet loaded.
 * @returns {string}
 */
function getCurrentDocPath() {
  const hash = window.location.hash.substring(1);
  if (!hash || !state.wasmModule) return "";

  const parts = state.wasmModule.BeliefBaseWasm.pathParts(hash);
  return parts.path ? `${parts.path}/${parts.filename}` : parts.filename;
}
