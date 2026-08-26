/**
 * viewer/xlsx-tabs.js — Xlsx workbook tab switcher
 *
 * Reads tab data from <script type="application/json" id="noet-xlsx-data">,
 * renders Tabulator tables lazily on first tab open, and handles hash-based
 * tab routing for both static (direct-file) and SPA (hash-routed) contexts.
 *
 * Exported entry point: window.noetInitXlsxTabs
 *   Called automatically on DOMContentLoaded.
 *   Also called by processLoadedContent (content.js) after SPA injection,
 *   because reExecuteScripts won't re-run this already-loaded global script.
 *   Accepts an optional callbacks object (passed from content.js):
 *     callbacks.showMetadataPanel(bid)  — open the metadata panel for a node
 *     callbacks.navigateToLink(key, el, event) — navigate to a relation key
 *
 * Callback late-binding:
 *   Callbacks are stored on window.__xlsxCbs so that rowClick and cellClick
 *   handlers always read the latest value, even if the callbacks object is
 *   updated after Tabulator instances are created. Use window.noetSetXlsxCallbacks
 *   to update callbacks without re-running init.
 */

(function () {
  "use strict";

  var initializedTabs = {};
  var tabulatorInstances = {};
  var xlsxData = null; // points to the "tabs" sub-object after parsing
  var tabOrder = []; // tab IDs in workbook sheet order (from _tab_order in JSON)
  var netBref = ""; // network bref for get_bid_from_id lookups
  // cbs is a convenience alias; all handlers read from window.__xlsxCbs so
  // the latest callbacks are always used regardless of when Tabulator was built.
  var cbs = {};

  /** BID of the row currently focused via keyboard navigation (null = none). */
  var focusedRowBid = null;

  /** Whether the global keyboard handler has been attached. */
  var keyboardHandlerAttached = false;

  // =========================================================================
  // Column builder
  // =========================================================================

  /**
   * Transform raw column definitions from the JSON data into Tabulator column
   * config objects, applying formatters by role:
   *   "relation" — comma-separated keys rendered as clickable buttons
   *   "text"     — pre-rendered HTML content (html formatter, no header filter)
   *   other      — default Tabulator formatter with header filter
   *
   * @param {Array} colDefs - Raw column definitions from xlsxData[tabId].columns
   * @returns {Array} Tabulator column config objects
   */
  /**
   * Normalize a string to an anchor/id slug.
   * Delegates to window.__noetToAnchor (BeliefBaseWasm.toAnchor exposed by wasm.js)
   * which calls the canonical Rust to_anchor() implementation.
   * Falls back to an inline approximation when WASM is not yet initialized.
   * @param {string} s
   * @returns {string}
   */
  function toAnchor(s) {
    if (typeof window.__noetToAnchor === "function") {
      return window.__noetToAnchor(s);
    }
    // Fallback: approximate to_anchor without WASM (should rarely be needed).
    return s
      .toLowerCase()
      .replace(/\s+/g, "-")
      .replace(/[^a-z0-9\-._()[\]@]/g, "")
      .replace(/-{2,}/g, "-")
      .replace(/^-+|-+$/g, "");
  }

  function buildColumns(colDefs) {
    return colDefs
      .filter(function (col) {
        // Skip hidden companion columns (e.g. "{field}_text" for relation display).
        // These carry raw cell text for use by the relation formatter but must not
        // appear as visible Tabulator columns.
        return col.role !== "hidden";
      })
      .map(function (col) {
        var tabCol = {
          title: col.title,
          field: col.field,
          headerFilter: "input",
        };

        if (col.wrap) {
          tabCol.cssClass = "xlsx-cell-wrap";
          tabCol.variableHeight = true;
          // Give wrap columns more proportional space under fitColumns.
          tabCol.widthGrow = 3;
        }

        if (col.role === "relation") {
          // The cell value is the raw semicolon-separated display text stored in
          // doc[field] at parse time (e.g. "Load Switch Controller; PDU Firmware").
          //
          // Resolution strategy (in priority order):
          // 1. {field}_bids — parse-time resolved BIDs written by inject_context.
          //    These are authoritative: they come from the actual graph edges and
          //    are cross-shard correct. Use them directly without any re-resolution.
          // 2. NodeKey derivation from raw label + col.key_format — fallback when
          //    the relation target wasn't resolved at parse time (unloaded shard,
          //    unresolvable id, etc.).
          //
          // col.key_format is one of: "auto" | "id" | "path" | "bid" | "bref"
          var keyFormat = col.key_format || "auto";
          var bidsField = col.field + "_bids";
          tabCol.formatter = function (cell) {
            var val = cell.getValue() || "";
            if (!val) return "";
            var bb = window.__noetBeliefBase;
            var rowData = cell.getRow().getData();
            // Parse-time resolved BIDs — one per semicolon item, "" when unresolved.
            var parsedBids = (rowData[bidsField] || "").split(";");
            // val is semicolon-separated raw text; split to get per-item labels.
            return val
              .split(";")
              .map(function (rawLabel, idx) {
                rawLabel = rawLabel.trim();
                if (!rawLabel) return "";

                var bid = null;
                var title = rawLabel;
                var path = "";
                var bref = "";

                // Strategy 1: use parse-time BID directly.
                var parsedBid = (parsedBids[idx] || "").trim();
                if (parsedBid && bb) {
                  try {
                    var node = bb.get_by_bid(parsedBid);
                    if (node) {
                      bid = parsedBid;
                      title = node.title || rawLabel;
                      bref = parsedBid.replace(/-/g, "").slice(-12);
                      var ctx = bb.get_context(parsedBid);
                      if (ctx && ctx.root_path) {
                        path = ctx.root_path;
                      }
                    }
                  } catch (err) {
                    console.warn("[Noet] xlsx-tabs: parse-time BID lookup error:", err);
                  }
                }

                // Strategy 2: derive NodeKey ephemerally and resolve via BeliefBase.
                // Only runs when the parse-time BID was absent or the node isn't loaded.
                if (!bid && bb) {
                  try {
                    var k;
                    if (keyFormat === "id") {
                      k = "id:" + toAnchor(rawLabel);
                    } else if (keyFormat === "path") {
                      k = "path:" + rawLabel;
                    } else if (keyFormat === "bid") {
                      k = "bid:" + rawLabel;
                    } else if (keyFormat === "bref") {
                      k = "bref:" + rawLabel;
                    } else {
                      k = rawLabel;
                    }
                    var resolved = null;
                    if (k.indexOf("id:") === 0) {
                      var resolvedId = k.substring(3);
                      if (netBref) {
                        resolved = bb.get_bid_from_id(netBref, resolvedId);
                      }
                    } else if (k.indexOf("bref:") === 0) {
                      resolved = bb.get_bid_from_bref(k.substring(5));
                    } else if (/^[0-9a-f]{12}$/.test(k)) {
                      resolved = bb.get_bid_from_bref(k);
                    }
                    if (resolved && resolved.bid) {
                      var node2 = bb.get_by_bid(resolved.bid);
                      if (node2) {
                        bid = resolved.bid;
                        title = node2.title || rawLabel;
                        bref = bid.replace(/-/g, "").slice(-12);
                        var ctx2 = bb.get_context(bid);
                        if (ctx2 && ctx2.root_path) {
                          path = ctx2.root_path;
                        }
                      }
                    }
                  } catch (err) {
                    console.warn(
                      "[Noet] xlsx-tabs: relation resolve error for key=" +
                        rawLabel +
                        ":",
                      err,
                    );
                  }
                }

                if (bid && bref) {
                  // Resolved node — emit a navigable link using the same attributes
                  // as the rest of the viewer (href for navigation, title="bref://..."
                  // for BID lookup, data-bid for direct showMetadataPanel calls).
                  // cellClick reads these and calls navigateToLink / showMetadataPanel.
                  var hrefAttr = path ? ' href="' + escapeHtml(path) + '"' : "";
                  return (
                    "<a" +
                    hrefAttr +
                    ' title="bref://' +
                    bref +
                    '"' +
                    ' class="xlsx-rel-link noet-node-link"' +
                    ' data-bid="' +
                    bid +
                    '">' +
                    escapeHtml(title) +
                    "</a>"
                  );
                }

                // Unresolved key — plain text, no click action.
                // Avoids the old fallthrough that called navigateToLink with a raw
                // "id:xxx" string, which was misclassified as a schema URL and
                // opened a new browser tab.
                // Downgrade log to debug-level; unresolved keys are expected for
                // cross-shard relations and produce a lot of noise at console.log.
                return (
                  '<span class="xlsx-rel-unresolved" title="' +
                  escapeHtml(k) +
                  '">' +
                  escapeHtml(title) +
                  "</span>"
                );
              })
              .join(" ");
          };
          // cellClick: delegate to handleNodeLinkClick for the two-click pattern
          // (first click → metadata panel, second click → navigate).
          // Falls back to showMetadataPanel when no href is available.
          tabCol.cellClick = function (e, cell) {
            var link = e.target.closest(".xlsx-rel-link");
            if (!link) return;
            // Mark the event so the rowClick handler can detect that the click
            // originated on a relation link. Tabulator v6 fires rowClick via its
            // own internal dispatcher after cellClick, independent of DOM bubbling,
            // so e.stopPropagation() alone is not sufficient.
            e._noetRelLink = true;
            var bid = link.getAttribute("data-bid");
            var href = link.getAttribute("href") || null;
            var activeCbs = window.__xlsxCbs || {};
            console.log("[Noet] xlsx-tabs: cellClick bid=" + bid + " href=" + href);
            if (activeCbs.handleNodeLinkClick) {
              // Two-click pattern: first click shows metadata, second navigates.
              activeCbs.handleNodeLinkClick(bid, href, link);
            } else if (bid && activeCbs.showMetadataPanel) {
              // Fallback when handleNodeLinkClick not yet registered.
              activeCbs.showMetadataPanel(bid);
            }
          };
        } else if (col.role === "text") {
          tabCol.formatter = "html";
          tabCol.headerFilter = false;
        }

        return tabCol;
      });
  }

  /**
   * Minimal HTML escaper for attribute values and text content emitted inside
   * the relation formatter. Keeps xlsx-tabs.js self-contained (no import of
   * utils.js, which is an ES module and cannot be used in this IIFE).
   * @param {string} str
   * @returns {string}
   */
  function escapeHtml(str) {
    return String(str)
      .replace(/&/g, "&amp;")
      .replace(/</g, "&lt;")
      .replace(/>/g, "&gt;")
      .replace(/"/g, "&quot;")
      .replace(/'/g, "&#39;");
  }

  // =========================================================================
  // Data loading
  // =========================================================================

  /**
   * Parse the embedded JSON data block and cache it.
   * Returns the parsed object, or null on failure.
   */
  function loadXlsxData() {
    var el = document.getElementById("noet-xlsx-data");
    if (!el) return null;
    try {
      var wrapper = JSON.parse(el.textContent);
      // Support both the old flat structure and the new {_net_bref, tabs} wrapper.
      if (wrapper && wrapper.tabs) {
        netBref = wrapper._net_bref || "";
        xlsxData = wrapper.tabs;
        // Use _tab_order for workbook sheet order; fall back to Object.keys()
        // which gives alphabetical order due to serde_json::Map using BTreeMap.
        tabOrder = Array.isArray(wrapper._tab_order)
          ? wrapper._tab_order
          : Object.keys(xlsxData);
      } else {
        netBref = "";
        xlsxData = wrapper;
        tabOrder = Object.keys(xlsxData);
      }
      console.log(
        "[Noet] xlsx-tabs: loaded data, netBref=" +
          netBref +
          ", tabs=" +
          Object.keys(xlsxData).length,
      );
      return xlsxData;
    } catch (e) {
      console.warn("[Noet] xlsx-tabs: failed to parse noet-xlsx-data JSON:", e);
      return null;
    }
  }

  // =========================================================================
  // Fullscreen panel
  // =========================================================================

  /**
   * Open the focused panel in fullscreen mode, rendering a full workbook view
   * that includes the tab nav and all tab sections — mirroring the inline view.
   *
   * The active tab is initialised with a Tabulator immediately; others are
   * lazy-initialised when their tab link is clicked inside the panel.
   *
   * @param {string} activeTabId - The tab to show initially
   */
  function openFullscreen(activeTabId) {
    var panelEl = document.getElementById("noet-focused-panel");
    if (!panelEl) return;

    if (!xlsxData) return;
    var tabIds = tabOrder.length ? tabOrder : Object.keys(xlsxData);
    if (!tabIds.length) return;

    // Determine a sensible active tab.
    if (!xlsxData[activeTabId]) {
      activeTabId = tabIds[0];
    }

    // ------------------------------------------------------------------
    // Build nav HTML — one link per tab, same structure as the inline nav.
    // ------------------------------------------------------------------
    var navHtml = '<nav class="xlsx-tab-nav is-active" id="xlsx-fs-tab-nav">';
    tabIds.forEach(function (tid) {
      // Look up the display name from the main document tab links.
      var linkEl = document.querySelector('.xlsx-tab-link[data-tab="' + tid + '"]');
      var label = linkEl ? linkEl.textContent : tid;
      var activeClass = tid === activeTabId ? " is-active" : "";
      navHtml +=
        '<a href="#" class="xlsx-tab-link' +
        activeClass +
        '" data-tab="' +
        tid +
        '">' +
        label +
        "</a>";
    });
    navHtml += "</nav>";

    // ------------------------------------------------------------------
    // Build section HTML — one section per tab, same structure as inline.
    // ------------------------------------------------------------------
    var sectionsHtml = "";
    tabIds.forEach(function (tid) {
      var activeClass = tid === activeTabId ? " is-active" : "";
      sectionsHtml +=
        '<section class="xlsx-tab' +
        activeClass +
        '" id="xlsx-fs-tab-' +
        tid +
        '">' +
        '<div class="xlsx-table-container" id="xlsx-fs-tbl-' +
        tid +
        '"></div>' +
        "</section>";
    });

    // ------------------------------------------------------------------
    // Assemble panel HTML.
    // ------------------------------------------------------------------
    panelEl.innerHTML =
      '<div class="noet-traceability">' +
      '<div class="noet-traceability__header">' +
      '<span class="noet-traceability__title">Workbook</span>' +
      '<button class="noet-traceability__close" id="xlsx-fullscreen-close">\u2715</button>' +
      "</div>" +
      '<div class="noet-traceability__body">' +
      navHtml +
      sectionsHtml +
      "</div>" +
      "</div>";

    panelEl.classList.add("is-open");

    // Mobile: move into metadata panel
    if (window.matchMedia("(max-width: 1023px)").matches) {
      var metadataEl = document.getElementById("metadata-panel");
      if (metadataEl && panelEl.parentElement !== metadataEl) {
        metadataEl.appendChild(panelEl);
        metadataEl.classList.add("has-focused-panel");
      }
    }

    // Wire close button.
    var closeBtn = document.getElementById("xlsx-fullscreen-close");
    if (closeBtn) {
      closeBtn.addEventListener("click", closeFullscreen);
    }

    // Track which tabs have been initialised inside the fullscreen panel.
    var fsInitialized = {};

    /**
     * Initialise a Tabulator inside the fullscreen panel for the given tab id.
     * @param {string} tid
     */
    function initFsTab(tid) {
      if (fsInitialized[tid]) return;
      var data = xlsxData[tid];
      if (!data || !data.columns || !data.rows) return;
      var divEl = document.getElementById("xlsx-fs-tbl-" + tid);
      if (!divEl || typeof Tabulator === "undefined") return;

      try {
        fsInitialized[tid] = true;
        var cols = buildColumns(data.columns);
        var fsTbl = new Tabulator(divEl, {
          data: data.rows,
          layout: "fitColumns",
          pagination: "local",
          paginationSize: 100,
          columns: cols,
          rowFormatter: function (row) {
            var d = row.getData();
            var bid = d._bid;
            if (bid) {
              var el = row.getElement();
              el.setAttribute("data-bid", bid);
              var bref = bid.replace(/-/g, "").slice(-12);
              el.setAttribute("title", "bref://" + bref);
            }
            if (d._id) {
              row.getElement().id = d._id;
            }
          },
        });
        // Tabulator v6: rowClick must be wired via table.on(), not constructor option.
        fsTbl.on("rowClick", function (e, row) {
          if (e._noetRelLink) return;
          var bid = row.getData()._bid;
          var activeCbs = window.__xlsxCbs || {};
          if (bid && activeCbs.showMetadataPanel) {
            activeCbs.showMetadataPanel(bid);
            focusedRowBid = bid;
          }
          // No hash update in fullscreen — the panel manages its own state.
        });
        // Pin column width after manual resize: update the column definition
        // with a fixed pixel width so fitColumns won't recalculate it.
        fsTbl.on("columnResized", function (column) {
          var w = column.getWidth();
          column.updateDefinition({ width: w, widthGrow: 0, widthShrink: 0 });
        });
      } catch (e) {
        console.warn("[Noet] Tabulator fullscreen init failed for tab " + tid + ":", e);
      }
    }

    /**
     * Show a tab section inside the fullscreen panel and lazy-init its table.
     * @param {string} tid
     */
    function showFsTab(tid) {
      panelEl.querySelectorAll(".xlsx-tab").forEach(function (s) {
        s.classList.toggle("is-active", s.id === "xlsx-fs-tab-" + tid);
      });
      panelEl.querySelectorAll(".xlsx-tab-link").forEach(function (a) {
        a.classList.toggle("is-active", a.dataset.tab === tid);
      });
      initFsTab(tid);
    }

    // Wire tab link click handlers inside the panel.
    panelEl.querySelectorAll(".xlsx-tab-link").forEach(function (a) {
      a.addEventListener("click", function (e) {
        e.preventDefault();
        showFsTab(this.dataset.tab);
      });
    });

    // Initialise the active tab immediately.
    initFsTab(activeTabId);
  }

  /**
   * Close the fullscreen focused panel and restore it to document.body.
   */
  function closeFullscreen() {
    var panelEl = document.getElementById("noet-focused-panel");
    if (!panelEl) return;
    panelEl.classList.remove("is-open");
    if (panelEl.parentElement !== document.body) {
      document.body.appendChild(panelEl);
    }
    var metadataEl = document.getElementById("metadata-panel");
    if (metadataEl) {
      metadataEl.classList.remove("has-focused-panel");
    }
  }

  // =========================================================================
  // Tab display
  // =========================================================================

  /**
   * Show a tab section by its id.
   * Toggles is-active on .xlsx-tab sections and .xlsx-tab-link anchors.
   * Lazy-initializes Tabulator for the tab on first open.
   * Inserts a fullscreen button if not already present.
   *
   * @param {string} tabId
   */
  function showTab(tabId) {
    document.querySelectorAll(".xlsx-tab").forEach(function (s) {
      s.classList.toggle("is-active", s.id === tabId);
    });
    document.querySelectorAll(".xlsx-tab-link").forEach(function (a) {
      a.classList.toggle("is-active", a.dataset.tab === tabId);
    });

    // Lazy-initialize Tabulator for this tab on first open.
    if (!initializedTabs[tabId] && typeof Tabulator !== "undefined") {
      var data = xlsxData && xlsxData[tabId];
      if (data && data.columns && data.rows) {
        var section = document.getElementById(tabId);
        var divEl = section && section.querySelector(".xlsx-table-container");
        if (divEl) {
          // Insert fullscreen button before the table container if not present.
          if (!section.querySelector(".xlsx-fullscreen-btn")) {
            var btn = document.createElement("button");
            btn.className = "xlsx-fullscreen-btn";
            btn.setAttribute("data-tab", tabId);
            btn.textContent = "\u26F6 Full View";
            (function (tid) {
              btn.addEventListener("click", function () {
                openFullscreen(tid);
              });
            })(tabId);
            divEl.parentNode.insertBefore(btn, divEl);
          }

          try {
            // Destroy any existing Tabulator on this element before re-creating.
            // Creating a second Tabulator on the same div silently fails or produces
            // stale rowClick handlers that capture the old (no-callback) closure.
            if (tabulatorInstances[tabId]) {
              try {
                tabulatorInstances[tabId].destroy();
              } catch (ex) {}
              delete tabulatorInstances[tabId];
            }
            initializedTabs[tabId] = true;
            var cols = buildColumns(data.columns);
            tabulatorInstances[tabId] = new Tabulator(divEl, {
              data: data.rows,
              layout: "fitColumns",
              pagination: "local",
              paginationSize: 50,
              columns: cols,
              rowFormatter: function (row) {
                var data = row.getData();
                var bid = data._bid;
                if (bid) {
                  var el = row.getElement();
                  el.setAttribute("data-bid", bid);
                  // bref is the last 12 hex chars of the UUID (no dashes).
                  var bref = bid.replace(/-/g, "").slice(-12);
                  el.setAttribute("title", "bref://" + bref);
                }
                // Set DOM id from _id so getElementById-based navigation works.
                var rowId = data._id;
                if (rowId) {
                  row.getElement().id = rowId;
                }
              },
            });
            // Track when the table finishes building so scrollToRow can
            // safely defer searchData/getData calls until ready.
            tabulatorInstances[tabId]._noetBuilt = false;
            tabulatorInstances[tabId].on("tableBuilt", function () {
              tabulatorInstances[tabId]._noetBuilt = true;
            });
            // Tabulator v6: rowClick must be wired via table.on(), not constructor option.
            tabulatorInstances[tabId].on("rowClick", function (e, row) {
              // Skip when the click originated on a relation link — cellClick
              // already handled it. Tabulator fires rowClick independently of
              // DOM bubbling so e.stopPropagation() in cellClick is insufficient.
              if (e._noetRelLink) return;
              var rowData = row.getData();
              var bid = rowData._bid;
              console.log("[Noet] xlsx-tabs: rowClick bid=" + bid);
              var activeCbs = window.__xlsxCbs || {};
              console.log(
                "[Noet] xlsx-tabs: showMetadataPanel=" +
                  typeof activeCbs.showMetadataPanel,
              );
              if (bid && activeCbs.showMetadataPanel) {
                activeCbs.showMetadataPanel(bid);
                // Sync keyboard navigation focus with the clicked row.
                focusedRowBid = bid;
                // Update hash so nav tree highlights the selected row node.
                // Use the row's explicit id (_id field) as the anchor so the
                // URL points at the individual row, not just the tab section.
                var docPath = getSpaDocPath();
                if (docPath) {
                  var rowId = rowData._id || tabId;
                  var newHash = "#/" + docPath + "#" + rowId;
                  // Use a root-relative absolute URL so replaceState resolves
                  // against the origin, not the current fetch URL. A bare hash
                  // like "#/doc.html#row" is relative — the browser resolves it
                  // against window.location.href which may be /pages/doc.html,
                  // producing /pages/#/doc.html#row instead of /#/doc.html#row.
                  history.replaceState(null, "", "/" + newHash);
                }
              } else if (!bid) {
                console.warn(
                  "[Noet] xlsx-tabs: row has no _bid — was workbook built with --write?",
                );
              } else {
                console.warn(
                  "[Noet] xlsx-tabs: showMetadataPanel not registered in callbacks",
                );
              }
            });
            // Pin column width after manual resize: update the column definition
            // with a fixed pixel width so fitColumns won't recalculate it.
            tabulatorInstances[tabId].on("columnResized", function (column) {
              var w = column.getWidth();
              column.updateDefinition({ width: w, widthGrow: 0, widthShrink: 0 });
            });
          } catch (e) {
            console.warn("[Noet] Tabulator init failed for tab " + tabId + ":", e);
          }
        }
      }
    }
  }

  // =========================================================================
  // Hash routing helpers
  // =========================================================================

  /**
   * Derive the current SPA document path from window.location.hash.
   * SPA hash form: /#/doc.html  or  /#/doc.html#section
   * Returns the doc path portion (e.g. "sub/doc.html"), or empty string.
   *
   * @returns {string}
   */
  function getSpaDocPath() {
    var hash = window.location.hash;
    if (!hash) return "";
    // Strip leading #
    var clean = hash.replace(/^#/, "");
    // SPA form: /doc.html or /doc.html#section — strip leading slash.
    // Non-SPA form: doc.html#section (served directly from /pages/).
    // Both cases: strip any existing anchor to get just the doc path.
    clean = clean.replace(/^\//, "");
    var parts = clean.split("#");
    return parts[0] || "";
  }

  /**
   * Determine which tab id to activate from the current hash.
   * Handles both SPA hashes (/#/doc.html#tab-id) and plain anchors (#tab-id).
   * For row anchors, walks up to find the parent .xlsx-tab section.
   *
   * @returns {string|null} tab element id, or null if not determinable
   */
  function getTabFromHash() {
    var hash = window.location.hash.replace(/^#/, "");
    if (!hash) return null;

    // SPA hashes have the form: /doc.html#tab-id or /doc.html
    // Strip the leading doc path prefix to get just the anchor part.
    var anchorIdx = hash.indexOf("#");
    if (anchorIdx !== -1) {
      hash = hash.substring(anchorIdx + 1);
    } else if (hash.indexOf(".html") !== -1 || hash.indexOf("/") !== -1) {
      // Pure doc path with no anchor — no tab to activate.
      return null;
    }

    if (!hash) return null;

    // Hash may be a tab id or a row anchor; try tab first.
    var tabEl = document.getElementById(hash);
    if (tabEl && tabEl.classList.contains("xlsx-tab")) return hash;

    // For row anchors, find the parent tab section.
    if (tabEl) {
      var parent = tabEl.closest(".xlsx-tab");
      if (parent) return parent.id;
    }

    return null;
  }

  /**
   * Activate the correct tab based on current hash, defaulting to the first tab.
   * After showing the tab, scrolls to a row anchor if the hash points to one.
   */
  /**
   * Activate the correct tab based on the provided anchor (or current hash),
   * defaulting to the first tab.  After showing the tab, scrolls to the row
   * identified by the anchor using Tabulator's scrollToRow API (handles
   * pagination) with a fallback to getElementById for non-row anchors.
   *
   * @param {string} [anchor] - Optional anchor to navigate to (e.g. "row-id").
   *   When provided, overrides the hash-derived anchor.
   */
  function activateFromHash(anchor) {
    // Determine row anchor: explicit argument > hash-derived.
    var rowAnchor = anchor || null;
    var tabId = null;

    if (rowAnchor) {
      // Try to find which tab contains this anchor by checking xlsxData rows.
      tabId = findTabForRow(rowAnchor);
    }
    if (!tabId) {
      tabId = getTabFromHash();
    }

    var tabs = document.querySelectorAll(".xlsx-tab");
    if (!tabId && tabs.length > 0) {
      tabId = tabs[0].id;
    }
    if (tabId) {
      showTab(tabId);
    }

    // Derive row anchor from hash if not explicitly provided.
    if (!rowAnchor) {
      var rawHash = window.location.hash.replace(/^#/, "");
      var anchorIdx = rawHash.indexOf("#");
      rowAnchor = anchorIdx !== -1 ? rawHash.substring(anchorIdx + 1) : rawHash;
    }

    if (rowAnchor && rowAnchor !== tabId) {
      scrollToRow(tabId, rowAnchor);
    }
  }

  /**
   * Find which tab contains a row with the given _id.
   * @param {string} rowId - The row's _id value
   * @returns {string|null} Tab ID, or null if not found
   */
  function findTabForRow(rowId) {
    if (!xlsxData) return null;
    for (var tid in xlsxData) {
      if (!xlsxData.hasOwnProperty(tid)) continue;
      var tabData = xlsxData[tid];
      if (tabData && tabData.rows) {
        for (var i = 0; i < tabData.rows.length; i++) {
          if (tabData.rows[i]._id === rowId) {
            return tid;
          }
        }
      }
    }
    return null;
  }

  /**
   * Scroll to a specific row within a tab's Tabulator table.
   * Handles pagination: uses Tabulator's scrollToRow API which navigates to
   * the correct page before scrolling.  Falls back to getElementById for
   * non-row anchors (e.g. tab section headers).
   *
   * @param {string} tabId - The tab containing the target row
   * @param {string} rowAnchor - The row's _id or element id to scroll to
   */
  /**
   * Internal: perform the actual scroll once the table is ready.
   */
  function _doScrollToRow(table, tabId, rowAnchor) {
    var matches = table.searchData("_id", "=", rowAnchor);
    if (matches.length > 0) {
      var allData = table.getData();
      var rowIndex = -1;
      for (var i = 0; i < allData.length; i++) {
        if (allData[i]._id === rowAnchor) {
          rowIndex = i;
          break;
        }
      }
      if (rowIndex >= 0) {
        var pageSize = table.getPageSize();
        var targetPage = Math.floor(rowIndex / pageSize) + 1;
        table.setPage(targetPage);
      }
      // After page change, the row element should exist in the DOM.
      // Use a brief delay for Tabulator to render the page.
      setTimeout(function () {
        var el = document.getElementById(rowAnchor);
        if (el) {
          el.scrollIntoView({ behavior: "smooth", block: "center" });
          // Set keyboard focus so the row gets the :focus outline style
          // and subsequent ArrowUp/ArrowDown work immediately.
          var bid = el.getAttribute("data-bid");
          if (bid) focusedRowBid = bid;
          if (!el.hasAttribute("tabindex")) {
            el.setAttribute("tabindex", "-1");
          }
          el.focus({ preventScroll: true });
        }
      }, 100);
      return;
    }

    // Fallback: try getElementById for non-row anchors (tab headers, etc.)
    var el = document.getElementById(rowAnchor);
    if (el) {
      setTimeout(function () {
        el.scrollIntoView({ behavior: "smooth", block: "center" });
      }, 50);
    }
  }

  function scrollToRow(tabId, rowAnchor) {
    var table = tabulatorInstances[tabId];
    if (!table) return;

    // Wait for tableBuilt before calling searchData/getData.
    // Tabulator v6 errors if these are called before initialization completes.
    if (table._noetBuilt) {
      // Table is already built — scroll immediately.
      _doScrollToRow(table, tabId, rowAnchor);
    } else {
      // Table is still initializing — defer until tableBuilt fires.
      table.on("tableBuilt", function () {
        _doScrollToRow(table, tabId, rowAnchor);
      });
    }
  }

  /**
   * Wire up tab link click handlers so that clicking a tab updates the hash
   * without triggering a full navigation, and shows the tab immediately.
   */
  function wireTabLinks() {
    document.querySelectorAll(".xlsx-tab-link").forEach(function (a) {
      a.addEventListener("click", function (e) {
        e.preventDefault();
        var tabId = this.dataset.tab;
        var docPath = getSpaDocPath();
        var newHash = docPath ? "#/" + docPath + "#" + tabId : "#" + tabId;
        // Use root-relative absolute URL — bare hash is relative to current
        // fetch URL which may be /pages/doc.html, producing /pages/#/... instead of /#/...
        history.pushState(null, "", "/" + newHash);
        showTab(tabId);
      });
    });
  }

  // =========================================================================
  // Keyboard navigation
  // =========================================================================

  /**
   * Get the Tabulator instance for the currently active tab, determined by
   * the URL hash anchor.  Falls back to the `.is-active` DOM class.
   * Returns null if no table is initialized for the active tab.
   */
  function getActiveTable() {
    // Try hash first: /#/doc.html#tab-id or /#/doc.html#row-anchor
    var hashTabId = getTabFromHash();
    if (hashTabId) {
      var tabId = hashTabId.replace(/^xlsx-tab-/, "");
      if (tabulatorInstances[tabId]) return tabulatorInstances[tabId];
    }
    // Fallback: DOM class
    var activeSection = document.querySelector(".xlsx-tab.is-active");
    if (!activeSection) return null;
    var domTabId = (activeSection.id || "").replace(/^xlsx-tab-/, "");
    return tabulatorInstances[domTabId] || null;
  }

  /**
   * Attach a global keydown handler for ArrowUp/ArrowDown navigation through
   * xlsx table rows.  Called once from init(); idempotent via the
   * keyboardHandlerAttached flag.
   *
   * Navigation moves through the Tabulator's visible (filtered/paginated)
   * row set.  Each step calls showMetadataPanel on the new row's BID and
   * scrolls the row into view.
   */
  function attachKeyboardHandler() {
    if (keyboardHandlerAttached) return;
    keyboardHandlerAttached = true;

    // Use capture phase so this handler runs before bubble-phase handlers
    // (e.g. traceability panel, nav tree scroll).  stopImmediatePropagation
    // prevents any other capture or bubble listeners from firing.
    document.addEventListener(
      "keydown",
      function (e) {
        // Escape closes the fullscreen xlsx panel.
        if (e.key === "Escape") {
          var panelEl = document.getElementById("noet-focused-panel");
          if (panelEl && panelEl.classList.contains("is-open")) {
            e.preventDefault();
            e.stopImmediatePropagation();
            closeFullscreen();
          }
          return;
        }

        if (e.key !== "ArrowUp" && e.key !== "ArrowDown") return;

        // Only handle when xlsx data is loaded (this page is a workbook).
        if (!xlsxData) return;

        // Don't hijack arrows when focus is inside a form control.
        var tag = document.activeElement?.tagName;
        if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT") return;

        var table = getActiveTable();
        if (!table || !table._noetBuilt) return;

        e.preventDefault();
        e.stopImmediatePropagation();

        var rows = table.getRows("active"); // visible/filtered rows
        if (rows.length === 0) return;

        var goingDown = e.key === "ArrowDown";

        // Find the index of the currently focused row.
        var currentIdx = -1;
        if (focusedRowBid) {
          for (var i = 0; i < rows.length; i++) {
            if (rows[i].getData()._bid === focusedRowBid) {
              currentIdx = i;
              break;
            }
          }
        }

        // Compute the next index.
        var nextIdx;
        if (currentIdx < 0) {
          // No focus yet — start at first or last row.
          nextIdx = goingDown ? 0 : rows.length - 1;
        } else {
          nextIdx = currentIdx + (goingDown ? 1 : -1);
          if (nextIdx < 0 || nextIdx >= rows.length) return; // at boundary
        }

        var nextRow = rows[nextIdx];
        var bid = nextRow.getData()._bid;
        if (!bid) return;

        focusedRowBid = bid;
        nextRow.getElement().scrollIntoView({ block: "nearest" });

        var activeCbs = window.__xlsxCbs || {};
        if (activeCbs.showMetadataPanel) {
          activeCbs.showMetadataPanel(bid);
        }

        // Update hash so the URL reflects the focused row, matching
        // the rowClick handler's behavior.
        var rowData = nextRow.getData();
        var docPath = getSpaDocPath();
        if (docPath) {
          var rowId = rowData._id || "";
          if (rowId) {
            var newHash = "#/" + docPath + "#" + rowId;
            history.replaceState(null, "", "/" + newHash);
          }
        }

        // Refocus the table row so the nav drawer doesn't capture
        // subsequent arrow keys.  showMetadataPanel triggers a nav tree
        // highlight update which can shift DOM focus to the nav panel.
        var rowEl = nextRow.getElement();
        if (rowEl) {
          // Ensure the row is focusable (tabindex=-1 allows programmatic focus
          // without adding it to the tab order).
          if (!rowEl.hasAttribute("tabindex")) {
            rowEl.setAttribute("tabindex", "-1");
          }
          rowEl.focus({ preventScroll: true });
        }
      },
      { capture: true },
    );
  }

  // =========================================================================
  // Initializer
  // =========================================================================

  /**
   * Update the shared callbacks object without re-running init.
   * Useful when viewer.js wires up showMetadataPanel after xlsx-tabs has
   * already been initialised — subsequent rowClick/cellClick handlers will
   * automatically pick up the new value via window.__xlsxCbs.
   *
   * @param {object} callbacks
   */
  window.noetSetXlsxCallbacks = function (callbacks) {
    window.__xlsxCbs = callbacks || {};
    cbs = window.__xlsxCbs;
  };

  /**
   * Main initializer. Safe to call multiple times (e.g. after SPA injection).
   * Resets per-instance state (initializedTabs, xlsxData) so re-injection
   * of a workbook document gets a fresh Tabulator for each tab.
   *
   * Callbacks are stored on window.__xlsxCbs so that all event handlers
   * (rowClick, cellClick) always read the latest value — even if the
   * callbacks object is enriched after Tabulator instances are created.
   *
   * @param {object} [callbacks] - Optional callbacks from the viewer:
   *   callbacks.showMetadataPanel(bid) — open metadata panel for a node
   *   callbacks.navigateToLink(key, el, event) — navigate to a relation key
   * @param {string} [pendingAnchor] - Optional section anchor (e.g. "#row-id")
   *   to navigate to after initialization.  Activates the tab containing the
   *   row and scrolls to it.
   */
  function init(callbacks, pendingAnchor) {
    // Bail early if there is no workbook data element in the current DOM.
    var dataEl = document.getElementById("noet-xlsx-data");
    if (!dataEl) return;

    // Store callbacks on window.__xlsxCbs so late-bound handlers always get
    // the most recent value. If callbacks is not provided, preserve whatever
    // was previously stored (another call may have set a richer object).
    if (callbacks) {
      window.__xlsxCbs = callbacks;
    } else if (!window.__xlsxCbs) {
      window.__xlsxCbs = {};
    }
    cbs = window.__xlsxCbs;

    // Reset per-document state so SPA re-navigation to a workbook page
    // gets fresh Tabulator instances rather than reusing stale ones.
    // Destroy all existing Tabulator instances before clearing the registry
    // so they release their DOM event listeners.
    Object.keys(tabulatorInstances).forEach(function (tid) {
      try {
        tabulatorInstances[tid].destroy();
      } catch (ex) {}
    });
    tabulatorInstances = {};
    initializedTabs = {};
    xlsxData = null;
    tabOrder = [];

    var data = loadXlsxData();
    if (!data) return;

    // Show nav bar now that JS is active.
    var nav = document.querySelector(".xlsx-tab-nav");
    if (nav) nav.classList.add("is-active");

    // Strip leading "#" from the anchor if present.
    var anchor = pendingAnchor ? pendingAnchor.replace(/^#/, "") : null;
    activateFromHash(anchor);
    wireTabLinks();
    attachKeyboardHandler();

    // Reset keyboard focus — new document, no row selected yet.
    focusedRowBid = null;

    // Signal to routing.js that xlsx-tabs handled the anchor navigation.
    // This prevents the redundant navigateToSection call which would fail
    // because Tabulator row elements don't exist as DOM IDs until after
    // tableBuilt fires.
    if (anchor) {
      window.__xlsxHandledAnchor = true;
    }
  }

  // Expose as a global so content.js can call it explicitly after SPA injection.
  window.noetInitXlsxTabs = init;

  // Auto-run on initial page load.
  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", init);
  } else {
    // DOMContentLoaded already fired (e.g. script loaded late or SPA context).
    init();
  }

  // Re-activate on hash changes (e.g. user navigates back/forward within the workbook).
  window.addEventListener("hashchange", activateFromHash);
})();
