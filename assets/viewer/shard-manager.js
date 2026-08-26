/**
 * viewer/shard-manager.js — ShardManager: memory-budgeted BeliefBase shard loading
 *
 * Manages loading and unloading of per-network BeliefBase shards under a
 * browser memory budget. Search indices are loaded lazily via
 * `loadSearchIndices()` (triggered on first search-input focus) to keep
 * ~20 MB of index data off the critical render path.
 *
 * ## Initialization (sharded mode)
 *
 *   const manager = new ShardManager(beliefbase, manifest);
 *   await manager.init();                  // loads global shard + target/entry network
 *   // Entry network data is loaded and queryable — nav tree can render
 *   await manager.loadSearchIndices();     // loads all search indices (call lazily)
 *   // Full-corpus search is now available via beliefbase.search(query, limit)
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
 *   - Loaded data shards tracked internally by `BeliefBaseWasm.loaded_shards()`
 *   - Search indices (loaded lazily via `loadSearchIndices()`, not during init)
 *
 * Memory accounting uses `BeliefBaseWasm.memory_usage_mb()` for the data side
 * and tracks search index sizes separately.
 */
export class ShardManager {
  /**
   * @param {import('./wasm.js').BeliefBaseWasm} beliefbase
   *   The `BeliefBaseWasm` instance created via `BeliefBaseWasm.from_manifest`.
   * @param {ShardManifest} manifest
   *   Parsed contents of `beliefbase/manifest.json`.
   */
  constructor(beliefbase, manifest, assetVersion = "", baseUrl = "") {
    /** @type {import('./wasm.js').BeliefBaseWasm} */
    this.beliefbase = beliefbase;

    /** @type {ShardManifest} */
    this.manifest = manifest;

    /**
     * Asset version token — appended as `?v=` to all fetch URLs to bust the
     * HTTP cache and browser module-specifier cache between deployments.
     * @type {string}
     */
    this._assetVersion = assetVersion;

    /**
     * Base URL for all asset fetches (e.g. "https://example.github.io/repo").
     * Empty string means root-relative (default local serve).
     * Never has a trailing slash.
     * @type {string}
     */
    this._baseUrl = baseUrl.replace(/\/$/, "");

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

    /**
     * Whether `loadSearchIndices()` has been called (guards against double-load).
     * @type {boolean}
     */
    this._searchIndicesLoaded = false;

    /**
     * In-flight load promises, keyed by bref. Prevents duplicate concurrent
     * fetches when multiple callers request the same network before the
     * first load completes.
     * @type {Map<string, Promise<number>>}
     */
    this._pendingLoads = new Map();
  }

  // ===========================================================================
  // Initialization
  // ===========================================================================

  /**
   * Initialize the shard manager:
   *
   * 1. Load the global shard (required for cross-network link resolution).
   * 2. Resolve the URL hash to a target network and load its shard.
   *    Falls back to the entry-point network when no target is identified.
   *
   * Search indices are NOT loaded here — call `loadSearchIndices()` lazily
   * (e.g. on first search-input focus) to keep ~20 MB of index fetches off
   * the critical render path.
   *
   * @returns {Promise<void>}
   * @throws {Error} if the global shard cannot be fetched
   */
  async init() {
    console.log("[ShardManager] Initializing...");

    // Step 1: Load the global shard — required for cross-network link resolution
    // and for resolving the URL hash to a network bref.
    await this._loadGlobalShard();

    // Step 2: Determine which network shard to load.
    // After the global shard is loaded, the bref_index is available — we can
    // resolve the URL hash path to a BID and find its network.  Loading the
    // target network first means the user sees their destination content
    // without waiting for the root network shard.
    const entryPoint = this.beliefbase.entryPoint();
    const entryBref = entryPoint.bref;
    let targetBref = null;

    const hash = window.location.hash.substring(1);
    if (hash) {
      // get_paths returns the entry-point PathMap (repo-root-relative paths).
      // Path resolution uses the compiled PathMap which was loaded with the
      // global shard, so it works even before any network shard is loaded.
      try {
        const paths = this.beliefbase.get_paths();
        const entryPaths = paths[entryPoint.bid];
        if (entryPaths) {
          // Strip anchor from hash and normalize (remove leading /, .html ext)
          const cleanHash = hash.replace(/^\//, "").replace(/#.*$/, "");
          const bid = entryPaths[cleanHash];
          if (bid) {
            const netBref = this.beliefbase.network_bref_for_bid(bid);
            if (netBref && netBref !== entryBref) {
              targetBref = netBref;
              console.log(
                `[ShardManager] URL hash '${hash}' resolved to network '${targetBref}'`,
              );
            }
          }
        }
      } catch (e) {
        console.warn(`[ShardManager] Failed to resolve URL hash to network: ${e}`);
      }
    }

    const primaryBref = targetBref || entryBref;
    const primaryMeta = this.manifest.networks.find((n) => n.bref === primaryBref);

    if (!primaryMeta) {
      if (targetBref) {
        // Target not found — fall back to entry network.
        console.warn(
          `[ShardManager] Target network '${targetBref}' not in manifest. ` +
            `Falling back to entry network '${entryBref}'.`,
        );
        const entryMeta = this.manifest.networks.find((n) => n.bref === entryBref);
        if (entryMeta) {
          await this.loadNetwork(entryBref);
        }
      } else {
        console.warn(
          `[ShardManager] Entry network bref '${entryBref}' not found in manifest. ` +
            "The viewer may have limited data available.",
        );
      }
      console.log(
        `[ShardManager] Init complete. Node count: ${this.beliefbase.node_count()}`,
      );
      return;
    }

    await this.loadNetwork(primaryBref);

    // If we loaded a non-entry network, also load the entry network so the
    // nav tree has the root structure.  Fire-and-forget — don't block first paint.
    if (primaryBref !== entryBref) {
      const entryMeta = this.manifest.networks.find((n) => n.bref === entryBref);
      if (entryMeta && !this.isNetworkLoaded(entryBref)) {
        document.dispatchEvent(
          new CustomEvent("noet:shard-loading", {
            detail: { bref: entryBref, title: entryMeta.title },
          }),
        );
        this.loadNetwork(entryBref)
          .then(() => {
            console.log(
              `[ShardManager] Background: entry network '${entryBref}' loaded.`,
            );
            document.dispatchEvent(
              new CustomEvent("noet:shard-loaded", { detail: { bref: entryBref } }),
            );
          })
          .catch((err) => {
            console.warn(
              `[ShardManager] Background entry network load failed: ${err.message}`,
            );
            document.dispatchEvent(
              new CustomEvent("noet:shard-load-failed", {
                detail: { bref: entryBref, title: entryMeta.title },
              }),
            );
          });
      }
    }

    console.log(
      `[ShardManager] Init complete. Loaded global + network '${primaryBref}'. ` +
        `Node count: ${this.beliefbase.node_count()}`,
    );
  }

  /**
   * Load all search indices into WASM.  Call this lazily — e.g. when the user
   * first focuses the search input — rather than on init.
   *
   * Safe to call multiple times; subsequent calls are no-ops if indices are
   * already loaded.
   *
   * @returns {Promise<void>}
   */
  async loadSearchIndices() {
    if (this._searchIndicesLoaded) return;
    this._searchIndicesLoaded = true;
    await this._loadAllSearchIndices();
  }

  // ===========================================================================
  // Search index loading
  // ===========================================================================

  /**
   * Fetch `search/manifest.json` and all `.idx.msgpack` files listed there.
   *
   * Each msgpack binary is passed directly into WASM via
   * `beliefbase.load_search_index(bref, bytes)`. No JS-side parsing occurs.
   * Failures for individual index files are logged but do not abort init —
   * the user simply gets reduced search coverage for that network.
   *
   * @returns {Promise<void>}
   */
  async _loadAllSearchIndices() {
    console.log("[ShardManager] Loading search manifest...");

    let searchManifest;
    try {
      const resp = await fetch(
        `${this._baseUrl}/search/manifest.json?v=${this._assetVersion}`,
      );
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
    const totalSizeKB = networks.reduce((sum, n) => sum + (n.size_kb ?? 0), 0);
    console.log(
      `[ShardManager] Fetching ${networks.length} search index file(s), ` +
        `total ~${(totalSizeKB / 1024).toFixed(1)} MB...`,
    );

    // Fetch all indices in parallel and load into WASM.
    const fetches = networks.map(async (meta) => {
      try {
        const resp = await fetch(
          `${this._baseUrl}/search/${meta.path}?v=${this._assetVersion}`,
        );
        if (!resp.ok) {
          console.warn(
            `[ShardManager] Failed to fetch search index '${meta.path}': ${resp.status}`,
          );
          return;
        }
        const buffer = await resp.arrayBuffer();
        const docCount = this.beliefbase.load_search_index(
          meta.bref,
          new Uint8Array(buffer),
        );
        console.log(
          `[ShardManager] Search index '${meta.bref}' loaded: ${docCount} docs`,
        );
      } catch (err) {
        console.warn(
          `[ShardManager] Error loading search index for '${meta.bref}': ${err}`,
        );
      }
    });

    await Promise.all(fetches);
    const loadedCount = this.beliefbase.loaded_search_indices().size;
    console.log(
      `[ShardManager] Search indices loaded: ${loadedCount} / ${networks.length} networks`,
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
    const resp = await fetch(
      `${this._baseUrl}/beliefbase/global.msgpack?v=${this._assetVersion}`,
    );
    if (!resp.ok) {
      throw new Error(`[ShardManager] Failed to fetch global shard: ${resp.status}`);
    }
    const buffer = await resp.arrayBuffer();
    console.log(
      `[ShardManager] Global shard fetched: ${(buffer.byteLength / 1024).toFixed(1)} KB`,
    );
    const nodeCount = this.beliefbase.load_shard("global", new Uint8Array(buffer));
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
    // Deduplicate concurrent loads for the same bref.
    if (this._pendingLoads.has(bref)) {
      return this._pendingLoads.get(bref);
    }
    const promise = this._doLoadNetwork(bref);
    this._pendingLoads.set(bref, promise);
    promise.finally(() => this._pendingLoads.delete(bref));
    return promise;
  }

  async _doLoadNetwork(bref) {
    const meta = this.manifest.networks.find((n) => n.bref === bref);
    if (!meta) {
      throw new Error(`[ShardManager] loadNetwork: bref '${bref}' not in manifest`);
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

    console.log(
      `[ShardManager] Loading network shard '${bref}' ("${meta.title}", ` +
        `~${meta.estimated_size_mb.toFixed(1)} MB)...`,
    );
    const resp = await fetch(
      `${this._baseUrl}/beliefbase/networks/${bref}.msgpack?v=${this._assetVersion}`,
    );
    if (!resp.ok) {
      throw new Error(
        `[ShardManager] Failed to fetch network shard '${bref}': ${resp.status}`,
      );
    }
    const buffer = await resp.arrayBuffer();
    console.log(
      `[ShardManager] Network shard '${bref}' fetched: ${(buffer.byteLength / 1024 / 1024).toFixed(2)} MB`,
    );
    const nodeCount = this.beliefbase.load_shard(bref, new Uint8Array(buffer));
    this._loadedNetworks.add(bref);
    this._notifyListeners({ type: "loaded", bref, nodeCount });
    console.log(`[ShardManager] Network '${bref}' loaded. Total nodes: ${nodeCount}`);
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
   * Returns the sum of `estimated_size_mb` across all network shards and the
   * global shard.  This is the total footprint if everything were loaded.
   *
   * @returns {number}
   */
  getTotalAvailableMb() {
    const networkTotal = this.manifest.networks.reduce(
      (sum, n) => sum + (n.estimated_size_mb ?? 0),
      0,
    );
    const globalSize = this.manifest.global?.estimated_size_mb ?? 0;
    return networkTotal + globalSize;
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

  /**
   * Returns the network metadata entry for a full BID string, or null if not found.
   * Used to detect whether a given node BID is a sharded network root.
   *
   * @param {string} bid — full UUID string (e.g. "1f132825-49b0-611c-8f4b-6ace77cb4a7d")
   * @returns {NetworkShardMeta|null}
   */
  getNetworkMetaByBid(bid) {
    return this.manifest.networks.find((n) => n.bid === bid) ?? null;
  }

  /**
   * Find the network shard that contains a given BID.
   *
   * Strategy (in order):
   *   1. Exact match — bid IS a network root BID.
   *   2. bref_index lookup — extract the node's bref from the BID and query
   *      the WASM `network_bref_for_bref` method, which consults the
   *      bref→network_bref index loaded from the global shard.
   *
   * Returns null when no manifest entry matches.
   *
   * @param {string} bid — full UUID string
   * @returns {NetworkShardMeta|null}
   */
  findNetworkForBid(bid) {
    if (!bid) return null;

    // 1. Exact match (bid is a network root).
    const exact = this.getNetworkMetaByBid(bid);
    if (exact) return exact;

    // 2. bref_index lookup via WASM (computes the node's bref internally).
    const netBref = this.beliefbase.network_bref_for_bid(bid);
    if (netBref) {
      const meta = this.manifest.networks.find((n) => n.bref === netBref);
      if (meta) return meta;
    }

    return null;
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
 * In monolithic mode the `beliefbase.msgpack` is already loaded by `initializeWasm`.
 * This function handles the search-only part: fetch `search/manifest.json` and
 * all `.idx.msgpack` files, passing each into WASM via
 * `beliefbase.load_search_index(bref, bytes)`.
 *
 * Called by `initializeWasm` when no `beliefbase/manifest.json` is detected.
 *
 * @param {import('./wasm.js').BeliefBaseWasm} beliefbase
 * @param {string} assetVersion
 * @param {string} baseUrl
 * @returns {Promise<number>} Count of successfully loaded indices
 */
export async function loadMonolithicSearchIndices(
  beliefbase,
  assetVersion = "",
  baseUrl = "",
) {
  const baseUrlNorm = baseUrl.replace(/\/$/, "");

  let manifest;
  try {
    const resp = await fetch(`${baseUrlNorm}/search/manifest.json?v=${assetVersion}`);
    if (!resp.ok) {
      console.warn(
        `[Noet] search/manifest.json not found (${resp.status}). Search unavailable.`,
      );
      return 0;
    }
    manifest = await resp.json();
  } catch (err) {
    console.warn(`[Noet] Failed to load search manifest: ${err}`);
    return 0;
  }

  const networks = manifest.networks ?? [];
  let loadedCount = 0;
  const fetches = networks.map(async (meta) => {
    try {
      const resp = await fetch(`${baseUrlNorm}/search/${meta.path}?v=${assetVersion}`);
      if (!resp.ok) {
        console.warn(`[Noet] Could not fetch '${meta.path}': ${resp.status}`);
        return;
      }
      const buffer = await resp.arrayBuffer();
      beliefbase.load_search_index(meta.bref, new Uint8Array(buffer));
      loadedCount++;
    } catch (err) {
      console.warn(`[Noet] Error fetching search index '${meta.bref}': ${err}`);
    }
  });

  await Promise.all(fetches);
  console.log(
    `[Noet] Monolithic search indices loaded: ${loadedCount} / ${networks.length}`,
  );
  return loadedCount;
}

/**
 * If `targetBid` corresponds to an unloaded network shard, begin loading it in
 * the background (fire-and-forget). Logs to console but never blocks the caller.
 *
 * Safe to call from synchronous contexts (nav toggle, link click handlers).
 * No-op in monolithic mode (state.shardManager is null) or when already loaded.
 *
 * @param {string|null} targetBid — BID of the node being navigated to
 * @param {import('./state.js').State} state — shared viewer state
 */
export function ensureNetworkLoaded(targetBid, state) {
  if (!targetBid || !state.shardManager) return;

  const meta = state.shardManager.findNetworkForBid(targetBid);
  if (!meta) return; // Cannot determine which network shard to load.

  if (state.shardManager.isNetworkLoaded(meta.bref)) return; // Already loaded.

  console.log(
    `[ShardManager] Background-loading network '${meta.title}' (${meta.bref}) ` +
      `triggered by navigation to ${targetBid}`,
  );

  // Notify UI that a background shard load has started so it can show a spinner.
  document.dispatchEvent(
    new CustomEvent("noet:shard-loading", {
      detail: { bref: meta.bref, title: meta.title },
    }),
  );

  state.shardManager
    .loadNetwork(meta.bref)
    .then(() => {
      console.log(`[ShardManager] Background load complete: '${meta.title}'`);
      // Rebuild the nav tree so child nodes become visible (they were missing
      // from BeliefBase before the shard was loaded).
      if (typeof state.navTree !== "undefined" && state.navTree) {
        // buildNavigation is not importable here (circular dep risk), so we
        // dispatch a custom event that viewer.js or navigation.js can handle.
        document.dispatchEvent(
          new CustomEvent("noet:shard-loaded", { detail: { bref: meta.bref } }),
        );
      }
    })
    .catch((err) => {
      console.warn(
        `[ShardManager] Background load failed for '${meta.title}' (${meta.bref}): ${err.message}`,
      );
      // Clear the loading indicator even on failure.
      document.dispatchEvent(
        new CustomEvent("noet:shard-load-failed", {
          detail: { bref: meta.bref, title: meta.title },
        }),
      );
    });
}

/**
 * Ensure the shard containing `targetBid` is loaded, awaiting completion.
 *
 * Unlike `ensureNetworkLoaded` (fire-and-forget), this returns a Promise that
 * resolves to `true` when the shard is ready and `false` when the shard could
 * not be identified or loaded. Callers that need the node data immediately
 * (e.g. showMetadataPanel) should await this before querying BeliefBase.
 *
 * @param {string} targetBid — BID of the node whose shard is needed
 * @param {Object} state — viewer state (needs .shardManager, .navTree)
 * @returns {Promise<boolean>}
 */
export async function ensureShardForBid(targetBid, state) {
  if (!targetBid || !state.shardManager) return false;

  const meta = state.shardManager.findNetworkForBid(targetBid);
  if (!meta) return false;

  if (state.shardManager.isNetworkLoaded(meta.bref)) return true;

  console.log(
    `[ShardManager] Loading network '${meta.title}' (${meta.bref}) for BID ${targetBid}`,
  );

  document.dispatchEvent(
    new CustomEvent("noet:shard-loading", {
      detail: { bref: meta.bref, title: meta.title },
    }),
  );

  try {
    await state.shardManager.loadNetwork(meta.bref);
    console.log(`[ShardManager] Shard load complete: '${meta.title}'`);
    document.dispatchEvent(
      new CustomEvent("noet:shard-loaded", { detail: { bref: meta.bref } }),
    );
    return true;
  } catch (err) {
    console.warn(
      `[ShardManager] Shard load failed for '${meta.title}' (${meta.bref}): ${err.message}`,
    );
    document.dispatchEvent(
      new CustomEvent("noet:shard-load-failed", {
        detail: { bref: meta.bref, title: meta.title },
      }),
    );
    return false;
  }
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
