/**
 * viewer/search.js — Full-text search delegated to WASM
 *
 * Scoring is handled entirely by `beliefbase.search(query, limit)` in WASM.
 * Search indices are loaded as msgpack binaries via
 * `beliefbase.load_search_index(bref, bytes)` during WASM init (both sharded
 * and monolithic modes). The JS layer is responsible only for:
 *   - Tokenizing the query for snippet highlighting
 *   - Extracting text snippets from loaded WASM data shards
 *   - Rendering the results panel and handling keyboard navigation
 *
 * ## Architecture
 *
 * Query pipeline:
 *   1. Delegate to `beliefbase.search(query, MAX_RESULTS)` → JSON string of
 *      [{bid, network_bref, title, path, score}, ...]
 *   2. Tokenize the raw query (for snippet term highlighting only)
 *   3. Enrich each result with a snippet from the loaded data shard (best-effort)
 *   4. Render results panel
 *
 * ## Migration path to WASM — COMPLETE
 *
 * Issue 54 is resolved. `BeliefBaseWasm.load_search_index()` / `.search()` are
 * live. The old JS TF-IDF engine (levenshtein, _fuzzyMatches, _accumulateTerm)
 * has been removed. The UI layer (input, results panel, keyboard nav) is unchanged.
 *
 * ## Keyboard navigation
 *
 *   ↑ / ↓      — move selection through results
 *   Enter      — navigate to selected result
 *   Escape     — close results panel
 *   Ctrl+K     — focus search input from anywhere
 *
 * ## References
 *
 * - docs/design/search_and_sharding.md §7 — Search index format and query model
 * - src/shard/search.rs — Compile-time index builder (tokenizer, stemmer, weights)
 * - assets/viewer/shard-manager.js — search index loading (msgpack → WASM)
 * - assets/viewer/wasm.js — WASM init and beliefbase construction
 */

import { state, callbacks } from "./state.js";
import { escapeHtml, brefFromBid } from "./utils.js";

// =============================================================================
// Constants
// =============================================================================

/** Maximum number of results to return and render. */
const MAX_RESULTS = 20;

/** Debounce delay in milliseconds before running a query after input. */
const DEBOUNCE_MS = 250;

/** Minimum query length before running a search (avoids trivial results). */
const MIN_QUERY_LENGTH = 2;

/** Approximate characters to show around a match in a snippet. */
const SNIPPET_WINDOW = 120;

// Stop-words for query tokenization (subset of compile-time list — short list
// sufficient for query terms; compile-time list is authoritative for index terms).
const QUERY_STOP_WORDS = new Set([
  "a",
  "an",
  "the",
  "and",
  "or",
  "but",
  "in",
  "on",
  "at",
  "to",
  "for",
  "of",
  "with",
  "by",
  "from",
  "is",
  "was",
  "are",
  "were",
  "be",
  "been",
  "being",
  "have",
  "has",
  "had",
  "do",
  "does",
  "did",
  "will",
  "would",
  "could",
  "should",
  "may",
  "might",
  "shall",
  "can",
  "not",
  "no",
  "nor",
  "so",
  "yet",
  "both",
  "either",
  "it",
  "its",
  "this",
  "that",
  "these",
  "those",
  "i",
  "me",
  "my",
  "we",
  "our",
  "you",
  "your",
  "he",
  "she",
  "they",
  "them",
  "their",
  "what",
  "which",
  "who",
  "whom",
  "as",
]);

// =============================================================================
// Module state
// =============================================================================

/** @type {number|null} Pending debounce timer id. */
let _debounceTimer = null;

/** @type {HTMLElement|null} The results panel container. */
let _resultsPanel = null;

/** @type {number} Index of the currently keyboard-selected result row (-1 = none). */
let _selectedIndex = -1;

/** @type {SearchResult[]} The last computed result set, for keyboard navigation. */
let _currentResults = [];

// =============================================================================
// Public API
// =============================================================================

/**
 * Initialize search: wire up the input element, create the results panel,
 * and attach all event listeners. Safe to call after WASM init completes.
 *
 * Expects state.searchInput to be set by initializeDOMReferences() in viewer.js.
 */
export function initSearch() {
  if (!state.searchInput) {
    console.warn("[Search] searchInput element not found — search unavailable");
    return;
  }

  _resultsPanel = _createResultsPanel();
  _attachInputListeners();
  _attachGlobalListeners();

  const loadedCount = state.beliefbase
    ? state.beliefbase.loaded_search_indices().size
    : 0;
  console.log(`[Search] Initialized. Indices loaded: ${loadedCount} network(s).`);
}

// =============================================================================
// Tokenizer (mirrors src/shard/search.rs::tokenize)
// =============================================================================

/**
 * Tokenize a query string into search terms.
 *
 * Rules (must stay in sync with compile-time tokenizer in search.rs):
 *   - Lowercase
 *   - Split on whitespace and punctuation (keep apostrophes for contractions)
 *   - Drop tokens shorter than 3 characters
 *   - Drop pure-numeric tokens
 *   - Remove stop words
 *
 * Used on the query side for snippet term highlighting only — scoring is handled
 * in WASM where identical stemming is applied automatically.
 *
 * @param {string} text
 * @returns {string[]} Deduplicated array of normalised terms
 */
function tokenize(text) {
  if (!text) return [];

  // Strip query syntax before tokenizing for highlighting:
  // - Remove boolean operators (AND, OR, NOT)
  // - Remove field prefixes (field:)
  // - Remove id:// anchors
  // - Remove traversal syntax (k-..., s(...))
  let cleaned = text
    .replace(/\b(AND|OR|NOT)\b/g, " ")
    .replace(/id:\/\/\S+/g, " ")
    .replace(/\b[a-z]+:/gi, " ")
    .replace(/[ks]-[a-z]+-[a-z]+\([^)]*\)/gi, " ")
    .replace(/[ks]-[a-z]+\([^)]*\)/gi, " ");

  // Split on anything that isn't a letter, digit, or apostrophe
  const raw = cleaned
    .toLowerCase()
    .split(/[^a-z0-9']+/)
    .map((t) => t.replace(/^'+|'+$/g, "")); // strip leading/trailing apostrophes

  const seen = new Set();
  const result = [];

  for (const tok of raw) {
    if (tok.length < 3) continue;
    if (/^\d+$/.test(tok)) continue;
    if (QUERY_STOP_WORDS.has(tok)) continue;
    if (seen.has(tok)) continue;
    seen.add(tok);
    result.push(tok);
  }

  return result;
}

// =============================================================================
// Query engine
// =============================================================================

/**
 * @typedef {Object} SearchResult
 * @property {string} bid
 * @property {string} networkBref
 * @property {string} title
 * @property {string} path        — HTML-relative path (may be empty for network roots)
 * @property {string} snippet     — Content excerpt; empty string for unloaded networks
 * @property {number} score       — TF-IDF relevance score (computed in WASM)
 * @property {boolean} loaded     — Whether the network's data shard is currently loaded
 */

/**
 * Run a query across all loaded search indices via WASM.
 *
 * @param {string} query — Raw query string from the input
 * @returns {SearchResult[]} Top MAX_RESULTS results sorted descending by score
 */
function runQuery(query) {
  if (!state.beliefbase) return [];

  // Delegate TF-IDF scoring to WASM (search indices loaded via load_search_index).
  // Returns JSON string: [{bid, network_bref, title, path, score}, ...]
  let rawResults;
  try {
    rawResults = JSON.parse(state.beliefbase.search(query, MAX_RESULTS));
  } catch (e) {
    console.warn("[search] WASM search failed:", e);
    return [];
  }

  if (!rawResults || rawResults.length === 0) return [];

  // Determine which networks have loaded data shards (for snippet extraction).
  const loadedBrefs = _getLoadedBrefs();

  // Enrich with snippets where data shard is available.
  // WASM returns network_bref; JS calls it networkBref — normalise.
  const terms = tokenize(query);
  return rawResults.map((r) => {
    const loaded = loadedBrefs.has(r.network_bref);
    const snippet = loaded ? _extractSnippet(r.bid, terms) : "";
    return {
      bid: r.bid,
      networkBref: r.network_bref,
      title: r.title,
      path: r.path,
      snippet,
      score: r.score,
      loaded,
    };
  });
}

/**
 * Returns a Set of brefs whose data shards are currently loaded.
 * Works for both sharded mode (ShardManager) and monolithic mode (all loaded).
 *
 * @returns {Set<string>}
 */
function _getLoadedBrefs() {
  if (!state.beliefbase) return new Set();

  if (state.shardManager) {
    // Sharded mode: ask ShardManager which networks are loaded.
    return new Set(state.shardManager.getLoadedNetworks());
  }

  // Monolithic mode: everything is loaded — return all brefs from WASM.
  return new Set(state.beliefbase.loaded_search_indices());
}

/**
 * Extract a ~SNIPPET_WINDOW-character excerpt from a node's payload["text"]
 * centred on the first occurrence of any query term. Falls back to the first
 * SNIPPET_WINDOW characters if no term is found.
 *
 * Requires the node's data shard to be loaded (caller must check).
 *
 * @param {string} bid
 * @param {string[]} terms — Tokenized query terms
 * @returns {string} Plain-text snippet (not HTML-escaped — caller escapes on render)
 */
function _extractSnippet(bid, terms) {
  if (!state.beliefbase) return "";

  try {
    const nodeJs = state.beliefbase.get_by_bid(bid);
    if (!nodeJs) return "";

    // get_by_bid returns a serialized BeliefNode; payload is a plain JS object.
    const text = nodeJs.payload && nodeJs.payload.text ? nodeJs.payload.text : "";
    if (!text) return "";

    // Find the earliest match position of any query term in the text.
    const lower = text.toLowerCase();
    let bestPos = -1;
    for (const term of terms) {
      const pos = lower.indexOf(term);
      if (pos !== -1 && (bestPos === -1 || pos < bestPos)) {
        bestPos = pos;
      }
    }

    const start =
      bestPos === -1 ? 0 : Math.max(0, bestPos - Math.floor(SNIPPET_WINDOW / 2));
    const end = Math.min(text.length, start + SNIPPET_WINDOW);
    let excerpt = text.slice(start, end).replace(/\s+/g, " ").trim();

    if (start > 0) excerpt = "…" + excerpt;
    if (end < text.length) excerpt = excerpt + "…";

    return excerpt;
  } catch (_e) {
    return "";
  }
}

// =============================================================================
// DOM: results panel
// =============================================================================

/**
 * Create and insert the results panel into the nav panel, immediately after
 * the search input's wrapper element. The panel is hidden by default.
 *
 * @returns {HTMLElement}
 */
function _createResultsPanel() {
  const panel = document.createElement("div");
  panel.id = "search-results";
  panel.className = "noet-search-results";
  panel.setAttribute("role", "listbox");
  panel.setAttribute("aria-label", "Search results");
  panel.hidden = true;

  // The template structure is:
  //   .noet-nav__content
  //     .noet-nav-header        ← contains search input
  //     #nav-content            ← nav tree lives here
  //
  // Insert the results panel between the header and the nav tree so it
  // appears directly below the search input without displacing nav content.
  const navOuter = state.searchInput.closest(".noet-nav__content");
  if (navOuter) {
    const header = navOuter.querySelector(".noet-nav-header");
    if (header) {
      header.insertAdjacentElement("afterend", panel);
    } else {
      navOuter.prepend(panel);
    }
  }

  return panel;
}

/**
 * Render search results into the panel and make it visible.
 * An empty results array shows a "no results" message.
 *
 * @param {SearchResult[]} results
 * @param {string} query — Original query string for highlighting
 */
function _renderResults(results, query) {
  if (!_resultsPanel) return;

  _selectedIndex = -1;

  if (results.length === 0) {
    _resultsPanel.innerHTML = `
      <p class="noet-search-results__empty">No results for <em>${escapeHtml(query)}</em></p>
    `;
    _resultsPanel.hidden = false;
    return;
  }

  const items = results.map((r, i) => _renderResultItem(r, i, query)).join("");
  _resultsPanel.innerHTML = `<ul class="noet-search-results__list" role="listbox">${items}</ul>`;
  _resultsPanel.hidden = false;
  _currentResults = results;
}

/**
 * Render a single result row.
 *
 * @param {SearchResult} result
 * @param {number} index — Position in results array (for keyboard nav data-index)
 * @param {string} query — For term highlighting in snippet
 * @returns {string} HTML string
 */
function _renderResultItem(result, index, query) {
  const title = escapeHtml(result.title || "(untitled)");
  const path = result.path ? escapeHtml(result.path) : "";

  // Highlight query terms in snippet.
  let snippetHtml = "";
  if (result.snippet) {
    snippetHtml = _highlightTerms(escapeHtml(result.snippet), tokenize(query));
  }

  const unloadedBadge = !result.loaded
    ? `<span class="noet-search-result__badge" title="Data not loaded — search index only">idx</span>`
    : "";

  const href = result.path ? `/#${result.path}` : "";

  // Explore button opens the traceability panel anchored to this result.
  const exploreBid = escapeHtml(result.bid);
  const exploreNet = escapeHtml(result.networkBref || "");
  const exploreBtn = `<button class="noet-search-result__explore"
    data-bid="${exploreBid}" data-network-bref="${exploreNet}"
    title="Explore in traceability panel (e)"
    tabindex="-1" aria-label="Explore ${title} in traceability panel (press e)"
    >&#x1F50D;</button>`;

  return `
    <li class="noet-search-result"
        role="option"
        data-index="${index}"
        data-bid="${escapeHtml(result.bid)}"
        data-href="${escapeHtml(href)}"
        aria-selected="false">
      <a class="noet-search-result__link"
         href="${escapeHtml(href)}"
         tabindex="-1">
        <span class="noet-search-result__title">${title}${unloadedBadge}</span>
        ${path ? `<span class="noet-search-result__path">${path}</span>` : ""}
        ${snippetHtml ? `<span class="noet-search-result__snippet">${snippetHtml}</span>` : ""}
      </a>
      ${exploreBtn}
    </li>
  `;
}

/**
 * Wrap occurrences of each term in `<mark>` tags inside an already-escaped HTML string.
 * Only matches whole-word-ish boundaries to avoid splitting tags.
 *
 * @param {string} html — Already HTML-escaped text
 * @param {string[]} terms
 * @returns {string}
 */
function _highlightTerms(html, terms) {
  let result = html;
  for (const term of terms) {
    // Use a simple case-insensitive replace; avoid regex special chars in term.
    const escaped = term.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
    const re = new RegExp(`(${escaped})`, "gi");
    result = result.replace(re, "<mark>$1</mark>");
  }
  return result;
}

/**
 * Hide the results panel and reset selection state.
 */
function _closeResults() {
  if (_resultsPanel) {
    _resultsPanel.hidden = true;
    _resultsPanel.innerHTML = "";
  }
  _selectedIndex = -1;
  _currentResults = [];
  if (state.searchInput) {
    state.searchInput.setAttribute("aria-expanded", "false");
  }
}

// =============================================================================
// Keyboard navigation
// =============================================================================

/**
 * Move the keyboard selection by `delta` (+1 down, -1 up).
 * Clamps to [0, results.length - 1].
 */
function _moveSelection(delta) {
  if (!_resultsPanel || _resultsPanel.hidden) return;
  const items = _resultsPanel.querySelectorAll(".noet-search-result");
  if (items.length === 0) return;

  // Deselect current
  if (_selectedIndex >= 0 && _selectedIndex < items.length) {
    items[_selectedIndex].classList.remove("is-selected");
    items[_selectedIndex].setAttribute("aria-selected", "false");
  }

  _selectedIndex = Math.max(0, Math.min(items.length - 1, _selectedIndex + delta));

  const next = items[_selectedIndex];
  next.classList.add("is-selected");
  next.setAttribute("aria-selected", "true");
  next.scrollIntoView({ block: "nearest" });
}

/**
 * Activate the currently selected result (navigate to it).
 */
function _activateSelected() {
  if (_selectedIndex < 0 || _selectedIndex >= _currentResults.length) return;
  const result = _currentResults[_selectedIndex];
  _navigateToResult(result);
}

/**
 * Navigate to a search result. Handles loaded and unloaded-network cases.
 *
 * @param {SearchResult} result
 */
function _navigateToResult(result) {
  _closeResults();
  if (state.searchInput) state.searchInput.blur();

  if (!result.path) return;

  if (!result.loaded && state.shardManager) {
    console.log(
      `[Search] Loading network '${result.networkBref}' for result navigation…`,
    );
    state.shardManager
      .loadNetwork(result.networkBref)
      .then(() => {
        window.location.hash = result.path;
      })
      .catch((err) => {
        console.warn(`[Search] Failed to load network '${result.networkBref}':`, err);
        // Navigate anyway — viewer will handle missing data gracefully.
        window.location.hash = result.path;
      });
  } else {
    window.location.hash = result.path;
  }
}

/**
 * Open the traceability panel in Submap mode anchored to a search result node.
 * Resolves the network bref to a network BID via the nav tree, loads the
 * network shard if needed, then opens the traceability modal.
 *
 * @param {string} bid - BID of the node to explore.
 * @param {string} networkBref - Network bref of the node's home network.
 */
async function _openExplore(bid, networkBref) {
  // Resolve network bref → network BID via the beliefbase index.
  let homeNetBid = null;
  if (state.beliefbase?.get_bid_from_bref) {
    try {
      homeNetBid = state.beliefbase.get_bid_from_bref(networkBref) || null;
    } catch (_) {
      /* bref not found */
    }
  }
  // Fallback: scan nav tree roots (works for top-level networks).
  if (!homeNetBid && state.navTree?.roots) {
    for (const rootBid of state.navTree.roots) {
      if (brefFromBid(rootBid) === networkBref) {
        homeNetBid = rootBid;
        break;
      }
    }
  }

  if (!homeNetBid) {
    console.warn(`[Search] Cannot resolve network bref '${networkBref}' to a BID`);
    return;
  }

  // Ensure the network shard is loaded before opening the panel.
  if (state.shardManager) {
    try {
      await state.shardManager.loadNetwork(networkBref);
    } catch (err) {
      console.warn(`[Search] Failed to load network '${networkBref}':`, err);
      // Continue anyway — the panel will show what it can.
    }
  }

  if (callbacks.openTraceabilityModal) {
    callbacks.openTraceabilityModal(bid, homeNetBid, bid);
  }
}

// =============================================================================
// Event wiring
// =============================================================================

function _attachInputListeners() {
  const input = state.searchInput;

  input.setAttribute("aria-expanded", "false");
  input.setAttribute("aria-autocomplete", "list");
  input.setAttribute("aria-controls", "search-results");
  input.setAttribute("role", "combobox");

  // Show/hide the clear button based on input content.
  function _syncClearButton() {
    if (state.searchClear) {
      state.searchClear.style.display = input.value ? "block" : "none";
    }
  }

  if (state.searchClear) {
    state.searchClear.addEventListener("mousedown", (e) => {
      // Prevent blur on the input before we clear it.
      e.preventDefault();
    });
    state.searchClear.addEventListener("click", () => {
      input.value = "";
      _syncClearButton();
      _closeResults();
      input.focus();
    });
  }

  // Debounced input handler
  input.addEventListener("input", () => {
    _syncClearButton();
    clearTimeout(_debounceTimer);
    const q = input.value.trim();

    if (q.length < MIN_QUERY_LENGTH) {
      _closeResults();
      return;
    }

    _debounceTimer = setTimeout(() => {
      const _loadedIndexCount = state.beliefbase
        ? state.beliefbase.loaded_search_indices().size
        : 0;
      if (_loadedIndexCount === 0) {
        // Indices are still loading (kicked off after first paint) —
        // the user's next keystroke (after the debounce) will retry.
        console.log("[Search] Search indices still loading...");
        return;
      }
      const results = runQuery(q);
      _currentResults = results;
      _renderResults(results, q);
      input.setAttribute("aria-expanded", String(!_resultsPanel.hidden));
    }, DEBOUNCE_MS);
  });

  // Keyboard navigation within the input
  input.addEventListener("keydown", (e) => {
    switch (e.key) {
      case "ArrowDown":
        e.preventDefault();
        _moveSelection(+1);
        break;
      case "ArrowUp":
        e.preventDefault();
        _moveSelection(-1);
        break;
      case "Enter":
        e.preventDefault();
        if (_selectedIndex >= 0) {
          _activateSelected();
        } else if (_currentResults.length > 0) {
          // Enter with no explicit selection → navigate to top result
          _navigateToResult(_currentResults[0]);
        }
        break;
      case "Escape":
        e.preventDefault();
        _closeResults();
        input.blur();
        break;
      case "e":
        // Explore: open traceability panel for the selected result.
        if (_selectedIndex >= 0 && _selectedIndex < _currentResults.length) {
          e.preventDefault();
          const result = _currentResults[_selectedIndex];
          if (result.bid && result.networkBref && callbacks.openTraceabilityModal) {
            _closeResults();
            input.blur();
            _openExplore(result.bid, result.networkBref);
          }
        }
        break;
    }
  });

  // Close results when input loses focus, unless focus moved into the panel.
  input.addEventListener("blur", () => {
    // Small delay so a click on a result row fires before we hide the panel.
    setTimeout(() => {
      if (_resultsPanel && !_resultsPanel.contains(document.activeElement)) {
        _closeResults();
      }
    }, 150);
  });
}

function _attachGlobalListeners() {
  // Ctrl+K — focus search from anywhere
  document.addEventListener("keydown", (e) => {
    if (e.ctrlKey && e.key === "k") {
      e.preventDefault();
      if (state.searchInput) {
        state.searchInput.focus();
        state.searchInput.select();
      }
    }
  });

  // Click on a result row
  document.addEventListener("click", (e) => {
    if (!_resultsPanel) return;

    // Explore button: open traceability panel anchored to the result node.
    // Must be checked before the general result-item click handler.
    const exploreBtn = e.target.closest(".noet-search-result__explore");
    if (exploreBtn && _resultsPanel.contains(exploreBtn)) {
      e.preventDefault();
      e.stopPropagation();
      const bid = exploreBtn.dataset.bid;
      const networkBref = exploreBtn.dataset.networkBref;
      if (bid && callbacks.openTraceabilityModal) {
        _closeResults();
        if (state.searchInput) state.searchInput.blur();
        // Resolve the network BID from the bref so we can open the
        // traceability panel anchored to this node's network.
        _openExplore(bid, networkBref);
      }
      return;
    }

    const item = e.target.closest(".noet-search-result");
    if (item && _resultsPanel.contains(item)) {
      e.preventDefault();
      const idx = parseInt(item.dataset.index ?? "-1", 10);
      if (idx >= 0 && idx < _currentResults.length) {
        _navigateToResult(_currentResults[idx]);
      }
      return;
    }

    // Click outside input and panel → close
    if (
      !_resultsPanel.hidden &&
      !_resultsPanel.contains(e.target) &&
      e.target !== state.searchInput
    ) {
      _closeResults();
    }
  });
}
