#!/usr/bin/env node
/**
 * Node.js test for NavTree structure in WASM output
 *
 * ## What This Tests
 *
 * Regression test for the visited-set interleaving bug in `get_nav_tree()`:
 *
 *   The pre-pass that discovers subnet BIDs shared a `visited: BTreeSet<Bid>`
 *   with the build pass.  `recursive_map` inserts `self.net` into `visited` on
 *   entry, so every subnet enumerated in the pre-pass was already marked visited
 *   when the build pass reached it.  `recursive_map` then returned an empty list
 *   for that subnet, causing its documents to vanish from the tree or be stranded
 *   as direct children of the root network.
 *
 * The minimal corpus structure needed to trigger this is:
 *
 *   root (network_1)
 *   ├── subnet1/          ← marked visited in pre-pass
 *   │   ├── subnet1_file1.md
 *   │   └── subnet1a/     ← nested subnet, also marked visited in pre-pass
 *   │       └── subnet1a_doc.md
 *   └── subnet2/          ← peer of subnet1; second to be marked visited
 *       └── subnet2_doc.md
 *
 * Two peer subnets (subnet1 + subnet2) are required: with only one subnet the
 * pre-pass poisons it before the build pass runs, but the effect is still visible
 * as missing/misplaced nodes.  With two peers the interleaving is unambiguous —
 * any node that ends up as a direct root child when it should be under a subnet
 * is a clear regression.
 *
 * ## Assertions
 *
 * Tree structure (primary — these are the get_nav_tree regression assertions):
 *   - Exactly one root BID (the root network).
 *   - subnet1, subnet2, subnet1a do NOT appear in tree.roots.
 *   - subnet1 and subnet2 are direct children of the root node.
 *   - subnet1a is a direct child of subnet1, NOT of root or subnet2.
 *   - subnet1_file1 and subnet2_doc are reachable only through their correct
 *     parent subnet — never as direct children of root.
 *
 * Known limitation (subnet1a_doc — separate compiler-level bug):
 *   - subnet1a_doc.md lives in subnet1a/ but the compiler currently assigns it
 *     a Section relation to the root network rather than to subnet1a.  This is a
 *     separate bug (likely the AnchorPath::new_dir strip_prefix issue described
 *     in AGENTS.md §"Network node dual-path representation").  The assertions
 *     for subnet1a_doc placement are marked with TODO and are currently skipped
 *     so that CI stays green while the primary get_nav_tree regression is caught.
 *   TODO: once the compiler assigns subnet1a_doc to subnet1a correctly, remove
 *   the KNOWN_LIMITATION guard and enable the full placement assertions.
 *
 * Structural integrity (applicable to any corpus):
 *   - Every BID referenced in children[] or parent exists in nodes.
 *   - No cycles in any parent chain.
 *   - Every non-root node's parent chain terminates at a root BID.
 *   - All node paths use .html extension (or are empty).
 *
 * ## CI Integration
 *
 * Runs in the `wasm-interface` GitHub Actions job alongside test_related_nodes.js.
 * Requires:
 *   - WASM module built:  cargo build --features bin
 *   - Test data generated: noet parse tests/network_1 --html-output tests/browser/test-output
 */

import { readFile } from "fs/promises";
import { fileURLToPath } from "url";
import { dirname, join } from "path";

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const projectRoot = join(__dirname, "../..");

// ── ANSI colour codes ────────────────────────────────────────────────────────
const GREEN = "\x1b[32m";
const RED = "\x1b[31m";
const BLUE = "\x1b[34m";
const YELLOW = "\x1b[33m";
const RESET = "\x1b[0m";

// ── Test accounting ──────────────────────────────────────────────────────────
let testsPassed = 0;
let testsFailed = 0;

function log(message, type = "info") {
  const prefix =
    {
      pass: `${GREEN}✓${RESET}`,
      fail: `${RED}✗${RESET}`,
      info: `${BLUE}ℹ${RESET}`,
      warn: `${YELLOW}⚠${RESET}`,
    }[type] ?? "";
  console.log(`${prefix} ${message}`);
}

/**
 * Record a pass/fail assertion.  On failure, logs the message and throws so
 * the test function aborts immediately (same behaviour as test_related_nodes.js).
 */
function assert(condition, message) {
  if (condition) {
    testsPassed++;
    log(message, "pass");
  } else {
    testsFailed++;
    log(message, "fail");
    throw new Error(`Assertion failed: ${message}`);
  }
}

/**
 * Soft assert — records pass/fail but does NOT throw.
 * Use for checks where we want to report all failures in a loop rather than
 * aborting on the first one.
 */
function check(condition, message) {
  if (condition) {
    testsPassed++;
    log(message, "pass");
  } else {
    testsFailed++;
    log(message, "fail");
  }
  return condition;
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/**
 * Build a lookup function and key-list from the NavTree nodes value, which
 * may be a Map (wasm-bindgen ≥ 0.2.90) or a plain object (older bindgen).
 */
function makeNodeAccessors(treeNodes) {
  if (treeNodes instanceof Map) {
    return {
      getNode: (bid) => treeNodes.get(bid),
      allBids: () => Array.from(treeNodes.keys()),
      hasNode: (bid) => treeNodes.has(bid),
    };
  }
  return {
    getNode: (bid) => treeNodes[bid],
    allBids: () => Object.keys(treeNodes),
    hasNode: (bid) => Object.prototype.hasOwnProperty.call(treeNodes, bid),
  };
}

/**
 * Walk the parent chain of `startBid` upward, returning the ordered list of
 * BIDs from `startBid` to the root (inclusive).  Detects cycles by tracking
 * visited BIDs; returns null if a cycle is found.
 */
function parentChain(startBid, getNode) {
  const chain = [];
  const seen = new Set();
  let cur = startBid;
  while (cur) {
    if (seen.has(cur)) return null; // cycle
    seen.add(cur);
    chain.push(cur);
    cur = getNode(cur)?.parent ?? null;
  }
  return chain;
}

/**
 * Find all BIDs whose node title contains `titleSubstring` (case-insensitive).
 */
function findByTitle(titleSubstring, allBids, getNode) {
  const lower = titleSubstring.toLowerCase();
  return allBids().filter((bid) => {
    const title = getNode(bid)?.title ?? "";
    return title.toLowerCase().includes(lower);
  });
}

/**
 * Return the set of BIDs that are direct children of `parentBid`.
 */
function directChildBids(parentBid, getNode) {
  return new Set(getNode(parentBid)?.children ?? []);
}

/**
 * Return the set of all descendant BIDs of `ancestorBid` (not including itself),
 * traversing children recursively.  Guards against cycles with a visited set.
 */
function allDescendants(ancestorBid, getNode) {
  const result = new Set();
  const queue = [...(getNode(ancestorBid)?.children ?? [])];
  while (queue.length > 0) {
    const bid = queue.shift();
    if (result.has(bid)) continue;
    result.add(bid);
    queue.push(...(getNode(bid)?.children ?? []));
  }
  return result;
}

// ── Main test suite ──────────────────────────────────────────────────────────

async function runTests() {
  console.log(`${BLUE}=== Testing NavTree Structure ===${RESET}\n`);

  // ── 1. Load WASM ────────────────────────────────────────────────────────
  console.log(`${BLUE}[1/5] Loading WASM module${RESET}`);

  const wasmModule = await import(join(projectRoot, "target/wasm-build/pkg/noet_core.js"));
  const wasmBuffer = await readFile(join(projectRoot, "target/wasm-build/pkg/noet_core_bg.wasm"));
  await wasmModule.default(wasmBuffer);
  assert(true, "WASM module loaded");

  // ── 2. Load beliefbase.json ─────────────────────────────────────────────
  console.log(`\n${BLUE}[2/5] Loading beliefbase.json${RESET}`);

  const beliefbaseJson = await readFile(join(__dirname, "test-output/beliefbase.json"), "utf-8");
  assert(beliefbaseJson.length > 0, "beliefbase.json is non-empty");

  const beliefbaseData = JSON.parse(beliefbaseJson);
  assert(
    beliefbaseData.states && Object.keys(beliefbaseData.states).length > 0,
    "beliefbase.json has at least one node in states",
  );

  // Find the root network node (has kind containing "Network" and no parent
  // network in the corpus — identified as the shallowest Network node).
  const networkNodes = Object.values(beliefbaseData.states).filter(
    (node) => node.kind && node.kind.includes("Network"),
  );
  assert(networkNodes.length > 0, `Found ${networkNodes.length} Network node(s) in beliefbase`);

  // Use the Network node with the shortest id as the entry point (heuristic
  // matching how the test runner identifies the corpus root).
  const rootNetworkNode = networkNodes[0];
  const entryBid = rootNetworkNode.bid;
  assert(typeof entryBid === "string" && entryBid.length > 0, `Entry BID identified: ${entryBid}`);

  // ── 3. Initialise BeliefBase and call get_nav_tree ──────────────────────
  console.log(`\n${BLUE}[3/5] Initialising BeliefBase${RESET}`);

  const bb = new wasmModule.BeliefBaseWasm(beliefbaseJson, entryBid);
  assert(bb !== null, "BeliefBaseWasm constructed");

  const tree = bb.get_nav_tree();
  assert(tree !== null && tree !== undefined, "get_nav_tree() returned a value");
  assert(Array.isArray(tree.roots), "NavTree.roots is an Array");
  assert(tree.roots.length > 0, `NavTree.roots is non-empty (${tree.roots.length} root(s))`);
  assert(tree.nodes !== null && tree.nodes !== undefined, "NavTree.nodes exists");

  const { getNode, allBids, hasNode } = makeNodeAccessors(tree.nodes);
  const totalNodes = allBids().length;
  assert(totalNodes > 0, `NavTree.nodes contains ${totalNodes} node(s)`);

  log(`Roots: ${tree.roots.length}, Total nodes: ${totalNodes}`, "info");

  // ── 4. Structural integrity checks (corpus-independent) ─────────────────
  console.log(`\n${BLUE}[4/5] Structural integrity checks${RESET}`);

  // 4a. All root BIDs resolve to nodes.
  let rootsResolvable = true;
  for (const rootBid of tree.roots) {
    if (!hasNode(rootBid)) {
      log(`  Root BID ${rootBid} not found in nodes`, "warn");
      rootsResolvable = false;
    }
  }
  check(rootsResolvable, "All root BIDs resolve to nodes in the map");

  // 4b. All parent and children cross-references resolve.
  let refsOk = true;
  for (const bid of allBids()) {
    const node = getNode(bid);
    if (!node) continue;
    for (const childBid of node.children ?? []) {
      if (!hasNode(childBid)) {
        log(`  Node ${bid} ("${node.title}") has unresolvable child ${childBid}`, "warn");
        refsOk = false;
      }
    }
    if (node.parent !== null && node.parent !== undefined && !hasNode(node.parent)) {
      log(`  Node ${bid} ("${node.title}") has unresolvable parent ${node.parent}`, "warn");
      refsOk = false;
    }
  }
  check(refsOk, "All parent/child BID cross-references resolve");

  // 4c. No cycles in any parent chain.
  let noCycles = true;
  for (const bid of allBids()) {
    const chain = parentChain(bid, getNode);
    if (chain === null) {
      log(`  Cycle detected in parent chain starting at ${bid} ("${getNode(bid)?.title}")`, "warn");
      noCycles = false;
    }
  }
  check(noCycles, "No cycles detected in any parent chain");

  // 4d. Every non-root node's parent chain terminates at a root BID.
  const rootSet = new Set(tree.roots);
  let allChainsTerminate = true;
  for (const bid of allBids()) {
    if (rootSet.has(bid)) continue;
    const chain = parentChain(bid, getNode);
    if (chain === null) continue; // already reported as cycle above
    const terminus = chain[chain.length - 1];
    if (!rootSet.has(terminus)) {
      log(
        `  Node ${bid} ("${getNode(bid)?.title}") parent chain ends at ${terminus}, which is not a root`,
        "warn",
      );
      allChainsTerminate = false;
    }
  }
  check(allChainsTerminate, "Every non-root node's parent chain terminates at a root BID");

  // 4e. All node paths use .html extension (or are empty).
  let pathsOk = true;
  for (const bid of allBids()) {
    const node = getNode(bid);
    const p = node?.path ?? "";
    if (p.length > 0 && !p.endsWith(".html") && !p.includes(".html#")) {
      log(`  Node ${bid} ("${node?.title}") has unexpected path: "${p}"`, "warn");
      pathsOk = false;
    }
  }
  check(pathsOk, "All NavNode paths use .html extension (or are empty)");

  // 4f. Children lists contain no duplicate BIDs.
  let noDuplicateChildren = true;
  for (const bid of allBids()) {
    const children = getNode(bid)?.children ?? [];
    const childSet = new Set(children);
    if (childSet.size !== children.length) {
      log(`  Node ${bid} ("${getNode(bid)?.title}") has duplicate entries in children[]`, "warn");
      noDuplicateChildren = false;
    }
  }
  check(noDuplicateChildren, "No node has duplicate BIDs in its children list");

  // 4g. Parent/child relationship is symmetric: if A.children contains B then B.parent == A.
  let parentChildSymmetric = true;
  for (const bid of allBids()) {
    for (const childBid of getNode(bid)?.children ?? []) {
      const childNode = getNode(childBid);
      if (!childNode) continue; // already caught by 4b
      if (childNode.parent !== bid) {
        log(
          `  Asymmetric edge: ${bid} ("${getNode(bid)?.title}") lists child ${childBid} ("${childNode.title}") but child.parent = ${childNode.parent}`,
          "warn",
        );
        parentChildSymmetric = false;
      }
    }
  }
  check(
    parentChildSymmetric,
    "Parent/child relationship is symmetric (if A.children has B then B.parent == A)",
  );

  // ── 5. Subnet placement checks (network_1-specific) ──────────────────────
  console.log(`\n${BLUE}[5/5] Subnet placement checks (network_1 corpus)${RESET}`);
  console.log(
    `${BLUE}  Required fixture structure:${RESET}
    root
    ├── net1_dir1/                    ← non-network directory
    │   └── net1_dir1_subnet/         ← subnet inside non-network dir
    │       └── net1_dir1_subnet_doc.md
    ├── subnet1/
    │   ├── subnet1_file1.md
    │   └── subnet1a/
    │       └── subnet1a_doc.md
    └── subnet2/
        └── subnet2_doc.md`,
  );

  // Locate nodes by title substring.  We use substrings rather than hard-coded
  // BIDs because BIDs are time-based for unpersisted nodes and change each run.
  const subnet1Bids = findByTitle("small subnet for beliefbase", allBids, getNode);
  const subnet2Bids = findByTitle("peer subnet 2", allBids, getNode);
  const subnet1aBids = findByTitle("nested sub-network", allBids, getNode);
  const subnet1FileBids = findByTitle("subnet1 file1", allBids, getNode);
  const subnet1aDocBids = findByTitle("subnet 1a document", allBids, getNode);
  const subnet2DocBids = findByTitle("subnet 2 document", allBids, getNode);
  // net1_dir1_subnet: subnet inside a non-network intermediate directory.
  // Its PathMap key in root is "net1_dir1/net1_dir1_subnet" — a multi-component
  // string — which is the case the stack-reconstruction fix must handle.
  const net1Dir1SubnetBids = findByTitle("subnet inside a non-network directory", allBids, getNode);
  const net1Dir1SubnetDocBids = findByTitle("net1 dir1 subnet document", allBids, getNode);

  log(
    `Located by title: subnet1=${subnet1Bids.length}, subnet2=${subnet2Bids.length}, ` +
      `subnet1a=${subnet1aBids.length}, subnet1_file1=${subnet1FileBids.length}, ` +
      `subnet1a_doc=${subnet1aDocBids.length}, subnet2_doc=${subnet2DocBids.length}, ` +
      `net1_dir1_subnet=${net1Dir1SubnetBids.length}, ` +
      `net1_dir1_subnet_doc=${net1Dir1SubnetDocBids.length}`,
    "info",
  );

  // These nodes must all be present — if any are missing the fixture was not
  // compiled correctly and subsequent assertions are meaningless.
  assert(
    subnet1Bids.length === 1,
    `Exactly one subnet1 node found (got ${subnet1Bids.length}; check fixture id/title)`,
  );
  assert(
    subnet2Bids.length === 1,
    `Exactly one subnet2 node found (got ${subnet2Bids.length}; check fixture id/title)`,
  );
  assert(
    subnet1aBids.length === 1,
    `Exactly one subnet1a node found (got ${subnet1aBids.length}; check fixture id/title)`,
  );
  assert(subnet1FileBids.length >= 1, `subnet1_file1 node found (got ${subnet1FileBids.length})`);
  assert(subnet1aDocBids.length >= 1, `subnet1a_doc node found (got ${subnet1aDocBids.length})`);
  assert(subnet2DocBids.length >= 1, `subnet2_doc node found (got ${subnet2DocBids.length})`);

  assert(
    net1Dir1SubnetBids.length === 1,
    `Exactly one net1_dir1_subnet node found (got ${net1Dir1SubnetBids.length}; check fixture id/title)`,
  );
  assert(
    net1Dir1SubnetDocBids.length >= 1,
    `net1_dir1_subnet_doc node found (got ${net1Dir1SubnetDocBids.length})`,
  );

  const subnet1Bid = subnet1Bids[0];
  const subnet2Bid = subnet2Bids[0];
  const subnet1aBid = subnet1aBids[0];
  const subnet1FileBid = subnet1FileBids[0];
  const subnet1aDocBid = subnet1aDocBids[0];
  const subnet2DocBid = subnet2DocBids[0];
  const net1Dir1SubnetBid = net1Dir1SubnetBids[0];
  const net1Dir1SubnetDocBid = net1Dir1SubnetDocBids[0];

  // 5a. Subnets must NOT appear in roots (they are not top-level networks).
  check(
    !rootSet.has(subnet1Bid),
    `subnet1 does NOT appear in NavTree.roots (was bug: interleaved at root level)`,
  );
  check(
    !rootSet.has(subnet2Bid),
    `subnet2 does NOT appear in NavTree.roots (was bug: interleaved at root level)`,
  );
  check(
    !rootSet.has(subnet1aBid),
    `subnet1a does NOT appear in NavTree.roots (was bug: interleaved at root level)`,
  );

  // 5b. The root network node's direct children must include subnet1 and subnet2.
  // (There must be exactly one root for network_1.)
  assert(
    tree.roots.length === 1,
    `NavTree has exactly one root for network_1 corpus (got ${tree.roots.length})`,
  );
  const rootBid = tree.roots[0];
  const rootDirectChildren = directChildBids(rootBid, getNode);
  // net1_dir1_subnet must NOT appear in roots.
  check(!rootSet.has(net1Dir1SubnetBid), `net1_dir1_subnet does NOT appear in NavTree.roots`);

  log(
    `Root ("${getNode(rootBid)?.title}") has ${rootDirectChildren.size} direct child(ren)`,
    "info",
  );
  check(rootDirectChildren.has(subnet1Bid), `subnet1 is a direct child of the root network node`);
  check(rootDirectChildren.has(subnet2Bid), `subnet2 is a direct child of the root network node`);

  // 5b continued: net1_dir1_subnet must be a direct child of root.
  // Its PathMap key in root is "net1_dir1/net1_dir1_subnet" — verifies that
  // the stack reconstruction correctly handles a multi-component path key.
  check(
    rootDirectChildren.has(net1Dir1SubnetBid),
    `net1_dir1_subnet is a direct child of the root network node`,
  );

  // 5c. subnet1a must be a direct child of subnet1, NOT of root or subnet2.
  const subnet1DirectChildren = directChildBids(subnet1Bid, getNode);
  check(subnet1DirectChildren.has(subnet1aBid), `subnet1a is a direct child of subnet1`);
  check(
    !rootDirectChildren.has(subnet1aBid),
    `subnet1a is NOT a direct child of root (was bug: misplaced by visited-set interleaving)`,
  );
  check(
    !directChildBids(subnet2Bid, getNode).has(subnet1aBid),
    `subnet1a is NOT a direct child of subnet2`,
  );

  // 5d. Per-subnet document placement — each doc must be a descendant of its
  // own subnet and NOT a direct child of root.
  const rootDescendants = allDescendants(rootBid, getNode);
  const subnet1Descendants = allDescendants(subnet1Bid, getNode);
  const subnet2Descendants = allDescendants(subnet2Bid, getNode);
  const subnet1aDescendants = allDescendants(subnet1aBid, getNode);
  const net1Dir1SubnetDescendants = allDescendants(net1Dir1SubnetBid, getNode);

  // subnet1_file1 must be under subnet1.
  check(subnet1Descendants.has(subnet1FileBid), `subnet1_file1 is a descendant of subnet1`);
  check(!rootDirectChildren.has(subnet1FileBid), `subnet1_file1 is NOT a direct child of root`);

  // subnet2_doc must be under subnet2 and not under subnet1.
  check(subnet2Descendants.has(subnet2DocBid), `subnet2_doc is a descendant of subnet2`);
  check(
    !rootDirectChildren.has(subnet2DocBid),
    `subnet2_doc is NOT a direct child of root (was bug: dropped to root by visited-set interleaving)`,
  );
  check(!subnet1Descendants.has(subnet2DocBid), `subnet2_doc is NOT under subnet1`);

  // net1_dir1_subnet_doc must be under net1_dir1_subnet and not a direct child of root.
  // This is the primary regression test for the non-network intermediate directory case:
  // if stack reconstruction fails, the doc gets a Section relation to root with path
  // "net1_dir1/net1_dir1_subnet/net1_dir1_subnet_doc.md" instead of being correctly
  // owned by net1_dir1_subnet.
  check(
    net1Dir1SubnetDescendants.has(net1Dir1SubnetDocBid),
    `net1_dir1_subnet_doc is a descendant of net1_dir1_subnet`,
  );
  check(
    rootDescendants.has(net1Dir1SubnetDocBid),
    `net1_dir1_subnet_doc is a descendant of root (transitively)`,
  );
  check(
    !rootDirectChildren.has(net1Dir1SubnetDocBid),
    `net1_dir1_subnet_doc is NOT a direct child of root`,
  );

  // 5e. Parent field consistency for directly-verifiable key nodes.
  check(getNode(net1Dir1SubnetBid)?.parent === rootBid, `net1_dir1_subnet.parent === rootBid`);
  check(
    getNode(net1Dir1SubnetDocBid)?.parent === net1Dir1SubnetBid,
    `net1_dir1_subnet_doc.parent === net1Dir1SubnetBid`,
  );
  check(getNode(subnet1Bid)?.parent === rootBid, `subnet1.parent === rootBid`);
  check(getNode(subnet2Bid)?.parent === rootBid, `subnet2.parent === rootBid`);
  check(getNode(subnet1aBid)?.parent === subnet1Bid, `subnet1a.parent === subnet1Bid`);
  check(getNode(subnet2DocBid)?.parent === subnet2Bid, `subnet2_doc.parent === subnet2Bid`);

  // 5f (formerly known limitation). subnet1a_doc placement — compiler bug now fixed.
  //
  // subnet1a_doc.md was previously mis-assigned a Section relation to the root network
  // instead of subnet1a, due to try_initialize_stack_from_session_cache calling
  // order_for_bid on root's PathMap when parent_bid was a deeply-nested subnet (subnet1a
  // lives in subnet1's PathMap, not root's).  Fixed by using PathMapMap::indexed_path
  // which searches across all PathMaps and returns the combined order vector.
  check(subnet1aDescendants.has(subnet1aDocBid), `subnet1a_doc is a descendant of subnet1a`);
  check(
    subnet1Descendants.has(subnet1aDocBid),
    `subnet1a_doc is a descendant of subnet1 (transitively)`,
  );
  check(!rootDirectChildren.has(subnet1aDocBid), `subnet1a_doc is NOT a direct child of root`);
  check(getNode(subnet1aDocBid)?.parent === subnet1aBid, `subnet1a_doc.parent === subnet1aBid`);

  // 5f. Total reachability: every non-root node in the tree must be reachable
  // from the root via the children graph.  Unreachable nodes indicate the old
  // bug where nodes were inserted into root_nodes_map but not wired into the tree.
  const reachable = new Set([rootBid, ...rootDescendants]);
  const allBidList = allBids();
  let allReachable = true;
  const unreachable = [];
  for (const bid of allBidList) {
    if (!reachable.has(bid)) {
      unreachable.push(`${bid} ("${getNode(bid)?.title}")`);
      allReachable = false;
    }
  }
  if (!allReachable) {
    log(`  Unreachable nodes: ${unreachable.join(", ")}`, "warn");
  }
  check(
    allReachable,
    `All ${allBidList.length} nodes are reachable from the root via children links`,
  );

  // ── Summary ──────────────────────────────────────────────────────────────
  console.log(`\n${BLUE}=== Test Summary ===${RESET}`);
  console.log(`${GREEN}Passed: ${testsPassed}${RESET}`);
  if (testsFailed > 0) {
    console.log(`${RED}Failed: ${testsFailed}${RESET}`);
  }

  if (testsFailed === 0) {
    console.log(`\n${GREEN}✓ All tests passed!${RESET}`);
    console.log(`${GREEN}✓ Subnet nodes are correctly nested in the nav tree${RESET}`);
    console.log(`${GREEN}✓ No subnet BIDs interleaved at the root level${RESET}`);
    console.log(`${GREEN}✓ Parent/child chains are consistent and acyclic${RESET}`);
    process.exit(0);
  } else {
    console.log(`\n${RED}✗ ${testsFailed} test(s) failed${RESET}`);
    process.exit(1);
  }
}

runTests().catch((err) => {
  console.error(`\n${RED}✗ Fatal error: ${err.message}${RESET}`);
  console.error(err.stack);
  process.exit(1);
});
