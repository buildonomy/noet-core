/**
 * viewer/panels.js — Panel collapse/expand, keyboard shortcuts, error display
 *
 * Manages the three-panel layout (nav left, content center, metadata right).
 * Panel collapse state is persisted to localStorage under "noet-panel-state".
 *
 * Keyboard shortcuts (desktop only, min-width 1024px):
 *   Ctrl+\  — Toggle navigation panel
 *   Ctrl+]  — Toggle metadata panel
 */

import { state } from "./state.js";
import { adjustGutterForCollapse } from "./resize.js";

// =============================================================================
// Panel state persistence
// =============================================================================

/**
 * Load panel collapse state from localStorage and apply it to the DOM.
 * Must be called after DOM references are populated.
 */
export function initializePanelState() {
  const saved = localStorage.getItem("noet-panel-state");
  if (saved) {
    try {
      state.panelState = JSON.parse(saved);
    } catch (e) {
      console.warn("[Noet] Failed to parse saved panel state");
    }
  }

  applyPanelState();
}

/**
 * Persist current panel state to localStorage.
 */
export function savePanelState() {
  localStorage.setItem("noet-panel-state", JSON.stringify(state.panelState));
}

// =============================================================================
// Toggle actions
// =============================================================================

/**
 * Toggle the navigation panel (desktop only).
 */
export function toggleNavPanel() {
  state.panelState.navCollapsed = !state.panelState.navCollapsed;
  adjustGutterForCollapse("nav", state.panelState.navCollapsed);
  applyPanelState();
  savePanelState();
}

/**
 * Toggle the metadata panel (desktop only).
 */
export function toggleMetadataPanel() {
  if (!state.metadataPanel) return;
  state.panelState.metadataCollapsed = !state.panelState.metadataCollapsed;
  adjustGutterForCollapse("metadata", state.panelState.metadataCollapsed);
  applyPanelState();
  savePanelState();
}

// =============================================================================
// DOM application
// =============================================================================

/**
 * Apply the current panelState to the DOM by toggling CSS classes and
 * updating collapse button labels/aria attributes.
 */
export function applyPanelState() {
  if (!state.containerElement) return;

  // Nav panel
  if (state.panelState.navCollapsed) {
    state.containerElement.classList.add("nav-collapsed");
    if (state.navCollapseBtn) {
      state.navCollapseBtn.classList.add("is-collapsed");
      state.navCollapseBtn.setAttribute("aria-label", "Expand navigation panel");
    }
  } else {
    state.containerElement.classList.remove("nav-collapsed");
    if (state.navCollapseBtn) {
      state.navCollapseBtn.classList.remove("is-collapsed");
      state.navCollapseBtn.setAttribute("aria-label", "Collapse navigation panel");
    }
  }

  // Metadata panel
  if (state.panelState.metadataCollapsed) {
    state.containerElement.classList.add("metadata-collapsed");
    if (state.metadataCollapseBtn) {
      state.metadataCollapseBtn.classList.add("is-collapsed");
      state.metadataCollapseBtn.setAttribute("aria-label", "Show metadata panel");
    }
  } else {
    state.containerElement.classList.remove("metadata-collapsed");
    if (state.metadataCollapseBtn) {
      state.metadataCollapseBtn.classList.remove("is-collapsed");
      state.metadataCollapseBtn.setAttribute("aria-label", "Hide metadata panel");
    }
  }
}

// =============================================================================
// Error display
// =============================================================================

/**
 * Classify an initialization failure into an actionable explanation.
 *
 * Callers hand us whatever `initializeWasm()` threw. Most such failures share
 * one of a few root causes, and the fix differs sharply between them: a missing
 * asset is a deployment problem, a `file://` origin is a how-you-opened-it
 * problem. Naming which one we hit turns "navigation is broken" into something
 * the reader can act on.
 *
 * Returns null when the error does not match a cause we can describe usefully —
 * better to show the generic message than to guess wrong.
 *
 * @param {unknown} error
 * @returns {string|null} Explanation text, or null if the cause is unknown.
 */
function diagnoseInitError(error) {
  const message = error instanceof Error ? error.message : String(error ?? "");

  // file:// origins fail every fetch with an opaque TypeError, so check the
  // origin before trying to interpret the message itself.
  if (window.location.protocol === "file:") {
    return (
      "This page was opened directly from disk. Browsers block the data " +
      "requests the viewer needs on file:// URLs. Serve the output directory " +
      "over HTTP instead — for example, `noet watch --html-output DIR --serve`."
    );
  }

  // Order matters: a parse failure means the file was found but is corrupt,
  // which is a different fix from the file being absent. Check it first so the
  // broader codecs.json branch below does not claim files are missing.
  if (message.includes("Failed to parse codecs.json")) {
    return (
      "The codec manifest (codecs.json) was found but could not be parsed. " +
      "It is likely truncated or corrupted — re-export, or re-copy the file."
    );
  }

  if (message.includes("codec manifest") || message.includes("codecs.json")) {
    return (
      "The codec manifest (codecs.json) could not be loaded. It is written " +
      "next to the beliefbase data on every export, so this usually means the " +
      "deployment is missing files — copy the whole output directory, not just " +
      "the HTML."
    );
  }

  if (message.includes("noet-entry-bid")) {
    return (
      "This page is missing its entry-point marker, so the viewer cannot tell " +
      "which document to open. The HTML may have been edited or generated by " +
      "a different tool version."
    );
  }

  return null;
}

/**
 * Show the navigation error banner and clear nav content.
 *
 * Pass the caught error to add a cause-specific explanation beneath the generic
 * message. Without it the banner still renders, just without the diagnosis.
 *
 * @param {unknown} [error] The error that caused initialization to fail.
 */
export function showNavError(error) {
  if (state.navError) {
    state.navError.hidden = false;
  }
  if (state.navContent) {
    state.navContent.innerHTML = "";
  }

  const detail = document.getElementById("nav-error-detail");
  if (!detail) {
    return; // Older template without the detail slot — generic banner only.
  }

  const explanation = error === undefined ? null : diagnoseInitError(error);
  if (!explanation) {
    detail.hidden = true;
    detail.textContent = "";
    return;
  }

  // textContent, not innerHTML: the message can carry a URL or a parser error
  // from arbitrary JSON, and none of it is trusted markup.
  detail.textContent = explanation;
  detail.hidden = false;
}

/**
 * Show the metadata error banner.
 */
export function showMetadataError() {
  if (state.metadataError) {
    state.metadataError.hidden = false;
  }
}

// =============================================================================
// Keyboard shortcuts
// =============================================================================

/**
 * Handle keydown events for panel toggle shortcuts.
 * Only active on desktop (viewport >= 1024px).
 * @param {KeyboardEvent} e
 */
export function handleKeyboardShortcuts(e) {
  const isDesktop = window.matchMedia("(min-width: 1024px)").matches;
  if (!isDesktop) return;

  // Ctrl+\ — Toggle navigation panel
  if (e.ctrlKey && e.key === "\\") {
    e.preventDefault();
    toggleNavPanel();
  }

  // Ctrl+] — Toggle metadata panel
  if (e.ctrlKey && e.key === "]") {
    e.preventDefault();
    toggleMetadataPanel();
  }
}
