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
 * External asset highlighting (highlightExternalInContent):
 *   Called from the metadata panel when the user clicks a relation link that
 *   points to an asset or external href embedded in the content. Searches the
 *   article for any element whose src or href contains the given path —
 *   covering <img>, <a href="...pdf">, and plain anchor hrefs — then scrolls
 *   to and highlights the first match.
 *
 * Circular-import note:
 *   showMetadataPanel lives in metadata.js. Rather than importing it directly
 *   (which would create a metadata↔content cycle through routing), it is
 *   invoked via callbacks.showMetadataPanel registered in viewer.js at startup.
 */

import { state, callbacks } from "./state.js";
import { escapeHtml, copyToClipboard } from "./utils.js";

// =============================================================================
// Public API
// =============================================================================

/**
 * Post-process a freshly loaded article container:
 *   - Wrap images for the two-click / modal pattern
 *   - Inject anchor links on section headers
 *   - Initialize xlsx tab switcher (if workbook data present)
 *
 * @param {HTMLElement} container - Element whose first <article> child to process
 * @param {string} [pendingAnchor] - Optional section anchor (e.g. "#row-id") to
 *   navigate to after xlsx tab initialization.  Passed through to
 *   noetInitXlsxTabs so it can activate the correct tab and scroll to the row.
 */
export function processLoadedContent(container, pendingAnchor) {
  if (!container) return;

  const article = container.querySelector("article");
  if (!article) return;

  wrapImages(article);
  injectHeaderAnchors(article);
  attachQuerySearchButtons(article);
  reExecuteScripts(article);

  // If the loaded content contains xlsx workbook data, initialize the tab switcher.
  // xlsx-tabs.js is loaded globally via the template <script> tag and won't be
  // re-executed by reExecuteScripts, so we call the exported entry point directly.
  if (
    container.querySelector("#noet-xlsx-data") &&
    typeof window.noetInitXlsxTabs === "function"
  ) {
    window.noetInitXlsxTabs(callbacks, pendingAnchor);
  }
}

/**
 * Re-execute all inline <script> elements inside a container.
 *
 * Browsers do not execute scripts injected via innerHTML. This function
 * replaces each <script> with a fresh clone so the browser runs it.
 * Needed for embedded Tabulator initializers in xlsx tab HTML files.
 *
 * External scripts (src=) are also re-executed so any deferred asset
 * loads triggered by injected content fire correctly.
 *
 * @param {HTMLElement} container
 */
function reExecuteScripts(container) {
  container.querySelectorAll("script").forEach((oldScript) => {
    const newScript = document.createElement("script");
    for (const attr of oldScript.attributes) {
      newScript.setAttribute(attr.name, attr.value);
    }
    newScript.textContent = oldScript.textContent;
    oldScript.parentNode.replaceChild(newScript, oldScript);
  });
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
 * Highlight an external element in the currently loaded article content and
 * scroll it into view.
 *
 * Every link and image produced by the content pipeline carries a
 * `title="bref://..."` attribute. We resolve `targetBid` against those title
 * attributes so the lookup works uniformly for images, PDF anchors, and
 * external href anchors without having to match against src/href substrings.
 *
 * For <img> elements the highlight is applied to the closest
 * .noet-image-wrapper ancestor (if present) so the CSS outline covers the
 * whole wrapper, not just the bare <img>.
 *
 * @param {string} targetBid - BID of the external node to locate in the article
 * @returns {boolean} true if a matching element was found and highlighted
 */
export function highlightExternalInContent(targetBid) {
  if (!state.contentElement || !targetBid || !state.beliefbase) return false;

  const article = state.contentElement.querySelector("article") || state.contentElement;

  // Every pipeline-generated link/image has title="bref://... [optional metadata]"
  // Select all elements that could carry such a title.
  const candidates = article.querySelectorAll("[title]");

  console.log(
    `[Noet] highlightExternalInContent: searching for BID: ${targetBid}, candidates: ${candidates.length}`,
  );
  for (const el of candidates) {
    const title = el.getAttribute("title") || "";
    if (!title.includes("bref://")) continue;

    // Resolve the bref in the title to a BID and compare
    const brefMatch = title.match(/bref:\/\/(\S+)/);
    if (!brefMatch) continue;

    const resolvedBid = state.beliefbase.get_bid_from_bref(brefMatch[1]);
    console.log(
      `[Noet] highlightExternalInContent: candidate <${el.tagName.toLowerCase()}> bref=${brefMatch[1]} -> resolvedBid=${resolvedBid} (match=${resolvedBid === targetBid})`,
      el,
    );
    if (resolvedBid !== targetBid) continue;

    // Match found — determine the highlight target
    const highlightTarget =
      el.tagName === "IMG" ? el.closest(".noet-image-wrapper") || el : el;

    console.log(
      `[Noet] highlightExternalInContent: matched, highlighting`,
      highlightTarget,
    );
    clearSelectedLinkHighlight();
    highlightTarget.classList.add("noet-link-selected");
    highlightTarget.scrollIntoView({ behavior: "smooth", block: "center" });
    return true;
  }

  console.warn(
    `[Noet] highlightExternalInContent: no element found for BID: ${targetBid}`,
  );
  return false;
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

    // Attempt to resolve section bref via WASM for two-click support
    let sectionBref = null;
    if (state.beliefbase) {
      // Use the document's home network bref, not the entry point bref.
      // Heading nodes live in the document's network PathMap; looking them up
      // via the root entry network bref silently misses them for subnet pages.
      const lookupBref =
        state.currentDocNetworkBref ?? state.beliefbase.entryPoint().bref;
      const result = state.beliefbase.get_bid_from_id(lookupBref, headerId);
      if (result && result.bref) {
        sectionBref = result.bref;
      }
    }

    const anchor = document.createElement("a");
    anchor.className = "noet-header-anchor";
    anchor.href = `#${headerId}`;
    anchor.textContent = "🔗";
    anchor.setAttribute("aria-label", "Copy link to this section");

    if (sectionBref) {
      anchor.setAttribute("title", `bref://${sectionBref}`);
    }

    anchor.addEventListener("click", (e) => {
      e.preventDefault();

      // Build the SPA URL: origin/pathname/?search#/doc/path#section-id
      // The hash has the form "#/doc/path" or "#/doc/path#existing-anchor".
      // We strip any existing anchor and append the clicked heading's id.
      const currentHash = window.location.hash.substring(1); // drop leading #
      let docPath = currentHash;
      if (state.wasmModule) {
        const parts = state.wasmModule.BeliefBaseWasm.pathParts(currentHash);
        docPath = parts.path ? `${parts.path}/${parts.filename}` : parts.filename || "";
      }

      const sectionUrl = `${window.location.origin}${window.location.pathname}${window.location.search}#/${docPath}#${headerId}`;
      copyToClipboard(sectionUrl, "Section link copied");
    });

    header.appendChild(anchor);
  });

  const inlineAnchors = article.querySelectorAll("a.noet-inline-anchor");
  inlineAnchors.forEach((el) => {
    const id = el.getAttribute("id");
    if (!id) return;

    // Skip if anchor already injected
    if (
      el.nextElementSibling &&
      el.nextElementSibling.classList.contains("noet-header-anchor")
    )
      return;

    // Try bref from the element's own title attribute first
    let sectionBref = null;
    const titleAttr = el.getAttribute("title");
    if (titleAttr && titleAttr.startsWith("bref://")) {
      sectionBref = titleAttr.substring(7);
    } else if (state.beliefbase) {
      const lookupBref =
        state.currentDocNetworkBref ?? state.beliefbase.entryPoint().bref;
      const result = state.beliefbase.get_bid_from_id(lookupBref, id);
      if (result && result.bref) {
        sectionBref = result.bref;
      }
    }

    const link = document.createElement("a");
    link.className = "noet-header-anchor";
    link.href = `#${id}`;
    link.textContent = "🔗";
    link.setAttribute("aria-label", "Copy link to this section");

    if (sectionBref) {
      link.setAttribute("title", `bref://${sectionBref}`);
    }

    link.addEventListener("click", (e) => {
      e.preventDefault();

      const currentHash = window.location.hash.substring(1);
      let docPath = currentHash;
      if (state.wasmModule) {
        const parts = state.wasmModule.BeliefBaseWasm.pathParts(currentHash);
        docPath = parts.path ? `${parts.path}/${parts.filename}` : parts.filename || "";
      }

      const sectionUrl = `${window.location.origin}${window.location.pathname}${window.location.search}#/${docPath}#${id}`;
      copyToClipboard(sectionUrl, "Section link copied");
    });

    // Append the 🔗 at the end of the parent block element (p, li) so it
    // appears after the text, matching heading anchor placement.  The id-
    // bearing <a class="noet-inline-anchor"> stays at the start of the
    // block for correct fragment-navigation scroll position.
    const parent = el.parentElement;
    if (parent) {
      parent.appendChild(link);
    } else {
      el.insertAdjacentElement("afterend", link);
    }
  });
}

// =============================================================================
// Internal — query search buttons
// =============================================================================

/**
 * Attach "Open in Search" buttons to each `.noet-query-result` block that has
 * an associated `.noet-query-meta` sibling emitted by the compile-time renderer.
 *
 * The meta div sits as a previous sibling of `.noet-query-result` and carries
 * `data-query` (the original query text) and `data-count` (result count).
 * The button is positioned in the top-right corner of the result block and
 * opens the traceability panel in search mode with the query pre-populated.
 *
 * @param {HTMLElement} article
 */
function attachQuerySearchButtons(article) {
  const metaDivs = article.querySelectorAll(".noet-query-meta");
  metaDivs.forEach((meta) => {
    const queryText = meta.getAttribute("data-query");
    if (!queryText) return;

    // The result block is the next element sibling of the meta div
    const resultBlock = meta.nextElementSibling?.classList.contains("noet-query-result")
      ? meta.nextElementSibling
      : meta.closest(".noet-query-result");
    if (!resultBlock) return;

    // Skip if button already attached
    if (resultBlock.querySelector(".noet-query-open-search")) return;

    // Ensure the result block is positioned for absolute placement of the button
    const position = getComputedStyle(resultBlock).position;
    if (position === "static") {
      resultBlock.style.position = "relative";
    }

    const button = document.createElement("button");
    button.className = "noet-query-open-search";
    button.textContent = "Open in Search";
    button.setAttribute("aria-label", "Open this query in the traceability search panel");
    button.addEventListener("click", () => {
      if (callbacks.openTraceabilitySearch) {
        callbacks.openTraceabilitySearch(queryText);
      }
    });

    resultBlock.insertBefore(button, resultBlock.firstChild);
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
