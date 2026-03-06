/**
 * viewer/wasm.js — WASM module initialization and path-to-BID lookup
 *
 * Responsible for:
 *   - Loading the noet_core.js WASM module
 *   - Detecting sharded vs monolithic BeliefBase export format
 *   - Fetching and parsing beliefbase data (sharded or monolithic)
 *   - Loading search indices (always, from search/)
 *   - Constructing the BeliefBaseWasm instance
 *   - Validating the entry point
 *   - Populating state.navTree and triggering buildNavigation()
 *
 * ## Format Detection
 *
 * initializeWasm() probes for `beliefbase/manifest.json` first:
 *   - 200 OK  → sharded mode: BeliefBaseWasm.from_manifest + ShardManager.init()
 *   - 404     → monolithic mode: BeliefBaseWasm.from_json (existing path, unchanged)
 *
 * ## After initializeWasm() resolves
 *
 *   state.wasmModule    — the imported JS/WASM module
 *   state.beliefbase    — BeliefBaseWasm instance
 *   state.navTree       — NavTree { nodes: Map, roots: Array }
 *   state.shardManager  — ShardManager instance (sharded mode only, else null)
 *   state.searchIndex   — Map<bref, index> (both modes — always populated)
 *
 * ## Log level control (Rust tracing → browser console)
 *
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
import { ShardManager, loadMonolithicSearchIndices } from "./shard-manager.js";

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
 * Detects sharded vs monolithic format automatically by probing for
 * `beliefbase/manifest.json`. Loads search indices in both modes.
 *
 * Mutates: state.wasmModule, state.beliefbase, state.navTree,
 *          state.shardManager, state.searchIndex
 * Side-effect: calls buildNavigation() on success.
 *
 * @throws {Error} if the WASM module, BeliefBase data, or entry point are unavailable
 */
export async function initializeWasm() {
  console.log("[Noet] Loading WASM module...");

  // Dynamically import the generated JS/WASM glue module
  state.wasmModule = await import("/assets/noet_core.js");
  await state.wasmModule.default();
  console.log("[Noet] WASM module loaded successfully");

  // Read entry point BID from the <script id="noet-entry-bid"> tag injected by
  // the site generator into every SPA shell page.
  const entryBidScript = document.getElementById("noet-entry-bid");
  if (!entryBidScript) {
    throw new Error("No entry point BID found: <script id='noet-entry-bid'> missing");
  }
  const entryBidString = JSON.parse(entryBidScript.textContent);
  console.log("[Noet] Entry point BID from script tag:", entryBidString);

  // --- Format detection: probe for shard manifest ---
  const shardManifestResp = await fetch("/beliefbase/manifest.json");
  const isSharded = shardManifestResp.ok;

  if (isSharded) {
    // =========================================================================
    // Sharded path
    // =========================================================================
    console.log("[Noet] Sharded BeliefBase detected. Initializing via ShardManager...");
    const manifestJson = await shardManifestResp.text();
    const manifest = JSON.parse(manifestJson);

    // Construct empty BeliefBaseWasm from the manifest.
    state.beliefbase = state.wasmModule.BeliefBaseWasm.from_manifest(manifestJson, entryBidString);
    console.log("[Noet] BeliefBaseWasm (sharded) initialized");

    // ShardManager loads search indices + global shard + entry network.
    state.shardManager = new ShardManager(state.beliefbase, manifest);
    await state.shardManager.init();

    // Expose the search index from the shard manager on state for Issue 54.
    state.searchIndex = state.shardManager.searchIndex;

    console.log(`[Noet] Sharded init complete. Loaded shards: ${state.beliefbase.loaded_shards()}`);
  } else {
    // =========================================================================
    // Monolithic path (existing behaviour — unchanged)
    // =========================================================================
    console.log("[Noet] No shard manifest found — loading monolithic beliefbase.json...");

    const response = await fetch("/beliefbase.json");
    if (!response.ok) {
      throw new Error(`Failed to fetch beliefbase.json: ${response.status}`);
    }
    const beliefbaseJson = await response.text();
    console.log("[Noet] BeliefBase JSON loaded successfully");

    state.beliefbase = new state.wasmModule.BeliefBaseWasm(beliefbaseJson, entryBidString);
    console.log("[Noet] BeliefBaseWasm (monolithic) initialized");

    state.shardManager = null;

    // Load search indices for full-corpus search (monolithic mode).
    state.searchIndex = await loadMonolithicSearchIndices();
  }

  // =========================================================================
  // Shared validation (both paths)
  // =========================================================================

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
