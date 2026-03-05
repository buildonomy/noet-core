/**
 * viewer/resize.js — Draggable resize handles for nav, metadata, and content panels
 *
 * Three handles (desktop only, min-width 1024px):
 *
 *   nav handle      (#nav-resize)      — drag horizontally to change --noet-nav-width
 *   metadata handle (#metadata-resize) — drag horizontally to change --noet-metadata-width
 *   content handle  (#content-resize)  — drag horizontally to change --noet-content-max-width
 *
 * Widths are applied as CSS custom properties on document.documentElement so that
 * all rules using var(--noet-nav-width, …) etc. pick them up automatically.
 *
 * State is persisted to localStorage under "noet-resize-state".
 */

// =============================================================================
// Constants
// =============================================================================

const STORAGE_KEY = "noet-resize-state";

const DEFAULTS = {
  navWidth: 280, // px
  metadataWidth: 320, // px
  contentMaxWidth: 720, // px  (45rem at 16px base)
};

const LIMITS = {
  navWidth: { min: 160, max: 520 },
  metadataWidth: { min: 200, max: 560 },
  contentMaxWidth: { min: 320, max: 1200 },
};

// =============================================================================
// State
// =============================================================================

/** @type {{ navWidth: number, metadataWidth: number, contentMaxWidth: number }} */
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
        metadataWidth: clamp(parsed.metadataWidth ?? DEFAULTS.metadataWidth, LIMITS.metadataWidth),
        contentMaxWidth: clamp(
          parsed.contentMaxWidth ?? DEFAULTS.contentMaxWidth,
          LIMITS.contentMaxWidth,
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
  root.style.setProperty("--noet-content-max-width", `${sizes.contentMaxWidth}px`);
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
 * Attach pointer-drag resize behaviour to a single handle element.
 *
 * @param {HTMLElement} handle      - The draggable button element
 * @param {"nav"|"metadata"|"content"} kind
 */
function attachDragHandler(handle, kind) {
  let startX = 0;
  let startSize = 0;

  function onPointerMove(e) {
    const dx = e.clientX - startX;
    let next;

    if (kind === "nav") {
      // Dragging right → wider nav
      next = clamp(startSize + dx, LIMITS.navWidth);
      sizes.navWidth = next;
    } else if (kind === "metadata") {
      // Dragging left → wider metadata (handle is on the left edge of the panel)
      next = clamp(startSize - dx, LIMITS.metadataWidth);
      sizes.metadataWidth = next;
    } else {
      // content: handle is on the right edge of content__inner
      // dragging right → wider content area (increase max-width)
      // We track viewport-relative position: wider when pointer moves right
      next = clamp(startSize + dx, LIMITS.contentMaxWidth);
      sizes.contentMaxWidth = next;
    }

    applySizes();
  }

  function onPointerUp(e) {
    handle.classList.remove("is-dragging");
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
    startSize =
      kind === "nav"
        ? sizes.navWidth
        : kind === "metadata"
          ? sizes.metadataWidth
          : sizes.contentMaxWidth;

    handle.classList.add("is-dragging");
    document.documentElement.classList.add("is-resizing");
    handle.setPointerCapture(e.pointerId);
    document.addEventListener("pointermove", onPointerMove);
    document.addEventListener("pointerup", onPointerUp);
  });
}

// =============================================================================
// Public init
// =============================================================================

/**
 * Initialise all three resize handles.
 * Safe to call before the WASM/nav tree is loaded — only touches the DOM.
 */
export function initResizeHandles() {
  const navHandle = document.getElementById("nav-resize");
  const metadataHandle = document.getElementById("metadata-resize");
  const contentHandle = document.getElementById("content-resize");

  loadSizes();
  applySizes();

  if (navHandle) attachDragHandler(navHandle, "nav");
  if (metadataHandle) attachDragHandler(metadataHandle, "metadata");
  if (contentHandle) attachDragHandler(contentHandle, "content");
}
