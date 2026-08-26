/**
 * viewer/utils.js — Pure utility functions
 *
 * No imports, no side-effects. Safe to import from any module.
 */

/**
 * Escape HTML to prevent XSS
 * @param {string|null|undefined} text - Text to escape
 * @returns {string} Escaped HTML string
 */
export function escapeHtml(text) {
  if (text === null || text === undefined) {
    return "";
  }
  const div = document.createElement("div");
  div.textContent = String(text);
  return div.innerHTML;
}

/**
 * Injected WASM resolver. Set via `setBrefResolver` when the BeliefBaseWasm
 * instance is available.
 * @type {((bid: string) => string|null)|null}
 */
let _brefResolver = null;

/**
 * Inject the WASM-backed bref resolver. Call this once after the
 * BeliefBaseWasm instance is ready:
 *   `setBrefResolver((bid) => bb.get_bref_from_bid(bid))`
 * @param {(bid: string) => string|null} resolver
 */
export function setBrefResolver(resolver) {
  _brefResolver = resolver;
}

/**
 * Extract the bref from a BID string.
 *
 * Uses the WASM resolver (correct UUIDv5-based computation) when available.
 * Falls back to last-12-hex-chars extraction when no resolver is set.
 *
 * @param {string} bid - BID string
 * @returns {string} bref or empty string if invalid
 */
export function brefFromBid(bid) {
  if (!bid || typeof bid !== "string") return "";
  if (_brefResolver) {
    const result = _brefResolver(bid);
    if (result) return result;
  }
  // Fallback: last 12 hex chars (incorrect but functional for display).
  const hex = bid.replace(/-/g, "");
  return hex.slice(-12);
}

/**
 * Format BID for display — shows first 8 and last 4 characters.
 * Full BID is preserved in the value; this is display-only truncation.
 * @param {string} bid - BID string
 * @returns {string} Formatted BID (e.g. "abcd1234...ef90")
 */
export function formatBid(bid) {
  if (!bid || typeof bid !== "string") {
    return "";
  }
  if (bid.length <= 13) {
    return bid;
  }
  return `${bid.substring(0, 8)}...${bid.substring(bid.length - 4)}`;
}

/**
 * Copy text to clipboard and show a brief toast notification.
 *
 * The toast appears at the bottom-center of the viewport and auto-dismisses
 * after 1.5 seconds. Only one toast is shown at a time — calling this while
 * a previous toast is visible replaces it.
 *
 * @param {string} text - Content to copy
 * @param {string} message - Toast message to display on success (e.g. "Link copied")
 * @returns {Promise<boolean>} true if the copy succeeded
 */
export async function copyToClipboard(text, message) {
  try {
    await navigator.clipboard.writeText(text);
  } catch {
    return false;
  }
  showToast(message);
  return true;
}

/**
 * Show a brief toast notification at the bottom of the viewport.
 * Replaces any currently visible toast.
 * @param {string} message
 * @param {number} [durationMs=1500]
 */
function showToast(message, durationMs = 1500) {
  const existing = document.querySelector(".noet-toast");
  if (existing) existing.remove();

  const toast = document.createElement("div");
  toast.className = "noet-toast";
  toast.textContent = message;
  document.body.appendChild(toast);

  // Force reflow so the transition triggers
  toast.offsetHeight; // eslint-disable-line no-unused-expressions
  toast.classList.add("noet-toast--visible");

  setTimeout(() => {
    toast.classList.remove("noet-toast--visible");
    toast.addEventListener("transitionend", () => toast.remove(), { once: true });
    // Fallback removal if transitionend doesn't fire (e.g. no CSS loaded)
    setTimeout(() => toast.remove(), 300);
  }, durationMs);
}
