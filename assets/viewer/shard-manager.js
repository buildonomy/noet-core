/**
 * viewer/shard-manager.js — ShardManager: memory-budgeted BeliefBase shard loading
 *
 * Manages loading and unloading of per-network BeliefBase shards under a
 * browser memory budget. On init, fetches search/manifest.json and all
 * .idx.json files so full-corpus search is available immediately — even
 * before any data shard is loaded.
 *
 * ## Initialization (sharded mode)
 *
 *   const manager = new ShardManager(beliefbase, manifest);
 *   await manager.init();          // loads search indices, global shard, entry network
 *   // Full-corpus search is now available via manager.searchIndex
 *   // Entry network data is loaded and queryable
 *
 * ## Network Loading
 *
 *   await manager.loadNetwork("abc12");   // loads beliefbase/networks/abc12.json
 *   await manager.unloadNetwork("abc12"); // removes its nodes from BeliefBase
 *
 * ## Memory Budget
 *
 *   manager.getMemoryUsage()    // { usedMb, budgetMb, percent }
 *   manager.canLoadNetwork(meta) // false if it would exceed budget
 *
 * ## References
 *
 * - docs/design/search_and_sharding.md §6 — Memory budget model
 * - docs/design/search_and_sharding.md §8 — WASM integration
 * - Issue 50: BeliefBase Sharding
 * - Issue 54: Full-Text Search MVP (consumes searchIndex)
 */

// =============================================================================
// Constants
// =============================================================================

/** Base path for shard manifest and network shard files. */
const BELIEFBASE_DIR = "/beliefbase";

/** Base path for search index files. */
const SEARCH_DIR = "/search";

/** Warn in the UI when memory usage exceeds this fraction of budget. */
const WARN_THRESHOLD_80 = 0.8;

/** Critical warning threshold — suggest unloading networks. */
const WARN_THRESHOLD_90 = 0.9;

// =============================================================================
// ShardManager
// =============================================================================

/**
 * Manages per-network BeliefBase shard loading under a memory budget.
 *
 * Constructed with a `BeliefBaseWasm` instance (from `from_manifest`) and the
 * parsed shard manifest object. After `init()`, the manager owns:
 *
 *   - `this.searchIndex`  — merged inverted index from all .idx.json files
 *   - Loaded data shards tracked internally by `BeliefBaseWasm.loaded_shards()`
 *
 * Memory accounting uses `BeliefBaseWasm.memory_usage_mb()` for the data side
 * and tracks search index sizes separately (they are loaded eagerly, always).
 */
export class ShardManager {
  /**
   * @param {import('./wasm.js').BeliefBaseWasm} beliefbase
   *   The `BeliefBaseWasm` instance created via `BeliefBaseWasm.from_manifest`.
   * @param {ShardManifest} manifest
   *   Parsed contents of `beliefbase/manifest.json`.
   */
  constructor(beliefbase, manifest) {
    /** @type {import('./wasm.js').BeliefBaseWasm} */
    this.beliefbase = beliefbase;

    /** @type {ShardManifest} */
    this.manifest = manifest;

    /**
     * Per-network search indices, keyed by bref string.
     * Populated eagerly during init().
     * @type {Map<string, NetworkSearchIndex>}
     */
    this.searchIndex = new Map();

    /**
     * Set of bref strings whose data shards are currently loaded.
     * Mirrors what BeliefBaseWasm tracks internally — kept in sync here
     * so JavaScript can check without calling into WASM.
     * @type {Set<string>}
     */
    this._loadedNetworks = new Set();

    /** Global shard is tracked under the special key "global". */
    this._globalLoaded = false;

    /**
     * Listeners registered for shard load/unload events.
     * @type {Array<function>}
     */
    this._listeners = [];
  }

  // ===========================================================================
  // Initialization
  // ===========================================================================

  /**
   * Initialize the shard manager:
   *
   * 1. Fetch search/manifest.json and all .idx.json files (full-corpus search).
   * 2. Load the global shard (beliefbase/global.json).
   * 3. Load the entry-point network shard (derived from `beliefbase.entryPoint()`).
   *
   * After this resolves, `this.searchIndex` is populated and the entry network
   * is queryable. The WASM module has enough data to render the initial page.
   *
   * @returns {Promise<void>}
   * @throws {Error} if the global shard or entry network shard cannot be fetched
   */
  async init() {
    console.log("[ShardManager] Initializing...");

    // Step 1: Load all search indices eagerly so full-corpus search works
    // before any data shard is loaded.
    await this._loadAllSearchIndices();

    // Step 2: Load the global shard — required for cross-network link resolution.
    await this._loadGlobalShard();

    // Step 3: Load the entry-point network shard.
    const entryPoint = this.beliefbase.entryPoint();
    const entryBref = entryPoint.bref;
    const entryNetworkMeta = this.manifest.networks.find(
      (n) => n.bref === entryBref,
    );

    if (!entryNetworkMeta) {
      // Entry point may be a top-level API node not in any network shard —
      // log a warning but do not fail hard.
      console.warn(
        `[ShardManager] Entry network bref '${entryBref}' not found in manifest. ` +
          "The viewer may have limited data available.",
      );
      return;
    }

    await this.loadNetwork(entryBref);
    console.log(
      `[ShardManager] Init complete. Loaded global + network '${entryBref}'. ` +
        `Node count: ${this.beliefbase.node_count()}`,
    );
  }

  // ===========================================================================
  // Search index loading
  // ===========================================================================

  /**
   * Fetch `search/manifest.json` and all `.idx.json` files listed there.
   *
   * Failures for individual index files are logged but do not abort init —
   * the user simply gets reduced search coverage for that network.
   *
   * @returns {Promise<void>}
   */
  async _loadAllSearchIndices() {
    console.log("[ShardManager] Loading search manifest...");

    let searchManifest;
    try {
      const resp = await fetch(`${SEARCH_DIR}/manifest.json`);
      if (!resp.ok) {
        console.warn(
          `[ShardManager] search/manifest.json not found (${resp.status}). ` +
            "Full-corpus search will be unavailable.",
        );
        return;
      }
      searchManifest = await resp.json();
    } catch (err) {
      console.warn(`[ShardManager] Failed to fetch search manifest: ${err}`);
      return;
    }

    const networks = searchManifest.networks ?? [];
    console.log(
      `[ShardManager] Fetching ${networks.length} search index file(s)...`,
    );

    // Fetch all indices in parallel.
    const fetches = networks.map(async (meta) => {
      try {
        const resp = await fetch(`${SEARCH_DIR}/${meta.path}`);
        if (!resp.ok) {
          console.warn(
            `[ShardManager] Failed to fetch search index '${meta.path}': ${resp.status}`,
          );
          return;
        }
        const index = await resp.json();
        this.searchIndex.set(meta.bref, index);
      } catch (err) {
        console.warn(
          `[ShardManager] Error loading search index for '${meta.bref}': ${err}`,
        );
      }
    });

    await Promise.all(fetches);
    console.log(
      `[ShardManager] Search indices loaded: ${this.searchIndex.size} / ${networks.length} networks`,
    );
  }

  // ===========================================================================
  // Data shard loading
  // ===========================================================================

  /**
   * Load the global shard (`beliefbase/global.json`) into BeliefBase.
   *
   * The global shard contains the API node, system namespace nodes, and
   * cross-network relations. It must be loaded before any network shard
   * so that cross-network link resolution works correctly.
   *
   * @returns {Promise<void>}
   * @throws {Error} if the global shard cannot be fetched or parsed
   */
  async _loadGlobalShard() {
    console.log("[ShardManager] Loading global shard...");
    const resp = await fetch(`${BELIEFBASE_DIR}/global.json`);
    if (!resp.ok) {
      throw new Error(
        `[ShardManager] Failed to fetch global shard: ${resp.status}`,
      );
    }
    const json = await resp.text();
    const nodeCount = this.beliefbase.load_shard("global", json);
    this._globalLoaded = true;
    console.log(
      `[ShardManager] Global shard loaded. BeliefBase node count: ${nodeCount}`,
    );
  }

  /**
   * Load a per-network data shard by bref.
   *
   * Checks the memory budget before loading. If the load would exceed the
   * configured budget, the load is refused and an error is thrown.
   *
   * Loading the same network twice is idempotent — the shard is unloaded
   * and reloaded from fresh data (handled by BeliefBaseWasm.load_shard).
   *
   * @param {string} bref — 5-hex-char network bref
   * @returns {Promise<number>} Total node count after loading
   * @throws {Error} if budget exceeded, fetch fails, or WASM rejects the shard
   */
  async loadNetwork(bref) {
    const meta = this.manifest.networks.find((n) => n.bref === bref);
    if (!meta) {
      throw new Error(
        `[ShardManager] loadNetwork: bref '${bref}' not in manifest`,
      );
    }

    // Budget check (skip if already loaded — reload is always safe).
    if (!this._loadedNetworks.has(bref) && !this.canLoadNetwork(meta)) {
      const usage = this.getMemoryUsage();
      throw new Error(
        `[ShardManager] Cannot load network '${bref}' (${meta.estimated_size_mb.toFixed(1)} MB): ` +
          `would exceed budget. Currently using ${usage.usedMb.toFixed(1)} / ${usage.budgetMb.toFixed(1)} MB. ` +
          "Unload other networks first.",
      );
    }

    console.log(`[ShardManager] Loading network shard '${bref}'...`);
    const resp = await fetch(`${BELIEFBASE_DIR}/networks/${bref}.json`);
    if (!resp.ok) {
      throw new Error(
        `[ShardManager] Failed to fetch network shard '${bref}': ${resp.status}`,
      );
    }
    const json = await resp.text();
    const nodeCount = this.beliefbase.load_shard(bref, json);
    this._loadedNetworks.add(bref);
    this._notifyListeners({ type: "loaded", bref, nodeCount });
    console.log(
      `[ShardManager] Network '${bref}' loaded. Total nodes: ${nodeCount}`,
    );
    return nodeCount;
  }

  /**
   * Unload a per-network data shard by bref, removing its nodes from BeliefBase.
   *
   * The global shard cannot be unloaded via this method — it is always required.
   *
   * @param {string} bref — 5-hex-char network bref
   * @returns {Promise<number>} Total node count after unloading
   * @throws {Error} if the network was not loaded or WASM rejects the unload
   */
  async unloadNetwork(bref) {
    if (!this._loadedNetworks.has(bref)) {
      console.warn(
        `[ShardManager] unloadNetwork: '${bref}' is not currently loaded — ignoring`,
      );
      return this.beliefbase.node_count();
    }

    console.log(`[ShardManager] Unloading network shard '${bref}'...`);
    const nodeCount = this.beliefbase.unload_shard(bref);
    this._loadedNetworks.delete(bref);
    this._notifyListeners({ type: "unloaded", bref, nodeCount });
    console.log(
      `[ShardManager] Network '${bref}' unloaded. Remaining nodes: ${nodeCount}`,
    );
    return nodeCount;
  }

  // ===========================================================================
  // Memory budget
  // ===========================================================================

  /**
   * Returns current memory usage information.
   *
   * @returns {{ usedMb: number, budgetMb: number, percent: number, warning: string|null }}
   *
   *   - `usedMb`    — estimated MB currently used by loaded data shards
   *   - `budgetMb`  — configured budget from the shard manifest
   *   - `percent`   — usedMb / budgetMb as a value in [0, 1]
   *   - `warning`   — null | "warn" | "critical" (for UI indicator color)
   */
  getMemoryUsage() {
    const usedMb = this.beliefbase.memory_usage_mb();
    const budgetMb = this.manifest.memoryBudgetMB ?? 200;
    const percent = budgetMb > 0 ? usedMb / budgetMb : 0;

    let warning = null;
    if (percent >= WARN_THRESHOLD_90) {
      warning = "critical";
    } else if (percent >= WARN_THRESHOLD_80) {
      warning = "warn";
    }

    return { usedMb, budgetMb, percent, warning };
  }

  /**
   * Returns true if loading the given network would stay within the memory budget.
   *
   * @param {NetworkShardMeta} meta — Entry from `manifest.networks`
   * @returns {boolean}
   */
  canLoadNetwork(meta) {
    const { usedMb, budgetMb } = this.getMemoryUsage();
    return usedMb + meta.estimated_size_mb <= budgetMb;
  }

  // ===========================================================================
  // State queries
  // ===========================================================================

  /**
   * Returns true if the given bref's data shard is currently loaded.
   *
   * @param {string} bref
   * @returns {boolean}
   */
  isNetworkLoaded(bref) {
    return this._loadedNetworks.has(bref);
  }

  /**
   * Returns an array of bref strings for currently-loaded network shards.
   * Does not include "global".
   *
   * @returns {string[]}
   */
  getLoadedNetworks() {
    return Array.from(this._loadedNetworks);
  }

  /**
   * Returns the network metadata entry for a bref, or null if not found.
   *
   * @param {string} bref
   * @returns {NetworkShardMeta|null}
   */
  getNetworkMeta(bref) {
    return this.manifest.networks.find((n) => n.bref === bref) ?? null;
  }

  // ===========================================================================
  // Event listeners
  // ===========================================================================

  /**
   * Register a callback invoked on shard load/unload events.
   *
   * The callback receives an event object:
   *   { type: "loaded"|"unloaded", bref: string, nodeCount: number }
   *
   * @param {function} listener
   */
  addListener(listener) {
    this._listeners.push(listener);
  }

  /**
   * Remove a previously-registered listener.
   *
   * @param {function} listener
   */
  removeListener(listener) {
    this._listeners = this._listeners.filter((l) => l !== listener);
  }

  /**
   * @private
   */
  _notifyListeners(event) {
    for (const listener of this._listeners) {
      try {
        listener(event);
      } catch (err) {
        console.error("[ShardManager] Listener error:", err);
      }
    }
  }
}

// =============================================================================
// Monolithic-mode helper
// =============================================================================

/**
 * Load search indices in monolithic mode (no sharding).
 *
 * In monolithic mode the `beliefbase.json` is already loaded by `initializeWasm`.
 * This function handles the search-only part: fetch `search/manifest.json` and
 * all `.idx.json` files, returning a `Map<bref, index>` suitable for use as
 * `state.searchIndex`.
 *
 * Called by `initializeWasm` when no `beliefbase/manifest.json` is detected.
 *
 * @returns {Promise<Map<string, NetworkSearchIndex>>}
 */
export async function loadMonolithicSearchIndices() {
  const searchIndex = new Map();

  let manifest;
  try {
    const resp = await fetch(`${SEARCH_DIR}/manifest.json`);
    if (!resp.ok) {
      console.warn(
        `[Noet] search/manifest.json not found (${resp.status}). Search unavailable.`,
      );
      return searchIndex;
    }
    manifest = await resp.json();
  } catch (err) {
    console.warn(`[Noet] Failed to load search manifest: ${err}`);
    return searchIndex;
  }

  const networks = manifest.networks ?? [];
  const fetches = networks.map(async (meta) => {
    try {
      const resp = await fetch(`${SEARCH_DIR}/${meta.path}`);
      if (!resp.ok) {
        console.warn(`[Noet] Could not fetch '${meta.path}': ${resp.status}`);
        return;
      }
      const index = await resp.json();
      searchIndex.set(meta.bref, index);
    } catch (err) {
      console.warn(`[Noet] Error fetching search index '${meta.bref}': ${err}`);
    }
  });

  await Promise.all(fetches);
  console.log(
    `[Noet] Monolithic search indices loaded: ${searchIndex.size} / ${networks.length}`,
  );
  return searchIndex;
}

// =============================================================================
// JSDoc type definitions (for IDE tooling only — not runtime)
// =============================================================================

/**
 * @typedef {Object} ShardManifest
 * @property {string} version
 * @property {true} sharded
 * @property {number} memoryBudgetMB
 * @property {NetworkShardMeta[]} networks
 * @property {GlobalShardMeta} global
 */

/**
 * @typedef {Object} NetworkShardMeta
 * @property {string} bref
 * @property {string} bid
 * @property {string} title
 * @property {number} node_count
 * @property {number} relation_count
 * @property {number} estimated_size_mb
 * @property {string} path
 * @property {string} search_index_path
 * @property {number} search_index_size_kb
 */

/**
 * @typedef {Object} GlobalShardMeta
 * @property {number} node_count
 * @property {number} estimated_size_mb
 * @property {string} path
 */

/**
 * @typedef {Object} NetworkSearchIndex
 * @property {string} network_bref
 * @property {number} doc_count
 * @property {string} stemmed
 * @property {Object[]} docs
 * @property {Object} index
 */
