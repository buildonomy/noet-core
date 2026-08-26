/**
 * viewer/wasm.js — WASM module initialization and path-to-BID lookup
 *
 * Responsible for:
 *   - Loading the noet_core.js WASM module
 *   - Loading the codec manifest (codecs.json) into the WASM extension registry
 *   - Detecting sharded vs monolithic BeliefBase export format
 *   - Fetching and parsing beliefbase data (sharded or monolithic)
 *   - Constructing the BeliefBaseWasm instance
 *   - Validating the entry point
 *   - Populating state.navTree and triggering buildNavigation()
 *
 * Search indices are loaded lazily by search.js on first user interaction,
 * NOT during initializeWasm(). This keeps ~20 MB of index fetches off the
 * critical render path.
 *
 * ## Format Detection
 *
 * initializeWasm() probes for `beliefbase/manifest.json` first:
 *   - 200 OK  → sharded mode: BeliefBaseWasm.from_manifest + ShardManager.init()
 *   - 404     → monolithic mode: BeliefBaseWasm.from_msgpack(beliefbase.msgpack)
 *
 * ## Required assets
 *
 * `codecs.json` is required, not optional. It is the only way the WASM runtime
 * learns which extensions are documents (`.yaml`, `.h`, ... — anything beyond
 * BUILTIN_EXTENSIONS), so initializeWasm() throws if it is missing, unreachable
 * or malformed rather than resolving links wrongly. viewer.js catches that and
 * shows the nav error state. See the fetch site below for the full rationale.
 *
 * ## After initializeWasm() resolves
 *
 *   state.wasmModule    — the imported JS/WASM module
 *   state.beliefbase    — BeliefBaseWasm instance
 *   state.navTree       — NavTree { nodes: Map, roots: Array }
 *   state.shardManager  — ShardManager instance (sharded mode only, else null)
 *   state.searchIndex   — Map (kept for API compatibility; search data now lives in WASM)
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
import { setBrefResolver } from "./utils.js";

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
    console.warn(
      "[Noet] setLogLevel called before WASM module loaded; level will not be applied",
    );
    return;
  }
  state.wasmModule.BeliefBaseWasm.set_log_level(level);
}

// =============================================================================
// Public API
// =============================================================================

/**
 * Read the base URL embedded by the site generator.
 *
 * Written into every SPA shell as `<script id="noet-base-url">`.
 * Returns an empty string when absent (root-relative, default local serve).
 * Never has a trailing slash.
 *
 * @returns {string}
 */
export function readBaseUrl() {
  const script = document.getElementById("noet-base-url");
  if (script) {
    try {
      const val = JSON.parse(script.textContent);
      return typeof val === "string" ? val.replace(/\/$/, "") : "";
    } catch (_) {
      // fall through
    }
  }
  return "";
}

/**
 * Read the asset version token embedded by the site generator.
 *
 * The token is the FNV-1a hex hash of the serialized beliefbase content,
 * written into every SPA shell as `<script id="noet-asset-version">`.
 * Appending it as `?v=<token>` to dynamic imports and data fetches busts
 * both the HTTP cache and the browser module-specifier cache when the
 * beliefbase changes between deployments.
 *
 * Falls back to the current timestamp (ms) when the tag is absent so that
 * a locally-served page without the tag still gets fresh assets.
 *
 * @returns {string}
 */
function readAssetVersion() {
  const script = document.getElementById("noet-asset-version");
  if (script) {
    try {
      return JSON.parse(script.textContent);
    } catch (_) {
      // fall through
    }
  }
  console.warn("[Noet] noet-asset-version tag missing — using timestamp as cache-buster");
  return String(Date.now());
}

/**
 * Load and initialize the WASM module, BeliefBase, and navigation tree.
 *
 * Detects sharded vs monolithic format automatically by probing for
 * `beliefbase/manifest.json`. Loads search indices in both modes.
 *
 * Mutates: state.wasmModule, state.beliefbase, state.navTree,
 *          state.shardManager
 * Side-effect: calls buildNavigation() on success.
 *
 * @throws {Error} if the WASM module, BeliefBase data, or entry point are unavailable
 */
export async function initializeWasm() {
  console.log("[Noet] Loading WASM module...");
  let searchIndexPromise = Promise.resolve();

  // Read the base URL and asset version token before any fetch so every
  // request in this function uses the same values.
  const baseUrl = readBaseUrl();
  const assetVersion = readAssetVersion();
  console.log("[Noet] Asset version:", assetVersion);

  // Dynamically import the generated JS/WASM glue module.
  // The ?v= query parameter makes the URL unique per beliefbase version,
  // defeating both the HTTP cache and the browser module-specifier cache.
  state.wasmModule = await import(`${baseUrl}/assets/noet_core.js?v=${assetVersion}`);
  // Pass the versioned WASM URL explicitly to bust Chrome's compiled WASM cache.
  // wasm-bindgen's generated init() hardcodes the .wasm filename via new URL(..., import.meta.url),
  // which strips query parameters — so the compiled binary is cached forever by URL.
  // Passing a fetch() promise with ?v= forces a new cache key on every asset version change.
  await state.wasmModule.default(
    fetch(`${baseUrl}/assets/noet_core_bg.wasm?v=${assetVersion}`),
  );
  console.log("[Noet] WASM module loaded successfully");

  // Load codec manifest so the WASM runtime knows about all document extensions
  // (including those registered by application shims at build time).
  // This must happen before any normalize_path_extension or link-resolution calls.
  //
  // A failure here is fatal, deliberately. Without the manifest the WASM side
  // knows only BUILTIN_EXTENSIONS (md/xlsx/ods), so every link to a document
  // with a walk-codec or shim extension (.yaml, .h, ...) silently normalises to
  // a directory URL that 404s — a site that looks fine until you click the
  // wrong link. Degrading to built-ins trades a loud, one-line failure for a
  // broad, silent one.
  //
  // Nor is it a transient condition worth retrying. `export_beliefbase` writes
  // codecs.json unconditionally in both monolithic and sharded modes, and this
  // code only runs once the shell HTML and the WASM binary have already been
  // fetched from the same origin. Anything that can serve those can serve a
  // sibling JSON file; if it cannot, the deployment is incomplete and a retry
  // will fail identically.
  const codecUrl = `${baseUrl}/codecs.json?v=${assetVersion}`;
  let codecResp;
  try {
    codecResp = await fetch(codecUrl);
  } catch (e) {
    throw new Error(
      `Failed to fetch the codec manifest (${codecUrl}): ${e.message}. ` +
        `Document links cannot be resolved without it.`,
    );
  }
  if (!codecResp.ok) {
    throw new Error(
      `Codec manifest missing (${codecUrl} returned ${codecResp.status}). ` +
        `codecs.json is written next to beliefbase data on every export — ` +
        `this usually means the deployment did not copy the whole output directory.`,
    );
  }
  // setKnownExtensions throws on malformed JSON; let it propagate for the same
  // reason a 404 does — a corrupt manifest resolves links no better than none.
  state.wasmModule.BeliefBaseWasm.setKnownExtensions(await codecResp.text());
  console.log("[Noet] Codec manifest loaded");

  // Read entry point BID from the <script id="noet-entry-bid"> tag injected by
  // the site generator into every SPA shell page.
  const entryBidScript = document.getElementById("noet-entry-bid");
  if (!entryBidScript) {
    throw new Error("No entry point BID found: <script id='noet-entry-bid'> missing");
  }
  const entryBidString = JSON.parse(entryBidScript.textContent);
  console.log("[Noet] Entry point BID from script tag:", entryBidString);

  // --- Format detection: probe for shard manifest ---
  const shardManifestResp = await fetch(
    `${baseUrl}/beliefbase/manifest.json?v=${assetVersion}`,
  );
  const isSharded = shardManifestResp.ok;

  if (isSharded) {
    // =========================================================================
    // Sharded path
    // =========================================================================
    console.log("[Noet] Sharded BeliefBase detected. Initializing via ShardManager...");
    const manifestJson = await shardManifestResp.text();
    const manifest = JSON.parse(manifestJson);

    // Construct empty BeliefBaseWasm from the manifest.
    state.beliefbase = state.wasmModule.BeliefBaseWasm.from_manifest(
      manifestJson,
      entryBidString,
    );
    window.__noetBeliefBase = state.beliefbase;
    window.__noetToAnchor = (s) => state.wasmModule.BeliefBaseWasm.toAnchor(s);
    console.log("[Noet] BeliefBaseWasm (sharded) initialized");

    // ShardManager loads global shard + target/entry network.
    // Search indices are deferred until the user interacts with search.
    // Pass assetVersion so it can cache-bust all shard and search fetches.
    state.shardManager = new ShardManager(
      state.beliefbase,
      manifest,
      assetVersion,
      baseUrl,
    );

    await state.shardManager.init();

    console.log(
      `[Noet] Sharded init complete. Loaded shards: ${state.beliefbase.loaded_shards()}`,
    );
  } else {
    // =========================================================================
    // Monolithic path — msgpack (mirrors sharded wire format)
    // =========================================================================
    console.log(
      "[Noet] No shard manifest found — loading monolithic beliefbase.msgpack...",
    );

    const response = await fetch(`${baseUrl}/beliefbase.msgpack?v=${assetVersion}`);
    if (!response.ok) {
      throw new Error(`Failed to fetch beliefbase.msgpack: ${response.status}`);
    }
    const buffer = await response.arrayBuffer();
    console.log(
      `[Noet] BeliefBase msgpack loaded: ${(buffer.byteLength / 1024 / 1024).toFixed(2)} MB`,
    );

    state.beliefbase = state.wasmModule.BeliefBaseWasm.from_msgpack(
      new Uint8Array(buffer),
      entryBidString,
    );
    window.__noetBeliefBase = state.beliefbase;
    window.__noetToAnchor = (s) => state.wasmModule.BeliefBaseWasm.toAnchor(s);
    console.log("[Noet] BeliefBaseWasm (monolithic) initialized");

    state.shardManager = null;

    // Load search indices concurrently — they are only needed when the user
    // types a query, so they must not block the nav tree build and first render.
    // Kick off the fetch chain now and join it after navigation is ready.
    searchIndexPromise = loadMonolithicSearchIndices(
      state.beliefbase,
      assetVersion,
      baseUrl,
    );
  }

  // =========================================================================
  // Shared validation (both paths)
  // =========================================================================

  const entryPoint = state.beliefbase.entryPoint();
  console.log("[Noet] Entry point BID:", entryPoint.bid, "bref:", entryPoint.bref);

  const entryPointNode = state.beliefbase.get_by_bid(entryPoint.bid);
  if (!entryPointNode) {
    throw new Error(
      `Entry point node ${entryPoint.bid} not found in beliefbase. ` +
        `This usually means the HTML pages and beliefbase.json were generated at different times. ` +
        `Try a clean rebuild with --write to realign them.`,
    );
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

  // Inject WASM-backed bref resolver so brefFromBid() uses the correct
  // UUIDv5-based computation instead of the fallback hex-slice.
  const bb = state.beliefbase;
  setBrefResolver((bid) => bb.get_bref_from_bid(bid));

  // --- Navigation tree ---

  state.navTree = state.beliefbase.get_nav_tree();
  console.log("[Noet] NavTree loaded:", state.navTree);

  buildNavigation();

  // Search indices: load after first paint, not blocking the render path.
  // Monolithic mode: indices were kicked off earlier — join them here.
  // Sharded mode: fire-and-forget — they load in the background while the
  // user sees content.  No await — search will be available shortly after
  // first paint without any lag on first query.
  if (state.shardManager) {
    state.shardManager.loadSearchIndices();
  } else {
    await searchIndexPromise;
  }
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
