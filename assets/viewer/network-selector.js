/**
 * viewer/network-selector.js — Shard drawer UI for memory management
 *
 * Drives the footer drawer that shows loaded networks and memory budget,
 * allowing users to load/unload per-network BeliefBase shards.
 *
 * ## Layout contract (template-responsive.html)
 *
 *   #shard-drawer-tab    — wrapper div in footer (hidden in monolithic mode)
 *   #shard-drawer-toggle — the <button> inside the tab; toggles drawer open/close
 *   #shard-tab-summary   — <span> inside the button; shows compact memory summary
 *   #shard-drawer-body   — fixed panel above footer; populated here
 *
 * ## Usage
 *
 *   // After initializeWasm() resolves:
 *   initNetworkSelector();
 *
 * ## Behavior
 *
 *   Sharded mode  (state.shardManager !== null):
 *     - Shows the drawer tab in the footer
 *     - Populates the drawer body with the network list + memory bar
 *     - Keeps the tab summary ("3 loaded · 42 / 200 MB") up to date
 *     - Drawer starts collapsed; click the tab to expand
 *
 *   Monolithic mode (state.shardManager === null):
 *     - Tab is still shown; summary reads "monolithic"
 *     - Drawer body explains that all data is loaded and no management is needed
 *
 * ## Animation
 *
 *   The drawer body uses CSS transitions on `transform` + `opacity`.
 *   Because `[hidden]` sets `display:none` we use a two-frame open sequence:
 *     1. Remove [hidden] → display:block but still translated/invisible
 *     2. rAF → remove translate/opacity via class, transition plays
 *   Close reverses this: add closing class, wait for transition, then set [hidden].
 *
 * ## References
 *
 * - docs/design/search_and_sharding.md §6 — Memory budget model
 * - Issue 50, Phase 4.2 — Network Selector UI
 */

import { state } from "./state.js";

// =============================================================================
// Constants
// =============================================================================

/** localStorage key for persisting which networks the user had loaded. */
const STORAGE_KEY = "noet-loaded-networks";

/** CSS class added to the drawer body when it is fully open (transition target). */
const OPEN_CLASS = "noet-shard-drawer__body--open";

/** Duration must match the CSS transition on .noet-shard-drawer__body (ms). */
const TRANSITION_MS = 200;

// =============================================================================
// Module state
// =============================================================================

/** Whether the drawer body is currently open (visible). */
let _isOpen = false;

/** Handle returned by setTimeout when closing, used to cancel on re-open. */
let _closeTimer = null;

// =============================================================================
// Public API
// =============================================================================

/**
 * Initialize the shard drawer UI.
 *
 * Must be called after `initializeWasm()` resolves. In monolithic mode
 * (state.shardManager is null) this is a no-op — the drawer tab stays hidden.
 *
 * Registers a ShardManager listener so the tab summary and drawer body
 * re-render automatically when shards are loaded or unloaded.
 */
export function initNetworkSelector() {
  const tab = document.getElementById("shard-drawer-tab");
  const toggleBtn = document.getElementById("shard-drawer-toggle");
  const body = document.getElementById("shard-drawer-body");

  if (!tab || !toggleBtn || !body) {
    console.warn(
      "[NetworkSelector] Required drawer elements not found in DOM",
      "— expected #shard-drawer-tab, #shard-drawer-toggle, #shard-drawer-body",
    );
    return;
  }

  // Always show the tab — in monolithic mode it still provides status info.
  tab.hidden = false;
  _renderBody(body);
  _renderTabSummary();

  // Wire up the toggle button.
  toggleBtn.addEventListener("click", () => {
    if (_isOpen) {
      _closeDrawer(toggleBtn, body);
    } else {
      _openDrawer(toggleBtn, body);
    }
  });

  // Close drawer when clicking outside (but not on the toggle itself).
  document.addEventListener("click", (e) => {
    if (!_isOpen) return;
    if (e.target.closest("#shard-drawer-body")) return;
    if (e.target.closest("#shard-drawer-tab")) return;
    _closeDrawer(toggleBtn, body);
  });

  // Close on Escape.
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape" && _isOpen) {
      _closeDrawer(toggleBtn, body);
      toggleBtn.focus();
    }
  });

  // Re-render whenever a shard is loaded or unloaded.
  state.shardManager.addListener(() => {
    _renderTabSummary();
    if (_isOpen) {
      _renderBody(body);
    }
  });
}

// =============================================================================
// Drawer open / close
// =============================================================================

/**
 * Open the drawer with a slide-up animation.
 *
 * Sequence:
 *   1. Remove [hidden] — element enters the render tree (still translated)
 *   2. rAF — browser has painted; now add OPEN_CLASS so transition fires
 *
 * @param {HTMLButtonElement} toggleBtn
 * @param {HTMLElement} body
 */
function _openDrawer(toggleBtn, body) {
  if (_closeTimer !== null) {
    clearTimeout(_closeTimer);
    _closeTimer = null;
  }

  _isOpen = true;
  toggleBtn.setAttribute("aria-expanded", "true");

  // Ensure the body is freshly rendered before showing.
  _renderBody(body);

  // Step 1: make element visible (but still in start-of-transition state).
  body.hidden = false;

  // Step 2: next frame — add class that triggers the CSS transition.
  requestAnimationFrame(() => {
    requestAnimationFrame(() => {
      body.classList.add(OPEN_CLASS);
    });
  });
}

/**
 * Close the drawer with a slide-down animation.
 *
 * Sequence:
 *   1. Remove OPEN_CLASS — CSS transition plays (translate + opacity)
 *   2. After transition ends — set [hidden] to remove from render tree
 *
 * @param {HTMLButtonElement} toggleBtn
 * @param {HTMLElement} body
 */
function _closeDrawer(toggleBtn, body) {
  _isOpen = false;
  toggleBtn.setAttribute("aria-expanded", "false");

  body.classList.remove(OPEN_CLASS);

  _closeTimer = setTimeout(() => {
    body.hidden = true;
    _closeTimer = null;
  }, TRANSITION_MS);
}

// =============================================================================
// Rendering — body
// =============================================================================

/**
 * Render (or re-render) the full drawer body content.
 *
 * @param {HTMLElement} body
 */
function _renderBody(body) {
  const manager = state.shardManager;

  if (!manager) {
    // Monolithic mode — all data is loaded; no per-network controls.
    body.innerHTML = `
      <div class="noet-shard-drawer__header">
        <h2 class="noet-shard-drawer__title">Networks</h2>
        <span class="noet-shard-drawer__header-hint">Monolithic mode</span>
      </div>
      <div class="noet-shard-drawer__content">
        <p class="noet-shard-drawer__monolithic-note">
          All network data is bundled into a single file and loaded at startup.
          Per-network memory management activates automatically when the export
          exceeds 10 MB.
        </p>
      </div>
    `;
    return;
  }

  const { networks } = manager.manifest;
  const usage = manager.getMemoryUsage();

  body.innerHTML = `
    <div class="noet-shard-drawer__header">
      <h2 class="noet-shard-drawer__title">Networks</h2>
      <span class="noet-shard-drawer__header-hint">
        Check a network to load its data into memory
      </span>
    </div>
    <div class="noet-shard-drawer__content">
      <ul class="noet-network-selector__list" role="list">
        ${networks.map((meta) => _renderNetworkItem(meta, manager)).join("")}
      </ul>
    </div>
    <div class="noet-shard-drawer__footer">
      ${_renderMemoryBar(usage)}
    </div>
  `;

  // Wire up checkboxes after innerHTML is set.
  const checkboxes = body.querySelectorAll(".noet-network-selector__checkbox");
  checkboxes.forEach((checkbox) => {
    checkbox.addEventListener("change", (e) => {
      const bref = e.target.dataset.bref;
      if (!bref) return;
      if (e.target.checked) {
        _handleLoad(bref, checkbox);
      } else {
        _handleUnload(bref, checkbox);
      }
    });
  });
}

/**
 * Render one network list item.
 *
 * @param {import('./shard-manager.js').NetworkShardMeta} meta
 * @param {import('./shard-manager.js').ShardManager} manager
 * @returns {string} HTML string
 */
function _renderNetworkItem(meta, manager) {
  const loaded = manager.isNetworkLoaded(meta.bref);
  const canLoad = loaded || manager.canLoadNetwork(meta);
  const sizeMb = meta.estimated_size_mb.toFixed(1);
  const disabledAttr = canLoad ? "" : "disabled";
  const checkedAttr = loaded ? "checked" : "";
  const itemClass = loaded
    ? "noet-network-selector__item noet-network-selector__item--loaded"
    : "noet-network-selector__item";
  const budgetNote = canLoad ? "" : " (memory budget exceeded)";
  const title = `${meta.title} — ${sizeMb} MB${budgetNote}`;

  return `
    <li class="${itemClass}">
      <label class="noet-network-selector__label" title="${_escHtml(title)}">
        <input
          type="checkbox"
          class="noet-network-selector__checkbox"
          data-bref="${_escHtml(meta.bref)}"
          ${checkedAttr}
          ${disabledAttr}
          aria-label="Load network: ${_escHtml(meta.title)}"
        />
        <span class="noet-network-selector__name">${_escHtml(meta.title)}</span>
        <span class="noet-network-selector__meta">
          ${meta.node_count} nodes · ${sizeMb} MB
        </span>
      </label>
    </li>
  `;
}

/**
 * Render the memory usage bar.
 *
 * @param {{ usedMb: number, budgetMb: number, percent: number, warning: string|null }} usage
 * @returns {string} HTML string
 */
function _renderMemoryBar(usage) {
  const pct = Math.min(100, Math.round(usage.percent * 100));
  const barClass =
    usage.warning === "critical"
      ? "noet-memory-bar__fill noet-memory-bar__fill--critical"
      : usage.warning === "warn"
        ? "noet-memory-bar__fill noet-memory-bar__fill--warn"
        : "noet-memory-bar__fill";

  const label = `${usage.usedMb.toFixed(0)} / ${usage.budgetMb.toFixed(0)} MB`;
  const ariaLabel = `Memory usage: ${label} (${pct}%)`;

  return `
    <div class="noet-memory-bar" role="group" aria-label="${ariaLabel}">
      <div class="noet-memory-bar__track" title="${ariaLabel}">
        <div class="${barClass}" style="width: ${pct}%"></div>
      </div>
      <span class="noet-memory-bar__label">${_escHtml(label)}</span>
    </div>
  `;
}

// =============================================================================
// Rendering — tab summary (compact, always visible when sharded)
// =============================================================================

/**
 * Update the compact memory summary in the footer tab button.
 * Format: "N loaded · X / 200 MB"
 *
 * Also sets `data-memory-warning` on the toggle button so CSS can tint the
 * summary text amber/red at high usage.
 */
function _renderTabSummary() {
  const summaryEl = document.getElementById("shard-tab-summary");
  const toggleBtn = document.getElementById("shard-drawer-toggle");
  if (!summaryEl || !toggleBtn) return;

  const manager = state.shardManager;
  if (!manager) {
    summaryEl.textContent = "monolithic";
    toggleBtn.dataset.memoryWarning = "";
    return;
  }

  const loaded = manager.getLoadedNetworks();
  const usage = manager.getMemoryUsage();
  const loadedCount = loaded.length;
  const label = `${loadedCount} loaded · ${usage.usedMb.toFixed(0)} / ${usage.budgetMb.toFixed(0)} MB`;

  summaryEl.textContent = label;
  toggleBtn.dataset.memoryWarning = usage.warning ?? "";
}

// =============================================================================
// Load / unload handlers
// =============================================================================

/**
 * Load a network shard. Sets the checkbox to indeterminate while loading.
 *
 * @param {string} bref
 * @param {HTMLInputElement} checkbox
 */
async function _handleLoad(bref, checkbox) {
  const manager = state.shardManager;
  if (!manager) return;

  // Show loading state.
  checkbox.indeterminate = true;
  checkbox.disabled = true;

  try {
    await manager.loadNetwork(bref);
    _persistLoadedNetworks();
    // Re-render triggered by ShardManager listener registered in initNetworkSelector.
  } catch (err) {
    console.error(`[NetworkSelector] Failed to load network '${bref}':`, err);
    checkbox.indeterminate = false;
    checkbox.checked = false;
    checkbox.disabled = false;
    _showError(bref, err.message);
  }
}

/**
 * Unload a network shard. Sets the checkbox to indeterminate while unloading.
 *
 * @param {string} bref
 * @param {HTMLInputElement} checkbox
 */
async function _handleUnload(bref, checkbox) {
  const manager = state.shardManager;
  if (!manager) return;

  checkbox.indeterminate = true;
  checkbox.disabled = true;

  try {
    await manager.unloadNetwork(bref);
    _persistLoadedNetworks();
  } catch (err) {
    console.error(`[NetworkSelector] Failed to unload network '${bref}':`, err);
    checkbox.indeterminate = false;
    checkbox.checked = true;
    checkbox.disabled = false;
  }
}

// =============================================================================
// Error feedback
// =============================================================================

/**
 * Show a brief error message inside the drawer body.
 * Auto-dismissed after 5 seconds.
 *
 * @param {string} bref
 * @param {string} message
 */
function _showError(bref, message) {
  const body = document.getElementById("shard-drawer-body");
  if (!body) return;

  // Remove any existing error banner.
  body.querySelector(".noet-shard-drawer__error")?.remove();

  const errEl = document.createElement("p");
  errEl.className = "noet-shard-drawer__error";
  errEl.textContent = `Failed to load '${bref}': ${message}`;

  // Insert after the header, before the content.
  const content = body.querySelector(".noet-shard-drawer__content");
  if (content) {
    body.insertBefore(errEl, content);
  } else {
    body.appendChild(errEl);
  }

  setTimeout(() => errEl.remove(), 5000);
}

// =============================================================================
// Persistence
// =============================================================================

/**
 * Persist the current set of loaded networks to localStorage so that on next
 * page load the same networks could be restored (restoration is left for a
 * future enhancement — currently only the global shard + entry network are
 * loaded on init).
 */
function _persistLoadedNetworks() {
  const manager = state.shardManager;
  if (!manager) return;
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(manager.getLoadedNetworks()));
  } catch (_) {
    // localStorage not available — ignore.
  }
}

// =============================================================================
// Utilities
// =============================================================================

/**
 * Minimal HTML escaping for values interpolated into innerHTML.
 * @param {string} str
 * @returns {string}
 */
function _escHtml(str) {
  return String(str)
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}
