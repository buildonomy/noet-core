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
