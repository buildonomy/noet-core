/**
 * viewer/version-selector.js — Version switcher dropdown
 *
 * Reads a `versions.json` manifest and renders a `<select>` dropdown in the
 * navigation header, allowing users to switch between documentation versions.
 *
 * ## Contract
 *
 *   The consuming project's CI is responsible for:
 *   - Building each version into `output/v/<tag>/` with the correct `--base-url`
 *   - Producing a `versions.json` manifest at the site root
 *
 *   Expected URL structure: `https://example.com/<base>/v/<version>/pages/...`
 *   The `v/` prefix is part of the contract.
 *
 * ## `versions.json` schema
 *
 *   {
 *     "versions": [
 *       { "label": "Latest (main)", "path": "v/latest/" },
 *       { "label": "v2.0.0",        "path": "v/v2.0.0/" }
 *     ]
 *   }
 *
 * ## Behavior
 *
 *   - On init, resolves the manifest URL by finding `/v/<version>/` in the
 *     current pathname and walking up to the site root.
 *   - If the URL pattern is not found (single-version deployment), does nothing.
 *   - If the fetch fails or the manifest is invalid, does nothing.
 *   - On selection change, navigates to the equivalent page in the selected
 *     version, preserving the hash fragment.
 *
 * ## Usage
 *
 *   import { initVersionSelector } from "./viewer/version-selector.js";
 *   initVersionSelector();
 *
 * ## References
 *
 * - Issue 73: Version Selector UI
 */

// =============================================================================
// Constants
// =============================================================================

/**
 * Regex to match `/v/<version>/` in the pathname.
 * Captures everything before `v/` as group 1 (the base path prefix).
 */
const VERSION_PATH_RE = /^(.*\/?)v\/([^/]+)\//;

// =============================================================================
// Public API
// =============================================================================

/**
 * Initialize the version selector dropdown.
 *
 * Safe to call unconditionally — returns silently when no versioned
 * deployment is detected (no `/v/` in the URL, or manifest not found).
 */
export async function initVersionSelector() {
  const pathname = window.location.pathname;
  const match = VERSION_PATH_RE.exec(pathname);

  if (!match) {
    // Not a versioned deployment — nothing to do.
    return;
  }

  const basePrefix = match[1]; // e.g. "/" or "/repo/"
  const currentVersion = match[2]; // e.g. "latest" or "v2.0.0"
  const manifestUrl = basePrefix + "versions.json";

  let manifest;
  try {
    const resp = await fetch(manifestUrl);
    if (!resp.ok) {
      return; // No manifest — hide selector silently.
    }
    manifest = await resp.json();
  } catch {
    return; // Network error or invalid JSON — hide selector silently.
  }

  if (!manifest || !Array.isArray(manifest.versions) || manifest.versions.length < 2) {
    // Need at least 2 versions for a selector to be useful.
    return;
  }

  _renderSelector(manifest.versions, currentVersion, basePrefix);
}

// =============================================================================
// Internal helpers
// =============================================================================

/**
 * Build and insert the `<select>` element into `.noet-nav-header`.
 *
 * @param {Array<{label: string, path: string}>} versions
 * @param {string} currentVersion - The version segment from the current URL.
 * @param {string} basePrefix - Path prefix before `v/` (e.g. "/" or "/repo/").
 */
function _renderSelector(versions, currentVersion, basePrefix) {
  const container = document.querySelector(".noet-nav-header");
  if (!container) {
    return;
  }

  const select = document.createElement("select");
  select.classList.add("noet-version-selector");
  select.setAttribute("aria-label", "Documentation version");

  versions.forEach((v) => {
    const opt = document.createElement("option");
    opt.value = v.path;
    opt.textContent = v.label;

    // Match current version by checking if the path contains the version segment.
    // e.g. path "v/latest/" matches currentVersion "latest"
    if (v.path === "v/" + currentVersion + "/") {
      opt.selected = true;
    }

    select.appendChild(opt);
  });

  select.addEventListener("change", () => {
    _navigateToVersion(select.value, basePrefix);
  });

  // Insert before the search wrapper (first child of nav-header).
  container.insertBefore(select, container.firstChild);
}

/**
 * Navigate to the selected version, preserving the current hash fragment.
 *
 * Replaces the `/v/<old>/` segment in the pathname with the new version's path.
 *
 * @param {string} newPath - The selected version's `path` value (e.g. "v/v2.0.0/").
 * @param {string} basePrefix - Path prefix before `v/` (e.g. "/" or "/repo/").
 */
function _navigateToVersion(newPath, basePrefix) {
  const hash = window.location.hash || "";
  const pathname = window.location.pathname;

  // Replace the /v/<version>/... portion with the new path, keeping everything after.
  const afterVersion = pathname.replace(VERSION_PATH_RE, "");
  const newUrl = basePrefix + newPath + afterVersion + hash;

  window.location.href = newUrl;
}
