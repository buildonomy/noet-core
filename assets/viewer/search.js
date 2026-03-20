/**
 * viewer/search.js — Full-text search over compile-time search indices
 *
 * Queries state.searchIndex (Map<bref, SearchIndex>) which is populated eagerly
 * during WASM init from the per-network search/*.idx.json files. No WASM calls
 * are needed for scoring; the indices are plain JS objects. Snippet extraction
 * does call into the loaded BeliefBase to get payload["text"] for loaded networks.
 *
 * ## Architecture
 *
 * The compile-time SearchIndex (see src/shard/search.rs) contains:
 *   - docs:  { bid → { title, path, term_count } }   — always available
 *   - index: { term → [(bid, freq)] }                 — pre-built inverted index
 *   - stemmed: "English" | "None"                     — whether stemming was applied
 *   - network_bref: string
 *
 * Query pipeline:
 *   1. Tokenize query (same rules as compile time: split, lowercase, strip short/stop)
 *   2. For each loaded search index, look up each term and accumulate TF-IDF scores
 *      Exact matches score at full weight; fuzzy matches (Levenshtein ≤ 2, terms
 *      shorter than FUZZY_MAX_QUERY_LEN) score at a reduced weight (see FUZZY_PENALTY).
 *   3. Merge scores across all networks, take top MAX_RESULTS
 *   4. Enrich with snippet from loaded WASM data (best-effort, empty if unloaded)
 *   5. Render results panel
 *
 * ## Migration path to WASM
 *
 * When Issue 54 adds BeliefBaseWasm.load_search_index() / .search(query, limit),
 * replace the runQuery() body with a single WASM call. The UI layer (input,
 * results panel, keyboard nav) is unchanged.
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
 * - assets/viewer/shard-manager.js — search index loading
 * - assets/viewer/wasm.js — state.searchIndex population
 */

import { state } from "./state.js";
import { escapeHtml } from "./utils.js";

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

/**
 * Maximum query-term length for which fuzzy matching is attempted.
 * Levenshtein on long terms is both expensive and imprecise — skip it.
 */
const FUZZY_MAX_QUERY_LEN = 20;

/**
 * Score multiplier applied to fuzzy (non-exact) term matches.
 * Distance-1 matches receive 0.6×, distance-2 matches receive 0.3×.
 */
const FUZZY_PENALTY = [0, 0.6, 0.3];

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

  console.log(`[Search] Initialized. Indices loaded: ${state.searchIndex.size} network(s).`);
}

// =============================================================================
// Fuzzy matching (Levenshtein distance)
// =============================================================================

/**
 * Compute the Levenshtein edit distance between two strings.
 *
 * Uses the standard Wagner-Fischer DP algorithm with a two-row rolling buffer.
 * Early-exits when the running minimum exceeds `maxDist` to avoid unnecessary
 * work for clearly non-matching index terms.
 *
 * @param {string} a
 * @param {string} b
 * @param {number} maxDist — Maximum distance to care about (default 2). Returns
 *   maxDist+1 immediately if the true distance exceeds this.
 * @returns {number} Edit distance, capped at maxDist+1.
 */
function levenshtein(a, b, maxDist = 2) {
  // Length difference alone rules out a match within maxDist.
  if (Math.abs(a.length - b.length) > maxDist) return maxDist + 1;

  const m = a.length;
  const n = b.length;

  // prev[j] = cost of aligning a[0..i-1] with b[0..j-1]
  let prev = new Uint16Array(n + 1);
  let curr = new Uint16Array(n + 1);

  for (let j = 0; j <= n; j++) prev[j] = j;

  for (let i = 1; i <= m; i++) {
    curr[0] = i;
    let rowMin = curr[0];

    for (let j = 1; j <= n; j++) {
      const cost = a[i - 1] === b[j - 1] ? 0 : 1;
      curr[j] = Math.min(
        curr[j - 1] + 1, // insertion
        prev[j] + 1, // deletion
        prev[j - 1] + cost, // substitution
      );
      if (curr[j] < rowMin) rowMin = curr[j];
    }

    // Prune: if the best possible score for this row already exceeds maxDist,
    // no further rows can bring it back within range.
    if (rowMin > maxDist) return maxDist + 1;

    // Swap buffers.
    const tmp = prev;
    prev = curr;
    curr = tmp;
  }

  return prev[n];
}

/**
 * Find all terms in `indexTerms` that are within Levenshtein distance `maxDist`
 * of `queryTerm`, returning pairs of [indexTerm, distance].
 *
 * Only called when `queryTerm.length < FUZZY_MAX_QUERY_LEN`.  Iterates the
 * full term set of one index — acceptable because index term sets are small
 * (typically < 5 000 entries) and this runs at most once per query term per
 * network.
 *
 * @param {string} queryTerm
 * @param {string[]} indexTerms — All keys of idx.index for one network
 * @param {number} maxDist
 * @returns {Array<[string, number]>} Pairs of (matchedIndexTerm, editDistance)
 */
function _fuzzyMatches(queryTerm, indexTerms, maxDist = 2) {
  const results = [];
  for (const term of indexTerms) {
    // Quick length pre-filter (same as inside levenshtein, but avoids the call overhead).
    if (Math.abs(term.length - queryTerm.length) > maxDist) continue;
    const dist = levenshtein(queryTerm, term, maxDist);
    if (dist > 0 && dist <= maxDist) {
      results.push([term, dist]);
    }
  }
  return results;
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
 * No stemming on the query side in this JS implementation. Because the compile-time
 * index may be stemmed (StemMode::English), we also add the raw term alongside any
 * stem-approximation. For now we just use the raw lowercase term — recall is slightly
 * lower than the Rust path for inflected forms, which is acceptable for the MVP.
 * When Issue 54 moves search into WASM, identical stemming is applied automatically.
 *
 * @param {string} text
 * @returns {string[]} Deduplicated array of normalised terms
 */
function tokenize(text) {
  if (!text) return [];

  // Split on anything that isn't a letter, digit, or apostrophe
  const raw = text
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
// TF-IDF query engine
// =============================================================================

/**
 * @typedef {Object} SearchResult
 * @property {string} bid
 * @property {string} networkBref
 * @property {string} title
 * @property {string} path        — HTML-relative path (may be empty for network roots)
 * @property {string} snippet     — Content excerpt; empty string for unloaded networks
 * @property {number} score       — TF-IDF relevance score
 * @property {boolean} loaded     — Whether the network's data shard is currently loaded
 */

/**
 * Run a TF-IDF query across all loaded search indices.
 *
 * @param {string} query — Raw query string from the input
 * @returns {SearchResult[]} Top MAX_RESULTS results sorted descending by score
 */
function runQuery(query) {
  const terms = tokenize(query);
  if (terms.length === 0) return [];

  // Collect total doc counts across all indices for IDF denominator.
  let totalDocCount = 0;
  for (const idx of state.searchIndex.values()) {
    totalDocCount += idx.doc_count ?? 0;
  }
  if (totalDocCount === 0) return [];

  // Per-bid score accumulator: bid → { score, networkBref, title, path }
  const scores = new Map();

  /**
   * Accumulate a TF-IDF contribution for one (idx, term, postings) triple.
   *
   * @param {string} bref
   * @param {object} idx
   * @param {string} term — the index term that matched (may differ from query term for fuzzy)
   * @param {number} penalty — score multiplier in (0, 1]; 1 for exact, <1 for fuzzy
   */
  function _accumulateTerm(bref, idx, term, penalty) {
    const postings = idx.index[term];
    if (!postings || postings.length === 0) return;

    // IDF = log((totalDocCount + 1) / (df + 1)) + 1  (smoothed)
    const df = postings.length;
    const idf = Math.log((totalDocCount + 1) / (df + 1)) + 1;

    for (const [bid, rawFreq] of postings) {
      const docMeta = idx.docs[bid];
      if (!docMeta) continue;

      // TF = rawFreq / term_count  (normalised by document length)
      const tf = rawFreq / (docMeta.term_count || 1);
      const contribution = tf * idf * penalty;

      if (scores.has(bid)) {
        scores.get(bid).score += contribution;
      } else {
        scores.set(bid, {
          score: contribution,
          networkBref: bref,
          title: docMeta.title,
          path: docMeta.path,
        });
      }
    }
  }

  for (const [bref, idx] of state.searchIndex.entries()) {
    const netDocCount = idx.doc_count ?? 0;
    if (netDocCount === 0) continue;

    // Cache the index term list once per network for fuzzy scanning.
    // Only materialised when at least one query term is short enough for fuzzy.
    let indexTerms = null;

    for (const term of terms) {
      // ── Exact match (full weight) ──────────────────────────────────────
      _accumulateTerm(bref, idx, term, 1.0);

      // ── Fuzzy matches (penalised weight) ──────────────────────────────
      // Skip fuzzy for long terms — too expensive and too noisy.
      if (term.length >= FUZZY_MAX_QUERY_LEN) continue;

      if (!indexTerms) indexTerms = Object.keys(idx.index);
      const matches = _fuzzyMatches(term, indexTerms);
      for (const [fuzzyTerm, dist] of matches) {
        _accumulateTerm(bref, idx, fuzzyTerm, FUZZY_PENALTY[dist]);
      }
    }
  }

  if (scores.size === 0) return [];

  // Sort descending by score, take top MAX_RESULTS
  const ranked = Array.from(scores.entries())
    .sort((a, b) => b[1].score - a[1].score)
    .slice(0, MAX_RESULTS);

  // Determine which networks have loaded data shards.
  const loadedBrefs = _getLoadedBrefs();

  // Build result objects, enriching with snippets where data is available.
  return ranked.map(([bid, meta]) => {
    const loaded = loadedBrefs.has(meta.networkBref);
    const snippet = loaded ? _extractSnippet(bid, terms) : "";
    return {
      bid,
      networkBref: meta.networkBref,
      title: meta.title,
      path: meta.path,
      snippet,
      score: meta.score,
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

  // Monolithic mode: everything is loaded — return all brefs in the index.
  return new Set(state.searchIndex.keys());
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

    const start = bestPos === -1 ? 0 : Math.max(0, bestPos - Math.floor(SNIPPET_WINDOW / 2));
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
    // Network data not loaded — confirm with the user (showing estimated size)
    // before fetching, so they aren't surprised by a large download.
    const networkMeta = state.shardManager.manifest.networks.find(
      (n) => n.bref === result.networkBref,
    );
    const sizeMb = networkMeta ? networkMeta.estimated_size_mb.toFixed(1) : "unknown";
    const networkTitle = networkMeta ? networkMeta.title || result.networkBref : result.networkBref;

    const confirmed = window.confirm(
      `"${result.title}" is in the network "${networkTitle}", which is not yet loaded.\n\n` +
        `Load it now? (~${sizeMb} MB)`,
    );
    if (!confirmed) return;

    console.log(`[Search] Loading network '${result.networkBref}' for result navigation…`);
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
      if (state.searchIndex.size === 0) {
        console.warn("[Search] No search indices loaded yet.");
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
