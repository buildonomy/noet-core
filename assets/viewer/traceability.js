/**
 * viewer/traceability.js — Traceability View
 *
 * Displays a modal traceability matrix for the selected node's network.
 * Rows are submap entries (ordered by path/sort key); columns are in/out
 * edge counts per WeightKind. A maps_to toggle replaces rows with the sink
 * nodes reached via owned {maps_to} edges.
 *
 * Public API:
 *   openTraceabilityModal(bid, homeNetBid, entryBid)  — open panel for a node
 *   openTraceabilitySearch(query)                     — open panel in search mode
 *   closeTraceabilityModal()                          — close panel
 *
 * WASM calls used:
 *   bb.queryView(spec, viewKey, null)
 *     → view-specific JSON (traceability matrix rows/columns)
 *   bb.get_context(bid)
 *     → NodeContext (used for the metadata panel)
 *   BeliefBaseWasm.parseQuery(grammar)
 *     → QuerySpec (parsed from the query grammar text)
 *   BeliefBaseWasm.export_xlsx(headers, rows)
 *     → Uint8Array (XLSX binary for spreadsheet export)
 */

import { state, callbacks } from "./state.js";
import { escapeHtml, brefFromBid } from "./utils.js";
import { showMetadataPanel, syncTraceabilityBtnState } from "./metadata.js";

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const WEIGHT_KINDS = ["Section", "Epistemic", "Pragmatic"];

// ---------------------------------------------------------------------------
// Module state
// ---------------------------------------------------------------------------

/** @type {HTMLDivElement|null} */
let panelEl = null;

/** @type {string|null} */
let currentHomeNetBid = null;

/** BID of the focused node to scope the submap from; empty string = entire network */
let currentEntryBid = "";

/** View data from the current queryView() call. Shape depends on displayMode. */
let currentViewData = null;

/** Cached QuerySpec from the last parse, for re-query on display mode switch. */
let cachedSpec = null;

/** Which WeightKinds are currently visible */
let visibleKinds = new Set(["Epistemic", "Pragmatic"]);

/** Whether the depth gutter is visible in the table */
let showDepthGutter = false;

/** Whether the pathmap order gutter is visible in the table */
let showOrderGutter = false;

/** Current display mode: "connectivity" (default) or "tape" */
let displayMode = "connectivity";

/** Tape view data from WASM queryView(). Array of entry objects, or null. */
let rawTapeEntries = null;

/** Cached sorted rows for connectivity mode. Built by renderNormalTable(),
 *  consumed by getNavRows() so keyboard navigation matches display order.
 *  Each entry: { row, originalIdx }. */
let sortedNavRows = null;

/** Which tape entry index to display in tape mode (0-based). */
let rawTapeSelectedEntry = 0;

/** Current search query text. The panel always uses query results
 *  via `refreshFromQuery`. When empty/short, the table is simply cleared. */
let searchQuery = "";

/** Network bref filter for search results ("" = all networks) */
let searchNetworkFilter = "";

/** Map of BID → TF-IDF score for the current search results */
let currentSearchScores = new Map();

/** Debounce timer for search input */
let searchDebounceTimer = null;

/** Debounce delay (ms) for search input */
const SEARCH_DEBOUNCE_MS = 300;

/** Max results returned from bb.search() */
const SEARCH_MAX_RESULTS = 200;

/**
 * Named traversal shorthands: verb → { input, kind, output }.
 * Bitfield encoding: Source=bit0, Sink=bit1, Owner=bit2 for roles;
 * Section=bit0, Pragmatic=bit1, Epistemic=bit2 for kinds.
 */
const TRAVERSAL_SHORTHANDS = [
  { name: "composed_of", input: 2, kind: 1, output: 1 },
  { name: "component_of", input: 1, kind: 1, output: 2 },
  { name: "uses", input: 2, kind: 2, output: 1 },
  { name: "used_by", input: 1, kind: 2, output: 2 },
  { name: "draws_from", input: 2, kind: 4, output: 1 },
  { name: "underlies", input: 1, kind: 4, output: 2 },
  { name: "covers", input: 4, kind: 2, output: 3 },
  { name: "halo", input: 7, kind: 7, output: 7 },
];

/**
 * Detect which named traversal shorthand matches the given Traverse object,
 * or return "custom" if none match.
 */
function detectTraversalShorthand(t) {
  if (!t) return "custom";
  for (const sh of TRAVERSAL_SHORTHANDS) {
    if (
      t.input_roles === sh.input &&
      t.kind_filter === sh.kind &&
      t.output_roles === sh.output
    ) {
      return sh.name;
    }
  }
  return "custom";
}

/** The currently parsed QuerySpec object, or null if no valid query. */
let currentSpec = null;

/** Path of the step editor currently open, or null if none. */
let openEditorPath = null;

/** Selected subject nodes for the multi-select subject picker.
 *  Each entry: { bid, title, path }. Empty array = Corpus / implicit. */
let subjectNodes = [];

/** Debounce timer for the subject autocomplete input. */
let subjectDebounceTimer = null;

/**
 * Navigation focus state.
 *
 * focusedRow / focusedCol index into the *rendered* flat row array from the
 * current view data.  Both views produce `{ rows: [{ cells }] }` — navigation
 * works identically across connectivity and tape modes.
 *
 * The renderer stamps `data-row-idx` on each `<tr>` and `data-col-idx` on
 * each `<td>`.  Focus highlight and metadata-panel updates are driven by
 * looking up the cell at (focusedRow, focusedCol) in the view data, not by
 * DOM queries.
 */
let focusedRow = -1;
let focusedCol = 0;

/**
 * Resolve a dot-delimited path to a step in the spec tree.
 * Path format: "idx" or "idx.branch.subIdx.branch.subIdx..."
 * Returns { steps, index, step } or null if invalid.
 */
function resolveStepPath(spec, path) {
  if (!spec || !path) return null;
  const parts = path.split(".");
  let steps = spec.steps;
  if (!steps) return null;
  let idx = parseInt(parts[0], 10);
  if (isNaN(idx) || idx < 0 || idx >= steps.length) return null;

  for (let i = 1; i < parts.length; i += 2) {
    const branch = parts[i]; // "left" or "right"
    const subIdx = parseInt(parts[i + 1], 10);
    if (branch !== "left" && branch !== "right") return null;
    if (isNaN(subIdx)) return null;
    const compose = steps[idx]?.operation?.Compose;
    if (!compose) return null;
    if (!Array.isArray(compose[branch])) compose[branch] = [];
    steps = compose[branch];
    idx = subIdx;
    if (idx < 0 || idx >= steps.length) return null;
  }

  return { steps, index: idx, step: steps[idx] };
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/**
 * Open the traceability modal for a given node.
 * @param {string} bid        - BID of the node whose network to display
 * @param {string} homeNetBid - BID of the node's home network
 */
export async function openTraceabilityModal(bid, homeNetBid, entryBid) {
  if (!state.beliefbase) {
    console.warn("[Traceability] beliefbase not initialized");
    return;
  }

  currentHomeNetBid = homeNetBid;
  currentEntryBid = entryBid || "";
  openEditorPath = null;
  // Set subject from the anchor node.
  const anchorBid = entryBid || homeNetBid;
  subjectNodes = anchorBid
    ? [{ bid: anchorBid, title: labelForBid(anchorBid), path: "" }]
    : [];
  searchQuery = anchorBid ? `bid:${anchorBid} composed_of(*)` : "";
  searchNetworkFilter = "";
  currentSearchScores = new Map();
  visibleKinds = new Set(["Epistemic", "Pragmatic"]);
  displayMode = "connectivity";
  rawTapeEntries = null;
  sortedNavRows = null;
  rawTapeSelectedEntry = 0;

  // Reset navigation focus; renderTable() will apply highlight.
  focusedRow = -1;
  focusedCol = 0;

  ensurePanel();
  renderSkeleton();
  renderSubjectChips();
  syncControls();
  panelEl.classList.add("is-open");
  // On mobile, move panel inside .noet-metadata and add .has-traceability class
  if (window.matchMedia("(max-width: 1023px)").matches) {
    const metadataEl = document.getElementById("metadata-panel");
    if (metadataEl && panelEl.parentElement !== metadataEl) {
      metadataEl.appendChild(panelEl);
      metadataEl.classList.add("has-focused-panel");
    }
  }

  // Show step cards and evaluate the initial query.
  if (searchQuery) {
    const parsed = tryParseQuery(searchQuery);
    if (parsed) {
      renderStepCards(parsed);
      syncSubjectFromSpec(parsed);
    }
    syncControls();
    populateNetworkFilter();
    await refreshFromQuery(searchQueryToGrammar(searchQuery));
  }
  clearDirty();
  syncTraceabilityBtnState();
}

/**
 * Open the traceability panel in search mode with a pre-populated query.
 * Used by the "Open in Search" button on {query} directive results
 * and potentially by the Explore affordance.
 *
 * @param {string} query - Search query to pre-populate.
 */
export async function openTraceabilitySearch(query) {
  if (!state.beliefbase) {
    console.warn("[Traceability] beliefbase not initialized");
    return;
  }

  // Use the current document's network context, or fall back to entry point.
  const entryPoint = state.beliefbase.entryPoint();
  currentHomeNetBid = entryPoint.bid;
  currentEntryBid = "";
  openEditorPath = null;
  searchQuery = query;
  searchNetworkFilter = "";
  currentSearchScores = new Map();
  visibleKinds = new Set(["Epistemic", "Pragmatic"]);
  displayMode = "connectivity";
  rawTapeEntries = null;
  sortedNavRows = null;
  rawTapeSelectedEntry = 0;
  focusedRow = -1;
  focusedCol = 0;

  ensurePanel();
  renderSkeleton();
  panelEl.classList.add("is-open");

  syncControls();
  populateNetworkFilter();

  if (window.matchMedia("(max-width: 1023px)").matches) {
    const metadataEl = document.getElementById("metadata-panel");
    if (metadataEl && panelEl.parentElement !== metadataEl) {
      metadataEl.appendChild(panelEl);
      metadataEl.classList.add("has-focused-panel");
    }
  }

  // Evaluate immediately when opening with a pre-populated query.
  if (searchQuery.length >= 2) {
    await executeQuery();
  }
  syncTraceabilityBtnState();
}

/**
 * Close the traceability panel.
 */
export function closeTraceabilityModal() {
  if (!panelEl) return;
  panelEl.classList.remove("is-open");
  // Restore panel to document body if it was moved into .noet-metadata on mobile
  if (panelEl.parentElement !== document.body) {
    document.body.appendChild(panelEl);
  }
  const metadataEl = document.getElementById("metadata-panel");
  if (metadataEl) {
    metadataEl.classList.remove("has-focused-panel");
  }
  syncTraceabilityBtnState();
  focusedRow = -1;
  focusedCol = 0;
}

// ---------------------------------------------------------------------------
// Panel lifecycle
// ---------------------------------------------------------------------------

/**
 * Locate the pre-existing traceability panel element in the DOM.
 * The panel div is declared in template-responsive.html as a sibling of
 * .noet-metadata; this function simply gets a reference to it and wires
 * the one-time Escape key handler.
 */
function ensurePanel() {
  if (panelEl) return;

  panelEl = document.getElementById("noet-focused-panel");
  if (!panelEl) {
    console.warn("[Traceability] #noet-focused-panel not found in DOM");
    return;
  }

  // Close on Escape key (global, gated by panel open state)
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape" && panelEl?.classList.contains("is-open")) {
      e.preventDefault();
      closeTraceabilityModal();
      // Restore focus to the trigger button
      document.querySelector(".noet-traceability-btn")?.focus();
    }
  });

  // Four-directional arrow key navigation (global, gated by panel open state).
  //
  // The focus model is data-driven: `focusedRow` and `focusedCol` index into
  // the flat rows array from `getNavRows()`.  Both connectivity and tape views
  // produce the same `{ cells: [...] }` row shape, so navigation is unified.
  //
  // ArrowUp / ArrowDown — move to the prev/next row that has a non-empty cell
  //   at focusedCol.  If focusedCol is 0, move to any adjacent row.
  //
  // ArrowLeft / ArrowRight — move to the prev/next non-empty cell within the
  //   current row.
  document.addEventListener("keydown", (e) => {
    if (!panelEl?.classList.contains("is-open")) return;
    if (
      e.key !== "ArrowUp" &&
      e.key !== "ArrowDown" &&
      e.key !== "ArrowLeft" &&
      e.key !== "ArrowRight"
    )
      return;

    // Don't hijack arrow keys when focus is inside a form control.
    const tag = document.activeElement?.tagName?.toLowerCase();
    if (tag === "input" || tag === "select" || tag === "textarea") return;

    e.preventDefault();

    const rows = getNavRows();
    if (rows.length === 0) return;

    if (e.key === "ArrowUp" || e.key === "ArrowDown") {
      const step = e.key === "ArrowDown" ? 1 : -1;

      // If unfocused, initialise to first/last row.
      if (focusedRow < 0) {
        focusedRow = step > 0 ? 0 : rows.length - 1;
        applyFocusHighlight();
        return;
      }

      // Scan for the next row that has a non-empty cell at focusedCol.
      let next = focusedRow + step;
      while (next >= 0 && next < rows.length) {
        if (navCellHasContent(rows[next], focusedCol)) break;
        next += step;
      }
      if (next < 0 || next >= rows.length) return; // stay put

      focusedRow = next;
      applyFocusHighlight();
    } else {
      // ArrowLeft / ArrowRight — step through visible columns, skip empties.
      if (focusedRow < 0) return;
      const visCols = getVisibleColIndices();
      if (visCols.length === 0) return;

      // Find current position in the visible-columns list.
      let posInVis = visCols.indexOf(focusedCol);
      if (posInVis < 0) posInVis = 0;

      const step = e.key === "ArrowRight" ? 1 : -1;
      let nextPos = posInVis + step;
      while (nextPos >= 0 && nextPos < visCols.length) {
        if (navCellHasContent(rows[focusedRow], visCols[nextPos])) break;
        nextPos += step;
      }
      if (nextPos < 0 || nextPos >= visCols.length) return;

      focusedCol = visCols[nextPos];
      applyFocusHighlight();
    }
  });
}

// ---------------------------------------------------------------------------
// Data-driven navigation helpers
// ---------------------------------------------------------------------------

/**
 * Return the flat rows array for the current view.  Both connectivity and
 * tape modes produce `{ rows: [{ cells }] }` — for tape mode we read from
 * the currently selected entry.
 *
 * @returns {Array<{ cells: any[] }>}
 */
function getNavRows() {
  if (displayMode === "tape") {
    const entry = rawTapeEntries?.[rawTapeSelectedEntry];
    return entry?.rows || [];
  }
  // Use the cached sorted rows so navigation matches display order.
  if (sortedNavRows) return sortedNavRows.map((e) => e.row);
  return currentViewData?.rows || [];
}

/**
 * Test whether a cell at `colIdx` in `row` has navigable content.
 * A cell has content if it is a BID reference, an edge object, or a
 * non-empty string.
 */
function navCellHasContent(row, colIdx) {
  const cell = (row?.cells || [])[colIdx];
  if (!cell) return false;
  if (typeof cell === "object" && (cell.bid || cell.edge)) return true;
  if (typeof cell === "string" && cell.length > 0) return true;
  return false;
}

/**
 * Return the set of column indices that are currently rendered.
 * In connectivity mode, hidden kind columns are excluded.
 * In tape mode, all columns are visible.
 */
function getVisibleColIndices() {
  if (displayMode === "tape") {
    const entry = rawTapeEntries?.[rawTapeSelectedEntry];
    const headers = entry?.headers || [];
    return headers.map((_, i) => i);
  }
  const headers = currentViewData?.headers || [];
  const indices = [0]; // Node column always visible
  for (let i = 1; i < headers.length; i++) {
    const kind = headers[i].split(" ")[0];
    if (visibleKinds.has(kind)) indices.push(i);
  }
  return indices;
}

/**
 * If the focused column is no longer visible (e.g. the user unchecked a kind
 * filter), snap focusedCol to the nearest visible column that has content,
 * falling back to column 0.
 */
function clampFocusToVisibleColumns() {
  if (focusedRow < 0) return;
  const visCols = getVisibleColIndices();
  if (visCols.length === 0) return;
  if (visCols.includes(focusedCol)) return; // still visible

  // Try the nearest visible column with content in the focused row.
  const rows = getNavRows();
  const row = rows[focusedRow];
  for (const ci of visCols) {
    if (navCellHasContent(row, ci)) {
      focusedCol = ci;
      return;
    }
  }
  // Fallback: first visible column (always column 0 for connectivity).
  focusedCol = visCols[0];
}

/**
 * Extract the BID from a cell value (for metadata panel updates).
 * Returns null for plain-text or empty cells.
 */
function navCellBid(cell) {
  if (!cell || typeof cell !== "object") return null;
  if (cell.bid) return cell.bid;
  if (cell.edge) {
    // For edge cells, use the source or sink depending on ownership.
    if (cell.edge.owned_by === "source" && cell.edge.source?.bid)
      return cell.edge.source.bid;
    if (cell.edge.owned_by === "sink" && cell.edge.sink?.bid) return cell.edge.sink.bid;
    if (cell.edge.owner?.bid) return cell.edge.owner.bid;
    return cell.edge.source?.bid || cell.edge.sink?.bid || null;
  }
  return null;
}

/**
 * Apply the `is-keyboard-focused` CSS class to the `<td>` at
 * (focusedRow, focusedCol) and open the metadata panel for that cell’s BID.
 *
 * The renderer stamps `data-row-idx` and `data-col-idx` on DOM elements,
 * so lookup is a single querySelector.
 */
function applyFocusHighlight() {
  if (!panelEl) return;
  const body = panelEl.querySelector(".noet-traceability__body");
  if (!body) return;

  // Clear previous highlight.
  body
    .querySelectorAll("td.is-keyboard-focused")
    .forEach((td) => td.classList.remove("is-keyboard-focused"));

  if (focusedRow < 0) return;

  const td = body.querySelector(
    `td[data-row-idx="${focusedRow}"][data-col-idx="${focusedCol}"]`,
  );
  if (td) {
    td.classList.add("is-keyboard-focused");
    td.scrollIntoView({ block: "nearest" });
  }

  // Open metadata panel for the focused cell’s BID.
  const rows = getNavRows();
  const cell = rows[focusedRow]?.cells?.[focusedCol];
  const bid = navCellBid(cell);
  if (bid) showMetadataPanel(bid);
}

// ---------------------------------------------------------------------------
// Mode switching helpers
// ---------------------------------------------------------------------------

/**
 * Sync control visibility and values with current state.
 * - Search input value synced.
 */
function syncControls() {
  if (!panelEl) return;
  const networkFilter = panelEl.querySelector("#noet-traceability-network-filter");
  if (networkFilter) networkFilter.style.display = "";

  const input = panelEl.querySelector("#noet-traceability-search-input");
  if (input && input.value !== searchQuery) {
    input.value = searchQuery;
  }
}

/**
 * Try to parse a query string via WASM and return the parsed spec object,
 * or null on failure. Shows/clears the parse error inline.
 */
function tryParseQuery(queryText) {
  const BeliefBaseWasm = state.wasmModule?.BeliefBaseWasm;
  if (!BeliefBaseWasm?.parseQuery) return null;
  try {
    const grammar = searchQueryToGrammar(queryText);
    const spec = BeliefBaseWasm.parseQuery(grammar);
    clearParseError();
    return spec;
  } catch (e) {
    showParseError(String(e));
    return null;
  }
}

/**
 * Show an inline parse error message below the query input.
 */
function showParseError(message) {
  const el = panelEl?.querySelector("#noet-traceability-parse-error");
  if (!el) return;
  el.textContent = message;
  el.hidden = false;
}

/**
 * Clear the inline parse error.
 */
function clearParseError() {
  const el = panelEl?.querySelector("#noet-traceability-parse-error");
  if (!el) return;
  el.textContent = "";
  el.hidden = true;
}

/**
 * Render step cards from a parsed QuerySpec.
 * Each step is shown as a compact card with type label, parameters,
 * and remove/reorder controls. Includes an "Add step" button.
 * Null spec clears the step area.
 *
 * @param {object|null} spec - Parsed QuerySpec ({ steps: [...] }).
 */
function renderStepCards(spec) {
  const container = panelEl?.querySelector("#noet-traceability-steps");
  if (!container) return;

  currentSpec = spec;

  if (!spec) {
    container.innerHTML = "";
    return;
  }

  container.innerHTML = renderStepList(spec.steps || [], "", 0);

  // Attach handlers to ALL [data-action] buttons (including nested)
  container.querySelectorAll("[data-action]").forEach((btn) => {
    btn.addEventListener("click", handleStepAction);
  });

  // Re-attach editor handlers for the open editor and any visible
  // ancestor editors (compose steps whose branch contains the open editor).
  if (openEditorPath !== null) {
    container
      .querySelectorAll(".noet-traceability__step-editor:not([hidden])")
      .forEach((el) => {
        const elPath = el.dataset.editorPath;
        if (!elPath) return;
        attachEditorHandlers(el, elPath);
      });
    const resolved = resolveStepPath(spec, openEditorPath);
    if (resolved?.step?.input?.Keys !== undefined) {
      subjectNodes = (resolved.step.input.Keys || []).map((k) => {
        const bid = k?.Bid?.bid || "";
        return { bid, title: labelForBid(bid), path: "" };
      });
      renderSubjectChips();
      attachSubjectInputHandlers();
    }
  }
}

/**
 * Render a list of steps as cards + editors. Called recursively for Compose
 * branches. pathPrefix is "" at top level, "2.left." inside a branch, etc.
 */
function renderStepList(steps, pathPrefix, depth) {
  let html = "";
  for (let i = 0; i < steps.length; i++) {
    const step = steps[i];
    const path = pathPrefix + i;

    const { typeLabel, detail } = formatStep(step);
    html += `<div class="noet-traceability__step-card" data-step-path="${path}">`;
    // Input connector inside the card (top-left)
    const inputLabel = formatTapeFn(step.input);
    html += `<span class="noet-traceability__step-input" title="Input: ${escapeHtml(inputLabel)}">${escapeHtml(inputLabel)}</span>`;
    if (steps.length > 1) {
      html += `<span class="noet-traceability__step-nav">`;
      if (i > 0) {
        html += `<button class="noet-traceability__step-btn" data-action="up" data-path="${path}" title="Move up">\u25B2</button>`;
      }
      if (i < steps.length - 1) {
        html += `<button class="noet-traceability__step-btn" data-action="down" data-path="${path}" title="Move down">\u25BC</button>`;
      }
      html += `</span>`;
    }
    const label = step.label
      ? `<span class="noet-traceability__step-label">${escapeHtml(step.label)}</span> `
      : "";
    html += `<span class="noet-traceability__step-type">${escapeHtml(typeLabel)}</span> ${label}`;
    html += `<span class="noet-traceability__step-detail" data-action="edit" data-path="${path}" title="Click to edit">${escapeHtml(detail)}</span>`;
    html += `<button class="noet-traceability__step-btn noet-traceability__step-btn--remove" data-action="remove" data-path="${path}" title="Remove step">\u2715</button>`;
    if (depth > 0) {
      html += `<button class="noet-traceability__step-btn" data-action="extract" data-path="${path}" title="Extract from branch">\u2B61</button>`;
    }
    html += `</div>`;

    // Inline editor for this step.
    // Open if this IS the target editor, or if the target editor is nested
    // inside this step (ancestor path — keeps compose editors visible when
    // editing a branch step within them).
    const editorOpen =
      openEditorPath === path ||
      (openEditorPath && openEditorPath.startsWith(path + "."));
    html += `<div class="noet-traceability__step-editor" data-editor-path="${path}"${editorOpen ? "" : " hidden"}>`;
    html += renderStepEditor(step, path, depth);
    html += `</div>`;
  }

  // Add step button for this level
  const addPath = pathPrefix ? pathPrefix.slice(0, -1) : "";
  html += `<button class="noet-traceability__step-add" data-action="add" data-path="${addPath}" title="Add step">+ Step</button>`;
  return html;
}

/**
 * Render the inline editor form for a single step.
 * Returns HTML string. The form controls have data attributes for the step path
 * so that change handlers can resolve the step via `resolveStepPath`.
 */
function renderStepEditor(step, path, depth) {
  const op = step?.operation || {};
  const currentType = op.Filter
    ? op.Filter.TextMatch
      ? "search"
      : "filter"
    : op.Traverse
      ? "traverse"
      : op.Compose
        ? "compose"
        : "traverse";

  // Input (TapeFn) selector — includes seed variants (Keys, Corpus, Bids)
  const tapeFn = step?.input || { Then: null };
  const currentTapeFn =
    tapeFn.Keys !== undefined
      ? "keys"
      : tapeFn === "Corpus"
        ? "corpus"
        : tapeFn.Bids !== undefined
          ? "bids"
          : tapeFn.Fold
            ? "fold"
            : tapeFn === "Terminal" || tapeFn.Terminal !== undefined
              ? "terminal"
              : tapeFn === "Orphan" || tapeFn.Orphan !== undefined
                ? "orphan"
                : "then";

  depth = depth || 0;
  let html = `<div class="noet-traceability__editor-row">`;
  html += `<label>Input: <select class="noet-traceability__editor-select" data-field="tape-fn" data-path="${path}">`;
  html += `<option value="then"${currentTapeFn === "then" ? " selected" : ""}>${"\u2192"} Then</option>`;
  html += `<option value="fold"${currentTapeFn === "fold" ? " selected" : ""}>Fold</option>`;
  html += `<option value="terminal"${currentTapeFn === "terminal" ? " selected" : ""}>Terminal</option>`;
  html += `<option value="orphan"${currentTapeFn === "orphan" ? " selected" : ""}>Orphan</option>`;
  html += `<option value="keys"${currentTapeFn === "keys" ? " selected" : ""}>Keys (node picker)</option>`;
  html += `<option value="corpus"${currentTapeFn === "corpus" ? " selected" : ""}>Corpus (all nodes)</option>`;
  html += `<option value="bids"${currentTapeFn === "bids" ? " selected" : ""}>Bids (explicit)</option>`;

  html += `</select></label>`;

  if (currentTapeFn === "fold") {
    const foldOp = tapeFn.Fold?.op || "Union";
    html += `<label>Op: <select class="noet-traceability__editor-select" data-field="fold-op" data-path="${path}">`;
    for (const o of ["Union", "Intersection", "LeftDiff", "RightDiff", "SymmetricDiff"]) {
      html += `<option value="${o}"${o === foldOp ? " selected" : ""}>${o}</option>`;
    }
    html += `</select></label>`;
  }
  const isIdentity = op === "Identity";
  if (!isIdentity) {
    html += `<label>Type: <select class="noet-traceability__editor-select" data-field="type" data-path="${path}">`;
    html += `<option value="search"${currentType === "search" ? " selected" : ""}>Search (TextMatch)</option>`;
    html += `<option value="filter"${currentType === "filter" ? " selected" : ""}>Filter (Predicate)</option>`;
    html += `<option value="traverse"${currentType === "traverse" ? " selected" : ""}>Traverse</option>`;
    html += `<option value="compose"${currentType === "compose" ? " selected" : ""}>Compose</option>`;
    html += `</select></label>`;
  }
  // Step label (for referencing in Fold ranges, Compose branches, etc.)
  const stepLabel = step?.label || "";
  html += `<label>Label: <input type="text" class="noet-traceability__editor-input" data-field="step-label" data-path="${path}" value="${escapeHtml(stepLabel)}" placeholder="${path}" title="Name this step for reference by other steps"></label>`;
  html += `</div>`;

  // Seed-specific inline controls rendered after the input selector row
  if (currentTapeFn === "keys") {
    html += `<div class="noet-traceability__editor-row noet-traceability__editor-row--seed">`;
    html += `<div class="noet-traceability__subject-chips" id="noet-traceability-subject-chips"></div>`;
    html += `<div class="noet-traceability__subject-input-wrap">`;
    html += `<input type="text" id="noet-traceability-subject-input"`;
    html += ` class="noet-traceability__subject-input"`;
    html += ` placeholder="Search nodes to add\u2026"`;
    html += ` aria-label="Search for nodes to add"`;
    html += ` autocomplete="off">`;
    html += `<ul id="noet-traceability-subject-suggestions"`;
    html += ` class="noet-traceability__subject-suggestions" hidden></ul>`;
    html += `</div>`;
    html += `</div>`;
  } else if (currentTapeFn === "bids") {
    const bidsStr = Array.isArray(tapeFn.Bids) ? tapeFn.Bids.join(", ") : "";
    html += `<div class="noet-traceability__editor-row">`;
    html += `<label>BIDs: <input type="text" class="noet-traceability__editor-input noet-traceability__editor-input--wide" data-field="bids-input" data-path="${path}" value="${escapeHtml(bidsStr)}" placeholder="comma-separated BID strings"></label>`;
    html += `</div>`;
  }
  // Corpus needs no additional controls.

  // Type-specific fields (skip for Identity — it's a pure seed pass-through)
  if (isIdentity) {
    return html;
  }

  if (currentType === "search") {
    const tm = op.Filter?.TextMatch || {};
    const field = tm.path?.map((s) => s.Key || "*").join(".") || "text";
    const query = tm.query || "";
    html += `<div class="noet-traceability__editor-row">`;
    html += `<label>Field: <input type="text" class="noet-traceability__editor-input" data-field="search-field" data-path="${path}" value="${escapeHtml(field)}" placeholder="text"></label>`;
    html += `<label>Query: <input type="text" class="noet-traceability__editor-input noet-traceability__editor-input--wide" data-field="search-query" data-path="${path}" value="${escapeHtml(query)}" placeholder="search terms"></label>`;
    html += `</div>`;
  } else if (currentType === "filter") {
    const pred = op.Filter?.Predicate || {};
    const filterPath = pred.path?.map((s) => s.Key || "*").join(".") || "";
    const opStr =
      typeof pred.op === "string"
        ? pred.op
        : pred.op
          ? Object.keys(pred.op)[0] || ""
          : "";
    const val =
      pred.value?.String || (pred.value?.Number != null ? String(pred.value.Number) : "");
    html += `<div class="noet-traceability__editor-row">`;
    html += `<label>Path: <input type="text" class="noet-traceability__editor-input" data-field="filter-path" data-path="${path}" value="${escapeHtml(filterPath)}" placeholder="title"></label>`;
    html += `<label>Op: <select class="noet-traceability__editor-select" data-field="filter-op" data-path="${path}">`;
    for (const o of [
      "Eq",
      "NotEq",
      "Contains",
      "Matches",
      "Exists",
      "In",
      "Gt",
      "Lt",
      "Gte",
      "Lte",
    ]) {
      html += `<option value="${o}"${o === opStr ? " selected" : ""}>${o}</option>`;
    }
    html += `</select></label>`;
    html += `<label>Value: <input type="text" class="noet-traceability__editor-input" data-field="filter-value" data-path="${path}" value="${escapeHtml(val)}" placeholder="value"></label>`;
    html += `</div>`;
  } else if (currentType === "traverse") {
    const t = op.Traverse || {};
    const isMaxDepth = t.depth?.count === "Max";
    const depthDisplay = isMaxDepth ? "*" : String(t.depth?.count?.N ?? 1);
    // Named shorthand dropdown
    const currentShorthand = detectTraversalShorthand(t);
    html += `<div class="noet-traceability__editor-row">`;
    html += `<select class="noet-traceability__editor-input" data-field="trav-shorthand" data-path="${path}">`;
    for (const sh of TRAVERSAL_SHORTHANDS) {
      const sel = sh.name === currentShorthand ? " selected" : "";
      html += `<option value="${sh.name}"${sel}>${sh.name}</option>`;
    }
    const customSel = currentShorthand === "custom" ? " selected" : "";
    html += `<option value="custom"${customSel}>Custom</option>`;
    html += `</select>`;
    html += `</div>`;
    // Layout mirrors the query grammar: [input roles] - [weight kinds] - [output roles] (depth)
    html += `<div class="noet-traceability__editor-row noet-traceability__editor-row--traverse">`;
    // Input roles
    html += `<span class="noet-traceability__editor-group">`;
    for (const [bit, label, abbr] of [
      [0, "Source", "s"],
      [1, "Sink", "k"],
      [2, "Owner", "o"],
    ]) {
      const checked =
        typeof t.input_roles === "number" ? (t.input_roles >> bit) & 1 : false;
      html += `<label class="noet-traceability__editor-cb" title="${label}"><input type="checkbox" data-field="trav-input" data-bit="${bit}" data-path="${path}"${checked ? " checked" : ""}> ${abbr}</label>`;
    }
    html += `</span>`;
    html += `<span class="noet-traceability__editor-divider">\u2013</span>`;
    // Weight kinds
    html += `<span class="noet-traceability__editor-group">`;
    for (const [bit, label] of [
      [0, "Section"],
      [1, "Pragmatic"],
      [2, "Epistemic"],
    ]) {
      const checked =
        typeof t.kind_filter === "number" ? (t.kind_filter >> bit) & 1 : false;
      html += `<label class="noet-traceability__editor-cb" title="${label}"><input type="checkbox" data-field="trav-kind" data-bit="${bit}" data-path="${path}"${checked ? " checked" : ""}> ${label}</label>`;
    }
    html += `</span>`;
    html += `<span class="noet-traceability__editor-divider">\u2013</span>`;
    // Output roles
    html += `<span class="noet-traceability__editor-group">`;
    for (const [bit, label, abbr] of [
      [0, "Source", "s"],
      [1, "Sink", "k"],
      [2, "Owner", "o"],
    ]) {
      const checked =
        typeof t.output_roles === "number" ? (t.output_roles >> bit) & 1 : false;
      html += `<label class="noet-traceability__editor-cb" title="${label}"><input type="checkbox" data-field="trav-output" data-bit="${bit}" data-path="${path}"${checked ? " checked" : ""}> ${abbr}</label>`;
    }
    html += `</span>`;
    // Depth — text input accepts a number or * for unbounded
    html += `<span class="noet-traceability__editor-divider">(</span>`;
    html += `<input type="text" class="noet-traceability__editor-input noet-traceability__editor-input--narrow" data-field="trav-depth" data-path="${path}" value="${depthDisplay}" placeholder="1" title="Depth: number or * for unbounded">`;
    html += `<span class="noet-traceability__editor-divider">)</span>`;
    html += `</div>`;
  } else if (currentType === "compose") {
    const c = op.Compose || {};
    const compOp = c.op || "And";
    const leftSteps = Array.isArray(c.left) ? c.left : [];
    const rightSteps = Array.isArray(c.right) ? c.right : [];

    // Operator selector
    html += `<div class="noet-traceability__editor-row">`;
    html += `<label>Operator: <select class="noet-traceability__editor-select" data-field="compose-op" data-path="${path}">`;
    for (const o of ["And", "Or", "Not"]) {
      html += `<option value="${o}"${o === compOp ? " selected" : ""}>${o}</option>`;
    }
    html += `</select></label>`;
    html += `</div>`;

    // Left branch — recursive
    html += `<div class="noet-traceability__editor-branch-container">`;
    html += `<div class="noet-traceability__editor-branch-label">Left</div>`;
    html += `<div class="noet-traceability__editor-branch-content" style="padding-left:${(depth + 1) * 12}px">`;
    html += renderStepList(leftSteps, path + ".left.", depth + 1);
    html += `</div></div>`;

    // Right branch — recursive
    html += `<div class="noet-traceability__editor-branch-container">`;
    html += `<div class="noet-traceability__editor-branch-label">Right</div>`;
    html += `<div class="noet-traceability__editor-branch-content" style="padding-left:${(depth + 1) * 12}px">`;
    html += renderStepList(rightSteps, path + ".right.", depth + 1);
    html += `</div></div>`;
  }

  return html;
}

/**
 * Attach change handlers to editor controls within a step editor.
 * Only binds controls whose data-path matches this editor's path,
 * preventing nested branch editors from being bound to the wrong step.
 */
function attachEditorHandlers(editorEl, path) {
  editorEl.querySelectorAll("[data-field]").forEach((el) => {
    if (el.dataset.path && el.dataset.path !== path) return;
    const event = el.tagName === "SELECT" || el.type === "checkbox" ? "change" : "input";
    el.addEventListener(event, () => applyEditorChange(el, path));
  });
}

/**
 * Attach subject autocomplete + keyboard handlers to the subject input
 * inside a step editor (when the step's input type is "keys").
 * Called after renderStepEditor creates the DOM containing
 * #noet-traceability-subject-input and related elements.
 */
function attachSubjectInputHandlers() {
  // Scope to the open editor so we wire up the correct input when multiple
  // steps have Keys inputs (each renders the same element IDs).
  const scope =
    (openEditorPath !== null &&
      panelEl?.querySelector(`[data-editor-path="${openEditorPath}"]:not([hidden])`)) ||
    panelEl;
  const input = scope?.querySelector("#noet-traceability-subject-input");
  if (!input) {
    console.warn(
      "[Traceability] subject input not found in DOM \u2014 autocomplete unavailable",
    );
    return;
  }

  // Autocomplete: debounced search.
  input.addEventListener("input", (e) => {
    const q = e.target.value.trim();
    if (subjectDebounceTimer) clearTimeout(subjectDebounceTimer);
    subjectDebounceTimer = setTimeout(() => subjectAutocomplete(q), 200);
  });

  // Resolve the suggestions list from the same scoped editor.
  const suggestions = scope?.querySelector("#noet-traceability-subject-suggestions");

  // Keyboard navigation: arrows, enter, escape, backspace.
  input.addEventListener("keydown", (e) => {
    if (!suggestions || suggestions.hidden) {
      if (e.key === "Backspace" && e.target.value === "" && subjectNodes.length > 0) {
        subjectNodes.pop();
        onSubjectChanged();
      }
      return;
    }
    const items = suggestions.querySelectorAll(".noet-traceability__subject-suggestion");
    let active = suggestions.querySelector(".is-active");
    let activeIdx = active ? parseInt(active.dataset.idx, 10) : -1;

    if (e.key === "ArrowDown") {
      e.preventDefault();
      activeIdx = Math.min(activeIdx + 1, items.length - 1);
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      activeIdx = Math.max(activeIdx - 1, 0);
    } else if (e.key === "Enter" && activeIdx >= 0) {
      e.preventDefault();
      items[activeIdx]?.click();
      return;
    } else if (e.key === "Escape") {
      suggestions.hidden = true;
      return;
    } else {
      return;
    }

    items.forEach((li) => li.classList.remove("is-active"));
    if (items[activeIdx]) {
      items[activeIdx].classList.add("is-active");
      items[activeIdx].scrollIntoView({ block: "nearest" });
    }
  });
}

/**
 * Apply a single editor field change to the step at the given path.
 */
function applyEditorChange(el, path) {
  const resolved = resolveStepPath(currentSpec, path);
  if (!resolved) return;
  const step = resolved.step;
  const field = el.dataset.field;

  switch (field) {
    case "tape-fn": {
      const newTapeFn = el.value;
      if (newTapeFn === "then") {
        step.input = { Then: null };
      } else if (newTapeFn === "fold") {
        step.input = { Fold: { op: "Union", range: null } };
      } else if (newTapeFn === "terminal") {
        step.input = { Terminal: null };
      } else if (newTapeFn === "orphan") {
        step.input = { Orphan: null };
      } else if (newTapeFn === "keys") {
        step.input = {
          Keys: subjectNodes.map((n) => ({ Bid: { bid: n.bid } })),
        };
      } else if (newTapeFn === "corpus") {
        step.input = "Corpus";
      } else if (newTapeFn === "bids") {
        step.input = { Bids: [] };
      }
      // Re-render this editor to show/hide type-specific fields.
      const editorEl = panelEl?.querySelector(`[data-editor-path="${path}"]`);
      if (editorEl) {
        editorEl.innerHTML = renderStepEditor(step, path);
        attachEditorHandlers(editorEl, path);
        if (newTapeFn === "keys") {
          renderSubjectChips();
          attachSubjectInputHandlers();
        }
      }
      syncSpecToText();
      return;
    }
    case "fold-op": {
      if (step.input?.Fold) {
        step.input.Fold.op = el.value;
      }
      break;
    }

    case "step-label": {
      step.label = el.value;
      break;
    }
    case "type": {
      const newType = el.value;
      // Rebuild the operation from scratch with defaults.
      if (newType === "search") {
        step.operation = {
          Filter: { TextMatch: { path: [{ Key: "text" }], query: "" } },
        };
      } else if (newType === "filter") {
        step.operation = {
          Filter: {
            Predicate: { path: [{ Key: "title" }], op: "Eq", value: { String: "" } },
          },
        };
      } else if (newType === "traverse") {
        step.operation = {
          Traverse: {
            input_roles: 2,
            kind_filter: 1,
            output_roles: 1,
            depth: { count: { N: 1 }, edge_filter: null },
          },
        };
      } else if (newType === "compose") {
        step.operation = { Compose: { left: [], op: "And", right: [] } };
      }
      // Re-render this editor with the new type's fields.
      const editorEl = panelEl?.querySelector(`[data-editor-path="${path}"]`);
      if (editorEl) {
        editorEl.innerHTML = renderStepEditor(step, path);
        attachEditorHandlers(editorEl, path);
        if (step.input?.Keys !== undefined) {
          renderSubjectChips();
          attachSubjectInputHandlers();
        }
      }
      syncSpecToText();
      return;
    }
    case "search-field": {
      const tm = step.operation?.Filter?.TextMatch;
      if (tm) tm.path = [{ Key: el.value || "text" }];
      break;
    }
    case "search-query": {
      const tm = step.operation?.Filter?.TextMatch;
      if (tm) tm.query = el.value;
      break;
    }
    case "filter-path": {
      // Multi-predicate: find the predicate by data-pred index.
      // For a single Predicate filter, data-pred is 0.
      const pred = step.operation?.Filter?.Predicate;
      if (pred) pred.path = [{ Key: el.value || "title" }];
      break;
    }
    case "filter-op": {
      const pred = step.operation?.Filter?.Predicate;
      if (pred) pred.op = el.value;
      break;
    }
    case "filter-value": {
      const pred = step.operation?.Filter?.Predicate;
      if (pred) {
        const num = Number(el.value);
        pred.value =
          isNaN(num) || el.value === "" ? { String: el.value } : { Number: num };
      }
      break;
    }
    case "trav-shorthand": {
      const t = step.operation?.Traverse;
      if (!t) break;
      const val = el.value;
      const sh = TRAVERSAL_SHORTHANDS.find((s) => s.name === val);
      if (sh) {
        t.input_roles = sh.input;
        t.kind_filter = sh.kind;
        t.output_roles = sh.output;
        if (val === "halo") {
          t.depth = { count: { N: 1 }, edge_filter: null };
        }
      }
      // "custom" — leave bitfields unchanged
      break;
    }
    case "trav-input": {
      const t = step.operation?.Traverse;
      if (t) t.input_roles = readBitfield(panelEl, "trav-input", path);
      break;
    }
    case "trav-kind": {
      const t = step.operation?.Traverse;
      if (t) t.kind_filter = readBitfield(panelEl, "trav-kind", path);
      break;
    }
    case "trav-output": {
      const t = step.operation?.Traverse;
      if (t) t.output_roles = readBitfield(panelEl, "trav-output", path);
      break;
    }
    case "trav-depth": {
      const t = step.operation?.Traverse;
      if (t) {
        const raw = el.value.trim();
        if (raw === "*") {
          t.depth = { count: "Max", edge_filter: t.depth?.edge_filter || null };
        } else {
          const val = parseInt(raw, 10);
          if (!isNaN(val) && val > 0) {
            t.depth = { count: { N: val }, edge_filter: t.depth?.edge_filter || null };
          }
        }
      }
      break;
    }
    case "compose-op": {
      const c = step.operation?.Compose;
      if (c) c.op = el.value;
      break;
    }
    case "bids-input": {
      const raw = el.value.trim();
      step.input = {
        Bids: raw ? raw.split(/\s*,\s*/).filter(Boolean) : [],
      };
      break;
    }
    default:
      return;
  }
  syncSpecToText();
}

/**
 * Read a bitfield value from a group of checkboxes with the same data-field and data-path.
 */
function readBitfield(container, fieldName, path) {
  let value = 0;
  container
    .querySelectorAll(`[data-field="${fieldName}"][data-path="${path}"]`)
    .forEach((cb) => {
      if (cb.checked) {
        value |= 1 << parseInt(cb.dataset.bit, 10);
      }
    });
  return value;
}

/**
 * Format a TapeFn (step input) for display as a connector label.
 * Returns null for the default Then(null) to keep the display clean.
 */
function formatTapeFn(input) {
  if (!input) return "\u2192"; // right arrow for implicit Then
  // Seed TapeFn variants
  if (input.Bids) return `BIDS(${input.Bids.length})`;
  if (input.Keys) {
    const keys = Array.isArray(input.Keys) ? input.Keys : [input.Keys];
    return `KEYS(${keys.map(formatNodeKey).join(",")})`;
  }
  if (input === "Corpus") return "CORPUS()";
  if (input.DocumentNodes) return "DOCNODES()";
  // Pipeline TapeFn variants
  if (input.Then === null || input.Then === undefined) return "\u2192";
  if (input.Then) return `THEN(${formatStepRef(input.Then)})`;
  if (input.Fold) {
    const op = input.Fold.op || "?";
    if (input.Fold.range) {
      const [a, b] = input.Fold.range;
      return `FOLD(${op}, ${formatStepRef(a)}, ${formatStepRef(b)})`;
    }
    return `FOLD(${op})`;
  }
  if (input === "Terminal" || input.Terminal !== undefined) {
    return "TERMINAL";
  }
  if (input === "Orphan" || input.Orphan !== undefined) {
    return "ORPHAN";
  }

  return "\u2192";
}

/**
 * Format a StepRef for display.
 */
function formatStepRef(ref) {
  if (!ref) return "?";
  if (ref.Label) return ref.Label;
  if (ref.Index !== undefined) return `#${ref.Index}`;
  return String(ref);
}

/**
 * Handle add/remove/reorder actions on step cards.
 * Modifies `currentSpec`, serializes back to text, and re-evaluates.
 */
function handleStepAction(e) {
  e.stopPropagation();
  if (!currentSpec) return;

  const action = e.currentTarget.dataset.action;
  const path = e.currentTarget.dataset.path;

  switch (action) {
    case "remove": {
      const resolved = resolveStepPath(currentSpec, path);
      if (resolved) {
        resolved.steps.splice(resolved.index, 1);
      }
      break;
    }

    case "up": {
      const resolved = resolveStepPath(currentSpec, path);
      if (resolved && resolved.index > 0) {
        const s = resolved.steps;
        const i = resolved.index;
        [s[i - 1], s[i]] = [s[i], s[i - 1]];
      }
      break;
    }

    case "down": {
      const resolved = resolveStepPath(currentSpec, path);
      if (resolved && resolved.index < resolved.steps.length - 1) {
        const s = resolved.steps;
        const i = resolved.index;
        [s[i], s[i + 1]] = [s[i + 1], s[i]];
      }
      break;
    }

    case "add": {
      // Determine which array to add to.
      let targetSteps;
      if (!path) {
        targetSteps = currentSpec.steps;
      } else {
        // path might be "2.left" meaning add to compose's left branch
        const lastDot = path.lastIndexOf(".");
        if (lastDot >= 0) {
          const parentPath = path.substring(0, lastDot);
          const branch = path.substring(lastDot + 1);
          const resolved = resolveStepPath(currentSpec, parentPath);
          if (
            resolved?.step?.operation?.Compose &&
            (branch === "left" || branch === "right")
          ) {
            const compose = resolved.step.operation.Compose;
            if (!Array.isArray(compose[branch])) compose[branch] = [];
            targetSteps = compose[branch];
          } else {
            targetSteps = currentSpec.steps;
          }
        } else {
          targetSteps = currentSpec.steps;
        }
      }
      targetSteps.push({
        label: "",
        input: targetSteps.length === 0 ? "Corpus" : { Then: null },
        operation: {
          Traverse: {
            input_roles: 2, // Sink (bit 1)
            kind_filter: 1, // Section (bit 0)
            output_roles: 1, // Source (bit 0)
            depth: { count: { N: 1 }, edge_filter: null },
          },
        },
      });
      break;
    }

    case "extract": {
      // Move step from a compose branch to the parent steps array.
      const resolved = resolveStepPath(currentSpec, path);
      if (!resolved) return;
      const [removed] = resolved.steps.splice(resolved.index, 1);
      // Find the parent compose step — the path minus the last ".branch.idx"
      const parts = path.split(".");
      if (parts.length >= 3) {
        const parentPath = parts.slice(0, -2).join(".");
        const parentResolved = resolveStepPath(currentSpec, parentPath);
        if (parentResolved) {
          parentResolved.steps.splice(parentResolved.index + 1, 0, removed);
        }
      } else {
        // Already at top level (shouldn't happen, but safe fallback)
        currentSpec.steps.push(removed);
      }
      break;
    }

    case "edit": {
      // Toggle the inline editor for this step.
      const editor = panelEl?.querySelector(`[data-editor-path="${path}"]`);
      if (!editor) return;
      const wasHidden = editor.hidden;
      // Close all editors that are NOT ancestors of the target path.
      // This keeps parent compose editors visible when editing a branch step.
      panelEl.querySelectorAll(".noet-traceability__step-editor").forEach((el) => {
        const elPath = el.dataset.editorPath;
        // Keep this editor open if the target path starts with it
        // (i.e., it's an ancestor). "0" is ancestor of "0.left.0".
        if (elPath && path.startsWith(elPath + ".")) return;
        el.hidden = true;
      });
      if (wasHidden) {
        editor.hidden = false;
        openEditorPath = path;
        attachEditorHandlers(editor, path);
        // If this step has a Keys input, load its Keys into subjectNodes
        // and render chips. Each step has its own Keys array; subjectNodes
        // is the working state for the currently open editor.
        const resolved = resolveStepPath(currentSpec, path);
        if (resolved?.step?.input?.Keys !== undefined) {
          subjectNodes = (resolved.step.input.Keys || []).map((k) => {
            const bid = k?.Bid?.bid || "";
            return { bid, title: labelForBid(bid), path: "" };
          });
          renderSubjectChips();
          attachSubjectInputHandlers();
        }
      } else {
        openEditorPath = null;
      }
      return; // Don't call syncSpecToText for toggle.
    }

    default:
      return;
  }

  syncSpecToText();
}

/**
 * Serialize the current `currentSpec` back to query grammar text
 * and update the text input. Does NOT re-evaluate the query — the user
 * must click Execute or press Enter to evaluate.
 */
/**
 * Prepare a spec copy for serialization: empty Keys → Corpus.
 * The original spec retains the empty Keys so the UI shows "no nodes selected".
 */
function prepareSpecForSerialize(spec) {
  if (!spec?.steps) return spec;
  const steps = spec.steps.map((step) => {
    if (step.input?.Keys && step.input.Keys.length === 0) {
      return { ...step, input: "Corpus" };
    }
    return step;
  });
  return { steps };
}

function syncSpecToText() {
  if (!currentSpec) return;

  const BeliefBaseWasm = state.wasmModule?.BeliefBaseWasm;
  if (!BeliefBaseWasm?.serializeQuery) return;

  try {
    // Serialize a copy with empty Keys replaced by Corpus for valid grammar.
    const specForSerialize = prepareSpecForSerialize(currentSpec);
    const queryText = BeliefBaseWasm.serializeQuery(specForSerialize);
    searchQuery = queryText.trim();

    // Update the text input.
    const input = panelEl?.querySelector("#noet-traceability-search-input");
    if (input) input.value = searchQuery;

    // Re-render step cards from the updated spec.
    renderStepCards(currentSpec);
    clearParseError();
    markDirty();
  } catch (e) {
    showParseError("Serialization error: " + String(e));
  }
}

/** Whether the query input has been edited but not yet evaluated. */
let queryDirty = false;

/**
 * Mark the query as edited but not yet evaluated.
 * Shows the Execute button.
 */
function markDirty() {
  queryDirty = true;
  const btn = panelEl?.querySelector("#noet-traceability-execute");
  if (btn) btn.classList.add("is-dirty");
}

/**
 * Clear the dirty state after evaluation.
 */
function clearDirty() {
  queryDirty = false;
  const btn = panelEl?.querySelector("#noet-traceability-execute");
  if (btn) btn.classList.remove("is-dirty");
}

/**
 * Execute the current query: parse, evaluate, render results.
 */
async function executeQuery() {
  if (!searchQuery || searchQuery.length < 2) {
    currentSearchScores = new Map();
    clearParseError();
    renderStepCards(null);
    syncSubjectFromSpec(null);
    syncControls();
    currentViewData = null;
    cachedSpec = null;
    renderTable();
    clearDirty();
    return;
  }
  // Parse for step card display and subject chip sync.
  // Save the full query before syncSubjectFromSpec strips the anchor.
  const fullQuery = searchQuery;
  const parsed = tryParseQuery(fullQuery);
  if (parsed) {
    renderStepCards(parsed);
    syncSubjectFromSpec(parsed);
  }
  syncControls();
  populateNetworkFilter();
  // Evaluate the full query (including anchor), not the stripped searchQuery.
  await refreshFromQuery(searchQueryToGrammar(fullQuery));
  clearDirty();
}

/**
 * Format an EnumSet value for display.
 * EnumSet serializes as a bitfield integer (e.g. 5 = bits 0+2 set).
 * Falls back to array.join if the value is already an array.
 *
 * @param {number|Array|undefined} value - Bitfield integer or array of strings.
 * @param {string[]} labels - Ordered labels for each bit position.
 * @returns {string}
 */
function formatEnumSet(value, labels) {
  if (Array.isArray(value)) return value.join(", ") || "all";
  if (typeof value === "number") {
    const active = labels.filter((_, i) => (value >> i) & 1);
    return active.length > 0 ? active.join(", ") : "all";
  }
  return "all";
}

/**
 * Format a seed TapeFn (first step's input) for display in the seed card.
 */
function formatSeed(input) {
  if (!input) return "Current document";
  if (input === "Corpus") return "All nodes";
  if (input.Bids) {
    return input.Bids.length === 1
      ? `Node ${brefFromBid(input.Bids[0]) || input.Bids[0]}`
      : `${input.Bids.length} node(s)`;
  }
  if (input.Keys) {
    const keys = Array.isArray(input.Keys) ? input.Keys : [input.Keys];
    return keys.map(formatNodeKey).join(", ");
  }
  if (input.DocumentNodes) return "Document nodes";
  // Implicit Then or null → current document context
  if (input.Then === null || input.Then === undefined) return "Current document";
  return JSON.stringify(input);
}

/**
 * Format a NodeKey for display.
 * NodeKey variants: {Bid: {bid}}, {Bref: {bref}}, {Path: {net, path}}, {Id: {net, id}}
 */
function formatNodeKey(key) {
  if (!key || typeof key !== "object") return String(key);
  if (key.Id) return key.Id.id || key.Id;
  if (key.Bid) return brefFromBid(key.Bid.bid || key.Bid) || key.Bid;
  if (key.Bref) return key.Bref.bref || key.Bref;
  if (key.Path) return key.Path.path || key.Path;
  return JSON.stringify(key);
}

/**
 * Format a step's operation for display.
 */
function formatStep(step) {
  if (!step || !step.operation) return { typeLabel: "?", detail: "" };
  const op = step.operation;

  // Identity is a pure seed step — show the seed info as the primary content.
  if (op === "Identity") return { typeLabel: "Seed", detail: formatSeed(step.input) };

  if (op.Filter) {
    const f = op.Filter;
    if (f.TextMatch) {
      const field = f.TextMatch.path?.map((s) => s.Key || "*").join(".") || "text";
      return { typeLabel: "Search", detail: `${field}: ${f.TextMatch.query}` };
    }
    if (f.Predicate) {
      const path = f.Predicate.path?.map((s) => s.Key || "*").join(".") || "?";
      const opStr = f.Predicate.op || "?";
      const val = f.Predicate.value?.String || f.Predicate.value?.Number || "";
      return { typeLabel: "Filter", detail: `${path} ${opStr} ${val}` };
    }
    return { typeLabel: "Filter", detail: JSON.stringify(f) };
  }

  if (op.Traverse) {
    const t = op.Traverse;
    const kinds = formatEnumSet(t.kind_filter, ["Section", "Pragmatic", "Epistemic"]);
    const depth = t.depth?.count === "Max" ? "*" : (t.depth?.count?.N ?? "?");
    const inRoles = formatEnumSet(t.input_roles, ["Source", "Sink", "Owner"]);
    const outRoles = formatEnumSet(t.output_roles, ["Source", "Sink", "Owner"]);
    return {
      typeLabel: "Traverse",
      detail: `${inRoles}\u2192${outRoles} ${kinds} depth=${depth}`,
    };
  }

  if (op.Compose) {
    const c = op.Compose;
    const opLabel = c.op || "?";
    const leftCount = Array.isArray(c.left) ? c.left.length : 0;
    const rightCount = Array.isArray(c.right) ? c.right.length : 0;
    return {
      typeLabel: opLabel,
      detail: `(${leftCount} step${leftCount !== 1 ? "s" : ""}) ${opLabel} (${rightCount} step${rightCount !== 1 ? "s" : ""})`,
    };
  }

  return { typeLabel: "Step", detail: JSON.stringify(op) };
}

// ---------------------------------------------------------------------------
// Subject selector: autocomplete multi-select
// ---------------------------------------------------------------------------

/**
 * Render subject chips from `subjectNodes`.
 * Empty = "All nodes" label. Otherwise one chip per selected node.
 */
function renderSubjectChips() {
  // Scope to the open editor to find the correct chips container.
  const scope =
    (openEditorPath !== null &&
      panelEl?.querySelector(`[data-editor-path="${openEditorPath}"]:not([hidden])`)) ||
    panelEl;
  const container = scope?.querySelector("#noet-traceability-subject-chips");
  if (!container) return;

  if (subjectNodes.length === 0) {
    container.innerHTML = `<span class="noet-traceability__subject-all">All nodes (Corpus)</span>`;
    return;
  }

  let html = "";
  for (const node of subjectNodes) {
    const chipLabel = node.title || brefFromBid(node.bid);
    const hoverTitle = node.path
      ? `${node.title || "(untitled)"}\n${node.path}\n${node.bid}`
      : `${node.title || "(untitled)"}\n${node.bid}`;
    html += `<span class="noet-traceability__subject-chip" data-bid="${escapeHtml(node.bid)}" title="${escapeHtml(hoverTitle)}">`;
    html += `<span class="noet-traceability__chip-label">${escapeHtml(chipLabel)}</span>`;
    html += `<button class="noet-traceability__chip-remove" data-bid="${escapeHtml(node.bid)}" title="Remove">\u2715</button>`;
    html += `</span>`;
  }
  container.innerHTML = html;

  // Attach remove handlers.
  container.querySelectorAll(".noet-traceability__chip-remove").forEach((btn) => {
    btn.addEventListener("mousedown", (e) => {
      // Use mousedown instead of click to fire before blur.
      e.preventDefault();
      e.stopPropagation();
      const bid = btn.dataset.bid;
      subjectNodes = subjectNodes.filter((n) => n.bid !== bid);
      onSubjectChanged();
    });
  });
}

/**
 * Run autocomplete search for the subject selector input.
 * Shows a dropdown of matching nodes.
 */
function subjectAutocomplete(query) {
  // Scope to the open editor to find the correct suggestions list.
  const scope =
    (openEditorPath !== null &&
      panelEl?.querySelector(`[data-editor-path="${openEditorPath}"]:not([hidden])`)) ||
    panelEl;
  const suggestions = scope?.querySelector("#noet-traceability-subject-suggestions");
  if (!suggestions || !state.beliefbase) return;

  if (!query || query.length < 2) {
    suggestions.hidden = true;
    return;
  }

  let results;
  try {
    results = JSON.parse(state.beliefbase.search(query, 10));
  } catch (e) {
    console.warn("[Traceability] subject search failed:", e);
    suggestions.hidden = true;
    return;
  }

  if (!results || results.length === 0) {
    suggestions.hidden = true;
    return;
  }

  // Filter out already-selected BIDs.
  const selectedSet = new Set(subjectNodes.map((n) => n.bid));
  const filtered = results.filter((r) => !selectedSet.has(r.bid));
  if (filtered.length === 0) {
    suggestions.hidden = true;
    return;
  }

  suggestions.innerHTML = filtered
    .map(
      (r, i) =>
        `<li class="noet-traceability__subject-suggestion" data-bid="${escapeHtml(r.bid)}" data-idx="${i}">` +
        `<span class="noet-traceability__suggestion-title">${escapeHtml(r.title || "(untitled)")}</span>` +
        (r.path
          ? `<span class="noet-traceability__suggestion-path">${escapeHtml(r.path)}</span>`
          : "") +
        `</li>`,
    )
    .join("");
  suggestions.hidden = false;

  // Click to select.
  suggestions.querySelectorAll(".noet-traceability__subject-suggestion").forEach((li) => {
    li.addEventListener("click", () => {
      const bid = li.dataset.bid;
      if (bid && !subjectNodes.some((n) => n.bid === bid)) {
        // Find the result to get title/path.
        const match = filtered.find((r) => r.bid === bid);
        subjectNodes.push({
          bid,
          title: match?.title || "",
          path: match?.path || "",
        });
        onSubjectChanged();
      }
      // Clear the input in the same scoped editor.
      const scopeEl =
        (openEditorPath !== null &&
          panelEl?.querySelector(
            `[data-editor-path="${openEditorPath}"]:not([hidden])`,
          )) ||
        panelEl;
      const inp = scopeEl?.querySelector("#noet-traceability-subject-input");
      if (inp) inp.value = "";
      suggestions.hidden = true;
    });
  });
}

/**
 * Called when the subject selection changes (chip added/removed).
 * Updates the query text and step cards.
 */
function onSubjectChanged() {
  renderSubjectChips();
  // Write subjectNodes back to the currently open editor's step.
  if (currentSpec && openEditorPath !== null) {
    const resolved = resolveStepPath(currentSpec, openEditorPath);
    if (resolved?.step?.input?.Keys !== undefined) {
      resolved.step.input.Keys = subjectNodes.map((n) => ({
        Bid: { bid: n.bid },
      }));
    }
  }
  syncSpecToText();
  markDirty();
}

/**
 * Initialize subject chips from a parsed spec's subject.
 * Called when a query is loaded (e.g. from ?q= or openTraceabilityModal).
 */
function syncSubjectFromSpec(spec) {
  // Find the first step with a seed TapeFn (Keys, Bids, Corpus).
  subjectNodes = [];
  if (!spec || !spec.steps) {
    renderSubjectChips();
    return;
  }

  let seedInput = null;
  for (const step of spec.steps) {
    const input = step?.input;
    if (input?.Keys || input?.Bids || input === "Corpus") {
      seedInput = input;
      break;
    }
  }

  if (!seedInput) {
    // No explicit seed — leave subjectNodes empty.
  } else if (seedInput === "Corpus") {
    // Corpus = all nodes, no explicit subject chips.
  } else if (seedInput.Bids) {
    subjectNodes = seedInput.Bids.map((bid) => ({
      bid,
      title: labelForBid(bid),
      path: "",
    }));
  } else if (seedInput.Keys) {
    for (const key of seedInput.Keys) {
      let bid = null;
      let title = "";
      if (key.Id?.id) {
        try {
          bid = state.beliefbase?.get_bid_from_id?.(key.Id.id);
          title = key.Id.id;
        } catch (_) {
          /* skip */
        }
      } else if (key.Bref?.bref) {
        try {
          bid = state.beliefbase?.get_bid_from_bref?.(key.Bref.bref);
          title = key.Bref.bref;
        } catch (_) {
          /* skip */
        }
      } else if (key.Bid?.bid) {
        bid = key.Bid.bid;
      }
      if (bid) {
        // Resolve title from context map, or fetch it directly.
        let resolvedTitle = labelForBid(bid) || title;
        if (!resolvedTitle || resolvedTitle === bid) {
          try {
            const ctx = state.beliefbase?.get_context?.(bid);
            if (ctx?.node?.title) resolvedTitle = ctx.node.title;
          } catch (_) {
            /* skip */
          }
        }
        subjectNodes.push({
          bid,
          title: resolvedTitle,
          path: "",
        });
      }
    }
  }
  renderSubjectChips();
}

/**
 * Populate the network filter dropdown with loaded networks.
 * Uses the beliefbase's nav tree roots (each root is a network node).
 */
function populateNetworkFilter() {
  const select = panelEl?.querySelector("#noet-traceability-network-filter");
  if (!select || !state.navTree) return;

  // Keep the "All networks" option; clear the rest.
  while (select.options.length > 1) select.remove(1);

  // NavTree roots are network nodes; each has a bref and title.
  const roots = state.navTree.roots || [];
  for (const root of roots) {
    const node = state.navTree.nodes?.get?.(root) ?? state.navTree.nodes?.[root];
    if (!node) continue;
    const bref = brefFromBid(root);
    const title = node.title || bref;
    const opt = document.createElement("option");
    opt.value = bref;
    opt.textContent = title;
    select.appendChild(opt);
  }

  select.value = searchNetworkFilter;
}

// ---------------------------------------------------------------------------
// Data fetching
// ---------------------------------------------------------------------------

/**
 * Evaluate a query grammar string and populate the traceability panel.
 *
 * Parses the query text into a QuerySpec via WASM, evaluates it via
 * bb.query(), and maps the resulting BeliefGraph to rows + context.
 * This is the unified query path used by both search mode (when the
 * user types a query) and the ?q= URL parameter.
 *
 * @param {string} queryText - Query grammar string to evaluate.
 */
async function refreshFromQuery(queryText) {
  if (!state.beliefbase) return;
  if (!queryText || queryText.trim().length < 2) return;

  const BeliefBaseWasm = state.wasmModule?.BeliefBaseWasm;
  if (!BeliefBaseWasm?.parseQuery) {
    console.warn("[Traceability] parseQuery not available");
    renderError("Query parsing not available in this WASM build.");
    return;
  }

  setLoadingState(true);

  try {
    let spec;
    try {
      spec = BeliefBaseWasm.parseQuery(queryText.trim());
    } catch (e) {
      renderError("Query parse error: " + String(e));
      return;
    }

    cachedSpec = spec;
    await refreshView(spec);
  } catch (err) {
    console.error("[Traceability] Error evaluating query:", err);
    renderError(String(err));
  } finally {
    setLoadingState(false);
  }
}

/**
 * Core view-refresh: calls queryView() with the appropriate view key
 * for the current displayMode, stores the result, and renders.
 *
 * @param {object} spec - Parsed QuerySpec.
 */
async function refreshView(spec) {
  const viewKey = displayMode === "tape" ? "raw_tape" : "connectivity";

  let viewData;
  try {
    viewData = state.beliefbase.queryView(spec, viewKey, null);
  } catch (e) {
    renderError("View evaluation failed: " + String(e));
    return;
  }

  currentViewData = viewData;
  sortedNavRows = null; // rebuilt by renderNormalTable

  if (displayMode === "tape") {
    rawTapeEntries = viewData?.entries || [];
    rawTapeSelectedEntry = 0;
  } else {
    rawTapeEntries = null;
  }

  renderTable();
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

function renderSkeleton() {
  if (!panelEl) return;

  const kindCheckboxes = WEIGHT_KINDS.map(
    (k) => `
    <label class="noet-traceability__kind-label">
      <input type="checkbox" class="noet-traceability__kind-cb" data-kind="${k}"${k !== "Section" ? " checked" : ""}>
      ${escapeHtml(k)}
    </label>`,
  ).join("");

  panelEl.innerHTML = `
    <div class="noet-traceability">
      <header class="noet-traceability__header">
        <h2 id="noet-traceability-title" class="noet-traceability__title">Traceability</h2>
        <button class="noet-traceability__close" aria-label="Close traceability view">&#x2715;</button>
      </header>

      <section class="noet-traceability__query-region" aria-label="Query">
        <div class="noet-traceability__query-bar">
          <input type="text" id="noet-traceability-search-input"
            class="noet-traceability__search-input"
            placeholder="e.g. bref:0e90d9cb composed_of(2), text:install"
            aria-label="Query grammar input">
          <button id="noet-traceability-execute"
            class="noet-traceability__execute-btn"
            title="Execute query (Enter)">\u25B6 Run</button>
          <button id="noet-traceability-query-toggle"
            class="noet-traceability__view-toggle"
            aria-expanded="true"
            aria-label="Toggle query builder">\u25BC</button>
        </div>
        <div id="noet-traceability-parse-error" class="noet-traceability__parse-error" hidden></div>
        <div id="noet-traceability-query-body" class="noet-traceability__query-body">
          <div id="noet-traceability-steps" class="noet-traceability__steps">
            <!-- Step cards rendered dynamically -->
          </div>
          <select id="noet-traceability-network-filter"
            class="noet-traceability__network-filter"
            aria-label="Filter by network">
            <option value="">All networks</option>
          </select>
        </div>
      </section>

      <section class="noet-traceability__view-region" aria-label="View options">
        <div class="noet-traceability__view-header">
          <span class="noet-traceability__section-label">View</span>
          <button id="noet-traceability-view-toggle"
            class="noet-traceability__view-toggle"
            aria-expanded="true"
            aria-label="Toggle view options">\u25BC</button>
        </div>
        <div id="noet-traceability-view-body" class="noet-traceability__view-body">
          <div class="noet-traceability__display-mode">
            <label>
              <select id="noet-traceability-display-mode" class="noet-traceability__mode-select" aria-label="Display mode">
                <option value="connectivity">Connectivity</option>
                <option value="tape">Tape</option>
              </select>
            </label>
          </div>

          <div class="noet-traceability__kind-filters" role="group" aria-label="WeightKind column filters">
            ${kindCheckboxes}
          </div>
          <span class="noet-traceability__gutter-controls">
            <label class="noet-traceability__kind-label">
              <input type="checkbox" id="noet-traceability-depth-col" class="noet-traceability__depth-cb" title="Query traversal depth this node was discovered at.">
              Depth
            </label>
            <label class="noet-traceability__kind-label">
              <input type="checkbox" id="noet-traceability-order-col" class="noet-traceability__depth-cb" title="Pathmap structural order (e.g. 0.3.1).">
              Order
            </label>
          </span>

          <div class="noet-traceability__view-actions">
            <button id="noet-traceability-export-csv" class="noet-traceability__btn">CSV</button>
            <button id="noet-traceability-export-xlsx" class="noet-traceability__btn">XLSX</button>
            <button id="noet-traceability-share" class="noet-traceability__btn"
              title="Copy shareable URL to clipboard">Share</button>
            <button id="noet-traceability-embed" class="noet-traceability__btn"
              title="Copy {query} directive for embedding">Embed</button>
          </div>
        </div>
      </section>

      <div class="noet-traceability__body"
        role="region"
        aria-label="Traceability table"
        aria-live="polite">
        <p class="noet-traceability__loading" aria-busy="true">Loading&#x2026;</p>
      </div>
    </div>
  `;

  attachControlHandlers();
}

function attachControlHandlers() {
  if (!panelEl) return;

  panelEl.querySelector(".noet-traceability__close")?.addEventListener("click", () => {
    closeTraceabilityModal();
  });

  // Query input: debounced parse for step card preview (does NOT evaluate).
  // Evaluation happens on Enter key or Execute button click.
  panelEl
    .querySelector("#noet-traceability-search-input")
    ?.addEventListener("input", (e) => {
      searchQuery = e.target.value.trim();
      if (searchDebounceTimer) clearTimeout(searchDebounceTimer);
      searchDebounceTimer = setTimeout(() => {
        if (searchQuery.length < 2) {
          clearParseError();
          renderStepCards(null);
          return;
        }
        // Parse to show step cards and detect errors (no evaluation).
        const parsed = tryParseQuery(searchQuery);
        if (parsed) {
          renderStepCards(parsed);
        }
        markDirty();
      }, SEARCH_DEBOUNCE_MS);
    });

  // Enter key in the query input triggers evaluation.
  panelEl
    .querySelector("#noet-traceability-search-input")
    ?.addEventListener("keydown", (e) => {
      if (e.key === "Enter") {
        e.preventDefault();
        searchQuery = e.target.value.trim();
        executeQuery();
      }
    });

  // Execute button.
  panelEl.querySelector("#noet-traceability-execute")?.addEventListener("click", () => {
    executeQuery();
  });

  // Close suggestions when clicking outside (subject autocomplete in step editor).
  document.addEventListener("click", (e) => {
    const suggestions = panelEl?.querySelector("#noet-traceability-subject-suggestions");
    const input = panelEl?.querySelector("#noet-traceability-subject-input");
    if (suggestions && !suggestions.contains(e.target) && e.target !== input) {
      suggestions.hidden = true;
    }
  });

  // Query builder toggle: collapse/expand subject selector + step cards
  panelEl
    .querySelector("#noet-traceability-query-toggle")
    ?.addEventListener("click", () => {
      const body = panelEl.querySelector("#noet-traceability-query-body");
      const btn = panelEl.querySelector("#noet-traceability-query-toggle");
      if (body && btn) {
        const collapsed = body.hidden;
        body.hidden = !collapsed;
        btn.textContent = collapsed ? "\u25BC" : "\u25B6";
        btn.setAttribute("aria-expanded", String(collapsed));
      }
    });

  // View toggle: collapse/expand view options
  panelEl
    .querySelector("#noet-traceability-view-toggle")
    ?.addEventListener("click", () => {
      const body = panelEl.querySelector("#noet-traceability-view-body");
      const btn = panelEl.querySelector("#noet-traceability-view-toggle");
      if (body && btn) {
        const collapsed = body.hidden;
        body.hidden = !collapsed;
        btn.textContent = collapsed ? "\u25BC" : "\u25B6";
        btn.setAttribute("aria-expanded", String(collapsed));
      }
    });

  // Display mode selector
  panelEl
    .querySelector("#noet-traceability-display-mode")
    ?.addEventListener("change", (e) => {
      displayMode = e.target.value;
      const kindFilters = panelEl.querySelector(".noet-traceability__kind-filters");
      const gutterControls = panelEl.querySelector(".noet-traceability__gutter-controls");
      if (displayMode === "tape") {
        if (kindFilters) kindFilters.style.display = "none";
        if (gutterControls) gutterControls.style.display = "none";
      } else {
        if (kindFilters) kindFilters.style.display = "";
        if (gutterControls) gutterControls.style.display = "";
      }
      if (cachedSpec) {
        setLoadingState(true);
        refreshView(cachedSpec).finally(() => setLoadingState(false));
      }
    });

  // Network filter dropdown
  panelEl
    .querySelector("#noet-traceability-network-filter")
    ?.addEventListener("change", () => {
      const select = panelEl.querySelector("#noet-traceability-network-filter");
      searchNetworkFilter = select?.value || "";
      renderTable();
    });

  // WeightKind checkboxes — only re-render table, no WASM call needed
  panelEl.querySelectorAll(".noet-traceability__kind-cb").forEach((cb) => {
    cb.addEventListener("change", () => {
      if (cb.checked) {
        visibleKinds.add(cb.dataset.kind);
      } else {
        visibleKinds.delete(cb.dataset.kind);
      }
      clampFocusToVisibleColumns();
      renderTable();
    });
  });

  // Gutter column toggles (depth and pathmap order)
  panelEl
    .querySelector("#noet-traceability-depth-col")
    ?.addEventListener("change", (e) => {
      showDepthGutter = e.target.checked;
      renderTable();
    });
  panelEl
    .querySelector("#noet-traceability-order-col")
    ?.addEventListener("change", (e) => {
      showOrderGutter = e.target.checked;
      renderTable();
    });

  // Export and sharing buttons
  panelEl
    .querySelector("#noet-traceability-export-csv")
    ?.addEventListener("click", exportToCsv);
  panelEl
    .querySelector("#noet-traceability-export-xlsx")
    ?.addEventListener("click", exportToXlsx);
  panelEl.querySelector("#noet-traceability-share")?.addEventListener("click", shareUrl);
  panelEl
    .querySelector("#noet-traceability-embed")
    ?.addEventListener("click", embedDirective);
}

function setLoadingState(loading) {
  const body = panelEl?.querySelector(".noet-traceability__body");
  if (!body) return;
  if (loading) {
    body.innerHTML = `<p class="noet-traceability__loading" aria-busy="true">Loading&#x2026;</p>`;
  }
}

function renderError(msg) {
  const body = panelEl?.querySelector(".noet-traceability__body");
  if (!body) return;
  body.innerHTML = `<p class="noet-traceability__error" role="alert">Error: ${escapeHtml(msg)}</p>`;
}

function renderTable() {
  const body = panelEl?.querySelector(".noet-traceability__body");
  if (!body) return;

  const html = displayMode === "tape" ? renderRawTapeTable() : renderNormalTable();
  body.innerHTML = html;

  // Tape nav bar event handlers (rendered dynamically inside the table body).
  const tapeSelect = body.querySelector(".noet-traceability__tape-select");
  if (tapeSelect) {
    tapeSelect.addEventListener("change", (e) => {
      rawTapeSelectedEntry = parseInt(e.target.value, 10) || 0;
      renderTable();
    });
  }
  body.querySelectorAll(".noet-traceability__tape-nav").forEach((btn) => {
    btn.addEventListener("click", () => {
      const dir = btn.getAttribute("data-tape-dir");
      const max = (rawTapeEntries?.length ?? 1) - 1;
      if (dir === "prev" && rawTapeSelectedEntry > 0) {
        rawTapeSelectedEntry--;
        renderTable();
      } else if (dir === "next" && rawTapeSelectedEntry < max) {
        rawTapeSelectedEntry++;
        renderTable();
      }
    });
  });

  // Apply initial focus highlight after render (handles panel open + data refresh).
  applyFocusHighlight();

  // Event delegation: click anywhere in a cell opens the metadata panel and
  // updates keyboard-navigation focus. Handles clicks on the td background,
  // not just the inner button.
  body.querySelector(".noet-traceability__table-wrap")?.addEventListener("click", (e) => {
    // Resolve the clicked cell — either a link cell (data-cell-bid) or path cell.
    const td = e.target.closest("td");
    if (!td) return;

    // Derive the BID to show: prefer the clicked element's data-bid (role links),
    // then inner button's data-bid, then the cell's own data-cell-bid.
    const clickedLink = e.target.closest("[data-bid]");
    const btn = td.querySelector(".noet-traceability__node-link");
    const bid =
      clickedLink?.getAttribute("data-bid") ??
      btn?.getAttribute("data-bid") ??
      td.getAttribute("data-cell-bid");
    if (!bid) return;

    // Set focus from data-row-idx / data-col-idx stamped on the <td>.
    const rowIdx = td.hasAttribute("data-row-idx")
      ? parseInt(td.getAttribute("data-row-idx"), 10)
      : -1;
    const colIdx = td.hasAttribute("data-col-idx")
      ? parseInt(td.getAttribute("data-col-idx"), 10)
      : 0;
    if (rowIdx >= 0) {
      focusedRow = rowIdx;
      focusedCol = isNaN(colIdx) ? 0 : colIdx;
    }

    applyFocusHighlight();
    showMetadataPanel(bid);
  });
}

// ---------------------------------------------------------------------------
// Normal mode table
// ---------------------------------------------------------------------------

function renderNormalTable() {
  if (!currentViewData || !currentViewData.rows) {
    return `<p class="noet-traceability__empty">No results.</p>`;
  }

  const headers = currentViewData.headers || [];
  const rows = currentViewData.rows || [];

  if (rows.length === 0) {
    return `<p class="noet-traceability__empty">Query returned no results.</p>`;
  }

  // Determine which column indices to show based on visibleKinds.
  // Headers: ["Node", "Section In", "Section Out", "Epistemic In", "Epistemic Out", "Pragmatic In", "Pragmatic Out"]
  const visibleColIndices = [0]; // Node column always visible
  for (let i = 1; i < headers.length; i++) {
    const header = headers[i];
    const kind = header.split(" ")[0];
    if (visibleKinds.has(kind)) {
      visibleColIndices.push(i);
    }
  }

  // Check if a row has BID data in any visible edge column.
  const hasEdgeData = (row) => {
    const cells = row.cells || [];
    return visibleColIndices.slice(1).some((ci) => {
      const cell = cells[ci];
      return cell && typeof cell === "object" && cell.bid;
    });
  };

  const showGutter = showDepthGutter || showOrderGutter;
  const orderMap = currentViewData.order || {};
  const depthMap = currentViewData.tape_depth || {};

  // Sort rows by pathmap order. Group sub-rows (continuation rows with empty
  // cell 0) under their parent node, then sort groups by the node's order
  // string using lexicographic comparison on the dot-separated indices.
  const sortedRows = sortRowsByOrder(rows, orderMap);

  // Cache for getNavRows() so keyboard navigation matches display order.
  sortedNavRows = sortedRows;

  let html = `<div class="noet-traceability__table-wrap">`;
  html += `<table class="noet-traceability__table"><thead><tr>`;
  if (showGutter) html += `<th class="noet-traceability__gutter-col"></th>`;
  for (const ci of visibleColIndices) {
    html += `<th>${escapeHtml(headers[ci])}</th>`;
  }
  html += `</tr></thead><tbody>`;

  // Track current node BID for grouping sub-rows.
  let currentNodeBid = null;
  let rowCount = 0;

  // Pre-compute which node BIDs to skip (edgeless groups in submap mode).
  const skipBids = new Set();
  if (searchQuery.length < 2) {
    let groupBid = null;
    let groupHasEdges = false;
    for (const { row } of sortedRows) {
      const nodeCell = row.cells?.[0];
      const isFirst = nodeCell && typeof nodeCell === "object" && nodeCell.bid;
      if (isFirst) {
        // Finalize previous group.
        if (groupBid && !groupHasEdges) skipBids.add(groupBid);
        groupBid = nodeCell.bid;
        groupHasEdges = hasEdgeData(row);
      } else {
        if (!groupHasEdges) groupHasEdges = hasEdgeData(row);
      }
    }
    if (groupBid && !groupHasEdges) skipBids.add(groupBid);
  }

  for (let si = 0; si < sortedRows.length; si++) {
    const { row } = sortedRows[si];
    const ri = si; // row index for DOM attributes — matches getNavRows() position
    const cells = row.cells || [];
    const nodeCell = cells[0];
    const isFirstSubRow = nodeCell && typeof nodeCell === "object" && nodeCell.bid;

    if (isFirstSubRow) {
      currentNodeBid = nodeCell.bid;
    }

    // Skip edgeless node groups in submap mode.
    if (currentNodeBid && skipBids.has(currentNodeBid)) continue;

    const rowBid = currentNodeBid || `row-${ri}`;
    // Only stamp data-row-bid on the first sub-row of each node group.
    // Continuation sub-rows omit it so that keyboard navigation can walk
    // nextElementSibling to find the group boundary.
    if (isFirstSubRow) {
      html += `<tr data-row-bid="${escapeHtml(rowBid)}">`;
    } else {
      html += `<tr>`;
    }

    // Gutter column: depth and/or order, only on first sub-row.
    if (showGutter) {
      if (isFirstSubRow && currentNodeBid) {
        let gutterHtml = "";
        if (showDepthGutter) {
          const d = depthMap[currentNodeBid];
          if (d !== undefined) {
            gutterHtml += depthBadge(d);
          }
        }
        if (showOrderGutter) {
          const o = orderMap[currentNodeBid];
          if (o) {
            if (gutterHtml) gutterHtml += " ";
            gutterHtml += escapeHtml(o);
          }
        }
        html += `<td class="noet-traceability__gutter-col" data-row-idx="${ri}">${gutterHtml}</td>`;
      } else {
        html += `<td class="noet-traceability__gutter-col" data-row-idx="${ri}"></td>`;
      }
    }

    for (const ci of visibleColIndices) {
      const cell = cells[ci];
      if (cell && typeof cell === "object" && cell.bid) {
        html += `<td class="noet-traceability__cell" data-row-idx="${ri}" data-col-idx="${ci}" data-cell-bid="${escapeHtml(cell.bid)}">`;
        html += renderEntryButton(cell.bid);
        html += `</td>`;
      } else {
        html += `<td class="noet-traceability__cell" data-row-idx="${ri}" data-col-idx="${ci}"></td>`;
      }
    }

    html += `</tr>`;
    rowCount++;
  }

  if (rowCount === 0) {
    const totalCols = visibleColIndices.length + (showGutter ? 1 : 0);
    html += `<tr><td colspan="${totalCols}">No nodes with visible edges.</td></tr>`;
  }

  html += `</tbody></table></div>`;
  return html;
}

// ---------------------------------------------------------------------------
// Tape mode table
// ---------------------------------------------------------------------------

/**
 * Render the selected raw tape entry as an HTML table.
 * @returns {string} HTML string
 */
function renderRawTapeTable() {
  if (!rawTapeEntries || rawTapeEntries.length === 0) {
    return `<p class="noet-traceability__empty">No tape entries. Run a query first.</p>`;
  }

  const idx = Math.min(rawTapeSelectedEntry, rawTapeEntries.length - 1);
  const entry = rawTapeEntries[idx];
  if (!entry) {
    return `<p class="noet-traceability__empty">Invalid tape entry index.</p>`;
  }

  const headers = entry.headers || [];
  const rows = entry.rows || [];

  // Build option elements for the selector with the current entry selected.
  let options = "";
  for (let i = 0; i < rawTapeEntries.length; i++) {
    const e = rawTapeEntries[i];
    const stepOp = e.step_operation ? ` ${e.step_operation}` : "";
    const label = `[${i}] ${e.step_label}${stepOp} (${e.content_type})`;
    const sel = i === idx ? " selected" : "";
    options += `<option value="${i}"${sel}>${escapeHtml(label)}</option>`;
  }

  let html = `<div class="noet-traceability__table-wrap">`;
  html += `<div class="noet-traceability__tape-nav-bar">`;
  html += `<button class="noet-traceability__tape-nav" data-tape-dir="prev" aria-label="Previous tape entry">◀</button>`;
  html += `<select class="noet-traceability__tape-select" aria-label="Tape entry">${options}</select>`;
  html += `<button class="noet-traceability__tape-nav" data-tape-dir="next" aria-label="Next tape entry">▶</button>`;
  html += `</div>`;
  html += `<table class="noet-traceability__table">`;
  html += `<thead><tr>`;
  for (const h of headers) {
    html += `<th>${escapeHtml(h)}</th>`;
  }
  html += `</tr></thead><tbody>`;

  for (let ri = 0; ri < rows.length; ri++) {
    const row = rows[ri];
    // Use a unique row ID combining the tape entry index and row index
    // to avoid duplicates when multiple rows share the same BID.
    const firstBidCell = (row.cells || []).find(
      (c) => c && typeof c === "object" && (c.bid || c.edge),
    );
    const cellBid = firstBidCell?.bid || firstBidCell?.edge?.source?.bid || null;
    const rowId = `tape-${idx}-${ri}`;
    const rowAttr = ` data-row-bid="${escapeHtml(rowId)}" data-row-cell-bid="${escapeHtml(cellBid || "")}"`;
    html += `<tr${rowAttr}>`;
    for (let ci = 0; ci < (row.cells || []).length; ci++) {
      const cell = row.cells[ci];
      if (cell && typeof cell === "object" && cell.edge) {
        // Edge cell: KIND(s), KIND(k), KIND(@), or KIND(s/k)
        const e = cell.edge;
        html += `<td class="noet-traceability__cell noet-traceability__edge-cell" data-row-idx="${ri}" data-col-idx="${ci}">`;
        html += `${escapeHtml(e.kind)}(`;
        if (e.owner?.bid) {
          html += renderRoleLink("@", e.owner.bid);
        } else if (e.owned_by === "source") {
          html += renderRoleLink("s", e.source?.bid);
        } else if (e.owned_by === "sink") {
          html += renderRoleLink("k", e.sink?.bid);
        } else {
          html += `<span title="${escapeHtml(e.error || "missing owned_by")}">⚠</span>`;
        }
        html += `)`;
        html += `</td>`;
      } else if (cell && typeof cell === "object" && cell.bid) {
        // Node reference cell: render as clickable button.
        const cellBid = cell.bid;
        html += `<td class="noet-traceability__cell" data-row-idx="${ri}" data-col-idx="${ci}" data-cell-bid="${escapeHtml(cellBid)}">`;
        html += renderEntryButton(cellBid);
        html += `</td>`;
      } else {
        // Plain text cell.
        html += `<td class="noet-traceability__cell" data-row-idx="${ri}" data-col-idx="${ci}">`;
        html += escapeHtml(typeof cell === "string" ? cell : String(cell ?? ""));
        html += `</td>`;
      }
    }
    html += `</tr>`;
  }

  html += `</tbody></table></div>`;
  return html;
}

/**
 * Render a role label ("s", "k", "@") as a clickable link to a node.
 * Shows the role letter; tooltip shows the node title.
 * @param {string} role - "s", "k", or "@"
 * @param {string|null} bid - Node BID
 * @returns {string} HTML fragment
 */
function renderRoleLink(role, bid) {
  if (!bid) return escapeHtml(role);
  const title = labelForBid(bid);
  return `<button class="noet-traceability__node-link noet-traceability__role-link" data-bid="${escapeHtml(bid)}" title="${escapeHtml(title)}">${escapeHtml(role)}</button>`;
}

// ---------------------------------------------------------------------------
// Entry button
// ---------------------------------------------------------------------------

/**
 * Sort view rows by pathmap order, keeping sub-row groups together.
 *
 * Groups rows by node BID (first sub-row has a BID cell at index 0;
 * continuations have empty cell 0). Sorts groups by the node's order
 * string using numeric-aware comparison on dot-separated segments
 * (e.g. "1.3" < "2.0" < "17.0").
 *
 * Returns a flat array of `{ row, originalIdx }` preserving sub-row
 * order within each group.
 *
 * @param {Array} rows - flat rows array from view data
 * @param {Object} orderMap - { bid_string → "0.3.1" }
 * @returns {Array<{ row: object, originalIdx: number }>}
 */
function sortRowsByOrder(rows, orderMap) {
  // Group rows: each group = { bid, entries: [{ row, originalIdx }] }
  const groups = [];
  for (let i = 0; i < rows.length; i++) {
    const row = rows[i];
    const nodeCell = row.cells?.[0];
    const isFirst = nodeCell && typeof nodeCell === "object" && nodeCell.bid;
    if (isFirst || groups.length === 0) {
      groups.push({
        bid: isFirst ? nodeCell.bid : null,
        entries: [{ row, originalIdx: i }],
      });
    } else {
      groups[groups.length - 1].entries.push({ row, originalIdx: i });
    }
  }

  // Parse an order string into a numeric array for comparison.
  const parseOrder = (s) => {
    if (!s) return [Infinity];
    return s.split(".").map((seg) => {
      const n = parseInt(seg, 10);
      return isNaN(n) ? 0xffff : n; // · (gateway) parses as NaN → 0xFFFF (sorts last within level)
    });
  };

  // Compare two order arrays lexicographically.
  const compareOrder = (a, b) => {
    const oa = parseOrder(orderMap[a]);
    const ob = parseOrder(orderMap[b]);
    for (let i = 0; i < Math.max(oa.length, ob.length); i++) {
      const va = i < oa.length ? oa[i] : -1;
      const vb = i < ob.length ? ob[i] : -1;
      if (va !== vb) return va - vb;
    }
    return 0;
  };

  groups.sort((a, b) => compareOrder(a.bid, b.bid));
  return groups.flatMap((g) => g.entries);
}

/**
 * Render a clickable button for a BID in a table cell.
 * The button label is the node title resolved via `labelForBid(bid)`.
 *
 * @param {string} bid - BID of the node to render
 * @returns {string} HTML fragment
 */
function renderEntryButton(bid) {
  const label = labelForBid(bid);
  return `<button class="noet-traceability__node-link" data-bid="${escapeHtml(bid)}"><span class="noet-traceability__ref-label">${escapeHtml(label)}</span></button>`;
}

/**
 * Render a depth value as a heatmap-colored badge.
 * 0 = cool (blue/teal), higher values = warmer (orange/red).
 * Uses HSL interpolation: hue 200 (cool) → 0 (hot) over range 0..8,
 * clamped so depths ≥ 8 are fully hot.
 *
 * @param {number} depth
 * @returns {string} HTML fragment
 */
function depthBadge(depth) {
  const maxDepth = 8;
  const t = Math.min(depth / maxDepth, 1); // 0..1
  // Hue: 200 (blue) → 0 (red)
  const hue = Math.round(200 * (1 - t));
  const bg = `hsl(${hue}, 60%, 85%)`;
  const fg = `hsl(${hue}, 70%, 25%)`;
  return `<span class="noet-traceability__depth-badge" style="background:${bg};color:${fg}" title="Query depth: ${depth}">${depth}</span>`;
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/**
 * Derive a human-readable label for a BID.
 * Checks currentViewData.nodes first, then falls back to the raw BID.
 * @param {string} bid
 * @returns {string}
 */
function labelForBid(bid) {
  const nodeInfo = currentViewData?.nodes?.[bid];
  if (nodeInfo) {
    const title = nodeInfo.title?.trim() ?? "";
    if (title.length > 0) return title;
    if (nodeInfo.id) return nodeInfo.id;
  }
  return brefFromBid(bid);
}

/**
 * Build export rows from a single view data section (headers + rows).
 * Returns an array of plain objects: [headerRow, ...dataRows].
 * Each object is keyed by column header strings.
 * BID cells are resolved to text via currentViewData.nodes.
 */
function buildExportRowsFromSection(viewData) {
  if (!viewData || !viewData.headers || !viewData.rows) return [];

  const headers = viewData.headers;
  const rows = viewData.rows;

  const headerRow = {};
  for (const h of headers) {
    headerRow[h] = h;
  }

  const seen = new Set();
  const dataRows = rows.map((row) => {
    const obj = {};
    const cells = row.cells || [];
    for (let i = 0; i < headers.length; i++) {
      const cell = cells[i];
      if (cell && typeof cell === "object" && cell.bid) {
        obj[headers[i]] = exportCellLabel(cell.bid, seen);
      } else if (cell && typeof cell === "object" && cell.edge) {
        const e = cell.edge;
        const role =
          e.owned_by === "source"
            ? "s"
            : e.owned_by === "sink"
              ? "k"
              : e.owner?.bid
                ? "@"
                : "\u26A0";
        obj[headers[i]] = `${e.kind}(${role})`;
      } else {
        obj[headers[i]] = typeof cell === "string" ? cell : "";
      }
    }
    return obj;
  });

  return [headerRow, ...dataRows];
}

/**
 * Build export rows for the current view.
 * For connectivity mode: single section from currentViewData.
 * For tape mode: all tape entries concatenated with delimiter rows.
 */
function buildExportRows() {
  if (displayMode === "tape" && rawTapeEntries && rawTapeEntries.length > 0) {
    return buildTapeExportRows();
  }
  return buildExportRowsFromSection(currentViewData);
}

/**
 * Build export rows for tape mode: all entries concatenated.
 * Each entry is preceded by a delimiter row showing the step label and index.
 */
function buildTapeExportRows() {
  const allRows = [];
  for (let i = 0; i < rawTapeEntries.length; i++) {
    const entry = rawTapeEntries[i];
    const stepOp = entry.step_operation ? ` ${entry.step_operation}` : "";
    const label = `[${i}] ${entry.step_label}${stepOp} (${entry.content_type})`;

    const sectionRows = buildExportRowsFromSection(entry);
    if (sectionRows.length === 0) continue;

    // Insert a delimiter row using the first section's column keys.
    if (allRows.length > 0) {
      // Add an empty separator row before each new section.
      const emptyRow = {};
      for (const k of Object.keys(sectionRows[0])) emptyRow[k] = "";
      allRows.push(emptyRow);
    }

    // Delimiter row: first column gets the label, rest empty.
    const delimiterRow = {};
    const keys = Object.keys(sectionRows[0]);
    for (const k of keys) delimiterRow[k] = "";
    delimiterRow[keys[0]] = label;
    allRows.push(delimiterRow);

    // Append header + data rows for this entry.
    allRows.push(...sectionRows);
  }
  return allRows;
}

/**
 * Resolve a BID to export text.
 *
 * First occurrence: "label\ntitle\ntext" — label (title or id or bref),
 * title, and body text (from get_context), newline-separated, with empty
 * segments filtered out.
 *
 * Subsequent occurrences: just the label.
 *
 * @param {string} bid
 * @param {Set<string>} seen - mutated in place; tracks already-rendered BIDs
 * @returns {string}
 */
function exportCellLabel(bid, seen) {
  const nodeInfo = currentViewData?.nodes?.[bid];
  const title = nodeInfo?.title?.trim() || "";
  const label = title || nodeInfo?.id || brefFromBid(bid);
  if (!seen.has(bid)) {
    seen.add(bid);
    // Fetch body text via get_context for full export fidelity.
    let text = "";
    try {
      const ctx = state.beliefbase?.get_context?.(bid);
      if (ctx?.node?.payload?.text) {
        text = String(ctx.node.payload.text).trim();
      }
    } catch (_) {
      // Shard not loaded or node unresolvable — skip body text.
    }
    return [label, title, text].filter(Boolean).join("\n");
  }
  return label;
}

/**
 * Convert the current table to CSV and trigger a browser download.
 */
function exportToCsv() {
  const exportRows = buildExportRows();
  if (exportRows.length === 0) return;

  const keys = Object.keys(exportRows[0]);

  const lines = exportRows.map((row) => {
    return keys
      .map((k) => {
        const raw = row[k] === undefined || row[k] === null ? "" : String(row[k]);
        // Escape newlines and double-quotes; wrap in double-quotes
        const escaped = raw
          .replace(/"/g, '""')
          .replace(/\n/g, "\\n")
          .replace(/\r/g, "\\r");
        return `"${escaped}"`;
      })
      .join(",");
  });

  const csvText = lines.join("\n");
  const blob = new Blob([csvText], { type: "text/csv;charset=utf-8;" });
  const url = URL.createObjectURL(blob);

  const a = document.createElement("a");
  a.href = url;
  a.download = "traceability.csv";
  a.style.display = "none";
  document.body.appendChild(a);
  a.click();
  document.body.removeChild(a);

  // Clean up the object URL after the click has been processed
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}

/**
 * Convert the current table to XLSX and trigger a browser download.
 *
 * For connectivity mode: single sheet via `export_xlsx()`.
 * For tape mode: one sheet per tape entry via `export_xlsx_multi()`.
 */
function exportToXlsx() {
  const BeliefBaseWasm = state.wasmModule?.BeliefBaseWasm;
  if (typeof BeliefBaseWasm?.export_xlsx !== "function") {
    console.error("exportToXlsx: BeliefBaseWasm.export_xlsx is not available");
    return;
  }

  let bytes;

  if (
    displayMode === "tape" &&
    rawTapeEntries &&
    rawTapeEntries.length > 0 &&
    typeof BeliefBaseWasm.export_xlsx_multi === "function"
  ) {
    // Multi-sheet: one worksheet per tape entry.
    const sheets = [];
    for (let i = 0; i < rawTapeEntries.length; i++) {
      const entry = rawTapeEntries[i];
      const sectionRows = buildExportRowsFromSection(entry);
      if (sectionRows.length === 0) continue;

      const keys = Object.keys(sectionRows[0]);
      const stepOp = entry.step_operation ? ` ${entry.step_operation}` : "";
      // Sheet name: "[idx] label" — truncated by Rust to 31 chars.
      const name = `[${i}] ${entry.step_label}${stepOp}`;

      const rowsArray = sectionRows.map((row) => {
        const obj = {};
        for (const k of keys) {
          obj[k] = row[k] === undefined || row[k] === null ? "" : String(row[k]);
        }
        return obj;
      });

      sheets.push({ name, headers: keys, rows: rowsArray });
    }

    if (sheets.length === 0) return;
    bytes = BeliefBaseWasm.export_xlsx_multi(sheets);
  } else {
    // Single sheet: connectivity mode or tape fallback.
    const exportRows = buildExportRows();
    if (exportRows.length === 0) return;

    const keys = Object.keys(exportRows[0]);
    const rowsArray = exportRows.map((row) => {
      const obj = {};
      for (const k of keys) {
        obj[k] = row[k] === undefined || row[k] === null ? "" : String(row[k]);
      }
      return obj;
    });

    bytes = BeliefBaseWasm.export_xlsx(keys, rowsArray);
  }

  if (!bytes || bytes.length === 0) {
    console.error("exportToXlsx: WASM returned empty buffer");
    return;
  }

  const blob = new Blob([bytes], {
    type: "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
  });
  const url = URL.createObjectURL(blob);

  const a = document.createElement("a");
  a.href = url;
  a.download = "traceability.xlsx";
  a.style.display = "none";
  document.body.appendChild(a);
  a.click();
  document.body.removeChild(a);

  setTimeout(() => URL.revokeObjectURL(url), 1000);
}

// ---------------------------------------------------------------------------
// Share and Embed
// ---------------------------------------------------------------------------

/**
 * Build a query string representing the current panel state.
 * In search mode: returns the search query text.
 * In submap mode: returns a textual anchor reference.
 *
 * @returns {string}
 */
/**
 * Convert a user-typed search string into query grammar syntax.
 *
 * If the input already looks like query grammar (contains field:term,
 * bref:, id://, operators like AND/NOT, or traversal steps like k-/s()), it is
 * returned as-is. Otherwise it is wrapped as `text:"..."` for a
 * corpus-wide TextMatch.
 *
 * @param {string} raw - User input from the search box.
 * @returns {string} Query grammar string.
 */
function searchQueryToGrammar(raw) {
  const trimmed = raw.trim();
  if (!trimmed) return "";
  // If it already looks like query grammar, pass through.
  if (
    /^(?:id:\/\/|bref:|bid:|KEYS\(|CORPUS\(|BIDS\()|[:,()\[\]]|\b(AND|OR|NOT|k-|s\()/.test(
      trimmed,
    )
  ) {
    return trimmed;
  }
  // Wrap as text:"..." for full-text search.
  if (trimmed.includes(" ") || trimmed.includes('"')) {
    const escaped = trimmed.replace(/"/g, '\\"');
    return `text:"${escaped}"`;
  }
  return `text:${trimmed}`;
}

function buildQueryText() {
  if (searchQuery.length >= 2) {
    return searchQuery.trim();
  }
  // Fallback: build a bid: anchor for the current entry point.
  const bid = currentEntryBid || currentHomeNetBid;
  if (!bid) return "";
  return `bid:${bid}`;
}

/**
 * Copy the current URL with query state to the clipboard.
 * Builds a URL from the current hash route + a `?q=` parameter.
 */
function shareUrl() {
  const queryText = buildQueryText();
  if (!queryText) {
    showToast("Nothing to share \u2014 open a query first.");
    return;
  }

  const url = new URL(window.location.href);
  url.searchParams.set("q", queryText);
  navigator.clipboard
    .writeText(url.toString())
    .then(() => showToast("URL copied to clipboard"))
    .catch(() => showToast("Failed to copy URL"));
}

/**
 * Copy a `{query}` directive suitable for embedding in a document.
 * The directive uses the current query text with an explicit bref: anchor
 * when in submap mode.
 */
function embedDirective() {
  const queryText = buildQueryText();
  if (!queryText) {
    showToast("Nothing to embed \u2014 open a query first.");
    return;
  }

  const directive = "```{query}\n" + queryText + "\n```";
  navigator.clipboard
    .writeText(directive)
    .then(() => showToast("{query} directive copied to clipboard"))
    .catch(() => showToast("Failed to copy directive"));
}

/**
 * Show a brief toast notification near the traceability panel header.
 *
 * @param {string} message
 */
function showToast(message) {
  const existing = panelEl?.querySelector(".noet-traceability__toast");
  if (existing) existing.remove();

  const toast = document.createElement("div");
  toast.className = "noet-traceability__toast";
  toast.textContent = message;
  toast.setAttribute("role", "status");
  toast.setAttribute("aria-live", "polite");

  const header = panelEl?.querySelector(".noet-traceability__header");
  if (header) {
    header.appendChild(toast);
  }

  setTimeout(() => toast.remove(), 2500);
}
