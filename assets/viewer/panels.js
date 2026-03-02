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
  applyPanelState();
  savePanelState();
}

/**
 * Toggle the metadata panel (desktop only).
 */
export function toggleMetadataPanel() {
  if (!state.metadataPanel) return;
  state.panelState.metadataCollapsed = !state.panelState.metadataCollapsed;
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
      state.navCollapseBtn.textContent = "▶";
      state.navCollapseBtn.setAttribute("aria-label", "Expand navigation panel");
    }
  } else {
    state.containerElement.classList.remove("nav-collapsed");
    if (state.navCollapseBtn) {
      state.navCollapseBtn.textContent = "◀";
      state.navCollapseBtn.setAttribute("aria-label", "Collapse navigation panel");
    }
  }

  // Metadata panel
  if (state.panelState.metadataCollapsed) {
    state.containerElement.classList.add("metadata-collapsed");
    if (state.metadataCollapseBtn) {
      state.metadataCollapseBtn.textContent = "◀";
      state.metadataCollapseBtn.setAttribute("aria-label", "Show metadata panel");
    }
  } else {
    state.containerElement.classList.remove("metadata-collapsed");
    if (state.metadataCollapseBtn) {
      state.metadataCollapseBtn.textContent = "▶";
      state.metadataCollapseBtn.setAttribute("aria-label", "Hide metadata panel");
    }
  }
}

// =============================================================================
// Error display
// =============================================================================

/**
 * Show the navigation error banner and clear nav content.
 */
export function showNavError() {
  if (state.navError) {
    state.navError.hidden = false;
  }
  if (state.navContent) {
    state.navContent.innerHTML = "";
  }
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
