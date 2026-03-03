/**
 * viewer/wasm.js — WASM module initialization and path-to-BID lookup
 *
 * Responsible for:
 *   - Loading the noet_core.js WASM module
 *   - Fetching and parsing beliefbase.json
 *   - Constructing the BeliefBaseWasm instance
 *   - Validating the entry point
 *   - Populating state.navTree and triggering buildNavigation()
 *
 * After initializeWasm() resolves, the following state fields are populated:
 *   state.wasmModule   — the imported JS/WASM module
 *   state.beliefbase   — the BeliefBaseWasm instance
 *   state.navTree      — NavTree { nodes: Map, roots: Array }
 *
 * Log level control (Rust tracing → browser console):
 *   setLogLevel("debug")  // verbose; default in debug WASM builds
 *   setLogLevel("info")
 *   setLogLevel("warn")   // default in release WASM builds
 *   setLogLevel("error")
 *   setLogLevel("off")
 *   Must be called after initializeWasm() resolves.
 *
 * ⚠️  WASM Data Type Patterns
 * ===========================
 * Rust BTreeMap/HashMap serialize to JavaScript **Map objects**, NOT plain objects.
 *
 *   WRONG:  Object.keys(data)      // ❌ always []
 *   RIGHT:  data.entries()         // ✅ iterator of [key, value]
 *           data.get(key)          // ✅
 *           data.size              // ✅
 *
 * Exception: get_paths() returns a plain Object (uses serde_json).
 *   RIGHT:  paths[bid]             // ✅
 */

import { state } from "./state.js";
import { buildNavigation } from "./navigation.js";

// =============================================================================
// Log level control
// =============================================================================

/**
 * Set the Rust tracing log level for the WASM module at runtime.
 *
 * Must be called after initializeWasm() resolves (requires state.wasmModule).
 * Valid levels: "trace", "debug", "info", "warn", "error", "off"
 * Default in debug builds: "debug". Default in release builds: "warn".
 *
 * @param {string} level
 */
export function setLogLevel(level) {
  if (!state.wasmModule) {
    console.warn("[Noet] setLogLevel called before WASM module loaded; level will not be applied");
    return;
  }
  state.wasmModule.BeliefBaseWasm.set_log_level(level);
}

// =============================================================================
// Public API
// =============================================================================

/**
 * Load and initialize the WASM module, BeliefBase, and navigation tree.
 *
 * Mutates: state.wasmModule, state.beliefbase, state.navTree
 * Side-effect: calls buildNavigation() on success.
 *
 * @throws {Error} if the WASM module, beliefbase.json, or entry point are unavailable
 */
export async function initializeWasm() {
  console.log("[Noet] Loading WASM module...");

  // Dynamically import the generated JS/WASM glue module
  state.wasmModule = await import("/assets/noet_core.js");
  await state.wasmModule.default();
  console.log("[Noet] WASM module loaded successfully");

  // Fetch the serialized belief base
  console.log("[Noet] Loading beliefbase.json...");
  const response = await fetch("/beliefbase.json");
  if (!response.ok) {
    throw new Error(`Failed to fetch beliefbase.json: ${response.status}`);
  }
  const beliefbaseJson = await response.text();
  console.log("[Noet] BeliefBase JSON loaded successfully");

  // Read entry point BID from the <script id="noet-entry-bid"> tag injected by
  // the site generator into every SPA shell page.
  const entryBidScript = document.getElementById("noet-entry-bid");
  if (!entryBidScript) {
    throw new Error("No entry point BID found: <script id='noet-entry-bid'> missing");
  }
  const entryBidString = JSON.parse(entryBidScript.textContent);
  console.log("[Noet] Entry point BID from script tag:", entryBidString);

  // Construct the BeliefBaseWasm instance
  state.beliefbase = new state.wasmModule.BeliefBaseWasm(beliefbaseJson, entryBidString);
  console.log("[Noet] BeliefBaseWasm initialized");

  // --- Validation ---

  const entryPoint = state.beliefbase.entryPoint();
  console.log("[Noet] Entry point BID:", entryPoint.bid, "bref:", entryPoint.bref);

  const entryPointNode = state.beliefbase.get_by_bid(entryPoint.bid);
  if (!entryPointNode) {
    throw new Error(`Entry point node ${entryPoint.bid} not found in beliefbase`);
  }
  console.log("[Noet] ✓ Entry point node exists:", entryPointNode.title);

  const paths = state.beliefbase.get_paths();
  if (!paths[entryPoint.bid]) {
    // Networks without a path map are valid (they contain documents but have no
    // direct HTML representation of their own).
    console.warn(
      "[Noet] ⚠️ Entry point has no path map (expected for Network nodes)",
      "| Available path maps:",
      Object.keys(paths),
    );
  } else {
    console.log(
      "[Noet] ✓ Entry point has path map with",
      Object.keys(paths[entryPoint.bid]).length,
      "paths",
    );
  }

  const nodeCount = state.beliefbase.node_count();
  console.log("[Noet] ✓ BeliefBase loaded:", nodeCount, "nodes");

  // --- Navigation tree ---

  state.navTree = state.beliefbase.get_nav_tree();
  console.log("[Noet] NavTree loaded:", state.navTree);

  buildNavigation();
}

// =============================================================================
// Path-to-BID lookup
// =============================================================================

/**
 * Resolve a document path to a BID using the beliefbase path map.
 *
 * The path map is keyed by relative paths without a leading slash
 * (e.g. "net1_dir1/doc.html", NOT "/net1_dir1/doc.html").
 * Section anchors in the path are stripped before lookup.
 *
 * @param {string} path - Document path, optionally with a section anchor
 *   (e.g. "/net1_dir1/doc.html" or "net1_dir1/doc.html#section")
 * @returns {string|null} BID if found, null otherwise
 */
export function getBidFromPath(path) {
  if (!state.beliefbase) return null;

  try {
    const entryPoint = state.beliefbase.entryPoint();
    const paths = state.beliefbase.get_paths();
    const pathsMap = paths[entryPoint.bid];

    if (!pathsMap) {
      console.warn("[Noet] No paths found for entry point:", entryPoint.bid);
      return null;
    }

    // Strip section anchor and leading slash — path map keys have neither
    let cleanPath = stripAnchor(path);
    if (cleanPath.startsWith("/")) {
      cleanPath = cleanPath.substring(1);
    }

    const bid = pathsMap[cleanPath];
    if (bid) {
      console.log(`[Noet] Found BID for path ${cleanPath}:`, bid);
      return bid;
    }

    console.log(`[Noet] No BID found for path: ${cleanPath}`);
    return null;
  } catch (error) {
    console.error("[Noet] Error looking up BID from path:", error);
    return null;
  }
}

// =============================================================================
// Internal helpers
// =============================================================================

/**
 * Remove the section anchor from a path string.
 * Uses the WASM pathParts helper when available; falls back to a string split.
 *
 * @param {string} path - e.g. "dir/doc.html#section"
 * @returns {string} e.g. "dir/doc.html"
 */
function stripAnchor(path) {
  if (!path) return path;

  if (state.wasmModule) {
    const parts = state.wasmModule.BeliefBaseWasm.pathParts(path);
    return parts.path ? `${parts.path}/${parts.filename}` : parts.filename;
  }

  // Fallback: naive split on last '#'
  const hashIndex = path.indexOf("#");
  return hashIndex !== -1 ? path.substring(0, hashIndex) : path;
}
