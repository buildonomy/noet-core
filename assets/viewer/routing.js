/**
 * viewer/routing.js — Client-side hash routing, document loading, and navigation
 *
 * Responsible for:
 *   - parseHashPath / stripAnchor / getCurrentDocPath — URL path helpers
 *   - handleHashChange()     — window hashchange handler
 *   - loadDefaultDocument()  — load /index.html on empty hash
 *   - loadDocument()         — fetch, parse, and inject an HTML document
 *   - navigateToLink()       — dispatch link clicks to document or section nav
 *   - navigateToDocument()   — update window.location.hash for document nav
 *   - navigateToSection()    — smooth-scroll to an in-page section
 *
 * Circular-import note:
 *   showMetadataPanel  lives in metadata.js  → called via callbacks.showMetadataPanel
 *   updateNavTreeHighlight lives in navigation.js → called via callbacks.updateNavTreeHighlight
 *   processLoadedContent   lives in content.js  → imported directly (no cycle)
 *   highlightElementById   lives in content.js  → imported directly (no cycle)
 *   clearSelectedLinkHighlight lives in content.js → imported directly
 */

import { state, callbacks } from "./state.js";
import { escapeHtml } from "./utils.js";
import {
  processLoadedContent,
  clearSelectedLinkHighlight,
  highlightElementById,
} from "./content.js";
import { getBidFromPath } from "./wasm.js";

// =============================================================================
// Public API — URL / path helpers
// =============================================================================

/**
 * Get the current document path from the URL hash, without any section anchor.
 * Returns an empty string when WASM is not yet loaded.
 * @returns {string} e.g. "net1_dir1/doc.html"
 */
export function getCurrentDocPath() {
  const hash = window.location.hash.substring(1);
  if (!hash || !state.wasmModule) return "";

  return docPathKey(state.wasmModule.BeliefBaseWasm.pathParts(hash));
}

/**
 * Canonical comparison key for a document path: "dir/filename" with no
 * leading slash. Used wherever two paths need to be compared for same-doc
 * identity regardless of whether they arrived with or without a leading "/".
 *
 * @param {PathParts} parts - Result of BeliefBaseWasm.pathParts(...)
 * @returns {string} e.g. "net1/doc.html" or "doc.html"
 */
function docPathKey(parts) {
  const joined = parts.path ? `${parts.path}/${parts.filename}` : parts.filename;
  return joined.startsWith("/") ? joined.substring(1) : joined;
}

/**
 * Parse a hash string into a document path and an optional section anchor.
 *
 * @param {string} hash - Hash string with or without a leading "#"
 * @returns {{ path: string, anchor: string|null }}
 *
 * @example
 * parseHashPath("#net1/doc.html#intro") → { path: "net1/doc.html", anchor: "intro" }
 * parseHashPath("dir/doc.html")         → { path: "dir/doc.html",  anchor: null  }
 */
export function parseHashPath(hash) {
  const cleanHash = hash.startsWith("#") ? hash.substring(1) : hash;
  if (!cleanHash || !state.wasmModule) {
    return { path: cleanHash, anchor: null };
  }

  const parts = state.wasmModule.BeliefBaseWasm.pathParts(cleanHash);
  const path = parts.path ? `${parts.path}/${parts.filename}` : parts.filename;
  const anchor = parts.anchor || null;

  return { path, anchor };
}

/**
 * Strip the section anchor from a path string.
 * Falls back to a naive string split when WASM is unavailable.
 *
 * @param {string} path - e.g. "dir/doc.html#section"
 * @returns {string} e.g. "dir/doc.html"
 */
export function stripAnchor(path) {
  if (!path) return path;

  if (state.wasmModule) {
    const parts = state.wasmModule.BeliefBaseWasm.pathParts(path);
    return parts.path ? `${parts.path}/${parts.filename}` : parts.filename;
  }

  const hashIndex = path.indexOf("#");
  return hashIndex !== -1 ? path.substring(0, hashIndex) : path;
}

// =============================================================================
// Public API — navigation
// =============================================================================

/**
 * Handle window `hashchange` events (client-side routing entry point).
 * Also called manually on DOMContentLoaded when a hash is already present.
 */
export async function handleHashChange() {
  const hash = window.location.hash;

  // Reset two-click selection on every navigation
  state.selectedNodeBid = null;
  clearSelectedLinkHighlight();

  if (!hash || hash === "#") {
    await loadDefaultDocument();
    return;
  }

  let path = hash.substring(1); // strip leading "#"

  if (path.startsWith("#")) {
    // Double-hash — shouldn't happen in practice; ignore gracefully
    return;
  }

  // Split path and optional section anchor
  const parsed = parseHashPath(path);
  let sectionAnchor = parsed.anchor ? `#${parsed.anchor}` : null;
  path = parsed.path;

  // Normalise path segments (resolve ".." / ".")
  if (state.wasmModule) {
    path = state.wasmModule.BeliefBaseWasm.normalizePath(path);
  }

  // Normalise file extension (.md → .html)
  let normalizedPath = path;
  if (state.wasmModule?.BeliefBaseWasm?.normalize_path_extension) {
    normalizedPath = state.wasmModule.BeliefBaseWasm.normalize_path_extension(path);
  }

  // If the path contains no ".html" treat it as a section anchor in the current doc
  if (!normalizedPath.includes(".html")) {
    navigateToSection("#" + path, state.pendingMetadataBid);
    state.pendingMetadataBid = null;
    return;
  }

  // If we're already on this document and only the anchor changed, scroll to
  // the section instead of reloading the whole document.
  // Guard: only short-circuit if a real document has been fetched —
  // on a force-refresh the hash already contains the full doc#anchor form but
  // loadDocument() hasn't run yet (the shell's placeholder <article> is not enough).
  const normalizedNew = docPathKey(state.wasmModule.BeliefBaseWasm.pathParts(normalizedPath));
  const normalizedCurrent = state.currentDocPath;
  if (normalizedCurrent && normalizedCurrent === normalizedNew && sectionAnchor) {
    navigateToSection(sectionAnchor, state.pendingMetadataBid);
    state.pendingMetadataBid = null;
    return;
  }

  await loadDocument(path, sectionAnchor, state.pendingMetadataBid);
  state.pendingMetadataBid = null;
}

/**
 * Load the default document (site root index.html).
 */
export async function loadDefaultDocument() {
  await loadDocument("/index.html");
}

/**
 * Fetch and display an HTML document from /pages/.
 *
 * Steps:
 *   1. Normalise path extension and ensure leading "/"
 *   2. Fetch /pages/<path>
 *   3. Parse the response HTML; extract article content
 *   4. Extract document BID and metadata from embedded JSON script tags
 *   5. Replace main content area
 *   6. Post-process content (images, header anchors)
 *   7. Update nav highlight and show metadata panel
 *   8. Scroll to section anchor if provided
 *
 * @param {string} path - Document path, e.g. "/net1/doc.html"
 * @param {string|null} sectionAnchor - Optional "#section-id" to scroll to after load
 * @param {string|null} targetBid - Optional BID to show in the metadata panel
 */
export async function loadDocument(path, sectionAnchor = null, targetBid = null) {
  if (!state.contentElement) {
    console.error("[Noet] Content element not found");
    return;
  }

  state.currentDocPath = null;
  state.currentDocBid = null;

  try {
    // --- 1. Normalise path ---
    let normalizedPath = path;
    if (state.wasmModule?.BeliefBaseWasm?.normalize_path_extension) {
      normalizedPath = state.wasmModule.BeliefBaseWasm.normalize_path_extension(path);
      console.log(`[Noet] Normalised path: ${path} -> ${normalizedPath}`);
    } else {
      normalizedPath = path.replace(/\.md(#|$)/, ".html$1");
    }

    if (!normalizedPath.startsWith("/")) {
      normalizedPath = "/" + normalizedPath;
    }

    // --- 2. Fetch ---
    const fetchPath = `/pages${normalizedPath}`;
    console.log(`[Noet] Fetching document: ${fetchPath}`);

    const response = await fetch(fetchPath);
    if (!response.ok) {
      throw new Error(`HTTP ${response.status}: ${response.statusText}`);
    }

    const html = await response.text();

    // --- 3. Parse ---
    const parser = new DOMParser();
    const doc = parser.parseFromString(html, "text/html");

    // Rewrite relative src attributes to absolute /pages/<dir>/... paths before
    // extracting innerHTML. The shell's <base href="/pages/"> handles root-relative
    // documents, but subdirectory documents (e.g. /pages/subnet1/doc.html) may
    // contain src paths like "../assets/img.png" that resolve incorrectly against
    // the base tag. DOMParser has no base URL, so we must make them absolute here.
    const fetchBase = fetchPath.substring(0, fetchPath.lastIndexOf("/") + 1);
    doc.querySelectorAll("[src]").forEach((el) => {
      const src = el.getAttribute("src");
      if (src && !src.startsWith("/") && !src.includes("://")) {
        el.setAttribute("src", fetchBase + src);
      }
    });

    const articleElement = doc.querySelector("article");
    const bodyContent = articleElement ? articleElement.innerHTML : doc.body.innerHTML;

    if (!bodyContent) {
      throw new Error("No content found in fetched document");
    }

    // --- 4. Extract metadata ---
    let documentBid = null;
    const bodyElement = doc.querySelector("body[data-document-bid]");
    if (bodyElement) {
      documentBid = bodyElement.getAttribute("data-document-bid");
      console.log(`[Noet] Extracted document BID: ${documentBid}`);
    } else {
      console.warn("[Noet] No data-document-bid found in loaded document");
    }

    const metadataScript = doc.querySelector('script[type="application/json"]#noet-metadata');
    if (metadataScript) {
      try {
        state.documentMetadata = JSON.parse(metadataScript.textContent);
        console.log("[Noet] Updated document metadata:", state.documentMetadata);
      } catch (e) {
        console.warn("[Noet] Failed to parse document metadata:", e);
      }
    }

    const titleElement = doc.querySelector("title");
    if (titleElement) {
      document.title = titleElement.textContent;
    }

    // --- 5. Replace content ---
    const contentInner = state.contentElement.querySelector(".noet-content__inner");
    if (contentInner) {
      contentInner.innerHTML = `<article>${bodyContent}</article>`;
    } else {
      state.contentElement.innerHTML = bodyContent;
    }

    // --- 6. Post-process ---
    processLoadedContent(contentInner || state.contentElement);

    state.currentDocPath = docPathKey(state.wasmModule.BeliefBaseWasm.pathParts(normalizedPath));
    state.currentDocBid = documentBid;
    console.log(`[Noet] Document loaded: ${path}`);

    // --- 7. Nav highlight + metadata ---
    if (callbacks.updateNavTreeHighlight) {
      callbacks.updateNavTreeHighlight();
    }

    if (!sectionAnchor) {
      state.contentElement.scrollTo({ top: 0, behavior: "smooth" });

      const bidToShow = targetBid || documentBid || getBidFromPath(path);
      if (bidToShow && callbacks.showMetadataPanel) {
        callbacks.showMetadataPanel(bidToShow);
      } else if (!bidToShow) {
        console.warn("[Noet] No BID available to show metadata for document:", path);
      }
    }

    // Consume pending external-element highlight (set by metadata panel when the
    // owner document was not loaded at click time). Runs after section navigation
    // so the setTimeout fires after navigateToSection's synchronous showMetadataPanel
    // call, ensuring the asset node ends up in the panel rather than the section node.
    const pendingHighlight = state.pendingHighlightPath;
    state.pendingHighlightPath = null;

    // --- 8. Scroll to section ---
    if (sectionAnchor) {
      // Resolve the section BID so the metadata panel can show it.
      // targetBid may be null on a force-refresh (no pendingMetadataBid was set),
      // so derive it from the section ID via WASM if possible.
      let sectionBid = targetBid;
      if (!sectionBid && state.beliefbase) {
        const sectionId = sectionAnchor.substring(1); // strip leading "#"
        const entryPoint = state.beliefbase.entryPoint();
        const result = state.beliefbase.get_bid_from_id(entryPoint.bref, sectionId);
        if (result) {
          sectionBid = result.bid;
        }
      }
      setTimeout(() => {
        navigateToSection(sectionAnchor, sectionBid);
      }, 100); // brief delay to ensure content is rendered
    }

    // Fire pending highlight after content and section navigation have settled.
    // Uses a longer delay than section scroll (150 vs 100) so it runs after
    // navigateToSection's synchronous showMetadataPanel, letting the asset node
    // win in the panel.
    if (pendingHighlight) {
      setTimeout(() => {
        if (callbacks.highlightExternalInContent) {
          callbacks.highlightExternalInContent(pendingHighlight);
        }
        if (callbacks.showMetadataPanel) {
          callbacks.showMetadataPanel(pendingHighlight);
        }
      }, 150);
    }
  } catch (error) {
    console.error(`[Noet] Failed to load document: ${path}`, error);

    const contentInner = state.contentElement.querySelector(".noet-content__inner");
    const target = contentInner || state.contentElement;

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
 * Navigate to a document by updating window.location.hash.
 * Stores targetBid in state.pendingMetadataBid so the hashchange handler
 * can open the metadata panel after the document loads.
 *
 * @param {string} path - Document path
 * @param {string|null} targetBid
 */
export function navigateToDocument(path, targetBid = null) {
  if (targetBid) {
    state.pendingMetadataBid = targetBid;
  }
  window.location.hash = path;
}

/**
 * Scroll to a section within the current document.
 * Updates the URL hash to include the document path + anchor without
 * triggering a full hashchange navigation.
 *
 * @param {string} anchor - Section anchor including the "#" prefix, e.g. "#intro"
 * @param {string|null} targetBid - Optional BID to show in the metadata panel
 */
export function navigateToSection(anchor, targetBid = null) {
  const sectionId = anchor.substring(1); // strip leading "#"
  const targetElement = document.getElementById(sectionId);

  if (targetElement) {
    targetElement.scrollIntoView({ behavior: "smooth", block: "start" });
    highlightElementById(sectionId);

    // Reconstruct hash as "<docPath>#<sectionId>" to preserve the document path
    const currentHash = window.location.hash.substring(1);
    let newHash = anchor;

    if (currentHash && state.wasmModule) {
      const parts = state.wasmModule.BeliefBaseWasm.pathParts(currentHash);
      if (parts.filename) {
        const docPath = parts.path ? `${parts.path}/${parts.filename}` : parts.filename;
        newHash = `#${docPath}${anchor}`;
      }
    }

    if (!newHash.startsWith("#")) {
      newHash = "#" + newHash;
    }

    // Use replaceState with an absolute URL anchored to the SPA root ("/").
    // A bare hash like "#doc#section" is a relative URL — the browser resolves
    // it against the current fetch URL (/pages/doc.html), producing
    // /pages/#doc#section instead of /#doc#section.
    history.replaceState(null, "", "/" + newHash);

    if (targetBid && callbacks.showMetadataPanel) {
      callbacks.showMetadataPanel(targetBid);
    }
  } else {
    console.warn(`[Noet] Section not found: ${sectionId}`, new Error().stack);
  }
}

/**
 * Dispatch a link click to the appropriate navigation handler.
 *
 * Handles four cases in order:
 *   1. Section anchor link ("#...")         → navigateToSection
 *   2. href_namespace link (external URL)   → window.open
 *   3. asset_namespace link (PDF, image)    → window.open to /pages/<path>
 *   4. Internal document link               → navigateToDocument (via hash)
 *
 * Relative hrefs are resolved against the current document's directory.
 *
 * @param {string} href - The link's href attribute value
 * @param {HTMLElement} link - The clicked element (used for context only)
 * @param {string|null} targetBid - BID of the target node (may be null)
 */
export function navigateToLink(href, link, targetBid = null) {
  // 1. In-page section anchor
  if (href.startsWith("#")) {
    navigateToSection(href, targetBid);
    return;
  }

  // 2. External href_namespace link — open original href directly, no path resolution
  if (
    targetBid &&
    state.wasmModule &&
    targetBid.endsWith(state.wasmModule.BeliefBaseWasm.href_namespace().bref)
  ) {
    console.log(`[Noet] Opening external link: ${href}`);
    window.open(href, "_blank", "noopener,noreferrer");
    return;
  }

  // 3. Asset link — open original href directly, no path resolution
  if (
    targetBid &&
    state.wasmModule &&
    targetBid.endsWith(state.wasmModule.BeliefBaseWasm.asset_namespace().bref)
  ) {
    console.log(`[Noet] Opening asset: /pages/${href}`);
    window.open(`/pages/${href}`, "_blank");
    return;
  }

  // Resolve relative paths for internal document links only.
  // pathParts(href).hasSchema is the structural check — external URLs
  // (https://, file://, etc.) must never be passed through pathJoin.
  let resolvedPath = href;
  if (state.wasmModule) {
    const hrefParts = state.wasmModule.BeliefBaseWasm.pathParts(href);
    if (!hrefParts.hasSchema && !href.startsWith("/")) {
      const currentHash = window.location.hash.substring(1);
      if (currentHash) {
        const currentParts = state.wasmModule.BeliefBaseWasm.pathParts(currentHash);
        const parentDir = currentParts.path;
        resolvedPath = state.wasmModule.BeliefBaseWasm.pathJoin(parentDir, href, false);
        console.log(`[Noet] Resolved relative path: ${href} -> ${resolvedPath}`);
      }
    }
  }

  // 4. Internal document link (possibly with section anchor)
  const parsed = parseHashPath(resolvedPath);
  if (parsed.anchor) {
    const hashPath = parsed.path.startsWith("/")
      ? `${parsed.path}#${parsed.anchor}`
      : `/${parsed.path}#${parsed.anchor}`;
    window.location.hash = hashPath;
    if (targetBid) {
      state.pendingMetadataBid = targetBid;
    }
    return;
  }

  navigateToDocument(resolvedPath, targetBid);
}
