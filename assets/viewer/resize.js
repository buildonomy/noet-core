/**
 * viewer/resize.js — Resize/collapse handles for panels and content gutters
 *
 * Panel collapse buttons (#nav-collapse, #metadata-collapse):
 *   Short click (< DRAG_THRESHOLD px) → toggles collapse.
 *   Drag (≥ DRAG_THRESHOLD px) → resizes the panel width.
 *
 * Content gutter handles (.noet-content__inner ::before/::after):
 *   Drag the left/right edge of the content paper to adjust the gutter
 *   width between the panel and the content.
 *
 * Panel widths and gutter sizes are applied as CSS custom properties on
 * document.documentElement. Content layout is driven by CSS calc() rules
 * that consume these variables.
 *
 * State is persisted to localStorage under "noet-resize-state".
 */

// =============================================================================
// Constants
// =============================================================================

const STORAGE_KEY = "noet-resize-state";

// Minimum gutter matches CSS --noet-panel-gutter (--size-3 ≈ 16px)
const MIN_GUTTER = 16;

const DEFAULTS = {
  navWidth: 280, // px
  metadataWidth: 320, // px
  gutterLeft: MIN_GUTTER, // px
  gutterRight: MIN_GUTTER, // px
};

const LIMITS = {
  navWidth: { min: 160, max: 520 },
  metadataWidth: { min: 200, max: 560 },
  gutterLeft: { min: MIN_GUTTER, max: 600 },
  gutterRight: { min: MIN_GUTTER, max: 600 },
};

const DRAG_THRESHOLD = 4; // px — movement below this is treated as a click

// =============================================================================
// State
// =============================================================================

/** @type {{ navWidth: number, metadataWidth: number, gutterLeft: number, gutterRight: number }} */
let sizes = { ...DEFAULTS };

// =============================================================================
// Persistence
// =============================================================================

function loadSizes() {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (raw) {
      const parsed = JSON.parse(raw);
      sizes = {
        navWidth: clamp(parsed.navWidth ?? DEFAULTS.navWidth, LIMITS.navWidth),
        metadataWidth: clamp(
          parsed.metadataWidth ?? DEFAULTS.metadataWidth,
          LIMITS.metadataWidth,
        ),
        gutterLeft: clamp(parsed.gutterLeft ?? DEFAULTS.gutterLeft, LIMITS.gutterLeft),
        gutterRight: clamp(
          parsed.gutterRight ?? DEFAULTS.gutterRight,
          LIMITS.gutterRight,
        ),
      };
    }
  } catch (e) {
    console.warn("[Noet] Failed to load resize state:", e);
  }
}

function saveSizes() {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(sizes));
  } catch (e) {
    console.warn("[Noet] Failed to save resize state:", e);
  }
}

// =============================================================================
// CSS application
// =============================================================================

function applySizes() {
  const root = document.documentElement;
  root.style.setProperty("--noet-nav-width", `${sizes.navWidth}px`);
  root.style.setProperty("--noet-metadata-width", `${sizes.metadataWidth}px`);
  root.style.setProperty("--noet-gutter-left", `${sizes.gutterLeft}px`);
  root.style.setProperty("--noet-gutter-right", `${sizes.gutterRight}px`);
}

// =============================================================================
// Helpers
// =============================================================================

/** @param {number} val @param {{ min: number, max: number }} limit */
function clamp(val, limit) {
  return Math.max(limit.min, Math.min(limit.max, val));
}

function isDesktop() {
  return window.matchMedia("(min-width: 1024px)").matches;
}

// =============================================================================
// Drag logic
// =============================================================================

/**
 * Attach unified drag+click behaviour to a collapse button.
 *
 * Short click (pointer moves < DRAG_THRESHOLD px): calls onShortClick().
 * Drag (pointer moves >= DRAG_THRESHOLD px): resizes via panel width delta.
 *
 * @param {HTMLElement} handle
 * @param {"nav"|"metadata"} kind
 * @param {() => void} onShortClick  — called on short click
 */
function attachCollapseResizeHandler(handle, kind, onShortClick) {
  let startX = 0;
  let startSize = 0;
  let didDrag = false;

  function onPointerMove(e) {
    const dx = e.clientX - startX;
    if (!didDrag && Math.abs(dx) < DRAG_THRESHOLD) return;
    didDrag = true;

    let next;
    if (kind === "nav") {
      next = clamp(startSize + dx, LIMITS.navWidth);
      sizes.navWidth = next;
    } else {
      next = clamp(startSize - dx, LIMITS.metadataWidth);
      sizes.metadataWidth = next;
    }
    applySizes();
  }

  function onPointerUp(e) {
    handle.classList.remove("is-dragging");
    document.documentElement.classList.remove("is-resizing");
    handle.releasePointerCapture(e.pointerId);
    document.removeEventListener("pointermove", onPointerMove);
    document.removeEventListener("pointerup", onPointerUp);

    if (!didDrag) {
      onShortClick();
    } else {
      saveSizes();
    }
  }

  handle.addEventListener("pointerdown", (e) => {
    if (!isDesktop()) return;
    e.preventDefault();

    startX = e.clientX;
    startSize = kind === "nav" ? sizes.navWidth : sizes.metadataWidth;
    didDrag = false;

    handle.classList.add("is-dragging");
    document.documentElement.classList.add("is-resizing");
    handle.setPointerCapture(e.pointerId);
    document.addEventListener("pointermove", onPointerMove);
    document.addEventListener("pointerup", onPointerUp);

    // Suppress the synthetic click event so the existing click handler
    // on this button doesn't double-fire on short press.
    // (e.preventDefault() on pointerdown does NOT suppress click in browsers.)
    handle.addEventListener(
      "click",
      function suppressClick(e) {
        e.stopImmediatePropagation();
        handle.removeEventListener("click", suppressClick);
      },
      { capture: true },
    );
  });
}

/**
 * Attach drag behaviour to an existing gutter handle element.
 *
 * @param {HTMLElement} handle — the .noet-gutter-handle element
 * @param {"left"|"right"} side
 * @param {"gutterLeft"|"gutterRight"} sizeKey
 */
function attachGutterHandleDrag(handle, side, sizeKey) {
  const limit = sizeKey === "gutterLeft" ? LIMITS.gutterLeft : LIMITS.gutterRight;

  let startX = 0;
  let startSize = 0;

  function onPointerMove(e) {
    const dx = e.clientX - startX;
    const delta = side === "left" ? dx : -dx;
    sizes[sizeKey] = clamp(startSize + delta, limit);
    applySizes();
  }

  function onPointerUp(e) {
    document.documentElement.classList.remove("is-resizing");
    handle.releasePointerCapture(e.pointerId);
    document.removeEventListener("pointermove", onPointerMove);
    document.removeEventListener("pointerup", onPointerUp);
    saveSizes();
  }

  handle.addEventListener("pointerdown", (e) => {
    if (!isDesktop()) return;
    e.preventDefault();

    startX = e.clientX;
    startSize = sizes[sizeKey];

    document.documentElement.classList.add("is-resizing");
    handle.setPointerCapture(e.pointerId);
    document.removeEventListener("pointermove", onPointerMove);
    document.removeEventListener("pointerup", onPointerUp);
    document.addEventListener("pointermove", onPointerMove);
    document.addEventListener("pointerup", onPointerUp);
  });
}

// =============================================================================
// Gutter absorption on panel collapse/expand
// =============================================================================

/**
 * Absorb a panel's width into the corresponding gutter so content doesn't
 * reflow when the panel collapses. On expand, subtract the panel width back
 * out (clamped to MIN_GUTTER).
 *
 * @param {"nav"|"metadata"} panel
 * @param {boolean} collapsed — true if the panel is now collapsed
 */
export function adjustGutterForCollapse(panel, collapsed) {
  if (panel === "nav") {
    if (collapsed) {
      sizes.gutterLeft = sizes.gutterLeft + sizes.navWidth;
    } else {
      sizes.gutterLeft = Math.max(MIN_GUTTER, sizes.gutterLeft - sizes.navWidth);
    }
  } else {
    if (collapsed) {
      sizes.gutterRight = sizes.gutterRight + sizes.metadataWidth;
    } else {
      sizes.gutterRight = Math.max(MIN_GUTTER, sizes.gutterRight - sizes.metadataWidth);
    }
  }
  applySizes();
  saveSizes();
}

// =============================================================================
// Public init
// =============================================================================

/**
 * Initialise all resize/collapse handles.
 *
 * Accepts toggle callbacks from viewer.js to avoid a circular import between
 * resize.js and panels.js.
 *
 * @param {{ toggleNav: () => void, toggleMetadata: () => void }} toggles
 */
export function initResizeHandles(toggles) {
  const navCollapse = document.getElementById("nav-collapse");
  const metadataCollapse = document.getElementById("metadata-collapse");
  const gutterLeft = document.querySelector(".noet-gutter-handle--left");
  const gutterRight = document.querySelector(".noet-gutter-handle--right");

  loadSizes();
  applySizes();

  if (navCollapse) {
    attachCollapseResizeHandler(navCollapse, "nav", toggles.toggleNav);
  }
  if (metadataCollapse) {
    attachCollapseResizeHandler(metadataCollapse, "metadata", toggles.toggleMetadata);
  }
  if (gutterLeft) {
    attachGutterHandleDrag(gutterLeft, "left", "gutterLeft");
  }
  if (gutterRight) {
    attachGutterHandleDrag(gutterRight, "right", "gutterRight");
  }
}
