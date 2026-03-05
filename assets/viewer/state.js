/**
 * viewer/state.js — Shared mutable state and DOM references
 *
 * All modules import from this file to access shared state.
 * State is mutated in-place on the exported `state` object so that
 * all modules always see the current value (live reference semantics).
 *
 * DOM references are populated once by initializeDOMReferences() in viewer.js
 * and then read by any module that needs them.
 *
 * Callback registry breaks circular import cycles: modules that need to call
 * across the routing↔metadata↔navigation boundary register their functions
 * here at startup rather than importing each other directly.
 */

// =============================================================================
// Shared mutable state
// =============================================================================

export const state = {
  // ---- DOM References ----

  /** @type {HTMLElement|null} */
  containerElement: null,

  /** @type {HTMLElement|null} */
  navElement: null,

  /** @type {HTMLElement|null} */
  navContent: null,

  /** @type {HTMLElement|null} */
  navError: null,

  /** @type {HTMLElement|null} */
  contentElement: null,

  /** @type {HTMLElement|null} */
  metadataPanel: null,

  /** @type {HTMLElement|null} */
  metadataContent: null,

  /** @type {HTMLElement|null} */
  metadataError: null,

  /** @type {HTMLElement|null} */
  graphContainer: null,

  /** @type {HTMLElement|null} */
  graphCanvas: null,

  /** @type {HTMLElement|null} */
  footerElement: null,

  /** @type {HTMLSelectElement|null} */
  themeSelect: null,

  /** @type {HTMLButtonElement|null} */
  metadataClose: null,

  /** @type {HTMLButtonElement|null} */
  graphClose: null,

  /** @type {HTMLButtonElement|null} */
  navCollapseBtn: null,

  /** @type {HTMLButtonElement|null} */
  metadataCollapseBtn: null,

  /** @type {HTMLLinkElement|null} */
  themeLightLink: null,

  /** @type {HTMLLinkElement|null} */
  themeDarkLink: null,

  // ---- Application State ----

  /** Document metadata loaded from embedded JSON */
  documentMetadata: null,

  /** Current theme: "system", "light", or "dark" */
  currentTheme: "system",

  /**
   * Currently selected node BID (two-click pattern).
   * First click sets this; second click on same element navigates.
   */
  selectedNodeBid: null,

  /** WASM module instance (result of `await import(...)`) */
  wasmModule: null,

  /** BeliefBaseWasm instance */
  beliefbase: null,

  /** Navigation tree data — NavTree { nodes: Map, roots: Array } from get_nav_tree() */
  navTree: null,

  /** Set of expanded node BIDs in the navigation tree */
  expandedNodes: new Set(),

  /** True until the first nav render; used to auto-expand roots once */
  isFirstNavRender: true,

  /**
   * The docPathKey of the currently loaded document (e.g. "file1.html" or
   * "subnet1/subnet1_file1.html"), or null if no document has been successfully
   * fetched yet. Null serves the same purpose as the former `documentLoaded`
   * boolean — distinguishing the shell's placeholder <article> (present on
   * force-refresh before loadDocument() completes) from real loaded content.
   * Set on successful loadDocument(); reset to null at the start of each load.
   * Used by handleHashChange for same-doc short-circuit without re-reading
   * window.location.hash (which already reflects the new target by the time
   * hashchange fires).
   */
  currentDocPath: null,

  /**
   * BID of the currently loaded document. Set alongside currentDocPath on
   * successful loadDocument(); reset to null at the start of each load.
   * Used by the nav tree to highlight the current document node distinctly
   * from the selectedNodeBid (metadata panel focus).
   */
  currentDocBid: null,

  /**
   * Asset/href node BID to highlight in content once the next document load
   * completes. Set by the metadata panel external link handler when the owner
   * document is not currently loaded. Consumed by loadDocument() after content
   * is injected and post-processed.
   */
  pendingHighlightPath: null,

  /**
   * BID to show in the metadata panel once the next navigation completes.
   * Set before a hash change is triggered so the handler can pick it up.
   */
  pendingMetadataBid: null,

  /** Panel collapse state — persisted to localStorage */
  panelState: {
    navCollapsed: false,
    metadataCollapsed: true, // Start collapsed
  },
};

// =============================================================================
// Cross-module callback registry
//
// Modules that would otherwise create circular imports register their
// functions here at startup (in viewer.js DOMContentLoaded). Callers
// invoke them via this registry instead of importing directly.
//
// Current cycles broken this way:
//   routing  → metadata  (showMetadataPanel called from loadDocument / navigateToSection)
//   routing  → navigation (updateNavTreeHighlight called after loadDocument)
//   metadata → routing   (navigateToLink called from attachMetadataLinkHandlers)
//   content  → metadata  (showMetadataPanel called from image click handlers)
// =============================================================================

export const callbacks = {
  /** @type {((nodeBid: string) => void)|null} */
  showMetadataPanel: null,

  /** @type {(() => void)|null} */
  updateNavTreeHighlight: null,

  /**
   * @type {((href: string, link: HTMLElement, targetBid: string|null) => void)|null}
   */
  navigateToLink: null,

  /**
   * Highlight an external asset (image, PDF link, or href anchor) in the
   * currently loaded content and scroll it into view.
   * @type {((assetPath: string) => void)|null}
   */
  highlightExternalInContent: null,
};
